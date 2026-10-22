use async_trait::async_trait;
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
    /// 指定的模型（Workspace 依角色覆寫，spec D-30）；`None` 時沿用帳號的預設模型。
    pub model: Option<String>,
}

/// 已啟動 Run 的控制代碼。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AgentHandle(pub String);

/// 原生 Session 的參照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRef {
    pub native_session_id: String,
}

/// Adapter 能否以原生方式接續某個 Session。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    Resumable,
    NotResumable,
    Unknown,
}

/// Adapter 回報的事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdapterEvent {
    Started { native_session_id: Option<String> },
    /// 存活訊號；每一次都續約該 Run 的 execution lease（spec D-31）。
    Heartbeat,
    Message { text: String },
    RateLimited { reset_at_millis: Option<i64> },
    /// Agent 回報的錯誤（例如 API 錯誤、未登入）；緊接著的 `Exited { success: false }` 以它作為失敗原因。
    Error { message: String },
    Exited { success: bool },
}

#[async_trait]
pub trait AgentAdapter: Send + Sync {
    /// 這個 Run 的啟動指令。純函式：同一個 `RunSpec` 永遠得到同一個結果，Runtime 重建後據此 `find` 原程序。
    fn spawn_spec(&self, spec: &RunSpec) -> SpawnSpec;
    /// 啟動程序之前準備這個 Run 需要的檔案（例如 Prompt、MCP 設定）；重複呼叫安全。
    async fn prepare(&self, _spec: &RunSpec) -> PortResult<()> {
        Ok(())
    }
    /// 程序已啟動後，建立事件來源（控制代碼只由 `run_id` 決定，重複呼叫安全）。
    async fn launch(&self, spec: RunSpec) -> PortResult<AgentHandle>;
    fn events(&self, handle: &AgentHandle) -> BoxStream<'static, AdapterEvent>;
    async fn resume_capability(&self, session: &SessionRef) -> PortResult<Capability>;
}
