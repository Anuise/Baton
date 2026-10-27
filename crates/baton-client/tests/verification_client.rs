//! 桌面殼經由 RuntimeClient 檢視驗證狀況與手動接受成果（票 13）：與 App 實際使用的路徑相同（讀憑證檔、帶標頭）。

use std::sync::Arc;

use baton_api::{AcceptTaskRequest, CreateTaskRequest, CreateWorkspaceRequest};
use baton_client::{ClientError, DispatchOutcome, RuntimeClient};
use baton_domain::task::{ContractContent, ContractField, TaskStatus};
use baton_domain::verification::{VerificationMethod, VerificationOutcome};
use baton_harness::fakes::FakeSecretProtector;
use baton_harness::Harness;
use baton_ports::AdapterEvent;

fn app_client(harness: &Harness) -> RuntimeClient {
    RuntimeClient::new(harness.addr(), harness.data_dir().to_path_buf(), Arc::new(FakeSecretProtector))
}

fn filled(text: &str) -> Option<ContractField> {
    Some(ContractField::Filled { text: text.into(), references: vec![] })
}

/// 沒有宣告確定性檢查的契約：Worker 成功結束後只能由使用者手動接受。
fn complete() -> ContractContent {
    ContractContent {
        objective: filled("整理資料"),
        inputs: Some(ContractField::NotApplicable { reason: "沒有輸入".into() }),
        scope: filled("公開資料"),
        deliverables: filled("比較表"),
        done_criteria: filled("使用者看過比較表"),
        blocker_handling: filled("回報"),
        required_capabilities: vec![],
        verification_checks: vec![],
    }
}

/// 建立 Workspace 與 Task、派工並讓 Worker 成功結束：回傳 (workspace_id, task_id)，Task 停在 verifying。
async fn verifying_task(harness: &Harness, client: &RuntimeClient) -> (String, String) {
    let dir = harness.data_dir().join("app");
    std::fs::create_dir_all(&dir).unwrap();
    let workspace_id = client
        .create_workspace(&CreateWorkspaceRequest { path: dir.to_string_lossy().into_owned(), name: None })
        .await
        .unwrap()
        .id;
    let goal = client.create_goal(&workspace_id, "研究競品").await.unwrap();
    let created = client
        .create_task(&workspace_id, &CreateTaskRequest { goal_id: goal.id, title: "整理".into(), kind: Default::default(), contract: complete() })
        .await
        .unwrap();
    let task_id = created.task.id;
    client.mark_task_ready(&workspace_id, &task_id).await.unwrap();
    let DispatchOutcome::Started(run) = client.dispatch_task(&workspace_id, &task_id).await.unwrap() else {
        panic!("應該直接派出");
    };
    harness.supervise().await;
    harness.agent_adapter.emit(&run.run.id, AdapterEvent::Exited { success: true });
    harness.supervise().await;
    (workspace_id, task_id)
}

#[tokio::test]
async fn app_views_the_verification_and_accepts_the_task_with_the_basis_it_saw() {
    let harness = Harness::start().await;
    harness.add_account("主要帳號", "main@example.com").await;
    let client = app_client(&harness);
    let (workspace_id, task_id) = verifying_task(&harness, &client).await;

    let view = client.task_verification(&workspace_id, &task_id).await.unwrap();
    assert_eq!(view.status, TaskStatus::Verifying);
    assert!(!view.automatic);
    let basis = view.basis.expect("驗證中的 Task 應該有接受基礎");

    let accepted = client
        .accept_task(&workspace_id, &task_id, &AcceptTaskRequest { basis: basis.clone(), note: Some("可以".into()) })
        .await
        .unwrap();

    assert_eq!(accepted.status, TaskStatus::Completed);
    assert_eq!(
        (accepted.records[0].method, accepted.records[0].outcome),
        (VerificationMethod::Manual, VerificationOutcome::Passed)
    );
    let board = client.task_board(&workspace_id).await.unwrap();
    assert_eq!(board.verifications.len(), 1);

    let again = client.accept_task(&workspace_id, &task_id, &AcceptTaskRequest { basis, note: None }).await;
    assert!(matches!(again, Err(ClientError::Rejected { ref code, .. }) if code == "task_not_verifying"), "{again:?}");
}
