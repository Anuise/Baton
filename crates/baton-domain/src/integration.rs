//! 整合佇列（票 20）：開發 Task 的成果通過驗證後，平台分支排入單一整合佇列，
//! 依序在整合目錄合併、執行整合驗證，通過才把目標分支前移並讓 Task 完成。
//!
//! 純邏輯、無 I/O。幾條貫穿全檔的規則：
//! - **單一佇列、一次一個**：全部 Workspace 共用一個佇列，依排入順序處理；任何時刻最多一個項目在處理中
//!   （`merging`、`verifying`、`advancing`），資料庫的唯一索引也保證這點。
//! - **未通過整合不能完成**：成果接受通過後，開發 Task 仍停在 `verifying`；只有整合驗證通過、目標分支前移成功，
//!   Task 才進入 `completed`。合併衝突、整合驗證未通過、目標分支已被更新都讓 Task 回到 `ready`（可修正），
//!   證據留在整合項目中。
//! - **目標分支在驗證通過前不動**：合併與驗證都在平台專用的整合目錄（detached HEAD）中進行，
//!   失敗時主線完全不受影響。
//! - **崩潰後重做而不猜測**：停在 `merging` 或 `verifying` 的項目重新從合併開始（整合目錄會被重設）；
//!   停在 `advancing` 的項目以同一個合併結果重試前移（前移是冪等的）。

use serde::{Deserialize, Serialize};

use crate::event::DomainEvent;
use crate::task::{Task, TaskStatus, Versioned};
use crate::verification::{CheckResult, CheckStatus, VerificationCheck};
use crate::workdir::{self, BATON_DIR};
use crate::workspace::{Workspace, WorkspaceStatus};
use crate::Timestamp;

pub const AGGREGATE_TYPE: &str = "integration";
pub const EVENT_QUEUED: &str = "integration_queued";
pub const EVENT_MERGE_STARTED: &str = "integration_merge_started";
pub const EVENT_MERGED: &str = "integration_merged";
pub const EVENT_CHECKS_PASSED: &str = "integration_checks_passed";
pub const EVENT_INTEGRATED: &str = "integration_completed";
pub const EVENT_FAILED: &str = "integration_failed";

/// 整合目錄：`<Workspace>\.baton\integration`（每個 Workspace 一個；佇列一次只處理一個項目，不會同時使用）。
pub const INTEGRATION_DIR: &str = "integration";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationStatus {
    /// 排隊中。
    Queued,
    /// 正在整合目錄合併（已持久化意圖，結果未知）。
    Merging,
    /// 合併成功，正在整合目錄執行整合驗證。
    Verifying,
    /// 整合驗證通過，正在把目標分支前移到合併結果。
    Advancing,
    /// 已整合：目標分支在合併結果上，Task 已完成。
    Integrated,
    /// 未通過整合：Task 不完成（回到 `ready`），原因與證據保存在項目中。
    Failed,
}

impl IntegrationStatus {
    pub const ALL: [IntegrationStatus; 6] = [
        IntegrationStatus::Queued,
        IntegrationStatus::Merging,
        IntegrationStatus::Verifying,
        IntegrationStatus::Advancing,
        IntegrationStatus::Integrated,
        IntegrationStatus::Failed,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            IntegrationStatus::Queued => "queued",
            IntegrationStatus::Merging => "merging",
            IntegrationStatus::Verifying => "verifying",
            IntegrationStatus::Advancing => "advancing",
            IntegrationStatus::Integrated => "integrated",
            IntegrationStatus::Failed => "failed",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.as_str() == raw)
    }

    /// 正在處理中（佇列的「目前這一個」）。
    pub fn is_active(self) -> bool {
        matches!(self, IntegrationStatus::Merging | IntegrationStatus::Verifying | IntegrationStatus::Advancing)
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, IntegrationStatus::Integrated | IntegrationStatus::Failed)
    }
}

/// 整合佇列的一個項目：一個 Task 的一個受驗 Run 的平台分支。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntegrationEntry {
    pub id: String,
    /// 排入順序（Store 指派，全域遞增）；佇列依它處理。
    pub seq: i64,
    pub workspace_id: String,
    pub task_id: String,
    /// 成果接受通過的 Run 與它的世代。
    pub run_id: String,
    pub generation: i64,
    /// 要合併的平台分支與整合目標分支。
    pub branch: String,
    pub target_branch: String,
    pub status: IntegrationStatus,
    /// 每個事件加一；事件的 `aggregate_version` 與它對應。
    pub version: i64,
    /// 整合目錄中的合併結果與合併當時目標分支所在的 commit。
    pub merge_commit: Option<String>,
    pub target_tip: Option<String>,
    /// 整合驗證的結果（依契約宣告的順序）。
    pub checks: Vec<CheckResult>,
    /// 未通過的原因（給使用者看）。
    pub failure: Option<String>,
    pub queued_at: Timestamp,
    pub started_at: Option<Timestamp>,
    pub finished_at: Option<Timestamp>,
}

/// 全域佇列的總覽：目前處理中的一個（若有）與全部項目（依排入順序）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntegrationQueue {
    pub active: Option<String>,
    pub entries: Vec<IntegrationEntry>,
}

/// Workspace 的整合目錄。
pub fn integration_dir(workspace: &Workspace) -> String {
    workdir::join(&workspace.path, &[BATON_DIR, INTEGRATION_DIR])
}

/// 成果接受通過時，為開發 Task 建立的整合項目（`seq` 由 Store 寫入時指派，這裡先填 0）。
pub fn enqueue(
    id: String,
    task: &Task,
    run_id: &str,
    generation: i64,
    branch: &str,
    target_branch: &str,
    now: Timestamp,
) -> (IntegrationEntry, DomainEvent) {
    let mut entry = IntegrationEntry {
        id,
        seq: 0,
        workspace_id: task.workspace_id.clone(),
        task_id: task.id.clone(),
        run_id: run_id.to_string(),
        generation,
        branch: branch.to_string(),
        target_branch: target_branch.to_string(),
        status: IntegrationStatus::Queued,
        version: 0,
        merge_commit: None,
        target_tip: None,
        checks: Vec::new(),
        failure: None,
        queued_at: now,
        started_at: None,
        finished_at: None,
    };
    let event = push(&mut entry, EVENT_QUEUED, now, serde_json::json!({ "branch": branch, "target_branch": target_branch }));
    (entry, event)
}

/// 佇列中下一個要處理的項目：有處理中的就是它（崩潰後接續），否則是最早排入的 `queued`。
pub fn next(entries: &[IntegrationEntry]) -> Option<&IntegrationEntry> {
    entries
        .iter()
        .find(|e| e.status.is_active())
        .or_else(|| entries.iter().filter(|e| e.status == IntegrationStatus::Queued).min_by_key(|e| e.seq))
}

/// 這個 Run 已經排入整合（不論結果）：成果接受不再重做，也不再接受手動接受。
pub fn has_entry_for_run(entries: &[IntegrationEntry], run_id: &str) -> bool {
    entries.iter().any(|e| e.run_id == run_id)
}

// ---------------------------------------------------------------- 命令

/// 合併的結果（Runtime 把 WorkspaceFs 的結果轉成這個型別；Domain 不依賴 Port）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeResult {
    Merged { commit: String, target_tip: String },
    Conflict { detail: String },
    /// 無法執行合併（Port 錯誤等）。
    Error { reason: String },
}

/// 前移目標分支的結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdvanceResult {
    Advanced,
    TargetMoved { actual: String },
    Blocked { detail: String },
    Error { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntegrationCommand {
    /// 開始（或崩潰後重新開始）合併：`queued`、`merging`、`verifying` → `merging`，先持久化才執行合併。
    /// Task 已經不在驗證中（例如被取消）或 Workspace 已封存時，項目直接以失敗結束，不合併。
    BeginMerge { id: String },
    RecordMerge { id: String, result: MergeResult },
    /// 整合驗證的結果（依契約宣告的順序，一項對一項）。
    RecordChecks { id: String, results: Vec<CheckResult> },
    RecordAdvance { id: String, result: AdvanceResult },
}

impl IntegrationCommand {
    fn id(&self) -> &str {
        match self {
            IntegrationCommand::BeginMerge { id }
            | IntegrationCommand::RecordMerge { id, .. }
            | IntegrationCommand::RecordChecks { id, .. }
            | IntegrationCommand::RecordAdvance { id, .. } => id,
        }
    }
}

/// 判定需要的目前狀態：項目本身、它的 Task、Workspace 與 Task 目前契約宣告的檢查。
pub struct IntegrationContext<'a> {
    pub entry: &'a IntegrationEntry,
    pub workspace: Option<&'a Workspace>,
    pub task: Option<&'a Task>,
    /// Task 目前生效的契約宣告的確定性檢查（整合驗證執行同一份清單）。
    pub checks: &'a [VerificationCheck],
}

#[derive(Debug, Clone, PartialEq)]
pub struct IntegrationChange {
    pub entry: IntegrationEntry,
    /// 因整合結束而改變的 Task（完成或回到 `ready`）。
    pub task: Option<Task>,
    pub events: Vec<DomainEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IntegrationError {
    #[error("找不到整合項目 {0}")]
    NotFound(String),
    #[error("整合項目目前是 {}，不能{}", .from.as_str(), .action)]
    TransitionInvalid { from: IntegrationStatus, action: &'static str },
    #[error("整合驗證的結果與契約宣告的檢查不一致")]
    ResultsMismatch,
}

impl IntegrationError {
    pub fn code(&self) -> &'static str {
        match self {
            IntegrationError::NotFound(_) => "integration_not_found",
            IntegrationError::TransitionInvalid { .. } => "integration_transition_invalid",
            IntegrationError::ResultsMismatch => "integration_results_mismatch",
        }
    }
}

/// 套用一個命令。
pub fn decide(
    ctx: &IntegrationContext<'_>,
    command: IntegrationCommand,
    now: Timestamp,
) -> Result<IntegrationChange, IntegrationError> {
    if ctx.entry.id != command.id() {
        return Err(IntegrationError::NotFound(command.id().to_string()));
    }
    let mut entry = ctx.entry.clone();
    let mut events = Vec::new();
    let invalid = |action| IntegrationError::TransitionInvalid { from: ctx.entry.status, action };
    match command {
        IntegrationCommand::BeginMerge { .. } => {
            if !matches!(entry.status, IntegrationStatus::Queued | IntegrationStatus::Merging | IntegrationStatus::Verifying) {
                return Err(invalid("開始合併"));
            }
            if let Some(reason) = cannot_integrate(ctx) {
                return fail(ctx, entry, events, reason, now);
            }
            let restarted = entry.status != IntegrationStatus::Queued;
            entry.status = IntegrationStatus::Merging;
            entry.merge_commit = None;
            entry.target_tip = None;
            entry.checks = Vec::new();
            entry.started_at.get_or_insert(now);
            events.push(push(&mut entry, EVENT_MERGE_STARTED, now, serde_json::json!({ "restarted": restarted })));
            Ok(IntegrationChange { entry, task: None, events })
        }
        IntegrationCommand::RecordMerge { result, .. } => {
            if entry.status != IntegrationStatus::Merging {
                return Err(invalid("記錄合併結果"));
            }
            match result {
                MergeResult::Merged { commit, target_tip } => {
                    entry.status = IntegrationStatus::Verifying;
                    events.push(push(
                        &mut entry,
                        EVENT_MERGED,
                        now,
                        serde_json::json!({ "merge_commit": commit, "target_tip": target_tip }),
                    ));
                    entry.merge_commit = Some(commit);
                    entry.target_tip = Some(target_tip);
                    Ok(IntegrationChange { entry, task: None, events })
                }
                MergeResult::Conflict { detail } => {
                    let reason = format!("合併「{}」到「{}」時發生衝突，已放棄合併：{detail}", entry.branch, entry.target_branch);
                    fail(ctx, entry, events, reason, now)
                }
                MergeResult::Error { reason } => {
                    fail(ctx, entry, events, format!("無法在整合目錄合併：{reason}"), now)
                }
            }
        }
        IntegrationCommand::RecordChecks { results, .. } => {
            if entry.status != IntegrationStatus::Verifying {
                return Err(invalid("記錄整合驗證結果"));
            }
            let matches =
                ctx.checks.len() == results.len() && ctx.checks.iter().zip(&results).all(|(d, r)| *d == r.check);
            if !matches {
                return Err(IntegrationError::ResultsMismatch);
            }
            let failed = results.iter().filter(|r| r.status == CheckStatus::Failed).count();
            entry.checks = results;
            if failed > 0 {
                let reason = format!("合併後的整合驗證未通過：{} 項確定性檢查中有 {failed} 項未通過", entry.checks.len());
                return fail(ctx, entry, events, reason, now);
            }
            entry.status = IntegrationStatus::Advancing;
            let total = entry.checks.len();
            events.push(push(&mut entry, EVENT_CHECKS_PASSED, now, serde_json::json!({ "checks": total })));
            Ok(IntegrationChange { entry, task: None, events })
        }
        IntegrationCommand::RecordAdvance { result, .. } => {
            if entry.status != IntegrationStatus::Advancing {
                return Err(invalid("記錄目標分支的前移結果"));
            }
            match result {
                AdvanceResult::Advanced => {
                    entry.status = IntegrationStatus::Integrated;
                    entry.finished_at = Some(now);
                    let payload =
                        serde_json::json!({ "merge_commit": entry.merge_commit, "target_branch": entry.target_branch });
                    events.push(push(&mut entry, EVENT_INTEGRATED, now, payload));
                    let task = move_task(ctx, &mut events, TaskStatus::Completed, "integration_passed", now);
                    Ok(IntegrationChange { entry, task, events })
                }
                AdvanceResult::TargetMoved { actual } => {
                    let reason = format!(
                        "整合期間目標分支「{}」已被更新（合併時在 {}，現在在 {actual}），沒有前移；需要重新整合",
                        entry.target_branch,
                        entry.target_tip.as_deref().unwrap_or("？")
                    );
                    fail(ctx, entry, events, reason, now)
                }
                AdvanceResult::Blocked { detail } => {
                    let reason = format!("無法把「{}」前移到整合結果：{detail}", entry.target_branch);
                    fail(ctx, entry, events, reason, now)
                }
                AdvanceResult::Error { reason } => {
                    let reason = format!("無法把「{}」前移到整合結果：{reason}", entry.target_branch);
                    fail(ctx, entry, events, reason, now)
                }
            }
        }
    }
}

/// 不能（再）整合的原因：Task 已不在驗證中、受驗 Run 已不是這個項目的 Run，或 Workspace 已封存。
fn cannot_integrate(ctx: &IntegrationContext<'_>) -> Option<String> {
    if ctx.workspace.is_none_or(|w| w.status == WorkspaceStatus::Archived) {
        return Some("Workspace 已經封存，不再整合".into());
    }
    match ctx.task {
        None => Some("找不到這個 Task".into()),
        Some(task) if task.status != TaskStatus::Verifying => {
            Some(format!("Task 目前是 {} 狀態，已不在等待整合，不再整合", task.status.as_str()))
        }
        Some(_) => None,
    }
}

/// 以失敗結束：Task 若仍在驗證中就回到 `ready`（可修正、可重新派工）。
fn fail(
    ctx: &IntegrationContext<'_>,
    mut entry: IntegrationEntry,
    mut events: Vec<DomainEvent>,
    reason: String,
    now: Timestamp,
) -> Result<IntegrationChange, IntegrationError> {
    entry.status = IntegrationStatus::Failed;
    entry.finished_at = Some(now);
    events.push(push(&mut entry, EVENT_FAILED, now, serde_json::json!({ "reason": reason })));
    entry.failure = Some(reason);
    let task = move_task(ctx, &mut events, TaskStatus::Ready, "integration_failed", now);
    Ok(IntegrationChange { entry, task, events })
}

fn move_task(
    ctx: &IntegrationContext<'_>,
    events: &mut Vec<DomainEvent>,
    to: TaskStatus,
    reason: &str,
    now: Timestamp,
) -> Option<Task> {
    let current = ctx.task.filter(|t| t.status == TaskStatus::Verifying)?;
    let mut moved = current.clone();
    let mut versioned = Versioned { task: &mut moved, events: Vec::new(), now };
    versioned.change_status(to, reason).ok()?;
    events.extend(versioned.events);
    Some(moved)
}

fn push(entry: &mut IntegrationEntry, event_type: &'static str, now: Timestamp, payload: serde_json::Value) -> DomainEvent {
    entry.version += 1;
    DomainEvent {
        aggregate_type: AGGREGATE_TYPE,
        aggregate_id: entry.id.clone(),
        aggregate_version: entry.version,
        event_type,
        occurred_at: now,
        payload: serde_json::json!({ "task_id": entry.task_id, "run_id": entry.run_id, "detail": payload }),
    }
}

#[cfg(test)]
mod tests;
