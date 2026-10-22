//! Workspace 授權端點（桌面角色專用）：讀取目前授權與範本、確認前的摘要、確認並保存、Run 准入檢查。
//!
//! 規則都在 Domain（`baton_domain::authorization`）；端點只做「解析請求 → 交給 Store → 對應成穩定的錯誤代碼」。

use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use axum::Json;
use baton_domain::authorization::{
    self, Admission, AdmissionDenied, AuthorizationError, AuthorizationRevision, Capability, ConfirmOutcome,
    ConfirmRequest, ConfirmationSummary, Grant, Template,
};
use baton_store::{AdmissionCheckError, AuthorizationApplyError};
use serde::{Deserialize, Serialize};

use crate::workspaces::ApiError;
use crate::ApiState;

/// 授權路徑樣板（`{id}` 是 Workspace 識別）：GET 讀取、PUT 確認並保存。
pub(crate) const AUTHORIZATION_ROUTE: &str = "/api/v1/workspaces/{id}/authorization";
/// 確認前的摘要（預覽，不保存任何東西）。
pub(crate) const SUMMARY_ROUTE: &str = "/api/v1/workspaces/{id}/authorization/summary";
/// Run 的准入檢查。
pub(crate) const ADMISSION_ROUTE: &str = "/api/v1/workspaces/{id}/admission-check";

/// 授權路徑；Runtime 與用戶端共用。
pub fn authorization_path(workspace_id: &str) -> String {
    format!("{}/{workspace_id}/authorization", crate::WORKSPACES_PATH)
}

pub fn authorization_summary_path(workspace_id: &str) -> String {
    format!("{}/summary", authorization_path(workspace_id))
}

pub fn admission_check_path(workspace_id: &str) -> String {
    format!("{}/{workspace_id}/admission-check", crate::WORKSPACES_PATH)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorizationStatus {
    /// 還沒有確認過任何授權；需要能力的 Run 不能被准入。
    Unconfirmed,
    Confirmed,
}

/// 能力清單中的一項，供編輯畫面列出可選的能力。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityInfo {
    pub capability: Capability,
    pub label: String,
    pub high_risk: bool,
}

/// 授權設定畫面需要的全部資料：目前生效的 revision、歷史、建議範本與能力清單。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorizationView {
    pub workspace_id: String,
    pub status: AuthorizationStatus,
    /// 目前生效的 revision（最新的一個）；尚未確認時為 `null`。
    pub effective: Option<AuthorizationRevision>,
    /// 全部 revision，由舊到新。
    pub revisions: Vec<AuthorizationRevision>,
    /// 目標資源已代入這個 Workspace 目錄的建議範本；沒有任何範本會被自動套用。
    pub templates: Vec<Template>,
    pub capabilities: Vec<CapabilityInfo>,
    /// 可信程式以目前 Windows 使用者身分執行的風險說明。
    pub risk_disclosure: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfirmAuthorizationRequest {
    /// 以哪個範本為起點；省略表示自行組成。
    #[serde(default)]
    pub template_id: Option<String>,
    pub grants: Vec<Grant>,
    /// 使用者已看過確認摘要並承認其中的風險。
    #[serde(default)]
    pub acknowledged_risk: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SummaryRequest {
    pub grants: Vec<Grant>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdmissionCheckRequest {
    pub required_capabilities: Vec<Capability>,
}

impl From<AuthorizationError> for ApiError {
    fn from(error: AuthorizationError) -> Self {
        let status = match &error {
            AuthorizationError::WorkspaceNotFound(_) => StatusCode::NOT_FOUND,
            AuthorizationError::WorkspaceArchived(_) => StatusCode::CONFLICT,
            AuthorizationError::UnknownTemplate(_)
            | AuthorizationError::DuplicateCapability(_)
            | AuthorizationError::MissingTarget(_)
            | AuthorizationError::RiskNotAcknowledged => StatusCode::BAD_REQUEST,
        };
        ApiError { status, code: error.code(), message: error.to_string() }
    }
}

impl From<AuthorizationApplyError> for ApiError {
    fn from(error: AuthorizationApplyError) -> Self {
        match error {
            AuthorizationApplyError::Rejected(rejected) => rejected.into(),
            AuthorizationApplyError::Store(store) => store.into(),
        }
    }
}

impl From<AdmissionDenied> for ApiError {
    fn from(denied: AdmissionDenied) -> Self {
        let status = match &denied {
            AdmissionDenied::WorkspaceNotFound(_) => StatusCode::NOT_FOUND,
            _ => StatusCode::CONFLICT,
        };
        ApiError { status, code: denied.code(), message: denied.to_string() }
    }
}

impl From<AdmissionCheckError> for ApiError {
    fn from(error: AdmissionCheckError) -> Self {
        match error {
            AdmissionCheckError::Denied(denied) => denied.into(),
            AdmissionCheckError::Store(store) => store.into(),
        }
    }
}

pub(crate) fn bad_json(rejection: JsonRejection) -> ApiError {
    ApiError {
        status: StatusCode::BAD_REQUEST,
        code: "bad_request",
        message: format!("請求內容不是有效的 JSON，或欄位不正確：{}", rejection.body_text()),
    }
}

/// 取得 Workspace；不存在時回報 `workspace_not_found`。
async fn find_workspace(state: &ApiState, id: &str) -> Result<baton_domain::workspace::Workspace, ApiError> {
    state
        .store
        .list_workspaces()
        .await?
        .into_iter()
        .find(|w| w.id == id)
        .ok_or_else(|| AuthorizationError::WorkspaceNotFound(id.to_string()).into())
}

pub(crate) async fn view(
    State(state): State<Arc<ApiState>>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<AuthorizationView>, ApiError> {
    let workspace = find_workspace(&state, &id).await?;
    let revisions = state.store.authorization_revisions(&id).await?;
    let effective = revisions.last().cloned();
    Ok(Json(AuthorizationView {
        workspace_id: id,
        status: if effective.is_some() { AuthorizationStatus::Confirmed } else { AuthorizationStatus::Unconfirmed },
        effective,
        revisions,
        templates: authorization::templates(&workspace.path),
        capabilities: Capability::ALL
            .into_iter()
            .map(|c| CapabilityInfo { capability: c, label: c.label().to_string(), high_risk: c.is_high_risk() })
            .collect(),
        risk_disclosure: authorization::TRUSTED_EXECUTION_RISK.to_string(),
    }))
}

pub(crate) async fn summary(
    State(state): State<Arc<ApiState>>,
    UrlPath(id): UrlPath<String>,
    body: Result<Json<SummaryRequest>, JsonRejection>,
) -> Result<Json<ConfirmationSummary>, ApiError> {
    let Json(request) = body.map_err(bad_json)?;
    find_workspace(&state, &id).await?;
    Ok(Json(authorization::summarize(request.grants)?))
}

/// 確認並保存：新內容回 201 與新的 revision；與目前生效的內容相同則回 200 與既有 revision。
pub(crate) async fn confirm(
    State(state): State<Arc<ApiState>>,
    UrlPath(id): UrlPath<String>,
    body: Result<Json<ConfirmAuthorizationRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<AuthorizationRevision>), ApiError> {
    let Json(request) = body.map_err(bad_json)?;
    let request = ConfirmRequest {
        template_id: request.template_id,
        grants: request.grants,
        acknowledged_risk: request.acknowledged_risk,
    };
    Ok(match state.store.confirm_authorization(id, request, state.clock.now()).await? {
        ConfirmOutcome::Created { revision, .. } => (StatusCode::CREATED, Json(revision)),
        ConfirmOutcome::Unchanged(revision) => (StatusCode::OK, Json(revision)),
    })
}

pub(crate) async fn admission_check(
    State(state): State<Arc<ApiState>>,
    UrlPath(id): UrlPath<String>,
    body: Result<Json<AdmissionCheckRequest>, JsonRejection>,
) -> Result<Json<Admission>, ApiError> {
    let Json(request) = body.map_err(bad_json)?;
    Ok(Json(state.store.check_admission(&id, request.required_capabilities).await?))
}
