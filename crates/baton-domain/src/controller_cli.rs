//! 互動式 Controller Run（desktop-controller-cli 票 03，ADR-0014）：使用者在 Controller 頁簽選一個 Account Profile
//! 按「開啟」，平台在外部終端機開出原生互動式 Claude Code（Controller CLI），以這個 Run 追蹤它的生命週期。
//!
//! 純邏輯、無 I/O。狀態機：
//! - `starting`：使用者要求開啟（已持久化）。Supervisor 在開窗之前先寫下 launch intent（`launched_at`），
//!   所以重啟後看到「已寫 launch intent、沒有回執」就知道開窗結果不明，只以 `command_id` 找回、不重複開窗。
//! - `running`：開窗回執（宿主程序身分）已寫回，或宿主已回報 `claude` 啟動。
//! - `ended`：宿主回報結束、宿主消失、使用者按結束、或開啟失敗。終態；Controller 憑證隨之失效。
//! - `interrupted`：Runtime 當掉恢復時的處理（票 07），本票不產生。
//!
//! 每個 Workspace 同時最多一個非終態的 Controller Run；同一個 Profile 可以同時是多個 Workspace 的 Controller 帳號。
//!
//! 票 05：同一個 Workspace 以同一個 Profile 重開時以 `--continue` 接回原生 Session（`continue_session`）；
//! 「換帳號重開」先要求結束目前的 Run（記下新 Profile），Run 結束的同一個交易中以新 Profile 開新的 Run（不接續）。

use serde::{Deserialize, Serialize};

use crate::event::DomainEvent;
use crate::profile::{self, AccountProfile};
use crate::run::ProcessRecord;
use crate::workspace::{Workspace, WorkspaceStatus};
use crate::Timestamp;

pub const CONTROLLER_CLI_AGGREGATE: &str = "controller_cli";
/// 使用者要求開啟（記下 Controller 帳號）。
pub const EVENT_CONTROLLER_CLI_OPENED: &str = "controller_cli_opened";
/// Supervisor 寫下 launch intent，接著開窗。
pub const EVENT_CONTROLLER_CLI_LAUNCH_STARTED: &str = "controller_cli_launch_started";
/// 開窗回執寫回或宿主回報 `claude` 已啟動：Controller CLI 執行中。
pub const EVENT_CONTROLLER_CLI_RUNNING: &str = "controller_cli_running";
/// 使用者按「結束」：等 Supervisor 結束宿主。
pub const EVENT_CONTROLLER_CLI_END_REQUESTED: &str = "controller_cli_end_requested";
/// 使用者按「換帳號重開」：等 Supervisor 結束宿主，Run 結束時以新 Profile 開新的 Run。
pub const EVENT_CONTROLLER_CLI_SWITCH_REQUESTED: &str = "controller_cli_switch_requested";
pub const EVENT_CONTROLLER_CLI_ENDED: &str = "controller_cli_ended";

/// 終端機內執行的 MCP shim 子命令：「Controller 宿主」，它啟動 `claude` 並回報開始與結束。
pub const HOST_SUBCOMMAND: &str = "controller-host";
/// 宿主回報 `claude` 已啟動（Agent API，只接受 Controller 憑證）。
pub const HOST_STARTED_PATH: &str = "/agent/v1/controller-host/started";
/// 宿主回報 `claude` 已結束（Agent API，只接受 Controller 憑證）。
pub const HOST_ENDED_PATH: &str = "/agent/v1/controller-host/ended";
/// Controller CLI 終端機視窗標題的前綴；後面接 Workspace 名稱。
pub const WINDOW_TITLE_PREFIX: &str = "Baton Controller";

/// 結束原因代碼（穩定、機器可讀）；頁簽依它顯示原因。
pub const END_HOST_EXITED: &str = "host_exited";
pub const END_HOST_GONE: &str = "host_gone";
pub const END_USER_ENDED: &str = "user_ended";
/// 換帳號重開：由 App 結束，接著以新 Profile 開新的 Run。
pub const END_SWITCHED: &str = "switched";
pub const END_CLAUDE_NOT_FOUND: &str = "claude_not_found";
pub const END_PROFILE_UNAVAILABLE: &str = "profile_unavailable";
pub const END_TERMINAL_FAILED: &str = "terminal_failed";
pub const END_LAUNCH_FAILED: &str = "launch_failed";
pub const END_LAUNCH_OUTCOME_UNKNOWN: &str = "launch_outcome_unknown";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControllerCliStatus {
    Starting,
    Running,
    Ended,
    Interrupted,
}

impl ControllerCliStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            ControllerCliStatus::Starting => "starting",
            ControllerCliStatus::Running => "running",
            ControllerCliStatus::Ended => "ended",
            ControllerCliStatus::Interrupted => "interrupted",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "starting" => Some(ControllerCliStatus::Starting),
            "running" => Some(ControllerCliStatus::Running),
            "ended" => Some(ControllerCliStatus::Ended),
            "interrupted" => Some(ControllerCliStatus::Interrupted),
            _ => None,
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, ControllerCliStatus::Ended | ControllerCliStatus::Interrupted)
    }
}

/// 一個互動式 Controller Run。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControllerCliRun {
    pub id: String,
    pub workspace_id: String,
    /// Controller 帳號。
    pub profile_id: String,
    pub status: ControllerCliStatus,
    /// 每個事件加一；事件的 `aggregate_version` 與它對應。
    pub version: i64,
    /// 這次開窗的識別：宿主以它命名程序群組，Runtime 重建後以它找回宿主，不重複開窗。
    pub command_id: String,
    /// Controller CLI 的工作目錄＝Workspace 目錄。
    pub working_dir: String,
    /// 開啟時採用的共同設定版本；還沒有採用任何版本為 `None`。
    pub settings_version: Option<i64>,
    /// 使用者要求開啟的時間。
    pub opened_at: Timestamp,
    /// launch intent：Supervisor 開窗前寫下；有值而沒有 `host` 代表開窗結果不明。
    pub launched_at: Option<Timestamp>,
    /// 宿主程序身分（開窗回執）。
    pub host: Option<ProcessRecord>,
    /// 宿主回報的 `claude` 程序識別。
    pub claude_pid: Option<u32>,
    pub running_at: Option<Timestamp>,
    /// 使用者按「結束」的時間；Supervisor 結束宿主後 Run 才進入 `ended`。
    pub end_requested_at: Option<Timestamp>,
    pub ended_at: Option<Timestamp>,
    /// 結束原因代碼（`END_*`）與給人看的說明。
    pub end_code: Option<String>,
    pub end_reason: Option<String>,
    /// 宿主回報的 `claude` 結束碼。
    pub exit_code: Option<i32>,
    /// 以 `--continue` 接回這個 Workspace 在這個帳號下最近的原生 Session（同帳號重開，票 05）。
    pub continue_session: bool,
    /// 已要求換帳號重開：這個 Run 結束後以這個 Profile 開新的 Run。
    pub switch_to_profile_id: Option<String>,
    /// 由換帳號重開產生時，前一個 Run。
    pub switched_from_run_id: Option<String>,
}

/// 一次變更：新的 Run 與這次產生的事件，由 Store 在一個交易中套用。
#[derive(Debug, Clone, PartialEq)]
pub struct ControllerCliChange {
    pub run: ControllerCliRun,
    pub events: Vec<DomainEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ControllerCliError {
    #[error("找不到 Workspace {0}")]
    WorkspaceNotFound(String),
    #[error("Workspace 已封存，不能開啟 Controller CLI")]
    WorkspaceArchived,
    #[error("找不到帳號 Profile {0}")]
    ProfileNotFound(String),
    #[error("這個 Profile 目前不能當 Controller 帳號：{0}")]
    ProfileUnavailable(String),
    #[error("這個 Workspace 已有開啟中的 Controller CLI（{0}）；請切到已開啟的視窗，或先結束它")]
    AlreadyOpen(String),
    #[error("這個 Workspace 沒有開啟中的 Controller CLI")]
    NotOpen,
    #[error("找不到 Controller Run {0}")]
    RunNotFound(String),
    #[error("Controller Run 已經結束")]
    AlreadyEnded,
    #[error("Controller CLI 已經在用這個帳號；換帳號重開請選另一個帳號")]
    SameProfile,
    #[error("Controller CLI 正在結束，請等它結束後再開啟")]
    Ending,
}

impl ControllerCliError {
    pub fn code(&self) -> &'static str {
        match self {
            ControllerCliError::WorkspaceNotFound(_) => "workspace_not_found",
            ControllerCliError::WorkspaceArchived => "workspace_archived",
            ControllerCliError::ProfileNotFound(_) => "profile_not_found",
            ControllerCliError::ProfileUnavailable(_) => "profile_unavailable",
            ControllerCliError::AlreadyOpen(_) => "controller_cli_already_open",
            ControllerCliError::NotOpen => "controller_cli_not_open",
            ControllerCliError::RunNotFound(_) => "controller_run_not_found",
            ControllerCliError::AlreadyEnded => "controller_cli_ended",
            ControllerCliError::SameProfile => "controller_cli_same_profile",
            ControllerCliError::Ending => "controller_cli_ending",
        }
    }
}

/// 開啟時需要的識別（由呼叫端以亂數產生）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenIds {
    pub run_id: String,
    pub command_id: String,
}

/// Profile 能不能當 Controller 帳號：與 Worker 的可派工條件相同（使用中、已登入且身分核對通過）。
/// 不能時回傳給人看的原因。
pub fn profile_unavailable_reason(profile: &AccountProfile, all: &[AccountProfile]) -> Option<String> {
    let blockers = profile::blockers(profile, all);
    (!blockers.is_empty()).then(|| blockers.iter().map(|b| b.message()).collect::<Vec<_>>().join("；"))
}

/// 使用者要求開啟：Workspace 使用中、Profile 可用、這個 Workspace 沒有非終態的 Controller Run。
/// `runs` 是這個 Workspace 的 Controller Run（其他 Workspace 的不影響：同一個 Profile 可以同時當多個 Workspace 的 Controller）。
pub fn open(
    workspace: &Workspace,
    profile_id: &str,
    profiles: &[AccountProfile],
    runs: &[ControllerCliRun],
    settings_version: Option<i64>,
    ids: OpenIds,
    now: Timestamp,
) -> Result<ControllerCliChange, ControllerCliError> {
    if workspace.status == WorkspaceStatus::Archived {
        return Err(ControllerCliError::WorkspaceArchived);
    }
    if let Some(active) = runs.iter().find(|r| r.workspace_id == workspace.id && !r.status.is_terminal()) {
        return Err(ControllerCliError::AlreadyOpen(active.id.clone()));
    }
    let profile = usable_profile(profile_id, profiles)?;
    let run = ControllerCliRun {
        profile_id: profile.id.clone(),
        ..blank_run(workspace, settings_version, ids, continues_session(workspace, runs, profile_id), None, now)
    };
    let payload = opened_payload(&run, &profile.name);
    Ok(apply(run, EVENT_CONTROLLER_CLI_OPENED, payload, now))
}

/// 同帳號重開才接續：這個 Workspace 最近一次真的執行過（進入過 `running`）的 Controller Run 用的是同一個 Profile。
/// 開啟失敗的 Run 沒有原生 Session，不算數；`runs` 依建立順序。
pub fn continues_session(workspace: &Workspace, runs: &[ControllerCliRun], profile_id: &str) -> bool {
    runs.iter()
        .rev()
        .find(|r| r.workspace_id == workspace.id && r.running_at.is_some())
        .is_some_and(|r| r.profile_id == profile_id)
}

/// 換帳號重開：要求結束目前的 Run，並記下要改用的 Profile（必須可用、且與目前不同）。
/// 已要求結束或換帳號時拒絕（`Ending`）。
pub fn request_switch(
    run: &ControllerCliRun,
    profile_id: &str,
    profiles: &[AccountProfile],
    now: Timestamp,
) -> Result<ControllerCliChange, ControllerCliError> {
    ensure_open(run)?;
    if run.end_requested_at.is_some() {
        return Err(ControllerCliError::Ending);
    }
    if run.profile_id == profile_id {
        return Err(ControllerCliError::SameProfile);
    }
    let profile = usable_profile(profile_id, profiles)?;
    let mut next = run.clone();
    next.end_requested_at = Some(now);
    next.switch_to_profile_id = Some(profile.id.clone());
    let payload = serde_json::json!({
        "from_profile_id": run.profile_id,
        "to_profile_id": profile.id,
        "to_profile_name": profile.name,
    });
    Ok(apply(next, EVENT_CONTROLLER_CLI_SWITCH_REQUESTED, payload, now))
}

/// 已要求換帳號重開的 Run 結束後，以新 Profile 開新的 Run（不接續原生 Session）。沒有要求換帳號時回 `None`。
/// `profiles` 只用來在時間線記下帳號名稱；Profile 在這之間失效時照樣建立，由 `begin_launch` 以 `profile_unavailable`
/// 結束並顯示原因。
pub fn open_switched(
    workspace: &Workspace,
    previous: &ControllerCliRun,
    profiles: &[AccountProfile],
    settings_version: Option<i64>,
    ids: OpenIds,
    now: Timestamp,
) -> Result<Option<ControllerCliChange>, ControllerCliError> {
    if !previous.status.is_terminal() {
        return Err(ControllerCliError::AlreadyOpen(previous.id.clone()));
    }
    let Some(profile_id) = previous.switch_to_profile_id.clone() else { return Ok(None) };
    if workspace.status == WorkspaceStatus::Archived {
        return Err(ControllerCliError::WorkspaceArchived);
    }
    let name = profiles.iter().find(|p| p.id == profile_id).map_or(profile_id.as_str(), |p| p.name.as_str()).to_string();
    let run = ControllerCliRun { profile_id, ..blank_run(workspace, settings_version, ids, false, Some(previous.id.clone()), now) };
    let payload = opened_payload(&run, &name);
    Ok(Some(apply(run, EVENT_CONTROLLER_CLI_OPENED, payload, now)))
}

fn usable_profile<'a>(profile_id: &str, profiles: &'a [AccountProfile]) -> Result<&'a AccountProfile, ControllerCliError> {
    let profile = profiles.iter().find(|p| p.id == profile_id).ok_or_else(|| ControllerCliError::ProfileNotFound(profile_id.into()))?;
    if let Some(reason) = profile_unavailable_reason(profile, profiles) {
        return Err(ControllerCliError::ProfileUnavailable(reason));
    }
    Ok(profile)
}

fn opened_payload(run: &ControllerCliRun, profile_name: &str) -> serde_json::Value {
    serde_json::json!({
        "workspace_id": run.workspace_id,
        "profile_id": run.profile_id,
        "profile_name": profile_name,
        "settings_version": run.settings_version,
        "continue_session": run.continue_session,
        "switched_from_run_id": run.switched_from_run_id,
    })
}

/// 剛要求開啟、尚未指定帳號的 Run（呼叫端補上 `profile_id`）。
fn blank_run(
    workspace: &Workspace,
    settings_version: Option<i64>,
    ids: OpenIds,
    continue_session: bool,
    switched_from_run_id: Option<String>,
    now: Timestamp,
) -> ControllerCliRun {
    ControllerCliRun {
        id: ids.run_id,
        workspace_id: workspace.id.clone(),
        profile_id: String::new(),
        status: ControllerCliStatus::Starting,
        version: 0,
        command_id: ids.command_id,
        working_dir: workspace.path.clone(),
        settings_version,
        opened_at: now,
        launched_at: None,
        host: None,
        claude_pid: None,
        running_at: None,
        end_requested_at: None,
        ended_at: None,
        end_code: None,
        end_reason: None,
        exit_code: None,
        continue_session,
        switch_to_profile_id: None,
        switched_from_run_id,
    }
}

/// Supervisor 開窗前寫下 launch intent；Profile 在要求開啟之後失效時改為結束（不開窗）。
/// 已寫過 launch intent 或已不是 `starting` 時拒絕（呼叫端改走核對）。
/// `continuable` 為 false 代表要接續的原生 Session 確定不存在（例如上次開啟後沒有任何對話就關掉）：改開新的 Session，
/// 否則 `claude --continue` 找不到 Session 會直接結束。這個決定隨 launch intent 持久化，重建後組出相同的啟動指令。
pub fn begin_launch(
    run: &ControllerCliRun,
    profiles: &[AccountProfile],
    continuable: bool,
    now: Timestamp,
) -> Result<ControllerCliChange, ControllerCliError> {
    ensure_open(run)?;
    if run.status != ControllerCliStatus::Starting || run.launched_at.is_some() {
        return Err(ControllerCliError::AlreadyEnded);
    }
    let unavailable = match profiles.iter().find(|p| p.id == run.profile_id) {
        None => Some("找不到這個帳號 Profile".to_string()),
        Some(profile) => profile_unavailable_reason(profile, profiles),
    };
    if let Some(reason) = unavailable {
        return end(run, END_PROFILE_UNAVAILABLE, &format!("Controller 帳號目前不能使用：{reason}"), None, now);
    }
    let mut next = run.clone();
    next.launched_at = Some(now);
    next.continue_session = run.continue_session && continuable;
    let payload = serde_json::json!({ "command_id": run.command_id, "continue_session": next.continue_session });
    Ok(apply(next, EVENT_CONTROLLER_CLI_LAUNCH_STARTED, payload, now))
}

/// 開窗回執：記下宿主程序身分並進入 `running`（宿主可能已先回報啟動，此時只補上身分）。
pub fn record_receipt(run: &ControllerCliRun, host: ProcessRecord, now: Timestamp) -> Result<ControllerCliChange, ControllerCliError> {
    ensure_open(run)?;
    let mut next = run.clone();
    let payload = serde_json::json!({ "host_pid": host.pid, "profile_id": run.profile_id });
    next.host = Some(host);
    Ok(to_running(next, payload, now))
}

/// 宿主回報 `claude` 已啟動。
pub fn host_started(run: &ControllerCliRun, claude_pid: u32, now: Timestamp) -> Result<ControllerCliChange, ControllerCliError> {
    ensure_open(run)?;
    let mut next = run.clone();
    next.claude_pid = Some(claude_pid);
    let payload = serde_json::json!({ "claude_pid": claude_pid, "profile_id": run.profile_id });
    Ok(to_running(next, payload, now))
}

/// 使用者按「結束」：記下要求，由 Supervisor 結束宿主；重複要求不改變任何東西。
pub fn request_end(run: &ControllerCliRun, now: Timestamp) -> Result<Option<ControllerCliChange>, ControllerCliError> {
    ensure_open(run)?;
    if run.end_requested_at.is_some() {
        return Ok(None);
    }
    let mut next = run.clone();
    next.end_requested_at = Some(now);
    Ok(Some(apply(next, EVENT_CONTROLLER_CLI_END_REQUESTED, serde_json::json!({}), now)))
}

/// 結束（終態）：記下原因代碼、說明與 `claude` 的結束碼。已結束時拒絕。
pub fn end(
    run: &ControllerCliRun,
    code: &str,
    reason: &str,
    exit_code: Option<i32>,
    now: Timestamp,
) -> Result<ControllerCliChange, ControllerCliError> {
    ensure_open(run)?;
    let mut next = run.clone();
    next.status = ControllerCliStatus::Ended;
    next.ended_at = Some(now);
    next.end_code = Some(code.to_string());
    next.end_reason = Some(reason.to_string());
    next.exit_code = exit_code;
    let payload = serde_json::json!({
        "profile_id": run.profile_id,
        "end_code": code,
        "end_reason": reason,
        "exit_code": exit_code,
    });
    Ok(apply(next, EVENT_CONTROLLER_CLI_ENDED, payload, now))
}

fn ensure_open(run: &ControllerCliRun) -> Result<(), ControllerCliError> {
    if run.status.is_terminal() {
        Err(ControllerCliError::AlreadyEnded)
    } else {
        Ok(())
    }
}

fn to_running(mut next: ControllerCliRun, payload: serde_json::Value, now: Timestamp) -> ControllerCliChange {
    if next.status == ControllerCliStatus::Running {
        // 已在執行中：只補上資料，不重複記一次「執行中」
        return ControllerCliChange { run: next, events: Vec::new() };
    }
    next.status = ControllerCliStatus::Running;
    next.running_at = Some(now);
    apply(next, EVENT_CONTROLLER_CLI_RUNNING, payload, now)
}

fn apply(mut next: ControllerCliRun, event_type: &'static str, payload: serde_json::Value, now: Timestamp) -> ControllerCliChange {
    next.version += 1;
    let event = DomainEvent {
        aggregate_type: CONTROLLER_CLI_AGGREGATE,
        aggregate_id: next.id.clone(),
        aggregate_version: next.version,
        event_type,
        occurred_at: now,
        payload,
    };
    ControllerCliChange { run: next, events: vec![event] }
}

#[cfg(test)]
mod tests;
