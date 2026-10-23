//! 情境測試：Controller 帳號排除、同帳號接續與換帳號重開（desktop-controller-cli 票 05），經由 Runtime harness 的真實 HTTP API 驗證。
//!
//! 排程情境用 fake AgentAdapter（Worker 何時結束由測試決定）；啟動規格的情境用正式的 `ClaudeCodeAdapter`（`--continue` 是真的組裝）。
//! Claude 環境 Port 是 fake：記錄每次開窗的啟動規格，宿主登記在 fake ProcessControl。只斷言外部行為：API 回應、排程總覽、
//! 時間線、Port 被呼叫的方式（開了幾個視窗、啟動規格、停止了哪個程序）。
//!
//! 每個測試名稱對應票 05 的驗收條件：
//! - Controller Run 非終態期間不分給新 Worker、既有 Worker 繼續、結束後恢復：
//!   `while_the_controller_cli_is_open_its_account_gets_no_new_workers_until_it_ends`、
//!   `another_account_takes_the_work_while_the_controller_account_is_excluded`
//! - 同一 Workspace 同一 Profile 重開帶 `--continue`（原生 Session 確定不存在時不帶），換 Profile 不帶：
//!   `reopening_with_the_same_profile_continues_the_native_session`
//! - 換帳號重開先結束再開新的，時間線可追查：`switching_accounts_ends_the_current_cli_then_opens_a_new_session_with_the_new_profile`、
//!   `a_switch_still_opens_the_new_cli_when_the_terminal_was_closed_first`、`switching_is_refused_for_the_same_or_an_unusable_profile`
//! - 換帳號開出的新 Run 接續上一個 Controller 確認到的事件位置（票 06）：`the_switched_controller_continues_the_event_cursor`
//! - Runtime 重啟後被重新綁定（票 07）的 Controller CLI 也能換帳號重開：`a_rebound_controller_cli_can_switch_accounts`

use std::sync::Arc;

use baton_domain::agent::AGENT_TOKEN_ENV;
use baton_domain::controller_cli::HOST_ENDED_PATH;
use baton_domain::controller_wait::WAIT_PATH;
use baton_domain::Timestamp;
use baton_harness::Harness;
use baton_ports::{AdapterEvent, TerminalLaunch};
use baton_runtime::claude_adapter::ClaudeCodeAdapter;
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

const WORKSPACES: &str = "/api/v1/workspaces";
const SCHEDULING: &str = "/api/v1/scheduling";
const SHIM: &str = r"C:\Baton\bin\baton-mcp-shim.exe";

/// 2023-11-14 22:13:20.123 UTC
const CLOCK_START: Timestamp = Timestamp::from_millis(1_700_000_000_123);

// --- 共用輔助 ---

/// 正式的 Claude Code Adapter（啟動規格是真的組裝）。
async fn start_with_real_adapter() -> Harness {
    Harness::builder()
        .clock_at(CLOCK_START)
        .agent_adapter(|data_dir| Arc::new(ClaudeCodeAdapter::new("claude".into(), SHIM.into(), data_dir, Vec::new())))
        .start()
        .await
}

async fn new_workspace(harness: &Harness, name: &str) -> String {
    let dir = harness.data_dir().join("projects").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let (status, body) =
        harness.send_json(Method::POST, WORKSPACES, Some(json!({ "path": dir.to_string_lossy() }))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["id"].as_str().unwrap().to_string()
}

fn cli_path(ws: &str) -> String {
    format!("{WORKSPACES}/{ws}/controller-cli")
}

async fn open(harness: &Harness, ws: &str, profile: &str) -> (StatusCode, Value) {
    harness.send_json(Method::POST, &cli_path(ws), Some(json!({ "profile_id": profile }))).await
}

async fn switch(harness: &Harness, ws: &str, profile: &str) -> (StatusCode, Value) {
    harness.send_json(Method::POST, &format!("{}/switch", cli_path(ws)), Some(json!({ "profile_id": profile }))).await
}

async fn end(harness: &Harness, ws: &str) {
    let (status, body) = harness.send_json(Method::POST, &format!("{}/end", cli_path(ws)), None).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    harness.supervise().await;
}

async fn view(harness: &Harness, ws: &str) -> Value {
    let (status, body) = harness.send_json(Method::GET, &cli_path(ws), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["run"].clone()
}

/// 開啟並讓 Supervisor 開窗，回傳 (Run, 開窗的啟動規格)。
async fn open_and_launch(harness: &Harness, ws: &str, profile: &str) -> (Value, TerminalLaunch) {
    let (status, body) = open(harness, ws, profile).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    harness.supervise().await;
    let run = view(harness, ws).await;
    assert_eq!(run["status"], "running", "{run}");
    (run, last_terminal(harness))
}

fn last_terminal(harness: &Harness) -> TerminalLaunch {
    harness.claude_env.controller_terminals().last().cloned().expect("應該開出 Controller 終端機")
}

fn env_of(launch: &TerminalLaunch, name: &str) -> Option<String> {
    launch.spawn.env.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone())
}

fn has_continue(launch: &TerminalLaunch) -> bool {
    launch.spawn.args.iter().any(|a| a == "--continue")
}

async fn config_dir_of(harness: &Harness, profile: &str) -> String {
    let profiles = harness.send_json(Method::GET, "/api/v1/profiles", None).await.1;
    profiles["profiles"].as_array().unwrap().iter().find(|p| p["id"] == profile).unwrap()["config_dir"]
        .as_str()
        .unwrap()
        .to_string()
}

/// 模擬 claude 在這個帳號下為工作目錄留下原生 Session 檔（`projects\{目錄名}\*.jsonl`，目錄名是非英數字換成 `-`）。
async fn write_native_session(harness: &Harness, profile: &str, working_dir: &str) {
    let project: String = working_dir.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let dir = std::path::Path::new(&config_dir_of(harness, profile).await).join("projects").join(project);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("326f0edb-57c5-40e9-8560-99ae95e9027e.jsonl"), "{}\n").unwrap();
}

async fn timeline_events(harness: &Harness, ws: &str) -> Vec<Value> {
    let (status, body) = harness.send_json(Method::GET, &format!("{WORKSPACES}/{ws}/timeline"), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["events"].as_array().unwrap().iter().filter(|e| e["aggregate_type"] == "controller_cli").cloned().collect()
}

fn filled(text: &str) -> Value {
    json!({ "status": "filled", "text": text })
}

/// 建立一個 Goal 底下 ready 的 Task（不需要任何能力），回傳識別。
async fn ready_task(harness: &Harness, ws: &str, title: &str) -> String {
    let (status, goal) =
        harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/goals"), Some(json!({ "title": title }))).await;
    assert_eq!(status, StatusCode::CREATED, "{goal}");
    let contract = json!({
        "objective": filled("整理資料"),
        "inputs": filled("既有的資料"),
        "scope": filled("公開資料"),
        "deliverables": filled("比較表"),
        "done_criteria": filled("五個產品"),
        "blocker_handling": filled("卡住時回報"),
        "required_capabilities": [],
    });
    let body = json!({ "goal_id": goal["id"], "title": title, "contract": contract });
    let (status, task) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks"), Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    let tid = task["id"].as_str().unwrap().to_string();
    let (status, ready) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks/{tid}/ready"), None).await;
    assert_eq!(status, StatusCode::OK, "{ready}");
    tid
}

async fn dispatch(harness: &Harness, ws: &str, tid: &str) -> (StatusCode, Value) {
    harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks/{tid}/dispatch"), None).await
}

async fn run_view(harness: &Harness, ws: &str, rid: &str) -> Value {
    let (status, run) = harness.send_json(Method::GET, &format!("{WORKSPACES}/{ws}/runs/{rid}"), None).await;
    assert_eq!(status, StatusCode::OK, "{run}");
    run
}

async fn overview(harness: &Harness) -> Value {
    let (status, body) = harness.send_json(Method::GET, SCHEDULING, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

/// 佇列中這個 Task 的等待原因代碼。
fn queued_codes(overview: &Value, tid: &str) -> Vec<String> {
    let entry = overview["queue"].as_array().unwrap().iter().find(|e| e["task_id"] == tid);
    let entry = entry.unwrap_or_else(|| panic!("{tid} 不在佇列：{overview}"));
    entry["reasons"].as_array().unwrap().iter().map(|r| r["code"].as_str().unwrap().to_string()).collect()
}

// --- 帳號排除 ---

#[tokio::test]
async fn while_the_controller_cli_is_open_its_account_gets_no_new_workers_until_it_ends() {
    let harness = Harness::builder().clock_at(CLOCK_START).start().await;
    let profile = harness.add_account("帳號 1", "account1@example.com").await;
    let ws = new_workspace(&harness, "main").await;

    // 開啟 Controller 之前已在執行的 Worker
    let before = ready_task(&harness, &ws, "已在執行").await;
    let (status, running) = dispatch(&harness, &ws, &before).await;
    assert_eq!(status, StatusCode::CREATED, "{running}");
    let worker = running["id"].as_str().unwrap().to_string();
    harness.supervise().await;

    open_and_launch(&harness, &ws, &profile).await;

    // 新的 Task 排隊，原因是帳號正在當 Controller 帳號
    let waiting = ready_task(&harness, &ws, "等 Controller 結束").await;
    let (status, queued) = dispatch(&harness, &ws, &waiting).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{queued}");
    let codes: Vec<&str> = queued["reasons"].as_array().unwrap().iter().map(|r| r["code"].as_str().unwrap()).collect();
    assert_eq!(codes, ["controller_account"], "{queued}");
    harness.supervise().await;
    assert_eq!(queued_codes(&overview(&harness).await, &waiting), ["controller_account"]);

    // 既有的 Worker 不受影響：沒有被停止、仍在執行
    assert!(harness.process_control.stop_calls().is_empty(), "開啟 Controller 不中斷既有 Worker");
    assert_eq!(run_view(&harness, &ws, &worker).await["status"], "running");

    // Controller CLI 結束後恢復為候選：佇列中的 Task 派給它
    end(&harness, &ws).await;
    harness.supervise().await;
    let after = overview(&harness).await;
    assert!(after["queue"].as_array().unwrap().is_empty(), "{after}");
    let runs = after["accounts"][0]["runs"].as_array().unwrap();
    assert!(runs.iter().any(|r| r["task_id"] == waiting.as_str()), "{after}");

    // 收尾：既有 Worker 照常結束
    harness.agent_adapter.emit(&worker, AdapterEvent::Exited { success: true });
    harness.supervise().await;
    assert_eq!(run_view(&harness, &ws, &worker).await["status"], "succeeded");
}

#[tokio::test]
async fn another_account_takes_the_work_while_the_controller_account_is_excluded() {
    let harness = Harness::builder().clock_at(CLOCK_START).start().await;
    let controller = harness.add_account("帳號 1", "account1@example.com").await;
    let worker_account = harness.add_account("帳號 2", "account2@example.com").await;
    let ws = new_workspace(&harness, "main").await;
    let other_ws = new_workspace(&harness, "other").await;
    open_and_launch(&harness, &ws, &controller).await;

    // 帳號 1 較早建立、沒有任何 Run，平常會先拿到工作；開著 Controller 時改給帳號 2（任何 Workspace 的 Task 都一樣）
    for (workspace, title) in [(&ws, "第一個"), (&other_ws, "第二個")] {
        let tid = ready_task(&harness, workspace, title).await;
        let (status, run) = dispatch(&harness, workspace, &tid).await;
        assert_eq!(status, StatusCode::CREATED, "{run}");
        assert_eq!(run["profile_id"], worker_account.as_str(), "{run}");
    }
}

// --- 同帳號接續 ---

#[tokio::test]
async fn reopening_with_the_same_profile_continues_the_native_session() {
    let harness = start_with_real_adapter().await;
    let first = harness.add_account("帳號 1", "account1@example.com").await;
    let second = harness.add_account("帳號 2", "account2@example.com").await;
    let ws = new_workspace(&harness, "main").await;

    let (run, launch) = open_and_launch(&harness, &ws, &first).await;
    assert!(!has_continue(&launch), "第一次開啟沒有可接續的 Session：{:?}", launch.spawn.args);
    assert_eq!(run["continue_session"], false);
    end(&harness, &ws).await;

    // 上次開啟後沒有任何對話（沒有留下原生 Session 檔）：同帳號重開也開新的 Session，否則 claude --continue 會直接結束
    let (run, launch) = open_and_launch(&harness, &ws, &first).await;
    assert!(!has_continue(&launch), "{:?}", launch.spawn.args);
    assert_eq!(run["continue_session"], false);
    write_native_session(&harness, &first, run["working_dir"].as_str().unwrap()).await;
    end(&harness, &ws).await;

    // 同一 Workspace、同一 Profile 重開：接回原生 Session
    let (run, launch) = open_and_launch(&harness, &ws, &first).await;
    assert!(has_continue(&launch), "{:?}", launch.spawn.args);
    assert_eq!(run["continue_session"], true);
    let opened = timeline_events(&harness, &ws).await.into_iter().rfind(|e| e["event_type"] == "controller_cli_opened").unwrap();
    assert_eq!(opened["payload"]["continue_session"], true);
    end(&harness, &ws).await;

    // 換 Profile：不接續
    let (run, launch) = open_and_launch(&harness, &ws, &second).await;
    assert!(!has_continue(&launch), "{:?}", launch.spawn.args);
    assert_eq!(run["continue_session"], false);
    end(&harness, &ws).await;

    // 其他 Workspace 的 Controller 不算：同一 Profile 第一次在這個 Workspace 開啟不接續
    let other = new_workspace(&harness, "other").await;
    let (_, launch) = open_and_launch(&harness, &other, &second).await;
    assert!(!has_continue(&launch), "{:?}", launch.spawn.args);
}

// --- 換帳號重開 ---

#[tokio::test]
async fn switching_accounts_ends_the_current_cli_then_opens_a_new_session_with_the_new_profile() {
    let harness = start_with_real_adapter().await;
    let first = harness.add_account("帳號 1", "account1@example.com").await;
    let second = harness.add_account("帳號 2", "account2@example.com").await;
    let ws = new_workspace(&harness, "main").await;
    // 先以帳號 2 跑過一次，再以帳號 1 開著：換回帳號 2 時也不接續（換帳號一律開新 Session）
    open_and_launch(&harness, &ws, &second).await;
    end(&harness, &ws).await;
    let (old, old_launch) = open_and_launch(&harness, &ws, &first).await;
    let old_token = env_of(&old_launch, AGENT_TOKEN_ENV).unwrap();
    let old_host = harness.process_control.identity_of(&old_launch.spawn.command_id).unwrap();
    let terminals_before = harness.claude_env.controller_terminals().len();

    let (status, body) = switch(&harness, &ws, &second).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert_eq!(body["run"]["id"], old["id"]);
    assert_eq!(body["run"]["switch_to_profile_id"], second.as_str());
    assert!(body["run"]["end_requested_at"].is_number(), "{body}");

    // 先結束目前的 Controller CLI（停止宿主、憑證失效），同時開出新的 Run
    harness.supervise().await;
    assert!(harness.process_control.stop_calls().contains(&old_host), "結束目前的宿主");
    let (status, _) = harness.call_tool(&old_token, "get_workflows", json!({})).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "舊的 Controller 憑證失效");
    let new = view(&harness, &ws).await;
    assert_ne!(new["id"], old["id"]);
    assert_eq!(new["profile_id"], second.as_str());
    assert_eq!(new["switched_from_run_id"], old["id"]);
    assert_eq!(new["continue_session"], false);

    // 再處理一次就開出新的視窗：帳號 2 的目錄、不接續
    harness.supervise().await;
    let launches = harness.claude_env.controller_terminals();
    assert_eq!(launches.len(), terminals_before + 1);
    let new_launch = launches.last().unwrap();
    assert_eq!(env_of(new_launch, "CLAUDE_CONFIG_DIR"), Some(config_dir_of(&harness, &second).await));
    assert!(!has_continue(new_launch), "{:?}", new_launch.spawn.args);
    assert_eq!(view(&harness, &ws).await["status"], "running");

    // 時間線：換帳號要求 → 舊的結束（switched）→ 新的開啟（指回舊的）→ 執行中
    let events = timeline_events(&harness, &ws).await;
    let tail: Vec<(String, String)> = events[events.len() - 5..]
        .iter()
        .map(|e| (e["aggregate_id"].as_str().unwrap().to_string(), e["event_type"].as_str().unwrap().to_string()))
        .collect();
    let (old_id, new_id) = (old["id"].as_str().unwrap().to_string(), new["id"].as_str().unwrap().to_string());
    assert_eq!(
        tail,
        [
            (old_id.clone(), "controller_cli_switch_requested".to_string()),
            (old_id.clone(), "controller_cli_ended".to_string()),
            (new_id.clone(), "controller_cli_opened".to_string()),
            (new_id.clone(), "controller_cli_launch_started".to_string()),
            (new_id, "controller_cli_running".to_string()),
        ]
    );
    let requested = &events[events.len() - 5];
    assert_eq!((requested["payload"]["from_profile_id"].as_str(), requested["payload"]["to_profile_id"].as_str()), (Some(first.as_str()), Some(second.as_str())));
    assert_eq!(events[events.len() - 4]["payload"]["end_code"], "switched");
    assert_eq!(events[events.len() - 3]["payload"]["switched_from_run_id"], old_id.as_str());
    assert_eq!(events[events.len() - 3]["payload"]["profile_id"], second.as_str());
}

#[tokio::test]
async fn a_switch_still_opens_the_new_cli_when_the_terminal_was_closed_first() {
    let harness = Harness::builder().clock_at(CLOCK_START).start().await;
    let first = harness.add_account("帳號 1", "account1@example.com").await;
    let second = harness.add_account("帳號 2", "account2@example.com").await;
    let ws = new_workspace(&harness, "main").await;
    let (old, launch) = open_and_launch(&harness, &ws, &first).await;
    let token = env_of(&launch, AGENT_TOKEN_ENV).unwrap();
    assert_eq!(switch(&harness, &ws, &second).await.0, StatusCode::ACCEPTED);

    // Supervisor 處理之前，使用者自己在終端機結束了 claude
    let response = harness
        .request(Method::POST, HOST_ENDED_PATH)
        .bearer_auth(&token)
        .header(baton_api::CLIENT_HEADER, baton_api::CLIENT_HEADER_VALUE)
        .json(&json!({ "exit_code": 0 }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let new = view(&harness, &ws).await;
    assert_eq!((new["profile_id"].as_str(), new["switched_from_run_id"].as_str()), (Some(second.as_str()), old["id"].as_str()), "{new}");
    harness.supervise().await;
    assert_eq!(view(&harness, &ws).await["status"], "running");
    assert_eq!(harness.claude_env.controller_terminals().len(), 2);
}

#[tokio::test]
async fn switching_is_refused_for_the_same_or_an_unusable_profile() {
    let harness = Harness::builder().clock_at(CLOCK_START).start().await;
    let first = harness.add_account("帳號 1", "account1@example.com").await;
    let ws = new_workspace(&harness, "main").await;

    let (status, refused) = switch(&harness, &ws, &first).await;
    assert_eq!((status, refused["error"].as_str()), (StatusCode::CONFLICT, Some("controller_cli_not_open")), "{refused}");

    let (old, _) = open_and_launch(&harness, &ws, &first).await;
    let (status, refused) = switch(&harness, &ws, &first).await;
    assert_eq!((status, refused["error"].as_str()), (StatusCode::CONFLICT, Some("controller_cli_same_profile")), "{refused}");

    let (status, created) = harness.send_json(Method::POST, "/api/v1/profiles", Some(json!({ "name": "未登入" }))).await;
    assert!(status.is_success(), "{created}");
    let (status, refused) = switch(&harness, &ws, created["id"].as_str().unwrap()).await;
    assert_eq!((status, refused["error"].as_str()), (StatusCode::CONFLICT, Some("profile_unavailable")), "{refused}");

    // 被拒時目前的 Controller CLI 不受影響
    harness.supervise().await;
    let run = view(&harness, &ws).await;
    assert_eq!((run["id"].clone(), run["status"].as_str(), run["end_requested_at"].clone()), (old["id"].clone(), Some("running"), Value::Null));
}

// --- 換帳號與事件等待 ---

/// 以 Controller 憑證查一次事件（不等待）；有事件就確認，回傳各事件的 `run_id`。
async fn take_events(harness: &Harness, token: &str) -> Vec<Value> {
    let wait = |body: Value| async move {
        let response = harness
            .request(Method::POST, WAIT_PATH)
            .bearer_auth(token)
            .header(baton_api::CLIENT_HEADER, baton_api::CLIENT_HEADER_VALUE)
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        response.json::<Value>().await.unwrap()
    };
    let response = wait(json!({ "timeout_seconds": 0 })).await;
    let events = response["events"].as_array().unwrap().clone();
    if !events.is_empty() {
        wait(json!({ "ack": response["cursor"], "timeout_seconds": 0 })).await;
    }
    events.iter().map(|e| e["run_id"].clone()).collect()
}

/// 派工並讓 Worker 結束（成功），回傳 Run 的識別。
async fn worker_runs_to_the_end(harness: &Harness, ws: &str, title: &str) -> String {
    let tid = ready_task(harness, ws, title).await;
    let (status, run) = dispatch(harness, ws, &tid).await;
    assert_eq!(status, StatusCode::CREATED, "{run}");
    let rid = run["id"].as_str().unwrap().to_string();
    harness.supervise().await;
    harness.agent_adapter.emit(&rid, AdapterEvent::Exited { success: true });
    harness.supervise().await;
    rid
}

#[tokio::test]
async fn the_switched_controller_continues_the_event_cursor() {
    let harness = Harness::builder().clock_at(CLOCK_START).start().await;
    let first = harness.add_account("帳號 1", "account1@example.com").await;
    let second = harness.add_account("帳號 2", "account2@example.com").await;
    harness.add_account("Worker 帳號", "worker@example.com").await;
    let ws = new_workspace(&harness, "main").await;
    let (_, launch) = open_and_launch(&harness, &ws, &first).await;
    let old_token = env_of(&launch, AGENT_TOKEN_ENV).unwrap();

    // 舊的 Controller 確認了第一個 Worker 的結束；第二個 Worker 結束時還沒確認就換帳號
    let confirmed = worker_runs_to_the_end(&harness, &ws, "一").await;
    assert_eq!(take_events(&harness, &old_token).await, [json!(confirmed)]);
    let pending = worker_runs_to_the_end(&harness, &ws, "二").await;
    assert_eq!(switch(&harness, &ws, &second).await.0, StatusCode::ACCEPTED);
    harness.supervise().await;
    harness.supervise().await;
    let new = view(&harness, &ws).await;
    assert_eq!((new["profile_id"].as_str(), new["status"].as_str()), (Some(second.as_str()), Some("running")), "{new}");

    // 新的 Controller 從舊的確認到的位置接續：收到沒確認的，已確認的不重複
    let new_token = env_of(&last_terminal(&harness), AGENT_TOKEN_ENV).unwrap();
    assert_eq!(take_events(&harness, &new_token).await, [json!(pending)]);
}

#[tokio::test]
async fn a_rebound_controller_cli_can_switch_accounts() {
    let mut harness = Harness::builder().clock_at(CLOCK_START).start().await;
    let first = harness.add_account("帳號 1", "account1@example.com").await;
    let second = harness.add_account("帳號 2", "account2@example.com").await;
    let ws = new_workspace(&harness, "main").await;
    let (old, launch) = open_and_launch(&harness, &ws, &first).await;
    let old_host = harness.process_control.identity_of(&launch.spawn.command_id).unwrap();

    // 當掉重啟後宿主仍存活：重新綁定（舊憑證失效），使用者改以換帳號重開
    harness.crash_restart().await;
    harness.supervise().await;
    assert!(view(&harness, &ws).await["rebound_at"].is_number());
    assert_eq!(switch(&harness, &ws, &second).await.0, StatusCode::ACCEPTED);
    harness.supervise().await;
    harness.supervise().await;

    assert!(harness.process_control.stop_calls().contains(&old_host), "Runtime 重建後仍能結束舊宿主");
    let new = view(&harness, &ws).await;
    assert_eq!(new["switched_from_run_id"], old["id"]);
    assert_eq!((new["status"].as_str(), new["rebound_at"].clone()), (Some("running"), Value::Null), "{new}");
    let token = env_of(&last_terminal(&harness), AGENT_TOKEN_ENV).unwrap();
    assert_eq!(harness.call_tool(&token, "get_workflows", json!({})).await.0, StatusCode::OK, "新的 Run 取得新的憑證");
}
