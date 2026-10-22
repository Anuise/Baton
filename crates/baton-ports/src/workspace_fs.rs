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
    /// 把一組檔案寫成唯讀目錄（Run 專屬的設定快照，spec D-09）：目錄已存在時整個取代；
    /// 先寫到同層的暫存目錄、檔案設為唯讀後才改名，讀取端不會看到寫到一半的目錄。
    async fn write_readonly_files(&self, dir: &Path, files: &[(String, Vec<u8>)]) -> PortResult<()>;
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

/// 把檔案寫成唯讀目錄；正式版與測試用的 fake 共用（寫檔本來就是真實檔案系統的事）。
///
/// `files` 的名稱只能是單純的檔名（不可含路徑分隔字元或 `..`），避免寫到目錄之外。
pub fn write_readonly_tree(dir: &Path, files: &[(String, Vec<u8>)]) -> PortResult<()> {
    use std::io::Write;

    let io = |path: &Path, e: std::io::Error| PortError::Failed(format!("無法寫入 {}：{e}", path.display()));
    let name = dir
        .file_name()
        .ok_or_else(|| PortError::Failed(format!("{} 不是有效的目錄路徑", dir.display())))?
        .to_string_lossy()
        .into_owned();
    let staging = dir.with_file_name(format!("{name}.staging"));
    remove_tree(&staging)?;
    std::fs::create_dir_all(&staging).map_err(|e| io(&staging, e))?;
    for (file_name, bytes) in files {
        if file_name.is_empty() || file_name == ".." || file_name.contains(['/', '\\']) {
            return Err(PortError::Failed(format!("快照檔名不合法：{file_name}")));
        }
        let path = staging.join(file_name);
        let mut file = std::fs::File::create(&path).map_err(|e| io(&path, e))?;
        file.write_all(bytes).map_err(|e| io(&path, e))?;
        file.sync_all().map_err(|e| io(&path, e))?;
        drop(file);
        set_readonly(&path, true).map_err(|e| io(&path, e))?;
    }
    remove_tree(dir)?;
    std::fs::rename(&staging, dir).map_err(|e| io(dir, e))?;
    set_readonly(dir, true).map_err(|e| io(dir, e))
}

fn set_readonly(path: &Path, readonly: bool) -> std::io::Result<()> {
    let mut permissions = std::fs::metadata(path)?.permissions();
    permissions.set_readonly(readonly);
    std::fs::set_permissions(path, permissions)
}

/// 刪除目錄樹；先解除唯讀（Windows 不能刪唯讀檔），不存在時不是錯誤。
fn remove_tree(path: &Path) -> PortResult<()> {
    if !path.exists() {
        return Ok(());
    }
    let failed = |e: std::io::Error| PortError::Failed(format!("無法移除 {}：{e}", path.display()));
    set_readonly(path, false).map_err(failed)?;
    for entry in std::fs::read_dir(path).map_err(failed)? {
        let entry_path = entry.map_err(failed)?.path();
        set_readonly(&entry_path, false).map_err(failed)?;
    }
    std::fs::remove_dir_all(path).map_err(failed)
}
