//! harness 的暫存資料目錄（review-notes #28）。
//!
//! Runtime 的背景工作在測試的 tokio runtime 結束前仍開著控制資料庫與日誌；Windows 上開著的檔案不能刪除，
//! 所以 `TempDir` 在 harness 被丟棄的當下刪除會失敗並靜默留下目錄。這裡改為：
//! - harness 丟棄時記到待刪清單，由背景執行緒重試刪除，等 runtime 結束、檔案關閉後就刪得掉；
//! - 程序結束時（`atexit`，各測試的 runtime 都已結束）同步重試刪除清單中剩下的目錄（例如最後一個測試的）；
//! - 程序沒有正常結束（被終止）而留下的，由之後任何一個 harness 啟動時清掉。
//!
//! 目錄都放在暫存區的 `baton-harness` 之下，每個目錄旁有一個鎖檔（`<目錄名>.lock`）。持有鎖檔的程序
//! 還在使用或正在刪除那個目錄，其他程序不碰；Windows 上以不共用的方式開啟，程序結束時自動釋放。

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, Once, PoisonError};
use std::time::{Duration, Instant};

/// 背景重試刪除的上限；超過就等程序結束時再試。
const REMOVE_RETRY_LIMIT: Duration = Duration::from_secs(600);
/// 程序結束時同步重試刪除的上限；超過就留給之後的 harness 清理。
const EXIT_RETRY_LIMIT: Duration = Duration::from_secs(10);

/// 已丟棄、還沒刪掉的資料目錄與它們的鎖檔。
static PENDING: Mutex<Vec<(PathBuf, Option<File>)>> = Mutex::new(Vec::new());
static REGISTER_AT_EXIT: Once = Once::new();

pub(crate) struct DataDir {
    path: PathBuf,
    /// 持有期間其他程序不會清掉這個目錄；交給背景刪除時一起移交。
    lock: Option<File>,
}

impl DataDir {
    pub(crate) fn create() -> io::Result<Self> {
        let root = std::env::temp_dir().join("baton-harness");
        fs::create_dir_all(&root)?;
        sweep_abandoned(&root);
        let path = tempfile::Builder::new().prefix("h").tempdir_in(&root)?.keep();
        let lock = open_lock(&lock_path(&path), true)?;
        Ok(Self { path, lock: Some(lock) })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for DataDir {
    fn drop(&mut self) {
        let path = std::mem::take(&mut self.path);
        let lock = self.lock.take();
        if fs::remove_dir_all(&path).is_ok() {
            release(&path, lock);
            return;
        }
        // 還有開著的檔案（Runtime 的背景工作要等測試的 tokio runtime 結束才會關閉）：記到待刪清單，背景重試
        REGISTER_AT_EXIT.call_once(|| unsafe {
            libc::atexit(remove_pending_at_exit);
        });
        PENDING.lock().unwrap_or_else(PoisonError::into_inner).push((path, lock));
        std::thread::spawn(|| {
            let deadline = Instant::now() + REMOVE_RETRY_LIMIT;
            let mut wait = Duration::from_millis(10);
            while Instant::now() < deadline {
                std::thread::sleep(wait);
                if remove_pending() {
                    return;
                }
                wait = (wait * 2).min(Duration::from_secs(1));
            }
        });
    }
}

/// 試著刪除待刪清單中的目錄，刪掉的移出清單；回傳清單是否已清空。
fn remove_pending() -> bool {
    let mut pending = PENDING.lock().unwrap_or_else(PoisonError::into_inner);
    let mut i = 0;
    while i < pending.len() {
        let path = &pending[i].0;
        if fs::remove_dir_all(path).is_ok() || !path.exists() {
            let (path, lock) = pending.swap_remove(i);
            release(&path, lock);
        } else {
            i += 1;
        }
    }
    pending.is_empty()
}

/// 程序結束時執行：這時各測試的 tokio runtime 都已結束，剩下的檔案很快就會關閉。
extern "C" fn remove_pending_at_exit() {
    let deadline = Instant::now() + EXIT_RETRY_LIMIT;
    while !remove_pending() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn lock_path(dir: &Path) -> PathBuf {
    let mut name = dir.file_name().unwrap_or_default().to_os_string();
    name.push(".lock");
    dir.with_file_name(name)
}

/// 以不共用的方式開啟鎖檔：別的程序持有時失敗。
fn open_lock(path: &Path, create: bool) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    if create {
        options.create_new(true);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(0);
    }
    options.open(path)
}

/// 目錄已刪除：放掉並刪除鎖檔。
fn release(dir: &Path, lock: Option<File>) {
    drop(lock);
    let _ = fs::remove_file(lock_path(dir));
}

/// 清掉之前的程序沒刪完的目錄：鎖檔已沒有人持有的，就是沒人在用。
fn sweep_abandoned(root: &Path) {
    // 只有 Windows 的鎖檔是獨占的；其他平台開著的檔案本來就刪得掉，不會留下目錄
    if !cfg!(windows) {
        return;
    }
    let Ok(entries) = fs::read_dir(root) else { return };
    for entry in entries.flatten() {
        let lock = entry.path();
        if lock.extension().is_none_or(|ext| ext != "lock") {
            continue;
        }
        let Ok(handle) = open_lock(&lock, false) else { continue };
        let dir = lock.with_extension("");
        if fs::remove_dir_all(&dir).is_ok() || !dir.exists() {
            drop(handle);
            let _ = fs::remove_file(&lock);
        }
    }
}
