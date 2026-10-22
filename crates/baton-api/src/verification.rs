//! 成果接受與驗證的端點（桌面角色專用，票 13）：檢視一個 Task 的驗證狀況、使用者手動接受成果。
//!
//! 規則都在 Domain（`baton_domain::verification`）；端點只做「解析請求 → 交給 Store → 對應成穩定的錯誤代碼」。
//! 手動接受的接受者取自 guard 已驗證的桌面憑證（只記錄指紋），請求不能指定；Agent 憑證在 guard 就被拒絕。

use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use baton_domain::auth::CredentialRecord;
use baton_domain::verification::{AcceptanceBasis, AcceptanceCommand, AcceptanceError, VerificationView};
use baton_store::AcceptanceApplyError;
use serde::{Deserialize, Serialize};

use crate::authorization::bad_json;
use crate::workspaces::{new_id, ApiError};
use crate::ApiState;

pub(crate) const VERIFICATION_ROUTE: &str = "/api/v1/workspaces/{id}/tasks/{task_id}/verification";
pub(crate) const ACCEPT_ROUTE: &str = "/api/v1/workspaces/{id}/tasks/{task_id}/verification/accept";

/// 接受者紀錄使用的憑證指紋長度（憑證雜湊的前幾個十六進位字元）。
const FINGERPRINT_CHARS: usize = 16;

/// 路徑；Runtime 與用戶端共用。
pub fn task_verification_path(workspace_id: &str, task_id: &str) -> String {
    format!("{}/verification", crate::tasks::task_path(workspace_id, task_id))
}

pub fn task_accept_path(workspace_id: &str, task_id: &str) -> String {
    format!("{}/accept", task_verification_path(workspace_id, task_id))
}

/// 手動接受的請求：`basis` 必須原樣送回檢視時拿到的接受基礎。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcceptTaskRequest {
    pub basis: AcceptanceBasis,
    #[serde(default)]
    pub note: Option<String>,
}

fn rejected(error: AcceptanceError) -> ApiError {
    let status = match &error {
        AcceptanceError::WorkspaceNotFound(_) | AcceptanceError::TaskNotFound(_) => StatusCode::NOT_FOUND,
        AcceptanceError::NoteTooLong => StatusCode::BAD_REQUEST,
        AcceptanceError::WorkspaceArchived(_)
        | AcceptanceError::TaskNotVerifying(_)
        | AcceptanceError::RunStale { .. }
        | AcceptanceError::ContractStale { .. }
        | AcceptanceError::ManifestStale
        | AcceptanceError::ResultsMismatch
        | AcceptanceError::AlreadyQueuedForIntegration
        | AcceptanceError::WorktreeMissing => StatusCode::CONFLICT,
    };
    ApiError { status, code: error.code(), message: error.to_string() }
}

impl From<AcceptanceApplyError> for ApiError {
    fn from(error: AcceptanceApplyError) -> Self {
        match error {
            AcceptanceApplyError::Rejected(error) => rejected(error),
            AcceptanceApplyError::Store(store) => store.into(),
        }
    }
}

pub(crate) async fn view(
    State(state): State<Arc<ApiState>>,
    UrlPath((id, task_id)): UrlPath<(String, String)>,
) -> Result<Json<VerificationView>, ApiError> {
    Ok(Json(state.store.task_verification(&id, &task_id).await?))
}

/// 使用者手動接受：成果接受交易同時校驗 Run 世代、契約版本與成果 manifest，通過後回傳最新的驗證狀況。
pub(crate) async fn accept(
    State(state): State<Arc<ApiState>>,
    Extension(record): Extension<CredentialRecord>,
    UrlPath((id, task_id)): UrlPath<(String, String)>,
    body: Result<Json<AcceptTaskRequest>, JsonRejection>,
) -> Result<Json<VerificationView>, ApiError> {
    let Json(request) = body.map_err(bad_json)?;
    let accepted_by = record.token_hash.chars().take(FINGERPRINT_CHARS).collect();
    let command =
        AcceptanceCommand::AcceptManually { task_id: task_id.clone(), basis: request.basis, accepted_by, note: request.note };
    state.store.apply_acceptance(id.clone(), command, new_id("ver")?, state.clock.now()).await?;
    Ok(Json(state.store.task_verification(&id, &task_id).await?))
}
