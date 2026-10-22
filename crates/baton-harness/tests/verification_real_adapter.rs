//! 情境測試：以正式的 CLI／API 驗證 Port（`LocalVerificationDriver`）對真實的程序與 HTTP 服務驗證（票 45）。
//!
//! 其餘 Port 仍是 fake；受驗的「產品」是測試在本機 loopback 上啟動的小型記帳 API，命令是作業系統的 shell。
//! 對應驗收條件「新 Profile 至少實際跑過一條代表旅程」：代表旅程真的送出 HTTP 請求、真的執行命令、
//! 真的建立與移除狀態目錄，Profile 才被證明可用；產品實際沒有保存資料時判定為產品缺陷。

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::StatusCode as HttpStatus;
use axum::routing::get;
use axum::{Json, Router};
use baton_domain::Timestamp;
use baton_harness::Harness;
use baton_runtime::verification_driver::LocalVerificationDriver;
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

const WORKSPACES: &str = "/api/v1/workspaces";

/// 受驗的記帳 API：`persist` 為 false 時回報建立成功，卻沒有真的保存（Worker 說做好了，但副作用不存在）。
#[derive(Default)]
struct Ledger {
    expenses: Mutex<Vec<Value>>,
    persist: AtomicBool,
}

async fn serve_ledger(ledger: Arc<Ledger>) -> SocketAddr {
    let app = Router::new()
        .route("/health", get(|| async { "ready" }))
        .route(
            "/expenses",
            get(|State(ledger): State<Arc<Ledger>>| async move { Json(Value::Array(ledger.expenses.lock().unwrap().clone())) }).post(
                |State(ledger): State<Arc<Ledger>>, Json(expense): Json<Value>| async move {
                    if ledger.persist.load(Ordering::SeqCst) {
                        ledger.expenses.lock().unwrap().push(expense);
                    }
                    (HttpStatus::CREATED, Json(json!({ "ok": true })))
                },
            ),
        )
        .with_state(ledger);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    addr
}

/// 作業系統的 shell 執行一行命令（不經過 Baton 的 shell：程式與引數分開傳入）。
fn shell(line: &str) -> Value {
    if cfg!(windows) {
        json!({ "kind": "command", "program": "cmd", "args": ["/C", line] })
    } else {
        json!({ "kind": "command", "program": "sh", "args": ["-c", line] })
    }
}

fn profile(api: &str) -> Value {
    // 暫存目錄的路徑沒有空白，不加引號（cmd 不認得 Rust 為引數加上的跳脫引號）
    let seed = if cfg!(windows) { r"echo {{run_id}}> {{state_dir}}\seed.txt" } else { "echo {{run_id}} > {{state_dir}}/seed.txt" };
    json!({
        "interface": "cli_api",
        "doctor": [{ "key": "shell", "operation": shell("echo doctor-ok"), "assertions": [
            { "kind": "exit_code", "equals": 0 }, { "kind": "contains", "text": "doctor-ok" },
        ] }],
        "readiness": {
            "step": { "key": "health", "operation": { "kind": "http", "method": "GET", "url": format!("{api}/health") },
                      "assertions": [{ "kind": "status", "equals": 200 }, { "kind": "contains", "text": "ready" }] },
            "attempts": 5,
            "interval_ms": 50,
        },
        // 資料準備寫進本次驗證專屬的狀態目錄
        "setup": [{ "key": "seed", "operation": shell(seed), "assertions": [{ "kind": "exit_code", "equals": 0 }] }],
        "journeys": [{
            "key": "record-expense",
            "title": "記一筆支出並讀回",
            "representative": true,
            "steps": [
                { "key": "create", "operation": { "kind": "http", "method": "POST", "url": format!("{api}/expenses"),
                    "headers": [{ "name": "Content-Type", "value": "application/json" }], "body": "{\"memo\":\"午餐\"}" },
                  "assertions": [{ "kind": "status", "equals": 201 }] },
                { "key": "read-back", "operation": { "kind": "http", "method": "GET", "url": format!("{api}/expenses") },
                  "assertions": [{ "kind": "status", "equals": 200 }, { "kind": "json_equals", "pointer": "/0/memo", "value": "午餐" }] },
            ],
        }],
        "teardown": [shell("echo teardown")],
        "feature_map": [
            { "key": "expenses", "title": "記帳", "criterion": "新增的支出可以讀回", "journeys": ["record-expense"] },
            { "key": "export", "title": "匯出 CSV", "criterion": "可以匯出 CSV", "journeys": [] },
        ],
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_profile_actually_runs_its_representative_journey_against_a_real_cli_and_http_service() {
    let ledger = Arc::new(Ledger::default());
    let api = format!("http://{}", serve_ledger(ledger.clone()).await);
    let harness = Harness::builder()
        .clock_at(Timestamp::from_millis(1_700_000_000_000))
        .verification_driver(Arc::new(LocalVerificationDriver::default()))
        .start()
        .await;
    let dir = harness.data_dir().join("projects").join("ledger");
    std::fs::create_dir_all(&dir).unwrap();
    let (status, ws) = harness.send_json(Method::POST, WORKSPACES, Some(json!({ "path": dir.to_string_lossy() }))).await;
    assert_eq!(status, StatusCode::CREATED, "{ws}");
    let base = format!("{WORKSPACES}/{}/verification", ws["id"].as_str().unwrap());
    let (status, body) = harness.send_json(Method::POST, &format!("{base}/profile"), Some(profile(&api))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    let verify = || async {
        let (status, request) = harness.send_json(Method::POST, &format!("{base}/runs"), Some(json!({}))).await;
        assert_eq!(status, StatusCode::CREATED, "{request}");
        harness.supervise().await;
        let (_, overview) = harness.send_json(Method::GET, &base, None).await;
        let id = overview["requests"][0]["bundle_id"].as_str().unwrap_or_else(|| panic!("驗證應該已執行：{overview}")).to_string();
        let (_, bundle) = harness.send_json(Method::GET, &format!("{base}/bundles/{id}"), None).await;
        (request, bundle, overview)
    };

    // 產品沒有真的保存：建立回 201，但讀不回來 → 產品缺陷，Profile 仍未證明可用
    let (request, bundle, overview) = verify().await;
    assert_eq!((bundle["verdict"].as_str(), bundle["reason_code"].as_str()), (Some("product_failure"), Some("assertion_failed")), "{bundle}");
    assert!(overview["proven_by"].is_null());
    let doctor = bundle["steps"].as_array().unwrap().iter().find(|s| s["key"] == "shell").unwrap();
    assert!(doctor["output"].as_str().unwrap().contains("doctor-ok"), "真的執行了命令：{doctor}");
    let state = harness.data_dir().join("verification").join(request["id"].as_str().unwrap());
    assert!(!state.exists(), "本次驗證的狀態目錄已移除");
    assert_eq!(bundle["cleanup"]["state_dir_removed"], true);

    // 產品修好之後，代表旅程真的通過 → Profile 已證明可用，未覆蓋範圍仍標示
    ledger.persist.store(true, Ordering::SeqCst);
    let (_, bundle, overview) = verify().await;
    assert_eq!(bundle["verdict"], "passed", "{bundle}");
    assert_eq!(overview["proven_by"], bundle["id"]);
    assert_eq!(bundle["uncovered"][0]["area"], "匯出 CSV");
    let seed = bundle["steps"].as_array().unwrap().iter().find(|s| s["key"] == "seed").unwrap();
    assert_eq!(seed["verdict"], "passed", "資料準備寫得進本次的狀態目錄：{seed}");
    assert_eq!(ledger.expenses.lock().unwrap().len(), 1);
}
