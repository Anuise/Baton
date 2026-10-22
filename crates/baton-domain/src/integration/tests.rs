use super::*;
use crate::workdir::TaskKind;
use crate::workspace::{self, WorkspaceCommand};

const T0: Timestamp = Timestamp::from_millis(1_000_000);

fn later(ms: i64) -> Timestamp {
    T0.plus_millis(ms)
}

fn workspace() -> Workspace {
    let create = WorkspaceCommand::Create { id: "w1".into(), name: None, path: r"C:\work\app".into() };
    workspace::decide(&[], create, T0).unwrap().workspace
}

fn task(status: TaskStatus) -> Task {
    Task {
        id: "t1".into(),
        workspace_id: "w1".into(),
        goal_id: "g1".into(),
        title: "t1".into(),
        kind: TaskKind::Development,
        status,
        version: 7,
        contract_version: 1,
        created_at: T0,
        updated_at: T0,
    }
}

fn check() -> VerificationCheck {
    VerificationCheck::Command { program: "cargo".into(), args: vec!["test".into()], expected_exit_code: 0 }
}

fn result(status: CheckStatus) -> CheckResult {
    CheckResult { check: check(), status, detail: "結束碼".into() }
}

/// 一個項目與它的 Task；每次命令後套回結果。
struct World {
    workspace: Workspace,
    task: Task,
    entry: IntegrationEntry,
    checks: Vec<VerificationCheck>,
}

impl World {
    fn queued() -> Self {
        let task = task(TaskStatus::Verifying);
        let (mut entry, event) = enqueue("int_1".into(), &task, "r1", 1, "baton/t1", "main", T0);
        entry.seq = 1;
        assert_eq!((event.event_type, event.aggregate_version), (EVENT_QUEUED, 1));
        Self { workspace: workspace(), task, entry, checks: vec![check()] }
    }

    fn step(&mut self, command: IntegrationCommand, now: Timestamp) -> Result<IntegrationChange, IntegrationError> {
        let ctx =
            IntegrationContext { entry: &self.entry, workspace: Some(&self.workspace), task: Some(&self.task), checks: &self.checks };
        let change = decide(&ctx, command, now)?;
        self.entry = change.entry.clone();
        if let Some(task) = &change.task {
            self.task = task.clone();
        }
        Ok(change)
    }

    fn merged(&mut self) {
        self.step(IntegrationCommand::BeginMerge { id: "int_1".into() }, later(1)).unwrap();
        let result = MergeResult::Merged { commit: "m1".into(), target_tip: "base".into() };
        self.step(IntegrationCommand::RecordMerge { id: "int_1".into(), result }, later(2)).unwrap();
    }
}

#[test]
fn a_passing_integration_merges_verifies_advances_and_only_then_completes_the_task() {
    let mut world = World::queued();

    world.step(IntegrationCommand::BeginMerge { id: "int_1".into() }, later(1)).unwrap();
    assert_eq!((world.entry.status, world.entry.started_at), (IntegrationStatus::Merging, Some(later(1))));
    let result = MergeResult::Merged { commit: "m1".into(), target_tip: "base".into() };
    world.step(IntegrationCommand::RecordMerge { id: "int_1".into(), result }, later(2)).unwrap();
    assert_eq!(world.entry.status, IntegrationStatus::Verifying);
    assert_eq!((world.entry.merge_commit.as_deref(), world.entry.target_tip.as_deref()), (Some("m1"), Some("base")));
    assert_eq!(world.task.status, TaskStatus::Verifying, "合併了但還沒驗證：不完成");

    let change =
        world.step(IntegrationCommand::RecordChecks { id: "int_1".into(), results: vec![result_passed()] }, later(3)).unwrap();
    assert_eq!(world.entry.status, IntegrationStatus::Advancing);
    assert_eq!(change.task, None, "驗證通過但目標分支還沒前移：不完成");

    let change = world.step(IntegrationCommand::RecordAdvance { id: "int_1".into(), result: AdvanceResult::Advanced }, later(4)).unwrap();
    assert_eq!((world.entry.status, world.entry.finished_at), (IntegrationStatus::Integrated, Some(later(4))));
    assert_eq!(world.task.status, TaskStatus::Completed);
    let types: Vec<_> = change.events.iter().map(|e| e.event_type).collect();
    assert_eq!(types, vec![EVENT_INTEGRATED, crate::task::EVENT_STATUS_CHANGED]);
    assert_eq!(world.entry.version, 5, "排入、開始合併、合併、驗證通過、完成各一個事件");
}

fn result_passed() -> CheckResult {
    result(CheckStatus::Passed)
}

#[test]
fn failing_integration_checks_keep_the_task_from_completing_and_keep_the_evidence() {
    let mut world = World::queued();
    world.merged();

    world.step(IntegrationCommand::RecordChecks { id: "int_1".into(), results: vec![result(CheckStatus::Failed)] }, later(3)).unwrap();

    assert_eq!(world.entry.status, IntegrationStatus::Failed);
    assert_eq!(world.task.status, TaskStatus::Ready, "未通過整合：Task 回到可修正狀態，不完成");
    assert_eq!(world.entry.checks, vec![result(CheckStatus::Failed)], "證據保存在項目中");
    assert!(world.entry.failure.as_deref().unwrap().contains("整合驗證未通過"), "{:?}", world.entry.failure);
    assert_eq!(
        world.step(IntegrationCommand::RecordAdvance { id: "int_1".into(), result: AdvanceResult::Advanced }, later(4)),
        Err(IntegrationError::TransitionInvalid { from: IntegrationStatus::Failed, action: "記錄目標分支的前移結果" }),
        "失敗的項目不能再前移"
    );
}

#[test]
fn a_merge_conflict_or_a_moved_target_fails_without_completing() {
    let mut world = World::queued();
    world.step(IntegrationCommand::BeginMerge { id: "int_1".into() }, later(1)).unwrap();
    let conflict = MergeResult::Conflict { detail: "src/lib.rs".into() };
    world.step(IntegrationCommand::RecordMerge { id: "int_1".into(), result: conflict }, later(2)).unwrap();
    assert_eq!((world.entry.status, world.task.status), (IntegrationStatus::Failed, TaskStatus::Ready));
    assert!(world.entry.failure.as_deref().unwrap().contains("衝突"));

    let mut world = World::queued();
    world.merged();
    world.step(IntegrationCommand::RecordChecks { id: "int_1".into(), results: vec![result_passed()] }, later(3)).unwrap();
    let moved = AdvanceResult::TargetMoved { actual: "other".into() };
    world.step(IntegrationCommand::RecordAdvance { id: "int_1".into(), result: moved }, later(4)).unwrap();
    assert_eq!((world.entry.status, world.task.status), (IntegrationStatus::Failed, TaskStatus::Ready));
    assert!(world.entry.failure.as_deref().unwrap().contains("已被更新"));
}

#[test]
fn checks_must_match_the_contract_one_for_one() {
    let mut world = World::queued();
    world.merged();
    assert_eq!(
        world.step(IntegrationCommand::RecordChecks { id: "int_1".into(), results: vec![] }, later(3)),
        Err(IntegrationError::ResultsMismatch)
    );
    assert_eq!(world.entry.status, IntegrationStatus::Verifying, "被拒絕時不變");
}

#[test]
fn without_declared_checks_a_clean_merge_is_the_integration_evidence() {
    let mut world = World::queued();
    world.checks.clear();
    world.merged();
    world.step(IntegrationCommand::RecordChecks { id: "int_1".into(), results: vec![] }, later(3)).unwrap();
    assert_eq!(world.entry.status, IntegrationStatus::Advancing);
}

#[test]
fn restarting_after_a_crash_redoes_the_merge_from_scratch() {
    let mut world = World::queued();
    world.merged();
    assert_eq!(world.entry.status, IntegrationStatus::Verifying);

    world.step(IntegrationCommand::BeginMerge { id: "int_1".into() }, later(10)).unwrap();

    assert_eq!(world.entry.status, IntegrationStatus::Merging);
    assert_eq!((world.entry.merge_commit.clone(), world.entry.started_at), (None, Some(later(1))), "保留第一次開始的時間");
}

#[test]
fn a_task_that_left_verifying_is_not_integrated() {
    let mut world = World::queued();
    world.task.status = TaskStatus::Cancelled;

    let change = world.step(IntegrationCommand::BeginMerge { id: "int_1".into() }, later(1)).unwrap();

    assert_eq!(world.entry.status, IntegrationStatus::Failed);
    assert_eq!(change.task, None, "已取消的 Task 不會被拉回 ready");
    assert!(world.entry.failure.as_deref().unwrap().contains("cancelled"));
}

#[test]
fn the_queue_processes_the_active_entry_first_then_the_oldest_queued_one() {
    let base = World::queued().entry;
    let entry = |id: &str, seq, status| IntegrationEntry { id: id.into(), seq, status, ..base.clone() };

    let entries = vec![entry("c", 3, IntegrationStatus::Queued), entry("b", 2, IntegrationStatus::Queued), entry("a", 1, IntegrationStatus::Integrated)];
    assert_eq!(next(&entries).map(|e| e.id.as_str()), Some("b"));

    let entries = vec![entry("b", 2, IntegrationStatus::Queued), entry("c", 3, IntegrationStatus::Verifying)];
    assert_eq!(next(&entries).map(|e| e.id.as_str()), Some("c"), "崩潰前處理中的先接續，一次只處理一個");

    let entries = vec![entry("a", 1, IntegrationStatus::Failed)];
    assert_eq!(next(&entries), None);
}

#[test]
fn the_integration_directory_lives_under_the_workspace_baton_directory() {
    assert_eq!(integration_dir(&workspace()), r"C:\work\app\.baton\integration");
}
