//! conversation、conversation_turns 與 Controller Run 的持久化（票 16）。
//!
//! 規則都在 Domain（`baton_domain::conversation`）；這裡只做「在同一個交易中載入目前狀態 → 交給 Domain 判定 →
//! 寫入 conversation、turn、Controller Run、Session 與事件」，以及給 Supervisor 與「對話」分頁的唯讀查詢。
//! Worker 的成果或阻礙在產生 Run 事件的同一個交易中成為工作事件（`enqueue_work_events`），所以不會遺失。

use baton_domain::conversation::{
    self, BeginTurn, ControllerRun, ControllerRunStatus, Conversation, ConversationChange, ConversationError,
    ConversationState, ConversationTurn, ConversationView, TurnKind, TurnOutcome, TurnStatus, WakePlan,
};
use baton_domain::run::{self, ProcessRecord, RunChange};
use baton_domain::session::{self, AgentSession, SessionOwner};
use baton_domain::workspace::{AgentRole, WorkspaceStatus};
use rusqlite::OptionalExtension;

use super::sessions::{load_sessions, save_session};
use super::*;

/// 套用對話命令的失敗：Domain 拒絕或儲存層錯誤。
#[derive(Debug, thiserror::Error)]
pub enum ConversationApplyError {
    #[error(transparent)]
    Rejected(#[from] ConversationError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl From<rusqlite::Error> for ConversationApplyError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error.into())
    }
}

/// 現在需要喚醒 Controller 的 conversation，附上 Supervisor 在交易外決定帳號與 Session 所需的事實。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControllerWake {
    pub conversation_id: String,
    pub workspace_id: String,
    /// Workspace 目錄：Controller 的工作目錄。
    pub working_dir: String,
    /// Workspace 為 Controller 角色覆寫的模型（spec D-30）。
    pub model: Option<String>,
    /// 這個 conversation 最近的 Session 的帳號：可派時優先留在它，才能原生接續。
    pub preferred_profile_id: Option<String>,
}

impl Store {
    /// 使用者送出一則訊息：第一則時建立 conversation。回傳保存的 turn，以及 Controller 當時是否忙碌。
    pub async fn post_user_message(
        &self,
        workspace_id: &str,
        text: String,
        now: Timestamp,
    ) -> Result<(ConversationTurn, bool), ConversationApplyError> {
        let workspace_id = workspace_id.to_string();
        let result = self
            .write(move |tx| {
                let workspace = load_workspaces(tx)?.into_iter().find(|w| w.id == workspace_id);
                match workspace {
                    None => return Ok(Err(ConversationError::WorkspaceNotFound(workspace_id))),
                    Some(w) if w.status == WorkspaceStatus::Archived => {
                        return Ok(Err(ConversationError::WorkspaceArchived(workspace_id)))
                    }
                    Some(_) => {}
                }
                let state = match load_state_of_workspace(tx, &workspace_id)? {
                    Some(state) => state,
                    None => conversation::start(&workspace_id, now),
                };
                let change = match conversation::receive(&state, TurnKind::UserMessage, &text, None, now) {
                    Ok(change) => change,
                    Err(rejected) => return Ok(Err(rejected)),
                };
                save(tx, &change)?;
                let busy = state.latest_run.as_ref().is_some_and(|r| !r.status.is_terminal());
                Ok(Ok((change.turns[0].clone(), busy)))
            })
            .await?;
        Ok(result?)
    }

    /// 「對話」分頁的內容；Workspace 不存在時拒絕。
    pub async fn conversation_view(&self, workspace_id: &str, now: Timestamp) -> Result<ConversationView, ConversationApplyError> {
        let workspace_id = workspace_id.to_string();
        let result = self
            .read(move |conn| {
                if !load_workspaces(conn)?.iter().any(|w| w.id == workspace_id) {
                    return Ok(Err(ConversationError::WorkspaceNotFound(workspace_id)));
                }
                let state = load_state_of_workspace(conn, &workspace_id)?;
                let runs = match &state {
                    Some(state) => load_runs(conn, "conversation_id = ?1", [&state.conversation.id])?,
                    None => Vec::new(),
                };
                Ok(Ok(ConversationView::of(state.as_ref(), runs, now)))
            })
            .await?;
        Ok(result?)
    }

    /// 現在需要喚醒 Controller 的 conversation（跨 Workspace）；停止意圖生效或 Workspace 暫停的不列出（票 25）。
    pub async fn controller_wakes(&self, now: Timestamp) -> StoreResult<Vec<ControllerWake>> {
        self.read(move |conn| {
            // 每次 tick 都會查：只載入有待處理輸入的 conversation，沒有事件時不讀任何對話內容
            let waiting: Vec<String> = conn
                .prepare(
                    "SELECT DISTINCT c.workspace_id FROM conversation c
                     JOIN conversation_turn t ON t.conversation_id = c.id WHERE t.status = 'pending'",
                )?
                .query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let mut wakes = Vec::new();
            let workspaces = load_workspaces(conn)?;
            let active = workspaces.into_iter().filter(|w| w.status == WorkspaceStatus::Active && waiting.contains(&w.id));
            for workspace in active {
                let Some(state) = load_state_of_workspace(conn, &workspace.id)? else { continue };
                if !matches!(conversation::plan_wake(&state, now), WakePlan::Wake { .. }) {
                    continue;
                }
                if super::lifecycle::launch_halted_in(conn, &workspace.id)? {
                    continue;
                }
                let sessions = load_sessions(conn, &workspace.id)?;
                let preferred = session::latest_for(&sessions, SessionOwner::Conversation(&state.conversation.id))
                    .map(|s| s.profile_id.clone());
                wakes.push(ControllerWake {
                    conversation_id: state.conversation.id.clone(),
                    workspace_id: workspace.id.clone(),
                    working_dir: workspace.path.clone(),
                    model: workspace.model_overrides.model_for(AgentRole::Controller).map(str::to_string),
                    preferred_profile_id: preferred,
                });
            }
            Ok(wakes)
        })
        .await
    }

    /// 喚起前要查驗接續能力的 Controller Session（`session::resume_candidate_for`）；沒有候選回 `None`。
    pub async fn controller_session_candidate(
        &self,
        workspace_id: &str,
        conversation_id: &str,
        profile_id: &str,
        working_dir: &str,
    ) -> StoreResult<Option<AgentSession>> {
        let (workspace_id, conversation_id, profile_id, working_dir) =
            (workspace_id.to_string(), conversation_id.to_string(), profile_id.to_string(), working_dir.to_string());
        self.read(move |conn| {
            let sessions = load_sessions(conn, &workspace_id)?;
            let owner = SessionOwner::Conversation(&conversation_id);
            Ok(session::resume_candidate_for(&sessions, owner, &profile_id, &working_dir).cloned())
        })
        .await
    }

    /// 喚起：持久化 Controller Run 的 launch intent、把輸入標為處理中、決定 Session（同一個交易）。
    /// 現在不該喚醒（已有一輪、合併視窗內、停止意圖生效）時拒絕，什麼都不寫。
    pub async fn begin_controller_run(
        &self,
        conversation_id: &str,
        begin: BeginTurn,
        now: Timestamp,
    ) -> Result<ControllerRun, ConversationApplyError> {
        let conversation_id = conversation_id.to_string();
        let result = self
            .write(move |tx| {
                let Some(state) = load_state(tx, &conversation_id)? else {
                    return Ok(Err(ConversationError::NothingToWake));
                };
                let workspace_id = state.conversation.workspace_id.clone();
                if super::lifecycle::launch_halted_in(tx, &workspace_id)? {
                    return Ok(Err(ConversationError::LaunchHalted));
                }
                let sessions = load_sessions(tx, &workspace_id)?;
                match conversation::begin_turn(&state, &sessions, begin, now) {
                    Err(rejected) => Ok(Err(rejected)),
                    Ok(change) => {
                        save(tx, &change)?;
                        Ok(Ok(change.run.expect("begin_turn 一定建立 Controller Run")))
                    }
                }
            })
            .await?;
        Ok(result?)
    }

    /// 尚未結束（starting、running）的 Controller Run（跨 Workspace，依建立順序）。
    pub async fn executing_controller_runs(&self) -> StoreResult<Vec<ControllerRun>> {
        self.read(|conn| Ok(load_runs(conn, "status IN ('starting', 'running')", [])?)).await
    }

    /// 寫回 Controller 程序的回執。
    pub async fn record_controller_receipt(
        &self,
        run_id: &str,
        process: ProcessRecord,
        now: Timestamp,
    ) -> Result<ControllerRun, ConversationApplyError> {
        self.apply_to_run(run_id, move |_, run, _| conversation::record_receipt(run, process, now)).await
    }

    /// Controller Run 回報原生 Session 識別：條件綁定。
    pub async fn bind_controller_native_session(
        &self,
        run_id: &str,
        native_session_id: String,
        now: Timestamp,
    ) -> Result<ControllerRun, ConversationApplyError> {
        self.apply_to_run(run_id, move |_, run, sessions| {
            conversation::bind_native(run, sessions, &native_session_id, now).map(|(change, _)| change)
        })
        .await
    }

    /// 結果保存（或中斷）：在同一個交易中結束這一輪、更新輸入、寫入回覆並釋放 Session。
    pub async fn finish_controller_run(
        &self,
        run_id: &str,
        outcome: TurnOutcome,
        now: Timestamp,
    ) -> Result<ControllerRun, ConversationApplyError> {
        self.apply_to_run(run_id, move |state, run, sessions| conversation::finish(state, run, sessions, outcome, now))
            .await
    }

    async fn apply_to_run(
        &self,
        run_id: &str,
        decide: impl FnOnce(&ConversationState, &ControllerRun, &[AgentSession]) -> Result<ConversationChange, ConversationError>
            + Send
            + 'static,
    ) -> Result<ControllerRun, ConversationApplyError> {
        let run_id = run_id.to_string();
        let result = self
            .write(move |tx| {
                let Some(run) = load_runs(tx, "id = ?1", [&run_id])?.pop() else {
                    return Ok(Err(ConversationError::RunNotFound(run_id)));
                };
                let Some(state) = load_state(tx, &run.conversation_id)? else {
                    return Ok(Err(ConversationError::RunNotFound(run_id)));
                };
                let mut sessions = load_sessions(tx, &run.workspace_id)?;
                // 原生識別的命名空間不以 Workspace 為界（同 Worker 的綁定）
                for other in load_all_sessions_except(tx, &run.workspace_id)? {
                    sessions.push(other);
                }
                match decide(&state, &run, &sessions) {
                    Err(rejected) => Ok(Err(rejected)),
                    Ok(change) => {
                        save(tx, &change)?;
                        Ok(Ok(change.run.unwrap_or(run)))
                    }
                }
            })
            .await?;
        Ok(result?)
    }
}

// ---------------------------------------------------------------- 工作事件

/// Worker 的 Run 成功、失敗或回報受阻：在同一個交易中成為該 Workspace conversation 的工作事件。
/// Workspace 還沒有 conversation（使用者沒有找過 Controller）時不建立，避免無人要求的 Controller 輪次。
pub(crate) fn enqueue_work_events(tx: &Transaction<'_>, change: &RunChange, now: Timestamp) -> rusqlite::Result<()> {
    let Some(run) = &change.run else { return Ok(()) };
    for event in change.events.iter().filter(|e| e.aggregate_type == run::RUN_AGGREGATE && e.aggregate_id == run.id) {
        let detail = match event.event_type {
            run::EVENT_RUN_BLOCKED_REPORTED => event.payload["reason"].as_str().map(str::to_string),
            run::EVENT_RUN_FAILED => run.end_reason.clone(),
            _ => None,
        };
        let Some(state) = load_state_of_workspace(tx, &run.workspace_id)? else { return Ok(()) };
        let title = match &change.task {
            Some(task) if task.id == run.task_id => task.title.clone(),
            _ => tx
                .query_row("SELECT title FROM task WHERE id = ?1", [&run.task_id], |r| r.get(0))
                .optional()?
                .unwrap_or_default(),
        };
        let Some(content) =
            conversation::work_event_content(event.event_type, &title, &run.task_id, &run.id, detail.as_deref())
        else {
            continue;
        };
        let source = format!("{}:{}:{}", event.event_type, run.id, event.aggregate_version);
        let received = conversation::receive(&state, TurnKind::WorkEvent, &content, Some(source), now)
            .expect("工作事件沒有長度限制，不會被拒絕");
        save(tx, &received)?;
    }
    Ok(())
}

// ---------------------------------------------------------------- 讀寫

const RUN_COLUMNS: &str = "id, conversation_id, workspace_id, profile_id, status, version, command_id, prompt, working_dir, \
     model, input_turn_ids, agent_session_id, resume_native_session_id, native_session_id, pid, process_created_at, \
     command_hash, end_reason, created_at, updated_at, started_at, ended_at";

const TURN_COLUMNS: &str =
    "id, conversation_id, seq, kind, status, content, controller_run_id, attempts, source, created_at, processed_at";

fn unknown(index: usize, message: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Text, message.to_string().into())
}

fn load_state_of_workspace(conn: &Connection, workspace_id: &str) -> rusqlite::Result<Option<ConversationState>> {
    let id: Option<String> =
        conn.query_row("SELECT id FROM conversation WHERE workspace_id = ?1", [workspace_id], |r| r.get(0)).optional()?;
    match id {
        Some(id) => load_state(conn, &id),
        None => Ok(None),
    }
}

fn load_state(conn: &Connection, conversation_id: &str) -> rusqlite::Result<Option<ConversationState>> {
    let conversation = conn
        .query_row(
            "SELECT id, workspace_id, version, created_at FROM conversation WHERE id = ?1",
            [conversation_id],
            |r| {
                Ok(Conversation {
                    id: r.get(0)?,
                    workspace_id: r.get(1)?,
                    version: r.get(2)?,
                    created_at: Timestamp::from_millis(r.get(3)?),
                })
            },
        )
        .optional()?;
    let Some(conversation) = conversation else { return Ok(None) };
    let turns = conn
        .prepare(&format!("SELECT {TURN_COLUMNS} FROM conversation_turn WHERE conversation_id = ?1 ORDER BY seq"))?
        .query_map([conversation_id], turn_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let latest_run = conn
        .query_row(
            &format!("SELECT {RUN_COLUMNS} FROM controller_run WHERE conversation_id = ?1 ORDER BY rowid DESC LIMIT 1"),
            [conversation_id],
            run_row,
        )
        .optional()?;
    Ok(Some(ConversationState { conversation, turns, latest_run }))
}

/// 以條件載入 Controller Run，依建立順序。
fn load_runs(conn: &Connection, condition: &str, params: impl rusqlite::Params) -> rusqlite::Result<Vec<ControllerRun>> {
    conn.prepare(&format!("SELECT {RUN_COLUMNS} FROM controller_run WHERE {condition} ORDER BY rowid"))?
        .query_map(params, run_row)?
        .collect()
}

fn load_all_sessions_except(conn: &Connection, workspace_id: &str) -> rusqlite::Result<Vec<AgentSession>> {
    let others: Vec<String> = conn
        .prepare("SELECT DISTINCT workspace_id FROM agent_session WHERE workspace_id <> ?1")?
        .query_map([workspace_id], |r| r.get(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut sessions = Vec::new();
    for other in others {
        sessions.extend(load_sessions(conn, &other)?);
    }
    Ok(sessions)
}

fn turn_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ConversationTurn> {
    let kind: String = r.get(3)?;
    let status: String = r.get(4)?;
    Ok(ConversationTurn {
        id: r.get(0)?,
        conversation_id: r.get(1)?,
        seq: r.get(2)?,
        kind: TurnKind::parse(&kind).ok_or_else(|| unknown(3, "未知的 turn 種類"))?,
        status: TurnStatus::parse(&status).ok_or_else(|| unknown(4, "未知的 turn 狀態"))?,
        content: r.get(5)?,
        controller_run_id: r.get(6)?,
        attempts: r.get(7)?,
        source: r.get(8)?,
        created_at: Timestamp::from_millis(r.get(9)?),
        processed_at: r.get::<_, Option<i64>>(10)?.map(Timestamp::from_millis),
    })
}

fn run_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ControllerRun> {
    let status: String = r.get(4)?;
    let inputs: String = r.get(10)?;
    let process = match (r.get::<_, Option<u32>>(14)?, r.get::<_, Option<i64>>(15)?, r.get::<_, Option<String>>(16)?) {
        (Some(pid), Some(created_at_millis), Some(command_hash)) => {
            Some(ProcessRecord { pid, created_at_millis, command_hash })
        }
        _ => None,
    };
    Ok(ControllerRun {
        id: r.get(0)?,
        conversation_id: r.get(1)?,
        workspace_id: r.get(2)?,
        profile_id: r.get(3)?,
        status: ControllerRunStatus::parse(&status).ok_or_else(|| unknown(4, "未知的 Controller Run 狀態"))?,
        version: r.get(5)?,
        command_id: r.get(6)?,
        prompt: r.get(7)?,
        working_dir: r.get(8)?,
        model: r.get(9)?,
        input_turn_ids: json_column(10, &inputs)?,
        agent_session_id: r.get(11)?,
        resume_native_session_id: r.get(12)?,
        native_session_id: r.get(13)?,
        process,
        end_reason: r.get(17)?,
        created_at: Timestamp::from_millis(r.get(18)?),
        updated_at: Timestamp::from_millis(r.get(19)?),
        started_at: r.get::<_, Option<i64>>(20)?.map(Timestamp::from_millis),
        ended_at: r.get::<_, Option<i64>>(21)?.map(Timestamp::from_millis),
    })
}

/// 把 Domain 的結果寫進同一個交易。順序依外鍵：conversation → Session → Controller Run → turn → 事件。
fn save(tx: &Transaction<'_>, change: &ConversationChange) -> rusqlite::Result<()> {
    if let Some(conversation) = &change.conversation {
        tx.execute(
            "INSERT INTO conversation (id, workspace_id, version, created_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET version = excluded.version",
            rusqlite::params![conversation.id, conversation.workspace_id, conversation.version, conversation.created_at.as_millis()],
        )?;
    }
    for session in &change.sessions {
        save_session(tx, session)?;
    }
    if let Some(run) = &change.run {
        let (pid, process_created_at, command_hash) = match &run.process {
            Some(p) => (Some(p.pid), Some(p.created_at_millis), Some(p.command_hash.as_str())),
            None => (None, None, None),
        };
        let millis = |t: Option<Timestamp>| t.map(|t| t.as_millis());
        tx.execute(
            &format!(
                "INSERT INTO controller_run ({RUN_COLUMNS}) VALUES
                    (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22)
                 ON CONFLICT(id) DO UPDATE SET
                   status = excluded.status, version = excluded.version, native_session_id = excluded.native_session_id,
                   pid = excluded.pid, process_created_at = excluded.process_created_at, command_hash = excluded.command_hash,
                   end_reason = excluded.end_reason, updated_at = excluded.updated_at,
                   started_at = excluded.started_at, ended_at = excluded.ended_at"
            ),
            rusqlite::params![
                run.id,
                run.conversation_id,
                run.workspace_id,
                run.profile_id,
                run.status.as_str(),
                run.version,
                run.command_id,
                run.prompt,
                run.working_dir,
                run.model,
                serde_json::to_string(&run.input_turn_ids).expect("字串清單可序列化"),
                run.agent_session_id,
                run.resume_native_session_id,
                run.native_session_id,
                pid,
                process_created_at,
                command_hash,
                run.end_reason,
                run.created_at.as_millis(),
                run.updated_at.as_millis(),
                millis(run.started_at),
                millis(run.ended_at),
            ],
        )?;
    }
    for turn in &change.turns {
        tx.execute(
            &format!(
                "INSERT INTO conversation_turn ({TURN_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                 ON CONFLICT(id) DO UPDATE SET
                   status = excluded.status, controller_run_id = excluded.controller_run_id,
                   attempts = excluded.attempts, processed_at = excluded.processed_at"
            ),
            rusqlite::params![
                turn.id,
                turn.conversation_id,
                turn.seq,
                turn.kind.as_str(),
                turn.status.as_str(),
                turn.content,
                turn.controller_run_id,
                turn.attempts,
                turn.source,
                turn.created_at.as_millis(),
                turn.processed_at.map(|t| t.as_millis()),
            ],
        )?;
    }
    for event in &change.events {
        append_event(tx, event)?;
    }
    Ok(())
}
