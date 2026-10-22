use super::*;
use crate::session::{ResumeCapability, SessionState};

const T0: Timestamp = Timestamp::from_millis(1_000_000);
const DIR: &str = "C:\\work\\app";

fn at(offset_ms: i64) -> Timestamp {
    Timestamp::from_millis(T0.as_millis() + offset_ms)
}

/// 把變更套回狀態（模擬 Store 寫入後重新載入）。
fn apply(state: &mut ConversationState, change: &ConversationChange) {
    if let Some(conversation) = &change.conversation {
        state.conversation = conversation.clone();
    }
    for turn in &change.turns {
        match state.turns.iter_mut().find(|t| t.id == turn.id) {
            Some(existing) => *existing = turn.clone(),
            None => state.turns.push(turn.clone()),
        }
    }
    if let Some(run) = &change.run {
        state.latest_run = Some(run.clone());
    }
}

fn apply_sessions(sessions: &mut Vec<AgentSession>, change: &ConversationChange) {
    for session in &change.sessions {
        match sessions.iter_mut().find(|s| s.id == session.id) {
            Some(existing) => *existing = session.clone(),
            None => sessions.push(session.clone()),
        }
    }
}

/// 收到一則輸入，回傳它的識別。
fn input(state: &mut ConversationState, kind: TurnKind, text: &str, now: Timestamp) -> String {
    let change = receive(state, kind, text, None, now).unwrap();
    let id = change.turns[0].id.clone();
    apply(state, &change);
    id
}

fn begin(run_id: &str, check: Option<ResumeCheck>) -> BeginTurn {
    BeginTurn {
        run_id: run_id.into(),
        command_id: format!("cmd_{run_id}"),
        profile_id: "prof_a".into(),
        working_dir: DIR.into(),
        model: None,
        resume_check: check,
    }
}

#[test]
fn the_first_input_creates_the_conversation_and_later_inputs_only_append() {
    let mut state = start("ws_1", T0);
    let first = receive(&state, TurnKind::UserMessage, "  幫我整理需求  ", None, T0).unwrap();
    let types: Vec<_> = first.events.iter().map(|e| e.event_type).collect();
    assert_eq!(types, [EVENT_CONVERSATION_CREATED, EVENT_CONVERSATION_INPUT_RECEIVED]);
    assert_eq!(first.turns[0].content, "幫我整理需求");
    assert_eq!(first.turns[0].status, TurnStatus::Pending);
    assert_eq!(first.turns[0].seq, 1);
    assert_eq!(first.turns[0].id, "turn_1_1");
    assert_eq!(first.conversation.as_ref().unwrap().id, "conv_1");
    apply(&mut state, &first);
    let second = receive(&state, TurnKind::WorkEvent, "Run 成功", Some("run_succeeded:run_1".into()), T0).unwrap();
    assert_eq!(second.events.len(), 1);
    assert_eq!(second.turns[0].seq, 2);
    assert_eq!(second.conversation.unwrap().version, 3);
}

#[test]
fn blank_or_overlong_user_messages_are_rejected() {
    let state = start("ws_1", T0);
    assert_eq!(receive(&state, TurnKind::UserMessage, "   ", None, T0).unwrap_err(), ConversationError::MessageEmpty);
    let long = "字".repeat(MAX_MESSAGE_CHARS + 1);
    assert_eq!(receive(&state, TurnKind::UserMessage, &long, None, T0).unwrap_err(), ConversationError::MessageTooLong);
}

#[test]
fn without_pending_inputs_the_controller_is_not_woken() {
    let state = start("ws_1", T0);
    assert_eq!(plan_wake(&state, at(3_600_000)), WakePlan::Idle);
}

#[test]
fn inputs_within_the_merge_window_are_handled_in_one_turn() {
    let mut state = start("ws_1", T0);
    let u1 = input(&mut state, TurnKind::UserMessage, "第一則", T0);
    let u2 = input(&mut state, TurnKind::UserMessage, "第二則", at(3_000));
    assert_eq!(plan_wake(&state, at(4_999)), WakePlan::Wait { due_at: at(MERGE_WINDOW_MS) });
    assert_eq!(plan_wake(&state, at(5_000)), WakePlan::Wake { input_turn_ids: vec![u1.clone(), u2.clone()] });
}

#[test]
fn user_messages_go_before_work_events() {
    let mut state = start("ws_1", T0);
    input(&mut state, TurnKind::WorkEvent, "Run 成功", T0);
    let u1 = input(&mut state, TurnKind::UserMessage, "先做這個", at(1_000));
    assert_eq!(plan_wake(&state, at(5_000)), WakePlan::Wake { input_turn_ids: vec![u1.clone()] });
}

#[test]
fn a_busy_conversation_never_starts_a_second_turn() {
    let mut state = start("ws_1", T0);
    input(&mut state, TurnKind::UserMessage, "第一則", T0);
    let change = begin_turn(&state, &[], begin("ctl_1", None), at(5_000)).unwrap();
    apply(&mut state, &change);
    input(&mut state, TurnKind::UserMessage, "忙碌時送出", at(6_000));
    assert_eq!(plan_wake(&state, at(60_000)), WakePlan::Busy { run_id: "ctl_1".into() });
    let busy = begin_turn(&state, &[], begin("ctl_2", None), at(60_000)).unwrap_err();
    assert_eq!(busy, ConversationError::Busy("ctl_1".into()));
}

#[test]
fn begin_turn_persists_the_launch_intent_claims_the_inputs_and_creates_a_controller_session() {
    let mut state = start("ws_1", T0);
    let u1 = input(&mut state, TurnKind::UserMessage, "第一則", T0);
    let change = begin_turn(&state, &[], begin("ctl_1", None), at(5_000)).unwrap();
    let run = change.run.as_ref().unwrap();
    assert_eq!(run.status, ControllerRunStatus::Starting);
    assert_eq!(run.command_id, "cmd_ctl_1");
    assert_eq!(run.input_turn_ids, vec![u1.clone()]);
    assert!(run.prompt.contains("你是 Baton 這個 Workspace 的 Controller"));
    assert!(run.prompt.contains("1. 第一則"));
    assert_eq!(change.turns[0].status, TurnStatus::Processing);
    assert_eq!(change.turns[0].controller_run_id.as_deref(), Some("ctl_1"));
    assert_eq!(change.turns[0].attempts, 1);
    let session = &change.sessions[0];
    assert_eq!(session.role, crate::workspace::AgentRole::Controller);
    assert_eq!(session.conversation_id.as_deref(), Some("conv_1"));
    assert_eq!(session.task_id, None);
    assert_eq!(session.driver_run_id.as_deref(), Some("ctl_1"));
    assert_eq!(run.agent_session_id, session.id);
}

#[test]
fn the_merge_window_rejects_an_early_begin() {
    let mut state = start("ws_1", T0);
    input(&mut state, TurnKind::UserMessage, "第一則", T0);
    assert_eq!(begin_turn(&state, &[], begin("ctl_1", None), at(1_000)).unwrap_err(), ConversationError::NothingToWake);
}

/// 跑完一輪：喚起、回執、綁定原生 Session、成功結束。
fn complete_turn(
    state: &mut ConversationState,
    sessions: &mut Vec<AgentSession>,
    run_id: &str,
    check: Option<ResumeCheck>,
    native: &str,
    reply: &str,
    now: Timestamp,
) -> ControllerRun {
    let change = begin_turn(state, sessions, begin(run_id, check), now).unwrap();
    apply(state, &change);
    apply_sessions(sessions, &change);
    let run = state.latest_run.clone().unwrap();
    let process = ProcessRecord { pid: 1, created_at_millis: 1, command_hash: "h".into() };
    let change = record_receipt(&run, process, now).unwrap();
    apply(state, &change);
    let run = state.latest_run.clone().unwrap();
    let (change, outcome) = bind_native(&run, sessions, native, now).unwrap();
    assert_eq!(outcome, BindOutcome::Bound);
    apply(state, &change);
    apply_sessions(sessions, &change);
    let run = state.latest_run.clone().unwrap();
    let outcome = TurnOutcome::Succeeded { reply: reply.into() };
    let change = finish(state, &run, sessions, outcome, now).unwrap();
    apply(state, &change);
    apply_sessions(sessions, &change);
    state.latest_run.clone().unwrap()
}

#[test]
fn a_successful_turn_marks_its_inputs_processed_and_saves_the_reply() {
    let mut state = start("ws_1", T0);
    let mut sessions = Vec::new();
    let u1 = input(&mut state, TurnKind::UserMessage, "第一則", T0);
    let run = complete_turn(&mut state, &mut sessions, "ctl_1", None, "native_1", "收到，我先整理", at(5_000));
    assert_eq!(run.status, ControllerRunStatus::Succeeded);
    assert_eq!(run.native_session_id.as_deref(), Some("native_1"));
    let u1 = state.turns.iter().find(|t| t.id == u1).unwrap();
    assert_eq!((u1.status, u1.controller_run_id.as_deref()), (TurnStatus::Processed, Some("ctl_1")));
    let reply = state.turns.last().unwrap();
    assert_eq!((reply.kind, reply.content.as_str(), reply.seq), (TurnKind::ControllerReply, "收到，我先整理", 2));
    assert_eq!(sessions[0].state, SessionState::Suspended);
    assert_eq!(plan_wake(&state, at(3_600_000)), WakePlan::Idle);
    // 已結束的一輪不能再結束
    let again = finish(&state, &run, &sessions, TurnOutcome::Interrupted { reason: "x".into() }, at(6_000));
    assert!(matches!(again, Err(ConversationError::TransitionInvalid { .. })));
}

#[test]
fn a_resumable_session_is_resumed_and_the_prompt_only_carries_the_new_inputs() {
    let mut state = start("ws_1", T0);
    let mut sessions = Vec::new();
    input(&mut state, TurnKind::UserMessage, "第一則", T0);
    complete_turn(&mut state, &mut sessions, "ctl_1", None, "native_1", "好", at(5_000));
    input(&mut state, TurnKind::UserMessage, "第二則", at(10_000));
    let check = ResumeCheck { session_id: sessions[0].id.clone(), capability: ResumeCapability::Resumable };
    let change = begin_turn(&state, &sessions, begin("ctl_2", Some(check)), at(15_000)).unwrap();
    let run = change.run.unwrap();
    assert_eq!(run.agent_session_id, sessions[0].id);
    assert_eq!(run.resume_native_session_id.as_deref(), Some("native_1"));
    assert!(!run.prompt.contains("第一則"), "沿用原生 Session 時不重送已處理的對話：{}", run.prompt);
    assert!(run.prompt.contains("1. 第二則"));
}

#[test]
fn a_replacement_session_keeps_the_conversation_and_only_lists_pending_inputs_as_instructions() {
    let mut state = start("ws_1", T0);
    let mut sessions = Vec::new();
    input(&mut state, TurnKind::UserMessage, "第一則指示", T0);
    complete_turn(&mut state, &mut sessions, "ctl_1", None, "native_1", "已處理第一則", at(5_000));
    let u2 = input(&mut state, TurnKind::UserMessage, "第二則指示", at(10_000));
    let check = ResumeCheck { session_id: sessions[0].id.clone(), capability: ResumeCapability::NotResumable };
    let change = begin_turn(&state, &sessions, begin("ctl_2", Some(check)), at(15_000)).unwrap();
    let run = change.run.clone().unwrap();
    assert_eq!(run.conversation_id, "conv_1");
    assert_ne!(run.agent_session_id, sessions[0].id);
    assert_eq!(run.resume_native_session_id, None);
    let successor = change.sessions.iter().find(|s| s.id == run.agent_session_id).unwrap();
    assert_eq!(successor.predecessor_session_id.as_deref(), Some(sessions[0].id.as_str()));
    assert_eq!(successor.conversation_id.as_deref(), Some("conv_1"));
    // 已處理的只在背景區，本輪的指示只有第二則
    let (history, instructions) = run.prompt.split_once("## 本輪要處理的使用者訊息").unwrap();
    assert!(history.contains("使用者：第一則指示") && history.contains("Controller：已處理第一則"), "{}", run.prompt);
    assert!(history.contains("不要重新執行"));
    assert!(instructions.contains("1. 第二則指示"));
    assert!(!instructions.contains("第一則指示"));
    assert_eq!(run.input_turn_ids, vec![u2.clone()]);
}

#[test]
fn an_interrupted_turn_releases_its_inputs_for_redelivery_after_a_backoff() {
    let mut state = start("ws_1", T0);
    let u1 = input(&mut state, TurnKind::UserMessage, "第一則", T0);
    let change = begin_turn(&state, &[], begin("ctl_1", None), at(5_000)).unwrap();
    let mut sessions = change.sessions.clone();
    apply(&mut state, &change);
    let run = state.latest_run.clone().unwrap();
    let change =
        finish(&state, &run, &sessions, TurnOutcome::Interrupted { reason: "launch_outcome_unknown".into() }, at(6_000)).unwrap();
    assert_eq!(change.run.as_ref().unwrap().status, ControllerRunStatus::Interrupted);
    apply(&mut state, &change);
    apply_sessions(&mut sessions, &change);
    let released = &state.turns[0];
    assert_eq!((released.status, released.controller_run_id.as_deref(), released.attempts), (TurnStatus::Pending, None, 1));
    assert_eq!(plan_wake(&state, at(6_000 + RETRY_BACKOFF_BASE_MS - 1)), WakePlan::Wait { due_at: at(6_000 + RETRY_BACKOFF_BASE_MS) });
    assert_eq!(plan_wake(&state, at(6_000 + RETRY_BACKOFF_BASE_MS)), WakePlan::Wake { input_turn_ids: vec![u1.clone()] });
    let change = begin_turn(&state, &sessions, begin("ctl_2", None), at(6_000 + RETRY_BACKOFF_BASE_MS)).unwrap();
    assert_eq!(change.turns[0].attempts, 2);
    assert!(change.run.unwrap().prompt.contains("可能已部分處理"));
}

#[test]
fn the_retry_backoff_doubles_up_to_ten_minutes() {
    assert_eq!(retry_backoff_ms(1), 60_000);
    assert_eq!(retry_backoff_ms(2), 120_000);
    assert_eq!(retry_backoff_ms(4), 480_000);
    assert_eq!(retry_backoff_ms(5), RETRY_BACKOFF_MAX_MS);
    assert_eq!(retry_backoff_ms(50), RETRY_BACKOFF_MAX_MS);
}

#[test]
fn work_events_describe_worker_results_and_blockers_only() {
    let content = work_event_content(crate::run::EVENT_RUN_BLOCKED_REPORTED, "整理文件", "task_1", "run_1", Some("缺少權限"));
    assert_eq!(content.as_deref(), Some("Task「整理文件」（task_1）Worker 回報受阻（Run run_1）：缺少權限"));
    assert_eq!(work_event_content(crate::run::EVENT_RUN_CANCELLED, "x", "t", "r", None), None);
}
