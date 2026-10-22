//! 桌面殼經由 RuntimeClient 管理 Workspace（票 05）：與 App 實際使用的路徑相同（讀憑證檔、帶標頭）。

use std::sync::Arc;

use baton_api::CreateWorkspaceRequest;
use baton_client::{ClientError, RuntimeClient};
use baton_domain::workspace::WorkspaceStatus;
use baton_harness::fakes::FakeSecretProtector;
use baton_harness::Harness;

fn app_client(harness: &Harness) -> RuntimeClient {
    RuntimeClient::new(harness.addr(), harness.data_dir().to_path_buf(), Arc::new(FakeSecretProtector))
}

fn request(path: &std::path::Path) -> CreateWorkspaceRequest {
    CreateWorkspaceRequest { path: path.to_string_lossy().into_owned(), name: None }
}

#[tokio::test]
async fn app_creates_lists_and_archives_a_workspace_and_it_survives_a_runtime_rebuild() {
    let mut harness = Harness::start().await;
    let dir = harness.data_dir().join("notes");
    std::fs::create_dir_all(&dir).unwrap();
    let client = app_client(&harness);

    let created = client.create_workspace(&request(&dir)).await.unwrap();
    assert_eq!((created.name.as_str(), created.status), ("notes", WorkspaceStatus::Active));
    assert_eq!(client.workspaces().await.unwrap().workspaces, vec![created.clone()]);

    harness.restart().await;
    let client = app_client(&harness);
    assert_eq!(client.workspaces().await.unwrap().workspaces, vec![created.clone()]);

    let archived = client.archive_workspace(&created.id).await.unwrap();
    assert_eq!(archived.status, WorkspaceStatus::Archived);
    assert_eq!(client.workspaces().await.unwrap().workspaces, vec![archived]);
}

#[tokio::test]
async fn rejection_reasons_reach_the_app_with_their_stable_code() {
    let harness = Harness::start().await;
    let client = app_client(&harness);

    let missing = client.create_workspace(&request(&harness.data_dir().join("missing"))).await.unwrap_err();
    let unknown = client.archive_workspace("ws_nope").await.unwrap_err();

    let ClientError::Rejected { code, message } = &missing else { panic!("{missing:?}") };
    assert_eq!(code, "workspace_path_not_found");
    assert!(message.contains("missing"));
    assert!(matches!(&unknown, ClientError::Rejected { code, .. } if code == "workspace_not_found"));
}
