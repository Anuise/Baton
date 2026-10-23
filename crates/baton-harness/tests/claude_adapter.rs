//! 情境測試：真實 Claude Code Adapter 接在 Runtime 上（票 14），經由 Runtime harness 的真實 HTTP API 驗證。
//!
//! Agent Adapter 換成正式的 `ClaudeCodeAdapter`（啟動參數、Run 檔案、stream-json 解析都是真的），
//! 只有 ProcessControl 仍是 fake：它不真的啟動 `claude`，測試改為直接寫入 Run 的 `stream.jsonl`
//! 扮演 CLI 的輸出。所以這裡驗證的是「Runtime 交給 CLI 的東西」與「Runtime 如何理解 CLI 的輸出」。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use baton_harness::Harness;
use baton_ports::{
    AdapterEvent, AgentAdapter, AgentHandle, Capability, PortResult, RunSpec, SessionRef, SpawnSpec, ToolProbe,
};
use baton_runtime::claude_adapter::{ClaudeCodeAdapter, PROMPT_FILE, RUNS_DIR, RUN_MCP_FILE, STREAM_FILE};
use futures::stream::BoxStream;
use futures::StreamExt;
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};
use tokio::sync::Notify;

const WORKSPACES: &str = "/api/v1/workspaces";
const SHIM: &str = "C:\\Baton\\bin\\baton-mcp-shim.exe";

/// Runtime「繼承」到的環境變數名稱：含會改變認證來源的變數。
const INHERITED: [&str; 4] = ["PATH", "ANTHROPIC_API_KEY", "CLAUDE_CODE_OAUTH_TOKEN", "CLAUDECODE"];

fn session_init(session: &str) -> String {
    json!({ "type": "system", "subtype": "init", "session_id": session, "claude_code_version": "2.1.307" }).to_string()
}

fn result_line(is_error: bool, text: &str) -> String {
    json!({ "type": "result", "subtype": "success", "is_error": is_error, "result": text, "session_id": "s" }).to_string()
}

/// 等 Adapter 事件送達的上限（真實時間；正常情況下幾毫秒內就到）。
const EVENT_WAIT: Duration = Duration::from_secs(10);

/// 正式 Adapter 外包一層：事件原樣轉交給 Runtime，每轉交一個就通知測試。
/// 測試據此等「下一個事件送達」再讓 Supervisor 處理，而不是睡固定毫秒數等背景的檔案追蹤。
struct Observed {
    inner: ClaudeCodeAdapter,
    delivered: Arc<Notify>,
}

#[async_trait]
impl AgentAdapter for Observed {
    fn spawn_spec(&self, spec: &RunSpec) -> SpawnSpec {
        self.inner.spawn_spec(spec)
    }

    fn controller_spawn_spec(&self, spec: &RunSpec) -> SpawnSpec {
        self.inner.controller_spawn_spec(spec)
    }

    async fn prepare(&self, spec: &RunSpec) -> PortResult<()> {
        self.inner.prepare(spec).await
    }

    async fn launch(&self, spec: RunSpec) -> PortResult<AgentHandle> {
        self.inner.launch(spec).await
    }

    fn events(&self, handle: &AgentHandle) -> BoxStream<'static, AdapterEvent> {
        let mut inner = self.inner.events(handle);
        let delivered = self.delivered.clone();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    // Runtime 丟棄串流（例如重建）時跟著結束，正式 Adapter 的追蹤也隨之停止
                    _ = tx.closed() => return,
                    next = inner.next() => {
                        let Some(event) = next else { return };
                        if tx.send(event).is_err() {
                            return;
                        }
                        // 保留一次通知：事件比測試開始等待更早送達也不會漏
                        delivered.notify_one();
                    }
                }
            }
        });
        Box::pin(futures::stream::poll_fn(move |cx| rx.poll_recv(cx)))
    }

    async fn resume_capability(&self, session: &SessionRef) -> PortResult<Capability> {
        self.inner.resume_capability(session).await
    }
}

async fn harness() -> Harness {
    observed_harness().await.0
}

/// 與 `harness` 相同，另外回傳「Adapter 送出事件」的通知。
async fn observed_harness() -> (Harness, Arc<Notify>) {
    let delivered = Arc::new(Notify::new());
    let observed = delivered.clone();
    let harness = Harness::builder()
        .agent_adapter(move |data_dir| {
            let inherited = INHERITED.iter().map(|s| s.to_string()).collect();
            let adapter = ClaudeCodeAdapter::new("claude".into(), SHIM.into(), data_dir, inherited)
                .with_poll(Duration::from_millis(5));
            Arc::new(Observed { inner: adapter, delivered: observed })
        })
        .start()
        .await;
    (harness, delivered)
}

fn filled(text: &str) -> Value {
    json!({ "status": "filled", "text": text })
}

/// 建立 Workspace 與 Goal，回傳 (workspace_id, goal_id)。
async fn workspace(harness: &Harness) -> (String, String) {
    let dir = harness.data_dir().join("projects").join("main");
    std::fs::create_dir_all(&dir).unwrap();
    let (status, ws) = harness.send_json(Method::POST, WORKSPACES, Some(json!({ "path": dir }))).await;
    assert_eq!(status, StatusCode::CREATED, "{ws}");
    let ws = ws["id"].as_str().unwrap().to_string();
    let (status, goal) =
        harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/goals"), Some(json!({ "title": "目標" }))).await;
    assert_eq!(status, StatusCode::CREATED, "{goal}");
    (ws, goal["id"].as_str().unwrap().to_string())
}

/// 建立 ready 的 Task 並派工，回傳派工回應（201 是 Run，202 是排隊）。
async fn dispatch_new_task(harness: &Harness, ws: &str, goal: &str, title: &str) -> (StatusCode, Value) {
    let contract = json!({
        "objective": filled(title), "inputs": filled("既有的資料"), "scope": filled("公開資料"),
        "deliverables": filled("比較表"), "done_criteria": filled("五個產品"), "blocker_handling": filled("卡住時回報"),
        "required_capabilities": [],
    });
    let body = json!({ "goal_id": goal, "title": title, "contract": contract });
    let (status, task) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks"), Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    let task_path = format!("{WORKSPACES}/{ws}/tasks/{}", task["id"].as_str().unwrap());
    let (status, ready) = harness.send_json(Method::POST, &format!("{task_path}/ready"), None).await;
    assert_eq!(status, StatusCode::OK, "{ready}");
    harness.send_json(Method::POST, &format!("{task_path}/dispatch"), None).await
}

async fn dispatch_ok(harness: &Harness, ws: &str, goal: &str, title: &str) -> String {
    let (status, run) = dispatch_new_task(harness, ws, goal, title).await;
    assert_eq!(status, StatusCode::CREATED, "{run}");
    run["id"].as_str().unwrap().to_string()
}

async fn run_view(harness: &Harness, ws: &str, run: &str) -> Value {
    let (status, view) = harness.send_json(Method::GET, &format!("{WORKSPACES}/{ws}/runs/{run}"), None).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    view
}

async fn adopt_shared_settings(harness: &Harness) -> Value {
    harness.user_settings.write_settings(r#"{"theme":"dark"}"#);
    harness.user_settings.write_instructions("一律使用繁體中文。\n");
    let (status, preview) = harness.send_json(Method::POST, "/api/v1/shared-settings/import-preview", None).await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    let adopt = json!({ "expected_hash": preview["candidate_hash"] });
    let (status, version) = harness.send_json(Method::POST, "/api/v1/shared-settings/adopt", Some(adopt)).await;
    assert_eq!(status, StatusCode::CREATED, "{version}");
    version
}

fn run_dir(harness: &Harness, run: &str) -> PathBuf {
    harness.data_dir().join(RUNS_DIR).join(run)
}

/// 扮演 CLI：在 Run 的 stream 檔案尾端寫入幾行輸出。
fn emit(harness: &Harness, run: &str, lines: &[String]) {
    use std::io::Write;
    let path = run_dir(harness, run).join(STREAM_FILE);
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path).unwrap();
    for line in lines {
        writeln!(file, "{line}").unwrap();
    }
}

/// 讓 Supervisor 反覆處理，直到 Run 符合條件。Adapter 在背景追蹤檔案，事件稍後才會到：
/// 條件不成立時等下一個事件送達（`delivered`）再處理，不睡固定時間；最多等 `EVENT_WAIT`。
async fn supervise_until(
    harness: &Harness,
    delivered: &Notify,
    ws: &str,
    run: &str,
    done: impl Fn(&Value) -> bool,
) -> Value {
    let reached = tokio::time::timeout(EVENT_WAIT, async {
        loop {
            harness.supervise().await;
            let view = run_view(harness, ws, run).await;
            if done(&view) {
                return view;
            }
            delivered.notified().await;
        }
    })
    .await;
    match reached {
        Ok(view) => view,
        Err(_) => panic!("Run {run} 一直沒有達到預期狀態：{}", run_view(harness, ws, run).await),
    }
}

/// 同 `supervise_until`，但等的是 Run 記下某種事件。
async fn supervise_until_event(harness: &Harness, delivered: &Notify, run: &str, event_type: &str) {
    let found = tokio::time::timeout(EVENT_WAIT, async {
        loop {
            harness.supervise().await;
            if event_types(harness, run).iter().any(|t| t == event_type) {
                return;
            }
            delivered.notified().await;
        }
    })
    .await;
    assert!(found.is_ok(), "Run {run} 一直沒有記下 {event_type}：{:?}", event_types(harness, run));
}

/// 某個 Run 的啟動指令（依 stdout 檔案所在的 Run 目錄找出）。
fn spawned_for(harness: &Harness, run: &str) -> SpawnSpec {
    let dir = run_dir(harness, run).to_string_lossy().into_owned();
    harness
        .process_control
        .spawned()
        .into_iter()
        .find(|spec| spec.stdout_file.as_deref().is_some_and(|f| f.starts_with(&dir)))
        .unwrap_or_else(|| panic!("Run {run} 沒有被啟動"))
}

fn value_after<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).map(String::as_str)
}

fn event_types(harness: &Harness, run: &str) -> Vec<String> {
    let db = harness.db();
    let mut statement = db.prepare("SELECT event_type FROM event WHERE aggregate_id = ?1 ORDER BY rowid").unwrap();
    statement.query_map([run], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
}

fn profile_config_dir(profile: &Value) -> String {
    profile["config_dir"].as_str().unwrap().to_string()
}

async fn profile(harness: &Harness, id: &str) -> Value {
    let (_, list) = harness.send_json(Method::GET, "/api/v1/profiles", None).await;
    list["profiles"].as_array().unwrap().iter().find(|p| p["id"] == id).cloned().unwrap()
}

// ---------------------------------------------------------------- 啟動參數與環境

#[tokio::test]
async fn the_worker_starts_with_the_profiles_config_dir_the_runs_settings_snapshot_and_the_worker_model() {
    let harness = harness().await;
    adopt_shared_settings(&harness).await;
    let profile_id = harness.add_account("主要帳號", "main@example.com").await;
    let (ws, goal) = workspace(&harness).await;
    let overrides = json!({ "controller": "sonnet", "worker": "claude-opus-5-5", "reviewer": null });
    let (status, updated) =
        harness.send_json(Method::PUT, &format!("{WORKSPACES}/{ws}/model-overrides"), Some(overrides)).await;
    assert_eq!((status, updated["model_overrides"]["worker"].as_str()), (StatusCode::OK, Some("claude-opus-5-5")));
    let run = dispatch_ok(&harness, &ws, &goal, "工作").await;

    harness.supervise().await;

    let spec = spawned_for(&harness, &run);
    assert_eq!(spec.program, "claude");
    assert_eq!(&spec.args[..4], ["-p", "--output-format", "stream-json", "--verbose"]);
    assert_eq!(value_after(&spec.args, "--model"), Some("claude-opus-5-5"), "Worker Run 用 Worker 角色的模型");
    let snapshot = harness.data_dir().join("run-settings").join(&run);
    let in_snapshot = |name: &str| snapshot.join(name).to_string_lossy().into_owned();
    assert_eq!(value_after(&spec.args, "--settings"), Some(in_snapshot("settings.json").as_str()));
    assert_eq!(value_after(&spec.args, "--append-system-prompt-file"), Some(in_snapshot("CLAUDE.md").as_str()));
    assert_eq!(value_after(&spec.args, "--setting-sources"), Some(""), "不載入使用者層與 Workspace 內的設定檔");
    let config_dir = profile_config_dir(&profile(&harness, &profile_id).await);
    assert!(spec.env.contains(&("CLAUDE_CONFIG_DIR".to_string(), config_dir)), "{:?}", spec.env);
    for removed in ["ANTHROPIC_API_KEY", "CLAUDE_CODE_OAUTH_TOKEN", "CLAUDECODE"] {
        assert!(spec.env_remove.contains(&removed.to_string()), "{removed} 應該被移除：{:?}", spec.env_remove);
    }
    assert!(!spec.env_remove.contains(&"PATH".to_string()));
    // 快照已綁定到這個 Run
    let (status, pinned) =
        harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/runs/{run}/settings-snapshot"), Some(json!({}))).await;
    assert_eq!((status, pinned["directory"].as_str()), (StatusCode::OK, Some(snapshot.to_string_lossy().as_ref())));
}

#[tokio::test]
async fn no_credential_value_appears_in_the_launch_arguments_or_the_runs_files() {
    let harness = harness().await;
    adopt_shared_settings(&harness).await;
    let profile_id = harness.add_account("主要帳號", "main@example.com").await;
    let (ws, goal) = workspace(&harness).await;
    let run = dispatch_ok(&harness, &ws, &goal, "工作").await;

    harness.supervise().await;

    let spec = spawned_for(&harness, &run);
    let token = spec.env.iter().find(|(name, _)| name == "BATON_AGENT_TOKEN").map(|(_, v)| v.clone()).unwrap();
    let login_file = Path::new(&profile_config_dir(&profile(&harness, &profile_id).await)).join(".credentials.json");
    let login: Value = serde_json::from_str(&std::fs::read_to_string(login_file).unwrap()).unwrap();
    let login_secret = login["claudeAiOauth"]["accessToken"].as_str().unwrap().to_string();
    let secrets = [token.as_str(), login_secret.as_str()];

    let mut exposed = vec![format!("{} {}", spec.program, spec.args.join(" "))];
    for name in [PROMPT_FILE, RUN_MCP_FILE] {
        exposed.push(std::fs::read_to_string(run_dir(&harness, &run).join(name)).unwrap());
    }
    for text in &exposed {
        for secret in secrets {
            assert!(!text.contains(secret), "憑證出現在啟動參數或 Run 檔案中：{text}");
        }
    }
    let mcp: Value = serde_json::from_str(&exposed[2]).unwrap();
    assert_eq!(mcp["mcpServers"]["baton"]["env"]["BATON_AGENT_TOKEN"], "${BATON_AGENT_TOKEN}", "只以環境變數參照");
    assert_eq!(mcp["mcpServers"]["baton"]["command"], SHIM);
    assert!(exposed[1].contains("工作"), "Prompt 經標準輸入檔案交付");
}

#[tokio::test]
async fn the_worker_model_is_fixed_at_dispatch_so_later_overrides_only_affect_later_runs() {
    let harness = harness().await;
    harness.add_account("帳號一", "one@example.com").await;
    harness.add_account("帳號二", "two@example.com").await;
    let (ws, goal) = workspace(&harness).await;
    let before = dispatch_ok(&harness, &ws, &goal, "之前").await;
    let overrides = json!({ "worker": "haiku" });
    let (status, _) = harness.send_json(Method::PUT, &format!("{WORKSPACES}/{ws}/model-overrides"), Some(overrides)).await;
    assert_eq!(status, StatusCode::OK);
    let after = dispatch_ok(&harness, &ws, &goal, "之後").await;

    harness.supervise().await;

    assert_eq!(value_after(&spawned_for(&harness, &before).args, "--model"), None, "沒有覆寫就不指定模型");
    assert_eq!(value_after(&spawned_for(&harness, &after).args, "--model"), Some("haiku"));
}

// ---------------------------------------------------------------- 原生 Session、結果與錯誤

#[tokio::test]
async fn the_first_native_session_is_bound_and_a_different_one_never_replaces_it() {
    let (harness, delivered) = observed_harness().await;
    harness.add_account("主要帳號", "main@example.com").await;
    let (ws, goal) = workspace(&harness).await;
    let run = dispatch_ok(&harness, &ws, &goal, "工作").await;
    harness.supervise().await;

    emit(&harness, &run, &[session_init("sess-a")]);
    let view = supervise_until(&harness, &delivered, &ws, &run, |v| v["native_session_id"].is_string()).await;
    assert_eq!(view["native_session_id"], "sess-a");

    emit(&harness, &run, &[session_init("sess-b")]);
    supervise_until_event(&harness, &delivered, &run, "run_native_session_conflict").await;
    assert_eq!(run_view(&harness, &ws, &run).await["native_session_id"], "sess-a", "不靜默改指其他原生 Session");
    let types = event_types(&harness, &run);
    assert_eq!(types.iter().filter(|t| *t == "run_native_session_bound").count(), 1, "{types:?}");
    assert!(types.iter().any(|t| t == "run_native_session_conflict"), "衝突留下紀錄：{types:?}");

    emit(&harness, &run, &[result_line(false, "完成")]);
    let view = supervise_until(&harness, &delivered, &ws, &run, |v| v["status"] != "running").await;
    assert_eq!((view["status"].as_str(), view["native_session_id"].as_str()), (Some("succeeded"), Some("sess-a")));
}

#[tokio::test]
async fn an_error_result_fails_the_run_with_the_reason_reported_by_the_cli() {
    let (harness, delivered) = observed_harness().await;
    harness.add_account("主要帳號", "main@example.com").await;
    let (ws, goal) = workspace(&harness).await;
    let run = dispatch_ok(&harness, &ws, &goal, "工作").await;
    harness.supervise().await;

    emit(&harness, &run, &[session_init("sess-a"), result_line(true, "Not logged in · Please run /login")]);
    let view = supervise_until(&harness, &delivered, &ws, &run, |v| v["status"] != "running").await;

    assert_eq!(view["status"], "failed");
    assert!(view["end_reason"].as_str().unwrap().contains("Not logged in"), "{view}");
}

#[tokio::test]
async fn after_a_runtime_restart_the_adapter_reattaches_to_the_same_run_without_a_second_launch() {
    let (mut harness, delivered) = observed_harness().await;
    adopt_shared_settings(&harness).await;
    harness.add_account("主要帳號", "main@example.com").await;
    let (ws, goal) = workspace(&harness).await;
    let run = dispatch_ok(&harness, &ws, &goal, "工作").await;
    harness.supervise().await;
    let first = spawned_for(&harness, &run);
    emit(&harness, &run, &[session_init("sess-a")]);
    supervise_until(&harness, &delivered, &ws, &run, |v| v["native_session_id"].is_string()).await;

    harness.restart().await;
    emit(&harness, &run, &[result_line(false, "完成")]);
    let view = supervise_until(&harness, &delivered, &ws, &run, |v| v["status"] != "running").await;

    assert_eq!(view["status"], "succeeded");
    assert_eq!(harness.process_control.spawn_count(), 1, "重建後以同一個啟動指令找回程序，不重新啟動");
    assert_eq!(view["native_session_id"], "sess-a");
    let types = event_types(&harness, &run);
    assert!(!types.iter().any(|t| t == "run_native_session_conflict"), "重播同一個 Session 不是衝突：{types:?}");
    assert_eq!(first.args, spawned_for(&harness, &run).args);
}

// ---------------------------------------------------------------- 版本阻擋

#[tokio::test]
async fn an_unsupported_claude_code_version_blocks_the_profiles_dispatch_and_says_why() {
    let harness = harness().await;
    harness.claude_env.set_claude(ToolProbe::Found { version_output: Some("2.0.5 (Claude Code)".into()) });
    let (status, created) =
        harness.send_json(Method::POST, "/api/v1/profiles", Some(json!({ "name": "舊版帳號" }))).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["id"].as_str().unwrap().to_string();
    harness.claude_env.log_in(Path::new(&profile_config_dir(&created)), "main@example.com");
    let (_, verified) = harness.send_json(Method::POST, &format!("/api/v1/profiles/{id}/verify"), None).await;

    assert_eq!(verified["dispatchable"], false, "{verified}");
    assert_eq!(verified["verification"]["cli_version"], "2.0.5 (Claude Code)");
    let blocker = &verified["blockers"][0];
    assert_eq!(blocker["code"], "cli_version_unsupported");
    assert!(blocker["message"].as_str().unwrap().contains("2.0.5"), "{blocker}");

    let (ws, goal) = workspace(&harness).await;
    let (status, queued) = dispatch_new_task(&harness, &ws, &goal, "工作").await;
    assert_eq!(status, StatusCode::ACCEPTED, "{queued}");
    let codes: Vec<&str> = queued["reasons"].as_array().unwrap().iter().map(|r| r["code"].as_str().unwrap()).collect();
    assert_eq!(codes, ["cli_version_unsupported"], "{queued}");
    harness.supervise().await;
    assert_eq!(harness.process_control.spawn_count(), 0, "版本不符時不啟動 Worker");

    // 更新 Claude Code 後重新核對，排隊中的 Task 就派出去
    harness.claude_env.set_claude(ToolProbe::Found { version_output: Some("2.1.307 (Claude Code)".into()) });
    let (_, verified) = harness.send_json(Method::POST, &format!("/api/v1/profiles/{id}/verify"), None).await;
    assert_eq!(verified["dispatchable"], true, "{verified}");
    harness.supervise().await;
    assert_eq!(harness.process_control.spawn_count(), 1);
}

#[tokio::test]
async fn a_missing_claude_code_blocks_dispatch_as_an_unknown_version() {
    let harness = harness().await;
    harness.claude_env.set_claude(ToolProbe::NotFound);
    let (_, created) = harness.send_json(Method::POST, "/api/v1/profiles", Some(json!({ "name": "帳號" }))).await;
    let id = created["id"].as_str().unwrap().to_string();
    harness.claude_env.log_in(Path::new(&profile_config_dir(&created)), "main@example.com");

    let (_, verified) = harness.send_json(Method::POST, &format!("/api/v1/profiles/{id}/verify"), None).await;

    assert_eq!(verified["dispatchable"], false);
    assert_eq!(verified["blockers"][0]["code"], "cli_version_unknown", "{verified}");
}

// ---------------------------------------------------------------- 設定快照的核對與序列化

fn make_writable(path: &Path) {
    let mut permissions = std::fs::metadata(path).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    permissions.set_readonly(false);
    std::fs::set_permissions(path, permissions).unwrap();
}

#[tokio::test]
async fn a_tampered_settings_snapshot_blocks_the_launch_instead_of_running_with_unknown_settings() {
    let harness = harness().await;
    adopt_shared_settings(&harness).await;
    harness.add_account("主要帳號", "main@example.com").await;
    let (ws, goal) = workspace(&harness).await;
    let run = dispatch_ok(&harness, &ws, &goal, "工作").await;
    let snapshot_path = format!("{WORKSPACES}/{ws}/runs/{run}/settings-snapshot");
    let (status, snapshot) = harness.send_json(Method::POST, &snapshot_path, Some(json!({}))).await;
    assert_eq!(status, StatusCode::CREATED, "{snapshot}");
    let settings = Path::new(snapshot["directory"].as_str().unwrap()).join("settings.json");
    make_writable(&settings);
    std::fs::write(&settings, r#"{"permissions":{"allow":["Bash(*)"]}}"#).unwrap();

    harness.supervise().await;

    let view = run_view(&harness, &ws, &run).await;
    assert_eq!(view["status"], "failed", "{view}");
    assert!(view["end_reason"].as_str().unwrap().contains("settings_snapshot_mismatch"), "{view}");
    assert_eq!(harness.process_control.spawn_count(), 0, "核對不過就不啟動");
    let (status, rejected) = harness.send_json(Method::POST, &snapshot_path, Some(json!({}))).await;
    assert_eq!((status, rejected["error"].as_str()), (StatusCode::CONFLICT, Some("settings_snapshot_mismatch")));
}

#[tokio::test]
async fn concurrent_snapshot_requests_for_one_run_with_different_versions_are_serialized() {
    let harness = harness().await;
    adopt_shared_settings(&harness).await;
    harness.user_settings.write_settings(r#"{"theme":"light"}"#);
    let (_, preview) = harness.send_json(Method::POST, "/api/v1/shared-settings/import-preview", None).await;
    let adopt = json!({ "expected_hash": preview["candidate_hash"] });
    let (status, _) = harness.send_json(Method::POST, "/api/v1/shared-settings/adopt", Some(adopt)).await;
    assert_eq!(status, StatusCode::CREATED);
    harness.add_account("主要帳號", "main@example.com").await;
    let (ws, goal) = workspace(&harness).await;
    let run = dispatch_ok(&harness, &ws, &goal, "工作").await;
    let path = format!("{WORKSPACES}/{ws}/runs/{run}/settings-snapshot");

    let (first, second) = tokio::join!(
        harness.send_json(Method::POST, &path, Some(json!({ "version": 1 }))),
        harness.send_json(Method::POST, &path, Some(json!({ "version": 2 }))),
    );

    let mut statuses = [first.0, second.0];
    statuses.sort();
    assert_eq!(statuses, [StatusCode::CREATED, StatusCode::CONFLICT], "{first:?} {second:?}");
    let pinned = if first.0 == StatusCode::CREATED { &first.1 } else { &second.1 };
    // 目錄內容就是綁定的那個版本：再要求一次會通過核對
    let (status, again) = harness.send_json(Method::POST, &path, Some(json!({}))).await;
    assert_eq!((status, again["version"].clone()), (StatusCode::OK, pinned["version"].clone()));
    harness.supervise().await;
    assert_eq!(harness.process_control.spawn_count(), 1);
}
