use std::io::Write;
use std::path::{Path, PathBuf};

use async_trait::async_trait;

use baton_domain::artifact::STAGING_SUFFIX;

use crate::{PortError, PortResult};

/// 移除前核對到的 worktree 實際狀態（票 21）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorktreeState {
    /// 目錄不存在。
    Missing,
    /// `repo` 登記的 worktree（不是主 checkout）：簽出 `branch`（detached HEAD 為 `None`）；
    /// `unsaved` 是未提交的變更與未追蹤（且未被忽略）的檔案，路徑相對於 worktree。
    Registered { branch: Option<String>, unsaved: Vec<String> },
    /// 目錄存在，但不是 `repo` 登記的 worktree，或是主 checkout。
    Foreign,
}

/// 使用者指定的目錄實際上是什麼。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectoryState {
    Directory,
    NotFound,
    /// 路徑存在但不是目錄（例如檔案）。
    NotADirectory,
}

/// 目錄的 Git 狀態（開發 Task 需要 Git Workspace，票 20）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitState {
    /// 不是 Git repo 的頂層工作目錄（找不到 git 執行檔則是錯誤，讓上層顯示原因）。
    NotRepository,
    /// Git repo 的頂層工作目錄；`branch` 是目前簽出的分支，detached HEAD 時是 `None`。
    Repository { branch: Option<String> },
}

/// 在整合目錄中合併的結果（目標分支本身沒有改變）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeOutcome {
    /// 合併成功：`commit` 是整合目錄的合併結果，`target_tip` 是合併當時目標分支所在的 commit。
    Merged { commit: String, target_tip: String },
    /// 有衝突，已放棄合併；`detail` 是衝突的檔案與 git 輸出的摘要。
    Conflict { detail: String },
}

/// 把目標分支前移到整合結果的結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdvanceOutcome {
    /// 目標分支已經在整合結果上（含先前已前移過，冪等）。
    Advanced,
    /// 目標分支已經不在合併當時的 commit（整合期間有其他更新），沒有改變任何東西。
    TargetMoved { actual: String },
    /// 無法前移（例如簽出目標分支的工作目錄有未提交的變更擋住），沒有改變任何東西。
    Blocked { detail: String },
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
    /// 目錄是不是 Git repo 的頂層工作目錄，以及目前簽出的分支（票 20）。
    async fn inspect_git(&self, path: &Path) -> PortResult<GitState>;
    /// 在 `target` 建立 `repo` 的 worktree，並從 `repo` 目前的 HEAD 建立新分支 `branch` 簽出在其中（票 20）。
    /// 冪等：`target` 已經是簽出 `branch` 的 worktree 時直接成功；`branch` 已存在但沒有 worktree 時沿用該分支。
    async fn create_worktree(&self, repo: &Path, branch: &str, target: &Path) -> PortResult<()>;
    /// 在平台專用的整合目錄 `integration_dir`（可能殘留上一次的內容，會被整個重設）以 detached HEAD
    /// 簽出 `target_branch` 目前的 commit，再以 `--no-ff` 合併 `branch`（票 20）。目標分支本身不會改變；
    /// 衝突時放棄合併並回報 `Conflict`。
    async fn merge_for_integration(
        &self,
        repo: &Path,
        target_branch: &str,
        branch: &str,
        integration_dir: &Path,
    ) -> PortResult<MergeOutcome>;
    /// 把 `target_branch` 從 `expected_tip` 以 fast-forward 前移到 `commit`（票 20）；簽出該分支的工作目錄一併更新。
    /// 已經在 `commit` 時回 `Advanced`（冪等）；不在 `expected_tip` 時什麼都不改，回 `TargetMoved`。
    async fn advance_branch(
        &self,
        repo: &Path,
        target_branch: &str,
        expected_tip: &str,
        commit: &str,
    ) -> PortResult<AdvanceOutcome>;
    /// 核對 `target` 是不是 `repo` 登記的 worktree，以及其中未保存的變更與歸屬不明的檔案（票 21）。
    async fn inspect_worktree(&self, repo: &Path, target: &Path) -> PortResult<WorktreeState>;
    /// 移除 `repo` 登記的 worktree `target`（票 21）：先把目錄改名為 `trash`（檔案被占用時在這一步失敗，什麼都不變），
    /// 清掉 Git 的 worktree 登記，再刪除 `trash`。冪等：`target` 已不存在時只收拾 Git 登記與 `trash` 的殘留。
    /// `target` 存在但不是 `repo` 登記的 worktree（或是主 checkout）時拒絕、不刪除任何檔案。不刪除分支。
    async fn remove_worktree(&self, repo: &Path, target: &Path, trash: &Path) -> PortResult<()>;
    /// `repo` 中 `from` 到 `to` 之間變更的檔案（相對 repo 根目錄，`/` 分隔；改名時新舊路徑都列出），票 44。
    /// 不是 Git repo 或任一個 revision 不存在是錯誤：呼叫端據此判定「無法證明不受影響」。
    async fn changed_paths(&self, repo: &Path, from: &str, to: &str) -> PortResult<Vec<String>>;
    /// 先寫暫存、fsync、再原子改名（spec D-17）。
    ///
    /// 暫存檔就在目標旁邊，檔名是 [`partial_path`]；寫到一半中斷只會留下暫存檔，目標路徑要嘛不存在、要嘛內容完整。
    async fn write_artifact_atomic(&self, path: &Path, bytes: &[u8]) -> PortResult<()>;
    /// 把一組檔案寫成唯讀目錄（Run 專屬的設定快照，spec D-09）：目錄已存在時整個取代；
    /// 先寫到同層的暫存目錄、檔案設為唯讀後才改名，讀取端不會看到寫到一半的目錄。
    async fn write_readonly_files(&self, dir: &Path, files: &[(String, Vec<u8>)]) -> PortResult<()>;
    /// 讀取成果檔案；檔案不存在回 `None`。
    async fn read_artifact(&self, path: &Path) -> PortResult<Option<Vec<u8>>>;
    /// 讀取工作目錄內的檔案（驗證的檔案檢查用，票 13）；不存在回 `None`，不是檔案（例如目錄）是錯誤。
    async fn read_workspace_file(&self, path: &Path) -> PortResult<Option<Vec<u8>>>;
    /// 讀取 `root` 之下的相對路徑 `relative`（驗證的檔案檢查用，票 13），規則見 [`read_file_within`]。
    async fn read_file_within(&self, root: &Path, relative: &Path) -> PortResult<ContainedFile>;
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

/// [`read_file_within`] 的結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContainedFile {
    Missing,
    Found(Vec<u8>),
    /// 正規化（解開符號連結與 junction）後不在 `root` 之內；附上實際指向的路徑，內容不讀取。
    OutsideRoot(PathBuf),
}

/// 讀取 `root` 之下的相對路徑 `relative`；正式版與測試用的 fake 共用（審查 R152）。
///
/// 路徑先正規化再確認仍在正規化後的 `root` 之內，經由符號連結或 junction 指向外部的回 `OutsideRoot`；
/// 指向 `root` 之內的連結照常讀取。不存在（含指向不存在目標的連結）回 `Missing`；不是檔案（例如目錄）是錯誤。
pub fn read_file_within(root: &Path, relative: &Path) -> PortResult<ContainedFile> {
    let path = root.join(relative);
    let resolved = match std::fs::canonicalize(&path) {
        Ok(resolved) => resolved,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(ContainedFile::Missing),
        Err(e) => return Err(PortError::Failed(format!("無法解析 {}：{e}", path.display()))),
    };
    let root = std::fs::canonicalize(root)
        .map_err(|e| PortError::Failed(format!("無法解析工作目錄 {}：{e}", root.display())))?;
    if !resolved.starts_with(&root) {
        return Ok(ContainedFile::OutsideRoot(resolved));
    }
    Ok(match read_file_if_exists(&resolved)? {
        Some(bytes) => ContainedFile::Found(bytes),
        None => ContainedFile::Missing,
    })
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

    /// 建立目錄 junction（不需要系統管理員權限）。
    #[cfg(windows)]
    fn junction(link: &Path, target: &Path) {
        let output = std::process::Command::new("cmd").args(["/d", "/c", "mklink", "/J"]).arg(link).arg(target).output().unwrap();
        assert!(output.status.success(), "mklink 失敗：{}", String::from_utf8_lossy(&output.stderr));
    }

    #[cfg(windows)]
    #[test]
    fn a_file_reached_through_a_junction_pointing_outside_the_root_is_not_read() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("work");
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(root.join("inner")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.txt"), "s").unwrap();
        std::fs::write(root.join("inner").join("ok.txt"), "ok").unwrap();
        junction(&root.join("escape"), &outside);
        junction(&root.join("alias"), &root.join("inner"));
        junction(&temp.path().join("work-link"), &root);
        let found = ContainedFile::Found(b"ok".to_vec());

        assert_eq!(read_file_within(&root, Path::new("inner/ok.txt")).unwrap(), found);
        assert_eq!(read_file_within(&root, Path::new("alias/ok.txt")).unwrap(), found, "指向工作目錄內的連結照常讀取");
        assert_eq!(read_file_within(&temp.path().join("work-link"), Path::new("inner/ok.txt")).unwrap(), found, "工作目錄本身經由連結也可以");
        assert_eq!(read_file_within(&root, Path::new("inner/missing.txt")).unwrap(), ContainedFile::Missing);
        let escaped = read_file_within(&root, Path::new("escape/secret.txt")).unwrap();
        let ContainedFile::OutsideRoot(resolved) = escaped else { panic!("{escaped:?}") };
        assert!(resolved.ends_with(Path::new("outside").join("secret.txt")), "{}", resolved.display());
    }
}
