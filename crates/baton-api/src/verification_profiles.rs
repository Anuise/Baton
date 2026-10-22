//! Verification Profile 與 CLI／API 驗證的端點（票 45，spec D-29）。
//!
//! - 管理 API（桌面憑證）：Workspace 的驗證概況（最新 Profile、是否已被證明可用、未覆蓋範圍、最近的要求與
//!   Evidence Bundle）、發布 Profile 新版本、要求執行驗證、讀取一份 Evidence Bundle。Profile 決定「怎麼驗收」，
//!   所以只有使用者能發布（spec D-15：修改驗收標準必須由使用者決定）。
//! - Controller 工具（只接受 Controller 憑證）：`run_verification` 要求執行驗證，`get_verification` 讀取概況；
//!   Evidence Bundle 的識別可作為 Stage exit check 的 `verification` 證據。
//! - 驗證本身由 Supervisor 執行（Runtime 的 CLI／API Adapter），請求只建立要求並喚醒 Supervisor。

use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use baton_domain::auth::CredentialRecord;
use baton_domain::verification_profile::{
    self, EvidenceBundle, ProfileDefinition, ProfileError, ProfileVersion, UncoveredItem, VerificationRequest,
};
use baton_store::{VerificationApplyError, VerificationOverview};
use serde::{Deserialize, Serialize};

use crate::agent::Claims;
use crate::authorization::bad_json;
use crate::controller::controller_scope;
use crate::workspaces::{new_id, ApiError};
use crate::ApiState;

pub(crate) const VERIFICATION_ROUTE: &str = "/api/v1/workspaces/{id}/verification";
pub(crate) const PROFILE_ROUTE: &str = "/api/v1/workspaces/{id}/verification/profile";
pub(crate) const RUNS_ROUTE: &str = "/api/v1/workspaces/{id}/verification/runs";
pub(crate) const BUNDLE_ROUTE: &str = "/api/v1/workspaces/{id}/verification/bundles/{bundle_id}";

/// 發布者紀錄使用的憑證指紋長度（憑證雜湊的前幾個十六進位字元）。
const FINGERPRINT_CHARS: usize = 16;

pub fn workspace_verification_path(workspace_id: &str) -> String {
    format!("/api/v1/workspaces/{workspace_id}/verification")
}

pub fn verification_profile_path(workspace_id: &str) -> String {
    format!("{}/profile", workspace_verification_path(workspace_id))
}

pub fn verification_runs_path(workspace_id: &str) -> String {
    format!("{}/runs", workspace_verification_path(workspace_id))
}

pub fn evidence_bundle_path(workspace_id: &str, bundle_id: &str) -> String {
    format!("{}/bundles/{bundle_id}", workspace_verification_path(workspace_id))
}

/// Workspace 的驗證概況。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerificationOverviewView {
    pub workspace_id: String,
    /// 最新的 Profile 版本；還沒有 Profile 時為 `None`。
    pub profile: Option<ProfileVersion>,
    /// 證明最新版本可用的 Evidence Bundle；`None` 表示這個版本還沒有實際跑過並通過代表旅程。
    pub proven_by: Option<String>,
    /// 最新版本未覆蓋的範圍（沒有旅程的功能與明確宣告的範圍）。
    pub uncovered: Vec<UncoveredItem>,
    /// 最近的驗證要求（新的在前）。
    pub requests: Vec<VerificationRequest>,
    /// 最近的 Evidence Bundle（新的在前）。
    pub bundles: Vec<EvidenceBundle>,
}

/// 發布 Profile 的結果：`created` 為 false 表示內容與最新版本相同，沒有新增版本。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishProfileResponse {
    pub profile: ProfileVersion,
    pub created: bool,
}

/// 要求執行驗證：`task_id` 指定受驗的 Task（在它的工作目錄中驗證並綁定它的契約版本）；
/// `journeys` 空的時候執行全部旅程。Profile 版本還沒被證明可用時一定包含代表旅程。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunVerificationRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(default)]
    pub journeys: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct RunVerificationToolRequest {
    #[serde(flatten)]
    request: RunVerificationRequest,
    #[serde(flatten)]
    claims: Claims,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct GetVerificationRequest {
    #[serde(flatten)]
    claims: Claims,
}

impl From<ProfileError> for ApiError {
    fn from(error: ProfileError) -> Self {
        let status = match error {
            ProfileError::Invalid(_) | ProfileError::RequestInvalid(_) => StatusCode::UNPROCESSABLE_ENTITY,
            ProfileError::NotFound | ProfileError::BundleNotFound(_) => StatusCode::NOT_FOUND,
        };
        ApiError { status, code: error.code(), message: error.to_string() }
    }
}

impl From<VerificationApplyError> for ApiError {
    fn from(error: VerificationApplyError) -> Self {
        let message = error.to_string();
        match error {
            VerificationApplyError::Rejected(rejected) => rejected.into(),
            VerificationApplyError::WorkspaceNotFound(_) => {
                ApiError { status: StatusCode::NOT_FOUND, code: "workspace_not_found", message }
            }
            VerificationApplyError::WorkspaceArchived(_) => {
                ApiError { status: StatusCode::CONFLICT, code: "workspace_archived", message }
            }
            VerificationApplyError::TaskNotFound(_) => ApiError { status: StatusCode::NOT_FOUND, code: "task_not_found", message },
            VerificationApplyError::Store(store) => store.into(),
        }
    }
}

async fn require_workspace(state: &ApiState, id: &str) -> Result<(), ApiError> {
    if state.store.list_workspaces().await?.iter().any(|w| w.id == id) {
        return Ok(());
    }
    Err(ApiError { status: StatusCode::NOT_FOUND, code: "workspace_not_found", message: format!("找不到 Workspace {id}") })
}

fn overview_view(workspace_id: String, overview: VerificationOverview) -> VerificationOverviewView {
    VerificationOverviewView {
        uncovered: overview.profile.as_ref().map(|p| verification_profile::uncovered(&p.definition)).unwrap_or_default(),
        workspace_id,
        profile: overview.profile,
        proven_by: overview.proven_by,
        requests: overview.requests,
        bundles: overview.bundles,
    }
}

async fn request_run(
    state: &ApiState,
    workspace_id: &str,
    request: RunVerificationRequest,
    requested_by: String,
) -> Result<VerificationRequest, ApiError> {
    let task_id = request.task_id.map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
    let journeys = request.journeys.into_iter().map(|j| j.trim().to_string()).filter(|j| !j.is_empty()).collect();
    let created = state
        .store
        .request_verification(workspace_id, task_id, journeys, requested_by, new_id("vreq")?, state.clock.now())
        .await?;
    state.supervisor_wake.notify_one();
    Ok(created)
}

// ---------------------------------------------------------------- 管理 API

pub(crate) async fn overview(
    State(state): State<Arc<ApiState>>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<VerificationOverviewView>, ApiError> {
    require_workspace(&state, &id).await?;
    let overview = state.store.verification_overview(&id).await?;
    Ok(Json(overview_view(id, overview)))
}

/// 發布 Profile 的新版本：驗證後追加，不覆寫舊版本；新版本要實際跑過代表旅程才算可用。
pub(crate) async fn publish(
    State(state): State<Arc<ApiState>>,
    Extension(record): Extension<CredentialRecord>,
    UrlPath(id): UrlPath<String>,
    body: Result<Json<ProfileDefinition>, JsonRejection>,
) -> Result<(StatusCode, Json<PublishProfileResponse>), ApiError> {
    let Json(definition) = body.map_err(bad_json)?;
    let published_by = record.token_hash.chars().take(FINGERPRINT_CHARS).collect();
    let (profile, created) = state.store.publish_verification_profile(&id, definition, published_by, state.clock.now()).await?;
    let status = if created { StatusCode::CREATED } else { StatusCode::OK };
    Ok((status, Json(PublishProfileResponse { profile, created })))
}

pub(crate) async fn run(
    State(state): State<Arc<ApiState>>,
    UrlPath(id): UrlPath<String>,
    body: Result<Json<RunVerificationRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<VerificationRequest>), ApiError> {
    let Json(request) = body.map_err(bad_json)?;
    Ok((StatusCode::CREATED, Json(request_run(&state, &id, request, "desktop".into()).await?)))
}

pub(crate) async fn bundle(
    State(state): State<Arc<ApiState>>,
    UrlPath((id, bundle_id)): UrlPath<(String, String)>,
) -> Result<Json<EvidenceBundle>, ApiError> {
    match state.store.evidence_bundle(&id, &bundle_id).await? {
        Some(bundle) => Ok(Json(bundle)),
        None => Err(ProfileError::BundleNotFound(bundle_id).into()),
    }
}

// ---------------------------------------------------------------- Controller 工具

pub(crate) async fn run_verification(
    State(state): State<Arc<ApiState>>,
    Extension(record): Extension<CredentialRecord>,
    body: Result<Json<RunVerificationToolRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<VerificationRequest>), ApiError> {
    let Json(request) = body.map_err(bad_json)?;
    let scope = controller_scope(&state, &record, &request.claims).await?;
    let created = request_run(&state, &scope.workspace_id, request.request, scope.controller_run_id.clone()).await?;
    Ok((StatusCode::CREATED, Json(created)))
}

pub(crate) async fn get_verification(
    State(state): State<Arc<ApiState>>,
    Extension(record): Extension<CredentialRecord>,
    body: Result<Json<GetVerificationRequest>, JsonRejection>,
) -> Result<Json<VerificationOverviewView>, ApiError> {
    let Json(request) = body.map_err(bad_json)?;
    let scope = controller_scope(&state, &record, &request.claims).await?;
    let overview = state.store.verification_overview(&scope.workspace_id).await?;
    Ok(Json(overview_view(scope.workspace_id, overview)))
}
