//! 審查視圖（票 44）：桌面殼經由 RuntimeClient 讀取 Workspace 的雙軸審查與設計偏離（與 App 實際使用的路徑相同）。

use std::sync::Arc;

use baton_client::{ClientError, RuntimeClient};
use baton_harness::fakes::FakeSecretProtector;
use baton_harness::Harness;
use reqwest::Method;
use serde_json::json;

fn app_client(harness: &Harness) -> RuntimeClient {
    RuntimeClient::new(harness.addr(), harness.data_dir().to_path_buf(), Arc::new(FakeSecretProtector))
}

#[tokio::test]
async fn the_desktop_shell_reads_reviews_and_deviations_and_reports_unknown_workspaces() {
    let harness = Harness::start().await;
    let client = app_client(&harness);
    let dir = harness.data_dir().join("projects").join("main");
    std::fs::create_dir_all(&dir).unwrap();
    let (_, ws) = harness.send_json(Method::POST, "/api/v1/workspaces", Some(json!({ "path": dir.to_string_lossy() }))).await;
    let ws = ws["id"].as_str().unwrap().to_string();

    let list = client.reviews(&ws).await.unwrap();
    assert_eq!(list.workspace_id, ws);
    assert!(list.reviews.is_empty());
    assert!(list.deviations.is_empty());

    match client.reviews("ws_nope").await {
        Err(ClientError::Rejected { code, .. }) => assert_eq!(code, "workspace_not_found"),
        other => panic!("應該被拒絕：{other:?}"),
    }
}
