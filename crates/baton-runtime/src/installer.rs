//! 安裝程式的準備步驟（票 34；spec D-19、D-25）：`baton-runtime.exe maintenance prepare-update | prepare-uninstall`。
//!
//! 由 NSIS 安裝程式的 hook 在複製檔案前（覆蓋安裝）與解除安裝前執行，讓 Runtime 與監護在檔案被取代或刪除前就結束，
//! 而且之後不會被監護或登入啟動入口拉起：
//! 1. 生命週期檔已經有更新的維護（使用者在 App 內同意更新，桌面殼執行的安裝程式）：不再要求任何事，
//!    也不移除登入啟動登記（更新後照常自動啟動）。
//! 2. 有執行中的 Runtime：要求維護，Runtime 執行與完全退出相同的停止流程（更新另做更新前備份；解除安裝移除登入啟動登記）
//!    後自行結束；等到它不再回應。
//! 3. 最後確認生命週期檔帶著維護抑制（沒有 Runtime 時直接寫入），並等 Runtime 與監護的程序結束。
//! 4. 解除安裝再移除一次登入啟動登記。資料目錄一律保留。
//!
//! 每一步失敗都只記下來、繼續往下：安裝程式不能因為準備步驟失敗而卡住。

use std::path::Path;
use std::time::Duration;

use baton_api::LogSink;
use baton_client::{ClientError, RuntimeClient};
use baton_domain::guardian::LifecycleFileRead;
use baton_domain::lifecycle::{LifecycleFile, MaintenanceSuppression, StopSuppression, LIFECYCLE_FILE_VERSION};
use baton_domain::maintenance::MaintenanceKind;
use baton_ports::{Clock, PortError, StartupRegistration};

/// `baton-runtime.exe maintenance <動作>` 的子命令名稱。
pub const MAINTENANCE_COMMAND: &str = "maintenance";
pub const PREPARE_UPDATE_ARG: &str = "prepare-update";
pub const PREPARE_UNINSTALL_ARG: &str = "prepare-uninstall";

/// 子命令的動作對應的維護種類。
pub fn kind_of_arg(arg: &str) -> Option<MaintenanceKind> {
    match arg {
        PREPARE_UPDATE_ARG => Some(MaintenanceKind::Update),
        PREPARE_UNINSTALL_ARG => Some(MaintenanceKind::Uninstall),
        _ => None,
    }
}

/// 等待的上限。
#[derive(Debug, Clone, Copy)]
pub struct PrepareTiming {
    /// 等 Runtime 完成停止流程並結束（40 秒上限 + 備份 + 5 秒的結果顯示）。
    pub runtime_exit: Duration,
    /// 等 Runtime 與監護的程序結束。
    pub processes_exit: Duration,
    pub poll: Duration,
}

impl Default for PrepareTiming {
    fn default() -> Self {
        Self { runtime_exit: Duration::from_secs(120), processes_exit: Duration::from_secs(15), poll: Duration::from_millis(250) }
    }
}

/// 準備步驟怎麼完成的。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prepared {
    /// 已經在進行更新的維護（App 內同意的更新）：沒有再要求任何事。
    AlreadyUpdating,
    /// 執行中的 Runtime 接受維護、完成停止流程並結束。
    RuntimeStopped,
    /// 沒有執行中的 Runtime：直接在生命週期檔寫入維護抑制。
    MarkedWithoutRuntime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrepareReport {
    pub prepared: Prepared,
    /// 解除安裝時是否已移除登入啟動登記。
    pub registration_removed: bool,
    /// 沒有擋下安裝程式、但值得記下的問題。
    pub notes: Vec<String>,
}

/// 執行準備步驟；不會回傳錯誤（問題記在 `notes`），讓安裝程式照常繼續。
pub async fn prepare(
    kind: MaintenanceKind,
    data_dir: &Path,
    client: &RuntimeClient,
    registration: &dyn StartupRegistration,
    clock: &dyn Clock,
    timing: PrepareTiming,
    log: &LogSink,
) -> PrepareReport {
    let mut notes = Vec::new();
    let prepared = if updating(data_dir) {
        Prepared::AlreadyUpdating
    } else if client.health().await.is_ok() {
        match client.request_maintenance(kind).await {
            Ok(_) => {}
            // 另一種維護（App 內同意的更新）已在進行：交給它
            Err(ClientError::Rejected { code, .. }) if code == "maintenance_in_progress" && updating_now(client).await => {}
            Err(error) => notes.push(format!("Runtime 沒有接受維護要求：{error}")),
        }
        if !wait_until(timing.runtime_exit, timing.poll, || async { client.health().await.is_err() }).await {
            notes.push(format!("Runtime 沒有在 {} 秒內結束", timing.runtime_exit.as_secs()));
        }
        if updating_after_request(data_dir, kind) { Prepared::AlreadyUpdating } else { Prepared::RuntimeStopped }
    } else {
        Prepared::MarkedWithoutRuntime
    };

    if prepared != Prepared::AlreadyUpdating {
        if let Err(error) = ensure_maintenance_in_file(data_dir, kind, clock) {
            notes.push(format!("無法在生命週期檔寫入維護抑制：{error}"));
        }
    }
    let runtime = baton_guardian::files::runtime_instance_path(data_dir);
    let guardian = baton_guardian::files::guardian_instance_path(data_dir);
    let processes_gone = wait_until(timing.processes_exit, timing.poll, || async {
        !baton_guardian::files::instance_alive(&runtime) && !baton_guardian::files::instance_alive(&guardian)
    })
    .await;
    if !processes_gone {
        notes.push(format!("Runtime 或監護的程序沒有在 {} 秒內結束", timing.processes_exit.as_secs()));
    }

    let mut registration_removed = false;
    if kind == MaintenanceKind::Uninstall && prepared != Prepared::AlreadyUpdating {
        match registration.set_registered(false) {
            Ok(()) => registration_removed = true,
            Err(PortError::NotImplemented(_)) => {}
            Err(error) => notes.push(format!("無法移除登入啟動登記：{error}")),
        }
    }

    (log)(&format!(
        "安裝程式準備（{}）：{prepared:?}{}{}",
        kind.as_str(),
        if registration_removed { "，已移除登入啟動登記" } else { "" },
        if notes.is_empty() { String::new() } else { format!("；{}", notes.join("；")) }
    ));
    PrepareReport { prepared, registration_removed, notes }
}

/// 生命週期檔已經帶著更新的維護抑制。
fn updating(data_dir: &Path) -> bool {
    matches!(
        baton_guardian::files::read_lifecycle(data_dir),
        LifecycleFileRead::Parsed(LifecycleFile { maintenance: Some(m), .. }) if m.kind == MaintenanceKind::Update.as_str()
    )
}

/// 要求的是解除安裝、但 Runtime 已在更新的維護中（例如舊版解除安裝程式在更新時被執行）。
fn updating_after_request(data_dir: &Path, kind: MaintenanceKind) -> bool {
    kind == MaintenanceKind::Uninstall && updating(data_dir)
}

async fn updating_now(client: &RuntimeClient) -> bool {
    client
        .updates()
        .await
        .is_ok_and(|status| status.maintenance.is_some_and(|m| m.kind == MaintenanceKind::Update))
}

/// 確認生命週期檔帶著維護抑制；沒有就加上（保留既有的停止抑制）。讀不懂的檔案保守地另外記為停止。
fn ensure_maintenance_in_file(data_dir: &Path, kind: MaintenanceKind, clock: &dyn Clock) -> std::io::Result<()> {
    let now = clock.now();
    let maintenance = MaintenanceSuppression { kind: kind.as_str().to_string(), requested_at: now };
    let file = match baton_guardian::files::read_lifecycle(data_dir) {
        LifecycleFileRead::Parsed(file) if file.maintenance.is_some() => return Ok(()),
        LifecycleFileRead::Parsed(file) => LifecycleFile { maintenance: Some(maintenance), updated_at: now, ..file },
        LifecycleFileRead::Missing => {
            LifecycleFile { version: LIFECYCLE_FILE_VERSION, stop: None, maintenance: Some(maintenance), updated_at: now }
        }
        LifecycleFileRead::Unreadable => LifecycleFile {
            version: LIFECYCLE_FILE_VERSION,
            stop: Some(StopSuppression { requested_at: now, reason: "unreadable_lifecycle_file".to_string() }),
            maintenance: Some(maintenance),
            updated_at: now,
        },
    };
    baton_store::write_lifecycle_file(data_dir, &file)
}

async fn wait_until<F, Fut>(limit: Duration, poll: Duration, mut done: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + limit;
    loop {
        if done().await {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(poll).await;
    }
}
