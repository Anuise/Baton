use serde_json::json;

use super::*;

const AT: Timestamp = Timestamp::from_millis(5_000);

fn facts(aggregate_type: &str, aggregate_id: &str, event_type: &str, payload: serde_json::Value) -> EventFacts {
    EventFacts {
        seq: 42,
        aggregate_type: aggregate_type.into(),
        aggregate_id: aggregate_id.into(),
        event_type: event_type.into(),
        occurred_at: AT,
        payload,
        task_id: Some("task_1".into()),
        task_title: Some("整理文件".into()),
        question: None,
    }
}

#[test]
fn a_finished_worker_run_becomes_a_run_ended_event() {
    let event = describe(&facts(run::RUN_AGGREGATE, "run_1", run::EVENT_RUN_SUCCEEDED, json!({ "reason": "exit_0" }))).unwrap();

    assert_eq!(event.kind, ControllerEventKind::RunEnded);
    assert_eq!((event.cursor, event.occurred_at), (42, AT));
    assert_eq!((event.task_id.as_deref(), event.run_id.as_deref()), (Some("task_1"), Some("run_1")));
    assert!(event.summary.contains("整理文件") && event.summary.contains("run_1"), "{}", event.summary);
    assert!(event.summary.contains("成功"), "{}", event.summary);
}

#[test]
fn failed_cancelled_and_interrupted_runs_are_also_endings_and_carry_the_reason() {
    for (event_type, word) in [
        (run::EVENT_RUN_FAILED, "失敗"),
        (run::EVENT_RUN_CANCELLED, "取消"),
        (run::EVENT_RUN_INTERRUPTED, "中斷"),
    ] {
        let event = describe(&facts(run::RUN_AGGREGATE, "run_1", event_type, json!({ "reason": "缺少權限" }))).unwrap();
        assert_eq!(event.kind, ControllerEventKind::RunEnded, "{event_type}");
        assert!(event.summary.contains(word) && event.summary.contains("缺少權限"), "{}", event.summary);
    }
}

#[test]
fn other_run_events_are_not_reported() {
    for event_type in [run::EVENT_RUN_STARTED, run::EVENT_RUN_PROGRESS_REPORTED, run::EVENT_RUN_HANDED_OFF] {
        assert_eq!(describe(&facts(run::RUN_AGGREGATE, "run_1", event_type, json!({}))), None, "{event_type}");
    }
}

#[test]
fn a_task_turning_blocked_is_reported_but_other_status_changes_are_not() {
    let blocked = facts(task::TASK_AGGREGATE, "task_1", task::EVENT_STATUS_CHANGED, json!({ "from": "running", "to": "blocked", "reason": "run_blocked_reported" }));
    let event = describe(&blocked).unwrap();
    assert_eq!(event.kind, ControllerEventKind::TaskBlocked);
    assert_eq!((event.task_id.as_deref(), event.run_id.as_deref()), (Some("task_1"), None));
    assert!(event.summary.contains("整理文件") && event.summary.contains("受阻"), "{}", event.summary);

    let ready = facts(task::TASK_AGGREGATE, "task_1", task::EVENT_STATUS_CHANGED, json!({ "from": "running", "to": "ready" }));
    assert_eq!(describe(&ready), None);
}

#[test]
fn an_answered_decision_request_is_reported_with_its_question_and_answer() {
    let mut answered = facts(
        decision::DECISION_AGGREGATE,
        "decision_1",
        decision::EVENT_DECISION_ANSWERED,
        json!({ "action": "choose", "option_id": "keep", "note": "先保留" }),
    );
    answered.question = Some("要不要保留舊格式？".into());
    let event = describe(&answered).unwrap();

    assert_eq!(event.kind, ControllerEventKind::DecisionAnswered);
    assert_eq!(event.decision_request_id.as_deref(), Some("decision_1"));
    for part in ["要不要保留舊格式？", "decision_1", "keep", "先保留"] {
        assert!(event.summary.contains(part), "{part}：{}", event.summary);
    }
    let requested = facts(decision::DECISION_AGGREGATE, "decision_1", decision::EVENT_DECISION_REQUESTED, json!({}));
    assert_eq!(describe(&requested), None);
}

#[test]
fn a_message_to_the_controller_is_reported_and_points_to_read_inbox() {
    let stored = message_received_event("ws_1", 3, "msg_1", MessageKind::Question, "介面要用哪一版？", Some(("task_2", "做介面")), AT);
    assert_eq!((stored.aggregate_type, stored.aggregate_id.as_str(), stored.aggregate_version), (CONTROLLER_INBOX_AGGREGATE, "ws_1", 3));
    assert_eq!(stored.event_type, EVENT_CONTROLLER_MESSAGE_RECEIVED);

    let mut read_back = facts(CONTROLLER_INBOX_AGGREGATE, "ws_1", EVENT_CONTROLLER_MESSAGE_RECEIVED, stored.payload.clone());
    read_back.task_id = None;
    read_back.task_title = None;
    let event = describe(&read_back).unwrap();

    assert_eq!(event.kind, ControllerEventKind::MessageReceived);
    assert_eq!((event.message_id.as_deref(), event.task_id.as_deref()), (Some("msg_1"), Some("task_2")));
    for part in ["介面要用哪一版？", "做介面", "read_inbox"] {
        assert!(event.summary.contains(part), "{part}：{}", event.summary);
    }
}

#[test]
fn the_timeout_defaults_and_has_an_upper_bound() {
    assert_eq!(timeout_seconds(None), Ok(DEFAULT_TIMEOUT_SECONDS));
    assert_eq!(timeout_seconds(Some(0)), Ok(0));
    assert_eq!(timeout_seconds(Some(MAX_TIMEOUT_SECONDS)), Ok(MAX_TIMEOUT_SECONDS));
    assert_eq!(timeout_seconds(Some(MAX_TIMEOUT_SECONDS + 1)), Err(WaitError::TimeoutTooLong));
    assert_eq!(WaitError::TimeoutTooLong.code(), "wait_timeout_invalid");
}

#[test]
fn acknowledging_only_moves_the_cursor_forward_and_never_past_existing_events() {
    assert_eq!(acknowledge(10, 15, 20), Ok(15));
    assert_eq!(acknowledge(10, 5, 20), Ok(10), "較舊的確認不倒退");
    assert_eq!(acknowledge(10, 20, 20), Ok(20));
    assert_eq!(acknowledge(10, 21, 20), Err(WaitError::CursorInvalid));
    assert_eq!(acknowledge(10, -1, 20), Err(WaitError::CursorInvalid));
    assert_eq!(WaitError::CursorInvalid.code(), "wait_cursor_invalid");
}

#[test]
fn a_new_controller_starts_where_the_previous_one_left_off_otherwise_where_it_opened() {
    assert_eq!(initial_cursor(Some(30), Some(50), 90), 30);
    assert_eq!(initial_cursor(None, Some(50), 90), 50);
    assert_eq!(initial_cursor(None, None, 90), 90);
}

#[test]
fn rendering_lists_each_event_and_says_they_are_confirmed() {
    let event = describe(&facts(run::RUN_AGGREGATE, "run_1", run::EVENT_RUN_SUCCEEDED, json!({}))).unwrap();
    let text = render_events(&[event.clone(), ControllerEvent { cursor: 43, ..event }]);

    assert!(text.starts_with("Baton 事件（2 件）"), "{text}");
    assert_eq!(text.matches("[Run 結束]").count(), 2, "{text}");
    assert!(text.trim_end().ends_with("再執行 wait 等待下一批事件。"), "{text}");
}

#[test]
fn wait_response_round_trips_through_json() {
    let event = describe(&facts(run::RUN_AGGREGATE, "run_1", run::EVENT_RUN_FAILED, json!({ "reason": "x" }))).unwrap();
    let response = WaitResponse { events: vec![event], cursor: 42, timed_out: false };
    let json = serde_json::to_value(&response).unwrap();
    assert_eq!(json["events"][0]["kind"], "run_ended");
    assert_eq!(serde_json::from_value::<WaitResponse>(json).unwrap(), response);
}
