use baton_domain::external_operation::{OPTION_VERIFIED_DONE, OPTION_VERIFIED_NOT_DONE};
use baton_domain::run::{ProcessRecord, RunCommand};
use baton_domain::task::{ContractContent, ContractField, TaskCommand, TaskStatus};
use baton_domain::workspace::WorkspaceCommand;

use super::*;

const T1: Timestamp = Timestamp::from_millis(1_000);
const T2: Timestamp = Timestamp::from_millis(2_000);

fn filled(text: &str) -> Option<ContractField> {
    Some(ContractField::Filled { text: text.into(), references: vec![] })
}

fn complete() -> ContractContent {
    ContractContent {
        objective: filled("部署"),
        inputs: Some(ContractField::NotApplicable { reason: "沒有輸入".into() }),
        scope: filled("正式環境"),
        deliverables: filled("新版本"),
        done_criteria: filled("上線"),
        blocker_handling: filled("回報"),
        required_capabilities: vec![],
        verification_checks: vec![],
    }
}

/// 一個 Workspace，兩個 Task（t1、t2）各有一個 running 的 Run（r1、r2，世代 1）。
async fn store_with_running_tasks(dir: &std::path::Path) -> Store {
    let store = Store::open(dir, T1).await.unwrap();
    let create = WorkspaceCommand::Create { id: "w1".into(), name: None, path: "C:\\work\\app".into() };
    store.apply_workspace_command(create, T1).await.unwrap();
    store.apply_task_command("w1".into(), TaskCommand::CreateGoal { id: "g1".into(), title: "目標".into() }, T1).await.unwrap();
    for (task, run) in [("t1", "r1"), ("t2", "r2")] {
        let command = TaskCommand::CreateTask { id: task.into(), goal_id: "g1".into(), title: task.into(), contract: complete() };
        store.apply_task_command("w1".into(), command, T1).await.unwrap();
        store.apply_task_command("w1".into(), TaskCommand::MarkReady { task_id: task.into() }, T1).await.unwrap();
        let commands = [
            RunCommand::Dispatch { run_id: run.into(), task_id: task.into(), profile_id: "p1".into() },
            RunCommand::BeginLaunch { run_id: run.into(), command_id: format!("cmd-{run}") },
            RunCommand::RecordReceipt {
                run_id: run.into(),
                process: ProcessRecord { pid: 1, created_at_millis: 1, command_hash: "h".into() },
            },
        ];
        for command in commands {
            store.apply_run_command("w1".into(), command, T1).await.unwrap();
        }
    }
    store
}

fn scope(run: &str) -> AgentScope {
    AgentScope { workspace_id: "w1".into(), run_id: run.into(), generation: 1 }
}

fn deploy(key: &str) -> OperationRequest {
    OperationRequest::new(key, "deploy", "production", serde_json::json!({ "version": "1.2.3" })).unwrap()
}

async fn task_status(store: &Store, task: &str) -> TaskStatus {
    store.task_board("w1").await.unwrap().tasks.into_iter().find(|t| t.task.id == task).unwrap().task.status
}

#[tokio::test]
async fn a_run_ending_with_an_unreported_operation_marks_it_unknown_in_the_same_transaction() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_with_running_tasks(dir.path()).await;
    store.begin_external_operation(&scope("r1"), "op1".into(), deploy("deploy-1"), T1).await.unwrap();

    let interrupt = RunCommand::Interrupt { run_id: "r1".into(), reason: "process_dead_after_lease_expired".into() };
    store.apply_run_command("w1".into(), interrupt, T2).await.unwrap();

    let operation = store.external_operations("w1").await.unwrap().remove(0);
    assert_eq!(operation.status, OperationStatus::Unknown);
    let request_id = operation.decision_request_id.clone().unwrap();
    let decision = store.decision_detail("w1", &request_id, T2).await.unwrap();
    assert_eq!(decision.status, baton_domain::decision::DecisionStatus::Pending);
    assert_eq!(task_status(&store, "t1").await, TaskStatus::WaitingDecision, "不會停在可派工的 ready");
    assert_eq!(task_status(&store, "t2").await, TaskStatus::Running, "獨立的 Task 不受影響");
}

#[tokio::test]
async fn answering_verified_done_closes_the_operation_and_resumes_the_task_together() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_with_running_tasks(dir.path()).await;
    store.begin_external_operation(&scope("r1"), "op1".into(), deploy("deploy-1"), T1).await.unwrap();
    let (_, _, request) = store
        .record_external_operation_result(&scope("r1"), "deploy-1".into(), OperationResult::unknown("逾時").unwrap(), T1)
        .await
        .unwrap();
    let request_id = request.unwrap();
    assert_eq!(task_status(&store, "t1").await, TaskStatus::WaitingDecision);
    store.apply_run_command("w1".into(), RunCommand::Interrupt { run_id: "r1".into(), reason: "x".into() }, T2).await.unwrap();

    let answer = store.answer_decision("w1", &request_id, OPTION_VERIFIED_DONE.into(), Some("看過紀錄".into()), T2).await.unwrap();

    assert_eq!(answer.resumed_task_ids, vec!["t1".to_string()]);
    let operation = store.external_operations("w1").await.unwrap().remove(0);
    assert_eq!(operation.status, OperationStatus::Succeeded);
    assert_eq!(task_status(&store, "t1").await, TaskStatus::Ready);
}

#[tokio::test]
async fn answering_verified_not_done_lets_the_next_run_try_again() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_with_running_tasks(dir.path()).await;
    store.begin_external_operation(&scope("r1"), "op1".into(), deploy("deploy-1"), T1).await.unwrap();
    store.apply_run_command("w1".into(), RunCommand::Interrupt { run_id: "r1".into(), reason: "x".into() }, T2).await.unwrap();
    let request_id = store.external_operations("w1").await.unwrap()[0].decision_request_id.clone().unwrap();

    store.answer_decision("w1", &request_id, OPTION_VERIFIED_NOT_DONE.into(), None, T2).await.unwrap();
    let operation = store.external_operations("w1").await.unwrap().remove(0);

    assert_eq!(operation.status, OperationStatus::Failed);
    // 另一個 Task 的 Run 用同一個鍵：這次可以執行（第二次嘗試）
    let next = store.begin_external_operation(&scope("r2"), "ignored".into(), deploy("deploy-1"), T2).await.unwrap();
    assert_eq!(next, BeginOutcome::Proceed { operation_id: "op1".into(), attempt: 2, existing: false });
}

#[tokio::test]
async fn another_task_hitting_an_unknown_operation_joins_the_same_decision() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_with_running_tasks(dir.path()).await;
    store.begin_external_operation(&scope("r1"), "op1".into(), deploy("deploy-1"), T1).await.unwrap();
    store.apply_run_command("w1".into(), RunCommand::Interrupt { run_id: "r1".into(), reason: "x".into() }, T2).await.unwrap();
    let request_id = store.external_operations("w1").await.unwrap()[0].decision_request_id.clone().unwrap();

    let outcome = store.begin_external_operation(&scope("r2"), "ignored".into(), deploy("deploy-1"), T2).await.unwrap();

    assert_eq!(outcome, BeginOutcome::ResultUnknown { operation_id: "op1".into(), decision_request_id: Some(request_id.clone()) });
    let decision = store.decision_detail("w1", &request_id, T2).await.unwrap();
    let blocked: Vec<&str> = decision.blocking.iter().map(|b| b.task_id.as_str()).collect();
    assert_eq!(blocked, vec!["t1", "t2"]);
    assert_eq!(task_status(&store, "t2").await, TaskStatus::WaitingDecision);
    let answer = store.answer_decision("w1", &request_id, OPTION_VERIFIED_DONE.into(), None, T2).await.unwrap();
    assert_eq!(answer.resumed_task_ids, vec!["t1".to_string(), "t2".to_string()]);
}

#[tokio::test]
async fn a_rejected_command_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_with_running_tasks(dir.path()).await;
    store.begin_external_operation(&scope("r1"), "op1".into(), deploy("deploy-1"), T1).await.unwrap();
    let other = OperationRequest::new("deploy-1", "deploy", "staging", serde_json::Value::Null).unwrap();

    let error = store.begin_external_operation(&scope("r1"), "op2".into(), other, T1).await.unwrap_err();

    assert!(matches!(error, ExternalApplyError::Rejected(ExternalOperationError::KeyConflict(_))), "{error}");
    let operations = store.external_operations("w1").await.unwrap();
    assert_eq!((operations.len(), operations[0].target.as_str()), (1, "production"));
}
