//! 情境測試：以正式的瀏覽器 Runner（`LocalBrowserDriver`，真實的 Playwright 與 Chromium）跑完整的 web 驗證（票 46）。
//!
//! 受驗的網站是測試在 loopback 上啟動的小型商店；其餘 Port（judge、秘密參照、受驗服務）仍是 fake。
//! Playwright 不在 repo 中：受驗專案與 `BATON_PLAYWRIGHT_DIR` 都解析不到時印出原因並略過。
//! 斷言：以秘密參照登入、核對實際身分、旅程通過；保存的截圖是真的 PNG；輸入過秘密的 Session 不保存 trace；
//! 畫面顯示秘密的截圖與那個 Session 的 trace 不保存；秘密的值不在 Bundle 與資料目錄的任何檔案中。

use std::path::Path;
use std::sync::Arc;

use axum::response::Html;
use axum::routing::{get, post};
use baton_domain::Timestamp;
use baton_harness::Harness;
use baton_ports::JudgeOutcome;
use baton_runtime::browser_driver::LocalBrowserDriver;
use base64::Engine;
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

const WORKSPACES: &str = "/api/v1/workspaces";
const PASSWORD: &str = "clerk-密碼-real-7c1d";
const SORTED: &str = "商品依價格由低到高排列";

#[derive(serde::Deserialize)]
struct Login {
    user: String,
    password: String,
}

async fn start_shop() -> String {
    let app = axum::Router::new()
        .route(
            "/login",
            get(|| async {
                Html(
                    r#"<!doctype html><title>登入</title><form method="post" action="/login">
<input id="user" name="user"><input id="password" name="password" type="password">
<button id="submit" type="submit">登入</button></form>"#,
                )
            }),
        )
        .route(
            "/login",
            post(|axum::extract::Form(login): axum::extract::Form<Login>| async move {
                let role = if login.user == "clerk01" && login.password == PASSWORD { "店員" } else { "訪客" };
                Html(format!("<!doctype html><title>我的帳號</title><p id=\"me\">登入者：{}　角色：{role}</p>", login.user))
            }),
        )
        .route(
            "/products",
            get(|| async { Html(r#"<!doctype html><title>商品</title><ul id="list"><li>A 100 元</li><li>B 200 元</li></ul>"#) }),
        )
        // 有缺陷的頁面：不需登入就把密碼顯示在畫面上
        .route("/status", get(|| async { Html(format!("<!doctype html><title>狀態</title><p id=\"debug\">除錯：{PASSWORD}</p>")) }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

fn step(key: &str, action: Value, assertions: Value) -> Value {
    json!({ "key": key, "title": key, "operation": { "kind": "browser", "action": action }, "assertions": assertions })
}

fn profile(site: &str) -> Value {
    json!({
        "interface": "web",
        "secrets": [{ "key": "clerk-password", "reference": "env:SHOP_CLERK_PASSWORD" }],
        "personas": [{ "key": "shopper", "title": "顧客", "description": "想找便宜商品的顧客" }],
        "identities": [{
            "key": "clerk",
            "title": "店員帳號",
            "login": [
                step("open-login", json!({ "kind": "goto", "url": format!("{site}/login") }), json!([{ "kind": "visible", "selector": "#user" }])),
                step("user", json!({ "kind": "fill", "selector": "#user", "text": "clerk01" }), json!([{ "kind": "visible", "selector": "#password" }])),
                step("password", json!({ "kind": "fill", "selector": "#password", "text": "{{secret:clerk-password}}" }), json!([{ "kind": "visible", "selector": "#submit" }])),
                step("submit", json!({ "kind": "click", "selector": "#submit" }), json!([{ "kind": "status", "equals": 200 }])),
            ],
            "check": step("whoami", json!({ "kind": "wait_for", "selector": "#me" }), json!([{ "kind": "contains", "text": "角色：店員" }])),
        }],
        "journeys": [{
            "key": "browse",
            "title": "瀏覽商品",
            "representative": true,
            "persona": "shopper",
            "identity": "clerk",
            "steps": [step(
                "open",
                json!({ "kind": "goto", "url": format!("{site}/products") }),
                json!([{ "kind": "visible", "selector": "#list" }, { "kind": "contains", "text": "A 100 元" }, { "kind": "judge", "expectation": SORTED }]),
            )],
        }, {
            "key": "status",
            "title": "查看狀態",
            "persona": "shopper",
            "steps": [
                step("open-products", json!({ "kind": "goto", "url": format!("{site}/products") }), json!([{ "kind": "visible", "selector": "#list" }])),
                step("open-status", json!({ "kind": "goto", "url": format!("{site}/status") }), json!([{ "kind": "visible", "selector": "#debug" }])),
            ],
        }],
        "evidence": { "screenshots": "every_step", "trace": true },
        "feature_map": [
            { "key": "catalog", "title": "商品目錄", "criterion": "看得到商品與價格", "journeys": ["browse"] },
            { "key": "status", "title": "狀態頁", "criterion": "看得到狀態", "journeys": ["status"] },
        ],
    })
}

fn files_containing(dir: &Path, needle: &str) -> Vec<String> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else { return found };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(files_containing(&path, needle));
        } else if let Ok(bytes) = std::fs::read(&path) {
            if bytes.windows(needle.len()).any(|w| w == needle.as_bytes()) {
                found.push(path.display().to_string());
            }
        }
    }
    found
}

#[tokio::test(flavor = "multi_thread")]
async fn a_real_browser_logs_in_by_secret_reference_and_keeps_only_safe_evidence() {
    let site = start_shop().await;
    let harness = Harness::builder()
        .clock_at(Timestamp::from_millis(1_700_000_000_123))
        .browser_driver(Arc::new(LocalBrowserDriver::default()))
        .start()
        .await;
    harness.add_account("帳號 A", "account0@example.com").await;
    harness.secret_resolver.set("env:SHOP_CLERK_PASSWORD", PASSWORD);
    harness.semantic_judge.script(SORTED, vec![JudgeOutcome::Decided { satisfied: true, rationale: "A 在 B 之前".into() }]);
    let dir = harness.data_dir().join("projects").join("shop");
    std::fs::create_dir_all(&dir).unwrap();
    let (status, body) = harness.send_json(Method::POST, WORKSPACES, Some(json!({ "path": dir.to_string_lossy() }))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let path = |rest: &str| format!("{WORKSPACES}/{}/verification{rest}", body["id"].as_str().unwrap());

    let (status, published) = harness.send_json(Method::POST, &path("/profile"), Some(profile(&site))).await;
    assert_eq!(status, StatusCode::CREATED, "{published}");
    let (status, request) = harness.send_json(Method::POST, &path("/runs"), Some(json!({}))).await;
    assert_eq!(status, StatusCode::CREATED, "{request}");
    harness.supervise().await;
    let (_, overview) = harness.send_json(Method::GET, &path(""), None).await;
    let bundle = overview["bundles"][0].clone();

    if bundle["reason_code"] == "browser_unavailable" {
        let reason = bundle["steps"][0]["output"].as_str().unwrap_or_default();
        if ["找不到 Playwright", "無法啟動 Node.js", "Executable doesn't exist"].iter().any(|m| reason.contains(m)) {
            eprintln!("略過：{reason}（可設定 BATON_PLAYWRIGHT_DIR 指向已安裝 playwright 的目錄）");
            return;
        }
    }
    assert_eq!(bundle["verdict"], "passed", "{bundle}");
    let journey = &bundle["journeys"][0];
    assert_eq!((journey["persona"].as_str(), journey["identity"].as_str()), (Some("shopper"), Some("clerk")));
    let whoami = bundle["steps"].as_array().unwrap().iter().find(|s| s["key"] == "whoami").unwrap();
    assert!(whoami["output"].as_str().unwrap().contains("角色：店員"), "{whoami}");
    let judged = &harness.semantic_judge.requests()[0];
    assert!(judged.page_text.contains("A 100 元"), "judge 看到的是真實畫面：{judged:?}");

    // 截圖是真的 PNG；輸入過秘密的 Session 的 trace 不保存
    let artifacts = bundle["artifacts"].as_array().unwrap();
    let screenshot = artifacts.iter().position(|a| a["kind"] == "screenshot" && a["storage_path"].is_string()).expect("有保存的截圖");
    let bundle_id = bundle["id"].as_str().unwrap();
    let (status, content) = harness.send_json(Method::GET, &path(&format!("/bundles/{bundle_id}/artifacts/{screenshot}")), None).await;
    assert_eq!(status, StatusCode::OK, "{content}");
    let png = base64::engine::general_purpose::STANDARD.decode(content["content_base64"].as_str().unwrap()).unwrap();
    assert!(png.starts_with(b"\x89PNG"), "截圖是 PNG");
    let trace = |journey: &str| artifacts.iter().find(|a| a["kind"] == "trace" && a["journey"] == journey).expect("有 trace 的紀錄");
    assert_eq!(trace("browse")["withheld"], "secret_input_recorded", "{bundle}");

    // 沒有輸入秘密、但畫面顯示了秘密：PNG 的位元組中找不到文字，仍依畫面文字不保存那一步的截圖與整個 Session 的 trace
    let screenshot = |step: &str| artifacts.iter().find(|a| a["kind"] == "screenshot" && a["step"] == step).unwrap();
    assert!(screenshot("open-products")["storage_path"].is_string(), "{bundle}");
    assert_eq!(screenshot("open-status")["withheld"], "secret_detected", "{bundle}");
    assert_eq!(trace("status")["withheld"], "secret_detected", "{bundle}");

    // 秘密的值不在 Bundle，也不在資料目錄的任何檔案中（暫存的 Runner 目錄已隨狀態目錄移除）
    assert!(!bundle.to_string().contains(PASSWORD));
    assert_eq!(files_containing(harness.data_dir(), PASSWORD), Vec::<String>::new());
    assert_eq!(bundle["cleanup"]["state_dir_removed"], true);
}
