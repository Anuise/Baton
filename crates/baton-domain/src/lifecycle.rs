//! 桌面生命週期與完全退出（票 25；DR-20261021-01、spec D-11、D-25）。
//!
//! 純邏輯、無 I/O。幾條貫穿全檔的規則：
//! - **停止意圖先持久化**：完全退出的第一步是把停止意圖寫進控制資料庫（並由呼叫端鏡像到生命週期檔），
//!   之後佇列重試、額度恢復與交接都經過 `dispatch_halt`，看到停止意圖就不再啟動新的 Run。
//! - **完全退出不改變 Task 的結論**：被停下的 Run 以 `interrupted` 結束，Task 退回 `ready` 並重新排入派工佇列；
//!   不會因為退出而被標為取消或完成。
//! - **重新開啟先核對、再依各 Workspace 設定續跑**：停止意圖生效時，每個 Workspace 都被「暫停」；
//!   重新開啟並核對完程序之後，只有開啟「完全退出後自動續跑」的 Workspace 解除暫停，其餘等使用者按「繼續工作」。
//!   這個設定預設關閉，變更設定不會立即啟動或停止目前的工作。
//! - **有界的停止**：要求 Run 在 30 秒內保存並停止，再以 10 秒終止剩餘程序樹並確認（spec D-11），總上限 40 秒；
//!   無法確認停止的程序記為停止異常，不假裝已停止（它仍持有寫入所有權）。

use serde::{Deserialize, Serialize};

use crate::event::DomainEvent;
use crate::scheduler::{WaitCategory, WaitReason};
use crate::workspace::{Workspace, WorkspaceStatus};
use crate::Timestamp;

pub const LIFECYCLE_AGGREGATE: &str = "runtime_lifecycle";
pub const LIFECYCLE_ID: &str = "runtime";
pub const WORKSPACE_LIFECYCLE_AGGREGATE: &str = "workspace_lifecycle";

pub const EVENT_FULL_EXIT_REQUESTED: &str = "full_exit_requested";
pub const EVENT_FULL_EXIT_COMPLETED: &str = "full_exit_completed";
pub const EVENT_RUNTIME_REOPENED: &str = "runtime_reopened";
pub const EVENT_WORKSPACE_PAUSED_BY_EXIT: &str = "workspace_paused_by_exit";
pub const EVENT_WORKSPACE_RESUMED: &str = "workspace_resumed";
pub const EVENT_WORKSPACE_AUTO_RESUME_CHANGED: &str = "workspace_auto_resume_changed";

/// 完全退出時，Run 被停下（程序確認停止或根本沒有程序）後的結束原因。
pub const RUN_STOPPED_BY_EXIT_REASON: &str = "stopped_by_full_exit";
/// 重新開啟核對時，發現上次退出沒能確認停止的程序已經不在。
pub const RUN_GONE_AFTER_EXIT_REASON: &str = "process_gone_after_full_exit";

/// 受控停止的寬限期（spec D-11）：通知 Run 保存進度並停止。
pub const STOP_GRACE_MS: i64 = 30_000;
/// 寬限期過後終止剩餘程序樹並確認的上限（spec D-11）。
pub const KILL_CONFIRM_MS: i64 = 10_000;

/// 完全退出的時間參數；預設值即 spec D-11（Validation Required 的初始值）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExitTiming {
    pub stop_grace_ms: i64,
    pub kill_confirm_ms: i64,
}

impl Default for ExitTiming {
    fn default() -> Self {
        Self { stop_grace_ms: STOP_GRACE_MS, kill_confirm_ms: KILL_CONFIRM_MS }
    }
}

impl ExitTiming {
    /// 從要求退出到停止流程結束的總上限。
    pub fn total_ms(&self) -> i64 {
        self.stop_grace_ms + self.kill_confirm_ms
    }
}

// ---------------------------------------------------------------- Runtime 的生命週期

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecyclePhase {
    /// 沒有停止意圖。
    #[default]
    Running,
    /// 停止意圖已生效，正在停止 Run。
    Stopping,
    /// 停止流程已結束；Runtime 隨後結束。下次開啟時先核對再依設定續跑。
    Stopped,
}

impl LifecyclePhase {
    pub const ALL: [LifecyclePhase; 3] = [LifecyclePhase::Running, LifecyclePhase::Stopping, LifecyclePhase::Stopped];

    pub fn as_str(self) -> &'static str {
        match self {
            LifecyclePhase::Running => "running",
            LifecyclePhase::Stopping => "stopping",
            LifecyclePhase::Stopped => "stopped",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.as_str() == raw)
    }
}

/// 一個 Run 在完全退出中的處理結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStopOutcome {
    /// 還在等程序停止。
    Pending,
    /// 沒有程序（還沒啟動，或啟動結果不明且找不到程序）：直接停下。
    NotStarted,
    /// 程序在寬限期內自行結束。
    StoppedGracefully,
    /// 寬限期過後被終止整個程序樹，並已確認停止。
    ForceKilled,
    /// 無法確認停止：仍持有寫入所有權，記為停止異常。
    Anomaly,
}

impl RunStopOutcome {
    /// 程序確定已不在（或從未存在），Run 可以結束。
    pub fn is_stopped(self) -> bool {
        matches!(self, RunStopOutcome::NotStarted | RunStopOutcome::StoppedGracefully | RunStopOutcome::ForceKilled)
    }
}

/// 停止異常的種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopAnomaly {
    /// 終止程序樹之後仍有程序存活。
    StillAlive,
    /// 停止的呼叫失敗。
    StopFailed,
    /// 到總上限仍沒有得到停止結果。
    StopUnconfirmed,
}

impl StopAnomaly {
    pub fn code(self) -> &'static str {
        match self {
            StopAnomaly::StillAlive => "still_alive",
            StopAnomaly::StopFailed => "stop_failed",
            StopAnomaly::StopUnconfirmed => "stop_unconfirmed",
        }
    }
}

/// 一個 Run 的停止紀錄（顯示停止進度與異常用）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunStopRecord {
    pub run_id: String,
    pub workspace_id: String,
    pub task_id: String,
    pub outcome: RunStopOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anomaly: Option<StopAnomaly>,
    /// 給使用者看的說明（異常原因等）；沒有時為 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl RunStopRecord {
    pub fn pending(run_id: &str, workspace_id: &str, task_id: &str) -> Self {
        Self {
            run_id: run_id.to_string(),
            workspace_id: workspace_id.to_string(),
            task_id: task_id.to_string(),
            outcome: RunStopOutcome::Pending,
            anomaly: None,
            message: None,
        }
    }
}

/// 一次完全退出的經過：何時要求、最晚何時結束、每個 Run 的處理結果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExitReport {
    pub requested_at: Timestamp,
    /// `requested_at` 加上總上限（spec D-11 的 40 秒）。
    pub deadline_at: Timestamp,
    /// 停止流程結束的時間；進行中為 `None`。
    #[serde(default)]
    pub completed_at: Option<Timestamp>,
    #[serde(default)]
    pub runs: Vec<RunStopRecord>,
    /// 停止意圖寫入生命週期檔失敗等非 Run 的問題。
    #[serde(default)]
    pub notes: Vec<String>,
}

impl ExitReport {
    pub fn anomalies(&self) -> impl Iterator<Item = &RunStopRecord> {
        self.runs.iter().filter(|r| r.outcome == RunStopOutcome::Anomaly)
    }

    /// 更新一個 Run 的處理結果（沒有這個 Run 時追加）。
    pub fn record(&mut self, record: RunStopRecord) {
        match self.runs.iter_mut().find(|r| r.run_id == record.run_id) {
            Some(existing) => *existing = record,
            None => self.runs.push(record),
        }
    }
}

/// Runtime 的生命週期狀態（只有一份）。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct LifecycleState {
    pub phase: LifecyclePhase,
    /// 每次變更加一；事件的 `aggregate_version` 與它對應。重新開啟時以它確認「核對期間沒有新的退出要求」。
    pub version: i64,
    /// 最近一次完全退出（重新開啟後仍保留，供顯示上次的停止異常）。
    pub last_exit: Option<ExitReport>,
    pub updated_at: Option<Timestamp>,
}

impl LifecycleState {
    /// 停止意圖生效中：不能啟動任何新的 Run。
    pub fn stop_intent_active(&self) -> bool {
        self.phase != LifecyclePhase::Running
    }

    fn push(&mut self, event_type: &'static str, now: Timestamp, payload: serde_json::Value) -> DomainEvent {
        self.version += 1;
        self.updated_at = Some(now);
        DomainEvent {
            aggregate_type: LIFECYCLE_AGGREGATE,
            aggregate_id: LIFECYCLE_ID.to_string(),
            aggregate_version: self.version,
            event_type,
            occurred_at: now,
            payload,
        }
    }
}

/// Workspace 被暫停的原因：決定等待「繼續工作」時的說明，以及重新開啟時哪些暫停可由「完全退出後自動續跑」解除。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PauseCause {
    /// 完全退出（DR-01）：重新開啟時依「完全退出後自動續跑」解除。
    FullExit,
    /// 跨 Windows 登入啟動，而這個 Workspace 沒有開啟「跨登入自動續跑」（DR-07，票 33）。
    NewLogin,
    /// 無法核對這次啟動是否跨了登入（取不到登入識別）：不自動續跑（DR-07，票 33）。
    LoginUnverified,
}

impl PauseCause {
    pub const ALL: [PauseCause; 3] = [PauseCause::FullExit, PauseCause::NewLogin, PauseCause::LoginUnverified];

    pub fn as_str(self) -> &'static str {
        match self {
            PauseCause::FullExit => "full_exit",
            PauseCause::NewLogin => "new_login",
            PauseCause::LoginUnverified => "login_unverified",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.as_str() == raw)
    }
}

/// 每個 Workspace 的生命週期設定與暫停狀態。從未設定過的 Workspace 等同預設值（兩種自動續跑都關閉、未暫停）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceLifecycle {
    pub workspace_id: String,
    /// 「完全退出後自動續跑」（DR-01）；預設關閉。
    pub auto_resume_after_exit: bool,
    /// 「跨登入自動續跑」（DR-07，票 33）；預設關閉。與 `auto_resume_after_exit` 分開保存，互不影響。
    #[serde(default)]
    pub auto_resume_across_login: bool,
    /// 被暫停的時間；有值代表還在等「繼續工作」（或 Runtime 仍在停止中）。
    pub paused_since: Option<Timestamp>,
    /// 暫停的原因；沒有暫停時為 `None`。
    #[serde(default)]
    pub paused_by: Option<PauseCause>,
    pub version: i64,
    pub updated_at: Option<Timestamp>,
}

impl WorkspaceLifecycle {
    pub fn new(workspace_id: &str) -> Self {
        Self {
            workspace_id: workspace_id.to_string(),
            auto_resume_after_exit: false,
            auto_resume_across_login: false,
            paused_since: None,
            paused_by: None,
            version: 0,
            updated_at: None,
        }
    }

    pub(crate) fn push(&mut self, event_type: &'static str, now: Timestamp, payload: serde_json::Value) -> DomainEvent {
        self.version += 1;
        self.updated_at = Some(now);
        DomainEvent {
            aggregate_type: WORKSPACE_LIFECYCLE_AGGREGATE,
            aggregate_id: self.workspace_id.clone(),
            aggregate_version: self.version,
            event_type,
            occurred_at: now,
            payload,
        }
    }
}

/// 查詢某個 Workspace 的設定；沒有紀錄時是預設值。
pub fn workspace_lifecycle_of(all: &[WorkspaceLifecycle], workspace_id: &str) -> WorkspaceLifecycle {
    all.iter().find(|w| w.workspace_id == workspace_id).cloned().unwrap_or_else(|| WorkspaceLifecycle::new(workspace_id))
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LifecycleError {
    #[error("Runtime 正在完全退出（或已退出、尚未重新開啟），現在不能繼續工作")]
    StopIntentActive,
}

impl LifecycleError {
    pub fn code(&self) -> &'static str {
        match self {
            LifecycleError::StopIntentActive => "stop_intent_active",
        }
    }
}

/// 套用結果：新的 Runtime 狀態、有變動的 Workspace 設定與事件。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LifecycleChange {
    pub state: Option<LifecycleState>,
    pub workspaces: Vec<WorkspaceLifecycle>,
    pub events: Vec<DomainEvent>,
}

/// 要求完全退出：持久化停止意圖，並暫停每個使用中的 Workspace。
/// 已經在停止中時沒有任何變動（重複按「完全退出」是冪等的）；上次退出後尚未重新開啟（`stopped`）時視為新的退出要求。
pub fn request_exit(
    state: &LifecycleState,
    workspaces: &[Workspace],
    existing: &[WorkspaceLifecycle],
    timing: ExitTiming,
    now: Timestamp,
) -> LifecycleChange {
    if state.phase == LifecyclePhase::Stopping {
        return LifecycleChange::default();
    }
    let mut next = state.clone();
    next.phase = LifecyclePhase::Stopping;
    let report = ExitReport {
        requested_at: now,
        deadline_at: now.plus_millis(timing.total_ms()),
        completed_at: None,
        runs: Vec::new(),
        notes: Vec::new(),
    };
    let mut events = vec![next.push(
        EVENT_FULL_EXIT_REQUESTED,
        now,
        serde_json::json!({ "deadline_at": report.deadline_at, "timing": timing }),
    )];
    next.last_exit = Some(report);

    let mut paused = Vec::new();
    for workspace in workspaces.iter().filter(|w| w.status == WorkspaceStatus::Active) {
        let mut lifecycle = workspace_lifecycle_of(existing, &workspace.id);
        if lifecycle.paused_since.is_some() {
            continue;
        }
        lifecycle.paused_since = Some(now);
        lifecycle.paused_by = Some(PauseCause::FullExit);
        events.push(lifecycle.push(EVENT_WORKSPACE_PAUSED_BY_EXIT, now, serde_json::json!({})));
        paused.push(lifecycle);
    }
    LifecycleChange { state: Some(next), workspaces: paused, events }
}

/// 更新停止進度（不產生事件；進度只是同一次退出的經過）。只有停止中才接受。
pub fn record_progress(state: &LifecycleState, report: ExitReport, now: Timestamp) -> Option<LifecycleState> {
    if state.phase != LifecyclePhase::Stopping {
        return None;
    }
    let mut next = state.clone();
    next.last_exit = Some(report);
    next.updated_at = Some(now);
    Some(next)
}

/// 停止流程結束：記下結束時間與最終結果（含停止異常）。
pub fn complete_exit(state: &LifecycleState, mut report: ExitReport, now: Timestamp) -> Option<(LifecycleState, DomainEvent)> {
    if state.phase != LifecyclePhase::Stopping {
        return None;
    }
    report.completed_at = Some(now);
    let mut next = state.clone();
    next.phase = LifecyclePhase::Stopped;
    let anomalies: Vec<serde_json::Value> = report
        .anomalies()
        .map(|r| serde_json::json!({ "run_id": r.run_id, "anomaly": r.anomaly, "message": r.message }))
        .collect();
    let stopped = report.runs.iter().filter(|r| r.outcome.is_stopped()).count();
    let event = next.push(
        EVENT_FULL_EXIT_COMPLETED,
        now,
        serde_json::json!({ "runs": report.runs.len(), "stopped": stopped, "anomalies": anomalies }),
    );
    next.last_exit = Some(report);
    Some((next, event))
}

/// 重新開啟（核對完程序之後）：解除停止意圖，只有開啟「完全退出後自動續跑」的 Workspace 解除暫停。
/// 只解除因完全退出而起的暫停；跨登入造成的暫停（票 33）不由這個設定解除，照樣等「繼續工作」。
/// `expected_version` 是 Runtime 啟動時看到的版本；核對期間若又有新的退出要求（版本變了），不重新開啟。
pub fn reopen(
    state: &LifecycleState,
    expected_version: i64,
    workspaces: &[WorkspaceLifecycle],
    now: Timestamp,
) -> Option<LifecycleChange> {
    if !state.stop_intent_active() || state.version != expected_version {
        return None;
    }
    let mut next = state.clone();
    let from = next.phase;
    next.phase = LifecyclePhase::Running;
    let mut resumed = Vec::new();
    let mut awaiting = Vec::new();
    let mut changed = Vec::new();
    let mut workspace_events = Vec::new();
    for lifecycle in workspaces.iter().filter(|w| w.paused_since.is_some()) {
        let paused_by_exit = lifecycle.paused_by.unwrap_or(PauseCause::FullExit) == PauseCause::FullExit;
        if lifecycle.auto_resume_after_exit && paused_by_exit {
            let mut released = lifecycle.clone();
            released.paused_since = None;
            released.paused_by = None;
            workspace_events.push(released.push(EVENT_WORKSPACE_RESUMED, now, serde_json::json!({ "by": "auto_resume" })));
            resumed.push(released.workspace_id.clone());
            changed.push(released);
        } else {
            awaiting.push(lifecycle.workspace_id.clone());
        }
    }
    let mut events = vec![next.push(
        EVENT_RUNTIME_REOPENED,
        now,
        serde_json::json!({ "from": from, "resumed": resumed, "awaiting_resume": awaiting }),
    )];
    events.extend(workspace_events);
    Some(LifecycleChange { state: Some(next), workspaces: changed, events })
}

/// 使用者按「繼續工作」：解除這個 Workspace 的暫停。停止意圖仍生效時拒絕；沒有暫停時沒有任何變動。
pub fn resume_workspace(
    state: &LifecycleState,
    lifecycle: &WorkspaceLifecycle,
    now: Timestamp,
) -> Result<Option<(WorkspaceLifecycle, DomainEvent)>, LifecycleError> {
    if state.stop_intent_active() {
        return Err(LifecycleError::StopIntentActive);
    }
    if lifecycle.paused_since.is_none() {
        return Ok(None);
    }
    let mut next = lifecycle.clone();
    next.paused_since = None;
    next.paused_by = None;
    let event = next.push(EVENT_WORKSPACE_RESUMED, now, serde_json::json!({ "by": "user" }));
    Ok(Some((next, event)))
}

/// 變更「完全退出後自動續跑」。只影響下次重新開啟，不會立即啟動或停止目前的工作；相同值沒有任何變動。
pub fn set_auto_resume(
    lifecycle: &WorkspaceLifecycle,
    enabled: bool,
    now: Timestamp,
) -> Option<(WorkspaceLifecycle, DomainEvent)> {
    if lifecycle.auto_resume_after_exit == enabled {
        return None;
    }
    let mut next = lifecycle.clone();
    next.auto_resume_after_exit = enabled;
    let event = next.push(EVENT_WORKSPACE_AUTO_RESUME_CHANGED, now, serde_json::json!({ "enabled": enabled }));
    Some((next, event))
}

/// 派工（含佇列重試、額度恢復後的派工與交接）是否被生命週期擋住；被擋住時回傳等待原因。
/// 所有會建立新 Run 的路徑都經過這裡（Scheduler 的 `evaluate`），啟動程序前也再檢查一次（`launch_halted`）。
pub fn dispatch_halt(state: &LifecycleState, workspace: &WorkspaceLifecycle, workspace_name: &str) -> Option<WaitReason> {
    if state.stop_intent_active() {
        return Some(WaitReason {
            category: WaitCategory::Lifecycle,
            code: "runtime_stopping".to_string(),
            message: "Runtime 正在完全退出（或已退出、尚未重新開啟），不啟動新的 Run".to_string(),
            profile_id: None,
            guard: None,
        });
    }
    workspace.paused_since.map(|_| WaitReason {
        category: WaitCategory::Lifecycle,
        code: "awaiting_resume".to_string(),
        message: match workspace.paused_by.unwrap_or(PauseCause::FullExit) {
            PauseCause::FullExit => format!("Workspace「{workspace_name}」在完全退出後等待「繼續工作」"),
            PauseCause::NewLogin => {
                format!("Workspace「{workspace_name}」未開啟跨登入自動續跑，重新登入 Windows 後等待「繼續工作」")
            }
            PauseCause::LoginUnverified => {
                format!("無法核對這次啟動是否跨了 Windows 登入，Workspace「{workspace_name}」等待「繼續工作」")
            }
        },
        profile_id: None,
        guard: None,
    })
}

/// 啟動程序（launch intent）前的最後一道檢查：停止意圖生效或 Workspace 被暫停時不啟動。
pub fn launch_halted(state: &LifecycleState, workspace: &WorkspaceLifecycle) -> bool {
    state.stop_intent_active() || workspace.paused_since.is_some()
}

// ---------------------------------------------------------------- 生命週期檔

/// 生命週期檔的格式版本。
pub const LIFECYCLE_FILE_VERSION: u32 = 1;

/// 生命週期檔的檔名（位於資料目錄）。
pub const LIFECYCLE_FILE_NAME: &str = "lifecycle.json";

/// 生命週期檔：停止意圖的鏡像（spec D-25），讓 launcher 不開資料庫就能判斷是否受停止抑制。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifecycleFile {
    pub version: u32,
    /// 停止抑制：有值代表停止意圖生效中。
    pub stop: Option<StopSuppression>,
    /// 維護抑制（更新、解除安裝；由票 34 的流程寫入）：有值時監護與登入啟動都不拉起 Runtime（票 32）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maintenance: Option<MaintenanceSuppression>,
    pub updated_at: Timestamp,
}

/// 維護抑制的內容。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MaintenanceSuppression {
    /// `update`、`uninstall` 等。
    pub kind: String,
    pub requested_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StopSuppression {
    pub requested_at: Timestamp,
    pub reason: String,
}

impl LifecycleFile {
    /// 依資料庫中的狀態產生鏡像。
    pub fn mirror(state: &LifecycleState, now: Timestamp) -> Self {
        let stop = state.stop_intent_active().then(|| StopSuppression {
            requested_at: state.last_exit.as_ref().map(|r| r.requested_at).unwrap_or(now),
            reason: "full_exit".to_string(),
        });
        Self { version: LIFECYCLE_FILE_VERSION, stop, maintenance: None, updated_at: now }
    }
}

/// 生命週期檔讀取的結果。檔案毀損（讀得到卻解析不了）時保守地視為停止（spec D-25）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileStopView {
    Missing,
    Clear,
    Stop { requested_at: Option<Timestamp> },
}

/// 資料庫與生命週期檔是否一致；不一致時採較保守者（視為停止）。回傳需要補寫進資料庫的停止意圖時間。
pub fn conservative_stop(state: &LifecycleState, file: &FileStopView) -> Option<Option<Timestamp>> {
    match file {
        FileStopView::Stop { requested_at } if !state.stop_intent_active() => Some(*requested_at),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
