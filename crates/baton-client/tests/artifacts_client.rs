//! 桌面殼經由 RuntimeClient 瀏覽、預覽與下載成果（票 12）：與 App 實際使用的路徑相同（讀憑證檔、帶標頭）。

use std::sync::Arc;

use baton_api::{CreateTaskRequest, CreateWorkspaceRequest};
use baton_client::{ClientError, RuntimeClient};
use baton_domain::artifact::{Preview, TOOL_REGISTER_ARTIFACT};
use baton_domain::task::{ContractContent, ContractField};
use baton_harness::fakes::FakeSecretProtector;
use baton_harness::Harness;
use serde_json::json;

const SHA256_ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

fn app_client(harness: &Harness) -> RuntimeClient {
    RuntimeClient::new(harness.addr(), harness.data_dir().to_path_buf(), Arc::new(FakeSecretProtector))
}

fn filled(text: &str) -> Option<ContractField> {
    Some(ContractField::Filled { text: text.into(), references: vec![] })
}

fn complete() -> ContractContent {
    ContractContent {
        objective: filled("整理資料"),
        inputs: filled("既有的資料"),
        scope: filled("公開資料"),
        deliverables: filled("比較表"),
        done_criteria: filled("五個產品"),
        blocker_handling: filled("回報"),
        required_capabilities: vec![],
    }
}

/// 一個進行中的 Run 並登記了一個成果：回傳 (workspace_id, artifact_id)。
async fn workspace_with_artifact(harness: &Harness, client: &RuntimeClient) -> (String, String) {
    let dir = harness.data_dir().join("app");
    std::fs::create_dir_all(&dir).unwrap();
    let workspace = client
        .create_workspace(&CreateWorkspaceRequest { path: dir.to_string_lossy().into_owned(), name: None })
        .await
        .unwrap();
    let goal = client.create_goal(&workspace.id, "研究競品").await.unwrap();
    let task = client
        .create_task(&workspace.id, &CreateTaskRequest { goal_id: goal.id, title: "整理".into(), contract: complete() })
        .await
        .unwrap();
    client.mark_task_ready(&workspace.id, &task.task.id).await.unwrap();
    let run = client.dispatch_task(&workspace.id, &task.task.id).await.unwrap();
    harness.supervise().await;
    let token = harness.agent_credential(&run.run.id).expect("Worker 的 Agent 憑證");
    let (status, registered) =
        harness.call_tool(&token, TOOL_REGISTER_ARTIFACT, json!({ "name": "report.md", "content": "abc" })).await;
    assert_eq!(status, reqwest::StatusCode::CREATED, "{registered}");
    (workspace.id, registered["artifact_id"].as_str().unwrap().to_string())
}

#[tokio::test]
async fn app_lists_previews_and_downloads_an_artifact() {
    let harness = Harness::start().await;
    let client = app_client(&harness);
    let (workspace_id, artifact_id) = workspace_with_artifact(&harness, &client).await;

    let listing = client.list_artifacts(&workspace_id).await.unwrap();
    assert_eq!(listing.artifacts.len(), 1);
    assert_eq!(listing.artifacts[0].name, "report.md");
    assert_eq!(listing.artifacts[0].versions[0].version.sha256, SHA256_ABC);
    assert!(listing.unregistered_files.is_empty());

    let preview = client.preview_artifact(&workspace_id, &artifact_id, 1).await.unwrap();
    assert_eq!(preview.preview, Preview::Text { text: "abc".into(), truncated: false });

    let download = client.download_artifact(&workspace_id, &artifact_id, 1).await.unwrap();
    assert_eq!((download.bytes.as_slice(), download.sha256.as_str()), (&b"abc"[..], SHA256_ABC));
}

#[tokio::test]
async fn app_download_of_a_missing_version_or_a_tampered_file_is_rejected_with_the_runtime_code() {
    let harness = Harness::start().await;
    let client = app_client(&harness);
    let (workspace_id, artifact_id) = workspace_with_artifact(&harness, &client).await;

    let missing = client.download_artifact(&workspace_id, &artifact_id, 2).await.unwrap_err();
    assert!(matches!(&missing, ClientError::Rejected { code, .. } if code == "artifact_not_found"), "{missing:?}");

    let listing = client.list_artifacts(&workspace_id).await.unwrap();
    let path = harness.artifacts_dir().join(&listing.artifacts[0].versions[0].version.storage_path);
    std::fs::write(path, b"abd").unwrap();
    let tampered = client.download_artifact(&workspace_id, &artifact_id, 1).await.unwrap_err();
    assert!(matches!(&tampered, ClientError::Rejected { code, .. } if code == "artifact_integrity_failed"), "{tampered:?}");
}
