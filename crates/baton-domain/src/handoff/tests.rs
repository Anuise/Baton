//! 額度門檻與交接 phase 的單元測試（票 28）：純 Domain，輸入目前狀態與時間，斷言下一個狀態與事件。

use super::*;
use crate::agent::ReportContent;
use crate::quota::{self, QuotaWindow, RateLimitSignal};

const T0: Timestamp = Timestamp::from_millis(1_700_000_000_000);
const MINUTE: i64 = 60_000;

fn at(minutes: i64) -> Timestamp {
    T0.plus_millis(minutes * MINUTE)
}

/// 一個帳號只有一個五小時視窗的 CLI 訊號時的判斷。
fn assessed(status: LimitStatus, used: Option<f64>, reset_at: Option<Timestamp>, now: Timestamp) -> QuotaAssessment {
    let signal = RateLimitSignal { window: Some(QuotaWindow::FiveHour), status, used, reset_at, raw: "{}".into() };
    quota::assess(&[quota::observe_signal("p1", "run_1", &signal, now)], now)
}

fn used(ratio: f64, now: Timestamp) -> QuotaAssessment {
    assessed(LimitStatus::Allowed, Some(ratio), None, now)
}

fn unknown(now: Timestamp) -> QuotaAssessment {
    quota::assess(&[], now)
}

/// 依序套用多次判斷，回傳最後的門檻（`None` = 仍是 open 且從未離開）。
fn walk(steps: &[(QuotaAssessment, Timestamp)]) -> Option<QuotaGate> {
    let mut gate: Option<QuotaGate> = None;
    for (assessment, now) in steps {
        if let Some((next, event)) = evaluate_gate("p1", gate.as_ref(), assessment, *now) {
            assert_eq!(event.event_type, EVENT_QUOTA_GATE_CHANGED);
            assert_eq!(event.aggregate_version, next.version);
            gate = Some(next);
        }
    }
    gate
}

fn state(gate: &Option<QuotaGate>) -> GateState {
    gate.as_ref().map_or(GateState::Open, |g| g.state)
}

// --- 額度門檻 ---

#[test]
fn thresholds_pause_at_75_and_drain_at_85_percent() {
    assert_eq!(state(&walk(&[(used(0.74, T0), T0)])), GateState::Open);
    assert_eq!(state(&walk(&[(used(0.75, T0), T0)])), GateState::Paused);
    assert_eq!(state(&walk(&[(used(0.849, T0), T0)])), GateState::Paused);
    assert_eq!(state(&walk(&[(used(0.85, T0), T0)])), GateState::Draining);
}

#[test]
fn a_limit_warning_drains_even_without_a_number() {
    let gate = walk(&[(assessed(LimitStatus::Warning, None, None, T0), T0)]).unwrap();
    assert_eq!(gate.state, GateState::Draining);
    assert_eq!(gate.reason, "limit_warning");
    let exhausted = walk(&[(assessed(LimitStatus::Exhausted, None, None, T0), T0)]).unwrap();
    assert_eq!(exhausted.state, GateState::Draining);
}

#[test]
fn recovery_needs_below_60_percent_hysteresis_between_60_and_75() {
    let gate = walk(&[(used(0.8, at(0)), at(0)), (used(0.7, at(1)), at(1)), (used(0.61, at(2)), at(2))]);
    assert_eq!(state(&gate), GateState::Paused, "60%～75% 之間維持 paused");
    let gate = walk(&[(used(0.8, at(0)), at(0)), (used(0.7, at(1)), at(1)), (used(0.59, at(2)), at(2))]).unwrap();
    assert_eq!(gate.state, GateState::Open);
    assert_eq!(gate.reason, "recovered");
    assert_eq!(gate.reset_at, None);
}

#[test]
fn draining_does_not_fall_back_to_paused_until_reset_or_recovery() {
    let gate = walk(&[(used(0.9, at(0)), at(0)), (used(0.8, at(1)), at(1)), (used(0.65, at(2)), at(2))]);
    assert_eq!(state(&gate), GateState::Draining);
    let gate = walk(&[(used(0.9, at(0)), at(0)), (used(0.5, at(1)), at(1))]);
    assert_eq!(state(&gate), GateState::Open);
}

#[test]
fn passing_the_recorded_reset_time_reopens_the_account() {
    let reset = at(30);
    let entering = assessed(LimitStatus::Allowed, Some(0.8), Some(reset), at(0));
    let gate = walk(&[(entering.clone(), at(0))]).unwrap();
    assert_eq!(gate.reset_at, Some(reset), "進入時記下重置時間");

    // 重置前用量 70%：維持；重置之後同樣 70%：恢復
    let before = walk(&[(entering.clone(), at(0)), (used(0.7, at(29)), at(29))]);
    assert_eq!(state(&before), GateState::Paused);
    let after = walk(&[(entering, at(0)), (used(0.7, at(31)), at(31))]).unwrap();
    assert_eq!(after.state, GateState::Open);
    assert_eq!(after.reason, "reset_observed");
}

#[test]
fn a_signal_that_expired_at_its_reset_leaves_usage_unknown_and_reopens() {
    let reset = at(30);
    let gate = walk(&[
        (assessed(LimitStatus::Warning, None, Some(reset), at(0)), at(0)),
        // 訊號在重置時間失效，用量回到未知
        (unknown(at(31)), at(31)),
    ])
    .unwrap();
    assert_eq!(gate.state, GateState::Open);
    assert_eq!(gate.reason, "usage_unknown");
}

#[test]
fn an_unchanged_state_writes_nothing() {
    let gate = walk(&[(used(0.8, at(0)), at(0))]).unwrap();
    assert!(evaluate_gate("p1", Some(&gate), &used(0.8, at(1)), at(1)).is_none());
    assert!(evaluate_gate("p1", None, &used(0.3, at(1)), at(1)).is_none(), "從未離開 open");
}

// --- 交接 ---

fn running(id: &str, profile: &str, task: &str, generation: i64) -> Run {
    Run {
        id: id.into(),
        workspace_id: "ws".into(),
        task_id: task.into(),
        profile_id: profile.into(),
        status: RunStatus::Running,
        version: 3,
        generation,
        contract_version: 1,
        authorization_revision: None,
        admitted_at: Some(T0),
        settings_version: None,
        command_id: Some(format!("cmd_{id}")),
        process: None,
        cancel_requested_at: None,
        end_reason: None,
        created_at: T0,
        updated_at: T0,
        started_at: Some(T0),
        ended_at: None,
        native_session_id: None,
        agent_session_id: None,
        resume_native_session_id: None,
    }
}

fn gates(entries: &[(&str, GateState)]) -> HashMap<String, QuotaGate> {
    entries
        .iter()
        .map(|(profile, state)| {
            let gate = QuotaGate {
                profile_id: profile.to_string(),
                state: *state,
                reason: "drain_threshold".into(),
                used: Some(0.9),
                reset_at: None,
                since: T0,
                version: 1,
            };
            (profile.to_string(), gate)
        })
        .collect()
}

fn checkpoint(run_id: &str, generation: i64, summary: &str, saved_at: Timestamp) -> ReportRecord {
    ReportRecord {
        workspace_id: "ws".into(),
        run_id: run_id.into(),
        generation,
        accepted: true,
        content: ReportContent::Checkpoint { summary: summary.into(), details: format!("{summary} 的細節") },
        received_at: saved_at,
    }
}

fn requested(run: &Run, now: Timestamp) -> Handoff {
    let (handoff, event) = request("hof_1".into(), run, "p2", "drain_threshold", now);
    assert_eq!(event.event_type, EVENT_HANDOFF_REQUESTED);
    assert_eq!(event.aggregate_version, 1);
    handoff
}

#[test]
fn only_running_runs_on_a_draining_account_are_handed_off() {
    let mut cancelled = running("r3", "p1", "t3", 1);
    cancelled.cancel_requested_at = Some(T0);
    let mut queued = running("r4", "p1", "t4", 1);
    queued.status = RunStatus::Queued;
    let runs = vec![running("r1", "p1", "t1", 1), running("r2", "p2", "t2", 1), cancelled, queued];
    let gates = gates(&[("p1", GateState::Draining), ("p2", GateState::Paused)]);

    let picked: Vec<(&str, String)> =
        candidates(&runs, &gates, &[], T0).into_iter().map(|(r, reason)| (r.id.as_str(), reason)).collect();
    assert_eq!(picked, [("r1", "drain_threshold".to_string())], "paused 帳號上的 Run 繼續執行");
}

#[test]
fn a_run_already_being_handed_off_is_not_requested_twice() {
    let run = running("r1", "p1", "t1", 1);
    let handoff = requested(&run, T0);
    let runs = vec![run];
    assert!(candidates(&runs, &gates(&[("p1", GateState::Draining)]), &[handoff], at(1)).is_empty());
}

#[test]
fn the_same_task_waits_ten_minutes_between_account_switches() {
    let first = running("r1", "p1", "t1", 1);
    let successor = running("r2", "p2", "t1", 2);
    let (stopping, _) = begin_stop(&requested(&first, at(0)), None, at(1)).unwrap().unwrap();
    let (moved, _) = transfer(&stopping, &successor, None, "stopped_gracefully", at(2)).unwrap();
    assert_eq!(cooldown_until(&[moved.clone()], "t1"), Some(at(12)));

    let runs = vec![successor];
    let draining = gates(&[("p2", GateState::Draining)]);
    assert!(candidates(&runs, &draining, &[moved.clone()], at(11)).is_empty(), "冷卻中");
    assert_eq!(candidates(&runs, &draining, &[moved], at(12)).len(), 1, "滿 10 分鐘");
}

#[test]
fn the_safe_boundary_is_a_checkpoint_saved_after_the_request_or_the_wait_limit() {
    let run = running("r1", "p1", "t1", 1);
    let handoff = requested(&run, at(10));
    let runs = vec![run];

    let old = latest_checkpoint(&[checkpoint("r1", 1, "舊的", at(5))], &runs, "t1");
    assert!(!at_safe_boundary(&handoff, old.as_ref(), at(11)), "要求之前的 checkpoint 不算");
    let fresh = latest_checkpoint(&[checkpoint("r1", 1, "舊的", at(5)), checkpoint("r1", 1, "新的", at(11))], &runs, "t1");
    assert_eq!(fresh.as_ref().unwrap().summary, "新的");
    assert!(at_safe_boundary(&handoff, fresh.as_ref(), at(11)));
    assert!(at_safe_boundary(&handoff, old.as_ref(), at(12)), "等滿兩分鐘");
}

#[test]
fn the_latest_checkpoint_spans_generations_and_ignores_rejected_reports() {
    let runs = vec![running("r1", "p1", "t1", 1), running("r2", "p2", "t1", 2), running("x", "p1", "other", 1)];
    let mut stale = checkpoint("r1", 1, "過期世代", at(9));
    stale.accepted = false;
    let reports = [checkpoint("r1", 1, "第一代", at(1)), checkpoint("r2", 2, "第二代", at(5)), stale, checkpoint("x", 1, "別的 Task", at(8))];
    let latest = latest_checkpoint(&reports, &runs, "t1").unwrap();
    assert_eq!((latest.run_id.as_str(), latest.generation, latest.summary.as_str()), ("r2", 2, "第二代"));
}

#[test]
fn phases_advance_in_order_and_each_step_is_versioned() {
    let run = running("r1", "p1", "t1", 1);
    let successor = running("r2", "p3", "t1", 2);
    let saved = latest_checkpoint(&[checkpoint("r1", 1, "做到一半", at(1))], &[run.clone()], "t1");

    let handoff = requested(&run, at(0));
    assert_eq!((handoff.phase, handoff.from_generation, handoff.to_profile_id.as_str()), (HandoffPhase::Requested, 1, "p2"));
    let (stopping, event) = begin_stop(&handoff, saved.clone(), at(1)).unwrap().unwrap();
    assert_eq!((stopping.phase, event.event_type, event.aggregate_version), (HandoffPhase::Stopping, EVENT_HANDOFF_STOPPING, 2));
    assert!(begin_stop(&stopping, None, at(2)).unwrap().is_none(), "重複進入 stopping 是冪等的");

    let (moved, event) = transfer(&stopping, &successor, saved.clone(), "force_killed", at(3)).unwrap();
    assert_eq!(event.event_type, EVENT_HANDOFF_TRANSFERRED);
    assert_eq!(moved.phase, HandoffPhase::Transferred);
    assert_eq!((moved.to_run_id.as_deref(), moved.to_generation, moved.to_profile_id.as_str()), (Some("r2"), Some(2), "p3"));
    assert_eq!(moved.stop_outcome.as_deref(), Some("force_killed"));
    assert_eq!(moved.checkpoint, saved);
    assert!(transfer(&moved, &successor, None, "exited", at(4)).is_err(), "不會轉移兩次");
    assert!(abandon(&moved, "x", at(4)).is_err());

    let confirmation = HandoffConfirmation::new("步驟一", "步驟二", "無").unwrap();
    assert_eq!(confirm(&moved, "r1", confirmation.clone(), at(5)), Err(HandoffError::NotPending), "只有 successor 能確認");
    let (done, event) = confirm(&moved, "r2", confirmation.clone(), at(5)).unwrap().unwrap();
    assert_eq!((done.phase, event.event_type, done.version), (HandoffPhase::Confirmed, EVENT_HANDOFF_CONFIRMED, 4));
    assert_eq!(done.confirmation, Some(confirmation.clone()));
    assert!(confirm(&done, "r2", confirmation, at(6)).unwrap().is_none(), "重複確認是冪等的");
}

#[test]
fn a_requested_handoff_can_be_abandoned_but_not_confirmed() {
    let run = running("r1", "p1", "t1", 1);
    let handoff = requested(&run, at(0));
    let confirmation = HandoffConfirmation::new("a", "b", "c").unwrap();
    assert_eq!(confirm(&handoff, "r1", confirmation, at(1)), Err(HandoffError::NotPending));
    let (gone, event) = abandon(&handoff, "from_run_ended", at(1)).unwrap();
    assert_eq!((gone.phase, event.event_type), (HandoffPhase::Abandoned, EVENT_HANDOFF_ABANDONED));
    assert_eq!(gone.abandoned_reason.as_deref(), Some("from_run_ended"));
    assert!(!gone.phase.is_in_flight());
}

#[test]
fn a_confirmation_needs_all_three_fields() {
    assert_eq!(HandoffConfirmation::new(" ", "b", "c"), Err(ConfirmationError::Empty("已完成")));
    assert_eq!(HandoffConfirmation::new("a", "b", ""), Err(ConfirmationError::Empty("未知副作用")));
    let long = "字".repeat(CONFIRMATION_MAX_CHARS + 1);
    assert_eq!(HandoffConfirmation::new("a", &long, "c"), Err(ConfirmationError::TooLong("未完成")));
    assert_eq!(HandoffConfirmation::new(" a ", "b", "無").unwrap().completed, "a");
}

#[test]
fn the_successor_prompt_carries_the_checkpoint_and_asks_for_confirmation() {
    let run = running("r1", "p1", "t1", 1);
    let handoff = requested(&run, at(0));
    let saved = latest_checkpoint(&[checkpoint("r1", 1, "完成步驟一", at(1))], &[run], "t1");
    let prompt = successor_prompt(&handoff, saved.as_ref());
    assert!(prompt.contains("完成步驟一") && prompt.contains("完成步驟一 的細節"), "{prompt}");
    assert!(prompt.contains("confirm_handoff"));
    assert!(successor_prompt(&handoff, None).contains("沒有保存任何檢查點"));
}

#[test]
fn names_round_trip() {
    for phase in HandoffPhase::ALL {
        assert_eq!(HandoffPhase::parse(phase.as_str()), Some(phase));
    }
    for state in GateState::ALL {
        assert_eq!(GateState::parse(state.as_str()), Some(state));
    }
    assert!(GateState::Paused.blocks_dispatch() && GateState::Draining.blocks_dispatch());
    assert!(!GateState::Open.blocks_dispatch());
}
