//! 多帳號並行排程與容量：決定「現在能不能派、派給哪個帳號、不能派的話為什麼」（票 19；spec D-14、D-13）。
//!
//! 純邏輯、無 I/O。幾條貫穿全檔的規則：
//! - **容量預設**：每帳號最多 2 個並行 Run、全域最多 6 個；可用帳號數 ≥ 3 時保留 1 個全域名額給交接 successor，
//!   一般派工（`SlotKind::Normal`）佔不到這個名額，只有交接（`SlotKind::Handoff`）可以用。
//!   使用者可在兩組預設（`CapacityMode`）之間切換。
//! - **阻擋原因分開呈現**：不能派的原因分成容量、額度、授權、相依、狀態不明與帳號問題（`WaitCategory`），
//!   使用者看到的是該處理哪一類問題，而不是一句「暫時不能派」。
//! - **只信任傳進來的事實**：帳號核對結果、進行中的 Run 與額度樣本都由呼叫端（Store 在同一個交易中）載入，
//!   所以判定與寫入之間不會有別人插進來改變容量。
//! - **額度只做「停止派新工作」這一道門檻**（spec D-13 的 ≥75%）：沒有樣本（未觀測）不阻擋；
//!   遲滯（<60% 才恢復）、draining 與交接屬於額度觀測與交接的票。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::authorization::AdmissionDenied;
use crate::event::DomainEvent;
use crate::guard::GuardSource;
use crate::profile::{self, AccountProfile, ProfileStatus};
use crate::run::{self, Run, RunContext, RunError};
use crate::task::MissingCode;
use crate::Timestamp;

pub const SETTINGS_AGGREGATE: &str = "scheduler_settings";
pub const SETTINGS_ID: &str = "global";
pub const EVENT_SETTINGS_CHANGED: &str = "scheduler_settings_changed";

/// 可用帳號數達到這個數量才保留交接槽（spec D-14）。
pub const HANDOFF_RESERVE_MIN_ACCOUNTS: usize = 3;

/// 推估用量達到這個比例就不再派新 Run 給該帳號（spec D-13）。
pub const QUOTA_STOP_DISPATCH_RATIO: f64 = 0.75;

// ---------------------------------------------------------------- 容量設定

/// 並行容量的兩組預設（spec D-14）：預設是「保留交接容量」。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapacityMode {
    /// 每帳號 2、全域 6，可用帳號數 ≥ 3 時保留 1 個交接槽。
    #[default]
    ReserveHandoff,
    /// 每帳號 3、全域 8，不保留交接槽。
    HigherConcurrency,
}

/// 一組預設對應的容量數字。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapacityLimits {
    pub per_account: usize,
    pub global: usize,
    /// 設定上要保留的交接槽數；帳號太少時實際保留數是 0（`effective_reserve`）。
    pub handoff_reserve: usize,
}

impl CapacityLimits {
    /// 實際生效的保留數：可用帳號數不到 `HANDOFF_RESERVE_MIN_ACCOUNTS` 時不保留。
    pub fn effective_reserve(&self, usable_accounts: usize) -> usize {
        if usable_accounts >= HANDOFF_RESERVE_MIN_ACCOUNTS {
            self.handoff_reserve
        } else {
            0
        }
    }

    /// 一般派工最多能佔用的全域名額。
    pub fn normal_limit(&self, usable_accounts: usize) -> usize {
        self.global.saturating_sub(self.effective_reserve(usable_accounts))
    }
}

impl CapacityMode {
    pub const ALL: [CapacityMode; 2] = [CapacityMode::ReserveHandoff, CapacityMode::HigherConcurrency];

    pub fn as_str(self) -> &'static str {
        match self {
            CapacityMode::ReserveHandoff => "reserve_handoff",
            CapacityMode::HigherConcurrency => "higher_concurrency",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.as_str() == raw)
    }

    pub fn label(self) -> &'static str {
        match self {
            CapacityMode::ReserveHandoff => "保留交接容量",
            CapacityMode::HigherConcurrency => "較高並行",
        }
    }

    pub fn limits(self) -> CapacityLimits {
        match self {
            CapacityMode::ReserveHandoff => CapacityLimits { per_account: 2, global: 6, handoff_reserve: 1 },
            CapacityMode::HigherConcurrency => CapacityLimits { per_account: 3, global: 8, handoff_reserve: 0 },
        }
    }

    pub fn description(self) -> String {
        let limits = self.limits();
        let reserve = if limits.handoff_reserve == 0 {
            "不保留交接槽".to_string()
        } else {
            format!("可用帳號數達 {HANDOFF_RESERVE_MIN_ACCOUNTS} 個以上時保留 {} 個交接槽", limits.handoff_reserve)
        };
        format!("每個帳號最多 {} 個並行 Run，全域最多 {} 個；{reserve}", limits.per_account, limits.global)
    }
}

/// 目前的排程設定；從未改過時是預設值（`version` 為 0）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SchedulerSettings {
    pub mode: CapacityMode,
    /// 每次變更加一；事件的 `aggregate_version` 與它對應。
    pub version: i64,
    pub updated_at: Option<Timestamp>,
}

/// 切換容量預設。與目前相同時沒有任何變動（冪等）。
pub fn change_mode(
    current: &SchedulerSettings,
    mode: CapacityMode,
    now: Timestamp,
) -> Option<(SchedulerSettings, DomainEvent)> {
    if current.mode == mode {
        return None;
    }
    let next = SchedulerSettings { mode, version: current.version + 1, updated_at: Some(now) };
    let event = DomainEvent {
        aggregate_type: SETTINGS_AGGREGATE,
        aggregate_id: SETTINGS_ID.to_string(),
        aggregate_version: next.version,
        event_type: EVENT_SETTINGS_CHANGED,
        occurred_at: now,
        payload: serde_json::json!({ "from": current.mode, "to": mode }),
    };
    Some((next, event))
}

// ---------------------------------------------------------------- 等待原因

/// 不能派的原因類別：使用者看到的是「該處理哪一類問題」。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaitCategory {
    /// 並行名額已滿（每帳號、全域或交接保留），名額空出來會自動派。
    Capacity,
    /// 帳號的額度接近上限。
    Quota,
    /// Workspace 授權不足或尚未確認。
    Authorization,
    /// 相依的 Task 還沒完成。
    Dependency,
    /// 帳號的狀態還不明（尚未核對、無法確認身分）。
    StateUnknown,
    /// 帳號本身不可用（未登入、認證被覆蓋、身分不符、已封存）或根本沒有帳號。
    Account,
    /// 完全退出的停止意圖生效中，或 Workspace 在完全退出後等待「繼續工作」（票 25）。
    Lifecycle,
    /// 已啟用的 Guard 擋下（票 38）：依來源教訓修改契約，或在知識分頁停用該 Guard 後自動再試。
    Guard,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaitReason {
    pub category: WaitCategory,
    /// 穩定的機器可讀代碼，UI 與測試依它判斷。
    pub code: String,
    pub message: String,
    /// 原因屬於某個帳號時的 Profile 識別。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    /// 被 Guard 擋下時，該 Guard 與它的來源教訓。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guard: Option<GuardSource>,
}

fn reason(category: WaitCategory, code: &str, message: String, profile_id: Option<&str>) -> WaitReason {
    WaitReason { category, code: code.to_string(), message, profile_id: profile_id.map(str::to_string), guard: None }
}

/// Task 層級的等待原因（授權、相依、Guard）；其他派工錯誤（Task 不是 ready、已有寫入者、契約失效）不是「等一等就好」，
/// 回傳 `None` 讓呼叫端照原樣拒絕。
pub fn task_wait_reasons(error: &RunError) -> Option<Vec<WaitReason>> {
    let authorization = |code: &str, message: String| reason(WaitCategory::Authorization, code, message, None);
    match error {
        RunError::DependenciesUnfinished(_) => {
            Some(vec![reason(WaitCategory::Dependency, "dependencies_unfinished", error.to_string(), None)])
        }
        // 每個違反的 Guard 各一個原因，帶著來源教訓
        RunError::GuardViolated(violations) => Some(
            violations
                .iter()
                .map(|v| WaitReason {
                    guard: Some(v.source()),
                    ..reason(
                        WaitCategory::Guard,
                        "guard_violated",
                        format!("{}（來源教訓「{}」）", v.message, v.lesson_title),
                        None,
                    )
                })
                .collect(),
        ),
        RunError::AdmissionRejected(
            denied @ (AdmissionDenied::AuthorizationUnconfirmed(_) | AdmissionDenied::CapabilityNotGranted { .. }),
        ) => Some(vec![authorization(denied.code(), denied.to_string())]),
        RunError::AdmissionDenied(missing)
            if !missing.is_empty()
                && missing.iter().all(|m| {
                    matches!(m.code, MissingCode::AuthorizationUnconfirmed | MissingCode::CapabilityNotGranted)
                }) =>
        {
            Some(
                missing
                    .iter()
                    .map(|m| {
                        let code = match m.code {
                            MissingCode::AuthorizationUnconfirmed => "authorization_unconfirmed",
                            _ => "capability_not_granted",
                        };
                        authorization(code, m.message.clone())
                    })
                    .collect(),
            )
        }
        _ => None,
    }
}

// ---------------------------------------------------------------- 容量與選帳號

/// 名額的用途：一般派工佔不到交接保留槽。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotKind {
    Normal,
    Handoff,
}

/// 進行中的 Run：還沒結束（queued、starting、running，含已要求取消但程序還沒確認停止的）都佔用名額。
pub fn is_active(run: &Run) -> bool {
    !run.status.is_terminal()
}

/// 判定容量與選帳號需要的事實（全部由呼叫端在同一個交易中載入）。
pub struct Capacity<'a> {
    /// 全部 Profile（含已封存，依建立順序）。
    pub profiles: &'a [AccountProfile],
    pub settings: &'a SchedulerSettings,
    /// 全部 Workspace 中進行中的 Run。
    pub active_runs: &'a [Run],
    /// Profile 識別 → 已知的用量比例（0.0–1.0）；未觀測的帳號沒有項目，不當作 0。
    pub quota_used: &'a HashMap<String, f64>,
    /// 生命週期的阻擋（完全退出的停止意圖、Workspace 等待「繼續工作」，`lifecycle::dispatch_halt`）。
    /// 有值時任何名額（含交接）都不派，佇列重試與額度恢復也就不會啟動新的 Run。
    pub halt: Option<WaitReason>,
}

impl Capacity<'_> {
    fn running_of(&self, profile_id: &str) -> usize {
        self.active_runs.iter().filter(|r| r.profile_id == profile_id).count()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Placement {
    Place { profile_id: String },
    Wait(Vec<WaitReason>),
}

/// 可派工的（使用中且核對無誤的）帳號，依建立順序。
pub fn usable_profiles(profiles: &[AccountProfile]) -> Vec<&AccountProfile> {
    profiles
        .iter()
        .filter(|p| p.status == ProfileStatus::Active && profile::blockers(p, profiles).is_empty())
        .collect()
}

/// 把 Profile 的阻擋原因對應成呼叫端看得懂的類別：還沒核對、無法確認身分是「狀態不明」，其餘是帳號問題。
fn profile_blocker_category(code: &str) -> WaitCategory {
    match code {
        "profile_unverified" | "identity_unconfirmed" => WaitCategory::StateUnknown,
        _ => WaitCategory::Account,
    }
}

/// 一個名額要派給哪個帳號；不能派時列出全部適用的原因。
///
/// 選帳號：在「核對無誤、額度未達門檻、帳號名額未滿」的帳號中，挑進行中 Run 最少的，同數時取較早建立的。
/// 這讓兩個帳號同時有空時，第一個 Task 給第一個帳號、第二個 Task 給第二個帳號。
pub fn place(capacity: &Capacity<'_>, kind: SlotKind) -> Placement {
    if let Some(halt) = &capacity.halt {
        return Placement::Wait(vec![halt.clone()]);
    }
    let limits = capacity.settings.mode.limits();
    let active: Vec<&AccountProfile> = capacity.profiles.iter().filter(|p| p.status == ProfileStatus::Active).collect();
    if active.is_empty() {
        return Placement::Wait(vec![reason(
            WaitCategory::Account,
            "no_account",
            "還沒有任何帳號 Profile；請先到帳號頁新增並完成登入".to_string(),
            None,
        )]);
    }

    let mut reasons = Vec::new();
    let mut usable = Vec::new();
    for profile in &active {
        let blockers = profile::blockers(profile, capacity.profiles);
        if blockers.is_empty() {
            usable.push(*profile);
        }
        for blocker in blockers {
            reasons.push(reason(
                profile_blocker_category(blocker.code()),
                blocker.code(),
                format!("帳號「{}」：{}", profile.name, blocker.message()),
                Some(&profile.id),
            ));
        }
    }
    if usable.is_empty() {
        return Placement::Wait(reasons);
    }

    // 全域名額：一般派工扣掉保留的交接槽
    let total = capacity.active_runs.len();
    let allowed = match kind {
        SlotKind::Normal => limits.normal_limit(usable.len()),
        SlotKind::Handoff => limits.global,
    };
    let global_full = total >= allowed;
    if global_full {
        reasons.insert(0, if total >= limits.global {
            reason(
                WaitCategory::Capacity,
                "global_capacity_full",
                format!("全域並行已達上限（{total}/{}）", limits.global),
                None,
            )
        } else {
            reason(
                WaitCategory::Capacity,
                "handoff_slot_reserved",
                format!(
                    "全域 {} 個名額中保留 {} 個給交接，一般派工最多 {allowed} 個（目前 {total} 個）",
                    limits.global,
                    limits.effective_reserve(usable.len())
                ),
                None,
            )
        });
    }

    let mut candidates = Vec::new();
    for profile in usable {
        if let Some(used) = capacity.quota_used.get(&profile.id).filter(|u| **u >= QUOTA_STOP_DISPATCH_RATIO) {
            reasons.push(reason(
                WaitCategory::Quota,
                "quota_near_limit",
                format!(
                    "帳號「{}」的用量約 {:.0}%，已達 {:.0}% 的停止派工門檻",
                    profile.name,
                    used * 100.0,
                    QUOTA_STOP_DISPATCH_RATIO * 100.0
                ),
                Some(&profile.id),
            ));
            continue;
        }
        let running = capacity.running_of(&profile.id);
        if running >= limits.per_account {
            reasons.push(reason(
                WaitCategory::Capacity,
                "account_capacity_full",
                format!("帳號「{}」已達並行上限（{running}/{}）", profile.name, limits.per_account),
                Some(&profile.id),
            ));
            continue;
        }
        candidates.push((running, profile));
    }

    if global_full {
        return Placement::Wait(reasons);
    }
    // `min_by_key` 在同數時取最先出現的，也就是較早建立的帳號
    match candidates.into_iter().min_by_key(|(running, _)| *running) {
        Some((_, profile)) => Placement::Place { profile_id: profile.id.clone() },
        None => Placement::Wait(reasons),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Evaluation {
    Dispatch { profile_id: String },
    Wait(Vec<WaitReason>),
}

/// 一個 ready 的 Task 現在能不能派：先過派工的前置檢查（授權、相依），再看帳號與容量。
/// Task 不是 ready、已有寫入者或契約失效這類不是「等一等就好」的錯誤原樣回傳。
pub fn evaluate(
    ctx: &RunContext<'_>,
    capacity: &Capacity<'_>,
    task_id: &str,
    kind: SlotKind,
) -> Result<Evaluation, RunError> {
    if let Err(error) = run::check_dispatch(ctx, task_id) {
        return match task_wait_reasons(&error) {
            Some(mut reasons) => {
                // 生命週期的阻擋也一併列出，使用者才知道等授權或相依之外還要「繼續工作」
                if let Some(halt) = &capacity.halt {
                    reasons.insert(0, halt.clone());
                }
                Ok(Evaluation::Wait(reasons))
            }
            None => Err(error),
        };
    }
    Ok(match place(capacity, kind) {
        Placement::Place { profile_id } => Evaluation::Dispatch { profile_id },
        Placement::Wait(reasons) => Evaluation::Wait(reasons),
    })
}

#[cfg(test)]
mod tests;
