use std::path::Path;

use async_trait::async_trait;

use crate::PortResult;

/// 工作目錄是否有未保存的變更。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirtyState {
    Clean,
    Dirty,
}

#[async_trait]
pub trait WorkspaceFs: Send + Sync {
    async fn create_worktree(&self, repo: &Path, branch: &str, target: &Path) -> PortResult<()>;
    async fn remove_worktree(&self, target: &Path) -> PortResult<()>;
    async fn inspect_dirty(&self, target: &Path) -> PortResult<DirtyState>;
    /// 先寫暫存、fsync、再原子改名（spec D-17）。
    async fn write_artifact_atomic(&self, path: &Path, bytes: &[u8]) -> PortResult<()>;
}
