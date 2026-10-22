//! Worker 間訊息的持久化（票 18）：送出、收件匣讀取、Run 啟動時的未讀摘要，以及範圍變更嘗試的事件。
//!
//! 規則都在 Domain（`baton_domain::message`）；這裡只做「在同一個交易中載入狀態 → 交給 Domain 判定 → 寫入」。
//! 範圍（Workspace、Run、世代）一律來自憑證；收件匣以憑證所屬 Run 的 Task 為界，沒有任何查詢接受別人的 Task。

use baton_domain::message::{
    self, Delivery, InboxMessage, Message, MessageError, MessageKind, ScopeChangeAttempt, SendContext, SendDecision,
    Sender, StoredMessage, UnreadSummaryItem, UNREAD_SUMMARY_MAX_ITEMS,
};
use baton_domain::run;

use super::tasks::load;
use super::*;

/// 送出或讀取失敗：Domain 拒絕、憑證所屬的 Run 不存在，或儲存層錯誤。
#[derive(Debug, thiserror::Error)]
pub enum MessageApplyError {
    #[error(transparent)]
    Rejected(#[from] MessageError),
    #[error("找不到 Run：{0}")]
    RunNotFound(String),
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl From<rusqlite::Error> for MessageApplyError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error.into())
    }
}

/// 送出的結果：訊息與每個收件人目前的交付狀態；`duplicate` 表示這是重送，什麼都沒有新增。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendOutcome {
    pub stored: StoredMessage,
    pub duplicate: bool,
}

/// `read_inbox` 的結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inbox {
    pub task_id: String,
    pub messages: Vec<InboxMessage>,
    /// 這次讀取之後，這個 Task 仍未讀的訊息數。
    pub unread_remaining: usize,
}

/// 憑證所屬的寄件者／讀取者，以及它此刻是否還是 Task 的寫入所有者；Run 不存在時回報 `RunNotFound`。
fn sender_of(tx: &Connection, scope: &AgentScope) -> Result<(Sender, Vec<String>), MessageApplyError> {
    let loaded = load(tx, &scope.workspace_id)?;
    let Some(run) = loaded.runs.iter().find(|r| r.id == scope.run_id) else {
        return Err(MessageApplyError::RunNotFound(scope.run_id.clone()));
    };
    let lease = loaded.leases.iter().find(|l| l.run_id == run.id);
    let sender = Sender {
        workspace_id: scope.workspace_id.clone(),
        run_id: run.id.clone(),
        task_id: run.task_id.clone(),
        generation: scope.generation,
        is_current: run::is_current_writer(run, lease, scope.generation),
    };
    Ok((sender, loaded.tasks.iter().map(|t| t.id.clone()).collect()))
}

impl Store {
    /// 送出一則已驗證的訊息。`generated_id` 在請求沒有指定 `message_id` 時使用。
    /// 同一個寄件 Task 以同一個 `message_id` 重送相同內容時回傳既有的訊息（`duplicate`），不新增任何交付。
    pub async fn send_message(
        &self,
        scope: &AgentScope,
        input: message::NewMessage,
        generated_id: String,
        now: Timestamp,
    ) -> Result<SendOutcome, MessageApplyError> {
        let scope = scope.clone();
        self.write(move |tx| {
            let (sender, task_ids) = match sender_of(tx, &scope) {
                Ok(found) => found,
                Err(MessageApplyError::Store(store)) => return Err(store),
                Err(other) => return Ok(Err(other)),
            };
            let existing = match &input.message_id {
                Some(id) => load_message(tx, &scope.workspace_id, id)?,
                None => None,
            };
            let parent = match &input.in_reply_to {
                Some(id) => load_message(tx, &scope.workspace_id, id)?,
                None => None,
            };
            let ctx = SendContext {
                sender: &sender,
                workspace_task_ids: &task_ids,
                existing: existing.as_ref(),
                parent: parent.as_ref(),
            };
            Ok(match message::decide_send(&ctx, input, generated_id, now) {
                Err(rejected) => Err(rejected.into()),
                Ok(SendDecision::Duplicate(stored)) => Ok(SendOutcome { stored, duplicate: true }),
                Ok(SendDecision::New(stored)) => {
                    save_message(tx, &stored)?;
                    Ok(SendOutcome { stored, duplicate: false })
                }
            })
        })
        .await?
    }

    /// 範圍變更嘗試：訊息不儲存也不交付，只追加一個給 Controller 的事件。寄件者已不是寫入所有者時同樣被拒絕。
    pub async fn record_scope_change_attempt(
        &self,
        scope: &AgentScope,
        attempt: ScopeChangeAttempt,
        attempt_id: String,
        now: Timestamp,
    ) -> Result<(), MessageApplyError> {
        let scope = scope.clone();
        self.write(move |tx| {
            let (sender, _) = match sender_of(tx, &scope) {
                Ok(found) => found,
                Err(MessageApplyError::Store(store)) => return Err(store),
                Err(other) => return Ok(Err(other)),
            };
            if !sender.is_current {
                return Ok(Err(MessageError::StaleSender.into()));
            }
            append_event(tx, &attempt.into_event(&sender, attempt_id, now))?;
            Ok(Ok(()))
        })
        .await?
    }

    /// 讀取寄給憑證所屬 Task 的訊息，依送出順序；回傳的未讀訊息會被標為已讀。
    /// Run 已結束或被接替時拒絕（`MessageError::Revoked`），不改變任何已讀狀態。
    pub async fn read_inbox(
        &self,
        scope: &AgentScope,
        include_read: bool,
        limit: usize,
        now: Timestamp,
    ) -> Result<Inbox, MessageApplyError> {
        let scope = scope.clone();
        self.write(move |tx| {
            let (reader, _) = match sender_of(tx, &scope) {
                Ok(found) => found,
                Err(MessageApplyError::Store(store)) => return Err(store),
                Err(other) => return Ok(Err(other)),
            };
            if !reader.is_current {
                return Ok(Err(MessageError::Revoked.into()));
            }
            let filter = if include_read { "" } else { "AND d.read_at IS NULL" };
            let sql = format!(
                "SELECT m.message_id, m.thread_id, m.correlation_id, m.in_reply_to, m.kind, m.subject, m.body,
                        m.sender_task_id, t.title, m.created_at, d.read_at IS NOT NULL
                 FROM message_delivery d
                 JOIN message m ON m.workspace_id = d.workspace_id AND m.message_id = d.message_id
                 JOIN task t ON t.id = m.sender_task_id
                 WHERE d.workspace_id = ?1 AND d.recipient_task_id = ?2 {filter}
                 ORDER BY m.rowid LIMIT ?3"
            );
            let messages = tx
                .prepare(&sql)?
                .query_map(rusqlite::params![scope.workspace_id, reader.task_id, limit as i64], |r| {
                    Ok(InboxMessage {
                        message_id: r.get(0)?,
                        thread_id: r.get(1)?,
                        correlation_id: r.get(2)?,
                        in_reply_to: r.get(3)?,
                        kind: kind_column(4, &r.get::<_, String>(4)?)?,
                        subject: r.get(5)?,
                        body: r.get(6)?,
                        from_task_id: r.get(7)?,
                        from_task_title: r.get(8)?,
                        sent_at: Timestamp::from_millis(r.get(9)?),
                        already_read: r.get(10)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for message in messages.iter().filter(|m| !m.already_read) {
                tx.execute(
                    "UPDATE message_delivery SET read_at = ?4, delivered_at = COALESCE(delivered_at, ?4)
                     WHERE workspace_id = ?1 AND message_id = ?2 AND recipient_task_id = ?3",
                    rusqlite::params![scope.workspace_id, message.message_id, reader.task_id, now.as_millis()],
                )?;
            }
            let unread_remaining: i64 = tx.query_row(
                "SELECT COUNT(*) FROM message_delivery WHERE workspace_id = ?1 AND recipient_task_id = ?2 AND read_at IS NULL",
                rusqlite::params![scope.workspace_id, reader.task_id],
                |r| r.get(0),
            )?;
            Ok(Ok(Inbox { task_id: reader.task_id, messages, unread_remaining: unread_remaining as usize }))
        })
        .await?
    }
}

/// 收件 Task 的未讀摘要（最多 [`UNREAD_SUMMARY_MAX_ITEMS`] 則，依送達順序）與實際的未讀總數；派工時由 Context Builder 放進 Prompt。
pub(crate) fn unread_summary(
    tx: &Transaction<'_>,
    workspace_id: &str,
    task_id: &str,
) -> rusqlite::Result<(Vec<UnreadSummaryItem>, usize)> {
    let items = tx
        .prepare(
            "SELECT m.message_id, m.kind, m.subject, t.title
             FROM message_delivery d
             JOIN message m ON m.workspace_id = d.workspace_id AND m.message_id = d.message_id
             JOIN task t ON t.id = m.sender_task_id
             WHERE d.workspace_id = ?1 AND d.recipient_task_id = ?2 AND d.read_at IS NULL
             ORDER BY m.rowid LIMIT ?3",
        )?
        .query_map(rusqlite::params![workspace_id, task_id, UNREAD_SUMMARY_MAX_ITEMS as i64], |r| {
            Ok(UnreadSummaryItem {
                message_id: r.get(0)?,
                kind: kind_column(1, &r.get::<_, String>(1)?)?,
                subject: r.get(2)?,
                from_task_title: r.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let total: i64 = tx.query_row(
        "SELECT COUNT(*) FROM message_delivery WHERE workspace_id = ?1 AND recipient_task_id = ?2 AND read_at IS NULL",
        rusqlite::params![workspace_id, task_id],
        |r| r.get(0),
    )?;
    Ok((items, total as usize))
}

/// 摘要放進 Prompt 後，把摘要裡列出的訊息標為已交付。
pub(crate) fn mark_summary_delivered(
    tx: &Transaction<'_>,
    workspace_id: &str,
    task_id: &str,
    items: &[UnreadSummaryItem],
    now: Timestamp,
) -> rusqlite::Result<()> {
    for item in items {
        tx.execute(
            "UPDATE message_delivery SET delivered_at = ?4
             WHERE workspace_id = ?1 AND message_id = ?2 AND recipient_task_id = ?3 AND delivered_at IS NULL",
            rusqlite::params![workspace_id, item.message_id, task_id, now.as_millis()],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;

fn kind_column(index: usize, raw: &str) -> rusqlite::Result<MessageKind> {
    MessageKind::parse(raw).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Text, "未知的訊息類型".into())
    })
}

fn load_message(conn: &Connection, workspace_id: &str, message_id: &str) -> rusqlite::Result<Option<StoredMessage>> {
    let message = conn
        .query_row(
            "SELECT message_id, workspace_id, thread_id, correlation_id, in_reply_to, kind, subject, body,
                    sender_run_id, sender_task_id, sender_generation, created_at
             FROM message WHERE workspace_id = ?1 AND message_id = ?2",
            [workspace_id, message_id],
            |r| {
                Ok(Message {
                    message_id: r.get(0)?,
                    workspace_id: r.get(1)?,
                    thread_id: r.get(2)?,
                    correlation_id: r.get(3)?,
                    in_reply_to: r.get(4)?,
                    kind: kind_column(5, &r.get::<_, String>(5)?)?,
                    subject: r.get(6)?,
                    body: r.get(7)?,
                    sender_run_id: r.get(8)?,
                    sender_task_id: r.get(9)?,
                    sender_generation: r.get(10)?,
                    created_at: Timestamp::from_millis(r.get(11)?),
                })
            },
        )
        .map(Some)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(other),
        })?;
    let Some(message) = message else { return Ok(None) };
    let deliveries = conn
        .prepare(
            "SELECT recipient_task_id, delivered_at, read_at FROM message_delivery
             WHERE workspace_id = ?1 AND message_id = ?2 ORDER BY rowid",
        )?
        .query_map([workspace_id, message_id], |r| {
            Ok(Delivery {
                message_id: message_id.to_string(),
                recipient_task_id: r.get(0)?,
                delivered_at: r.get::<_, Option<i64>>(1)?.map(Timestamp::from_millis),
                read_at: r.get::<_, Option<i64>>(2)?.map(Timestamp::from_millis),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Some(StoredMessage { message, deliveries }))
}

fn save_message(tx: &Transaction<'_>, stored: &StoredMessage) -> rusqlite::Result<()> {
    let m = &stored.message;
    tx.execute(
        "INSERT INTO message (workspace_id, message_id, thread_id, correlation_id, in_reply_to, kind, subject, body,
                              sender_run_id, sender_task_id, sender_generation, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        rusqlite::params![
            m.workspace_id,
            m.message_id,
            m.thread_id,
            m.correlation_id,
            m.in_reply_to,
            m.kind.as_str(),
            m.subject,
            m.body,
            m.sender_run_id,
            m.sender_task_id,
            m.sender_generation,
            m.created_at.as_millis(),
        ],
    )?;
    for delivery in &stored.deliveries {
        tx.execute(
            "INSERT INTO message_delivery (workspace_id, message_id, recipient_task_id) VALUES (?1, ?2, ?3)",
            rusqlite::params![m.workspace_id, delivery.message_id, delivery.recipient_task_id],
        )?;
    }
    Ok(())
}
