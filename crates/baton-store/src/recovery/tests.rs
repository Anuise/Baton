//! 崩潰恢復持久化的單元測試（票 31）：正常結束標記、啟動判定、嘗試序號的唯一性。

use baton_domain::guardian::{GuardianAttempt, JournalRecord};
use baton_domain::recovery::{ImportDisposition, RecoveryCommand, RecoveryStatus};

use super::*;

const T0: Timestamp = Timestamp::from_millis(1_000);

async fn open(dir: &Path) -> Store {
    Store::open(dir, T0).await.unwrap()
}

#[tokio::test]
async fn unclean_previous_boot_is_detected_and_clean_shutdown_is_not() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path()).await;
    store.record_boot(T0).await.unwrap();
    assert_eq!(store.previous_boot_ended_cleanly().await.unwrap(), None, "之前沒有啟動過");
    store.record_boot(T0).await.unwrap();
    assert_eq!(store.previous_boot_ended_cleanly().await.unwrap(), Some(false), "第一次啟動沒有正常結束");
    store.mark_clean_shutdown(T0).await.unwrap();
    store.record_boot(T0).await.unwrap();
    assert_eq!(store.previous_boot_ended_cleanly().await.unwrap(), Some(true), "第二次啟動正常結束");
    store.record_boot(T0).await.unwrap();
    assert_eq!(store.previous_boot_ended_cleanly().await.unwrap(), Some(false), "只看前一次啟動");
}

#[tokio::test]
async fn crash_creates_event_and_attempts_persist_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path()).await;
    assert!(store.note_runtime_start(false, "rec_x".into(), T0).await.unwrap().is_none());
    let event = store.note_runtime_start(true, "rec_1".into(), T0).await.unwrap().unwrap();
    assert_eq!((event.id.as_str(), event.status), ("rec_1", RecoveryStatus::Waiting));

    let begun = store.apply_recovery(RecoveryCommand::Begin { attempt_id: "a1".into() }, T0).await.unwrap();
    assert_eq!(begun.attempts_used, 1);
    drop(store);

    // 重新開啟資料庫後讀到同樣的事件與嘗試；嘗試進行中被記為失敗並進入退避
    let store = open(dir.path()).await;
    assert_eq!(store.recovery_event().await.unwrap().unwrap(), begun);
    let after = store.note_runtime_start(true, "rec_2".into(), T0).await.unwrap().unwrap();
    assert_eq!((after.id.as_str(), after.status, after.attempts_used), ("rec_1", RecoveryStatus::Waiting, 1));
    assert_eq!(store.recovery_event().await.unwrap().unwrap(), after);
}

#[tokio::test]
async fn stop_intent_at_start_is_not_a_crash() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path()).await;
    store.request_exit(Default::default(), T0).await.unwrap();
    assert!(store.note_runtime_start(true, "rec_1".into(), T0).await.unwrap().is_none());
}

#[tokio::test]
async fn the_schema_rejects_a_fourth_attempt_in_the_same_round() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path()).await;
    store.note_runtime_start(true, "rec_1".into(), T0).await.unwrap();
    let result = store
        .write(|tx| {
            tx.execute(
                "INSERT INTO recovery_attempt (id, event_id, round, seq, started_at, outcome, affected)
                 VALUES ('a4', 'rec_1', 1, 4, 0, 'in_progress', '[]')",
                [],
            )?;
            Ok(())
        })
        .await;
    assert!(result.is_err(), "序號 4 違反 CHECK");
}

#[tokio::test]
async fn pending_recovery_halts_launches() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path()).await;
    let halted = |store: &Store| {
        let store = store.clone();
        async move { store.read(|conn| Ok(super::super::lifecycle::launch_halted_in(conn, "ws")?)).await.unwrap() }
    };
    assert!(!halted(&store).await);
    store.note_runtime_start(true, "rec_1".into(), T0).await.unwrap();
    assert!(halted(&store).await, "第一次嘗試完成前不啟動");
    store.apply_recovery(RecoveryCommand::Begin { attempt_id: "a1".into() }, T0).await.unwrap();
    store.apply_recovery(RecoveryCommand::Complete { affected: vec![] }, T0).await.unwrap();
    assert!(!halted(&store).await, "穩定觀察期間可以啟動");
}

// ------------------------------------------------------------ 監護日誌匯入（票 32）

fn guardian(seq: u32, id: &str) -> JournalRecord {
    JournalRecord::Attempt(GuardianAttempt {
        event_id: "rec_g".into(),
        round: 1,
        seq,
        attempt_id: id.into(),
        detected_at: T0,
        at: T0,
        reason: "監護確認 Runtime 意外終止".into(),
    })
}

#[tokio::test]
async fn guardian_records_are_imported_once_and_deduplicated_by_attempt_id() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path()).await;
    let journal = vec![guardian(1, "rcv_g1")];
    let imported = store.import_guardian_journal(journal.clone(), T0).await.unwrap();
    assert_eq!(imported, vec![("rcv_g1".to_string(), ImportDisposition::Counted)]);
    let event = store.recovery_event().await.unwrap().unwrap();
    assert_eq!((event.id.as_str(), event.attempts_used, event.status), ("rec_g", 1, RecoveryStatus::Attempting));

    // 再次啟動、再次讀到同一份日誌：不重複計次
    assert!(store.import_guardian_journal(journal.clone(), T0).await.unwrap().is_empty());
    assert_eq!(store.recovery_event().await.unwrap().unwrap(), event);

    // 這個 Runtime 就是為了 rcv_g1 啟動的：不記為失敗
    let kept = store.note_runtime_start_owning(true, Some("rcv_g1".into()), "rec_x".into(), T0).await.unwrap().unwrap();
    assert_eq!(kept, event);

    let mirror = store.recovery_mirror(T0).await.unwrap();
    assert_eq!(mirror.journal_cursor.as_deref(), Some("rcv_g1"));
    assert_eq!(mirror.event.unwrap().attempts_used, 1);
}

#[tokio::test]
async fn withdrawn_and_stop_intent_records_are_remembered_but_not_counted() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path()).await;
    let withdrawn = vec![
        guardian(1, "rcv_g1"),
        JournalRecord::Withdrawn { attempt_id: "rcv_g1".into(), at: T0, reason: "更新中".into() },
    ];
    let imported = store.import_guardian_journal(withdrawn, T0).await.unwrap();
    assert_eq!(imported, vec![("rcv_g1".to_string(), ImportDisposition::Withdrawn)]);
    assert!(store.recovery_event().await.unwrap().is_none());

    store.request_exit(Default::default(), T0).await.unwrap();
    let journal = vec![guardian(1, "rcv_g1"), guardian(1, "rcv_g2")];
    let imported = store.import_guardian_journal(journal.clone(), T0).await.unwrap();
    assert_eq!(imported, vec![("rcv_g2".to_string(), ImportDisposition::IgnoredStopIntent)], "完全退出優先");
    assert!(store.recovery_event().await.unwrap().is_none());
    assert_eq!(store.recovery_mirror(T0).await.unwrap().journal_cursor.as_deref(), Some("rcv_g2"));
}

fn guardian_for(event_id: &str, id: &str) -> JournalRecord {
    match guardian(1, id) {
        JournalRecord::Attempt(attempt) => JournalRecord::Attempt(GuardianAttempt { event_id: event_id.into(), ..attempt }),
        other => other,
    }
}

/// 把最新的恢復事件推到成功（處置完成、觀察期滿）。
async fn succeed_latest(store: &Store) {
    store.apply_recovery(RecoveryCommand::Complete { affected: vec![] }, T0).await.unwrap();
    let after = T0.plus_millis(24 * 60 * 60 * 1000);
    let event = store.apply_recovery(RecoveryCommand::Observe, after).await.unwrap();
    assert_eq!(event.status, RecoveryStatus::Succeeded);
}

#[tokio::test]
async fn a_guardian_record_pointing_at_a_closed_event_starts_a_new_event_instead_of_failing_the_import() {
    // 監護的鏡像過期：rec_g 已成功結束，監護仍以 rec_g 寫下新的嘗試（R151 第 2 項）
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path()).await;
    store.import_guardian_journal(vec![guardian(1, "rcv_g1")], T0).await.unwrap();
    succeed_latest(&store).await;

    let journal = vec![guardian(1, "rcv_g1"), guardian_for("rec_g", "rcv_g2")];
    let imported = store.import_guardian_journal(journal.clone(), T0).await.unwrap();
    assert_eq!(imported, vec![("rcv_g2".to_string(), ImportDisposition::Counted)]);
    let event = store.recovery_event().await.unwrap().unwrap();
    assert_ne!(event.id, "rec_g", "不能沿用已結束事件的識別");
    assert_eq!((event.attempts_used, event.status), (1, RecoveryStatus::Attempting));
    assert_eq!(event.current_attempt().unwrap().id, "rcv_g2");

    // 舊事件維持成功，之後再次啟動讀到同一份日誌也不會失敗
    let closed: String = store
        .read(|conn| Ok(conn.query_row("SELECT status FROM recovery_event WHERE id = 'rec_g'", [], |r| r.get(0))?))
        .await
        .unwrap();
    assert_eq!(closed, "succeeded");
    assert!(store.import_guardian_journal(journal, T0).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_guardian_record_pointing_at_an_older_closed_event_also_starts_a_new_event() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path()).await;
    store.import_guardian_journal(vec![guardian(1, "rcv_g1")], T0).await.unwrap();
    succeed_latest(&store).await;
    // Runtime 自己偵測到另一次崩潰並恢復成功，最新事件變成 rec_2
    store.note_runtime_start(true, "rec_2".into(), T0).await.unwrap();
    store.apply_recovery(RecoveryCommand::Begin { attempt_id: "a1".into() }, T0).await.unwrap();
    succeed_latest(&store).await;

    let imported = store.import_guardian_journal(vec![guardian_for("rec_g", "rcv_g3")], T0).await.unwrap();
    assert_eq!(imported, vec![("rcv_g3".to_string(), ImportDisposition::Counted)]);
    let event = store.recovery_event().await.unwrap().unwrap();
    assert!(event.id != "rec_g" && event.id != "rec_2", "新事件：{}", event.id);
    assert_eq!(event.status, RecoveryStatus::Attempting);
}
