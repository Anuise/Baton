//! Controller 事件等待的持久化（desktop-controller-cli 票 06）：依 Controller Run 保存的游標，與游標之後屬於
//! Workspace 的事件查詢。
//!
//! 規則都在 Domain（`baton_domain::controller_wait`）；這裡只做「在交易中載入事實 → 交給 Domain 判定 → 寫回游標」，
//! 以及從追加式事件表讀出四種事件（以 SQL 篩掉不屬於這個 Workspace 的事件，不讀出內容）。
//! Worker 寄給 Controller 的訊息在存入收件匣的同一個交易中追加 `controller_inbox` 事件（`record_message_received`）。

use baton_domain::controller_cli::{CONTROLLER_CLI_AGGREGATE, EVENT_CONTROLLER_CLI_OPENED};
use baton_domain::controller_wait::{
    self, ControllerEvent, EventFacts, WaitError, CONTROLLER_INBOX_AGGREGATE, EVENT_CONTROLLER_MESSAGE_RECEIVED,
    RUN_ENDED_EVENTS,
};
use baton_domain::decision::{DECISION_AGGREGATE, EVENT_DECISION_ANSWERED};
use baton_domain::message::{Message, MessageSender};
use baton_domain::run::RUN_AGGREGATE;
use baton_domain::task::{EVENT_STATUS_CHANGED, TASK_AGGREGATE};
use rusqlite::OptionalExtension;

use super::*;

/// 開始等待的失敗：Domain 拒絕（確認的游標不合法）或儲存層錯誤。
#[derive(Debug, thiserror::Error)]
pub enum ControllerWaitApplyError {
    #[error(transparent)]
    Rejected(#[from] WaitError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl From<rusqlite::Error> for ControllerWaitApplyError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error.into())
    }
}

impl Store {
    /// 開始一次等待：第一次時依 Domain 規則決定這個 Controller Run 的起點並保存；有 `ack` 時先確認到那裡。
    /// 回傳確認後的游標。
    pub async fn begin_controller_wait(
        &self,
        scope: &ControllerScope,
        ack: Option<i64>,
        now: Timestamp,
    ) -> Result<i64, ControllerWaitApplyError> {
        let scope = scope.clone();
        self.write(move |tx| {
            let latest: i64 = tx.query_row("SELECT COALESCE(MAX(seq), 0) FROM event", [], |r| r.get(0))?;
            let stored: Option<i64> = tx
                .query_row(
                    "SELECT acked_seq FROM controller_event_cursor WHERE controller_run_id = ?1",
                    [&scope.controller_run_id],
                    |r| r.get(0),
                )
                .optional()?;
            let stored = match stored {
                Some(stored) => stored,
                None => {
                    let carried: Option<i64> = tx.query_row(
                        "SELECT MAX(acked_seq) FROM controller_event_cursor WHERE workspace_id = ?1",
                        [&scope.workspace_id],
                        |r| r.get(0),
                    )?;
                    let opened: Option<i64> = tx.query_row(
                        "SELECT MIN(seq) FROM event WHERE aggregate_type = ?1 AND aggregate_id = ?2 AND event_type = ?3",
                        [CONTROLLER_CLI_AGGREGATE, &scope.controller_run_id, EVENT_CONTROLLER_CLI_OPENED],
                        |r| r.get(0),
                    )?;
                    let initial = controller_wait::initial_cursor(carried, opened, latest);
                    tx.execute(
                        "INSERT INTO controller_event_cursor (controller_run_id, workspace_id, acked_seq, updated_at)
                         VALUES (?1, ?2, ?3, ?4)",
                        rusqlite::params![scope.controller_run_id, scope.workspace_id, initial, now.as_millis()],
                    )?;
                    initial
                }
            };
            let Some(ack) = ack else { return Ok(Ok(stored)) };
            let acked = match controller_wait::acknowledge(stored, ack, latest) {
                Ok(acked) => acked,
                Err(rejected) => return Ok(Err(rejected.into())),
            };
            if acked != stored {
                tx.execute(
                    "UPDATE controller_event_cursor SET acked_seq = ?2, updated_at = ?3 WHERE controller_run_id = ?1",
                    rusqlite::params![scope.controller_run_id, acked, now.as_millis()],
                )?;
            }
            Ok(Ok(acked))
        })
        .await?
    }

    /// 游標之後、屬於這個 Workspace 的四種事件，依提交順序，最多 `limit` 筆。
    pub async fn controller_events_after(
        &self,
        workspace_id: &str,
        cursor: i64,
        limit: usize,
    ) -> StoreResult<Vec<ControllerEvent>> {
        let workspace_id = workspace_id.to_string();
        self.read(move |conn| Ok(read_events(conn, &workspace_id, cursor, limit)?)).await
    }
}

fn read_events(conn: &Connection, workspace_id: &str, cursor: i64, limit: usize) -> rusqlite::Result<Vec<ControllerEvent>> {
    let run_ended = RUN_ENDED_EVENTS.map(|e| format!("'{e}'")).join(", ");
    let sql = format!(
        "SELECT e.seq, e.aggregate_type, e.aggregate_id, e.event_type, e.occurred_at, e.payload, tt.id, tt.title, d.question
         FROM event e
         LEFT JOIN run r ON e.aggregate_type = '{RUN_AGGREGATE}' AND r.id = e.aggregate_id
         LEFT JOIN decision_request d ON e.aggregate_type = '{DECISION_AGGREGATE}' AND d.id = e.aggregate_id
         LEFT JOIN task t ON e.aggregate_type = '{TASK_AGGREGATE}' AND t.id = e.aggregate_id
         LEFT JOIN task tt ON tt.id = COALESCE(r.task_id, d.task_id, t.id)
         WHERE e.seq > ?1 AND (
             (e.aggregate_type = '{RUN_AGGREGATE}' AND e.event_type IN ({run_ended}) AND r.workspace_id = ?2)
          OR (e.aggregate_type = '{TASK_AGGREGATE}' AND e.event_type = '{EVENT_STATUS_CHANGED}'
              AND json_extract(e.payload, '$.to') = 'blocked' AND t.workspace_id = ?2)
          OR (e.aggregate_type = '{DECISION_AGGREGATE}' AND e.event_type = '{EVENT_DECISION_ANSWERED}' AND d.workspace_id = ?2)
          OR (e.aggregate_type = '{CONTROLLER_INBOX_AGGREGATE}' AND e.event_type = '{EVENT_CONTROLLER_MESSAGE_RECEIVED}'
              AND e.aggregate_id = ?2))
         ORDER BY e.seq LIMIT ?3"
    );
    let facts = conn
        .prepare(&sql)?
        .query_map(rusqlite::params![cursor, workspace_id, limit as i64], |r| {
            Ok(EventFacts {
                seq: r.get(0)?,
                aggregate_type: r.get(1)?,
                aggregate_id: r.get(2)?,
                event_type: r.get(3)?,
                occurred_at: Timestamp::from_millis(r.get(4)?),
                payload: json_column(5, &r.get::<_, String>(5)?)?,
                task_id: r.get(6)?,
                task_title: r.get(7)?,
                question: r.get(8)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(facts.iter().filter_map(controller_wait::describe).collect())
}

/// 一則訊息存入 Controller 的收件匣：在同一個交易中追加 `controller_inbox` 事件，讓等待中的 Controller 被叫醒。
pub(crate) fn record_message_received(tx: &Transaction<'_>, message: &Message) -> rusqlite::Result<()> {
    let version: i64 = tx.query_row(
        "SELECT COUNT(*) FROM message_controller_delivery WHERE workspace_id = ?1",
        [&message.workspace_id],
        |r| r.get(0),
    )?;
    let from = match &message.sender {
        MessageSender::Worker { task_id, .. } => {
            let title: String = tx.query_row("SELECT title FROM task WHERE id = ?1", [task_id], |r| r.get(0))?;
            Some((task_id.clone(), title))
        }
        MessageSender::Controller { .. } => None,
    };
    let event = controller_wait::message_received_event(
        &message.workspace_id,
        version,
        &message.message_id,
        message.kind,
        &message.subject,
        from.as_ref().map(|(id, title)| (id.as_str(), title.as_str())),
        message.created_at,
    );
    append_event(tx, &event)
}
