//! 時間線與觀測指標端點（桌面角色專用，票 41）。
//!
//! 時間線：Workspace 內派工、交接、裁定、恢復、驗證與清理（以及指標用到的 Task 狀態、外部操作、重犯、用量）事件，
//! 依提交順序，可依 Task 過濾。指標：只由同一份時間線事件以 `baton_domain::metrics::compute` 計算，
//! 所以拿時間線重新計算一定得到相同的數字。服務端額度只採用 Claude Code 回報的帳號層級用量，觀測不到為 `null`（未知）。

use std::sync::Arc;

use axum::extract::rejection::QueryRejection;
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::StatusCode;
use axum::Json;
use baton_domain::metrics::{self, Metrics};
use baton_domain::profile::ProfileStatus;
use baton_domain::quota::{QuotaAssessment, QuotaSource, QuotaWindow};
use baton_domain::timeline::TimelineEntry;
use baton_store::TimelineError;
use serde::{Deserialize, Serialize};

use crate::workspaces::ApiError;
use crate::ApiState;

pub(crate) const TIMELINE_ROUTE: &str = "/api/v1/workspaces/{id}/timeline";
pub(crate) const METRICS_ROUTE: &str = "/api/v1/workspaces/{id}/metrics";

/// 時間線路徑；`task_id` 由呼叫端放在查詢字串（`?task_id=`）。
pub fn timeline_path(workspace_id: &str) -> String {
    format!("/api/v1/workspaces/{workspace_id}/timeline")
}

pub fn metrics_path(workspace_id: &str) -> String {
    format!("/api/v1/workspaces/{workspace_id}/metrics")
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct TimelineQuery {
    #[serde(default)]
    pub task_id: Option<String>,
}

/// 時間線：事件依提交順序（`seq` 遞增）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimelineResponse {
    pub workspace_id: String,
    pub task_id: Option<String>,
    pub events: Vec<TimelineEntry>,
}

/// 一個帳號的服務端額度：只有 Claude Code（或外部觀測器）回報的帳號層級已用比例；看不到為 `null`（未知，不填零）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServerQuota {
    pub profile_id: String,
    pub name: String,
    pub five_hour_used: Option<f64>,
    pub seven_day_used: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricsResponse {
    pub workspace_id: String,
    pub metrics: Metrics,
    pub server_quota: Vec<ServerQuota>,
}

impl From<TimelineError> for ApiError {
    fn from(error: TimelineError) -> Self {
        let message = error.to_string();
        let code = match error {
            TimelineError::Store(store) => return store.into(),
            TimelineError::WorkspaceNotFound(_) => "workspace_not_found",
            TimelineError::TaskNotFound(_) => "task_not_found",
        };
        ApiError { status: StatusCode::NOT_FOUND, code, message }
    }
}

pub(crate) async fn timeline(
    State(state): State<Arc<ApiState>>,
    UrlPath(id): UrlPath<String>,
    query: Result<Query<TimelineQuery>, QueryRejection>,
) -> Result<Json<TimelineResponse>, ApiError> {
    let Query(query) = query.map_err(|rejection| ApiError {
        status: StatusCode::BAD_REQUEST,
        code: "bad_request",
        message: format!("查詢參數不正確：{}", rejection.body_text()),
    })?;
    let task_id = query.task_id.filter(|t| !t.trim().is_empty());
    let events = state.store.timeline(&id, task_id.as_deref()).await?;
    Ok(Json(TimelineResponse { workspace_id: id, task_id, events }))
}

pub(crate) async fn metrics(
    State(state): State<Arc<ApiState>>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<MetricsResponse>, ApiError> {
    let events = state.store.timeline(&id, None).await?;
    let now = state.clock.now();
    let mut quota = crate::quota::current(&state.store, &*state.quota_observer, now).await?;
    let server_quota = state
        .store
        .list_profiles()
        .await?
        .into_iter()
        .filter(|p| p.status == ProfileStatus::Active)
        .map(|p| {
            let assessment = quota.remove(&p.id).map(|q| q.assessment);
            let used = |window| assessment.as_ref().and_then(|a| server_used(a, window));
            ServerQuota {
                five_hour_used: used(QuotaWindow::FiveHour),
                seven_day_used: used(QuotaWindow::SevenDay),
                profile_id: p.id,
                name: p.name,
            }
        })
        .collect();
    Ok(Json(MetricsResponse { workspace_id: id, metrics: metrics::compute(&events), server_quota }))
}

/// 視窗的服務端已用比例：CLI 回報的帳號層級數字優先，其次是外部觀測器的已觀測樣本；本機推估不是服務端額度。
fn server_used(assessment: &QuotaAssessment, window: QuotaWindow) -> Option<f64> {
    let cli = assessment.windows.iter().find(|w| w.window == window).and_then(|w| w.observed.as_ref()).and_then(|o| o.used);
    let external =
        assessment.external.as_ref().filter(|o| o.source == QuotaSource::Observed && o.window.is_none_or(|w| w == window));
    cli.or_else(|| external.and_then(|o| o.used))
}
