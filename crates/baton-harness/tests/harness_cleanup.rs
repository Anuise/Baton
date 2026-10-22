//! harness 本身的清理：測試結束後不留下暫存資料目錄（review-notes #28）。
//!
//! 每個 harness 的資料目錄含 `control.sqlite`、`logs` 等；Runtime 的背景工作在測試的 tokio runtime
//! 結束前仍開著這些檔案，Windows 上無法在 harness 被丟棄的當下刪除。這裡以手動建立的 runtime
//! 模擬 `#[tokio::test]` 的生命週期，確認 runtime 結束後資料目錄確實被刪除。

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use baton_harness::Harness;

/// 清理在背景進行；等到目錄消失（有上限），逾時就失敗。
fn wait_until_removed(dir: &Path) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if !dir.exists() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    !dir.exists()
}

/// 在一個新的 tokio runtime 中執行，回傳時 runtime 已結束（等同 `#[tokio::test]` 的測試函式結束）。
fn in_test_runtime<F: std::future::Future<Output = PathBuf>>(flavor_multi: bool, body: impl FnOnce() -> F) -> PathBuf {
    let runtime = if flavor_multi {
        tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build()
    } else {
        tokio::runtime::Builder::new_current_thread().enable_all().build()
    }
    .expect("無法建立 tokio runtime");
    runtime.block_on(body())
}

#[test]
fn the_data_dir_is_removed_after_the_test_ends() {
    let dir = in_test_runtime(false, || async {
        let harness = Harness::start().await;
        let dir = harness.data_dir().to_path_buf();
        assert!(dir.join(baton_store::DB_FILE_NAME).exists(), "Runtime 已在資料目錄建立控制資料庫");
        dir
    });
    assert!(wait_until_removed(&dir), "harness 結束後資料目錄仍在：{}", dir.display());
}

#[test]
fn the_data_dir_is_removed_after_restarts_and_on_a_multi_thread_runtime() {
    let dir = in_test_runtime(true, || async {
        let mut harness = Harness::start().await;
        harness.restart().await;
        harness.crash_restart().await;
        harness.data_dir().to_path_buf()
    });
    assert!(wait_until_removed(&dir), "harness 結束後資料目錄仍在：{}", dir.display());
}

#[tokio::test]
async fn restarting_keeps_the_data_dir() {
    let mut harness = Harness::start().await;
    let dir = harness.data_dir().to_path_buf();
    harness.restart().await;
    assert_eq!(harness.data_dir(), dir);
    assert!(dir.join(baton_store::DB_FILE_NAME).exists(), "重建 Runtime 時保留資料目錄");
}

/// 之前的程序在刪完之前就結束（鎖檔沒人持有）：之後任何一個 harness 啟動時清掉；還有人持有的不碰。
#[cfg(windows)]
#[tokio::test]
async fn a_later_harness_removes_dirs_abandoned_by_exited_processes_but_not_ones_in_use() {
    use std::os::windows::fs::OpenOptionsExt;

    let root = std::env::temp_dir().join("baton-harness");
    std::fs::create_dir_all(&root).unwrap();
    let unique = format!("{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
    let abandoned = root.join(format!("habandoned{unique}"));
    let in_use = root.join(format!("hinuse{unique}"));
    for dir in [&abandoned, &in_use] {
        std::fs::create_dir_all(dir.join("logs")).unwrap();
        std::fs::write(dir.join("control.sqlite"), b"x").unwrap();
    }
    std::fs::write(root.join(format!("habandoned{unique}.lock")), b"").unwrap();
    let held = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .share_mode(0)
        .open(root.join(format!("hinuse{unique}.lock")))
        .unwrap();

    let _harness = Harness::start().await;

    assert!(!abandoned.exists(), "沒人持有鎖檔的目錄被清掉");
    assert!(!root.join(format!("habandoned{unique}.lock")).exists());
    assert!(in_use.join("control.sqlite").exists(), "有人持有鎖檔的目錄完全不碰");
    // 放掉之後就是沒人在用的目錄：並行的其他 harness 也可能先清掉，所以這裡不在乎刪除結果
    drop(held);
    let _ = std::fs::remove_dir_all(&in_use);
    let _ = std::fs::remove_file(root.join(format!("hinuse{unique}.lock")));
}
