//! 更新與解除安裝的維護在 Runtime 中的兩個步驟（票 34；spec D-19）。
//!
//! - `continue_after_stop`：與完全退出相同的停止流程結束之後執行。更新先備份（失敗就不套用），
//!   解除安裝移除登入啟動登記；之後維護進入「準備套用」（或「失敗」），Runtime 照完全退出的規則結束。
//! - `finish_on_open`：使用者手動開啟（或安裝程式代為開啟）時結束維護、記下結果並清掉維護抑制與下載的安裝程式；
//!   停止意圖仍在，之後由 Supervisor 依 DR-01 先核對再依各 Workspace 設定續跑。

use std::path::Path;
use std::sync::Arc;

use baton_api::LogSink;
use baton_domain::backup::BackupTrigger;
use baton_domain::guardian::LifecycleFileRead;
use baton_domain::maintenance::{AfterStop, MaintenanceKind, MaintenancePhase};
use baton_ports::{Clock, PortError, StartupRegistration};
use baton_store::Store;

use crate::backup::BackupScheduler;
use crate::updates::UPDATES_DIR;

/// 停止流程之後的維護步驟；沒有停止中的維護時什麼都不做。
pub(crate) async fn continue_after_stop(
    store: &Store,
    backups: &BackupScheduler,
    registration: Arc<dyn StartupRegistration>,
    clock: &dyn Clock,
    log: &LogSink,
) {
    let record = match store.maintenance().await {
        Ok(record) => record,
        Err(error) => return (log)(&format!("維護：無法讀取維護狀態：{error}")),
    };
    let Some(current) = record.current.filter(|m| m.phase == MaintenancePhase::Stopping) else { return };
    let result = match current.kind {
        MaintenanceKind::Update => match backups.back_up_now(BackupTrigger::PreUpdate).await {
            Ok(backup_id) => AfterStop::Ready { backup_id: Some(backup_id), note: None },
            Err(error) => AfterStop::Failed(format!("更新前備份失敗，不套用更新：{error}")),
        },
        MaintenanceKind::Uninstall => {
            let removed = tokio::task::spawn_blocking(move || registration.set_registered(false)).await;
            let note = match removed {
                Ok(Ok(())) | Ok(Err(PortError::NotImplemented(_))) => None,
                Ok(Err(error)) => Some(format!("無法移除登入啟動登記（解除安裝程式會再移除一次）：{error}")),
                Err(error) => Some(format!("無法移除登入啟動登記（解除安裝程式會再移除一次）：{error}")),
            };
            AfterStop::Ready { backup_id: None, note }
        }
    };
    match &result {
        AfterStop::Ready { backup_id: Some(id), .. } => (log)(&format!("維護：停止流程與更新前備份（{id}）完成，可以套用更新")),
        AfterStop::Ready { note: Some(note), .. } => (log)(&format!("維護：停止流程完成；{note}")),
        AfterStop::Ready { .. } => (log)("維護：停止流程完成，已移除登入啟動登記，可以繼續解除安裝"),
        AfterStop::Failed(message) => (log)(&format!("維護：{message}")),
    }
    if let Err(error) = store.complete_maintenance(result, clock.now()).await {
        (log)(&format!("維護：無法記下停止流程之後的結果：{error}"));
    }
}

/// 手動開啟時結束維護（監護與登入啟動拉起的 Runtime 不呼叫）。生命週期檔仍帶著維護抑制時改寫成資料庫的狀態。
pub(crate) async fn finish_on_open(
    store: &Store,
    data_dir: &Path,
    running_version: &str,
    clock: &dyn Clock,
    log: &LogSink,
) -> Result<(), baton_store::StoreError> {
    let outcome = store.finish_maintenance(running_version, clock.now()).await?;
    if let Some(outcome) = &outcome {
        (log)(&match (outcome.kind, outcome.applied) {
            (MaintenanceKind::Update, Some(true)) => {
                format!("維護：已從 {} 更新到 {}，依完全退出後的規則重新開啟", outcome.from_version, outcome.running_version)
            }
            (MaintenanceKind::Update, _) => format!(
                "維護：更新沒有套用（仍是 {}），依完全退出後的規則重新開啟",
                outcome.running_version
            ),
            (MaintenanceKind::Uninstall, _) => "維護：解除安裝後重新開啟，依完全退出後的規則重新開啟".to_string(),
        });
    }
    let dir = data_dir.to_path_buf();
    let file_has_maintenance = tokio::task::spawn_blocking(move || {
        matches!(baton_guardian::files::read_lifecycle(&dir), LifecycleFileRead::Parsed(file) if file.maintenance.is_some())
    })
    .await
    .unwrap_or(true);
    if outcome.is_some() || file_has_maintenance {
        let mirror = store.lifecycle_mirror(clock.now()).await?;
        let dir = data_dir.to_path_buf();
        let written = tokio::task::spawn_blocking(move || baton_store::write_lifecycle_file(&dir, &mirror)).await;
        if !matches!(written, Ok(Ok(()))) {
            (log)("維護：無法清除生命週期檔中的維護抑制；監護與登入啟動會繼續受抑制，直到下次手動開啟");
        }
        // 已用過（或沒用上）的安裝程式
        let updates = data_dir.join(UPDATES_DIR);
        let _ = tokio::task::spawn_blocking(move || std::fs::remove_dir_all(updates)).await;
    }
    Ok(())
}
