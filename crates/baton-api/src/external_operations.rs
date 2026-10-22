//! 外部操作的端點（票 26）：Worker 送出部署、發訊、付費這類操作前後的登記（Agent API），以及桌面查詢。
//!
//! 規則都在 Domain（`baton_domain::external_operation`）。登記的四種結果都回 200，Worker 必須依 `status` 決定下一步：
//! 只有 `proceed` 可以送出；`already_succeeded` 沿用回執、不要重送；`result_unknown` 與 `in_progress_elsewhere` 不要送出。

use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use baton_domain::auth::CredentialRecord;
use baton_domain::external_operation::{
    BeginOutcome, ExternalOperation, ExternalOperationError, OperationRequest, OperationResult, OperationStatus,
};
use baton_store::ExternalApplyError;
use serde::{Deserialize, Serialize};

use crate::agent::{scope_of, Claims};
use crate::authorization::bad_json;
use crate::workspaces::{new_id, ApiError};
use crate::ApiState;

/// 桌面查詢路徑樣板。
pub(crate) const EXTERNAL_OPERATIONS_ROUTE: &str = "/api/v1/workspaces/{id}/external-operations";

pub fn external_operations_path(workspace_id: &str) -> String {
    format!("/api/v1/workspaces/{workspace_id}/external-operations")
}

impl From<ExternalApplyError> for ApiError {
    fn from(error: ExternalApplyError) -> Self {
        match error {
            ExternalApplyError::Rejected(rejected) => {
                let status = match &rejected {
                    ExternalOperationError::Invalid(_) => StatusCode::BAD_REQUEST,
                    ExternalOperationError::WorkspaceNotFound(_)
                    | ExternalOperationError::RunNotFound(_)
                    | ExternalOperationError::TaskNotFound(_)
                    | ExternalOperationError::NotFound(_) => StatusCode::NOT_FOUND,
                    ExternalOperationError::StaleRun
                    | ExternalOperationError::KeyConflict(_)
                    | ExternalOperationError::NotInProgress(_)
                    | ExternalOperationError::AlreadyRecorded(_) => StatusCode::CONFLICT,
                };
                ApiError { status, code: rejected.code(), message: rejected.to_string() }
            }
            ExternalApplyError::Store(store) => store.into(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct BeginRequest {
    idempotency_key: String,
    operation: String,
    target: String,
    #[serde(default)]
    request: serde_json::Value,
    #[serde(flatten)]
    claims: Claims,
}

/// `begin_external_operation` 的回應。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BeginResponse {
    pub operation_id: String,
    /// `proceed`、`already_succeeded`、`result_unknown`、`in_progress_elsewhere`。
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<i64>,
    /// 同一個 Run 重送登記，沿用原本的紀錄。
    #[serde(default)]
    pub existing: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision_request_id: Option<String>,
    /// 正在進行同一個操作的另一個 Run。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    pub message: String,
}

pub(crate) async fn begin(
    State(state): State<Arc<ApiState>>,
    Extension(record): Extension<CredentialRecord>,
    body: Result<Json<BeginRequest>, JsonRejection>,
) -> Result<Json<BeginResponse>, ApiError> {
    let Json(request) = body.map_err(bad_json)?;
    let scope = scope_of(&record, &request.claims)?;
    let operation = OperationRequest::new(&request.idempotency_key, &request.operation, &request.target, request.request)
        .map_err(ExternalApplyError::from)?;
    let outcome = state.store.begin_external_operation(&scope, new_id("extop")?, operation, state.clock.now()).await?;
    let empty = |operation_id: String, status: &str, message: &str| BeginResponse {
        operation_id,
        status: status.to_string(),
        attempt: None,
        existing: false,
        receipt: None,
        decision_request_id: None,
        run_id: None,
        message: message.to_string(),
    };
    Ok(Json(match outcome {
        BeginOutcome::Proceed { operation_id, attempt, existing } => BeginResponse {
            attempt: Some(attempt),
            existing,
            ..empty(
                operation_id,
                "proceed",
                "已登記，可以送出這個操作。送出之後務必以 record_external_operation_result 回報結果；\
                 無法確定是否已經生效時回報 unknown，不要自行重送。",
            )
        },
        BeginOutcome::AlreadySucceeded { operation_id, receipt } => BeginResponse {
            receipt,
            ..empty(operation_id, "already_succeeded", "這個操作先前已經成功，不要再送一次；沿用回執繼續後面的步驟。")
        },
        BeginOutcome::ResultUnknown { operation_id, decision_request_id } => BeginResponse {
            decision_request_id,
            ..empty(
                operation_id,
                "result_unknown",
                "這個操作先前送出後結果未知，正在等使用者查證；不要送出。\
                 請先保存 checkpoint，再以 report_blocked 結束，或改做不需要這個操作的其他步驟。",
            )
        },
        BeginOutcome::InProgressElsewhere { operation_id, run_id } => BeginResponse {
            run_id: Some(run_id),
            ..empty(
                operation_id,
                "in_progress_elsewhere",
                "另一個 Run 正在進行同一個操作；不要送出，等它回報結果之後再以同一個冪等鍵登記一次。",
            )
        },
    }))
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ResultKind {
    Succeeded,
    Failed,
    Unknown,
}

#[derive(Debug, Deserialize)]
pub(crate) struct RecordRequest {
    idempotency_key: String,
    result: ResultKind,
    #[serde(default)]
    receipt: Option<String>,
    #[serde(default)]
    reason: Option<String>,
    #[serde(flatten)]
    claims: Claims,
}

/// `record_external_operation_result` 的回應。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordResponse {
    pub operation_id: String,
    pub status: OperationStatus,
    /// 結果未知時建立的裁定請求。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision_request_id: Option<String>,
    pub message: String,
}

pub(crate) async fn record_result(
    State(state): State<Arc<ApiState>>,
    Extension(record): Extension<CredentialRecord>,
    body: Result<Json<RecordRequest>, JsonRejection>,
) -> Result<Json<RecordResponse>, ApiError> {
    let Json(request) = body.map_err(bad_json)?;
    let scope = scope_of(&record, &request.claims)?;
    let text = |value: &Option<String>| value.clone().unwrap_or_default();
    let result = match request.result {
        ResultKind::Succeeded => OperationResult::succeeded(&text(&request.receipt)),
        ResultKind::Failed => OperationResult::failed(&text(&request.reason)),
        ResultKind::Unknown => OperationResult::unknown(&text(&request.reason)),
    }
    .map_err(ExternalApplyError::from)?;
    let (operation_id, status, decision_request_id) = state
        .store
        .record_external_operation_result(&scope, request.idempotency_key, result, state.clock.now())
        .await?;
    let message = match status {
        OperationStatus::Unknown => "已記為結果未知並建立裁定請求，等使用者查證；不要重送。\
                                     請先保存 checkpoint，再以 report_blocked 結束，或改做不需要這個操作的其他步驟。",
        _ => "已記錄結果。",
    };
    Ok(Json(RecordResponse { operation_id, status, decision_request_id, message: message.to_string() }))
}

/// 桌面查詢的回應：Workspace 的全部外部操作，依建立順序。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalOperationList {
    pub operations: Vec<ExternalOperation>,
}

pub(crate) async fn list(
    State(state): State<Arc<ApiState>>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<ExternalOperationList>, ApiError> {
    Ok(Json(ExternalOperationList { operations: state.store.external_operations(&id).await? }))
}
