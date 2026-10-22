//! 裁定端點（票 22、票 35）：桌面角色讀取裁定中心（總覽與逐項）、處理（單筆或批次）、合併與連結請求、
//! 檢視與收窄決策權限表。
//!
//! 只有桌面憑證能提交裁定：這些路徑都在 `/api/v1/` 之下，Agent 憑證一律在 `guard` 就被擋下（403 `role_not_allowed`）；
//! Agent 只能經 Agent API 提出請求（`request_decision`、`permission_prompt`、`controller_action`）。規則都在 Domain
//! （`baton_domain::decision`、`baton_domain::authority`），端點只做「解析請求 → 交給 Store → 對應成穩定的錯誤代碼」。

use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use axum::Json;
use baton_domain::authority::{AuthorityEntryRequest, AuthorityError, AuthorityTable};
use baton_domain::decision::{DecisionAction, DecisionError, DecisionView, Resolution};
use baton_store::{AnswerResult, DecisionApplyError, DecisionList, PendingByWorkspace};
use serde::{Deserialize, Serialize};

use crate::authorization::bad_json;
use crate::workspaces::{new_id, ApiError};
use crate::ApiState;

pub(crate) const DECISIONS_ROUTE: &str = "/api/v1/workspaces/{id}/decisions";
pub(crate) const DECISION_ROUTE: &str = "/api/v1/workspaces/{id}/decisions/{request_id}";
pub(crate) const ANSWER_ROUTE: &str = "/api/v1/workspaces/{id}/decisions/{request_id}/answer";
pub(crate) const MERGE_ROUTE: &str = "/api/v1/workspaces/{id}/decisions/{request_id}/merge";
pub(crate) const LINKS_ROUTE: &str = "/api/v1/workspaces/{id}/decisions/{request_id}/links";
pub(crate) const AUTHORITY_ROUTE: &str = "/api/v1/workspaces/{id}/decision-authority";
/// 全域待處理裁定數（狀態列）。
pub const PENDING_DECISIONS_PATH: &str = "/api/v1/decisions/pending";
/// 批次處理（可跨 Workspace，每一項各自保存）。
pub const DECISIONS_BATCH_PATH: &str = "/api/v1/decisions/batch";
/// 一次批次最多幾項。
pub const BATCH_MAX_ITEMS: usize = 50;

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

pub fn decision_merge_path(workspace_id: &str, request_id: &str) -> String {
    format!("{}/merge", decision_path(workspace_id, request_id))
}

pub fn decision_links_path(workspace_id: &str, request_id: &str) -> String {
    format!("{}/links", decision_path(workspace_id, request_id))
}

pub fn decision_authority_path(workspace_id: &str) -> String {
    format!("{}/{workspace_id}/decision-authority", crate::WORKSPACES_PATH)
}

fn default_action() -> DecisionAction {
    DecisionAction::Accept
}

/// 處理一個請求的內容。`expected_revision` 是使用者看到的內容版本，請求之後有任何變動就拒絕。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnswerRequest {
    #[serde(default = "default_action")]
    pub action: DecisionAction,
    pub expected_revision: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub option_id: Option<String>,
    /// 修改後的條件、回答、要調查的事或備註。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// 只有延後用到；省略時延後一天。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub defer_minutes: Option<i64>,
}

impl AnswerRequest {
    fn resolution(self) -> Resolution {
        Resolution { action: self.action, option_id: self.option_id, note: self.note, defer_minutes: self.defer_minutes }
    }
}

/// 批次中的一項：指明 Workspace 與請求，處理內容與單筆相同。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatchItem {
    pub workspace_id: String,
    pub request_id: String,
    #[serde(flatten)]
    pub answer: AnswerRequest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatchRequest {
    pub items: Vec<BatchItem>,
}

/// 批次中一項的結果；成功與失敗各自回報，順序與請求相同。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BatchItemResult {
    pub workspace_id: String,
    pub request_id: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<DecisionView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resumed_task_ids: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BatchResponse {
    /// 只用於稽核：每筆裁定各自保存，這個識別不代表任何概括授權。
    pub batch_id: String,
    pub results: Vec<BatchItemResult>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeRequest {
    pub into: String,
    pub expected_revision: i64,
    pub into_expected_revision: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MergeResponse {
    /// 合併後的目標請求。
    pub decision: DecisionView,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkRequest {
    pub other_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateAuthorityRequest {
    pub expected_revision: i64,
    pub entries: Vec<AuthorityEntryRequest>,
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
                    DecisionError::Invalid(_)
                    | DecisionError::OptionUnknown(_)
                    | DecisionError::ActionNotAllowed(_)
                    | DecisionError::MergeInvalid(_) => StatusCode::BAD_REQUEST,
                    DecisionError::StaleRun
                    | DecisionError::AlreadyAnswered(_)
                    | DecisionError::RevisionMismatch { .. }
                    | DecisionError::Superseded(_)
                    | DecisionError::Merged(_) => StatusCode::CONFLICT,
                    DecisionError::NotDesktop => StatusCode::FORBIDDEN,
                };
                ApiError { status, code: rejected.code(), message: rejected.to_string() }
            }
            DecisionApplyError::Authority(rejected) => {
                let status = match rejected {
                    AuthorityError::RevisionMismatch { .. } => StatusCode::CONFLICT,
                    AuthorityError::WideningRefused(..) | AuthorityError::HumanOnlyFixed(_) => StatusCode::BAD_REQUEST,
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

/// 處理一個請求：裁定、請求狀態、受影響 Task 的恢復與喚醒 Outbox 在同一個交易提交。
/// 同樣的處理重送是冪等的（200）；不同的處理回 409 `decision_already_answered`。
pub(crate) async fn answer(
    State(state): State<Arc<ApiState>>,
    UrlPath((id, request_id)): UrlPath<(String, String)>,
    body: Result<Json<AnswerRequest>, JsonRejection>,
) -> Result<Json<AnswerResult>, ApiError> {
    let Json(request) = body.map_err(bad_json)?;
    let revision = request.expected_revision;
    let result = state
        .store
        .answer_decision(&id, &request_id, revision, request.resolution(), None, state.clock.now())
        .await;
    wake_if_resumed(&state, &result);
    let result = result?;
    // 正式裁定是重要裁定：要求備份（同一小時內的要求合併為一次，冪等重送也不會多備份）；延後不是裁定
    if result.decision.decision.is_some() {
        (state.request_backup)("正式裁定");
    }
    Ok(Json(result))
}

/// 恢復的 Task 已在同一個交易排進派工佇列；叫醒 Supervisor 立刻排程，不必等下一次背景處理。
/// 過期的請求也可能讓 Task 恢復（錯誤回應之前已經寫入），所以失敗時也叫醒一次。
fn wake_if_resumed(state: &ApiState, result: &Result<AnswerResult, DecisionApplyError>) {
    let resumed = match result {
        Ok(answer) => !answer.resumed_task_ids.is_empty(),
        Err(DecisionApplyError::Rejected(DecisionError::Superseded(_))) => true,
        Err(_) => false,
    };
    if resumed {
        state.supervisor_wake.notify_one();
    }
}

/// 批次處理：逐項各自一個交易，一項失敗不影響其他項；每筆裁定只對應它自己的請求。
pub(crate) async fn batch(
    State(state): State<Arc<ApiState>>,
    body: Result<Json<BatchRequest>, JsonRejection>,
) -> Result<Json<BatchResponse>, ApiError> {
    let Json(request) = body.map_err(bad_json)?;
    let invalid = |message: String| ApiError { status: StatusCode::BAD_REQUEST, code: "decision_batch_invalid", message };
    if request.items.is_empty() || request.items.len() > BATCH_MAX_ITEMS {
        return Err(invalid(format!("批次必須有 1 到 {BATCH_MAX_ITEMS} 項")));
    }
    for (index, item) in request.items.iter().enumerate() {
        if request.items[..index].iter().any(|other| other.workspace_id == item.workspace_id && other.request_id == item.request_id) {
            return Err(invalid(format!("裁定請求 {} 在同一批中出現了兩次", item.request_id)));
        }
    }
    let batch_id = new_id("batch")?;
    let mut results = Vec::with_capacity(request.items.len());
    for item in request.items {
        let result = state
            .store
            .answer_decision(
                &item.workspace_id,
                &item.request_id,
                item.answer.expected_revision,
                item.answer.resolution(),
                Some(batch_id.clone()),
                state.clock.now(),
            )
            .await;
        wake_if_resumed(&state, &result);
        let base = BatchItemResult {
            workspace_id: item.workspace_id,
            request_id: item.request_id,
            ok: false,
            decision: None,
            resumed_task_ids: None,
            error: None,
            message: None,
        };
        results.push(match result {
            Ok(answer) => BatchItemResult {
                ok: true,
                decision: Some(answer.decision),
                resumed_task_ids: Some(answer.resumed_task_ids),
                ..base
            },
            Err(error) => {
                let error = ApiError::from(error);
                BatchItemResult { error: Some(error.code.to_string()), message: Some(error.message), ..base }
            }
        });
    }
    if results.iter().any(|r| r.decision.as_ref().is_some_and(|d| d.decision.is_some())) {
        (state.request_backup)("正式裁定");
    }
    Ok(Json(BatchResponse { batch_id, results }))
}

pub(crate) async fn merge(
    State(state): State<Arc<ApiState>>,
    UrlPath((id, request_id)): UrlPath<(String, String)>,
    body: Result<Json<MergeRequest>, JsonRejection>,
) -> Result<Json<MergeResponse>, ApiError> {
    let Json(request) = body.map_err(bad_json)?;
    let decision = state
        .store
        .merge_decision(&id, &request_id, request.expected_revision, &request.into, request.into_expected_revision, state.clock.now())
        .await?;
    Ok(Json(MergeResponse { decision }))
}

pub(crate) async fn link(
    State(state): State<Arc<ApiState>>,
    UrlPath((id, request_id)): UrlPath<(String, String)>,
    body: Result<Json<LinkRequest>, JsonRejection>,
) -> Result<Json<DecisionView>, ApiError> {
    let Json(request) = body.map_err(bad_json)?;
    Ok(Json(state.store.link_decision(&id, &request_id, &request.other_id, state.clock.now()).await?))
}

pub(crate) async fn authority(
    State(state): State<Arc<ApiState>>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<AuthorityTable>, ApiError> {
    Ok(Json(state.store.decision_authority(&id).await?))
}

/// 取代 Workspace 的決策權限表：只能收窄，任何放寬整個拒絕（400 `authority_widening_refused`）。
pub(crate) async fn update_authority(
    State(state): State<Arc<ApiState>>,
    UrlPath(id): UrlPath<String>,
    body: Result<Json<UpdateAuthorityRequest>, JsonRejection>,
) -> Result<Json<AuthorityTable>, ApiError> {
    let Json(request) = body.map_err(bad_json)?;
    Ok(Json(
        state
            .store
            .update_decision_authority(&id, request.expected_revision, request.entries, state.clock.now())
            .await?,
    ))
}

pub(crate) async fn pending(State(state): State<Arc<ApiState>>) -> Result<Json<PendingDecisions>, ApiError> {
    let by_workspace = state.store.pending_decisions_by_workspace(state.clock.now()).await?;
    Ok(Json(PendingDecisions { pending_count: by_workspace.iter().map(|w| w.pending_count).sum(), by_workspace }))
}
