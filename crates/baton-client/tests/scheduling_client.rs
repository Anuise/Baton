//! 桌面殼經由 RuntimeClient 讀取排程總覽、切換容量預設，並在沒有名額時得到「排隊中」（票 19）。

use std::sync::Arc;

use baton_api::{CreateTaskRequest, CreateWorkspaceRequest};
use baton_client::{DispatchOutcome, RuntimeClient};
use baton_domain::scheduler::{CapacityMode, WaitCategory};
use baton_domain::task::{ContractContent, ContractField};
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

/// 建立一個 Task 並標為 ready；回傳 task id。
async fn ready_task(client: &RuntimeClient, workspace_id: &str, goal_id: &str, title: &str) -> String {
    let created = client
        .create_task(
            workspace_id,
            &CreateTaskRequest { goal_id: goal_id.to_string(), title: title.into(), contract: complete() },
        )
        .await
        .unwrap();
    client.mark_task_ready(workspace_id, &created.task.id).await.unwrap();
    created.task.id
}

#[tokio::test]
async fn app_reads_the_scheduling_overview_with_both_capacity_presets() {
    let harness = Harness::start().await;
    let profile_id = harness.add_account("主要帳號", "main@example.com").await;
    let client = app_client(&harness);

    let overview = client.scheduling().await.unwrap();

    assert_eq!(overview.settings.mode, CapacityMode::ReserveHandoff);
    assert_eq!(overview.settings.modes.len(), 2);
    assert_eq!(overview.capacity.running, 0);
    assert_eq!(overview.capacity.usable_accounts, 1);
    assert_eq!(overview.accounts.len(), 1);
    assert_eq!(overview.accounts[0].profile_id, profile_id);
    assert_eq!(overview.accounts[0].running, 0);
    assert!(overview.accounts[0].quota_used.is_none());
    assert!(overview.queue.is_empty());
}

#[tokio::test]
async fn app_switches_the_capacity_preset_and_the_overview_follows() {
    let harness = Harness::start().await;
    let client = app_client(&harness);
    let before = client.scheduling().await.unwrap();

    let switched = client.set_scheduling_mode(CapacityMode::HigherConcurrency).await.unwrap();
    let after = client.scheduling().await.unwrap();

    assert_eq!(before.settings.mode, CapacityMode::ReserveHandoff);
    assert_eq!(switched.settings.mode, CapacityMode::HigherConcurrency);
    assert_eq!(after.settings.mode, CapacityMode::HigherConcurrency);
    assert!(after.capacity.per_account_limit > before.capacity.per_account_limit);
}

#[tokio::test]
async fn app_gets_a_queued_outcome_with_reasons_when_there_is_no_capacity() {
    let harness = Harness::start().await;
    let profile_id = harness.add_account("主要帳號", "main@example.com").await;
    let client = app_client(&harness);
    let workspace_id = new_workspace(&harness, &client).await;
    let goal = client.create_goal(&workspace_id, "研究競品").await.unwrap();
    let limit = client.scheduling().await.unwrap().capacity.per_account_limit;

    // 單一帳號先用滿每帳號的並行上限
    for index in 0..limit {
        let task_id = ready_task(&client, &workspace_id, &goal.id, &format!("整理 {index}")).await;
        match client.dispatch_task(&workspace_id, &task_id).await.unwrap() {
            DispatchOutcome::Started(view) => assert_eq!(view.run.profile_id, profile_id),
            DispatchOutcome::Queued(queued) => panic!("名額還沒滿，不應排隊：{queued:?}"),
        }
    }
    let waiting_id = ready_task(&client, &workspace_id, &goal.id, "排不進去").await;

    let outcome = client.dispatch_task(&workspace_id, &waiting_id).await.unwrap();

    match outcome {
        DispatchOutcome::Queued(queued) => {
            assert!(queued.queued);
            assert_eq!(queued.task_id, waiting_id);
            assert!(queued.reasons.iter().any(|r| r.category == WaitCategory::Capacity), "{:?}", queued.reasons);
        }
        DispatchOutcome::Started(view) => panic!("名額已滿，應該排隊：{view:?}"),
    }
    let overview = client.scheduling().await.unwrap();
    assert_eq!(overview.accounts[0].running, limit);
    assert_eq!(overview.accounts[0].runs.len(), limit);
    assert_eq!(overview.queue.len(), 1);
    assert_eq!(overview.queue[0].task_id, waiting_id);
    assert_eq!(overview.queue[0].position, 1);
}
