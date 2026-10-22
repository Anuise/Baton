//! 專案理解（票 42）：桌面殼經由 RuntimeClient 查詢、重新比對與協調投影檔（與 App 實際使用的路徑相同）。

use std::sync::Arc;

use baton_api::ReconcileRequest;
use baton_client::{ClientError, RuntimeClient};
use baton_domain::understanding::{ProjectionStatus, ReconcileAction};
use baton_harness::fakes::FakeSecretProtector;
use baton_harness::Harness;
use reqwest::{Method, StatusCode};
use serde_json::json;

fn app_client(harness: &Harness) -> RuntimeClient {
    RuntimeClient::new(harness.addr(), harness.data_dir().to_path_buf(), Arc::new(FakeSecretProtector))
}

#[tokio::test]
async fn the_desktop_shell_reads_syncs_and_reconciles_projections() {
    let harness = Harness::start().await;
    let client = app_client(&harness);
    let dir = harness.data_dir().join("projects").join("main");
    std::fs::create_dir_all(&dir).unwrap();
    harness.workspace_fs.init_repo(&dir, "main");
    let (_, ws) = harness.send_json(Method::POST, "/api/v1/workspaces", Some(json!({ "path": dir.to_string_lossy() }))).await;
    let ws = ws["id"].as_str().unwrap().to_string();

    let empty = client.sync_understanding(&ws).await.unwrap();
    assert!(empty.projection_enabled);
    assert!(empty.items.is_empty() && empty.projections.is_empty());

    let decision = json!({ "kind": "decision", "scope": { "kind": "workspace", "workspace_id": ws }, "title": "用 SQLite" });
    let (status, entry) = harness.send_json(Method::POST, "/api/v1/knowledge", Some(decision)).await;
    assert_eq!(status, StatusCode::CREATED, "{entry}");
    let path = format!("docs/adr/baton-{}.md", entry["id"].as_str().unwrap());
    std::fs::write(dir.join(&path), "改過了").unwrap();

    let view = client.sync_understanding(&ws).await.unwrap();
    assert_eq!(view.projections[0].status, ProjectionStatus::Conflict);
    let request = ReconcileRequest {
        path: path.clone(),
        action: ReconcileAction::KeepExternal,
        expected_external_hash: view.projections[0].external_hash.clone(),
    };
    let kept = client.reconcile_projection(&ws, &request).await.unwrap();
    assert_eq!(kept.projections[0].status, ProjectionStatus::Detached);
    assert_eq!(client.project_understanding(&ws).await.unwrap(), kept);

    let missing = ReconcileRequest { path: "GLOSSARY.md".into(), ..request };
    match client.reconcile_projection(&ws, &missing).await {
        Err(ClientError::Rejected { code, .. }) => assert_eq!(code, "projection_not_found"),
        other => panic!("應該被拒絕：{other:?}"),
    }
}
