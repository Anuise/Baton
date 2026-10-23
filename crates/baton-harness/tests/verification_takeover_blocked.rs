//! 情境測試：驗證交接時，承接者無法確認前一次的測試程序已停止（票 49，AI-429，最終 review #25）。
//!
//! 舊服務可能仍在執行、仍在使用同一個狀態目錄（`<資料目錄>\verification\<驗證要求>`）。承接者此時不能執行任何操作：
//! 不重建或移除狀態目錄、不執行 teardown、不啟動服務、不重跑旅程，只停止（嘗試停止）前一次留下的程序並判為
//! `environment_blocked`（`previous_session_unconfirmed`）。
//!
//! 受驗服務由 fake VerificationDriver 扮演（它記下每一個操作，含準備與移除狀態目錄）；「Runtime 中途崩潰」與
//! `verification_handoff.rs` 相同：讓請求永遠不回來、以 `timeout` 丟棄那次處理，再 `crash_restart`。

use baton_domain::Timestamp;
use baton_harness::fakes::FakePage;
use baton_harness::Harness;
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

const WORKSPACES: &str = "/api/v1/workspaces";
const CLOCK_START: Timestamp = Timestamp::from_millis(1_700_000_000_123);
const SITE: &str = "http://127.0.0.1:3000";
const API: &str = "http://127.0.0.1:3000/api";

fn step(key: &str, operation: Value, assertions: Value) -> Value {
    json!({ "key": key, "title": format!("步驟 {key}"), "operation": operation, "assertions": assertions })
}

fn http(method: &str, path: &str) -> Value {
    json!({ "kind": "http", "method": method, "url": format!("{API}{path}") })
}

fn goto(path: &str) -> Value {
    json!({ "kind": "browser", "action": { "kind": "goto", "url": format!("{SITE}{path}") } })
}

/// 網路商店：啟動服務、就緒、準備資料、一條旅程（開啟商品頁、查庫存），最後以命令 teardown。
fn profile() -> Value {
    json!({
        "interface": "web",
        "start": { "program": "shop", "args": ["serve", "--data", "{{state_dir}}"] },
        "readiness": { "step": step("health", http("GET", "/health"), json!([{ "kind": "status", "equals": 200 }])), "attempts": 1 },
        "setup": [step("seed", http("POST", "/seed"), json!([{ "kind": "status", "equals": 200 }]))],
        "journeys": [{
            "key": "stock",
            "title": "查庫存",
            "representative": true,
            "steps": [
                step("open", goto("/products"), json!([{ "kind": "contains", "text": "商品清單" }])),
                step("stock", http("GET", "/stock"), json!([{ "kind": "status", "equals": 200 }])),
            ],
        }],
        "teardown": [{ "kind": "command", "program": "shop", "args": ["purge", "--data", "{{state_dir}}"] }],
        "feature_map": [{ "key": "shop", "title": "商店", "criterion": "顧客可以查庫存", "journeys": ["stock"] }],
    })
}

async fn workspace(harness: &Harness) -> String {
    let dir = harness.data_dir().join("projects").join("shop");
    std::fs::create_dir_all(&dir).unwrap();
    let (status, body) = harness.send_json(Method::POST, WORKSPACES, Some(json!({ "path": dir.to_string_lossy() }))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn a_blocked_successor_leaves_the_state_dir_alone_and_runs_no_teardown() {
    let mut harness = Harness::builder().clock_at(CLOCK_START).start().await;
    harness.add_account("帳號 A", "account0@example.com").await;
    let ws = workspace(&harness).await;
    let path = |rest: &str| format!("{WORKSPACES}/{ws}/verification{rest}");
    let driver = &harness.verification_driver;
    driver.script_response("GET", &format!("{API}/health"), 200, "ok");
    driver.script_response("POST", &format!("{API}/seed"), 200, "seeded");
    driver.script_response("GET", &format!("{API}/stock"), 200, "{\"A\":3}");
    driver.script_exit("shop", &[], 0, "purged");
    harness.browser_driver.script_page(&format!("{SITE}/products"), FakePage::new("商品", "商品清單：A、B", &[]));
    let (status, body) = harness.send_json(Method::POST, &path("/profile"), Some(profile())).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    // 查庫存時 Runtime 當掉：前一個 Runtime 啟動的服務仍在執行，而且仍在使用這個要求的狀態目錄
    let (status, request) = harness.send_json(Method::POST, &path("/runs"), Some(json!({}))).await;
    assert_eq!(status, StatusCode::CREATED, "{request}");
    let request = request["id"].as_str().unwrap().to_string();
    harness.verification_driver.hang_next_http("GET", &format!("{API}/stock"));
    harness.supervise_until(harness.verification_driver.wait_for_http_hang()).await;
    harness.crash_restart().await;
    let state = harness.data_dir().join("verification").join(&request);
    std::fs::write(state.join("orders.db"), "舊服務仍在寫入").expect("前一次嘗試準備了狀態目錄");

    // 舊服務停不下來：承接者無法確認它已停止
    harness.verification_driver.fail_stop("svc-1", "存取被拒");
    let before = harness.verification_driver.operations().len();
    harness.supervise().await;

    let (status, overview) = harness.send_json(Method::GET, &path(""), None).await;
    assert_eq!(status, StatusCode::OK, "{overview}");
    let done = overview["requests"].as_array().unwrap().iter().find(|r| r["id"] == request.as_str()).unwrap().clone();
    assert_eq!(done["status"], "completed", "{done}");
    let (status, bundle) =
        harness.send_json(Method::GET, &path(&format!("/bundles/{}", done["bundle_id"].as_str().unwrap())), None).await;
    assert_eq!(status, StatusCode::OK, "{bundle}");
    assert_eq!(
        (bundle["verdict"].as_str(), bundle["reason_code"].as_str()),
        (Some("environment_blocked"), Some("previous_session_unconfirmed")),
        "{bundle}"
    );

    // 承接者只嘗試停止舊服務：沒有準備或移除狀態目錄、沒有 teardown、沒有啟動服務或送出請求
    let operations = harness.verification_driver.operations();
    assert_eq!(operations[before..], ["stop svc-1".to_string()], "{operations:?}");
    assert_eq!(std::fs::read_to_string(state.join("orders.db")).unwrap(), "舊服務仍在寫入", "舊服務的狀態目錄原封不動");
    assert_eq!(bundle["cleanup"]["state_dir_removed"], false, "{}", bundle["cleanup"]);
    assert!(bundle["steps"].as_array().unwrap().iter().all(|s| s["phase"] != "teardown"), "沒有 teardown 的紀錄：{bundle}");
}
