//! 所有外部副作用的 Port 介面（spec D-04）。
//!
//! Runtime 只依賴這些 trait；正式實作與測試用的 fake 都在別處提供。
//! 型別形狀依 spec 的「Port」草圖；欄位只放目前需要的最小集合，後續票再擴充。

mod agent;
mod claude_env;
mod command;
mod clock;
mod git;
mod guardian;
mod login;
mod platform;
mod process;
mod quota;
mod secret;
mod user_settings;
mod workspace_fs;

use std::sync::Arc;

pub use agent::{AdapterEvent, AgentAdapter, AgentHandle, Capability, RunSpec, SessionRef};
pub use claude_env::{AuthReport, AuthSource, ClaudeEnvironment, ToolProbe};
pub use clock::Clock;
pub use command::{CommandOutput, CommandRequest, CommandRunner};
pub use git::{
    git_advance_branch, git_create_worktree, git_inspect, git_inspect_worktree, git_merge_for_integration,
    git_remove_worktree,
};
pub use guardian::GuardianProbe;
pub use login::{LoginSession, StartupRegistration};
pub use platform::{OsFamily, PlatformInfo, PlatformSnapshot};
pub use process::{command_hash, Liveness, ProcessControl, ProcessIdentity, Receipt, SpawnSpec, StopResult};
pub use quota::{Confidence, ProfileRef, QuotaObserver, QuotaSample, QuotaSource};
pub use secret::SecretProtector;
pub use user_settings::{DirUserSettingsSource, SourceFile, SourceFiles, UserSettingsSource};
pub use workspace_fs::{
    directory_state, list_files_recursive, partial_path, read_file_if_exists, write_file_atomic, write_readonly_tree,
    AdvanceOutcome, DirectoryState, FileEntry, GitState, MergeOutcome, WorkspaceFs, WorktreeState,
};

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
    pub claude_env: Arc<dyn ClaudeEnvironment>,
    /// 執行契約宣告的驗證命令（票 13）。
    pub command_runner: Arc<dyn CommandRunner>,
    pub user_settings: Arc<dyn UserSettingsSource>,
    /// 監護程序是否還在（票 32；顯示降級用）。
    pub guardian_probe: Arc<dyn GuardianProbe>,
    /// 目前 Windows 登入工作階段的識別（票 33；判斷是否跨登入啟動）。
    pub login_session: Arc<dyn LoginSession>,
    /// 「登入時自動啟動」的 Windows 登記（票 33；HKCU Run 機碼）。
    pub startup_registration: Arc<dyn StartupRegistration>,
}
