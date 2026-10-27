//! 整合測試共用的輔助（review-notes #28）。

use std::time::{Duration, Instant};

/// 刪除測試的暫存目錄並確認刪掉。剛被終止的子程序可能還佔著它的工作目錄（Windows 上程序完全結束前
/// 不能刪除），`TempDir` 丟棄時又會靜默忽略刪除失敗而留下目錄，所以這裡有上限地重試；逾時表示子程序
/// 沒有結束，測試失敗。
pub async fn remove_temp_dir(dir: tempfile::TempDir) {
    let path = dir.keep();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match std::fs::remove_dir_all(&path) {
            Ok(()) => return,
            Err(_) if !path.exists() => return,
            Err(e) if Instant::now() >= deadline => panic!("10 秒內無法刪除暫存目錄 {}：{e}", path.display()),
            Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
        }
    }
}
