use async_trait::async_trait;
use baton_domain::quota::{LimitStatus, QuotaWindow};
use futures::stream::BoxStream;

use crate::{PortResult, SpawnSpec};

/// 啟動一個 Run 所需的描述。
///
/// 全部欄位都由持久化的狀態決定（派工 Outbox、Run 的帳號、設定快照的綁定），
/// 所以 Runtime 重建後組出的 `RunSpec` 與啟動時完全相同。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunSpec {
    pub run_id: String,
    pub task_id: String,
    /// 與 `SpawnSpec::command_id` 相同，Adapter 組出的啟動指令必須帶著它。
    pub command_id: String,
    pub prompt: String,
    pub working_dir: Option<String>,
    /// 這個 Run 的帳號 Profile 的 `CLAUDE_CONFIG_DIR`（只放原生登入與 Session 狀態，spec D-09）。
    pub config_dir: Option<String>,
    /// 這個 Run 准入、已核對過的共同設定快照目錄；沒有採用任何共同設定版本時為 `None`。
    pub settings_dir: Option<String>,
    /// 快照中實際有的設定檔名稱（`settings.json`、`mcp.json`、`CLAUDE.md` 中有內容的類別）。
    /// 由綁定的版本渲染決定、不讀檔案系統，所以快照目錄在 Run 啟動後被刪，Runtime 重建後組出的啟動指令仍相同。
    pub settings_files: Vec<String>,
    /// 指定的模型（Workspace 依角色覆寫，spec D-30）；`None` 時沿用帳號的預設模型。
    pub model: Option<String>,
    /// 要以原生方式接續的原生 Session（票 15）；`None` 時建立新的原生 Session。
    /// 在寫入 launch intent 的同一個交易中決定並持久化在 Run 上，所以 Runtime 重建後組出相同的啟動指令。
    pub resume_native_session_id: Option<String>,
    /// 互動式 Controller CLI 以 `--continue` 接續工作目錄在這個帳號下最近的原生 Session（desktop-controller-cli 票 07）。
    /// 開啟時決定並持久化在 Controller Run 上；Worker 一律為 `false`。
    pub continue_session: bool,
}

/// 已啟動 Run 的控制代碼。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AgentHandle(pub String);

/// 原生 Session 的參照：原生識別只在同一個執行工具、帳號與 Session 儲存命名空間內唯一，
/// 所以查驗時一併帶上帳號的 `CLAUDE_CONFIG_DIR` 與當初的工作目錄。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRef {
    pub native_session_id: String,
    pub config_dir: Option<String>,
    pub working_dir: Option<String>,
}

/// Adapter 能否以原生方式接續某個 Session。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    Resumable,
    NotResumable,
    Unknown,
}

/// Adapter 回報的事件。
#[derive(Debug, Clone, PartialEq)]
pub enum AdapterEvent {
    Started { native_session_id: Option<String> },
    /// 存活訊號；每一次都續約該 Run 的 execution lease（spec D-31）。
    Heartbeat,
    Message { text: String },
    /// CLI 串流或錯誤中的限流訊號（「已觀測」來源，spec D-12）；觀測不到的欄位為 `None`。
    /// `used` 是 CLI 回報的已用比例（0.0–1.0），`raw` 是原始事件內容（保存為原始資料）。
    RateLimited {
        window: Option<QuotaWindow>,
        status: LimitStatus,
        used: Option<f64>,
        reset_at_millis: Option<i64>,
        raw: String,
    },
    /// Run 結果訊息回報的用量（「推估」來源）：這一輪用掉的 token 數與原始內容。
    /// `context_tokens` 是模型讀入的 Context token（觀測指標用，票 41）；結果訊息沒有對應欄位時為 `None`。
    UsageReported { tokens: u64, context_tokens: Option<u64>, raw: String },
    /// 原生對話紀錄的一筆（票 15）：`position` 是這個 Run 內從 1 起、單調遞增的事件位置，
    /// Runtime 以它去重與找出缺口；`line` 是原始內容（匯入時才遮蔽秘密）。重新接上串流時可能重播已送過的位置。
    Transcript { position: u64, line: String },
    /// Agent 回報的錯誤（例如 API 錯誤、未登入）；緊接著的 `Exited { success: false }` 以它作為失敗原因。
    Error { message: String },
    Exited { success: bool },
}

#[async_trait]
pub trait AgentAdapter: Send + Sync {
    /// 這個 Run 的啟動指令。純函式：同一個 `RunSpec` 永遠得到同一個結果，Runtime 重建後據此 `find` 原程序。
    fn spawn_spec(&self, spec: &RunSpec) -> SpawnSpec;
    /// 互動式 Controller CLI 的啟動指令（desktop-controller-cli 票 03）：與 `spawn_spec` 共用環境與設定的組裝，
    /// 但不以 `-p` 執行；程式是 MCP shim 的「Controller 宿主」子命令（`args[0]`），其後是它要啟動的 `claude` 與引數。
    /// 同樣是純函式，Runtime 重建後據此找回宿主。
    fn controller_spawn_spec(&self, spec: &RunSpec) -> SpawnSpec;
    /// 啟動程序之前準備這個 Run 需要的檔案（例如 Prompt、MCP 設定）；重複呼叫安全。
    async fn prepare(&self, _spec: &RunSpec) -> PortResult<()> {
        Ok(())
    }
    /// 程序已啟動後，建立事件來源（控制代碼只由 `run_id` 決定，重複呼叫安全）。
    async fn launch(&self, spec: RunSpec) -> PortResult<AgentHandle>;
    fn events(&self, handle: &AgentHandle) -> BoxStream<'static, AdapterEvent>;
    /// 能否以原生方式接續這個 Session；判斷不了時回 `Unknown`（Runtime 不會把 `Unknown` 當成可接續）。
    async fn resume_capability(&self, session: &SessionRef) -> PortResult<Capability>;
}
