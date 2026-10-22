//! 檢查點與主動跨帳號交接（票 28；issue §5.2、spec D-13）。
//!
//! 純邏輯、無 I/O。兩部分：
//!
//! **帳號的額度門檻（遲滯）**：每個帳號一個 `QuotaGate`，依額度判斷（`quota::assess`）推進：
//! - 用量（兩個視窗中較緊者）≥ 75% → `paused`：不再派新 Run 給它；
//! - ≥ 85%，或目前有 CLI 的限流警告（或已達上限）→ `draining`：除了不派新工作，進行中的 Run 要交接出去；
//! - 回到 < 60%、用量變成未知（訊號已在重置時間失效），或已過進入時記下的重置時間 → 回到 `open`。
//!   60%～75% 之間維持原狀（遲滯），避免在門檻附近來回切換。
//!
//! **交接**：issue §5.2 的五個步驟以 phase 狀態機表達，每一個 phase 持久化之後才做下一步的外部副作用：
//! 1. `requested`：帳號進入 draining，已選定承接帳號（可派工、額度未達門檻、名額夠，可用交接保留槽）。
//!    Worker 的回報回應會帶 `stop_requested`，請它保存 checkpoint 並結束。
//! 2. `stopping`：到了安全邊界（舊 Run 在要求之後保存了 checkpoint，或等滿 `CHECKPOINT_WAIT_MS`）而且承接帳號仍有名額，
//!    記下要延續的 checkpoint，之後 Supervisor 才要求舊程序停止。
//! 3. `transferred`：舊程序**確認停止**（或自行結束）之後，在同一個交易中：舊 Run 以 `interrupted`（`handed_off`）結束並釋放
//!    寫入所有權、以條件更新把世代轉給新建立的 successor Run（Prompt 附上 checkpoint 與交接說明）。
//!    舊程序停不下來就一直停在 `stopping`，successor 不會被建立，所以不會有兩個 writer。
//! 4. `confirmed`：successor Worker 以 `confirm_handoff` 確認已完成、未完成與未知副作用。
//! - `abandoned`：舊 Run 已由別的路徑結束（取消、程序消失、完全退出），或轉移時沒有承接帳號；Task 依原本的流程處理。
//!
//! 交接不是失敗：舊 Run 以 `handed_off` 結束，不計入任何修正或重試預算（spec D-16）。同一個 Task 兩次換帳號至少間隔
//! 10 分鐘（冷卻，spec D-13）。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::agent::{ReportContent, ReportRecord};
use crate::event::DomainEvent;
use crate::quota::{LimitStatus, QuotaAssessment};
use crate::run::{Run, RunStatus};
use crate::scheduler::QUOTA_STOP_DISPATCH_RATIO;
use crate::Timestamp;

/// 用量達到這個比例（或收到限流警告）就進入 draining，進行中的 Run 要交接（spec D-13）。
pub const QUOTA_DRAIN_RATIO: f64 = 0.85;
/// 用量回到這個比例以下才恢復可派（spec D-13 的遲滯）。
pub const QUOTA_RESUME_RATIO: f64 = 0.60;
/// 同一個 Task 兩次換帳號的最短間隔（spec D-13）。
pub const HANDOFF_COOLDOWN_MS: i64 = 10 * 60_000;
/// 要求交接之後，最多等舊 Worker 多久保存 checkpoint，再要求它停止。
pub const CHECKPOINT_WAIT_MS: i64 = 2 * 60_000;
/// 交接而結束的 Run 的結束原因。
pub const HANDED_OFF_REASON: &str = "handed_off";

pub const QUOTA_GATE_AGGREGATE: &str = "quota_gate";
pub const EVENT_QUOTA_GATE_CHANGED: &str = "quota_gate_changed";
pub const HANDOFF_AGGREGATE: &str = "handoff";
pub const EVENT_HANDOFF_REQUESTED: &str = "handoff_requested";
pub const EVENT_HANDOFF_STOPPING: &str = "handoff_stopping";
pub const EVENT_HANDOFF_TRANSFERRED: &str = "handoff_transferred";
pub const EVENT_HANDOFF_CONFIRMED: &str = "handoff_confirmed";
pub const EVENT_HANDOFF_ABANDONED: &str = "handoff_abandoned";

/// 確認內容每一欄的長度上限（字元數）。
pub const CONFIRMATION_MAX_CHARS: usize = 4000;

// ---------------------------------------------------------------- 額度門檻

/// 帳號的額度門檻狀態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateState {
    /// 可派工。
    #[default]
    Open,
    /// 已達停止派工門檻：不派新 Run，進行中的 Run 繼續。
    Paused,
    /// 接近上限：不派新 Run，進行中的 Run 交接到其他帳號。
    Draining,
}

impl GateState {
    pub const ALL: [GateState; 3] = [GateState::Open, GateState::Paused, GateState::Draining];

    pub fn as_str(self) -> &'static str {
        match self {
            GateState::Open => "open",
            GateState::Paused => "paused",
            GateState::Draining => "draining",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.as_str() == raw)
    }

    pub fn label(self) -> &'static str {
        match self {
            GateState::Open => "可派工",
            GateState::Paused => "停止派工",
            GateState::Draining => "交接中（draining）",
        }
    }

    /// 這個狀態下不派新 Run 給該帳號。
    pub fn blocks_dispatch(self) -> bool {
        self != GateState::Open
    }
}

/// 一個帳號目前的門檻狀態；從未離開 `open` 的帳號沒有紀錄。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuotaGate {
    pub profile_id: String,
    pub state: GateState,
    /// 進入這個狀態的原因代碼（`stop_dispatch_threshold`、`drain_threshold`、`limit_warning`、`recovered`、`reset_observed`、`usage_unknown`）。
    pub reason: String,
    /// 轉換當下的判斷用量。
    pub used: Option<f64>,
    /// 進入 paused／draining 時記下的重置時間；過了這個時間就視為已重置。
    pub reset_at: Option<Timestamp>,
    pub since: Timestamp,
    pub version: i64,
}

/// 額度判斷中是否有目前仍有效的 CLI 限流警告（含已達上限）。
pub fn has_limit_warning(assessment: &QuotaAssessment) -> bool {
    assessment
        .windows
        .iter()
        .filter_map(|w| w.observed.as_ref())
        .any(|o| matches!(o.status, Some(LimitStatus::Warning | LimitStatus::Exhausted)))
}

/// 依最新的額度判斷推進一個帳號的門檻；狀態沒有改變時回傳 `None`（不寫入）。
pub fn evaluate_gate(
    profile_id: &str,
    current: Option<&QuotaGate>,
    assessment: &QuotaAssessment,
    now: Timestamp,
) -> Option<(QuotaGate, DomainEvent)> {
    let previous = current.map_or(GateState::Open, |g| g.state);
    let reset = current.and_then(|g| g.reset_at).is_some_and(|reset_at| now >= reset_at);
    let used = assessment.used;
    let (state, reason) = if has_limit_warning(assessment) {
        (GateState::Draining, "limit_warning")
    } else {
        match used {
            Some(u) if u >= QUOTA_DRAIN_RATIO => (GateState::Draining, "drain_threshold"),
            // 已在 draining 的帳號沒有重置之前不會因為用量稍降就退回 paused
            Some(u) if u >= QUOTA_STOP_DISPATCH_RATIO && previous == GateState::Draining && !reset => {
                (GateState::Draining, current.map_or("drain_threshold", |g| g.reason.as_str()))
            }
            Some(u) if u >= QUOTA_STOP_DISPATCH_RATIO => (GateState::Paused, "stop_dispatch_threshold"),
            None => (GateState::Open, "usage_unknown"),
            Some(u) if u < QUOTA_RESUME_RATIO => (GateState::Open, "recovered"),
            // 60%～75%：遲滯，維持原狀；已過重置時間則恢復
            Some(_) if reset => (GateState::Open, "reset_observed"),
            Some(_) => (previous, current.map_or("recovered", |g| g.reason.as_str())),
        }
    };
    if state == previous {
        return None;
    }
    let version = current.map_or(0, |g| g.version) + 1;
    let gate = QuotaGate {
        profile_id: profile_id.to_string(),
        state,
        reason: reason.to_string(),
        used,
        reset_at: if state == GateState::Open { None } else { assessment.reset_at },
        since: now,
        version,
    };
    let event = DomainEvent {
        aggregate_type: QUOTA_GATE_AGGREGATE,
        aggregate_id: profile_id.to_string(),
        aggregate_version: version,
        event_type: EVENT_QUOTA_GATE_CHANGED,
        occurred_at: now,
        payload: serde_json::json!({
            "from": previous,
            "to": state,
            "reason": reason,
            "used": used,
            "reset_at": gate.reset_at,
        }),
    };
    Some((gate, event))
}

// ---------------------------------------------------------------- 交接

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HandoffPhase {
    Requested,
    Stopping,
    Transferred,
    Confirmed,
    Abandoned,
}

impl HandoffPhase {
    pub const ALL: [HandoffPhase; 5] = [
        HandoffPhase::Requested,
        HandoffPhase::Stopping,
        HandoffPhase::Transferred,
        HandoffPhase::Confirmed,
        HandoffPhase::Abandoned,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            HandoffPhase::Requested => "requested",
            HandoffPhase::Stopping => "stopping",
            HandoffPhase::Transferred => "transferred",
            HandoffPhase::Confirmed => "confirmed",
            HandoffPhase::Abandoned => "abandoned",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.as_str() == raw)
    }

    /// 舊 Run 仍可能是寫入者、交接還在進行（requested、stopping）。
    pub fn is_in_flight(self) -> bool {
        matches!(self, HandoffPhase::Requested | HandoffPhase::Stopping)
    }
}

/// 交接要延續的 checkpoint（Worker 以 `save_checkpoint` 回報、已被接受的最新一筆）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointRef {
    pub run_id: String,
    pub generation: i64,
    pub summary: String,
    pub details: String,
    pub saved_at: Timestamp,
}

/// successor Worker 接手時的確認。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandoffConfirmation {
    /// 已完成的部分。
    pub completed: String,
    /// 尚未完成的部分。
    pub incomplete: String,
    /// 結果不明的副作用（沒有就寫「無」）。
    pub unknown_side_effects: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfirmationError {
    #[error("{0}不能是空白（沒有就寫「無」）")]
    Empty(&'static str),
    #[error("{0}最多 {CONFIRMATION_MAX_CHARS} 個字元")]
    TooLong(&'static str),
}

impl ConfirmationError {
    pub fn code(&self) -> &'static str {
        match self {
            ConfirmationError::Empty(_) => "handoff_confirmation_empty",
            ConfirmationError::TooLong(_) => "handoff_confirmation_too_long",
        }
    }
}

impl HandoffConfirmation {
    /// 三欄都必須填寫（去掉前後空白），而且不超過長度上限。
    pub fn new(completed: &str, incomplete: &str, unknown_side_effects: &str) -> Result<Self, ConfirmationError> {
        let field = |raw: &str, label: &'static str| {
            let text = raw.trim();
            if text.is_empty() {
                return Err(ConfirmationError::Empty(label));
            }
            if text.chars().count() > CONFIRMATION_MAX_CHARS {
                return Err(ConfirmationError::TooLong(label));
            }
            Ok(text.to_string())
        };
        Ok(Self {
            completed: field(completed, "已完成")?,
            incomplete: field(incomplete, "未完成")?,
            unknown_side_effects: field(unknown_side_effects, "未知副作用")?,
        })
    }
}

/// 一次交接（handoff 紀錄）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Handoff {
    pub id: String,
    pub workspace_id: String,
    pub task_id: String,
    pub from_run_id: String,
    pub from_profile_id: String,
    pub from_generation: i64,
    /// 要求交接時選定的承接帳號；轉移時名額變了會改用別的可派帳號，最後以 `to_profile_id` 為準。
    pub to_profile_id: String,
    pub to_run_id: Option<String>,
    pub to_generation: Option<i64>,
    pub phase: HandoffPhase,
    /// 為什麼交接（帳號門檻的原因代碼，例如 `drain_threshold`、`limit_warning`）。
    pub reason: String,
    pub checkpoint: Option<CheckpointRef>,
    /// 舊程序怎麼停下的（`stopped_gracefully`、`force_killed`、`exited`、`dead`）。
    pub stop_outcome: Option<String>,
    pub confirmation: Option<HandoffConfirmation>,
    pub abandoned_reason: Option<String>,
    pub requested_at: Timestamp,
    pub stopping_at: Option<Timestamp>,
    pub transferred_at: Option<Timestamp>,
    pub confirmed_at: Option<Timestamp>,
    pub updated_at: Timestamp,
    pub version: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HandoffError {
    #[error("交接目前是 {} 階段，不能{}", .from.as_str(), .action)]
    PhaseInvalid { from: HandoffPhase, action: &'static str },
    #[error("這個 Run 沒有等待確認的交接")]
    NotPending,
}

impl HandoffError {
    pub fn code(&self) -> &'static str {
        match self {
            HandoffError::PhaseInvalid { .. } => "handoff_phase_invalid",
            HandoffError::NotPending => "handoff_not_pending",
        }
    }
}

impl Handoff {
    fn push(&mut self, event_type: &'static str, now: Timestamp, payload: serde_json::Value) -> DomainEvent {
        self.version += 1;
        self.updated_at = now;
        DomainEvent {
            aggregate_type: HANDOFF_AGGREGATE,
            aggregate_id: self.id.clone(),
            aggregate_version: self.version,
            event_type,
            occurred_at: now,
            payload,
        }
    }
}

/// 步驟 1：帳號進入 draining，為進行中的 Run 建立交接紀錄（已選定承接帳號）。
pub fn request(id: String, run: &Run, to_profile_id: &str, reason: &str, now: Timestamp) -> (Handoff, DomainEvent) {
    let mut handoff = Handoff {
        id,
        workspace_id: run.workspace_id.clone(),
        task_id: run.task_id.clone(),
        from_run_id: run.id.clone(),
        from_profile_id: run.profile_id.clone(),
        from_generation: run.generation,
        to_profile_id: to_profile_id.to_string(),
        to_run_id: None,
        to_generation: None,
        phase: HandoffPhase::Requested,
        reason: reason.to_string(),
        checkpoint: None,
        stop_outcome: None,
        confirmation: None,
        abandoned_reason: None,
        requested_at: now,
        stopping_at: None,
        transferred_at: None,
        confirmed_at: None,
        updated_at: now,
        version: 0,
    };
    let event = handoff.push(
        EVENT_HANDOFF_REQUESTED,
        now,
        serde_json::json!({
            "task_id": run.task_id,
            "from_run_id": run.id,
            "from_profile_id": run.profile_id,
            "from_generation": run.generation,
            "to_profile_id": to_profile_id,
            "reason": reason,
        }),
    );
    (handoff, event)
}

/// 這個 Task 下一次可以換帳號的時間（冷卻）；從未換過帳號為 `None`。
pub fn cooldown_until(handoffs: &[Handoff], task_id: &str) -> Option<Timestamp> {
    handoffs
        .iter()
        .filter(|h| h.task_id == task_id)
        .filter_map(|h| h.transferred_at)
        .max()
        .map(|at| at.plus_millis(HANDOFF_COOLDOWN_MS))
}

/// 現在要交接出去的 Run：帳號在 draining、程序在執行、沒有要求取消、沒有進行中的交接，而且 Task 不在冷卻期。
pub fn candidates<'a>(
    runs: &'a [Run],
    gates: &HashMap<String, QuotaGate>,
    handoffs: &[Handoff],
    now: Timestamp,
) -> Vec<(&'a Run, String)> {
    runs.iter()
        .filter(|r| r.status == RunStatus::Running && r.cancel_requested_at.is_none())
        .filter_map(|r| {
            let gate = gates.get(&r.profile_id).filter(|g| g.state == GateState::Draining)?;
            let busy = handoffs.iter().any(|h| h.from_run_id == r.id && h.phase.is_in_flight());
            let cooling = cooldown_until(handoffs, &r.task_id).is_some_and(|until| now < until);
            (!busy && !cooling).then(|| (r, gate.reason.clone()))
        })
        .collect()
}

/// Task 最新一筆被接受的 checkpoint（任一世代的 Run 保存的都算，取最晚保存者）。
pub fn latest_checkpoint(reports: &[ReportRecord], runs: &[Run], task_id: &str) -> Option<CheckpointRef> {
    reports
        .iter()
        .filter(|r| r.accepted && runs.iter().any(|run| run.id == r.run_id && run.task_id == task_id))
        .filter_map(|r| match &r.content {
            ReportContent::Checkpoint { summary, details } => Some(CheckpointRef {
                run_id: r.run_id.clone(),
                generation: r.generation,
                summary: summary.clone(),
                details: details.clone(),
                saved_at: r.received_at,
            }),
            _ => None,
        })
        .max_by_key(|c| c.saved_at)
}

/// 到了安全邊界嗎：舊 Run 在要求交接之後保存了 checkpoint，或已等滿 `CHECKPOINT_WAIT_MS`。
pub fn at_safe_boundary(handoff: &Handoff, latest: Option<&CheckpointRef>, now: Timestamp) -> bool {
    let saved_after_request =
        latest.is_some_and(|c| c.run_id == handoff.from_run_id && c.saved_at >= handoff.requested_at);
    saved_after_request || now >= handoff.requested_at.plus_millis(CHECKPOINT_WAIT_MS)
}

/// 步驟 2：記下要延續的 checkpoint，進入 `stopping`（之後才要求舊程序停止）。已在 `stopping` 時沒有變動。
pub fn begin_stop(
    handoff: &Handoff,
    checkpoint: Option<CheckpointRef>,
    now: Timestamp,
) -> Result<Option<(Handoff, DomainEvent)>, HandoffError> {
    match handoff.phase {
        HandoffPhase::Stopping => return Ok(None),
        HandoffPhase::Requested => {}
        from => return Err(HandoffError::PhaseInvalid { from, action: "要求舊程序停止" }),
    }
    let mut next = handoff.clone();
    next.phase = HandoffPhase::Stopping;
    next.stopping_at = Some(now);
    next.checkpoint = checkpoint;
    let payload = serde_json::json!({
        "checkpoint_run_id": next.checkpoint.as_ref().map(|c| c.run_id.clone()),
        "checkpoint_saved_at": next.checkpoint.as_ref().map(|c| c.saved_at),
    });
    let event = next.push(EVENT_HANDOFF_STOPPING, now, payload);
    Ok(Some((next, event)))
}

/// 步驟 3：舊程序已確認停止，世代轉給 successor Run。
pub fn transfer(
    handoff: &Handoff,
    successor: &Run,
    checkpoint: Option<CheckpointRef>,
    stop_outcome: &str,
    now: Timestamp,
) -> Result<(Handoff, DomainEvent), HandoffError> {
    if !handoff.phase.is_in_flight() {
        return Err(HandoffError::PhaseInvalid { from: handoff.phase, action: "轉移給 successor" });
    }
    let mut next = handoff.clone();
    next.phase = HandoffPhase::Transferred;
    next.to_profile_id = successor.profile_id.clone();
    next.to_run_id = Some(successor.id.clone());
    next.to_generation = Some(successor.generation);
    next.checkpoint = checkpoint;
    next.stop_outcome = Some(stop_outcome.to_string());
    next.transferred_at = Some(now);
    let payload = serde_json::json!({
        "to_run_id": successor.id,
        "to_profile_id": successor.profile_id,
        "to_generation": successor.generation,
        "stop_outcome": stop_outcome,
        "checkpoint_run_id": next.checkpoint.as_ref().map(|c| c.run_id.clone()),
    });
    let event = next.push(EVENT_HANDOFF_TRANSFERRED, now, payload);
    Ok((next, event))
}

/// 放棄交接：舊 Run 已由別的路徑結束，或轉移時沒有承接帳號。
pub fn abandon(handoff: &Handoff, reason: &str, now: Timestamp) -> Result<(Handoff, DomainEvent), HandoffError> {
    if !handoff.phase.is_in_flight() {
        return Err(HandoffError::PhaseInvalid { from: handoff.phase, action: "放棄" });
    }
    let mut next = handoff.clone();
    next.phase = HandoffPhase::Abandoned;
    next.abandoned_reason = Some(reason.to_string());
    let event = next.push(EVENT_HANDOFF_ABANDONED, now, serde_json::json!({ "reason": reason }));
    Ok((next, event))
}

/// 步驟 4：successor Worker 確認接手。同一個 Run 重複確認是冪等的（回傳 `None`）。
pub fn confirm(
    handoff: &Handoff,
    run_id: &str,
    confirmation: HandoffConfirmation,
    now: Timestamp,
) -> Result<Option<(Handoff, DomainEvent)>, HandoffError> {
    if handoff.to_run_id.as_deref() != Some(run_id) {
        return Err(HandoffError::NotPending);
    }
    match handoff.phase {
        HandoffPhase::Confirmed => return Ok(None),
        HandoffPhase::Transferred => {}
        _ => return Err(HandoffError::NotPending),
    }
    let mut next = handoff.clone();
    next.phase = HandoffPhase::Confirmed;
    next.confirmed_at = Some(now);
    let payload = serde_json::json!({
        "run_id": run_id,
        "completed": confirmation.completed,
        "incomplete": confirmation.incomplete,
        "unknown_side_effects": confirmation.unknown_side_effects,
    });
    next.confirmation = Some(confirmation);
    let event = next.push(EVENT_HANDOFF_CONFIRMED, now, payload);
    Ok(Some((next, event)))
}

/// 附在 successor Run Prompt 後面的交接說明：要延續的 checkpoint 與接手時必須確認的事項。
pub fn successor_prompt(handoff: &Handoff, checkpoint: Option<&CheckpointRef>) -> String {
    let mut text = format!(
        "\n\n交接：這個 Task 原本由 Run {}（第 {} 世代）在另一個帳號執行，因額度接近上限交接給你。\
         舊程序已確認停止，工作目錄中的變更都是它留下的實際成果。",
        handoff.from_run_id, handoff.from_generation
    );
    match checkpoint {
        Some(c) => {
            text.push_str(&format!("\n最近的有效檢查點（Run {}，第 {} 世代保存）：{}", c.run_id, c.generation, c.summary));
            if !c.details.trim().is_empty() {
                text.push_str(&format!("\n檢查點細節：\n{}", c.details));
            }
        }
        None => text.push_str("\n舊 Run 沒有保存任何檢查點：請從工作目錄的實際狀態與已登記的成果判斷進度。"),
    }
    text.push_str(
        "\n開始工作前，先核對檢查點與工作目錄、成果的實際狀態，再呼叫 confirm_handoff 回報已完成、未完成與結果不明的副作用；\
         不要重做已完成的部分，結果不明的外部操作不要直接重送。",
    );
    text
}

#[cfg(test)]
mod tests;
