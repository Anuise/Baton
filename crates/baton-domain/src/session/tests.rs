use super::*;

const T0: Timestamp = Timestamp::from_millis(1_000);
const T1: Timestamp = Timestamp::from_millis(2_000);
const DIR: &str = "C:\\work\\app";

fn session(id: &str, state: SessionState, native: Option<&str>) -> AgentSession {
    let driven = matches!(state, SessionState::Allocating | SessionState::Active);
    AgentSession {
        id: id.into(),
        runtime_type: RUNTIME_CLAUDE_CODE.into(),
        workspace_id: "ws".into(),
        role: AgentRole::Worker,
        task_id: Some("task".into()),
        conversation_id: None,
        profile_id: "prof_a".into(),
        working_directory: DIR.into(),
        native_session_id: native.map(Into::into),
        state,
        state_reason: None,
        version: 1,
        driver_run_id: driven.then(|| "run_old".to_string()),
        predecessor_session_id: None,
        replacement_reason: None,
        history_gap: None,
        resume_capability: None,
        capability_checked_at: None,
        created_at: T0,
        last_activity_at: T0,
    }
}

fn check(id: &str, capability: ResumeCapability) -> ResumeCheck {
    ResumeCheck { session_id: id.into(), capability }
}

fn plan(sessions: &[AgentSession], profile: &str, dir: &str, check: Option<&ResumeCheck>) -> SessionPlan {
    plan_launch(sessions, "run_new", "task", profile, dir, check).unwrap()
}

fn replaced(id: &str, reason: ReplacementReason) -> SessionPlan {
    SessionPlan::Create { predecessor: Some((id.into(), reason)) }
}

fn facts() -> LaunchFacts<'static> {
    LaunchFacts { run_id: "run_new", workspace_id: "ws", task_id: "task", profile_id: "prof_a", working_dir: DIR }
}

// ------------------------------------------------------------ 啟動時的決定

#[test]
fn a_task_without_sessions_gets_a_new_session_without_predecessor() {
    assert_eq!(plan(&[], "prof_a", DIR, None), SessionPlan::Create { predecessor: None });
}

#[test]
fn a_compatible_suspended_session_confirmed_resumable_is_resumed() {
    let sessions = [session("ses_1", SessionState::Suspended, Some("N1"))];
    let resumable = check("ses_1", ResumeCapability::Resumable);
    assert_eq!(resume_candidate(&sessions, "task", "prof_a", DIR).map(|s| s.id.as_str()), Some("ses_1"));
    assert_eq!(plan(&sessions, "prof_a", DIR, Some(&resumable)), SessionPlan::Resume { session_id: "ses_1".into() });
}

#[test]
fn only_the_latest_session_of_the_same_task_is_considered() {
    let mut other_task = session("ses_other", SessionState::Suspended, Some("N9"));
    other_task.task_id = Some("other".into());
    let sessions = [session("ses_1", SessionState::Suspended, Some("N1")), other_task];
    assert_eq!(latest_for_task(&sessions, "task").map(|s| s.id.as_str()), Some("ses_1"));
    let mut older = session("ses_0", SessionState::Unavailable, None);
    older.created_at = Timestamp::from_millis(0);
    let sessions = [older, session("ses_1", SessionState::Suspended, Some("N1"))];
    assert_eq!(latest_for_task(&sessions, "task").map(|s| s.id.as_str()), Some("ses_1"));
}

#[test]
fn a_session_that_is_not_resumable_or_unknown_is_replaced_and_never_claimed_resumable() {
    let sessions = [session("ses_1", SessionState::Suspended, Some("N1"))];
    let lost = check("ses_1", ResumeCapability::NotResumable);
    let unknown = check("ses_1", ResumeCapability::Unknown);
    let someone_else = check("ses_x", ResumeCapability::Resumable);
    assert_eq!(plan(&sessions, "prof_a", DIR, Some(&lost)), replaced("ses_1", ReplacementReason::NativeSessionUnavailable));
    assert_eq!(plan(&sessions, "prof_a", DIR, Some(&unknown)), replaced("ses_1", ReplacementReason::ResumeCapabilityUnknown));
    assert_eq!(plan(&sessions, "prof_a", DIR, None), replaced("ses_1", ReplacementReason::ResumeCapabilityUnknown));
    assert_eq!(
        plan(&sessions, "prof_a", DIR, Some(&someone_else)),
        replaced("ses_1", ReplacementReason::ResumeCapabilityUnknown)
    );
}

#[test]
fn incompatible_sessions_are_replaced_without_asking_the_adapter() {
    let resumable = check("ses_1", ResumeCapability::Resumable);
    let suspended = [session("ses_1", SessionState::Suspended, Some("N1"))];
    assert_eq!(resume_candidate(&suspended, "task", "prof_b", DIR), None);
    assert_eq!(plan(&suspended, "prof_b", DIR, Some(&resumable)), replaced("ses_1", ReplacementReason::AccountChanged));
    assert_eq!(resume_candidate(&suspended, "task", "prof_a", "C:\\other"), None);
    assert_eq!(
        plan(&suspended, "prof_a", "C:\\other", Some(&resumable)),
        replaced("ses_1", ReplacementReason::WorkingDirectoryChanged)
    );
    let mut never_bound = session("ses_1", SessionState::Unavailable, None);
    never_bound.driver_run_id = None;
    never_bound.state_reason = Some("native_session_never_bound".into());
    let never_bound = [never_bound];
    assert_eq!(resume_candidate(&never_bound, "task", "prof_a", DIR), None);
    assert_eq!(plan(&never_bound, "prof_a", DIR, None), replaced("ses_1", ReplacementReason::NativeSessionNeverBound));
    let mut closed_without_reason = session("ses_1", SessionState::Closed, Some("N1"));
    closed_without_reason.driver_run_id = None;
    assert_eq!(
        plan(&[closed_without_reason], "prof_a", DIR, None),
        replaced("ses_1", ReplacementReason::PredecessorNotResumable)
    );
    let mut suspended_without_native = session("ses_1", SessionState::Suspended, None);
    suspended_without_native.driver_run_id = None;
    assert_eq!(
        plan(&[suspended_without_native], "prof_a", DIR, None),
        replaced("ses_1", ReplacementReason::NativeSessionNeverBound)
    );
}

#[test]
fn a_session_still_driven_by_another_run_cannot_be_taken_over() {
    let sessions = [session("ses_1", SessionState::Active, Some("N1"))];
    let resumable = check("ses_1", ResumeCapability::Resumable);
    assert_eq!(resume_candidate(&sessions, "task", "prof_a", DIR).map(|s| s.id.as_str()), Some("ses_1"));
    assert_eq!(
        plan_launch(&sessions, "run_new", "task", "prof_a", DIR, Some(&resumable)),
        Err(SessionBusy { session_id: "ses_1".into(), run_id: "run_old".into() })
    );
}

// ------------------------------------------------------------ 套用

#[test]
fn resuming_marks_the_session_active_driven_by_the_new_run_and_returns_its_native_id() {
    let sessions = [session("ses_1", SessionState::Suspended, Some("N1"))];
    let resumable = check("ses_1", ResumeCapability::Resumable);
    let mut edits = SessionEdits::default();
    let plan = SessionPlan::Resume { session_id: "ses_1".into() };
    let (id, native) = apply_plan(&mut edits, &sessions, plan, Some(&resumable), &facts(), T1);
    assert_eq!((id.as_str(), native.as_deref()), ("ses_1", Some("N1")));
    let updated = &edits.sessions[0];
    assert_eq!(updated.state, SessionState::Active);
    assert_eq!(updated.driver_run_id.as_deref(), Some("run_new"));
    assert_eq!(updated.resume_capability, Some(ResumeCapability::Resumable));
    assert_eq!(updated.capability_checked_at, Some(T1));
    assert_eq!(updated.version, 2);
    assert_eq!(edits.events.iter().map(|e| e.event_type).collect::<Vec<_>>(), [EVENT_SESSION_RESUMED]);
}

#[test]
fn replacing_keeps_the_predecessor_id_marks_it_and_records_a_history_gap_on_the_new_session() {
    let sessions = [session("ses_1", SessionState::Suspended, Some("N1"))];
    let lost = check("ses_1", ResumeCapability::NotResumable);
    let mut edits = SessionEdits::default();
    let plan = replaced("ses_1", ReplacementReason::NativeSessionUnavailable);
    let (id, native) = apply_plan(&mut edits, &sessions, plan, Some(&lost), &facts(), T1);
    assert_eq!((id.as_str(), native), ("ses_new", None));

    let old = edits.sessions.iter().find(|s| s.id == "ses_1").unwrap();
    assert_eq!(old.state, SessionState::Unavailable);
    assert_eq!(old.state_reason.as_deref(), Some("native_session_unavailable"));
    assert_eq!(old.native_session_id.as_deref(), Some("N1"), "原本的原生識別不被覆蓋");
    assert_eq!(old.resume_capability, Some(ResumeCapability::NotResumable));

    let new = edits.sessions.iter().find(|s| s.id == "ses_new").unwrap();
    assert_eq!(new.state, SessionState::Allocating);
    assert_eq!(new.driver_run_id.as_deref(), Some("run_new"));
    assert_eq!(new.native_session_id, None);
    assert_eq!(new.predecessor_session_id.as_deref(), Some("ses_1"));
    assert_eq!(new.replacement_reason.as_deref(), Some("native_session_unavailable"));
    assert!(new.history_gap.as_deref().is_some_and(|g| g.contains("ses_1")));
    assert_eq!(new.version, 1);
    let types: Vec<_> = edits.events.iter().map(|e| e.event_type).collect();
    assert_eq!(types, [EVENT_SESSION_UNAVAILABLE, EVENT_SESSION_CREATED]);
}

#[test]
fn an_incompatible_predecessor_is_closed_and_an_already_closed_one_is_left_as_is() {
    let sessions = [session("ses_1", SessionState::Suspended, Some("N1"))];
    let mut edits = SessionEdits::default();
    apply_plan(&mut edits, &sessions, replaced("ses_1", ReplacementReason::AccountChanged), None, &facts(), T1);
    let old = edits.sessions.iter().find(|s| s.id == "ses_1").unwrap();
    assert_eq!((old.state, old.state_reason.as_deref()), (SessionState::Closed, Some("account_changed")));

    let mut closed = session("ses_1", SessionState::Closed, Some("N1"));
    closed.state_reason = Some("account_changed".into());
    let mut edits = SessionEdits::default();
    let plan = replaced("ses_1", ReplacementReason::PredecessorNotResumable);
    apply_plan(&mut edits, &[closed], plan, None, &facts(), T1);
    assert_eq!(edits.sessions.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["ses_new"]);
}

#[test]
fn the_new_session_id_is_derived_from_the_run_id() {
    assert_eq!(session_id_for_run("run_0a1b"), "ses_0a1b");
    assert_eq!(session_id_for_run("r1"), "ses_r1");
}

// ------------------------------------------------------------ 綁定與結束

#[test]
fn binding_is_conditional_first_report_binds_same_is_a_no_op_different_is_only_recorded() {
    let mut allocating = session("ses_1", SessionState::Allocating, None);
    allocating.driver_run_id = Some("run_1".into());
    let mut edits = SessionEdits::default();
    assert_eq!(bind_native(&mut edits, &[allocating], "ses_1", "run_1", "N1", T1), BindOutcome::Bound);
    let bound = edits.sessions[0].clone();
    assert_eq!((bound.state, bound.native_session_id.as_deref()), (SessionState::Active, Some("N1")));

    let mut edits = SessionEdits::default();
    assert_eq!(bind_native(&mut edits, &[bound.clone()], "ses_1", "run_1", "N1", T1), BindOutcome::Bound);
    assert!(edits.sessions.is_empty() && edits.events.is_empty());

    assert_eq!(bind_native(&mut edits, &[bound], "ses_1", "run_1", "N2", T1), BindOutcome::Mismatch);
    assert_eq!(edits.sessions[0].native_session_id.as_deref(), Some("N1"));
    assert_eq!(edits.events[0].event_type, EVENT_SESSION_NATIVE_CONFLICT);
}

#[test]
fn a_native_id_already_bound_in_the_same_account_is_not_bound_twice() {
    let mut mine = session("ses_2", SessionState::Allocating, None);
    mine.driver_run_id = Some("run_2".into());
    mine.task_id = Some("task_2".into());
    let theirs = session("ses_1", SessionState::Active, Some("N1"));
    let mut edits = SessionEdits::default();
    let outcome = bind_native(&mut edits, &[theirs.clone(), mine.clone()], "ses_2", "run_2", "N1", T1);
    assert_eq!(outcome, BindOutcome::InUse { other_session_id: "ses_1".into(), driven: true });
    assert_eq!(edits.sessions[0].native_session_id, None);
    assert_eq!(edits.events[0].event_type, EVENT_SESSION_NATIVE_CONFLICT);

    let mut idle = theirs.clone();
    idle.state = SessionState::Suspended;
    idle.driver_run_id = None;
    let mut edits = SessionEdits::default();
    let outcome = bind_native(&mut edits, &[idle, mine.clone()], "ses_2", "run_2", "N1", T1);
    assert_eq!(outcome, BindOutcome::InUse { other_session_id: "ses_1".into(), driven: false });

    // 別的帳號的同名原生識別屬於另一個命名空間
    let mut other_account = theirs;
    other_account.profile_id = "prof_b".into();
    let mut edits = SessionEdits::default();
    assert_eq!(bind_native(&mut edits, &[other_account, mine], "ses_2", "run_2", "N1", T1), BindOutcome::Bound);
}

#[test]
fn when_the_driving_run_ends_the_session_is_suspended_or_unavailable_if_never_bound() {
    let mut edits = SessionEdits::default();
    release_driver(&mut edits, &[session("ses_1", SessionState::Active, Some("N1"))], "ses_1", "run_old", T1);
    let suspended = &edits.sessions[0];
    assert_eq!((suspended.state, suspended.driver_run_id.clone()), (SessionState::Suspended, None));

    let mut edits = SessionEdits::default();
    release_driver(&mut edits, &[session("ses_1", SessionState::Allocating, None)], "ses_1", "run_old", T1);
    let unavailable = &edits.sessions[0];
    assert_eq!(unavailable.state, SessionState::Unavailable);
    assert_eq!(unavailable.state_reason.as_deref(), Some("native_session_never_bound"));

    let mut edits = SessionEdits::default();
    release_driver(&mut edits, &[session("ses_1", SessionState::Active, Some("N1"))], "ses_1", "run_other", T1);
    assert!(edits.sessions.is_empty(), "不是驅動者的 Run 結束不影響 Session");
}

// ------------------------------------------------------------ 對話紀錄

fn entries(positions: &[u64]) -> Vec<TranscriptEntry> {
    positions.iter().map(|p| TranscriptEntry { position: *p, line: format!("{{\"n\":{p}}}") }).collect()
}

#[test]
fn transcript_import_skips_positions_already_stored_and_duplicates() {
    let plans = plan_transcript_import(2, &entries(&[1, 2, 3, 3, 4]));
    assert_eq!(
        plans,
        [SegmentPlan::Stored {
            first_position: 3,
            last_position: 4,
            content: "{\"n\":3}\n{\"n\":4}\n".into(),
            entry_count: 2,
            redactions: 0
        }]
    );
    assert!(plan_transcript_import(4, &entries(&[1, 2, 3, 4])).is_empty());
}

#[test]
fn transcript_import_marks_skipped_positions_as_a_gap() {
    let plans = plan_transcript_import(0, &entries(&[1, 4, 5]));
    assert_eq!(plans.len(), 3);
    assert!(matches!(plans[0], SegmentPlan::Stored { first_position: 1, last_position: 1, .. }));
    assert_eq!(plans[1], SegmentPlan::Missing { first_position: 2, last_position: 3 });
    assert!(matches!(plans[2], SegmentPlan::Stored { first_position: 4, last_position: 5, .. }));
    assert_eq!(plan_transcript_import(3, &entries(&[6]))[0], SegmentPlan::Missing { first_position: 4, last_position: 5 });
}

#[test]
fn credentials_never_reach_the_stored_transcript() {
    let token = "0123456789abcdef".repeat(4);
    let key = "sk-ant-api03-abcdefghijklmnop";
    let lines = [
        format!(r#"{{"type":"user","message":{{"content":"echo $BATON_AGENT_TOKEN\n{token}"}}}}"#),
        format!(r#"{{"type":"assistant","text":"key={key} Authorization: Bearer abcdefghijklmnopqrstuvwxyz0123"}}"#),
        format!("plain text {token}"),
    ];
    let batch: Vec<TranscriptEntry> =
        lines.iter().enumerate().map(|(i, line)| TranscriptEntry { position: i as u64 + 1, line: line.clone() }).collect();
    let plans = plan_transcript_import(0, &batch);
    let SegmentPlan::Stored { content, redactions, .. } = &plans[0] else { panic!("{plans:?}") };
    assert!(!content.contains(&token), "{content}");
    assert!(!content.contains(key), "{content}");
    assert!(!content.contains("abcdefghijklmnopqrstuvwxyz0123"), "{content}");
    assert!(content.contains(REDACTED));
    assert!(*redactions >= 4, "{redactions}");
    // 遮蔽後的 JSON 行仍是合法 JSON
    for line in content.lines().take(2) {
        serde_json::from_str::<serde_json::Value>(line).unwrap();
    }
}

#[test]
fn ordinary_lines_are_kept_verbatim() {
    let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"完成：commit 0e35b07"}]}}"#;
    assert_eq!(redact_line(line), (line.to_string(), 0));
    assert_eq!(redact_line("hello world\r\n"), ("hello world".to_string(), 0));
}
