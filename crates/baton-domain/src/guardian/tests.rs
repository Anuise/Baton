//! 監護規則的單元測試（票 32）。純 Domain，不碰檔案。

use super::*;
use crate::lifecycle::{LifecycleFile, MaintenanceSuppression, StopSuppression, LIFECYCLE_FILE_VERSION};
use crate::recovery::{self, RecoveryCommand, RecoveryEvent};

const T0: Timestamp = Timestamp::from_millis(1_000_000);

fn attempt(event: &str, round: u32, seq: u32, id: &str, at: Timestamp) -> GuardianAttempt {
    GuardianAttempt {
        event_id: event.into(),
        round,
        seq,
        attempt_id: id.into(),
        detected_at: at,
        at,
        reason: "監護確認 Runtime 意外終止".into(),
    }
}

fn plan(mirror: Option<&RecoveryMirror>, journal: &[JournalRecord], exit_at: Timestamp, now: Timestamp) -> RestartPlan {
    plan_restart(&RestartInput { mirror, journal, suppression: None, exit_observed_at: exit_at, now }, "rec_new", "rcv_new")
}

fn mirror_of(event: &RecoveryEvent, cursor: Option<&str>) -> RecoveryMirror {
    RecoveryMirror::of(Some(event), cursor.map(String::from), T0)
}

fn crashed() -> RecoveryEvent {
    recovery::on_start(None, true, "rec_1", T0).unwrap().event
}

fn begin(event: &RecoveryEvent, id: &str, now: Timestamp) -> RecoveryEvent {
    recovery::decide(Some(event), RecoveryCommand::Begin { attempt_id: id.into() }, now).unwrap().unwrap().event
}

// ------------------------------------------------------------ 日誌格式

#[test]
fn journal_records_are_single_lines_that_round_trip() {
    let records = vec![
        JournalRecord::Attempt(attempt("rec_1", 1, 1, "rcv_1", T0)),
        JournalRecord::Withdrawn { attempt_id: "rcv_1".into(), at: T0, reason: "完全退出".into() },
    ];
    let text: String = records.iter().map(|r| format!("{}\n", r.to_line())).collect();
    assert_eq!(text.lines().count(), 2, "每筆紀錄一行");
    assert_eq!(parse_journal(&text), records);
}

#[test]
fn torn_or_unknown_lines_are_skipped() {
    let good = JournalRecord::Attempt(attempt("rec_1", 1, 1, "rcv_1", T0)).to_line();
    let text = format!("{good}\n{{\"kind\":\"attempt\",\"event_i\n\nnot json\n{good}");
    assert_eq!(parse_journal(&text).len(), 2, "寫到一半的行與空行略過，最後一行沒有換行也算");
}

// ------------------------------------------------------------ 抑制

fn file(stop: bool, maintenance: Option<&str>) -> LifecycleFile {
    LifecycleFile {
        version: LIFECYCLE_FILE_VERSION,
        stop: stop.then(|| StopSuppression { requested_at: T0, reason: "full_exit".into() }),
        maintenance: maintenance.map(|kind| MaintenanceSuppression { kind: kind.into(), requested_at: T0 }),
        updated_at: T0,
    }
}

#[test]
fn full_exit_update_uninstall_and_unknown_stop_intent_suppress_restart() {
    assert_eq!(suppression_of(&LifecycleFileRead::Missing), None);
    assert_eq!(suppression_of(&LifecycleFileRead::Parsed(file(false, None))), None);
    assert_eq!(suppression_of(&LifecycleFileRead::Parsed(file(true, None))), Some(Suppression::FullExit));
    assert_eq!(
        suppression_of(&LifecycleFileRead::Parsed(file(false, Some("update")))),
        Some(Suppression::Maintenance("update".into()))
    );
    assert_eq!(
        suppression_of(&LifecycleFileRead::Parsed(file(false, Some("uninstall")))),
        Some(Suppression::Maintenance("uninstall".into()))
    );
    assert_eq!(suppression_of(&LifecycleFileRead::Unreadable), Some(Suppression::Unknown), "停止意圖不明時不推定允許啟動");
}

#[test]
fn suppression_wins_over_everything_else() {
    let input = RestartInput {
        mirror: None,
        journal: &[],
        suppression: Some(Suppression::FullExit),
        exit_observed_at: T0,
        now: T0,
    };
    assert_eq!(plan_restart(&input, "rec_new", "rcv_new"), RestartPlan::Suppressed(Suppression::FullExit));
}

// ------------------------------------------------------------ 計次與退避

#[test]
fn first_crash_without_event_starts_a_new_event_immediately() {
    match plan(None, &[], T0, T0) {
        RestartPlan::Start(a) => {
            assert_eq!((a.event_id.as_str(), a.round, a.seq, a.attempt_id.as_str()), ("rec_new", 1, 1, "rcv_new"));
            assert_eq!((a.detected_at, a.at), (T0, T0));
        }
        other => panic!("應立即啟動：{other:?}"),
    }
}

#[test]
fn crash_during_an_attempt_waits_the_backoff_from_the_observed_exit() {
    let event = begin(&crashed(), "rcv_1", T0);
    let mirror = mirror_of(&event, None);
    let exit_at = T0.plus_millis(5_000);
    let until = exit_at.plus_millis(recovery::BACKOFF_BEFORE_SECOND_MS);
    assert_eq!(plan(Some(&mirror), &[], exit_at, exit_at), RestartPlan::Wait { until });
    match plan(Some(&mirror), &[], exit_at, until) {
        RestartPlan::Start(a) => assert_eq!((a.event_id.as_str(), a.round, a.seq), ("rec_1", 1, 2), "沿用原事件"),
        other => panic!("退避期滿應啟動第二次：{other:?}"),
    }
}

#[test]
fn records_the_runtime_has_not_imported_still_count_so_early_failures_are_counted() {
    // Runtime 從沒啟動成功：鏡像裡什麼都沒有，日誌有兩筆嘗試
    let journal = vec![
        JournalRecord::Attempt(attempt("rec_g", 1, 1, "rcv_1", T0)),
        JournalRecord::Attempt(attempt("rec_g", 1, 2, "rcv_2", T0.plus_millis(30_000))),
    ];
    let exit_at = T0.plus_millis(31_000);
    let until = exit_at.plus_millis(recovery::BACKOFF_BEFORE_THIRD_MS);
    assert_eq!(plan(None, &journal, exit_at, exit_at), RestartPlan::Wait { until });
    match plan(None, &journal, exit_at, until) {
        RestartPlan::Start(a) => assert_eq!((a.event_id.as_str(), a.seq), ("rec_g", 3)),
        other => panic!("{other:?}"),
    }
    let journal = [journal, vec![JournalRecord::Attempt(attempt("rec_g", 1, 3, "rcv_3", until))]].concat();
    assert_eq!(plan(None, &journal, until, until.plus_millis(600_000)), RestartPlan::Exhausted, "三次都用掉就不再啟動");
}

#[test]
fn records_already_imported_are_not_counted_twice() {
    // 監護啟動第 1 次 → Runtime 匯入並寫下鏡像（游標指到 rcv_1）→ Runtime 處置完成、觀察中又當掉
    let journal = vec![JournalRecord::Attempt(attempt("rec_1", 1, 1, "rcv_1", T0))];
    let event = begin(&crashed(), "rcv_1", T0);
    let mirror = mirror_of(&event, Some("rcv_1"));
    let exit_at = T0.plus_millis(60_000);
    match plan(Some(&mirror), &journal, exit_at, exit_at.plus_millis(recovery::BACKOFF_BEFORE_SECOND_MS)) {
        RestartPlan::Start(a) => assert_eq!(a.seq, 2, "已匯入的第 1 次不重複計算"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn withdrawn_attempts_do_not_count() {
    let journal = vec![
        JournalRecord::Attempt(attempt("rec_g", 1, 1, "rcv_1", T0)),
        JournalRecord::Withdrawn { attempt_id: "rcv_1".into(), at: T0, reason: "維護".into() },
    ];
    match plan(None, &journal, T0, T0) {
        RestartPlan::Start(a) => assert_eq!((a.event_id.as_str(), a.seq), ("rec_new", 1)),
        other => panic!("{other:?}"),
    }
}

#[test]
fn runtime_side_backoff_and_exhaustion_are_respected() {
    // Runtime 自己的嘗試失敗（無法確認舊程序停止）後在退避中當掉：期限照舊，不另外計次
    let event = begin(&crashed(), "rcv_1", T0);
    let failed = recovery::decide(
        Some(&event),
        RecoveryCommand::Fail { reason: "仍存活".into(), affected: vec![] },
        T0.plus_millis(1_000),
    )
    .unwrap()
    .unwrap()
    .event;
    let deadline = failed.next_attempt_at.unwrap();
    let mirror = mirror_of(&failed, None);
    assert_eq!(plan(Some(&mirror), &[], T0.plus_millis(2_000), T0.plus_millis(2_000)), RestartPlan::Wait { until: deadline });

    let mut exhausted = failed.clone();
    exhausted.status = RecoveryStatus::Exhausted;
    assert_eq!(plan(Some(&mirror_of(&exhausted, None)), &[], T0, T0), RestartPlan::Exhausted);
}

#[test]
fn a_succeeded_event_means_the_next_crash_is_a_new_event() {
    let mut done = begin(&crashed(), "rcv_1", T0);
    done.status = RecoveryStatus::Succeeded;
    match plan(Some(&mirror_of(&done, None)), &[], T0, T0) {
        RestartPlan::Start(a) => assert_eq!((a.event_id.as_str(), a.round, a.seq), ("rec_new", 1, 1)),
        other => panic!("{other:?}"),
    }
}

#[test]
fn guardian_status_reports_degradation_with_the_fixed_message() {
    assert_eq!(GuardianStatus::of(true), GuardianStatus { state: GuardianState::Active, message: None });
    let degraded = GuardianStatus::of(false);
    assert_eq!(degraded.state, GuardianState::Unavailable);
    assert_eq!(degraded.message.as_deref(), Some(DEGRADED_MESSAGE));
    assert_eq!(DEGRADED_MESSAGE, "工作繼續，自動重啟保護不可用");
}
