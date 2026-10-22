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

/// 只有本機推估（五小時視窗、方案上限 1000 token）時的判斷；`usage` 是（分鐘, token）。
fn estimated(usage: &[(i64, u64)], now: Timestamp) -> QuotaAssessment {
    let records: Vec<quota::UsageRecord> = usage
        .iter()
        .enumerate()
        .map(|(i, (minutes, tokens))| quota::UsageRecord { seq: i as i64, tokens: *tokens, reported_at: at(*minutes) })
        .collect();
    let limits = quota::PlanLimits { five_hour_tokens: Some(1000), seven_day_tokens: None };
    quota::assess(&[quota::estimate("p1", QuotaWindow::FiveHour, &records, &limits, now)], now)
}

#[test]
fn an_estimated_reset_time_is_not_an_observed_reset_and_keeps_the_hysteresis() {
    // 最早一筆用量在 1 分鐘後滑出視窗：推估的「重置時間」只是滑動視窗的邊界，不是帳號真的重置（R151 第 7 項）
    let usage = [(-299, 100), (0, 700)];
    let entering = estimated(&usage, at(0));
    assert_eq!((entering.used, entering.reset_at), (Some(0.8), Some(at(1))));

    let paused = walk(&[(entering.clone(), at(0)), (estimated(&usage, at(2)), at(2))]);
    assert_eq!(state(&paused), GateState::Paused, "推估 70% 仍在遲滯區間，只能等用量低於 60% 才恢復");

    let draining_usage = [(-299, 100), (0, 800)];
    let draining = walk(&[(estimated(&draining_usage, at(0)), at(0)), (estimated(&draining_usage, at(2)), at(2))]);
    assert_eq!(state(&draining), GateState::Draining, "推估 80% 的 draining 帳號沒有觀測到重置前不退回 paused");
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
    assert_eq!(cooldown_until(std::slice::from_ref(&moved), "t1"), Some(at(12)));

    let runs = vec![successor];
    let draining = gates(&[("p2", GateState::Draining)]);
    assert!(candidates(&runs, &draining, std::slice::from_ref(&moved), at(11)).is_empty(), "冷卻中");
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
    let saved = latest_checkpoint(&[checkpoint("r1", 1, "做到一半", at(1))], std::slice::from_ref(&run), "t1");

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

// --- 突然限流與等待資源（票 29） ---

fn draining_for(profile: &str, reason: &str) -> HashMap<String, QuotaGate> {
    let mut gates = gates(&[(profile, GateState::Draining)]);
    gates.get_mut(profile).unwrap().reason = reason.into();
    gates
}

fn quota_reason(code: &str) -> WaitReason {
    WaitReason {
        category: crate::scheduler::WaitCategory::Quota,
        code: code.into(),
        message: format!("{code} 的說明"),
        profile_id: Some("p2".into()),
        guard: None,
    }
}

#[test]
fn an_exhausted_signal_drains_as_a_sudden_limit_and_a_warning_does_not() {
    let gate = walk(&[(assessed(LimitStatus::Exhausted, None, None, T0), T0)]).unwrap();
    assert_eq!((gate.state, gate.reason.as_str()), (GateState::Draining, GATE_REASON_LIMIT_REACHED));
    assert!(is_sudden_limit(&gate.reason));
    let warned = walk(&[(assessed(LimitStatus::Warning, None, None, T0), T0)]).unwrap();
    assert!(!is_sudden_limit(&warned.reason), "警告還能繼續工作，照一般交接");
    // 警告之後撞到上限：原因改為突然限流（狀態不變也要寫入）
    let upgraded =
        walk(&[(assessed(LimitStatus::Warning, None, None, at(0)), at(0)), (assessed(LimitStatus::Exhausted, None, None, at(1)), at(1))])
            .unwrap();
    assert_eq!((upgraded.state, upgraded.reason.as_str(), upgraded.version), (GateState::Draining, GATE_REASON_LIMIT_REACHED, 2));
}

#[test]
fn a_sudden_limit_skips_the_switch_cooldown() {
    let first = running("r1", "p1", "t1", 1);
    let successor = running("r2", "p2", "t1", 2);
    let (stopping, _) = begin_stop(&requested(&first, at(0)), None, at(1)).unwrap().unwrap();
    let (moved, _) = transfer(&stopping, &successor, None, "stopped_gracefully", at(2)).unwrap();
    let runs = vec![successor];

    assert!(candidates(&runs, &draining_for("p2", "drain_threshold"), std::slice::from_ref(&moved), at(3)).is_empty(), "冷卻中");
    let picked = candidates(&runs, &draining_for("p2", GATE_REASON_LIMIT_REACHED), &[moved], at(3));
    assert_eq!(picked.len(), 1, "突然限流不受冷卻限制（spec D-13）");
    assert_eq!(picked[0].1, GATE_REASON_LIMIT_REACHED);
}

#[test]
fn a_sudden_limit_does_not_wait_for_a_last_checkpoint() {
    let run = running("r1", "p1", "t1", 1);
    let (handoff, _) = request("hof_1".into(), &run, "", GATE_REASON_LIMIT_REACHED, at(10));
    assert_eq!(handoff.to_profile_id, "", "沒有承接帳號也要交接：舊 Worker 已無法工作");
    assert!(at_safe_boundary(&handoff, None, at(10)), "不等最後一輪的 checkpoint");
}

#[test]
fn the_next_check_is_one_minute_after_the_earliest_known_reset() {
    let (next, reset) = next_resource_check(&[Some(at(90)), Some(at(30))], 0, at(0));
    assert_eq!((next, reset), (at(31), Some(at(30))));
    // 已過的重置時間不算（帳號仍受限）：改以退避
    let (next, reset) = next_resource_check(&[Some(at(-5))], 0, at(0));
    assert_eq!((next, reset), (at(5), None));
}

#[test]
fn without_a_reset_time_the_check_backs_off_from_five_to_sixty_minutes() {
    let minutes: Vec<i64> = (0..7)
        .map(|attempts| (next_resource_check(&[None], attempts, at(0)).0.as_millis() - at(0).as_millis()) / MINUTE)
        .collect();
    assert_eq!(minutes, [5, 10, 20, 40, 60, 60, 60]);
    assert_eq!(next_resource_check(&[], 0, at(0)), (at(5), None), "沒有任何可用帳號也是未知");
    // 部分帳號重置時間未知：取較早者
    assert_eq!(next_resource_check(&[Some(at(3)), None], 0, at(0)), (at(4), Some(at(3))));
    assert_eq!(next_resource_check(&[Some(at(30)), None], 0, at(0)), (at(5), Some(at(30))));
}

#[test]
fn a_stopped_sudden_limit_waits_for_resources_then_transfers_from_the_wait() {
    let run = running("r1", "p1", "t1", 1);
    let saved = latest_checkpoint(&[checkpoint("r1", 1, "做到一半", at(1))], std::slice::from_ref(&run), "t1");
    let (handoff, _) = request("hof_1".into(), &run, "", GATE_REASON_LIMIT_REACHED, at(2));
    let (stopping, _) = begin_stop(&handoff, saved.clone(), at(2)).unwrap().unwrap();

    let reasons = vec![quota_reason("quota_draining")];
    let (waiting, event) =
        wait_for_resource(&stopping, saved.clone(), "exited", &[Some(at(30))], reasons.clone(), at(3)).unwrap();
    assert_eq!((waiting.phase, event.event_type, waiting.version), (HandoffPhase::WaitingResource, EVENT_HANDOFF_WAITING_RESOURCE, 3));
    assert_eq!((waiting.waiting_at, waiting.next_check_at, waiting.wait_reset_at), (Some(at(3)), Some(at(31)), Some(at(30))));
    assert_eq!((waiting.wait_attempts, waiting.wait_reasons.clone()), (0, reasons.clone()));
    assert_eq!((waiting.checkpoint.clone(), waiting.stop_outcome.as_deref()), (saved.clone(), Some("exited")));
    assert!(!waiting.phase.is_in_flight(), "舊 Run 已結束，不再是寫入者");
    assert_eq!(event.payload["next_check_at"], at(31).as_millis());

    // 重新評估仍受限：重置時間未知時退避加倍
    let (again, event) = reschedule_wait(&waiting, &[None], reasons.clone(), at(31)).unwrap();
    assert_eq!((event.event_type, again.wait_attempts, again.next_check_at, again.wait_reset_at), (EVENT_HANDOFF_WAIT_RESCHEDULED, 1, Some(at(41)), None));
    assert!(reschedule_wait(&stopping, &[None], reasons, at(4)).is_err(), "只有等待中的交接能重新排定");

    // 資源恢復：由新帳號的 successor 從等待中的 checkpoint 接手
    let successor = running("r2", "p3", "t1", 2);
    let (moved, _) = transfer(&again, &successor, saved.clone(), "exited", at(41)).unwrap();
    assert_eq!((moved.phase, moved.to_profile_id.as_str(), moved.to_run_id.as_deref()), (HandoffPhase::Transferred, "p3", Some("r2")));
    assert_eq!(moved.checkpoint, saved);
    // 等待中也可以放棄（例如使用者取消了 Task）
    assert_eq!(abandon(&again, "task_cancelled", at(42)).unwrap().0.phase, HandoffPhase::Abandoned);
}

#[test]
fn the_sudden_limit_prompt_does_not_rely_on_a_last_summary() {
    let run = running("r1", "p1", "t1", 1);
    let (handoff, _) = request("hof_1".into(), &run, "", GATE_REASON_LIMIT_REACHED, at(0));
    let prompt = successor_prompt(&handoff, None);
    assert!(prompt.contains("已達上限") && prompt.contains("最後一輪"), "{prompt}");
    assert!(prompt.contains("confirm_handoff"));
    assert!(!successor_prompt(&requested(&run, at(0)), None).contains("已達上限"));
}

#[test]
fn only_a_waiting_resource_task_is_resumed_to_ready() {
    let mut task = crate::task::Task {
        id: "t1".into(),
        workspace_id: "ws".into(),
        goal_id: "g1".into(),
        title: "整理資料".into(),
        kind: Default::default(),
        status: crate::task::TaskStatus::WaitingResource,
        version: 4,
        contract_version: 1,
        created_at: T0,
        updated_at: T0,
    };
    let (ready, events) = resume_task(&task, at(5)).unwrap();
    assert_eq!(ready.status, crate::task::TaskStatus::Ready);
    assert_eq!((ready.version, events.len()), (task.version + 1, 1));
    task.status = crate::task::TaskStatus::Cancelled;
    assert!(resume_task(&task, at(5)).is_none());
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
