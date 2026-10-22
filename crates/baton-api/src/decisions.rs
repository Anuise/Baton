//! 裁定端點（票 22）：桌面角色讀取待處理清單與單筆詳情、提交正式裁定。
//!
//! 只有桌面憑證能提交裁定：這些路徑都在 `/api/v1/` 之下，Agent 憑證一律在 `guard` 就被擋下（403 `role_not_allowed`）；
//! Agent 只能經 Agent API 提出請求（`request_decision`、`permission_prompt`）。規則都在 Domain（`baton_domain::decision`），
//! 端點只做「解析請求 → 交給 Store → 對應成穩定的錯誤代碼」。

use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use axum::Json;
use baton_domain::decision::{DecisionError, DecisionView};
use baton_store::{AnswerResult, DecisionApplyError, DecisionList, PendingByWorkspace};
use serde::{Deserialize, Serialize};

use crate::authorization::bad_json;
use crate::workspaces::ApiError;
use crate::ApiState;

pub(crate) const DECISIONS_ROUTE: &str = "/api/v1/workspaces/{id}/decisions";
pub(crate) const DECISION_ROUTE: &str = "/api/v1/workspaces/{id}/decisions/{request_id}";
pub(crate) const ANSWER_ROUTE: &str = "/api/v1/workspaces/{id}/decisions/{request_id}/answer";
/// 全域待處理裁定數（狀態列）。
pub const PENDING_DECISIONS_PATH: &str = "/api/v1/decisions/pending";

/// 路徑；Runtime 與用戶端共用。
pub fn decisions_path(workspace_id: &str) -> String {
    format!("{}/{workspace_id}/decisions", crate::WORKSPACES_PATH)
}

pub fn decision_path(workspace_id: &str, request_id: &str) -> String {
    format!("{}/{request_id}", decisions_path(workspace_id))
}

pub fn decision_answer_path(workspace_id: &str, request_id: &str) -> String {
    format!("{}/answer", decision_path(workspace_id, request_id))
}

/// 回答請求的內容。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnswerRequest {
    pub option_id: String,
    #[serde(default)]
    pub note: Option<String>,
}

/// 全域待處理裁定數。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingDecisions {
    pub pending_count: usize,
    pub by_workspace: Vec<PendingByWorkspace>,
}

impl From<DecisionApplyError> for ApiError {
    fn from(error: DecisionApplyError) -> Self {
        match error {
            DecisionApplyError::Rejected(rejected) => {
                let status = match &rejected {
                    DecisionError::WorkspaceNotFound(_)
                    | DecisionError::RunNotFound(_)
                    | DecisionError::TaskNotFound(_)
                    | DecisionError::NotFound(_) => StatusCode::NOT_FOUND,
                    DecisionError::Invalid(_) | DecisionError::OptionUnknown(_) => StatusCode::BAD_REQUEST,
                    DecisionError::StaleRun | DecisionError::AlreadyAnswered(_) => StatusCode::CONFLICT,
                    DecisionError::NotDesktop => StatusCode::FORBIDDEN,
                };
                ApiError { status, code: rejected.code(), message: rejected.to_string() }
            }
            DecisionApplyError::Store(store) => store.into(),
        }
    }
}

pub(crate) async fn list(
    State(state): State<Arc<ApiState>>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<DecisionList>, ApiError> {
    Ok(Json(state.store.decision_list(&id, state.clock.now()).await?))
}

pub(crate) async fn detail(
    State(state): State<Arc<ApiState>>,
    UrlPath((id, request_id)): UrlPath<(String, String)>,
) -> Result<Json<DecisionView>, ApiError> {
    Ok(Json(state.store.decision_detail(&id, &request_id, state.clock.now()).await?))
}

/// 提交正式裁定：裁定、請求狀態、受影響 Task 的恢復與喚醒 Outbox 在同一個交易提交。
/// 同樣的答案重送是冪等的（200）；不同的答案回 409 `decision_already_answered`。
pub(crate) async fn answer(
    State(state): State<Arc<ApiState>>,
    UrlPath((id, request_id)): UrlPath<(String, String)>,
    body: Result<Json<AnswerRequest>, JsonRejection>,
) -> Result<Json<AnswerResult>, ApiError> {
    let Json(request) = body.map_err(bad_json)?;
    let result = state.store.answer_decision(&id, &request_id, request.option_id, request.note, state.clock.now()).await?;
    Ok(Json(result))
}

pub(crate) async fn pending(State(state): State<Arc<ApiState>>) -> Result<Json<PendingDecisions>, ApiError> {
    let by_workspace = state.store.pending_decisions_by_workspace().await?;
    Ok(Json(PendingDecisions { pending_count: by_workspace.iter().map(|w| w.pending_count).sum(), by_workspace }))
}
