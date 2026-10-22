//! Run 端點（桌面角色專用）：派工、列出、讀取、取消。
//!
//! 規則都在 Domain（`baton_domain::run`、`baton_domain::scheduler`）；端點只做「交給 Store → 對應成穩定的錯誤代碼 → 通知 Supervisor」。
//! 派工要求交給 Scheduler：有帳號與名額就建立 Run（201），否則留在佇列並回報原因（202）；
//! 派工只寫入 Run、lease 與派工 Outbox，不會同步啟動程序；啟動由 Supervisor 在交易外進行。
//! 取消不等程序停止：已啟動的 Run 回傳時仍是 `starting`／`running`，並帶著 `cancel_requested_at`。

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use baton_domain::run::{RunCommand, RunError, RunView};
use baton_domain::scheduler::WaitReason;
use baton_store::{RunApplyError, ScheduleRequest};
use serde::{Deserialize, Serialize};

use crate::scheduling::schedule_now;
use crate::tasks::TaskApiError;
use crate::workspaces::ApiError;
use crate::ApiState;

pub(crate) const DISPATCH_ROUTE: &str = "/api/v1/workspaces/{id}/tasks/{task_id}/dispatch";
pub(crate) const RUNS_ROUTE: &str = "/api/v1/workspaces/{id}/runs";
pub(crate) const RUN_ROUTE: &str = "/api/v1/workspaces/{id}/runs/{run_id}";
pub(crate) const CANCEL_ROUTE: &str = "/api/v1/workspaces/{id}/runs/{run_id}/cancel";

/// 路徑；Runtime 與用戶端共用。
pub fn task_dispatch_path(workspace_id: &str, task_id: &str) -> String {
    format!("{}/dispatch", crate::task_path(workspace_id, task_id))
}

pub fn runs_path(workspace_id: &str) -> String {
    format!("{}/{workspace_id}/runs", crate::WORKSPACES_PATH)
}

pub fn run_path(workspace_id: &str, run_id: &str) -> String {
    format!("{}/{run_id}", runs_path(workspace_id))
}

pub fn run_cancel_path(workspace_id: &str, run_id: &str) -> String {
    format!("{}/cancel", run_path(workspace_id, run_id))
}

/// Workspace 的全部 Run（含已結束），依建立順序。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunList {
    pub runs: Vec<RunView>,
}

impl From<RunError> for TaskApiError {
    fn from(error: RunError) -> Self {
        let status = match &error {
            RunError::WorkspaceNotFound(_)
            | RunError::TaskNotFound(_)
            | RunError::RunNotFound(_)
            | RunError::SessionNotFound(_) => StatusCode::NOT_FOUND,
            RunError::AdmissionDenied(missing) => {
                return Self::Denied { message: error.to_string(), missing: missing.clone() }
            }
            RunError::TaskNotReady(_)
            | RunError::AdmissionRejected(_)
            | RunError::DependenciesUnfinished(_)
            | RunError::ExecutionConflict { .. }
            | RunError::TransitionInvalid { .. }
            | RunError::StaleReport
            | RunError::LeaseNotExpired
            | RunError::CancelNotRequested
            | RunError::LaunchHalted
            | RunError::SessionBusy { .. } => StatusCode::CONFLICT,
        };
        Self::Plain(ApiError { status, code: error.code(), message: error.to_string() })
    }
}

impl From<RunApplyError> for TaskApiError {
    fn from(error: RunApplyError) -> Self {
        match error {
            RunApplyError::Rejected(rejected) => rejected.into(),
            RunApplyError::Store(store) => Self::Plain(store.into()),
        }
    }
}

/// 派工請求的內容（可省略）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DispatchRequest {
    /// 數字越大越優先，預設 0；同優先序依要求順序。
    #[serde(default)]
    pub priority: i32,
}

/// 派工要求被接受但還沒有名額：留在佇列，名額空出來時由 Scheduler 自動派出。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueuedDispatch {
    /// 固定為 `true`，用戶端以它區分「已建立 Run」與「排隊中」。
    pub queued: bool,
    pub task_id: String,
    pub priority: i32,
    /// 為什麼還沒開始：容量、額度、授權、相依、狀態不明或帳號問題，各自分開。
    pub reasons: Vec<WaitReason>,
}

/// 派工的兩種結果：已建立 Run（201，本文是 Run），或排隊中（202，本文是 `QueuedDispatch`）。
pub(crate) enum DispatchResponse {
    Started(Box<RunView>),
    Queued(QueuedDispatch),
}

impl IntoResponse for DispatchResponse {
    fn into_response(self) -> Response {
        match self {
            DispatchResponse::Started(view) => (StatusCode::CREATED, Json(view)).into_response(),
            DispatchResponse::Queued(queued) => (StatusCode::ACCEPTED, Json(queued)).into_response(),
        }
    }
}

/// 派工：Scheduler 依相依、授權、帳號狀態與容量決定現在能不能派。能派就選定帳號、建立 Run（`queued`）、
/// 取得 execution lease、寫入派工 Outbox，Task 進入 `running`；不能派就排進佇列並回報原因。
pub(crate) async fn dispatch(
    State(state): State<Arc<ApiState>>,
    UrlPath((id, task_id)): UrlPath<(String, String)>,
    body: Bytes,
) -> Result<DispatchResponse, TaskApiError> {
    let request = if body.is_empty() {
        DispatchRequest::default()
    } else {
        serde_json::from_slice::<DispatchRequest>(&body).map_err(|e| ApiError {
            status: StatusCode::BAD_REQUEST,
            code: "bad_request",
            message: format!("請求內容不正確：{e}"),
        })?
    };
    let schedule = ScheduleRequest { workspace_id: id.clone(), task_id: task_id.clone(), priority: request.priority };
    let outcome = schedule_now(&state, Some(schedule)).await?;
    if let Some(started) = outcome.started.iter().find(|s| s.task_id == task_id) {
        let view = state.store.run_detail(&id, &started.run_id).await?;
        return Ok(DispatchResponse::Started(Box::new(view)));
    }
    let reasons = outcome
        .waiting
        .into_iter()
        .find(|w| w.task_id == task_id)
        .map(|w| w.reasons)
        .unwrap_or_default();
    Ok(DispatchResponse::Queued(QueuedDispatch { queued: true, task_id, priority: request.priority, reasons }))
}

pub(crate) async fn list(
    State(state): State<Arc<ApiState>>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<RunList>, TaskApiError> {
    Ok(Json(RunList { runs: state.store.run_list(&id).await? }))
}

pub(crate) async fn detail(
    State(state): State<Arc<ApiState>>,
    UrlPath((id, run_id)): UrlPath<(String, String)>,
) -> Result<Json<RunView>, TaskApiError> {
    Ok(Json(state.store.run_detail(&id, &run_id).await?))
}

/// 要求取消 Run；已經結束或已經要求過取消的 Run 是冪等的（回 200 與目前狀態）。
pub(crate) async fn cancel(
    State(state): State<Arc<ApiState>>,
    UrlPath((id, run_id)): UrlPath<(String, String)>,
) -> Result<Json<RunView>, TaskApiError> {
    let command = RunCommand::RequestCancel { run_id: run_id.clone() };
    state.store.apply_run_command(id.clone(), command, state.clock.now()).await?;
    state.supervisor_wake.notify_one();
    Ok(Json(state.store.run_detail(&id, &run_id).await?))
}
