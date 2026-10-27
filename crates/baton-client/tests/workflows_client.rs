//! 流程視圖（票 43）：桌面殼經由 RuntimeClient 讀取 Playbook 目錄、Workflow 與跳過 Stage（與 App 實際使用的路徑相同）。

use std::sync::Arc;

use baton_api::SkipStageRequest;
use baton_client::{ClientError, RuntimeClient};
use baton_harness::fakes::FakeSecretProtector;
use baton_harness::Harness;
use reqwest::Method;
use serde_json::json;

fn app_client(harness: &Harness) -> RuntimeClient {
    RuntimeClient::new(harness.addr(), harness.data_dir().to_path_buf(), Arc::new(FakeSecretProtector))
}

#[tokio::test]
async fn the_desktop_shell_reads_the_catalog_and_workflows_and_reports_skip_errors() {
    let harness = Harness::start().await;
    let client = app_client(&harness);
    let dir = harness.data_dir().join("projects").join("main");
    std::fs::create_dir_all(&dir).unwrap();
    let (_, ws) = harness.send_json(Method::POST, "/api/v1/workspaces", Some(json!({ "path": dir.to_string_lossy() }))).await;
    let ws = ws["id"].as_str().unwrap().to_string();

    let catalog = client.playbooks().await.unwrap();
    assert_eq!(catalog.playbooks.len(), 7, "內建七個 Playbook");
    assert!(catalog.playbooks.iter().all(|p| p.version == 1));
    assert!(!catalog.methods.is_empty());

    let list = client.workflows(&ws).await.unwrap();
    assert_eq!(list.workspace_id, ws);
    assert!(list.workflows.is_empty());

    let request = SkipStageRequest { stage: "design".into(), expected_revision: 1, reason: "不需要".into() };
    match client.skip_stage(&ws, "wf_nope", &request).await {
        Err(ClientError::Rejected { code, .. }) => assert_eq!(code, "workflow_not_found"),
        other => panic!("應該被拒絕：{other:?}"),
    }
}
