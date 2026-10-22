//! 「對話」端點（桌面角色專用，票 16）：讀取 Workspace 與 Controller 的長期對話、送出使用者訊息。
//!
//! 規則都在 Domain（`baton_domain::conversation`）；送出訊息只保存為「已收到／待處理」，從不同步喚起 Controller，
//! 也不中斷進行中的一輪。喚起由 Supervisor 在合併視窗（spec D-31）到期後進行。暫停與取消不經過這裡（DR-20261022-01）。

use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use axum::Json;
use baton_domain::conversation::{ConversationError, ConversationTurn, ConversationView};
use baton_store::ConversationApplyError;
use serde::{Deserialize, Serialize};

use crate::workspaces::ApiError;
use crate::ApiState;

pub(crate) const CONVERSATION_ROUTE: &str = "/api/v1/workspaces/{id}/conversation";
pub(crate) const MESSAGES_ROUTE: &str = "/api/v1/workspaces/{id}/conversation/messages";

pub fn conversation_path(workspace_id: &str) -> String {
    format!("{}/{workspace_id}/conversation", crate::WORKSPACES_PATH)
}

pub fn conversation_messages_path(workspace_id: &str) -> String {
    format!("{}/messages", conversation_path(workspace_id))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PostMessageRequest {
    pub text: String,
}

/// 送出訊息的回應：保存的 turn（待處理），以及當時 Controller 是否正在處理上一輪。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PostedMessage {
    pub turn: ConversationTurn,
    pub busy: bool,
}

impl From<ConversationApplyError> for ApiError {
    fn from(error: ConversationApplyError) -> Self {
        match error {
            ConversationApplyError::Store(store) => store.into(),
            ConversationApplyError::Rejected(rejected) => {
                let status = match &rejected {
                    ConversationError::WorkspaceNotFound(_) | ConversationError::RunNotFound(_) => StatusCode::NOT_FOUND,
                    ConversationError::MessageEmpty | ConversationError::MessageTooLong => StatusCode::BAD_REQUEST,
                    _ => StatusCode::CONFLICT,
                };
                ApiError { status, code: rejected.code(), message: rejected.to_string() }
            }
        }
    }
}

pub(crate) async fn view(
    State(state): State<Arc<ApiState>>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<ConversationView>, ApiError> {
    Ok(Json(state.store.conversation_view(&id, state.clock.now()).await?))
}

pub(crate) async fn post_message(
    State(state): State<Arc<ApiState>>,
    UrlPath(id): UrlPath<String>,
    body: Result<Json<PostMessageRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<PostedMessage>), ApiError> {
    let Json(request) = body.map_err(crate::authorization::bad_json)?;
    let (turn, busy) = state.store.post_user_message(&id, request.text, state.clock.now()).await?;
    // 合併視窗到期時 Supervisor 會喚起 Controller；背景迴圈每秒檢查一次，這裡只提早叫醒它
    state.supervisor_wake.notify_one();
    Ok((StatusCode::CREATED, Json(PostedMessage { turn, busy })))
}
