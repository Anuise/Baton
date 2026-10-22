//! Runtime 日誌：附加寫入資料目錄下的 `logs/runtime.log`（spec D-17 的日誌子目錄）。
//!
//! 每行以 Clock Port 的毫秒時間戳開頭。呼叫端負責不把憑證寫進訊息；
//! 寫入失敗只會被忽略，不影響請求處理。

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use baton_api::LogSink;
use baton_ports::Clock;

/// 日誌檔相對於資料目錄的位置。
pub const LOG_FILE: &str = "logs/runtime.log";

pub fn log_path(data_dir: &Path) -> PathBuf {
    data_dir.join(LOG_FILE)
}

/// 開啟日誌並回傳寫入函式。
pub(crate) fn open_log(data_dir: &Path, clock: Arc<dyn Clock>) -> std::io::Result<LogSink> {
    let path = log_path(data_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file: Mutex<File> = Mutex::new(OpenOptions::new().create(true).append(true).open(path)?);
    Ok(Arc::new(move |message: &str| {
        let mut file = file.lock().unwrap_or_else(|e| e.into_inner());
        let _ = writeln!(file, "{} {message}", clock.now().as_millis());
    }))
}
