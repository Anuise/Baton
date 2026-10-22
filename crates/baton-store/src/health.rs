//! 控制資料庫的寫入故障追蹤（票 26）：磁碟滿、被鎖住或無法寫入時記下原因，Supervisor 據此停止新派工，
//! UI 經健康端點顯示原因與處理方式。
//!
//! - 每個寫入工作失敗時，依 SQLite 的錯誤代碼歸類；不是儲存問題的錯誤（例如約束違反）不改變狀態。
//! - 故障只在一次真正寫入資料的試寫（`Store::probe_storage`）成功後才解除，不會因為一個沒有寫入任何東西的交易而誤判恢復。
//! - 測試可以注入故障（`Store::inject_write_fault`）：之後每個寫入工作都得到與真實情況相同的 SQLite 錯誤，
//!   走同一條歸類路徑；真實錯誤的歸類另有單元測試。

use baton_domain::storage_health::{StorageFault, StorageFaultKind, StorageHealth};
use baton_domain::Timestamp;
use rusqlite::ErrorCode;

use super::{Store, StoreError, StoreResult};

/// 依 SQLite 的錯誤代碼判斷是不是儲存故障。
pub(crate) fn classify(error: &StoreError) -> Option<StorageFault> {
    let StoreError::Sqlite(rusqlite::Error::SqliteFailure(failure, message)) = error else {
        return None;
    };
    let kind = match failure.code {
        ErrorCode::DiskFull => StorageFaultKind::DiskFull,
        ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked => StorageFaultKind::Busy,
        ErrorCode::SystemIoFailure | ErrorCode::ReadOnly | ErrorCode::CannotOpen => StorageFaultKind::Unwritable,
        _ => return None,
    };
    Some(StorageFault::new(kind, message.clone().unwrap_or_else(|| failure.to_string())))
}

/// 注入的故障對應的 SQLite 錯誤（代碼與訊息與 SQLite 實際回傳的相同）。
pub(crate) fn injected_error(kind: StorageFaultKind) -> StoreError {
    let (code, message) = match kind {
        StorageFaultKind::DiskFull => (rusqlite::ffi::SQLITE_FULL, "database or disk is full"),
        StorageFaultKind::Busy => (rusqlite::ffi::SQLITE_BUSY, "database is locked"),
        StorageFaultKind::Unwritable => (rusqlite::ffi::SQLITE_IOERR, "disk I/O error"),
    };
    StoreError::Sqlite(rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(code), Some(message.to_string())))
}

impl Store {
    /// 目前的可寫狀態；有故障時新派工停止。
    pub fn storage_health(&self) -> StorageHealth {
        StorageHealth::from_fault(self.inner.health.lock().unwrap_or_else(|e| e.into_inner()).clone())
    }

    /// 試寫一次：成功就解除故障；失敗時回傳錯誤（故障由寫入佇列記下）。
    pub async fn probe_storage(&self, now: Timestamp) -> StoreResult<()> {
        self.write(move |tx| {
            tx.execute(
                "INSERT INTO storage_probe (id, probed_at) VALUES (1, ?1)
                 ON CONFLICT(id) DO UPDATE SET probed_at = excluded.probed_at",
                [now.as_millis()],
            )?;
            Ok(())
        })
        .await?;
        *self.inner.health.lock().unwrap_or_else(|e| e.into_inner()) = None;
        Ok(())
    }

    /// 測試用的故障注入：`Some` 之後每個寫入工作都以該種故障失敗（不執行工作本身），`None` 恢復正常。
    /// 正式執行時沒有任何呼叫端。
    #[doc(hidden)]
    pub fn inject_write_fault(&self, kind: Option<StorageFaultKind>) {
        *self.inner.injected.lock().unwrap_or_else(|e| e.into_inner()) = kind;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: Timestamp = Timestamp::from_millis(500);

    async fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path(), T0).await.unwrap();
        (dir, store)
    }

    #[tokio::test]
    async fn a_full_database_is_recorded_as_disk_full_until_a_probe_succeeds() {
        let (_dir, store) = store().await;
        // 以 SQLite 自己的頁數上限重現「磁碟已滿」：之後需要新頁面的寫入都得到真實的 SQLITE_FULL
        store
            .write(|tx| {
                let pages: i64 = tx.query_row("PRAGMA page_count", [], |r| r.get(0))?;
                tx.pragma_update(None, "max_page_count", pages)?;
                Ok(())
            })
            .await
            .unwrap();
        let big = format!("\"{}\"", "x".repeat(1024 * 1024));

        let error = store
            .write(move |tx| {
                tx.execute(
                    "INSERT INTO event (aggregate_type, aggregate_id, aggregate_version, event_type, occurred_at, payload)
                     VALUES ('test', 't1', 1, 'filled', 0, ?1)",
                    [&big],
                )?;
                Ok(())
            })
            .await
            .unwrap_err();

        assert_eq!(classify(&error).map(|f| f.kind), Some(StorageFaultKind::DiskFull), "{error}");
        let health = store.storage_health();
        assert!(health.dispatch_halted);
        assert_eq!(health.fault.as_ref().map(|f| f.code.as_str()), Some("storage_disk_full"));

        // 空間恢復（放寬上限）之後，試寫成功才解除
        store.write(|tx| Ok(tx.pragma_update(None, "max_page_count", 1_073_741_823)?)).await.unwrap();
        assert!(store.storage_health().dispatch_halted, "沒有寫入任何東西的交易不算恢復");
        store.probe_storage(T0).await.unwrap();
        assert_eq!(store.storage_health(), StorageHealth::default());
    }

    #[tokio::test]
    async fn a_database_locked_by_another_program_is_recorded_as_busy() {
        let (dir, store) = store().await;
        let other = rusqlite::Connection::open(dir.path().join(crate::DB_FILE_NAME)).unwrap();
        other.execute_batch("BEGIN EXCLUSIVE").unwrap();

        // busy timeout（5 秒）之後才會失敗
        let error = store.probe_storage(T0).await.unwrap_err();
        other.execute_batch("ROLLBACK").unwrap();

        assert_eq!(classify(&error).map(|f| f.kind), Some(StorageFaultKind::Busy), "{error}");
        assert_eq!(store.storage_health().fault.map(|f| f.kind), Some(StorageFaultKind::Busy));
        store.probe_storage(T0).await.unwrap();
        assert!(!store.storage_health().dispatch_halted);
    }

    #[tokio::test]
    async fn an_injected_fault_fails_every_write_through_the_same_classification() {
        let (_dir, store) = store().await;
        store.inject_write_fault(Some(StorageFaultKind::DiskFull));

        let error = store.record_boot(T0).await.unwrap_err();
        let probe = store.probe_storage(T0).await;

        assert_eq!(classify(&error).map(|f| f.kind), Some(StorageFaultKind::DiskFull));
        assert!(probe.is_err());
        assert!(store.storage_health().dispatch_halted);
        store.inject_write_fault(None);
        store.probe_storage(T0).await.unwrap();
        assert!(!store.storage_health().dispatch_halted);
    }

    #[tokio::test]
    async fn errors_that_are_not_storage_faults_do_not_halt_dispatch() {
        let (_dir, store) = store().await;

        let error = store.write(|tx| Ok(tx.execute("INSERT INTO storage_probe (id, probed_at) VALUES (2, 0)", [])?)).await;

        assert!(error.is_err(), "違反 CHECK 約束");
        assert!(!store.storage_health().dispatch_halted);
    }
}
