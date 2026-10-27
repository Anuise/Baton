//! UI E2E「開啟與結束 Controller CLI」（desktop-controller-cli 票 09）：使用者在側欄選 Workspace、切到 Controller 頁簽、
//! 選一個 Account Profile 按「開啟」；Supervisor 經 fake Claude 環境 Port 開出終端機後頁簽顯示「執行中」；
//! 使用者按「結束」，Supervisor 停止宿主後頁簽顯示「已結束」。
//!
//! 前置（第二個帳號、兩個 Workspace）直接以 API 建立；使用者的動作一律經畫面完成。Supervisor 的推進（`supervise`）
//! 在 Rust 端進行，頁簽開著時每 2 秒自行重新讀取，所以只靠瀏覽器的等待看到狀態變化。
//! API 與 fake Port 只用來讀取（佐證開窗的帳號、工作目錄與宿主已停止），不改任何狀態。

use reqwest::{Method, StatusCode};
use serde_json::json;

use crate::browser::{exact_text, role, text};
use crate::UiE2e;

const WORKSPACES: &str = "/api/v1/workspaces";
/// 被選為 Controller 帳號的 Profile（`UiE2e::start` 已有一個「主要帳號」，頁簽預設選它）。
const CONTROLLER_PROFILE: &str = "協調帳號";

#[tokio::test(flavor = "multi_thread")]
#[ignore = "UI E2E：需要 Playwright，以 node scripts/test-e2e.mjs 執行"]
async fn a_controller_cli_opened_from_the_controller_tab_runs_and_ends() {
    let e2e = UiE2e::start().await;
    let (harness, browser) = (&e2e.harness, &e2e.browser);

    // 前置：第二個可用帳號與兩個 Workspace；重新載入讓側欄列出它們
    let profile = harness.add_account(CONTROLLER_PROFILE, "coordinator@example.com").await;
    let config_dir = profile_config_dir(harness, &profile).await;
    new_workspace(harness, "alpha").await;
    let (ws, ws_path) = new_workspace(harness, "beta").await;
    browser.goto(&e2e.server.url("/")).await;

    // 1. 在側欄選 Workspace，切到 Controller 頁簽：尚未開啟
    browser.click(&[role("button", "選擇 beta")]).await;
    browser.click(&[role("tablist", "Workspace 功能"), role("tab", "Controller")]).await;
    let page = role("region", "Controller");
    browser.wait_visible(&[page.clone(), role("heading", "Controller：beta")]).await;
    browser.wait_visible(&[page.clone(), exact_text("未開啟")]).await;

    // 2. 選 Controller 帳號（兩個帳號都還沒有額度觀測、沒有執行中的 Worker）並開啟
    let option = format!("{CONTROLLER_PROFILE}（5h 未知｜7d 未知｜執行中 Worker 0）");
    browser.select(&[page.clone(), role("combobox", "Controller 帳號")], &option).await;
    browser.click(&[page.clone(), role("button", "開啟")]).await;
    browser.wait_visible(&[page.clone(), exact_text("開啟中")]).await;

    // 3. Supervisor 開出終端機：頁簽在下一次重新讀取時顯示執行中、Controller 帳號是選的那個
    harness.supervise().await;
    browser.wait_visible(&[page.clone(), exact_text("執行中")]).await;
    browser.wait_visible(&[page.clone(), exact_text(CONTROLLER_PROFILE)]).await;
    browser.wait_visible(&[page.clone(), text("Controller CLI 已開啟，請切到它的終端機視窗")]).await;
    // 只開了一個視窗：以選的帳號目錄、在 Workspace 目錄中執行 Controller 宿主
    let launches = harness.claude_env.controller_terminals();
    assert_eq!(launches.len(), 1, "{launches:?}");
    let spawn = &launches[0].spawn;
    assert_eq!(spawn.working_dir.as_deref(), Some(ws_path.as_str()));
    assert_eq!(env_of(spawn, "CLAUDE_CONFIG_DIR").as_deref(), Some(config_dir.as_str()), "{spawn:?}");
    let host = harness.process_control.identity_of(&spawn.command_id).expect("宿主登記在 fake ProcessControl");
    assert!(harness.process_control.alive().contains(&host));

    // 4. 結束：按下後顯示「結束中…」；Supervisor 停止宿主後頁簽顯示已結束，可以再次開啟
    browser.click(&[page.clone(), role("button", "結束")]).await;
    browser.wait_visible(&[page.clone(), role("button", "結束中…")]).await;
    harness.supervise().await;
    browser.wait_visible(&[page.clone(), exact_text("已結束")]).await;
    browser.wait_visible(&[page.clone(), text("結束原因：")]).await;
    browser.wait_visible(&[page.clone(), role("button", "開啟")]).await;

    // 5. 佐證：宿主已被停止、不再存活；Run 已結束；沒有開出第二個視窗
    assert_eq!(harness.process_control.stop_calls(), vec![host.clone()]);
    assert!(!harness.process_control.alive().contains(&host), "宿主不應該再存活");
    let (status, view) = harness.send_json(Method::GET, &format!("{WORKSPACES}/{ws}/controller-cli"), None).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(view["run"]["status"], "ended", "{view}");
    assert_eq!(view["run"]["profile_id"], profile.as_str(), "{view}");
    assert_eq!(harness.claude_env.controller_terminals().len(), 1);

    e2e.finish().await;
}

/// 以 API 建立 Workspace（目錄放在 harness 的資料目錄內，隨 harness 刪除），回傳 (識別, 正規化後的路徑)。
async fn new_workspace(harness: &baton_harness::Harness, name: &str) -> (String, String) {
    let dir = harness.data_dir().join("projects").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let body = json!({ "path": dir.to_string_lossy() });
    let (status, created) = harness.send_json(Method::POST, WORKSPACES, Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    (created["id"].as_str().unwrap().to_string(), created["path"].as_str().unwrap().to_string())
}

async fn profile_config_dir(harness: &baton_harness::Harness, profile: &str) -> String {
    let (status, list) = harness.send_json(Method::GET, "/api/v1/profiles", None).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let profiles = list["profiles"].as_array().expect("Profile 清單是陣列");
    let found = profiles.iter().find(|p| p["id"] == profile).expect("找得到剛建立的 Profile");
    found["config_dir"].as_str().expect("Profile 有 config_dir").to_string()
}

fn env_of(spawn: &baton_ports::SpawnSpec, name: &str) -> Option<String> {
    spawn.env.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone())
}
