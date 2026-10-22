use proptest::prelude::*;

use super::*;
use crate::authorization::Capability;
use crate::guard::{self, ActiveGuard, GuardRule};
use crate::knowledge::{self, KnowledgeContent, KnowledgeKind, KnowledgeScope, KnowledgeSource, NewKnowledge};
use crate::task::{
    ContractField, Dependency, Goal, TaskChange, TaskCommand, TaskContract,
};
use crate::workdir::{self as wd, PrepareFailure, TaskKind};
use crate::workspace::{self, Workspace, WorkspaceCommand};

const T0: Timestamp = Timestamp::from_millis(1_000_000);

fn filled(text: &str) -> Option<ContractField> {
    Some(ContractField::Filled { text: text.into(), references: vec![] })
}

fn complete(capabilities: Vec<Capability>) -> ContractContent {
    ContractContent {
        objective: filled("整理資料"),
        inputs: Some(ContractField::NotApplicable { reason: "沒有輸入".into() }),
        scope: filled("公開資料"),
        deliverables: filled("比較表"),
        done_criteria: filled("五個產品"),
        blocker_handling: filled("回報"),
        required_capabilities: capabilities,
        verification_checks: vec![],
    }
}

/// 記憶體中的一個 Workspace：Domain 判定需要的全部狀態，並且能套用每個命令的結果。
#[derive(Clone, PartialEq)]
struct World {
    workspace: Workspace,
    goals: Vec<Goal>,
    tasks: Vec<Task>,
    dependencies: Vec<Dependency>,
    contracts: Vec<TaskContract>,
    runs: Vec<Run>,
    leases: Vec<ExecutionLease>,
    workdirs: Vec<Workdir>,
    guards: Vec<ActiveGuard>,
    next_run: u32,
}

impl World {
    fn new(task_ids: &[&str]) -> Self {
        let create = WorkspaceCommand::Create { id: "w1".into(), name: None, path: "C:\\work\\app".into() };
        let workspace = workspace::decide(&[], create, T0).unwrap().workspace;
        let mut world = Self {
            workspace,
            goals: vec![],
            tasks: vec![],
            dependencies: vec![],
            contracts: vec![],
            runs: vec![],
            leases: vec![],
            workdirs: vec![],
            guards: vec![],
            next_run: 0,
        };
        world.task_command(TaskCommand::CreateGoal { id: "g1".into(), title: "目標".into() }).unwrap();
        for id in task_ids {
            let create = TaskCommand::CreateTask {
                id: (*id).into(),
                goal_id: "g1".into(),
                title: (*id).into(),
                kind: TaskKind::Research,
                contract: complete(vec![]),
            };
            world.task_command(create).unwrap();
            world.task_command(TaskCommand::MarkReady { task_id: (*id).into() }).unwrap();
            world.prepare(id);
        }
        world
    }

    /// 登記 Task 的工作目錄為可用（Runtime 在交易外建立目錄後做的事）。
    fn prepare(&mut self, task_id: &str) {
        let plan = wd::plan(&self.workspace, self.task(task_id));
        let base = plan.branch.as_ref().map(|_| "main".to_string());
        let existing = self.workdirs.iter().find(|w| w.task_id == task_id).cloned();
        let (workdir, _) = wd::record_prepared(existing.as_ref(), &plan, base, T0);
        self.workdirs.retain(|w| w.task_id != task_id);
        self.workdirs.push(workdir);
    }

    fn task_ctx(&self) -> TaskContext<'_> {
        TaskContext {
            workspace_id: "w1",
            workspace: Some(&self.workspace),
            authorization: None,
            goals: &self.goals,
            tasks: &self.tasks,
            dependencies: &self.dependencies,
            contracts: &self.contracts,
        }
    }

    fn task_command(&mut self, command: TaskCommand) -> Result<TaskChange, task::TaskError> {
        let change = task::decide(&self.task_ctx(), command, T0)?;
        if let Some(goal) = &change.goal {
            self.goals.push(goal.clone());
        }
        if let Some(task) = &change.task {
            upsert(&mut self.tasks, task.clone(), |t| t.id.clone());
        }
        if let Some(contract) = &change.contract {
            self.contracts.retain(|c| c.task_id != contract.task_id);
            self.contracts.push(contract.clone());
        }
        if let Some(dependency) = &change.dependency {
            self.dependencies.push(dependency.clone());
        }
        Ok(change)
    }

    fn decide(&self, command: RunCommand, now: Timestamp) -> Result<RunChange, RunError> {
        let task = self.task_ctx();
        decide(
            &RunContext {
                task: &task,
                runs: &self.runs,
                leases: &self.leases,
                workdirs: &self.workdirs,
                guards: &self.guards,
            },
            command,
            now,
        )
    }

    /// 判定並套用；被拒絕時狀態完全不變。
    fn step(&mut self, command: RunCommand, now: Timestamp) -> Result<RunChange, RunError> {
        let change = self.decide(command, now)?;
        if let Some(run) = &change.run {
            upsert(&mut self.runs, run.clone(), |r| r.id.clone());
        }
        if let Some(lease) = &change.lease {
            upsert(&mut self.leases, lease.clone(), |l| l.run_id.clone());
        }
        if let Some(task) = &change.task {
            upsert(&mut self.tasks, task.clone(), |t| t.id.clone());
        }
        Ok(change)
    }

    fn dispatch(&mut self, task_id: &str, now: Timestamp) -> Result<String, RunError> {
        self.next_run += 1;
        let run_id = format!("run{}", self.next_run);
        self.step(RunCommand::Dispatch { run_id: run_id.clone(), task_id: task_id.into(), profile_id: "p1".into() }, now)?;
        Ok(run_id)
    }

    fn run(&self, id: &str) -> &Run {
        self.runs.iter().find(|r| r.id == id).unwrap()
    }

    fn lease(&self, run_id: &str) -> &ExecutionLease {
        self.leases.iter().find(|l| l.run_id == run_id).unwrap()
    }

    fn task(&self, id: &str) -> &Task {
        self.tasks.iter().find(|t| t.id == id).unwrap()
    }

    /// 把 Run 推進到 running，回傳 Run 識別。
    fn running(&mut self, task_id: &str, now: Timestamp) -> String {
        let run_id = self.dispatch(task_id, now).unwrap();
        self.step(RunCommand::BeginLaunch { run_id: run_id.clone(), command_id: format!("cmd-{run_id}") }, now).unwrap();
        self.step(RunCommand::RecordReceipt { run_id: run_id.clone(), process: process(1) }, now).unwrap();
        run_id
    }
}

fn upsert<T>(items: &mut Vec<T>, item: T, key: impl Fn(&T) -> String) {
    match items.iter().position(|i| key(i) == key(&item)) {
        Some(index) => items[index] = item,
        None => items.push(item),
    }
}

fn process(pid: u32) -> ProcessRecord {
    ProcessRecord { pid, created_at_millis: 42, command_hash: "hash".into() }
}

fn later(ms: i64) -> Timestamp {
    T0.plus_millis(ms)
}

// ---------------------------------------------------------------- 狀態表

#[test]
fn terminal_run_statuses_have_no_exit_and_every_active_status_can_end() {
    for status in RunStatus::ALL {
        if status.is_terminal() {
            assert!(status.allowed_next().is_empty(), "{status:?} 已結束，不該有出口");
        } else {
            assert!(status.can_transition_to(RunStatus::Cancelled), "{status:?} 必須能取消");
        }
    }
    assert!(!RunStatus::Queued.can_transition_to(RunStatus::Running), "不經過 starting 就不能 running");
    assert!(!RunStatus::Running.can_transition_to(RunStatus::Queued));
    for status in RunStatus::ALL {
        assert_eq!(RunStatus::parse(status.as_str()), Some(status));
    }
}

// ---------------------------------------------------------------- 派工

#[test]
fn dispatch_creates_queued_run_lease_and_outbox_and_moves_task_to_running() {
    let mut world = World::new(&["t1"]);

    let change = world.step(RunCommand::Dispatch { run_id: "r1".into(), task_id: "t1".into(), profile_id: "p1".into() }, T0).unwrap();

    let run = change.run.unwrap();
    assert_eq!((run.status, run.generation, run.task_id.as_str()), (RunStatus::Queued, 1, "t1"));
    assert_eq!(run.command_id, None, "launch intent 要等 Supervisor 在交易外持久化");
    let lease = change.lease.unwrap();
    assert!(lease.is_held());
    assert_eq!(lease.expires_at, T0.plus_millis(LEASE_DURATION_MS));
    let outbox = change.outbox.unwrap();
    assert_eq!((outbox.kind.as_str(), outbox.run_id.as_str()), (OUTBOX_LAUNCH_RUN, "r1"));
    assert!(outbox.payload.prompt.contains("整理資料"));
    assert_eq!(outbox.payload.working_dir, r"C:\work\app\.baton\outputs\t1", "研究 Task 在自己的輸出目錄中執行");
    assert_eq!(world.task("t1").status, TaskStatus::Running);
    let types: Vec<_> = change.events.iter().map(|e| (e.aggregate_type, e.event_type)).collect();
    assert_eq!(types, vec![("run", EVENT_RUN_CREATED), ("task", "task_status_changed")]);
}

#[test]
fn dispatch_waits_until_the_task_has_a_registered_ready_workdir() {
    let mut world = World::new(&["t1"]);
    world.workdirs.clear();

    assert_eq!(world.dispatch("t1", T0), Err(RunError::WorkdirNotPrepared(WorkdirKind::OutputDir)));

    // 準備失敗：不派工，原因就是登記的失敗原因
    let plan = wd::plan(&world.workspace, world.task("t1"));
    let (failed, _) = wd::record_failed(None, &plan, &PrepareFailure::Io("磁碟已滿".into()), T0);
    world.workdirs.push(failed);
    let refused = world.dispatch("t1", T0);
    assert!(matches!(&refused, Err(RunError::WorkdirUnavailable(reason)) if reason.contains("磁碟已滿")), "{refused:?}");
    assert!(world.runs.is_empty() && world.leases.is_empty(), "不可用時什麼都不建立");
    assert_eq!(world.task("t1").status, TaskStatus::Ready);

    world.prepare("t1");
    assert!(world.dispatch("t1", T0).is_ok());
}

#[test]
fn development_tasks_each_run_in_their_own_worktree_and_later_runs_reuse_it() {
    let mut world = World::new(&[]);
    for id in ["d1", "d2"] {
        let create = TaskCommand::CreateTask {
            id: id.into(),
            goal_id: "g1".into(),
            title: id.into(),
            kind: TaskKind::Development,
            contract: complete(vec![]),
        };
        world.task_command(create).unwrap();
        world.task_command(TaskCommand::MarkReady { task_id: id.into() }).unwrap();
        world.prepare(id);
    }

    let dispatch = |world: &mut World, run_id: &str, task_id: &str, now| {
        let command = RunCommand::Dispatch { run_id: run_id.into(), task_id: task_id.into(), profile_id: "p1".into() };
        world.step(command, now).unwrap().outbox.unwrap().payload.working_dir
    };
    assert_eq!(dispatch(&mut world, "r1", "d1", T0), r"C:\work\app\.baton\worktrees\d1");
    assert_eq!(dispatch(&mut world, "r2", "d2", T0), r"C:\work\app\.baton\worktrees\d2");

    // 第一個 Run 失敗後重新派工：同一個 Task 回到同一個 worktree（原生 Session 才接得回來）
    world.step(RunCommand::BeginLaunch { run_id: "r1".into(), command_id: "c1".into() }, later(1)).unwrap();
    world.step(RunCommand::RecordReceipt { run_id: "r1".into(), process: process(1) }, later(1)).unwrap();
    let failed = RunOutcome::Failed { reason: "x".into() };
    world.step(RunCommand::Finish { run_id: "r1".into(), generation: 1, outcome: failed }, later(2)).unwrap();
    assert_eq!(dispatch(&mut world, "r3", "d1", later(3)), r"C:\work\app\.baton\worktrees\d1");
}

#[test]
fn dispatch_is_refused_unless_the_task_is_ready() {
    let mut world = World::new(&["t1"]);
    world.task_command(TaskCommand::Cancel { task_id: "t1".into() }).unwrap();

    let result = world.dispatch("t1", T0);

    assert_eq!(result, Err(RunError::TaskNotReady(TaskStatus::Cancelled)));
    assert!(world.runs.is_empty() && world.leases.is_empty());
}

#[test]
fn dispatch_re_checks_admission_so_a_missing_capability_blocks_it() {
    let mut world = World::new(&["t1"]);
    // 契約改成需要寫檔：Task 退回 pending，但這裡直接模擬「ready 之後授權不足」
    let contract = TaskContract {
        content: complete(vec![Capability::WriteFiles]),
        version: 2,
        ..world.contracts[0].clone()
    };
    world.contracts[0] = contract;
    world.tasks[0].contract_version = 2;

    let result = world.dispatch("t1", T0);

    assert!(matches!(result, Err(RunError::AdmissionDenied(ref missing)) if !missing.is_empty()), "{result:?}");
    assert_eq!(world.task("t1").status, TaskStatus::Ready, "被拒絕時 Task 不變");
}

/// 一個 Workspace 範圍的有效教訓轉成的 Guard。
fn workspace_guard(id: &str, rule: GuardRule) -> ActiveGuard {
    let content = KnowledgeContent::new(KnowledgeKind::Lesson, "部署要先備份", "", &[], &[], None).unwrap();
    let new = NewKnowledge {
        id: format!("lesson-{id}"),
        kind: KnowledgeKind::Lesson,
        scope: KnowledgeScope { kind: knowledge::ScopeKind::Workspace, workspace_id: Some("w1".into()), goal_id: None, task_id: None },
        content,
        source: KnowledgeSource::User,
        valid_from: None,
        valid_until: None,
    };
    let lesson = knowledge::create(new, None, T0).unwrap().entries.remove(0);
    let guard = guard::create(id.into(), &lesson, rule, T0).unwrap().guard;
    ActiveGuard { guard, lesson }
}

#[test]
fn dispatch_runs_the_applicable_guards_and_a_violation_blocks_it_with_the_source_lesson() {
    let mut world = World::new(&["t1"]);
    world.guards.push(workspace_guard(
        "g1",
        GuardRule::Preflight { field: ContractFieldName::DoneCriteria, must_mention: "備份".into() },
    ));

    let result = world.dispatch("t1", T0);

    let Err(RunError::GuardViolated(violations)) = &result else { panic!("{result:?}") };
    assert_eq!(violations.len(), 1);
    assert_eq!((violations[0].guard_id.as_str(), violations[0].lesson_title.as_str()), ("g1", "部署要先備份"));
    assert_eq!(result.as_ref().unwrap_err().code(), "guard_violated");
    assert!(world.runs.is_empty() && world.leases.is_empty(), "被 Guard 擋下時什麼都不寫");
    assert_eq!(world.task("t1").status, TaskStatus::Ready);

    // 符合規則的 Guard 不擋
    world.guards[0] = workspace_guard(
        "g1",
        GuardRule::Preflight { field: ContractFieldName::DoneCriteria, must_mention: "五個".into() },
    );
    assert!(world.dispatch("t1", T0).is_ok());
}

#[test]
fn dispatch_is_refused_while_a_dependency_is_unfinished() {
    let mut world = World::new(&["a", "b"]);
    world.task_command(TaskCommand::AddDependency { task_id: "b".into(), depends_on: "a".into() }).unwrap();

    let blocked = world.dispatch("b", T0);
    let free = world.dispatch("a", T0);

    assert_eq!(blocked, Err(RunError::DependenciesUnfinished(vec!["a".into()])));
    assert!(free.is_ok());
}

#[test]
fn a_held_lease_blocks_a_second_dispatch_even_if_the_task_looks_ready() {
    let mut world = World::new(&["t1"]);
    world.dispatch("t1", T0).unwrap();
    // 模擬資料不一致：Task 狀態被改回 ready，但舊 Run 的 lease 還沒釋放
    world.tasks[0].status = TaskStatus::Ready;

    let result = world.dispatch("t1", T0);

    assert_eq!(result, Err(RunError::ExecutionConflict { task_id: "t1".into(), run_id: "run1".into() }));
}

#[test]
fn generations_increase_for_each_new_run_of_the_same_task() {
    let mut world = World::new(&["t1"]);
    let first = world.running("t1", T0);
    world.step(RunCommand::Interrupt { run_id: first, reason: "測試".into() }, T0).unwrap();

    let second = world.dispatch("t1", T0).unwrap();

    assert_eq!(world.run(&second).generation, 2);
}

// ---------------------------------------------------------------- 啟動與回執

#[test]
fn launch_intent_and_receipt_are_recorded_in_order_and_are_idempotent() {
    let mut world = World::new(&["t1"]);
    let run_id = world.dispatch("t1", T0).unwrap();

    let begin = RunCommand::BeginLaunch { run_id: run_id.clone(), command_id: "cmd-1".into() };
    world.step(begin.clone(), later(1_000)).unwrap();
    let again = world.step(begin, later(2_000)).unwrap();
    let receipt = RunCommand::RecordReceipt { run_id: run_id.clone(), process: process(77) };
    let recorded = world.step(receipt.clone(), later(3_000)).unwrap();
    let repeated = world.step(receipt, later(4_000)).unwrap();

    assert!(!again.changed(), "重複寫入同一個 launch intent 不產生任何變動");
    assert_eq!(recorded.outbox_delivered.as_deref(), Some(run_id.as_str()));
    assert!(!repeated.changed());
    let run = world.run(&run_id);
    assert_eq!((run.status, run.command_id.as_deref(), run.process.as_ref().map(|p| p.pid)), (RunStatus::Running, Some("cmd-1"), Some(77)));
    assert_eq!(run.started_at, Some(later(3_000)));
    assert_eq!(world.lease(&run_id).expires_at, later(3_000).plus_millis(LEASE_DURATION_MS));
}

#[test]
fn a_receipt_cannot_skip_the_launch_intent() {
    let mut world = World::new(&["t1"]);
    let run_id = world.dispatch("t1", T0).unwrap();

    let result = world.step(RunCommand::RecordReceipt { run_id, process: process(1) }, T0);

    assert_eq!(result, Err(RunError::TransitionInvalid { from: RunStatus::Queued, to: RunStatus::Running }));
}

#[test]
fn a_failed_launch_releases_the_lease_and_returns_the_task_to_ready() {
    let mut world = World::new(&["t1"]);
    let run_id = world.dispatch("t1", T0).unwrap();
    world.step(RunCommand::BeginLaunch { run_id: run_id.clone(), command_id: "c".into() }, T0).unwrap();

    world.step(RunCommand::LaunchFailed { run_id: run_id.clone(), reason: "找不到程式".into() }, T0).unwrap();

    assert_eq!(world.run(&run_id).status, RunStatus::Failed);
    assert!(!world.lease(&run_id).is_held());
    assert_eq!(world.task("t1").status, TaskStatus::Ready);
}

// ---------------------------------------------------------------- 結束

#[test]
fn a_successful_exit_only_moves_the_task_to_verifying_never_completed() {
    let mut world = World::new(&["t1"]);
    let run_id = world.running("t1", T0);

    let change = world
        .step(RunCommand::Finish { run_id: run_id.clone(), generation: 1, outcome: RunOutcome::Succeeded }, later(5_000))
        .unwrap();

    assert_eq!(world.run(&run_id).status, RunStatus::Succeeded);
    assert_eq!(world.task("t1").status, TaskStatus::Verifying);
    assert!(!world.lease(&run_id).is_held());
    assert_eq!(world.lease(&run_id).released_at, Some(later(5_000)));
    assert!(change.events.iter().any(|e| e.event_type == EVENT_RUN_SUCCEEDED));
}

#[test]
fn a_failed_exit_returns_the_task_to_ready_and_frees_it_for_another_run() {
    let mut world = World::new(&["t1"]);
    let run_id = world.running("t1", T0);

    world
        .step(
            RunCommand::Finish {
                run_id: run_id.clone(),
                generation: 1,
                outcome: RunOutcome::Failed { reason: "exit 1".into() },
            },
            T0,
        )
        .unwrap();

    assert_eq!(world.run(&run_id).status, RunStatus::Failed);
    assert_eq!(world.task("t1").status, TaskStatus::Ready);
    assert!(world.dispatch("t1", T0).is_ok());
}

#[test]
fn reports_from_an_old_generation_or_a_finished_run_change_nothing() {
    let mut world = World::new(&["t1"]);
    let first = world.running("t1", T0);
    world.step(RunCommand::Interrupt { run_id: first.clone(), reason: "核對".into() }, T0).unwrap();
    let second = world.running("t1", T0);
    let before = world.clone();

    let old_run = world.step(RunCommand::Finish { run_id: first.clone(), generation: 1, outcome: RunOutcome::Succeeded }, T0);
    let old_heartbeat = world.step(RunCommand::Heartbeat { run_id: first, generation: 1 }, T0);
    let wrong_generation =
        world.step(RunCommand::Finish { run_id: second.clone(), generation: 1, outcome: RunOutcome::Succeeded }, T0);

    assert_eq!(old_run, Err(RunError::StaleReport));
    assert_eq!(old_heartbeat, Err(RunError::StaleReport));
    assert_eq!(wrong_generation, Err(RunError::StaleReport));
    assert!(world == before, "過期的回報不改變任何狀態");
    assert_eq!(world.run(&second).status, RunStatus::Running);
}

// ---------------------------------------------------------------- Worker 回報（Agent API）

fn report(run_id: &str, generation: i64, content: ReportContent) -> RunCommand {
    RunCommand::Report { run_id: run_id.into(), generation, content }
}

#[test]
fn a_progress_report_renews_the_lease_and_is_recorded_for_audit() {
    let mut world = World::new(&["t1"]);
    let run_id = world.running("t1", T0);
    let content = ReportContent::progress("完成一半").unwrap();

    let change = world.step(report(&run_id, 1, content.clone()), later(45_000)).unwrap();

    assert_eq!(world.lease(&run_id).expires_at, later(45_000 + LEASE_DURATION_MS));
    let record = change.report.expect("被接受的回報要留下稽核紀錄");
    assert_eq!((record.run_id.as_str(), record.generation, record.accepted), (run_id.as_str(), 1, true));
    assert_eq!(record.content, content);
    assert_eq!(change.events.iter().map(|e| e.event_type).collect::<Vec<_>>(), [EVENT_RUN_PROGRESS_REPORTED]);
    assert_eq!(world.run(&run_id).status, RunStatus::Running);
}

#[test]
fn a_checkpoint_event_carries_only_the_summary_not_the_details() {
    let mut world = World::new(&["t1"]);
    let run_id = world.running("t1", T0);

    let change = world.step(report(&run_id, 1, ReportContent::checkpoint("寫完第一章", "很長的細節").unwrap()), T0).unwrap();

    let payload = change.events[0].payload.to_string();
    assert!(payload.contains("寫完第一章") && !payload.contains("很長的細節"));
}

#[test]
fn a_blocked_report_moves_a_running_task_to_blocked_and_a_later_exit_does_not_revive_it() {
    let mut world = World::new(&["t1"]);
    let run_id = world.running("t1", T0);

    let change = world.step(report(&run_id, 1, ReportContent::blocked("缺少 API 金鑰").unwrap()), T0).unwrap();
    assert_eq!(change.task.as_ref().map(|t| t.status), Some(TaskStatus::Blocked));
    assert_eq!(world.task("t1").status, TaskStatus::Blocked);
    assert_eq!(world.run(&run_id).status, RunStatus::Running, "回報受阻不結束 Run，程序結束才結束");

    world.step(RunCommand::Finish { run_id, generation: 1, outcome: RunOutcome::Succeeded }, T0).unwrap();
    assert_eq!(world.task("t1").status, TaskStatus::Blocked, "Run 結束不把受阻的 Task 拉回 verifying 或 ready");
}

#[test]
fn reports_from_a_superseded_or_finished_run_are_refused_and_change_nothing() {
    let mut world = World::new(&["t1"]);
    let first = world.running("t1", T0);
    world.step(RunCommand::Interrupt { run_id: first.clone(), reason: "核對".into() }, T0).unwrap();
    let second = world.running("t1", T0);
    let before = world.clone();

    for content in [
        ReportContent::progress("遲到的進度").unwrap(),
        ReportContent::checkpoint("遲到的 checkpoint", "").unwrap(),
        ReportContent::blocked("遲到的受阻").unwrap(),
    ] {
        assert_eq!(world.step(report(&first, 1, content.clone()), later(1)), Err(RunError::StaleReport));
        // 世代對不上目前的 Run 也一樣
        assert_eq!(world.step(report(&second, 1, content), later(1)), Err(RunError::StaleReport));
    }
    assert!(world == before);
    assert_eq!(world.task("t1").status, TaskStatus::Running);
}

#[test]
fn a_report_before_the_launch_intent_is_refused() {
    let mut world = World::new(&["t1"]);
    let run_id = world.dispatch("t1", T0).unwrap();

    let result = world.step(report(&run_id, 1, ReportContent::progress("太早").unwrap()), T0);

    assert!(matches!(result, Err(RunError::TransitionInvalid { .. })));
}

#[test]
fn the_run_view_shows_the_latest_accepted_report_of_each_kind_only() {
    let mut world = World::new(&["t1"]);
    let run_id = world.running("t1", T0);
    let at = |ms: i64, accepted: bool, content: ReportContent| ReportRecord {
        workspace_id: "w1".into(),
        run_id: run_id.clone(),
        generation: 1,
        accepted,
        content,
        received_at: later(ms),
    };
    let reports = vec![
        at(1, true, ReportContent::progress("第一步").unwrap()),
        at(2, true, ReportContent::progress("第二步").unwrap()),
        at(3, false, ReportContent::progress("過期世代的進度").unwrap()),
        at(4, true, ReportContent::checkpoint("存檔", "細節").unwrap()),
    ];

    let view = RunView::new(world.run(&run_id).clone(), None, &reports);

    assert_eq!(view.latest_progress, Some(ProgressNote { summary: "第二步".into(), reported_at: later(2) }));
    assert_eq!(
        view.latest_checkpoint,
        Some(CheckpointNote { summary: "存檔".into(), details: "細節".into(), saved_at: later(4) })
    );
    assert_eq!(view.blocked, None);
}

// ---------------------------------------------------------------- heartbeat 與 lease

#[test]
fn a_heartbeat_renews_the_lease_without_adding_events() {
    let mut world = World::new(&["t1"]);
    let run_id = world.running("t1", T0);

    let change = world.step(RunCommand::Heartbeat { run_id: run_id.clone(), generation: 1 }, later(45_000)).unwrap();

    assert!(change.events.is_empty() && change.run.is_none());
    let lease = world.lease(&run_id);
    assert_eq!((lease.last_heartbeat_at, lease.expires_at), (later(45_000), later(45_000 + LEASE_DURATION_MS)));
}

#[test]
fn an_expired_lease_only_triggers_a_check_and_never_decides_death_by_itself() {
    let mut world = World::new(&["t1"]);
    let run_id = world.running("t1", T0);
    let reconcile = |observed| RunCommand::ReconcileLease { run_id: run_id.clone(), observed };

    // 還沒過期：不需要核對
    let early = world.step(reconcile(ProcessObservation::Dead), later(LEASE_DURATION_MS));
    assert_eq!(early, Err(RunError::LeaseNotExpired));

    let after = later(LEASE_DURATION_MS + 1);
    // 過期了，但無法確認程序狀態：什麼都不改
    let unknown = world.step(reconcile(ProcessObservation::Unknown), after).unwrap();
    assert!(!unknown.changed());
    assert_eq!(world.run(&run_id).status, RunStatus::Running);
    assert!(world.lease(&run_id).is_held());

    // 過期且確認程序還活著：續約，Run 不變
    let alive = world.step(reconcile(ProcessObservation::Alive), after).unwrap();
    assert!(alive.events.iter().any(|e| e.event_type == EVENT_RUN_LEASE_VERIFIED));
    assert_eq!(world.run(&run_id).status, RunStatus::Running);
    assert_eq!(world.lease(&run_id).expires_at, after.plus_millis(LEASE_DURATION_MS));

    // 再次過期，這次確認程序已經不在：才中斷 Run、釋放所有權、Task 退回 ready
    let dead_at = after.plus_millis(LEASE_DURATION_MS + 1);
    world.step(reconcile(ProcessObservation::Dead), dead_at).unwrap();
    assert_eq!(world.run(&run_id).status, RunStatus::Interrupted);
    assert!(!world.lease(&run_id).is_held());
    assert_eq!(world.task("t1").status, TaskStatus::Ready);
}

// ---------------------------------------------------------------- 取消

#[test]
fn cancelling_a_queued_run_takes_effect_immediately() {
    let mut world = World::new(&["t1"]);
    let run_id = world.dispatch("t1", T0).unwrap();

    let change = world.step(RunCommand::RequestCancel { run_id: run_id.clone() }, T0).unwrap();

    assert_eq!(world.run(&run_id).status, RunStatus::Cancelled);
    assert!(!world.lease(&run_id).is_held());
    assert_eq!(world.task("t1").status, TaskStatus::Ready);
    assert_eq!(change.outbox_delivered.as_deref(), Some(run_id.as_str()));
}

#[test]
fn cancelling_a_running_run_keeps_the_lease_until_the_process_is_confirmed_stopped() {
    let mut world = World::new(&["t1"]);
    let run_id = world.running("t1", T0);

    world.step(RunCommand::RequestCancel { run_id: run_id.clone() }, later(1_000)).unwrap();
    let repeated = world.step(RunCommand::RequestCancel { run_id: run_id.clone() }, later(2_000)).unwrap();

    assert!(!repeated.changed(), "重複要求取消是冪等的");
    assert_eq!(world.run(&run_id).status, RunStatus::Running);
    assert_eq!(world.run(&run_id).cancel_requested_at, Some(later(1_000)));
    assert!(world.lease(&run_id).is_held(), "程序還沒確認停止，不能釋放寫入所有權");
    assert_eq!(world.dispatch("t1", later(2_000)).unwrap_err().code(), "task_not_ready");

    world.step(RunCommand::ConfirmCancelled { run_id: run_id.clone() }, later(3_000)).unwrap();

    assert_eq!(world.run(&run_id).status, RunStatus::Cancelled);
    assert!(!world.lease(&run_id).is_held());
    assert_eq!(world.task("t1").status, TaskStatus::Ready);
}

#[test]
fn confirming_a_cancel_that_was_never_requested_is_refused() {
    let mut world = World::new(&["t1"]);
    let run_id = world.running("t1", T0);

    let result = world.step(RunCommand::ConfirmCancelled { run_id }, T0);

    assert_eq!(result, Err(RunError::CancelNotRequested));
}

#[test]
fn a_run_ending_does_not_revive_a_task_the_user_already_cancelled() {
    let mut world = World::new(&["t1"]);
    let run_id = world.running("t1", T0);
    world.step(RunCommand::RequestCancel { run_id: run_id.clone() }, T0).unwrap();
    world.task_command(TaskCommand::Cancel { task_id: "t1".into() }).unwrap();

    world.step(RunCommand::ConfirmCancelled { run_id: run_id.clone() }, T0).unwrap();

    assert_eq!(world.run(&run_id).status, RunStatus::Cancelled);
    assert_eq!(world.task("t1").status, TaskStatus::Cancelled);
}

// ---------------------------------------------------------------- 性質測試

/// 隨機命令序列。每個索引都對應到「目前存在的第 n 個 Run／Task」，不存在時略過。
#[derive(Debug, Clone)]
enum Op {
    Dispatch(usize),
    BeginLaunch(usize),
    Receipt(usize),
    LaunchFailed(usize),
    Heartbeat(usize, bool),
    Finish(usize, bool, bool),
    Interrupt(usize),
    Reconcile(usize, u8),
    RequestCancel(usize),
    ConfirmCancel(usize),
    MakeReady(usize),
    CancelTask(usize),
    Advance(u32),
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        (0..3usize).prop_map(Op::Dispatch),
        (0..6usize).prop_map(Op::BeginLaunch),
        (0..6usize).prop_map(Op::Receipt),
        (0..6usize).prop_map(Op::LaunchFailed),
        (0..6usize, any::<bool>()).prop_map(|(i, stale)| Op::Heartbeat(i, stale)),
        (0..6usize, any::<bool>(), any::<bool>()).prop_map(|(i, ok, stale)| Op::Finish(i, ok, stale)),
        (0..6usize).prop_map(Op::Interrupt),
        (0..6usize, 0..3u8).prop_map(|(i, o)| Op::Reconcile(i, o)),
        (0..6usize).prop_map(Op::RequestCancel),
        (0..6usize).prop_map(Op::ConfirmCancel),
        (0..3usize).prop_map(Op::MakeReady),
        (0..3usize).prop_map(Op::CancelTask),
        (1..90_000u32).prop_map(Op::Advance),
    ]
}

fn check_invariants(world: &World) {
    for task in &world.tasks {
        let held: Vec<_> = world.leases.iter().filter(|l| l.task_id == task.id && l.is_held()).collect();
        assert!(held.len() <= 1, "Task {} 同時有 {} 個有效寫入所有權", task.id, held.len());
        if task.status == TaskStatus::Running {
            assert_eq!(held.len(), 1, "running 的 Task 必須恰好有一個寫入所有者");
        }
        assert_ne!(task.status, TaskStatus::Completed, "程序退出不能把 Task 標為 completed");
        let mut generations: Vec<_> = world.runs.iter().filter(|r| r.task_id == task.id).map(|r| r.generation).collect();
        let count = generations.len();
        generations.sort_unstable();
        generations.dedup();
        assert_eq!(generations.len(), count, "同一個 Task 的世代不可重複");
    }
    for run in &world.runs {
        let lease = world.lease(&run.id);
        assert_eq!(lease.is_held(), !run.status.is_terminal(), "lease 是否持有必須和 Run 是否在進行一致：{run:?} {lease:?}");
        assert_eq!(lease.generation, run.generation);
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// 同一 Task 任何時刻最多一個有效寫入所有權；被拒絕（含過期世代）的命令不改變任何狀態。
    #[test]
    fn at_most_one_effective_writer_per_task_under_any_command_sequence(ops in proptest::collection::vec(op(), 1..80)) {
        let task_ids = ["t1", "t2", "t3"];
        let mut world = World::new(&task_ids);
        let mut now = T0;
        for op in ops {
            let run_of = |world: &World, i: usize| world.runs.get(i % world.runs.len().max(1)).map(|r| (r.id.clone(), r.generation));
            let command = match op {
                Op::Advance(ms) => { now = now.plus_millis(i64::from(ms)); continue; }
                Op::MakeReady(i) => {
                    // Task 回到 ready 要重新通過准入；這是 Task 自己的命令，不經過 Run
                    let _ = world.task_command(TaskCommand::MarkReady { task_id: task_ids[i % 3].into() });
                    check_invariants(&world);
                    continue;
                }
                Op::CancelTask(i) => {
                    // 使用者取消 Task：同一個交易會要求取消它的 Run（Store 的連動），這裡直接模擬連動
                    let task_id = task_ids[i % 3];
                    let active = world.runs.iter().find(|r| r.task_id == task_id && !r.status.is_terminal()).map(|r| r.id.clone());
                    if world.task_command(TaskCommand::Cancel { task_id: task_id.into() }).is_ok() {
                        if let Some(run_id) = active {
                            let _ = world.step(RunCommand::RequestCancel { run_id }, now);
                        }
                    }
                    check_invariants(&world);
                    continue;
                }
                Op::Dispatch(i) => {
                    world.next_run += 1;
                    RunCommand::Dispatch { run_id: format!("run{}", world.next_run), task_id: task_ids[i % 3].into(), profile_id: "p1".into() }
                }
                Op::BeginLaunch(i) => match run_of(&world, i) { Some((run_id, _)) => RunCommand::BeginLaunch { command_id: format!("cmd-{run_id}"), run_id }, None => continue },
                Op::Receipt(i) => match run_of(&world, i) { Some((run_id, _)) => RunCommand::RecordReceipt { run_id, process: process(5) }, None => continue },
                Op::LaunchFailed(i) => match run_of(&world, i) { Some((run_id, _)) => RunCommand::LaunchFailed { run_id, reason: "x".into() }, None => continue },
                Op::Heartbeat(i, stale) => match run_of(&world, i) { Some((run_id, generation)) => RunCommand::Heartbeat { run_id, generation: generation + i64::from(stale) }, None => continue },
                Op::Finish(i, ok, stale) => match run_of(&world, i) {
                    Some((run_id, generation)) => RunCommand::Finish {
                        run_id,
                        generation: generation + i64::from(stale),
                        outcome: if ok { RunOutcome::Succeeded } else { RunOutcome::Failed { reason: "x".into() } },
                    },
                    None => continue,
                },
                Op::Interrupt(i) => match run_of(&world, i) { Some((run_id, _)) => RunCommand::Interrupt { run_id, reason: "x".into() }, None => continue },
                Op::Reconcile(i, o) => match run_of(&world, i) {
                    Some((run_id, _)) => RunCommand::ReconcileLease {
                        run_id,
                        observed: [ProcessObservation::Alive, ProcessObservation::Dead, ProcessObservation::Unknown][usize::from(o) % 3],
                    },
                    None => continue,
                },
                Op::RequestCancel(i) => match run_of(&world, i) { Some((run_id, _)) => RunCommand::RequestCancel { run_id }, None => continue },
                Op::ConfirmCancel(i) => match run_of(&world, i) { Some((run_id, _)) => RunCommand::ConfirmCancelled { run_id }, None => continue },
            };
            let before = world.clone();
            if world.step(command, now).is_err() {
                prop_assert!(world == before, "被拒絕的命令不可改變狀態");
            }
            check_invariants(&world);
        }
    }
}

// ---------------------------------------------------------------- 原生 Session 與模型

fn bind(world: &mut World, run_id: &str, session: &str) -> Result<RunChange, RunError> {
    let generation = world.run(run_id).generation;
    let command =
        RunCommand::BindNativeSession { run_id: run_id.into(), generation, native_session_id: session.into() };
    world.step(command, later(1_000))
}

#[test]
fn the_first_native_session_id_is_bound_to_the_run_with_an_event() {
    let mut world = World::new(&["t1"]);
    let run_id = world.running("t1", T0);

    let change = bind(&mut world, &run_id, "sess-a").unwrap();

    assert_eq!(world.run(&run_id).native_session_id.as_deref(), Some("sess-a"));
    assert_eq!(change.events.len(), 1);
    assert_eq!(change.events[0].event_type, EVENT_RUN_NATIVE_SESSION_BOUND);
    assert_eq!(change.events[0].payload["native_session_id"], "sess-a");
}

#[test]
fn reporting_the_same_native_session_again_changes_nothing() {
    let mut world = World::new(&["t1"]);
    let run_id = world.running("t1", T0);
    bind(&mut world, &run_id, "sess-a").unwrap();
    let before = world.run(&run_id).clone();

    let change = bind(&mut world, &run_id, "sess-a").unwrap();

    assert!(!change.changed(), "重新接上串流時重播的 Session 識別是冪等的");
    assert_eq!(world.run(&run_id), &before);
}

#[test]
fn a_different_native_session_never_replaces_the_bound_one_and_the_conflict_is_recorded() {
    let mut world = World::new(&["t1"]);
    let run_id = world.running("t1", T0);
    bind(&mut world, &run_id, "sess-a").unwrap();

    let change = bind(&mut world, &run_id, "sess-b").unwrap();

    assert_eq!(world.run(&run_id).native_session_id.as_deref(), Some("sess-a"), "不靜默改指其他原生 Session");
    assert_eq!(change.events.len(), 1);
    let event = &change.events[0];
    assert_eq!(event.event_type, EVENT_RUN_NATIVE_SESSION_CONFLICT);
    assert_eq!((event.payload["bound"].as_str(), event.payload["reported"].as_str()), (Some("sess-a"), Some("sess-b")));
}

#[test]
fn native_sessions_from_a_stale_generation_or_an_ended_run_are_rejected() {
    let mut world = World::new(&["t1"]);
    let run_id = world.running("t1", T0);
    let generation = world.run(&run_id).generation;

    let stale = RunCommand::BindNativeSession {
        run_id: run_id.clone(),
        generation: generation + 1,
        native_session_id: "sess-x".into(),
    };
    assert_eq!(world.step(stale, later(1)), Err(RunError::StaleReport));

    world.step(RunCommand::Finish { run_id: run_id.clone(), generation, outcome: RunOutcome::Succeeded }, later(2)).unwrap();
    assert_eq!(bind(&mut world, &run_id, "sess-x").map(|_| ()), Err(RunError::StaleReport));
    assert_eq!(world.run(&run_id).native_session_id, None);
}

#[test]
fn the_workers_model_override_is_fixed_into_the_launch_payload_at_dispatch() {
    let mut world = World::new(&["t1", "t2"]);
    world.workspace.model_overrides.worker = Some("claude-opus-5-5".into());
    world.workspace.model_overrides.controller = Some("sonnet".into());

    let change = world.step(RunCommand::Dispatch { run_id: "r1".into(), task_id: "t1".into(), profile_id: "p1".into() }, T0).unwrap();
    assert_eq!(change.outbox.unwrap().payload.model.as_deref(), Some("claude-opus-5-5"), "Worker Run 用 Worker 角色的覆寫");

    world.workspace.model_overrides.worker = None;
    let change = world.step(RunCommand::Dispatch { run_id: "r2".into(), task_id: "t2".into(), profile_id: "p1".into() }, T0).unwrap();
    assert_eq!(change.outbox.unwrap().payload.model, None, "沒有覆寫就不指定模型（spec D-30）");
}

#[test]
fn launch_payloads_stored_before_models_existed_have_no_model() {
    let old = r#"{"task_id":"t1","prompt":"p","working_dir":"C:\\a"}"#;

    let payload: LaunchPayload = serde_json::from_str(old).unwrap();

    assert_eq!(payload.model, None);
}
