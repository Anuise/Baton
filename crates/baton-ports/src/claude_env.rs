//! ClaudeEnvironment Port：本機的 Claude Code 與 Git 環境（spec D-20）。
//!
//! 只回報事實與開啟登入終端機；沒有任何下載或安裝的能力，缺項只能由使用者自己處理。

use std::path::Path;

use async_trait::async_trait;

pub use baton_domain::profile::{AuthReport, AuthSource};
pub use baton_domain::setup::ToolProbe;

use crate::PortResult;

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
}
