//! 驗證的嘗試與日誌（票 49）。
//!
//! - 一個驗證要求同時最多一個執行中的嘗試（部分唯一索引）。開始新的嘗試時，仍在執行中的嘗試是前一個 Runtime 留下的
//!   （驗證一次只由一個工作執行），在同一個交易中記為中斷。
//! - 日誌只追加，而且只能寫給執行中的嘗試：已經交接或中斷的嘗試不能再寫入（舊的執行者不會蓋過承接者）。
//! - 產生 Evidence Bundle 時，執行中的嘗試在同一個交易中記為完成（`verification_profiles.rs` 的 `record_evidence_bundle`）。

use baton_domain::verification_attempt::{self, Attempt, AttemptStatus, JournalEntry};
use rusqlite::OptionalExtension;

use super::*;

fn load_attempts(conn: &Connection, request_id: &str) -> rusqlite::Result<Vec<Attempt>> {
    let mut attempts: Vec<Attempt> = conn
        .prepare(
            "SELECT request_id, attempt, status, judge_profile_id, end_reason, started_at, ended_at
             FROM verification_attempt WHERE request_id = ?1 ORDER BY attempt",
        )?
        .query_map([request_id], |r| {
            let status: String = r.get(2)?;
            Ok(Attempt {
                request_id: r.get(0)?,
                attempt: r.get(1)?,
                status: AttemptStatus::parse(&status).ok_or_else(|| {
                    rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, "未知的驗證嘗試狀態".into())
                })?,
                judge_profile_id: r.get(3)?,
                end_reason: r.get(4)?,
                started_at: Timestamp::from_millis(r.get(5)?),
                ended_at: r.get::<_, Option<i64>>(6)?.map(Timestamp::from_millis),
                journal: Vec::new(),
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    let mut statement =
        conn.prepare("SELECT attempt, entry FROM verification_journal WHERE request_id = ?1 ORDER BY attempt, seq")?;
    let entries = statement.query_map([request_id], |r| Ok((r.get::<_, u32>(0)?, json_column::<JournalEntry>(1, &r.get::<_, String>(1)?)?)))?;
    for entry in entries {
        let (attempt, entry) = entry?;
        if let Some(owner) = attempts.iter_mut().find(|a| a.attempt == attempt) {
            owner.journal.push(entry);
        }
    }
    Ok(attempts)
}

/// 結束執行中的嘗試（交接或中斷）並追加事件；不是執行中時什麼都不寫，回傳 `false`。
fn end_attempt(
    tx: &Transaction<'_>,
    request_id: &str,
    attempt: u32,
    status: AttemptStatus,
    reason: &str,
    now: Timestamp,
) -> rusqlite::Result<bool> {
    let updated = tx.execute(
        "UPDATE verification_attempt SET status = ?3, end_reason = ?4, ended_at = ?5
         WHERE request_id = ?1 AND attempt = ?2 AND status = 'running'",
        rusqlite::params![request_id, attempt, status.as_str(), reason, now.as_millis()],
    )?;
    if updated == 0 {
        return Ok(false);
    }
    append_event(tx, &verification_attempt::ended_event(request_id, attempt, status, reason, now))?;
    Ok(true)
}

/// 產生 Bundle 時把執行中的嘗試記為完成（與 Bundle 同一個交易）。
pub(crate) fn complete_running_attempt(tx: &Transaction<'_>, request_id: &str, now: Timestamp) -> rusqlite::Result<()> {
    tx.execute(
        "UPDATE verification_attempt SET status = 'completed', ended_at = ?2 WHERE request_id = ?1 AND status = 'running'",
        rusqlite::params![request_id, now.as_millis()],
    )?;
    Ok(())
}

impl Store {
    /// 一個驗證要求的全部嘗試與各自的日誌（依嘗試順序）。
    pub async fn verification_attempts(&self, request_id: &str) -> StoreResult<Vec<Attempt>> {
        let request_id = request_id.to_string();
        self.read(move |conn| Ok(load_attempts(conn, &request_id)?)).await
    }

    /// 開始新的嘗試：仍在執行中的嘗試（前一個 Runtime 留下的）記為中斷。回傳新的嘗試與先前的全部嘗試；
    /// 要求已經完成時不寫入，回傳 `None`。
    pub async fn begin_verification_attempt(
        &self,
        request_id: &str,
        judge_profile_id: Option<String>,
        now: Timestamp,
    ) -> StoreResult<Option<(Attempt, Vec<Attempt>)>> {
        let request_id = request_id.to_string();
        self.write(move |tx| {
            let queued: Option<i64> = tx
                .query_row("SELECT 1 FROM verification_request WHERE id = ?1 AND status = 'queued'", [&request_id], |r| r.get(0))
                .optional()?;
            if queued.is_none() {
                return Ok(None);
            }
            let mut interrupted = Vec::new();
            for previous in load_attempts(tx, &request_id)?.into_iter().filter(|a| a.status == AttemptStatus::Running) {
                let reason = verification_attempt::REASON_RUNTIME_INTERRUPTED;
                end_attempt(tx, &request_id, previous.attempt, AttemptStatus::Interrupted, reason, now)?;
                interrupted.push(previous.attempt);
            }
            let previous = load_attempts(tx, &request_id)?;
            let attempt = Attempt {
                request_id: request_id.clone(),
                attempt: previous.last().map_or(1, |a| a.attempt + 1),
                status: AttemptStatus::Running,
                judge_profile_id,
                end_reason: None,
                started_at: now,
                ended_at: None,
                journal: Vec::new(),
            };
            tx.execute(
                "INSERT INTO verification_attempt (request_id, attempt, status, judge_profile_id, started_at)
                 VALUES (?1, ?2, 'running', ?3, ?4)",
                rusqlite::params![attempt.request_id, attempt.attempt, attempt.judge_profile_id, now.as_millis()],
            )?;
            append_event(tx, &verification_attempt::started_event(&attempt, &interrupted))?;
            Ok(Some((attempt, previous)))
        })
        .await
    }

    /// 追加一筆日誌給執行中的嘗試；那次嘗試已經不是執行中（交接、中斷或完成）時不寫入，回傳 `false`。
    pub async fn append_verification_journal(
        &self,
        request_id: &str,
        attempt: u32,
        entry: JournalEntry,
        now: Timestamp,
    ) -> StoreResult<bool> {
        let request_id = request_id.to_string();
        self.write(move |tx| {
            let inserted = tx.execute(
                "INSERT INTO verification_journal (request_id, attempt, seq, kind, entry, created_at)
                 SELECT ?1, ?2, COALESCE((SELECT MAX(seq) FROM verification_journal WHERE request_id = ?1 AND attempt = ?2), 0) + 1,
                        ?3, ?4, ?5
                 WHERE EXISTS (SELECT 1 FROM verification_attempt WHERE request_id = ?1 AND attempt = ?2 AND status = 'running')",
                rusqlite::params![
                    request_id,
                    attempt,
                    entry.kind(),
                    serde_json::to_string(&entry).expect("日誌一定可以序列化"),
                    now.as_millis(),
                ],
            )?;
            Ok(inserted == 1)
        })
        .await
    }

    /// 這次嘗試在步驟邊界交接（測試程序已停止）；不是執行中時不寫入，回傳 `false`。
    pub async fn hand_off_verification_attempt(
        &self,
        request_id: &str,
        attempt: u32,
        reason: &str,
        now: Timestamp,
    ) -> StoreResult<bool> {
        let (request_id, reason) = (request_id.to_string(), reason.to_string());
        self.write(move |tx| Ok(end_attempt(tx, &request_id, attempt, AttemptStatus::HandedOff, &reason, now)?)).await
    }
}
