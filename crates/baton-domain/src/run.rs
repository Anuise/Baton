//! Run、execution lease 與派工 Outbox：Task 從 ready 到「有一個程序在替它工作」的狀態機（票 10）。
//!
//! 純邏輯、無 I/O。所有 Run 的變更都只能經過 `decide`：輸入「目前狀態（`RunContext`）+ 命令 + 時間」，
//! 輸出「新狀態 + 事件 + Outbox 項目」，由 Store 在一個交易中套用。
//!
//! 幾條貫穿全檔的規則：
//! - **寫入所有權**：同一個 Task 任何時刻最多一個尚未釋放的 execution lease（`ExecutionLease::is_held`）。
//!   派工時取得，Run 結束（成功、失敗、中斷、取消）時才釋放；取消要等程序確認停止才釋放，
//!   所以舊 Run 還沒確定停止之前，新的 Run 派不出去（spec 使用者故事 43）。
//! - **lease 過期只觸發核對**：`ReconcileLease` 要帶「實際觀察到的程序狀態」，只有觀察到 `Dead` 才中斷 Run；
//!   `Alive` 續約、`Unknown` 不改變任何東西。沒有任何命令能只靠時間流逝就把 Run 判定為結束。
//! - **程序退出不等於 Task 完成**：Run 成功只把 Task 推進到 `verifying`，失敗、中斷、取消都退回 `ready`。
//!   `completed` 只能由後續票的驗證閘門設定。
//! - **過期世代的回報不改變狀態**：Worker 回報（heartbeat、進度、checkpoint、受阻、結束）都帶 lease 的世代，與目前不符就拒絕。
//! - **准入 = launch intent 持久化（R1-a，票 24）**：派工只建立排隊中的 Run；`BeginLaunch` 的交易才以「這一刻生效」的
//!   授權重新檢查並固定授權 revision 與共同設定版本。授權更新與 `BeginLaunch` 都經過 Store 的單一寫入佇列，
//!   所以兩者的先後就是持久化順序：先准入的沿用舊版，排隊中或有 Outbox 但尚未准入的一律用新版。
//! - **修正與重試預算（票 36）**：Worker 以失敗結束計入實作失敗修正，啟動失敗與程序意外消失計入基礎設施重試；
//!   突然限流而結束（額度交接）、恢復與完全退出的中斷、取消都不計入。預算用盡時 Task 改為進入 `blocked`，
//!   並在同一個交易建立「預算用盡後的下一步」裁定請求；基礎設施重試在預算內時排回派工佇列（等待退避期限）。

use serde::{Deserialize, Serialize};

use crate::agent::{BlockedNote, CheckpointNote, ProgressNote, ReportContent, ReportRecord};
use crate::authorization::{self, AdmissionDenied};
use crate::budget::{self, BudgetKind, ChargeOutcome, TaskBudget};
use crate::decision::{self, DecisionRequest};
use crate::event::DomainEvent;
use crate::guard::{self, ActiveGuard, GuardViolation};
use crate::session::{self, AgentSession, BindOutcome, LaunchFacts, ResumeCheck, SessionEdits};
use crate::task::{
    self, ContractContent, ContractField, ContractFieldName, MissingItem, Task, TaskContext, TaskStatus, Versioned,
};
use crate::workdir::{Workdir, WorkdirKind};
use crate::workspace::{AgentRole, Workspace};
use crate::Timestamp;

pub const RUN_AGGREGATE: &str = "run";
pub const EVENT_RUN_CREATED: &str = "run_created";
pub const EVENT_RUN_LAUNCH_STARTED: &str = "run_launch_started";
pub const EVENT_RUN_STARTED: &str = "run_started";
pub const EVENT_RUN_LEASE_VERIFIED: &str = "run_lease_verified";
pub const EVENT_RUN_SUCCEEDED: &str = "run_succeeded";
pub const EVENT_RUN_FAILED: &str = "run_failed";
pub const EVENT_RUN_INTERRUPTED: &str = "run_interrupted";
pub const EVENT_RUN_CANCEL_REQUESTED: &str = "run_cancel_requested";
pub const EVENT_RUN_CANCELLED: &str = "run_cancelled";
pub const EVENT_RUN_PROGRESS_REPORTED: &str = "run_progress_reported";
pub const EVENT_RUN_CHECKPOINT_SAVED: &str = "run_checkpoint_saved";
pub const EVENT_RUN_BLOCKED_REPORTED: &str = "run_blocked_reported";
/// 啟動准入被拒（授權在派工之後被撤銷或縮小）：Run 沒有啟動就結束（票 24）。
pub const EVENT_RUN_ADMISSION_DENIED: &str = "run_admission_denied";
/// 准入被拒的結束原因前綴。
pub const ADMISSION_DENIED_REASON: &str = "admission_denied";
pub const EVENT_RUN_NATIVE_SESSION_BOUND: &str = "run_native_session_bound";
pub const EVENT_RUN_NATIVE_SESSION_CONFLICT: &str = "run_native_session_conflict";
/// 交接（票 28）：舊程序已確認停止，Run 結束、寫入所有權交給 successor。
pub const EVENT_RUN_HANDED_OFF: &str = "run_handed_off";

/// 派工 Outbox 項目的種類：要求 Supervisor 啟動這個 Run 的程序。
pub const OUTBOX_LAUNCH_RUN: &str = "launch_run";

/// Worker 的存活訊號間隔（spec D-31）。
pub const HEARTBEAT_INTERVAL_MS: i64 = 15_000;
/// lease 的有效期間（spec D-31）；每次 heartbeat 或核對確認存活就從當下重新計算。
pub const LEASE_DURATION_MS: i64 = 60_000;

// ---------------------------------------------------------------- 狀態機

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Queued,
    Starting,
    Running,
    Succeeded,
    Failed,
    Interrupted,
    Cancelled,
}

impl RunStatus {
    pub const ALL: [RunStatus; 7] = [
        RunStatus::Queued,
        RunStatus::Starting,
        RunStatus::Running,
        RunStatus::Succeeded,
        RunStatus::Failed,
        RunStatus::Interrupted,
        RunStatus::Cancelled,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            RunStatus::Queued => "queued",
            RunStatus::Starting => "starting",
            RunStatus::Running => "running",
            RunStatus::Succeeded => "succeeded",
            RunStatus::Failed => "failed",
            RunStatus::Interrupted => "interrupted",
            RunStatus::Cancelled => "cancelled",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.as_str() == raw)
    }

    /// 已結束：不能再有任何轉換。
    pub fn is_terminal(self) -> bool {
        matches!(self, RunStatus::Succeeded | RunStatus::Failed | RunStatus::Interrupted | RunStatus::Cancelled)
    }

    /// 這個狀態可以直接轉到哪些狀態。
    pub fn allowed_next(self) -> &'static [RunStatus] {
        use RunStatus::*;
        match self {
            // 准入被拒時沒有程序，直接中斷（票 24）
            Queued => &[Starting, Interrupted, Cancelled],
            Starting => &[Running, Failed, Interrupted, Cancelled],
            Running => &[Succeeded, Failed, Interrupted, Cancelled],
            Succeeded | Failed | Interrupted | Cancelled => &[],
        }
    }

    pub fn can_transition_to(self, next: RunStatus) -> bool {
        self.allowed_next().contains(&next)
    }
}

// ---------------------------------------------------------------- 實體

/// 程序身分：PID + 程序建立時間 + 啟動指令雜湊（spec D-10），避免 PID 重用誤判。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessRecord {
    pub pid: u32,
    pub created_at_millis: i64,
    pub command_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Run {
    pub id: String,
    pub workspace_id: String,
    pub task_id: String,
    /// 這個 Run 使用的帳號（account_profile）；由 Scheduler 在派工時選定，之後不會改變（票 19）。
    pub profile_id: String,
    pub status: RunStatus,
    /// 每個事件加一；事件的 `aggregate_version` 與它對應。
    pub version: i64,
    /// 這個 Run 取得寫入所有權時的世代：同一個 Task 的 Run 依序遞增。
    pub generation: i64,
    /// 派工當下生效的契約版本與授權 revision（沒有任何授權時為 `None`）。准入（`BeginLaunch`）時授權 revision
    /// 改寫為准入當下生效的版本，之後不再改變：已准入的 Run 沿用它（R1-a）。
    pub contract_version: i64,
    pub authorization_revision: Option<i64>,
    /// 准入時間（launch intent 持久化的那一刻）；`None` 代表尚未准入，或是票 24 之前建立的 Run。
    #[serde(default)]
    pub admitted_at: Option<Timestamp>,
    /// 准入時固定的共同設定版本（S3）；沒有採用任何版本或尚未准入為 `None`。
    #[serde(default)]
    pub settings_version: Option<i64>,
    /// launch intent：Supervisor 啟動程序之前先持久化；Runtime 重建後以它找回原程序。
    pub command_id: Option<String>,
    /// 啟動回執（程序身分）；回執寫入前為 `None`。
    pub process: Option<ProcessRecord>,
    /// 使用者要求取消的時間；程序確認停止之前 Run 仍是 starting／running。
    pub cancel_requested_at: Option<Timestamp>,
    pub end_reason: Option<String>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub started_at: Option<Timestamp>,
    pub ended_at: Option<Timestamp>,
    /// Adapter 首次回報的原生 Session 識別；綁定後不會被改成別的 Session。
    #[serde(default)]
    pub native_session_id: Option<String>,
    /// 這個 Run 驅動的平台 Session（票 15）；寫入 launch intent 時決定，之前為 `None`。
    #[serde(default)]
    pub agent_session_id: Option<String>,
    /// 啟動時要以原生方式接續的原生 Session；建立新原生 Session 時為 `None`。與 launch intent 一起持久化。
    #[serde(default)]
    pub resume_native_session_id: Option<String>,
}

/// execution lease：Task 的寫入所有權。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionLease {
    pub run_id: String,
    pub task_id: String,
    pub generation: i64,
    pub acquired_at: Timestamp,
    pub last_heartbeat_at: Timestamp,
    pub expires_at: Timestamp,
    pub released_at: Option<Timestamp>,
    pub release_reason: Option<String>,
}

impl ExecutionLease {
    /// 仍持有寫入所有權（尚未釋放）。
    pub fn is_held(&self) -> bool {
        self.released_at.is_none()
    }

    /// 已過期只代表「該去核對了」，不代表程序死亡。
    pub fn is_expired(&self, now: Timestamp) -> bool {
        now > self.expires_at
    }
}

/// 派工 Outbox 的一個項目；`delivered_at` 有值代表已經不需要再處理。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutboxItem {
    pub kind: String,
    pub run_id: String,
    pub payload: LaunchPayload,
    pub created_at: Timestamp,
    pub delivered_at: Option<Timestamp>,
}

/// 啟動一個 Run 需要的資訊；派工當下就固定下來，Runtime 重建後據此重組出相同的啟動指令。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchPayload {
    pub task_id: String,
    pub prompt: String,
    /// Worker 的工作目錄：Task 在平台登記的 worktree 或輸出目錄（票 20）。
    pub working_dir: String,
    /// 派工當下 Workspace 對 Worker 角色的模型覆寫（spec D-30）；`None` 表示沿用帳號的預設模型。
    #[serde(default)]
    pub model: Option<String>,
}

/// 面板與 API 看到的一個 Run：Run 本身加上它的 lease，以及 Worker 最新的進度、checkpoint 與受阻回報。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunView {
    #[serde(flatten)]
    pub run: Run,
    pub lease: Option<ExecutionLease>,
    #[serde(default)]
    pub latest_progress: Option<ProgressNote>,
    #[serde(default)]
    pub latest_checkpoint: Option<CheckpointNote>,
    #[serde(default)]
    pub blocked: Option<BlockedNote>,
}

impl RunView {
    /// `reports` 依收到的順序排列；只採用這個 Run 已被接受的回報，同一種類以最後一筆為準。
    pub fn new(run: Run, lease: Option<ExecutionLease>, reports: &[ReportRecord]) -> Self {
        let mut view = Self { run, lease, latest_progress: None, latest_checkpoint: None, blocked: None };
        for report in reports.iter().filter(|r| r.accepted && r.run_id == view.run.id) {
            let at = report.received_at;
            match &report.content {
                ReportContent::Progress { summary } => {
                    view.latest_progress = Some(ProgressNote { summary: summary.clone(), reported_at: at });
                }
                ReportContent::Checkpoint { summary, details } => {
                    view.latest_checkpoint =
                        Some(CheckpointNote { summary: summary.clone(), details: details.clone(), saved_at: at });
                }
                ReportContent::Blocked { reason } => {
                    view.blocked = Some(BlockedNote { reason: reason.clone(), reported_at: at });
                }
            }
        }
        view
    }
}

// ---------------------------------------------------------------- 命令

/// Worker 或程序的結束結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunOutcome {
    Succeeded,
    Failed { reason: String },
    /// 帳號突然限流而結束（Adapter 回報額度用盡之後程序以失敗結束）：Run 中斷、Task 回到 `ready` 等其他帳號接手，
    /// 不計入任何預算（票 36）。
    QuotaLimited { reason: String },
}

/// Supervisor 實際觀察到的程序狀態（對應 ProcessControl 的 `Liveness`；Domain 不依賴 Port）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessObservation {
    Alive,
    Dead,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunCommand {
    /// 派工交易：建立 Run、取得 lease、寫入派工 Outbox，Task 進入 `running`。`run_id` 由呼叫端產生；
    /// `profile_id` 是 Scheduler（`crate::scheduler`）在同一個交易中選定的帳號。
    Dispatch { run_id: String, task_id: String, profile_id: String },
    /// Supervisor 在啟動程序之前先持久化 launch intent 與 command_id（`queued` → `starting`），
    /// 同一個交易中決定沿用或建立 Session（票 15）：`working_dir` 是派工時固定的工作目錄，
    /// `resume_check` 是 Supervisor 對候選 Session（`session::resume_candidate`）查驗接續能力的結果。
    BeginLaunch { run_id: String, command_id: String, working_dir: String, resume_check: Option<ResumeCheck> },
    /// 啟動回執寫回（`starting` → `running`）；重複寫回同一個回執是冪等的。
    RecordReceipt { run_id: String, process: ProcessRecord },
    /// 啟動程序失敗（確定沒有程序在跑）。
    LaunchFailed { run_id: String, reason: String },
    /// Worker 的存活訊號：續約 lease。
    Heartbeat { run_id: String, generation: i64 },
    /// Worker 回報進度、checkpoint 或受阻（Agent API）；同時續約 lease。受阻會讓 Task 進入 `blocked`。
    Report { run_id: String, generation: i64, content: ReportContent },
    /// Adapter 回報了原生 Session 識別：第一次就綁定；之後相同是冪等的，不同則只記錄衝突、不改綁定。
    BindNativeSession { run_id: String, generation: i64, native_session_id: String },
    /// Worker 回報結束。
    Finish { run_id: String, generation: i64, outcome: RunOutcome },
    /// 確認程序已經不在（核對結果、啟動結果不明且找不到程序）。
    Interrupt { run_id: String, reason: String },
    /// lease 過期後的核對結果。
    ReconcileLease { run_id: String, observed: ProcessObservation },
    /// 使用者要求取消：尚未啟動的立即取消，已啟動的先記下意圖，等程序確認停止。
    RequestCancel { run_id: String },
    /// 程序已確認停止，完成取消。
    ConfirmCancelled { run_id: String },
    /// 交接（票 28）：舊程序已確認停止（或已自行結束），Run 以 `interrupted`（`handed_off`）結束並釋放寫入所有權；
    /// Task 退回 `ready`，由同一個交易中的 successor 派工立刻接手。不是失敗，不計入任何預算。
    HandOff { run_id: String, handoff_id: String },
    /// 突然限流（票 29）：舊程序已確認停止，但所有合適帳號都受限。Run 以 `interrupted`（`handed_off_waiting_resource`）
    /// 結束並釋放寫入所有權，Task 進入 `waiting_resource`，資源恢復時由交接的 successor 接手。不計入任何預算。
    ParkForResource { run_id: String, handoff_id: String },
}

impl RunCommand {
    fn run_id(&self) -> &str {
        match self {
            RunCommand::Dispatch { run_id, .. }
            | RunCommand::BeginLaunch { run_id, .. }
            | RunCommand::RecordReceipt { run_id, .. }
            | RunCommand::LaunchFailed { run_id, .. }
            | RunCommand::Heartbeat { run_id, .. }
            | RunCommand::Report { run_id, .. }
            | RunCommand::BindNativeSession { run_id, .. }
            | RunCommand::Finish { run_id, .. }
            | RunCommand::Interrupt { run_id, .. }
            | RunCommand::ReconcileLease { run_id, .. }
            | RunCommand::RequestCancel { run_id }
            | RunCommand::ConfirmCancelled { run_id }
            | RunCommand::HandOff { run_id, .. }
            | RunCommand::ParkForResource { run_id, .. } => run_id,
        }
    }
}

/// 命令成功後的結果：要寫入的新狀態與要追加的事件。沒有任何變動（冪等）時整個結果是空的。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RunChange {
    pub run: Option<Run>,
    pub lease: Option<ExecutionLease>,
    /// 因 Run 而改變的 Task（狀態轉換）。
    pub task: Option<Task>,
    /// 新增的派工 Outbox 項目。
    pub outbox: Option<OutboxItem>,
    /// 這個 Run 的派工 Outbox 項目已處理完畢。
    pub outbox_delivered: Option<String>,
    /// 被接受的 Worker 回報（追加式稽核）。
    pub report: Option<ReportRecord>,
    /// Run 結束計入後的預算（票 36）。
    pub budget: Option<TaskBudget>,
    /// 預算用盡時建立的「預算用盡後的下一步」裁定請求（票 36）。
    pub decision: Option<DecisionRequest>,
    /// 基礎設施重試在預算內：把 Task 排回派工佇列（排程會等到退避期限）。
    pub requeue: bool,
    /// 因 Run 而改變（或新建）的 Session（票 15）。
    pub sessions: Vec<AgentSession>,
    pub events: Vec<DomainEvent>,
}

impl RunChange {
    pub fn changed(&self) -> bool {
        self.run.is_some()
            || self.lease.is_some()
            || self.task.is_some()
            || self.outbox.is_some()
            || self.outbox_delivered.is_some()
            || self.report.is_some()
            || self.budget.is_some()
            || !self.sessions.is_empty()
            || !self.events.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RunError {
    #[error("找不到 Workspace {0}")]
    WorkspaceNotFound(String),
    #[error("找不到 Task {0}")]
    TaskNotFound(String),
    #[error("找不到 Run {0}")]
    RunNotFound(String),
    #[error("Task 目前是 {} 狀態，只有 ready 的 Task 可以派工", .0.as_str())]
    TaskNotReady(TaskStatus),
    #[error("准入檢查未通過，缺少：{}", join_messages(.0))]
    AdmissionDenied(Vec<MissingItem>),
    #[error(transparent)]
    AdmissionRejected(#[from] AdmissionDenied),
    #[error("Guard 擋下了派工：{}", join_violations(.0))]
    GuardViolated(Vec<GuardViolation>),
    #[error("還有相依的 Task 沒有完成：{}", .0.join("、"))]
    DependenciesUnfinished(Vec<String>),
    #[error("Task {task_id} 已經有 Run {run_id} 持有寫入所有權，不能同時派第二個")]
    ExecutionConflict { task_id: String, run_id: String },
    #[error("Run 目前是 {} 狀態，不能轉換到 {}", .from.as_str(), .to.as_str())]
    TransitionInvalid { from: RunStatus, to: RunStatus },
    #[error("這份回報屬於已經不是目前寫入所有者的世代，已忽略")]
    StaleReport,
    #[error("Run 的 lease 還沒有過期，不需要核對")]
    LeaseNotExpired,
    #[error("Run 沒有被要求取消，不能完成取消")]
    CancelNotRequested,
    #[error("完全退出的停止意圖生效中（或 Workspace 在完全退出後等待「繼續工作」），不啟動新的程序")]
    LaunchHalted,
    #[error("Task 的{}還沒有準備好", workdir_label(*.0))]
    WorkdirNotPrepared(WorkdirKind),
    #[error("Task 的工作目錄不可用：{0}")]
    WorkdirUnavailable(String),
    #[error("找不到 Session {0}")]
    SessionNotFound(String),
    #[error("Session {session_id} 仍由 Run {run_id} 驅動，同一個 Session 不能有兩個執行者")]
    SessionBusy { session_id: String, run_id: String },
}

fn workdir_label(kind: WorkdirKind) -> &'static str {
    match kind {
        WorkdirKind::Worktree => " worktree ",
        WorkdirKind::OutputDir => "輸出目錄",
    }
}

fn join_messages(items: &[MissingItem]) -> String {
    items.iter().map(|i| i.message.as_str()).collect::<Vec<_>>().join("；")
}

fn join_violations(items: &[GuardViolation]) -> String {
    items.iter().map(|v| format!("{}（來源教訓「{}」）", v.message, v.lesson_title)).collect::<Vec<_>>().join("；")
}

impl RunError {
    /// 穩定的機器可讀代碼，API 與 UI 依它判斷錯誤種類。
    pub fn code(&self) -> &'static str {
        match self {
            RunError::WorkspaceNotFound(_) => "workspace_not_found",
            RunError::TaskNotFound(_) => "task_not_found",
            RunError::RunNotFound(_) => "run_not_found",
            RunError::TaskNotReady(_) => "task_not_ready",
            RunError::AdmissionDenied(_) => "task_admission_denied",
            RunError::AdmissionRejected(_) => "admission_rejected",
            RunError::GuardViolated(_) => "guard_violated",
            RunError::DependenciesUnfinished(_) => "dependencies_unfinished",
            RunError::ExecutionConflict { .. } => "execution_conflict",
            RunError::TransitionInvalid { .. } => "run_transition_invalid",
            RunError::StaleReport => "run_report_stale",
            RunError::LeaseNotExpired => "lease_not_expired",
            RunError::CancelNotRequested => "run_cancel_not_requested",
            RunError::LaunchHalted => "launch_halted",
            RunError::WorkdirNotPrepared(_) => "workdir_not_prepared",
            RunError::WorkdirUnavailable(_) => "workdir_unavailable",
            RunError::SessionNotFound(_) => "session_not_found",
            RunError::SessionBusy { .. } => "session_busy",
        }
    }
}

// ---------------------------------------------------------------- 判定

/// 目前狀態的唯讀快照（同一個 Workspace 內）。Store 在交易中載入。
pub struct RunContext<'a> {
    pub task: &'a TaskContext<'a>,
    pub runs: &'a [Run],
    pub leases: &'a [ExecutionLease],
    /// 這一刻准入的 Run 會固定使用的共同設定版本（沒有採用任何版本為 `None`）；只有 `BeginLaunch` 使用。
    pub settings_version: Option<i64>,
    /// Workspace 內平台登記的工作目錄（票 20）；派工只認已可用的目錄。
    pub workdirs: &'a [Workdir],
    /// Run 准入時要檢查的 Guard（Store 已篩出「啟用且來源教訓現在有效」者；範圍由 `guard::violations` 判定）。
    pub guards: &'a [ActiveGuard],
    /// Workspace 內的 Session（依建立順序）；綁定原生識別時另含同一帳號下綁定了該識別的其他 Session。
    pub sessions: &'a [AgentSession],
    /// Workspace 內 Task 的修正與重試預算（票 36）；Run 結束時依結束原因計入。
    pub budgets: &'a [TaskBudget],
}

impl RunContext<'_> {
    fn run(&self, id: &str) -> Result<&Run, RunError> {
        self.runs.iter().find(|r| r.id == id).ok_or_else(|| RunError::RunNotFound(id.to_string()))
    }

    fn lease_of(&self, run_id: &str) -> Option<&ExecutionLease> {
        self.leases.iter().find(|l| l.run_id == run_id)
    }

    fn held_lease_of_task(&self, task_id: &str) -> Option<&ExecutionLease> {
        self.leases.iter().find(|l| l.task_id == task_id && l.is_held())
    }
}

/// 把一個 Run 與它的 lease 投影成對外看到的樣子。
pub fn view_run(ctx: &RunContext<'_>, run: &Run) -> RunView {
    RunView::new(run.clone(), ctx.lease_of(&run.id).cloned(), &[])
}

/// 一個 Run 在判定過程中的可變副本；每個事件讓 Run 的版本加一。
struct Editing<'a> {
    run: Run,
    lease: Option<ExecutionLease>,
    events: Vec<DomainEvent>,
    sessions: SessionEdits,
    now: Timestamp,
    ctx: &'a RunContext<'a>,
}

impl<'a> Editing<'a> {
    fn new(ctx: &'a RunContext<'a>, run: &Run, now: Timestamp) -> Self {
        Self {
            run: run.clone(),
            lease: ctx.lease_of(&run.id).cloned(),
            events: Vec::new(),
            sessions: SessionEdits::default(),
            now,
            ctx,
        }
    }

    /// Run 的事件之後接著 Session 的事件。
    fn take_events(&mut self) -> (Vec<AgentSession>, Vec<DomainEvent>) {
        let edits = std::mem::take(&mut self.sessions);
        let mut events = std::mem::take(&mut self.events);
        events.extend(edits.events);
        (edits.sessions, events)
    }

    fn push(&mut self, event_type: &'static str, payload: serde_json::Value) {
        self.run.version += 1;
        self.run.updated_at = self.now;
        self.events.push(DomainEvent {
            aggregate_type: RUN_AGGREGATE,
            aggregate_id: self.run.id.clone(),
            aggregate_version: self.run.version,
            event_type,
            occurred_at: self.now,
            payload,
        });
    }

    fn change_status(&mut self, to: RunStatus) -> Result<(), RunError> {
        let from = self.run.status;
        if !from.can_transition_to(to) {
            return Err(RunError::TransitionInvalid { from, to });
        }
        self.run.status = to;
        Ok(())
    }

    /// 續約：從現在起重新計算有效期間。
    fn renew_lease(&mut self) {
        if let Some(lease) = self.lease.as_mut().filter(|l| l.is_held()) {
            lease.last_heartbeat_at = self.now;
            lease.expires_at = self.now.plus_millis(LEASE_DURATION_MS);
        }
    }

    fn release_lease(&mut self, reason: &str) {
        if let Some(lease) = self.lease.as_mut().filter(|l| l.is_held()) {
            lease.released_at = Some(self.now);
            lease.release_reason = Some(reason.to_string());
        }
    }

    /// Run 結束：釋放寫入所有權、Task 依結果轉換（只有 Task 還在 `running` 才轉換；
    /// 使用者已經取消的 Task 不會被拉回來）、派工 Outbox 視為處理完畢。
    /// `charge` 是這種結束要計入的預算（票 36）；Task 沒有轉換時不計入。
    fn finish(
        mut self,
        to: RunStatus,
        event_type: &'static str,
        reason: &str,
        task_after: TaskStatus,
        charge: Option<BudgetKind>,
    ) -> Result<RunChange, RunError> {
        let from = self.run.status;
        self.change_status(to)?;
        self.run.ended_at = Some(self.now);
        self.run.end_reason = Some(reason.to_string());
        self.push(event_type, serde_json::json!({ "from": from, "reason": reason }));
        self.release_lease(reason);
        // 驅動的 Session 跟著放開：有原生識別的暫停（可能可以接續），從未取得的標為不可用
        if let Some(session_id) = self.run.agent_session_id.clone() {
            session::release_driver(&mut self.sessions, self.ctx.sessions, &session_id, &self.run.id, self.now);
        }

        let (mut task, mut budget, mut decision, mut requeue) = (None, None, None, false);
        if let Some(current) = self.ctx.task.task(&self.run.task_id).filter(|t| t.status == TaskStatus::Running) {
            let mut task_after = (task_after, event_type);
            let mut budget_events = Vec::new();
            if let Some(kind) = charge {
                let request_id = format!("dr_budget_{}", self.run.id);
                let current_budget = TaskBudget::of(self.ctx.budgets, current);
                let charged = budget::charge(&current_budget, kind, &self.run.id, reason, request_id, self.now);
                budget_events.push(charged.event);
                match charged.outcome {
                    ChargeOutcome::Retry { requeue: again } => requeue = again,
                    ChargeOutcome::Exhausted { request_id } => {
                        // 預算用盡：不再自動重試，Task 阻擋並等使用者裁定下一步
                        let (request, requested) = decision::budget_exhausted_request(
                            request_id,
                            &self.run,
                            current,
                            kind,
                            vec![reason.to_string()],
                            self.now,
                        );
                        budget_events.push(requested);
                        decision = Some(request);
                        task_after = (TaskStatus::Blocked, budget::EVENT_BUDGET_EXHAUSTED);
                    }
                }
                budget = Some(charged.budget);
            }
            let mut moved = current.clone();
            let mut versioned = Versioned { task: &mut moved, events: Vec::new(), now: self.now };
            versioned.change_status(task_after.0, task_after.1).map_err(|_| RunError::TaskNotReady(current.status))?;
            self.events.extend(versioned.events);
            self.events.extend(budget_events);
            task = Some(moved);
        }
        let outbox_delivered = matches!(from, RunStatus::Queued | RunStatus::Starting).then(|| self.run.id.clone());
        let (sessions, events) = self.take_events();
        Ok(RunChange {
            run: Some(self.run),
            lease: self.lease,
            task,
            outbox: None,
            outbox_delivered,
            report: None,
            budget,
            decision,
            requeue,
            sessions,
            events,
        })
    }

    fn into_change(mut self) -> RunChange {
        let (sessions, events) = self.take_events();
        RunChange { run: Some(self.run), lease: self.lease, sessions, events, ..RunChange::default() }
    }
}

/// 套用一個命令。
pub fn decide(ctx: &RunContext<'_>, command: RunCommand, now: Timestamp) -> Result<RunChange, RunError> {
    let workspace =
        ctx.task.workspace.ok_or_else(|| RunError::WorkspaceNotFound(ctx.task.workspace_id.to_string()))?;
    if let RunCommand::Dispatch { run_id, task_id, profile_id } = command {
        return dispatch(ctx, workspace, run_id, task_id, profile_id, now);
    }

    let run = ctx.run(command.run_id())?;
    let mut edit = Editing::new(ctx, run, now);
    match command {
        RunCommand::Dispatch { .. } => unreachable!("上面已經處理"),
        RunCommand::BeginLaunch { command_id, working_dir, resume_check, .. } => {
            if run.status == RunStatus::Starting && run.command_id.as_deref() == Some(command_id.as_str()) {
                return Ok(RunChange::default());
            }
            if run.status != RunStatus::Queued {
                return Err(RunError::TransitionInvalid { from: run.status, to: RunStatus::Starting });
            }
            // 准入：以這一刻生效的授權重新檢查（R1-a）。被拒時不寫 launch intent，Run 沒有程序就結束
            let admission = match admit_at_launch(ctx, run) {
                Ok(admission) => admission,
                Err(denied) => return deny_admission(edit, &denied, ctx.task.authorization.map(|a| a.revision)),
            };
            edit.change_status(RunStatus::Starting)?;
            // 原生識別取得前，平台 Session、launch intent 與 command_id 在同一個交易中持久化
            let check = resume_check.as_ref();
            let plan = session::plan_launch(ctx.sessions, &run.id, &run.task_id, &run.profile_id, &working_dir, check)
                .map_err(|busy| RunError::SessionBusy { session_id: busy.session_id, run_id: busy.run_id })?;
            let facts = LaunchFacts {
                run_id: &run.id,
                workspace_id: &run.workspace_id,
                owner: session::SessionOwner::Task(&run.task_id),
                profile_id: &run.profile_id,
                working_dir: &working_dir,
            };
            let (session_id, resume_native) = session::apply_plan(&mut edit.sessions, ctx.sessions, plan, check, &facts, now);
            edit.run.command_id = Some(command_id.clone());
            edit.run.authorization_revision = admission.authorization_revision;
            edit.run.settings_version = ctx.settings_version;
            edit.run.admitted_at = Some(now);
            edit.run.agent_session_id = Some(session_id.clone());
            edit.run.resume_native_session_id = resume_native.clone();
            edit.renew_lease();
            edit.push(
                EVENT_RUN_LAUNCH_STARTED,
                serde_json::json!({
                    "command_id": command_id,
                    "authorization_revision": admission.authorization_revision,
                    "settings_version": ctx.settings_version,
                    "agent_session_id": session_id,
                    "resume_native_session_id": resume_native,
                }),
            );
            Ok(edit.into_change())
        }
        RunCommand::RecordReceipt { process, .. } => {
            if run.status == RunStatus::Running && run.process.as_ref() == Some(&process) {
                return Ok(RunChange::default());
            }
            edit.change_status(RunStatus::Running)?;
            edit.run.started_at = Some(now);
            edit.renew_lease();
            edit.push(
                EVENT_RUN_STARTED,
                serde_json::json!({
                    "pid": process.pid,
                    "created_at_millis": process.created_at_millis,
                    "command_hash": process.command_hash,
                }),
            );
            edit.run.process = Some(process);
            let mut change = edit.into_change();
            change.outbox_delivered = Some(run.id.clone());
            Ok(change)
        }
        RunCommand::LaunchFailed { reason, .. } => {
            let reason = format!("launch_failed：{reason}");
            edit.finish(RunStatus::Failed, EVENT_RUN_FAILED, &reason, TaskStatus::Ready, Some(BudgetKind::Infrastructure))
        }
        RunCommand::Heartbeat { generation, .. } => {
            check_report(run, edit.lease.as_ref(), generation)?;
            if run.status == RunStatus::Queued {
                return Err(RunError::TransitionInvalid { from: run.status, to: RunStatus::Running });
            }
            edit.renew_lease();
            // 存活訊號很頻繁，只更新 lease，不追加事件
            Ok(RunChange { lease: edit.lease, ..RunChange::default() })
        }
        RunCommand::Report { generation, content, .. } => {
            check_report(run, edit.lease.as_ref(), generation)?;
            if run.status == RunStatus::Queued {
                return Err(RunError::TransitionInvalid { from: run.status, to: RunStatus::Running });
            }
            edit.renew_lease();
            let (event_type, payload) = match &content {
                ReportContent::Progress { summary } => {
                    (EVENT_RUN_PROGRESS_REPORTED, serde_json::json!({ "generation": generation, "summary": summary }))
                }
                // 細節只存在回報的稽核紀錄裡，事件只記摘要，避免事件表被大量文字撐大
                ReportContent::Checkpoint { summary, .. } => {
                    (EVENT_RUN_CHECKPOINT_SAVED, serde_json::json!({ "generation": generation, "summary": summary }))
                }
                ReportContent::Blocked { reason } => {
                    (EVENT_RUN_BLOCKED_REPORTED, serde_json::json!({ "generation": generation, "reason": reason }))
                }
            };
            edit.push(event_type, payload);
            // 受阻：Task 從 running 進入 blocked；使用者已取消或已由別的路徑離開 running 的 Task 不動
            let mut task = None;
            if matches!(content, ReportContent::Blocked { .. }) {
                if let Some(current) = ctx.task.task(&run.task_id).filter(|t| t.status == TaskStatus::Running) {
                    let mut moved = current.clone();
                    let mut versioned = Versioned { task: &mut moved, events: Vec::new(), now };
                    versioned
                        .change_status(TaskStatus::Blocked, event_type)
                        .map_err(|_| RunError::TaskNotReady(current.status))?;
                    edit.events.extend(versioned.events);
                    task = Some(moved);
                }
            }
            let report = ReportRecord {
                workspace_id: run.workspace_id.clone(),
                run_id: run.id.clone(),
                generation,
                accepted: true,
                content,
                received_at: now,
            };
            Ok(RunChange { task, report: Some(report), ..edit.into_change() })
        }
        RunCommand::BindNativeSession { generation, native_session_id, .. } => {
            check_report(run, edit.lease.as_ref(), generation)?;
            // 平台 Session 的條件綁定（票 15）：同一個帳號內已由別的 Session 綁定的原生識別不再綁定
            let outcome = match &run.agent_session_id {
                Some(session_id) => {
                    session::bind_native(&mut edit.sessions, ctx.sessions, session_id, &run.id, &native_session_id, now)
                }
                None => BindOutcome::Bound,
            };
            match &run.native_session_id {
                Some(bound) if *bound == native_session_id => {}
                Some(bound) => {
                    // 條件綁定：已綁定的 Session 不會被靜默改指；衝突留下事件供查核
                    let payload = serde_json::json!({ "bound": bound, "reported": native_session_id });
                    edit.push(EVENT_RUN_NATIVE_SESSION_CONFLICT, payload);
                }
                None => {
                    edit.push(EVENT_RUN_NATIVE_SESSION_BOUND, serde_json::json!({ "native_session_id": native_session_id }));
                    edit.run.native_session_id = Some(native_session_id);
                }
            }
            // 同一個原生 Session 正由另一個 Run 驅動：後到的這個 Run 必須停下，不能有兩個執行者
            if matches!(outcome, BindOutcome::InUse { driven: true, .. }) && edit.run.cancel_requested_at.is_none() {
                edit.run.cancel_requested_at = Some(now);
                edit.push(EVENT_RUN_CANCEL_REQUESTED, serde_json::json!({ "reason": session::NATIVE_SESSION_IN_USE }));
            }
            if edit.events.is_empty() && edit.sessions.events.is_empty() {
                return Ok(RunChange::default());
            }
            Ok(edit.into_change())
        }
        RunCommand::Finish { generation, outcome, .. } => {
            check_report(run, edit.lease.as_ref(), generation)?;
            match outcome {
                RunOutcome::Succeeded => edit.finish(
                    RunStatus::Succeeded,
                    EVENT_RUN_SUCCEEDED,
                    "exited_successfully",
                    TaskStatus::Verifying,
                    None,
                ),
                RunOutcome::Failed { reason } => edit.finish(
                    RunStatus::Failed,
                    EVENT_RUN_FAILED,
                    &format!("exited_with_failure：{reason}"),
                    TaskStatus::Ready,
                    Some(BudgetKind::Implementation),
                ),
                RunOutcome::QuotaLimited { reason } => edit.finish(
                    RunStatus::Interrupted,
                    EVENT_RUN_INTERRUPTED,
                    &format!("quota_limited：{reason}"),
                    TaskStatus::Ready,
                    None,
                ),
            }
        }
        // 中斷都來自恢復、完全退出或還原：不計入任何預算
        RunCommand::Interrupt { reason, .. } => {
            edit.finish(RunStatus::Interrupted, EVENT_RUN_INTERRUPTED, &reason, TaskStatus::Ready, None)
        }
        RunCommand::ReconcileLease { observed, .. } => {
            if !matches!(run.status, RunStatus::Starting | RunStatus::Running) {
                return Err(RunError::TransitionInvalid { from: run.status, to: RunStatus::Interrupted });
            }
            if !edit.lease.as_ref().is_some_and(|l| l.is_held() && l.is_expired(now)) {
                return Err(RunError::LeaseNotExpired);
            }
            match observed {
                ProcessObservation::Alive => {
                    edit.renew_lease();
                    edit.push(EVENT_RUN_LEASE_VERIFIED, serde_json::json!({ "observed": "alive" }));
                    Ok(edit.into_change())
                }
                // 只有確實觀察到程序已經不在，才把 Run 判定為中斷
                // 程序意外消失是基礎設施問題
                ProcessObservation::Dead => edit.finish(
                    RunStatus::Interrupted,
                    EVENT_RUN_INTERRUPTED,
                    "process_dead_after_lease_expired",
                    TaskStatus::Ready,
                    Some(BudgetKind::Infrastructure),
                ),
                ProcessObservation::Unknown => Ok(RunChange::default()),
            }
        }
        RunCommand::RequestCancel { .. } => {
            if run.status.is_terminal() || run.cancel_requested_at.is_some() {
                return Ok(RunChange::default());
            }
            if run.status == RunStatus::Queued {
                // 還沒有任何程序：立即取消，不需要等任何確認
                return edit.finish(RunStatus::Cancelled, EVENT_RUN_CANCELLED, "cancelled_before_launch", TaskStatus::Ready, None);
            }
            edit.run.cancel_requested_at = Some(now);
            edit.push(EVENT_RUN_CANCEL_REQUESTED, serde_json::json!({}));
            Ok(edit.into_change())
        }
        RunCommand::ConfirmCancelled { .. } => {
            if run.cancel_requested_at.is_none() && !run.status.is_terminal() {
                return Err(RunError::CancelNotRequested);
            }
            if run.status == RunStatus::Cancelled {
                return Ok(RunChange::default());
            }
            edit.finish(RunStatus::Cancelled, EVENT_RUN_CANCELLED, "cancelled_by_user", TaskStatus::Ready, None)
        }
        RunCommand::HandOff { handoff_id, .. } => {
            if !matches!(run.status, RunStatus::Starting | RunStatus::Running) {
                return Err(RunError::TransitionInvalid { from: run.status, to: RunStatus::Interrupted });
            }
            // 交接不是失敗：不計入任何預算（spec D-16）
            let reason = crate::handoff::HANDED_OFF_REASON;
            let mut change = edit.finish(RunStatus::Interrupted, EVENT_RUN_HANDED_OFF, reason, TaskStatus::Ready, None)?;
            if let Some(event) = change.events.iter_mut().find(|e| e.event_type == EVENT_RUN_HANDED_OFF) {
                event.payload["handoff_id"] = serde_json::json!(handoff_id);
            }
            Ok(change)
        }
        RunCommand::ParkForResource { handoff_id, .. } => {
            if !matches!(run.status, RunStatus::Starting | RunStatus::Running) {
                return Err(RunError::TransitionInvalid { from: run.status, to: RunStatus::Interrupted });
            }
            // 突然限流不是失敗：不計入任何預算（spec D-16）
            let reason = crate::handoff::HANDED_OFF_WAITING_REASON;
            let mut change =
                edit.finish(RunStatus::Interrupted, EVENT_RUN_HANDED_OFF, reason, TaskStatus::WaitingResource, None)?;
            if let Some(event) = change.events.iter_mut().find(|e| e.event_type == EVENT_RUN_HANDED_OFF) {
                event.payload["handoff_id"] = serde_json::json!(handoff_id);
            }
            Ok(change)
        }
    }
}

/// Run 仍是目前的寫入所有者：尚未結束、持有 lease，而且世代與給定的世代（回報者憑證所屬）相符。
pub fn is_current_writer(run: &Run, lease: Option<&ExecutionLease>, generation: i64) -> bool {
    let held = lease.is_some_and(|l| l.is_held() && l.generation == generation);
    !run.status.is_terminal() && run.generation == generation && held
}

/// 啟動准入：Run 的 Task 目前契約需要的能力，必須被這一刻生效的授權涵蓋（Workspace 也必須仍在使用中）。
/// 派工時記下的 revision 不算數：排隊中或有 Outbox 的 Run 都還沒准入（R1-a）。
fn admit_at_launch(ctx: &RunContext<'_>, run: &Run) -> Result<authorization::Admission, AdmissionDenied> {
    let required = ctx
        .task
        .task(&run.task_id)
        .and_then(|task| ctx.task.contract_of(task))
        .map(|contract| contract.content.required_capabilities.as_slice())
        .unwrap_or_default();
    authorization::admit(ctx.task.workspace_id, ctx.task.workspace, ctx.task.authorization, required)
}

/// 准入被拒：Run 以 `interrupted` 結束、釋放寫入所有權、Task 回到 `ready`（Store 在同一交易中重新排隊）。
/// 授權不足不是失敗，不計入任何預算（票 36）。
fn deny_admission(edit: Editing<'_>, denied: &AdmissionDenied, current: Option<i64>) -> Result<RunChange, RunError> {
    let reason = format!("{ADMISSION_DENIED_REASON}：{denied}");
    let mut change =
        edit.finish(RunStatus::Interrupted, EVENT_RUN_ADMISSION_DENIED, &reason, TaskStatus::Ready, None)?;
    if let Some(event) = change.events.iter_mut().find(|e| e.event_type == EVENT_RUN_ADMISSION_DENIED) {
        event.payload["code"] = serde_json::json!(denied.code());
        event.payload["authorization_revision"] = serde_json::json!(current);
    }
    Ok(change)
}

/// 回報必須來自目前的寫入所有者。
fn check_report(run: &Run, lease: Option<&ExecutionLease>, generation: i64) -> Result<(), RunError> {
    if !is_current_writer(run, lease, generation) {
        return Err(RunError::StaleReport);
    }
    Ok(())
}

/// 派工前的檢查通過後，派工交易需要用到的事實。
struct Checked<'a> {
    task: &'a Task,
    contract: &'a task::TaskContract,
    admission: authorization::Admission,
    workdir: &'a Workdir,
}

/// 派工的前置檢查（與帳號、容量無關）：Task 必須是 `ready`、沒有人持有寫入所有權、准入檢查通過（授權可能在
/// ready 之後被撤銷，AI-61）、適用的 Guard 都沒有違反（票 38）、相依都已完成，而且 Task 的工作目錄已經登記為可用（票 20）。Scheduler 用同一個檢查決定「現在能不能派」或「為什麼要等」。
pub fn check_dispatch(ctx: &RunContext<'_>, task_id: &str) -> Result<(), RunError> {
    check_dispatch_inner(ctx, task_id).map(|_| ())
}

fn check_dispatch_inner<'a>(ctx: &'a RunContext<'a>, task_id: &str) -> Result<Checked<'a>, RunError> {
    let task = ctx.task.task(task_id).ok_or_else(|| RunError::TaskNotFound(task_id.to_string()))?;
    if task.status != TaskStatus::Ready {
        return Err(RunError::TaskNotReady(task.status));
    }
    // 寫入所有權：只要還有一個沒釋放的 lease，就不能再派
    if let Some(held) = ctx.held_lease_of_task(&task.id) {
        return Err(RunError::ExecutionConflict { task_id: task.id.clone(), run_id: held.run_id.clone() });
    }
    let contract = ctx.task.contract_of(task).ok_or_else(|| RunError::TaskNotFound(task_id.to_string()))?;
    // 授權可能在 ready 之後被撤銷，所以派工時重新做完整的准入檢查（AI-61）
    let missing = task::admission_report(ctx.task, contract);
    if !missing.is_empty() {
        return Err(RunError::AdmissionDenied(missing));
    }
    let admission = authorization::admit(
        ctx.task.workspace_id,
        ctx.task.workspace,
        ctx.task.authorization,
        &contract.content.required_capabilities,
    )?;
    // 已驗證教訓轉成的 Guard：在 Run 准入時檢查，違反就不派（票 38）
    let violations = guard::violations(ctx.guards, task, contract, ctx.runs);
    if !violations.is_empty() {
        return Err(RunError::GuardViolated(violations));
    }
    // 相依是否完成是派工時才檢查的條件（AI-61）
    let unfinished: Vec<String> = ctx
        .task
        .dependencies
        .iter()
        .filter(|d| d.task_id == task.id)
        .filter(|d| ctx.task.task(&d.depends_on).is_none_or(|t| t.status != TaskStatus::Completed))
        .map(|d| d.depends_on.clone())
        .collect();
    if !unfinished.is_empty() {
        return Err(RunError::DependenciesUnfinished(unfinished));
    }
    // 工作目錄在交易外由 Runtime 準備並登記；沒有可用的目錄就不派工
    let kind = task.kind.workdir_kind();
    let workdir = match ctx.workdirs.iter().find(|w| w.task_id == task.id && w.kind == kind) {
        Some(w) if w.is_ready() => w,
        Some(w) => return Err(RunError::WorkdirUnavailable(w.failure.clone().unwrap_or_default())),
        None => return Err(RunError::WorkdirNotPrepared(kind)),
    };
    Ok(Checked { task, contract, admission, workdir })
}

fn dispatch(
    ctx: &RunContext<'_>,
    workspace: &Workspace,
    run_id: String,
    task_id: String,
    profile_id: String,
    now: Timestamp,
) -> Result<RunChange, RunError> {
    let Checked { task, contract, admission, workdir } = check_dispatch_inner(ctx, &task_id)?;

    let generation = ctx.runs.iter().filter(|r| r.task_id == task.id).map(|r| r.generation).max().unwrap_or(0) + 1;
    let mut run = Run {
        id: run_id.clone(),
        workspace_id: ctx.task.workspace_id.to_string(),
        task_id: task.id.clone(),
        profile_id: profile_id.clone(),
        status: RunStatus::Queued,
        version: 0,
        generation,
        contract_version: task.contract_version,
        authorization_revision: admission.authorization_revision,
        admitted_at: None,
        settings_version: None,
        command_id: None,
        process: None,
        cancel_requested_at: None,
        end_reason: None,
        created_at: now,
        updated_at: now,
        started_at: None,
        ended_at: None,
        native_session_id: None,
        agent_session_id: None,
        resume_native_session_id: None,
    };
    let lease = ExecutionLease {
        run_id: run_id.clone(),
        task_id: task.id.clone(),
        generation,
        acquired_at: now,
        last_heartbeat_at: now,
        expires_at: now.plus_millis(LEASE_DURATION_MS),
        released_at: None,
        release_reason: None,
    };
    let outbox = OutboxItem {
        kind: OUTBOX_LAUNCH_RUN.to_string(),
        run_id: run_id.clone(),
        payload: LaunchPayload {
            task_id: task.id.clone(),
            prompt: render_prompt(&task.title, &contract.content),
            working_dir: workdir.path.clone(),
            model: workspace.model_overrides.model_for(AgentRole::Worker).map(str::to_string),
        },
        created_at: now,
        delivered_at: None,
    };

    let mut edit =
        Editing { run: run.clone(), lease: Some(lease), events: Vec::new(), sessions: SessionEdits::default(), now, ctx };
    edit.push(
        EVENT_RUN_CREATED,
        serde_json::json!({
            "task_id": task.id,
            "profile_id": profile_id,
            "generation": generation,
            "contract_version": task.contract_version,
            "authorization_revision": admission.authorization_revision,
            "working_dir": workdir.path,
        }),
    );
    run = edit.run;

    let mut moved = task.clone();
    let mut versioned = Versioned { task: &mut moved, events: Vec::new(), now };
    versioned.change_status(TaskStatus::Running, "dispatched").map_err(|_| RunError::TaskNotReady(task.status))?;
    edit.events.extend(versioned.events);

    Ok(RunChange {
        run: Some(run),
        lease: edit.lease,
        task: Some(moved),
        outbox: Some(outbox),
        outbox_delivered: None,
        report: None,
        budget: None,
        decision: None,
        requeue: false,
        sessions: Vec::new(),
        events: edit.events,
    })
}

/// 已准入、尚未結束、仍沿用舊授權 revision 的 Run：舊 revision 有、目前 revision 已撤銷或縮小的能力，
/// 它仍然保有，新准入的 Run 不再取得（R1-a）。撤權沒有固定完成時間，使用者可以自行暫停或取消它（票 24）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegacyRun {
    pub run_id: String,
    pub task_id: String,
    pub task_title: String,
    pub status: RunStatus,
    /// 這個 Run 沿用的 revision（比目前生效的舊）。
    pub authorization_revision: i64,
    pub admitted_at: Option<Timestamp>,
    /// 已要求停止，正在等程序確認停止。
    pub cancel_requested: bool,
    pub revoked_capabilities: Vec<authorization::Capability>,
}

/// 找出沿用舊授權、而且保有已撤銷範圍的 Run；`revisions` 由舊到新，最後一個是目前生效的。依建立順序。
pub fn legacy_runs(runs: &[Run], tasks: &[Task], revisions: &[authorization::AuthorizationRevision]) -> Vec<LegacyRun> {
    let Some(current) = revisions.last() else { return Vec::new() };
    runs.iter()
        .filter(|r| matches!(r.status, RunStatus::Starting | RunStatus::Running))
        .filter_map(|run| {
            let number = run.authorization_revision.filter(|n| *n < current.revision)?;
            let admitted = revisions.iter().find(|r| r.revision == number)?;
            let revoked = authorization::revoked_capabilities(admitted, current);
            (!revoked.is_empty()).then(|| LegacyRun {
                run_id: run.id.clone(),
                task_id: run.task_id.clone(),
                task_title: tasks.iter().find(|t| t.id == run.task_id).map_or_else(|| run.task_id.clone(), |t| t.title.clone()),
                status: run.status,
                authorization_revision: number,
                admitted_at: run.admitted_at,
                cancel_requested: run.cancel_requested_at.is_some(),
                revoked_capabilities: revoked,
            })
        })
        .collect()
}

/// 給 Worker 的最小提示：標題與六項契約（Context Builder 由後續的票加入，屆時取代這個函式）。
fn render_prompt(title: &str, content: &ContractContent) -> String {
    let mut prompt = format!("Task：{title}");
    for name in ContractFieldName::ALL {
        let text = match content.field(name) {
            Some(ContractField::Filled { text, .. }) => text.clone(),
            Some(ContractField::NotApplicable { reason }) => format!("（不適用：{reason}）"),
            Some(ContractField::Unknown { .. }) => "（未知）".to_string(),
            None => "（未提供）".to_string(),
        };
        prompt.push_str(&format!("\n\n{}：{text}", name.label()));
    }
    if !content.verification_checks.is_empty() {
        prompt.push_str("\n\n確定性檢查（你結束後由 Runtime 逐項執行，全部通過 Task 才算完成）：");
        for check in &content.verification_checks {
            prompt.push_str(&format!("\n- {}", check.describe()));
        }
    }
    prompt
}

#[cfg(test)]
mod tests;
