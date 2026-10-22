//! 安裝後的更新檢查與維護（更新、解除安裝）（票 34；spec D-19、D-25）。
//!
//! 純邏輯、無 I/O。幾條貫穿全檔的規則：
//! - **只通知、不自動套用**：Runtime 啟動時與之後每日檢查一次簽章更新，看到較新的版本只記為「可更新」，
//!   不會因此停止任何工作；只有使用者同意後才開始維護。
//! - **維護 = 維護抑制 + 與完全退出相同的停止流程**：開始維護的同一個交易持久化停止意圖（`lifecycle::request_exit`），
//!   生命週期檔另外帶著維護抑制，監護與登入啟動入口看到就不拉起 Runtime。
//! - **停止之後**：更新先備份再「準備套用」（由桌面殼執行驗證過簽章的安裝程式）；解除安裝移除登入啟動登記。
//!   兩者都不刪除資料目錄。
//! - **只有手動開啟才結束維護**：下次由使用者（或安裝程式代為）開 App 啟動 Runtime 時結束維護、記下結果，
//!   之後依 DR-01 先核對再依各 Workspace 設定續跑。監護與登入啟動拉起的 Runtime 不結束維護。

use serde::{Deserialize, Serialize};

use crate::event::DomainEvent;
use crate::lifecycle::{LifecycleState, MaintenanceSuppression};
use crate::Timestamp;

pub const MAINTENANCE_AGGREGATE: &str = "maintenance";
pub const MAINTENANCE_ID: &str = "runtime";

pub const EVENT_MAINTENANCE_REQUESTED: &str = "maintenance_requested";
pub const EVENT_MAINTENANCE_READY: &str = "maintenance_ready";
pub const EVENT_MAINTENANCE_FAILED: &str = "maintenance_failed";
pub const EVENT_MAINTENANCE_FINISHED: &str = "maintenance_finished";

/// 更新檢查的間隔（spec D-19：啟動時與每日一次）。
pub const UPDATE_CHECK_INTERVAL_MS: i64 = 24 * 60 * 60 * 1000;

// ---------------------------------------------------------------- 版本

/// `candidate` 是否比 `current` 新（語意化版本：數字逐段比較；同號時正式版比預覽版新；忽略建置中繼資料與前綴 `v`）。
/// 任一方看不懂時視為「不是較新」，不會因此提示更新。
pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(a), Some(b)) => compare_versions(&a, &b) == std::cmp::Ordering::Greater,
        _ => false,
    }
}

struct Version {
    core: [u64; 3],
    pre: Vec<String>,
}

fn parse_version(raw: &str) -> Option<Version> {
    let raw = raw.trim();
    let raw = raw.strip_prefix('v').unwrap_or(raw);
    let raw = raw.split('+').next()?;
    let (core, pre) = match raw.split_once('-') {
        Some((core, pre)) => (core, pre.split('.').map(str::to_string).collect()),
        None => (raw, Vec::new()),
    };
    let mut numbers = [0u64; 3];
    let parts: Vec<&str> = core.split('.').collect();
    if parts.is_empty() || parts.len() > 3 {
        return None;
    }
    for (slot, part) in numbers.iter_mut().zip(parts) {
        *slot = part.parse().ok()?;
    }
    Some(Version { core: numbers, pre })
}

fn compare_versions(a: &Version, b: &Version) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    a.core.cmp(&b.core).then_with(|| match (a.pre.is_empty(), b.pre.is_empty()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        (false, false) => {
            for (x, y) in a.pre.iter().zip(&b.pre) {
                let order = match (x.parse::<u64>(), y.parse::<u64>()) {
                    (Ok(x), Ok(y)) => x.cmp(&y),
                    (Ok(_), Err(_)) => Ordering::Less,
                    (Err(_), Ok(_)) => Ordering::Greater,
                    (Err(_), Err(_)) => x.cmp(y),
                };
                if order != Ordering::Equal {
                    return order;
                }
            }
            a.pre.len().cmp(&b.pre.len())
        }
    })
}

// ---------------------------------------------------------------- 更新檢查

/// 更新來源公布的最新版本。`url` 與 `signature` 只交給更新來源下載與驗證簽章，不顯示給使用者。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseInfo {
    pub version: String,
    #[serde(default)]
    pub notes: Option<String>,
    /// 發行時間（更新來源提供的原文）。
    #[serde(default)]
    pub published_at: Option<String>,
    pub url: String,
    pub signature: String,
}

/// 一次檢查的結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckResult {
    /// 取得更新來源公布的最新版本（不一定比目前新）。
    Latest(ReleaseInfo),
    /// 這個版本沒有設定更新來源（開發版或未設定簽章公鑰）。
    Unconfigured,
    Failed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateCheckStatus {
    NotChecked,
    UpToDate,
    /// 有較新的版本；只通知，等使用者同意。
    Available,
    Unconfigured,
    Failed,
}

/// 更新檢查的狀態（只存在記憶體；Runtime 每次啟動都會重新檢查）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateCheck {
    pub current_version: String,
    pub status: UpdateCheckStatus,
    /// 可更新的版本；檢查失敗時保留之前看到的。
    pub available: Option<ReleaseInfo>,
    pub checked_at: Option<Timestamp>,
    pub next_check_at: Option<Timestamp>,
    pub error: Option<String>,
}

impl UpdateCheck {
    pub fn new(current_version: &str) -> Self {
        Self {
            current_version: current_version.to_string(),
            status: UpdateCheckStatus::NotChecked,
            available: None,
            checked_at: None,
            next_check_at: None,
            error: None,
        }
    }

    /// 現在該不該檢查：啟動後第一次一定要；之後每 24 小時一次（不論上次成功與否）。
    pub fn due(&self, now: Timestamp) -> bool {
        self.next_check_at.is_none_or(|next| now >= next)
    }
}

/// 記下一次檢查的結果。較新的版本只記為「可更新」，不會開始任何維護。
pub fn record_check(previous: &UpdateCheck, result: CheckResult, now: Timestamp) -> UpdateCheck {
    let mut next = previous.clone();
    next.checked_at = Some(now);
    next.next_check_at = Some(now.plus_millis(UPDATE_CHECK_INTERVAL_MS));
    next.error = None;
    match result {
        CheckResult::Latest(release) if is_newer(&release.version, &previous.current_version) => {
            next.status = UpdateCheckStatus::Available;
            next.available = Some(release);
        }
        CheckResult::Latest(_) => {
            next.status = UpdateCheckStatus::UpToDate;
            next.available = None;
        }
        CheckResult::Unconfigured => {
            next.status = UpdateCheckStatus::Unconfigured;
            next.available = None;
        }
        CheckResult::Failed(message) => {
            next.status = UpdateCheckStatus::Failed;
            next.error = Some(message);
        }
    }
    next
}

// ---------------------------------------------------------------- 維護

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaintenanceKind {
    Update,
    Uninstall,
}

impl MaintenanceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            MaintenanceKind::Update => "update",
            MaintenanceKind::Uninstall => "uninstall",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaintenancePhase {
    /// 停止意圖已生效，正在執行與完全退出相同的停止流程。
    Stopping,
    /// 停止流程（與更新前備份）已完成：更新可以套用、解除安裝可以繼續。Runtime 隨後結束。
    Ready,
    /// 停止之後的步驟失敗（例如更新前備份失敗）：不套用更新。Runtime 仍會結束，下次開啟時結束維護。
    Failed,
}

/// 進行中的維護。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Maintenance {
    pub kind: MaintenanceKind,
    pub phase: MaintenancePhase,
    pub requested_at: Timestamp,
    /// 開始維護時的版本。
    pub from_version: String,
    /// 要更新到的版本；安裝程式直接要求的更新（不經 App）不知道目標版本。
    #[serde(default)]
    pub target_version: Option<String>,
    /// 已下載並驗證簽章的安裝程式（由桌面殼執行）。
    #[serde(default)]
    pub package_path: Option<String>,
    /// 更新前備份的識別。
    #[serde(default)]
    pub backup_id: Option<String>,
    /// 失敗原因或附註。
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub completed_at: Option<Timestamp>,
}

/// 一次已結束的維護（顯示「已更新到 x」用）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MaintenanceOutcome {
    pub kind: MaintenanceKind,
    pub from_version: String,
    /// 結束維護時（也就是重新開啟後）執行中的版本。
    pub running_version: String,
    pub target_version: Option<String>,
    /// 更新是否已套用；解除安裝後又重新安裝時為 `None`。
    pub applied: Option<bool>,
    pub requested_at: Timestamp,
    pub finished_at: Timestamp,
    pub backup_id: Option<String>,
    pub message: Option<String>,
}

/// 維護的持久化狀態（只有一份）。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct MaintenanceRecord {
    pub current: Option<Maintenance>,
    pub last: Option<MaintenanceOutcome>,
    pub version: i64,
    pub updated_at: Option<Timestamp>,
}

impl MaintenanceRecord {
    /// 寫進生命週期檔的維護抑制；沒有進行中的維護時為 `None`。
    pub fn suppression(&self) -> Option<MaintenanceSuppression> {
        self.current
            .as_ref()
            .map(|m| MaintenanceSuppression { kind: m.kind.as_str().to_string(), requested_at: m.requested_at })
    }

    fn push(&mut self, event_type: &'static str, now: Timestamp, payload: serde_json::Value) -> DomainEvent {
        self.version += 1;
        self.updated_at = Some(now);
        DomainEvent {
            aggregate_type: MAINTENANCE_AGGREGATE,
            aggregate_id: MAINTENANCE_ID.to_string(),
            aggregate_version: self.version,
            event_type,
            occurred_at: now,
            payload,
        }
    }
}

/// 開始維護的要求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaintenanceRequest {
    pub kind: MaintenanceKind,
    pub from_version: String,
    pub target_version: Option<String>,
    pub package_path: Option<String>,
}

impl MaintenanceRequest {
    pub fn update(from_version: &str, target_version: Option<String>, package_path: Option<String>) -> Self {
        Self { kind: MaintenanceKind::Update, from_version: from_version.to_string(), target_version, package_path }
    }

    pub fn uninstall(from_version: &str) -> Self {
        Self { kind: MaintenanceKind::Uninstall, from_version: from_version.to_string(), target_version: None, package_path: None }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MaintenanceError {
    #[error("Runtime 正在完全退出（或已退出、尚未重新開啟），現在不能開始維護")]
    StopIntentActive,
    #[error("已有進行中的維護（{}）", .0.as_str())]
    InProgress(MaintenanceKind),
}

impl MaintenanceError {
    pub fn code(&self) -> &'static str {
        match self {
            MaintenanceError::StopIntentActive => "stop_intent_active",
            MaintenanceError::InProgress(_) => "maintenance_in_progress",
        }
    }
}

/// 開始維護。呼叫端在同一個交易中以 `lifecycle::request_exit` 持久化停止意圖。
/// 同一種維護的重複要求沒有任何變動（`Ok(None)`）；另一種維護進行中、或完全退出的停止意圖已生效時拒絕。
pub fn begin(
    record: &MaintenanceRecord,
    lifecycle: &LifecycleState,
    request: MaintenanceRequest,
    now: Timestamp,
) -> Result<Option<(MaintenanceRecord, DomainEvent)>, MaintenanceError> {
    if let Some(current) = &record.current {
        return if current.kind == request.kind { Ok(None) } else { Err(MaintenanceError::InProgress(current.kind)) };
    }
    if lifecycle.stop_intent_active() {
        return Err(MaintenanceError::StopIntentActive);
    }
    let mut next = record.clone();
    let event = next.push(
        EVENT_MAINTENANCE_REQUESTED,
        now,
        serde_json::json!({
            "kind": request.kind,
            "from_version": request.from_version,
            "target_version": request.target_version,
        }),
    );
    next.current = Some(Maintenance {
        kind: request.kind,
        phase: MaintenancePhase::Stopping,
        requested_at: now,
        from_version: request.from_version,
        target_version: request.target_version,
        package_path: request.package_path,
        backup_id: None,
        message: None,
        completed_at: None,
    });
    Ok(Some((next, event)))
}

/// 停止流程之後的步驟結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AfterStop {
    /// 可以套用（更新）或繼續解除安裝；`note` 是不影響結果的問題（例如登記移除失敗，安裝程式會再移除一次）。
    Ready { backup_id: Option<String>, note: Option<String> },
    Failed(String),
}

/// 停止流程之後：進入「準備套用」或「失敗」。只有停止中的維護才接受，而且只完成一次。
pub fn complete(record: &MaintenanceRecord, result: AfterStop, now: Timestamp) -> Option<(MaintenanceRecord, DomainEvent)> {
    let current = record.current.as_ref().filter(|m| m.phase == MaintenancePhase::Stopping)?;
    let mut updated = current.clone();
    updated.completed_at = Some(now);
    let event_type = match result {
        AfterStop::Ready { backup_id, note } => {
            updated.phase = MaintenancePhase::Ready;
            updated.backup_id = backup_id;
            updated.message = note;
            EVENT_MAINTENANCE_READY
        }
        AfterStop::Failed(message) => {
            updated.phase = MaintenancePhase::Failed;
            updated.message = Some(message);
            EVENT_MAINTENANCE_FAILED
        }
    };
    let mut next = record.clone();
    let event = next.push(
        event_type,
        now,
        serde_json::json!({ "kind": updated.kind, "backup_id": updated.backup_id, "message": updated.message }),
    );
    next.current = Some(updated);
    Some((next, event))
}

/// 使用者手動開啟後結束維護，記下結果；沒有進行中的維護時為 `None`。
pub fn finish(
    record: &MaintenanceRecord,
    running_version: &str,
    now: Timestamp,
) -> Option<(MaintenanceRecord, MaintenanceOutcome, DomainEvent)> {
    let current = record.current.as_ref()?;
    let applied = match (current.kind, &current.target_version) {
        (MaintenanceKind::Uninstall, _) => None,
        (MaintenanceKind::Update, Some(target)) => Some(running_version == target),
        (MaintenanceKind::Update, None) => Some(running_version != current.from_version),
    };
    let outcome = MaintenanceOutcome {
        kind: current.kind,
        from_version: current.from_version.clone(),
        running_version: running_version.to_string(),
        target_version: current.target_version.clone(),
        applied,
        requested_at: current.requested_at,
        finished_at: now,
        backup_id: current.backup_id.clone(),
        message: current.message.clone(),
    };
    let mut next = record.clone();
    let event = next.push(
        EVENT_MAINTENANCE_FINISHED,
        now,
        serde_json::json!({
            "kind": outcome.kind,
            "from_version": outcome.from_version,
            "running_version": outcome.running_version,
            "applied": outcome.applied,
        }),
    );
    next.current = None;
    next.last = Some(outcome.clone());
    Some((next, outcome, event))
}

#[cfg(test)]
mod tests;
