//! Workspace 管理端點：建立、列出、封存（桌面角色專用，入口防護由 `guard` 統一處理）。
//!
//! 端點只做「解析請求 → 檢查目錄（經 WorkspaceFs Port）→ 交給 Store 套用 Domain 命令」，
//! 狀態規則與不變條件都在 Domain；所有寫入由 Store 的單一寫入佇列序列化。

use std::path::Path;
use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use baton_domain::workspace::{self, ModelOverrides, Workspace, WorkspaceCommand, WorkspaceError};
use baton_ports::DirectoryState;
use baton_store::{StoreError, WorkspaceApplyError};
use serde::{Deserialize, Serialize};

use crate::{ApiState, ErrorBody};

/// Workspace 集合路徑：GET 列出、POST 建立。
pub const WORKSPACES_PATH: &str = "/api/v1/workspaces";
/// 封存路徑樣板（`{id}` 是 Workspace 識別）。
pub(crate) const ARCHIVE_ROUTE: &str = "/api/v1/workspaces/{id}/archive";
/// 依角色的模型覆寫路徑樣板：PUT 整組取代（spec D-30）。
pub(crate) const MODEL_OVERRIDES_ROUTE: &str = "/api/v1/workspaces/{id}/model-overrides";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateWorkspaceRequest {
    /// 使用者選擇的目錄；可以不是 Git repo。
    pub path: String,
    /// 省略時取目錄名稱。
    #[serde(default)]
    pub name: Option<String>,
}

/// 列出端點的回應：全部 Workspace（含已封存），依建立順序。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceList {
    pub workspaces: Vec<Workspace>,
}

/// 端點錯誤：穩定的 `code` 加上給使用者看的說明。
pub(crate) struct ApiError {
    pub(crate) status: StatusCode,
    pub(crate) code: &'static str,
    pub(crate) message: String,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(ErrorBody { error: self.code.to_string(), message: self.message })).into_response()
    }
}

impl From<WorkspaceError> for ApiError {
    fn from(error: WorkspaceError) -> Self {
        let status = match &error {
            WorkspaceError::PathEmpty
            | WorkspaceError::PathNotAbsolute(_)
            | WorkspaceError::NameTooLong
            | WorkspaceError::ModelNameInvalid(_) => StatusCode::BAD_REQUEST,
            WorkspaceError::NotFound(_) => StatusCode::NOT_FOUND,
            WorkspaceError::AlreadyRegistered { .. } | WorkspaceError::AlreadyArchived(_) => StatusCode::CONFLICT,
        };
        ApiError { status, code: error.code(), message: error.to_string() }
    }
}

impl From<StoreError> for ApiError {
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::IncompatibleSchema { .. } => ApiError {
                status: StatusCode::SERVICE_UNAVAILABLE,
                code: "store_incompatible",
                message: error.to_string(),
            },
            other => ApiError {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                code: "internal_error",
                message: format!("Runtime 內部錯誤：{other}"),
            },
        }
    }
}

impl From<WorkspaceApplyError> for ApiError {
    fn from(error: WorkspaceApplyError) -> Self {
        match error {
            WorkspaceApplyError::Rejected(rejected) => rejected.into(),
            WorkspaceApplyError::Store(store) => store.into(),
        }
    }
}

pub(crate) async fn list(State(state): State<Arc<ApiState>>) -> Result<Json<WorkspaceList>, ApiError> {
    Ok(Json(WorkspaceList { workspaces: state.store.list_workspaces().await? }))
}

pub(crate) async fn create(
    State(state): State<Arc<ApiState>>,
    body: Result<Json<CreateWorkspaceRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<Workspace>), ApiError> {
    let Json(request) = body.map_err(|rejection| ApiError {
        status: StatusCode::BAD_REQUEST,
        code: "bad_request",
        message: format!("請求內容不是有效的 JSON，或缺少必要欄位 path：{}", rejection.body_text()),
    })?;

    // 先在 Domain 檢查路徑形式，才去問檔案系統
    let path = workspace::normalize_path(&request.path)?;
    match state.workspace_fs.inspect_directory(Path::new(&path)).await {
        Ok(DirectoryState::Directory) => {}
        Ok(DirectoryState::NotFound) => {
            return Err(ApiError {
                status: StatusCode::BAD_REQUEST,
                code: "workspace_path_not_found",
                message: format!("找不到目錄：{path}"),
            })
        }
        Ok(DirectoryState::NotADirectory) => {
            return Err(ApiError {
                status: StatusCode::BAD_REQUEST,
                code: "workspace_path_not_directory",
                message: format!("這個路徑不是目錄：{path}"),
            })
        }
        Err(error) => {
            return Err(ApiError {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                code: "internal_error",
                message: format!("無法檢查目錄 {path}：{error}"),
            })
        }
    }

    let command = WorkspaceCommand::Create { id: new_id("ws")?, name: request.name, path };
    let created = state.store.apply_workspace_command(command, state.clock.now()).await?;
    Ok((StatusCode::CREATED, Json(created)))
}

pub(crate) async fn archive(
    State(state): State<Arc<ApiState>>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<Workspace>, ApiError> {
    let archived = state.store.apply_workspace_command(WorkspaceCommand::Archive { id }, state.clock.now()).await?;
    Ok(Json(archived))
}

/// 設定依角色的模型覆寫；空字串或省略的角色表示不覆寫（沿用帳號的 Claude Code 預設）。
/// 只影響之後派出的 Run：已派出的 Run 在派工時就固定了模型。
pub(crate) async fn set_model_overrides(
    State(state): State<Arc<ApiState>>,
    UrlPath(id): UrlPath<String>,
    body: Result<Json<ModelOverrides>, JsonRejection>,
) -> Result<Json<Workspace>, ApiError> {
    let Json(overrides) = body.map_err(|rejection| ApiError {
        status: StatusCode::BAD_REQUEST,
        code: "bad_request",
        message: format!("請求內容不是有效的模型覆寫：{}", rejection.body_text()),
    })?;
    let command = WorkspaceCommand::SetModelOverrides { id, overrides };
    Ok(Json(state.store.apply_workspace_command(command, state.clock.now()).await?))
}

/// 穩定識別：前綴加 96 位元隨機值（Domain 不產生亂數，由呼叫端提供）。
pub(crate) fn new_id(prefix: &str) -> Result<String, ApiError> {
    let mut entropy = [0u8; 12];
    getrandom::fill(&mut entropy).map_err(|e| ApiError {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        code: "internal_error",
        message: format!("無法取得系統亂數：{e}"),
    })?;
    let hex: String = entropy.iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!("{prefix}_{hex}"))
}
