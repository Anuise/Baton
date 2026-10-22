//! Adapter 契約測試（spec D-04 第 2 層）：同一份契約同時跑在 fake 與真實 Claude Code Adapter 上，
//! 確保 Runtime harness 用的 fake 行為與真實 CLI 一致。
//!
//! 三種受測對象：
//! - **fake**：`FakeAgentAdapter`，腳本化成真實 CLI 在各情境下的事件順序。每次 `cargo test` 都執行。
//! - **replay**：真實 `ClaudeCodeAdapter`，餵入真實 CLI 錄下的 stream-json（`tests/fixtures/`）。
//!   不需要 CLI，每次 `cargo test` 都執行，確保解析器與錄下的真實輸出一致。
//! - **live**：真實 `ClaudeCodeAdapter` + Windows Job Object ProcessControl + 已登入的 Claude Code。
//!   只在已登入的 Windows 環境手動或夜間執行（`#[ignore]`），執行方式與結果記錄見 `docs/adapter-contract.md`。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use baton_harness::fakes::FakeAgentAdapter;
use baton_ports::{AdapterEvent, AgentAdapter, RunSpec};
use baton_runtime::claude_adapter::{ClaudeCodeAdapter, STREAM_FILE};
use futures::StreamExt;

/// 契約情境：要 Worker 做什麼。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scenario {
    /// 回覆一句話就結束。
    Completes,
    /// 指定不存在的模型：CLI 回報錯誤並以失敗結束。
    InvalidModel,
}

impl Scenario {
    fn fixture(self) -> &'static str {
        match self {
            Scenario::Completes => "claude-2.1.307-completes.jsonl",
            Scenario::InvalidModel => "claude-2.1.307-invalid-model.jsonl",
        }
    }
}

const PROMPT: &str = "請只回覆 OK 兩個字，不要使用任何工具。";
const INVALID_MODEL: &str = "baton-contract-no-such-model";
/// 一個情境最多等多久（真實 CLI 需要連線）。
const SCENARIO_TIMEOUT: Duration = Duration::from_secs(300);

fn contract_spec(run_id: &str, scenario: Scenario, model: Option<String>, config_dir: Option<String>, working_dir: &Path) -> RunSpec {
    RunSpec {
        run_id: run_id.to_string(),
        task_id: "task_contract".to_string(),
        command_id: format!("cmd_{run_id}"),
        prompt: PROMPT.to_string(),
        working_dir: Some(working_dir.to_string_lossy().into_owned()),
        config_dir,
        settings_dir: None,
        model: match scenario {
            Scenario::Completes => model,
            Scenario::InvalidModel => Some(INVALID_MODEL.to_string()),
        },
    }
}

#[async_trait]
trait Subject: Send + Sync {
    fn adapter(&self) -> Arc<dyn AgentAdapter>;
    /// 讓 Worker 依情境執行到結束，回傳 Adapter 交出的全部事件。
    async fn run(&self, scenario: Scenario, spec: RunSpec) -> Vec<AdapterEvent>;
}

async fn collect(adapter: &dyn AgentAdapter, spec: RunSpec) -> Vec<AdapterEvent> {
    let handle = adapter.launch(spec).await.expect("launch 失敗");
    tokio::time::timeout(SCENARIO_TIMEOUT, adapter.events(&handle).collect::<Vec<_>>())
        .await
        .expect("情境在時限內沒有結束")
}

// ---------------------------------------------------------------- 契約條款

/// 不含 heartbeat 的事件（heartbeat 的次數取決於輸出的時機，不屬於契約）。
fn meaningful(events: &[AdapterEvent]) -> Vec<&AdapterEvent> {
    events.iter().filter(|e| **e != AdapterEvent::Heartbeat).collect()
}

/// 同一個 Run 回報的原生 Session 只有一個，而且在任何結果之前就回報。
fn assert_single_native_session_first(events: &[AdapterEvent]) -> String {
    let meaningful = meaningful(events);
    let Some(AdapterEvent::Started { native_session_id: Some(first) }) = meaningful.first() else {
        panic!("第一個事件必須是帶原生 Session 識別的 Started：{events:?}");
    };
    for event in &meaningful {
        if let AdapterEvent::Started { native_session_id } = event {
            assert_eq!(native_session_id.as_deref(), Some(first.as_str()), "同一個 Run 不能回報第二個原生 Session：{events:?}");
        }
    }
    first.clone()
}

/// 事件以恰好一個 `Exited` 結尾。
fn assert_ends_with_one_exit(events: &[AdapterEvent], success: bool) {
    let exits: Vec<_> = events.iter().filter(|e| matches!(e, AdapterEvent::Exited { .. })).collect();
    assert_eq!(exits, [&AdapterEvent::Exited { success }], "{events:?}");
    assert_eq!(events.last(), Some(&AdapterEvent::Exited { success }), "Exited 之後不能再有事件：{events:?}");
}

async fn contract_completes(subject: &dyn Subject, spec: RunSpec) {
    let events = subject.run(Scenario::Completes, spec).await;

    assert_single_native_session_first(&events);
    assert_ends_with_one_exit(&events, true);
    assert!(!events.iter().any(|e| matches!(e, AdapterEvent::Error { .. })), "正常完成不應有錯誤：{events:?}");
    assert!(
        events.iter().any(|e| matches!(e, AdapterEvent::Message { text } if text.contains("OK"))),
        "Worker 的回覆以 Message 交出：{events:?}"
    );
}

async fn contract_invalid_model(subject: &dyn Subject, spec: RunSpec) {
    let events = subject.run(Scenario::InvalidModel, spec).await;

    assert_single_native_session_first(&events);
    assert_ends_with_one_exit(&events, false);
    let error_at = events.iter().position(|e| matches!(e, AdapterEvent::Error { message } if !message.trim().is_empty()));
    let exit_at = events.iter().position(|e| matches!(e, AdapterEvent::Exited { .. }));
    assert!(error_at.is_some() && error_at < exit_at, "失敗前先交出非空的錯誤原因：{events:?}");
}

/// 啟動指令只由 RunSpec 決定（Runtime 重建後據此找回程序），而且 Prompt 不放在指令列上。
fn contract_spawn_spec(subject: &dyn Subject, spec: &RunSpec) {
    let adapter = subject.adapter();
    let first = adapter.spawn_spec(spec);

    assert_eq!(first, adapter.spawn_spec(spec), "同一個 RunSpec 必須得到同一個啟動指令");
    assert_eq!(first.command_id, spec.command_id);
    assert!(!first.args.iter().any(|a| a.contains(PROMPT)), "Prompt 不放在指令列：{:?}", first.args);
}

// ---------------------------------------------------------------- fake

/// fake：依真實 CLI 的行為腳本化（見 replay 的錄製檔）。
struct FakeSubject {
    adapter: Arc<FakeAgentAdapter>,
}

#[async_trait]
impl Subject for FakeSubject {
    fn adapter(&self) -> Arc<dyn AgentAdapter> {
        self.adapter.clone()
    }

    async fn run(&self, scenario: Scenario, spec: RunSpec) -> Vec<AdapterEvent> {
        let started = AdapterEvent::Started { native_session_id: Some(format!("fake-session-{}", spec.run_id)) };
        let script = match scenario {
            Scenario::Completes => vec![
                started,
                AdapterEvent::Heartbeat,
                AdapterEvent::Message { text: "OK".into() },
                AdapterEvent::Exited { success: true },
            ],
            Scenario::InvalidModel => vec![
                started,
                AdapterEvent::Message { text: format!("There's an issue with the selected model ({INVALID_MODEL}).") },
                AdapterEvent::Error { message: format!("There's an issue with the selected model ({INVALID_MODEL}).") },
                AdapterEvent::Exited { success: false },
            ],
        };
        self.adapter.script_next_run(script);
        collect(&*self.adapter, spec).await
    }
}

fn fake() -> FakeSubject {
    FakeSubject { adapter: Arc::new(FakeAgentAdapter::default()) }
}

#[tokio::test]
async fn fake_adapter_completes() {
    let dir = tempfile::tempdir().unwrap();
    contract_completes(&fake(), contract_spec("run_fake_ok", Scenario::Completes, None, None, dir.path())).await;
}

#[tokio::test]
async fn fake_adapter_reports_an_invalid_model() {
    let dir = tempfile::tempdir().unwrap();
    contract_invalid_model(&fake(), contract_spec("run_fake_bad", Scenario::InvalidModel, None, None, dir.path())).await;
}

#[test]
fn fake_adapter_spawn_spec() {
    let dir = tempfile::tempdir().unwrap();
    contract_spawn_spec(&fake(), &contract_spec("run_fake_spec", Scenario::Completes, None, None, dir.path()));
}

// ---------------------------------------------------------------- replay

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures").join(name)
}

/// replay：真實 Adapter 讀取真實 CLI 錄下的輸出（扮演一個已經跑完的程序）。
struct ReplaySubject {
    adapter: Arc<ClaudeCodeAdapter>,
}

#[async_trait]
impl Subject for ReplaySubject {
    fn adapter(&self) -> Arc<dyn AgentAdapter> {
        self.adapter.clone()
    }

    async fn run(&self, scenario: Scenario, spec: RunSpec) -> Vec<AdapterEvent> {
        self.adapter.prepare(&spec).await.expect("prepare 失敗");
        let recorded = std::fs::read(fixture_path(scenario.fixture())).expect("找不到錄製檔");
        std::fs::write(self.adapter.run_dir(&spec.run_id).join(STREAM_FILE), recorded).unwrap();
        collect(&*self.adapter, spec).await
    }
}

fn replay(data_dir: &Path) -> ReplaySubject {
    let adapter = ClaudeCodeAdapter::new("claude".into(), "baton-mcp-shim.exe".into(), data_dir, Vec::new())
        .with_poll(Duration::from_millis(5));
    ReplaySubject { adapter: Arc::new(adapter) }
}

#[tokio::test]
async fn recorded_claude_code_completes() {
    let dir = tempfile::tempdir().unwrap();
    contract_completes(&replay(dir.path()), contract_spec("run_replay_ok", Scenario::Completes, None, None, dir.path())).await;
}

#[tokio::test]
async fn recorded_claude_code_reports_an_invalid_model() {
    let dir = tempfile::tempdir().unwrap();
    let spec = contract_spec("run_replay_bad", Scenario::InvalidModel, None, None, dir.path());
    contract_invalid_model(&replay(dir.path()), spec).await;
}

#[test]
fn claude_code_adapter_spawn_spec() {
    let dir = tempfile::tempdir().unwrap();
    contract_spawn_spec(&replay(dir.path()), &contract_spec("run_replay_spec", Scenario::Completes, None, None, dir.path()));
}

// ---------------------------------------------------------------- live

/// live：真的啟動 `claude -p`。設定：
/// - `BATON_LIVE_PROFILE_DIR`（必要）：已登入 Claude Code 的設定目錄，作為 Run 的 `CLAUDE_CONFIG_DIR`。
/// - `BATON_LIVE_MODEL`（選用）：正常完成情境使用的模型（例如 `haiku`），省略時用帳號預設。
/// - `BATON_LIVE_RECORD_DIR`（選用）：把 CLI 的 stream-json 另存到這個目錄（更新 replay 的錄製檔用）。
#[cfg(windows)]
mod live {
    use super::*;
    use baton_ports::{ClaudeEnvironment, ProcessControl, ToolProbe};
    use baton_runtime::claude_cli::ClaudeCli;
    use baton_runtime::windows_process::WindowsProcessControl;

    struct LiveSubject {
        adapter: Arc<ClaudeCodeAdapter>,
        process_control: WindowsProcessControl,
        record_dir: Option<PathBuf>,
    }

    #[async_trait]
    impl Subject for LiveSubject {
        fn adapter(&self) -> Arc<dyn AgentAdapter> {
            self.adapter.clone()
        }

        async fn run(&self, scenario: Scenario, spec: RunSpec) -> Vec<AdapterEvent> {
            self.adapter.prepare(&spec).await.expect("prepare 失敗");
            let mut spawn = self.adapter.spawn_spec(&spec);
            // 與 Supervisor 相同：憑證只經環境變數交付（這裡沒有 Runtime，shim 的工具呼叫會失敗，不影響契約）
            spawn.env.push(("BATON_AGENT_TOKEN".into(), "contract-test-token".into()));
            spawn.env.push(("BATON_RUNTIME_URL".into(), "http://127.0.0.1:9".into()));
            let receipt = self.process_control.spawn(spawn).await.expect("無法啟動 claude");
            let events = collect(&*self.adapter, spec.clone()).await;
            self.process_control.stop(&receipt.identity, Duration::from_secs(10)).await.ok();
            if let Some(dir) = &self.record_dir {
                std::fs::create_dir_all(dir).unwrap();
                let stream = self.adapter.run_dir(&spec.run_id).join(STREAM_FILE);
                std::fs::copy(stream, dir.join(scenario.fixture())).unwrap();
            }
            eprintln!("{scenario:?}：{events:?}");
            events
        }
    }

    struct Live {
        subject: LiveSubject,
        config_dir: String,
        model: Option<String>,
        data_dir: tempfile::TempDir,
    }

    async fn live() -> Live {
        let config_dir = std::env::var("BATON_LIVE_PROFILE_DIR").expect("請設定 BATON_LIVE_PROFILE_DIR");
        let version = ClaudeCli.probe_claude().await.expect("無法探測 Claude Code");
        eprintln!("Claude Code：{version:?}");
        let ToolProbe::Found { version_output } = version else { panic!("找不到 Claude Code") };
        assert!(
            matches!(baton_domain::claude_code::check(version_output.as_deref()), baton_domain::claude_code::VersionCheck::Supported(_)),
            "契約只在支援的版本上執行：{version_output:?}"
        );
        let data_dir = tempfile::tempdir().unwrap();
        let mut adapter = ClaudeCodeAdapter::for_current_process(data_dir.path());
        if let Ok(shim) = std::env::var("BATON_LIVE_SHIM") {
            let inherited = std::env::vars_os().map(|(k, _)| k.to_string_lossy().into_owned()).collect();
            adapter = ClaudeCodeAdapter::new("claude".into(), shim, data_dir.path(), inherited);
        }
        Live {
            subject: LiveSubject {
                adapter: Arc::new(adapter),
                process_control: WindowsProcessControl::new(),
                record_dir: std::env::var_os("BATON_LIVE_RECORD_DIR").map(PathBuf::from),
            },
            config_dir,
            model: std::env::var("BATON_LIVE_MODEL").ok().filter(|m| !m.is_empty()),
            data_dir,
        }
    }

    fn unique(tag: &str) -> String {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        format!("run_live_{tag}_{nanos}")
    }

    #[tokio::test]
    #[ignore = "需要已登入的 Claude Code；設定 BATON_LIVE_PROFILE_DIR，見 docs/adapter-contract.md"]
    async fn live_claude_code_completes() {
        let live = live().await;
        let work = live.data_dir.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        let spec = contract_spec(&unique("ok"), Scenario::Completes, live.model.clone(), Some(live.config_dir.clone()), &work);
        contract_completes(&live.subject, spec).await;
    }

    #[tokio::test]
    #[ignore = "需要已登入的 Claude Code；設定 BATON_LIVE_PROFILE_DIR，見 docs/adapter-contract.md"]
    async fn live_claude_code_reports_an_invalid_model() {
        let live = live().await;
        let work = live.data_dir.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        let spec = contract_spec(&unique("bad"), Scenario::InvalidModel, None, Some(live.config_dir.clone()), &work);
        contract_invalid_model(&live.subject, spec).await;
    }

    #[tokio::test]
    #[ignore = "需要已登入的 Claude Code；設定 BATON_LIVE_PROFILE_DIR，見 docs/adapter-contract.md"]
    async fn live_claude_code_adapter_spawn_spec() {
        let live = live().await;
        let spec = contract_spec(&unique("spec"), Scenario::Completes, live.model.clone(), Some(live.config_dir.clone()), live.data_dir.path());
        contract_spawn_spec(&live.subject, &spec);
    }
}
