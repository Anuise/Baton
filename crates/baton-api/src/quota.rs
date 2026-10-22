//! 額度觀測（票 27）：外部觀測器的輪詢、每個帳號目前的額度判斷，以及方案上限端點（桌面角色專用）。
//!
//! 判斷規則都在 Domain（`baton_domain::quota`）。輪詢每個帳號每 5 分鐘最多一次（spec D-31），由 Supervisor 的
//! 每次處理與排程端點觸發（到期才真的呼叫 Port），所以兩條路徑看到的是同一份持久化的觀測。

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use axum::Json;
use baton_domain::quota::{Confidence, ObservationOrigin, PlanLimits, QuotaObservation, QuotaSource};
use baton_domain::Timestamp;
use baton_ports::{PortError, ProfileRef, QuotaObserver};
use baton_store::{AccountQuota, QuotaApplyError, Store, StoreResult};

use crate::scheduling::{overview, SchedulingOverview};
use crate::tasks::TaskApiError;
use crate::workspaces::ApiError;
use crate::ApiState;

/// 方案上限路徑樣板（PUT）。
pub(crate) const PLAN_LIMITS_ROUTE: &str = "/api/v1/profiles/{id}/quota-limits";

/// 方案上限路徑。
pub fn plan_limits_path(profile_id: &str) -> String {
    format!("/api/v1/profiles/{profile_id}/quota-limits")
}

/// 輪詢到期的帳號：向外部觀測器取樣並記下，同時重新推估。沒有外部觀測器（`NotImplemented`）時不留下樣本；
/// 其他失敗記為一筆未知的觀測（不填零）。
pub async fn poll_due(store: &Store, observer: &dyn QuotaObserver, now: Timestamp) -> StoreResult<()> {
    let due = store.quota_polls_due(now).await?;
    if due.is_empty() {
        return Ok(());
    }
    let mut polled = Vec::with_capacity(due.len());
    for profile_id in due {
        let sample = match observer.observe(&ProfileRef(profile_id.clone())).await {
            Err(PortError::NotImplemented(_)) => None,
            Ok(sample) => Some(QuotaObservation {
                profile_id: profile_id.clone(),
                origin: ObservationOrigin::Observer,
                source: sample.source,
                window: None,
                status: None,
                used: sample.used,
                tokens: None,
                limit_tokens: None,
                reset_at: sample.reset_at,
                observed_at: sample.observed_at,
                confidence: if sample.used.is_some() { sample.confidence } else { Confidence::Unknown },
                raw_ref: "quota_observer".to_string(),
                raw: None,
            }),
            Err(error) => Some(QuotaObservation {
                profile_id: profile_id.clone(),
                origin: ObservationOrigin::Observer,
                source: QuotaSource::Observed,
                window: None,
                status: None,
                used: None,
                tokens: None,
                limit_tokens: None,
                reset_at: None,
                observed_at: now,
                confidence: Confidence::Unknown,
                raw_ref: "quota_observer".to_string(),
                raw: Some(baton_domain::quota::truncate_raw(&error.to_string())),
            }),
        };
        polled.push((profile_id, sample));
    }
    store.record_quota_poll(polled, now).await
}

/// 先輪詢到期的帳號，再回傳全部 Profile 目前的額度判斷。
pub async fn current(store: &Store, observer: &dyn QuotaObserver, now: Timestamp) -> StoreResult<HashMap<String, AccountQuota>> {
    poll_due(store, observer, now).await?;
    store.quota_status(now).await
}

/// 排程用：只放已知用量的帳號（以較保守者判斷）；未知的帳號不在結果中，不當作 0 也不阻擋。
pub fn known_used(status: &HashMap<String, AccountQuota>) -> HashMap<String, f64> {
    status.iter().filter_map(|(id, q)| q.assessment.used.map(|used| (id.clone(), used))).collect()
}

impl From<QuotaApplyError> for ApiError {
    fn from(error: QuotaApplyError) -> Self {
        let message = error.to_string();
        let (status, code) = match error {
            QuotaApplyError::Store(store) => return store.into(),
            QuotaApplyError::Invalid(_) => (StatusCode::BAD_REQUEST, "plan_limit_invalid"),
            QuotaApplyError::ProfileNotFound(_) => (StatusCode::NOT_FOUND, "profile_not_found"),
            QuotaApplyError::ProfileArchived(_) => (StatusCode::CONFLICT, "profile_archived"),
        };
        ApiError { status, code, message }
    }
}

/// 設定一個 Profile 的方案上限（推估的分母，token）；回傳新的排程總覽。
pub(crate) async fn set_plan_limits(
    State(state): State<Arc<ApiState>>,
    UrlPath(id): UrlPath<String>,
    body: Result<Json<PlanLimits>, JsonRejection>,
) -> Result<Json<SchedulingOverview>, TaskApiError> {
    let Json(limits) = body.map_err(|rejection| ApiError {
        status: StatusCode::BAD_REQUEST,
        code: "bad_request",
        message: format!("請求內容不正確（five_hour_tokens、seven_day_tokens 必須是正整數或 null）：{rejection}"),
    })?;
    state.store.set_quota_plan_limits(id, limits, state.clock.now()).await.map_err(ApiError::from)?;
    Ok(Json(overview(&state).await?))
}
