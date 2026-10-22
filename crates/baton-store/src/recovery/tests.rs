//! 崩潰恢復持久化的單元測試（票 31）：正常結束標記、啟動判定、嘗試序號的唯一性。

use baton_domain::recovery::{RecoveryCommand, RecoveryStatus};

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
