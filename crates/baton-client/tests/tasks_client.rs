//! 桌面殼經由 RuntimeClient 管理 Task 與契約（票 09）：與 App 實際使用的路徑相同（讀憑證檔、帶標頭）。

use std::sync::Arc;

use baton_api::{CreateTaskRequest, CreateWorkspaceRequest};
use baton_client::{ClientError, RuntimeClient};
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
        verification_checks: vec![],
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

#[tokio::test]
async fn app_creates_a_task_sees_missing_items_and_marks_it_ready_once_the_contract_is_complete() {
    let mut harness = Harness::start().await;
    let client = app_client(&harness);
    let workspace_id = new_workspace(&harness, &client).await;

    let goal = client.create_goal(&workspace_id, "研究競品").await.unwrap();
    let created = client
        .create_task(
            &workspace_id,
            &CreateTaskRequest { goal_id: goal.id.clone(), title: "整理".into(), contract: ContractContent::default() },
        )
        .await
        .unwrap();
    let refused = client.mark_task_ready(&workspace_id, &created.task.id).await.unwrap_err();
    let report = client.check_task_admission(&workspace_id, &created.task.id).await.unwrap();
    let updated = client
        .update_task_contract(&workspace_id, &created.task.id, complete(), Some("補齊".into()))
        .await
        .unwrap();
    let ready = client.mark_task_ready(&workspace_id, &created.task.id).await.unwrap();

    assert!(matches!(&refused, ClientError::Rejected { code, .. } if code == "task_admission_denied"), "{refused:?}");
    assert!(!report.admissible);
    assert_eq!(report.missing.len(), 6);
    assert_eq!((updated.task.contract_version, updated.task.status), (2, TaskStatus::Pending));
    assert_eq!(ready.task.status, TaskStatus::Ready);

    harness.restart().await;
    let client = app_client(&harness);
    let board = client.task_board(&workspace_id).await.unwrap();
    let detail = client.task_detail(&workspace_id, &created.task.id).await.unwrap();
    assert_eq!((board.goals.len(), board.tasks.len(), board.capabilities.len()), (1, 1, 12));
    assert_eq!(detail.contract_history.iter().map(|c| c.version).collect::<Vec<_>>(), vec![1, 2]);
    assert_eq!(detail.task.task.status, TaskStatus::Ready);
}

#[tokio::test]
async fn app_cannot_add_a_dependency_that_closes_a_cycle() {
    let harness = Harness::start().await;
    let client = app_client(&harness);
    let workspace_id = new_workspace(&harness, &client).await;
    let goal = client.create_goal(&workspace_id, "目標").await.unwrap();
    let mut ids = Vec::new();
    for title in ["甲", "乙"] {
        let request =
            CreateTaskRequest { goal_id: goal.id.clone(), title: title.into(), contract: ContractContent::default() };
        ids.push(client.create_task(&workspace_id, &request).await.unwrap().task.id);
    }

    client.add_task_dependency(&workspace_id, &ids[0], &ids[1]).await.unwrap();
    let cycle = client.add_task_dependency(&workspace_id, &ids[1], &ids[0]).await.unwrap_err();

    assert!(matches!(&cycle, ClientError::Rejected { code, .. } if code == "dependency_cycle"), "{cycle:?}");
}
