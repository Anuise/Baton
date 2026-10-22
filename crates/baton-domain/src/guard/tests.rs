use super::*;
use crate::knowledge::{self, KnowledgeContent, KnowledgeKind, KnowledgeScope, KnowledgeSource, NewKnowledge, ScopeKind};
use crate::task::{ContractContent, ContractField};

const T0: Timestamp = Timestamp::from_millis(1_000);
const T1: Timestamp = Timestamp::from_millis(2_000);

fn lesson_in(scope: KnowledgeScope) -> KnowledgeEntry {
    let content = KnowledgeContent::new(KnowledgeKind::Lesson, "部署前要寫回滾計畫", "", &[], &[], None).unwrap();
    let new = NewKnowledge {
        id: "k1".into(),
        kind: KnowledgeKind::Lesson,
        scope,
        content,
        source: KnowledgeSource::User,
        valid_from: None,
        valid_until: None,
    };
    knowledge::create(new, None, T0).unwrap().entries.remove(0)
}

fn workspace_scope(ws: &str) -> KnowledgeScope {
    KnowledgeScope { kind: ScopeKind::Workspace, workspace_id: Some(ws.into()), goal_id: None, task_id: None }
}

fn task_scope(ws: &str, goal: &str, task: &str) -> KnowledgeScope {
    KnowledgeScope {
        kind: ScopeKind::Task,
        workspace_id: Some(ws.into()),
        goal_id: Some(goal.into()),
        task_id: Some(task.into()),
    }
}

fn lesson() -> KnowledgeEntry {
    lesson_in(workspace_scope("w1"))
}

fn preflight() -> GuardRule {
    GuardRule::Preflight { field: ContractFieldName::DoneCriteria, must_mention: "回滾".into() }
}

fn guard_for(lesson: &KnowledgeEntry, rule: GuardRule) -> Guard {
    create("g1".into(), lesson, rule, T0).unwrap().guard
}

fn task(id: &str) -> Task {
    Task {
        id: id.into(),
        workspace_id: "w1".into(),
        goal_id: "goal1".into(),
        title: id.into(),
        kind: crate::workdir::TaskKind::default(),
        status: crate::task::TaskStatus::Ready,
        version: 1,
        contract_version: 1,
        created_at: T0,
        updated_at: T0,
    }
}

fn contract(done: &str, capabilities: Vec<Capability>) -> TaskContract {
    let filled = |text: &str| Some(ContractField::Filled { text: text.into(), references: vec![] });
    TaskContract {
        task_id: "t1".into(),
        version: 1,
        content: ContractContent {
            objective: filled("部署新版"),
            inputs: filled("既有設定"),
            scope: filled("正式環境"),
            deliverables: filled("新版上線"),
            done_criteria: filled(done),
            blocker_handling: filled("回報"),
            required_capabilities: capabilities,
            verification_checks: vec![],
        },
        note: None,
        created_at: T0,
    }
}

fn failed_run(id: &str, contract_version: i64) -> Run {
    Run {
        id: id.into(),
        workspace_id: "w1".into(),
        task_id: "t1".into(),
        profile_id: "p1".into(),
        status: crate::run::RunStatus::Failed,
        version: 2,
        generation: 1,
        contract_version,
        authorization_revision: None,
        command_id: None,
        process: None,
        cancel_requested_at: None,
        end_reason: Some("exited_with_failure".into()),
        created_at: T0,
        updated_at: T0,
        started_at: None,
        ended_at: Some(T0),
        native_session_id: None,
        agent_session_id: None,
        resume_native_session_id: None,
    }
}

fn active(guard: Guard, lesson: KnowledgeEntry) -> ActiveGuard {
    ActiveGuard { guard, lesson }
}

#[test]
fn only_effective_lessons_can_become_guards_and_the_user_enables_them_by_creating() {
    let lesson = lesson();
    let change = create("g1".into(), &lesson, preflight(), T0).unwrap();
    assert_eq!(change.guard.status, GuardStatus::Enabled);
    assert_eq!(change.guard.lesson_id, "k1");
    assert_eq!(change.guard.revision, 1);
    assert_eq!(change.event.event_type, EVENT_CREATED);

    let mut candidate = lesson.clone();
    candidate.status = knowledge::KnowledgeStatus::Candidate;
    assert_eq!(create("g2".into(), &candidate, preflight(), T0).unwrap_err().code(), "lesson_ineligible");

    let decision = {
        let mut d = lesson.clone();
        d.kind = KnowledgeKind::Decision;
        d
    };
    assert_eq!(create("g3".into(), &decision, preflight(), T0).unwrap_err().code(), "lesson_ineligible");
}

#[test]
fn rules_are_validated() {
    let lesson = lesson();
    let empty = GuardRule::Preflight { field: ContractFieldName::Scope, must_mention: "  ".into() };
    assert_eq!(create("g1".into(), &lesson, empty, T0).unwrap_err().code(), "guard_invalid");
    let zero = GuardRule::RetryPolicy { max_failed_runs: 0 };
    assert_eq!(create("g1".into(), &lesson, zero, T0).unwrap_err().code(), "guard_invalid");
    let too_many = GuardRule::RetryPolicy { max_failed_runs: MAX_FAILED_RUNS_LIMIT + 1 };
    assert_eq!(create("g1".into(), &lesson, too_many, T0).unwrap_err().code(), "guard_invalid");

    let trimmed = GuardRule::Preflight { field: ContractFieldName::Scope, must_mention: " 回滾 ".into() };
    let guard = create("g1".into(), &lesson, trimmed, T0).unwrap().guard;
    assert_eq!(guard.rule, GuardRule::Preflight { field: ContractFieldName::Scope, must_mention: "回滾".into() });
}

#[test]
fn disabling_and_enabling_check_the_expected_revision() {
    let lesson = lesson();
    let guard = guard_for(&lesson, preflight());
    let stale = decide(&guard, &lesson, 0, GuardCommand::Disable, T1).unwrap_err();
    assert_eq!(stale.code(), "guard_revision_conflict");

    let disabled = decide(&guard, &lesson, 1, GuardCommand::Disable, T1).unwrap().unwrap();
    assert_eq!((disabled.guard.status, disabled.guard.revision), (GuardStatus::Disabled, 2));
    assert_eq!(disabled.event.event_type, EVENT_DISABLED);

    // 已經是目標狀態：沒有任何變動
    assert!(decide(&disabled.guard, &lesson, 2, GuardCommand::Disable, T1).unwrap().is_none());

    let enabled = decide(&disabled.guard, &lesson, 2, GuardCommand::Enable, T1).unwrap().unwrap();
    assert_eq!((enabled.guard.status, enabled.guard.revision), (GuardStatus::Enabled, 3));
}

#[test]
fn a_guard_cannot_be_reenabled_once_its_lesson_is_no_longer_effective() {
    let lesson = lesson();
    let guard = guard_for(&lesson, preflight());
    let disabled = decide(&guard, &lesson, 1, GuardCommand::Disable, T1).unwrap().unwrap().guard;
    let mut revoked = lesson.clone();
    revoked.status = knowledge::KnowledgeStatus::Revoked;
    let error = decide(&disabled, &revoked, 2, GuardCommand::Enable, T1).unwrap_err();
    assert_eq!(error.code(), "lesson_ineligible");
    // 停用不需要教訓仍有效
    assert!(decide(&guard, &revoked, 1, GuardCommand::Disable, T1).is_ok());
}

#[test]
fn a_guard_is_active_only_while_enabled_and_its_lesson_is_effective() {
    let lesson = lesson();
    let guard = guard_for(&lesson, preflight());
    assert_eq!(inactive_reason(&guard, &lesson, T1), None);

    let disabled = decide(&guard, &lesson, 1, GuardCommand::Disable, T1).unwrap().unwrap().guard;
    assert!(inactive_reason(&disabled, &lesson, T1).is_some());

    let mut retired = lesson.clone();
    retired.status = knowledge::KnowledgeStatus::Retired;
    assert!(inactive_reason(&guard, &retired, T1).unwrap().contains("淘汰"));

    let mut expired = lesson.clone();
    expired.valid_until = Some(T1);
    assert!(inactive_reason(&guard, &expired, T1).unwrap().contains("有效期間"));
}

#[test]
fn preflight_requires_the_contract_field_to_mention_the_text() {
    let lesson = lesson();
    let guards = [active(guard_for(&lesson, preflight()), lesson)];
    let found = violations(&guards, &task("t1"), &contract("新版上線", vec![]), &[]);
    assert_eq!(found.len(), 1);
    let violation = &found[0];
    assert_eq!((violation.guard_id.as_str(), violation.lesson_id.as_str()), ("g1", "k1"));
    assert_eq!(violation.lesson_title, "部署前要寫回滾計畫");
    assert!(violation.message.contains("完成與驗證條件"), "{}", violation.message);
    assert!(violation.message.contains("回滾"), "{}", violation.message);

    assert!(violations(&guards, &task("t1"), &contract("新版上線，附回滾步驟", vec![]), &[]).is_empty());
}

#[test]
fn preflight_mentions_are_case_insensitive() {
    let lesson = lesson();
    let rule = GuardRule::Preflight { field: ContractFieldName::DoneCriteria, must_mention: "Rollback".into() };
    let guards = [active(guard_for(&lesson, rule), lesson)];
    assert!(violations(&guards, &task("t1"), &contract("has a ROLLBACK plan", vec![]), &[]).is_empty());
}

#[test]
fn tool_restriction_blocks_tasks_requiring_the_capability() {
    let lesson = lesson();
    let rule = GuardRule::ToolRestriction { capability: Capability::Deploy };
    let guards = [active(guard_for(&lesson, rule), lesson)];
    let found = violations(&guards, &task("t1"), &contract("x", vec![Capability::Deploy]), &[]);
    assert_eq!(found.len(), 1);
    assert!(found[0].message.contains(Capability::Deploy.label()), "{}", found[0].message);
    assert!(violations(&guards, &task("t1"), &contract("x", vec![Capability::ReadFiles]), &[]).is_empty());
}

#[test]
fn retry_policy_counts_failed_runs_of_the_current_contract_version() {
    let lesson = lesson();
    let rule = GuardRule::RetryPolicy { max_failed_runs: 2 };
    let guards = [active(guard_for(&lesson, rule), lesson)];
    let t = task("t1");
    let c = contract("x", vec![]);
    assert!(violations(&guards, &t, &c, &[failed_run("r1", 1)]).is_empty());
    let found = violations(&guards, &t, &c, &[failed_run("r1", 1), failed_run("r2", 1)]);
    assert_eq!(found.len(), 1);
    assert!(found[0].message.contains('2'), "{}", found[0].message);
    // 改過契約（新版本）就重新計算
    assert!(violations(&guards, &t, &c, &[failed_run("r1", 0), failed_run("r2", 0)]).is_empty());
    // 別的 Task 的失敗不算
    let mut other = failed_run("r3", 1);
    other.task_id = "t2".into();
    assert!(violations(&guards, &t, &c, &[failed_run("r1", 1), other]).is_empty());
}

#[test]
fn guards_only_apply_to_tasks_within_their_lessons_scope() {
    let in_other_task = lesson_in(task_scope("w1", "goal1", "t9"));
    let guards = [active(guard_for(&in_other_task, preflight()), in_other_task)];
    assert!(violations(&guards, &task("t1"), &contract("x", vec![]), &[]).is_empty());

    let in_this_task = lesson_in(task_scope("w1", "goal1", "t1"));
    let guards = [active(guard_for(&in_this_task, preflight()), in_this_task)];
    assert_eq!(violations(&guards, &task("t1"), &contract("x", vec![]), &[]).len(), 1);

    let other_workspace = lesson_in(workspace_scope("w2"));
    let guards = [active(guard_for(&other_workspace, preflight()), other_workspace)];
    assert!(violations(&guards, &task("t1"), &contract("x", vec![]), &[]).is_empty());

    let global = lesson_in(KnowledgeScope::global());
    let guards = [active(guard_for(&global, preflight()), global)];
    assert_eq!(violations(&guards, &task("t1"), &contract("x", vec![]), &[]).len(), 1);
}

#[test]
fn a_prevented_recurrence_points_at_the_lesson_and_guard_and_is_keyed_by_task_contract_version() {
    let lesson = lesson();
    let guards = [active(guard_for(&lesson, preflight()), lesson)];
    let violation = violations(&guards, &task("t1"), &contract("x", vec![]), &[]).remove(0);
    let (recurrence, event) = prevented(&violation.source(), &violation.message, &task("t1"), T1);
    assert_eq!(recurrence.kind, RecurrenceKind::Prevented);
    assert_eq!(recurrence.lesson_id, "k1");
    assert_eq!(recurrence.guard_id.as_deref(), Some("g1"));
    assert_eq!((recurrence.task_id.as_deref(), recurrence.contract_version), (Some("t1"), Some(1)));
    assert_eq!(recurrence.message, violation.message);
    let again = prevented(&violation.source(), &violation.message, &task("t1"), T0).0;
    assert_eq!(recurrence.id, again.id, "同一個 Guard、Task 與契約版本只記一次");
    assert_eq!(event.event_type, EVENT_RECURRENCE_RECORDED);
    assert_eq!(event.payload["lesson_id"], "k1");
}

#[test]
fn reported_recurrences_need_a_verified_lesson_and_a_note() {
    let lesson = lesson();
    let (recurrence, _) = report("rec1".into(), &lesson, Some(&task("t1")), "又忘了寫回滾", T1).unwrap();
    assert_eq!(recurrence.kind, RecurrenceKind::Reported);
    assert_eq!(recurrence.guard_id, None);
    assert_eq!(recurrence.task_id.as_deref(), Some("t1"));

    assert_eq!(report("rec2".into(), &lesson, None, " ", T1).unwrap_err().code(), "guard_invalid");
    let mut candidate = lesson.clone();
    candidate.status = knowledge::KnowledgeStatus::Candidate;
    assert_eq!(report("rec3".into(), &candidate, None, "x", T1).unwrap_err().code(), "lesson_ineligible");
}
