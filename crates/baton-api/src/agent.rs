//! Agent API（票 11，spec D-05）：MCP shim 代 Worker 呼叫的工具端點，只接受 Agent 角色的憑證。
//!
//! 身分與資源歸屬一律來自憑證（Workspace、Run、世代），**不信任請求內填寫的 Workspace 或 Run**：
//! 請求可以（但不需要）帶 `workspace_id`／`run_id`，只要和憑證的範圍不一致就拒絕（403 `agent_scope_mismatch`），
//! 不會悄悄改用請求裡的值，也不會碰到別的 Workspace 或 Run 的資料。
//!
//! Run 結束或被接替之後，憑證就不再是寫入所有者：讀取（`get_task_contract`）被拒絕（401 `credential_revoked`）；
//! 回報（進度、checkpoint、受阻）被拒絕（409 `run_report_stale`）、不改變任何狀態，但內容保留在稽核紀錄。

use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::State;
use axum::http::StatusCode;
use axum::{Extension, Json};
use baton_domain::agent::{ReportContent, ReportError, ReportKind, AGENT_TOOLS_PREFIX};
use baton_domain::auth::{AgentScope, CredentialRecord};
use baton_domain::run::RunError;
use baton_domain::task::ContractContent;
use baton_store::RunApplyError;
use serde::{Deserialize, Serialize};

use crate::authorization::bad_json;
use crate::workspaces::ApiError;
use crate::ApiState;

/// 路徑；Runtime、MCP shim 與測試共用。
pub fn tool_path(tool: &str) -> String {
    format!("{AGENT_TOOLS_PREFIX}/{tool}")
}

/// 請求中「聲稱」的歸屬。Runtime 不採信它們，只用來偵測 Worker 想碰別人的資料。
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct Claims {
    #[serde(default)]
    workspace_id: Option<String>,
    #[serde(default)]
    run_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ProgressRequest {
    summary: String,
    #[serde(flatten)]
    claims: Claims,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CheckpointRequest {
    summary: String,
    #[serde(default)]
    details: String,
    #[serde(flatten)]
    claims: Claims,
}

#[derive(Debug, Deserialize)]
pub(crate) struct BlockedRequest {
    reason: String,
    #[serde(flatten)]
    claims: Claims,
}

/// `get_task_contract` 的回應：這個 Run 派工當下固定下來的契約版本。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskContractResponse {
    pub workspace_id: String,
    pub run_id: String,
    pub task_id: String,
    pub task_title: String,
    pub generation: i64,
    pub contract_version: i64,
    pub contract: ContractContent,
}

/// 回報被接受的回應。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportAck {
    pub accepted: bool,
    pub kind: ReportKind,
}

fn api_error(status: StatusCode, code: &'static str, message: &str) -> ApiError {
    ApiError { status, code, message: message.to_string() }
}

/// 憑證的範圍；請求聲稱的 Workspace 或 Run 與它不一致就拒絕。
fn scope_of(record: &CredentialRecord, claims: &Claims) -> Result<AgentScope, ApiError> {
    // guard 已經確保 Agent 憑證一定有範圍；沒有就當作無效，不退回任何預設
    let scope = record
        .scope
        .clone()
        .ok_or_else(|| api_error(StatusCode::UNAUTHORIZED, "credential_invalid", "憑證沒有範圍，無法使用 Agent API"))?;
    let foreign = |claimed: &Option<String>, actual: &str| claimed.as_deref().is_some_and(|c| c != actual);
    if foreign(&claims.workspace_id, &scope.workspace_id) || foreign(&claims.run_id, &scope.run_id) {
        return Err(api_error(
            StatusCode::FORBIDDEN,
            "agent_scope_mismatch",
            "請求指定的 Workspace 或 Run 不是這個憑證所屬的範圍，已拒絕",
        ));
    }
    Ok(scope)
}

impl From<ReportError> for ApiError {
    fn from(error: ReportError) -> Self {
        api_error(StatusCode::BAD_REQUEST, error.code(), &error.to_string())
    }
}

impl From<RunApplyError> for ApiError {
    fn from(error: RunApplyError) -> Self {
        match error {
            RunApplyError::Rejected(RunError::StaleReport) => api_error(
                StatusCode::CONFLICT,
                RunError::StaleReport.code(),
                "這份回報屬於已經不是目前寫入所有者的世代（Run 已結束或已被接替）：內容已保留為稽核紀錄，沒有改變任何狀態",
            ),
            RunApplyError::Rejected(rejected) => {
                let status = match rejected {
                    RunError::RunNotFound(_) | RunError::TaskNotFound(_) | RunError::WorkspaceNotFound(_) => {
                        StatusCode::NOT_FOUND
                    }
                    _ => StatusCode::CONFLICT,
                };
                api_error(status, rejected.code(), &rejected.to_string())
            }
            RunApplyError::Store(store) => store.into(),
        }
    }
}

pub(crate) async fn get_task_contract(
    State(state): State<Arc<ApiState>>,
    Extension(record): Extension<CredentialRecord>,
    body: Result<Json<Claims>, JsonRejection>,
) -> Result<Json<TaskContractResponse>, ApiError> {
    let Json(claims) = body.map_err(bad_json)?;
    let scope = scope_of(&record, &claims)?;
    let context = state.store.agent_context(&scope).await?;
    if !context.is_current {
        return Err(api_error(
            StatusCode::UNAUTHORIZED,
            "credential_revoked",
            "這個 Run 已經結束或已被接替，憑證已失效",
        ));
    }
    Ok(Json(TaskContractResponse {
        workspace_id: scope.workspace_id,
        run_id: scope.run_id,
        task_id: context.task.id,
        task_title: context.task.title,
        generation: scope.generation,
        contract_version: context.contract.version,
        contract: context.contract.content,
    }))
}

async fn report(
    state: &ApiState,
    scope: AgentScope,
    content: Result<ReportContent, ReportError>,
) -> Result<Json<ReportAck>, ApiError> {
    let content = content?;
    let kind = content.kind();
    state.store.apply_agent_report(&scope, content, state.clock.now()).await?;
    Ok(Json(ReportAck { accepted: true, kind }))
}

pub(crate) async fn report_progress(
    State(state): State<Arc<ApiState>>,
    Extension(record): Extension<CredentialRecord>,
    body: Result<Json<ProgressRequest>, JsonRejection>,
) -> Result<Json<ReportAck>, ApiError> {
    let Json(request) = body.map_err(bad_json)?;
    let scope = scope_of(&record, &request.claims)?;
    report(&state, scope, ReportContent::progress(&request.summary)).await
}

pub(crate) async fn save_checkpoint(
    State(state): State<Arc<ApiState>>,
    Extension(record): Extension<CredentialRecord>,
    body: Result<Json<CheckpointRequest>, JsonRejection>,
) -> Result<Json<ReportAck>, ApiError> {
    let Json(request) = body.map_err(bad_json)?;
    let scope = scope_of(&record, &request.claims)?;
    report(&state, scope, ReportContent::checkpoint(&request.summary, &request.details)).await
}

pub(crate) async fn report_blocked(
    State(state): State<Arc<ApiState>>,
    Extension(record): Extension<CredentialRecord>,
    body: Result<Json<BlockedRequest>, JsonRejection>,
) -> Result<Json<ReportAck>, ApiError> {
    let Json(request) = body.map_err(bad_json)?;
    let scope = scope_of(&record, &request.claims)?;
    report(&state, scope, ReportContent::blocked(&request.reason)).await
}
