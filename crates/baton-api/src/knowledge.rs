//! 知識與教訓端點（票 37，spec D-26、D-28）。
//!
//! - 管理 API（桌面憑證）：在「知識」分頁列出與搜尋條目、建立（直接是已驗證）、確認候選、修正、撤銷、淘汰，以及查看版本與變更歷史。
//! - Agent API：`record_lesson_candidate` 只能提交候選教訓（狀態由 Runtime 決定，請求無法指定）；
//!   `search_knowledge` 只回傳這個 Run 適用範圍內「已驗證且在有效期間內」的條目。
//! - 搜尋以 FTS5 + trigram 為主；FTS5 不可用時退回 LIKE，回應的 `search_mode` 會標明降級。

use std::sync::Arc;

use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use baton_domain::auth::CredentialRecord;
use baton_domain::knowledge::{
    self, KnowledgeCommand, KnowledgeContent, KnowledgeEntry, KnowledgeError, KnowledgeEvent, KnowledgeKind,
    KnowledgeVersion, LessonDetail, ScopeKind, SearchMode,
};
use baton_domain::Timestamp;
use baton_store::{KnowledgeApplyError, KnowledgeAudience, NewKnowledgeRequest, RunApplyError, ScopeRequest};
use serde::{Deserialize, Serialize};

use crate::agent::{scope_of, Claims};
use crate::authorization::bad_json;
use crate::workspaces::{new_id, ApiError};
use crate::ApiState;

/// 知識列表與建立的路徑。
pub const KNOWLEDGE_PATH: &str = "/api/v1/knowledge";
pub(crate) const ENTRY_ROUTE: &str = "/api/v1/knowledge/{id}";
pub(crate) const CONFIRM_ROUTE: &str = "/api/v1/knowledge/{id}/confirm";
pub(crate) const CORRECT_ROUTE: &str = "/api/v1/knowledge/{id}/correct";
pub(crate) const REVOKE_ROUTE: &str = "/api/v1/knowledge/{id}/revoke";
pub(crate) const RETIRE_ROUTE: &str = "/api/v1/knowledge/{id}/retire";

/// 路徑；Runtime、用戶端與測試共用。
pub fn knowledge_entry_path(id: &str) -> String {
    format!("{KNOWLEDGE_PATH}/{id}")
}

/// 條目操作的路徑：`action` 是 `confirm`、`correct`、`revoke` 或 `retire`。
pub fn knowledge_action_path(id: &str, action: &str) -> String {
    format!("{}/{action}", knowledge_entry_path(id))
}

/// 條目加上「現在是否有效」（已驗證且在有效期間內）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnowledgeEntryView {
    #[serde(flatten)]
    pub entry: KnowledgeEntry,
    pub effective: bool,
}

/// 列表或搜尋的結果；`search_mode = like_fallback` 表示全文檢索降級。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnowledgeList {
    pub search_mode: SearchMode,
    pub entries: Vec<KnowledgeEntryView>,
}

/// 單一條目：目前狀態、全部內容版本（由舊到新）與變更歷史（由舊到新）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KnowledgeDetail {
    pub entry: KnowledgeEntryView,
    pub versions: Vec<KnowledgeVersion>,
    pub history: Vec<KnowledgeEvent>,
}

/// 列表查詢參數：`workspace_id` 省略時只列全域條目；`q` 為空時不過濾文字；
/// `include_inactive` 為 false（預設）時只回傳有效條目，為 true 時連同候選、撤銷、淘汰、被取代與過期的條目一起回傳。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct KnowledgeListQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub q: Option<String>,
    #[serde(default)]
    pub include_inactive: bool,
}

/// 適用範圍的請求形狀：`kind` 是 `global`、`workspace`、`goal`、`task`；Task 範圍的 Goal 由 Runtime 補上。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopeInput {
    pub kind: ScopeKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
}

/// 使用者建立條目（直接是已驗證）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateKnowledgeRequest {
    pub kind: KnowledgeKind,
    pub scope: ScopeInput,
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub evidence: Vec<String>,
    /// 只有教訓可以填。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lesson: Option<LessonDetail>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_from: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_until: Option<Timestamp>,
    /// 要取代的既有條目（同種類、已驗證）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<String>,
}

/// 確認候選。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfirmKnowledgeRequest {
    pub expected_revision: i64,
}

/// 修正：以完整的新內容取代目前內容（追加新版本）；`valid_until` 為空表示沒有期限。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CorrectKnowledgeRequest {
    pub expected_revision: i64,
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub evidence: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lesson: Option<LessonDetail>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_until: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// 撤銷或淘汰（都要說明原因）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnowledgeReasonRequest {
    pub expected_revision: i64,
    pub reason: String,
}

/// `record_lesson_candidate` 的回應。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LessonCandidateRecorded {
    pub entry_id: String,
    pub status: knowledge::KnowledgeStatus,
    pub revision: i64,
}

impl From<KnowledgeError> for ApiError {
    fn from(error: KnowledgeError) -> Self {
        let status = match error {
            KnowledgeError::Invalid(_) | KnowledgeError::QueryInvalid(_) | KnowledgeError::ScopeInvalid(_) => {
                StatusCode::BAD_REQUEST
            }
            KnowledgeError::NotFound(_) => StatusCode::NOT_FOUND,
            KnowledgeError::RevisionConflict { .. }
            | KnowledgeError::TransitionInvalid { .. }
            | KnowledgeError::SupersedeInvalid(_) => StatusCode::CONFLICT,
        };
        ApiError { status, code: error.code(), message: error.to_string() }
    }
}

impl From<KnowledgeApplyError> for ApiError {
    fn from(error: KnowledgeApplyError) -> Self {
        match error {
            KnowledgeApplyError::Rejected(rejected) => rejected.into(),
            KnowledgeApplyError::Run(run) => RunApplyError::Rejected(run).into(),
            KnowledgeApplyError::Store(store) => store.into(),
        }
    }
}

fn bad_query(rejection: QueryRejection) -> ApiError {
    ApiError { status: StatusCode::BAD_REQUEST, code: "bad_request", message: format!("查詢參數不正確：{}", rejection.body_text()) }
}

fn view(entry: KnowledgeEntry, now: Timestamp) -> KnowledgeEntryView {
    KnowledgeEntryView { effective: entry.is_effective_at(now), entry }
}

// --- 管理 API ---

pub(crate) async fn list(
    State(state): State<Arc<ApiState>>,
    query: Result<Query<KnowledgeListQuery>, QueryRejection>,
) -> Result<Json<KnowledgeList>, ApiError> {
    let Query(query) = query.map_err(bad_query)?;
    let terms = knowledge::parse_query(query.q.as_deref().unwrap_or_default())?;
    if let Some(workspace_id) = &query.workspace_id {
        if !state.store.list_workspaces().await?.iter().any(|w| &w.id == workspace_id) {
            return Err(ApiError {
                status: StatusCode::NOT_FOUND,
                code: "workspace_not_found",
                message: format!("找不到 Workspace {workspace_id}"),
            });
        }
    }
    let now = state.clock.now();
    let audience = KnowledgeAudience::Desktop { workspace_id: query.workspace_id };
    let entries = state.store.knowledge_search(audience, terms, query.include_inactive, now).await?;
    Ok(Json(KnowledgeList {
        search_mode: state.store.search_mode(),
        entries: entries.into_iter().map(|e| view(e, now)).collect(),
    }))
}

pub(crate) async fn create(
    State(state): State<Arc<ApiState>>,
    body: Result<Json<CreateKnowledgeRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<KnowledgeEntryView>), ApiError> {
    let Json(request) = body.map_err(bad_json)?;
    let content = KnowledgeContent::new(
        request.kind,
        &request.title,
        &request.body,
        &request.tags,
        &request.evidence,
        request.lesson,
    )?;
    let input = NewKnowledgeRequest {
        id: new_id("know")?,
        kind: request.kind,
        scope: ScopeRequest {
            kind: request.scope.kind,
            workspace_id: request.scope.workspace_id,
            goal_id: request.scope.goal_id,
            task_id: request.scope.task_id,
        },
        content,
        valid_from: request.valid_from,
        valid_until: request.valid_until,
        supersedes: request.supersedes,
    };
    let now = state.clock.now();
    let entry = state.store.create_knowledge(input, now).await?;
    Ok((StatusCode::CREATED, Json(view(entry, now))))
}

pub(crate) async fn detail(
    State(state): State<Arc<ApiState>>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<KnowledgeDetail>, ApiError> {
    let (entry, versions, history) = state.store.knowledge_detail(&id).await?;
    Ok(Json(KnowledgeDetail { entry: view(entry, state.clock.now()), versions, history }))
}

async fn apply(
    state: &ApiState,
    id: String,
    expected_revision: i64,
    command: KnowledgeCommand,
) -> Result<Json<KnowledgeEntryView>, ApiError> {
    let now = state.clock.now();
    let entry = state.store.apply_knowledge_command(&id, expected_revision, command, now).await?;
    Ok(Json(view(entry, now)))
}

pub(crate) async fn confirm(
    State(state): State<Arc<ApiState>>,
    UrlPath(id): UrlPath<String>,
    body: Result<Json<ConfirmKnowledgeRequest>, JsonRejection>,
) -> Result<Json<KnowledgeEntryView>, ApiError> {
    let Json(request) = body.map_err(bad_json)?;
    apply(&state, id, request.expected_revision, KnowledgeCommand::Confirm).await
}

pub(crate) async fn correct(
    State(state): State<Arc<ApiState>>,
    UrlPath(id): UrlPath<String>,
    body: Result<Json<CorrectKnowledgeRequest>, JsonRejection>,
) -> Result<Json<KnowledgeEntryView>, ApiError> {
    let Json(request) = body.map_err(bad_json)?;
    // 種類決定是否允許教訓欄位，所以先讀出目前的條目
    let (current, _, _) = state.store.knowledge_detail(&id).await?;
    let content = KnowledgeContent::new(
        current.kind,
        &request.title,
        &request.body,
        &request.tags,
        &request.evidence,
        request.lesson,
    )?;
    let command = KnowledgeCommand::Correct { content, valid_until: request.valid_until, note: request.note };
    apply(&state, id, request.expected_revision, command).await
}

pub(crate) async fn revoke(
    State(state): State<Arc<ApiState>>,
    UrlPath(id): UrlPath<String>,
    body: Result<Json<KnowledgeReasonRequest>, JsonRejection>,
) -> Result<Json<KnowledgeEntryView>, ApiError> {
    let Json(request) = body.map_err(bad_json)?;
    apply(&state, id, request.expected_revision, KnowledgeCommand::Revoke { reason: request.reason }).await
}

pub(crate) async fn retire(
    State(state): State<Arc<ApiState>>,
    UrlPath(id): UrlPath<String>,
    body: Result<Json<KnowledgeReasonRequest>, JsonRejection>,
) -> Result<Json<KnowledgeEntryView>, ApiError> {
    let Json(request) = body.map_err(bad_json)?;
    apply(&state, id, request.expected_revision, KnowledgeCommand::Retire { reason: request.reason }).await
}

// --- Agent API ---

#[derive(Debug, Deserialize)]
pub(crate) struct LessonCandidateRequest {
    title: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    trigger: String,
    #[serde(default)]
    root_cause: String,
    #[serde(default)]
    resolution: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    evidence: Vec<String>,
    /// `task`（預設）、`goal` 或 `workspace`；一律以憑證所屬 Run 的 Task 解析，不接受識別。
    #[serde(default)]
    scope: Option<ScopeKind>,
    #[serde(flatten)]
    claims: Claims,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SearchRequest {
    #[serde(default)]
    query: String,
    #[serde(flatten)]
    claims: Claims,
}

pub(crate) async fn record_lesson_candidate(
    State(state): State<Arc<ApiState>>,
    Extension(record): Extension<CredentialRecord>,
    body: Result<Json<LessonCandidateRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<LessonCandidateRecorded>), ApiError> {
    let Json(request) = body.map_err(bad_json)?;
    let scope = scope_of(&record, &request.claims)?;
    let lesson = LessonDetail { trigger: request.trigger, root_cause: request.root_cause, resolution: request.resolution };
    let content = KnowledgeContent::new(
        KnowledgeKind::Lesson,
        &request.title,
        &request.body,
        &request.tags,
        &request.evidence,
        Some(lesson),
    )?;
    let entry = state
        .store
        .record_lesson_candidate(
            &scope,
            new_id("know")?,
            request.scope.unwrap_or(ScopeKind::Task),
            content,
            state.clock.now(),
        )
        .await?;
    let recorded = LessonCandidateRecorded { entry_id: entry.id, status: entry.status, revision: entry.revision };
    Ok((StatusCode::CREATED, Json(recorded)))
}

pub(crate) async fn search_knowledge(
    State(state): State<Arc<ApiState>>,
    Extension(record): Extension<CredentialRecord>,
    body: Result<Json<SearchRequest>, JsonRejection>,
) -> Result<Json<KnowledgeList>, ApiError> {
    let Json(request) = body.map_err(bad_json)?;
    let scope = scope_of(&record, &request.claims)?;
    let terms = knowledge::parse_query(&request.query)?;
    let context = state.store.agent_context(&scope).await?;
    if !context.is_current {
        return Err(ApiError {
            status: StatusCode::UNAUTHORIZED,
            code: "credential_revoked",
            message: "這個 Run 已經結束或已被接替，憑證已失效".to_string(),
        });
    }
    let now = state.clock.now();
    let audience = KnowledgeAudience::Run {
        workspace_id: scope.workspace_id,
        goal_id: context.task.goal_id,
        task_id: context.task.id,
    };
    // Agent 只能拿到有效的條目：候選、撤銷、淘汰、被取代與過期的一律不回傳
    let entries = state.store.knowledge_search(audience, terms, false, now).await?;
    Ok(Json(KnowledgeList {
        search_mode: state.store.search_mode(),
        entries: entries.into_iter().map(|e| view(e, now)).collect(),
    }))
}
