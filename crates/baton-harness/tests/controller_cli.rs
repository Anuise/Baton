//! 情境測試：Controller CLI 的開啟與結束（desktop-controller-cli 票 03，ADR-0014），經由 Runtime harness 的真實 HTTP API 驗證。
//!
//! Agent Adapter 是正式的 `ClaudeCodeAdapter`（啟動規格是真的組裝），Claude 環境 Port 是 fake：它記錄每次開窗的完整啟動規格，
//! 把開出的宿主登記在 fake ProcessControl（與正式版相同：之後以 command_id 找回、查驗存活、停止）。
//! 宿主的回報由測試以開窗時交給宿主的 Controller 憑證呼叫 Agent API 扮演。只斷言外部行為：API 回應、時間線、
//! Port 被呼叫的方式（開了幾個視窗、啟動規格、停止了哪個程序）。
//!
//! 每個測試名稱對應票 03 的驗收條件：
//! - 開出終端機、啟動規格正確：`opening_launches_native_claude_in_a_terminal_with_the_shared_assembly`
//! - starting → running → ended、時間線：同上與 `the_host_reporting_the_end_ends_the_run_and_revokes_the_credential`
//! - 同一 Workspace 第二次開啟被拒、同一 Profile 可當兩個 Workspace 的 Controller：
//!   `a_second_open_in_the_same_workspace_is_refused_but_a_profile_may_serve_two_workspaces`
//! - 宿主回報結束、宿主消失、使用者按結束都結束 Run 並使憑證失效：`the_host_reporting_*`、`a_vanished_host_*`、`ending_from_the_app_*`
//! - launch intent 與開窗回執之間崩潰不重複開窗：`a_crash_between_launch_intent_and_receipt_*`
//! - 開窗失敗、找不到 claude、Profile 失效：`open_failures_*`、`an_unusable_profile_*`
//! - Controller 憑證可呼叫 Controller 工具：`the_controller_credential_can_propose_and_dispatch_tasks`
//! - 最終 review 修正：Run 目錄只放 MCP 設定，不寫用不到的 Worker 檔案：`the_run_directory_gets_the_mcp_config_but_no_worker_files`

use std::sync::Arc;
use std::time::Duration;

use baton_domain::agent::{AGENT_TOKEN_ENV, RUNTIME_URL_ENV};
use baton_domain::controller_cli::{HOST_ENDED_PATH, HOST_STARTED_PATH, HOST_SUBCOMMAND};
use baton_domain::Timestamp;
use baton_harness::Harness;
use baton_ports::{Liveness, TerminalLaunch, ToolProbe};
use baton_runtime::claude_adapter::{ClaudeCodeAdapter, PROMPT_FILE, RUNS_DIR, RUN_MCP_FILE};
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

const WORKSPACES: &str = "/api/v1/workspaces";
const SHIM: &str = r"C:\Baton\bin\baton-mcp-shim.exe";
/// Runtime「繼承」到的環境變數名稱：含會改變認證來源與外層代理留下的變數。
const INHERITED: [&str; 5] = ["PATH", "ANTHROPIC_API_KEY", "CLAUDECODE", "AI_AGENT", "BAGGAGE"];

/// 2023-11-14 22:13:20.123 UTC
const CLOCK_START: Timestamp = Timestamp::from_millis(1_700_000_000_123);

async fn start() -> Harness {
    Harness::builder()
        .clock_at(CLOCK_START)
        .agent_adapter(|data_dir| {
            let inherited = INHERITED.iter().map(|s| s.to_string()).collect();
            Arc::new(ClaudeCodeAdapter::new("claude".into(), SHIM.into(), data_dir, inherited))
        })
        .start()
        .await
}

async fn new_workspace(harness: &Harness, name: &str) -> (String, String) {
    let dir = harness.data_dir().join("projects").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.to_string_lossy().into_owned();
    let (status, body) = harness.send_json(Method::POST, WORKSPACES, Some(json!({ "path": path }))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    (body["id"].as_str().unwrap().to_string(), body["path"].as_str().unwrap().to_string())
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
    assert_eq!(body["run"]["status"], "starting");
    harness.supervise().await;
    let run = view(harness, ws).await;
    let launch = harness
        .claude_env
        .controller_terminals()
        .into_iter()
        .rfind(|l| l.spawn.working_dir.as_deref() == run["working_dir"].as_str())
        .expect("應該開出 Controller 終端機");
    (run, launch.clone(), env_of(&launch, AGENT_TOKEN_ENV).expect("宿主應該收到 Controller 憑證"))
}

fn env_of(launch: &TerminalLaunch, name: &str) -> Option<String> {
    launch.spawn.env.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone())
}

fn value_after<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).map(String::as_str)
}

async fn host(harness: &Harness, token: &str, path: &str, body: Value) -> (StatusCode, Value) {
    let response = harness
        .request(Method::POST, path)
        .bearer_auth(token)
        .header(baton_api::CLIENT_HEADER, baton_api::CLIENT_HEADER_VALUE)
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = response.status();
    (status, response.json().await.unwrap_or(Value::Null))
}

/// 以 Controller 憑證呼叫一個唯讀的 Controller 工具；憑證失效時回 401。
async fn controller_tool_status(harness: &Harness, token: &str) -> StatusCode {
    harness.call_tool(token, "get_workflows", json!({})).await.0
}

async fn timeline_events(harness: &Harness, ws: &str) -> Vec<Value> {
    let (status, body) = harness.send_json(Method::GET, &format!("{WORKSPACES}/{ws}/timeline"), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["events"].as_array().unwrap().iter().filter(|e| e["aggregate_type"] == "controller_cli").cloned().collect()
}

fn event_types(events: &[Value]) -> Vec<String> {
    events.iter().map(|e| e["event_type"].as_str().unwrap().to_string()).collect()
}

#[tokio::test]
async fn opening_launches_native_claude_in_a_terminal_with_the_shared_assembly() {
    let harness = start().await;
    let profile = harness.add_account("帳號 1", "account1@example.com").await;
    let (ws, ws_path) = new_workspace(&harness, "main").await;
    let profiles = harness.send_json(Method::GET, "/api/v1/profiles", None).await.1;
    let config_dir = profiles["profiles"].as_array().unwrap().iter().find(|p| p["id"] == profile.as_str()).unwrap()["config_dir"]
        .as_str()
        .unwrap()
        .to_string();

    let (run, launch, token) = open_and_launch(&harness, &ws, &profile).await;

    assert_eq!(harness.claude_env.controller_terminals().len(), 1);
    assert_eq!(run["status"], "running", "{run}");
    assert_eq!(run["profile_id"], profile.as_str());
    assert!(run["opened_at"].is_number() && run["running_at"].is_number(), "{run}");
    assert!(launch.title.contains("Baton Controller"), "{}", launch.title);

    // 終端機裡執行的是 Controller 宿主，由它啟動互動式 claude
    let spawn = &launch.spawn;
    assert_eq!(spawn.program, SHIM);
    assert_eq!(&spawn.args[..2], [HOST_SUBCOMMAND, "claude"]);
    let claude_args = &spawn.args[2..];
    for absent in ["-p", "--print", "--output-format", "--permission-mode", "--permission-prompt-tool", "--dangerously-skip-permissions"] {
        assert!(!claude_args.contains(&absent.to_string()), "{absent}：{claude_args:?}");
    }
    // 工作目錄＝Workspace；帳號目錄＝選定的 Profile；清除繼承的認證相關與代理留下的環境變數
    assert_eq!(spawn.working_dir.as_deref(), Some(ws_path.as_str()));
    assert_eq!(env_of(&launch, "CLAUDE_CONFIG_DIR").as_deref(), Some(config_dir.as_str()));
    for removed in ["ANTHROPIC_API_KEY", "CLAUDECODE", "AI_AGENT", "BAGGAGE"] {
        assert!(spawn.env_remove.contains(&removed.to_string()), "{removed}：{:?}", spawn.env_remove);
    }
    assert!(!spawn.env_remove.contains(&"PATH".to_string()));
    assert_eq!(env_of(&launch, RUNTIME_URL_ENV), Some(harness.runtime_url()));
    // strict MCP 設定指向 shim；憑證只經環境變數交付，不在參數或 MCP 設定檔裡
    assert!(claude_args.contains(&"--strict-mcp-config".to_string()));
    let run_id = run["id"].as_str().unwrap();
    let mcp = harness.data_dir().join(RUNS_DIR).join(run_id).join(RUN_MCP_FILE);
    assert_eq!(value_after(claude_args, "--mcp-config"), Some(mcp.to_string_lossy().as_ref()));
    let mcp_text = std::fs::read_to_string(&mcp).unwrap();
    assert!(mcp_text.contains("baton-mcp-shim"), "{mcp_text}");
    assert!(!mcp_text.contains(&token) && !spawn.args.iter().any(|a| a.contains(&token)));

    // 宿主回報 claude 已啟動
    let (status, body) = host(&harness, &token, HOST_STARTED_PATH, json!({ "claude_pid": 4321 })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(view(&harness, &ws).await["claude_pid"], 4321);

    // 時間線記錄開啟、執行中與帳號
    let events = timeline_events(&harness, &ws).await;
    assert_eq!(event_types(&events), ["controller_cli_opened", "controller_cli_launch_started", "controller_cli_running"]);
    assert_eq!(events[0]["payload"]["profile_id"], profile.as_str());
    assert_eq!(events[0]["category"], "controller");
}

#[tokio::test]
async fn the_run_directory_gets_the_mcp_config_but_no_worker_files() {
    let harness = start().await;
    let profile = harness.add_account("帳號 1", "account1@example.com").await;
    let (ws, _) = new_workspace(&harness, "main").await;

    let (run, _, _) = open_and_launch(&harness, &ws, &profile).await;

    let run_dir = harness.data_dir().join(RUNS_DIR).join(run["id"].as_str().unwrap());
    assert!(run_dir.join(RUN_MCP_FILE).is_file(), "Controller CLI 的 --mcp-config 指向這個檔案");
    // Worker 的 Prompt 與 Worker 通則系統提示用不到（Controller CLI 的系統提示是唯讀目錄裡的操作手冊）
    let mut names: Vec<String> =
        std::fs::read_dir(&run_dir).unwrap().map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    assert_eq!(names, [RUN_MCP_FILE], "不應寫出 {PROMPT_FILE} 或 Worker 通則：{names:?}");
}

#[tokio::test]
async fn a_second_open_in_the_same_workspace_is_refused_but_a_profile_may_serve_two_workspaces() {
    let harness = start().await;
    let profile = harness.add_account("帳號 1", "account1@example.com").await;
    let (ws1, _) = new_workspace(&harness, "one").await;
    let (ws2, _) = new_workspace(&harness, "two").await;
    let (first, _, _) = open_and_launch(&harness, &ws1, &profile).await;

    let (status, refused) = open(&harness, &ws1, &profile).await;
    assert_eq!((status, refused["error"].as_str()), (StatusCode::CONFLICT, Some("controller_cli_already_open")), "{refused}");
    assert!(refused["message"].as_str().unwrap().contains("已有開啟中的 Controller CLI"));
    assert_eq!(view(&harness, &ws1).await["id"], first["id"], "原本的 Controller 不受影響");

    let (second, _, _) = open_and_launch(&harness, &ws2, &profile).await;
    assert_eq!(second["status"], "running");
    assert_eq!(second["profile_id"], profile.as_str());
    assert_eq!(harness.claude_env.controller_terminals().len(), 2);
    assert_eq!(view(&harness, &ws1).await["status"], "running");
}

#[tokio::test]
async fn the_host_reporting_the_end_ends_the_run_and_revokes_the_credential() {
    let harness = start().await;
    let profile = harness.add_account("帳號 1", "account1@example.com").await;
    let (ws, _) = new_workspace(&harness, "main").await;
    let (_, _, token) = open_and_launch(&harness, &ws, &profile).await;
    assert_eq!(controller_tool_status(&harness, &token).await, StatusCode::OK);

    let (status, body) = host(&harness, &token, HOST_ENDED_PATH, json!({ "exit_code": 0 })).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let run = view(&harness, &ws).await;
    assert_eq!((run["status"].as_str(), run["end_code"].as_str(), run["exit_code"].as_i64()), (Some("ended"), Some("host_exited"), Some(0)));
    assert_eq!(controller_tool_status(&harness, &token).await, StatusCode::UNAUTHORIZED, "Run 結束後憑證失效");
    let (status, _) = host(&harness, &token, HOST_STARTED_PATH, json!({ "claude_pid": 1 })).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let events = timeline_events(&harness, &ws).await;
    let ended = events.last().unwrap();
    assert_eq!((ended["event_type"].as_str(), ended["payload"]["profile_id"].as_str()), (Some("controller_cli_ended"), Some(profile.as_str())));

    // 結束之後可以再開
    harness.supervise().await;
    assert_eq!(open(&harness, &ws, &profile).await.0, StatusCode::ACCEPTED);
}

#[tokio::test]
async fn a_vanished_host_ends_the_run_and_revokes_the_credential() {
    let harness = start().await;
    let profile = harness.add_account("帳號 1", "account1@example.com").await;
    let (ws, _) = new_workspace(&harness, "main").await;
    let (run, launch, token) = open_and_launch(&harness, &ws, &profile).await;
    let identity = harness.process_control.identity_of(&launch.spawn.command_id).unwrap();
    assert_eq!(run["host"]["pid"], identity.pid);

    // 存活時什麼都不變
    harness.supervise().await;
    assert_eq!(view(&harness, &ws).await["status"], "running");

    // 使用者直接關掉終端機（宿主被強制結束、沒有回報）
    harness.process_control.script_liveness(&identity, Liveness::Dead);
    harness.supervise().await;

    let run = view(&harness, &ws).await;
    assert_eq!((run["status"].as_str(), run["end_code"].as_str()), (Some("ended"), Some("host_gone")), "{run}");
    assert_eq!(controller_tool_status(&harness, &token).await, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn ending_from_the_app_stops_the_host_and_revokes_the_credential() {
    let harness = start().await;
    let profile = harness.add_account("帳號 1", "account1@example.com").await;
    let (ws, _) = new_workspace(&harness, "main").await;
    let (_, launch, token) = open_and_launch(&harness, &ws, &profile).await;
    let identity = harness.process_control.identity_of(&launch.spawn.command_id).unwrap();

    let (status, body) = harness.send_json(Method::POST, &format!("{}/end", cli_path(&ws)), None).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert!(body["run"]["end_requested_at"].is_number(), "{body}");
    harness.supervise().await;

    assert_eq!(harness.process_control.stop_calls(), std::slice::from_ref(&identity), "結束宿主（連同它啟動的 claude）");
    assert!(!harness.process_control.alive().contains(&identity));
    let run = view(&harness, &ws).await;
    assert_eq!((run["status"].as_str(), run["end_code"].as_str()), (Some("ended"), Some("user_ended")), "{run}");
    assert_eq!(controller_tool_status(&harness, &token).await, StatusCode::UNAUTHORIZED);

    let (status, refused) = harness.send_json(Method::POST, &format!("{}/end", cli_path(&ws)), None).await;
    assert_eq!((status, refused["error"].as_str()), (StatusCode::CONFLICT, Some("controller_cli_not_open")));
}

/// 讓 Supervisor 卡在開窗的呼叫裡（視窗與宿主已開出、回執沒有交還），再丟棄這一次處理。
async fn supervise_until_terminal_hangs(harness: &Harness) {
    let tick = harness.supervise();
    tokio::select! {
        _ = tick => panic!("Supervisor 應該卡在開窗"),
        _ = harness.claude_env.wait_for_terminal_hang() => {}
        _ = tokio::time::sleep(Duration::from_secs(20)) => panic!("等不到開窗"),
    }
}

#[tokio::test]
async fn a_crash_between_launch_intent_and_receipt_does_not_open_a_second_window() {
    let mut harness = start().await;
    let profile = harness.add_account("帳號 1", "account1@example.com").await;
    let (ws, _) = new_workspace(&harness, "main").await;
    assert_eq!(open(&harness, &ws, &profile).await.0, StatusCode::ACCEPTED);
    harness.claude_env.hang_next_controller_terminal();
    supervise_until_terminal_hangs(&harness).await;

    harness.crash_restart().await;
    harness.supervise().await;
    harness.supervise().await;

    // 以 command_id 找回已開出的宿主，補寫回執，不再開第二個視窗
    let terminals = harness.claude_env.controller_terminals();
    assert_eq!(terminals.len(), 1);
    let identity = harness.process_control.identity_of(&terminals[0].spawn.command_id).unwrap();
    let run = view(&harness, &ws).await;
    assert_eq!((run["status"].as_str(), run["host"]["pid"].as_u64()), (Some("running"), Some(u64::from(identity.pid))), "{run}");
}

#[tokio::test]
async fn a_crash_whose_window_is_already_gone_ends_the_run_without_reopening() {
    let mut harness = start().await;
    let profile = harness.add_account("帳號 1", "account1@example.com").await;
    let (ws, _) = new_workspace(&harness, "main").await;
    assert_eq!(open(&harness, &ws, &profile).await.0, StatusCode::ACCEPTED);
    harness.claude_env.hang_next_controller_terminal();
    supervise_until_terminal_hangs(&harness).await;
    let command_id = harness.claude_env.controller_terminals()[0].spawn.command_id.clone();
    let identity = harness.process_control.identity_of(&command_id).unwrap();
    harness.process_control.script_liveness(&identity, Liveness::Dead);

    harness.crash_restart().await;
    harness.supervise().await;

    assert_eq!(harness.claude_env.controller_terminals().len(), 1, "結果不明時不重複開窗");
    let run = view(&harness, &ws).await;
    assert_eq!((run["status"].as_str(), run["end_code"].as_str()), (Some("ended"), Some("launch_outcome_unknown")), "{run}");
    assert!(run["end_reason"].as_str().unwrap().contains("重新開啟"), "{run}");
}

#[tokio::test]
async fn open_failures_show_the_reason_and_leave_no_unfinished_run() {
    let harness = start().await;
    let profile = harness.add_account("帳號 1", "account1@example.com").await;
    let (ws, _) = new_workspace(&harness, "main").await;

    // 找不到 claude：不開窗
    harness.claude_env.set_claude(ToolProbe::NotFound);
    assert_eq!(open(&harness, &ws, &profile).await.0, StatusCode::ACCEPTED);
    harness.supervise().await;
    let run = view(&harness, &ws).await;
    assert_eq!((run["status"].as_str(), run["end_code"].as_str()), (Some("ended"), Some("claude_not_found")), "{run}");
    assert!(run["end_reason"].as_str().unwrap().contains("claude"), "{run}");
    assert!(harness.claude_env.controller_terminals().is_empty());

    // 終端機開不起來
    harness.claude_env.set_claude(ToolProbe::Found { version_output: Some("2.1.307 (Claude Code)".into()) });
    harness.claude_env.fail_next_controller_terminal("找不到可用的主控台");
    assert_eq!(open(&harness, &ws, &profile).await.0, StatusCode::ACCEPTED);
    harness.supervise().await;
    let run = view(&harness, &ws).await;
    assert_eq!((run["status"].as_str(), run["end_code"].as_str()), (Some("ended"), Some("terminal_failed")), "{run}");
    assert!(run["end_reason"].as_str().unwrap().contains("找不到可用的主控台"), "{run}");
    let failed_token = env_of(&harness.claude_env.controller_terminals()[0], AGENT_TOKEN_ENV).unwrap();
    assert_eq!(controller_tool_status(&harness, &failed_token).await, StatusCode::UNAUTHORIZED);

    // 失敗不留下非終態 Run：可以再開
    let (_, _, token) = open_and_launch(&harness, &ws, &profile).await;
    assert_eq!(view(&harness, &ws).await["status"], "running");
    assert_eq!(controller_tool_status(&harness, &token).await, StatusCode::OK);
}

#[tokio::test]
async fn an_unusable_profile_cannot_open_and_a_profile_that_fails_after_the_request_ends_the_run() {
    let harness = start().await;
    let (ws, _) = new_workspace(&harness, "main").await;

    // 尚未登入的 Profile 不能選
    let (status, created) = harness.send_json(Method::POST, "/api/v1/profiles", Some(json!({ "name": "未登入" }))).await;
    assert!(status.is_success(), "{created}");
    let (status, refused) = open(&harness, &ws, created["id"].as_str().unwrap()).await;
    assert_eq!((status, refused["error"].as_str()), (StatusCode::CONFLICT, Some("profile_unavailable")), "{refused}");
    assert_eq!(view(&harness, &ws).await, Value::Null, "被拒時不建立 Run");

    // 要求開啟之後、開窗之前 Profile 被封存：不開窗，頁簽顯示原因
    let profile = harness.add_account("帳號 1", "account1@example.com").await;
    assert_eq!(open(&harness, &ws, &profile).await.0, StatusCode::ACCEPTED);
    let (status, archived) = harness.send_json(Method::POST, &format!("/api/v1/profiles/{profile}/archive"), None).await;
    assert!(status.is_success(), "{archived}");
    harness.supervise().await;

    let run = view(&harness, &ws).await;
    assert_eq!((run["status"].as_str(), run["end_code"].as_str()), (Some("ended"), Some("profile_unavailable")), "{run}");
    assert!(harness.claude_env.controller_terminals().is_empty());
}

#[tokio::test]
async fn the_controller_credential_can_propose_and_dispatch_tasks() {
    let harness = start().await;
    let profile = harness.add_account("帳號 1", "account1@example.com").await;
    let (ws, _) = new_workspace(&harness, "main").await;
    let path = format!("{WORKSPACES}/{ws}/authorization");
    let view_auth = harness.send_json(Method::GET, &path, None).await.1;
    let grants = view_auth["templates"].as_array().unwrap().iter().find(|t| t["id"] == "research_docs").unwrap()["grants"].clone();
    let body = json!({ "template_id": "research_docs", "grants": grants, "acknowledged_risk": false, "acknowledged_expansion": true });
    assert_eq!(harness.send_json(Method::PUT, &path, Some(body)).await.0, StatusCode::CREATED);
    let (_, _, token) = open_and_launch(&harness, &ws, &profile).await;

    let filled = |text: &str| json!({ "status": "filled", "text": text });
    let contract = json!({
        "objective": filled("整理競品資料"),
        "inputs": filled("公開網站"),
        "scope": filled("只看公開資料"),
        "deliverables": filled("比較表"),
        "done_criteria": filled("至少五個產品"),
        "blocker_handling": filled("缺資料就回報"),
        "required_capabilities": [],
    });
    let (status, proposed) =
        harness.call_tool(&token, "propose_task", json!({ "goal_title": "競品調查", "title": "整理競品", "contract": contract })).await;
    assert_eq!(status, StatusCode::CREATED, "{proposed}");
    let task = proposed["task"]["id"].as_str().unwrap().to_string();
    let (status, dispatched) = harness.call_tool(&token, "dispatch_task", json!({ "task_id": task })).await;
    assert!(status.is_success(), "{dispatched}");

    let board = harness.send_json(Method::GET, &format!("{WORKSPACES}/{ws}/tasks"), None).await.1;
    let on_board = board["tasks"].as_array().unwrap().iter().find(|t| t["id"] == task.as_str()).cloned().unwrap();
    assert_ne!(on_board["status"], "pending", "{on_board}");
}

// --- harness 的開啟輔助函式本身 ---

#[tokio::test]
#[should_panic(expected = "這次開啟沒有進入執行中")]
async fn the_harness_open_helper_fails_loudly_when_this_open_did_not_launch() {
    let harness = start().await;
    let profile = harness.add_account("帳號 1", "account1@example.com").await;
    let (ws, _) = new_workspace(&harness, "main").await;
    harness.open_controller_cli(&ws, &profile).await;
    harness.end_controller_cli(&ws).await;

    // 這次開窗失敗：不可默默交回上一次（或失敗那次）開窗的憑證
    harness.claude_env.fail_next_controller_terminal("找不到可用的主控台");
    harness.open_controller_cli(&ws, &profile).await;
}
