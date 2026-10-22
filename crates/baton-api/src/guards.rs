//! Guard 與重犯事件端點（票 38，桌面憑證專用）。
//!
//! - 使用者在知識分頁把有效的教訓轉成 Guard（建立即啟用）、啟用或停用，並看到每個 Guard 的成效（攔下次數、未攔下的回報次數）。
//! - Guard 在 Run 准入（派工）時檢查；擋下時派工回 202、等待原因的類別是 `guard`，帶來源教訓（見 `baton_domain::scheduler`）。
//! - 重犯事件掛在教訓底下：被 Guard 攔下的自動記錄，沒被攔下的由使用者回報。

use std::sync::Arc;

use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::StatusCode;
use axum::Json;
use baton_domain::guard::{self, Guard, GuardCommand, GuardError, GuardRule, Recurrence};
use baton_domain::knowledge::{KnowledgeScope, KnowledgeStatus};
use baton_domain::Timestamp;
use baton_store::{GuardApplyError, GuardOverview};
use serde::{Deserialize, Serialize};

use crate::authorization::bad_json;
use crate::workspaces::{new_id, ApiError};
use crate::ApiState;

/// Guard 列表與建立的路徑。
pub const GUARDS_PATH: &str = "/api/v1/guards";
pub(crate) const ENABLE_ROUTE: &str = "/api/v1/guards/{id}/enable";
pub(crate) const DISABLE_ROUTE: &str = "/api/v1/guards/{id}/disable";
pub(crate) const RECURRENCES_ROUTE: &str = "/api/v1/knowledge/{id}/recurrences";

/// Guard 操作的路徑：`action` 是 `enable` 或 `disable`。
pub fn guard_action_path(id: &str, action: &str) -> String {
    format!("{GUARDS_PATH}/{id}/{action}")
}

/// 教訓的重犯事件路徑。
pub fn recurrences_path(lesson_id: &str) -> String {
    format!("{}/recurrences", crate::knowledge_entry_path(lesson_id))
}

/// Guard 的來源教訓（目前狀態）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuardLesson {
    pub id: String,
    pub title: String,
    pub status: KnowledgeStatus,
    pub effective: bool,
    pub scope: KnowledgeScope,
}

/// 列表上的一個 Guard：本身、來源教訓、是否生效與成效。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuardView {
    #[serde(flatten)]
    pub guard: Guard,
    pub lesson: GuardLesson,
    /// 啟用且來源教訓現在有效：Run 准入時會檢查。
    pub active: bool,
    /// 不生效的原因（已停用、來源教訓已撤銷或過期等）。
    pub inactive_reason: Option<String>,
    /// 規則的說明。
    pub description: String,
    /// 成效：攔下的重犯次數與最近一次的時間。
    pub prevented_count: i64,
    pub last_prevented_at: Option<Timestamp>,
    /// 成效：Guard 建立之後，同一個教訓沒被攔下、由使用者回報的重犯次數。
    pub reported_count: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuardList {
    pub guards: Vec<GuardView>,
}

/// 列表查詢參數：`workspace_id` 省略時只列來源教訓是全域的 Guard。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GuardListQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
}

/// 把教訓轉成 Guard（建立即啟用）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateGuardRequest {
    pub lesson_id: String,
    pub rule: GuardRule,
}

/// 啟用或停用。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuardRevisionRequest {
    pub expected_revision: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecurrenceList {
    pub recurrences: Vec<Recurrence>,
}

/// 回報一次沒被攔下的重犯；指定 Task 時要同時指定它的 Workspace。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportRecurrenceRequest {
    pub note: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
}

impl From<GuardError> for ApiError {
    fn from(error: GuardError) -> Self {
        let status = match error {
            GuardError::Invalid(_) => StatusCode::BAD_REQUEST,
            GuardError::NotFound(_) => StatusCode::NOT_FOUND,
            GuardError::LessonIneligible(_) | GuardError::RevisionConflict { .. } => StatusCode::CONFLICT,
        };
        ApiError { status, code: error.code(), message: error.to_string() }
    }
}

impl From<GuardApplyError> for ApiError {
    fn from(error: GuardApplyError) -> Self {
        match error {
            GuardApplyError::Rejected(rejected) => rejected.into(),
            GuardApplyError::Knowledge(knowledge) => knowledge.into(),
            GuardApplyError::Store(store) => store.into(),
        }
    }
}

fn view(overview: GuardOverview, now: Timestamp) -> GuardView {
    let GuardOverview { guard, lesson, prevented_count, last_prevented_at, reported_count } = overview;
    let inactive_reason = guard::inactive_reason(&guard, &lesson, now);
    GuardView {
        description: guard.rule.describe(),
        active: inactive_reason.is_none(),
        inactive_reason,
        lesson: GuardLesson {
            effective: lesson.is_effective_at(now),
            id: lesson.id,
            title: lesson.content.title,
            status: lesson.status,
            scope: lesson.scope,
        },
        guard,
        prevented_count,
        last_prevented_at,
        reported_count,
    }
}

pub(crate) async fn list(
    State(state): State<Arc<ApiState>>,
    query: Result<Query<GuardListQuery>, QueryRejection>,
) -> Result<Json<GuardList>, ApiError> {
    let Query(query) = query.map_err(|rejection| ApiError {
        status: StatusCode::BAD_REQUEST,
        code: "bad_request",
        message: format!("查詢參數不正確：{}", rejection.body_text()),
    })?;
    let now = state.clock.now();
    let guards = state.store.guard_list(query.workspace_id).await?;
    Ok(Json(GuardList { guards: guards.into_iter().map(|g| view(g, now)).collect() }))
}

pub(crate) async fn create(
    State(state): State<Arc<ApiState>>,
    body: Result<Json<CreateGuardRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<GuardView>), ApiError> {
    let Json(request) = body.map_err(bad_json)?;
    let now = state.clock.now();
    let created = state.store.create_guard(new_id("guard")?, &request.lesson_id, request.rule, now).await?;
    Ok((StatusCode::CREATED, Json(view(created, now))))
}

async fn apply(state: &ApiState, id: &str, expected_revision: i64, command: GuardCommand) -> Result<Json<GuardView>, ApiError> {
    let now = state.clock.now();
    let changed = state.store.apply_guard_command(id, expected_revision, command, now).await?;
    // Guard 停用後，佇列中被它擋下的 Task 可能可以派了：喚醒 Supervisor 重試佇列
    state.supervisor_wake.notify_one();
    Ok(Json(view(changed, now)))
}

pub(crate) async fn enable(
    State(state): State<Arc<ApiState>>,
    UrlPath(id): UrlPath<String>,
    body: Result<Json<GuardRevisionRequest>, JsonRejection>,
) -> Result<Json<GuardView>, ApiError> {
    let Json(request) = body.map_err(bad_json)?;
    apply(&state, &id, request.expected_revision, GuardCommand::Enable).await
}

pub(crate) async fn disable(
    State(state): State<Arc<ApiState>>,
    UrlPath(id): UrlPath<String>,
    body: Result<Json<GuardRevisionRequest>, JsonRejection>,
) -> Result<Json<GuardView>, ApiError> {
    let Json(request) = body.map_err(bad_json)?;
    apply(&state, &id, request.expected_revision, GuardCommand::Disable).await
}

pub(crate) async fn recurrences(
    State(state): State<Arc<ApiState>>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<RecurrenceList>, ApiError> {
    Ok(Json(RecurrenceList { recurrences: state.store.recurrences_of(&id).await? }))
}

pub(crate) async fn report(
    State(state): State<Arc<ApiState>>,
    UrlPath(id): UrlPath<String>,
    body: Result<Json<ReportRecurrenceRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<Recurrence>), ApiError> {
    let Json(request) = body.map_err(bad_json)?;
    // 只帶 Workspace（App 的重犯表單沒填 Task）時也記下 Workspace，不靜默丟棄
    let scope = match (request.workspace_id, request.task_id) {
        (Some(workspace_id), task_id) => Some((workspace_id, task_id)),
        (None, None) => None,
        (None, Some(_)) => {
            return Err(GuardError::Invalid("指定 Task 時必須同時指定它的 Workspace".into()).into());
        }
    };
    let recurrence =
        state.store.report_recurrence(new_id("rec")?, &id, scope, request.note, state.clock.now()).await?;
    Ok((StatusCode::CREATED, Json(recurrence)))
}
