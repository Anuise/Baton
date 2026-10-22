use super::*;
use crate::workspace::{Workspace, WorkspaceStatus};

const T0: Timestamp = Timestamp::from_millis(1_000);
const T1: Timestamp = Timestamp::from_millis(2_000);

fn option(id: &str) -> DecisionOption {
    DecisionOption { id: id.into(), label: format!("選項 {id}"), tradeoff: None }
}

fn content(options: Vec<DecisionOption>, recommendation: Option<&str>) -> Result<RequestContent, DecisionError> {
    RequestContent::new("要用哪個方案？", "兩個方案的取捨取決於使用者價值", options, recommendation, vec!["證據".into()], None, None)
}

fn workspace() -> Workspace {
    Workspace {
        id: "ws".into(),
        name: "ws".into(),
        path: "C:\\ws".into(),
        status: WorkspaceStatus::Active,
        version: 1,
        created_at: T0,
        archived_at: None,
        model_overrides: Default::default(),
    }
}

fn task(id: &str, status: TaskStatus, contract_version: i64) -> Task {
    Task {
        id: id.into(),
        workspace_id: "ws".into(),
        goal_id: "g".into(),
        title: format!("工作 {id}"),
        kind: crate::workdir::TaskKind::default(),
        status,
        version: 1,
        contract_version,
        created_at: T0,
        updated_at: T0,
    }
}

fn pending(id: &str, kind: DecisionKind, task_id: &str) -> DecisionRequest {
    DecisionRequest {
        id: id.into(),
        workspace_id: "ws".into(),
        run_id: format!("run_{task_id}"),
        task_id: task_id.into(),
        kind,
        status: DecisionStatus::Pending,
        question: format!("問題 {id}"),
        reason: "原因".into(),
        options: vec![option("a"), option("b")],
        recommendation: None,
        evidence: vec![],
        urgency: Urgency::Normal,
        deadline_at: None,
        permission: None,
        authority: None,
        external_operation_id: None,
        blocking: vec![task_id.into()],
        target: DecisionTarget { task_id: task_id.into(), contract_version: 1, authorization_revision: None },
        revision: 1,
        event_version: 1,
        deferred_until: None,
        merged_into: None,
        status_reason: None,
        links: vec![],
        created_at: T0,
        updated_at: T0,
    }
}

/// 只有 Task 與裁定請求的世界（處理、合併、連結、重新整理都不需要 Run）。
struct World {
    workspace: Workspace,
    tasks: Vec<Task>,
    requests: Vec<DecisionRequest>,
    decisions: Vec<Decision>,
}

impl World {
    fn new(tasks: Vec<Task>, requests: Vec<DecisionRequest>) -> Self {
        Self { workspace: workspace(), tasks, requests, decisions: vec![] }
    }

    fn run(&self, command: DecisionCommand) -> Result<DecisionChange, DecisionError> {
        let task_ctx = TaskContext {
            workspace_id: "ws",
            workspace: Some(&self.workspace),
            authorization: None,
            goals: &[],
            tasks: &self.tasks,
            dependencies: &[],
            contracts: &[],
        };
        let policy = AuthorityPolicy::default();
        let ctx = DecisionContext {
            task: &task_ctx,
            runs: &[],
            leases: &[],
            requests: &self.requests,
            decisions: &self.decisions,
            uses: &[],
            authority: &policy,
            artifacts: &[],
            authorizations: &[],
        };
        decide(&ctx, command, T1)
    }
}

fn answer(request_id: &str, expected_revision: i64, action: DecisionAction, option: Option<&str>, note: Option<&str>) -> DecisionCommand {
    DecisionCommand::Answer {
        request_id: request_id.into(),
        expected_revision,
        resolution: Resolution {
            action,
            option_id: option.map(str::to_string),
            note: note.map(str::to_string),
            defer_minutes: None,
        },
        decided_by: DECIDED_BY_DESKTOP.into(),
        batch_id: None,
    }
}

#[test]
fn request_content_is_validated_and_trimmed() {
    let ok = content(vec![option("a"), option("b")], Some(" a ")).unwrap();
    assert_eq!(ok.recommendation.as_deref(), Some("a"));
    assert_eq!(ok.urgency, Urgency::Normal);

    let bad = |r: Result<RequestContent, DecisionError>| assert_eq!(r.unwrap_err().code(), "decision_invalid");
    bad(content(vec![option("a")], None));
    bad(content(vec![option("a"), option("a")], None));
    bad(content(vec![option("a"), option("b c")], None));
    bad(content(vec![option("a"), option("b")], Some("zzz")));
    bad(RequestContent::new(" ", "原因", vec![option("a"), option("b")], None, vec![], None, None));
    bad(RequestContent::new("問題", "", vec![option("a"), option("b")], None, vec![], None, None));
    bad(RequestContent::new("問題", "原因", vec![option("a"), option("b")], None, vec![], None, Some(0)));
    bad(RequestContent::new("問題", "原因", vec![option("a"), option("b")], None, vec!["x".into(); 11], None, None));
    bad(RequestContent::new(&"長".repeat(501), "原因", vec![option("a"), option("b")], None, vec![], None, None));
    bad(content((0..9).map(|i| option(&format!("o{i}"))).collect(), None));
}

#[test]
fn overdue_and_deferred_are_only_labels_for_pending_requests() {
    let mut request = pending("d1", DecisionKind::Agent, "t");
    request.deadline_at = Some(Timestamp::from_millis(1_000));
    request.deferred_until = Some(Timestamp::from_millis(5_000));
    assert!(!request.is_overdue(Timestamp::from_millis(1_000)));
    assert!(request.is_overdue(Timestamp::from_millis(1_001)));
    assert!(request.is_deferred(Timestamp::from_millis(4_999)));
    assert!(!request.is_deferred(Timestamp::from_millis(5_000)));
    request.status = DecisionStatus::Answered;
    assert!(!request.is_overdue(Timestamp::from_millis(9_999)));
    assert!(!request.is_deferred(Timestamp::from_millis(1_000)));
}

#[test]
fn similar_questions_share_a_key_regardless_of_case_spacing_and_punctuation() {
    let mut a = pending("a", DecisionKind::Agent, "t1");
    let mut b = pending("b", DecisionKind::Agent, "t2");
    a.question = "要用哪個資料來源？".into();
    b.question = " 要用 哪個資料來源 ".into();
    assert_eq!(a.similarity_key(), b.similarity_key());
    b.question = "Use API v2?".into();
    a.question = "use api V2".into();
    assert_eq!(a.similarity_key(), b.similarity_key());
    // 不同種類即使問題相同也不相似
    let mut c = pending("c", DecisionKind::Authority, "t3");
    c.question = a.question.clone();
    assert_ne!(a.similarity_key(), c.similarity_key());
}

#[test]
fn a_stale_revision_is_refused_without_any_change() {
    let world = World::new(vec![task("t", TaskStatus::WaitingDecision, 1)], vec![pending("d", DecisionKind::Agent, "t")]);
    let refused = world.run(answer("d", 0, DecisionAction::Accept, Some("a"), None)).unwrap_err();
    assert_eq!(refused, DecisionError::RevisionMismatch { current: 1 });
}

#[test]
fn a_request_whose_target_contract_changed_is_superseded_instead_of_applied() {
    let world = World::new(vec![task("t", TaskStatus::WaitingDecision, 2)], vec![pending("d", DecisionKind::Agent, "t")]);
    let change = world.run(answer("d", 1, DecisionAction::Accept, Some("a"), None)).unwrap();

    let DecisionOutcome::Superseded { reason, resumed_task_ids, .. } = &change.outcome else { panic!("應該過期：{:?}", change.outcome) };
    assert!(reason.contains("版本 1"), "{reason}");
    assert_eq!(resumed_task_ids, &["t".to_string()]);
    assert!(change.decision.is_none(), "過期的請求不能套用");
    assert_eq!(change.updated[0].status, DecisionStatus::Superseded);
    assert_eq!(change.updated[0].revision, 2);
    assert_eq!(change.outbox.len(), 1);
}

#[test]
fn refresh_supersedes_only_requests_whose_target_changed() {
    let world = World::new(
        vec![task("t1", TaskStatus::WaitingDecision, 2), task("t2", TaskStatus::WaitingDecision, 1), task("t3", TaskStatus::Cancelled, 1)],
        vec![pending("d1", DecisionKind::Agent, "t1"), pending("d2", DecisionKind::Agent, "t2"), pending("d3", DecisionKind::Agent, "t3")],
    );
    let change = world.run(DecisionCommand::Refresh).unwrap();
    assert_eq!(change.outcome, DecisionOutcome::Refreshed { superseded: vec!["d1".into(), "d3".into()] });
    assert_eq!(change.resumed, vec!["t1".to_string()], "已取消的 Task 不會恢復");
}

#[test]
fn each_action_validates_its_option_and_text() {
    let world = World::new(vec![task("t", TaskStatus::WaitingDecision, 1)], vec![pending("d", DecisionKind::Agent, "t")]);
    let code = |cmd| world.run(cmd).unwrap_err().code();
    assert_eq!(code(answer("d", 1, DecisionAction::Modify, Some("a"), None)), "decision_invalid");
    assert_eq!(code(answer("d", 1, DecisionAction::Answer, None, Some("  "))), "decision_invalid");
    assert_eq!(code(answer("d", 1, DecisionAction::Investigate, None, None)), "decision_invalid");
    assert_eq!(code(answer("d", 1, DecisionAction::Accept, None, None)), "decision_invalid");
    assert_eq!(code(answer("d", 1, DecisionAction::Accept, Some("zzz"), None)), "decision_option_unknown");

    let modified = world.run(answer("d", 1, DecisionAction::Modify, Some("a"), Some(" 只限測試環境 "))).unwrap();
    let decision = modified.decision.unwrap();
    assert_eq!((decision.action, decision.option_id.as_deref(), decision.note.as_deref()), (DecisionAction::Modify, Some("a"), Some("只限測試環境")));
    assert_eq!(modified.resumed, vec!["t".to_string()]);

    // 回答與要求調查不選選項（即使送了也不記錄）
    let answered = world.run(answer("d", 1, DecisionAction::Answer, Some("a"), Some("用 B 來源"))).unwrap();
    assert_eq!(answered.decision.unwrap().option_id, None);
}

#[test]
fn permission_requests_only_accept_reject_or_defer() {
    let world = World::new(vec![task("t", TaskStatus::WaitingDecision, 1)], vec![pending("d", DecisionKind::Permission, "t")]);
    let refused = world.run(answer("d", 1, DecisionAction::Modify, Some("a"), Some("條件"))).unwrap_err();
    assert_eq!(refused.code(), "decision_action_not_allowed");
    assert!(world.run(answer("d", 1, DecisionAction::Reject, None, None)).is_ok());
}

/// 外部操作結果未知時的查證請求（票 26）：選項只有「已生效／沒有生效」。
fn verification(id: &str, task_id: &str) -> DecisionRequest {
    let mut request = pending(id, DecisionKind::Agent, task_id);
    request.options = vec![option("verified_done"), option("verified_not_done")];
    request.external_operation_id = Some(format!("extop_{id}"));
    request
}

#[test]
fn verification_requests_can_only_be_answered_by_accepting_an_option_or_deferred() {
    let world = World::new(vec![task("t", TaskStatus::WaitingDecision, 1)], vec![verification("d", "t")]);
    // 這些處理方式會讓請求結案、外部操作卻停在結果未知
    for (action, option, note) in [
        (DecisionAction::Reject, None, None),
        (DecisionAction::Answer, None, Some("應該好了")),
        (DecisionAction::Investigate, None, Some("去查部署紀錄")),
        (DecisionAction::Modify, Some("verified_done"), Some("條件")),
    ] {
        let refused = world.run(answer("d", 1, action, option, note)).unwrap_err();
        assert_eq!(refused.code(), "decision_action_not_allowed", "{action:?}");
    }
    assert_eq!(world.run(answer("d", 1, DecisionAction::Defer, None, None)).unwrap().outcome, DecisionOutcome::Deferred);
    let accepted = world.run(answer("d", 1, DecisionAction::Accept, Some("verified_not_done"), Some("看過紀錄"))).unwrap();
    assert_eq!(accepted.decision.unwrap().option_id.as_deref(), Some("verified_not_done"));
    assert_eq!(accepted.resumed, vec!["t".to_string()]);

    let view = view(&world.requests[0], &world.requests, &[], &[], &world.tasks, T1);
    assert_eq!(view.allowed_actions, vec![DecisionAction::Accept, DecisionAction::Defer]);
}

#[test]
fn verification_requests_never_go_stale_and_cannot_be_merged() {
    // 契約已改版、另一個 Task 已取消：一般請求會過期，查證請求不會
    let world = World::new(
        vec![task("t1", TaskStatus::WaitingDecision, 2), task("t2", TaskStatus::Cancelled, 1), task("t3", TaskStatus::WaitingDecision, 1)],
        vec![verification("d1", "t1"), verification("d2", "t2"), pending("d3", DecisionKind::Agent, "t3")],
    );
    assert_eq!(world.run(DecisionCommand::Refresh).unwrap().outcome, DecisionOutcome::Refreshed { superseded: vec![] });
    let accepted = world.run(answer("d1", 1, DecisionAction::Accept, Some("verified_done"), None)).unwrap();
    assert!(accepted.decision.is_some(), "契約改版後仍可以作答：{:?}", accepted.outcome);

    let merge = |from: &str, into: &str| DecisionCommand::Merge {
        request_id: from.into(),
        expected_revision: 1,
        into: into.into(),
        into_expected_revision: 1,
    };
    assert_eq!(world.run(merge("d1", "d3")).unwrap_err().code(), "decision_merge_invalid");
    assert_eq!(world.run(merge("d3", "d1")).unwrap_err().code(), "decision_merge_invalid");
}

#[test]
fn deferring_keeps_the_request_pending_and_the_task_waiting() {
    let world = World::new(vec![task("t", TaskStatus::WaitingDecision, 1)], vec![pending("d", DecisionKind::Agent, "t")]);
    let change = world.run(answer("d", 1, DecisionAction::Defer, None, None)).unwrap();
    assert_eq!(change.outcome, DecisionOutcome::Deferred);
    let updated = &change.updated[0];
    assert_eq!((updated.status, updated.revision), (DecisionStatus::Pending, 2));
    assert_eq!(updated.deferred_until, Some(T1.plus_millis(DEFER_DEFAULT_MINUTES * 60_000)));
    assert!(change.tasks.is_empty() && change.decision.is_none() && change.outbox.is_empty());
}

#[test]
fn a_closed_request_cannot_be_answered_differently() {
    let mut world = World::new(vec![task("t", TaskStatus::Ready, 1)], vec![pending("d", DecisionKind::Agent, "t")]);
    world.requests[0].status = DecisionStatus::Answered;
    world.requests[0].revision = 2;
    world.decisions.push(Decision {
        request_id: "d".into(),
        action: DecisionAction::Accept,
        option_id: Some("a".into()),
        note: None,
        decided_by: DECIDED_BY_DESKTOP.into(),
        decided_at: T0,
        batch_id: None,
    });
    // 同樣的處理重送是冪等的（即使帶的是舊的 revision）
    assert!(!world.run(answer("d", 1, DecisionAction::Accept, Some("a"), None)).unwrap().changed());
    assert_eq!(world.run(answer("d", 2, DecisionAction::Reject, None, None)).unwrap_err().code(), "decision_already_answered");

    world.requests[0].status = DecisionStatus::Merged;
    world.requests[0].merged_into = Some("x".into());
    assert_eq!(world.run(answer("d", 2, DecisionAction::Reject, None, None)).unwrap_err().code(), "decision_merged");
}

#[test]
fn merging_moves_the_blocking_scope_and_answering_the_target_resumes_both_tasks() {
    let world = World::new(
        vec![task("t1", TaskStatus::WaitingDecision, 1), task("t2", TaskStatus::WaitingDecision, 1)],
        vec![pending("a", DecisionKind::Agent, "t1"), pending("b", DecisionKind::Agent, "t2")],
    );
    let merged = world.run(DecisionCommand::Merge { request_id: "b".into(), expected_revision: 1, into: "a".into(), into_expected_revision: 1 }).unwrap();
    assert_eq!(merged.new_blocks, vec![("a".to_string(), "t2".to_string())]);
    assert_eq!(merged.new_links, vec![("a".to_string(), "b".to_string())]);
    let source = merged.updated.iter().find(|r| r.id == "b").unwrap();
    assert_eq!((source.status, source.merged_into.as_deref()), (DecisionStatus::Merged, Some("a")));

    // 套用合併後的狀態再回答目標
    let mut after = World::new(world.tasks.clone(), merged.updated.clone());
    after.requests.sort_by(|x, y| x.id.cmp(&y.id));
    let answered = after.run(answer("a", 2, DecisionAction::Accept, Some("a"), None)).unwrap();
    assert_eq!(answered.resumed, vec!["t1".to_string(), "t2".to_string()]);
    assert_eq!(answered.outbox.len(), 2, "被合併的請求的提出者也要被叫醒");
}

#[test]
fn merging_requires_two_pending_requests_of_the_same_non_permission_kind() {
    let world = World::new(
        vec![task("t1", TaskStatus::WaitingDecision, 1), task("t2", TaskStatus::WaitingDecision, 1)],
        vec![pending("a", DecisionKind::Permission, "t1"), pending("b", DecisionKind::Permission, "t2"), pending("c", DecisionKind::Agent, "t2")],
    );
    let merge = |from: &str, into: &str| {
        world.run(DecisionCommand::Merge { request_id: from.into(), expected_revision: 1, into: into.into(), into_expected_revision: 1 })
    };
    assert_eq!(merge("a", "b").unwrap_err().code(), "decision_merge_invalid");
    assert_eq!(merge("c", "a").unwrap_err().code(), "decision_merge_invalid");
    assert_eq!(merge("c", "c").unwrap_err().code(), "decision_merge_invalid");
}

#[test]
fn linking_is_idempotent_and_does_not_change_the_revision() {
    let mut world = World::new(vec![task("t", TaskStatus::WaitingDecision, 1)], vec![pending("a", DecisionKind::Agent, "t"), pending("b", DecisionKind::Agent, "t")]);
    let linked = world.run(DecisionCommand::Link { request_id: "b".into(), other_id: "a".into() }).unwrap();
    assert_eq!(linked.new_links, vec![("a".to_string(), "b".to_string())]);
    assert!(linked.updated.is_empty());
    world.requests[1].links.push("a".into());
    assert!(!world.run(DecisionCommand::Link { request_id: "b".into(), other_id: "a".into() }).unwrap().changed());
    assert_eq!(world.run(DecisionCommand::Link { request_id: "a".into(), other_id: "a".into() }).unwrap_err().code(), "decision_invalid");
}

#[test]
fn only_the_desktop_can_answer() {
    let world = World::new(vec![task("t", TaskStatus::WaitingDecision, 1)], vec![pending("d", DecisionKind::Agent, "t")]);
    let DecisionCommand::Answer { request_id, expected_revision, resolution, batch_id, .. } = answer("d", 1, DecisionAction::Accept, Some("a"), None) else {
        unreachable!()
    };
    let refused = world
        .run(DecisionCommand::Answer { request_id, expected_revision, resolution, decided_by: "agent".into(), batch_id })
        .unwrap_err();
    assert_eq!(refused, DecisionError::NotDesktop);
}
