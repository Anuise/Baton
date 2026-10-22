use std::path::Path;

use async_trait::async_trait;

use crate::{PortError, PortResult};

/// 工作目錄是否有未保存的變更。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirtyState {
    Clean,
    Dirty,
}

/// 使用者指定的目錄實際上是什麼。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectoryState {
    Directory,
    NotFound,
    /// 路徑存在但不是目錄（例如檔案）。
    NotADirectory,
}

#[async_trait]
pub trait WorkspaceFs: Send + Sync {
    /// 建立 Workspace 前確認目錄存在；目錄不必是 Git repo。
    async fn inspect_directory(&self, path: &Path) -> PortResult<DirectoryState>;
    /// 建立目錄（含上層）；已存在時不是錯誤。用於 Profile 專屬的設定目錄。
    async fn ensure_directory(&self, path: &Path) -> PortResult<()>;
    async fn create_worktree(&self, repo: &Path, branch: &str, target: &Path) -> PortResult<()>;
    async fn remove_worktree(&self, target: &Path) -> PortResult<()>;
    async fn inspect_dirty(&self, target: &Path) -> PortResult<DirtyState>;
    /// 先寫暫存、fsync、再原子改名（spec D-17）。
    async fn write_artifact_atomic(&self, path: &Path, bytes: &[u8]) -> PortResult<()>;
}

/// 檢查路徑目前是什麼；正式版與測試用的 fake 共用，因為「目錄存在嗎」本來就是真實檔案系統的事實。
pub fn directory_state(path: &Path) -> PortResult<DirectoryState> {
    match std::fs::metadata(path) {
        Ok(meta) if meta.is_dir() => Ok(DirectoryState::Directory),
        Ok(_) => Ok(DirectoryState::NotADirectory),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(DirectoryState::NotFound),
        Err(e) => Err(PortError::Failed(format!("無法讀取 {}：{e}", path.display()))),
    }
}
