//! agent_sessions、session_events 與 transcript_segments 的持久化（票 15）。
//!
//! 規則都在 Domain（`baton_domain::session`）；Session 只隨 Run 命令在同一個交易中改變（見 `runs::save`）。
//! 這裡提供載入與寫入 Session、Supervisor 啟動前的候選查詢、對話紀錄段落的寫入，以及面板的唯讀查詢。
//! 對話紀錄的內容由 Supervisor 經 WorkspaceFs 寫入成果儲存，這裡只記錄位置、雜湊與保存進度。

use baton_domain::run::RunError;
use baton_domain::session::{
    self, AgentSession, ResumeCapability, SegmentAvailability, SessionDetail, SessionEventRecord, SessionState,
    SessionView, TranscriptSegment, TranscriptSummary,
};
use baton_domain::workspace::AgentRole;
use rusqlite::OptionalExtension;

use super::runs::RunApplyError;
use super::*;

/// 一個 Run 的對話紀錄匯入進度：它驅動的 Session 與已保存到的最後位置（含缺口）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptProgress {
    pub agent_session_id: String,
    pub imported_through: u64,
}

impl Store {
    /// Supervisor 寫入 launch intent 之前要查驗接續能力的 Session（`session::resume_candidate`）；沒有候選回 `None`。
    pub async fn session_resume_candidate(
        &self,
        workspace_id: &str,
        task_id: &str,
        profile_id: &str,
        working_dir: &str,
    ) -> StoreResult<Option<AgentSession>> {
        let (workspace_id, task_id, profile_id, working_dir) =
            (workspace_id.to_string(), task_id.to_string(), profile_id.to_string(), working_dir.to_string());
        self.read(move |conn| {
            let sessions = load_sessions(conn, &workspace_id)?;
            Ok(session::resume_candidate(&sessions, &task_id, &profile_id, &working_dir).cloned())
        })
        .await
    }

    /// Workspace 內的全部 Session，依建立順序。
    pub async fn session_list(&self, workspace_id: &str) -> Result<Vec<SessionView>, RunApplyError> {
        let workspace_id = workspace_id.to_string();
        let result = self
            .read(move |conn| {
                if !workspace_exists(conn, &workspace_id)? {
                    return Ok(Err(RunError::WorkspaceNotFound(workspace_id)));
                }
                let views = load_sessions(conn, &workspace_id)?
                    .into_iter()
                    .map(|s| view(conn, s))
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(Ok(views))
            })
            .await?;
        Ok(result?)
    }

    /// 一個 Session 的完整紀錄：Session、session_events 與對話紀錄的段落。
    pub async fn session_detail(&self, workspace_id: &str, session_id: &str) -> Result<SessionDetail, RunApplyError> {
        let (workspace_id, session_id) = (workspace_id.to_string(), session_id.to_string());
        let result = self
            .read(move |conn| {
                if !workspace_exists(conn, &workspace_id)? {
                    return Ok(Err(RunError::WorkspaceNotFound(workspace_id)));
                }
                let Some(found) = load_sessions(conn, &workspace_id)?.into_iter().find(|s| s.id == session_id) else {
                    return Ok(Err(RunError::SessionNotFound(session_id)));
                };
                let events = conn
                    .prepare(
                        "SELECT event_type, aggregate_version, occurred_at, payload FROM session_events
                         WHERE agent_session_id = ?1 ORDER BY seq",
                    )?
                    .query_map([&session_id], |r| {
                        Ok(SessionEventRecord {
                            event_type: r.get(0)?,
                            aggregate_version: r.get(1)?,
                            occurred_at: Timestamp::from_millis(r.get(2)?),
                            payload: json_column(3, &r.get::<_, String>(3)?)?,
                        })
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                let segments = load_segments(conn, &session_id)?;
                Ok(Ok(SessionDetail { session: view(conn, found)?, events, segments }))
            })
            .await?;
        Ok(result?)
    }

    /// 一個 Run 的對話紀錄匯入進度；Run 還沒有 Session（尚未寫入 launch intent，或票 15 之前的 Run）時回 `None`。
    pub async fn transcript_progress(&self, run_id: &str) -> StoreResult<Option<TranscriptProgress>> {
        let run_id = run_id.to_string();
        self.read(move |conn| {
            let session: Option<String> = conn
                .query_row("SELECT agent_session_id FROM run WHERE id = ?1", [&run_id], |r| r.get(0))
                .optional()?
                .flatten();
            let Some(agent_session_id) = session else { return Ok(None) };
            Ok(Some(TranscriptProgress { agent_session_id, imported_through: imported_through(conn, &run_id)? }))
        })
        .await
    }

    /// 寫入對話紀錄的段落（內容已由呼叫端存進成果儲存）。已保存過的位置一律略過：同一個 Run 的區間不重疊。
    /// 回傳實際寫入的段落數。
    pub async fn record_transcript_segments(&self, segments: Vec<TranscriptSegment>, now: Timestamp) -> StoreResult<usize> {
        self.write(move |tx| {
            let mut written = 0;
            for segment in &segments {
                if segment.first_position <= imported_through(tx, &segment.run_id)? {
                    continue;
                }
                tx.execute(
                    "INSERT INTO transcript_segment (agent_session_id, run_id, first_position, last_position, availability,
                        storage_path, sha256, byte_len, entry_count, redactions, recorded_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                    rusqlite::params![
                        segment.agent_session_id,
                        segment.run_id,
                        segment.first_position as i64,
                        segment.last_position as i64,
                        segment.availability.as_str(),
                        segment.storage_path,
                        segment.sha256,
                        segment.byte_len.map(|b| b as i64),
                        segment.entry_count as i64,
                        segment.redactions as i64,
                        segment.recorded_at.as_millis(),
                    ],
                )?;
                tx.execute(
                    "UPDATE agent_session SET last_activity_at = MAX(last_activity_at, ?2) WHERE id = ?1",
                    rusqlite::params![segment.agent_session_id, now.as_millis()],
                )?;
                written += 1;
            }
            Ok(written)
        })
        .await
    }
}

fn workspace_exists(conn: &Connection, workspace_id: &str) -> rusqlite::Result<bool> {
    conn.query_row("SELECT EXISTS (SELECT 1 FROM workspace WHERE id = ?1)", [workspace_id], |r| r.get(0))
}

fn imported_through(conn: &Connection, run_id: &str) -> rusqlite::Result<u64> {
    let last: i64 = conn.query_row(
        "SELECT COALESCE(MAX(last_position), 0) FROM transcript_segment WHERE run_id = ?1",
        [run_id],
        |r| r.get(0),
    )?;
    Ok(last as u64)
}

fn view(conn: &Connection, session: AgentSession) -> rusqlite::Result<SessionView> {
    let run_ids = conn
        .prepare("SELECT id FROM run WHERE agent_session_id = ?1 ORDER BY rowid")?
        .query_map([&session.id], |r| r.get(0))?
        .collect::<rusqlite::Result<Vec<String>>>()?;
    let transcript = TranscriptSummary::of(&load_segments(conn, &session.id)?);
    Ok(SessionView { session, run_ids, transcript })
}

fn load_segments(conn: &Connection, session_id: &str) -> rusqlite::Result<Vec<TranscriptSegment>> {
    conn.prepare(
        "SELECT agent_session_id, run_id, first_position, last_position, availability, storage_path, sha256, byte_len,
            entry_count, redactions, recorded_at
         FROM transcript_segment WHERE agent_session_id = ?1 ORDER BY id",
    )?
    .query_map([session_id], |r| {
        let availability: String = r.get(4)?;
        Ok(TranscriptSegment {
            agent_session_id: r.get(0)?,
            run_id: r.get(1)?,
            first_position: r.get::<_, i64>(2)? as u64,
            last_position: r.get::<_, i64>(3)? as u64,
            availability: SegmentAvailability::parse(&availability).ok_or_else(|| unknown(4, "未知的段落可用性"))?,
            storage_path: r.get(5)?,
            sha256: r.get(6)?,
            byte_len: r.get::<_, Option<i64>>(7)?.map(|b| b as u64),
            entry_count: r.get::<_, i64>(8)? as u64,
            redactions: r.get::<_, i64>(9)? as u64,
            recorded_at: Timestamp::from_millis(r.get(10)?),
        })
    })?
    .collect()
}

const SESSION_COLUMNS: &str = "id, runtime_type, workspace_id, role, task_id, conversation_id, profile_id, working_directory, \
     native_session_id, state, state_reason, version, driver_run_id, predecessor_session_id, replacement_reason, history_gap, \
     resume_capability, capability_checked_at, created_at, last_activity_at";

fn unknown(index: usize, message: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Text, message.to_string().into())
}

fn session_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<AgentSession> {
    let role: String = r.get(3)?;
    let state: String = r.get(9)?;
    let capability: Option<String> = r.get(16)?;
    Ok(AgentSession {
        id: r.get(0)?,
        runtime_type: r.get(1)?,
        workspace_id: r.get(2)?,
        role: serde_json::from_value(serde_json::Value::String(role)).map_err(|_| unknown(3, "未知的 Session 角色"))?,
        task_id: r.get(4)?,
        conversation_id: r.get(5)?,
        profile_id: r.get(6)?,
        working_directory: r.get(7)?,
        native_session_id: r.get(8)?,
        state: SessionState::parse(&state).ok_or_else(|| unknown(9, "未知的 Session 狀態"))?,
        state_reason: r.get(10)?,
        version: r.get(11)?,
        driver_run_id: r.get(12)?,
        predecessor_session_id: r.get(13)?,
        replacement_reason: r.get(14)?,
        history_gap: r.get(15)?,
        resume_capability: match capability {
            Some(raw) => Some(ResumeCapability::parse(&raw).ok_or_else(|| unknown(16, "未知的接續能力"))?),
            None => None,
        },
        capability_checked_at: r.get::<_, Option<i64>>(17)?.map(Timestamp::from_millis),
        created_at: Timestamp::from_millis(r.get(18)?),
        last_activity_at: Timestamp::from_millis(r.get(19)?),
    })
}

/// Workspace 內的全部 Session，依建立順序。
pub(crate) fn load_sessions(conn: &Connection, workspace_id: &str) -> rusqlite::Result<Vec<AgentSession>> {
    conn.prepare(&format!("SELECT {SESSION_COLUMNS} FROM agent_session WHERE workspace_id = ?1 ORDER BY rowid"))?
        .query_map([workspace_id], session_row)?
        .collect()
}

/// 其他 Workspace 中綁定了這個原生識別的 Session（原生識別的命名空間是執行工具 + 帳號，不以 Workspace 為界）。
pub(crate) fn load_other_sessions_with_native(
    conn: &Connection,
    workspace_id: &str,
    native_session_id: &str,
) -> rusqlite::Result<Vec<AgentSession>> {
    conn.prepare(&format!(
        "SELECT {SESSION_COLUMNS} FROM agent_session WHERE native_session_id = ?1 AND workspace_id <> ?2 ORDER BY rowid"
    ))?
    .query_map([native_session_id, workspace_id], session_row)?
    .collect()
}

/// 寫入（或更新）一個 Session；必須在引用它的 Run 之前寫入（外鍵）。
pub(crate) fn save_session(tx: &Transaction<'_>, session: &AgentSession) -> rusqlite::Result<()> {
    let role = match session.role {
        AgentRole::Controller => "controller",
        AgentRole::Worker => "worker",
        AgentRole::Reviewer => "reviewer",
    };
    tx.execute(
        &format!(
            "INSERT INTO agent_session ({SESSION_COLUMNS}) VALUES
                (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)
             ON CONFLICT(id) DO UPDATE SET
               native_session_id = excluded.native_session_id, state = excluded.state,
               state_reason = excluded.state_reason, version = excluded.version, driver_run_id = excluded.driver_run_id,
               resume_capability = excluded.resume_capability, capability_checked_at = excluded.capability_checked_at,
               last_activity_at = excluded.last_activity_at"
        ),
        rusqlite::params![
            session.id,
            session.runtime_type,
            session.workspace_id,
            role,
            session.task_id,
            session.conversation_id,
            session.profile_id,
            session.working_directory,
            session.native_session_id,
            session.state.as_str(),
            session.state_reason,
            session.version,
            session.driver_run_id,
            session.predecessor_session_id,
            session.replacement_reason,
            session.history_gap,
            session.resume_capability.map(ResumeCapability::as_str),
            session.capability_checked_at.map(|t| t.as_millis()),
            session.created_at.as_millis(),
            session.last_activity_at.as_millis(),
        ],
    )?;
    Ok(())
}
