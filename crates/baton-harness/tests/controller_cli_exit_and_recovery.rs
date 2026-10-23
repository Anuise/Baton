//! 情境測試：Controller CLI 的完全退出與當掉恢復（desktop-controller-cli 票 07，ADR-0014），經由 Runtime harness 的真實 HTTP API 驗證。
//!
//! 與票 03 的情境測試相同：Agent Adapter 是正式的 `ClaudeCodeAdapter`，Claude 環境 Port 是 fake（記錄每次開窗的啟動規格，
//! 把宿主登記在 fake ProcessControl）。只斷言外部行為：API 回應、停止報告、時間線、Port 被呼叫的方式。
//!
//! 每個測試名稱對應票 07 的驗收條件：
//! - 關閉 App 視窗（Runtime 仍在）不影響 Controller Run：`closing_the_app_window_leaves_the_controller_cli_running`
//! - 完全退出時結束宿主、Run 進入 ended、退出進度列出這一項：`full_exit_ends_the_controller_host_and_lists_it_in_the_exit_progress`
//! - 崩潰重啟：宿主不存活 → interrupted：`after_a_crash_a_vanished_host_marks_the_run_interrupted`
//! - 崩潰重啟：宿主存活 → 舊憑證失效、頁簽提示重新開啟：`after_a_crash_a_live_host_is_rebound_and_its_old_credential_revoked`
//! - 恢復後同帳號重新開啟帶 `--continue`：`reopening_with_the_same_account_after_recovery_continues_the_native_session`、
//!   `reopening_after_an_interruption_continues_only_with_the_same_account`
//! - 沒有經過完全退出的正常重啟也同樣查驗：`an_orderly_restart_also_rebinds_a_live_controller_cli`

use std::sync::Arc;

use baton_domain::agent::AGENT_TOKEN_ENV;
use baton_domain::controller_cli::{HOST_ENDED_PATH, HOST_STARTED_PATH};
use baton_domain::Timestamp;
use baton_harness::Harness;
use baton_ports::{Liveness, TerminalLaunch};
use baton_runtime::claude_adapter::ClaudeCodeAdapter;
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

const WORKSPACES: &str = "/api/v1/workspaces";
const LIFECYCLE: &str = "/api/v1/lifecycle";
const LIFECYCLE_EXIT: &str = "/api/v1/lifecycle/exit";
const SHIM: &str = r"C:\Baton\bin\baton-mcp-shim.exe";

/// 2023-11-14 22:13:20.123 UTC
const CLOCK_START: Timestamp = Timestamp::from_millis(1_700_000_000_123);

async fn start() -> Harness {
    Harness::builder()
        .clock_at(CLOCK_START)
        .agent_adapter(|data_dir| Arc::new(ClaudeCodeAdapter::new("claude".into(), SHIM.into(), data_dir, Vec::new())))
        .start()
        .await
}

async fn new_workspace(harness: &Harness, name: &str) -> String {
    let dir = harness.data_dir().join("projects").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.to_string_lossy().into_owned();
    let (status, body) = harness.send_json(Method::POST, WORKSPACES, Some(json!({ "path": path }))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["id"].as_str().unwrap().to_string()
}

fn cli_path(ws: &str) -> String {
    format!("{WORKSPACES}/{ws}/controller-cli")
}

async fn open(harness: &Harness, ws: &str, profile: &str) -> (StatusCode, Value) {
    harness.send_json(Method::POST, &cli_path(ws), Some(json!({ "profile_id": profile }))).await
}

async fn view(harness: &Harness, ws: &str) -> Value {
    let (status, body) = harness.send_json(Method::GET, &cli_path(ws), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["run"].clone()
}

/// 開啟並讓 Supervisor 開窗，回傳 (Run, 開窗的啟動規格, Controller 憑證)。
async fn open_and_launch(harness: &Harness, ws: &str, profile: &str) -> (Value, TerminalLaunch, String) {
    let (status, body) = open(harness, ws, profile).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    harness.supervise().await;
    let run = view(harness, ws).await;
    assert_eq!(run["status"], "running", "{run}");
    let launch = harness.claude_env.controller_terminals().last().cloned().expect("應該開出 Controller 終端機");
    let token = launch.spawn.env.iter().find(|(n, _)| n == AGENT_TOKEN_ENV).map(|(_, v)| v.clone()).expect("宿主應該收到 Controller 憑證");
    (run, launch, token)
}

async fn end_from_app(harness: &Harness, ws: &str) {
    let (status, body) = harness.send_json(Method::POST, &format!("{}/end", cli_path(ws)), None).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    harness.supervise().await;
    assert_eq!(view(harness, ws).await["status"], "ended");
}

/// 開窗啟動規格中 `claude` 的參數有沒有 `--continue`。
fn continues(launch: &TerminalLaunch) -> bool {
    launch.spawn.args.iter().any(|a| a == "--continue")
}

async fn host(harness: &Harness, token: &str, path: &str, body: Value) -> StatusCode {
    harness
        .request(Method::POST, path)
        .bearer_auth(token)
        .header(baton_api::CLIENT_HEADER, baton_api::CLIENT_HEADER_VALUE)
        .json(&body)
        .send()
        .await
        .unwrap()
        .status()
}

/// 以 Controller 憑證呼叫一個唯讀的 Controller 工具；憑證失效時回 401。
async fn controller_tool_status(harness: &Harness, token: &str) -> StatusCode {
    harness.call_tool(token, "get_workflows", json!({})).await.0
}

async fn event_types(harness: &Harness, ws: &str) -> Vec<String> {
    let (status, body) = harness.send_json(Method::GET, &format!("{WORKSPACES}/{ws}/timeline"), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["aggregate_type"] == "controller_cli")
        .map(|e| e["event_type"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn closing_the_app_window_leaves_the_controller_cli_running() {
    let harness = start().await;
    let profile = harness.add_account("帳號 1", "account1@example.com").await;
    let ws = new_workspace(&harness, "main").await;
    let (run, _, token) = open_and_launch(&harness, &ws, &profile).await;

    // 關閉 App 視窗：App 只是 Runtime 的用戶端，視窗關掉後不再有任何桌面端的請求；Runtime 照常運作
    for _ in 0..5 {
        harness.clock.advance_millis(60_000);
        harness.supervise().await;
    }

    // 重新打開視窗：同一個 Controller Run 仍在執行，宿主沒有被停止，也沒有開第二個視窗
    let after = view(&harness, &ws).await;
    assert_eq!((after["id"].clone(), after["status"].as_str()), (run["id"].clone(), Some("running")), "{after}");
    assert!(after["rebound_at"].is_null(), "{after}");
    assert!(harness.process_control.stop_calls().is_empty());
    assert_eq!(harness.claude_env.controller_terminals().len(), 1);
    assert_eq!(controller_tool_status(&harness, &token).await, StatusCode::OK);
}

#[tokio::test]
async fn full_exit_ends_the_controller_host_and_lists_it_in_the_exit_progress() {
    let harness = start().await;
    let profile = harness.add_account("帳號 1", "account1@example.com").await;
    let ws = new_workspace(&harness, "main").await;
    let (run, launch, token) = open_and_launch(&harness, &ws, &profile).await;
    let identity = harness.process_control.identity_of(&launch.spawn.command_id).unwrap();
    let run_id = run["id"].as_str().unwrap();

    let (status, body) = harness.send_json(Method::POST, LIFECYCLE_EXIT, None).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    harness.wait_for_exit().await;

    // 宿主（連同它啟動的 claude）被結束
    assert_eq!(harness.process_control.stop_calls(), std::slice::from_ref(&identity));
    assert!(!harness.process_control.alive().contains(&identity));
    let ended = view(&harness, &ws).await;
    assert_eq!((ended["status"].as_str(), ended["end_code"].as_str()), (Some("ended"), Some("full_exit")), "{ended}");
    assert!(ended["end_reason"].as_str().unwrap().contains("完全退出"), "{ended}");
    assert_eq!(controller_tool_status(&harness, &token).await, StatusCode::UNAUTHORIZED, "Run 結束後憑證失效");

    // 退出進度列出這一項
    let lifecycle: Value = harness.get_json(LIFECYCLE).await;
    let entry = lifecycle["exit"]["runs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["run_id"] == run_id)
        .unwrap_or_else(|| panic!("停止報告沒有列出 Controller CLI：{lifecycle}"))
        .clone();
    assert_eq!(entry["controller_cli"], true, "{entry}");
    assert_eq!(entry["workspace_id"], ws.as_str());
    assert_eq!(entry["outcome"], "stopped_gracefully", "{entry}");
    assert_eq!(event_types(&harness, &ws).await.last().map(String::as_str), Some("controller_cli_ended"));
}

#[tokio::test]
async fn full_exit_records_a_controller_host_that_cannot_be_stopped_as_a_stop_anomaly() {
    let harness = start().await;
    let profile = harness.add_account("帳號 1", "account1@example.com").await;
    let ws = new_workspace(&harness, "main").await;
    let (run, _, _) = open_and_launch(&harness, &ws, &profile).await;
    harness.process_control.set_stop_result(baton_ports::StopResult::StillAlive);

    let (status, body) = harness.send_json(Method::POST, LIFECYCLE_EXIT, None).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    harness.wait_for_exit().await;

    let lifecycle: Value = harness.get_json(LIFECYCLE).await;
    let entry = lifecycle["exit"]["runs"].as_array().unwrap().iter().find(|r| r["run_id"] == run["id"]).cloned().unwrap();
    assert_eq!((entry["outcome"].as_str(), entry["anomaly"].as_str()), (Some("anomaly"), Some("still_alive")), "{entry}");
    assert_eq!(view(&harness, &ws).await["status"], "running", "無法確認停止時不假裝已結束");
}

#[tokio::test]
async fn after_a_crash_a_vanished_host_marks_the_run_interrupted() {
    let mut harness = start().await;
    let profile = harness.add_account("帳號 1", "account1@example.com").await;
    let ws = new_workspace(&harness, "main").await;
    let (_, launch, token) = open_and_launch(&harness, &ws, &profile).await;
    let identity = harness.process_control.identity_of(&launch.spawn.command_id).unwrap();

    // Runtime 當掉期間，使用者關掉了終端機
    harness.process_control.script_liveness(&identity, Liveness::Dead);
    harness.crash_restart().await;
    harness.supervise().await;

    let run = view(&harness, &ws).await;
    assert_eq!((run["status"].as_str(), run["end_code"].as_str()), (Some("interrupted"), Some("runtime_restarted")), "{run}");
    assert!(run["end_reason"].as_str().unwrap().contains("重新開啟"), "{run}");
    assert_eq!(controller_tool_status(&harness, &token).await, StatusCode::UNAUTHORIZED);
    assert_eq!(harness.claude_env.controller_terminals().len(), 1, "不自動重新開窗");
    assert_eq!(event_types(&harness, &ws).await.last().map(String::as_str), Some("controller_cli_interrupted"));
}

#[tokio::test]
async fn after_a_crash_a_live_host_is_rebound_and_its_old_credential_revoked() {
    let mut harness = start().await;
    let profile = harness.add_account("帳號 1", "account1@example.com").await;
    let ws = new_workspace(&harness, "main").await;
    let (run, launch, token) = open_and_launch(&harness, &ws, &profile).await;
    let identity = harness.process_control.identity_of(&launch.spawn.command_id).unwrap();
    assert_eq!(controller_tool_status(&harness, &token).await, StatusCode::OK);

    harness.crash_restart().await;
    harness.supervise().await;

    // 宿主仍存活：Run 維持執行中（不打斷使用者正在用的 CLI），但舊憑證失效，頁簽提示重新開啟
    let rebound = view(&harness, &ws).await;
    assert_eq!((rebound["id"].clone(), rebound["status"].as_str()), (run["id"].clone(), Some("running")), "{rebound}");
    assert!(rebound["rebound_at"].is_number(), "{rebound}");
    assert_eq!(controller_tool_status(&harness, &token).await, StatusCode::UNAUTHORIZED, "舊憑證失效");
    assert_eq!(host(&harness, &token, HOST_STARTED_PATH, json!({ "claude_pid": 1 })).await, StatusCode::UNAUTHORIZED);
    assert!(harness.process_control.stop_calls().is_empty(), "不結束使用者的 CLI");
    assert_eq!(harness.claude_env.controller_terminals().len(), 1, "不自動重新開窗");
    assert!(event_types(&harness, &ws).await.contains(&"controller_cli_rebound".to_string()));

    // 再處理一次不會重複重新綁定
    harness.supervise().await;
    assert_eq!(view(&harness, &ws).await["rebound_at"], rebound["rebound_at"]);
    assert_eq!(event_types(&harness, &ws).await.iter().filter(|e| *e == "controller_cli_rebound").count(), 1);

    // 舊憑證回報不了結束；使用者關掉終端機後由存活查驗結束 Run
    assert_eq!(host(&harness, &token, HOST_ENDED_PATH, json!({ "exit_code": 0 })).await, StatusCode::UNAUTHORIZED);
    harness.process_control.script_liveness(&identity, Liveness::Dead);
    harness.supervise().await;
    let ended = view(&harness, &ws).await;
    assert_eq!((ended["status"].as_str(), ended["end_code"].as_str()), (Some("ended"), Some("host_gone")), "{ended}");
}

#[tokio::test]
async fn reopening_with_the_same_account_after_recovery_continues_the_native_session() {
    let mut harness = start().await;
    let profile = harness.add_account("帳號 1", "account1@example.com").await;
    let ws = new_workspace(&harness, "main").await;
    let (_, first, _) = open_and_launch(&harness, &ws, &profile).await;
    assert!(!continues(&first), "第一次開啟沒有可接續的原生 Session：{:?}", first.spawn.args);
    let identity = harness.process_control.identity_of(&first.spawn.command_id).unwrap();

    harness.crash_restart().await;
    harness.supervise().await;
    assert!(view(&harness, &ws).await["rebound_at"].is_number());

    // 頁簽提示重新開啟：使用者從 App 結束舊的 CLI，再以同一個帳號開啟
    end_from_app(&harness, &ws).await;
    assert_eq!(harness.process_control.stop_calls(), std::slice::from_ref(&identity), "Runtime 重建後仍能結束舊宿主");
    let (run, second, token) = open_and_launch(&harness, &ws, &profile).await;

    assert!(continues(&second), "同帳號重新開啟要接續原生 Session：{:?}", second.spawn.args);
    assert_eq!(run["continue_session"], true, "{run}");
    assert_eq!(second.spawn.working_dir, first.spawn.working_dir);
    assert_eq!(controller_tool_status(&harness, &token).await, StatusCode::OK, "重新開啟取得新的憑證");
}

#[tokio::test]
async fn reopening_after_an_interruption_continues_only_with_the_same_account() {
    let mut harness = start().await;
    let first_profile = harness.add_account("帳號 1", "account1@example.com").await;
    let other_profile = harness.add_account("帳號 2", "account2@example.com").await;
    let ws = new_workspace(&harness, "main").await;
    let (_, first, _) = open_and_launch(&harness, &ws, &first_profile).await;
    let identity = harness.process_control.identity_of(&first.spawn.command_id).unwrap();
    harness.process_control.script_liveness(&identity, Liveness::Dead);
    harness.crash_restart().await;
    harness.supervise().await;
    assert_eq!(view(&harness, &ws).await["status"], "interrupted");

    // 中斷之後以同一個帳號重新開啟：接續原生 Session
    let (_, same, _) = open_and_launch(&harness, &ws, &first_profile).await;
    assert!(continues(&same), "{:?}", same.spawn.args);

    // 換帳號不接續
    end_from_app(&harness, &ws).await;
    let (run, other, _) = open_and_launch(&harness, &ws, &other_profile).await;
    assert!(!continues(&other), "{:?}", other.spawn.args);
    assert_eq!(run["continue_session"], false, "{run}");
}

#[tokio::test]
async fn an_orderly_restart_also_rebinds_a_live_controller_cli() {
    let mut harness = start().await;
    let profile = harness.add_account("帳號 1", "account1@example.com").await;
    let ws = new_workspace(&harness, "main").await;
    let (_, _, token) = open_and_launch(&harness, &ws, &profile).await;

    // 沒有經過完全退出的重啟（例如更新後重新啟動）：新的 Runtime 實例同樣不信任上一個實例交出的憑證
    harness.restart().await;
    harness.supervise().await;

    let run = view(&harness, &ws).await;
    assert_eq!(run["status"], "running", "{run}");
    assert!(run["rebound_at"].is_number(), "{run}");
    assert_eq!(controller_tool_status(&harness, &token).await, StatusCode::UNAUTHORIZED);
}
