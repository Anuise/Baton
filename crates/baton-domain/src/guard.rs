//! Guard 與重犯事件（票 38，spec D-28、使用者故事 60、93）。
//!
//! 純邏輯、無 I/O。幾條貫穿全檔的規則：
//! - **Guard 來自已驗證的教訓，一律由使用者啟用**：只有「有效」（已驗證且在有效期間內）的教訓能轉成 Guard；
//!   只有桌面憑證能建立、啟用、停用（D-28）。使用者建立 Guard 即代表啟用。
//! - **有效的 Guard = 啟用 + 來源教訓仍有效**：教訓被撤銷、淘汰、取代或過期之後，Guard 自動不再生效，
//!   錯誤的教訓不會繼續擋工作。
//! - **在 Run 准入（派工交易）時檢查**：只檢查來源教訓範圍涵蓋的 Task（全域、同 Workspace、同 Goal、同 Task）。
//!   三種規則：啟動前檢查（契約某欄位必須提到指定文字）、工具限制（不得要求某項能力）、重試政策（目前契約版本失敗次數上限）。
//! - **重犯事件可追溯到教訓**：Guard 擋下即記一筆「已攔下」的重犯事件（同一個 Guard、Task 與契約版本只記一次）；
//!   沒被攔下的重犯由使用者在知識分頁回報。兩者都帶來源教訓，Guard 成效由這兩種事件計算。

use serde::{Deserialize, Serialize};

use crate::authorization::Capability;
use crate::event::DomainEvent;
use crate::knowledge::{Audience, KnowledgeEntry, KnowledgeKind, KnowledgeStatus};
use crate::run::{Run, RunStatus};
use crate::task::{ContractField, ContractFieldName, Task, TaskContract};
use crate::Timestamp;

pub const AGGREGATE: &str = "guard";
pub const EVENT_CREATED: &str = "guard_created";
pub const EVENT_ENABLED: &str = "guard_enabled";
pub const EVENT_DISABLED: &str = "guard_disabled";
pub const RECURRENCE_AGGREGATE: &str = "lesson_recurrence";
pub const EVENT_RECURRENCE_RECORDED: &str = "lesson_recurrence_recorded";

/// 啟動前檢查要求提到的文字最多幾個字元。
pub const MUST_MENTION_MAX_CHARS: usize = 200;
/// 重試政策的失敗次數上限最多可以設多少。
pub const MAX_FAILED_RUNS_LIMIT: u32 = 20;
/// 回報重犯的說明最多幾個字元。
pub const NOTE_MAX_CHARS: usize = 1_000;

/// Guard 的規則：可機械化檢查的教訓。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GuardRule {
    /// 啟動前檢查：契約的某個欄位必須（不分大小寫）提到指定文字。
    Preflight { field: ContractFieldName, must_mention: String },
    /// 工具限制：Task 不得要求這項能力。
    ToolRestriction { capability: Capability },
    /// 重試政策：同一個 Task 在目前契約版本下失敗的 Run 達到上限後不再派工（改契約即重新計算）。
    RetryPolicy { max_failed_runs: u32 },
}

impl GuardRule {
    pub fn kind_str(&self) -> &'static str {
        match self {
            GuardRule::Preflight { .. } => "preflight",
            GuardRule::ToolRestriction { .. } => "tool_restriction",
            GuardRule::RetryPolicy { .. } => "retry_policy",
        }
    }

    /// 給使用者看的規則說明。
    pub fn describe(&self) -> String {
        match self {
            GuardRule::Preflight { field, must_mention } => {
                format!("啟動前檢查：契約的「{}」必須提到「{must_mention}」", field.label())
            }
            GuardRule::ToolRestriction { capability } => format!("工具限制：不得要求「{}」", capability.label()),
            GuardRule::RetryPolicy { max_failed_runs } => {
                format!("重試政策：同一份契約失敗 {max_failed_runs} 次後不再派工")
            }
        }
    }

    /// 驗證並正規化（去除前後空白）。
    fn validated(self) -> Result<Self, GuardError> {
        match self {
            GuardRule::Preflight { field, must_mention } => {
                let text = must_mention.trim().to_string();
                if text.is_empty() {
                    return Err(GuardError::Invalid("啟動前檢查必須指定要提到的文字".into()));
                }
                if text.chars().count() > MUST_MENTION_MAX_CHARS {
                    return Err(GuardError::Invalid(format!("要提到的文字最多 {MUST_MENTION_MAX_CHARS} 個字元")));
                }
                Ok(GuardRule::Preflight { field, must_mention: text })
            }
            GuardRule::RetryPolicy { max_failed_runs } if !(1..=MAX_FAILED_RUNS_LIMIT).contains(&max_failed_runs) => {
                Err(GuardError::Invalid(format!("失敗次數上限必須介於 1 到 {MAX_FAILED_RUNS_LIMIT}")))
            }
            rule => Ok(rule),
        }
    }

    /// 這條規則對一個 Task 的違反說明；沒有違反回傳 `None`。
    fn violation(&self, task: &Task, contract: &TaskContract, runs: &[Run]) -> Option<String> {
        match self {
            GuardRule::Preflight { field, must_mention } => {
                let mentioned = match contract.content.field(*field) {
                    Some(ContractField::Filled { text, .. }) => {
                        text.to_lowercase().contains(&must_mention.to_lowercase())
                    }
                    _ => false,
                };
                (!mentioned).then(|| format!("契約的「{}」沒有提到「{must_mention}」", field.label()))
            }
            GuardRule::ToolRestriction { capability } => contract
                .content
                .required_capabilities
                .contains(capability)
                .then(|| format!("Task 要求了被禁止的能力「{}」", capability.label())),
            GuardRule::RetryPolicy { max_failed_runs } => {
                let failed = runs
                    .iter()
                    .filter(|r| r.task_id == task.id && r.status == RunStatus::Failed)
                    .filter(|r| r.contract_version == task.contract_version)
                    .count();
                (failed >= *max_failed_runs as usize).then(|| {
                    format!("這份契約（第 {} 版）已經失敗 {failed} 次，達到上限 {max_failed_runs} 次", task.contract_version)
                })
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuardStatus {
    Enabled,
    Disabled,
}

impl GuardStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            GuardStatus::Enabled => "enabled",
            GuardStatus::Disabled => "disabled",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        [GuardStatus::Enabled, GuardStatus::Disabled].into_iter().find(|s| s.as_str() == raw)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Guard {
    pub id: String,
    /// 來源教訓（knowledge_entry，`kind = lesson`）。
    pub lesson_id: String,
    pub rule: GuardRule,
    pub status: GuardStatus,
    /// 每次變更加一；啟用、停用時以它做 expected_revision 校驗。
    pub revision: i64,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

/// Run 准入時要檢查的 Guard：Store 已確認「啟用且來源教訓現在有效」，附上教訓本身（範圍與標題）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveGuard {
    pub guard: Guard,
    pub lesson: KnowledgeEntry,
}

/// Guard 的來源，給等待原因與 UI 顯示「哪個教訓擋下的」。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuardSource {
    pub guard_id: String,
    pub lesson_id: String,
    pub lesson_title: String,
}

/// 一個 Guard 擋下一個 Task 的原因。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuardViolation {
    pub guard_id: String,
    pub lesson_id: String,
    pub lesson_title: String,
    pub rule: GuardRule,
    pub message: String,
}

impl GuardViolation {
    pub fn source(&self) -> GuardSource {
        GuardSource {
            guard_id: self.guard_id.clone(),
            lesson_id: self.lesson_id.clone(),
            lesson_title: self.lesson_title.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GuardError {
    #[error("內容不正確：{0}")]
    Invalid(String),
    #[error("找不到 Guard {0}")]
    NotFound(String),
    #[error("{0}")]
    LessonIneligible(String),
    #[error("這個 Guard 已經被修改過（目前是第 {current} 次變更），請重新整理後再操作")]
    RevisionConflict { current: i64 },
}

impl GuardError {
    /// 穩定的機器可讀代碼，API 與 UI 依它判斷錯誤種類。
    pub fn code(&self) -> &'static str {
        match self {
            GuardError::Invalid(_) => "guard_invalid",
            GuardError::NotFound(_) => "guard_not_found",
            GuardError::LessonIneligible(_) => "lesson_ineligible",
            GuardError::RevisionConflict { .. } => "guard_revision_conflict",
        }
    }
}

/// 教訓不是有效教訓的原因；有效回傳 `None`。
fn lesson_problem(lesson: &KnowledgeEntry, now: Timestamp) -> Option<String> {
    if lesson.kind != KnowledgeKind::Lesson {
        return Some("只有教訓可以轉成 Guard 或回報重犯".into());
    }
    match lesson.status {
        KnowledgeStatus::Verified if lesson.is_effective_at(now) => None,
        KnowledgeStatus::Verified => Some("來源教訓不在有效期間內".into()),
        KnowledgeStatus::Candidate => Some("來源教訓還是候選，必須先確認為已驗證".into()),
        KnowledgeStatus::Revoked => Some("來源教訓已撤銷".into()),
        KnowledgeStatus::Retired => Some("來源教訓已淘汰".into()),
        KnowledgeStatus::Superseded => Some("來源教訓已被取代".into()),
    }
}

/// Guard 現在不生效的原因；生效（啟用且來源教訓有效）回傳 `None`。
pub fn inactive_reason(guard: &Guard, lesson: &KnowledgeEntry, now: Timestamp) -> Option<String> {
    if guard.status == GuardStatus::Disabled {
        return Some("已停用".into());
    }
    lesson_problem(lesson, now)
}

/// 一次變更的結果：要寫回的 Guard 與要追加的事件。
#[derive(Debug, Clone, PartialEq)]
pub struct GuardChange {
    pub guard: Guard,
    pub event: DomainEvent,
}

fn event(guard: &Guard, event_type: &'static str, now: Timestamp, payload: serde_json::Value) -> DomainEvent {
    DomainEvent {
        aggregate_type: AGGREGATE,
        aggregate_id: guard.id.clone(),
        aggregate_version: guard.revision,
        event_type,
        occurred_at: now,
        payload,
    }
}

/// 使用者把有效的教訓轉成 Guard；建立即啟用。
pub fn create(id: String, lesson: &KnowledgeEntry, rule: GuardRule, now: Timestamp) -> Result<GuardChange, GuardError> {
    if let Some(problem) = lesson_problem(lesson, now) {
        return Err(GuardError::LessonIneligible(problem));
    }
    let rule = rule.validated()?;
    let guard = Guard {
        id,
        lesson_id: lesson.id.clone(),
        rule,
        status: GuardStatus::Enabled,
        revision: 1,
        created_at: now,
        updated_at: now,
    };
    let payload = serde_json::json!({ "lesson_id": guard.lesson_id, "rule": guard.rule });
    let event = event(&guard, EVENT_CREATED, now, payload);
    Ok(GuardChange { guard, event })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardCommand {
    Enable,
    Disable,
}

/// 啟用或停用；先校驗 expected_revision。已經是目標狀態時沒有任何變動（`None`）。
/// 啟用時來源教訓必須仍有效；停用隨時可以。
pub fn decide(
    current: &Guard,
    lesson: &KnowledgeEntry,
    expected_revision: i64,
    command: GuardCommand,
    now: Timestamp,
) -> Result<Option<GuardChange>, GuardError> {
    if current.revision != expected_revision {
        return Err(GuardError::RevisionConflict { current: current.revision });
    }
    let (status, event_type) = match command {
        GuardCommand::Enable => (GuardStatus::Enabled, EVENT_ENABLED),
        GuardCommand::Disable => (GuardStatus::Disabled, EVENT_DISABLED),
    };
    if current.status == status {
        return Ok(None);
    }
    if command == GuardCommand::Enable {
        if let Some(problem) = lesson_problem(lesson, now) {
            return Err(GuardError::LessonIneligible(problem));
        }
    }
    let mut guard = current.clone();
    guard.status = status;
    guard.revision += 1;
    guard.updated_at = now;
    let event = event(&guard, event_type, now, serde_json::json!({}));
    Ok(Some(GuardChange { guard, event }))
}

/// Run 准入時檢查：範圍涵蓋這個 Task 的每個 Guard 各自判定，回傳全部違反（依傳入順序）。
pub fn violations(guards: &[ActiveGuard], task: &Task, contract: &TaskContract, runs: &[Run]) -> Vec<GuardViolation> {
    let audience = Audience::Run { workspace_id: &task.workspace_id, goal_id: &task.goal_id, task_id: &task.id };
    guards
        .iter()
        .filter(|g| audience.can_see(&g.lesson.scope))
        .filter_map(|g| {
            g.guard.rule.violation(task, contract, runs).map(|message| GuardViolation {
                guard_id: g.guard.id.clone(),
                lesson_id: g.lesson.id.clone(),
                lesson_title: g.lesson.content.title.clone(),
                rule: g.guard.rule.clone(),
                message,
            })
        })
        .collect()
}

// ---------------------------------------------------------------- 重犯事件

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecurrenceKind {
    /// Guard 在 Run 准入時攔下。
    Prevented,
    /// 沒被攔下、由使用者回報。
    Reported,
}

impl RecurrenceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            RecurrenceKind::Prevented => "prevented",
            RecurrenceKind::Reported => "reported",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        [RecurrenceKind::Prevented, RecurrenceKind::Reported].into_iter().find(|k| k.as_str() == raw)
    }
}

/// 一次重犯：教訓警告過的情況又出現了。一律指向來源教訓；被 Guard 攔下時另外指向該 Guard。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recurrence {
    pub id: String,
    pub lesson_id: String,
    pub guard_id: Option<String>,
    pub workspace_id: Option<String>,
    pub task_id: Option<String>,
    /// 攔下時 Task 的契約版本。
    pub contract_version: Option<i64>,
    pub kind: RecurrenceKind,
    pub message: String,
    pub occurred_at: Timestamp,
}

fn recurrence_event(recurrence: &Recurrence) -> DomainEvent {
    DomainEvent {
        aggregate_type: RECURRENCE_AGGREGATE,
        aggregate_id: recurrence.id.clone(),
        aggregate_version: 1,
        event_type: EVENT_RECURRENCE_RECORDED,
        occurred_at: recurrence.occurred_at,
        payload: serde_json::json!({
            "lesson_id": recurrence.lesson_id,
            "guard_id": recurrence.guard_id,
            "task_id": recurrence.task_id,
            "kind": recurrence.kind,
        }),
    }
}

/// Guard 攔下一個 Task 的重犯事件（`message` 是擋下的原因）。識別由 Guard、Task 與契約版本決定，
/// 所以佇列每次重試看到同一個違反只會記一次。
pub fn prevented(source: &GuardSource, message: &str, task: &Task, now: Timestamp) -> (Recurrence, DomainEvent) {
    let recurrence = Recurrence {
        id: format!("rec-{}-{}-v{}", source.guard_id, task.id, task.contract_version),
        lesson_id: source.lesson_id.clone(),
        guard_id: Some(source.guard_id.clone()),
        workspace_id: Some(task.workspace_id.clone()),
        task_id: Some(task.id.clone()),
        contract_version: Some(task.contract_version),
        kind: RecurrenceKind::Prevented,
        message: message.to_string(),
        occurred_at: now,
    };
    let event = recurrence_event(&recurrence);
    (recurrence, event)
}

/// 使用者回報一次沒被攔下的重犯；教訓必須是已驗證的教訓（過期的也可以），可指定相關的 Workspace 或 Task。
/// 指定 Task 時以 Task 所在的 Workspace 為準；只指定 Workspace 時記下它（App 的重犯表單沒填 Task 時就是這樣）。
pub fn report(
    id: String,
    lesson: &KnowledgeEntry,
    workspace_id: Option<&str>,
    task: Option<&Task>,
    note: &str,
    now: Timestamp,
) -> Result<(Recurrence, DomainEvent), GuardError> {
    if lesson.kind != KnowledgeKind::Lesson || lesson.status != KnowledgeStatus::Verified {
        let problem = lesson_problem(lesson, now).unwrap_or_else(|| "只有已驗證的教訓可以回報重犯".into());
        return Err(GuardError::LessonIneligible(problem));
    }
    let note = note.trim();
    if note.is_empty() {
        return Err(GuardError::Invalid("必須說明重犯的經過".into()));
    }
    if note.chars().count() > NOTE_MAX_CHARS {
        return Err(GuardError::Invalid(format!("說明最多 {NOTE_MAX_CHARS} 個字元")));
    }
    let recurrence = Recurrence {
        id,
        lesson_id: lesson.id.clone(),
        guard_id: None,
        workspace_id: task.map(|t| t.workspace_id.clone()).or_else(|| workspace_id.map(str::to_string)),
        task_id: task.map(|t| t.id.clone()),
        contract_version: task.map(|t| t.contract_version),
        kind: RecurrenceKind::Reported,
        message: note.to_string(),
        occurred_at: now,
    };
    let event = recurrence_event(&recurrence);
    Ok((recurrence, event))
}

#[cfg(test)]
mod tests;
