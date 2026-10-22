//! Store：SQLite 存取。
//!
//! 目前只有開啟資料庫、套用 migration 與記錄 Runtime 啟動；
//! 單一 writer 佇列、備份與完整資料表由後續的票加入。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use baton_domain::Timestamp;
use rusqlite::Connection;

/// 控制資料庫檔名，位於資料目錄之下。
pub const DB_FILE_NAME: &str = "control.sqlite";

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("無法建立資料目錄 {0}：{1}")]
    CreateDir(PathBuf, std::io::Error),
    #[error("資料庫錯誤：{0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("資料庫設定未生效：{0}")]
    PragmaNotApplied(String),
    #[error("資料庫工作被中斷")]
    Join,
}

pub type StoreResult<T> = Result<T, StoreError>;

/// 每次 migration 以 `PRAGMA user_version` 記錄已套用到哪一版。
const MIGRATIONS: &[&str] = &[
    "CREATE TABLE runtime_boot (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        started_at INTEGER NOT NULL
    );",
];

#[derive(Clone)]
pub struct Store {
    conn: Arc<Mutex<Connection>>,
}

impl Store {
    /// 在 `data_dir` 開啟（必要時建立）控制資料庫並套用 migration。
    pub async fn open(data_dir: &Path) -> StoreResult<Self> {
        let data_dir = data_dir.to_path_buf();
        tokio::task::spawn_blocking(move || Self::open_blocking(&data_dir))
            .await
            .map_err(|_| StoreError::Join)?
    }

    fn open_blocking(data_dir: &Path) -> StoreResult<Self> {
        std::fs::create_dir_all(data_dir)
            .map_err(|e| StoreError::CreateDir(data_dir.to_path_buf(), e))?;
        let conn = Connection::open(data_dir.join(DB_FILE_NAME))?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        // WAL 模式；啟動時讀回驗證（spec「SQLite 設定…啟動時讀回驗證」）
        let mode: String = conn.pragma_update_and_check(None, "journal_mode", "WAL", |r| r.get(0))?;
        if !mode.eq_ignore_ascii_case("wal") {
            return Err(StoreError::PragmaNotApplied(format!("journal_mode={mode}")));
        }
        conn.pragma_update(None, "synchronous", "FULL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        migrate(&conn)?;
        Ok(Self { conn: Arc::new(Mutex::new(conn)) })
    }

    /// 記錄一次 Runtime 啟動，回傳累計的啟動次數（含這一次）。
    pub async fn record_boot(&self, now: Timestamp) -> StoreResult<u64> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().unwrap_or_else(|e| e.into_inner());
            conn.execute("INSERT INTO runtime_boot (started_at) VALUES (?1)", [now.as_millis()])?;
            let count: i64 = conn.query_row("SELECT COUNT(*) FROM runtime_boot", [], |r| r.get(0))?;
            Ok(count as u64)
        })
        .await
        .map_err(|_| StoreError::Join)?
    }
}

fn migrate(conn: &Connection) -> StoreResult<()> {
    let applied: usize = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    for (index, sql) in MIGRATIONS.iter().enumerate().skip(applied) {
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", index + 1)?;
        tx.commit()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn boot_count_accumulates_across_reopen_of_same_data_dir() {
        let dir = tempfile::tempdir().unwrap();
        let first = Store::open(dir.path()).await.unwrap();
        assert_eq!(first.record_boot(Timestamp::from_millis(1)).await.unwrap(), 1);
        drop(first);

        let second = Store::open(dir.path()).await.unwrap();
        assert_eq!(second.record_boot(Timestamp::from_millis(2)).await.unwrap(), 2);
    }

    #[tokio::test]
    async fn separate_data_dirs_do_not_share_state() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let store_a = Store::open(a.path()).await.unwrap();
        store_a.record_boot(Timestamp::from_millis(1)).await.unwrap();
        let store_b = Store::open(b.path()).await.unwrap();
        assert_eq!(store_b.record_boot(Timestamp::from_millis(1)).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn open_creates_missing_data_dir() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a").join("b");
        Store::open(&nested).await.unwrap();
        assert!(nested.join(DB_FILE_NAME).exists());
    }
}
