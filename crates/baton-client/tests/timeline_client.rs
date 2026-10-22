//! 時間線與觀測指標（票 41）：桌面殼經由 RuntimeClient 查詢（與 App 實際使用的路徑相同）。

use std::sync::Arc;

use baton_client::{ClientError, RuntimeClient};
use baton_harness::fakes::FakeSecretProtector;
use baton_harness::Harness;
use reqwest::{Method, StatusCode};
use serde_json::json;

fn app_client(harness: &Harness) -> RuntimeClient {
    RuntimeClient::new(harness.addr(), harness.data_dir().to_path_buf(), Arc::new(FakeSecretProtector))
}

#[tokio::test]
async fn the_desktop_shell_reads_the_timeline_filtered_by_task_and_the_metrics() {
    let harness = Harness::start().await;
    let client = app_client(&harness);
    let dir = harness.data_dir().join("projects").join("main");
    std::fs::create_dir_all(&dir).unwrap();
    let (_, ws) = harness.send_json(Method::POST, "/api/v1/workspaces", Some(json!({ "path": dir.to_string_lossy() }))).await;
    let ws = ws["id"].as_str().unwrap().to_string();
    let (_, goal) = harness.send_json(Method::POST, &format!("/api/v1/workspaces/{ws}/goals"), Some(json!({ "title": "目標" }))).await;
    let mut tasks = Vec::new();
    for title in ["甲", "乙"] {
        let body = json!({ "goal_id": goal["id"], "title": title });
        let (status, task) = harness.send_json(Method::POST, &format!("/api/v1/workspaces/{ws}/tasks"), Some(body)).await;
        assert_eq!(status, StatusCode::CREATED, "{task}");
        tasks.push(task["id"].as_str().unwrap().to_string());
    }

    let all = client.timeline(&ws, None).await.unwrap();
    assert_eq!(all.events.len(), 2);
    let first = client.timeline(&ws, Some(&tasks[0])).await.unwrap();
    assert_eq!(first.task_id.as_deref(), Some(tasks[0].as_str()));
    assert_eq!(first.events.len(), 1);
    assert_eq!(first.events[0].task_ids, [tasks[0].clone()]);

    let metrics = client.metrics(&ws).await.unwrap();
    assert_eq!(metrics.metrics.events, 2);
    assert_eq!(metrics.metrics.task_completion.completion.denominator, 2);

    match client.timeline(&ws, Some("不存在的 Task")).await {
        Err(ClientError::Rejected { code, .. }) => assert_eq!(code, "task_not_found"),
        other => panic!("應該被拒絕：{other:?}"),
    }
}
