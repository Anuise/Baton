use super::*;

const NOW: Timestamp = Timestamp::from_millis(1_000);

fn input(to: &[&str], subject: &str) -> SendInput {
    SendInput { to: to.iter().map(|t| t.to_string()).collect(), subject: subject.into(), ..SendInput::default() }
}

fn message(to: &[&str], subject: &str) -> NewMessage {
    match input(to, subject).classify().unwrap() {
        Classified::Message(m) => m,
        other => panic!("應該是一般訊息：{other:?}"),
    }
}

fn worker(task_id: &str, run_id: &str, generation: i64) -> Sender {
    let identity = MessageSender::Worker { run_id: run_id.into(), task_id: task_id.into(), generation };
    Sender { workspace_id: "w".into(), identity, is_current: true }
}

fn sender(is_current: bool) -> Sender {
    Sender { is_current, ..worker("task_a", "run_a", 1) }
}

fn controller() -> Sender {
    let identity = MessageSender::Controller { controller_run_id: "crun_1".into() };
    Sender { workspace_id: "w".into(), identity, is_current: true }
}

fn tasks() -> Vec<String> {
    ["task_a", "task_b", "task_c"].iter().map(|t| t.to_string()).collect()
}

fn decide(
    sender: &Sender,
    message: NewMessage,
    existing: Option<&StoredMessage>,
    parent: Option<&StoredMessage>,
) -> Result<SendDecision, MessageError> {
    let tasks = tasks();
    decide_send(&SendContext { sender, workspace_task_ids: &tasks, existing, parent }, message, "msg_gen".into(), NOW)
}

fn sent(message: NewMessage) -> StoredMessage {
    match decide(&sender(true), message, None, None).unwrap() {
        SendDecision::New(stored) => stored,
        other => panic!("應該是新訊息：{other:?}"),
    }
}

#[test]
fn a_plain_message_defaults_to_a_note_and_dedupes_recipients() {
    let m = message(&["task_b", " task_b ", "task_c"], "  介面確認  ");

    assert_eq!((m.kind, m.subject.as_str(), m.to.as_slice()), (MessageKind::Note, "介面確認", &["task_b".to_string(), "task_c".to_string()][..]));
}

#[test]
fn invalid_messages_are_rejected_with_a_stable_code() {
    let cases: Vec<(SendInput, MessageError)> = vec![
        (input(&["task_b"], "   "), MessageError::SubjectEmpty),
        (input(&["task_b"], &"字".repeat(SUBJECT_MAX_CHARS + 1)), MessageError::SubjectTooLong),
        (SendInput { body: "x".repeat(BODY_MAX_BYTES + 1), ..input(&["task_b"], "主旨") }, MessageError::BodyTooLarge),
        (SendInput { kind: Some("shout".into()), ..input(&["task_b"], "主旨") }, MessageError::KindUnknown("shout".into())),
        (input(&[], "主旨"), MessageError::NoRecipient),
        (input(&["  "], "主旨"), MessageError::NoRecipient),
        (input(&(0..=MAX_RECIPIENTS).map(|i| format!("t{i}")).collect::<Vec<_>>().iter().map(String::as_str).collect::<Vec<_>>(), "主旨"), MessageError::TooManyRecipients),
        (SendInput { message_id: Some("has space".into()), ..input(&["task_b"], "主旨") }, MessageError::IdInvalid),
        (SendInput { correlation_id: Some("x".repeat(ID_MAX_CHARS + 1)), ..input(&["task_b"], "主旨") }, MessageError::IdInvalid),
    ];
    for (raw, expected) in cases {
        assert_eq!(raw.classify().unwrap_err(), expected);
        assert_eq!(expected.code(), "message_invalid");
    }
}

#[test]
fn a_scope_changing_kind_or_field_is_classified_as_an_attempt_not_a_message() {
    let by_kind = SendInput { kind: Some("scope_change".into()), body: "把範圍擴大到全站".into(), ..input(&["task_b"], "擴大範圍") };
    let by_field = SendInput { extra_fields: vec!["new_scope".into(), "note".into()], ..input(&["task_b"], "x") };

    let Classified::ScopeChange(kind_attempt) = by_kind.classify().unwrap() else { panic!("應該是範圍變更嘗試") };
    let Classified::ScopeChange(field_attempt) = by_field.classify().unwrap() else { panic!("應該是範圍變更嘗試") };

    assert_eq!(kind_attempt.attempted_kind.as_deref(), Some("scope_change"));
    assert_eq!(kind_attempt.body_excerpt, "把範圍擴大到全站");
    assert_eq!(field_attempt.attempted_fields, ["new_scope"], "只記錄會改範圍的欄位");
}

#[test]
fn a_sent_message_starts_a_thread_and_a_correlation_with_one_pending_delivery_per_recipient() {
    let stored = sent(message(&["task_b", "task_c"], "問題"));

    assert_eq!(stored.message.message_id, "msg_gen");
    assert_eq!((stored.message.thread_id.as_str(), stored.message.correlation_id.as_str()), ("msg_gen", "msg_gen"));
    assert_eq!(stored.deliveries.len(), 2);
    assert!(stored.deliveries.iter().all(|d| d.state() == DeliveryState::Pending));
    assert_eq!(stored.message.sender, MessageSender::Worker { run_id: "run_a".into(), task_id: "task_a".into(), generation: 1 });
}

#[test]
fn a_client_message_id_is_kept() {
    let stored = sent(NewMessage { message_id: Some("m-1".into()), ..message(&["task_b"], "x") });

    assert_eq!(stored.message.message_id, "m-1");
    assert_eq!(stored.deliveries[0].message_id, "m-1");
}

#[test]
fn resending_the_same_message_id_with_the_same_content_is_a_duplicate() {
    let first = sent(NewMessage { message_id: Some("m-1".into()), ..message(&["task_c", "task_b"], "x") });

    // 收件人順序不同也算同一則；新的 Run（世代）重送也一樣
    let again = NewMessage { message_id: Some("m-1".into()), ..message(&["task_b", "task_c"], "x") };
    let decision = decide(&worker("task_a", "run_a2", 2), again, Some(&first), None);

    assert_eq!(decision, Ok(SendDecision::Duplicate(first)));
}

#[test]
fn reusing_a_message_id_for_different_content_or_another_sender_is_a_conflict() {
    let first = sent(NewMessage { message_id: Some("m-1".into()), ..message(&["task_b"], "x") });
    let different = NewMessage { message_id: Some("m-1".into()), ..message(&["task_b"], "y") };
    let other_sender = worker("task_b", "run_a", 1);
    let same_content = NewMessage { message_id: Some("m-1".into()), ..message(&["task_c"], "x") };

    assert_eq!(decide(&sender(true), different, Some(&first), None), Err(MessageError::IdConflict("m-1".into())));
    assert_eq!(decide(&other_sender, same_content, Some(&first), None), Err(MessageError::IdConflict("m-1".into())));
}

#[test]
fn recipients_must_be_other_tasks_of_the_same_workspace() {
    assert_eq!(decide(&sender(true), message(&["task_a"], "x"), None, None), Err(MessageError::SelfRecipient));
    assert_eq!(
        decide(&sender(true), message(&["task_b", "task_zzz"], "x"), None, None),
        Err(MessageError::RecipientNotFound("task_zzz".into()))
    );
}

#[test]
fn a_run_that_is_no_longer_the_writer_cannot_send() {
    assert_eq!(decide(&sender(false), message(&["task_b"], "x"), None, None), Err(MessageError::StaleSender));
    assert_eq!(MessageError::StaleSender.code(), "run_report_stale");
}

#[test]
fn a_reply_joins_the_thread_and_inherits_the_correlation() {
    let question = sent(NewMessage { correlation_id: Some("corr-7".into()), ..message(&["task_b"], "問題") });
    let reply = NewMessage { in_reply_to: Some("msg_gen".into()), ..message(&["task_a"], "回答") };

    let b = worker("task_b", "run_b", 1);
    let Ok(SendDecision::New(stored)) = decide(&b, reply, None, Some(&question)) else { panic!("回覆應該成功") };

    assert_eq!((stored.message.thread_id.as_str(), stored.message.correlation_id.as_str()), ("msg_gen", "corr-7"));
    assert_eq!(stored.message.in_reply_to.as_deref(), Some("msg_gen"));
}

#[test]
fn a_task_cannot_reply_to_a_message_it_is_not_part_of() {
    let question = sent(message(&["task_b"], "問題"));
    let outsider = worker("task_c", "run_c", 1);
    let reply = NewMessage { in_reply_to: Some("msg_gen".into()), ..message(&["task_a"], "插嘴") };

    assert_eq!(decide(&outsider, reply.clone(), None, Some(&question)), Err(MessageError::ReplyTargetNotFound("msg_gen".into())));
    assert_eq!(decide(&outsider, reply, None, None), Err(MessageError::ReplyTargetNotFound("msg_gen".into())));
}

#[test]
fn delivery_state_follows_delivered_and_read_times() {
    let t = Some(Timestamp::from_millis(1));
    assert_eq!(DeliveryState::of(None, None), DeliveryState::Pending);
    assert_eq!(DeliveryState::of(t, None), DeliveryState::Delivered);
    assert_eq!(DeliveryState::of(t, t), DeliveryState::Read);
    assert_eq!(DeliveryState::of(None, t), DeliveryState::Read, "直接讀取也算讀過");
}

#[test]
fn the_unread_summary_lists_subjects_but_never_bodies_and_notes_the_overflow() {
    assert_eq!(render_unread_summary(&[], 0), "");

    let items = vec![UnreadSummaryItem {
        message_id: "m1".into(),
        kind: MessageKind::Question,
        subject: "API 的欄位名稱".into(),
        from_task_title: Some("後端".into()),
    }];
    let text = render_unread_summary(&items, 3);

    assert!(text.contains("未讀訊息：3 則") && text.contains("read_inbox"));
    assert!(text.contains("[question] API 的欄位名稱（來自 Task「後端」，m1）"));
    assert!(text.contains("另有 2 則"));
}

#[test]
fn a_scope_change_attempt_becomes_a_controller_event_that_names_who_tried_what() {
    let attempt = ScopeChangeAttempt {
        attempted_kind: Some("reassign".into()),
        attempted_fields: vec!["owner".into()],
        recipients: vec!["task_b".into()],
        subject: "改歸屬".into(),
        body_excerpt: "請改成我負責".into(),
    };

    let event = attempt.into_event(&sender(true), "msg_attempt".into(), NOW);

    assert_eq!((event.aggregate_type, event.aggregate_id.as_str(), event.aggregate_version), ("message", "msg_attempt", 1));
    assert_eq!(event.event_type, EVENT_MESSAGE_SCOPE_CHANGE_IGNORED);
    assert_eq!(event.occurred_at, NOW);
    assert_eq!(event.payload["task_id"], "task_a");
    assert_eq!(event.payload["run_id"], "run_a");
    assert_eq!(event.payload["attempted_fields"], serde_json::json!(["owner"]));
    assert_eq!(event.payload["reason"], SCOPE_CHANGE_REASON);
}

// ---------------------------------------------------------------- Controller 的地址（票 54）

#[test]
fn the_controller_sends_from_its_reserved_address_to_tasks_of_its_workspace() {
    let Ok(SendDecision::New(stored)) = decide(&controller(), message(&["task_a", "task_b"], "分工"), None, None) else {
        panic!("Controller 應該可以寄給 Workspace 內的 Task")
    };

    assert_eq!(stored.message.sender.address(), CONTROLLER_ADDRESS);
    assert_eq!(stored.message.sender, MessageSender::Controller { controller_run_id: "crun_1".into() });
    assert_eq!(decide(&controller(), message(&[CONTROLLER_ADDRESS], "x"), None, None), Err(MessageError::SelfRecipient));
    assert_eq!(
        decide(&controller(), message(&["task_other_ws"], "x"), None, None),
        Err(MessageError::RecipientNotFound("task_other_ws".into()))
    );
}

#[test]
fn a_worker_can_write_to_the_controller_and_reply_to_what_the_controller_sent_it() {
    assert!(matches!(decide(&sender(true), message(&[CONTROLLER_ADDRESS], "回報"), None, None), Ok(SendDecision::New(_))));

    let Ok(SendDecision::New(question)) = decide(&controller(), message(&["task_a"], "問題"), None, None) else { panic!() };
    let reply = NewMessage { in_reply_to: Some("msg_gen".into()), ..message(&[CONTROLLER_ADDRESS], "回答") };
    assert!(matches!(decide(&sender(true), reply.clone(), None, Some(&question)), Ok(SendDecision::New(_))));
    // 沒有收到這則訊息的 Task 不能回覆它
    assert_eq!(
        decide(&worker("task_b", "run_b", 1), reply, None, Some(&question)),
        Err(MessageError::ReplyTargetNotFound("msg_gen".into()))
    );
}

#[test]
fn the_controller_can_reply_to_messages_sent_to_it_in_any_round() {
    let report = sent(message(&[CONTROLLER_ADDRESS], "回報"));
    let reply = NewMessage { in_reply_to: Some("msg_gen".into()), ..message(&["task_a"], "收到") };
    let next_round = Sender { identity: MessageSender::Controller { controller_run_id: "crun_2".into() }, ..controller() };

    let Ok(SendDecision::New(stored)) = decide(&next_round, reply, None, Some(&report)) else { panic!("應該可以回覆") };
    assert_eq!(stored.message.thread_id, "msg_gen");
}

#[test]
fn the_unread_summary_names_the_controller_as_sender() {
    let items = vec![UnreadSummaryItem {
        message_id: "m1".into(),
        kind: MessageKind::Note,
        subject: "先做甲".into(),
        from_task_title: None,
    }];

    assert!(render_unread_summary(&items, 1).contains("[note] 先做甲（來自 Controller，m1）"));
}
