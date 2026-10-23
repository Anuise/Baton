//! Controller 專屬的拆工與派工工具（票 17，spec D-05、D-15、ADR-0010）：只接受 Controller 憑證。
//!
//! Controller 不直接寫資料庫、也不能自己啟動 Worker：它只能透過這些工具「提出」變更，由 Runtime 驗證後才接受。
//! 每個工具都走與桌面端點相同的 Domain 規則（`task::decide`、`task::admission_report`、Scheduler），
//! 所以 Controller 建立的 Task 與使用者手動建立的 Task 受相同的准入檢查；另外依實際變更推導決策權限表的事項：
//! - 派工時 Task 需要的能力超出 Workspace 授權 → `exceed_authorization`，被拒絕並建立裁定請求；
//! - 已派工過的 Task 修改完成條件或驗證檢查 → `modify_done_criteria`，必須先經使用者核准（核准用過一次就失效）。
//!
//! 漸進拆工（ADR-0010「邊釐清、邊調查」）：`propose_decision_request` 向使用者提問並列出依賴答案的 Task，
//! 它們在使用者回答前不能進入 ready 或派工；不依賴答案的 Task 照常派出。

use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use baton_domain::auth::{ControllerScope, CredentialRecord};
use baton_domain::authority::Matter;
use baton_domain::decision::{ActionContent, ControllerOutcome, DecisionOption, DecisionOutcome, RequestContent, Urgency};
use baton_domain::run::RunView;
use baton_domain::scheduler::WaitReason;
use baton_domain::task::{ContractContent, ContractField, MissingCode, MissingItem, TaskCommand, TaskStatus, TaskView};
use baton_domain::workdir::TaskKind;
use baton_store::DecisionApplyError;
use serde::{Deserialize, Serialize};

use crate::agent::Claims;
use crate::runs::{dispatch_now, DispatchResponse};
use crate::tasks::{AdmissionReport, TaskApiError};
use crate::workspaces::{new_id, ApiError};
use crate::{ApiState, ErrorBody};

fn api_error(status: StatusCode, code: &'static str, message: &str) -> ApiError {
    ApiError { status, code, message: message.to_string() }
}

/// 不是 Controller 憑證就拒絕；在解析請求之前檢查，Worker 不論送什麼內容都得到同一個答案。
pub(crate) fn require_controller(record: &CredentialRecord) -> Result<ControllerScope, ApiError> {
    record.controller.clone().ok_or_else(|| {
        api_error(
            StatusCode::FORBIDDEN,
            "controller_only",
            "這個工具只有 Controller 能使用；Worker 需要調整工作時請以 report_blocked 或 request_decision 回報",
        )
    })
}

/// Controller 憑證的範圍；Worker 憑證、聲稱別的 Workspace 或已經結束（或已重新綁定）的 Controller CLI 都拒絕。
pub(crate) async fn controller_scope(state: &ApiState, record: &CredentialRecord, claims: &Claims) -> Result<ControllerScope, ApiError> {
    let scope = require_controller(record)?;
    let foreign = |claimed: &Option<String>, actual: &str| claimed.as_deref().is_some_and(|c| c != actual);
    if foreign(&claims.workspace_id, &scope.workspace_id) || foreign(&claims.run_id, &scope.controller_run_id) {
        return Err(api_error(
            StatusCode::FORBIDDEN,
            "agent_scope_mismatch",
            "請求指定的 Workspace 或 Run 不是這個憑證所屬的範圍，已拒絕",
        ));
    }
    if !state.store.controller_run_active(&scope).await? {
        return Err(api_error(StatusCode::UNAUTHORIZED, "credential_revoked", "Controller CLI 已結束或已重新綁定，憑證已失效"));
    }
    Ok(scope)
}

/// 本組工具的錯誤：一般錯誤、准入缺項，或「必須送裁定」（附裁定請求）。
pub(crate) enum ControllerToolError {
    Task(TaskApiError),
    RequiresDecision { matter: Matter, decision_request_id: String, existing: bool, message: String },
}

#[derive(Serialize)]
struct RequiresDecisionBody {
    #[serde(flatten)]
    error: ErrorBody,
    status: &'static str,
    matter: Matter,
    decision_request_id: String,
    existing: bool,
}

impl IntoResponse for ControllerToolError {
    fn into_response(self) -> Response {
        match self {
            ControllerToolError::Task(error) => error.into_response(),
            ControllerToolError::RequiresDecision { matter, decision_request_id, existing, message } => (
                StatusCode::CONFLICT,
                Json(RequiresDecisionBody {
                    error: ErrorBody { error: "requires_decision".to_string(), message },
                    status: "requires_decision",
                    matter,
                    decision_request_id,
                    existing,
                }),
            )
                .into_response(),
        }
    }
}

impl<E: Into<TaskApiError>> From<E> for ControllerToolError {
    fn from(error: E) -> Self {
        Self::Task(error.into())
    }
}

impl From<DecisionApplyError> for TaskApiError {
    fn from(error: DecisionApplyError) -> Self {
        TaskApiError::Plain(error.into())
    }
}

/// Runtime 依實際變更推導出事項，依決策權限表判定（票 17；AI-227 要求不信任 Agent 自報的事項）。
/// 可以做（自主處理，或使用者已核准這一次）回 `Ok(())`；否則回對應的錯誤，變更不會執行。
async fn gate(
    state: &ApiState,
    scope: &ControllerScope,
    matter: Matter,
    task_id: &str,
    summary: &str,
    reason: &str,
) -> Result<(), ControllerToolError> {
    let content =
        ActionContent::new(matter, Some(task_id), summary, reason, Vec::new(), None).map_err(DecisionApplyError::from)?;
    let outcome =
        state.store.controller_gate(scope, new_id("decision")?, new_id("authz")?, content, state.clock.now()).await?;
    let DecisionOutcome::Controller { outcome, .. } = outcome else { unreachable!("ControllerGate 只會得到 Controller 結果") };
    match outcome {
        ControllerOutcome::Allowed | ControllerOutcome::Approved { .. } => Ok(()),
        ControllerOutcome::RequiresDecision { request_id, existing } => Err(ControllerToolError::RequiresDecision {
            matter,
            message: format!(
                "依決策權限表，「{}」必須由使用者裁定：變更沒有執行，已建立裁定請求 {request_id}。\
                 請等待裁定結果（回答後背景執行的 wait 會回報「裁定已回答」事件），不要自行繞過；可以先做不依賴它的其他工作。",
                matter.label()
            ),
            decision_request_id: request_id,
            existing,
        }),
        ControllerOutcome::Denied { request_id, note } => Err(ControllerToolError::Task(TaskApiError::Plain(api_error(
            StatusCode::CONFLICT,
            "decision_denied",
            &format!(
                "使用者沒有核准「{}」（裁定請求 {request_id}）{}；請改用其他做法",
                matter.label(),
                note.map(|n| format!("：{n}")).unwrap_or_default()
            ),
        )))),
        ControllerOutcome::HumanOnly => Err(ControllerToolError::Task(TaskApiError::Plain(api_error(
            StatusCode::FORBIDDEN,
            "human_only",
            "這個事項只能由使用者從對應介面發起",
        )))),
    }
}

// ---------------------------------------------------------------- propose_task

#[derive(Debug, Deserialize)]
pub(crate) struct ProposeTaskRequest {
    title: String,
    /// 既有的 Goal；與 `goal_title` 二擇一。
    #[serde(default)]
    goal_id: Option<String>,
    /// 建立新的 Goal。
    #[serde(default)]
    goal_title: Option<String>,
    #[serde(default)]
    kind: TaskKind,
    #[serde(default)]
    contract: ContractContent,
    #[serde(flatten)]
    claims: Claims,
}

/// 建立或修改 Task 之後的回應：Task 目前的樣子與准入檢查結果（與工作面板的准入預覽相同）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ControllerTaskResponse {
    pub task: TaskView,
    pub goal_id: String,
    /// 這次呼叫是否改變了任何東西（重複送出相同內容時為 `false`）。
    pub changed: bool,
    pub admission: AdmissionReport,
}

async fn task_response(
    state: &ApiState,
    workspace_id: &str,
    task_id: &str,
    changed: bool,
) -> Result<ControllerTaskResponse, ControllerToolError> {
    let detail = state.store.task_detail(workspace_id, task_id).await?;
    let missing = state.store.task_admission_report(workspace_id, task_id).await?;
    Ok(ControllerTaskResponse {
        goal_id: detail.task.task.goal_id.clone(),
        task: detail.task,
        changed,
        admission: AdmissionReport { admissible: missing.is_empty(), missing },
    })
}

/// 建立 Task（一律是 pending）：規則與使用者在工作面板建立 Task 完全相同，契約可以先不完整。
pub(crate) async fn propose_task(
    State(state): State<Arc<ApiState>>,
    Extension(record): Extension<CredentialRecord>,
    body: Result<Json<ProposeTaskRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<ControllerTaskResponse>), ControllerToolError> {
    require_controller(&record)?;
    let Json(request) = body?;
    let scope = controller_scope(&state, &record, &request.claims).await?;
    let (goal_id, new_goal) = match (request.goal_id, request.goal_title) {
        (Some(goal_id), None) => (goal_id, None),
        (None, Some(title)) => {
            let id = new_id("goal")?;
            (id.clone(), Some(TaskCommand::CreateGoal { id, title }))
        }
        _ => {
            return Err(TaskApiError::Plain(api_error(
                StatusCode::BAD_REQUEST,
                "goal_choice_invalid",
                "goal_id（既有的 Goal）與 goal_title（建立新的 Goal）必須剛好提供一個",
            ))
            .into())
        }
    };
    let task_id = new_id("task")?;
    let command = TaskCommand::CreateTask {
        id: task_id.clone(),
        goal_id,
        title: request.title,
        kind: request.kind,
        contract: request.contract,
    };
    state.store.propose_task(scope.workspace_id.clone(), new_goal, command, state.clock.now()).await?;
    let response = task_response(&state, &scope.workspace_id, &task_id, true).await?;
    Ok((StatusCode::CREATED, Json(response)))
}

// ---------------------------------------------------------------- update_task

#[derive(Debug, Deserialize)]
pub(crate) struct UpdateTaskRequest {
    task_id: String,
    /// 整份取代。
    contract: ContractContent,
    #[serde(default)]
    note: Option<String>,
    #[serde(flatten)]
    claims: Claims,
}

/// 完成條件或驗證檢查有沒有實質改變：原本已填寫的完成條件被改掉，或驗證檢查不同（新增、刪除、修改都算）。
/// 補上原本缺少或未知的完成條件不算「修改」（漸進拆工時本來就會補齊）。
fn changes_done_criteria(current: &ContractContent, next: &ContractContent) -> bool {
    let filled = |field: &Option<ContractField>| match field {
        Some(ContractField::Filled { text, references }) => Some((text.trim().to_string(), references.clone())),
        _ => None,
    };
    let done_changed = filled(&current.done_criteria).is_some() && filled(&current.done_criteria) != filled(&next.done_criteria);
    done_changed || current.verification_checks != next.verification_checks
}

/// 整份取代 Task 的契約。已派工過的 Task 修改完成條件或驗證檢查時必須先經使用者核准（spec D-15）。
pub(crate) async fn update_task(
    State(state): State<Arc<ApiState>>,
    Extension(record): Extension<CredentialRecord>,
    body: Result<Json<UpdateTaskRequest>, JsonRejection>,
) -> Result<Json<ControllerTaskResponse>, ControllerToolError> {
    require_controller(&record)?;
    let Json(request) = body?;
    let scope = controller_scope(&state, &record, &request.claims).await?;
    let workspace_id = scope.workspace_id.clone();
    let current = state.store.task_detail(&workspace_id, &request.task_id).await?.task;
    let dispatched = state.store.run_list(&workspace_id).await?.iter().any(|r| r.run.task_id == request.task_id);
    if dispatched && changes_done_criteria(&current.contract.content, &request.contract) {
        let summary = format!("修改 Task「{}」（{}）的完成條件或驗證檢查", current.task.title, current.task.id);
        let reason = request.note.clone().unwrap_or_else(|| "Controller 更新契約".to_string());
        gate(&state, &scope, Matter::ModifyDoneCriteria, &request.task_id, &summary, &reason).await?;
    }
    let command =
        TaskCommand::UpdateContract { task_id: request.task_id.clone(), contract: request.contract, note: request.note };
    let change = state.store.apply_task_command(workspace_id.clone(), command, state.clock.now()).await?;
    Ok(Json(task_response(&state, &workspace_id, &request.task_id, change.changed()).await?))
}

// ---------------------------------------------------------------- add_dependency

#[derive(Debug, Deserialize)]
pub(crate) struct AddDependencyRequest {
    task_id: String,
    depends_on: String,
    #[serde(flatten)]
    claims: Claims,
}

/// 新增相依；自我相依與循環由 Domain 拒絕（與桌面端點相同）。
pub(crate) async fn add_dependency(
    State(state): State<Arc<ApiState>>,
    Extension(record): Extension<CredentialRecord>,
    body: Result<Json<AddDependencyRequest>, JsonRejection>,
) -> Result<Json<ControllerTaskResponse>, ControllerToolError> {
    require_controller(&record)?;
    let Json(request) = body?;
    let scope = controller_scope(&state, &record, &request.claims).await?;
    let command = TaskCommand::AddDependency { task_id: request.task_id.clone(), depends_on: request.depends_on };
    let change = state.store.apply_task_command(scope.workspace_id.clone(), command, state.clock.now()).await?;
    Ok(Json(task_response(&state, &scope.workspace_id, &request.task_id, change.changed()).await?))
}

// ---------------------------------------------------------------- dispatch_task

#[derive(Debug, Deserialize)]
pub(crate) struct DispatchTaskRequest {
    task_id: String,
    #[serde(default)]
    priority: i32,
    #[serde(flatten)]
    claims: Claims,
}

/// `dispatch_task` 的回應：已建立 Run（201，`status: "started"`），或留在佇列（202，`status: "queued"` 與等待原因）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ControllerDispatch {
    Started { run: Box<RunView> },
    Queued { task_id: String, priority: i32, reasons: Vec<WaitReason> },
}

/// 派工：先做完整的准入檢查（與桌面「標記 ready」相同的 `admission_report`），超出授權就送裁定；
/// pending 的 Task 通過後轉為 ready，再交給與桌面派工端點相同的流程（Scheduler 選帳號、建立 Run）。
pub(crate) async fn dispatch_task(
    State(state): State<Arc<ApiState>>,
    Extension(record): Extension<CredentialRecord>,
    body: Result<Json<DispatchTaskRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<ControllerDispatch>), ControllerToolError> {
    require_controller(&record)?;
    let Json(request) = body?;
    let scope = controller_scope(&state, &record, &request.claims).await?;
    let (workspace_id, task_id) = (scope.workspace_id.clone(), request.task_id.clone());
    let task = state.store.task_detail(&workspace_id, &task_id).await?.task;
    if matches!(task.task.status, TaskStatus::Pending | TaskStatus::Ready) {
        let missing = state.store.task_admission_report(&workspace_id, &task_id).await?;
        let beyond: Vec<&MissingItem> = missing.iter().filter(|m| m.code == MissingCode::CapabilityNotGranted).collect();
        if !beyond.is_empty() {
            let detail = beyond.iter().map(|m| m.message.as_str()).collect::<Vec<_>>().join("；");
            let summary = format!("派出 Task「{}」（{}），它需要 Workspace 授權沒有涵蓋的能力", task.task.title, task.task.id);
            gate(&state, &scope, Matter::ExceedAuthorization, &task_id, &summary, &detail).await?;
            // 使用者核准了，但核准不會擴大授權：Workspace 授權仍未涵蓋時照樣不能派出
            return Err(TaskApiError::Plain(api_error(
                StatusCode::CONFLICT,
                "authorization_not_expanded",
                &format!("使用者已核准，但 Workspace 授權仍未涵蓋這個 Task 需要的能力（{detail}）；請使用者到授權頁修改授權後再派工"),
            ))
            .into());
        }
        if !missing.is_empty() {
            return Err(TaskApiError::Denied { message: admission_message(&missing), missing }.into());
        }
        if task.task.status == TaskStatus::Pending {
            let command = TaskCommand::MarkReady { task_id: task_id.clone() };
            state.store.apply_task_command(workspace_id.clone(), command, state.clock.now()).await?;
        }
    }
    Ok(match dispatch_now(&state, workspace_id, task_id, request.priority).await? {
        DispatchResponse::Started(run) => (StatusCode::CREATED, Json(ControllerDispatch::Started { run })),
        DispatchResponse::Queued(queued) => (
            StatusCode::ACCEPTED,
            Json(ControllerDispatch::Queued { task_id: queued.task_id, priority: queued.priority, reasons: queued.reasons }),
        ),
    })
}

fn admission_message(missing: &[MissingItem]) -> String {
    let items = missing.iter().map(|m| m.message.as_str()).collect::<Vec<_>>().join("；");
    format!("准入檢查未通過，缺少：{items}")
}

// ---------------------------------------------------------------- cancel_task

#[derive(Debug, Deserialize)]
pub(crate) struct CancelTaskRequest {
    task_id: String,
    #[serde(flatten)]
    claims: Claims,
}

/// 取消 Task（與桌面端點相同：還在進行的 Run 在同一個交易中一起被要求取消）。
pub(crate) async fn cancel_task(
    State(state): State<Arc<ApiState>>,
    Extension(record): Extension<CredentialRecord>,
    body: Result<Json<CancelTaskRequest>, JsonRejection>,
) -> Result<Json<ControllerTaskResponse>, ControllerToolError> {
    require_controller(&record)?;
    let Json(request) = body?;
    let scope = controller_scope(&state, &record, &request.claims).await?;
    // 預算用盡後的下一步必須送裁定（spec D-15）：取消再另建相同的 Task 會拿到全新的預算，所以取消也要經使用者核准（R151 第 9 項）
    let detail = state.store.task_detail(&scope.workspace_id, &request.task_id).await?;
    if detail.budget.exhausted.is_some() && !detail.task.task.status.is_terminal() {
        let summary = format!("取消預算已用盡的 Task「{}」（{}）", detail.task.task.title, detail.task.task.id);
        let reason = "這個 Task 的預算已用盡；取消後另建相同的 Task 會拿到全新的預算，所以取消也屬於「預算用盡後的下一步」";
        gate(&state, &scope, Matter::NextStepAfterBudget, &request.task_id, &summary, reason).await?;
    }
    let command = TaskCommand::Cancel { task_id: request.task_id.clone() };
    let change = state.store.apply_task_command(scope.workspace_id.clone(), command, state.clock.now()).await?;
    state.supervisor_wake.notify_one();
    Ok(Json(task_response(&state, &scope.workspace_id, &request.task_id, change.changed()).await?))
}

// ---------------------------------------------------------------- propose_decision_request

#[derive(Debug, Deserialize)]
pub(crate) struct ProposeDecisionRequest {
    question: String,
    reason: String,
    options: Vec<DecisionOption>,
    #[serde(default)]
    recommendation: Option<String>,
    #[serde(default)]
    evidence: Vec<String>,
    #[serde(default)]
    urgency: Option<Urgency>,
    #[serde(default)]
    deadline_minutes: Option<i64>,
    /// 依賴這個答案的 Task（只能是還沒派工的 pending、ready）。
    blocking: Vec<String>,
    #[serde(flatten)]
    claims: Claims,
}

/// `propose_decision_request` 的回應。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControllerDecisionRequested {
    pub decision_request_id: String,
    pub status: String,
    /// 同一個問題仍在等待時回到既有的請求（新列出的 Task 一併加入阻擋範圍）。
    pub existing: bool,
    pub message: String,
}

/// 向使用者提問：列在 `blocking` 的 Task 在回答前不能進入 ready 或派工，其他工作不受影響。
pub(crate) async fn propose_decision_request(
    State(state): State<Arc<ApiState>>,
    Extension(record): Extension<CredentialRecord>,
    body: Result<Json<ProposeDecisionRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<ControllerDecisionRequested>), ControllerToolError> {
    require_controller(&record)?;
    let Json(request) = body?;
    let scope = controller_scope(&state, &record, &request.claims).await?;
    let content = RequestContent::new(
        &request.question,
        &request.reason,
        request.options,
        request.recommendation.as_deref(),
        request.evidence,
        request.urgency,
        request.deadline_minutes,
    )
    .map_err(DecisionApplyError::from)?;
    let outcome = state
        .store
        .controller_question(&scope, new_id("decision")?, content, request.blocking, state.clock.now())
        .await?;
    let DecisionOutcome::Requested { request_id, existing } = outcome else {
        unreachable!("ControllerQuestion 命令只會得到 Requested 結果")
    };
    let status = if existing { StatusCode::OK } else { StatusCode::CREATED };
    Ok((
        status,
        Json(ControllerDecisionRequested {
            decision_request_id: request_id,
            status: "waiting_decision".to_string(),
            existing,
            message: "已建立裁定請求，等待使用者回答；列出的 Task 在回答前不會被派出，不依賴答案的工作可以照常派出。\
                      使用者回答後，背景執行的 wait 會回報「裁定已回答」事件通知你，請依答案更新契約後再派工。"
                .to_string(),
        }),
    ))
}
