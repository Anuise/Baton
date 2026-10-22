//! Controller 長期對話與事件驅動的輪次（票 16，spec D-06、D-31，ADR-0009 的 A1＋B1）。
//!
//! 純邏輯、無 I/O。輸入「目前的對話狀態 + 命令 + 時間」，輸出「新狀態 + 事件」，由 Store 在一個交易中套用。
//!
//! 幾條貫穿全檔的規則：
//! - **事件驅動（A1）**：只有使用者訊息與工作事件（Worker 成果或阻礙）會成為 conversation 的輸入；
//!   沒有待處理的輸入就不喚醒 Controller（`plan_wake` 回 `Idle`），沒有任何定時巡查。
//! - **合併視窗（D-31）**：第一則待處理輸入到達後等 5 秒，期間到達的輸入併入同一輪。
//! - **依序處理（B1）**：同一個 conversation 任何時刻最多一個未結束的 Controller Run；忙碌時新的輸入只保存為
//!   「已收到／待處理」，不中斷進行中的一輪。下一輪先處理全部待處理的使用者訊息，沒有使用者訊息時才處理工作事件。
//! - **喚起與結果保存都是持久化的邊界**：喚起（`begin_turn`）在同一個交易中建立 Controller Run 的 launch intent、
//!   把輸入標為處理中並決定 Session；結果保存（`finish`）在同一個交易中把輸入標為已處理並寫入 Controller 的回覆。
//!   兩者之間中斷時，Controller Run 以中斷結束（`interrupt`），輸入退回待處理，下一輪重新交付並註明可能已部分處理。
//! - **conversation 與 Session 分開**：Controller 換原生 Session 時 conversation 不變；替代 Session 的 Prompt
//!   附上已處理的對話作為背景（明示不要重新執行），只有待處理的輸入列為本輪的指示，所以不會重複執行。

use serde::{Deserialize, Serialize};

use crate::event::DomainEvent;
use crate::run::ProcessRecord;
use crate::session::{self, AgentSession, BindOutcome, LaunchFacts, ResumeCheck, SessionEdits, SessionOwner};
use crate::Timestamp;

pub const CONVERSATION_AGGREGATE: &str = "conversation";
pub const CONTROLLER_RUN_AGGREGATE: &str = "controller_run";

pub const EVENT_CONVERSATION_CREATED: &str = "conversation_created";
pub const EVENT_CONVERSATION_INPUT_RECEIVED: &str = "conversation_input_received";
pub const EVENT_CONVERSATION_INPUTS_DISPATCHED: &str = "conversation_inputs_dispatched";
pub const EVENT_CONVERSATION_INPUTS_PROCESSED: &str = "conversation_inputs_processed";
pub const EVENT_CONVERSATION_INPUTS_RELEASED: &str = "conversation_inputs_released";
pub const EVENT_CONTROLLER_RUN_LAUNCH_STARTED: &str = "controller_run_launch_started";
pub const EVENT_CONTROLLER_RUN_STARTED: &str = "controller_run_started";
pub const EVENT_CONTROLLER_RUN_NATIVE_SESSION_BOUND: &str = "controller_run_native_session_bound";
pub const EVENT_CONTROLLER_RUN_NATIVE_SESSION_CONFLICT: &str = "controller_run_native_session_conflict";
pub const EVENT_CONTROLLER_RUN_SUCCEEDED: &str = "controller_run_succeeded";
pub const EVENT_CONTROLLER_RUN_FAILED: &str = "controller_run_failed";
pub const EVENT_CONTROLLER_RUN_INTERRUPTED: &str = "controller_run_interrupted";

/// Controller 事件合併視窗（spec D-31）。
pub const MERGE_WINDOW_MS: i64 = 5_000;
/// 上一輪沒有完成的輸入重新交付前的退避：1 分鐘起、每次加倍、上限 10 分鐘（與 spec D-16 的基礎設施重試同形）。
pub const RETRY_BACKOFF_BASE_MS: i64 = 60_000;
pub const RETRY_BACKOFF_MAX_MS: i64 = 600_000;
/// 一則使用者訊息的長度上限（字元）。
pub const MAX_MESSAGE_CHARS: usize = 20_000;
/// 替代 Session 的 Prompt 最多附上的已處理對話則數（最近的）。
pub const HISTORY_LIMIT: usize = 40;

// ---------------------------------------------------------------- 實體

/// 一個 Workspace 與 Controller 的長期對話；第一則使用者訊息到達時建立，之後不會改變識別。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conversation {
    pub id: String,
    pub workspace_id: String,
    /// 每個事件加一；事件的 `aggregate_version` 與它對應。
    pub version: i64,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnKind {
    /// 使用者在「對話」分頁送出的訊息。
    UserMessage,
    /// Worker 的成果或阻礙（Run 成功、失敗或回報受阻）。
    WorkEvent,
    /// Controller 一輪結束時的回覆。
    ControllerReply,
}

impl TurnKind {
    pub const ALL: [TurnKind; 3] = [TurnKind::UserMessage, TurnKind::WorkEvent, TurnKind::ControllerReply];

    pub fn as_str(self) -> &'static str {
        match self {
            TurnKind::UserMessage => "user_message",
            TurnKind::WorkEvent => "work_event",
            TurnKind::ControllerReply => "controller_reply",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == raw)
    }

    /// 需要 Controller 處理的輸入（回覆不是輸入）。
    pub fn is_input(self) -> bool {
        self != TurnKind::ControllerReply
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnStatus {
    /// 已收到、還沒交給任何一輪（UI 顯示「已收到／待處理」）。
    Pending,
    /// 已交給目前進行中的一輪。
    Processing,
    /// 那一輪已結束並保存結果（回覆一律是這個狀態）。
    Processed,
}

impl TurnStatus {
    pub const ALL: [TurnStatus; 3] = [TurnStatus::Pending, TurnStatus::Processing, TurnStatus::Processed];

    pub fn as_str(self) -> &'static str {
        match self {
            TurnStatus::Pending => "pending",
            TurnStatus::Processing => "processing",
            TurnStatus::Processed => "processed",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.as_str() == raw)
    }
}

/// conversation_turns 的一筆：使用者訊息、工作事件或 Controller 的回覆。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationTurn {
    pub id: String,
    pub conversation_id: String,
    /// 在這個 conversation 中的順序，從 1 起連續遞增。
    pub seq: i64,
    pub kind: TurnKind,
    pub content: String,
    pub status: TurnStatus,
    /// 處理中：正在處理它的 Controller Run；已處理：處理它（或產生這則回覆）的 Controller Run。
    pub controller_run_id: Option<String>,
    /// 已經交給 Controller Run 的次數；大於 1 代表先前有一輪沒有完成。
    pub attempts: i64,
    /// 工作事件的來源（例如 `run_succeeded:run_x`）；同一個來源只會成為一個輸入。
    pub source: Option<String>,
    pub created_at: Timestamp,
    pub processed_at: Option<Timestamp>,
}

impl ConversationTurn {
    fn is_pending_input(&self) -> bool {
        self.kind.is_input() && self.status == TurnStatus::Pending
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControllerRunStatus {
    /// launch intent 已持久化，程序的回執還沒寫回。
    Starting,
    Running,
    Succeeded,
    Failed,
    Interrupted,
}

impl ControllerRunStatus {
    pub const ALL: [ControllerRunStatus; 5] = [
        ControllerRunStatus::Starting,
        ControllerRunStatus::Running,
        ControllerRunStatus::Succeeded,
        ControllerRunStatus::Failed,
        ControllerRunStatus::Interrupted,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ControllerRunStatus::Starting => "starting",
            ControllerRunStatus::Running => "running",
            ControllerRunStatus::Succeeded => "succeeded",
            ControllerRunStatus::Failed => "failed",
            ControllerRunStatus::Interrupted => "interrupted",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.as_str() == raw)
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, ControllerRunStatus::Succeeded | ControllerRunStatus::Failed | ControllerRunStatus::Interrupted)
    }
}

/// Controller 的一輪：一次 `claude -p`（spec D-06）。只綁 conversation 與 Workspace，不綁 Task。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControllerRun {
    pub id: String,
    pub conversation_id: String,
    pub workspace_id: String,
    pub profile_id: String,
    pub status: ControllerRunStatus,
    /// 每個事件加一；事件的 `aggregate_version` 與它對應。
    pub version: i64,
    /// launch intent：與 Run 建立在同一個交易；Runtime 重建後以它找回原程序。
    pub command_id: String,
    /// 交給 `claude -p` 的 Prompt；與 launch intent 一起持久化，重建後組出相同的啟動指令。
    pub prompt: String,
    pub working_dir: String,
    pub model: Option<String>,
    /// 這一輪處理的輸入（conversation_turns 的識別），依順序。
    pub input_turn_ids: Vec<String>,
    pub agent_session_id: String,
    /// 要以原生方式接續的原生 Session；建立新原生 Session 時為 `None`。
    pub resume_native_session_id: Option<String>,
    /// Adapter 回報的原生 Session 識別。
    pub native_session_id: Option<String>,
    pub process: Option<ProcessRecord>,
    pub end_reason: Option<String>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub started_at: Option<Timestamp>,
    pub ended_at: Option<Timestamp>,
}

/// 決定一個 conversation 下一步需要的狀態（全部由 Store 在同一個交易中載入）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationState {
    pub conversation: Conversation,
    /// 全部的 turn，依 `seq`。
    pub turns: Vec<ConversationTurn>,
    /// 最近建立的 Controller Run（進行中的那一輪一定是它）。
    pub latest_run: Option<ControllerRun>,
}

impl ConversationState {
    fn last_seq(&self) -> i64 {
        self.turns.last().map(|t| t.seq).unwrap_or(0)
    }

    fn active_run(&self) -> Option<&ControllerRun> {
        self.latest_run.as_ref().filter(|r| !r.status.is_terminal())
    }
}

/// 「對話」分頁的內容。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationView {
    /// 還沒有任何訊息時為 `None`。
    pub conversation_id: Option<String>,
    /// 有一輪 Controller 正在進行（B1）。
    pub busy: bool,
    pub active_run: Option<ControllerRun>,
    /// 不忙碌且有待處理輸入時，預計喚醒 Controller 的時間（已到期而尚未喚醒時是現在）。
    pub next_wake_at: Option<Timestamp>,
    /// 依 `seq`。
    pub turns: Vec<ConversationTurn>,
    /// 全部 Controller Run，依建立順序。
    pub runs: Vec<ControllerRun>,
}

impl ConversationView {
    pub fn of(state: Option<&ConversationState>, runs: Vec<ControllerRun>, now: Timestamp) -> Self {
        let Some(state) = state else {
            return Self { conversation_id: None, busy: false, active_run: None, next_wake_at: None, turns: Vec::new(), runs };
        };
        let next_wake_at = match plan_wake(state, now) {
            WakePlan::Wait { due_at } => Some(due_at),
            WakePlan::Wake { .. } => Some(now),
            WakePlan::Idle | WakePlan::Busy { .. } => None,
        };
        let active_run = state.active_run().cloned();
        Self {
            conversation_id: Some(state.conversation.id.clone()),
            busy: active_run.is_some(),
            active_run,
            next_wake_at,
            turns: state.turns.clone(),
            runs,
        }
    }
}

// ---------------------------------------------------------------- 錯誤

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConversationError {
    #[error("找不到 Workspace {0}")]
    WorkspaceNotFound(String),
    #[error("Workspace {0} 已封存，不能再與 Controller 對話")]
    WorkspaceArchived(String),
    #[error("訊息不能是空白")]
    MessageEmpty,
    #[error("訊息超過 {MAX_MESSAGE_CHARS} 個字元")]
    MessageTooLong,
    #[error("沒有需要 Controller 處理的輸入，或還在合併視窗內")]
    NothingToWake,
    #[error("Controller 正在處理上一輪（{0}），同一個對話不並行")]
    Busy(String),
    #[error("conversation 的 Session {session_id} 仍由 {run_id} 驅動")]
    SessionBusy { session_id: String, run_id: String },
    #[error("完全退出的停止意圖生效中或 Workspace 已暫停，不喚醒 Controller")]
    LaunchHalted,
    #[error("找不到 Controller Run {0}")]
    RunNotFound(String),
    #[error("Controller Run {run_id} 目前是 {from}，不能{action}")]
    TransitionInvalid { run_id: String, from: &'static str, action: &'static str },
}

impl ConversationError {
    /// 穩定的錯誤代碼（API 回應用）。
    pub fn code(&self) -> &'static str {
        match self {
            ConversationError::WorkspaceNotFound(_) => "workspace_not_found",
            ConversationError::WorkspaceArchived(_) => "workspace_archived",
            ConversationError::MessageEmpty => "message_empty",
            ConversationError::MessageTooLong => "message_too_long",
            ConversationError::NothingToWake => "nothing_to_wake",
            ConversationError::Busy(_) => "controller_busy",
            ConversationError::SessionBusy { .. } => "session_busy",
            ConversationError::LaunchHalted => "launch_halted",
            ConversationError::RunNotFound(_) => "controller_run_not_found",
            ConversationError::TransitionInvalid { .. } => "transition_invalid",
        }
    }
}

// ---------------------------------------------------------------- 變更

/// 一個命令的結果：被改變（或新建）的 conversation、turn、Controller Run、Session 與事件。
#[derive(Debug, Default)]
pub struct ConversationChange {
    pub conversation: Option<Conversation>,
    pub turns: Vec<ConversationTurn>,
    pub run: Option<ControllerRun>,
    pub sessions: Vec<AgentSession>,
    pub events: Vec<DomainEvent>,
}

impl ConversationChange {
    fn conversation_event(&mut self, conversation: &Conversation, event_type: &'static str, payload: serde_json::Value, now: Timestamp) {
        let mut conversation = self.conversation.clone().unwrap_or_else(|| conversation.clone());
        conversation.version += 1;
        self.events.push(DomainEvent {
            aggregate_type: CONVERSATION_AGGREGATE,
            aggregate_id: conversation.id.clone(),
            aggregate_version: conversation.version,
            event_type,
            occurred_at: now,
            payload,
        });
        self.conversation = Some(conversation);
    }

    fn run_event(&mut self, mut run: ControllerRun, event_type: &'static str, payload: serde_json::Value, now: Timestamp) {
        run.version += 1;
        run.updated_at = now;
        self.events.push(DomainEvent {
            aggregate_type: CONTROLLER_RUN_AGGREGATE,
            aggregate_id: run.id.clone(),
            aggregate_version: run.version,
            event_type,
            occurred_at: now,
            payload,
        });
        self.run = Some(run);
    }

    fn absorb_sessions(&mut self, edits: SessionEdits) {
        self.sessions = edits.sessions;
        self.events.extend(edits.events);
    }
}

/// 每個 Workspace 只有一個 conversation，識別由 Workspace 導出。
pub fn conversation_id_for(workspace_id: &str) -> String {
    format!("conv_{}", workspace_id.strip_prefix("ws_").unwrap_or(workspace_id))
}

/// turn 的識別由 conversation 與順序導出（同一個順序只會有一筆）。
pub fn turn_id_for(conversation_id: &str, seq: i64) -> String {
    format!("turn_{}_{seq}", conversation_id.strip_prefix("conv_").unwrap_or(conversation_id))
}

/// 建立 Workspace 的 conversation（第一則使用者訊息到達時，與那則訊息在同一個交易中）。
pub fn start(workspace_id: &str, now: Timestamp) -> ConversationState {
    ConversationState {
        conversation: Conversation {
            id: conversation_id_for(workspace_id),
            workspace_id: workspace_id.to_string(),
            version: 0,
            created_at: now,
        },
        turns: Vec::new(),
        latest_run: None,
    }
}

/// 收到一則輸入（使用者訊息或工作事件）：保存為待處理。Controller 忙碌與否都一樣，從不中斷進行中的一輪。
/// 新建的 conversation（版本 0）一併記下建立事件。
pub fn receive(
    state: &ConversationState,
    kind: TurnKind,
    content: &str,
    source: Option<String>,
    now: Timestamp,
) -> Result<ConversationChange, ConversationError> {
    let content = content.trim();
    if kind == TurnKind::UserMessage {
        if content.is_empty() {
            return Err(ConversationError::MessageEmpty);
        }
        if content.chars().count() > MAX_MESSAGE_CHARS {
            return Err(ConversationError::MessageTooLong);
        }
    }
    let mut change = ConversationChange::default();
    if state.conversation.version == 0 {
        let payload = serde_json::json!({ "workspace_id": state.conversation.workspace_id });
        change.conversation_event(&state.conversation, EVENT_CONVERSATION_CREATED, payload, now);
    }
    let seq = state.last_seq() + 1;
    let turn = ConversationTurn {
        id: turn_id_for(&state.conversation.id, seq),
        conversation_id: state.conversation.id.clone(),
        seq,
        kind,
        content: content.to_string(),
        status: TurnStatus::Pending,
        controller_run_id: None,
        attempts: 0,
        source,
        created_at: now,
        processed_at: None,
    };
    let payload = serde_json::json!({
        "turn_id": turn.id,
        "seq": turn.seq,
        "kind": kind.as_str(),
        "source": turn.source,
        "controller_busy": state.active_run().map(|r| r.id.clone()),
    });
    change.conversation_event(&state.conversation, EVENT_CONVERSATION_INPUT_RECEIVED, payload, now);
    change.turns.push(turn);
    Ok(change)
}

// ---------------------------------------------------------------- 喚起

/// 一個 conversation 現在要不要喚醒 Controller。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WakePlan {
    /// 沒有待處理的輸入：不喚醒。
    Idle,
    /// 已有一輪進行中（B1）：新的輸入等它結束。
    Busy { run_id: String },
    /// 有待處理的輸入，但還在合併視窗或重試退避內。
    Wait { due_at: Timestamp },
    /// 現在喚醒，這一輪處理這些輸入（依順序）。
    Wake { input_turn_ids: Vec<String> },
}

/// 重新交付的退避：第 n 次重新交付等 1、2、4… 分鐘，上限 10 分鐘。
pub fn retry_backoff_ms(previous_attempts: i64) -> i64 {
    let exponent = (previous_attempts - 1).clamp(0, 10) as u32;
    (RETRY_BACKOFF_BASE_MS * 2i64.pow(exponent)).min(RETRY_BACKOFF_MAX_MS)
}

/// 決定下一步。這一輪的輸入：有待處理的使用者訊息時是全部的使用者訊息，否則是全部的工作事件（都依到達順序）。
pub fn plan_wake(state: &ConversationState, now: Timestamp) -> WakePlan {
    if let Some(active) = state.active_run() {
        return WakePlan::Busy { run_id: active.id.clone() };
    }
    let pending: Vec<&ConversationTurn> = state.turns.iter().filter(|t| t.is_pending_input()).collect();
    let Some(first) = pending.iter().map(|t| t.created_at).min() else { return WakePlan::Idle };
    let users: Vec<&ConversationTurn> = pending.iter().copied().filter(|t| t.kind == TurnKind::UserMessage).collect();
    let batch = if users.is_empty() { pending } else { users };
    // 合併視窗從第一則待處理的輸入開始算
    let mut due_at = Timestamp::from_millis(first.as_millis() + MERGE_WINDOW_MS);
    // 先前沒有完成的輸入：從上一輪結束起退避，避免一直失敗時反覆喚醒
    let retried = batch.iter().map(|t| t.attempts).max().unwrap_or(0);
    if let Some(ended_at) = state.latest_run.as_ref().and_then(|r| r.ended_at).filter(|_| retried > 0) {
        due_at = due_at.max(Timestamp::from_millis(ended_at.as_millis() + retry_backoff_ms(retried)));
    }
    if now < due_at {
        return WakePlan::Wait { due_at };
    }
    WakePlan::Wake { input_turn_ids: batch.iter().map(|t| t.id.clone()).collect() }
}

/// 喚起一輪所需、由 Supervisor 在交易外決定的事實。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BeginTurn {
    pub run_id: String,
    pub command_id: String,
    pub profile_id: String,
    /// Workspace 目錄（Controller 的工作目錄）。
    pub working_dir: String,
    pub model: Option<String>,
    /// Supervisor 對候選 Session 查驗接續能力的結果（`session::resume_candidate_for`）。
    pub resume_check: Option<ResumeCheck>,
}

/// 喚起：在同一個交易中建立 Controller Run 的 launch intent、把這一輪的輸入標為處理中、決定沿用或替換 Session，
/// 並組出 Prompt。現在不該喚醒（沒有輸入、合併視窗內、已有一輪進行中）時拒絕，什麼都不寫。
pub fn begin_turn(
    state: &ConversationState,
    sessions: &[AgentSession],
    begin: BeginTurn,
    now: Timestamp,
) -> Result<ConversationChange, ConversationError> {
    let input_turn_ids = match plan_wake(state, now) {
        WakePlan::Wake { input_turn_ids } => input_turn_ids,
        WakePlan::Busy { run_id } => return Err(ConversationError::Busy(run_id)),
        WakePlan::Idle | WakePlan::Wait { .. } => return Err(ConversationError::NothingToWake),
    };
    let conversation = &state.conversation;
    let owner = SessionOwner::Conversation(&conversation.id);
    let check = begin.resume_check.as_ref();
    let plan = session::plan_launch_for(sessions, &begin.run_id, owner, &begin.profile_id, &begin.working_dir, check)
        .map_err(|busy| ConversationError::SessionBusy { session_id: busy.session_id, run_id: busy.run_id })?;
    let facts = LaunchFacts {
        run_id: &begin.run_id,
        workspace_id: &conversation.workspace_id,
        owner,
        profile_id: &begin.profile_id,
        working_dir: &begin.working_dir,
    };
    let mut edits = SessionEdits::default();
    let (session_id, resume_native) = session::apply_plan(&mut edits, sessions, plan, check, &facts, now);
    let session = edits.sessions.iter().find(|s| s.id == session_id).or_else(|| sessions.iter().find(|s| s.id == session_id));
    let history_gap = session.and_then(|s| s.history_gap.clone());

    let mut change = ConversationChange::default();
    let inputs: Vec<ConversationTurn> = input_turn_ids
        .iter()
        .filter_map(|id| state.turns.iter().find(|t| &t.id == id))
        .map(|turn| ConversationTurn {
            status: TurnStatus::Processing,
            controller_run_id: Some(begin.run_id.clone()),
            attempts: turn.attempts + 1,
            ..turn.clone()
        })
        .collect();
    let prompt = build_prompt(state, &inputs, resume_native.is_some(), history_gap.as_deref());
    let run = ControllerRun {
        id: begin.run_id.clone(),
        conversation_id: conversation.id.clone(),
        workspace_id: conversation.workspace_id.clone(),
        profile_id: begin.profile_id.clone(),
        status: ControllerRunStatus::Starting,
        version: 0,
        command_id: begin.command_id.clone(),
        prompt,
        working_dir: begin.working_dir.clone(),
        model: begin.model.clone(),
        input_turn_ids: input_turn_ids.clone(),
        agent_session_id: session_id.clone(),
        resume_native_session_id: resume_native.clone(),
        native_session_id: None,
        process: None,
        end_reason: None,
        created_at: now,
        updated_at: now,
        started_at: None,
        ended_at: None,
    };
    let payload = serde_json::json!({
        "conversation_id": conversation.id,
        "profile_id": begin.profile_id,
        "command_id": begin.command_id,
        "agent_session_id": session_id,
        "resume_native_session_id": resume_native,
        "input_turn_ids": input_turn_ids,
    });
    change.run_event(run, EVENT_CONTROLLER_RUN_LAUNCH_STARTED, payload, now);
    let payload = serde_json::json!({ "controller_run_id": begin.run_id, "input_turn_ids": input_turn_ids });
    change.conversation_event(conversation, EVENT_CONVERSATION_INPUTS_DISPATCHED, payload, now);
    change.turns = inputs;
    change.absorb_sessions(edits);
    Ok(change)
}

/// 組出這一輪的 Prompt。沿用原生 Session 時只需要本輪的輸入；新的（含替代的）Session 沒有先前的原生對話，
/// 附上角色說明與已處理的對話作為背景，並明示背景不要重新執行。
pub fn build_prompt(
    state: &ConversationState,
    inputs: &[ConversationTurn],
    resuming: bool,
    history_gap: Option<&str>,
) -> String {
    let mut prompt = String::new();
    if !resuming {
        prompt.push_str(
            "你是 Baton 這個 Workspace 的 Controller：與使用者長期對話、釐清目標、拆工、協調 Worker 並評估成果。\n\
             你不是常駐程序：每當有新的使用者訊息或工作事件，Runtime 才會喚起你一輪。\n\
             你不直接執行指令、寫檔或資料庫，也不自己啟動 Worker；一律以 Baton 的工具（propose_task、update_task、\
             add_dependency、dispatch_task、cancel_task、propose_decision_request）提出變更，由 Runtime 驗證後執行。\n\
             邊釐清、邊調查：分清已確認的事項、假設與缺口，資訊不足的契約欄位標為 unknown，不要虛構需求；\
             只以 propose_decision_request 問會阻擋下一步的關鍵問題並列出依賴答案的 Task，不依賴答案的研究可以先派出。\n\
             專案理解：先以 get_project_understanding 查看；Workspace 首次接入專案（還沒有任何項目）時，\
             先從 repo 與文件找答案，以 record_understanding 逐項建立 Project Brief、Glossary 與 Project Map，之後持續維護。\
             每項附來源（Workspace 內的檔案路徑，或附版本的外部來源）；只有被來源支持的描述才標為 confirmed，\
             推測與未確認的關係標為 hypothesis，不知道的標為 unknown。\n",
        );
        if let Some(gap) = history_gap {
            prompt.push_str(&format!("\n注意：{gap}\n"));
        }
        let history: Vec<&ConversationTurn> =
            state.turns.iter().filter(|t| t.status == TurnStatus::Processed).collect();
        if !history.is_empty() {
            prompt.push_str("\n## 先前的對話（都已處理，只作為背景，不要重新執行其中的指示）\n\n");
            let skipped = history.len().saturating_sub(HISTORY_LIMIT);
            if skipped > 0 {
                prompt.push_str(&format!("（更早的 {skipped} 則省略）\n"));
            }
            for turn in &history[skipped..] {
                prompt.push_str(&format!("- {}：{}\n", speaker(turn.kind), turn.content));
            }
        }
    }
    let heading = match inputs.first().map(|t| t.kind) {
        Some(TurnKind::WorkEvent) => "## 本輪要處理的工作事件",
        _ => "## 本輪要處理的使用者訊息",
    };
    prompt.push_str(&format!("\n{heading}（依到達順序）\n\n"));
    for (index, turn) in inputs.iter().enumerate() {
        prompt.push_str(&format!("{}. {}\n", index + 1, turn.content));
        if turn.attempts > 1 {
            prompt.push_str(
                "   （這則先前已交給 Controller，但那一輪沒有完成或結果沒有保存；可能已部分處理，請先核對目前狀態，不要重複已完成的動作。）\n",
            );
        }
    }
    prompt
}

fn speaker(kind: TurnKind) -> &'static str {
    match kind {
        TurnKind::UserMessage => "使用者",
        TurnKind::WorkEvent => "工作事件",
        TurnKind::ControllerReply => "Controller",
    }
}

// ---------------------------------------------------------------- 執行中

fn ensure_active(run: &ControllerRun, action: &'static str) -> Result<(), ConversationError> {
    if run.status.is_terminal() {
        return Err(ConversationError::TransitionInvalid { run_id: run.id.clone(), from: run.status.as_str(), action });
    }
    Ok(())
}

/// 程序已啟動：寫回回執（程序身分）。已經寫過同一個回執時沒有變動。
pub fn record_receipt(run: &ControllerRun, process: ProcessRecord, now: Timestamp) -> Result<ConversationChange, ConversationError> {
    ensure_active(run, "寫回回執")?;
    let mut change = ConversationChange::default();
    if run.status == ControllerRunStatus::Running {
        return Ok(change);
    }
    let payload = serde_json::json!({ "pid": process.pid, "created_at_millis": process.created_at_millis });
    let run = ControllerRun {
        status: ControllerRunStatus::Running,
        process: Some(process),
        started_at: Some(now),
        ..run.clone()
    };
    change.run_event(run, EVENT_CONTROLLER_RUN_STARTED, payload, now);
    Ok(change)
}

/// Adapter 回報原生 Session 識別：條件綁定到這一輪的平台 Session（規則同 Worker，票 15）。
pub fn bind_native(
    run: &ControllerRun,
    sessions: &[AgentSession],
    native_session_id: &str,
    now: Timestamp,
) -> Result<(ConversationChange, BindOutcome), ConversationError> {
    ensure_active(run, "綁定原生 Session")?;
    let mut change = ConversationChange::default();
    if run.native_session_id.as_deref() == Some(native_session_id) {
        return Ok((change, BindOutcome::Bound));
    }
    let mut edits = SessionEdits::default();
    let outcome = session::bind_native(&mut edits, sessions, &run.agent_session_id, &run.id, native_session_id, now);
    let payload = serde_json::json!({ "native_session_id": native_session_id, "agent_session_id": run.agent_session_id });
    match outcome {
        BindOutcome::Bound => {
            let run = ControllerRun { native_session_id: Some(native_session_id.to_string()), ..run.clone() };
            change.run_event(run, EVENT_CONTROLLER_RUN_NATIVE_SESSION_BOUND, payload, now);
        }
        BindOutcome::Mismatch | BindOutcome::InUse { .. } => {
            change.run_event(run.clone(), EVENT_CONTROLLER_RUN_NATIVE_SESSION_CONFLICT, payload, now);
        }
    }
    change.absorb_sessions(edits);
    Ok((change, outcome))
}

/// 一輪的結束方式。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnOutcome {
    /// 程序成功結束：輸入標為已處理，回覆（非空白時）成為一則 turn。
    Succeeded { reply: String },
    /// 程序以失敗結束：輸入退回待處理，退避後重新交付。
    Failed { reason: String },
    /// 程序不在了、啟動結果不明或被停止，結果沒有保存：輸入退回待處理。
    Interrupted { reason: String },
}

/// 結果保存：在同一個交易中結束 Controller Run、更新這一輪的輸入、寫入回覆並釋放 Session。
pub fn finish(
    state: &ConversationState,
    run: &ControllerRun,
    sessions: &[AgentSession],
    outcome: TurnOutcome,
    now: Timestamp,
) -> Result<ConversationChange, ConversationError> {
    ensure_active(run, "結束")?;
    let mut change = ConversationChange::default();
    // 只處理仍由這一輪持有的輸入
    let held: Vec<&ConversationTurn> = state
        .turns
        .iter()
        .filter(|t| t.status == TurnStatus::Processing && t.controller_run_id.as_deref() == Some(run.id.as_str()))
        .collect();
    let held_ids: Vec<String> = held.iter().map(|t| t.id.clone()).collect();
    let (status, event, reason) = match &outcome {
        TurnOutcome::Succeeded { reply } => {
            change.turns = held
                .iter()
                .map(|t| ConversationTurn { status: TurnStatus::Processed, processed_at: Some(now), ..(*t).clone() })
                .collect();
            let payload = serde_json::json!({ "controller_run_id": run.id, "turn_ids": held_ids });
            change.conversation_event(&state.conversation, EVENT_CONVERSATION_INPUTS_PROCESSED, payload, now);
            let reply = reply.trim();
            if !reply.is_empty() {
                let seq = state.last_seq() + 1;
                change.turns.push(ConversationTurn {
                    id: turn_id_for(&state.conversation.id, seq),
                    conversation_id: state.conversation.id.clone(),
                    seq,
                    kind: TurnKind::ControllerReply,
                    content: reply.to_string(),
                    status: TurnStatus::Processed,
                    controller_run_id: Some(run.id.clone()),
                    attempts: 0,
                    source: None,
                    created_at: now,
                    processed_at: Some(now),
                });
            }
            (ControllerRunStatus::Succeeded, EVENT_CONTROLLER_RUN_SUCCEEDED, "exited_successfully".to_string())
        }
        TurnOutcome::Failed { reason } | TurnOutcome::Interrupted { reason } => {
            change.turns = held
                .iter()
                .map(|t| ConversationTurn { status: TurnStatus::Pending, controller_run_id: None, ..(*t).clone() })
                .collect();
            let payload = serde_json::json!({ "controller_run_id": run.id, "turn_ids": held_ids, "reason": reason });
            change.conversation_event(&state.conversation, EVENT_CONVERSATION_INPUTS_RELEASED, payload, now);
            match &outcome {
                TurnOutcome::Failed { .. } => (ControllerRunStatus::Failed, EVENT_CONTROLLER_RUN_FAILED, reason.clone()),
                _ => (ControllerRunStatus::Interrupted, EVENT_CONTROLLER_RUN_INTERRUPTED, reason.clone()),
            }
        }
    };
    let ended = ControllerRun { status, end_reason: Some(reason.clone()), ended_at: Some(now), ..run.clone() };
    change.run_event(ended, event, serde_json::json!({ "reason": reason }), now);
    let mut edits = SessionEdits::default();
    session::release_driver(&mut edits, sessions, &run.agent_session_id, &run.id, now);
    change.absorb_sessions(edits);
    Ok(change)
}

/// 工作事件的內容：Worker 的 Run 成功、失敗或回報受阻（Store 在同一個交易中由 Run 事件產生）。
pub fn work_event_content(event_type: &str, task_title: &str, task_id: &str, run_id: &str, detail: Option<&str>) -> Option<String> {
    let what = match event_type {
        crate::run::EVENT_RUN_SUCCEEDED => "Worker 的 Run 成功結束，成果待評估",
        crate::run::EVENT_RUN_FAILED => "Worker 的 Run 以失敗結束",
        crate::run::EVENT_RUN_BLOCKED_REPORTED => "Worker 回報受阻",
        _ => return None,
    };
    let detail = detail.filter(|d| !d.trim().is_empty()).map(|d| format!("：{d}")).unwrap_or_default();
    Some(format!("Task「{task_title}」（{task_id}）{what}（Run {run_id}）{detail}"))
}

#[cfg(test)]
mod tests;
