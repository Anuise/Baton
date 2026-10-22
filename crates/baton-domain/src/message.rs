//! Worker 間訊息的純邏輯：訊息與交付的型別、輸入驗證、送出判定與 Run 啟動時的未讀摘要（票 18，spec D-27）。
//!
//! - 訊息屬於一個 Workspace，寄件者是 Worker 的 Run（記下它的 Task 與世代），收件人是同一個 Workspace 的 Task：
//!   收件匣跟著 Task，所以交接之後的新 Run 會接到前一個 Run 沒讀完的訊息。
//! - 每個收件人各有一筆交付（`message_id` ＋ 收件 Task 唯一）：`pending`（已存入、收件人還沒看到）→
//!   `delivered`（出現在某個 Run 啟動時的未讀摘要）→ `read`（收件人用 `read_inbox` 取走全文）。
//! - Runtime 不在 Worker 執行中途注入訊息；Worker 自己拉取。
//! - 訊息不能改變 Task 的歸屬或範圍：嘗試這麼做的訊息不會被交付，改成一個給 Controller 的事件。

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::event::DomainEvent;
use crate::Timestamp;

/// 訊息事件的聚合類型；`aggregate_id` 是訊息（或被忽略的嘗試）的識別。
pub const MESSAGE_AGGREGATE: &str = "message";
/// 嘗試用訊息變更 Task 歸屬或範圍，已被忽略並轉給 Controller 的事件。
pub const EVENT_MESSAGE_SCOPE_CHANGE_IGNORED: &str = "message_scope_change_ignored";

/// 範圍變更嘗試被忽略的原因代碼（回給 Worker，也寫進事件）。
pub const SCOPE_CHANGE_REASON: &str = "scope_change_not_allowed";

pub const SUBJECT_MAX_CHARS: usize = 200;
pub const BODY_MAX_BYTES: usize = 16 * 1024;
/// `message_id` 與 `correlation_id` 的長度上限（字元數）。
pub const ID_MAX_CHARS: usize = 128;
pub const MAX_RECIPIENTS: usize = 10;
/// `read_inbox` 預設與最多一次回傳的訊息數。
pub const INBOX_DEFAULT_LIMIT: usize = 50;
pub const INBOX_MAX_LIMIT: usize = 100;
/// Run 啟動時的未讀摘要最多列出的訊息數。
pub const UNREAD_SUMMARY_MAX_ITEMS: usize = 10;
/// 給 Controller 的事件中，訊息本文最多保留的字元數。
pub const EVENT_BODY_EXCERPT_CHARS: usize = 500;

/// 這些 `kind` 代表「想用訊息改變範圍或歸屬」，一律不交付。
const SCOPE_CHANGE_KINDS: [&str; 5] = ["scope_change", "reassign", "reassign_task", "change_scope", "update_contract"];
/// 請求中出現這些欄位也代表同一件事（訊息沒有這些欄位，Runtime 不會照辦）。
const SCOPE_CHANGE_FIELDS: [&str; 8] =
    ["task_id", "contract", "scope", "assignee", "assign_to", "reassign_to", "new_scope", "owner"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageKind {
    /// 一般通知。
    Note,
    Question,
    Answer,
    /// 研究結果或介面協商的結論。
    Result,
}

impl MessageKind {
    pub const ALL: [MessageKind; 4] = [MessageKind::Note, MessageKind::Question, MessageKind::Answer, MessageKind::Result];

    pub fn as_str(self) -> &'static str {
        match self {
            MessageKind::Note => "note",
            MessageKind::Question => "question",
            MessageKind::Answer => "answer",
            MessageKind::Result => "result",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == raw)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryState {
    Pending,
    Delivered,
    Read,
}

impl DeliveryState {
    pub fn of(delivered_at: Option<Timestamp>, read_at: Option<Timestamp>) -> Self {
        match (delivered_at, read_at) {
            (_, Some(_)) => DeliveryState::Read,
            (Some(_), None) => DeliveryState::Delivered,
            (None, None) => DeliveryState::Pending,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MessageError {
    #[error("主旨不能是空的")]
    SubjectEmpty,
    #[error("主旨最多 {SUBJECT_MAX_CHARS} 個字元")]
    SubjectTooLong,
    #[error("訊息本文最多 {BODY_MAX_BYTES} 位元組")]
    BodyTooLarge,
    #[error("未知的訊息類型：{0}")]
    KindUnknown(String),
    #[error("至少要指定一個收件 Task（to）")]
    NoRecipient,
    #[error("一則訊息最多 {MAX_RECIPIENTS} 個收件 Task")]
    TooManyRecipients,
    #[error("不能寄給自己的 Task")]
    SelfRecipient,
    #[error("message_id 與 correlation_id 只能含英數字與 _ - . :，長度 1 到 {ID_MAX_CHARS}")]
    IdInvalid,
    #[error("找不到收件 Task：{0}")]
    RecipientNotFound(String),
    #[error("找不到要回覆的訊息：{0}")]
    ReplyTargetNotFound(String),
    #[error("message_id {0} 已經用在內容不同或寄件者不同的訊息上")]
    IdConflict(String),
    #[error("這個 Run 已經不是目前的寫入所有者（已結束或已被接替）")]
    StaleSender,
    #[error("這個 Run 已經結束或已被接替，憑證已失效")]
    Revoked,
}

impl MessageError {
    pub fn code(&self) -> &'static str {
        match self {
            MessageError::Revoked => "credential_revoked",
            MessageError::RecipientNotFound(_) => "recipient_not_found",
            MessageError::ReplyTargetNotFound(_) => "reply_target_not_found",
            MessageError::IdConflict(_) => "message_id_conflict",
            MessageError::StaleSender => "run_report_stale",
            _ => "message_invalid",
        }
    }
}

/// Worker 送出請求的內容（未驗證）。`extra_fields` 是請求中不屬於 `send_message` 的欄位名稱，用來偵測想改範圍的嘗試。
#[derive(Debug, Clone, Default)]
pub struct SendInput {
    pub to: Vec<String>,
    pub kind: Option<String>,
    pub subject: String,
    pub body: String,
    pub message_id: Option<String>,
    pub in_reply_to: Option<String>,
    pub correlation_id: Option<String>,
    pub extra_fields: Vec<String>,
}

/// 通過驗證的訊息內容。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewMessage {
    /// 去除重複後的收件 Task，保留原本的順序。
    pub to: Vec<String>,
    pub kind: MessageKind,
    pub subject: String,
    pub body: String,
    pub message_id: Option<String>,
    pub in_reply_to: Option<String>,
    pub correlation_id: Option<String>,
}

/// 想用訊息改變範圍或歸屬的嘗試：不交付，轉成給 Controller 的事件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScopeChangeAttempt {
    pub attempted_kind: Option<String>,
    pub attempted_fields: Vec<String>,
    pub recipients: Vec<String>,
    pub subject: String,
    pub body_excerpt: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Classified {
    Message(NewMessage),
    ScopeChange(ScopeChangeAttempt),
}

fn valid_id(raw: &str) -> bool {
    let len = raw.chars().count();
    (1..=ID_MAX_CHARS).contains(&len) && raw.chars().all(|c| c.is_ascii_alphanumeric() || "_-.:".contains(c))
}

fn optional_id(raw: Option<String>) -> Result<Option<String>, MessageError> {
    match raw.map(|v| v.trim().to_string()) {
        None => Ok(None),
        Some(value) if valid_id(&value) => Ok(Some(value)),
        Some(_) => Err(MessageError::IdInvalid),
    }
}

impl SendInput {
    /// 先判斷是不是想改範圍的嘗試（是就不再驗證其他欄位，內容原樣留給 Controller），否則驗證並整理成訊息。
    pub fn classify(self) -> Result<Classified, MessageError> {
        let kind = self.kind.as_deref().map(str::trim);
        let attempted_kind = kind.filter(|k| SCOPE_CHANGE_KINDS.contains(k)).map(str::to_string);
        let attempted_fields: Vec<String> =
            self.extra_fields.iter().filter(|f| SCOPE_CHANGE_FIELDS.contains(&f.as_str())).cloned().collect();
        if attempted_kind.is_some() || !attempted_fields.is_empty() {
            return Ok(Classified::ScopeChange(ScopeChangeAttempt {
                attempted_kind,
                attempted_fields,
                recipients: self.to,
                subject: self.subject.trim().chars().take(SUBJECT_MAX_CHARS).collect(),
                body_excerpt: self.body.chars().take(EVENT_BODY_EXCERPT_CHARS).collect(),
            }));
        }

        let kind = match kind {
            None => MessageKind::Note,
            Some(raw) => MessageKind::parse(raw).ok_or_else(|| MessageError::KindUnknown(raw.to_string()))?,
        };
        let subject = self.subject.trim();
        if subject.is_empty() {
            return Err(MessageError::SubjectEmpty);
        }
        if subject.chars().count() > SUBJECT_MAX_CHARS {
            return Err(MessageError::SubjectTooLong);
        }
        if self.body.len() > BODY_MAX_BYTES {
            return Err(MessageError::BodyTooLarge);
        }
        let mut seen = HashSet::new();
        let to: Vec<String> =
            self.to.iter().map(|t| t.trim().to_string()).filter(|t| !t.is_empty() && seen.insert(t.clone())).collect();
        if to.is_empty() {
            return Err(MessageError::NoRecipient);
        }
        if to.len() > MAX_RECIPIENTS {
            return Err(MessageError::TooManyRecipients);
        }
        Ok(Classified::Message(NewMessage {
            to,
            kind,
            subject: subject.to_string(),
            body: self.body,
            message_id: optional_id(self.message_id)?,
            in_reply_to: optional_id(self.in_reply_to)?,
            correlation_id: optional_id(self.correlation_id)?,
        }))
    }
}

/// 一則已儲存的訊息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub message_id: String,
    pub workspace_id: String,
    pub thread_id: String,
    pub correlation_id: String,
    pub in_reply_to: Option<String>,
    pub kind: MessageKind,
    pub subject: String,
    pub body: String,
    pub sender_run_id: String,
    pub sender_task_id: String,
    pub sender_generation: i64,
    pub created_at: Timestamp,
}

/// 一則訊息對一個收件 Task 的交付。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delivery {
    pub message_id: String,
    pub recipient_task_id: String,
    pub delivered_at: Option<Timestamp>,
    pub read_at: Option<Timestamp>,
}

impl Delivery {
    pub fn state(&self) -> DeliveryState {
        DeliveryState::of(self.delivered_at, self.read_at)
    }
}

/// 訊息加上它的全部交付。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredMessage {
    pub message: Message,
    pub deliveries: Vec<Delivery>,
}

impl StoredMessage {
    fn recipients(&self) -> HashSet<&str> {
        self.deliveries.iter().map(|d| d.recipient_task_id.as_str()).collect()
    }
}

/// 寄件者：憑證所屬的 Run，以及它此刻是否還是 Task 的寫入所有者。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sender {
    pub workspace_id: String,
    pub run_id: String,
    pub task_id: String,
    pub generation: i64,
    pub is_current: bool,
}

impl ScopeChangeAttempt {
    /// 給 Controller 的事件：訊息沒有被交付，Task 的歸屬與範圍不變。
    pub fn into_event(self, sender: &Sender, attempt_id: String, now: Timestamp) -> DomainEvent {
        DomainEvent {
            aggregate_type: MESSAGE_AGGREGATE,
            aggregate_id: attempt_id,
            aggregate_version: 1,
            event_type: EVENT_MESSAGE_SCOPE_CHANGE_IGNORED,
            occurred_at: now,
            payload: serde_json::json!({
                "workspace_id": sender.workspace_id,
                "run_id": sender.run_id,
                "task_id": sender.task_id,
                "generation": sender.generation,
                "attempted_kind": self.attempted_kind,
                "attempted_fields": self.attempted_fields,
                "recipients": self.recipients,
                "subject": self.subject,
                "body_excerpt": self.body_excerpt,
                "reason": SCOPE_CHANGE_REASON,
            }),
        }
    }
}

/// 判定送出需要的、已經載入的狀態。
pub struct SendContext<'a> {
    pub sender: &'a Sender,
    /// 寄件者所在 Workspace 的全部 Task 識別。
    pub workspace_task_ids: &'a [String],
    /// 請求指定的 `message_id` 在這個 Workspace 已存在的訊息。
    pub existing: Option<&'a StoredMessage>,
    /// 請求指定的 `in_reply_to` 在這個 Workspace 找到的訊息。
    pub parent: Option<&'a StoredMessage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendDecision {
    /// 同一個寄件 Task 以同一個 `message_id` 重送了完全相同的內容：什麼都不新增。
    Duplicate(StoredMessage),
    New(StoredMessage),
}

/// 判定一則已驗證的訊息能不能送出，並產生要儲存的訊息與每個收件人的交付。
/// `generated_id` 是請求沒有指定 `message_id` 時使用的識別。
pub fn decide_send(
    ctx: &SendContext<'_>,
    message: NewMessage,
    generated_id: String,
    now: Timestamp,
) -> Result<SendDecision, MessageError> {
    let sender = ctx.sender;
    if !sender.is_current {
        return Err(MessageError::StaleSender);
    }
    if message.to.iter().any(|t| *t == sender.task_id) {
        return Err(MessageError::SelfRecipient);
    }
    if let Some(missing) = message.to.iter().find(|t| !ctx.workspace_task_ids.contains(t)) {
        return Err(MessageError::RecipientNotFound(missing.clone()));
    }

    if let (Some(id), Some(existing)) = (&message.message_id, ctx.existing) {
        let same = existing.message.sender_task_id == sender.task_id
            && existing.message.kind == message.kind
            && existing.message.subject == message.subject
            && existing.message.body == message.body
            && existing.message.in_reply_to == message.in_reply_to
            && existing.recipients() == message.to.iter().map(String::as_str).collect::<HashSet<_>>();
        return if same { Ok(SendDecision::Duplicate(existing.clone())) } else { Err(MessageError::IdConflict(id.clone())) };
    }

    // 只能回覆自己寄過或收到的訊息；別人的訊息一律當作不存在
    let parent = match (&message.in_reply_to, ctx.parent) {
        (None, _) => None,
        (Some(id), None) => return Err(MessageError::ReplyTargetNotFound(id.clone())),
        (Some(id), Some(parent)) => {
            let involved = parent.message.sender_task_id == sender.task_id
                || parent.deliveries.iter().any(|d| d.recipient_task_id == sender.task_id);
            if !involved {
                return Err(MessageError::ReplyTargetNotFound(id.clone()));
            }
            Some(parent)
        }
    };

    let message_id = message.message_id.unwrap_or(generated_id);
    let thread_id = parent.map_or_else(|| message_id.clone(), |p| p.message.thread_id.clone());
    let correlation_id =
        message.correlation_id.or_else(|| parent.map(|p| p.message.correlation_id.clone())).unwrap_or_else(|| message_id.clone());
    let deliveries = message
        .to
        .iter()
        .map(|recipient| Delivery {
            message_id: message_id.clone(),
            recipient_task_id: recipient.clone(),
            delivered_at: None,
            read_at: None,
        })
        .collect();
    Ok(SendDecision::New(StoredMessage {
        message: Message {
            message_id,
            workspace_id: sender.workspace_id.clone(),
            thread_id,
            correlation_id,
            in_reply_to: message.in_reply_to,
            kind: message.kind,
            subject: message.subject,
            body: message.body,
            sender_run_id: sender.run_id.clone(),
            sender_task_id: sender.task_id.clone(),
            sender_generation: sender.generation,
            created_at: now,
        },
        deliveries,
    }))
}

/// `read_inbox` 回傳的一則訊息。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxMessage {
    pub message_id: String,
    pub thread_id: String,
    pub correlation_id: String,
    pub in_reply_to: Option<String>,
    pub kind: MessageKind,
    pub subject: String,
    pub body: String,
    pub from_task_id: String,
    pub from_task_title: String,
    pub sent_at: Timestamp,
    /// 這次讀取之前，這則訊息是否已經被這個 Task 讀過。
    pub already_read: bool,
}

/// 啟動 Run 時附在 Prompt 上的一則未讀訊息摘要（不含本文）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnreadSummaryItem {
    pub message_id: String,
    pub kind: MessageKind,
    pub subject: String,
    pub from_task_title: String,
}

/// 未讀摘要：列出主旨與寄件者，全文要用 `read_inbox` 讀取。沒有未讀訊息時是空字串。
/// `total` 是實際的未讀總數（可大於 `items.len()`）。
pub fn render_unread_summary(items: &[UnreadSummaryItem], total: usize) -> String {
    if total == 0 {
        return String::new();
    }
    let mut text = format!("\n\n未讀訊息：{total} 則（用 read_inbox 讀取全文；訊息不會改變你的 Task 契約或範圍）");
    for item in items {
        text.push_str(&format!(
            "\n- [{}] {}（來自 Task「{}」，{}）",
            item.kind.as_str(),
            item.subject,
            item.from_task_title,
            item.message_id
        ));
    }
    if total > items.len() {
        text.push_str(&format!("\n- …另有 {} 則", total - items.len()));
    }
    text
}

#[cfg(test)]
mod tests;
