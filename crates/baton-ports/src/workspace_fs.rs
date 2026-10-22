use std::io::Write;
use std::path::{Path, PathBuf};

use async_trait::async_trait;

use baton_domain::artifact::STAGING_SUFFIX;

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

/// 成果儲存內的一個檔案。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    pub path: PathBuf,
    pub size_bytes: u64,
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
    ///
    /// 暫存檔就在目標旁邊，檔名是 [`partial_path`]；寫到一半中斷只會留下暫存檔，目標路徑要嘛不存在、要嘛內容完整。
    async fn write_artifact_atomic(&self, path: &Path, bytes: &[u8]) -> PortResult<()>;
    /// 讀取成果檔案；檔案不存在回 `None`。
    async fn read_artifact(&self, path: &Path) -> PortResult<Option<Vec<u8>>>;
    /// 遞迴列出目錄下的全部檔案（含暫存檔）；目錄不存在回空清單。
    async fn list_artifact_files(&self, dir: &Path) -> PortResult<Vec<FileEntry>>;
}

/// 成果寫入時的暫存檔路徑：目標檔名加上 [`STAGING_SUFFIX`]（與目標在同一個目錄，才能原子改名）。
pub fn partial_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(STAGING_SUFFIX);
    path.with_file_name(name)
}

/// 先寫暫存、fsync、再原子改名；正式版使用（fake 在各步驟之間插入鉤子，但順序相同）。
pub fn write_file_atomic(path: &Path, bytes: &[u8]) -> PortResult<()> {
    let fail = |what: &str, e: std::io::Error| PortError::Failed(format!("{what} {}：{e}", path.display()));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| fail("無法建立成果目錄", e))?;
    }
    let temp = partial_path(path);
    let mut file = std::fs::File::create(&temp).map_err(|e| fail("無法建立暫存檔", e))?;
    file.write_all(bytes).map_err(|e| fail("無法寫入暫存檔", e))?;
    file.sync_all().map_err(|e| fail("無法 fsync 暫存檔", e))?;
    drop(file);
    std::fs::rename(&temp, path).map_err(|e| fail("無法改名為成果檔", e))
}

/// 讀取檔案；不存在回 `None`。
pub fn read_file_if_exists(path: &Path) -> PortResult<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(PortError::Failed(format!("無法讀取 {}：{e}", path.display()))),
    }
}

/// 遞迴列出目錄下的全部檔案（依路徑排序）；目錄不存在回空清單。
pub fn list_files_recursive(dir: &Path) -> PortResult<Vec<FileEntry>> {
    fn walk(dir: &Path, found: &mut Vec<FileEntry>) -> PortResult<()> {
        let fail = |e: std::io::Error| PortError::Failed(format!("無法列出 {}：{e}", dir.display()));
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(fail(e)),
        };
        for entry in entries {
            let entry = entry.map_err(fail)?;
            let meta = entry.metadata().map_err(fail)?;
            if meta.is_dir() {
                walk(&entry.path(), found)?;
            } else {
                found.push(FileEntry { path: entry.path(), size_bytes: meta.len() });
            }
        }
        Ok(())
    }
    let mut found = Vec::new();
    walk(dir, &mut found)?;
    found.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(found)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_leaves_complete_content_and_no_staging_file() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("ws").join("blob_1");
        write_file_atomic(&target, b"hello").unwrap();

        assert_eq!(read_file_if_exists(&target).unwrap().as_deref(), Some(&b"hello"[..]));
        assert!(!partial_path(&target).exists());
        let listed = list_files_recursive(dir.path()).unwrap();
        assert_eq!(listed, vec![FileEntry { path: target, size_bytes: 5 }]);
    }

    #[test]
    fn staging_path_sits_next_to_the_target_with_the_staging_suffix() {
        let path = Path::new("a").join("blob_1");
        assert_eq!(partial_path(&path), Path::new("a").join("blob_1.partial"));
    }

    #[test]
    fn missing_file_and_directory_are_not_errors() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_file_if_exists(&dir.path().join("nope")).unwrap(), None);
        assert!(list_files_recursive(&dir.path().join("nope")).unwrap().is_empty());
    }
}
