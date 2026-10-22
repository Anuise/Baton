//! UserSettingsSource Port：唯讀讀取使用者平常使用的 Claude Code 設定（spec D-09、使用者故事 6）。
//!
//! 只讀白名單內的三個檔案（`settings.json`、`CLAUDE.md`、`.claude.json`）；沒有寫入能力，
//! 也不會碰 `.credentials.json`。使用者的原始設定檔因此在匯入前後位元組相同。

use std::path::{Path, PathBuf};

use async_trait::async_trait;

pub use baton_domain::shared_settings::{SourceFile, SourceFiles};

use crate::{PortError, PortResult};

#[async_trait]
pub trait UserSettingsSource: Send + Sync {
    /// 讀取來源檔；不存在的檔案以 `bytes: None` 回報，不是錯誤。
    async fn read(&self) -> PortResult<SourceFiles>;
}

/// 從本機目錄唯讀取得來源檔。正式版與測試用的 fake 共用，因為「讀檔」本來就是真實檔案系統的事實。
pub struct DirUserSettingsSource {
    /// 放 `settings.json` 與 `CLAUDE.md` 的目錄（通常是 `~/.claude`）。
    settings_dir: PathBuf,
    /// `.claude.json`（通常是 `~/.claude.json`；設定了 `CLAUDE_CONFIG_DIR` 時在該目錄之下）。
    user_state_file: PathBuf,
}

impl DirUserSettingsSource {
    pub fn new(settings_dir: impl Into<PathBuf>, user_state_file: impl Into<PathBuf>) -> Self {
        Self { settings_dir: settings_dir.into(), user_state_file: user_state_file.into() }
    }
}

#[async_trait]
impl UserSettingsSource for DirUserSettingsSource {
    async fn read(&self) -> PortResult<SourceFiles> {
        Ok(SourceFiles {
            settings: read_optional(&self.settings_dir.join("settings.json"))?,
            instructions: read_optional(&self.settings_dir.join("CLAUDE.md"))?,
            user_state: read_optional(&self.user_state_file)?,
        })
    }
}

fn read_optional(path: &Path) -> PortResult<SourceFile> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => Some(bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(PortError::Failed(format!("無法讀取 {}：{e}", path.display()))),
    };
    Ok(SourceFile { path: path.to_string_lossy().into_owned(), bytes })
}
