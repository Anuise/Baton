//! Controller 事件等待端點（Agent API，只接受仍開著的 Controller 憑證；desktop-controller-cli 票 06）。
//!
//! 長輪詢：先確認 `ack`（有的話），游標之後已有事件就立即回傳；否則等到有寫入提交（重新查詢）、到達逾時（以 Clock 計時）
//! 或 Runtime 停止。等待期間 Controller Run 結束（憑證失效）時回 401。規則都在 Domain（`baton_domain::controller_wait`）。

use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::State;
use axum::http::StatusCode;
use axum::{Extension, Json};
use baton_domain::auth::CredentialRecord;
use baton_domain::controller_wait::{self, WaitResponse, BATCH_LIMIT};
use baton_store::ControllerWaitApplyError;
use serde::Deserialize;

use crate::agent::Claims;
use crate::workspaces::ApiError;
use crate::ApiState;

#[derive(Debug, Deserialize)]
pub(crate) struct WaitRequest {
    /// 已確認（已處理）到哪一筆事件；之前的事件不再回傳。
    #[serde(default)]
    ack: Option<i64>,
    /// 最多等多久（秒）；`0` 只確認與查詢一次、不等待。
    #[serde(default)]
    timeout_seconds: Option<u64>,
    #[serde(flatten)]
    claims: Claims,
}

impl From<ControllerWaitApplyError> for ApiError {
    fn from(error: ControllerWaitApplyError) -> Self {
        match error {
            ControllerWaitApplyError::Rejected(rejected) => {
                ApiError { status: StatusCode::BAD_REQUEST, code: rejected.code(), message: rejected.to_string() }
            }
            ControllerWaitApplyError::Store(store) => store.into(),
        }
    }
}

pub(crate) async fn wait(
    State(state): State<Arc<ApiState>>,
    Extension(record): Extension<CredentialRecord>,
    body: Result<Json<WaitRequest>, JsonRejection>,
) -> Result<Json<WaitResponse>, ApiError> {
    // Worker 不論送什麼內容都得到同一個答案
    crate::controller::require_controller(&record)?;
    let Json(request) = body.map_err(crate::authorization::bad_json)?;
    let scope = crate::controller::controller_scope(&state, &record, &request.claims).await?;
    let timeout = controller_wait::timeout_seconds(request.timeout_seconds)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, code: e.code(), message: e.to_string() })?;
    let cursor = state.store.begin_controller_wait(&scope, request.ack, state.clock.now()).await?;
    let deadline = state.clock.now().plus_millis(timeout as i64 * 1000);
    let mut shutdown = state.shutdown.clone();
    let mut checked_scope = true;
    loop {
        // 先訂閱再查詢：查詢之後的提交一定叫得醒
        let mut commits = state.store.subscribe_commits();
        if !checked_scope {
            // 等待期間 Controller CLI 結束：憑證失效
            crate::controller::controller_scope(&state, &record, &request.claims).await?;
        }
        checked_scope = false;
        let events = state.store.controller_events_after(&scope.workspace_id, cursor, BATCH_LIMIT).await?;
        if let Some(last) = events.last() {
            let cursor = last.cursor;
            return Ok(Json(WaitResponse { events, cursor, timed_out: false }));
        }
        if state.clock.now() >= deadline {
            return Ok(Json(WaitResponse { events, cursor, timed_out: true }));
        }
        tokio::select! {
            _ = commits.changed() => {}
            _ = state.clock.sleep_until(deadline) => {}
            // 送端已不存在（Runtime 不會再通知停止）時這個分支停用，繼續等事件或逾時
            Ok(_) = shutdown.wait_for(|stopping| *stopping) => {
                return Err(ApiError {
                    status: StatusCode::SERVICE_UNAVAILABLE,
                    code: "runtime_stopping",
                    message: "Runtime 正在停止，等待已中止".to_string(),
                });
            }
        }
    }
}
