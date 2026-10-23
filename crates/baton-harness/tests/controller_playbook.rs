//! 情境測試：Controller 操作手冊與 AI 決策紀錄（desktop-controller-cli 票 04），經由 Runtime harness 的真實 HTTP API 驗證。
//!
//! Agent Adapter 是正式的 `ClaudeCodeAdapter`（啟動規格是真的組裝），Claude 環境 Port 是 fake：它記錄每次開窗的完整啟動規格。
//! 只斷言外部行為：API 回應、開窗的啟動規格、啟動規格指向的檔案內容，以及 Workspace 目錄裡的檔案。
//!
//! 每個測試名稱對應票 04 的驗收條件：
//! - 啟動規格以 `--append-system-prompt-file` 帶入 Run 專屬唯讀目錄中的操作手冊，內容與版本號一致：
//!   `the_controller_cli_loads_the_versioned_playbook_from_its_read_only_run_directory`、
//!   `with_shared_settings_the_playbook_comes_first_and_the_shared_instructions_follow`
//! - Controller 頁簽顯示操作手冊版本號：`the_controller_tab_reports_the_playbook_version_even_before_the_first_open`
//! - 沒有 `.baton/ai-decisions.md` 時建立、已存在時不覆寫：`opening_creates_the_ai_decisions_file_*`、
//!   `an_existing_ai_decisions_file_is_never_overwritten`、`when_the_ai_decisions_file_cannot_be_created_*`
//! - Worker 的系統提示包含 Worker 通則：見 `claude_adapter.rs` 的 `the_worker_*`

use std::path::{Path, PathBuf};
use std::sync::Arc;

use baton_domain::controller_cli::HOST_SUBCOMMAND;
use baton_domain::playbook::{self, AI_DECISIONS_TEMPLATE, CONTROLLER_PLAYBOOK, PLAYBOOK_VERSION};
use baton_domain::Timestamp;
use baton_harness::Harness;
use baton_ports::TerminalLaunch;
use baton_runtime::claude_adapter::ClaudeCodeAdapter;
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

const WORKSPACES: &str = "/api/v1/workspaces";
const SHIM: &str = r"C:\Baton\bin\baton-mcp-shim.exe";
const INSTRUCTIONS: &str = "一律使用繁體中文。\n";

/// 2023-11-14 22:13:20.123 UTC
const CLOCK_START: Timestamp = Timestamp::from_millis(1_700_000_000_123);

async fn start() -> Harness {
    Harness::builder()
        .clock_at(CLOCK_START)
        .agent_adapter(|data_dir| Arc::new(ClaudeCodeAdapter::new("claude".into(), SHIM.into(), data_dir, Vec::new())))
        .start()
        .await
}

async fn new_workspace(harness: &Harness, name: &str) -> (String, PathBuf) {
    let dir = harness.data_dir().join("projects").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let (status, body) =
        harness.send_json(Method::POST, WORKSPACES, Some(json!({ "path": dir.to_string_lossy() }))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    (body["id"].as_str().unwrap().to_string(), PathBuf::from(body["path"].as_str().unwrap()))
}

fn cli_path(ws: &str) -> String {
    format!("{WORKSPACES}/{ws}/controller-cli")
}

async fn view(harness: &Harness, ws: &str) -> Value {
    let (status, body) = harness.send_json(Method::GET, &cli_path(ws), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

/// 要求開啟並讓 Supervisor 處理一次，回傳最新的 Run。
async fn open(harness: &Harness, ws: &str, profile: &str) -> Value {
    let (status, body) = harness.send_json(Method::POST, &cli_path(ws), Some(json!({ "profile_id": profile }))).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    harness.supervise().await;
    view(harness, ws).await["run"].clone()
}

async fn end(harness: &Harness, ws: &str) {
    let (status, body) = harness.send_json(Method::POST, &format!("{}/end", cli_path(ws)), None).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    harness.supervise().await;
    assert_eq!(view(harness, ws).await["run"]["status"], "ended");
}

fn last_terminal(harness: &Harness) -> TerminalLaunch {
    harness.claude_env.controller_terminals().last().cloned().expect("應該開出 Controller 終端機")
}

/// 宿主之後交給 `claude` 的引數。
fn claude_args(launch: &TerminalLaunch) -> &[String] {
    assert_eq!(&launch.spawn.args[..2], [HOST_SUBCOMMAND, "claude"]);
    &launch.spawn.args[2..]
}

fn value_after<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).map(String::as_str)
}

/// 開窗的啟動規格帶的附加系統提示檔（只能有一個）。
fn system_prompt_file(launch: &TerminalLaunch) -> PathBuf {
    let args = claude_args(launch);
    assert_eq!(args.iter().filter(|a| *a == "--append-system-prompt-file").count(), 1, "{args:?}");
    PathBuf::from(value_after(args, "--append-system-prompt-file").unwrap())
}

async fn adopt_shared_settings(harness: &Harness) {
    harness.user_settings.write_settings(r#"{"theme":"dark"}"#);
    harness.user_settings.write_instructions(INSTRUCTIONS);
    let (status, preview) = harness.send_json(Method::POST, "/api/v1/shared-settings/import-preview", None).await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    let adopt = json!({ "expected_hash": preview["candidate_hash"] });
    let (status, version) = harness.send_json(Method::POST, "/api/v1/shared-settings/adopt", Some(adopt)).await;
    assert_eq!(status, StatusCode::CREATED, "{version}");
}

fn ai_decisions(workspace: &Path) -> PathBuf {
    workspace.join(".baton").join("ai-decisions.md")
}

#[tokio::test]
async fn the_controller_cli_loads_the_versioned_playbook_from_its_read_only_run_directory() {
    let harness = start().await;
    let profile = harness.add_account("帳號 1", "account1@example.com").await;
    let (ws, _) = new_workspace(&harness, "main").await;

    let run = open(&harness, &ws, &profile).await;

    assert_eq!(run["status"], "running", "{run}");
    let file = system_prompt_file(&last_terminal(&harness));
    let run_dir = harness.data_dir().join("run-settings").join(run["id"].as_str().unwrap());
    assert_eq!(file.parent(), Some(run_dir.as_path()), "操作手冊放在這個 Run 專屬的目錄：{}", file.display());
    assert!(std::fs::metadata(&file).unwrap().permissions().readonly(), "操作手冊檔是唯讀的");
    let content = std::fs::read_to_string(&file).unwrap();
    assert_eq!(content, CONTROLLER_PLAYBOOK, "沒有共同指示時就是內建的操作手冊");
    assert_eq!(content.lines().next().unwrap(), format!("# Baton Controller 操作手冊（版本 {PLAYBOOK_VERSION}）"));
    assert_eq!(view(&harness, &ws).await["playbook_version"], PLAYBOOK_VERSION, "頁簽顯示的版本與載入的內容一致");
}

#[tokio::test]
async fn with_shared_settings_the_playbook_comes_first_and_the_shared_instructions_follow() {
    let harness = start().await;
    adopt_shared_settings(&harness).await;
    let profile = harness.add_account("帳號 1", "account1@example.com").await;
    let (ws, _) = new_workspace(&harness, "main").await;

    let run = open(&harness, &ws, &profile).await;

    let launch = last_terminal(&harness);
    let file = system_prompt_file(&launch);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), playbook::system_prompt(CONTROLLER_PLAYBOOK, Some(INSTRUCTIONS)));
    // 共同設定的其他快照檔照舊傳入，放在同一個 Run 專屬目錄
    let run_dir = harness.data_dir().join("run-settings").join(run["id"].as_str().unwrap());
    let settings = run_dir.join("settings.json");
    assert_eq!(value_after(claude_args(&launch), "--settings"), Some(settings.to_string_lossy().as_ref()));
    assert_eq!(file.parent(), Some(run_dir.as_path()));
}

#[tokio::test]
async fn the_controller_tab_reports_the_playbook_version_even_before_the_first_open() {
    let harness = start().await;
    let (ws, _) = new_workspace(&harness, "main").await;

    let tab = view(&harness, &ws).await;

    assert_eq!(tab["run"], Value::Null);
    assert_eq!(tab["playbook_version"], PLAYBOOK_VERSION);
}

#[tokio::test]
async fn opening_creates_the_ai_decisions_file_from_the_template_when_the_workspace_has_none() {
    let harness = start().await;
    let profile = harness.add_account("帳號 1", "account1@example.com").await;
    let (ws, dir) = new_workspace(&harness, "main").await;
    assert!(!ai_decisions(&dir).exists());

    let run = open(&harness, &ws, &profile).await;

    assert_eq!(run["status"], "running", "{run}");
    assert_eq!(std::fs::read_to_string(ai_decisions(&dir)).unwrap(), AI_DECISIONS_TEMPLATE);
}

#[tokio::test]
async fn an_existing_ai_decisions_file_is_never_overwritten() {
    let harness = start().await;
    let profile = harness.add_account("帳號 1", "account1@example.com").await;
    let (ws, dir) = new_workspace(&harness, "main").await;
    std::fs::create_dir_all(dir.join(".baton")).unwrap();
    let existing = "# AI 決策紀錄\n\n### AI-01：既有的決定\n- 審閱：未審\n";
    std::fs::write(ai_decisions(&dir), existing).unwrap();

    assert_eq!(open(&harness, &ws, &profile).await["status"], "running");
    end(&harness, &ws).await;
    assert_eq!(open(&harness, &ws, &profile).await["status"], "running");

    assert_eq!(std::fs::read_to_string(ai_decisions(&dir)).unwrap(), existing);
}

#[tokio::test]
async fn when_the_ai_decisions_file_cannot_be_created_the_controller_does_not_open_and_says_why() {
    let harness = start().await;
    let profile = harness.add_account("帳號 1", "account1@example.com").await;
    let (ws, dir) = new_workspace(&harness, "main").await;
    // `.baton` 是一個檔案，不能在它底下建立紀錄檔
    std::fs::write(dir.join(".baton"), "不是目錄").unwrap();

    let run = open(&harness, &ws, &profile).await;

    assert_eq!((run["status"].as_str(), run["end_code"].as_str()), (Some("ended"), Some("launch_failed")), "{run}");
    assert!(run["end_reason"].as_str().unwrap().contains("AI 決策紀錄"), "{run}");
    assert!(harness.claude_env.controller_terminals().is_empty(), "沒有建立紀錄檔就不開窗");
}
