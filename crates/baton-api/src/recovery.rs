//! 崩潰恢復端點（桌面角色專用，票 31）：目前的恢復事件（次數、退避期限、受影響工作與診斷）與「重新嘗試恢復」。
//!
//! 規則都在 Domain（`baton_domain::recovery`）；端點只做「交給 Store 套用 → 對應成穩定的欄位」。
//! 「重新嘗試恢復」只能由桌面管理憑證發起（Agent 憑證進不了 `/api/v1`），而且只在恢復額度用盡時接受。

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use baton_domain::recovery::{self, RecoveryCommand, RecoveryEvent, RecoveryStatus, MAX_ATTEMPTS};
use baton_domain::scheduler::WaitReason;
use baton_store::RecoveryApplyError;
use serde::{Deserialize, Serialize};

use crate::workspaces::ApiError;
use crate::ApiState;

/// 恢復狀態路徑（GET）。
pub const RECOVERY_PATH: &str = "/api/v1/recovery";
/// 重新嘗試恢復路徑（POST）。
pub const RECOVERY_RETRY_PATH: &str = "/api/v1/recovery/retry";

/// 崩潰恢復的總覽。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryStatusView {
    /// 最新的恢復事件（含全部輪次的嘗試與受影響工作）；從未發生過崩潰為 `null`。
    pub event: Option<RecoveryEvent>,
    /// 每一輪最多的自動嘗試次數。
    pub max_attempts: u32,
    /// 恢復額度用盡：可以從恢復介面「重新嘗試恢復」。
    pub can_retry: bool,
    /// 目前恢復是否擋住派工，以及原因；不擋為 `null`。
    pub blocking: Option<WaitReason>,
}

pub(crate) fn status_of(event: Option<RecoveryEvent>) -> RecoveryStatusView {
    RecoveryStatusView {
        can_retry: event.as_ref().is_some_and(|e| e.status == RecoveryStatus::Exhausted),
        blocking: recovery::dispatch_halt(event.as_ref()),
        max_attempts: MAX_ATTEMPTS,
        event,
    }
}

impl From<RecoveryApplyError> for ApiError {
    fn from(error: RecoveryApplyError) -> Self {
        match error {
            RecoveryApplyError::Rejected(rejected) => {
                ApiError { status: StatusCode::CONFLICT, code: rejected.code(), message: rejected.to_string() }
            }
            RecoveryApplyError::Store(store) => store.into(),
        }
    }
}

pub(crate) async fn status(State(state): State<Arc<ApiState>>) -> Result<Json<RecoveryStatusView>, ApiError> {
    Ok(Json(status_of(state.store.recovery_event().await?)))
}

/// 「重新嘗試恢復」：建立新輪次（舊輪次的嘗試保留為歷史），由 Supervisor 立即開始新輪次的第一次嘗試。
/// 不是用盡狀態時回應 409。
pub(crate) async fn retry(State(state): State<Arc<ApiState>>) -> Result<Json<RecoveryStatusView>, ApiError> {
    let event = state.store.apply_recovery(RecoveryCommand::Retry, state.clock.now()).await?;
    (state.log)(&format!("崩潰恢復：使用者要求重新嘗試恢復，事件 {} 進入第 {} 輪", event.id, event.round));
    state.supervisor_wake.notify_one();
    Ok(Json(status_of(Some(event))))
}
