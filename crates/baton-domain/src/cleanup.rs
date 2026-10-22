//! Worktree 自動清理（票 21，ADR-0002／DR-20261021-02）：開發 Task 通過驗證、成果已整合、相關程序都已停止後，
//! 平台移除它登記的 worktree，回收磁碟空間。
//!
//! 純邏輯、無 I/O。幾條貫穿全檔的規則：
//! - **只在 Task 完成且整合後觸發**：Task 是 `completed`、最新的整合項目是 `integrated`、這個 Task 的 Run 全部結束。
//!   暫停、交接、失敗、取消、完全退出都不會讓 Task 完成，所以都不會觸發清理。
//! - **意圖先持久化**：符合條件時先登記「待清理」，之後每一次嘗試都先持久化「清理中」才在交易外移除；
//!   Runtime 在任何一點崩潰，重建後都從登記接續，而不是猜測。
//! - **每次嘗試重新核對安全條件**：目標必須仍是 repo 登記、簽出平台分支的 worktree（或已經不存在），
//!   而且沒有未保存的變更或歸屬不明的檔案；不符合就不刪，記下原因後有限重試。
//! - **Task 完成與清理狀態分開**：清理失敗不會改變 Task 的狀態，Task 仍然是已完成。
//! - **只移除工作副本**：不刪除 Git 分支、歷史或正式 Artifact（成果在成果儲存中，證據在控制資料庫中）。

use serde::{Deserialize, Serialize};

use crate::event::DomainEvent;
use crate::integration::{IntegrationEntry, IntegrationStatus};
use crate::run::Run;
use crate::task::{Task, TaskStatus};
use crate::workdir::{self, Workdir, WorkdirKind, BATON_DIR};
use crate::workspace::Workspace;
use crate::Timestamp;

pub const AGGREGATE_TYPE: &str = "worktree_cleanup";
pub const EVENT_REQUESTED: &str = "worktree_cleanup_requested";
pub const EVENT_STARTED: &str = "worktree_cleanup_started";
pub const EVENT_RETRY_SCHEDULED: &str = "worktree_cleanup_retry_scheduled";
pub const EVENT_CLEANED: &str = "worktree_cleaned";
pub const EVENT_FAILED: &str = "worktree_cleanup_failed";

/// 移除時先把 worktree 改名到這裡（`.baton\trash\<task_id>`）再刪除；崩潰後留下的只會是平台自己的殘留。
pub const TRASH_DIR: &str = "trash";

/// 第一次嘗試失敗後，最多再自動重試幾次（spec D-16 的基礎設施重試預算）。
pub const MAX_RETRIES: i64 = 5;
/// 重試間隔：從 1 分鐘起每次加倍，上限 10 分鐘（spec D-16）。
pub const RETRY_BASE_MS: i64 = 60_000;
pub const RETRY_CAP_MS: i64 = 600_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CleanupStatus {
    /// 待清理：意圖已持久化，等待第一次嘗試或下一次重試（`failure` 是上一次失敗的原因）。
    Pending,
    /// 清理中：已持久化這一次嘗試，正在核對與移除（結果未知）。
    Removing,
    /// 已清理：worktree 已移除。
    Cleaned,
    /// 清理失敗：重試用完，不再自動重試；worktree 保留，原因在 `failure`。
    Failed,
}

impl CleanupStatus {
    pub const ALL: [CleanupStatus; 4] =
        [CleanupStatus::Pending, CleanupStatus::Removing, CleanupStatus::Cleaned, CleanupStatus::Failed];

    pub fn as_str(self) -> &'static str {
        match self {
            CleanupStatus::Pending => "pending",
            CleanupStatus::Removing => "removing",
            CleanupStatus::Cleaned => "cleaned",
            CleanupStatus::Failed => "failed",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.as_str() == raw)
    }
}

/// 一個 worktree 的清理登記（與工作目錄登記一對一）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeCleanup {
    pub task_id: String,
    pub workspace_id: String,
    /// 要移除的 worktree（登記當時的工作目錄路徑）。
    pub path: String,
    /// worktree 應該簽出的平台分支（核對歸屬用）。
    pub branch: String,
    pub status: CleanupStatus,
    /// 每個事件加一；事件的 `aggregate_version` 與它對應。
    pub version: i64,
    /// 已經開始過的嘗試次數（崩潰後接續同一次嘗試不另外計算）。
    pub attempts: i64,
    /// 最近一次失敗的原因（給使用者看）；清理成功後清空。
    pub failure: Option<String>,
    pub requested_at: Timestamp,
    /// `pending` 時下一次可以嘗試的時間。
    pub next_attempt_at: Option<Timestamp>,
    /// 已清理或清理失敗的時間。
    pub finished_at: Option<Timestamp>,
}

/// worktree 移除時暫放的目錄：`<Workspace>\.baton\trash\<task_id>`。
pub fn trash_dir(workspace: &Workspace, task_id: &str) -> String {
    workdir::join(&workspace.path, &[BATON_DIR, TRASH_DIR, task_id])
}

/// 第 `attempts` 次嘗試失敗後的重試間隔。
pub fn retry_delay_ms(attempts: i64) -> i64 {
    let doublings = (attempts - 1).clamp(0, 10) as u32;
    (RETRY_BASE_MS << doublings).min(RETRY_CAP_MS)
}

// ---------------------------------------------------------------- 觸發

/// 這個工作目錄現在是否該登記待清理：平台建立並登記的 worktree、Task 已完成、最新的整合項目已整合、
/// 這個 Task 的 Run 全部結束，而且還沒有清理登記。
pub fn should_request(
    workdir: &Workdir,
    task: &Task,
    runs: &[Run],
    integrations: &[IntegrationEntry],
    existing: Option<&WorktreeCleanup>,
) -> bool {
    existing.is_none()
        && workdir.kind == WorkdirKind::Worktree
        && workdir.is_ready()
        && workdir.branch.is_some()
        && task.id == workdir.task_id
        && task.status == TaskStatus::Completed
        && integrations
            .iter()
            .filter(|e| e.task_id == task.id)
            .max_by_key(|e| e.seq)
            .is_some_and(|e| e.status == IntegrationStatus::Integrated)
        && runs.iter().filter(|r| r.task_id == task.id).all(|r| r.status.is_terminal())
}

/// 登記待清理（呼叫端先以 [`should_request`] 確認）。
pub fn request(workdir: &Workdir, now: Timestamp) -> (WorktreeCleanup, DomainEvent) {
    let mut cleanup = WorktreeCleanup {
        task_id: workdir.task_id.clone(),
        workspace_id: workdir.workspace_id.clone(),
        path: workdir.path.clone(),
        branch: workdir.branch.clone().unwrap_or_default(),
        status: CleanupStatus::Pending,
        version: 0,
        attempts: 0,
        failure: None,
        requested_at: now,
        next_attempt_at: Some(now),
        finished_at: None,
    };
    let event = push(&mut cleanup, EVENT_REQUESTED, now, serde_json::json!({ "branch": workdir.branch }));
    (cleanup, event)
}

/// 現在要不要處理：到了重試時間的待清理，或停在清理中（Runtime 在嘗試途中崩潰，重建後接續）。
pub fn is_due(cleanup: &WorktreeCleanup, now: Timestamp) -> bool {
    match cleanup.status {
        CleanupStatus::Pending => cleanup.next_attempt_at.is_none_or(|at| at <= now),
        CleanupStatus::Removing => true,
        CleanupStatus::Cleaned | CleanupStatus::Failed => false,
    }
}

// ---------------------------------------------------------------- 安全條件

/// 移除前核對到的 worktree 實際狀態（Runtime 把 WorkspaceFs 的結果轉成這個型別；Domain 不依賴 Port）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inspection {
    /// 目錄已經不存在（上一次嘗試已經移走，或被使用者手動刪除）；只剩平台自己的殘留要收拾。
    Missing,
    /// repo 登記的 worktree：簽出 `branch`（detached HEAD 為 `None`），`unsaved` 是未提交的變更與未追蹤的檔案。
    Registered { branch: Option<String>, unsaved: Vec<String> },
    /// 目錄存在，但不是這個 repo 登記的 worktree（或是主 checkout）：歸屬不明。
    Foreign,
}

/// 顯示未保存檔案時最多列出幾個。
const LISTED_FILES: usize = 5;

/// 依核對結果判斷能不能移除；不能時回傳給使用者看的原因。
pub fn check_safety(cleanup: &WorktreeCleanup, inspection: &Inspection) -> Result<(), String> {
    match inspection {
        Inspection::Missing => Ok(()),
        Inspection::Foreign => Err(format!(
            "{} 已經不是平台登記的 worktree（歸屬不明），為避免誤刪不會移除；請確認後自行處理",
            cleanup.path
        )),
        Inspection::Registered { branch, .. } if branch.as_deref() != Some(cleanup.branch.as_str()) => Err(format!(
            "worktree 目前簽出的是 {}，不是平台分支 {}（歸屬不明），為避免誤刪不會移除",
            branch.as_deref().unwrap_or("detached HEAD"),
            cleanup.branch
        )),
        Inspection::Registered { unsaved, .. } if !unsaved.is_empty() => {
            let mut listed: Vec<&str> = unsaved.iter().take(LISTED_FILES).map(String::as_str).collect();
            if unsaved.len() > LISTED_FILES {
                listed.push("…");
            }
            Err(format!(
                "worktree 中有 {} 個未保存的變更或歸屬不明的檔案（{}），為避免遺失不會移除；請提交或移除後平台會再試",
                unsaved.len(),
                listed.join("、")
            ))
        }
        Inspection::Registered { .. } => Ok(()),
    }
}

// ---------------------------------------------------------------- 命令

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CleanupCommand {
    /// 開始一次嘗試：`pending`（已到重試時間）→ `removing`，先持久化才核對與移除。
    /// 停在 `removing`（崩潰後接續）時也可以再開始，但不另外計算嘗試次數。
    Begin { task_id: String },
    /// 這次嘗試的結果：`Ok` 為已移除；`Err` 為原因（安全條件不符或移除失敗），依剩餘重試次數回到待清理或清理失敗。
    Finish { task_id: String, result: Result<(), String> },
}

impl CleanupCommand {
    pub fn task_id(&self) -> &str {
        match self {
            CleanupCommand::Begin { task_id } | CleanupCommand::Finish { task_id, .. } => task_id,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CleanupChange {
    pub cleanup: WorktreeCleanup,
    pub events: Vec<DomainEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CleanupError {
    #[error("找不到 Task {0} 的清理登記")]
    NotFound(String),
    #[error("清理目前是 {}，不能{}", .from.as_str(), .action)]
    TransitionInvalid { from: CleanupStatus, action: &'static str },
    #[error("還沒到下一次重試的時間")]
    NotDue,
}

impl CleanupError {
    pub fn code(&self) -> &'static str {
        match self {
            CleanupError::NotFound(_) => "cleanup_not_found",
            CleanupError::TransitionInvalid { .. } => "cleanup_transition_invalid",
            CleanupError::NotDue => "cleanup_not_due",
        }
    }
}

/// 套用一個命令。
pub fn decide(cleanup: &WorktreeCleanup, command: CleanupCommand, now: Timestamp) -> Result<CleanupChange, CleanupError> {
    if cleanup.task_id != command.task_id() {
        return Err(CleanupError::NotFound(command.task_id().to_string()));
    }
    let mut next = cleanup.clone();
    let invalid = |action| CleanupError::TransitionInvalid { from: cleanup.status, action };
    let event = match command {
        CleanupCommand::Begin { .. } => {
            let resumed = match cleanup.status {
                CleanupStatus::Pending if is_due(cleanup, now) => false,
                CleanupStatus::Pending => return Err(CleanupError::NotDue),
                CleanupStatus::Removing => true,
                CleanupStatus::Cleaned | CleanupStatus::Failed => return Err(invalid("開始清理")),
            };
            if !resumed {
                next.attempts += 1;
            }
            next.status = CleanupStatus::Removing;
            next.next_attempt_at = None;
            let payload = serde_json::json!({ "attempt": next.attempts, "resumed": resumed });
            push(&mut next, EVENT_STARTED, now, payload)
        }
        CleanupCommand::Finish { result, .. } => {
            if cleanup.status != CleanupStatus::Removing {
                return Err(invalid("記錄清理結果"));
            }
            match result {
                Ok(()) => {
                    next.status = CleanupStatus::Cleaned;
                    next.failure = None;
                    next.finished_at = Some(now);
                    let payload = serde_json::json!({ "attempt": next.attempts });
                    push(&mut next, EVENT_CLEANED, now, payload)
                }
                Err(reason) if next.attempts <= MAX_RETRIES => {
                    let at = now.plus_millis(retry_delay_ms(next.attempts));
                    next.status = CleanupStatus::Pending;
                    next.next_attempt_at = Some(at);
                    let payload = serde_json::json!({ "attempt": next.attempts, "reason": reason, "next_attempt_at": at });
                    next.failure = Some(reason);
                    push(&mut next, EVENT_RETRY_SCHEDULED, now, payload)
                }
                Err(reason) => {
                    next.status = CleanupStatus::Failed;
                    next.finished_at = Some(now);
                    let payload = serde_json::json!({ "attempt": next.attempts, "reason": reason });
                    next.failure = Some(reason);
                    push(&mut next, EVENT_FAILED, now, payload)
                }
            }
        }
    };
    Ok(CleanupChange { cleanup: next, events: vec![event] })
}

fn push(cleanup: &mut WorktreeCleanup, event_type: &'static str, now: Timestamp, payload: serde_json::Value) -> DomainEvent {
    cleanup.version += 1;
    DomainEvent {
        aggregate_type: AGGREGATE_TYPE,
        aggregate_id: cleanup.task_id.clone(),
        aggregate_version: cleanup.version,
        event_type,
        occurred_at: now,
        payload: serde_json::json!({ "path": cleanup.path, "detail": payload }),
    }
}

#[cfg(test)]
mod tests;
