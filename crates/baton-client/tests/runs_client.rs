//! 桌面殼經由 RuntimeClient 派工與取消 Run（票 10）：與 App 實際使用的路徑相同（讀憑證檔、帶標頭）。

use std::sync::Arc;

use baton_api::{CreateTaskRequest, CreateWorkspaceRequest};
use baton_client::{ClientError, DispatchOutcome, RuntimeClient};
use baton_domain::run::{RunStatus, RunView};
use baton_domain::task::{ContractContent, ContractField, TaskStatus};
use baton_harness::fakes::FakeSecretProtector;
use baton_harness::Harness;

fn app_client(harness: &Harness) -> RuntimeClient {
    RuntimeClient::new(harness.addr(), harness.data_dir().to_path_buf(), Arc::new(FakeSecretProtector))
}

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
    }
}

async fn new_workspace(harness: &Harness, client: &RuntimeClient) -> String {
    let dir = harness.data_dir().join("app");
    std::fs::create_dir_all(&dir).unwrap();
    client
        .create_workspace(&CreateWorkspaceRequest { path: dir.to_string_lossy().into_owned(), name: None })
        .await
        .unwrap()
        .id
}

/// 派工結果必須是已建立的 Run；排隊中代表測試的前提（帳號、名額）不對。
fn started(outcome: DispatchOutcome) -> RunView {
    match outcome {
        DispatchOutcome::Started(view) => *view,
        DispatchOutcome::Queued(queued) => panic!("應該直接派出，卻排隊中：{queued:?}"),
    }
}

/// 建立一個 Task 並標為 ready；回傳 task id。
async fn ready_task(client: &RuntimeClient, workspace_id: &str) -> String {
    let goal = client.create_goal(workspace_id, "研究競品").await.unwrap();
    let created = client
        .create_task(
            workspace_id,
            &CreateTaskRequest { goal_id: goal.id.clone(), title: "整理".into(), contract: complete() },
        )
        .await
        .unwrap();
    client.mark_task_ready(workspace_id, &created.task.id).await.unwrap();
    created.task.id
}

#[tokio::test]
async fn app_dispatches_a_ready_task_and_gets_a_queued_run() {
    let harness = Harness::start().await;
    let profile_id = harness.add_account("主要帳號", "main@example.com").await;
    let client = app_client(&harness);
    let workspace_id = new_workspace(&harness, &client).await;
    let task_id = ready_task(&client, &workspace_id).await;

    let view = started(client.dispatch_task(&workspace_id, &task_id).await.unwrap());

    assert_eq!(view.run.status, RunStatus::Queued);
    assert_eq!(view.run.profile_id, profile_id);
    assert_eq!(view.run.task_id, task_id);
    assert!(view.lease.is_some());
    let board = client.task_board(&workspace_id).await.unwrap();
    assert_eq!(board.runs.len(), 1);
    assert_eq!(board.runs[0].run.id, view.run.id);
    assert_eq!(board.tasks[0].task.status, TaskStatus::Running);
}

#[tokio::test]
async fn app_cancels_a_run_that_has_not_started_and_the_task_returns_to_ready() {
    let harness = Harness::start().await;
    harness.add_account("主要帳號", "main@example.com").await;
    let client = app_client(&harness);
    let workspace_id = new_workspace(&harness, &client).await;
    let task_id = ready_task(&client, &workspace_id).await;
    let dispatched = started(client.dispatch_task(&workspace_id, &task_id).await.unwrap());

    let cancelled = client.cancel_run(&workspace_id, &dispatched.run.id).await.unwrap();
    let again = client.cancel_run(&workspace_id, &dispatched.run.id).await.unwrap();

    assert_eq!(cancelled.run.status, RunStatus::Cancelled);
    assert_eq!(again.run.status, RunStatus::Cancelled);
    let board = client.task_board(&workspace_id).await.unwrap();
    assert_eq!(board.tasks[0].task.status, TaskStatus::Ready);
    assert_eq!(board.runs[0].run.status, RunStatus::Cancelled);
}

#[tokio::test]
async fn app_is_told_why_a_task_that_is_not_ready_cannot_be_dispatched() {
    let harness = Harness::start().await;
    let client = app_client(&harness);
    let workspace_id = new_workspace(&harness, &client).await;
    let goal = client.create_goal(&workspace_id, "研究競品").await.unwrap();
    let pending = client
        .create_task(
            &workspace_id,
            &CreateTaskRequest { goal_id: goal.id.clone(), title: "整理".into(), contract: ContractContent::default() },
        )
        .await
        .unwrap();

    let refused = client.dispatch_task(&workspace_id, &pending.task.id).await.unwrap_err();

    match &refused {
        ClientError::Rejected { code, message } => {
            assert_eq!(code, "task_not_ready");
            assert!(!message.is_empty());
        }
        other => panic!("應該回報 Runtime 的拒絕：{other:?}"),
    }
    assert!(client.task_board(&workspace_id).await.unwrap().runs.is_empty());
}
