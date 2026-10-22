//! 崩潰恢復的持久化（票 31）。
//!
//! 規則都在 Domain（`baton_domain::recovery`）；這裡只做「在同一個交易中載入目前事件 → 交給 Domain 判定 →
//! 寫入事件、嘗試與 Domain 事件」，以及判斷上一次執行是否正常結束（`runtime_boot.ended_cleanly_at`）。
//! 恢復的派工阻擋由 `lifecycle::halt_for`、`lifecycle::launch_halted_in` 在各自的交易中呼叫 `recovery_halt`。

use baton_domain::lifecycle::LifecyclePhase;
use baton_domain::recovery::{
    self, AffectedRun, AttemptOutcome, RecoveryAttempt, RecoveryChange, RecoveryCommand, RecoveryError, RecoveryEvent,
    RecoveryStatus, RUN_STOPPED_BY_RECOVERY_REASON,
};
use baton_domain::run::RunChange;
use baton_domain::scheduler::WaitReason;

use super::runs::RunApplyError;
use super::*;

/// 套用恢復命令的失敗：Domain 拒絕或儲存層錯誤。
#[derive(Debug, thiserror::Error)]
pub enum RecoveryApplyError {
    #[error(transparent)]
    Rejected(#[from] RecoveryError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl From<rusqlite::Error> for RecoveryApplyError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error.into())
    }
}

impl Store {
    /// 上一次啟動是否正常結束；從未啟動過為 `None`。必須在 `record_boot` 之前呼叫。
    pub async fn last_boot_ended_cleanly(&self) -> StoreResult<Option<bool>> {
        self.read(|conn| {
            let row = conn.query_row(
                "SELECT ended_cleanly_at IS NOT NULL FROM runtime_boot ORDER BY id DESC LIMIT 1",
                [],
                |r| r.get::<_, bool>(0),
            );
            match row {
                Ok(clean) => Ok(Some(clean)),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(other) => Err(other.into()),
            }
        })
        .await
    }

    /// 記下這一次啟動正常結束（Runtime 有秩序地停止服務）。下次啟動就不視為意外終止。
    pub async fn mark_clean_shutdown(&self, now: Timestamp) -> StoreResult<()> {
        self.write(move |tx| {
            tx.execute(
                "UPDATE runtime_boot SET ended_cleanly_at = ?1 WHERE id = (SELECT MAX(id) FROM runtime_boot)",
                [now.as_millis()],
            )?;
            Ok(())
        })
        .await
    }

    /// Runtime 啟動時的恢復判定（在任何程序副作用之前）：上一次沒有正常結束、而且沒有停止意圖時視為意外終止，
    /// 建立或沿用恢復事件；進行中的嘗試一律記為失敗。回傳判定後的最新事件（沒有任何事件為 `None`）。
    pub async fn note_runtime_start(
        &self,
        previous_unclean: bool,
        new_event_id: String,
        now: Timestamp,
    ) -> StoreResult<Option<RecoveryEvent>> {
        self.write(move |tx| {
            let stop_intent = super::lifecycle::load_lifecycle(tx)?.phase != LifecyclePhase::Running;
            let current = load_latest(tx)?;
            match recovery::on_start(current.as_ref(), previous_unclean && !stop_intent, &new_event_id, now) {
                Some(change) => {
                    save_change(tx, &change)?;
                    Ok(Some(change.event))
                }
                None => Ok(current),
            }
        })
        .await
    }

    /// 最新的恢復事件（含全部輪次的嘗試）；從未發生過崩潰為 `None`。
    pub async fn recovery_event(&self) -> StoreResult<Option<RecoveryEvent>> {
        self.read(|conn| Ok(load_latest(conn)?)).await
    }

    /// 對最新的恢復事件套用命令；沒有變動（例如觀察期未滿）時回傳目前的事件。
    pub async fn apply_recovery(&self, command: RecoveryCommand, now: Timestamp) -> Result<RecoveryEvent, RecoveryApplyError> {
        let outcome = self
            .write(move |tx| {
                let current = load_latest(tx)?;
                match recovery::decide(current.as_ref(), command, now) {
                    Err(rejected) => Ok(Err(rejected)),
                    Ok(Some(change)) => {
                        save_change(tx, &change)?;
                        Ok(Ok(change.event))
                    }
                    Ok(None) => Ok(current.ok_or(RecoveryError::NoEvent)),
                }
            })
            .await?;
        Ok(outcome?)
    }

    /// 恢復嘗試停下了一個舊程序（已確認停止）：Run 以 `interrupted` 結束、釋放寫入所有權，Task 退回 `ready`
    /// 並在同一個交易中重新排隊，之後以新 Run 恢復（新 Run 仍經過授權、設定與相依檢查）。
    /// 使用者先前已要求取消的 Run 則完成取消。
    pub async fn stop_run_for_recovery(
        &self,
        workspace_id: &str,
        run_id: &str,
        now: Timestamp,
    ) -> Result<RunChange, RunApplyError> {
        self.stop_run_and_requeue(workspace_id, run_id, RUN_STOPPED_BY_RECOVERY_REASON, now).await
    }
}

/// 恢復是否擋住派工（在呼叫端的交易中）。
pub(crate) fn recovery_halt(conn: &Connection) -> rusqlite::Result<Option<WaitReason>> {
    Ok(recovery::dispatch_halt(load_latest(conn)?.as_ref()))
}

fn load_latest(conn: &Connection) -> rusqlite::Result<Option<RecoveryEvent>> {
    let row = conn.query_row(
        "SELECT id, round, status, attempts_used, detected_at, next_attempt_at, observe_until, last_failure, closed_at,
                version, updated_at
         FROM recovery_event ORDER BY rowid DESC LIMIT 1",
        [],
        |r| {
            let status: String = r.get(2)?;
            Ok(RecoveryEvent {
                id: r.get(0)?,
                round: r.get(1)?,
                status: RecoveryStatus::parse(&status).ok_or_else(|| conversion(2, "未知的恢復狀態"))?,
                attempts_used: r.get(3)?,
                detected_at: Timestamp::from_millis(r.get(4)?),
                next_attempt_at: r.get::<_, Option<i64>>(5)?.map(Timestamp::from_millis),
                observe_until: r.get::<_, Option<i64>>(6)?.map(Timestamp::from_millis),
                last_failure: r.get(7)?,
                closed_at: r.get::<_, Option<i64>>(8)?.map(Timestamp::from_millis),
                version: r.get(9)?,
                updated_at: Timestamp::from_millis(r.get(10)?),
                attempts: Vec::new(),
            })
        },
    );
    let mut event = match row {
        Ok(event) => event,
        Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
        Err(other) => return Err(other),
    };
    event.attempts = conn
        .prepare(
            "SELECT id, round, seq, started_at, finished_at, outcome, failure, affected FROM recovery_attempt
             WHERE event_id = ?1 ORDER BY round, seq",
        )?
        .query_map([&event.id], |r| {
            let outcome: String = r.get(5)?;
            let affected: String = r.get(7)?;
            Ok(RecoveryAttempt {
                id: r.get(0)?,
                round: r.get(1)?,
                seq: r.get(2)?,
                started_at: Timestamp::from_millis(r.get(3)?),
                finished_at: r.get::<_, Option<i64>>(4)?.map(Timestamp::from_millis),
                outcome: AttemptOutcome::parse(&outcome).ok_or_else(|| conversion(5, "未知的嘗試結果"))?,
                failure: r.get(6)?,
                affected: serde_json::from_str::<Vec<AffectedRun>>(&affected)
                    .map_err(|e| rusqlite::Error::FromSqlConversionFailure(7, rusqlite::types::Type::Text, Box::new(e)))?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Some(event))
}

fn conversion(column: usize, message: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(column, rusqlite::types::Type::Text, message.into())
}

fn save_change(tx: &Transaction<'_>, change: &RecoveryChange) -> rusqlite::Result<()> {
    let event = &change.event;
    tx.execute(
        "INSERT INTO recovery_event (id, round, status, attempts_used, detected_at, next_attempt_at, observe_until,
           last_failure, closed_at, version, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
         ON CONFLICT(id) DO UPDATE SET round = excluded.round, status = excluded.status,
           attempts_used = excluded.attempts_used, next_attempt_at = excluded.next_attempt_at,
           observe_until = excluded.observe_until, last_failure = excluded.last_failure, closed_at = excluded.closed_at,
           version = excluded.version, updated_at = excluded.updated_at",
        rusqlite::params![
            event.id,
            event.round,
            event.status.as_str(),
            event.attempts_used,
            event.detected_at.as_millis(),
            event.next_attempt_at.map(|t| t.as_millis()),
            event.observe_until.map(|t| t.as_millis()),
            event.last_failure,
            event.closed_at.map(|t| t.as_millis()),
            event.version,
            event.updated_at.as_millis(),
        ],
    )?;
    for attempt in &event.attempts {
        // Domain 型別只含字串，序列化不會失敗
        let affected = serde_json::to_string(&attempt.affected).expect("受影響工作可序列化");
        tx.execute(
            "INSERT INTO recovery_attempt (id, event_id, round, seq, started_at, finished_at, outcome, failure, affected)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(id) DO UPDATE SET finished_at = excluded.finished_at, outcome = excluded.outcome,
               failure = excluded.failure, affected = excluded.affected",
            rusqlite::params![
                attempt.id,
                event.id,
                attempt.round,
                attempt.seq,
                attempt.started_at.as_millis(),
                attempt.finished_at.map(|t| t.as_millis()),
                attempt.outcome.as_str(),
                attempt.failure,
                affected,
            ],
        )?;
    }
    for domain_event in &change.events {
        append_event(tx, domain_event)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
