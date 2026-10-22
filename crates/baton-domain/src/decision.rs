//! 裁定請求與裁定（Decision）：最小裁定中心的狀態機（票 22，spec D-08、D-15、使用者故事 49–57）。
//!
//! 純邏輯、無 I/O。輸入「目前狀態（`DecisionContext`）+ 命令 + 時間」，輸出「新狀態 + 事件 + 喚醒 Outbox」，
//! 由 Store 在同一個交易中套用。幾條貫穿全檔的規則：
//! - **只有桌面憑證能裁定**：`Answer` 命令只能由桌面 API 發出，裁定記下 `decided_by`；Agent 憑證只能提出（`Request`、
//!   `PermissionPrompt`）與查詢。資料庫另有 CHECK 約束，`decided_by` 只能是桌面。
//! - **只阻擋受影響的 Task**：請求的阻擋範圍是明列的 Task 識別；只有這些 Task 進入 `waiting_decision`，
//!   裁定後也只有它們恢復（`ready`），而且要等**所有**阻擋它們的請求都有了答案。其他 Task 完全不受影響。
//! - **逾時不是同意**：期限（`deadline_at`）只用來標示「已逾期」；沒有任何命令會因時間流逝而回答請求、
//!   放行權限提示或讓 Task 恢復。
//! - **權限提示的單次批准（E1）**：範圍外的權限提示建立裁定請求並回「等待裁定」；請求建立時就算好它綁定的
//!   邏輯操作（`approval::ApprovalBinding`：操作識別、目標資源、重要參數、要求雜湊、成果版本）。使用者回答
//!   「允許這一次」之後，同一個 Task（任何一個 Run、任何一個帳號）再次提出時**逐項核對綁定**，全部相符才消耗這個批准
//!   （一次）；任何一項不同就不能沿用，改建新的請求並說明哪一項變了。用過之後再提出同一個操作，Runtime 不知道上一次的
//!   結果，所以新的請求會要求使用者先查證再決定。單次批准從不改變 Workspace 授權。

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::approval::{self, ApprovalBinding, ArtifactMark, Mismatch};
use crate::authorization::{AuthorizationRevision, Capability};
use crate::event::DomainEvent;
use crate::permission::{self, Verdict};
use crate::run::{self, ExecutionLease, Run};
use crate::task::{Task, TaskContext, TaskStatus, Versioned};
use crate::Timestamp;

pub const DECISION_AGGREGATE: &str = "decision_request";
pub const PERMISSION_AUDIT_AGGREGATE: &str = "permission_audit";
pub const EVENT_DECISION_REQUESTED: &str = "decision_requested";
pub const EVENT_DECISION_ANSWERED: &str = "decision_answered";
pub const EVENT_PERMISSION_APPROVAL_USED: &str = "permission_approval_used";
pub const EVENT_PERMISSION_AUTO_ALLOWED: &str = "permission_auto_allowed";

/// 喚醒 Outbox 項目的種類前綴；完整的種類是 `前綴 + 裁定請求識別`（Outbox 的唯一鍵是種類加 Run，
/// 同一個 Run 可以提出多個請求，所以種類要帶上請求識別）。
pub const OUTBOX_WAKE_PREFIX: &str = "decision_wake:";

/// 裁定者：目前只有桌面管理憑證。
pub const DECIDED_BY_DESKTOP: &str = "desktop";

/// 權限提示裁定請求的兩個固定選項。
pub const OPTION_ALLOW_ONCE: &str = "allow_once";
pub const OPTION_DENY: &str = "deny";

pub const QUESTION_MAX_CHARS: usize = 500;
pub const REASON_MAX_CHARS: usize = 1_000;
pub const OPTION_ID_MAX_CHARS: usize = 40;
pub const OPTION_LABEL_MAX_CHARS: usize = 200;
pub const TRADEOFF_MAX_CHARS: usize = 500;
pub const EVIDENCE_MAX_ITEMS: usize = 10;
pub const EVIDENCE_MAX_CHARS: usize = 500;
pub const NOTE_MAX_CHARS: usize = 1_000;
pub const OPTIONS_MIN: usize = 2;
pub const OPTIONS_MAX: usize = 8;
/// 期限最長 30 天（以分鐘計）。
pub const DEADLINE_MAX_MINUTES: i64 = 30 * 24 * 60;

pub fn wake_kind(request_id: &str) -> String {
    format!("{OUTBOX_WAKE_PREFIX}{request_id}")
}

// ---------------------------------------------------------------- 實體

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionKind {
    /// Agent 以 `request_decision` 提出。
    Agent,
    /// 權限提示超出授權範圍而建立。
    Permission,
}

impl DecisionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            DecisionKind::Agent => "agent",
            DecisionKind::Permission => "permission",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        [DecisionKind::Agent, DecisionKind::Permission].into_iter().find(|k| k.as_str() == raw)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionStatus {
    Pending,
    Answered,
}

impl DecisionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            DecisionStatus::Pending => "pending",
            DecisionStatus::Answered => "answered",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        [DecisionStatus::Pending, DecisionStatus::Answered].into_iter().find(|s| s.as_str() == raw)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Urgency {
    Low,
    Normal,
    High,
}

impl Urgency {
    pub fn as_str(self) -> &'static str {
        match self {
            Urgency::Low => "low",
            Urgency::Normal => "normal",
            Urgency::High => "high",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        [Urgency::Low, Urgency::Normal, Urgency::High].into_iter().find(|u| u.as_str() == raw)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionOption {
    pub id: String,
    pub label: String,
    /// 這個選項的取捨（可省略）。
    #[serde(default)]
    pub tradeoff: Option<String>,
}

/// 權限提示的細節（只有 `kind = permission` 的請求有）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionDetail {
    pub tool_name: String,
    pub input: Value,
    /// 這個操作需要的能力；無法歸類時為 `None`。
    pub capability: Option<Capability>,
    pub summary: String,
    pub fingerprint: String,
    /// 單次批准綁定的邏輯操作（票 23）。票 23 之前建立的請求沒有綁定，它們的「允許這一次」不能再被使用。
    #[serde(default)]
    pub binding: Option<ApprovalBinding>,
    /// 同一個操作先前已用掉一次批准、結果未知：使用者要先查證再回答。
    #[serde(default)]
    pub prior_use: Option<PriorUse>,
    /// 先前有一個單次批准，但這次的操作與它的綁定不符而不能沿用。
    #[serde(default)]
    pub rejected_approval: Option<RejectedApproval>,
}

/// 先前已被使用的單次批准。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PriorUse {
    pub request_id: String,
    pub run_id: String,
    pub used_at: Timestamp,
}

/// 不能沿用的單次批准與原因。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RejectedApproval {
    pub request_id: String,
    pub reason: Mismatch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionRequest {
    pub id: String,
    pub workspace_id: String,
    /// 提出請求的 Run 與 Task。
    pub run_id: String,
    pub task_id: String,
    pub kind: DecisionKind,
    pub status: DecisionStatus,
    pub question: String,
    /// 為什麼 Agent 無法自行決定。
    pub reason: String,
    pub options: Vec<DecisionOption>,
    /// 建議的選項識別。
    pub recommendation: Option<String>,
    pub evidence: Vec<String>,
    pub urgency: Urgency,
    pub deadline_at: Option<Timestamp>,
    pub permission: Option<PermissionDetail>,
    /// 阻擋範圍：受影響的 Task（不在這裡的 Task 從頭到尾不受阻擋）。
    pub blocking: Vec<String>,
    pub created_at: Timestamp,
}

impl DecisionRequest {
    pub fn is_pending(&self) -> bool {
        self.status == DecisionStatus::Pending
    }

    pub fn is_overdue(&self, now: Timestamp) -> bool {
        self.is_pending() && self.deadline_at.is_some_and(|d| now > d)
    }

    fn option(&self, id: &str) -> Option<&DecisionOption> {
        self.options.iter().find(|o| o.id == id)
    }
}

/// 使用者對一個請求的正式裁定（追加式）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decision {
    pub request_id: String,
    pub option_id: String,
    pub note: Option<String>,
    pub decided_by: String,
    pub decided_at: Timestamp,
}

/// 單次批准已被消耗的紀錄。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalUse {
    pub request_id: String,
    pub run_id: String,
    pub used_at: Timestamp,
}

/// 裁定後要追加的喚醒 Outbox 項目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeItem {
    pub kind: String,
    pub run_id: String,
    pub payload: Value,
    pub created_at: Timestamp,
}

// ---------------------------------------------------------------- Agent 提出請求的內容

/// Agent 以 `request_decision` 提出的內容（已驗證）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestContent {
    pub question: String,
    pub reason: String,
    pub options: Vec<DecisionOption>,
    pub recommendation: Option<String>,
    pub evidence: Vec<String>,
    pub urgency: Urgency,
    pub deadline_minutes: Option<i64>,
}

impl RequestContent {
    /// 驗證並整理（去除前後空白）；任何一項無效都拒絕整個請求，不寫入任何東西。
    pub fn new(
        question: &str,
        reason: &str,
        options: Vec<DecisionOption>,
        recommendation: Option<&str>,
        evidence: Vec<String>,
        urgency: Option<Urgency>,
        deadline_minutes: Option<i64>,
    ) -> Result<Self, DecisionError> {
        let invalid = |message: String| DecisionError::Invalid(message);
        let text = |name: &str, raw: &str, max: usize| -> Result<String, DecisionError> {
            let value = raw.trim();
            if value.is_empty() {
                return Err(invalid(format!("{name}不可空白")));
            }
            if value.chars().count() > max {
                return Err(invalid(format!("{name}不可超過 {max} 個字元")));
            }
            Ok(value.to_string())
        };
        let question = text("問題", question, QUESTION_MAX_CHARS)?;
        let reason = text("原因（為什麼無法自行決定）", reason, REASON_MAX_CHARS)?;
        if !(OPTIONS_MIN..=OPTIONS_MAX).contains(&options.len()) {
            return Err(invalid(format!("選項必須有 {OPTIONS_MIN} 到 {OPTIONS_MAX} 個")));
        }
        let mut cleaned: Vec<DecisionOption> = Vec::with_capacity(options.len());
        for option in options {
            let id = text("選項識別", &option.id, OPTION_ID_MAX_CHARS)?;
            if !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
                return Err(invalid(format!("選項識別「{id}」只能使用英數字、底線與連字號")));
            }
            if cleaned.iter().any(|o| o.id == id) {
                return Err(invalid(format!("選項識別「{id}」重複")));
            }
            let label = text("選項名稱", &option.label, OPTION_LABEL_MAX_CHARS)?;
            let tradeoff = match option.tradeoff.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
                Some(t) => Some(text("選項的取捨", t, TRADEOFF_MAX_CHARS)?),
                None => None,
            };
            cleaned.push(DecisionOption { id, label, tradeoff });
        }
        let recommendation = match recommendation.map(str::trim).filter(|r| !r.is_empty()) {
            Some(id) if cleaned.iter().any(|o| o.id == id) => Some(id.to_string()),
            Some(id) => return Err(invalid(format!("建議的選項「{id}」不在選項之中"))),
            None => None,
        };
        if evidence.len() > EVIDENCE_MAX_ITEMS {
            return Err(invalid(format!("證據最多 {EVIDENCE_MAX_ITEMS} 項")));
        }
        let evidence = evidence
            .iter()
            .map(|e| text("證據", e, EVIDENCE_MAX_CHARS))
            .collect::<Result<Vec<_>, _>>()?;
        if deadline_minutes.is_some_and(|m| !(1..=DEADLINE_MAX_MINUTES).contains(&m)) {
            return Err(invalid(format!("期限必須是 1 到 {DEADLINE_MAX_MINUTES} 分鐘")));
        }
        Ok(Self { question, reason, options: cleaned, recommendation, evidence, urgency: urgency.unwrap_or(Urgency::Normal), deadline_minutes })
    }
}

// ---------------------------------------------------------------- 命令、結果與錯誤

#[derive(Debug, Clone, PartialEq)]
pub enum DecisionCommand {
    /// Agent 提出裁定請求（`request_decision`）。`request_id` 由呼叫端產生。
    Request { request_id: String, run_id: String, generation: i64, content: RequestContent },
    /// Worker 的權限提示（`permission_prompt`）。`request_id`、`audit_id` 由呼叫端產生，用不到時丟棄。
    PermissionPrompt {
        request_id: String,
        audit_id: String,
        run_id: String,
        generation: i64,
        tool_name: String,
        input: Value,
    },
    /// 使用者（桌面憑證）回答一個請求。
    Answer { request_id: String, option_id: String, note: Option<String>, decided_by: String },
}

/// 等待裁定時，附帶說明先前的單次批准為什麼不能用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalCheck {
    /// 綁定不符（參數、目標、成果版本…改變）。
    Mismatch { request_id: String, reason: Mismatch },
    /// 同一個操作已經用掉一次批准，結果未知，要先查證。
    UsedResultUnknown { request_id: String },
}

impl ApprovalCheck {
    pub fn request_id(&self) -> &str {
        match self {
            ApprovalCheck::Mismatch { request_id, .. } | ApprovalCheck::UsedResultUnknown { request_id } => request_id,
        }
    }

    /// 穩定的機器可讀代碼。
    pub fn code(&self) -> &'static str {
        match self {
            ApprovalCheck::Mismatch { reason, .. } => reason.as_str(),
            ApprovalCheck::UsedResultUnknown { .. } => "used_result_unknown",
        }
    }
}

/// 權限提示給 Worker 的結果。
#[derive(Debug, Clone, PartialEq)]
pub enum PermissionOutcome {
    /// 在授權範圍內：自動允許並已記錄。
    Allowed { capability: Capability, revision: i64 },
    /// 範圍外：等待裁定（Worker 應保存 checkpoint 並以 blocked 結束，或改做其他步驟）。
    /// `check` 說明先前的單次批准為什麼不能用（沒有相關的批准時為 `None`）。
    Waiting { request_id: String, message: String, check: Option<ApprovalCheck> },
    /// 使用者已裁定「允許這一次」，這次提示消耗了它。
    Approved { request_id: String },
    /// 使用者已裁定拒絕（這次提示消耗了它）。
    Denied { request_id: String },
}

#[derive(Debug, Clone, PartialEq)]
pub enum DecisionOutcome {
    /// 請求已建立（`existing` 為真表示同一個請求仍在等待，沒有重複建立）。
    Requested { request_id: String, existing: bool },
    Permission(PermissionOutcome),
    Answered { resumed_task_ids: Vec<String> },
}

/// 命令成功後的結果；沒有任何變動（冪等）時各欄位都是空的。
#[derive(Debug, Clone, PartialEq)]
pub struct DecisionChange {
    /// 新建立的請求。
    pub request: Option<DecisionRequest>,
    /// 請求被回答：新的狀態在 `answered_request`，正式裁定在 `decision`。
    pub answered_request: Option<String>,
    pub decision: Option<Decision>,
    pub approval_use: Option<ApprovalUse>,
    /// 因此改變的 Task。
    pub tasks: Vec<Task>,
    pub outbox: Option<WakeItem>,
    pub events: Vec<DomainEvent>,
    pub outcome: DecisionOutcome,
}

impl DecisionChange {
    fn new(outcome: DecisionOutcome) -> Self {
        Self {
            request: None,
            answered_request: None,
            decision: None,
            approval_use: None,
            tasks: Vec::new(),
            outbox: None,
            events: Vec::new(),
            outcome,
        }
    }

    pub fn changed(&self) -> bool {
        self.request.is_some()
            || self.answered_request.is_some()
            || self.decision.is_some()
            || self.approval_use.is_some()
            || !self.tasks.is_empty()
            || self.outbox.is_some()
            || !self.events.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DecisionError {
    #[error("{0}")]
    Invalid(String),
    #[error("找不到 Workspace {0}")]
    WorkspaceNotFound(String),
    #[error("找不到 Run {0}")]
    RunNotFound(String),
    #[error("找不到 Task {0}")]
    TaskNotFound(String),
    #[error("找不到裁定請求 {0}")]
    NotFound(String),
    #[error("這個 Run 已經結束或已被接替，不能再提出裁定請求")]
    StaleRun,
    #[error("選項「{0}」不在這個裁定請求的選項之中")]
    OptionUnknown(String),
    #[error("這個裁定請求已經回答過了（選擇「{0}」），不能改成別的答案")]
    AlreadyAnswered(String),
    #[error("只有桌面管理憑證提交的裁定才算正式裁定")]
    NotDesktop,
}

impl DecisionError {
    /// 穩定的機器可讀代碼，API 與 UI 依它判斷錯誤種類。
    pub fn code(&self) -> &'static str {
        match self {
            DecisionError::Invalid(_) => "decision_invalid",
            DecisionError::WorkspaceNotFound(_) => "workspace_not_found",
            DecisionError::RunNotFound(_) => "run_not_found",
            DecisionError::TaskNotFound(_) => "task_not_found",
            DecisionError::NotFound(_) => "decision_not_found",
            DecisionError::StaleRun => "run_report_stale",
            DecisionError::OptionUnknown(_) => "decision_option_unknown",
            DecisionError::AlreadyAnswered(_) => "decision_already_answered",
            DecisionError::NotDesktop => "decision_not_desktop",
        }
    }
}

// ---------------------------------------------------------------- 判定

/// 目前狀態的唯讀快照（同一個 Workspace 內）。Store 在交易中載入。
pub struct DecisionContext<'a> {
    pub task: &'a TaskContext<'a>,
    pub runs: &'a [Run],
    pub leases: &'a [ExecutionLease],
    pub requests: &'a [DecisionRequest],
    pub decisions: &'a [Decision],
    pub uses: &'a [ApprovalUse],
    /// 各 Task 成果的最新版本（單次批准綁定的成果版本）。
    pub artifacts: &'a [ArtifactMark],
    /// Workspace 的全部授權 revision（由舊到新）：權限提示依「Run 准入時固定的 revision」判定（R1-a，票 24）。
    pub authorizations: &'a [AuthorizationRevision],
}

impl DecisionContext<'_> {
    fn decision_of(&self, request_id: &str) -> Option<&Decision> {
        self.decisions.iter().find(|d| d.request_id == request_id)
    }

    fn approval_used(&self, request_id: &str) -> bool {
        self.uses.iter().any(|u| u.request_id == request_id)
    }

    fn use_of(&self, request_id: &str) -> Option<&ApprovalUse> {
        self.uses.iter().find(|u| u.request_id == request_id)
    }

    /// Run 沿用的授權：准入時固定的 revision，不是 Workspace 目前生效的 revision（R1-a）。
    /// 准入時還沒有任何授權的 Run 是 `None`：之後才確認的授權不會回頭套用到它身上。
    fn authorization_of(&self, run: &Run) -> Option<&AuthorizationRevision> {
        run.authorization_revision.and_then(|number| self.authorizations.iter().find(|r| r.revision == number))
    }

    /// 確認 Run 仍是目前的寫入所有者，回傳 Run 與它的 Task。
    fn current_run(&self, run_id: &str, generation: i64) -> Result<(&Run, &Task), DecisionError> {
        let run = self.runs.iter().find(|r| r.id == run_id).ok_or_else(|| DecisionError::RunNotFound(run_id.to_string()))?;
        let lease = self.leases.iter().find(|l| l.run_id == run.id);
        if !run::is_current_writer(run, lease, generation) {
            return Err(DecisionError::StaleRun);
        }
        let task = self.task.task(&run.task_id).ok_or_else(|| DecisionError::TaskNotFound(run.task_id.clone()))?;
        Ok((run, task))
    }
}

fn event(
    aggregate_type: &'static str,
    aggregate_id: &str,
    aggregate_version: i64,
    event_type: &'static str,
    now: Timestamp,
    payload: Value,
) -> DomainEvent {
    DomainEvent { aggregate_type, aggregate_id: aggregate_id.to_string(), aggregate_version, event_type, occurred_at: now, payload }
}

pub fn decide(ctx: &DecisionContext<'_>, command: DecisionCommand, now: Timestamp) -> Result<DecisionChange, DecisionError> {
    if ctx.task.workspace.is_none() {
        return Err(DecisionError::WorkspaceNotFound(ctx.task.workspace_id.to_string()));
    }
    match command {
        DecisionCommand::Request { request_id, run_id, generation, content } => {
            request(ctx, request_id, &run_id, generation, content, now)
        }
        DecisionCommand::PermissionPrompt { request_id, audit_id, run_id, generation, tool_name, input } => {
            permission_prompt(ctx, request_id, audit_id, &run_id, generation, &tool_name, input, now)
        }
        DecisionCommand::Answer { request_id, option_id, note, decided_by } => {
            answer(ctx, &request_id, &option_id, note, &decided_by, now)
        }
    }
}

/// 建立請求、讓 Run 的 Task 進入 `waiting_decision`（只有 `running` 的 Task 會轉換）。
fn open_request(
    request: DecisionRequest,
    task: &Task,
    now: Timestamp,
    outcome: DecisionOutcome,
) -> Result<DecisionChange, DecisionError> {
    let mut change = DecisionChange::new(outcome);
    change.events.push(event(
        DECISION_AGGREGATE,
        &request.id,
        1,
        EVENT_DECISION_REQUESTED,
        now,
        serde_json::json!({
            "kind": request.kind,
            "run_id": request.run_id,
            "task_id": request.task_id,
            "question": request.question,
            "blocking": request.blocking,
        }),
    ));
    if task.status == TaskStatus::Running {
        let mut moved = task.clone();
        let mut versioned = Versioned { task: &mut moved, events: Vec::new(), now };
        versioned
            .change_status(TaskStatus::WaitingDecision, EVENT_DECISION_REQUESTED)
            .map_err(|_| DecisionError::TaskNotFound(task.id.clone()))?;
        change.events.extend(versioned.events);
        change.tasks.push(moved);
    }
    change.request = Some(request);
    Ok(change)
}

fn request(
    ctx: &DecisionContext<'_>,
    request_id: String,
    run_id: &str,
    generation: i64,
    content: RequestContent,
    now: Timestamp,
) -> Result<DecisionChange, DecisionError> {
    let (run, task) = ctx.current_run(run_id, generation)?;
    // 同一個 Run 重複提出同一個問題：回到仍在等待的那一個，不重複建立
    if let Some(existing) = ctx
        .requests
        .iter()
        .find(|r| r.is_pending() && r.kind == DecisionKind::Agent && r.run_id == run.id && r.question == content.question)
    {
        return Ok(DecisionChange::new(DecisionOutcome::Requested { request_id: existing.id.clone(), existing: true }));
    }
    let record = DecisionRequest {
        id: request_id.clone(),
        workspace_id: run.workspace_id.clone(),
        run_id: run.id.clone(),
        task_id: task.id.clone(),
        kind: DecisionKind::Agent,
        status: DecisionStatus::Pending,
        question: content.question,
        reason: content.reason,
        options: content.options,
        recommendation: content.recommendation,
        evidence: content.evidence,
        urgency: content.urgency,
        deadline_at: content.deadline_minutes.map(|m| now.plus_millis(m * 60_000)),
        permission: None,
        blocking: vec![task.id.clone()],
        created_at: now,
    };
    open_request(record, task, now, DecisionOutcome::Requested { request_id, existing: false })
}

const WAITING_MESSAGE: &str = "等待裁定：這個操作超出 Workspace 授權範圍，已建立裁定請求交給使用者。\
請先保存 checkpoint，再以 report_blocked 結束，或改做不需要這個操作的其他步驟；不要重試相同操作來繞過。";

#[allow(clippy::too_many_arguments)]
fn permission_prompt(
    ctx: &DecisionContext<'_>,
    request_id: String,
    audit_id: String,
    run_id: &str,
    generation: i64,
    tool_name: &str,
    input: Value,
    now: Timestamp,
) -> Result<DecisionChange, DecisionError> {
    if tool_name.trim().is_empty() {
        return Err(DecisionError::Invalid("權限提示必須帶有工具名稱 tool_name".to_string()));
    }
    let (run, task) = ctx.current_run(run_id, generation)?;
    let workspace = ctx.task.workspace.ok_or_else(|| DecisionError::WorkspaceNotFound(ctx.task.workspace_id.to_string()))?;
    let authorization = ctx.authorization_of(run);
    let summary = permission::summarize(tool_name, &input);

    let (capability, reason) = match permission::evaluate(tool_name, &input, &workspace.path, authorization) {
        Verdict::Allow { capability, revision } => {
            let mut change = DecisionChange::new(DecisionOutcome::Permission(PermissionOutcome::Allowed { capability, revision }));
            change.events.push(event(
                PERMISSION_AUDIT_AGGREGATE,
                &audit_id,
                1,
                EVENT_PERMISSION_AUTO_ALLOWED,
                now,
                serde_json::json!({
                    "run_id": run.id,
                    "task_id": task.id,
                    "tool_name": tool_name,
                    "summary": summary,
                    "capability": capability,
                    "authorization_revision": revision,
                }),
            ));
            return Ok(change);
        }
        Verdict::OutOfScope { capability, reason } => (capability, reason),
    };

    let fingerprint = permission::fingerprint(tool_name, &input);
    let current = approval::bind(tool_name, &input, approval::artifact_version_of(&task.id, ctx.artifacts));
    let is_permission_of_task = |r: &&DecisionRequest| r.kind == DecisionKind::Permission && r.task_id == task.id;
    // 同一個操作＝要求雜湊相同（說明欄位不算）；沒有綁定的舊請求以原本的指紋比對
    let same_operation = |r: &&DecisionRequest| {
        is_permission_of_task(r)
            && r.permission.as_ref().is_some_and(|p| match &p.binding {
                Some(binding) => binding.request_hash == current.request_hash,
                None => p.fingerprint == fingerprint,
            })
    };
    if let Some(pending) = ctx.requests.iter().filter(same_operation).find(|r| r.is_pending()) {
        return Ok(DecisionChange::new(DecisionOutcome::Permission(PermissionOutcome::Waiting {
            request_id: pending.id.clone(),
            message: WAITING_MESSAGE.to_string(),
            check: None,
        })));
    }

    // 已回答、還沒被使用的裁定：由新到舊，第一個適用的被這次提示消耗。
    // 拒絕一律適用；「允許這一次」要在執行時逐項核對綁定，任何一項不同就不能沿用
    let mut check: Option<ApprovalCheck> = None;
    let unused = ctx.requests.iter().filter(same_operation).filter(|r| !ctx.approval_used(&r.id)).rev();
    for answered in unused {
        let Some(decision) = ctx.decision_of(&answered.id) else { continue };
        let usable = decision.option_id == OPTION_DENY
            || match answered.permission.as_ref().and_then(|p| p.binding.as_ref()) {
                Some(binding) => match approval::check(binding, &current) {
                    Ok(()) => true,
                    Err(reason) => {
                        check.get_or_insert(ApprovalCheck::Mismatch { request_id: answered.id.clone(), reason });
                        false
                    }
                },
                // 票 23 之前的批准沒有綁定可以核對：保守地不使用
                None => false,
            };
        if !usable {
            continue;
        }
        let outcome = if decision.option_id == OPTION_ALLOW_ONCE {
            PermissionOutcome::Approved { request_id: answered.id.clone() }
        } else {
            PermissionOutcome::Denied { request_id: answered.id.clone() }
        };
        let mut change = DecisionChange::new(DecisionOutcome::Permission(outcome));
        change.approval_use = Some(ApprovalUse { request_id: answered.id.clone(), run_id: run.id.clone(), used_at: now });
        change.events.push(event(
            DECISION_AGGREGATE,
            &answered.id,
            3,
            EVENT_PERMISSION_APPROVAL_USED,
            now,
            serde_json::json!({
                "run_id": run.id,
                "option_id": decision.option_id,
                "tool_name": tool_name,
                "summary": summary,
                "binding": current,
            }),
        ));
        return Ok(change);
    }

    // 參數或目標改變：同一個 Task 對同一種操作還有未使用的「允許這一次」，但要求不同
    if check.is_none() {
        check = ctx
            .requests
            .iter()
            .filter(is_permission_of_task)
            .filter(|r| !ctx.approval_used(&r.id))
            .filter(|r| ctx.decision_of(&r.id).is_some_and(|d| d.option_id == OPTION_ALLOW_ONCE))
            .rev()
            .find_map(|r| {
                let binding = r.permission.as_ref()?.binding.as_ref()?;
                if binding.operation != current.operation {
                    return None;
                }
                approval::check(binding, &current).err().map(|reason| ApprovalCheck::Mismatch { request_id: r.id.clone(), reason })
            });
    }
    // 同一個操作已經用掉一次批准：Runtime 看不到上一次的結果，要先查證再決定
    let prior_use = ctx
        .requests
        .iter()
        .filter(same_operation)
        .filter(|r| ctx.decision_of(&r.id).is_some_and(|d| d.option_id == OPTION_ALLOW_ONCE))
        .filter_map(|r| ctx.use_of(&r.id))
        .next_back()
        .map(|u| PriorUse { request_id: u.request_id.clone(), run_id: u.run_id.clone(), used_at: u.used_at });
    if let Some(prior) = &prior_use {
        check.get_or_insert(ApprovalCheck::UsedResultUnknown { request_id: prior.request_id.clone() });
    }

    let mut evidence = vec![format!("工具：{tool_name}"), format!("輸入：{}", truncate(&input.to_string(), 300))];
    if let Some(ApprovalCheck::Mismatch { request_id, reason }) = &check {
        evidence.push(format!("先前的單次批准（{request_id}）不能沿用：{}", reason.label()));
    }
    let question = match &prior_use {
        Some(_) => format!("「{summary}」先前已批准並放行過一次，結果未知。查證之後，要再允許一次嗎？"),
        None => format!("允許 Worker 執行「{summary}」嗎？"),
    };
    let mut reason_text = match capability {
        Some(capability) => format!("這個操作需要「{}」，但目前的 Workspace 授權不涵蓋它：{reason}", capability.label()),
        None => format!("Runtime 無法確認這個操作在授權範圍內，所以不自動允許：{reason}"),
    };
    if let Some(prior) = &prior_use {
        evidence.push(format!("前一次批准（{}）已由 Run {} 使用，Runtime 無法得知執行結果", prior.request_id, prior.run_id));
        reason_text.push_str("。同一個操作先前已經放行過一次，Runtime 無法確認上一次是否已經生效；請先查證，避免重複執行");
    }
    let (allow_label, deny_label) = match &prior_use {
        Some(_) => ("已查證，允許再執行一次", "拒絕（不要重送）"),
        None => ("允許本次操作", "拒絕"),
    };
    let rejected_approval = match &check {
        Some(ApprovalCheck::Mismatch { request_id, reason }) => {
            Some(RejectedApproval { request_id: request_id.clone(), reason: *reason })
        }
        _ => None,
    };
    let record = DecisionRequest {
        id: request_id.clone(),
        workspace_id: run.workspace_id.clone(),
        run_id: run.id.clone(),
        task_id: task.id.clone(),
        kind: DecisionKind::Permission,
        status: DecisionStatus::Pending,
        question,
        reason: reason_text,
        options: vec![
            DecisionOption {
                id: OPTION_ALLOW_ONCE.to_string(),
                label: allow_label.to_string(),
                tradeoff: Some(
                    "只允許這個 Task 的這一個操作（同樣的目標、參數與成果版本），用過一次就失效；不會修改 Workspace 授權"
                        .to_string(),
                ),
            },
            DecisionOption {
                id: OPTION_DENY.to_string(),
                label: deny_label.to_string(),
                tradeoff: Some("Worker 會被告知這個操作不被允許，需要改用其他做法".to_string()),
            },
        ],
        // 高風險、無法歸類或結果未知的操作建議拒絕；其餘不給建議，交給使用者判斷
        recommendation: (capability.is_none_or(Capability::is_high_risk) || prior_use.is_some())
            .then(|| OPTION_DENY.to_string()),
        evidence,
        urgency: Urgency::Normal,
        deadline_at: None,
        permission: Some(PermissionDetail {
            tool_name: tool_name.to_string(),
            input,
            capability,
            summary,
            fingerprint,
            binding: Some(current),
            prior_use,
            rejected_approval,
        }),
        blocking: vec![task.id.clone()],
        created_at: now,
    };
    let message = match &check {
        Some(ApprovalCheck::Mismatch { reason, .. }) => {
            format!("先前的單次批准不能沿用（{}），已建立新的裁定請求。{WAITING_MESSAGE}", reason.label())
        }
        Some(ApprovalCheck::UsedResultUnknown { .. }) => format!(
            "這個操作先前已經用掉一次批准，Runtime 無法確認上一次的結果；不要直接重送，先查證上一次是否已經生效。\
             已建立新的裁定請求。{WAITING_MESSAGE}"
        ),
        None => WAITING_MESSAGE.to_string(),
    };
    open_request(record, task, now, DecisionOutcome::Permission(PermissionOutcome::Waiting { request_id, message, check }))
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        format!("{}…", text.chars().take(max).collect::<String>())
    }
}

fn answer(
    ctx: &DecisionContext<'_>,
    request_id: &str,
    option_id: &str,
    note: Option<String>,
    decided_by: &str,
    now: Timestamp,
) -> Result<DecisionChange, DecisionError> {
    if decided_by != DECIDED_BY_DESKTOP {
        return Err(DecisionError::NotDesktop);
    }
    let request = ctx.requests.iter().find(|r| r.id == request_id).ok_or_else(|| DecisionError::NotFound(request_id.to_string()))?;
    if request.option(option_id).is_none() {
        return Err(DecisionError::OptionUnknown(option_id.to_string()));
    }
    if let Some(existing) = ctx.decision_of(&request.id) {
        // 同樣的答案重送是冪等的；不同的答案不能改寫既有的裁定
        return if existing.option_id == option_id {
            Ok(DecisionChange::new(DecisionOutcome::Answered { resumed_task_ids: Vec::new() }))
        } else {
            Err(DecisionError::AlreadyAnswered(existing.option_id.clone()))
        };
    }
    let note = match note.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) if n.chars().count() > NOTE_MAX_CHARS => {
            return Err(DecisionError::Invalid(format!("備註不可超過 {NOTE_MAX_CHARS} 個字元")))
        }
        Some(n) => Some(n.to_string()),
        None => None,
    };

    let mut change = DecisionChange::new(DecisionOutcome::Answered { resumed_task_ids: Vec::new() });
    change.answered_request = Some(request.id.clone());
    change.decision = Some(Decision {
        request_id: request.id.clone(),
        option_id: option_id.to_string(),
        note: note.clone(),
        decided_by: decided_by.to_string(),
        decided_at: now,
    });

    // 受影響的 Task 恢復排程：必須還在等裁定，而且沒有別的未回答請求也阻擋它
    let mut resumed = Vec::new();
    for task_id in &request.blocking {
        let still_blocked = ctx
            .requests
            .iter()
            .any(|other| other.id != request.id && other.is_pending() && other.blocking.contains(task_id));
        let Some(task) = ctx.task.task(task_id).filter(|t| t.status == TaskStatus::WaitingDecision && !still_blocked) else {
            continue;
        };
        let mut moved = task.clone();
        let mut versioned = Versioned { task: &mut moved, events: Vec::new(), now };
        versioned
            .change_status(TaskStatus::Ready, EVENT_DECISION_ANSWERED)
            .map_err(|_| DecisionError::TaskNotFound(task.id.clone()))?;
        change.events.extend(versioned.events);
        change.tasks.push(moved);
        resumed.push(task.id.clone());
    }
    change.events.push(event(
        DECISION_AGGREGATE,
        &request.id,
        2,
        EVENT_DECISION_ANSWERED,
        now,
        serde_json::json!({
            "option_id": option_id,
            "decided_by": decided_by,
            "note": note,
            "resumed_task_ids": resumed,
        }),
    ));
    change.outbox = Some(WakeItem {
        kind: wake_kind(&request.id),
        run_id: request.run_id.clone(),
        payload: serde_json::json!({
            "workspace_id": request.workspace_id,
            "request_id": request.id,
            "option_id": option_id,
            "task_ids": request.blocking,
            "resumed_task_ids": resumed,
        }),
        created_at: now,
    });
    change.outcome = DecisionOutcome::Answered { resumed_task_ids: resumed };
    Ok(change)
}

// ---------------------------------------------------------------- 投影（API／面板）

/// 阻擋範圍中的一個 Task。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockedTask {
    pub task_id: String,
    pub task_title: String,
    pub task_status: Option<TaskStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionResult {
    pub option_id: String,
    pub option_label: String,
    pub note: Option<String>,
    pub decided_by: String,
    pub decided_at: Timestamp,
}

/// 裁定請求的完整投影：問題、原因、選項、建議、證據、阻擋範圍與（已回答時的）裁定。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionView {
    pub id: String,
    pub workspace_id: String,
    pub run_id: String,
    pub task_id: String,
    pub task_title: String,
    pub kind: DecisionKind,
    pub status: DecisionStatus,
    pub question: String,
    pub reason: String,
    pub options: Vec<DecisionOption>,
    pub recommendation: Option<String>,
    pub evidence: Vec<String>,
    pub urgency: Urgency,
    pub deadline_at: Option<Timestamp>,
    /// 已逾期仍未回答；只是標示，不代表同意，也不會改變任何狀態。
    pub overdue: bool,
    pub blocking: Vec<BlockedTask>,
    pub permission: Option<PermissionView>,
    pub created_at: Timestamp,
    pub decision: Option<DecisionResult>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionView {
    pub tool_name: String,
    pub input: Value,
    pub capability: Option<Capability>,
    pub capability_label: Option<String>,
    pub summary: String,
    /// 「允許本次操作」綁定的邏輯操作；票 23 之前的請求沒有。
    pub binding: Option<ApprovalBinding>,
    /// 同一個操作先前已用掉一次批准、結果未知（要先查證）。
    pub prior_use: Option<PriorUse>,
    /// 先前不能沿用的單次批准與原因。
    pub rejected_approval: Option<RejectedApproval>,
    /// 這個請求的裁定已被某個 Run 使用（單次批准用過即失效）。
    pub approval_use: Option<ApprovalUse>,
}

pub fn view(
    request: &DecisionRequest,
    decision: Option<&Decision>,
    approval_use: Option<&ApprovalUse>,
    tasks: &[Task],
    now: Timestamp,
) -> DecisionView {
    let title_of = |id: &str| tasks.iter().find(|t| t.id == id).map_or_else(|| id.to_string(), |t| t.title.clone());
    DecisionView {
        id: request.id.clone(),
        workspace_id: request.workspace_id.clone(),
        run_id: request.run_id.clone(),
        task_id: request.task_id.clone(),
        task_title: title_of(&request.task_id),
        kind: request.kind,
        status: request.status,
        question: request.question.clone(),
        reason: request.reason.clone(),
        options: request.options.clone(),
        recommendation: request.recommendation.clone(),
        evidence: request.evidence.clone(),
        urgency: request.urgency,
        deadline_at: request.deadline_at,
        overdue: request.is_overdue(now),
        blocking: request
            .blocking
            .iter()
            .map(|id| BlockedTask {
                task_id: id.clone(),
                task_title: title_of(id),
                task_status: tasks.iter().find(|t| &t.id == id).map(|t| t.status),
            })
            .collect(),
        permission: request.permission.as_ref().map(|p| PermissionView {
            tool_name: p.tool_name.clone(),
            input: p.input.clone(),
            capability: p.capability,
            capability_label: p.capability.map(|c| c.label().to_string()),
            summary: p.summary.clone(),
            binding: p.binding.clone(),
            prior_use: p.prior_use.clone(),
            rejected_approval: p.rejected_approval.clone(),
            approval_use: approval_use.cloned(),
        }),
        created_at: request.created_at,
        decision: decision.map(|d| DecisionResult {
            option_id: d.option_id.clone(),
            option_label: request.option(&d.option_id).map_or_else(|| d.option_id.clone(), |o| o.label.clone()),
            note: d.note.clone(),
            decided_by: d.decided_by.clone(),
            decided_at: d.decided_at,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn option(id: &str) -> DecisionOption {
        DecisionOption { id: id.into(), label: format!("選項 {id}"), tradeoff: None }
    }

    fn content(options: Vec<DecisionOption>, recommendation: Option<&str>) -> Result<RequestContent, DecisionError> {
        RequestContent::new("要用哪個方案？", "兩個方案的取捨取決於使用者價值", options, recommendation, vec!["證據".into()], None, None)
    }

    #[test]
    fn request_content_is_validated_and_trimmed() {
        let ok = content(vec![option("a"), option("b")], Some(" a ")).unwrap();
        assert_eq!(ok.recommendation.as_deref(), Some("a"));
        assert_eq!(ok.urgency, Urgency::Normal);

        let bad = |r: Result<RequestContent, DecisionError>| assert_eq!(r.unwrap_err().code(), "decision_invalid");
        bad(content(vec![option("a")], None));
        bad(content(vec![option("a"), option("a")], None));
        bad(content(vec![option("a"), option("b c")], None));
        bad(content(vec![option("a"), option("b")], Some("zzz")));
        bad(RequestContent::new(" ", "原因", vec![option("a"), option("b")], None, vec![], None, None));
        bad(RequestContent::new("問題", "", vec![option("a"), option("b")], None, vec![], None, None));
        bad(RequestContent::new("問題", "原因", vec![option("a"), option("b")], None, vec![], None, Some(0)));
        bad(RequestContent::new("問題", "原因", vec![option("a"), option("b")], None, vec!["x".into(); 11], None, None));
        bad(RequestContent::new(&"長".repeat(501), "原因", vec![option("a"), option("b")], None, vec![], None, None));
        bad(content((0..9).map(|i| option(&format!("o{i}"))).collect(), None));
    }

    #[test]
    fn overdue_is_only_a_label_for_pending_requests() {
        let mut request = DecisionRequest {
            id: "d1".into(),
            workspace_id: "ws".into(),
            run_id: "r".into(),
            task_id: "t".into(),
            kind: DecisionKind::Agent,
            status: DecisionStatus::Pending,
            question: "q".into(),
            reason: "r".into(),
            options: vec![option("a"), option("b")],
            recommendation: None,
            evidence: vec![],
            urgency: Urgency::Normal,
            deadline_at: Some(Timestamp::from_millis(1_000)),
            permission: None,
            blocking: vec!["t".into()],
            created_at: Timestamp::from_millis(0),
        };
        assert!(!request.is_overdue(Timestamp::from_millis(1_000)));
        assert!(request.is_overdue(Timestamp::from_millis(1_001)));
        request.status = DecisionStatus::Answered;
        assert!(!request.is_overdue(Timestamp::from_millis(9_999)));
    }
}
