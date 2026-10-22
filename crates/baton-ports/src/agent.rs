use async_trait::async_trait;
use baton_domain::quota::{LimitStatus, QuotaWindow};
use futures::stream::BoxStream;

use crate::{PortResult, SpawnSpec};

/// 啟動一個 Run 所需的描述。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunSpec {
    pub run_id: String,
    pub task_id: String,
    /// 與 `SpawnSpec::command_id` 相同，Adapter 組出的啟動指令必須帶著它。
    pub command_id: String,
    pub prompt: String,
    pub working_dir: Option<String>,
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
    UsageReported { tokens: u64, raw: String },
    Exited { success: bool },
}

#[async_trait]
pub trait AgentAdapter: Send + Sync {
    /// 這個 Run 的啟動指令。純函式：同一個 `RunSpec` 永遠得到同一個結果，Runtime 重建後據此 `find` 原程序。
    fn spawn_spec(&self, spec: &RunSpec) -> SpawnSpec;
    /// 程序已啟動後，建立事件來源（控制代碼只由 `run_id` 決定，重複呼叫安全）。
    async fn launch(&self, spec: RunSpec) -> PortResult<AgentHandle>;
    fn events(&self, handle: &AgentHandle) -> BoxStream<'static, AdapterEvent>;
    async fn resume_capability(&self, session: &SessionRef) -> PortResult<Capability>;
}
