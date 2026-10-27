//! 知識與教訓端點（票 37，spec D-26、D-28）。
//!
//! - 管理 API（桌面憑證）：在「知識」分頁列出與搜尋條目、建立（直接是已驗證）、確認候選、修正、撤銷、淘汰，以及查看版本與變更歷史。
//! - Agent API：`record_lesson_candidate` 只能提交候選教訓（狀態由 Runtime 決定，請求無法指定）；Worker 以自己的 Task 解析範圍，
//!   Controller（spec D-05，票 54）以 `task_id` 指定 Workspace 內的來源 Task，並先依決策權限表判定「把候選教訓提交給使用者」；
//!   `search_knowledge` 只回傳這個 Run 適用範圍內「已驗證且在有效期間內」的條目；Controller 憑證（spec D-05 共同工具）
//!   看得到全域與它的 Workspace 內所有範圍的有效條目。
//! - 搜尋以 FTS5 + trigram 為主；FTS5 不可用時退回 LIKE，回應的 `search_mode` 會標明降級。

use std::sync::Arc;

use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use baton_domain::artifact::sha256_hex;
use baton_domain::auth::{ControllerScope, CredentialRecord};
use baton_domain::authority::Matter;
use baton_domain::decision::{ControllerOutcome, EVIDENCE_MAX_CHARS, EVIDENCE_MAX_ITEMS, REASON_MAX_CHARS};
use baton_domain::knowledge::{
    self, KnowledgeCommand, KnowledgeContent, KnowledgeEntry, KnowledgeError, KnowledgeEvent, KnowledgeKind,
    KnowledgeVersion, LessonDetail, ScopeKind, SearchMode,
};
use baton_domain::Timestamp;
use baton_store::{KnowledgeApplyError, KnowledgeAudience, NewKnowledgeRequest, RunApplyError, ScopeRequest};
use serde::{Deserialize, Serialize};

use crate::agent::{scope_of, Claims};
use crate::controller::{controller_scope, ControllerToolError};
use crate::authorization::bad_json;
use crate::reviews::{clip, decide, requires_decision};
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
    crate::understanding::sync_after_knowledge_change(&state, &entry).await;
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
    crate::understanding::sync_after_knowledge_change(state, &entry).await;
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
    /// `task`（預設）、`goal` 或 `workspace`；Worker 一律以憑證所屬 Run 的 Task 解析，Controller 以 `task_id` 解析。
    #[serde(default)]
    scope: Option<ScopeKind>,
    /// 教訓來源的 Task：Controller 必填（必須在它的 Workspace 內）；Worker 送了也不採信。
    #[serde(default)]
    task_id: Option<String>,
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
) -> Result<(StatusCode, Json<LessonCandidateRecorded>), ControllerToolError> {
    let Json(request) = body.map_err(bad_json)?;
    // Controller（spec D-05 的 Controller 工具）：沒有自己的 Task，以 task_id 指定來源 Task
    let entry = if record.controller.is_some() {
        let scope = controller_scope(&state, &record, &request.claims).await?;
        record_for_controller(&state, &scope, request).await?
    } else {
        let scope = scope_of(&record, &request.claims)?;
        let scope_kind = request.scope.unwrap_or(ScopeKind::Task);
        let content = lesson_content(request)?;
        state
            .store
            .record_lesson_candidate(&scope, new_id("know")?, scope_kind, content, state.clock.now())
            .await
            .map_err(ApiError::from)?
    };
    let recorded = LessonCandidateRecorded { entry_id: entry.id, status: entry.status, revision: entry.revision };
    Ok((StatusCode::CREATED, Json(recorded)))
}

fn lesson_content(request: LessonCandidateRequest) -> Result<KnowledgeContent, ApiError> {
    let lesson = LessonDetail { trigger: request.trigger, root_cause: request.root_cause, resolution: request.resolution };
    Ok(KnowledgeContent::new(
        KnowledgeKind::Lesson,
        &request.title,
        &request.body,
        &request.tags,
        &request.evidence,
        Some(lesson),
    )?)
}

/// Controller 提交候選教訓：先驗證來源 Task、範圍與內容，再依決策權限表判定，可以做才寫入。
async fn record_for_controller(
    state: &ApiState,
    scope: &ControllerScope,
    request: LessonCandidateRequest,
) -> Result<KnowledgeEntry, ControllerToolError> {
    let Some(task_id) = request.task_id.as_deref().map(str::trim).filter(|t| !t.is_empty()).map(str::to_string) else {
        let message = "Controller 必須以 task_id 指定教訓來源的 Task（這個 Workspace 內的 Task）".to_string();
        return Err(ApiError::from(KnowledgeError::ScopeInvalid(message)).into());
    };
    let scope_kind = request.scope.unwrap_or(ScopeKind::Task);
    if scope_kind == ScopeKind::Global {
        let message = "Agent 只能提交 Workspace、Goal 或 Task 範圍的教訓".to_string();
        return Err(ApiError::from(KnowledgeError::ScopeInvalid(message)).into());
    }
    let content = lesson_content(request)?;
    // 核准綁定這一次的內容與範圍：內容改了就是另一個裁定
    let fingerprint = serde_json::to_vec(&(scope_kind, &content)).expect("內容一定可以序列化");
    let subject = format!("lesson:{}", sha256_hex(&fingerprint));
    let summary = format!("提交候選教訓「{}」", clip(&content.title, 200));
    let reason = lesson_reason(&content);
    let evidence = content.evidence.iter().take(EVIDENCE_MAX_ITEMS).map(|e| clip(e, EVIDENCE_MAX_CHARS - 1)).collect();
    // 來源 Task 不在這個 Workspace 時，判定就會回報 task_not_found，不建立任何東西
    match decide(state, scope, Matter::SubmitLessonCandidate, &task_id, &subject, &summary, &reason, evidence).await? {
        ControllerOutcome::Allowed | ControllerOutcome::Approved { .. } => {}
        ControllerOutcome::RequiresDecision { request_id, existing } => {
            return Err(requires_decision(Matter::SubmitLessonCandidate, request_id, existing, "候選教訓還沒有寫入"));
        }
        ControllerOutcome::Denied { request_id, note } => {
            return Err(ApiError {
                status: StatusCode::CONFLICT,
                code: "decision_denied",
                message: format!(
                    "使用者沒有核准提交這個候選教訓（裁定請求 {request_id}）{}",
                    note.map(|n| format!("：{n}")).unwrap_or_default()
                ),
            }
            .into());
        }
        ControllerOutcome::HumanOnly => {
            return Err(ApiError {
                status: StatusCode::FORBIDDEN,
                code: "human_only",
                message: "這個 Workspace 的決策權限表規定只有使用者能提交候選教訓".to_string(),
            }
            .into());
        }
    }
    let entry = state
        .store
        .record_controller_lesson_candidate(scope, new_id("know")?, &task_id, scope_kind, content, state.clock.now())
        .await
        .map_err(ApiError::from)?;
    Ok(entry)
}

/// 裁定請求的理由：教訓的內文、觸發條件、根因與解法（都空白時用標題）。
fn lesson_reason(content: &KnowledgeContent) -> String {
    let lesson = content.lesson.clone().unwrap_or_default();
    let parts: Vec<String> = [
        ("內文", content.body.as_str()),
        ("觸發條件", lesson.trigger.as_str()),
        ("根因", lesson.root_cause.as_str()),
        ("解法", lesson.resolution.as_str()),
    ]
    .into_iter()
    .filter(|(_, text)| !text.is_empty())
    .map(|(label, text)| format!("{label}：{text}"))
    .collect();
    let reason = if parts.is_empty() { content.title.clone() } else { parts.join("\n") };
    clip(&reason, REASON_MAX_CHARS - 1)
}

pub(crate) async fn search_knowledge(
    State(state): State<Arc<ApiState>>,
    Extension(record): Extension<CredentialRecord>,
    body: Result<Json<SearchRequest>, JsonRejection>,
) -> Result<Json<KnowledgeList>, ApiError> {
    let Json(request) = body.map_err(bad_json)?;
    // Controller（spec D-05 共同工具）：規劃整個 Workspace 的工作，看得到全域與它的 Workspace 內所有範圍的條目
    let (audience, terms) = if record.controller.is_some() {
        let scope = controller_scope(&state, &record, &request.claims).await?;
        (KnowledgeAudience::Desktop { workspace_id: Some(scope.workspace_id) }, knowledge::parse_query(&request.query)?)
    } else {
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
        let audience =
            KnowledgeAudience::Run { workspace_id: scope.workspace_id, goal_id: context.task.goal_id, task_id: context.task.id };
        (audience, terms)
    };
    let now = state.clock.now();
    // Agent 只能拿到有效的條目：候選、撤銷、淘汰、被取代與過期的一律不回傳
    let entries = state.store.knowledge_search(audience, terms, false, now).await?;
    Ok(Json(KnowledgeList {
        search_mode: state.store.search_mode(),
        entries: entries.into_iter().map(|e| view(e, now)).collect(),
    }))
}
