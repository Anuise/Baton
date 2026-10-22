//! Verification Profile（票 45）：桌面殼經由 RuntimeClient 讀取驗證概況、發布 Profile、要求執行驗證與讀取
//! Evidence Bundle（與 App 實際使用的路徑相同）。這裡只驗證路徑、狀態與回應形狀，不實際執行驗證。

use std::sync::Arc;

use baton_api::RunVerificationRequest;
use baton_client::{ClientError, RuntimeClient};
use baton_domain::verification_profile::{ProfileDefinition, RequestStatus};
use baton_harness::fakes::FakeSecretProtector;
use baton_harness::Harness;
use reqwest::Method;
use serde_json::json;

fn app_client(harness: &Harness) -> RuntimeClient {
    RuntimeClient::new(harness.addr(), harness.data_dir().to_path_buf(), Arc::new(FakeSecretProtector))
}

/// 一個最小但合法的 Profile：一條代表旅程、一項有旅程的功能、一項沒有旅程的功能與一個明確宣告的未覆蓋範圍。
fn definition(representative: bool) -> ProfileDefinition {
    serde_json::from_value(json!({
        "interface": "cli_api",
        "journeys": [{
            "key": "smoke",
            "title": "冒煙測試",
            "representative": representative,
            "steps": [{
                "key": "version",
                "operation": { "kind": "command", "program": "tool", "args": ["--version"] },
                "assertions": [{ "kind": "exit_code", "equals": 0 }]
            }]
        }],
        "feature_map": [
            { "key": "cli", "title": "命令列", "criterion": "可以顯示版本", "journeys": ["smoke"] },
            { "key": "export", "title": "匯出", "criterion": "可以匯出報表" }
        ],
        "uncovered": [{ "area": "圖形介面", "reason": "CLI／API Adapter 不涵蓋" }]
    }))
    .unwrap()
}

fn rejected_code<T: std::fmt::Debug>(result: Result<T, ClientError>) -> String {
    match result {
        Err(ClientError::Rejected { code, .. }) => code,
        other => panic!("應該被拒絕：{other:?}"),
    }
}

#[tokio::test]
async fn the_desktop_shell_publishes_a_profile_requests_a_run_and_reads_bundles() {
    let harness = Harness::start().await;
    let client = app_client(&harness);
    let dir = harness.data_dir().join("projects").join("main");
    std::fs::create_dir_all(&dir).unwrap();
    let (_, ws) = harness.send_json(Method::POST, "/api/v1/workspaces", Some(json!({ "path": dir.to_string_lossy() }))).await;
    let ws = ws["id"].as_str().unwrap().to_string();

    let empty = client.verification_overview(&ws).await.unwrap();
    assert_eq!(empty.workspace_id, ws);
    assert!(empty.profile.is_none() && empty.proven_by.is_none());
    assert!(empty.uncovered.is_empty() && empty.requests.is_empty() && empty.bundles.is_empty());

    // 還沒有 Profile 時不能要求執行
    let code = rejected_code(client.run_verification(&ws, &RunVerificationRequest::default()).await);
    assert_eq!(code, "verification_profile_not_found");

    // 沒有代表旅程的 Profile 不能發布
    let code = rejected_code(client.publish_verification_profile(&ws, &definition(false)).await);
    assert_eq!(code, "verification_profile_invalid");

    let published = client.publish_verification_profile(&ws, &definition(true)).await.unwrap();
    assert!(published.created);
    assert_eq!(published.profile.version, 1);
    assert_eq!(published.profile.definition, definition(true));

    // 內容相同不新增版本
    let again = client.publish_verification_profile(&ws, &definition(true)).await.unwrap();
    assert!(!again.created);
    assert_eq!(again.profile.id, published.profile.id);

    let overview = client.verification_overview(&ws).await.unwrap();
    assert_eq!(overview.profile.as_ref().map(|p| p.version), Some(1));
    assert!(overview.proven_by.is_none(), "新版本還沒有實際跑過代表旅程");
    let areas: Vec<&str> = overview.uncovered.iter().map(|u| u.area.as_str()).collect();
    assert_eq!(areas, ["匯出", "圖形介面"]);

    let request = RunVerificationRequest { task_id: None, journeys: vec!["smoke".into()] };
    let queued = client.run_verification(&ws, &request).await.unwrap();
    assert_eq!(queued.workspace_id, ws);
    assert_eq!(queued.profile_version_id, published.profile.id);
    assert_eq!(queued.journeys, ["smoke"]);
    assert_eq!(queued.requested_by, "desktop");
    assert_eq!(queued.status, RequestStatus::Queued);

    let code = rejected_code(client.evidence_bundle(&ws, "vbun_nope").await);
    assert_eq!(code, "evidence_bundle_not_found");
}
