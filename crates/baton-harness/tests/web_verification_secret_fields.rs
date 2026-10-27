//! 情境測試：以正式的瀏覽器 Runner（真實的 Playwright 與 Chromium）偵測只出現在輸入欄位的值、
//! 或只出現在步驟之間中間畫面的秘密（票 46，AI-423，最終 review #24）。
//!
//! Playwright 不在 repo 中：受驗專案與 `BATON_PLAYWRIGHT_DIR` 都解析不到時印出原因並略過。
//! 斷言：秘密只在 `<input>` 的值（含 shadow DOM）時，那一步的截圖與整個 Session 的 trace 不保存；
//! 秘密只在畫出之前就被換掉的中間畫面時，觀察到的截圖照常保存，但記下它的 trace 不保存；秘密的值不在任何保存的檔案中。

use std::path::Path;
use std::sync::Arc;

use axum::response::Html;
use axum::routing::get;
use baton_domain::Timestamp;
use baton_harness::Harness;
use baton_runtime::browser_driver::LocalBrowserDriver;
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

const WORKSPACES: &str = "/api/v1/workspaces";
const TOKEN: &str = "api-token-real-5f2e";

async fn start_site() -> String {
    let app = axum::Router::new()
        .route("/home", get(|| async { Html("<!doctype html><title>首頁</title><p id=\"hello\">歡迎</p>") }))
        // 有缺陷的設定頁：把權杖放在輸入欄位（畫面看得到，但不在 innerText 中）
        .route(
            "/settings",
            get(|| async {
                Html(format!(
                    "<!doctype html><title>設定</title><h1 id=\"title\">API 設定</h1><input id=\"key\" value=\"{TOKEN}\"><div id=\"host\"></div>\
                     <script>document.getElementById('host').attachShadow({{ mode: 'open' }}).innerHTML = '<input value=\"{TOKEN}\">';</script>"
                ))
            }),
        )
        // 回應中帶有權杖、畫出之前就被換掉：觀察到的畫面沒有權杖
        .route(
            "/flash",
            get(|| async {
                Html(format!(
                    "<!doctype html><title>處理中</title><p id=\"out\">{TOKEN}</p>\
                     <script>document.getElementById('out').textContent = '已處理';</script>"
                ))
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

fn step(key: &str, url: String, selector: &str) -> Value {
    json!({
        "key": key,
        "title": key,
        "operation": { "kind": "browser", "action": { "kind": "goto", "url": url } },
        "assertions": [{ "kind": "visible", "selector": selector }],
    })
}

fn profile(site: &str) -> Value {
    json!({
        "interface": "web",
        "secrets": [{ "key": "api-token", "reference": "env:SHOP_API_TOKEN" }],
        "journeys": [{
            "key": "settings",
            "title": "查看設定",
            "representative": true,
            "steps": [step("home", format!("{site}/home"), "#hello"), step("settings", format!("{site}/settings"), "#title")],
        }, {
            "key": "flash",
            "title": "送出處理",
            "steps": [step("flash", format!("{site}/flash"), "#out")],
        }],
        "evidence": { "screenshots": "every_step", "trace": true },
        "feature_map": [{ "key": "settings", "title": "設定", "criterion": "看得到設定", "journeys": ["settings", "flash"] }],
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
async fn a_secret_in_an_input_value_or_an_intermediate_screen_is_not_kept_as_evidence() {
    let site = start_site().await;
    let harness = Harness::builder()
        .clock_at(Timestamp::from_millis(1_700_000_000_123))
        .browser_driver(Arc::new(LocalBrowserDriver::default()))
        .start()
        .await;
    harness.add_account("帳號 A", "account0@example.com").await;
    harness.secret_resolver.set("env:SHOP_API_TOKEN", TOKEN);
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
    let artifacts = bundle["artifacts"].as_array().unwrap();
    let screenshot = |step: &str| artifacts.iter().find(|a| a["kind"] == "screenshot" && a["step"] == step).unwrap();
    let trace = |journey: &str| artifacts.iter().find(|a| a["kind"] == "trace" && a["journey"] == journey).unwrap();

    // 權杖只在輸入欄位的值：那一步的截圖與整個 Session 的 trace 不保存；之前的截圖照常保存
    assert!(screenshot("home")["storage_path"].is_string(), "{bundle}");
    assert_eq!(screenshot("settings")["withheld"], "secret_detected", "{bundle}");
    assert_eq!(trace("settings")["withheld"], "secret_detected", "{bundle}");
    // 權杖只在中間畫面：觀察到的畫面沒有權杖，截圖照常保存；記下中間畫面的 trace 不保存
    assert!(screenshot("flash")["storage_path"].is_string(), "{bundle}");
    assert_eq!(trace("flash")["withheld"], "secret_detected", "{bundle}");

    assert!(!bundle.to_string().contains(TOKEN));
    assert_eq!(files_containing(harness.data_dir(), TOKEN), Vec::<String>::new());
}
