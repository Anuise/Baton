//! 生命週期端點（桌面角色專用，票 25）：完全退出、停止進度、各 Workspace 的「完全退出後自動續跑」與「繼續工作」；
//! 跨 Windows 登入（票 33）：「登入時自動啟動」與各 Workspace 的「跨登入自動續跑」。兩項新設定與完全退出後的設定分開保存與顯示。
//!
//! 規則都在 Domain（`baton_domain::lifecycle`）；端點只做「交給 Store 套用 → 對應成穩定的欄位」。
//! 完全退出的端點只持久化停止意圖並通知 Runtime 開始停止，立即回應 202；停止進度之後以 `GET` 查詢。

use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use baton_domain::lifecycle::{
    ExitReport, ExitTiming, LifecycleError, LifecycleFile, LifecyclePhase, PauseCause, WorkspaceLifecycle,
};
use baton_domain::login::LoginStartup;
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
/// 「登入時自動啟動」的路徑（PUT，票 33）。
pub const LOGIN_STARTUP_PATH: &str = "/api/v1/lifecycle/login-startup";
pub(crate) const WORKSPACE_LOGIN_RESUME_ROUTE: &str = "/api/v1/workspaces/{id}/login-resume";

/// Workspace 生命週期設定的路徑（PUT）。
pub fn workspace_lifecycle_path(workspace_id: &str) -> String {
    format!("/api/v1/workspaces/{workspace_id}/lifecycle")
}

/// Workspace「跨登入自動續跑」的路徑（PUT，票 33）。
pub fn workspace_login_resume_path(workspace_id: &str) -> String {
    format!("/api/v1/workspaces/{workspace_id}/login-resume")
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
    /// 「跨登入自動續跑」（票 33）；預設關閉，與 `auto_resume_after_exit` 分開保存。
    pub auto_resume_across_login: bool,
    /// 被暫停的時間。
    pub paused_since: Option<Timestamp>,
    /// 暫停的原因：`full_exit`、`new_login`（未開啟跨登入自動續跑）或 `login_unverified`（無法核對是否跨登入）。
    pub paused_by: Option<PauseCause>,
    /// 停止意圖已解除、但這個 Workspace 還在等「繼續工作」。
    pub awaiting_resume: bool,
}

/// 「登入時自動啟動」（票 33）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoginStartupView {
    /// 使用者的偏好；預設關閉。完全退出不改變它。
    pub enabled: bool,
    /// Windows 上目前是否已登記（HKCU Run 機碼）；查不到為 `null`。
    pub registered: Option<bool>,
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
    /// 「登入時自動啟動」（票 33）。
    pub login_startup: LoginStartupView,
    /// 使用中的 Workspace，依建立順序。
    pub workspaces: Vec<WorkspaceLifecycleView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateWorkspaceLifecycleRequest {
    pub auto_resume_after_exit: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateLoginResumeRequest {
    pub auto_resume_across_login: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateLoginStartupRequest {
    pub enabled: bool,
}

pub(crate) fn view_of(name: &str, lifecycle: &WorkspaceLifecycle, stop_intent_active: bool) -> WorkspaceLifecycleView {
    WorkspaceLifecycleView {
        workspace_id: lifecycle.workspace_id.clone(),
        name: name.to_string(),
        auto_resume_after_exit: lifecycle.auto_resume_after_exit,
        auto_resume_across_login: lifecycle.auto_resume_across_login,
        paused_since: lifecycle.paused_since,
        paused_by: lifecycle.paused_since.and(lifecycle.paused_by),
        awaiting_resume: !stop_intent_active && lifecycle.paused_since.is_some(),
    }
}

fn login_startup_view(startup: &LoginStartup, state: &ApiState) -> LoginStartupView {
    LoginStartupView { enabled: startup.enabled, registered: state.startup_registration.is_registered().ok() }
}

fn status_of(snapshot: LifecycleSnapshot, state: &ApiState) -> LifecycleStatus {
    let stop_intent_active = snapshot.state.stop_intent_active();
    let timing = state.exit_timing;
    LifecycleStatus {
        login_startup: login_startup_view(&snapshot.login_startup, state),
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
    Ok(Json(status_of(snapshot, &state)))
}

/// 完全退出：持久化停止意圖（先資料庫、再生命週期檔，spec D-25），再通知 Runtime 開始停止。
/// 生命週期檔在回應前寫入，不必等停止流程開始；寫入失敗只記日誌，停止流程開始時會再鏡像一次。
/// 重複要求是冪等的。回應 202 與目前的狀態，停止進度之後以 `GET` 查詢。
pub(crate) async fn request_exit(State(state): State<Arc<ApiState>>) -> Result<Response, ApiError> {
    let now = state.clock.now();
    let lifecycle = state.store.request_exit(state.exit_timing, now).await?;
    (state.log)("生命週期：已持久化完全退出的停止意圖");
    let file = LifecycleFile::mirror(&lifecycle, now);
    let data_dir = state.data_dir.clone();
    if !matches!(
        tokio::task::spawn_blocking(move || baton_store::write_lifecycle_file(&data_dir, &file)).await,
        Ok(Ok(()))
    ) {
        (state.log)("生命週期：無法把停止意圖寫入生命週期檔，交給停止流程再試");
    }
    state.exit_signal.notify_one();
    let snapshot = state.store.lifecycle_snapshot().await?;
    Ok((StatusCode::ACCEPTED, Json(status_of(snapshot, &state))).into_response())
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

/// 變更「跨登入自動續跑」（票 33）；只影響之後的跨登入啟動，不會立即啟動或停止目前的工作。
pub(crate) async fn update_login_resume(
    State(state): State<Arc<ApiState>>,
    UrlPath(id): UrlPath<String>,
    body: Result<Json<UpdateLoginResumeRequest>, JsonRejection>,
) -> Result<Json<WorkspaceLifecycleView>, ApiError> {
    let Json(request) = body.map_err(bad_json)?;
    let (name, lifecycle, active) =
        state.store.set_login_resume(&id, request.auto_resume_across_login, state.clock.now()).await?;
    Ok(Json(view_of(&name, &lifecycle, active)))
}

/// 變更「登入時自動啟動」（票 33）：先改 Windows 的登記，成功才保存偏好；登記失敗時偏好不變並回報原因。
/// 只影響之後的 Windows 登入，不啟動也不停止任何工作。
pub(crate) async fn update_login_startup(
    State(state): State<Arc<ApiState>>,
    body: Result<Json<UpdateLoginStartupRequest>, JsonRejection>,
) -> Result<Json<LoginStartupView>, ApiError> {
    let Json(request) = body.map_err(bad_json)?;
    if let Err(error) = state.startup_registration.set_registered(request.enabled) {
        (state.log)(&format!("登入啟動：無法變更 Windows 的登記：{error}"));
        return Err(ApiError {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "login_startup_registration_failed",
            message: format!("無法變更 Windows 的登入啟動登記：{error}"),
        });
    }
    let startup = state.store.set_login_startup(request.enabled, state.clock.now()).await?;
    Ok(Json(login_startup_view(&startup, &state)))
}
