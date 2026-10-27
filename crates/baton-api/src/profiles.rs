//! account_profile 管理端點：建立、以既有目錄建立、列出、核對登入狀態、開啟原生登入終端機、封存（桌面角色專用）。
//!
//! 端點只做「解析請求 → 經 Port 取得事實 → 交給 Store 套用 Domain 命令」；
//! 數量上限、設定目錄不共用、可不可以派工都是 Domain 的規則。
//! 登入資料只存在各 Profile 自己的設定目錄中，由 Claude Code 保管；這裡不讀取、不保存、不回傳。

use std::path::Path;
use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use axum::Json;
use baton_domain::profile::{self, AccountProfile, ProfileCommand, ProfileError, ProfileStatus, ProfileView};
use baton_ports::{DirectoryState, ToolProbe};
use baton_store::{ProfileApplyError, StoreError};
use serde::{Deserialize, Serialize};

use crate::workspaces::{new_id, ApiError};
use crate::ApiState;

/// Profile 集合路徑：GET 列出、POST 建立。
pub const PROFILES_PATH: &str = "/api/v1/profiles";
/// 以既有目錄建立 Profile 的路徑（POST）。
pub const PROFILES_FROM_EXISTING_DIR_PATH: &str = "/api/v1/profiles/from-existing-dir";
/// 調整帳號顯示順序的路徑（PUT）。
pub const PROFILES_ORDER_PATH: &str = "/api/v1/profiles/order";
/// 核對登入狀態路徑樣板（`{id}` 是 Profile 識別）。
pub(crate) const VERIFY_ROUTE: &str = "/api/v1/profiles/{id}/verify";
pub(crate) const LOGIN_TERMINAL_ROUTE: &str = "/api/v1/profiles/{id}/login-terminal";
pub(crate) const ARCHIVE_ROUTE: &str = "/api/v1/profiles/{id}/archive";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateProfileRequest {
    pub name: String,
    /// 預期的帳號（電子郵件）；省略時以第一次核對到的原生登入帳號為準。
    #[serde(default)]
    pub expected_account: Option<String>,
}

/// 以使用者既有的 Claude Code 設定目錄（例如 `~/.claude-Work-3`）建立 Profile；平台不建立、不刪除、不修改這個目錄。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateProfileFromDirRequest {
    pub name: String,
    pub config_dir: String,
}

/// 調整帳號的顯示順序：依新的順序列出每個使用中的 Profile 恰好一次（只影響顯示，不影響派工）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReorderProfilesRequest {
    pub profile_ids: Vec<String>,
}

/// 列出端點的回應：全部 Profile（含已封存）與可派工判定，依使用者調整的顯示順序（新增的在最後）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileList {
    pub profiles: Vec<ProfileView>,
    /// 同時使用中的 Profile 數量上限。
    pub limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoginTerminalResponse {
    pub opened: bool,
}

impl From<ProfileError> for ApiError {
    fn from(error: ProfileError) -> Self {
        let status = match &error {
            ProfileError::NameEmpty
            | ProfileError::NameTooLong
            | ProfileError::ExpectedAccountInvalid(_)
            | ProfileError::ConfigDirInvalid(_)
            | ProfileError::ConfigDirManaged(_) => StatusCode::BAD_REQUEST,
            ProfileError::NotFound(_) => StatusCode::NOT_FOUND,
            ProfileError::NameTaken(_)
            | ProfileError::LimitReached
            | ProfileError::ConfigDirShared(_)
            | ProfileError::AlreadyArchived(_)
            | ProfileError::OrderMismatch => StatusCode::CONFLICT,
        };
        ApiError { status, code: error.code(), message: error.to_string() }
    }
}

impl From<ProfileApplyError> for ApiError {
    fn from(error: ProfileApplyError) -> Self {
        match error {
            ProfileApplyError::Rejected(rejected) => rejected.into(),
            ProfileApplyError::Store(store) => store.into(),
        }
    }
}

pub(crate) async fn list(State(state): State<Arc<ApiState>>) -> Result<Json<ProfileList>, ApiError> {
    Ok(Json(display_list(&state).await?))
}

pub(crate) async fn reorder(
    State(state): State<Arc<ApiState>>,
    body: Result<Json<ReorderProfilesRequest>, JsonRejection>,
) -> Result<Json<ProfileList>, ApiError> {
    let Json(request) = body.map_err(|rejection| ApiError {
        status: StatusCode::BAD_REQUEST,
        code: "bad_request",
        message: format!("請求內容不是有效的 JSON，或缺少必要欄位 profile_ids：{}", rejection.body_text()),
    })?;
    state.store.reorder_profiles(request.profile_ids).await?;
    Ok(Json(display_list(&state).await?))
}

/// 依顯示順序排列的 Profile 清單；可派工判定（重複帳號只算較早建立的）仍以建立順序計算。
async fn display_list(state: &ApiState) -> Result<ProfileList, StoreError> {
    let mut views = profile::views(&state.store.list_profiles().await?);
    let order = state.store.profile_display_order().await?;
    views.sort_by_key(|v| order.iter().position(|id| *id == v.profile.id));
    Ok(ProfileList { profiles: views, limit: profile::MAX_PROFILES })
}

pub(crate) async fn create(
    State(state): State<Arc<ApiState>>,
    body: Result<Json<CreateProfileRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<ProfileView>), ApiError> {
    let Json(request) = body.map_err(|rejection| ApiError {
        status: StatusCode::BAD_REQUEST,
        code: "bad_request",
        message: format!("請求內容不是有效的 JSON，或缺少必要欄位 name：{}", rejection.body_text()),
    })?;

    // 每個 Profile 一個獨立的設定目錄：資料目錄之下、以隨機識別命名，不會與任何其他 Profile 重疊
    let id = new_id("prof")?;
    let config_dir = state.data_dir.join("profiles").join(&id);
    let command = ProfileCommand::Create {
        id: id.clone(),
        name: request.name,
        config_dir: config_dir.to_string_lossy().into_owned(),
        expected_account: request.expected_account,
    };
    let now = state.clock.now();

    // 先以目前狀態預檢：被拒絕（例如已滿 8 個）時不建立任何目錄
    profile::decide(&state.store.list_profiles().await?, command.clone(), now)?;
    state.workspace_fs.ensure_directory(&config_dir).await.map_err(|error| ApiError {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        code: "internal_error",
        message: format!("無法建立設定目錄 {}：{error}", config_dir.display()),
    })?;
    state.store.apply_profile_command(command, now).await?;
    Ok((StatusCode::CREATED, Json(view_of(&state, &id).await?)))
}

pub(crate) async fn create_from_existing_dir(
    State(state): State<Arc<ApiState>>,
    body: Result<Json<CreateProfileFromDirRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<ProfileView>), ApiError> {
    let Json(request) = body.map_err(|rejection| ApiError {
        status: StatusCode::BAD_REQUEST,
        code: "bad_request",
        message: format!("請求內容不是有效的 JSON，或缺少必要欄位 name、config_dir：{}", rejection.body_text()),
    })?;

    let id = new_id("prof")?;
    let command = ProfileCommand::CreateFromExistingDir {
        id: id.clone(),
        name: request.name,
        config_dir: request.config_dir,
        managed_root: state.data_dir.join("profiles").to_string_lossy().into_owned(),
    };
    let now = state.clock.now();

    // 先由 Domain 檢查名稱、上限、路徑形式與重疊，才去問檔案系統；目錄由使用者準備，平台不建立
    let checked = profile::decide(&state.store.list_profiles().await?, command.clone(), now)?.profile;
    let config_dir = checked.config_dir;
    match state.workspace_fs.inspect_directory(Path::new(&config_dir)).await {
        Ok(DirectoryState::Directory) => {}
        Ok(DirectoryState::NotFound) => {
            return Err(ApiError {
                status: StatusCode::BAD_REQUEST,
                code: "profile_config_dir_not_found",
                message: format!("找不到設定目錄：{config_dir}"),
            })
        }
        Ok(DirectoryState::NotADirectory) => {
            return Err(ApiError {
                status: StatusCode::BAD_REQUEST,
                code: "profile_config_dir_not_directory",
                message: format!("這個路徑不是目錄：{config_dir}"),
            })
        }
        Err(error) => {
            return Err(ApiError {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                code: "internal_error",
                message: format!("無法檢查設定目錄 {config_dir}：{error}"),
            })
        }
    }
    let created = state.store.apply_profile_command(command, now).await?;

    // 立即核對登入身分；核對本身失敗時 Profile 仍已建立，顯示為尚未核對，使用者可再按「重新核對」
    let _ = verify_profile(&state, &created).await;
    Ok((StatusCode::CREATED, Json(view_of(&state, &id).await?)))
}

pub(crate) async fn verify(
    State(state): State<Arc<ApiState>>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<ProfileView>, ApiError> {
    let profile = active_profile(&state, &id).await?;
    verify_profile(&state, &profile).await?;
    Ok(Json(view_of(&state, &id).await?))
}

/// 核對一個 Profile 並記錄結果。核對本身失敗（例如 Claude Code 沒有回應）時回報錯誤，不改變已記錄的結果。
pub(crate) async fn verify_profile(state: &ApiState, profile: &AccountProfile) -> Result<(), ApiError> {
    let report = state.claude_env.auth_status(Path::new(&profile.config_dir)).await.map_err(|error| ApiError {
        status: StatusCode::BAD_GATEWAY,
        code: "profile_check_failed",
        message: format!("無法核對 Profile「{}」的登入狀態：{error}", profile.name),
    })?;
    // 同時記下 Claude Code 的版本；取不到時記為未知，由 Domain 阻擋派工（票 14）
    let cli_version = match state.claude_env.probe_claude().await {
        Ok(ToolProbe::Found { version_output }) => version_output,
        Ok(ToolProbe::NotFound) | Err(_) => None,
    };
    let command = ProfileCommand::RecordVerification { id: profile.id.clone(), report, cli_version };
    state.store.apply_profile_command(command, state.clock.now()).await?;
    Ok(())
}

pub(crate) async fn login_terminal(
    State(state): State<Arc<ApiState>>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<LoginTerminalResponse>, ApiError> {
    let profile = active_profile(&state, &id).await?;
    state.claude_env.open_login_terminal(Path::new(&profile.config_dir)).await.map_err(|error| ApiError {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        code: "login_terminal_failed",
        message: format!("無法開啟登入終端機：{error}"),
    })?;
    Ok(Json(LoginTerminalResponse { opened: true }))
}

pub(crate) async fn archive(
    State(state): State<Arc<ApiState>>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<ProfileView>, ApiError> {
    state.store.apply_profile_command(ProfileCommand::Archive { id: id.clone() }, state.clock.now()).await?;
    Ok(Json(view_of(&state, &id).await?))
}

/// 使用中的 Profile；找不到或已封存都以 Domain 的錯誤回報。
pub(crate) async fn active_profile(state: &ApiState, id: &str) -> Result<AccountProfile, ApiError> {
    let found = state.store.list_profiles().await?.into_iter().find(|p| p.id == id);
    match found {
        None => Err(ProfileError::NotFound(id.to_string()).into()),
        Some(p) if p.status == ProfileStatus::Archived => Err(ProfileError::AlreadyArchived(id.to_string()).into()),
        Some(p) => Ok(p),
    }
}

/// 目前的 Profile 呈現（可派工判定要看全部 Profile，所以每次重新計算）。
async fn view_of(state: &ApiState, id: &str) -> Result<ProfileView, StoreError> {
    let profiles = state.store.list_profiles().await?;
    Ok(profile::views(&profiles).into_iter().find(|v| v.profile.id == id).expect("剛寫入的 Profile 一定存在"))
}
