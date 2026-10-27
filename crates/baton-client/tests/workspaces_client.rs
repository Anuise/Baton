//! 桌面殼經由 RuntimeClient 管理 Workspace（票 05）：與 App 實際使用的路徑相同（讀憑證檔、帶標頭）。

use std::sync::Arc;

use baton_api::CreateWorkspaceRequest;
use baton_client::{ClientError, RuntimeClient};
use baton_domain::workspace::{ModelOverrides, WorkspaceStatus};
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

#[tokio::test]
async fn app_sets_model_overrides_per_role_and_rejects_invalid_model_names() {
    let harness = Harness::start().await;
    let dir = harness.data_dir().join("models");
    std::fs::create_dir_all(&dir).unwrap();
    let client = app_client(&harness);
    let created = client.create_workspace(&request(&dir)).await.unwrap();
    assert_eq!(created.model_overrides, ModelOverrides::default());

    // 空白視為不覆寫，前後空白會被去掉
    let overrides = ModelOverrides { controller: Some(" opus ".into()), worker: Some("claude-sonnet-4-5[1m]".into()), reviewer: Some("  ".into()) };
    let updated = client.set_model_overrides(&created.id, &overrides).await.unwrap();

    let expected = ModelOverrides { controller: Some("opus".into()), worker: Some("claude-sonnet-4-5[1m]".into()), reviewer: None };
    assert_eq!(updated.model_overrides, expected);
    assert_eq!(client.workspaces().await.unwrap().workspaces, vec![updated.clone()]);

    let invalid = ModelOverrides { worker: Some("opus; rm -rf".into()), ..ModelOverrides::default() };
    let error = client.set_model_overrides(&created.id, &invalid).await.unwrap_err();
    assert!(matches!(&error, ClientError::Rejected { code, .. } if code == "model_name_invalid"), "{error:?}");
    assert_eq!(client.workspaces().await.unwrap().workspaces, vec![updated]);
}

#[tokio::test]
async fn app_lists_the_builtin_models_that_the_model_settings_can_choose_from() {
    let harness = Harness::start().await;
    let client = app_client(&harness);

    let catalog = client.available_models().await.unwrap();

    let ids: Vec<&str> = catalog.models.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(
        ids,
        ["fable", "opus", "sonnet", "haiku", "claude-fable-5-1", "claude-opus-5-5", "claude-sonnet-5-5", "claude-haiku-5-5"]
    );
    assert!(catalog.models.iter().all(|m| !m.label.is_empty()));
    // 清單裡的每個模型都要能存成覆寫，選了不會被 Runtime 拒絕
    let dir = harness.data_dir().join("catalog");
    std::fs::create_dir_all(&dir).unwrap();
    let created = client.create_workspace(&request(&dir)).await.unwrap();
    for id in ids {
        let overrides = ModelOverrides { worker: Some(id.into()), ..ModelOverrides::default() };
        let updated = client.set_model_overrides(&created.id, &overrides).await.unwrap();
        assert_eq!(updated.model_overrides.worker.as_deref(), Some(id));
    }
}
