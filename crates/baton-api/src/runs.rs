//! Run 端點（桌面角色專用）：派工、列出、讀取、取消。
//!
//! 規則都在 Domain（`baton_domain::run`）；端點只做「交給 Store → 對應成穩定的錯誤代碼 → 通知 Supervisor」。
//! 派工只寫入 Run、lease 與派工 Outbox，不會同步啟動程序；啟動由 Supervisor 在交易外進行。
//! 取消不等程序停止：已啟動的 Run 回傳時仍是 `starting`／`running`，並帶著 `cancel_requested_at`。

use std::sync::Arc;

use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use axum::Json;
use baton_domain::run::{RunCommand, RunError, RunView};
use baton_store::RunApplyError;
use serde::{Deserialize, Serialize};

use crate::tasks::TaskApiError;
use crate::workspaces::{new_id, ApiError};
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
            RunError::WorkspaceNotFound(_) | RunError::TaskNotFound(_) | RunError::RunNotFound(_) => {
                StatusCode::NOT_FOUND
            }
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
            | RunError::CancelNotRequested => StatusCode::CONFLICT,
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

/// 派工：建立 Run（`queued`）、取得 execution lease、寫入派工 Outbox，Task 進入 `running`。
pub(crate) async fn dispatch(
    State(state): State<Arc<ApiState>>,
    UrlPath((id, task_id)): UrlPath<(String, String)>,
) -> Result<(StatusCode, Json<RunView>), TaskApiError> {
    let run_id = new_id("run")?;
    let command = RunCommand::Dispatch { run_id: run_id.clone(), task_id };
    state.store.apply_run_command(id.clone(), command, state.clock.now()).await?;
    state.supervisor_wake.notify_one();
    let view = state.store.run_detail(&id, &run_id).await?;
    Ok((StatusCode::CREATED, Json(view)))
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
