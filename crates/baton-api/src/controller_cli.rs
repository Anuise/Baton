//! Controller 頁簽的端點（桌面角色專用，desktop-controller-cli 票 03）與 Controller 宿主的回報端點（Agent 角色、Controller 憑證）。
//!
//! 開啟只持久化「使用者要求開啟」（`starting`），由 Supervisor 寫下 launch intent 後開窗；結束只記下要求，
//! 由 Supervisor 結束宿主與 `claude`。頁簽輪詢 `GET` 取得最新一次 Controller Run 的狀態與結束原因。
//! 換帳號重開（票 05）也只記下要求與新 Profile；目前的 Run 結束時，Store 在同一個交易中開新的 Run。
//! 規則都在 Domain（`baton_domain::controller_cli`）。

use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use baton_domain::auth::CredentialRecord;
use baton_domain::controller_cli::{ControllerCliError, ControllerCliRun};
use baton_store::ControllerCliApplyError;
use serde::{Deserialize, Serialize};

use crate::agent::Claims;
use crate::workspaces::ApiError;
use crate::ApiState;

pub(crate) const CONTROLLER_CLI_ROUTE: &str = "/api/v1/workspaces/{id}/controller-cli";
pub(crate) const CONTROLLER_CLI_END_ROUTE: &str = "/api/v1/workspaces/{id}/controller-cli/end";
pub(crate) const CONTROLLER_CLI_SWITCH_ROUTE: &str = "/api/v1/workspaces/{id}/controller-cli/switch";

/// `GET`：Controller 頁簽的狀態；`POST`（`OpenControllerCliRequest`）：開啟。
pub fn controller_cli_path(workspace_id: &str) -> String {
    format!("{}/{workspace_id}/controller-cli", crate::WORKSPACES_PATH)
}

/// `POST`：結束開啟中的 Controller CLI。
pub fn controller_cli_end_path(workspace_id: &str) -> String {
    format!("{}/end", controller_cli_path(workspace_id))
}

/// `POST`（`OpenControllerCliRequest`）：換帳號重開——結束開著的 Controller CLI，再以指定 Profile 開新的（不接續原生 Session）。
pub fn controller_cli_switch_path(workspace_id: &str) -> String {
    format!("{}/switch", controller_cli_path(workspace_id))
}

/// 開啟與換帳號重開的請求內容。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenControllerCliRequest {
    /// 當 Controller 帳號的 Account Profile。
    pub profile_id: String,
}

/// Controller 頁簽的狀態：這個 Workspace 最近一次的 Controller Run（含已結束、開啟失敗）；從未開啟過為 `None`。
/// `run.status` 是 `starting`／`running` 時代表 Controller CLI 開著。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControllerCliView {
    pub workspace_id: String,
    pub run: Option<ControllerCliRun>,
}

/// 宿主回報 `claude` 已啟動。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostStartedRequest {
    pub claude_pid: u32,
}

/// 宿主回報 `claude` 已結束；結束碼取不到（例如被訊號終止）時為 `None`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostEndedRequest {
    #[serde(default)]
    pub exit_code: Option<i32>,
}

/// 宿主回報被接受。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostAck {
    pub accepted: bool,
}

impl From<ControllerCliApplyError> for ApiError {
    fn from(error: ControllerCliApplyError) -> Self {
        match error {
            ControllerCliApplyError::Store(store) => store.into(),
            ControllerCliApplyError::Rejected(rejected) => {
                let status = match &rejected {
                    ControllerCliError::WorkspaceNotFound(_)
                    | ControllerCliError::ProfileNotFound(_)
                    | ControllerCliError::RunNotFound(_) => StatusCode::NOT_FOUND,
                    _ => StatusCode::CONFLICT,
                };
                ApiError { status, code: rejected.code(), message: rejected.to_string() }
            }
        }
    }
}

pub(crate) async fn view(
    State(state): State<Arc<ApiState>>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<ControllerCliView>, ApiError> {
    let run = state.store.latest_controller_cli_run(&id).await?;
    Ok(Json(ControllerCliView { workspace_id: id, run }))
}

pub(crate) async fn open(
    State(state): State<Arc<ApiState>>,
    UrlPath(id): UrlPath<String>,
    body: Result<Json<OpenControllerCliRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<ControllerCliView>), ApiError> {
    let Json(request) = body.map_err(crate::authorization::bad_json)?;
    let ids = baton_domain::controller_cli::OpenIds {
        run_id: crate::workspaces::new_id("ctl")?,
        command_id: crate::workspaces::new_id("cmd")?,
    };
    let run = state.store.open_controller_cli(&id, &request.profile_id, ids, state.clock.now()).await?;
    // 開窗由 Supervisor 進行；這裡只提早叫醒它
    state.supervisor_wake.notify_one();
    Ok((StatusCode::ACCEPTED, Json(ControllerCliView { workspace_id: id, run: Some(run) })))
}

/// 換帳號重開：結束開著的 Controller CLI，再以指定 Profile 開新的（不接續原生 Session）。
/// 回傳目前（即將結束）的 Run；新的 Run 在它結束後出現在 `GET`。
pub(crate) async fn switch(
    State(state): State<Arc<ApiState>>,
    UrlPath(id): UrlPath<String>,
    body: Result<Json<OpenControllerCliRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<ControllerCliView>), ApiError> {
    let Json(request) = body.map_err(crate::authorization::bad_json)?;
    let run = state.store.request_controller_cli_switch(&id, &request.profile_id, state.clock.now()).await?;
    state.supervisor_wake.notify_one();
    Ok((StatusCode::ACCEPTED, Json(ControllerCliView { workspace_id: id, run: Some(run) })))
}

pub(crate) async fn end(
    State(state): State<Arc<ApiState>>,
    UrlPath(id): UrlPath<String>,
) -> Result<(StatusCode, Json<ControllerCliView>), ApiError> {
    let run = state.store.request_controller_cli_end(&id, state.clock.now()).await?;
    state.supervisor_wake.notify_one();
    Ok((StatusCode::ACCEPTED, Json(ControllerCliView { workspace_id: id, run: Some(run) })))
}

/// 宿主回報 `claude` 已啟動：只接受仍開著的 Controller CLI 的 Controller 憑證。
pub(crate) async fn host_started(
    State(state): State<Arc<ApiState>>,
    Extension(record): Extension<CredentialRecord>,
    body: Result<Json<HostStartedRequest>, JsonRejection>,
) -> Result<Json<HostAck>, ApiError> {
    let scope = crate::controller::controller_scope(&state, &record, &Claims::default()).await?;
    let Json(request) = body.map_err(crate::authorization::bad_json)?;
    state.store.controller_cli_host_started(&scope, request.claude_pid, state.clock.now()).await?;
    Ok(Json(HostAck { accepted: true }))
}

/// 宿主回報 `claude` 已結束：Run 進入 `ended`，憑證隨之失效。
pub(crate) async fn host_ended(
    State(state): State<Arc<ApiState>>,
    Extension(record): Extension<CredentialRecord>,
    body: Result<Json<HostEndedRequest>, JsonRejection>,
) -> Result<Json<HostAck>, ApiError> {
    let scope = crate::controller::controller_scope(&state, &record, &Claims::default()).await?;
    let Json(request) = body.map_err(crate::authorization::bad_json)?;
    // 已要求換帳號重開時，結束的同一個交易中開新的 Run
    let next = baton_domain::controller_cli::OpenIds {
        run_id: crate::workspaces::new_id("ctl")?,
        command_id: crate::workspaces::new_id("cmd")?,
    };
    state.store.controller_cli_host_ended(&scope, request.exit_code, Some(next), state.clock.now()).await?;
    state.supervisor_wake.notify_one();
    Ok(Json(HostAck { accepted: true }))
}
