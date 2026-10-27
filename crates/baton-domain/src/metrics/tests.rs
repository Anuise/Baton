use serde_json::json;

use super::*;
use crate::timeline::category_of;
use crate::Timestamp;

/// 依序產生時間線事件；`seq` 自動遞增。
#[derive(Default)]
struct Events(Vec<TimelineEntry>);

impl Events {
    fn push(&mut self, at: i64, aggregate_type: &str, aggregate_id: &str, event_type: &str, payload: serde_json::Value) -> &mut Self {
        let category = category_of(aggregate_type, event_type).expect("測試事件必須在時間線上");
        self.0.push(TimelineEntry {
            seq: self.0.len() as i64 + 1,
            occurred_at: Timestamp::from_millis(at),
            category,
            aggregate_type: aggregate_type.into(),
            aggregate_id: aggregate_id.into(),
            event_type: event_type.into(),
            task_ids: Vec::new(),
            run_id: None,
            payload,
        });
        self
    }

    fn task(&mut self, id: &str, statuses: &[&str]) -> &mut Self {
        self.push(0, task::TASK_AGGREGATE, id, task::EVENT_TASK_CREATED, json!({}));
        for to in statuses {
            self.push(0, task::TASK_AGGREGATE, id, task::EVENT_STATUS_CHANGED, json!({ "to": to }));
        }
        self
    }
}

#[test]
fn no_events_give_no_data_instead_of_zero_rates() {
    let metrics = compute(&[]);
    assert_eq!(metrics.events, 0);
    assert_eq!(metrics.task_completion.completion.rate, None);
    assert_eq!(metrics.handoff.success.rate, None);
    assert_eq!((metrics.recovery.average_ms, metrics.decision_wait.max_ms), (None, None));
    assert_eq!(metrics.lesson_recurrence.rate.rate, None);
    assert_eq!((metrics.context.average_tokens, metrics.usage.total_tokens), (None, 0));
}

#[test]
fn completion_rate_counts_final_status_and_excludes_cancelled_tasks() {
    let mut e = Events::default();
    e.task("t1", &["ready", "running", "verifying", "completed"])
        .task("t2", &["ready", "running", "ready"])
        .task("t3", &["cancelled"])
        .task("t4", &[]);
    let metrics = compute(&e.0);
    assert_eq!(metrics.task_completion.completion, Ratio { numerator: 1, denominator: 3, rate: Some(1.0 / 3.0) });
    assert_eq!(metrics.task_completion.cancelled, 1);
}

#[test]
fn handoff_success_is_transferred_over_finished_handoffs() {
    let mut e = Events::default();
    let h = handoff::HANDOFF_AGGREGATE;
    for id in ["h1", "h2", "h3"] {
        e.push(0, h, id, handoff::EVENT_HANDOFF_REQUESTED, json!({}));
    }
    e.push(1, h, "h1", handoff::EVENT_HANDOFF_STOPPING, json!({}))
        .push(2, h, "h1", handoff::EVENT_HANDOFF_TRANSFERRED, json!({}))
        .push(3, h, "h1", handoff::EVENT_HANDOFF_CONFIRMED, json!({}))
        .push(4, h, "h2", handoff::EVENT_HANDOFF_ABANDONED, json!({}));
    let metrics = compute(&e.0);
    assert_eq!(metrics.handoff.success, Ratio { numerator: 1, denominator: 2, rate: Some(0.5) });
    assert_eq!((metrics.handoff.abandoned, metrics.handoff.confirmed, metrics.handoff.in_flight), (1, 1, 1));
}

#[test]
fn recovery_duration_runs_from_detection_to_the_successful_attempt_finishing_its_work() {
    let mut e = Events::default();
    let r = recovery::RECOVERY_AGGREGATE;
    e.push(1_000, r, "rec1", recovery::EVENT_RUNTIME_CRASH_DETECTED, json!({ "round": 1 }))
        .push(1_000, r, "rec1", recovery::EVENT_RECOVERY_ATTEMPT_STARTED, json!({ "attempt_id": "a1" }))
        .push(2_000, r, "rec1", recovery::EVENT_RECOVERY_ATTEMPT_COMPLETED, json!({ "attempt_id": "a1" }))
        // 觀察期內再次崩潰：第一次失敗，第二次才成功
        .push(3_000, r, "rec1", recovery::EVENT_RECOVERY_ATTEMPT_FAILED, json!({ "attempt_id": "a1" }))
        .push(33_000, r, "rec1", recovery::EVENT_RECOVERY_ATTEMPT_STARTED, json!({ "attempt_id": "a2" }))
        .push(41_000, r, "rec1", recovery::EVENT_RECOVERY_ATTEMPT_COMPLETED, json!({ "attempt_id": "a2" }))
        .push(641_000, r, "rec1", recovery::EVENT_RECOVERY_SUCCEEDED, json!({ "attempt_id": "a2" }))
        .push(700_000, r, "rec2", recovery::EVENT_RUNTIME_CRASH_DETECTED, json!({ "round": 1 }));
    let metrics = compute(&e.0);
    assert_eq!(metrics.recovery, DurationStats { count: 1, pending: 1, average_ms: Some(40_000), max_ms: Some(40_000) });
}

#[test]
fn duplicate_operations_count_each_blocked_registration_by_status() {
    let mut e = Events::default();
    let blocked = |status: &str| json!({ "status": status, "operation_id": "op1" });
    let d = external_operation::EXTERNAL_OPERATION_DUPLICATE_AGGREGATE;
    let x = external_operation::EXTERNAL_OPERATION_AGGREGATE;
    e.push(0, x, "op1", external_operation::EVENT_EXTERNAL_OPERATION_BEGUN, json!({}))
        .push(0, d, "dup1", external_operation::EVENT_EXTERNAL_OPERATION_DUPLICATE_BLOCKED, blocked("in_progress_elsewhere"))
        .push(0, x, "op1", external_operation::EVENT_EXTERNAL_OPERATION_RESULT_UNKNOWN, json!({}))
        .push(0, d, "dup2", external_operation::EVENT_EXTERNAL_OPERATION_DUPLICATE_BLOCKED, blocked("result_unknown"))
        .push(0, d, "dup3", external_operation::EVENT_EXTERNAL_OPERATION_DUPLICATE_BLOCKED, blocked("already_succeeded"))
        .push(0, d, "dup4", external_operation::EVENT_EXTERNAL_OPERATION_DUPLICATE_BLOCKED, blocked("already_succeeded"));
    let metrics = compute(&e.0);
    assert_eq!(
        metrics.duplicate_operations,
        DuplicateOperations { blocked: 4, already_succeeded: 2, result_unknown: 1, in_progress_elsewhere: 1, unknown_outcomes: 1 }
    );
}

#[test]
fn decision_wait_is_request_to_first_answer_and_superseded_requests_are_not_pending() {
    let mut e = Events::default();
    let d = decision::DECISION_AGGREGATE;
    e.push(1_000, d, "q1", decision::EVENT_DECISION_REQUESTED, json!({}))
        .push(5_000, d, "q1", decision::EVENT_DECISION_DEFERRED, json!({}))
        .push(61_000, d, "q1", decision::EVENT_DECISION_ANSWERED, json!({}))
        .push(2_000, d, "q2", decision::EVENT_DECISION_REQUESTED, json!({}))
        .push(22_000, d, "q2", decision::EVENT_DECISION_ANSWERED, json!({}))
        .push(3_000, d, "q3", decision::EVENT_DECISION_REQUESTED, json!({}))
        .push(4_000, d, "q4", decision::EVENT_DECISION_REQUESTED, json!({}))
        .push(9_000, d, "q4", decision::EVENT_DECISION_SUPERSEDED, json!({}));
    let metrics = compute(&e.0);
    assert_eq!(metrics.decision_wait, DurationStats { count: 2, pending: 1, average_ms: Some(40_000), max_ms: Some(60_000) });
}

#[test]
fn lesson_recurrence_rate_is_tasks_with_a_recurrence_over_created_tasks() {
    let mut e = Events::default();
    e.task("t1", &[]).task("t2", &[]).task("t3", &[]).task("t4", &[]);
    let r = guard::RECURRENCE_AGGREGATE;
    let ev = guard::EVENT_RECURRENCE_RECORDED;
    e.push(0, r, "rec-1", ev, json!({ "task_id": "t1", "kind": "prevented" }))
        .push(0, r, "rec-2", ev, json!({ "task_id": "t1", "kind": "reported" }))
        .push(0, r, "rec-3", ev, json!({ "task_id": "t2", "kind": "prevented" }))
        .push(0, r, "rec-4", ev, json!({ "task_id": null, "kind": "reported" }));
    let metrics = compute(&e.0);
    assert_eq!(metrics.lesson_recurrence.rate, Ratio { numerator: 2, denominator: 4, rate: Some(0.5) });
    assert_eq!((metrics.lesson_recurrence.recurrences, metrics.lesson_recurrence.prevented, metrics.lesson_recurrence.reported), (4, 2, 2));
}

#[test]
fn usage_sums_per_stage_and_context_is_per_run_with_unknown_kept_apart() {
    let mut e = Events::default();
    let u = quota::USAGE_AGGREGATE;
    let ev = quota::EVENT_RUN_USAGE_REPORTED;
    e.push(0, u, "r1", ev, json!({ "tokens": 1_000, "context_tokens": 40_000 }))
        .push(0, u, "r1", ev, json!({ "tokens": 500, "context_tokens": 20_000 }))
        .push(0, u, "r2", ev, json!({ "tokens": 300, "context_tokens": 30_000 }))
        .push(0, u, "r3", ev, json!({ "tokens": 200, "context_tokens": null }));
    let metrics = compute(&e.0);
    assert_eq!(metrics.usage.total_tokens, 2_000);
    assert_eq!(metrics.usage.runs, 3);
    assert_eq!(metrics.usage.by_stage, [StageUsage { stage: None, tokens: 2_000, runs: 3 }]);
    assert_eq!(
        metrics.context,
        ContextUsage { runs_known: 2, runs_unknown: 1, average_tokens: Some(45_000), max_tokens: Some(60_000) }
    );
}

#[test]
fn the_same_events_always_give_the_same_metrics() {
    let mut e = Events::default();
    e.task("t1", &["completed"]).push(0, quota::USAGE_AGGREGATE, "r1", quota::EVENT_RUN_USAGE_REPORTED, json!({ "tokens": 7 }));
    let roundtrip: Vec<TimelineEntry> = serde_json::from_value(serde_json::to_value(&e.0).unwrap()).unwrap();
    assert_eq!(compute(&e.0), compute(&roundtrip));
    assert_eq!(compute(&e.0).events, 3);
}
