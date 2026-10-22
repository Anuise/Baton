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
        origin: DecisionOrigin::Worker,
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
    contracts: Vec<crate::task::TaskContract>,
    assumptions: Vec<crate::task::Assumption>,
}

impl World {
    fn new(tasks: Vec<Task>, requests: Vec<DecisionRequest>) -> Self {
        Self { workspace: workspace(), tasks, requests, decisions: vec![], contracts: vec![], assumptions: vec![] }
    }

    fn run(&self, command: DecisionCommand) -> Result<DecisionChange, DecisionError> {
        let task_ctx = TaskContext {
            workspace_id: "ws",
            workspace: Some(&self.workspace),
            authorization: None,
            goals: &[],
            tasks: &self.tasks,
            dependencies: &[],
            contracts: &self.contracts,
            decision_holds: &[],
            assumptions: &self.assumptions,
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
            budgets: &[],
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

// ---------------------------------------------------------------- Controller 的提問與事項判定（票 17）

fn question(blocking: &[&str]) -> DecisionCommand {
    DecisionCommand::ControllerQuestion {
        request_id: "dq".into(),
        controller_run_id: "ctl_1".into(),
        content: content(vec![option("a"), option("b")], Some("a")).unwrap(),
        blocking: blocking.iter().map(|t| t.to_string()).collect(),
    }
}

fn controller_pending(id: &str, blocking: &[&str]) -> DecisionRequest {
    let mut request = pending(id, DecisionKind::Agent, blocking[0]);
    request.origin = DecisionOrigin::Controller;
    request.run_id = "ctl_1".into();
    request.question = "要用哪個方案？".into();
    request.blocking = blocking.iter().map(|t| t.to_string()).collect();
    request
}

#[test]
fn a_controller_question_blocks_the_listed_tasks_without_changing_their_status() {
    let world = World::new(vec![task("t1", TaskStatus::Pending, 1), task("t2", TaskStatus::Ready, 1), task("t3", TaskStatus::Pending, 1)], vec![]);

    let change = world.run(question(&["t1", "t2", "t1"])).unwrap();

    assert_eq!(change.outcome, DecisionOutcome::Requested { request_id: "dq".into(), existing: false });
    let created = change.created.unwrap();
    assert_eq!((created.origin, created.run_id.as_str(), created.kind), (DecisionOrigin::Controller, "ctl_1", DecisionKind::Agent));
    assert_eq!(created.blocking, vec!["t1".to_string(), "t2".to_string()]);
    assert_eq!(created.target.task_id, "t1");
    assert!(change.tasks.is_empty(), "依賴答案的 Task 狀態不變，只是不能准入");
}

#[test]
fn a_controller_question_only_blocks_tasks_that_have_not_been_dispatched() {
    let world = World::new(vec![task("t1", TaskStatus::Running, 1), task("t2", TaskStatus::Pending, 1)], vec![]);
    assert_eq!(world.run(question(&["t2", "t1"])).unwrap_err().code(), "decision_invalid");
    assert_eq!(world.run(question(&[])).unwrap_err().code(), "decision_invalid");
    assert_eq!(world.run(question(&["t9"])).unwrap_err().code(), "task_not_found");
}

#[test]
fn asking_the_same_question_again_returns_the_pending_request_and_adds_new_tasks() {
    let world = World::new(
        vec![task("t1", TaskStatus::Pending, 1), task("t2", TaskStatus::Pending, 1)],
        vec![controller_pending("d1", &["t1"])],
    );
    let change = world.run(question(&["t1", "t2"])).unwrap();
    assert_eq!(change.outcome, DecisionOutcome::Requested { request_id: "d1".into(), existing: true });
    assert!(change.created.is_none());
    assert_eq!(change.new_blocks, vec![("d1".to_string(), "t2".to_string())]);
}

#[test]
fn answering_a_controller_question_notifies_the_conversation_instead_of_the_outbox() {
    let world = World::new(vec![task("t1", TaskStatus::Pending, 3)], vec![controller_pending("d1", &["t1"])]);

    let change = world.run(answer("d1", 1, DecisionAction::Accept, Some("b"), Some("先做 B"))).unwrap();

    assert_eq!(change.outcome, DecisionOutcome::Answered { resumed_task_ids: vec![] });
    assert!(change.outbox.is_empty(), "Controller 沒有 Worker 的 Run，不寫喚醒 Outbox");
    assert_eq!(change.notices.len(), 1);
    let notice = &change.notices[0];
    assert_eq!((notice.workspace_id.as_str(), notice.source.as_str()), ("ws", "decision_wake:d1:2"));
    assert!(notice.content.contains("要用哪個方案？") && notice.content.contains("選項 b") && notice.content.contains("先做 B"), "{}", notice.content);
    assert!(change.tasks.is_empty(), "pending 的 Task 不會被改成 ready，由 Controller 補契約後再派工");
}

#[test]
fn a_controller_question_is_not_superseded_by_contract_changes_only_when_its_tasks_end() {
    let world = World::new(
        vec![task("t1", TaskStatus::Pending, 5), task("t2", TaskStatus::Cancelled, 1), task("t3", TaskStatus::Cancelled, 1)],
        vec![controller_pending("d1", &["t1", "t2"]), controller_pending("d2", &["t3"])],
    );
    let change = world.run(DecisionCommand::Refresh).unwrap();
    assert_eq!(change.outcome, DecisionOutcome::Refreshed { superseded: vec!["d2".into()] });
    assert!(change.notices[0].content.contains("已過期"), "{}", change.notices[0].content);
}

#[test]
fn controller_and_worker_requests_cannot_be_merged() {
    let mut worker = pending("w", DecisionKind::Agent, "t2");
    worker.question = "要用哪個方案？".into();
    let world = World::new(
        vec![task("t1", TaskStatus::Pending, 1), task("t2", TaskStatus::WaitingDecision, 1)],
        vec![controller_pending("c", &["t1"]), worker],
    );
    let merge = DecisionCommand::Merge { request_id: "c".into(), expected_revision: 1, into: "w".into(), into_expected_revision: 1 };
    assert_eq!(world.run(merge).unwrap_err().code(), "decision_merge_invalid");
}

fn gate(matter: Matter, task_id: &str) -> DecisionCommand {
    DecisionCommand::ControllerGate {
        request_id: "dg".into(),
        audit_id: "audit".into(),
        controller_run_id: "ctl_1".into(),
        content: ActionContent::new(matter, Some(task_id), "派出部署 Task", "需要部署能力", vec![], None).unwrap(),
    }
}

#[test]
fn a_controller_gate_creates_an_authority_request_from_the_controller_and_consumes_an_approval_once() {
    let mut world = World::new(vec![task("t1", TaskStatus::Pending, 1)], vec![]);

    let change = world.run(gate(Matter::ExceedAuthorization, "t1")).unwrap();
    let DecisionOutcome::Controller { outcome: ControllerOutcome::RequiresDecision { request_id, existing: false }, .. } = &change.outcome else {
        panic!("{:?}", change.outcome)
    };
    let created = change.created.clone().unwrap();
    assert_eq!((created.origin, created.run_id.as_str(), created.task_id.as_str()), (DecisionOrigin::Controller, "ctl_1", "t1"));
    assert!(created.options[0].tradeoff.as_deref().unwrap().contains("不會擴大 Workspace 授權"));
    assert_eq!(request_id, "dg");

    // 還在等待：回到同一個請求
    world.requests.push(created.clone());
    let again = world.run(gate(Matter::ExceedAuthorization, "t1")).unwrap();
    assert!(matches!(again.outcome, DecisionOutcome::Controller { outcome: ControllerOutcome::RequiresDecision { existing: true, .. }, .. }));

    // 使用者核准之後，下一次同一個事項消耗它（記在 Controller 的一輪上）
    let answered = world.run(answer("dg", 1, DecisionAction::Accept, Some(OPTION_APPROVE), None)).unwrap();
    world.requests = answered.updated.clone();
    world.decisions.push(answered.decision.clone().unwrap());
    let used = world.run(gate(Matter::ExceedAuthorization, "t1")).unwrap();
    assert!(matches!(used.outcome, DecisionOutcome::Controller { outcome: ControllerOutcome::Approved { .. }, .. }));
    assert_eq!(used.approval_use.unwrap().run_id, "ctl_1");
}

#[test]
fn a_controller_gate_needs_a_target_task() {
    let world = World::new(vec![task("t1", TaskStatus::Pending, 1)], vec![]);
    let command = DecisionCommand::ControllerGate {
        request_id: "dg".into(),
        audit_id: "audit".into(),
        controller_run_id: "ctl_1".into(),
        content: ActionContent::new(Matter::ModifyDoneCriteria, None, "改完成條件", "原因", vec![], None).unwrap(),
    };
    assert_eq!(world.run(command).unwrap_err().code(), "decision_invalid");
    assert!(matches!(
        world.run(gate(Matter::ArrangeTasks, "t1")).unwrap().outcome,
        DecisionOutcome::Controller { outcome: ControllerOutcome::Allowed, .. }
    ));
}

fn gate_on(matter: Matter, task_id: &str, subject: &str, request_id: &str) -> DecisionCommand {
    DecisionCommand::ControllerGate {
        request_id: request_id.into(),
        audit_id: "audit".into(),
        controller_run_id: "ctl_1".into(),
        content: ActionContent::new(matter, Some(task_id), &format!("駁回 {subject}"), "不影響需求", vec![], None)
            .unwrap()
            .with_subject(subject),
    }
}

#[test]
fn a_controller_gate_with_a_subject_only_shares_requests_and_approvals_for_the_same_subject() {
    let mut world = World::new(vec![task("t1", TaskStatus::Ready, 1)], vec![]);
    let first = world.run(gate_on(Matter::DismissReviewFinding, "t1", "fnd_1", "dr1")).unwrap();
    let created = first.created.clone().unwrap();
    assert_eq!(created.authority.as_ref().unwrap().fingerprint, "dismiss_review_finding:t1:fnd_1");
    world.requests.push(created);

    // 同一個 Task 的另一個 finding：另建請求，不回到 fnd_1 的請求
    let other = world.run(gate_on(Matter::DismissReviewFinding, "t1", "fnd_2", "dr2")).unwrap();
    assert!(matches!(other.outcome, DecisionOutcome::Controller { outcome: ControllerOutcome::RequiresDecision { existing: false, .. }, .. }));

    // fnd_1 的核准不能被 fnd_2 用掉
    let answered = world.run(answer("dr1", 1, DecisionAction::Accept, Some(OPTION_APPROVE), None)).unwrap();
    world.requests = answered.updated.clone();
    world.decisions.push(answered.decision.clone().unwrap());
    let other = world.run(gate_on(Matter::DismissReviewFinding, "t1", "fnd_2", "dr3")).unwrap();
    assert!(matches!(other.outcome, DecisionOutcome::Controller { outcome: ControllerOutcome::RequiresDecision { existing: false, .. }, .. }));
    let used = world.run(gate_on(Matter::DismissReviewFinding, "t1", "fnd_1", "dr4")).unwrap();
    assert!(matches!(used.outcome, DecisionOutcome::Controller { outcome: ControllerOutcome::Approved { .. }, .. }));
}

// --- 票 50：使用者的答案推翻既有假設 ---

/// Task 目前的契約，「輸入與依據」引用裁定請求 `request_id`（照它的建議先做）。
fn built_on(task_id: &str, request_id: &str) -> crate::task::TaskContract {
    use crate::task::{ContractContent, ContractField, Reference, ReferenceKind};
    crate::task::TaskContract {
        task_id: task_id.into(),
        version: 1,
        content: ContractContent {
            inputs: Some(ContractField::Filled {
                text: "依建議先做".into(),
                references: vec![Reference { kind: ReferenceKind::Decision, target: request_id.into(), version: 1 }],
            }),
            ..Default::default()
        },
        note: None,
        created_at: T0,
    }
}

/// d_api 問「要用哪個方案」並建議 a；t1 依這個建議先做，d_t1 是 t1 正在等的另一個請求；t2 與它無關。
fn assumption_world() -> World {
    let mut asked = pending("d_api", DecisionKind::Agent, "t0");
    asked.recommendation = Some("a".into());
    let mut world = World::new(
        vec![task("t0", TaskStatus::WaitingDecision, 1), task("t1", TaskStatus::WaitingDecision, 1), task("t2", TaskStatus::WaitingDecision, 1)],
        vec![asked, pending("d_t1", DecisionKind::Agent, "t1"), pending("d_t2", DecisionKind::Agent, "t2")],
    );
    world.contracts = vec![built_on("t1", "d_api")];
    world
}

#[test]
fn an_answer_that_overturns_the_recommendation_supersedes_pending_requests_of_tasks_built_on_it() {
    let world = assumption_world();
    let change = world.run(answer("d_api", 1, DecisionAction::Accept, Some("b"), None)).unwrap();

    assert_eq!(change.decision.as_ref().unwrap().option_id.as_deref(), Some("b"));
    let status = |id: &str| change.updated.iter().find(|r| r.id == id).map(|r| (r.status, r.status_reason.clone()));
    assert_eq!(status("d_api").unwrap().0, DecisionStatus::Answered);
    let (superseded, reason) = status("d_t1").expect("依據被推翻的 Task 的請求過期");
    assert_eq!(superseded, DecisionStatus::Superseded);
    assert!(reason.unwrap().contains("假設"));
    assert_eq!(status("d_t2"), None, "獨立的請求不受影響");
    let mut resumed = change.resumed.clone();
    resumed.sort();
    assert_eq!(resumed, vec!["t0".to_string(), "t1".to_string()], "等它們的 Task 不會一直卡在 waiting_decision");
    let notice = change.notices.iter().find(|n| n.content.contains("推翻")).expect("通知 Controller 重新整理");
    assert!(notice.content.contains("t1") && !notice.content.contains("t2"), "{}", notice.content);
}

#[test]
fn accepting_the_recommendation_confirms_the_assumption_and_changes_nothing_else() {
    let world = assumption_world();
    let change = world.run(answer("d_api", 1, DecisionAction::Accept, Some("a"), None)).unwrap();
    assert_eq!(change.updated.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), vec!["d_api"]);
    assert!(change.notices.iter().all(|n| !n.content.contains("推翻")));
}

#[test]
fn refresh_supersedes_requests_raised_before_the_assumption_of_their_task_was_overturned() {
    let mut world = assumption_world();
    world.requests.retain(|r| r.id != "d_api");
    let mut later = pending("d_t1_later", DecisionKind::Agent, "t1");
    later.created_at = Timestamp::from_millis(1_800);
    world.requests.push(later);
    world.assumptions = vec![crate::task::Assumption {
        request_id: "d_api".into(),
        revision: 2,
        overturned: Some(crate::task::Overturn { decided_at: Timestamp::from_millis(1_500), reason: "選了 b".into() }),
    }];
    let change = world.run(DecisionCommand::Refresh).unwrap();
    let DecisionOutcome::Refreshed { superseded } = &change.outcome else { panic!("{:?}", change.outcome) };
    assert_eq!(superseded, &vec!["d_t1".to_string()], "推翻之後才提出的請求已經知道新的答案");
}
