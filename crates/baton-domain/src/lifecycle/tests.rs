//! 生命週期與完全退出的單元測試（票 25）。純 Domain，不經 HTTP。

use super::*;

const T0: Timestamp = Timestamp::from_millis(1_000);
const T1: Timestamp = Timestamp::from_millis(2_000);

fn workspace(id: &str, status: WorkspaceStatus) -> Workspace {
    Workspace {
        id: id.into(),
        name: format!("名稱 {id}"),
        path: format!("C:\\work\\{id}"),
        status,
        version: 1,
        created_at: T0,
        archived_at: None,
    }
}

fn stopping(workspaces: &[Workspace], existing: &[WorkspaceLifecycle]) -> (LifecycleState, Vec<WorkspaceLifecycle>) {
    let change = request_exit(&LifecycleState::default(), workspaces, existing, ExitTiming::default(), T0);
    (change.state.unwrap(), change.workspaces)
}

#[test]
fn default_exit_timing_is_thirty_plus_ten_seconds_within_forty() {
    let timing = ExitTiming::default();
    assert_eq!(timing.stop_grace_ms, 30_000);
    assert_eq!(timing.kill_confirm_ms, 10_000);
    assert_eq!(timing.total_ms(), 40_000);
}

#[test]
fn requesting_exit_persists_stop_intent_with_forty_second_deadline_and_pauses_active_workspaces() {
    let workspaces = [workspace("ws_a", WorkspaceStatus::Active), workspace("ws_old", WorkspaceStatus::Archived)];
    let change = request_exit(&LifecycleState::default(), &workspaces, &[], ExitTiming::default(), T0);
    let state = change.state.unwrap();
    assert_eq!(state.phase, LifecyclePhase::Stopping);
    assert!(state.stop_intent_active());
    let report = state.last_exit.unwrap();
    assert_eq!(report.requested_at, T0);
    assert_eq!(report.deadline_at, T0.plus_millis(40_000));
    assert_eq!(report.completed_at, None);
    // 只暫停使用中的 Workspace；封存的不管
    assert_eq!(change.workspaces.len(), 1);
    assert_eq!(change.workspaces[0].workspace_id, "ws_a");
    assert_eq!(change.workspaces[0].paused_since, Some(T0));
    assert!(!change.workspaces[0].auto_resume_after_exit, "自動續跑預設關閉");
    let types: Vec<_> = change.events.iter().map(|e| e.event_type).collect();
    assert_eq!(types, [EVENT_FULL_EXIT_REQUESTED, EVENT_WORKSPACE_PAUSED_BY_EXIT]);
}

#[test]
fn requesting_exit_again_while_stopping_changes_nothing() {
    let (state, _) = stopping(&[workspace("ws_a", WorkspaceStatus::Active)], &[]);
    let again = request_exit(&state, &[workspace("ws_a", WorkspaceStatus::Active)], &[], ExitTiming::default(), T1);
    assert_eq!(again, LifecycleChange::default());
}

#[test]
fn requesting_exit_after_stopped_but_before_reopen_is_a_new_stop_intent() {
    let (state, _) = stopping(&[], &[]);
    let (stopped, _) = complete_exit(&state, state.last_exit.clone().unwrap(), T1).unwrap();
    let again = request_exit(&stopped, &[], &[], ExitTiming::default(), T1).state.unwrap();
    assert_eq!(again.phase, LifecyclePhase::Stopping);
    assert!(again.version > stopped.version, "新的退出要求讓版本前進，重新開啟的核對才不會把它清掉");
}

#[test]
fn completing_exit_records_anomalies_and_moves_to_stopped() {
    let (state, _) = stopping(&[], &[]);
    let mut report = state.last_exit.clone().unwrap();
    report.record(RunStopRecord { outcome: RunStopOutcome::StoppedGracefully, ..RunStopRecord::pending("run_1", "ws_a", "task_1") });
    report.record(RunStopRecord {
        outcome: RunStopOutcome::Anomaly,
        anomaly: Some(StopAnomaly::StillAlive),
        message: Some("程序樹終止後仍存活".into()),
        ..RunStopRecord::pending("run_2", "ws_a", "task_2")
    });
    let (stopped, event) = complete_exit(&state, report, T1).unwrap();
    assert_eq!(stopped.phase, LifecyclePhase::Stopped);
    assert!(stopped.stop_intent_active(), "停止流程結束後停止意圖仍然生效，直到重新開啟");
    let last = stopped.last_exit.unwrap();
    assert_eq!(last.completed_at, Some(T1));
    assert_eq!(last.anomalies().count(), 1);
    assert_eq!(event.event_type, EVENT_FULL_EXIT_COMPLETED);
    assert_eq!(event.payload["anomalies"][0]["run_id"], "run_2");
    assert_eq!(event.payload["anomalies"][0]["anomaly"], "still_alive");
}

#[test]
fn recording_the_same_run_twice_replaces_its_outcome() {
    let mut report = ExitReport { requested_at: T0, deadline_at: T1, completed_at: None, runs: vec![], notes: vec![] };
    report.record(RunStopRecord::pending("run_1", "ws", "task"));
    report.record(RunStopRecord { outcome: RunStopOutcome::ForceKilled, ..RunStopRecord::pending("run_1", "ws", "task") });
    assert_eq!(report.runs.len(), 1);
    assert_eq!(report.runs[0].outcome, RunStopOutcome::ForceKilled);
}

#[test]
fn progress_is_only_recorded_while_stopping() {
    assert!(record_progress(&LifecycleState::default(), stopping(&[], &[]).0.last_exit.unwrap(), T1).is_none());
    let (state, _) = stopping(&[], &[]);
    assert!(record_progress(&state, state.last_exit.clone().unwrap(), T1).is_some());
}

#[test]
fn reopening_releases_only_workspaces_with_auto_resume_enabled() {
    let mut a = WorkspaceLifecycle::new("ws_a");
    a.auto_resume_after_exit = true;
    let b = WorkspaceLifecycle::new("ws_b");
    let (state, paused) = stopping(
        &[workspace("ws_a", WorkspaceStatus::Active), workspace("ws_b", WorkspaceStatus::Active)],
        &[a, b],
    );
    let (stopped, _) = complete_exit(&state, state.last_exit.clone().unwrap(), T1).unwrap();

    let change = reopen(&stopped, stopped.version, &paused, T1).unwrap();
    let reopened = change.state.unwrap();
    assert_eq!(reopened.phase, LifecyclePhase::Running);
    assert!(reopened.last_exit.is_some(), "上次退出的結果保留供顯示");
    assert_eq!(change.workspaces.len(), 1);
    assert_eq!(change.workspaces[0].workspace_id, "ws_a");
    assert_eq!(change.workspaces[0].paused_since, None);
    let event = change.events.iter().find(|e| e.event_type == EVENT_RUNTIME_REOPENED).unwrap();
    assert_eq!(event.payload["resumed"], serde_json::json!(["ws_a"]));
    assert_eq!(event.payload["awaiting_resume"], serde_json::json!(["ws_b"]));
}

#[test]
fn reopening_is_skipped_when_a_new_exit_was_requested_after_startup() {
    let (state, _) = stopping(&[], &[]);
    let (stopped, _) = complete_exit(&state, state.last_exit.clone().unwrap(), T1).unwrap();
    let seen_at_startup = stopped.version;
    let again = request_exit(&stopped, &[], &[], ExitTiming::default(), T1).state.unwrap();
    assert!(reopen(&again, seen_at_startup, &[], T1).is_none());
    assert!(reopen(&LifecycleState::default(), 0, &[], T1).is_none(), "沒有停止意圖時不需要重新開啟");
}

#[test]
fn resuming_a_paused_workspace_requires_the_stop_intent_to_be_cleared() {
    let (state, paused) = stopping(&[workspace("ws_b", WorkspaceStatus::Active)], &[]);
    assert_eq!(resume_workspace(&state, &paused[0], T1), Err(LifecycleError::StopIntentActive));

    let running = LifecycleState { phase: LifecyclePhase::Running, ..state };
    let (resumed, event) = resume_workspace(&running, &paused[0], T1).unwrap().unwrap();
    assert_eq!(resumed.paused_since, None);
    assert_eq!(event.event_type, EVENT_WORKSPACE_RESUMED);
    assert_eq!(event.payload["by"], "user");
    assert_eq!(resume_workspace(&running, &resumed, T1), Ok(None), "沒有暫停時沒有任何變動");
}

#[test]
fn changing_auto_resume_does_not_touch_the_pause_and_is_idempotent() {
    let mut paused = WorkspaceLifecycle::new("ws_b");
    paused.paused_since = Some(T0);
    let (changed, event) = set_auto_resume(&paused, true, T1).unwrap();
    assert!(changed.auto_resume_after_exit);
    assert_eq!(changed.paused_since, Some(T0), "設定只影響下次重新開啟，不立即啟停工作");
    assert_eq!(event.event_type, EVENT_WORKSPACE_AUTO_RESUME_CHANGED);
    assert!(set_auto_resume(&changed, true, T1).is_none());
}

#[test]
fn dispatch_is_halted_while_stop_intent_is_active_or_workspace_awaits_resume() {
    let workspace = WorkspaceLifecycle::new("ws_a");
    assert_eq!(dispatch_halt(&LifecycleState::default(), &workspace, "A"), None);
    assert!(!launch_halted(&LifecycleState::default(), &workspace));

    let (state, _) = stopping(&[], &[]);
    let halt = dispatch_halt(&state, &workspace, "A").unwrap();
    assert_eq!(halt.category, WaitCategory::Lifecycle);
    assert_eq!(halt.code, "runtime_stopping");
    assert!(launch_halted(&state, &workspace));

    let mut paused = workspace.clone();
    paused.paused_since = Some(T0);
    let halt = dispatch_halt(&LifecycleState::default(), &paused, "A").unwrap();
    assert_eq!(halt.code, "awaiting_resume");
    assert!(halt.message.contains("「A」"));
    assert!(launch_halted(&LifecycleState::default(), &paused));
}

#[test]
fn lifecycle_file_mirrors_the_stop_intent() {
    let clear = LifecycleFile::mirror(&LifecycleState::default(), T1);
    assert_eq!(clear.stop, None);
    let (state, _) = stopping(&[], &[]);
    let file = LifecycleFile::mirror(&state, T1);
    assert_eq!(file.stop.unwrap().requested_at, T0);
    assert_eq!(file.version, LIFECYCLE_FILE_VERSION);
}

#[test]
fn file_only_stop_intent_is_adopted_conservatively() {
    let running = LifecycleState::default();
    assert_eq!(conservative_stop(&running, &FileStopView::Stop { requested_at: Some(T0) }), Some(Some(T0)));
    assert_eq!(conservative_stop(&running, &FileStopView::Clear), None);
    assert_eq!(conservative_stop(&running, &FileStopView::Missing), None);
    let (state, _) = stopping(&[], &[]);
    assert_eq!(conservative_stop(&state, &FileStopView::Stop { requested_at: None }), None, "資料庫已經是停止就不用補");
}
