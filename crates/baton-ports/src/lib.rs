//! 所有外部副作用的 Port 介面（spec D-04）。
//!
//! Runtime 只依賴這些 trait；正式實作與測試用的 fake 都在別處提供。
//! 型別形狀依 spec 的「Port」草圖；欄位只放目前需要的最小集合，後續票再擴充。

mod agent;
mod clock;
mod platform;
mod process;
mod quota;
mod secret;
mod workspace_fs;

use std::sync::Arc;

pub use agent::{AdapterEvent, AgentAdapter, AgentHandle, Capability, RunSpec, SessionRef};
pub use clock::Clock;
pub use platform::{OsFamily, PlatformInfo, PlatformSnapshot};
pub use process::{Liveness, ProcessControl, ProcessIdentity, Receipt, SpawnSpec, StopResult};
pub use quota::{Confidence, ProfileRef, QuotaObserver, QuotaSample, QuotaSource};
pub use secret::SecretProtector;
pub use workspace_fs::{directory_state, DirectoryState, DirtyState, WorkspaceFs};

/// Port 呼叫的共同錯誤。
#[derive(Debug, thiserror::Error)]
pub enum PortError {
    /// 該 Port 的正式實作尚未提供（後續票補上）。
    #[error("尚未實作：{0}")]
    NotImplemented(&'static str),
    #[error("{0}")]
    Failed(String),
}

pub type PortResult<T> = Result<T, PortError>;

/// Runtime 需要的全部 Port；`Clone` 只複製參照，重建 Runtime 時可共用同一組 fake。
#[derive(Clone)]
pub struct Ports {
    pub clock: Arc<dyn Clock>,
    pub process_control: Arc<dyn ProcessControl>,
    pub agent_adapter: Arc<dyn AgentAdapter>,
    pub quota_observer: Arc<dyn QuotaObserver>,
    pub workspace_fs: Arc<dyn WorkspaceFs>,
    pub platform_info: Arc<dyn PlatformInfo>,
    pub secret_protector: Arc<dyn SecretProtector>,
}
