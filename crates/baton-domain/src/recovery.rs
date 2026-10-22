//! Runtime 意外崩潰後的自動恢復（票 31；DR-20261021-05、ADR-0005）。
//!
//! 純邏輯、無 I/O。幾條貫穿全檔的規則：
//! - **一個恢復事件、最多三次嘗試**：Runtime 意外終止後再次啟動時建立（或沿用）恢復事件；一次嘗試是一次完整的
//!   「盤點、核對、受控停止舊程序、以新 Run 恢復」，不是每個 Worker 各一次。第二、第三次之前分別至少等待 30 秒、2 分鐘。
//! - **嘗試序號先持久化**：`Begin` 套用並寫入之後才可以停止或啟動任何程序；無法保存就不開始。
//!   所以在任何邊界中斷，下次啟動看到的都是「這次嘗試已經用掉」，不會突破三次上限。
//! - **穩定 10 分鐘才算成功**：處置完成後進入穩定觀察，觀察期滿才持久化成功並結束事件；
//!   觀察期內再次崩潰沿用原事件與已用次數。
//! - **用盡後阻擋**：三次都沒有成功就持久化「恢復額度用盡」，停止自動恢復與派工；重新啟動、等待都不清除，
//!   只有使用者從恢復介面要求「重新嘗試恢復」才建立新輪次（舊輪次的嘗試保留為歷史）。
//! - 恢復次數與 Task 的修正預算、額度交接互不相干（spec D-16）。

use serde::{Deserialize, Serialize};

use crate::event::DomainEvent;
use crate::scheduler::{WaitCategory, WaitReason};
use crate::Timestamp;

pub const RECOVERY_AGGREGATE: &str = "recovery_event";

pub const EVENT_RUNTIME_CRASH_DETECTED: &str = "runtime_crash_detected";
pub const EVENT_RECOVERY_ATTEMPT_STARTED: &str = "recovery_attempt_started";
pub const EVENT_RECOVERY_ATTEMPT_COMPLETED: &str = "recovery_attempt_completed";
pub const EVENT_RECOVERY_ATTEMPT_FAILED: &str = "recovery_attempt_failed";
pub const EVENT_RECOVERY_SUCCEEDED: &str = "recovery_succeeded";
pub const EVENT_RECOVERY_EXHAUSTED: &str = "recovery_exhausted";
pub const EVENT_RECOVERY_RETRY_REQUESTED: &str = "recovery_retry_requested";

/// 同一恢復事件（同一輪）最多的自動嘗試次數（含第一次）。
pub const MAX_ATTEMPTS: u32 = 3;
/// 第二次嘗試之前至少等待的時間。
pub const BACKOFF_BEFORE_SECOND_MS: i64 = 30_000;
/// 第三次嘗試之前至少等待的時間。
pub const BACKOFF_BEFORE_THIRD_MS: i64 = 120_000;
/// 處置完成後，連續穩定多久才持久化成功。
pub const STABLE_OBSERVATION_MS: i64 = 600_000;

/// 恢復嘗試停下舊程序後，Run 的結束原因（Task 重新排隊、以新 Run 恢復）。
pub const RUN_STOPPED_BY_RECOVERY_REASON: &str = "stopped_by_crash_recovery";

/// 第 `seq` 次嘗試之前至少要等待的時間（從上一次失敗被記下的時刻算起）。
pub fn backoff_before(seq: u32) -> i64 {
    match seq {
        0 | 1 => 0,
        2 => BACKOFF_BEFORE_SECOND_MS,
        _ => BACKOFF_BEFORE_THIRD_MS,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryStatus {
    /// 等待下一次嘗試（第一次立即可做；之後是退避期間）。
    Waiting,
    /// 嘗試進行中：核對、停止舊程序、重新排隊。
    Attempting,
    /// 處置完成，穩定觀察中。
    Observing,
    /// 穩定期滿，事件結束。
    Succeeded,
    /// 三次都沒有成功：停止自動恢復，等使用者決定。
    Exhausted,
}

impl RecoveryStatus {
    pub const ALL: [RecoveryStatus; 5] = [
        RecoveryStatus::Waiting,
        RecoveryStatus::Attempting,
        RecoveryStatus::Observing,
        RecoveryStatus::Succeeded,
        RecoveryStatus::Exhausted,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            RecoveryStatus::Waiting => "waiting",
            RecoveryStatus::Attempting => "attempting",
            RecoveryStatus::Observing => "observing",
            RecoveryStatus::Succeeded => "succeeded",
            RecoveryStatus::Exhausted => "exhausted",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.as_str() == raw)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptOutcome {
    /// 核對與處置進行中。
    InProgress,
    /// 處置完成，穩定觀察中。
    Observing,
    /// 穩定期滿。
    Succeeded,
    /// 這次嘗試沒有成功（Runtime 再次終止，或無法確認舊程序停止）。
    Failed,
}

impl AttemptOutcome {
    pub const ALL: [AttemptOutcome; 4] =
        [AttemptOutcome::InProgress, AttemptOutcome::Observing, AttemptOutcome::Succeeded, AttemptOutcome::Failed];

    pub fn as_str(self) -> &'static str {
        match self {
            AttemptOutcome::InProgress => "in_progress",
            AttemptOutcome::Observing => "observing",
            AttemptOutcome::Succeeded => "succeeded",
            AttemptOutcome::Failed => "failed",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|o| o.as_str() == raw)
    }
}

/// 一次嘗試對一個受影響 Run 的處置。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunDisposition {
    /// 舊程序確認停止：Run 結束，Task 重新排隊、以新 Run 恢復（仍經過授權、設定與相依檢查）。
    Requeued,
    /// 使用者在崩潰前已要求取消：舊程序確認停止後完成取消，不恢復。
    Cancelled,
    /// 無法確認舊程序停止：Run 保留寫入所有權，不建立新 Run；這次嘗試失敗。
    Unconfirmed,
}

/// 受影響的工作（UI 顯示用）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AffectedRun {
    pub run_id: String,
    pub workspace_id: String,
    pub task_id: String,
    pub disposition: RunDisposition,
    pub message: Option<String>,
}

/// 一次恢復嘗試。`(round, seq)` 在同一事件中唯一，`seq` 只會是 1～3。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryAttempt {
    pub id: String,
    pub round: u32,
    pub seq: u32,
    pub started_at: Timestamp,
    pub finished_at: Option<Timestamp>,
    pub outcome: AttemptOutcome,
    pub failure: Option<String>,
    pub affected: Vec<AffectedRun>,
}

/// 一個恢復事件：從偵測到崩潰起，到穩定成功或使用者處理為止。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryEvent {
    pub id: String,
    /// 輪次：從 1 開始；每次人工「重新嘗試恢復」加一。
    pub round: u32,
    pub status: RecoveryStatus,
    /// 目前這一輪已用掉的嘗試次數（開始即計次）。
    pub attempts_used: u32,
    pub detected_at: Timestamp,
    /// 下一次嘗試最早的時間（退避期限）；只有 `waiting` 有值。
    pub next_attempt_at: Option<Timestamp>,
    /// 穩定觀察的期滿時間；只有 `observing` 有值。
    pub observe_until: Option<Timestamp>,
    pub last_failure: Option<String>,
    /// 穩定成功、事件結束的時間。
    pub closed_at: Option<Timestamp>,
    pub version: i64,
    pub updated_at: Timestamp,
    /// 所有輪次的嘗試，依開始順序（人工重試後舊輪次仍保留）。
    pub attempts: Vec<RecoveryAttempt>,
}

impl RecoveryEvent {
    /// 事件尚未結束（成功以外的狀態，含用盡）。
    pub fn is_open(&self) -> bool {
        self.status != RecoveryStatus::Succeeded
    }

    /// 目前這一輪進行中（處置中或觀察中）的嘗試。
    pub fn current_attempt(&self) -> Option<&RecoveryAttempt> {
        self.attempts
            .iter()
            .rev()
            .find(|a| a.round == self.round && matches!(a.outcome, AttemptOutcome::InProgress | AttemptOutcome::Observing))
    }

    fn current_attempt_mut(&mut self) -> Option<&mut RecoveryAttempt> {
        let round = self.round;
        self.attempts
            .iter_mut()
            .rev()
            .find(|a| a.round == round && matches!(a.outcome, AttemptOutcome::InProgress | AttemptOutcome::Observing))
    }

    fn push(&mut self, event_type: &'static str, now: Timestamp, payload: serde_json::Value) -> DomainEvent {
        self.version += 1;
        self.updated_at = now;
        DomainEvent {
            aggregate_type: RECOVERY_AGGREGATE,
            aggregate_id: self.id.clone(),
            aggregate_version: self.version,
            event_type,
            occurred_at: now,
            payload,
        }
    }
}

/// 套用結果：新的事件狀態（含全部嘗試）與要追加的 Domain 事件。
#[derive(Debug, Clone, PartialEq)]
pub struct RecoveryChange {
    pub event: RecoveryEvent,
    pub events: Vec<DomainEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RecoveryError {
    #[error("沒有恢復事件")]
    NoEvent,
    #[error("恢復事件目前是 {0}，不能開始新的嘗試")]
    NotWaiting(&'static str),
    #[error("退避期間：下一次嘗試最早在 {0:?}")]
    BackoffActive(Timestamp),
    #[error("恢復事件目前是 {0}，沒有進行中的嘗試")]
    NotAttempting(&'static str),
    #[error("只有恢復額度用盡（三次都沒有成功）時才能重新嘗試恢復；目前是 {0}")]
    NotExhausted(&'static str),
}

impl RecoveryError {
    pub fn code(&self) -> &'static str {
        match self {
            RecoveryError::NoEvent => "no_recovery_event",
            RecoveryError::NotWaiting(_) => "recovery_not_waiting",
            RecoveryError::BackoffActive(_) => "recovery_backoff_active",
            RecoveryError::NotAttempting(_) => "recovery_not_attempting",
            RecoveryError::NotExhausted(_) => "recovery_not_exhausted",
        }
    }
}

/// Runtime 啟動時的判定。`crashed` 是「上一次執行沒有正常結束、也沒有停止意圖」。
///
/// - 嘗試進行中：Runtime 已經不是做這次嘗試的那一個，這次嘗試失敗（不論上次是否正常結束）。
/// - 觀察中又崩潰：沿用原事件，觀察中的那次嘗試失敗。
/// - 退避中或已用盡：不改變任何東西（退避期限照舊、不另外計次、用盡不會被清除）。
/// - 沒有未結束的事件而且崩潰了：建立新事件（第 1 輪、尚未嘗試）。
pub fn on_start(current: Option<&RecoveryEvent>, crashed: bool, new_event_id: &str, now: Timestamp) -> Option<RecoveryChange> {
    match current {
        Some(event) if event.status == RecoveryStatus::Attempting => {
            let reason = if crashed { "Runtime 在恢復嘗試進行中再次意外終止" } else { "Runtime 在恢復嘗試進行中結束" };
            Some(fail_current(event, reason, None, now))
        }
        Some(event) if event.status == RecoveryStatus::Observing && crashed => {
            Some(fail_current(event, "穩定觀察期間 Runtime 再次意外終止", None, now))
        }
        Some(event) if event.is_open() => None,
        _ if crashed => {
            let mut event = RecoveryEvent {
                id: new_event_id.to_string(),
                round: 1,
                status: RecoveryStatus::Waiting,
                attempts_used: 0,
                detected_at: now,
                next_attempt_at: Some(now),
                observe_until: None,
                last_failure: None,
                closed_at: None,
                version: 0,
                updated_at: now,
                attempts: Vec::new(),
            };
            let detected = event.push(EVENT_RUNTIME_CRASH_DETECTED, now, serde_json::json!({ "round": 1 }));
            Some(RecoveryChange { event, events: vec![detected] })
        }
        _ => None,
    }
}

/// 對恢復事件下的命令。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryCommand {
    /// 開始一次嘗試（在任何程序副作用之前持久化）。
    Begin { attempt_id: String },
    /// 處置完成：進入穩定觀察。
    Complete { affected: Vec<AffectedRun> },
    /// 這次嘗試沒有成功。
    Fail { reason: String, affected: Vec<AffectedRun> },
    /// 觀察期滿就持久化成功；未滿沒有任何變動。
    Observe,
    /// 使用者要求「重新嘗試恢復」：建立新輪次。
    Retry,
}

/// 套用一個命令。`Observe` 在觀察期未滿時回傳 `Ok(None)`（沒有變動）。
pub fn decide(current: Option<&RecoveryEvent>, command: RecoveryCommand, now: Timestamp) -> Result<Option<RecoveryChange>, RecoveryError> {
    let event = current.ok_or(RecoveryError::NoEvent)?;
    match command {
        RecoveryCommand::Begin { attempt_id } => begin_attempt(event, &attempt_id, now).map(Some),
        RecoveryCommand::Complete { affected } => complete_attempt(event, affected, now).map(Some),
        RecoveryCommand::Fail { reason, affected } => {
            if event.status != RecoveryStatus::Attempting {
                return Err(RecoveryError::NotAttempting(event.status.as_str()));
            }
            Ok(Some(fail_current(event, &reason, Some(affected), now)))
        }
        RecoveryCommand::Observe => Ok(observe(event, now)),
        RecoveryCommand::Retry => retry(event, now).map(Some),
    }
}

fn begin_attempt(event: &RecoveryEvent, attempt_id: &str, now: Timestamp) -> Result<RecoveryChange, RecoveryError> {
    if event.status != RecoveryStatus::Waiting {
        return Err(RecoveryError::NotWaiting(event.status.as_str()));
    }
    if let Some(until) = event.next_attempt_at.filter(|until| now < *until) {
        return Err(RecoveryError::BackoffActive(until));
    }
    // 等待狀態一定還有額度（用完會直接進入 exhausted）；這裡再擋一次，任何情況都不會出現第四次
    if event.attempts_used >= MAX_ATTEMPTS {
        return Err(RecoveryError::NotWaiting(RecoveryStatus::Exhausted.as_str()));
    }
    let mut next = event.clone();
    next.attempts_used += 1;
    next.status = RecoveryStatus::Attempting;
    next.next_attempt_at = None;
    next.attempts.push(RecoveryAttempt {
        id: attempt_id.to_string(),
        round: next.round,
        seq: next.attempts_used,
        started_at: now,
        finished_at: None,
        outcome: AttemptOutcome::InProgress,
        failure: None,
        affected: Vec::new(),
    });
    let started = next.push(
        EVENT_RECOVERY_ATTEMPT_STARTED,
        now,
        serde_json::json!({ "attempt_id": attempt_id, "round": next.round, "seq": next.attempts_used }),
    );
    Ok(RecoveryChange { event: next, events: vec![started] })
}

fn complete_attempt(event: &RecoveryEvent, affected: Vec<AffectedRun>, now: Timestamp) -> Result<RecoveryChange, RecoveryError> {
    if event.status != RecoveryStatus::Attempting {
        return Err(RecoveryError::NotAttempting(event.status.as_str()));
    }
    let mut next = event.clone();
    let observe_until = now.plus_millis(STABLE_OBSERVATION_MS);
    next.status = RecoveryStatus::Observing;
    next.observe_until = Some(observe_until);
    let attempt = next.current_attempt_mut().expect("嘗試進行中一定有目前的嘗試");
    attempt.outcome = AttemptOutcome::Observing;
    let payload = serde_json::json!({
        "attempt_id": attempt.id, "seq": attempt.seq, "affected": affected.len(), "observe_until": observe_until,
    });
    attempt.affected = affected;
    let completed = next.push(EVENT_RECOVERY_ATTEMPT_COMPLETED, now, payload);
    Ok(RecoveryChange { event: next, events: vec![completed] })
}

/// 目前的嘗試失敗：還有額度就等待退避，否則恢復額度用盡。`affected` 為 `None` 時保留嘗試原有的處置紀錄。
fn fail_current(event: &RecoveryEvent, reason: &str, affected: Option<Vec<AffectedRun>>, now: Timestamp) -> RecoveryChange {
    let mut next = event.clone();
    let mut events = Vec::new();
    if let Some(attempt) = next.current_attempt_mut() {
        attempt.outcome = AttemptOutcome::Failed;
        attempt.finished_at = Some(now);
        attempt.failure = Some(reason.to_string());
        if let Some(affected) = affected {
            attempt.affected = affected;
        }
        let payload = serde_json::json!({ "attempt_id": attempt.id, "round": attempt.round, "seq": attempt.seq, "reason": reason });
        events.push(next.push(EVENT_RECOVERY_ATTEMPT_FAILED, now, payload));
    }
    next.last_failure = Some(reason.to_string());
    next.observe_until = None;
    if next.attempts_used >= MAX_ATTEMPTS {
        next.status = RecoveryStatus::Exhausted;
        next.next_attempt_at = None;
        let payload = serde_json::json!({ "round": next.round, "attempts": next.attempts_used, "last_failure": reason });
        events.push(next.push(EVENT_RECOVERY_EXHAUSTED, now, payload));
    } else {
        next.status = RecoveryStatus::Waiting;
        next.next_attempt_at = Some(now.plus_millis(backoff_before(next.attempts_used + 1)));
    }
    RecoveryChange { event: next, events }
}

fn observe(event: &RecoveryEvent, now: Timestamp) -> Option<RecoveryChange> {
    let until = event.observe_until.filter(|_| event.status == RecoveryStatus::Observing)?;
    if now < until {
        return None;
    }
    let mut next = event.clone();
    next.status = RecoveryStatus::Succeeded;
    next.observe_until = None;
    next.closed_at = Some(now);
    let attempt = next.current_attempt_mut().expect("觀察中一定有目前的嘗試");
    attempt.outcome = AttemptOutcome::Succeeded;
    attempt.finished_at = Some(now);
    let payload = serde_json::json!({ "attempt_id": attempt.id, "round": attempt.round, "seq": attempt.seq });
    let succeeded = next.push(EVENT_RECOVERY_SUCCEEDED, now, payload);
    Some(RecoveryChange { event: next, events: vec![succeeded] })
}

fn retry(event: &RecoveryEvent, now: Timestamp) -> Result<RecoveryChange, RecoveryError> {
    if event.status != RecoveryStatus::Exhausted {
        return Err(RecoveryError::NotExhausted(event.status.as_str()));
    }
    let mut next = event.clone();
    next.round += 1;
    next.attempts_used = 0;
    next.status = RecoveryStatus::Waiting;
    next.next_attempt_at = Some(now);
    let requested = next.push(EVENT_RECOVERY_RETRY_REQUESTED, now, serde_json::json!({ "round": next.round, "by": "user" }));
    Ok(RecoveryChange { event: next, events: vec![requested] })
}

/// 派工（含排程、准備工作目錄與清理）是否被恢復擋住：等待或進行嘗試期間、以及用盡之後都不啟動新的工作。
/// 穩定觀察期間不擋（恢復出來的新 Run 就是在這段期間執行）。
pub fn dispatch_halt(event: Option<&RecoveryEvent>) -> Option<WaitReason> {
    let event = event?;
    let (code, message) = match event.status {
        RecoveryStatus::Waiting | RecoveryStatus::Attempting => (
            "crash_recovery_pending",
            format!(
                "Runtime 意外終止後正在恢復（第 {} 輪，已用 {}/{MAX_ATTEMPTS} 次嘗試），恢復完成前不啟動新的 Run",
                event.round, event.attempts_used
            ),
        ),
        RecoveryStatus::Exhausted => (
            "crash_recovery_exhausted",
            "崩潰恢復三次都沒有成功，自動恢復已停止；請檢視診斷後從恢復介面「重新嘗試恢復」".to_string(),
        ),
        RecoveryStatus::Observing | RecoveryStatus::Succeeded => return None,
    };
    Some(WaitReason { category: WaitCategory::Lifecycle, code: code.to_string(), message, profile_id: None, guard: None })
}

/// 啟動程序（launch intent）前的最後一道檢查。
pub fn launch_halted(event: Option<&RecoveryEvent>) -> bool {
    dispatch_halt(event).is_some()
}

#[cfg(test)]
mod tests;
