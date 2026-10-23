//! 正式的 AgentAdapter：Windows 原生 Claude Code（`claude -p` + stream-json，spec D-06、D-07）。
//!
//! 每個 Run 在資料目錄下有一個專屬目錄 `runs\{run_id}\`：
//! - `prompt.md`：Prompt，經標準輸入交給 `claude -p`（不放進指令列，避免長度上限與留在程序清單中）。
//! - `system-prompt.md`：附加的系統提示：固定的 Worker 通則，加上共同設定的指示（desktop-controller-cli 票 04）。
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
use baton_domain::agent::{mcp_config_json, MCP_SERVER_NAME, TOOL_PERMISSION_PROMPT};
use baton_domain::controller_cli::HOST_SUBCOMMAND;
use baton_domain::playbook::{self, SYSTEM_PROMPT_FILE, WORKER_RULES};
use baton_domain::quota::{LimitStatus, QuotaWindow};
use baton_domain::shared_settings::{INSTRUCTIONS_FILE, MCP_FILE, SETTINGS_FILE};
use baton_ports::{
    write_file_atomic, AdapterEvent, AgentAdapter, AgentHandle, Capability, PortError, PortResult, RunSpec, SessionRef,
    SpawnSpec,
};
use futures::stream::BoxStream;
use serde_json::Value;

use crate::claude_cli::is_batch_file;

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

/// 啟動 Claude Code 前要從繼承環境移除的變數（不分大小寫）：會改變認證來源的變數，加上外層 Claude Code
/// 或代理工具留下的 `AI_AGENT`、`BAGGAGE`（desktop-controller-cli spec：Worker 與 Controller CLI 共用的環境清理）。
pub fn inherited_to_clear(name: &str) -> bool {
    affects_auth(name) || ["AI_AGENT", "BAGGAGE"].contains(&name.to_ascii_uppercase().as_str())
}

pub struct ClaudeCodeAdapter {
    /// `claude` 的執行檔（CreateProcessW 依 PATH 尋找，並自動補上 `.exe`），
    /// 或 npm 安裝的批次檔完整路徑（經 cmd.exe 啟動，見 `claude_cli::claude_program`）。
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

    /// 正式執行：PATH 上的 `claude`（只有 npm 的 `claude.cmd` 時是它的完整路徑）、與 Runtime 同目錄的
    /// `baton-mcp-shim.exe`、目前程序的環境變數。
    pub fn for_current_process(data_dir: &Path) -> Self {
        let shim = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join("baton-mcp-shim.exe")))
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_else(|| "baton-mcp-shim.exe".to_string());
        let inherited = std::env::vars_os().map(|(name, _)| name.to_string_lossy().into_owned()).collect();
        Self::new(crate::claude_cli::claude_program(), shim, data_dir, inherited)
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

    /// Worker 與 Controller CLI 共用的 `claude` 引數：strict MCP 設定（shim 加上快照中的共同 MCP 伺服器）、
    /// Baton 工具免逐次核准、設定快照、附加系統提示檔、模型與原生接續。`permission_prompt` 為 true 時（只有 `-p`）把需要核准的操作轉給 Runtime。
    /// `system_prompt` 是合併了固定指示與共同指示的系統提示檔（Claude Code 只接受一個附加系統提示檔）。
    fn shared_args(&self, spec: &RunSpec, permission_prompt: bool, system_prompt: Option<String>) -> Vec<String> {
        // 只使用這個 Run 的 MCP 設定：Baton 的 shim，加上快照中的共同 MCP 伺服器
        let mut args: Vec<String> = ["--strict-mcp-config", "--mcp-config"].map(String::from).to_vec();
        args.push(self.file(&spec.run_id, RUN_MCP_FILE));
        // 只依綁定的版本決定要傳哪些快照檔，不讀檔案系統（啟動指令與它的雜湊在重建後必須相同）
        let snapshot = |name: &str| {
            let dir = spec.settings_dir.as_deref()?;
            spec.settings_files.iter().any(|file| file == name).then(|| Path::new(dir).join(name).to_string_lossy().into_owned())
        };
        if let Some(shared_mcp) = snapshot(MCP_FILE) {
            args.push(shared_mcp);
        }
        // Baton 的工具不需要逐次核准；其他需要核准的操作經 permission_prompt 工具轉給 Runtime 判定（spec D-08）
        args.extend(["--allowedTools".to_string(), format!("mcp__{MCP_SERVER_NAME}")]);
        if permission_prompt {
            args.extend(["--permission-prompt-tool".to_string(), format!("mcp__{MCP_SERVER_NAME}__{TOOL_PERMISSION_PROMPT}")]);
        }
        if let Some(settings) = snapshot(SETTINGS_FILE) {
            args.extend(["--settings".to_string(), settings]);
        }
        if let Some(system_prompt) = system_prompt {
            args.extend(["--append-system-prompt-file".to_string(), system_prompt]);
        }
        if let Some(model) = &spec.model {
            args.extend(["--model".to_string(), model.clone()]);
        }
        // 以原生方式接續既有的 Session（票 15）：init 與 result 回報的 session_id 維持同一個識別
        if let Some(native_session_id) = &spec.resume_native_session_id {
            args.extend(["--resume".to_string(), native_session_id.clone()]);
        }
        args
    }

    /// `claude` 是批次檔時改經 cmd.exe 執行，回傳實際的程式與引數。
    fn wrap_batch(&self, args: Vec<String>) -> (String, Vec<String>) {
        if is_batch_file(&self.program) {
            // CreateProcessW 不能直接執行批次檔：經 cmd.exe 執行。`/d` 不跑 AutoRun（不會有額外輸出混進 stream），
            // `/v:off` 關閉延遲展開；以 `call` 開頭，路徑含空白而加的引號不會被 `/c` 的引號規則剝掉。
            // 引數只有旗標、路徑與模型名稱（Prompt 經標準輸入），`prepare` 會拒絕 cmd 會另作解讀的字元。
            let mut wrapped: Vec<String> = ["/d", "/v:off", "/c", "call"].map(String::from).to_vec();
            wrapped.push(self.program.clone());
            wrapped.extend(args);
            (cmd_exe(), wrapped)
        } else {
            (self.program.clone(), args)
        }
    }

    /// 兩種啟動共用的環境與工作目錄：移除繼承的、會改變認證來源的變數，`CLAUDE_CONFIG_DIR` 指向 Run 的帳號。
    fn base_spec(&self, spec: &RunSpec) -> SpawnSpec {
        let mut env_remove: Vec<String> = self.inherited_env.iter().filter(|name| inherited_to_clear(name)).cloned().collect();
        env_remove.sort();
        env_remove.dedup();
        let env = spec.config_dir.iter().map(|dir| ("CLAUDE_CONFIG_DIR".to_string(), dir.clone())).collect();
        SpawnSpec {
            working_dir: spec.working_dir.clone(),
            command_id: spec.command_id.clone(),
            env,
            env_remove,
            ..Default::default()
        }
    }
}

#[async_trait]
impl AgentAdapter for ClaudeCodeAdapter {
    fn spawn_spec(&self, spec: &RunSpec) -> SpawnSpec {
        let mut args: Vec<String> = ["-p", "--output-format", "stream-json", "--verbose"].map(String::from).to_vec();
        // 不載入任何設定檔來源（使用者層、Workspace 的 .claude\settings*.json）：設定只來自 Run 的快照
        args.extend(["--setting-sources", ""].map(String::from));
        // Worker 通則與共同指示合併成 Run 目錄裡的系統提示檔（`prepare` 寫入，票 04）
        args.extend(self.shared_args(spec, true, Some(self.file(&spec.run_id, SYSTEM_PROMPT_FILE))));
        let (program, args) = self.wrap_batch(args);
        SpawnSpec {
            program,
            args,
            stdin_file: Some(self.file(&spec.run_id, PROMPT_FILE)),
            stdout_file: Some(self.file(&spec.run_id, STREAM_FILE)),
            stderr_file: Some(self.file(&spec.run_id, STDERR_FILE)),
            ..self.base_spec(spec)
        }
    }

    /// 互動式 Controller CLI（票 03）：與 Worker 共用同一套組裝（環境清理、`CLAUDE_CONFIG_DIR`、strict MCP 設定、設定快照、
    /// 工作目錄），差別只在不加 `-p`、不指定輸出格式與權限模式（`--permission-prompt-tool` 只在 `-p` 有效）。
    /// 設定檔來源只載入 Workspace 的專案層與本機層，Workspace 的 `CLAUDE.md` 與設定以原生方式生效（使用者層由快照提供）。
    /// 終端機裡執行的是 MCP shim 的「Controller 宿主」子命令，由它以繼承主控台的方式啟動 `claude`；標準輸入輸出不導向檔案。
    /// 附加的系統提示是 Runtime 寫在 Run 專屬唯讀目錄（`settings_dir`）的操作手冊檔（票 04）。
    fn controller_spawn_spec(&self, spec: &RunSpec) -> SpawnSpec {
        let mut args: Vec<String> = ["--setting-sources", "project,local"].map(String::from).to_vec();
        let system_prompt =
            spec.settings_dir.as_deref().map(|dir| Path::new(dir).join(SYSTEM_PROMPT_FILE).to_string_lossy().into_owned());
        args.extend(self.shared_args(spec, false, system_prompt));
        // 同帳號重新開啟：接續這個工作目錄在這個帳號下最近的原生 Session（票 07）
        if spec.continue_session {
            args.push("--continue".to_string());
        }
        let (claude, claude_args) = self.wrap_batch(args);
        let mut args = vec![HOST_SUBCOMMAND.to_string(), claude];
        args.extend(claude_args);
        SpawnSpec { program: self.shim_program.clone(), args, ..self.base_spec(spec) }
    }

    async fn prepare(&self, spec: &RunSpec) -> PortResult<()> {
        if is_batch_file(&self.program) {
            // 經 cmd.exe 啟動時，引數（含批次檔路徑）不能有 cmd 會另作解讀的字元，否則可能被拆成別的命令
            for arg in self.spawn_spec(spec).args.iter().skip(4) {
                if let Some(ch) = arg.chars().find(|c| CMD_UNSAFE.contains(*c) || c.is_control()) {
                    return Err(PortError::Failed(format!(
                        "Claude Code 是批次檔（{}），要經 cmd.exe 啟動，但引數 {arg:?} 含有 cmd.exe 會另作解讀的字元 {ch:?}；\
                         請安裝原生的 claude.exe，或調整資料目錄、模型名稱",
                        self.program
                    )));
                }
            }
        }
        let dir = self.run_dir(&spec.run_id);
        let prompt = spec.prompt.clone();
        let mcp = mcp_config_json(&self.shim_program);
        // 共同指示只看綁定的版本有沒有這一類（快照已由 Runtime 依 manifest 核對過）
        let instructions = match spec.settings_dir.as_deref() {
            Some(settings_dir) if spec.settings_files.iter().any(|file| file == INSTRUCTIONS_FILE) => {
                Some(Path::new(settings_dir).join(INSTRUCTIONS_FILE))
            }
            _ => None,
        };
        tokio::task::spawn_blocking(move || {
            let instructions = match instructions {
                Some(path) => Some(
                    std::fs::read_to_string(&path).map_err(|e| PortError::Failed(format!("無法讀取共同指示 {}：{e}", path.display())))?,
                ),
                None => None,
            };
            let system_prompt = playbook::system_prompt(WORKER_RULES, instructions.as_deref());
            std::fs::create_dir_all(&dir).map_err(|e| PortError::Failed(format!("無法建立 {}：{e}", dir.display())))?;
            for (name, content) in [(PROMPT_FILE, prompt), (RUN_MCP_FILE, mcp), (SYSTEM_PROMPT_FILE, system_prompt)] {
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
    ///
    /// 每個非空白行先交出 `Transcript`（位置是它在 stream 檔案中是第幾個非空白行，從 1 起），
    /// 再交出由它解析出的事件；重播時同一行得到同一個位置，Runtime 據此去重。
    fn events(&self, handle: &AgentHandle) -> BoxStream<'static, AdapterEvent> {
        let path = self.run_dir(&handle.0).join(STREAM_FILE);
        let poll = self.poll;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            let mut offset = 0u64;
            let mut position = 0u64;
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
                        let bytes: Vec<u8> = pending.drain(..=end).collect();
                        let decoded = String::from_utf8_lossy(&bytes);
                        let line = decoded.trim_end_matches(['\r', '\n']);
                        if line.trim().is_empty() {
                            continue;
                        }
                        position += 1;
                        if tx.send(AdapterEvent::Transcript { position, line: line.to_string() }).is_err() {
                            return;
                        }
                        for event in parse_stream_line(line) {
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

    /// 原生 Session 檔案 `{CLAUDE_CONFIG_DIR}\projects\{工作目錄的目錄名}\{原生識別}.jsonl` 存在才可接續。
    /// 缺帳號目錄或工作目錄、目錄名規則未實測、`projects` 目錄讀不到 → `Unknown`；
    /// `projects` 可讀但沒有這個檔案、或原生識別不是單純的識別（不能組成路徑）→ `NotResumable`。
    async fn resume_capability(&self, session: &SessionRef) -> PortResult<Capability> {
        let (Some(config_dir), Some(working_dir)) = (&session.config_dir, &session.working_dir) else {
            return Ok(Capability::Unknown);
        };
        if !is_plain_id(&session.native_session_id) {
            return Ok(Capability::NotResumable);
        }
        let project = native_project_dir(working_dir);
        if project.chars().count() > NATIVE_PROJECT_DIR_MAX_CHARS {
            return Ok(Capability::Unknown);
        }
        let projects = Path::new(config_dir).join("projects");
        let file = projects.join(project).join(format!("{}.jsonl", session.native_session_id));
        tokio::task::spawn_blocking(move || match std::fs::metadata(&file) {
            Ok(meta) if meta.is_file() => Capability::Resumable,
            Ok(_) => Capability::NotResumable,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && std::fs::read_dir(&projects).is_ok() => {
                Capability::NotResumable
            }
            Err(_) => Capability::Unknown,
        })
        .await
        .map_err(|e| PortError::Failed(format!("查驗原生 Session 的工作被中斷：{e}")))
    }

    /// 工作目錄在這個帳號下有沒有任何原生 Session 檔（`projects\{目錄名}\*.jsonl`）：有才能 `--continue`。
    /// 帳號目錄存在而找不到 Session 檔是 `NotResumable`（例如上次開啟後沒有任何對話就關掉）；讀不到時是 `Unknown`。
    async fn continue_capability(&self, config_dir: &str, working_dir: &str) -> PortResult<Capability> {
        let project = native_project_dir(working_dir);
        if project.chars().count() > NATIVE_PROJECT_DIR_MAX_CHARS {
            return Ok(Capability::Unknown);
        }
        let config_dir = PathBuf::from(config_dir);
        let dir = config_dir.join("projects").join(project);
        tokio::task::spawn_blocking(move || match std::fs::read_dir(&dir) {
            Ok(entries) => {
                let session = |entry: &std::fs::DirEntry| {
                    entry.path().extension().is_some_and(|ext| ext == "jsonl") && entry.file_type().is_ok_and(|t| t.is_file())
                };
                if entries.flatten().any(|entry| session(&entry)) {
                    Capability::Resumable
                } else {
                    Capability::NotResumable
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && config_dir.is_dir() => Capability::NotResumable,
            Err(_) => Capability::Unknown,
        })
        .await
        .map_err(|e| PortError::Failed(format!("查驗原生 Session 的工作被中斷：{e}")))
    }
}

/// 經 cmd.exe 啟動批次檔時，引數中不允許的字元：引號會改變 cmd 的引號狀態，`%`、`!` 會被展開，
/// 其餘是 cmd 的運算子或跳脫字元（`call` 與批次檔內的 `%*` 會再解讀一次，所以引號內也不允許）。
const CMD_UNSAFE: &str = "\"%!^&|<>()";

/// 系統目錄下的 cmd.exe（不依 PATH 或目前目錄尋找）。
fn cmd_exe() -> String {
    match std::env::var_os("SystemRoot") {
        Some(root) => Path::new(&root).join("System32").join("cmd.exe").to_string_lossy().into_owned(),
        None => "cmd.exe".to_string(),
    }
}

/// 原生 Session 識別只能是英數字、`-`、`_`（Claude Code 用 UUID），其餘一律不拿來組路徑，避免逃逸出帳號目錄。
fn is_plain_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// 已實測的目錄名規則所涵蓋的最大長度（字元數）；更長的工作目錄 Claude Code 可能另外截斷，未實測。
const NATIVE_PROJECT_DIR_MAX_CHARS: usize = 200;

/// Claude Code 存放某工作目錄原生 Session 的目錄名（`{CLAUDE_CONFIG_DIR}\projects\` 之下）：
/// 每個不是 ASCII 英數字的字元換成 `-`（2.1.307 實測）。
fn native_project_dir(working_dir: &str) -> String {
    working_dir.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
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
/// - `result`：有 `usage` 時先 `UsageReported`（Context = 輸入 + 快取建立 + 快取讀取）；`is_error` 為 false 且 `subtype` 為 `success` → `Exited { success: true }`；
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
            if let Some(usage) = value.get("usage") {
                if let Some(tokens) = usage_tokens(usage) {
                    let context_tokens = context_tokens(usage);
                    events.push(AdapterEvent::UsageReported { tokens, context_tokens, raw: line.to_string() });
                }
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

/// 模型讀入的 Context：輸入、快取建立與快取讀取的 token（快取讀取不計入額度推估，但佔用 Context）。
fn context_tokens(usage: &Value) -> Option<u64> {
    let field = |name: &str| usage.get(name).and_then(Value::as_u64);
    let read = ["input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens"].map(field);
    read.iter().any(Option::is_some).then(|| read.iter().flatten().sum())
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
            [AdapterEvent::UsageReported { tokens: 1_210, context_tokens: Some(51_010), raw: RESULT_OK.into() }, AdapterEvent::Exited { success: true }],
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
            // 有快照時預設三類設定都有內容
            settings_files: match settings_dir {
                Some(_) => [SETTINGS_FILE, MCP_FILE, INSTRUCTIONS_FILE].map(String::from).to_vec(),
                None => Vec::new(),
            },
            model: model.map(Into::into),
            resume_native_session_id: None,
            continue_session: false,
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
        assert_eq!(value_after(&spec.args, "--permission-prompt-tool"), Some("mcp__baton__permission_prompt"));
        for absent in ["--settings", "--model", "--dangerously-skip-permissions"] {
            assert!(!spec.args.contains(&absent.to_string()), "{absent}：{:?}", spec.args);
        }
        assert_eq!((spec.working_dir.as_deref(), spec.command_id.as_str()), (Some("C:\\work\\app"), "cmd_1"));
        let run_dir = data.path().join(RUNS_DIR).join("run_1");
        // 票 04：沒有共同設定時也附加 Worker 通則（Run 目錄裡的系統提示檔）
        let system_prompt = run_dir.join(SYSTEM_PROMPT_FILE);
        assert_eq!(value_after(&spec.args, "--append-system-prompt-file"), Some(system_prompt.to_string_lossy().as_ref()));
        assert_eq!(spec.stdin_file, Some(run_dir.join(PROMPT_FILE).to_string_lossy().into_owned()));
        assert_eq!(spec.stdout_file, Some(run_dir.join(STREAM_FILE).to_string_lossy().into_owned()));
        assert_eq!(spec.stderr_file, Some(run_dir.join(STDERR_FILE).to_string_lossy().into_owned()));
    }

    /// 票 03：Controller CLI 與 Worker 共用組裝，但不以 `-p` 執行、不指定輸出格式與權限模式，由 Controller 宿主啟動 `claude`。
    #[test]
    fn the_controller_cli_shares_the_worker_assembly_but_runs_interactively_through_the_host() {
        let data = tempfile::tempdir().unwrap();
        let snapshot = data.path().join("run-settings").join("run_1");
        let inherited = ["ANTHROPIC_API_KEY", "CLAUDECODE", "AI_AGENT", "baggage", "CLAUDE_CODE_GIT_BASH_PATH", "PATH"];
        let adapter = adapter(data.path(), &inherited);
        let spec = adapter.controller_spawn_spec(&run_spec(Some(&snapshot), None));

        assert_eq!(spec.program, r"C:\Baton\bin\baton-mcp-shim.exe");
        assert_eq!(&spec.args[..2], [HOST_SUBCOMMAND, "claude"]);
        let claude_args = &spec.args[2..];
        for absent in ["-p", "--output-format", "--verbose", "--permission-prompt-tool", "--permission-mode", "--dangerously-skip-permissions"] {
            assert!(!claude_args.contains(&absent.to_string()), "{absent}：{claude_args:?}");
        }
        assert_eq!(value_after(claude_args, "--setting-sources"), Some("project,local"), "Workspace 的 CLAUDE.md 與設定原生載入");
        assert!(claude_args.contains(&"--strict-mcp-config".to_string()));
        let mcp = data.path().join(RUNS_DIR).join("run_1").join(RUN_MCP_FILE);
        assert_eq!(value_after(claude_args, "--mcp-config"), Some(mcp.to_string_lossy().as_ref()));
        let mcp_at = claude_args.iter().position(|a| a == "--mcp-config").unwrap();
        assert_eq!(claude_args[mcp_at + 2], snapshot.join(MCP_FILE).to_string_lossy(), "共同設定的 MCP 伺服器");
        assert_eq!(value_after(claude_args, "--settings"), Some(snapshot.join(SETTINGS_FILE).to_string_lossy().as_ref()));
        assert_eq!(value_after(claude_args, "--allowedTools"), Some("mcp__baton"));
        // 票 04：附加的系統提示是 Run 專屬唯讀目錄裡的操作手冊檔（Runtime 寫入），不是 Worker 通則
        assert_eq!(
            value_after(claude_args, "--append-system-prompt-file"),
            Some(snapshot.join(SYSTEM_PROMPT_FILE).to_string_lossy().as_ref())
        );
        assert_eq!(claude_args.iter().filter(|a| *a == "--append-system-prompt-file").count(), 1);

        // 環境清理與帳號目錄與 Worker 相同
        let worker = adapter.spawn_spec(&run_spec(Some(&snapshot), None));
        assert_eq!(spec.env_remove, worker.env_remove);
        assert_eq!(spec.env_remove, ["AI_AGENT", "ANTHROPIC_API_KEY", "CLAUDECODE", "baggage"]);
        assert_eq!(spec.env, [("CLAUDE_CONFIG_DIR".to_string(), r"C:\Baton\profiles\prof_1".to_string())]);
        assert_eq!((spec.working_dir.as_deref(), spec.command_id.as_str()), (Some(r"C:\work\app"), "cmd_1"));
        assert_eq!((&spec.stdin_file, &spec.stdout_file, &spec.stderr_file), (&None, &None, &None), "使用終端機自己的主控台");
        assert_eq!(adapter.controller_spawn_spec(&run_spec(Some(&snapshot), None)), spec, "純函式：重建後組出相同的指令");
    }

    /// 票 05：`--continue` 只在工作目錄於這個帳號下有原生 Session 檔時可用。
    #[tokio::test]
    async fn continuing_needs_a_native_session_file_for_the_working_directory() {
        let data = tempfile::tempdir().unwrap();
        let adapter = adapter(data.path(), &[]);
        let config_dir = data.path().join("profile");
        let config = config_dir.to_string_lossy().into_owned();
        let working_dir = r"C:\work\app";

        let missing = data.path().join("no-such-profile").to_string_lossy().into_owned();
        assert_eq!(adapter.continue_capability(&missing, working_dir).await.unwrap(), Capability::Unknown, "帳號目錄讀不到");
        std::fs::create_dir_all(&config_dir).unwrap();
        assert_eq!(adapter.continue_capability(&config, working_dir).await.unwrap(), Capability::NotResumable, "從未在這裡執行過");

        let project = config_dir.join("projects").join(native_project_dir(working_dir));
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join("notes.txt"), "x").unwrap();
        assert_eq!(adapter.continue_capability(&config, working_dir).await.unwrap(), Capability::NotResumable, "沒有 Session 檔");
        std::fs::write(project.join("326f0edb-57c5-40e9-8560-99ae95e9027e.jsonl"), "{}").unwrap();
        assert_eq!(adapter.continue_capability(&config, working_dir).await.unwrap(), Capability::Resumable);
        assert_eq!(adapter.continue_capability(&config, r"C:\work\other").await.unwrap(), Capability::NotResumable, "其他工作目錄不算");
    }

    #[test]
    fn a_batch_claude_is_started_by_the_host_through_cmd() {
        let data = tempfile::tempdir().unwrap();
        let adapter = ClaudeCodeAdapter::new(
            r"C:\npm\claude.cmd".into(),
            r"C:\Baton\bin\baton-mcp-shim.exe".into(),
            data.path(),
            Vec::new(),
        );
        let spec = adapter.controller_spawn_spec(&run_spec(None, None));
        assert_eq!(spec.args[0], HOST_SUBCOMMAND);
        assert!(spec.args[1].to_ascii_lowercase().ends_with("cmd.exe"), "{:?}", spec.args);
        assert_eq!(&spec.args[2..7], ["/d", "/v:off", "/c", "call", r"C:\npm\claude.cmd"]);
    }

    #[test]
    fn the_controller_cli_continues_the_native_session_only_when_asked() {
        let data = tempfile::tempdir().unwrap();
        let adapter = adapter(data.path(), &[]);
        let fresh = adapter.controller_spawn_spec(&run_spec(None, None));
        assert!(!fresh.args.contains(&"--continue".to_string()), "{:?}", fresh.args);

        let continued = adapter.controller_spawn_spec(&RunSpec { continue_session: true, ..run_spec(None, None) });
        assert_eq!(continued.args.iter().filter(|arg| *arg == "--continue").count(), 1, "{:?}", continued.args);
        // Worker 不受影響
        let worker = adapter.spawn_spec(&RunSpec { continue_session: true, ..run_spec(None, None) });
        assert!(!worker.args.contains(&"--continue".to_string()), "{:?}", worker.args);
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
        // 共同指示與 Worker 通則合併在 Run 目錄的系統提示檔（Claude Code 只接受一個附加系統提示檔）
        let system_prompt = data.path().join(RUNS_DIR).join("run_1").join(SYSTEM_PROMPT_FILE);
        assert_eq!(value_after(&spec.args, "--append-system-prompt-file"), Some(system_prompt.to_string_lossy().as_ref()));
        let mcp_at = spec.args.iter().position(|a| a == "--mcp-config").unwrap();
        assert_eq!(spec.args[mcp_at + 2], path(MCP_FILE), "共同 MCP 伺服器接在 Baton 的設定之後：{:?}", spec.args);
        assert_eq!(value_after(&spec.args, "--model"), Some("claude-opus-5-5"));
    }

    /// 要傳哪些快照檔只看 Run 綁定的版本（`settings_files`），不看檔案系統：快照目錄在 Run 啟動後被刪，
    /// Runtime 重建時組出的啟動指令（與它的雜湊）仍與啟動時相同（review-notes #6）。
    #[test]
    fn the_snapshot_files_passed_come_from_the_run_spec_and_not_from_the_file_system() {
        let data = tempfile::tempdir().unwrap();
        let snapshot = data.path().join("run-settings").join("run_1");
        let adapter = adapter(data.path(), &[]);
        let spec = RunSpec {
            settings_files: [SETTINGS_FILE, INSTRUCTIONS_FILE].map(String::from).to_vec(),
            ..run_spec(Some(&snapshot), None)
        };

        let before = adapter.spawn_spec(&spec);
        std::fs::create_dir_all(&snapshot).unwrap();
        for name in [SETTINGS_FILE, MCP_FILE, INSTRUCTIONS_FILE] {
            std::fs::write(snapshot.join(name), "{}").unwrap();
        }
        let after = adapter.spawn_spec(&spec);

        let path = |name: &str| snapshot.join(name).to_string_lossy().into_owned();
        assert_eq!(value_after(&before.args, "--settings"), Some(path(SETTINGS_FILE).as_str()), "快照目錄不存在也照綁定的版本傳");
        assert!(!before.args.contains(&path(MCP_FILE)), "版本沒有共同 MCP 伺服器就不傳：{:?}", before.args);
        assert_eq!(after, before, "檔案系統的狀態不影響啟動指令");
        assert_eq!(baton_ports::command_hash(&after), baton_ports::command_hash(&before));
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

    /// 票 04：Worker 的系統提示是固定的 Worker 通則，加上快照中的共同指示（`CLAUDE.md`，只看綁定的版本有沒有這一類）。
    #[tokio::test]
    async fn prepare_writes_the_worker_rules_followed_by_the_shared_instructions_as_the_system_prompt() {
        let data = tempfile::tempdir().unwrap();
        let adapter = adapter(data.path(), &[]);
        let system_prompt = || std::fs::read_to_string(adapter.run_dir("run_1").join(SYSTEM_PROMPT_FILE)).unwrap();

        adapter.prepare(&run_spec(None, None)).await.unwrap();
        assert_eq!(system_prompt(), WORKER_RULES, "沒有共同設定時只有 Worker 通則");

        let snapshot = data.path().join("run-settings").join("run_1");
        std::fs::create_dir_all(&snapshot).unwrap();
        std::fs::write(snapshot.join(INSTRUCTIONS_FILE), "使用者的共同指示\n").unwrap();
        adapter.prepare(&run_spec(Some(&snapshot), None)).await.unwrap();
        assert_eq!(system_prompt(), playbook::system_prompt(WORKER_RULES, Some("使用者的共同指示\n")));

        let without_instructions = RunSpec { settings_files: vec![SETTINGS_FILE.to_string()], ..run_spec(Some(&snapshot), None) };
        adapter.prepare(&without_instructions).await.unwrap();
        assert_eq!(system_prompt(), WORKER_RULES, "綁定的版本沒有共同指示就不讀快照裡的 CLAUDE.md");
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
                transcript(1, INIT),
                AdapterEvent::Started { native_session_id: Some("326f0edb-57c5-40e9-8560-99ae95e9027e".into()) },
                transcript(2, ASSISTANT),
                AdapterEvent::Message { text: "OK".into() },
                transcript(3, RESULT_OK),
                AdapterEvent::UsageReported { tokens: 1_210, context_tokens: Some(51_010), raw: RESULT_OK.into() },
                AdapterEvent::Exited { success: true },
            ]
        );
    }

    fn transcript(position: u64, line: &str) -> AdapterEvent {
        AdapterEvent::Transcript { position, line: line.into() }
    }

    #[tokio::test]
    async fn every_non_blank_line_is_a_transcript_entry_with_the_same_position_when_the_stream_is_replayed() {
        use futures::StreamExt;
        let data = tempfile::tempdir().unwrap();
        let mut adapter = adapter(data.path(), &[]);
        adapter.poll = Duration::from_millis(10);
        let dir = adapter.run_dir("run_1");
        std::fs::create_dir_all(&dir).unwrap();
        // 空白行不佔位置；Windows 的 CRLF 與結尾換行都不屬於內容
        std::fs::write(dir.join(STREAM_FILE), format!("{INIT}\r\n\n   \n{THINKING}\n{RESULT_OK}\n")).unwrap();
        let handle = adapter.launch(run_spec(None, None)).await.unwrap();

        let transcripts = |events: Vec<AdapterEvent>| -> Vec<AdapterEvent> {
            events.into_iter().filter(|e| matches!(e, AdapterEvent::Transcript { .. })).collect()
        };
        let first = transcripts(adapter.events(&handle).collect().await);
        let replayed = transcripts(adapter.events(&handle).collect().await);

        assert_eq!(first, [transcript(1, INIT), transcript(2, THINKING), transcript(3, RESULT_OK)]);
        assert_eq!(replayed, first, "重新接上時從頭重播，同一行得到同一個位置");
    }

    #[test]
    fn a_resumed_run_passes_the_native_session_to_resume_and_nothing_else_changes() {
        let data = tempfile::tempdir().unwrap();
        let adapter = adapter(data.path(), &[]);
        let fresh = run_spec(None, Some("sonnet"));
        let resumed = RunSpec { resume_native_session_id: Some("326f0edb-57c5-40e9-8560-99ae95e9027e".into()), ..fresh.clone() };

        let fresh_spec = adapter.spawn_spec(&fresh);
        let resumed_spec = adapter.spawn_spec(&resumed);

        assert!(!fresh_spec.args.contains(&"--resume".to_string()), "{:?}", fresh_spec.args);
        assert_eq!(value_after(&resumed_spec.args, "--resume"), Some("326f0edb-57c5-40e9-8560-99ae95e9027e"));
        let without_resume: Vec<String> = {
            let at = resumed_spec.args.iter().position(|a| a == "--resume").unwrap();
            let mut args = resumed_spec.args.clone();
            args.drain(at..at + 2);
            args
        };
        assert_eq!(without_resume, fresh_spec.args, "除了 --resume 之外的參數不變");
        assert_eq!(SpawnSpec { args: fresh_spec.args.clone(), ..resumed_spec.clone() }, fresh_spec);
        assert_eq!(adapter.spawn_spec(&resumed), resumed_spec, "同一個 RunSpec 永遠得到同一個啟動指令");
    }

    #[test]
    fn the_native_project_directory_replaces_every_non_alphanumeric_character_with_a_dash() {
        assert_eq!(
            native_project_dir(r"C:\Users\User\AppData\Local\Temp\resume-probe"),
            "C--Users-User-AppData-Local-Temp-resume-probe",
            "實測的 Claude Code 2.1.307 目錄名"
        );
        assert_eq!(native_project_dir(r"D:\工作\my_app.v2"), "D-----my-app-v2");
    }

    #[tokio::test]
    async fn a_working_directory_too_long_for_the_verified_naming_rule_is_unknown() {
        let data = tempfile::tempdir().unwrap();
        let profile = data.path().join("profile");
        std::fs::create_dir_all(profile.join("projects")).unwrap();
        let long = format!(r"C:\{}", "a".repeat(NATIVE_PROJECT_DIR_MAX_CHARS));

        let capability = adapter(data.path(), &[]).resume_capability(&session(Some(&profile), Some(&long), SESSION_ID)).await;

        assert_eq!(capability.unwrap(), Capability::Unknown, "超過長度的目錄名規則未實測，不宣稱不可接續");
    }

    const SESSION_ID: &str = "326f0edb-57c5-40e9-8560-99ae95e9027e";
    const WORK_DIR: &str = r"C:\work\app";

    fn session(config_dir: Option<&Path>, working_dir: Option<&str>, id: &str) -> SessionRef {
        SessionRef {
            native_session_id: id.into(),
            config_dir: config_dir.map(|d| d.to_string_lossy().into_owned()),
            working_dir: working_dir.map(Into::into),
        }
    }

    /// 在帳號目錄下照 Claude Code 的規則放一個原生 Session 檔案。
    fn store_native_session(config_dir: &Path, working_dir: &str, id: &str) {
        let project = config_dir.join("projects").join("C--work-app");
        assert_eq!(working_dir, WORK_DIR);
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join(format!("{id}.jsonl")), "{}\n").unwrap();
    }

    #[tokio::test]
    async fn a_native_session_file_in_the_accounts_project_directory_is_resumable() {
        let data = tempfile::tempdir().unwrap();
        let profile = data.path().join("profile");
        store_native_session(&profile, WORK_DIR, SESSION_ID);

        let capability = adapter(data.path(), &[]).resume_capability(&session(Some(&profile), Some(WORK_DIR), SESSION_ID)).await;

        assert_eq!(capability.unwrap(), Capability::Resumable);
    }

    #[tokio::test]
    async fn a_missing_native_session_file_is_not_resumable() {
        let data = tempfile::tempdir().unwrap();
        let profile = data.path().join("profile");
        store_native_session(&profile, WORK_DIR, "other-session");
        let adapter = adapter(data.path(), &[]);

        let missing = adapter.resume_capability(&session(Some(&profile), Some(WORK_DIR), SESSION_ID)).await;
        let other_dir = adapter.resume_capability(&session(Some(&profile), Some(r"C:\work\other"), "other-session")).await;

        assert_eq!(missing.unwrap(), Capability::NotResumable);
        assert_eq!(other_dir.unwrap(), Capability::NotResumable, "原生 Session 依工作目錄分開存放");
    }

    #[tokio::test]
    async fn without_the_account_or_working_directory_or_a_readable_projects_directory_resumability_is_unknown() {
        let data = tempfile::tempdir().unwrap();
        let profile = data.path().join("profile");
        store_native_session(&profile, WORK_DIR, SESSION_ID);
        let adapter = adapter(data.path(), &[]);

        for unknown in [
            session(None, Some(WORK_DIR), SESSION_ID),
            session(Some(&profile), None, SESSION_ID),
            session(Some(&data.path().join("no-such-profile")), Some(WORK_DIR), SESSION_ID),
        ] {
            assert_eq!(adapter.resume_capability(&unknown).await.unwrap(), Capability::Unknown, "{unknown:?}");
        }
    }

    #[tokio::test]
    async fn a_native_session_id_that_is_not_a_plain_identifier_is_never_resumable() {
        let data = tempfile::tempdir().unwrap();
        let profile = data.path().join("profile");
        store_native_session(&profile, WORK_DIR, SESSION_ID);
        // 逃逸目標：若把識別直接接進路徑，這些檔案都「存在」
        std::fs::write(profile.join("projects").join("escape.jsonl"), "{}\n").unwrap();
        let outside = data.path().join("outside.jsonl");
        std::fs::write(&outside, "{}\n").unwrap();
        let absolute = outside.with_extension("").to_string_lossy().into_owned();
        let adapter = adapter(data.path(), &[]);

        for id in ["../escape", r"..\escape", "..", "", "a/b", r"C:\x", "C:escape", &absolute, "326f0edb.jsonl"] {
            let capability = adapter.resume_capability(&session(Some(&profile), Some(WORK_DIR), id)).await.unwrap();
            assert_eq!(capability, Capability::NotResumable, "{id:?}");
        }
    }

    /// `--resume` 不存在的原生 Session（2.1.307 實測）：沒有 `system/init`，只有一行失敗的 `result`。
    #[test]
    fn resuming_a_missing_native_session_ends_the_run_as_failed() {
        let line = r#"{"type":"result","subtype":"error_during_execution","is_error":true,"num_turns":0,"session_id":"326f0edb-57c5-40e9-8560-99ae95e9027e"}"#;

        assert_eq!(
            parse_stream_line(line),
            [AdapterEvent::Error { message: "error_during_execution".into() }, AdapterEvent::Exited { success: false }]
        );
    }

    const NPM_SHIM: &str = r"C:\Users\Jo Smith\AppData\Roaming\npm\claude.cmd";

    fn batch_adapter(dir: &Path, program: &str) -> ClaudeCodeAdapter {
        ClaudeCodeAdapter::new(program.into(), "C:\\Baton\\bin\\baton-mcp-shim.exe".into(), dir, Vec::new())
    }

    /// npm 安裝的 `claude.cmd` 不能直接以 CreateProcessW 啟動（審查 R152）：經 cmd.exe 的 `call` 執行，
    /// claude 收到的引數與直接啟動 `claude.exe` 時相同。
    #[test]
    fn an_npm_installed_claude_cmd_is_launched_through_cmd_exe_with_the_same_claude_arguments() {
        let data = tempfile::tempdir().unwrap();
        let spec = run_spec(None, Some("sonnet"));

        let launched = batch_adapter(data.path(), NPM_SHIM).spawn_spec(&spec);
        let direct = adapter(data.path(), &[]).spawn_spec(&spec);

        assert!(launched.program.to_ascii_lowercase().ends_with(r"\system32\cmd.exe"), "{}", launched.program);
        assert_eq!(&launched.args[..5], ["/d", "/v:off", "/c", "call", NPM_SHIM]);
        assert_eq!(&launched.args[5..], &direct.args[..], "claude 的引數不變");
        assert_eq!(SpawnSpec { program: direct.program.clone(), args: direct.args.clone(), ..launched.clone() }, direct);
        assert_eq!(batch_adapter(data.path(), NPM_SHIM).spawn_spec(&spec), launched, "同一個 RunSpec 永遠得到同一個啟動指令");
    }

    #[tokio::test]
    async fn a_batch_launch_refuses_arguments_that_cmd_would_reinterpret() {
        let data = tempfile::tempdir().unwrap();
        let batch = batch_adapter(data.path(), NPM_SHIM);

        for model in ["sonnet&calc", "a%PATH%b", "x^y", "a|b", "a>b", "a<b", "a\"b", "a!b", "(x)", "a\nb"] {
            let error = batch.prepare(&run_spec(None, Some(model))).await.unwrap_err();
            assert!(error.to_string().contains("cmd.exe"), "{model:?}：{error}");
        }
        let odd_path = batch_adapter(data.path(), r"C:\Users\Tom&Jerry\npm\claude.cmd");
        assert!(odd_path.prepare(&run_spec(None, None)).await.is_err(), "批次檔路徑本身也要檢查");
        batch.prepare(&run_spec(None, Some("claude-opus-5-5"))).await.unwrap();
        adapter(data.path(), &[]).prepare(&run_spec(None, Some("sonnet&calc"))).await.unwrap();
    }

    /// 真的經 cmd.exe 啟動一個模擬 npm 的 `claude.cmd`（路徑含空白）：它收到的引數與標準輸入都和預期相同。
    #[cfg(windows)]
    #[tokio::test]
    async fn a_claude_cmd_receives_the_arguments_and_the_prompt_when_launched_through_the_job_object() {
        use baton_ports::{Liveness, ProcessControl};
        let temp = tempfile::tempdir().unwrap();
        let npm = temp.path().join("npm dir");
        std::fs::create_dir_all(&npm).unwrap();
        let shim = npm.join("claude.cmd");
        std::fs::write(&shim, "@echo off\r\n> \"%~dp0args.txt\" echo(%*\r\nmore > \"%~dp0stdin.txt\"\r\n").unwrap();
        let data = temp.path().join("baton data");
        let snapshot = data.join("run-settings").join("run_1");
        std::fs::create_dir_all(&snapshot).unwrap();
        for name in [SETTINGS_FILE, MCP_FILE, INSTRUCTIONS_FILE] {
            std::fs::write(snapshot.join(name), "{}").unwrap();
        }
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let spec = RunSpec {
            prompt: "do the work".into(),
            command_id: format!("test-claude-cmd-{}-{nanos}", std::process::id()),
            working_dir: Some(temp.path().to_string_lossy().into_owned()),
            ..run_spec(Some(&snapshot), Some("sonnet"))
        };
        let adapter = batch_adapter(&data, &shim.to_string_lossy());
        adapter.prepare(&spec).await.unwrap();
        let spawn = adapter.spawn_spec(&spec);
        let control = crate::windows_process::WindowsProcessControl::new();

        let receipt = control.spawn(spawn.clone()).await.unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while control.inspect(&receipt.identity).await.unwrap() != Liveness::Dead {
            assert!(std::time::Instant::now() < deadline, "模擬的 claude.cmd 沒有結束");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        // cmd 原樣交給批次檔的引數列：含空白或空字串的引數加引號，其餘不變
        let quoted: Vec<String> =
            spawn.args[5..].iter().map(|a| if a.is_empty() || a.contains(' ') { format!("\"{a}\"") } else { a.clone() }).collect();
        let received = std::fs::read_to_string(npm.join("args.txt")).unwrap();
        assert_eq!(received.trim_end(), quoted.join(" "));
        assert!(received.contains("baton data"), "路徑含空白的引數完整送達：{received}");
        assert_eq!(std::fs::read_to_string(npm.join("stdin.txt")).unwrap().trim_end(), "do the work");
    }

    #[test]
    fn auth_affecting_variables_are_recognized_case_insensitively() {
        assert!(affects_auth("ANTHROPIC_BASE_URL") && affects_auth("Claude_Code_Use_Vertex") && affects_auth("CLAUDECODE"));
        assert!(!affects_auth("CLAUDE_CODE_GIT_BASH_PATH") && !affects_auth("PATH") && !affects_auth("BATON_AGENT_TOKEN"));
    }
}
