//! 備份排程（票 40，spec D-18）：每日備份、重要裁定或 Stage 完成後的備份（同一小時內合併為一次）、
//! 每次備份後套用保留規則，以及每週一次的還原演練。migration 前的備份由 Store 開啟資料庫時建立。
//!
//! 正式執行時由背景迴圈每分鐘呼叫 `tick`；測試 harness 不啟動背景迴圈，由測試搭配 fake Clock 呼叫。
//! 備份與演練一律依序進行（`tick` 互斥），所以保留規則刪除備份時不會有別的備份或演練正在讀寫。

use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use baton_api::LogSink;
use baton_domain::backup::{self, BackupTrigger};
use baton_domain::Timestamp;
use baton_ports::Clock;
use baton_store::backup::{self as store_backup, BackupError};
use baton_store::Store;
use tokio::sync::{watch, Mutex};
use tokio::task::JoinHandle;

/// 背景迴圈檢查是否該備份的間隔。
const BACKGROUND_INTERVAL: Duration = Duration::from_secs(60);

pub struct BackupScheduler {
    store: Store,
    data_dir: PathBuf,
    clock: Arc<dyn Clock>,
    log: LogSink,
    /// 尚未被備份涵蓋的重要裁定或 Stage 完成要求（最早的一次）。
    /// 只存在記憶體：Runtime 重啟時遺失的要求，最遲由下一次每日備份涵蓋。
    milestone_requested: StdMutex<Option<Timestamp>>,
    /// 同一時間只有一個 `tick` 在進行。
    tick_lock: Mutex<()>,
}

impl BackupScheduler {
    pub fn new(store: Store, data_dir: PathBuf, clock: Arc<dyn Clock>, log: LogSink) -> Arc<Self> {
        Arc::new(Self { store, data_dir, clock, log, milestone_requested: StdMutex::new(None), tick_lock: Mutex::new(()) })
    }

    /// 背景迴圈：每分鐘 `tick` 一次，Runtime 開始停止時結束。
    pub fn spawn_background(self: &Arc<Self>, mut stop: watch::Receiver<bool>) -> JoinHandle<()> {
        let scheduler = self.clone();
        tokio::spawn(async move {
            loop {
                scheduler.tick().await;
                tokio::select! {
                    _ = stop.wait_for(|stopping| *stopping) => break,
                    _ = tokio::time::sleep(BACKGROUND_INTERVAL) => {}
                }
            }
        })
    }

    /// 重要裁定或 Stage 完成時呼叫：下一次 `tick` 會建立備份；距離上一份備份不到一小時就等到滿一小時，期間的要求合併為一次。
    pub fn request_milestone_backup(&self, reason: &str) {
        self.pending().get_or_insert(self.clock.now());
        self.note(&format!("收到備份要求：{reason}"));
    }

    fn pending(&self) -> std::sync::MutexGuard<'_, Option<Timestamp>> {
        self.milestone_requested.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 該備份就備份、該演練就演練；失敗只記日誌，下一次 `tick` 會再試。
    pub async fn tick(&self) {
        let _serial = self.tick_lock.lock().await;
        let now = self.clock.now();
        match self.list().await {
            Ok(backups) => {
                let latest = backups.last().map(|b| b.created_at);
                let requested = *self.pending();
                if let Some(trigger) = backup::due_backup(latest, requested, now) {
                    // 只清除這份備份涵蓋得到的要求：備份進行中才到的要求留給下一份
                    if self.back_up(trigger, now).await {
                        let mut pending = self.pending();
                        if pending.is_some_and(|at| at <= now) {
                            *pending = None;
                        }
                    }
                }
            }
            Err(error) => self.note(&format!("無法列出備份：{error}")),
        }
        self.drill_if_due(now).await;
    }

    fn note(&self, message: &str) {
        (self.log)(&format!("備份：{message}"));
    }

    async fn list(&self) -> Result<Vec<baton_domain::backup::BackupManifest>, BackupError> {
        let data_dir = self.data_dir.clone();
        tokio::task::spawn_blocking(move || store_backup::list_backups(&data_dir)).await.map_err(|_| BackupError::Join)?
    }

    /// 建立一份備份並套用保留規則；回傳備份是否成功。
    async fn back_up(&self, trigger: BackupTrigger, now: Timestamp) -> bool {
        match self.store.create_backup(trigger, now).await {
            Ok(manifest) => {
                self.note(&format!("已建立備份 {}（{} 個成果檔案）", manifest.id, manifest.artifacts.len()));
            }
            Err(error) => {
                self.note(&format!("{} 備份失敗：{error}", trigger.as_str()));
                return false;
            }
        }
        let data_dir = self.data_dir.clone();
        match tokio::task::spawn_blocking(move || store_backup::apply_retention(&data_dir)).await {
            Ok(Ok(removed)) if !removed.is_empty() => self.note(&format!("依保留規則刪除 {} 份備份", removed.len())),
            Ok(Ok(_)) => {}
            Ok(Err(error)) => self.note(&format!("套用保留規則失敗：{error}")),
            Err(_) => self.note("套用保留規則被中斷"),
        }
        true
    }

    async fn drill_if_due(&self, now: Timestamp) {
        let data_dir = self.data_dir.clone();
        let result = tokio::task::spawn_blocking(move || {
            let last = store_backup::latest_drill(&data_dir)?.map(|d| d.performed_at);
            if !backup::drill_due(last, now) {
                return Ok(None);
            }
            store_backup::run_drill(&data_dir, now)
        })
        .await
        .map_err(|_| BackupError::Join);
        match result.and_then(|r| r) {
            Ok(Some(report)) if report.passed => self.note("還原演練通過"),
            Ok(Some(report)) => {
                let failed: Vec<_> = report.checks.iter().filter(|c| !c.passed).map(|c| c.detail.as_str()).collect();
                self.note(&format!("還原演練沒有通過：{}", failed.join("；")));
            }
            Ok(None) => {}
            Err(error) => self.note(&format!("還原演練無法進行：{error}")),
        }
    }
}
