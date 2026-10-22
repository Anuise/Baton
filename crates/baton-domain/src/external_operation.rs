//! 外部操作（external_operations）：Worker 執行部署、發訊、付費這類 Runtime 無法撤回的操作前後的登記（票 26）。
//!
//! 純邏輯、無 I/O。Worker 在送出操作**之前**以冪等鍵登記（`begin_external_operation`），拿到「可以執行」才送出；
//! 送出之後回報結果與回執（`record_external_operation_result`）。Runtime 據此保證：
//! - **同一個冪等鍵不重送**：已成功的回傳原回執；結果未知的一律擋下，不論是同一個 Run 重試、Runtime 重建後的
//!   新 Run，還是別的 Task 用了同一個鍵。只有使用者查證「沒有生效」之後才允許再執行一次。
//! - **結果未知就送裁定**：Worker 回報結果未知，或 Run 結束（含崩潰後核對、完全退出、還原）時還有尚未回報結果的
//!   操作，該操作就變成「結果未知」，建立裁定請求，相關 Task 進入 `waiting_decision`；其他 Task 照常進行。
//! - **要求雜湊**：冪等鍵相同但內容（操作、目標、參數）不同，視為鍵被誤用，拒絕，不覆寫原紀錄。
//!
//! 冪等鍵的範圍是 Workspace（與 `message_id` 一致，AI-129）。

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::decision::{self, DecisionKind, DecisionOption, DecisionRequest, DecisionStatus, Urgency};
use crate::event::DomainEvent;
use crate::run::{self, ExecutionLease, Run};
use crate::task::{Task, TaskContext, TaskStatus, Versioned};
use crate::Timestamp;

pub const EXTERNAL_OPERATION_AGGREGATE: &str = "external_operation";
pub const EVENT_EXTERNAL_OPERATION_BEGUN: &str = "external_operation_begun";
pub const EVENT_EXTERNAL_OPERATION_SUCCEEDED: &str = "external_operation_succeeded";
pub const EVENT_EXTERNAL_OPERATION_FAILED: &str = "external_operation_failed";
pub const EVENT_EXTERNAL_OPERATION_RESULT_UNKNOWN: &str = "external_operation_result_unknown";
pub const EVENT_EXTERNAL_OPERATION_VERIFIED: &str = "external_operation_verified";

/// Agent 工具名稱。
pub const TOOL_BEGIN_EXTERNAL_OPERATION: &str = "begin_external_operation";
pub const TOOL_RECORD_EXTERNAL_OPERATION_RESULT: &str = "record_external_operation_result";

/// 「結果未知」裁定請求的選項。
pub const OPTION_VERIFIED_DONE: &str = "verified_done";
pub const OPTION_VERIFIED_NOT_DONE: &str = "verified_not_done";

pub const KEY_MAX_CHARS: usize = 200;
pub const OPERATION_MAX_CHARS: usize = 100;
pub const TARGET_MAX_CHARS: usize = 500;
pub const REQUEST_MAX_BYTES: usize = 16 * 1024;
pub const RECEIPT_MAX_CHARS: usize = 2_000;
pub const REASON_MAX_CHARS: usize = 1_000;

// ---------------------------------------------------------------- 實體

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationStatus {
    /// 已登記、可能已經送出，還沒有回報結果。
    InProgress,
    Succeeded,
    /// 確定沒有生效（Worker 回報失敗，或使用者查證沒有生效）；可以用同一個鍵再執行一次。
    Failed,
    /// 結果未知：等使用者查證，期間同一個鍵一律擋下。
    Unknown,
}

impl OperationStatus {
    pub const ALL: [OperationStatus; 4] =
        [OperationStatus::InProgress, OperationStatus::Succeeded, OperationStatus::Failed, OperationStatus::Unknown];

    pub fn as_str(self) -> &'static str {
        match self {
            OperationStatus::InProgress => "in_progress",
            OperationStatus::Succeeded => "succeeded",
            OperationStatus::Failed => "failed",
            OperationStatus::Unknown => "unknown",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.as_str() == raw)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalOperation {
    pub id: String,
    pub workspace_id: String,
    /// 最近一次嘗試的 Task 與 Run。
    pub task_id: String,
    pub run_id: String,
    pub idempotency_key: String,
    /// 操作種類（例如 `deploy`、`send_email`）與目標資源（例如 `production`）。
    pub operation: String,
    pub target: String,
    /// 影響結果的參數。
    pub request: Value,
    /// 操作、目標與參數的 SHA-256（小寫十六進位）。
    pub request_hash: String,
    pub status: OperationStatus,
    /// 第幾次嘗試（只有確定沒有生效之後才會再嘗試）。
    pub attempt: i64,
    /// 外部系統的回執（成功時）。
    pub receipt: Option<String>,
    pub failure_reason: Option<String>,
    /// 為什麼結果未知。
    pub unknown_reason: Option<String>,
    /// 結果未知時建立的裁定請求。
    pub decision_request_id: Option<String>,
    pub version: i64,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

/// Worker 登記的操作內容（已驗證）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationRequest {
    pub idempotency_key: String,
    pub operation: String,
    pub target: String,
    pub request: Value,
}

impl OperationRequest {
    /// 驗證並整理（去除前後空白）；任何一項無效都拒絕，不寫入任何東西。
    pub fn new(idempotency_key: &str, operation: &str, target: &str, request: Value) -> Result<Self, ExternalOperationError> {
        let idempotency_key = text("冪等鍵 idempotency_key", idempotency_key, KEY_MAX_CHARS)?;
        let operation = text("操作種類 operation", operation, OPERATION_MAX_CHARS)?;
        let target = text("目標 target", target, TARGET_MAX_CHARS)?;
        let request = if request.is_null() { Value::Object(Default::default()) } else { request };
        if request.to_string().len() > REQUEST_MAX_BYTES {
            return Err(invalid(format!("參數 request 不可超過 {REQUEST_MAX_BYTES} 位元組")));
        }
        Ok(Self { idempotency_key, operation, target, request })
    }

    /// 操作、目標與參數的 SHA-256；冪等鍵不列入（鍵相同、雜湊不同就是鍵被誤用）。
    pub fn request_hash(&self) -> String {
        // serde_json 的物件以鍵排序（未啟用 preserve_order），同樣的內容永遠序列化成同樣的文字
        let text = format!("{}\n{}\n{}", self.operation, self.target, self.request);
        Sha256::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
    }
}

/// Worker 回報的結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperationResult {
    Succeeded { receipt: String },
    /// 確定沒有生效（例如外部系統明確拒絕）。不確定時必須回報 `Unknown`。
    Failed { reason: String },
    Unknown { reason: String },
}

impl OperationResult {
    pub fn succeeded(receipt: &str) -> Result<Self, ExternalOperationError> {
        Ok(Self::Succeeded { receipt: text("回執 receipt", receipt, RECEIPT_MAX_CHARS)? })
    }

    pub fn failed(reason: &str) -> Result<Self, ExternalOperationError> {
        Ok(Self::Failed { reason: text("原因 reason", reason, REASON_MAX_CHARS)? })
    }

    pub fn unknown(reason: &str) -> Result<Self, ExternalOperationError> {
        Ok(Self::Unknown { reason: text("原因 reason", reason, REASON_MAX_CHARS)? })
    }

    fn status(&self) -> OperationStatus {
        match self {
            OperationResult::Succeeded { .. } => OperationStatus::Succeeded,
            OperationResult::Failed { .. } => OperationStatus::Failed,
            OperationResult::Unknown { .. } => OperationStatus::Unknown,
        }
    }
}

fn invalid(message: String) -> ExternalOperationError {
    ExternalOperationError::Invalid(message)
}

fn text(name: &str, raw: &str, max: usize) -> Result<String, ExternalOperationError> {
    let value = raw.trim();
    if value.is_empty() {
        return Err(invalid(format!("{name}不可空白")));
    }
    if value.chars().count() > max {
        return Err(invalid(format!("{name}不可超過 {max} 個字元")));
    }
    Ok(value.to_string())
}

// ---------------------------------------------------------------- 命令、結果與錯誤

#[derive(Debug, Clone, PartialEq)]
pub enum ExternalCommand {
    /// Worker 送出操作之前登記；`operation_id` 由呼叫端產生，用不到時丟棄。
    Begin { operation_id: String, run_id: String, generation: i64, request: OperationRequest },
    /// Worker 回報結果。
    RecordResult { run_id: String, generation: i64, idempotency_key: String, result: OperationResult },
    /// Run 已經結束（同一個交易中）：它還沒有回報結果的操作一律變成結果未知。
    RunEnded { run_id: String },
    /// 使用者回答了「結果未知」的裁定請求（同一個交易中）：依查證結果結案。
    Verified { request_id: String, option_id: String, note: Option<String> },
}

/// 登記的結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BeginOutcome {
    /// 可以執行（`existing`：同一個 Run 重送登記，沿用原本的紀錄）。
    Proceed { operation_id: String, attempt: i64, existing: bool },
    /// 已經成功過：不要重送，沿用原回執。
    AlreadySucceeded { operation_id: String, receipt: Option<String> },
    /// 結果未知：不要重送，先等查證。
    ResultUnknown { operation_id: String, decision_request_id: Option<String> },
    /// 另一個仍在執行的 Run 正在進行同一個操作。
    InProgressElsewhere { operation_id: String, run_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExternalOutcome {
    Begin(BeginOutcome),
    Recorded { operation_id: String, status: OperationStatus, decision_request_id: Option<String> },
    /// Run 結束後變成結果未知的操作。
    Settled { unknown: Vec<String> },
    /// 查證結案的操作（不是外部操作的裁定時為 `None`）。
    Verified { operation_id: Option<String> },
}

/// 命令的結果：要寫入的操作、裁定請求（與它的事件）、Task 與事件。沒有任何變動時各欄位都是空的。
#[derive(Debug, Clone, PartialEq)]
pub struct ExternalChange {
    pub operations: Vec<ExternalOperation>,
    pub requests: Vec<DecisionRequest>,
    /// 加入既有裁定請求阻擋範圍的 Task：`(request_id, task_id)`。
    pub blocks: Vec<(String, String)>,
    pub tasks: Vec<Task>,
    pub events: Vec<DomainEvent>,
    pub outcome: ExternalOutcome,
}

impl ExternalChange {
    fn new(outcome: ExternalOutcome) -> Self {
        Self { operations: Vec::new(), requests: Vec::new(), blocks: Vec::new(), tasks: Vec::new(), events: Vec::new(), outcome }
    }

    pub fn changed(&self) -> bool {
        !self.operations.is_empty()
            || !self.requests.is_empty()
            || !self.blocks.is_empty()
            || !self.tasks.is_empty()
            || !self.events.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ExternalOperationError {
    #[error("{0}")]
    Invalid(String),
    #[error("找不到 Workspace {0}")]
    WorkspaceNotFound(String),
    #[error("找不到 Run {0}")]
    RunNotFound(String),
    #[error("找不到 Task {0}")]
    TaskNotFound(String),
    #[error("這個 Run 已經結束或已被接替，不能再登記或回報外部操作")]
    StaleRun,
    #[error("找不到冪等鍵為「{0}」的外部操作；請先以 begin_external_operation 登記")]
    NotFound(String),
    #[error("冪等鍵「{0}」已經用在內容不同的外部操作上（操作、目標或參數不同）；同一個操作才能沿用同一個鍵")]
    KeyConflict(String),
    #[error("這個外部操作目前是 {} 狀態，不屬於這個 Run 進行中的操作，不能回報結果", .0.as_str())]
    NotInProgress(OperationStatus),
    #[error("這個外部操作已經回報為 {}，不能改成別的結果", .0.as_str())]
    AlreadyRecorded(OperationStatus),
}

impl ExternalOperationError {
    /// 穩定的機器可讀代碼，API 依它判斷錯誤種類。
    pub fn code(&self) -> &'static str {
        match self {
            ExternalOperationError::Invalid(_) => "external_operation_invalid",
            ExternalOperationError::WorkspaceNotFound(_) => "workspace_not_found",
            ExternalOperationError::RunNotFound(_) => "run_not_found",
            ExternalOperationError::TaskNotFound(_) => "task_not_found",
            ExternalOperationError::StaleRun => "run_report_stale",
            ExternalOperationError::NotFound(_) => "external_operation_not_found",
            ExternalOperationError::KeyConflict(_) => "idempotency_key_conflict",
            ExternalOperationError::NotInProgress(_) => "external_operation_not_in_progress",
            ExternalOperationError::AlreadyRecorded(_) => "external_operation_already_recorded",
        }
    }
}

// ---------------------------------------------------------------- 判定

/// 目前狀態的唯讀快照（同一個 Workspace 內）。Store 在交易中載入。
pub struct ExternalContext<'a> {
    pub task: &'a TaskContext<'a>,
    pub runs: &'a [Run],
    pub leases: &'a [ExecutionLease],
    pub operations: &'a [ExternalOperation],
    /// 仍在等待回答的裁定請求識別（加入阻擋範圍前確認請求還在等待）。
    pub pending_requests: &'a [String],
}

impl ExternalContext<'_> {
    fn current_run(&self, run_id: &str, generation: i64) -> Result<&Run, ExternalOperationError> {
        let run = self.runs.iter().find(|r| r.id == run_id).ok_or_else(|| ExternalOperationError::RunNotFound(run_id.to_string()))?;
        let lease = self.leases.iter().find(|l| l.run_id == run.id);
        if !run::is_current_writer(run, lease, generation) {
            return Err(ExternalOperationError::StaleRun);
        }
        Ok(run)
    }

    fn by_key(&self, key: &str) -> Option<&ExternalOperation> {
        self.operations.iter().find(|o| o.idempotency_key == key)
    }

    fn run_ended(&self, run_id: &str) -> bool {
        self.runs.iter().find(|r| r.id == run_id).is_none_or(|r| r.status.is_terminal())
    }
}

pub fn decide(ctx: &ExternalContext<'_>, command: ExternalCommand, now: Timestamp) -> Result<ExternalChange, ExternalOperationError> {
    if ctx.task.workspace.is_none() {
        return Err(ExternalOperationError::WorkspaceNotFound(ctx.task.workspace_id.to_string()));
    }
    match command {
        ExternalCommand::Begin { operation_id, run_id, generation, request } => {
            begin(ctx, operation_id, &run_id, generation, request, now)
        }
        ExternalCommand::RecordResult { run_id, generation, idempotency_key, result } => {
            record(ctx, &run_id, generation, &idempotency_key, result, now)
        }
        ExternalCommand::RunEnded { run_id } => Ok(run_ended(ctx, &run_id, now)),
        ExternalCommand::Verified { request_id, option_id, note } => Ok(verified(ctx, &request_id, &option_id, note, now)),
    }
}

/// 一個操作在判定過程中的可變副本；每個事件讓版本加一。
struct Editing {
    operation: ExternalOperation,
    events: Vec<DomainEvent>,
    now: Timestamp,
}

impl Editing {
    fn new(operation: &ExternalOperation, now: Timestamp) -> Self {
        Self { operation: operation.clone(), events: Vec::new(), now }
    }

    fn push(&mut self, event_type: &'static str, payload: Value) {
        self.operation.version += 1;
        self.operation.updated_at = self.now;
        self.events.push(DomainEvent {
            aggregate_type: EXTERNAL_OPERATION_AGGREGATE,
            aggregate_id: self.operation.id.clone(),
            aggregate_version: self.operation.version,
            event_type,
            occurred_at: self.now,
            payload,
        });
    }
}

fn begin(
    ctx: &ExternalContext<'_>,
    operation_id: String,
    run_id: &str,
    generation: i64,
    request: OperationRequest,
    now: Timestamp,
) -> Result<ExternalChange, ExternalOperationError> {
    let run = ctx.current_run(run_id, generation)?;
    let request_hash = request.request_hash();
    let Some(existing) = ctx.by_key(&request.idempotency_key) else {
        let operation = ExternalOperation {
            id: operation_id.clone(),
            workspace_id: run.workspace_id.clone(),
            task_id: run.task_id.clone(),
            run_id: run.id.clone(),
            idempotency_key: request.idempotency_key,
            operation: request.operation,
            target: request.target,
            request: request.request,
            request_hash,
            status: OperationStatus::InProgress,
            attempt: 0,
            receipt: None,
            failure_reason: None,
            unknown_reason: None,
            decision_request_id: None,
            version: 0,
            created_at: now,
            updated_at: now,
        };
        return Ok(start_attempt(&operation, run, now, false));
    };
    if existing.request_hash != request_hash {
        return Err(ExternalOperationError::KeyConflict(existing.idempotency_key.clone()));
    }
    let id = existing.id.clone();
    let outcome = match existing.status {
        OperationStatus::Succeeded => {
            BeginOutcome::AlreadySucceeded { operation_id: id, receipt: existing.receipt.clone() }
        }
        // 確定沒有生效：可以再執行一次（新的嘗試）
        OperationStatus::Failed => return Ok(start_attempt(existing, run, now, true)),
        // 同一個 Run 重送登記（例如上一次的回應遺失）：Worker 還在等回應，沿用原紀錄
        OperationStatus::InProgress if existing.run_id == run.id => {
            BeginOutcome::Proceed { operation_id: id, attempt: existing.attempt, existing: true }
        }
        // 正常情況下不會發生（Run 結束的同一個交易就會轉成結果未知）；保守地當成結果未知
        OperationStatus::InProgress if ctx.run_ended(&existing.run_id) => {
            BeginOutcome::ResultUnknown { operation_id: id, decision_request_id: existing.decision_request_id.clone() }
        }
        OperationStatus::InProgress => {
            BeginOutcome::InProgressElsewhere { operation_id: id, run_id: existing.run_id.clone() }
        }
        OperationStatus::Unknown => {
            // 擋下這個 Run 的 Task：加入同一個裁定請求的阻擋範圍，查證之後一起恢復
            let mut change = ExternalChange::new(ExternalOutcome::Begin(BeginOutcome::ResultUnknown {
                operation_id: id,
                decision_request_id: existing.decision_request_id.clone(),
            }));
            if let Some(request_id) = existing.decision_request_id.as_ref().filter(|r| ctx.pending_requests.contains(r)) {
                let task = ctx.task.task(&run.task_id).ok_or_else(|| ExternalOperationError::TaskNotFound(run.task_id.clone()))?;
                if task.id != existing.task_id {
                    change.blocks.push((request_id.clone(), task.id.clone()));
                }
                if let Some(moved) = hold_task(task, now, &mut change.events) {
                    change.tasks.push(moved);
                }
            }
            return Ok(change);
        }
    };
    Ok(ExternalChange::new(ExternalOutcome::Begin(outcome)))
}

/// 開始一次新的嘗試（第一次登記，或確定沒有生效之後再執行）。
fn start_attempt(operation: &ExternalOperation, run: &Run, now: Timestamp, retry: bool) -> ExternalChange {
    let mut edit = Editing::new(operation, now);
    edit.operation.status = OperationStatus::InProgress;
    edit.operation.attempt += 1;
    edit.operation.task_id = run.task_id.clone();
    edit.operation.run_id = run.id.clone();
    edit.operation.receipt = None;
    edit.operation.failure_reason = None;
    edit.operation.unknown_reason = None;
    edit.operation.decision_request_id = None;
    edit.push(
        EVENT_EXTERNAL_OPERATION_BEGUN,
        serde_json::json!({
            "idempotency_key": edit.operation.idempotency_key,
            "operation": edit.operation.operation,
            "target": edit.operation.target,
            "request_hash": edit.operation.request_hash,
            "run_id": run.id,
            "task_id": run.task_id,
            "attempt": edit.operation.attempt,
            "retry": retry,
        }),
    );
    let outcome = ExternalOutcome::Begin(BeginOutcome::Proceed {
        operation_id: edit.operation.id.clone(),
        attempt: edit.operation.attempt,
        existing: false,
    });
    let mut change = ExternalChange::new(outcome);
    change.events = edit.events;
    change.operations.push(edit.operation);
    change
}

fn record(
    ctx: &ExternalContext<'_>,
    run_id: &str,
    generation: i64,
    key: &str,
    result: OperationResult,
    now: Timestamp,
) -> Result<ExternalChange, ExternalOperationError> {
    let run = ctx.current_run(run_id, generation)?;
    let existing = ctx.by_key(key.trim()).ok_or_else(|| ExternalOperationError::NotFound(key.trim().to_string()))?;
    if existing.run_id != run.id {
        return Err(ExternalOperationError::NotInProgress(existing.status));
    }
    if existing.status != OperationStatus::InProgress {
        // 回應遺失後重送同樣的結果是冪等的；改成別的結果不行
        let same = match &result {
            OperationResult::Succeeded { receipt } => existing.receipt.as_ref() == Some(receipt),
            _ => existing.status == result.status(),
        };
        if same && existing.status == result.status() {
            return Ok(ExternalChange::new(ExternalOutcome::Recorded {
                operation_id: existing.id.clone(),
                status: existing.status,
                decision_request_id: existing.decision_request_id.clone(),
            }));
        }
        return Err(ExternalOperationError::AlreadyRecorded(existing.status));
    }
    let mut edit = Editing::new(existing, now);
    let mut change = ExternalChange::new(ExternalOutcome::Settled { unknown: Vec::new() });
    match result {
        OperationResult::Succeeded { receipt } => {
            edit.operation.status = OperationStatus::Succeeded;
            edit.push(EVENT_EXTERNAL_OPERATION_SUCCEEDED, serde_json::json!({ "run_id": run.id, "receipt": receipt }));
            edit.operation.receipt = Some(receipt);
        }
        OperationResult::Failed { reason } => {
            edit.operation.status = OperationStatus::Failed;
            edit.push(EVENT_EXTERNAL_OPERATION_FAILED, serde_json::json!({ "run_id": run.id, "reason": reason }));
            edit.operation.failure_reason = Some(reason);
        }
        OperationResult::Unknown { reason } => {
            mark_unknown(ctx, &mut edit, reason, &mut change)?;
        }
    }
    change.outcome = ExternalOutcome::Recorded {
        operation_id: edit.operation.id.clone(),
        status: edit.operation.status,
        decision_request_id: edit.operation.decision_request_id.clone(),
    };
    change.events.splice(0..0, edit.events);
    change.operations.push(edit.operation);
    Ok(change)
}

fn run_ended(ctx: &ExternalContext<'_>, run_id: &str, now: Timestamp) -> ExternalChange {
    let mut change = ExternalChange::new(ExternalOutcome::Settled { unknown: Vec::new() });
    let mut unknown = Vec::new();
    let end_reason = ctx.runs.iter().find(|r| r.id == run_id).and_then(|r| r.end_reason.clone());
    for operation in ctx.operations.iter().filter(|o| o.run_id == run_id && o.status == OperationStatus::InProgress) {
        let mut edit = Editing::new(operation, now);
        let reason = match &end_reason {
            Some(end) => format!("Run 已結束（{end}），結束前沒有回報這個外部操作的結果"),
            None => "Run 已結束，結束前沒有回報這個外部操作的結果".to_string(),
        };
        // Task 一定存在（外鍵）；萬一找不到就只記操作狀態，不讓 Run 的結束失敗
        if mark_unknown(ctx, &mut edit, reason, &mut change).is_err() {
            continue;
        }
        unknown.push(edit.operation.id.clone());
        change.events.splice(0..0, edit.events);
        change.operations.push(edit.operation);
    }
    change.outcome = ExternalOutcome::Settled { unknown };
    change
}

/// 轉成結果未知：建立裁定請求，相關 Task 進入 `waiting_decision`。
fn mark_unknown(
    ctx: &ExternalContext<'_>,
    edit: &mut Editing,
    reason: String,
    change: &mut ExternalChange,
) -> Result<(), ExternalOperationError> {
    let now = edit.now;
    let task_id = edit.operation.task_id.clone();
    let task = ctx.task.task(&task_id).ok_or_else(|| ExternalOperationError::TaskNotFound(task_id.clone()))?;
    let request = unknown_request(&edit.operation, &reason, now);
    edit.operation.status = OperationStatus::Unknown;
    edit.operation.unknown_reason = Some(reason.clone());
    edit.operation.decision_request_id = Some(request.id.clone());
    edit.push(
        EVENT_EXTERNAL_OPERATION_RESULT_UNKNOWN,
        serde_json::json!({ "run_id": edit.operation.run_id, "reason": reason, "decision_request_id": request.id }),
    );
    change.events.push(decision::requested_event(&request, now));
    // 同一個交易中可能已經有這個 Task 的新狀態（同一個 Run 的多個操作）
    let current = change.tasks.iter().find(|t| t.id == task.id).cloned().unwrap_or_else(|| task.clone());
    if let Some(moved) = hold_task(&current, now, &mut change.events) {
        change.tasks.retain(|t| t.id != moved.id);
        change.tasks.push(moved);
    }
    change.requests.push(request);
    Ok(())
}

/// 讓 Task 停下等裁定：還在進行（running）、剛被退回（ready）或等待驗證（verifying）的進入 `waiting_decision`；
/// 已經在等裁定、受阻或已結束的維持原狀（它們本來就不會被派工）。
fn hold_task(task: &Task, now: Timestamp, events: &mut Vec<DomainEvent>) -> Option<Task> {
    if !matches!(task.status, TaskStatus::Running | TaskStatus::Ready | TaskStatus::Verifying) {
        return None;
    }
    let mut moved = task.clone();
    let mut versioned = Versioned { task: &mut moved, events: Vec::new(), now };
    versioned.change_status(TaskStatus::WaitingDecision, EVENT_EXTERNAL_OPERATION_RESULT_UNKNOWN).ok()?;
    events.extend(versioned.events);
    Some(moved)
}

/// 「結果未知」裁定請求的識別：每個操作的每次嘗試最多一個，所以由兩者決定，重做同一件事不會建立第二個。
pub fn unknown_request_id(operation: &ExternalOperation) -> String {
    format!("decision_{}_a{}", operation.id, operation.attempt)
}

fn unknown_request(operation: &ExternalOperation, reason: &str, now: Timestamp) -> DecisionRequest {
    DecisionRequest {
        id: unknown_request_id(operation),
        workspace_id: operation.workspace_id.clone(),
        run_id: operation.run_id.clone(),
        task_id: operation.task_id.clone(),
        // 資料庫的種類只有 agent／permission；這個請求由 Runtime 代 Worker 提出，與外部操作的關聯記在操作上
        kind: DecisionKind::Agent,
        status: DecisionStatus::Pending,
        question: format!(
            "外部操作「{}：{}」的結果未知，請查證它是否已經生效。",
            operation.operation, operation.target
        ),
        reason: format!(
            "{reason}。Runtime 無法確認這個外部操作是否已經生效，所以不會自動重送；\
             在你查證之前，同一個冪等鍵的操作一律擋下，只有受影響的 Task 暫停，其他 Task 照常進行。"
        ),
        options: vec![
            DecisionOption {
                id: OPTION_VERIFIED_DONE.to_string(),
                label: "已查證：操作已經生效（不要重送）".to_string(),
                tradeoff: Some("之後同一個冪等鍵的操作直接視為已完成，Worker 不會再送一次".to_string()),
            },
            DecisionOption {
                id: OPTION_VERIFIED_NOT_DONE.to_string(),
                label: "已查證：操作沒有生效（允許之後再執行一次）".to_string(),
                tradeoff: Some("Task 恢復後，Worker 可以用同一個冪等鍵再送一次；若其實已經生效就會重複".to_string()),
            },
        ],
        recommendation: None,
        evidence: vec![
            format!("冪等鍵：{}", operation.idempotency_key),
            format!("操作：{}；目標：{}", operation.operation, operation.target),
            format!("要求雜湊：{}", operation.request_hash),
            format!("Run：{}（第 {} 次嘗試）", operation.run_id, operation.attempt),
        ],
        urgency: Urgency::High,
        deadline_at: None,
        permission: None,
        blocking: vec![operation.task_id.clone()],
        created_at: now,
    }
}

fn verified(ctx: &ExternalContext<'_>, request_id: &str, option_id: &str, note: Option<String>, now: Timestamp) -> ExternalChange {
    let mut change = ExternalChange::new(ExternalOutcome::Verified { operation_id: None });
    let Some(operation) = ctx
        .operations
        .iter()
        .find(|o| o.status == OperationStatus::Unknown && o.decision_request_id.as_deref() == Some(request_id))
    else {
        return change;
    };
    let mut edit = Editing::new(operation, now);
    match option_id {
        OPTION_VERIFIED_DONE => {
            let receipt = match &note {
                Some(note) => format!("使用者查證已生效：{note}"),
                None => "使用者查證已生效".to_string(),
            };
            edit.operation.status = OperationStatus::Succeeded;
            edit.operation.receipt = Some(receipt);
        }
        OPTION_VERIFIED_NOT_DONE => {
            edit.operation.status = OperationStatus::Failed;
            edit.operation.failure_reason = Some(match &note {
                Some(note) => format!("使用者查證沒有生效：{note}"),
                None => "使用者查證沒有生效".to_string(),
            });
        }
        _ => return change,
    }
    edit.push(
        EVENT_EXTERNAL_OPERATION_VERIFIED,
        serde_json::json!({ "decision_request_id": request_id, "option_id": option_id, "status": edit.operation.status }),
    );
    change.outcome = ExternalOutcome::Verified { operation_id: Some(edit.operation.id.clone()) };
    change.events = edit.events;
    change.operations.push(edit.operation);
    change
}

#[cfg(test)]
mod tests;
