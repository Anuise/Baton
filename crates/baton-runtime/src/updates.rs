//! 更新檢查與安裝程式下載（票 34；spec D-19）。
//!
//! 啟動時檢查一次，之後每 24 小時一次；看到較新的版本只記下來給 UI 通知，不會開始任何維護。
//! 使用者同意後（`POST /api/v1/updates/apply`）才下載並驗證簽章；開始維護由 API 負責。
//! 檢查結果只存在記憶體：Runtime 每次啟動都會重新檢查。

use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use baton_api::{LogSink, UpdateControl, UpdatePrepareError};
use baton_domain::maintenance::{self, CheckResult, UpdateCheck};
use baton_ports::{Clock, PortError, UpdateSource};
use futures::future::BoxFuture;
use tokio::sync::{watch, Mutex};
use tokio::task::JoinHandle;

/// 下載的安裝程式放在資料目錄下的這個目錄；下次手動開啟、結束維護時清掉。
pub const UPDATES_DIR: &str = "updates";

/// 背景迴圈檢查「該不該檢查」的間隔；真正的檢查間隔是 24 小時。
const BACKGROUND_INTERVAL: Duration = Duration::from_secs(60);

pub struct UpdateService {
    source: Arc<dyn UpdateSource>,
    clock: Arc<dyn Clock>,
    log: LogSink,
    data_dir: PathBuf,
    state: StdMutex<UpdateCheck>,
    /// 同一時間只有一個檢查在進行。
    tick_lock: Mutex<()>,
}

impl UpdateService {
    pub fn new(
        source: Arc<dyn UpdateSource>,
        clock: Arc<dyn Clock>,
        log: LogSink,
        data_dir: PathBuf,
        app_version: &str,
    ) -> Arc<Self> {
        Arc::new(Self {
            source,
            clock,
            log,
            data_dir,
            state: StdMutex::new(UpdateCheck::new(app_version)),
            tick_lock: Mutex::new(()),
        })
    }

    /// 背景迴圈：一開始就檢查（啟動時），之後每分鐘看一次是否滿 24 小時；Runtime 開始停止時結束。
    pub fn spawn_background(self: &Arc<Self>, mut stop: watch::Receiver<bool>) -> JoinHandle<()> {
        let service = self.clone();
        tokio::spawn(async move {
            loop {
                service.tick().await;
                tokio::select! {
                    _ = stop.wait_for(|stopping| *stopping) => break,
                    _ = tokio::time::sleep(BACKGROUND_INTERVAL) => {}
                }
            }
        })
    }

    /// 該檢查就檢查一次（啟動後第一次一定檢查，之後每 24 小時）。時間由 Clock 決定。
    pub async fn tick(&self) {
        let _serial = self.tick_lock.lock().await;
        let now = self.clock.now();
        if !self.current().due(now) {
            return;
        }
        let result = match self.source.latest().await {
            Ok(release) => CheckResult::Latest(release),
            Err(PortError::NotImplemented(_)) => CheckResult::Unconfigured,
            Err(PortError::Failed(reason)) => CheckResult::Failed(reason),
        };
        let next = maintenance::record_check(&self.current(), result, now);
        match (&next.available, &next.error) {
            (_, Some(error)) => (self.log)(&format!("更新：檢查失敗，明天再試：{error}")),
            (Some(release), None) => {
                (self.log)(&format!("更新：有新版本 {}（只通知，等使用者同意才更新）", release.version))
            }
            (None, None) => {}
        }
        *self.state.lock().unwrap_or_else(|e| e.into_inner()) = next;
    }

    fn current(&self) -> UpdateCheck {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl UpdateControl for UpdateService {
    fn status(&self) -> UpdateCheck {
        self.current()
    }

    fn prepare<'a>(&'a self, version: &'a str) -> BoxFuture<'a, Result<PathBuf, UpdatePrepareError>> {
        Box::pin(async move {
            let check = self.current();
            if check.status == maintenance::UpdateCheckStatus::Unconfigured {
                return Err(UpdatePrepareError::Unconfigured);
            }
            let Some(release) = check.available.filter(|r| r.version == version) else {
                return Err(UpdatePrepareError::NotAvailable);
            };
            let dest = self.data_dir.join(UPDATES_DIR).join(&release.version);
            match self.source.download(&release, &dest).await {
                Ok(path) => {
                    (self.log)(&format!("更新：已下載並驗證 {} 的安裝程式：{}", release.version, path.display()));
                    Ok(path)
                }
                Err(PortError::NotImplemented(_)) => Err(UpdatePrepareError::Unconfigured),
                Err(PortError::Failed(reason)) => Err(UpdatePrepareError::Download(reason)),
            }
        })
    }
}
