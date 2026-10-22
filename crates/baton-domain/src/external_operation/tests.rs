use super::*;
use crate::run::{ExecutionLease, Run, RunStatus};
use crate::task::{Goal, Task, TaskStatus};
use crate::workspace::{self, WorkspaceCommand};

const T0: Timestamp = Timestamp::from_millis(1_000_000);

fn later(ms: i64) -> Timestamp {
    T0.plus_millis(ms)
}

/// 記憶體中的一個 Workspace：兩個 Task 各有一個進行中的 Run，以及外部操作與待回答的請求。
struct World {
    workspace: crate::workspace::Workspace,
    tasks: Vec<Task>,
    runs: Vec<Run>,
    leases: Vec<ExecutionLease>,
    operations: Vec<ExternalOperation>,
    pending: Vec<String>,
    next: u32,
}

fn task(id: &str, status: TaskStatus) -> Task {
    Task {
        id: id.into(),
        workspace_id: "w1".into(),
        goal_id: "g1".into(),
        title: id.into(),
        status,
        version: 1,
        contract_version: 1,
        created_at: T0,
        updated_at: T0,
    }
}

fn run(id: &str, task_id: &str, generation: i64) -> Run {
    Run {
        id: id.into(),
        workspace_id: "w1".into(),
        task_id: task_id.into(),
        profile_id: "p1".into(),
        status: RunStatus::Running,
        version: 3,
        generation,
        contract_version: 1,
        authorization_revision: None,
        command_id: Some(format!("cmd-{id}")),
        process: None,
        cancel_requested_at: None,
        end_reason: None,
        created_at: T0,
        updated_at: T0,
        started_at: Some(T0),
        ended_at: None,
    }
}

fn lease(run: &Run) -> ExecutionLease {
    ExecutionLease {
        run_id: run.id.clone(),
        task_id: run.task_id.clone(),
        generation: run.generation,
        acquired_at: T0,
        last_heartbeat_at: T0,
        expires_at: later(60_000),
        released_at: None,
        release_reason: None,
    }
}

impl World {
    fn new() -> Self {
        let create = WorkspaceCommand::Create { id: "w1".into(), name: None, path: "C:\\work\\app".into() };
        let workspace = workspace::decide(&[], create, T0).unwrap().workspace;
        let runs = vec![run("r1", "t1", 1), run("r2", "t2", 1)];
        let leases = runs.iter().map(lease).collect();
        Self {
            workspace,
            tasks: vec![task("t1", TaskStatus::Running), task("t2", TaskStatus::Running)],
            runs,
            leases,
            operations: vec![],
            pending: vec![],
            next: 0,
        }
    }

    fn decide(&self, command: ExternalCommand, now: Timestamp) -> Result<ExternalChange, ExternalOperationError> {
        let goals: Vec<Goal> = vec![];
        let task_ctx = TaskContext {
            workspace_id: "w1",
            workspace: Some(&self.workspace),
            authorization: None,
            goals: &goals,
            tasks: &self.tasks,
            dependencies: &[],
            contracts: &[],
        };
        let ctx = ExternalContext {
            task: &task_ctx,
            runs: &self.runs,
            leases: &self.leases,
            operations: &self.operations,
            pending_requests: &self.pending,
        };
        decide(&ctx, command, now)
    }

    /// 判定並套用；被拒絕時狀態完全不變。
    fn step(&mut self, command: ExternalCommand, now: Timestamp) -> Result<ExternalChange, ExternalOperationError> {
        let change = self.decide(command, now)?;
        for operation in &change.operations {
            self.operations.retain(|o| o.id != operation.id);
            self.operations.push(operation.clone());
        }
        for task in &change.tasks {
            self.tasks.retain(|t| t.id != task.id);
            self.tasks.push(task.clone());
        }
        self.pending.extend(change.requests.iter().map(|r| r.id.clone()));
        Ok(change)
    }

    fn begin(&mut self, run_id: &str, key: &str, params: Value) -> Result<BeginOutcome, ExternalOperationError> {
        self.next += 1;
        let generation = self.runs.iter().find(|r| r.id == run_id).unwrap().generation;
        let request = OperationRequest::new(key, "deploy", "production", params).unwrap();
        let command = ExternalCommand::Begin { operation_id: format!("op{}", self.next), run_id: run_id.into(), generation, request };
        match self.step(command, T0)?.outcome {
            ExternalOutcome::Begin(outcome) => Ok(outcome),
            other => panic!("{other:?}"),
        }
    }

    fn record(&mut self, run_id: &str, key: &str, result: OperationResult) -> Result<ExternalChange, ExternalOperationError> {
        let generation = self.runs.iter().find(|r| r.id == run_id).unwrap().generation;
        self.step(ExternalCommand::RecordResult { run_id: run_id.into(), generation, idempotency_key: key.into(), result }, later(1_000))
    }

    /// Run 結束（Run 的狀態機另外處理；這裡只標記結束與釋放 lease）並結算它的外部操作。
    fn end_run(&mut self, run_id: &str, task_after: TaskStatus) -> ExternalChange {
        let run = self.runs.iter_mut().find(|r| r.id == run_id).unwrap();
        run.status = RunStatus::Interrupted;
        run.end_reason = Some("process_dead_after_lease_expired".into());
        let task_id = run.task_id.clone();
        self.leases.iter_mut().filter(|l| l.run_id == run_id).for_each(|l| l.released_at = Some(later(2_000)));
        self.tasks.iter_mut().filter(|t| t.id == task_id).for_each(|t| t.status = task_after);
        self.step(ExternalCommand::RunEnded { run_id: run_id.into() }, later(2_000)).unwrap()
    }

    fn operation(&self, key: &str) -> &ExternalOperation {
        self.operations.iter().find(|o| o.idempotency_key == key).unwrap()
    }

    fn task(&self, id: &str) -> &Task {
        self.tasks.iter().find(|t| t.id == id).unwrap()
    }

    /// 新的 Run 接手 Task（舊 Run 已結束）。
    fn new_run(&mut self, id: &str, task_id: &str) {
        let generation = self.runs.iter().filter(|r| r.task_id == task_id).count() as i64 + 1;
        let run = run(id, task_id, generation);
        self.leases.push(lease(&run));
        self.runs.push(run);
        self.tasks.iter_mut().filter(|t| t.id == task_id).for_each(|t| t.status = TaskStatus::Running);
    }
}

fn params() -> Value {
    serde_json::json!({ "version": "1.2.3" })
}

#[test]
fn a_new_key_may_proceed_and_records_the_request_hash() {
    let mut world = World::new();

    let outcome = world.begin("r1", "deploy-1", params()).unwrap();

    assert_eq!(outcome, BeginOutcome::Proceed { operation_id: "op1".into(), attempt: 1, existing: false });
    let operation = world.operation("deploy-1");
    assert_eq!((operation.status, operation.run_id.as_str(), operation.task_id.as_str()), (OperationStatus::InProgress, "r1", "t1"));
    assert_eq!(operation.request_hash.len(), 64);
}

#[test]
fn the_request_hash_ignores_key_order_and_the_key_itself() {
    let a = OperationRequest::new("k1", "deploy", "prod", serde_json::json!({ "a": 1, "b": 2 })).unwrap();
    let b = OperationRequest::new("k2", "deploy", "prod", serde_json::json!({ "b": 2, "a": 1 })).unwrap();
    let c = OperationRequest::new("k1", "deploy", "prod", serde_json::json!({ "a": 1, "b": 3 })).unwrap();

    assert_eq!(a.request_hash(), b.request_hash());
    assert_ne!(a.request_hash(), c.request_hash());
}

#[test]
fn a_repeated_begin_from_the_same_run_reuses_the_record() {
    let mut world = World::new();
    world.begin("r1", "deploy-1", params()).unwrap();

    let again = world.begin("r1", "deploy-1", params()).unwrap();

    assert_eq!(again, BeginOutcome::Proceed { operation_id: "op1".into(), attempt: 1, existing: true });
    assert_eq!(world.operations.len(), 1);
}

#[test]
fn a_key_reused_with_different_content_is_rejected_without_touching_the_record() {
    let mut world = World::new();
    world.begin("r1", "deploy-1", params()).unwrap();
    let before = world.operation("deploy-1").clone();

    let result = world.begin("r1", "deploy-1", serde_json::json!({ "version": "9.9.9" }));

    assert_eq!(result, Err(ExternalOperationError::KeyConflict("deploy-1".into())));
    assert_eq!(world.operation("deploy-1"), &before);
}

#[test]
fn a_succeeded_operation_is_never_sent_again_and_returns_its_receipt() {
    let mut world = World::new();
    world.begin("r1", "deploy-1", params()).unwrap();
    world.record("r1", "deploy-1", OperationResult::succeeded("release #42").unwrap()).unwrap();

    // 另一個 Task 用同一個鍵
    let other = world.begin("r2", "deploy-1", params()).unwrap();

    assert_eq!(other, BeginOutcome::AlreadySucceeded { operation_id: "op1".into(), receipt: Some("release #42".into()) });
    assert_eq!(world.operation("deploy-1").attempt, 1);
}

#[test]
fn recording_the_same_result_again_is_idempotent_but_a_different_one_is_refused() {
    let mut world = World::new();
    world.begin("r1", "deploy-1", params()).unwrap();
    world.record("r1", "deploy-1", OperationResult::succeeded("release #42").unwrap()).unwrap();

    let again = world.record("r1", "deploy-1", OperationResult::succeeded("release #42").unwrap()).unwrap();
    let different = world.record("r1", "deploy-1", OperationResult::failed("其實失敗").unwrap());

    assert!(!again.changed());
    assert_eq!(different, Err(ExternalOperationError::AlreadyRecorded(OperationStatus::Succeeded)));
}

#[test]
fn an_unknown_result_opens_a_decision_and_holds_only_the_related_task() {
    let mut world = World::new();
    world.begin("r1", "deploy-1", params()).unwrap();

    let change = world.record("r1", "deploy-1", OperationResult::unknown("部署 API 逾時").unwrap()).unwrap();

    let operation = world.operation("deploy-1");
    assert_eq!(operation.status, OperationStatus::Unknown);
    let request = &change.requests[0];
    assert_eq!(operation.decision_request_id.as_deref(), Some(request.id.as_str()));
    assert_eq!(request.blocking, vec!["t1".to_string()]);
    assert_eq!(request.options.iter().map(|o| o.id.as_str()).collect::<Vec<_>>(), vec![OPTION_VERIFIED_DONE, OPTION_VERIFIED_NOT_DONE]);
    assert!(request.reason.contains("部署 API 逾時"));
    assert_eq!(world.task("t1").status, TaskStatus::WaitingDecision);
    assert_eq!(world.task("t2").status, TaskStatus::Running, "獨立的 Task 不受影響");
    assert!(change.events.iter().any(|e| e.event_type == crate::decision::EVENT_DECISION_REQUESTED));
}

#[test]
fn an_unknown_operation_blocks_every_later_attempt_with_the_same_key() {
    let mut world = World::new();
    world.begin("r1", "deploy-1", params()).unwrap();
    world.record("r1", "deploy-1", OperationResult::unknown("逾時").unwrap()).unwrap();
    let request_id = world.operation("deploy-1").decision_request_id.clone();

    let same_run = world.begin("r1", "deploy-1", params()).unwrap();
    let other_task = world.decide(
        ExternalCommand::Begin {
            operation_id: "x".into(),
            run_id: "r2".into(),
            generation: 1,
            request: OperationRequest::new("deploy-1", "deploy", "production", params()).unwrap(),
        },
        T0,
    )
    .unwrap();

    let blocked = BeginOutcome::ResultUnknown { operation_id: "op1".into(), decision_request_id: request_id.clone() };
    assert_eq!(same_run, blocked);
    assert_eq!(other_task.outcome, ExternalOutcome::Begin(blocked));
    assert_eq!(other_task.blocks, vec![(request_id.unwrap(), "t2".to_string())], "另一個 Task 加入同一個請求的阻擋範圍");
    assert_eq!(other_task.tasks[0].status, TaskStatus::WaitingDecision);
    assert_eq!(world.operation("deploy-1").attempt, 1, "不會開始新的嘗試");
}

#[test]
fn a_run_that_ends_with_an_unreported_operation_turns_it_unknown_and_holds_the_task() {
    let mut world = World::new();
    world.begin("r1", "deploy-1", params()).unwrap();
    world.begin("r1", "notify-1", serde_json::json!({ "to": "ops" })).unwrap();
    world.record("r1", "notify-1", OperationResult::succeeded("msg-1").unwrap()).unwrap();

    // Run 中斷時 Task 已被退回 ready
    let change = world.end_run("r1", TaskStatus::Ready);

    assert_eq!(change.outcome, ExternalOutcome::Settled { unknown: vec!["op1".into()] });
    assert_eq!(world.operation("deploy-1").status, OperationStatus::Unknown);
    assert!(world.operation("deploy-1").unknown_reason.as_deref().unwrap().contains("process_dead_after_lease_expired"));
    assert_eq!(world.operation("notify-1").status, OperationStatus::Succeeded, "已回報的不受影響");
    assert_eq!(world.task("t1").status, TaskStatus::WaitingDecision);
    assert_eq!(change.requests.len(), 1);
    assert_eq!(world.task("t2").status, TaskStatus::Running);
}

#[test]
fn ending_a_run_twice_changes_nothing_the_second_time() {
    let mut world = World::new();
    world.begin("r1", "deploy-1", params()).unwrap();
    world.end_run("r1", TaskStatus::Verifying);

    let again = world.step(ExternalCommand::RunEnded { run_id: "r1".into() }, later(5_000)).unwrap();

    assert!(!again.changed());
    assert_eq!(world.task("t1").status, TaskStatus::WaitingDecision, "等待驗證的 Task 也要先等查證");
}

#[test]
fn verifying_done_closes_the_operation_as_succeeded_so_the_next_run_does_not_resend() {
    let mut world = World::new();
    world.begin("r1", "deploy-1", params()).unwrap();
    world.end_run("r1", TaskStatus::Ready);
    let request_id = world.operation("deploy-1").decision_request_id.clone().unwrap();

    let command = ExternalCommand::Verified { request_id, option_id: OPTION_VERIFIED_DONE.into(), note: Some("看過部署紀錄".into()) };
    world.step(command, later(3_000)).unwrap();
    world.new_run("r3", "t1");
    let next = world.begin("r3", "deploy-1", params()).unwrap();

    assert_eq!(world.operation("deploy-1").status, OperationStatus::Succeeded);
    assert!(matches!(next, BeginOutcome::AlreadySucceeded { receipt: Some(ref r), .. } if r.contains("看過部署紀錄")), "{next:?}");
}

#[test]
fn verifying_not_done_allows_exactly_one_new_attempt() {
    let mut world = World::new();
    world.begin("r1", "deploy-1", params()).unwrap();
    world.end_run("r1", TaskStatus::Ready);
    let request_id = world.operation("deploy-1").decision_request_id.clone().unwrap();

    world.step(ExternalCommand::Verified { request_id, option_id: OPTION_VERIFIED_NOT_DONE.into(), note: None }, later(3_000)).unwrap();
    world.new_run("r3", "t1");
    let next = world.begin("r3", "deploy-1", params()).unwrap();

    assert_eq!(next, BeginOutcome::Proceed { operation_id: "op1".into(), attempt: 2, existing: false });
    let operation = world.operation("deploy-1");
    assert_eq!((operation.status, operation.run_id.as_str(), operation.decision_request_id.as_deref()), (OperationStatus::InProgress, "r3", None));
}

#[test]
fn answers_to_other_requests_do_not_touch_any_operation() {
    let mut world = World::new();
    world.begin("r1", "deploy-1", params()).unwrap();

    let change = world
        .step(ExternalCommand::Verified { request_id: "decision_other".into(), option_id: OPTION_VERIFIED_DONE.into(), note: None }, T0)
        .unwrap();

    assert!(!change.changed());
    assert_eq!(world.operation("deploy-1").status, OperationStatus::InProgress);
}

#[test]
fn a_finished_or_replaced_run_cannot_begin_or_record() {
    let mut world = World::new();
    world.begin("r1", "deploy-1", params()).unwrap();
    world.end_run("r1", TaskStatus::Ready);

    let begin = world.begin("r1", "deploy-2", params());
    let record = world.record("r1", "deploy-1", OperationResult::succeeded("late").unwrap());

    assert_eq!(begin, Err(ExternalOperationError::StaleRun));
    assert_eq!(record, Err(ExternalOperationError::StaleRun));
}

#[test]
fn another_run_cannot_record_the_result_of_an_operation_it_did_not_begin() {
    let mut world = World::new();
    world.begin("r1", "deploy-1", params()).unwrap();

    let result = world.record("r2", "deploy-1", OperationResult::succeeded("x").unwrap());

    assert_eq!(result, Err(ExternalOperationError::NotInProgress(OperationStatus::InProgress)));
}

#[test]
fn an_operation_in_progress_in_another_live_run_is_not_started_twice() {
    let mut world = World::new();
    world.begin("r1", "deploy-1", params()).unwrap();

    let other = world.begin("r2", "deploy-1", params()).unwrap();

    assert_eq!(other, BeginOutcome::InProgressElsewhere { operation_id: "op1".into(), run_id: "r1".into() });
}

#[test]
fn requests_are_validated() {
    assert!(OperationRequest::new(" ", "deploy", "prod", Value::Null).is_err());
    assert!(OperationRequest::new("k", "", "prod", Value::Null).is_err());
    assert!(OperationRequest::new("k", "deploy", " ", Value::Null).is_err());
    let huge = Value::String("x".repeat(REQUEST_MAX_BYTES));
    assert!(OperationRequest::new("k", "deploy", "prod", huge).is_err());
    assert!(OperationResult::succeeded(" ").is_err());
    assert_eq!(OperationRequest::new(" k ", "deploy", "prod", Value::Null).unwrap().idempotency_key, "k");
}
