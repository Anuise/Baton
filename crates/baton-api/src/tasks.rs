//! Goal、Task 與六項最小派工契約的端點（桌面角色專用）：工作面板資料、建立、契約變更、相依、准入檢查、ready 與取消。
//!
//! 規則都在 Domain（`baton_domain::task`）；端點只做「解析請求 → 交給 Store → 對應成穩定的錯誤代碼」。
//! 範圍一律以 URL 中的 Workspace 為界。

use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use baton_domain::authorization::Capability;
use baton_domain::run::RunView;
use baton_domain::verification::VerificationRecord;
use baton_domain::task::{ContractContent, Goal, MissingItem, TaskCommand, TaskContract, TaskError, TaskView};
use baton_store::TaskApplyError;
use serde::{Deserialize, Serialize};

use crate::authorization::{bad_json, CapabilityInfo};
use crate::workspaces::{new_id, ApiError};
use crate::{ApiState, ErrorBody};

pub(crate) const GOALS_ROUTE: &str = "/api/v1/workspaces/{id}/goals";
pub(crate) const TASKS_ROUTE: &str = "/api/v1/workspaces/{id}/tasks";
pub(crate) const TASK_ROUTE: &str = "/api/v1/workspaces/{id}/tasks/{task_id}";
pub(crate) const CONTRACT_ROUTE: &str = "/api/v1/workspaces/{id}/tasks/{task_id}/contract";
pub(crate) const DEPENDENCIES_ROUTE: &str = "/api/v1/workspaces/{id}/tasks/{task_id}/dependencies";
pub(crate) const ADMISSION_ROUTE: &str = "/api/v1/workspaces/{id}/tasks/{task_id}/admission-check";
pub(crate) const READY_ROUTE: &str = "/api/v1/workspaces/{id}/tasks/{task_id}/ready";
pub(crate) const CANCEL_ROUTE: &str = "/api/v1/workspaces/{id}/tasks/{task_id}/cancel";

/// 路徑；Runtime 與用戶端共用。
pub fn goals_path(workspace_id: &str) -> String {
    format!("{}/{workspace_id}/goals", crate::WORKSPACES_PATH)
}

pub fn tasks_path(workspace_id: &str) -> String {
    format!("{}/{workspace_id}/tasks", crate::WORKSPACES_PATH)
}

pub fn task_path(workspace_id: &str, task_id: &str) -> String {
    format!("{}/{task_id}", tasks_path(workspace_id))
}

pub fn task_contract_path(workspace_id: &str, task_id: &str) -> String {
    format!("{}/contract", task_path(workspace_id, task_id))
}

pub fn task_dependencies_path(workspace_id: &str, task_id: &str) -> String {
    format!("{}/dependencies", task_path(workspace_id, task_id))
}

pub fn task_admission_check_path(workspace_id: &str, task_id: &str) -> String {
    format!("{}/admission-check", task_path(workspace_id, task_id))
}

pub fn task_ready_path(workspace_id: &str, task_id: &str) -> String {
    format!("{}/ready", task_path(workspace_id, task_id))
}

pub fn task_cancel_path(workspace_id: &str, task_id: &str) -> String {
    format!("{}/cancel", task_path(workspace_id, task_id))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateGoalRequest {
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateTaskRequest {
    pub goal_id: String,
    pub title: String,
    /// 省略時建立一份空白契約（版本 1），之後再補。
    #[serde(default)]
    pub contract: ContractContent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateContractRequest {
    /// 整份取代。
    pub contract: ContractContent,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddDependencyRequest {
    pub depends_on: String,
}

/// 工作面板需要的全部資料。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskBoard {
    pub workspace_id: String,
    pub goals: Vec<Goal>,
    pub tasks: Vec<TaskView>,
    /// Workspace 的全部 Run（含已結束），依建立順序；卡片上的執行狀態由它組出。
    pub runs: Vec<RunView>,
    /// Workspace 的全部驗證紀錄，由舊到新；卡片上的驗證結果與證據由它組出（票 13）。
    pub verifications: Vec<VerificationRecord>,
    /// 契約「所需能力」可選的清單。
    pub capabilities: Vec<CapabilityInfo>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskDetail {
    pub task: TaskView,
    /// 契約的全部版本，由舊到新（含目前版本）。
    pub contract_history: Vec<TaskContract>,
}

/// 准入檢查的唯讀預覽。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdmissionReport {
    pub admissible: bool,
    pub missing: Vec<MissingItem>,
}

/// 本組端點的錯誤：一般錯誤，或附上缺項清單的准入拒絕。
pub(crate) enum TaskApiError {
    Plain(ApiError),
    Denied { message: String, missing: Vec<MissingItem> },
}

#[derive(Serialize)]
struct DeniedBody {
    #[serde(flatten)]
    error: ErrorBody,
    missing: Vec<MissingItem>,
}

impl IntoResponse for TaskApiError {
    fn into_response(self) -> Response {
        match self {
            TaskApiError::Plain(error) => error.into_response(),
            TaskApiError::Denied { message, missing } => (
                StatusCode::CONFLICT,
                Json(DeniedBody {
                    error: ErrorBody { error: "task_admission_denied".to_string(), message },
                    missing,
                }),
            )
                .into_response(),
        }
    }
}

impl From<ApiError> for TaskApiError {
    fn from(error: ApiError) -> Self {
        Self::Plain(error)
    }
}

impl From<TaskError> for TaskApiError {
    fn from(error: TaskError) -> Self {
        let status = match &error {
            TaskError::WorkspaceNotFound(_) | TaskError::GoalNotFound(_) | TaskError::TaskNotFound(_) => {
                StatusCode::NOT_FOUND
            }
            TaskError::GoalTitleInvalid
            | TaskError::TaskTitleInvalid
            | TaskError::ContractInvalid(_)
            | TaskError::DependencySelf(_) => StatusCode::BAD_REQUEST,
            TaskError::WorkspaceArchived(_)
            | TaskError::DependencyCycle { .. }
            | TaskError::TaskClosed { .. }
            | TaskError::TransitionInvalid { .. } => StatusCode::CONFLICT,
            TaskError::AdmissionDenied(missing) => {
                return Self::Denied { message: error.to_string(), missing: missing.clone() }
            }
        };
        Self::Plain(ApiError { status, code: error.code(), message: error.to_string() })
    }
}

impl From<TaskApplyError> for TaskApiError {
    fn from(error: TaskApplyError) -> Self {
        match error {
            TaskApplyError::Rejected(rejected) => rejected.into(),
            TaskApplyError::Store(store) => Self::Plain(store.into()),
        }
    }
}

impl From<JsonRejection> for TaskApiError {
    fn from(rejection: JsonRejection) -> Self {
        Self::Plain(bad_json(rejection))
    }
}

/// 套用命令後讀回該 Task 的最新投影；有變動回 201，冪等（沒有變動）回 200。
async fn apply_and_view(
    state: &ApiState,
    workspace_id: String,
    task_id: String,
    command: TaskCommand,
    created_status: StatusCode,
) -> Result<(StatusCode, Json<TaskView>), TaskApiError> {
    let change = state.store.apply_task_command(workspace_id.clone(), command, state.clock.now()).await?;
    let status = if change.changed() { created_status } else { StatusCode::OK };
    let detail = state.store.task_detail(&workspace_id, &task_id).await?;
    Ok((status, Json(detail.task)))
}

pub(crate) async fn board(
    State(state): State<Arc<ApiState>>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<TaskBoard>, TaskApiError> {
    let data = state.store.task_board(&id).await?;
    Ok(Json(TaskBoard {
        workspace_id: id,
        goals: data.goals,
        tasks: data.tasks,
        runs: data.runs,
        verifications: data.verifications,
        capabilities: Capability::ALL
            .into_iter()
            .map(|c| CapabilityInfo { capability: c, label: c.label().to_string(), high_risk: c.is_high_risk() })
            .collect(),
    }))
}

pub(crate) async fn create_goal(
    State(state): State<Arc<ApiState>>,
    UrlPath(id): UrlPath<String>,
    body: Result<Json<CreateGoalRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<Goal>), TaskApiError> {
    let Json(request) = body?;
    let command = TaskCommand::CreateGoal { id: new_id("goal")?, title: request.title };
    let change = state.store.apply_task_command(id, command, state.clock.now()).await?;
    let goal = change.goal.expect("建立 Goal 一定回傳 Goal");
    Ok((StatusCode::CREATED, Json(goal)))
}

pub(crate) async fn create_task(
    State(state): State<Arc<ApiState>>,
    UrlPath(id): UrlPath<String>,
    body: Result<Json<CreateTaskRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<TaskView>), TaskApiError> {
    let Json(request) = body?;
    let task_id = new_id("task")?;
    let command = TaskCommand::CreateTask {
        id: task_id.clone(),
        goal_id: request.goal_id,
        title: request.title,
        contract: request.contract,
    };
    apply_and_view(&state, id, task_id, command, StatusCode::CREATED).await
}

pub(crate) async fn detail(
    State(state): State<Arc<ApiState>>,
    UrlPath((id, task_id)): UrlPath<(String, String)>,
) -> Result<Json<TaskDetail>, TaskApiError> {
    let data = state.store.task_detail(&id, &task_id).await?;
    Ok(Json(TaskDetail { task: data.task, contract_history: data.contract_history }))
}

/// 契約內容不同回 201 與新版本的投影；內容相同回 200（不新增版本）。
pub(crate) async fn update_contract(
    State(state): State<Arc<ApiState>>,
    UrlPath((id, task_id)): UrlPath<(String, String)>,
    body: Result<Json<UpdateContractRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<TaskView>), TaskApiError> {
    let Json(request) = body?;
    let command =
        TaskCommand::UpdateContract { task_id: task_id.clone(), contract: request.contract, note: request.note };
    apply_and_view(&state, id, task_id, command, StatusCode::CREATED).await
}

pub(crate) async fn add_dependency(
    State(state): State<Arc<ApiState>>,
    UrlPath((id, task_id)): UrlPath<(String, String)>,
    body: Result<Json<AddDependencyRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<TaskView>), TaskApiError> {
    let Json(request) = body?;
    let command = TaskCommand::AddDependency { task_id: task_id.clone(), depends_on: request.depends_on };
    apply_and_view(&state, id, task_id, command, StatusCode::CREATED).await
}

pub(crate) async fn admission_check(
    State(state): State<Arc<ApiState>>,
    UrlPath((id, task_id)): UrlPath<(String, String)>,
) -> Result<Json<AdmissionReport>, TaskApiError> {
    let missing = state.store.task_admission_report(&id, &task_id).await?;
    Ok(Json(AdmissionReport { admissible: missing.is_empty(), missing }))
}

pub(crate) async fn mark_ready(
    State(state): State<Arc<ApiState>>,
    UrlPath((id, task_id)): UrlPath<(String, String)>,
) -> Result<(StatusCode, Json<TaskView>), TaskApiError> {
    apply_and_view(&state, id, task_id.clone(), TaskCommand::MarkReady { task_id }, StatusCode::OK).await
}

pub(crate) async fn cancel(
    State(state): State<Arc<ApiState>>,
    UrlPath((id, task_id)): UrlPath<(String, String)>,
) -> Result<(StatusCode, Json<TaskView>), TaskApiError> {
    apply_and_view(&state, id, task_id.clone(), TaskCommand::Cancel { task_id }, StatusCode::OK).await
}
