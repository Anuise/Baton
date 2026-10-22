use async_trait::async_trait;
use futures::stream::BoxStream;

use crate::PortResult;

/// 啟動一個 Run 所需的描述。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunSpec {
    pub run_id: String,
    pub prompt: String,
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
    Message { text: String },
    RateLimited { reset_at_millis: Option<i64> },
    Exited { success: bool },
}

#[async_trait]
pub trait AgentAdapter: Send + Sync {
    async fn launch(&self, spec: RunSpec) -> PortResult<AgentHandle>;
    fn events(&self, handle: &AgentHandle) -> BoxStream<'static, AdapterEvent>;
    async fn resume_capability(&self, session: &SessionRef) -> PortResult<Capability>;
}
