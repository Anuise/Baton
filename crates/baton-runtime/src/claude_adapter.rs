//! 正式的 AgentAdapter：Windows 原生 Claude Code（`claude -p` + stream-json，spec D-06、D-07）。
//!
//! 每個 Run 在資料目錄下有一個專屬目錄 `runs\{run_id}\`：
//! - `prompt.md`：Prompt，經標準輸入交給 `claude -p`（不放進指令列，避免長度上限與留在程序清單中）。
//! - `mcp.json`：只指向 MCP shim 的 MCP 設定（spec D-05）；憑證只以 `${BATON_AGENT_TOKEN}` 參照，值經環境變數交付。
//! - `stream.jsonl`：CLI 的標準輸出（stream-json）。事件串流從檔案讀取，所以 Runtime 重建後重新接上同一個檔案即可。
//! - `stderr.log`：CLI 的標準錯誤，留給人查看。
//!
//! 啟動參數只有旗標、檔案路徑與模型名稱，**不含任何憑證值**；`CLAUDE_CONFIG_DIR` 指向 Run 帳號的 Profile 目錄，
//! 並移除 Runtime 繼承到、會改變認證來源的環境變數（`ANTHROPIC*`、`CLAUDE*`）。
//! 共同設定以 Run 專屬唯讀快照傳入（spec D-09）；使用者層、專案層與本機層的設定檔一律不載入（`--setting-sources ""`），
//! Worker 無法藉由寫入 Workspace 內的 `.claude\settings*.json` 改變自己的權限。

use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use baton_domain::agent::{mcp_config_json, MCP_SERVER_NAME};
use baton_domain::quota::{LimitStatus, QuotaWindow};
use baton_domain::shared_settings::{INSTRUCTIONS_FILE, MCP_FILE, SETTINGS_FILE};
use baton_ports::{
    write_file_atomic, AdapterEvent, AgentAdapter, AgentHandle, Capability, PortError, PortResult, RunSpec, SessionRef,
    SpawnSpec,
};
use futures::stream::BoxStream;
use serde_json::Value;

/// Run 專屬目錄放在資料目錄下的子目錄（spec D-17）。
pub const RUNS_DIR: &str = "runs";
pub const PROMPT_FILE: &str = "prompt.md";
pub const RUN_MCP_FILE: &str = "mcp.json";
pub const STREAM_FILE: &str = "stream.jsonl";
pub const STDERR_FILE: &str = "stderr.log";

/// 繼承環境中仍保留的 `CLAUDE*` 變數：只影響 Claude Code 找 Git Bash，與認證來源無關。
const KEPT_ENV: [&str; 1] = ["CLAUDE_CODE_GIT_BASH_PATH"];
/// 錯誤訊息的長度上限（字元數），避免把整段輸出塞進 Run 的結束原因。
const ERROR_MAX_CHARS: usize = 500;
/// 讀取 stream 檔案的間隔。
const DEFAULT_POLL: Duration = Duration::from_millis(250);

/// 會改變 Claude Code 認證來源或行為的繼承環境變數（不分大小寫）。
pub fn affects_auth(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    (upper.starts_with("ANTHROPIC") || upper.starts_with("CLAUDE")) && !KEPT_ENV.contains(&upper.as_str())
}

pub struct ClaudeCodeAdapter {
    /// `claude` 的執行檔（CreateProcessW 依 PATH 尋找，並自動補上 `.exe`）。
    program: String,
    /// MCP shim 執行檔的完整路徑。
    shim_program: String,
    /// `runs\` 目錄。
    runs_dir: PathBuf,
    /// Runtime 啟動時繼承的環境變數名稱；啟動 Worker 前移除其中會改變認證來源的。
    inherited_env: Vec<String>,
    poll: Duration,
}

impl ClaudeCodeAdapter {
    pub fn new(program: String, shim_program: String, data_dir: &Path, inherited_env: Vec<String>) -> Self {
        Self { program, shim_program, runs_dir: data_dir.join(RUNS_DIR), inherited_env, poll: DEFAULT_POLL }
    }

    /// 正式執行：`claude`、與 Runtime 同目錄的 `baton-mcp-shim.exe`、目前程序的環境變數。
    pub fn for_current_process(data_dir: &Path) -> Self {
        let shim = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join("baton-mcp-shim.exe")))
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_else(|| "baton-mcp-shim.exe".to_string());
        let inherited = std::env::vars_os().map(|(name, _)| name.to_string_lossy().into_owned()).collect();
        Self::new("claude".to_string(), shim, data_dir, inherited)
    }

    /// 讀取 stream 檔案的間隔（測試用較短的間隔）。
    pub fn with_poll(mut self, poll: Duration) -> Self {
        self.poll = poll;
        self
    }

    pub fn run_dir(&self, run_id: &str) -> PathBuf {
        self.runs_dir.join(run_id)
    }

    fn file(&self, run_id: &str, name: &str) -> String {
        self.run_dir(run_id).join(name).to_string_lossy().into_owned()
    }
}

#[async_trait]
impl AgentAdapter for ClaudeCodeAdapter {
    fn spawn_spec(&self, spec: &RunSpec) -> SpawnSpec {
        let mut args: Vec<String> = ["-p", "--output-format", "stream-json", "--verbose"].map(String::from).to_vec();
        // 不載入任何設定檔來源（使用者層、Workspace 的 .claude\settings*.json）：設定只來自 Run 的快照
        args.extend(["--setting-sources", ""].map(String::from));
        // 只使用這個 Run 的 MCP 設定：Baton 的 shim，加上快照中的共同 MCP 伺服器
        args.extend(["--strict-mcp-config", "--mcp-config"].map(String::from));
        args.push(self.file(&spec.run_id, RUN_MCP_FILE));
        let snapshot = |name: &str| {
            let dir = spec.settings_dir.as_deref()?;
            let path = Path::new(dir).join(name);
            path.is_file().then(|| path.to_string_lossy().into_owned())
        };
        if let Some(shared_mcp) = snapshot(MCP_FILE) {
            args.push(shared_mcp);
        }
        // Baton 的工具不需要逐次核准；其他工具的權限由後續的權限轉接處理（spec D-08）
        args.extend(["--allowedTools".to_string(), format!("mcp__{MCP_SERVER_NAME}")]);
        if let Some(settings) = snapshot(SETTINGS_FILE) {
            args.extend(["--settings".to_string(), settings]);
        }
        if let Some(instructions) = snapshot(INSTRUCTIONS_FILE) {
            args.extend(["--append-system-prompt-file".to_string(), instructions]);
        }
        if let Some(model) = &spec.model {
            args.extend(["--model".to_string(), model.clone()]);
        }

        let mut env_remove: Vec<String> = self.inherited_env.iter().filter(|name| affects_auth(name)).cloned().collect();
        env_remove.sort();
        env_remove.dedup();
        let env = spec.config_dir.iter().map(|dir| ("CLAUDE_CONFIG_DIR".to_string(), dir.clone())).collect();
        SpawnSpec {
            program: self.program.clone(),
            args,
            working_dir: spec.working_dir.clone(),
            command_id: spec.command_id.clone(),
            env,
            env_remove,
            stdin_file: Some(self.file(&spec.run_id, PROMPT_FILE)),
            stdout_file: Some(self.file(&spec.run_id, STREAM_FILE)),
            stderr_file: Some(self.file(&spec.run_id, STDERR_FILE)),
        }
    }

    async fn prepare(&self, spec: &RunSpec) -> PortResult<()> {
        let dir = self.run_dir(&spec.run_id);
        let files = [(PROMPT_FILE, spec.prompt.clone()), (RUN_MCP_FILE, mcp_config_json(&self.shim_program))];
        tokio::task::spawn_blocking(move || {
            std::fs::create_dir_all(&dir).map_err(|e| PortError::Failed(format!("無法建立 {}：{e}", dir.display())))?;
            for (name, content) in files {
                write_file_atomic(&dir.join(name), content.as_bytes())?;
            }
            Ok(())
        })
        .await
        .map_err(|e| PortError::Failed(format!("準備啟動檔案的工作被中斷：{e}")))?
    }

    async fn launch(&self, spec: RunSpec) -> PortResult<AgentHandle> {
        Ok(AgentHandle(spec.run_id))
    }

    /// 從頭讀取 stream 檔案並持續追蹤新的內容；交出 `Exited` 之後串流結束。
    /// Runtime 重建後重新接上時會重播先前的事件（Session 綁定是冪等的，heartbeat 只會續約 lease）。
    fn events(&self, handle: &AgentHandle) -> BoxStream<'static, AdapterEvent> {
        let path = self.run_dir(&handle.0).join(STREAM_FILE);
        let poll = self.poll;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            let mut offset = 0u64;
            let mut pending: Vec<u8> = Vec::new();
            while !tx.is_closed() {
                let fresh = read_from(&path, offset).await;
                if !fresh.is_empty() {
                    offset += fresh.len() as u64;
                    pending.extend_from_slice(&fresh);
                    // 有新輸出就是程序還活著
                    if tx.send(AdapterEvent::Heartbeat).is_err() {
                        return;
                    }
                    while let Some(end) = pending.iter().position(|b| *b == b'\n') {
                        let line: Vec<u8> = pending.drain(..=end).collect();
                        for event in parse_stream_line(&String::from_utf8_lossy(&line)) {
                            let exited = matches!(event, AdapterEvent::Exited { .. });
                            if tx.send(event).is_err() || exited {
                                return;
                            }
                        }
                    }
                }
                tokio::time::sleep(poll).await;
            }
        });
        Box::pin(futures::stream::poll_fn(move |cx| rx.poll_recv(cx)))
    }

    /// 原生 Session 的接續屬於 Session 持久化與 resume 的票；在那之前不宣稱可以接續。
    async fn resume_capability(&self, _session: &SessionRef) -> PortResult<Capability> {
        Ok(Capability::Unknown)
    }
}

/// 讀取檔案從 `offset` 起的新內容；檔案還不存在或暫時讀不到時視為沒有新內容。
async fn read_from(path: &Path, offset: u64) -> Vec<u8> {
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    let Ok(mut file) = tokio::fs::File::open(path).await else { return Vec::new() };
    if file.seek(std::io::SeekFrom::Start(offset)).await.is_err() {
        return Vec::new();
    }
    let mut buffer = Vec::new();
    match file.read_to_end(&mut buffer).await {
        Ok(_) => buffer,
        Err(_) => Vec::new(),
    }
}

/// 把 stream-json 的一行轉成 Adapter 事件。只取用 Runtime 需要的欄位，其餘事件類型忽略。
///
/// - `system/init`：原生 Session 識別 → `Started`。
/// - `assistant`：文字內容 → `Message`。
/// - `rate_limit_event`：→ `RateLimited`（見 `rate_limits`；`resetsAt` 是秒）。
/// - `result`：有 `usage` 時先 `UsageReported`；`is_error` 為 false 且 `subtype` 為 `success` → `Exited { success: true }`；
///   否則先 `Error`（`result` 文字或結束原因）再 `Exited { success: false }`。
/// - 不是 JSON 的行 → `Error`（輸出被截斷或混入非協定內容）。
pub fn parse_stream_line(line: &str) -> Vec<AdapterEvent> {
    let line = line.trim();
    if line.is_empty() {
        return Vec::new();
    }
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return vec![AdapterEvent::Error { message: truncate(&format!("無法解析 Claude Code 的輸出：{line}")) }];
    };
    let text = |v: &Value, key: &str| v.get(key).and_then(Value::as_str).map(str::to_string);
    match value.get("type").and_then(Value::as_str) {
        Some("system") if value.get("subtype").and_then(Value::as_str) == Some("init") => {
            vec![AdapterEvent::Started { native_session_id: text(&value, "session_id").filter(|s| !s.is_empty()) }]
        }
        Some("assistant") => value
            .pointer("/message/content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|block| text(block, "text"))
            .filter(|t| !t.trim().is_empty())
            .map(|text| AdapterEvent::Message { text })
            .collect(),
        Some("rate_limit_event") => value.get("rate_limit_info").map(|info| rate_limits(info, line)).unwrap_or_default(),
        Some("result") => {
            let mut events = Vec::new();
            if let Some(tokens) = value.get("usage").and_then(usage_tokens) {
                events.push(AdapterEvent::UsageReported { tokens, raw: line.to_string() });
            }
            let is_error = value.get("is_error").and_then(Value::as_bool).unwrap_or(true);
            let subtype = text(&value, "subtype").unwrap_or_default();
            if !is_error && subtype == "success" {
                events.push(AdapterEvent::Exited { success: true });
                return events;
            }
            let detail = text(&value, "result")
                .filter(|r| !r.trim().is_empty())
                .or_else(|| text(&value, "terminal_reason"))
                .unwrap_or_else(|| subtype.clone());
            events.extend([AdapterEvent::Error { message: truncate(&detail) }, AdapterEvent::Exited { success: false }]);
            events
        }
        _ => Vec::new(),
    }
}

/// `rate_limit_event` 的 `rate_limit_info` → 限流訊號（「已觀測」來源，spec D-12）。
///
/// `status`／`rateLimitType` 描述 CLI 目前回報的那個限制：`allowed` → 未受限、`allowed_warning` → 警告、
/// `rejected` → 已達上限；不認識的狀態不轉換。`unifiedWindows` 另外帶有每個視窗的已用比例，
/// 其他視窗也各送一個訊號（狀態視為未受限，判斷以已用比例為準）。
fn rate_limits(info: &Value, raw: &str) -> Vec<AdapterEvent> {
    let status = match info.get("status").and_then(Value::as_str) {
        Some("allowed") => LimitStatus::Allowed,
        Some("allowed_warning") => LimitStatus::Warning,
        Some("rejected") => LimitStatus::Exhausted,
        _ => return Vec::new(),
    };
    let window_name = info.get("rateLimitType").and_then(Value::as_str);
    let unified = info.get("unifiedWindows").and_then(Value::as_object);
    let used_of = |name: &str| unified.and_then(|w| w.get(name)).and_then(|w| w.get("utilization")).and_then(Value::as_f64);
    let reset_of = |entry: Option<&Value>| entry.and_then(|e| e.get("resetsAt")).and_then(Value::as_i64).map(|secs| secs * 1000);
    let mut events = vec![AdapterEvent::RateLimited {
        window: window_name.and_then(QuotaWindow::parse),
        status,
        used: info.get("utilization").and_then(Value::as_f64).or_else(|| window_name.and_then(used_of)),
        reset_at_millis: reset_of(Some(info)),
        raw: raw.to_string(),
    }];
    for (name, entry) in unified.into_iter().flatten() {
        let Some(window) = QuotaWindow::parse(name).filter(|_| Some(name.as_str()) != window_name) else { continue };
        events.push(AdapterEvent::RateLimited {
            window: Some(window),
            status: LimitStatus::Allowed,
            used: entry.get("utilization").and_then(Value::as_f64),
            reset_at_millis: reset_of(Some(entry)),
            raw: raw.to_string(),
        });
    }
    events
}

/// `result` 的 `usage` → 這一輪用掉的 token 數：輸入 + 輸出 + 快取建立（快取讀取不計，AI-121 的建議）。
fn usage_tokens(usage: &Value) -> Option<u64> {
    let field = |name: &str| usage.get(name).and_then(Value::as_u64);
    let counted = ["input_tokens", "output_tokens", "cache_creation_input_tokens"].map(field);
    counted.iter().any(Option::is_some).then(|| counted.iter().flatten().sum())
}

fn truncate(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= ERROR_MAX_CHARS {
        return trimmed.to_string();
    }
    let mut cut: String = trimmed.chars().take(ERROR_MAX_CHARS).collect();
    cut.push('…');
    cut
}

#[cfg(test)]
mod tests {
    use super::*;

    // 以下各行取自 Claude Code 2.1.307 的實際 stream-json 輸出（欄位精簡、路徑與識別已改為範例值）
    const INIT: &str = r#"{"type":"system","subtype":"init","cwd":"C:\\work\\app","session_id":"326f0edb-57c5-40e9-8560-99ae95e9027e","tools":["Bash","Edit"],"mcp_servers":[{"name":"baton","status":"connected"}],"model":"claude-opus-5-5","permissionMode":"default","apiKeySource":"none","claude_code_version":"2.1.307","uuid":"04c0e190-abce-4973-ad8f-14ea3eba5e8b"}"#;
    const ASSISTANT: &str = r#"{"type":"assistant","message":{"id":"m1","role":"assistant","content":[{"type":"thinking","thinking":"..."},{"type":"text","text":"OK"},{"type":"tool_use","id":"t1","name":"Read","input":{}}]},"session_id":"326f0edb-57c5-40e9-8560-99ae95e9027e"}"#;
    const RATE_ALLOWED: &str = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed","resetsAt":1792660800,"rateLimitType":"five_hour","overageStatus":"rejected","isUsingOverage":false,"unifiedWindows":{"five_hour":{"utilization":0,"resetsAt":1792660800},"seven_day":{"utilization":0.01,"resetsAt":1792724400}}},"session_id":"c40db023"}"#;
    const RATE_WARNING: &str = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed_warning","resetsAt":1793116800,"rateLimitType":"seven_day","utilization":0.5,"unifiedWindows":{"five_hour":{"utilization":0.16,"resetsAt":1792660800},"seven_day":{"utilization":0.5,"resetsAt":1793116800}}},"session_id":"7fabe7af"}"#;
    const RATE_REJECTED: &str = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"rejected","resetsAt":1792660800,"rateLimitType":"five_hour"},"session_id":"c40db023"}"#;
    const RESULT_OK: &str = r#"{"type":"result","subtype":"success","is_error":false,"terminal_reason":"completed","api_error_status":null,"num_turns":91,"result":"RESULT: DONE","usage":{"input_tokens":10,"cache_creation_input_tokens":1000,"cache_read_input_tokens":50000,"output_tokens":200},"session_id":"c40db023-7d52-4b61-80bd-f12978568919"}"#;
    const RESULT_NOT_LOGGED_IN: &str = r#"{"duration_api_ms":0,"stop_reason":"stop_sequence","session_id":"326f0edb-57c5-40e9-8560-99ae95e9027e","total_cost_usd":0,"terminal_reason":"api_error","is_error":true,"num_turns":1,"subtype":"success","api_error_status":null,"result":"Not logged in · Please run /login","type":"result"}"#;
    const THINKING: &str = r#"{"type":"system","subtype":"thinking_tokens","estimated_tokens":50,"session_id":"c40db023"}"#;

    #[test]
    fn init_reports_the_native_session_id() {
        assert_eq!(
            parse_stream_line(INIT),
            [AdapterEvent::Started { native_session_id: Some("326f0edb-57c5-40e9-8560-99ae95e9027e".into()) }]
        );
    }

    #[test]
    fn assistant_text_blocks_become_messages_and_other_blocks_are_ignored() {
        assert_eq!(parse_stream_line(ASSISTANT), [AdapterEvent::Message { text: "OK".into() }]);
    }

    fn limited(window: QuotaWindow, status: LimitStatus, used: Option<f64>, reset_secs: i64, raw: &str) -> AdapterEvent {
        AdapterEvent::RateLimited { window: Some(window), status, used, reset_at_millis: Some(reset_secs * 1000), raw: raw.into() }
    }

    #[test]
    fn rate_limit_events_report_the_status_of_the_reported_window_and_the_usage_of_every_window() {
        assert_eq!(
            parse_stream_line(RATE_ALLOWED),
            [
                limited(QuotaWindow::FiveHour, LimitStatus::Allowed, Some(0.0), 1_792_660_800, RATE_ALLOWED),
                limited(QuotaWindow::SevenDay, LimitStatus::Allowed, Some(0.01), 1_792_724_400, RATE_ALLOWED),
            ]
        );
        assert_eq!(
            parse_stream_line(RATE_WARNING),
            [
                limited(QuotaWindow::SevenDay, LimitStatus::Warning, Some(0.5), 1_793_116_800, RATE_WARNING),
                limited(QuotaWindow::FiveHour, LimitStatus::Allowed, Some(0.16), 1_792_660_800, RATE_WARNING),
            ]
        );
        assert_eq!(
            parse_stream_line(RATE_REJECTED),
            [limited(QuotaWindow::FiveHour, LimitStatus::Exhausted, None, 1_792_660_800, RATE_REJECTED)]
        );
    }

    #[test]
    fn rate_limits_with_an_unknown_status_or_window_are_not_guessed() {
        let unknown_status = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"throttled","rateLimitType":"five_hour"}}"#;
        assert_eq!(parse_stream_line(unknown_status), []);
        let unknown_window = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"rejected","rateLimitType":"opus_weekly"}}"#;
        let events = parse_stream_line(unknown_window);
        assert!(
            matches!(&events[..], [AdapterEvent::RateLimited { window: None, status: LimitStatus::Exhausted, used: None, reset_at_millis: None, .. }]),
            "{events:?}"
        );
    }

    #[test]
    fn a_successful_result_reports_its_usage_then_ends_the_run_successfully() {
        assert_eq!(
            parse_stream_line(RESULT_OK),
            [AdapterEvent::UsageReported { tokens: 1_210, raw: RESULT_OK.into() }, AdapterEvent::Exited { success: true }],
            "快取讀取不計入"
        );
    }

    #[test]
    fn an_error_result_reports_its_text_then_ends_the_run_as_failed_even_with_subtype_success() {
        assert_eq!(
            parse_stream_line(RESULT_NOT_LOGGED_IN),
            [AdapterEvent::Error { message: "Not logged in · Please run /login".into() }, AdapterEvent::Exited { success: false }],
            "沒有 usage 就不回報用量"
        );
        let max_turns = r#"{"type":"result","subtype":"error_max_turns","is_error":false,"session_id":"s"}"#;
        assert_eq!(
            parse_stream_line(max_turns),
            [AdapterEvent::Error { message: "error_max_turns".into() }, AdapterEvent::Exited { success: false }]
        );
    }

    #[test]
    fn unrelated_events_and_blank_lines_produce_nothing_and_garbage_is_an_error() {
        assert_eq!(parse_stream_line(THINKING), []);
        assert_eq!(parse_stream_line("   "), []);
        assert!(matches!(&parse_stream_line("Error: unknown option")[..], [AdapterEvent::Error { .. }]));
    }

    #[test]
    fn long_error_texts_are_truncated() {
        let long = format!(r#"{{"type":"result","subtype":"success","is_error":true,"result":"{}"}}"#, "x".repeat(2000));

        let events = parse_stream_line(&long);

        let AdapterEvent::Error { message } = &events[0] else { panic!("{events:?}") };
        assert_eq!(message.chars().count(), ERROR_MAX_CHARS + 1);
    }

    fn adapter(dir: &Path, inherited: &[&str]) -> ClaudeCodeAdapter {
        ClaudeCodeAdapter::new(
            "claude".into(),
            "C:\\Baton\\bin\\baton-mcp-shim.exe".into(),
            dir,
            inherited.iter().map(|s| s.to_string()).collect(),
        )
    }

    fn run_spec(settings_dir: Option<&Path>, model: Option<&str>) -> RunSpec {
        RunSpec {
            run_id: "run_1".into(),
            task_id: "task_1".into(),
            command_id: "cmd_1".into(),
            prompt: "做事".into(),
            working_dir: Some("C:\\work\\app".into()),
            config_dir: Some("C:\\Baton\\profiles\\prof_1".into()),
            settings_dir: settings_dir.map(|d| d.to_string_lossy().into_owned()),
            model: model.map(Into::into),
        }
    }

    fn value_after<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
        args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).map(String::as_str)
    }

    #[test]
    fn the_command_is_a_print_mode_stream_json_run_with_only_this_runs_mcp_and_no_user_settings() {
        let data = tempfile::tempdir().unwrap();
        let spec = adapter(data.path(), &[]).spawn_spec(&run_spec(None, None));

        assert_eq!(spec.program, "claude");
        assert_eq!(&spec.args[..4], ["-p", "--output-format", "stream-json", "--verbose"]);
        assert_eq!(value_after(&spec.args, "--setting-sources"), Some(""), "不載入任何設定檔來源");
        assert!(spec.args.contains(&"--strict-mcp-config".to_string()));
        let mcp = data.path().join(RUNS_DIR).join("run_1").join(RUN_MCP_FILE);
        assert_eq!(value_after(&spec.args, "--mcp-config"), Some(mcp.to_string_lossy().as_ref()));
        assert_eq!(value_after(&spec.args, "--allowedTools"), Some("mcp__baton"));
        for absent in ["--settings", "--append-system-prompt-file", "--model", "--dangerously-skip-permissions"] {
            assert!(!spec.args.contains(&absent.to_string()), "{absent}：{:?}", spec.args);
        }
        assert_eq!((spec.working_dir.as_deref(), spec.command_id.as_str()), (Some("C:\\work\\app"), "cmd_1"));
        let run_dir = data.path().join(RUNS_DIR).join("run_1");
        assert_eq!(spec.stdin_file, Some(run_dir.join(PROMPT_FILE).to_string_lossy().into_owned()));
        assert_eq!(spec.stdout_file, Some(run_dir.join(STREAM_FILE).to_string_lossy().into_owned()));
        assert_eq!(spec.stderr_file, Some(run_dir.join(STDERR_FILE).to_string_lossy().into_owned()));
    }

    #[test]
    fn the_settings_snapshot_files_and_the_model_are_passed_when_present() {
        let data = tempfile::tempdir().unwrap();
        let snapshot = data.path().join("run-settings").join("run_1");
        std::fs::create_dir_all(&snapshot).unwrap();
        for name in [SETTINGS_FILE, MCP_FILE, INSTRUCTIONS_FILE] {
            std::fs::write(snapshot.join(name), "{}").unwrap();
        }

        let spec = adapter(data.path(), &[]).spawn_spec(&run_spec(Some(&snapshot), Some("claude-opus-5-5")));

        let path = |name: &str| snapshot.join(name).to_string_lossy().into_owned();
        assert_eq!(value_after(&spec.args, "--settings"), Some(path(SETTINGS_FILE).as_str()));
        assert_eq!(value_after(&spec.args, "--append-system-prompt-file"), Some(path(INSTRUCTIONS_FILE).as_str()));
        let mcp_at = spec.args.iter().position(|a| a == "--mcp-config").unwrap();
        assert_eq!(spec.args[mcp_at + 2], path(MCP_FILE), "共同 MCP 伺服器接在 Baton 的設定之後：{:?}", spec.args);
        assert_eq!(value_after(&spec.args, "--model"), Some("claude-opus-5-5"));
    }

    #[test]
    fn the_profiles_config_dir_is_set_and_inherited_auth_variables_are_removed() {
        let data = tempfile::tempdir().unwrap();
        let inherited =
            ["PATH", "ANTHROPIC_API_KEY", "anthropic_auth_token", "CLAUDE_CODE_OAUTH_TOKEN", "CLAUDECODE", "CLAUDE_CONFIG_DIR", "CLAUDE_CODE_USE_BEDROCK", "CLAUDE_CODE_GIT_BASH_PATH", "USERPROFILE"];

        let spec = adapter(data.path(), &inherited).spawn_spec(&run_spec(None, None));

        assert_eq!(spec.env, [("CLAUDE_CONFIG_DIR".to_string(), "C:\\Baton\\profiles\\prof_1".to_string())]);
        let mut removed = spec.env_remove.clone();
        removed.sort();
        assert_eq!(
            removed,
            ["ANTHROPIC_API_KEY", "CLAUDECODE", "CLAUDE_CODE_OAUTH_TOKEN", "CLAUDE_CODE_USE_BEDROCK", "CLAUDE_CONFIG_DIR", "anthropic_auth_token"]
        );
    }

    #[test]
    fn the_spawn_spec_is_the_same_every_time_for_the_same_run_spec() {
        let data = tempfile::tempdir().unwrap();
        let adapter = adapter(data.path(), &["ANTHROPIC_API_KEY"]);
        let spec = run_spec(None, Some("sonnet"));

        assert_eq!(adapter.spawn_spec(&spec), adapter.spawn_spec(&spec));
    }

    #[tokio::test]
    async fn prepare_writes_the_prompt_and_an_mcp_config_that_only_references_the_credential() {
        let data = tempfile::tempdir().unwrap();
        let adapter = adapter(data.path(), &[]);
        let spec = run_spec(None, None);

        adapter.prepare(&spec).await.unwrap();
        adapter.prepare(&spec).await.unwrap();

        let dir = adapter.run_dir("run_1");
        assert_eq!(std::fs::read_to_string(dir.join(PROMPT_FILE)).unwrap(), "做事");
        let mcp: Value = serde_json::from_str(&std::fs::read_to_string(dir.join(RUN_MCP_FILE)).unwrap()).unwrap();
        assert_eq!(mcp["mcpServers"]["baton"]["command"], "C:\\Baton\\bin\\baton-mcp-shim.exe");
        assert_eq!(mcp["mcpServers"]["baton"]["env"]["BATON_AGENT_TOKEN"], "${BATON_AGENT_TOKEN}");
    }

    #[tokio::test]
    async fn events_follow_the_stream_file_as_it_grows_and_end_after_the_result() {
        use futures::StreamExt;
        let data = tempfile::tempdir().unwrap();
        let mut adapter = adapter(data.path(), &[]);
        adapter.poll = Duration::from_millis(10);
        let dir = adapter.run_dir("run_1");
        std::fs::create_dir_all(&dir).unwrap();
        let handle = adapter.launch(run_spec(None, None)).await.unwrap();
        let mut events = adapter.events(&handle);

        // 第一行寫到一半：還不能解析
        let half = INIT.len() / 2;
        std::fs::write(dir.join(STREAM_FILE), &INIT[..half]).unwrap();
        assert_eq!(events.next().await, Some(AdapterEvent::Heartbeat));
        let rest = format!("{}\n{ASSISTANT}\n{RESULT_OK}\n", &INIT[half..]);
        let mut file = std::fs::OpenOptions::new().append(true).open(dir.join(STREAM_FILE)).unwrap();
        std::io::Write::write_all(&mut file, rest.as_bytes()).unwrap();

        let collected: Vec<_> = events.collect().await;
        assert_eq!(
            collected,
            [
                AdapterEvent::Heartbeat,
                AdapterEvent::Started { native_session_id: Some("326f0edb-57c5-40e9-8560-99ae95e9027e".into()) },
                AdapterEvent::Message { text: "OK".into() },
                AdapterEvent::UsageReported { tokens: 1_210, raw: RESULT_OK.into() },
                AdapterEvent::Exited { success: true },
            ]
        );
    }

    #[test]
    fn auth_affecting_variables_are_recognized_case_insensitively() {
        assert!(affects_auth("ANTHROPIC_BASE_URL") && affects_auth("Claude_Code_Use_Vertex") && affects_auth("CLAUDECODE"));
        assert!(!affects_auth("CLAUDE_CODE_GIT_BASH_PATH") && !affects_auth("PATH") && !affects_auth("BATON_AGENT_TOKEN"));
    }
}
