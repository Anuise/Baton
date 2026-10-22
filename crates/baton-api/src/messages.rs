//! Worker 間訊息的 Agent API（票 18，spec D-27）：`send_message` 與 `read_inbox`。
//!
//! 身分與範圍一律來自憑證；收件匣只屬於憑證所屬 Run 的 Task。Worker 在 checkpoint 與工具呼叫之間自己拉取，
//! Runtime 不在執行中途插話。訊息不能改變 Task 的歸屬或範圍：想這麼做的請求會被忽略（200，`ignored: true`），
//! 內容轉成一個給 Controller 的事件。

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::State;
use axum::http::StatusCode;
use axum::{Extension, Json};
use baton_domain::auth::CredentialRecord;
use baton_domain::message::{
    Classified, DeliveryState, InboxMessage, MessageError, SendInput, INBOX_DEFAULT_LIMIT, INBOX_MAX_LIMIT,
    SCOPE_CHANGE_REASON,
};
use baton_store::MessageApplyError;
use serde::de::IgnoredAny;
use serde::{Deserialize, Serialize};

use crate::agent::{scope_of, Claims};
use crate::authorization::bad_json;
use crate::workspaces::{new_id, ApiError};
use crate::ApiState;

#[derive(Debug, Deserialize)]
pub(crate) struct SendRequest {
    #[serde(default)]
    to: Vec<String>,
    #[serde(default)]
    kind: Option<String>,
    subject: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    message_id: Option<String>,
    #[serde(default)]
    in_reply_to: Option<String>,
    #[serde(default)]
    correlation_id: Option<String>,
    #[serde(flatten)]
    claims: Claims,
    /// 其餘的欄位：不屬於 `send_message`，只需要名稱，用來偵測想改範圍的嘗試。
    #[serde(flatten)]
    extra: HashMap<String, IgnoredAny>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct InboxRequest {
    #[serde(default)]
    include_read: bool,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(flatten)]
    claims: Claims,
}

/// 每個收件人的交付狀態。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryView {
    pub recipient_task_id: String,
    pub state: DeliveryState,
}

/// `send_message` 的回應。`ignored` 表示訊息想改變範圍或歸屬，沒有被儲存或交付；`duplicate` 表示重送，沒有新增任何交付。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendAck {
    pub accepted: bool,
    pub ignored: bool,
    pub duplicate: bool,
    pub message_id: Option<String>,
    pub thread_id: Option<String>,
    pub correlation_id: Option<String>,
    pub deliveries: Vec<DeliveryView>,
    pub reason: Option<String>,
}

/// `read_inbox` 的回應。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxResponse {
    pub task_id: String,
    pub messages: Vec<InboxMessage>,
    pub unread_remaining: usize,
}

fn api_error(status: StatusCode, code: &'static str, message: &str) -> ApiError {
    ApiError { status, code, message: message.to_string() }
}

impl From<MessageApplyError> for ApiError {
    fn from(error: MessageApplyError) -> Self {
        match error {
            MessageApplyError::Rejected(rejected) => {
                let status = match &rejected {
                    MessageError::RecipientNotFound(_) | MessageError::ReplyTargetNotFound(_) => StatusCode::NOT_FOUND,
                    MessageError::IdConflict(_) | MessageError::StaleSender => StatusCode::CONFLICT,
                    MessageError::Revoked => StatusCode::UNAUTHORIZED,
                    _ => StatusCode::BAD_REQUEST,
                };
                api_error(status, rejected.code(), &rejected.to_string())
            }
            MessageApplyError::RunNotFound(run_id) => {
                api_error(StatusCode::NOT_FOUND, "run_not_found", &format!("找不到 Run：{run_id}"))
            }
            MessageApplyError::Store(store) => store.into(),
        }
    }
}

pub(crate) async fn send_message(
    State(state): State<Arc<ApiState>>,
    Extension(record): Extension<CredentialRecord>,
    body: Result<Json<SendRequest>, JsonRejection>,
) -> Result<Json<SendAck>, ApiError> {
    let Json(request) = body.map_err(bad_json)?;
    let scope = scope_of(&record, &request.claims)?;
    let input = SendInput {
        to: request.to,
        kind: request.kind,
        subject: request.subject,
        body: request.body,
        message_id: request.message_id,
        in_reply_to: request.in_reply_to,
        correlation_id: request.correlation_id,
        extra_fields: request.extra.keys().cloned().collect(),
    };
    let now = state.clock.now();
    match input.classify()? {
        Classified::ScopeChange(attempt) => {
            state.store.record_scope_change_attempt(&scope, attempt, new_id("msg")?, now).await?;
            Ok(Json(SendAck {
                accepted: false,
                ignored: true,
                duplicate: false,
                message_id: None,
                thread_id: None,
                correlation_id: None,
                deliveries: Vec::new(),
                reason: Some(SCOPE_CHANGE_REASON.to_string()),
            }))
        }
        Classified::Message(message) => {
            let outcome = state.store.send_message(&scope, message, new_id("msg")?, now).await?;
            let message = outcome.stored.message;
            Ok(Json(SendAck {
                accepted: true,
                ignored: false,
                duplicate: outcome.duplicate,
                message_id: Some(message.message_id),
                thread_id: Some(message.thread_id),
                correlation_id: Some(message.correlation_id),
                deliveries: outcome
                    .stored
                    .deliveries
                    .iter()
                    .map(|d| DeliveryView { recipient_task_id: d.recipient_task_id.clone(), state: d.state() })
                    .collect(),
                reason: None,
            }))
        }
    }
}

pub(crate) async fn read_inbox(
    State(state): State<Arc<ApiState>>,
    Extension(record): Extension<CredentialRecord>,
    body: Result<Json<InboxRequest>, JsonRejection>,
) -> Result<Json<InboxResponse>, ApiError> {
    let Json(request) = body.map_err(bad_json)?;
    let scope = scope_of(&record, &request.claims)?;
    let limit = request.limit.unwrap_or(INBOX_DEFAULT_LIMIT);
    if !(1..=INBOX_MAX_LIMIT).contains(&limit) {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "message_invalid",
            &format!("limit 必須在 1 到 {INBOX_MAX_LIMIT} 之間"),
        ));
    }
    let inbox = state.store.read_inbox(&scope, request.include_read, limit, state.clock.now()).await?;
    Ok(Json(InboxResponse { task_id: inbox.task_id, messages: inbox.messages, unread_remaining: inbox.unread_remaining }))
}

impl From<MessageError> for ApiError {
    fn from(error: MessageError) -> Self {
        MessageApplyError::Rejected(error).into()
    }
}
