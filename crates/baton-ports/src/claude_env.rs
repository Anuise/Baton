//! ClaudeEnvironment Port：本機的 Claude Code 與 Git 環境（spec D-20）。
//!
//! 只回報事實與開啟登入終端機；沒有任何下載或安裝的能力，缺項只能由使用者自己處理。

use std::path::Path;

use async_trait::async_trait;

pub use baton_domain::profile::{AuthReport, AuthSource};
pub use baton_domain::setup::ToolProbe;

use crate::{PortError, PortResult, ProcessIdentity, SpawnSpec};

/// 開啟 Controller 終端機的完整啟動規格（desktop-controller-cli 票 03）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TerminalLaunch {
    /// 在終端機中執行的程式、參數、環境變數增刪、工作目錄與 `command_id`（Controller 宿主）。
    /// 標準輸入輸出欄位不使用：宿主以新主控台的輸入輸出與使用者互動。
    pub spawn: SpawnSpec,
    /// 視窗標題。
    pub title: String,
}

#[async_trait]
pub trait ClaudeEnvironment: Send + Sync {
    /// Claude Code 是否在 PATH 上，以及 `--version` 的輸出。
    async fn probe_claude(&self) -> PortResult<ToolProbe>;
    /// Git for Windows 是否在 PATH 上，以及 `--version` 的輸出。
    async fn probe_git(&self) -> PortResult<ToolProbe>;
    /// 以 `config_dir` 作為 `CLAUDE_CONFIG_DIR`，核對 Claude Code 實際使用的認證來源與帳號身分。
    ///
    /// 沿用 Runtime 目前的環境變數，所以環境變數或 credential helper 造成的覆蓋會如實回報。
    /// 回傳的內容不含任何 token。
    async fn auth_status(&self, config_dir: &Path) -> PortResult<AuthReport>;
    /// 開啟一個終端機視窗，在其中以 `config_dir` 作為 `CLAUDE_CONFIG_DIR` 執行原生登入。
    async fn open_login_terminal(&self, config_dir: &Path) -> PortResult<()>;
    /// 在新的終端機視窗執行 `launch.spawn`（Controller 宿主），回傳宿主的程序身分（雜湊為 `command_hash(&launch.spawn)`）。
    ///
    /// 宿主與它的子孫程序要放進以 `command_id` 命名、與 `ProcessControl::spawn` 相同的程序群組，
    /// 之後 Runtime 以 `ProcessControl::find(&launch.spawn)` 找回、`inspect` 查驗存活、`stop` 結束整棵程序樹；
    /// 同一個 `command_id` 已有程序時拒絕，不開第二個視窗。
    async fn open_controller_terminal(&self, launch: &TerminalLaunch) -> PortResult<ProcessIdentity> {
        let _ = launch;
        Err(PortError::NotImplemented("開啟 Controller 終端機"))
    }
}
