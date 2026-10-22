use baton_domain::run::{RunOutcome, EVENT_RUN_CANCEL_REQUESTED};
use baton_domain::task::{ContractContent, ContractField, TaskCommand, TaskStatus};

use super::*;

const T1: Timestamp = Timestamp::from_millis(1_000);
const T2: Timestamp = Timestamp::from_millis(2_000);

fn filled(text: &str) -> Option<ContractField> {
    Some(ContractField::Filled { text: text.into(), references: vec![] })
}

fn complete() -> ContractContent {
    ContractContent {
        objective: filled("整理資料"),
        inputs: Some(ContractField::NotApplicable { reason: "沒有輸入".into() }),
        scope: filled("公開資料"),
        deliverables: filled("比較表"),
        done_criteria: filled("五個產品"),
        blocker_handling: filled("回報"),
        required_capabilities: vec![],
        verification_checks: vec![],
    }
}

/// 一個 Workspace 與一個 ready 的 Task（t1）。
async fn store_with_ready_task(dir: &Path) -> Store {
    let store = Store::open(dir, T1).await.unwrap();
    let create = WorkspaceCommand::Create { id: "w1".into(), name: None, path: "C:\\work\\app".into() };
    store.apply_workspace_command(create, T1).await.unwrap();
    let goal = TaskCommand::CreateGoal { id: "g1".into(), title: "目標".into() };
    store.apply_task_command("w1".into(), goal, T1).await.unwrap();
    let task = TaskCommand::CreateTask { id: "t1".into(), goal_id: "g1".into(), title: "t1".into(), contract: complete() };
    store.apply_task_command("w1".into(), task, T1).await.unwrap();
    store.apply_task_command("w1".into(), TaskCommand::MarkReady { task_id: "t1".into() }, T1).await.unwrap();
    store
}

async fn run_cmd(store: &Store, command: RunCommand, now: Timestamp) -> Result<RunChange, RunApplyError> {
    store.apply_run_command("w1".into(), command, now).await
}

fn dispatch(run_id: &str) -> RunCommand {
    RunCommand::Dispatch { run_id: run_id.into(), task_id: "t1".into(), profile_id: "p1".into() }
}

fn raw(dir: &Path) -> Connection {
    Connection::open(dir.join(DB_FILE_NAME)).unwrap()
}

#[tokio::test]
async fn dispatch_persists_run_lease_outbox_and_task_together_and_survives_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_with_ready_task(dir.path()).await;
    run_cmd(&store, dispatch("r1"), T2).await.unwrap();
    drop(store);

    let reopened = Store::open(dir.path(), T2).await.unwrap();
    let pending = reopened.pending_launches().await.unwrap();
    let board = reopened.task_board("w1").await.unwrap();

    assert_eq!(pending.len(), 1);
    assert_eq!((pending[0].run.id.as_str(), pending[0].run.status), ("r1", RunStatus::Queued));
    assert_eq!(pending[0].payload.working_dir, "C:\\work\\app");
    assert_eq!(board.tasks[0].task.status, TaskStatus::Running);
    assert_eq!(board.runs.len(), 1);
    assert!(board.runs[0].lease.as_ref().is_some_and(|l| l.is_held()));
    let events: Vec<String> = raw(dir.path())
        .prepare("SELECT event_type FROM event WHERE aggregate_type = 'run' ORDER BY seq")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(events, vec!["run_created".to_string()]);
}

#[tokio::test]
async fn launch_intent_receipt_and_outbox_acknowledgement_are_persisted_step_by_step() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_with_ready_task(dir.path()).await;
    run_cmd(&store, dispatch("r1"), T1).await.unwrap();

    run_cmd(&store, RunCommand::BeginLaunch { run_id: "r1".into(), command_id: "cmd-1".into(), working_dir: "C:/work".into(), resume_check: None }, T1).await.unwrap();
    let starting = store.executing_runs().await.unwrap();
    let process = ProcessRecord { pid: 4242, created_at_millis: 99, command_hash: "h".into() };
    run_cmd(&store, RunCommand::RecordReceipt { run_id: "r1".into(), process: process.clone() }, T2).await.unwrap();
    let running = store.executing_runs().await.unwrap();

    let (starting, running) = (&starting[0].view.run, &running[0].view.run);
    assert_eq!((starting.status, starting.command_id.as_deref()), (RunStatus::Starting, Some("cmd-1")));
    assert!(starting.process.is_none(), "回執寫回之前沒有程序身分");
    assert_eq!(running.process, Some(process));
    assert_eq!(running.started_at, Some(T2));
    assert!(store.pending_launches().await.unwrap().is_empty(), "回執寫回後 Outbox 項目視為處理完畢");
}

#[tokio::test]
async fn the_database_itself_refuses_a_second_held_lease_for_the_same_task() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_with_ready_task(dir.path()).await;
    run_cmd(&store, dispatch("r1"), T1).await.unwrap();

    let conn = raw(dir.path());
    conn.pragma_update(None, "foreign_keys", "ON").unwrap();
    conn.execute(
        "INSERT INTO run (id, workspace_id, task_id, status, version, generation, contract_version, created_at, updated_at)
         VALUES ('r2', 'w1', 't1', 'queued', 1, 2, 1, 1, 1)",
        [],
    )
    .unwrap();
    let second = conn.execute(
        "INSERT INTO execution_lease (run_id, task_id, generation, acquired_at, last_heartbeat_at, expires_at)
         VALUES ('r2', 't1', 2, 1, 1, 2)",
        [],
    );

    assert!(second.is_err(), "同一個 Task 不能同時有兩個尚未釋放的 lease");
}

#[tokio::test]
async fn rejected_and_unchanged_run_commands_write_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_with_ready_task(dir.path()).await;
    run_cmd(&store, dispatch("r1"), T1).await.unwrap();
    let count = || -> i64 { raw(dir.path()).query_row("SELECT COUNT(*) FROM event", [], |r| r.get(0)).unwrap() };
    let before = count();

    let second_dispatch = run_cmd(&store, dispatch("r2"), T2).await;
    let stale =
        run_cmd(&store, RunCommand::Finish { run_id: "r1".into(), generation: 9, outcome: RunOutcome::Succeeded }, T2).await;
    let repeated_intent = {
        run_cmd(&store, RunCommand::BeginLaunch { run_id: "r1".into(), command_id: "c".into(), working_dir: "C:/work".into(), resume_check: None }, T2).await.unwrap();
        let after_first = count();
        run_cmd(&store, RunCommand::BeginLaunch { run_id: "r1".into(), command_id: "c".into(), working_dir: "C:/work".into(), resume_check: None }, T2).await.unwrap();
        (after_first, count())
    };

    assert!(matches!(second_dispatch, Err(RunApplyError::Rejected(RunError::TaskNotReady(TaskStatus::Running)))));
    assert!(matches!(stale, Err(RunApplyError::Rejected(RunError::StaleReport))));
    assert_eq!(repeated_intent.0, before + 2, "第一次寫入 launch intent 追加 Run 與新 Session 各一個事件（票 15）");
    assert_eq!(repeated_intent.1, repeated_intent.0, "重複寫入同一個 launch intent 不追加事件");
}

#[tokio::test]
async fn cancelling_a_task_asks_its_active_run_to_cancel_in_the_same_transaction() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_with_ready_task(dir.path()).await;
    run_cmd(&store, dispatch("r1"), T1).await.unwrap();
    run_cmd(&store, RunCommand::BeginLaunch { run_id: "r1".into(), command_id: "c".into(), working_dir: "C:/work".into(), resume_check: None }, T1).await.unwrap();
    let process = ProcessRecord { pid: 1, created_at_millis: 1, command_hash: "h".into() };
    run_cmd(&store, RunCommand::RecordReceipt { run_id: "r1".into(), process }, T1).await.unwrap();

    store.apply_task_command("w1".into(), TaskCommand::Cancel { task_id: "t1".into() }, T2).await.unwrap();

    let view = store.run_detail("w1", "r1").await.unwrap();
    assert_eq!(view.run.cancel_requested_at, Some(T2));
    assert_eq!(view.run.status, RunStatus::Running, "程序還沒確認停止，Run 不會直接變成 cancelled");
    assert!(view.lease.unwrap().is_held());
    let board = store.task_board("w1").await.unwrap();
    assert_eq!(board.tasks[0].task.status, TaskStatus::Cancelled);
    let requested: i64 = raw(dir.path())
        .query_row("SELECT COUNT(*) FROM event WHERE event_type = ?1", [EVENT_RUN_CANCEL_REQUESTED], |r| r.get(0))
        .unwrap();
    assert_eq!(requested, 1);
}

#[tokio::test]
async fn cancelling_a_task_whose_run_has_not_launched_cancels_the_run_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_with_ready_task(dir.path()).await;
    run_cmd(&store, dispatch("r1"), T1).await.unwrap();

    store.apply_task_command("w1".into(), TaskCommand::Cancel { task_id: "t1".into() }, T2).await.unwrap();

    let view = store.run_detail("w1", "r1").await.unwrap();
    assert_eq!(view.run.status, RunStatus::Cancelled);
    assert!(!view.lease.unwrap().is_held());
    assert!(store.pending_launches().await.unwrap().is_empty());
    assert_eq!(store.task_board("w1").await.unwrap().tasks[0].task.status, TaskStatus::Cancelled);
}

#[tokio::test]
async fn run_queries_are_scoped_to_the_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_with_ready_task(dir.path()).await;
    let other = WorkspaceCommand::Create { id: "w2".into(), name: None, path: "C:\\work\\other".into() };
    store.apply_workspace_command(other, T1).await.unwrap();
    run_cmd(&store, dispatch("r1"), T1).await.unwrap();

    let in_other = store.run_list("w2").await.unwrap();
    let wrong_workspace = store.run_detail("w2", "r1").await;
    let unknown_workspace = store.run_list("nope").await;

    assert!(in_other.is_empty());
    assert!(matches!(wrong_workspace, Err(RunApplyError::Rejected(RunError::RunNotFound(_)))));
    assert!(matches!(unknown_workspace, Err(RunApplyError::Rejected(RunError::WorkspaceNotFound(_)))));
}
