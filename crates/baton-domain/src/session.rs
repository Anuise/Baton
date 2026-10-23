//! Agent Session：平台 Session、原生 Session 與程序身分分開保存；沿用、替換與對話紀錄匯入（票 15，issue §3.1～3.3）。
//!
//! 純邏輯、無 I/O。Session 的狀態只隨 Run 的命令改變（`crate::run::decide` 呼叫這裡的函式），
//! 由 Store 在同一個交易中套用。
//!
//! 幾條貫穿全檔的規則：
//! - **三種識別不混用**：`AgentSession::id` 是平台產生的穩定主鍵；`native_session_id` 是執行工具回報的原生識別，
//!   取得前為空、不虛構；程序身分（PID + 建立時間 + 指令雜湊）留在 Run 上，只用來查驗存活。
//! - **何時決定沿用或替換**：Supervisor 寫入 launch intent（`BeginLaunch`）的同一個交易中決定（`plan_launch`），
//!   所以原生識別取得前，平台 Session、launch intent 與 command_id 已一起持久化。
//! - **沿用的條件**：同一個 Task 最近的 Session、同一個帳號、同一個工作目錄、已有原生識別、暫停中、沒有執行者，
//!   而且 Adapter 查驗結果是「可以接續」。查驗結果未知時不宣稱可續用，改建替代 Session。
//! - **替換不覆蓋**：替代 Session 保存 predecessor 與原因，前任標為 `closed`（不再相容）或 `unavailable`（原生 Session 不可用），
//!   原本的識別不改；替代 Session 一律標示紀錄缺口（`history_gap`），不宣稱已完整恢復。
//! - **同一原生 Session 只有一個執行者**：平台 Session 以 `driver_run_id` 記錄目前驅動它的 Run；
//!   同一個命名空間（執行工具 + 帳號）內的原生識別不會綁到兩個平台 Session。

use serde::{Deserialize, Serialize};

use crate::event::DomainEvent;
use crate::workspace::AgentRole;
use crate::Timestamp;

pub const SESSION_AGGREGATE: &str = "agent_session";
/// 第一個（目前唯一的）執行工具：Windows 原生 Claude Code。
pub const RUNTIME_CLAUDE_CODE: &str = "claude_code";

pub const EVENT_SESSION_CREATED: &str = "session_created";
pub const EVENT_SESSION_NATIVE_BOUND: &str = "session_native_bound";
pub const EVENT_SESSION_NATIVE_CONFLICT: &str = "session_native_conflict";
pub const EVENT_SESSION_RESUMED: &str = "session_resumed";
pub const EVENT_SESSION_SUSPENDED: &str = "session_suspended";
pub const EVENT_SESSION_UNAVAILABLE: &str = "session_unavailable";
pub const EVENT_SESSION_CLOSED: &str = "session_closed";

/// Run 回報的原生 Session 已經由另一個執行中的 Run 驅動：後到的 Run 被要求停止（`run_cancel_requested` 的原因）。
pub const NATIVE_SESSION_IN_USE: &str = "native_session_in_use";

/// 遮蔽後代替秘密的文字（與共同設定的遮蔽一致）。
pub const REDACTED: &str = "<已遮蔽>";

// ---------------------------------------------------------------- 狀態

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    /// 已建立、正在啟動，尚未取得原生識別。
    Allocating,
    /// 已綁定原生識別，有一個 Run 正在驅動它。
    Active,
    /// 驅動它的 Run 已結束；可能可以接續，但不保證程序已死或原生 Session 仍在。
    Suspended,
    /// 不再接新 Run（例如換了帳號或工作目錄），保留歷史。
    Closed,
    /// 原生 Session 遺失、不相容或從未取得。
    Unavailable,
}

impl SessionState {
    pub const ALL: [SessionState; 5] = [
        SessionState::Allocating,
        SessionState::Active,
        SessionState::Suspended,
        SessionState::Closed,
        SessionState::Unavailable,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            SessionState::Allocating => "allocating",
            SessionState::Active => "active",
            SessionState::Suspended => "suspended",
            SessionState::Closed => "closed",
            SessionState::Unavailable => "unavailable",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.as_str() == raw)
    }
}

/// Adapter 查驗原生 Session 能否接續的結果（對應 Port 的 `Capability`；Domain 不依賴 Port）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResumeCapability {
    Resumable,
    NotResumable,
    Unknown,
}

impl ResumeCapability {
    pub const ALL: [ResumeCapability; 3] =
        [ResumeCapability::Resumable, ResumeCapability::NotResumable, ResumeCapability::Unknown];

    pub fn as_str(self) -> &'static str {
        match self {
            ResumeCapability::Resumable => "resumable",
            ResumeCapability::NotResumable => "not_resumable",
            ResumeCapability::Unknown => "unknown",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.as_str() == raw)
    }
}

/// 為什麼不沿用前一個 Session（替代 Session 的 `replacement_reason`，也是前任的 `state_reason`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplacementReason {
    AccountChanged,
    WorkingDirectoryChanged,
    NativeSessionNeverBound,
    NativeSessionUnavailable,
    ResumeCapabilityUnknown,
    /// 前任早已關閉或不可用，而且沒有記錄原因。
    PredecessorNotResumable,
}

impl ReplacementReason {
    pub const ALL: [ReplacementReason; 6] = [
        ReplacementReason::AccountChanged,
        ReplacementReason::WorkingDirectoryChanged,
        ReplacementReason::NativeSessionNeverBound,
        ReplacementReason::NativeSessionUnavailable,
        ReplacementReason::ResumeCapabilityUnknown,
        ReplacementReason::PredecessorNotResumable,
    ];

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|r| r.as_str() == raw)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ReplacementReason::AccountChanged => "account_changed",
            ReplacementReason::WorkingDirectoryChanged => "working_directory_changed",
            ReplacementReason::NativeSessionNeverBound => "native_session_never_bound",
            ReplacementReason::NativeSessionUnavailable => "native_session_unavailable",
            ReplacementReason::ResumeCapabilityUnknown => "resume_capability_unknown",
            ReplacementReason::PredecessorNotResumable => "predecessor_not_resumable",
        }
    }

    fn describe(self) -> &'static str {
        match self {
            ReplacementReason::AccountChanged => "換了帳號，原生 Session 不能跨帳號接續",
            ReplacementReason::WorkingDirectoryChanged => "工作目錄不同，原生 Session 不相容",
            ReplacementReason::NativeSessionNeverBound => "前一個 Run 在取得原生 Session 識別之前就結束",
            ReplacementReason::NativeSessionUnavailable => "原生 Session 已不可用（遺失或無法讀取）",
            ReplacementReason::ResumeCapabilityUnknown => "無法確認原生 Session 能否接續",
            ReplacementReason::PredecessorNotResumable => "前一個 Session 已關閉或不可用",
        }
    }

    /// 前任 Session 的去向：原生 Session 本身有問題的標為不可用，其餘只是不再相容，標為關閉。
    fn predecessor_state(self) -> SessionState {
        match self {
            ReplacementReason::NativeSessionNeverBound | ReplacementReason::NativeSessionUnavailable => {
                SessionState::Unavailable
            }
            _ => SessionState::Closed,
        }
    }
}

// ---------------------------------------------------------------- 實體

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSession {
    /// 平台 Session 主鍵（agent_session_id）。
    pub id: String,
    pub runtime_type: String,
    pub workspace_id: String,
    pub role: AgentRole,
    /// Worker Session 綁定的 Task；Controller Session 改綁 conversation（票 16）。
    pub task_id: Option<String>,
    pub conversation_id: Option<String>,
    /// 原生 Session 所在的帳號（account_profile）；原生識別的唯一性以「執行工具 + 帳號」為範圍。
    pub profile_id: String,
    pub working_directory: String,
    pub native_session_id: Option<String>,
    pub state: SessionState,
    /// 關閉或不可用的原因代碼（`ReplacementReason::as_str`）。
    pub state_reason: Option<String>,
    /// 每個事件加一；事件的 `aggregate_version` 與它對應。
    pub version: i64,
    /// 目前驅動這個 Session 的 Run；只在 `allocating`／`active` 時有值。
    pub driver_run_id: Option<String>,
    pub predecessor_session_id: Option<String>,
    pub replacement_reason: Option<String>,
    /// 紀錄缺口的說明：替代 Session 沒有延續前任的原生對話，只能從平台保存的狀態重建。
    pub history_gap: Option<String>,
    /// 最近一次查驗的接續能力與時間；沒有查驗過為 `None`。
    pub resume_capability: Option<ResumeCapability>,
    pub capability_checked_at: Option<Timestamp>,
    pub created_at: Timestamp,
    pub last_activity_at: Timestamp,
}

/// Supervisor 寫入 launch intent 前對候選 Session 查驗接續能力的結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeCheck {
    pub session_id: String,
    pub capability: ResumeCapability,
}

/// 新 Session 的識別：每個 Run 最多在啟動時建立一個 Session，所以由 Run 識別導出（重送同一個命令得到同一個識別）。
pub fn session_id_for_run(run_id: &str) -> String {
    format!("ses_{}", run_id.strip_prefix("run_").unwrap_or(run_id))
}

/// Session 的歸屬：Worker Session 綁 Task。升級前 Controller 輪次的 Session 綁 conversation（票 16），
/// 對話退役後只保留在資料庫（`AgentRole::Controller`、`conversation_id`），不再建立或接續。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionOwner<'a> {
    Task(&'a str),
}

impl SessionOwner<'_> {
    fn owns(&self, session: &AgentSession) -> bool {
        match self {
            SessionOwner::Task(task_id) => session.task_id.as_deref() == Some(*task_id),
        }
    }
}

/// 一個 Task 最近建立的 Session（`sessions` 依建立順序）。
pub fn latest_for_task<'a>(sessions: &'a [AgentSession], task_id: &str) -> Option<&'a AgentSession> {
    latest_for(sessions, SessionOwner::Task(task_id))
}

/// 某個 Task 最近建立的 Session（`sessions` 依建立順序）。
pub fn latest_for<'a>(sessions: &'a [AgentSession], owner: SessionOwner<'_>) -> Option<&'a AgentSession> {
    sessions.iter().rev().find(|s| owner.owns(s))
}

// ---------------------------------------------------------------- 啟動時的決定

/// 啟動一個 Run 時要沿用哪個 Session，或建立新的。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionPlan {
    /// 沿用（原生接續）這個 Session。
    Resume { session_id: String },
    /// 建立新 Session；有前任時保存 predecessor 與原因。
    Create { predecessor: Option<(String, ReplacementReason)> },
}

/// 這個 Task 最近的 Session 還有別的 Run 在驅動（寫入所有權本來就會擋下，這裡是第二道防線）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionBusy {
    pub session_id: String,
    pub run_id: String,
}

/// 啟動這個 Run 時要查驗接續能力的 Session：同一個 Task 最近的 Session，而且除了接續能力以外都相容。
/// 沒有候選時不需要查驗（直接建立新 Session）。
pub fn resume_candidate<'a>(
    sessions: &'a [AgentSession],
    task_id: &str,
    profile_id: &str,
    working_dir: &str,
) -> Option<&'a AgentSession> {
    resume_candidate_for(sessions, SessionOwner::Task(task_id), profile_id, working_dir)
}

/// 同 `resume_candidate`，但以 `SessionOwner` 指定歸屬。
pub fn resume_candidate_for<'a>(
    sessions: &'a [AgentSession],
    owner: SessionOwner<'_>,
    profile_id: &str,
    working_dir: &str,
) -> Option<&'a AgentSession> {
    let latest = latest_for(sessions, owner)?;
    incompatibility(latest, profile_id, working_dir).is_none().then_some(latest)
}

/// 除了接續能力以外，不能沿用的原因。
fn incompatibility(session: &AgentSession, profile_id: &str, working_dir: &str) -> Option<ReplacementReason> {
    // 早已關閉或不可用：沿用當時記下的原因
    if matches!(session.state, SessionState::Closed | SessionState::Unavailable) {
        let recorded = session.state_reason.as_deref().and_then(ReplacementReason::parse);
        return Some(recorded.unwrap_or(ReplacementReason::PredecessorNotResumable));
    }
    if session.profile_id != profile_id {
        return Some(ReplacementReason::AccountChanged);
    }
    if session.working_directory != working_dir {
        return Some(ReplacementReason::WorkingDirectoryChanged);
    }
    if session.native_session_id.is_none() {
        return Some(ReplacementReason::NativeSessionNeverBound);
    }
    None
}

/// 決定這個 Run 要沿用哪個 Session。`check` 是 Supervisor 對 `resume_candidate` 的查驗結果；
/// 沒有查驗、查驗的不是這個候選，或結果未知時，都不宣稱可續用。
pub fn plan_launch(
    sessions: &[AgentSession],
    run_id: &str,
    task_id: &str,
    profile_id: &str,
    working_dir: &str,
    check: Option<&ResumeCheck>,
) -> Result<SessionPlan, SessionBusy> {
    plan_launch_for(sessions, run_id, SessionOwner::Task(task_id), profile_id, working_dir, check)
}

/// 同 `plan_launch`，但以 `SessionOwner` 指定歸屬。
pub fn plan_launch_for(
    sessions: &[AgentSession],
    run_id: &str,
    owner: SessionOwner<'_>,
    profile_id: &str,
    working_dir: &str,
    check: Option<&ResumeCheck>,
) -> Result<SessionPlan, SessionBusy> {
    let Some(latest) = latest_for(sessions, owner) else {
        return Ok(SessionPlan::Create { predecessor: None });
    };
    if let Some(driver) = latest.driver_run_id.as_ref().filter(|d| d.as_str() != run_id) {
        return Err(SessionBusy { session_id: latest.id.clone(), run_id: driver.clone() });
    }
    let reason = match incompatibility(latest, profile_id, working_dir) {
        Some(reason) => reason,
        None => match check.filter(|c| c.session_id == latest.id).map(|c| c.capability) {
            Some(ResumeCapability::Resumable) => return Ok(SessionPlan::Resume { session_id: latest.id.clone() }),
            Some(ResumeCapability::NotResumable) => ReplacementReason::NativeSessionUnavailable,
            Some(ResumeCapability::Unknown) | None => ReplacementReason::ResumeCapabilityUnknown,
        },
    };
    Ok(SessionPlan::Create { predecessor: Some((latest.id.clone(), reason)) })
}

// ---------------------------------------------------------------- 變更

/// 一次命令中被改變的 Session 與它們的事件；每個事件讓該 Session 的版本加一。
#[derive(Debug, Default)]
pub struct SessionEdits {
    pub sessions: Vec<AgentSession>,
    pub events: Vec<DomainEvent>,
}

impl SessionEdits {
    fn push(&mut self, mut session: AgentSession, event_type: &'static str, payload: serde_json::Value, now: Timestamp) {
        session.version += 1;
        session.last_activity_at = now;
        self.events.push(DomainEvent {
            aggregate_type: SESSION_AGGREGATE,
            aggregate_id: session.id.clone(),
            aggregate_version: session.version,
            event_type,
            occurred_at: now,
            payload,
        });
        match self.sessions.iter_mut().find(|s| s.id == session.id) {
            Some(existing) => *existing = session,
            None => self.sessions.push(session),
        }
    }

    /// 這次命令中已改過的版本優先，否則用載入的版本。
    fn current(&self, loaded: &AgentSession) -> AgentSession {
        self.sessions.iter().find(|s| s.id == loaded.id).cloned().unwrap_or_else(|| loaded.clone())
    }
}

/// 啟動一個 Run 時要建立的 Session 所需的事實。
pub struct LaunchFacts<'a> {
    pub run_id: &'a str,
    pub workspace_id: &'a str,
    pub owner: SessionOwner<'a>,
    pub profile_id: &'a str,
    pub working_dir: &'a str,
}

/// 套用啟動時的決定：回傳 Run 要綁定的 Session 識別，以及要接續的原生識別（沿用時）。
pub fn apply_plan(
    edits: &mut SessionEdits,
    sessions: &[AgentSession],
    plan: SessionPlan,
    check: Option<&ResumeCheck>,
    facts: &LaunchFacts<'_>,
    now: Timestamp,
) -> (String, Option<String>) {
    // 查驗結果記在被查驗的 Session 上，未知時也記，供日後查證
    let record_check = |session: &mut AgentSession| {
        if let Some(check) = check.filter(|c| c.session_id == session.id) {
            session.resume_capability = Some(check.capability);
            session.capability_checked_at = Some(now);
        }
    };
    match plan {
        SessionPlan::Resume { session_id } => {
            let loaded = sessions.iter().find(|s| s.id == session_id).expect("plan_launch 只會沿用載入的 Session");
            let mut session = edits.current(loaded);
            record_check(&mut session);
            session.state = SessionState::Active;
            session.driver_run_id = Some(facts.run_id.to_string());
            let native = session.native_session_id.clone();
            let payload = serde_json::json!({ "run_id": facts.run_id, "native_session_id": native });
            edits.push(session, EVENT_SESSION_RESUMED, payload, now);
            (session_id, native)
        }
        SessionPlan::Create { predecessor } => {
            let id = session_id_for_run(facts.run_id);
            let mut history_gap = None;
            if let Some((predecessor_id, reason)) = &predecessor {
                let loaded = sessions.iter().find(|s| &s.id == predecessor_id).expect("plan_launch 只會引用載入的 Session");
                let mut old = edits.current(loaded);
                record_check(&mut old);
                history_gap = Some(format!(
                    "替代 Session：{}。前一個 Session {predecessor_id} 的原生對話沒有延續，只能從平台保存的檢查點、訊息與成果重建，紀錄有缺口",
                    reason.describe()
                ));
                // 早已關閉或不可用的前任維持原狀與原因；其餘依原因標為關閉或不可用
                if !matches!(old.state, SessionState::Closed | SessionState::Unavailable) {
                    let to = reason.predecessor_state();
                    old.state = to;
                    old.state_reason = Some(reason.as_str().to_string());
                    let event = if to == SessionState::Closed { EVENT_SESSION_CLOSED } else { EVENT_SESSION_UNAVAILABLE };
                    let payload = serde_json::json!({ "reason": reason.as_str(), "successor_session_id": id });
                    edits.push(old, event, payload, now);
                }
            }
            let SessionOwner::Task(task_id) = facts.owner;
            let session = AgentSession {
                id: id.clone(),
                runtime_type: RUNTIME_CLAUDE_CODE.to_string(),
                workspace_id: facts.workspace_id.to_string(),
                role: AgentRole::Worker,
                task_id: Some(task_id.to_string()),
                conversation_id: None,
                profile_id: facts.profile_id.to_string(),
                working_directory: facts.working_dir.to_string(),
                native_session_id: None,
                state: SessionState::Allocating,
                state_reason: None,
                version: 0,
                driver_run_id: Some(facts.run_id.to_string()),
                predecessor_session_id: predecessor.as_ref().map(|(p, _)| p.clone()),
                replacement_reason: predecessor.as_ref().map(|(_, r)| r.as_str().to_string()),
                history_gap,
                resume_capability: None,
                capability_checked_at: None,
                created_at: now,
                last_activity_at: now,
            };
            let payload = serde_json::json!({
                "run_id": facts.run_id,
                "profile_id": facts.profile_id,
                "working_directory": facts.working_dir,
                "predecessor_session_id": session.predecessor_session_id,
                "replacement_reason": session.replacement_reason,
            });
            edits.push(session, EVENT_SESSION_CREATED, payload, now);
            (id, None)
        }
    }
}

/// Run 回報原生識別後，平台 Session 的條件綁定結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindOutcome {
    /// 已綁定（第一次）或本來就是同一個原生識別。
    Bound,
    /// 平台 Session 已綁定另一個原生識別：保留原綁定，只記錄衝突。
    Mismatch,
    /// 同一個命名空間內，另一個平台 Session 已經綁定這個原生識別；`driven` 表示它目前有 Run 在驅動。
    InUse { other_session_id: String, driven: bool },
}

/// 把原生識別條件綁定到 Run 的平台 Session。`sessions` 必須包含同一命名空間內綁定了這個原生識別的 Session（可跨 Workspace）。
pub fn bind_native(
    edits: &mut SessionEdits,
    sessions: &[AgentSession],
    session_id: &str,
    run_id: &str,
    native_session_id: &str,
    now: Timestamp,
) -> BindOutcome {
    let Some(loaded) = sessions.iter().find(|s| s.id == session_id) else { return BindOutcome::Bound };
    let mut session = edits.current(loaded);
    match session.native_session_id.as_deref() {
        Some(bound) if bound == native_session_id => return BindOutcome::Bound,
        Some(bound) => {
            let payload = serde_json::json!({ "run_id": run_id, "bound": bound, "reported": native_session_id });
            edits.push(session, EVENT_SESSION_NATIVE_CONFLICT, payload, now);
            return BindOutcome::Mismatch;
        }
        None => {}
    }
    let other = sessions.iter().find(|s| {
        s.id != session.id
            && s.runtime_type == session.runtime_type
            && s.profile_id == session.profile_id
            && s.native_session_id.as_deref() == Some(native_session_id)
    });
    if let Some(other) = other {
        let driven = other.driver_run_id.is_some();
        let payload = serde_json::json!({
            "run_id": run_id,
            "reported": native_session_id,
            "bound_to_session_id": other.id,
            "other_driver_run_id": other.driver_run_id,
        });
        let outcome = BindOutcome::InUse { other_session_id: other.id.clone(), driven };
        edits.push(session, EVENT_SESSION_NATIVE_CONFLICT, payload, now);
        return outcome;
    }
    session.native_session_id = Some(native_session_id.to_string());
    if session.state == SessionState::Allocating {
        session.state = SessionState::Active;
    }
    let payload = serde_json::json!({ "run_id": run_id, "native_session_id": native_session_id });
    edits.push(session, EVENT_SESSION_NATIVE_BOUND, payload, now);
    BindOutcome::Bound
}

/// 驅動 Session 的 Run 結束：有原生識別的暫停（可能可以接續），從未取得的標為不可用。
pub fn release_driver(
    edits: &mut SessionEdits,
    sessions: &[AgentSession],
    session_id: &str,
    run_id: &str,
    now: Timestamp,
) {
    let Some(loaded) = sessions.iter().find(|s| s.id == session_id) else { return };
    let mut session = edits.current(loaded);
    if session.driver_run_id.as_deref() != Some(run_id) {
        return;
    }
    session.driver_run_id = None;
    if session.native_session_id.is_some() {
        session.state = SessionState::Suspended;
        edits.push(session, EVENT_SESSION_SUSPENDED, serde_json::json!({ "run_id": run_id }), now);
    } else {
        let reason = ReplacementReason::NativeSessionNeverBound.as_str();
        session.state = SessionState::Unavailable;
        session.state_reason = Some(reason.to_string());
        edits.push(session, EVENT_SESSION_UNAVAILABLE, serde_json::json!({ "run_id": run_id, "reason": reason }), now);
    }
}

// ---------------------------------------------------------------- 對話紀錄

/// Adapter 回報的一筆對話紀錄。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptEntry {
    pub position: u64,
    pub line: String,
}

/// 一段要寫入的對話紀錄：已遮蔽的內容，或一段沒收到的位置（缺口）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SegmentPlan {
    Stored { first_position: u64, last_position: u64, content: String, entry_count: u64, redactions: u64 },
    Missing { first_position: u64, last_position: u64 },
}

/// 規劃一批對話紀錄的匯入：`imported_through` 是這個 Run 已保存到的最後位置（含缺口）。
///
/// 已保存過的位置（重新接上串流時的重播）與同一批內重複的位置都丟掉；位置跳號時先記一段缺口，
/// 讓「沒有匯入的部分」明確標示出來。每一行在保存之前遮蔽看起來像秘密的內容。
pub fn plan_transcript_import(imported_through: u64, entries: &[TranscriptEntry]) -> Vec<SegmentPlan> {
    let mut sorted: Vec<&TranscriptEntry> = entries.iter().filter(|e| e.position > imported_through).collect();
    sorted.sort_by_key(|e| e.position);
    sorted.dedup_by_key(|e| e.position);

    let mut plans = Vec::new();
    let mut expected = imported_through + 1;
    let mut current: Option<(u64, u64, String, u64, u64)> = None;
    let flush = |current: &mut Option<(u64, u64, String, u64, u64)>, plans: &mut Vec<SegmentPlan>| {
        if let Some((first_position, last_position, content, entry_count, redactions)) = current.take() {
            plans.push(SegmentPlan::Stored { first_position, last_position, content, entry_count, redactions });
        }
    };
    for entry in sorted {
        if entry.position > expected {
            flush(&mut current, &mut plans);
            plans.push(SegmentPlan::Missing { first_position: expected, last_position: entry.position - 1 });
        }
        let (line, count) = redact_line(&entry.line);
        let segment = current.get_or_insert((entry.position, entry.position, String::new(), 0, 0));
        segment.1 = entry.position;
        segment.2.push_str(&line);
        segment.2.push('\n');
        segment.3 += 1;
        segment.4 += count;
        expected = entry.position + 1;
    }
    flush(&mut current, &mut plans);
    plans
}

/// 對話紀錄的一段：已保存到受控成果儲存的內容，或一段沒收到的位置（缺口）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SegmentAvailability {
    Stored,
    Missing,
}

impl SegmentAvailability {
    pub fn as_str(self) -> &'static str {
        match self {
            SegmentAvailability::Stored => "stored",
            SegmentAvailability::Missing => "missing",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        [SegmentAvailability::Stored, SegmentAvailability::Missing].into_iter().find(|a| a.as_str() == raw)
    }
}

/// transcript_segments 的一列：同一個 Run 的位置區間不重疊，內容以雜湊命名存放在成果儲存（`storage_path` 相對成果根目錄）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptSegment {
    pub agent_session_id: String,
    pub run_id: String,
    pub first_position: u64,
    pub last_position: u64,
    pub availability: SegmentAvailability,
    pub storage_path: Option<String>,
    pub sha256: Option<String>,
    pub byte_len: Option<u64>,
    pub entry_count: u64,
    /// 匯入時遮蔽的秘密數量。
    pub redactions: u64,
    pub recorded_at: Timestamp,
}

/// 對話紀錄的保存進度：每個 Run 已保存到的最後位置（含缺口）與各種段落的數量。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TranscriptSummary {
    pub stored_segments: u64,
    pub missing_segments: u64,
    pub imported_through: Vec<RunTranscriptProgress>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunTranscriptProgress {
    pub run_id: String,
    pub last_position: u64,
}

impl TranscriptSummary {
    pub fn of(segments: &[TranscriptSegment]) -> Self {
        let mut summary = Self::default();
        for segment in segments {
            match segment.availability {
                SegmentAvailability::Stored => summary.stored_segments += 1,
                SegmentAvailability::Missing => summary.missing_segments += 1,
            }
            match summary.imported_through.iter_mut().find(|p| p.run_id == segment.run_id) {
                Some(progress) => progress.last_position = progress.last_position.max(segment.last_position),
                None => summary
                    .imported_through
                    .push(RunTranscriptProgress { run_id: segment.run_id.clone(), last_position: segment.last_position }),
            }
        }
        summary
    }
}

/// 面板與 API 看到的一個 Session：Session 本身、用過它的 Run，以及對話紀錄的保存進度。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionView {
    #[serde(flatten)]
    pub session: AgentSession,
    pub run_ids: Vec<String>,
    pub transcript: TranscriptSummary,
}

/// session_events 的一筆（Session 聚合的事件）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionEventRecord {
    pub event_type: String,
    pub aggregate_version: i64,
    pub occurred_at: Timestamp,
    pub payload: serde_json::Value,
}

/// 一個 Session 的完整紀錄：Session、事件與對話紀錄的段落。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionDetail {
    pub session: SessionView,
    pub events: Vec<SessionEventRecord>,
    pub segments: Vec<TranscriptSegment>,
}

/// 遮蔽一行對話紀錄中的秘密，回傳遮蔽後的內容與遮蔽的數量。JSON 行逐一遮蔽解碼後的字串值
/// （跳脫字元不會讓秘密躲過辨識），其他內容整行處理。
pub fn redact_line(line: &str) -> (String, u64) {
    let line = line.trim_end_matches(['\r', '\n']);
    match serde_json::from_str::<serde_json::Value>(line) {
        Ok(mut value) => {
            let count = redact_value(&mut value);
            if count == 0 {
                return (line.to_string(), 0);
            }
            (value.to_string(), count)
        }
        Err(_) => crate::shared_settings::redact_secrets(line, REDACTED),
    }
}

fn redact_value(value: &mut serde_json::Value) -> u64 {
    match value {
        serde_json::Value::String(text) => {
            let (redacted, count) = crate::shared_settings::redact_secrets(text, REDACTED);
            if count > 0 {
                *text = redacted;
            }
            count
        }
        serde_json::Value::Array(items) => items.iter_mut().map(redact_value).sum(),
        serde_json::Value::Object(entries) => entries.values_mut().map(redact_value).sum(),
        _ => 0,
    }
}

#[cfg(test)]
mod tests;
