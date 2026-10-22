//! 生命週期端點（桌面角色專用，票 25）：完全退出、停止進度、各 Workspace 的「完全退出後自動續跑」與「繼續工作」。
//!
//! 規則都在 Domain（`baton_domain::lifecycle`）；端點只做「交給 Store 套用 → 對應成穩定的欄位」。
//! 完全退出的端點只持久化停止意圖並通知 Runtime 開始停止，立即回應 202；停止進度之後以 `GET` 查詢。

use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use baton_domain::lifecycle::{ExitReport, ExitTiming, LifecycleError, LifecyclePhase, WorkspaceLifecycle};
use baton_domain::Timestamp;
use baton_store::{LifecycleApplyError, LifecycleSnapshot};
use serde::{Deserialize, Serialize};

use crate::authorization::bad_json;
use crate::workspaces::ApiError;
use crate::ApiState;

/// 生命週期狀態路徑（GET）。
pub const LIFECYCLE_PATH: &str = "/api/v1/lifecycle";
/// 完全退出路徑（POST）。
pub const LIFECYCLE_EXIT_PATH: &str = "/api/v1/lifecycle/exit";
pub(crate) const WORKSPACE_LIFECYCLE_ROUTE: &str = "/api/v1/workspaces/{id}/lifecycle";
pub(crate) const WORKSPACE_RESUME_ROUTE: &str = "/api/v1/workspaces/{id}/resume";

/// Workspace 生命週期設定的路徑（PUT）。
pub fn workspace_lifecycle_path(workspace_id: &str) -> String {
    format!("/api/v1/workspaces/{workspace_id}/lifecycle")
}

/// 「繼續工作」的路徑（POST）。
pub fn workspace_resume_path(workspace_id: &str) -> String {
    format!("/api/v1/workspaces/{workspace_id}/resume")
}

/// 一個 Workspace 的生命週期設定與暫停狀態。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceLifecycleView {
    pub workspace_id: String,
    pub name: String,
    /// 「完全退出後自動續跑」；預設關閉。
    pub auto_resume_after_exit: bool,
    /// 被完全退出暫停的時間。
    pub paused_since: Option<Timestamp>,
    /// 停止意圖已解除、但這個 Workspace 還在等「繼續工作」。
    pub awaiting_resume: bool,
}

/// 生命週期總覽：系統匣狀態、完全退出的停止進度與異常、各 Workspace 的設定。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifecycleStatus {
    pub phase: LifecyclePhase,
    /// 停止意圖生效中（`stopping` 或 `stopped`）：不會啟動任何新的 Run。
    pub stop_intent_active: bool,
    /// 受控停止寬限期與強制終止確認的上限（spec D-11）。
    pub timing: ExitTiming,
    /// 進行中或最近一次的完全退出（含每個 Run 的停止結果與停止異常）；從未退出過為 `null`。
    pub exit: Option<ExitReport>,
    /// 目前尚未結束的 Run 數。
    pub active_runs: usize,
    /// 使用中的 Workspace，依建立順序。
    pub workspaces: Vec<WorkspaceLifecycleView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateWorkspaceLifecycleRequest {
    pub auto_resume_after_exit: bool,
}

pub(crate) fn view_of(name: &str, lifecycle: &WorkspaceLifecycle, stop_intent_active: bool) -> WorkspaceLifecycleView {
    WorkspaceLifecycleView {
        workspace_id: lifecycle.workspace_id.clone(),
        name: name.to_string(),
        auto_resume_after_exit: lifecycle.auto_resume_after_exit,
        paused_since: lifecycle.paused_since,
        awaiting_resume: !stop_intent_active && lifecycle.paused_since.is_some(),
    }
}

fn status_of(snapshot: LifecycleSnapshot, timing: ExitTiming) -> LifecycleStatus {
    let stop_intent_active = snapshot.state.stop_intent_active();
    LifecycleStatus {
        phase: snapshot.state.phase,
        stop_intent_active,
        timing,
        exit: snapshot.state.last_exit,
        active_runs: snapshot.active_runs,
        workspaces: snapshot
            .workspaces
            .iter()
            .map(|(workspace, lifecycle)| view_of(&workspace.name, lifecycle, stop_intent_active))
            .collect(),
    }
}

impl From<LifecycleApplyError> for ApiError {
    fn from(error: LifecycleApplyError) -> Self {
        match error {
            LifecycleApplyError::WorkspaceNotFound(id) => ApiError {
                status: StatusCode::NOT_FOUND,
                code: "workspace_not_found",
                message: format!("找不到 Workspace {id}"),
            },
            LifecycleApplyError::Rejected(rejected @ LifecycleError::StopIntentActive) => {
                ApiError { status: StatusCode::CONFLICT, code: rejected.code(), message: rejected.to_string() }
            }
            LifecycleApplyError::Store(store) => store.into(),
        }
    }
}

pub(crate) async fn status(State(state): State<Arc<ApiState>>) -> Result<Json<LifecycleStatus>, ApiError> {
    let snapshot = state.store.lifecycle_snapshot().await?;
    Ok(Json(status_of(snapshot, state.exit_timing)))
}

/// 完全退出：持久化停止意圖（資料庫；生命週期檔由停止流程的第一步鏡像），再通知 Runtime 開始停止。
/// 重複要求是冪等的。回應 202 與目前的狀態，停止進度之後以 `GET` 查詢。
pub(crate) async fn request_exit(State(state): State<Arc<ApiState>>) -> Result<Response, ApiError> {
    let now = state.clock.now();
    state.store.request_exit(state.exit_timing, now).await?;
    (state.log)("生命週期：已持久化完全退出的停止意圖");
    state.exit_signal.notify_one();
    let snapshot = state.store.lifecycle_snapshot().await?;
    Ok((StatusCode::ACCEPTED, Json(status_of(snapshot, state.exit_timing))).into_response())
}

/// 變更「完全退出後自動續跑」；只影響下次重新開啟，不會立即啟動或停止目前的工作。
pub(crate) async fn update_workspace(
    State(state): State<Arc<ApiState>>,
    UrlPath(id): UrlPath<String>,
    body: Result<Json<UpdateWorkspaceLifecycleRequest>, JsonRejection>,
) -> Result<Json<WorkspaceLifecycleView>, ApiError> {
    let Json(request) = body.map_err(bad_json)?;
    let (name, lifecycle, active) =
        state.store.set_auto_resume(&id, request.auto_resume_after_exit, state.clock.now()).await?;
    Ok(Json(view_of(&name, &lifecycle, active)))
}

/// 「繼續工作」：解除這個 Workspace 在完全退出後的暫停，讓它排隊中的工作恢復派工。
pub(crate) async fn resume_workspace(
    State(state): State<Arc<ApiState>>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<WorkspaceLifecycleView>, ApiError> {
    let (name, lifecycle, active) = state.store.resume_workspace(&id, state.clock.now()).await?;
    // 讓排隊中的工作儘快被派出
    state.supervisor_wake.notify_one();
    Ok(Json(view_of(&name, &lifecycle, active)))
}
