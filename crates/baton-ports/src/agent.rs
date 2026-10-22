use async_trait::async_trait;
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdapterEvent {
    Started { native_session_id: Option<String> },
    /// 存活訊號；每一次都續約該 Run 的 execution lease（spec D-31）。
    Heartbeat,
    Message { text: String },
    RateLimited { reset_at_millis: Option<i64> },
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
