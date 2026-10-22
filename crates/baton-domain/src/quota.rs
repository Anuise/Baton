//! 額度觀測（spec D-12）：「已觀測」與「推估」兩個來源並存並標明可信度，判斷時取較保守者。
//!
//! - **已觀測**：Adapter 從 CLI 串流與錯誤中解析到的限流訊號（狀態、用量比例、重置時間）。
//! - **推估**：Runtime 依每個 Run 結果訊息回報的用量，以滾動五小時與七日視窗累計，除以使用者為 Profile
//!   設定的方案上限；沒有設定上限就算不出比例。
//! - **外部觀測器**：QuotaObserver Port 每 5 分鐘輪詢一次（spec D-31）的樣本；正式版目前沒有實作，
//!   若日後 CLI 提供直接的額度查詢，就由它提供。
//!
//! 觀測不到的值一律是 `None`（未知），不填零。推估只看得到本機 Baton 派出的 Run，其他裝置或互動式使用的
//! 用量看不到，所以只要缺少來自 CLI 的帳號層級用量，就明確標示「觀測不完整」。
//!
//! 本模組是純邏輯：輸入「最近的觀測 + 現在時間」，輸出判斷結果；持久化由 Store 負責。

use serde::{Deserialize, Serialize};

use crate::event::DomainEvent;
use crate::scheduler::QUOTA_STOP_DISPATCH_RATIO;
use crate::Timestamp;

/// 外部觀測器的輪詢間隔與推估的重新計算間隔（spec D-31：額度輪詢 5 分鐘）。
pub const QUOTA_POLL_INTERVAL_MILLIS: i64 = 5 * 60 * 1000;

/// 觀測與用量紀錄保留多久：比最長的視窗（七日）多一天，超過的在輪詢時刪除。
pub const QUOTA_RETENTION_MILLIS: i64 = 8 * 24 * 60 * 60 * 1000;

/// 原始資料最多保存的字元數（CLI 的一行事件可能很長）。
pub const QUOTA_RAW_MAX_CHARS: usize = 2000;

const HOUR_MILLIS: i64 = 60 * 60 * 1000;

/// 用量視窗。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaWindow {
    FiveHour,
    SevenDay,
}

impl QuotaWindow {
    pub const ALL: [QuotaWindow; 2] = [QuotaWindow::FiveHour, QuotaWindow::SevenDay];

    pub fn duration_millis(self) -> i64 {
        match self {
            QuotaWindow::FiveHour => 5 * HOUR_MILLIS,
            QuotaWindow::SevenDay => 7 * 24 * HOUR_MILLIS,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            QuotaWindow::FiveHour => "five_hour",
            QuotaWindow::SevenDay => "seven_day",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|w| w.as_str() == raw)
    }

    pub fn label(self) -> &'static str {
        match self {
            QuotaWindow::FiveHour => "五小時",
            QuotaWindow::SevenDay => "七日",
        }
    }
}

/// 額度資料來源（spec D-12）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaSource {
    Observed,
    Estimated,
}

impl QuotaSource {
    pub fn as_str(self) -> &'static str {
        match self {
            QuotaSource::Observed => "observed",
            QuotaSource::Estimated => "estimated",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        [QuotaSource::Observed, QuotaSource::Estimated].into_iter().find(|s| s.as_str() == raw)
    }
}

/// 可信度；觀測不到時為 Unknown，不填零。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    Observed,
    Estimated,
    Unknown,
}

impl Confidence {
    pub fn as_str(self) -> &'static str {
        match self {
            Confidence::Observed => "observed",
            Confidence::Estimated => "estimated",
            Confidence::Unknown => "unknown",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        [Confidence::Observed, Confidence::Estimated, Confidence::Unknown].into_iter().find(|c| c.as_str() == raw)
    }
}

/// CLI 回報的限流狀態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LimitStatus {
    /// 未受限。
    Allowed,
    /// 接近上限的警告。
    Warning,
    /// 已達上限，要等重置。
    Exhausted,
}

impl LimitStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            LimitStatus::Allowed => "allowed",
            LimitStatus::Warning => "warning",
            LimitStatus::Exhausted => "exhausted",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        [LimitStatus::Allowed, LimitStatus::Warning, LimitStatus::Exhausted].into_iter().find(|s| s.as_str() == raw)
    }
}

/// 一筆觀測的原始資料從哪裡來。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationOrigin {
    /// Adapter 從 CLI 串流解析到的限流訊號。
    Adapter,
    /// Runtime 依本機 Run 的用量推估。
    Estimate,
    /// QuotaObserver Port 的輪詢樣本。
    Observer,
}

impl ObservationOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            ObservationOrigin::Adapter => "adapter",
            ObservationOrigin::Estimate => "estimate",
            ObservationOrigin::Observer => "observer",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        [ObservationOrigin::Adapter, ObservationOrigin::Estimate, ObservationOrigin::Observer]
            .into_iter()
            .find(|o| o.as_str() == raw)
    }
}

/// 一筆額度觀測：保存來源、時間、可信度與原始資料參照。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuotaObservation {
    pub profile_id: String,
    pub origin: ObservationOrigin,
    pub source: QuotaSource,
    /// 適用的視窗；CLI 沒有說明是哪個視窗、或外部觀測器的帳號層級樣本為 `None`。
    pub window: Option<QuotaWindow>,
    /// CLI 回報的限流狀態（只有已觀測的訊號有）。
    pub status: Option<LimitStatus>,
    /// 已用比例（1.0 = 達到上限，推估可能超過 1.0）；未知為 `None`。
    pub used: Option<f64>,
    /// 推估：視窗內本機 Run 累計的 token 數。
    pub tokens: Option<u64>,
    /// 推估：計算時使用的方案上限（token）；沒有設定為 `None`。
    pub limit_tokens: Option<u64>,
    pub reset_at: Option<Timestamp>,
    pub observed_at: Timestamp,
    pub confidence: Confidence,
    /// 原始資料參照，例如 `run:<run_id>`、`quota_usage:seq=3..9`、`quota_observer`。
    pub raw_ref: String,
    /// 原始資料（CLI 事件原文或錯誤訊息，截斷到 `QUOTA_RAW_MAX_CHARS`）；不在 API 回傳。
    #[serde(skip_serializing, default)]
    pub raw: Option<String>,
}

impl QuotaObservation {
    /// 判斷用的用量：已達上限視為 100%；有警告但沒有數字時，至少視為停止派工門檻（保守）；其他依回報的數字。
    pub fn judged_used(&self) -> Option<f64> {
        match self.status {
            Some(LimitStatus::Exhausted) => Some(self.used.map_or(1.0, |u| u.max(1.0))),
            Some(LimitStatus::Warning) => Some(self.used.unwrap_or(QUOTA_STOP_DISPATCH_RATIO)),
            _ => self.used,
        }
    }

    /// 判斷時歸屬的視窗：沒有說明視窗的 CLI 訊號歸到五小時視窗。
    fn effective_window(&self) -> QuotaWindow {
        self.window.unwrap_or(QuotaWindow::FiveHour)
    }

    /// 這筆觀測現在還算數嗎。CLI 訊號在重置時間之後失效；沒有重置時間時，最多維持一個視窗長度。
    /// 推估與外部觀測器只取最新一筆（每次輪詢都會更新），所以永遠算數。
    pub fn is_current(&self, now: Timestamp) -> bool {
        match self.origin {
            ObservationOrigin::Adapter => {
                let until = self
                    .reset_at
                    .unwrap_or_else(|| self.observed_at.plus_millis(self.effective_window().duration_millis()));
                now < until
            }
            ObservationOrigin::Estimate | ObservationOrigin::Observer => true,
        }
    }
}

/// 使用者為 Profile 設定的方案上限（推估的分母），單位為 token；未設定為 `None`。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanLimits {
    pub five_hour_tokens: Option<u64>,
    pub seven_day_tokens: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlanLimitError {
    #[error("方案上限必須是正整數（token）；不知道就留空")]
    NotPositive,
}

impl PlanLimits {
    pub fn for_window(&self, window: QuotaWindow) -> Option<u64> {
        match window {
            QuotaWindow::FiveHour => self.five_hour_tokens,
            QuotaWindow::SevenDay => self.seven_day_tokens,
        }
    }

    pub fn validate(&self) -> Result<(), PlanLimitError> {
        if [self.five_hour_tokens, self.seven_day_tokens].contains(&Some(0)) {
            return Err(PlanLimitError::NotPositive);
        }
        Ok(())
    }
}

/// 變更一個 Profile 的方案上限：檢查後回傳 `quota_plan_limits_changed` 事件（版本為 `version + 1`）；
/// 與目前相同時回傳 `None`，不寫入。
pub fn change_plan_limits(
    profile_id: &str,
    current: &PlanLimits,
    version: i64,
    next: PlanLimits,
    now: Timestamp,
) -> Result<Option<DomainEvent>, PlanLimitError> {
    next.validate()?;
    if *current == next {
        return Ok(None);
    }
    Ok(Some(DomainEvent {
        aggregate_type: "quota_plan_limit",
        aggregate_id: profile_id.to_string(),
        aggregate_version: version + 1,
        event_type: "quota_plan_limits_changed",
        occurred_at: now,
        payload: serde_json::json!({
            "five_hour_tokens": next.five_hour_tokens,
            "seven_day_tokens": next.seven_day_tokens,
        }),
    }))
}

/// 一個 Run 的結果訊息回報的用量。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageRecord {
    pub seq: i64,
    pub tokens: u64,
    pub reported_at: Timestamp,
}

/// Adapter 解析到的限流訊號；觀測不到的欄位為 `None`。
#[derive(Debug, Clone, PartialEq)]
pub struct RateLimitSignal {
    pub window: Option<QuotaWindow>,
    pub status: LimitStatus,
    pub used: Option<f64>,
    pub reset_at: Option<Timestamp>,
    pub raw: String,
}

/// 把原始資料截斷到可保存的長度。
pub fn truncate_raw(raw: &str) -> String {
    raw.chars().take(QUOTA_RAW_MAX_CHARS).collect()
}

/// CLI 限流訊號 → 已觀測的觀測紀錄。
pub fn observe_signal(profile_id: &str, run_id: &str, signal: &RateLimitSignal, now: Timestamp) -> QuotaObservation {
    let known = signal.used.is_some() || signal.status == LimitStatus::Exhausted;
    QuotaObservation {
        profile_id: profile_id.to_string(),
        origin: ObservationOrigin::Adapter,
        source: QuotaSource::Observed,
        window: signal.window,
        status: Some(signal.status),
        used: signal.used,
        tokens: None,
        limit_tokens: None,
        reset_at: signal.reset_at,
        observed_at: now,
        confidence: if known { Confidence::Observed } else { Confidence::Unknown },
        raw_ref: format!("run:{run_id}"),
        raw: Some(truncate_raw(&signal.raw)),
    }
}

/// 依本機 Run 的用量推估一個視窗：視窗內累計 token ÷ 方案上限。沒有方案上限就算不出比例（未知）。
/// 重置時間是視窗內最早一筆用量滑出視窗的時間；視窗內沒有用量就沒有重置時間。
pub fn estimate(
    profile_id: &str,
    window: QuotaWindow,
    usage: &[UsageRecord],
    limits: &PlanLimits,
    now: Timestamp,
) -> QuotaObservation {
    let since = now.plus_millis(-window.duration_millis());
    let counted: Vec<&UsageRecord> = usage.iter().filter(|u| u.reported_at > since && u.reported_at <= now).collect();
    let tokens: u64 = counted.iter().map(|u| u.tokens).sum();
    let limit = limits.for_window(window);
    let used = limit.map(|limit| tokens as f64 / limit as f64);
    let raw_ref = match (counted.iter().map(|u| u.seq).min(), counted.iter().map(|u| u.seq).max()) {
        (Some(first), Some(last)) => format!("quota_usage:seq={first}..{last}"),
        _ => "quota_usage:none".to_string(),
    };
    QuotaObservation {
        profile_id: profile_id.to_string(),
        origin: ObservationOrigin::Estimate,
        source: QuotaSource::Estimated,
        window: Some(window),
        status: None,
        used,
        tokens: Some(tokens),
        limit_tokens: limit,
        reset_at: counted.iter().map(|u| u.reported_at).min().map(|t| t.plus_millis(window.duration_millis())),
        observed_at: now,
        confidence: if used.is_some() { Confidence::Estimated } else { Confidence::Unknown },
        raw_ref,
        raw: None,
    }
}

/// 一個視窗的判斷：兩個來源各自的最新觀測，與取較保守者後的結果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindowQuota {
    pub window: QuotaWindow,
    pub label: String,
    /// 判斷採用的觀測（用量最高者）；兩個來源都不知道用量時為 `None`。
    pub effective: Option<QuotaObservation>,
    pub observed: Option<QuotaObservation>,
    pub estimated: Option<QuotaObservation>,
}

/// 一個帳號的額度判斷。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuotaAssessment {
    /// 以較保守者（較高者）判斷的用量比例；全部來源都不知道時為 `None`（顯示「未知」，不是 0）。
    pub used: Option<f64>,
    /// 判斷採用的來源。
    pub source: Option<QuotaSource>,
    pub confidence: Confidence,
    pub reset_at: Option<Timestamp>,
    pub observed_at: Option<Timestamp>,
    /// 是否看得到帳號的完整用量（含其他裝置）；只有來自 CLI 或外部觀測器的帳號層級用量才算完整。
    pub complete: bool,
    pub incomplete_reason: Option<String>,
    pub windows: Vec<WindowQuota>,
    /// 外部觀測器（QuotaObserver Port）最近一次的樣本。
    pub external: Option<QuotaObservation>,
}

/// 兩筆觀測中較保守者：用量較高者；同值時偏好已觀測（較可信）。
fn more_conservative<'a>(a: Option<&'a QuotaObservation>, b: &'a QuotaObservation) -> &'a QuotaObservation {
    match a {
        None => b,
        Some(a) => {
            let (ua, ub) = (a.judged_used().unwrap_or(f64::MIN), b.judged_used().unwrap_or(f64::MIN));
            if ub > ua || (ub == ua && b.source == QuotaSource::Observed && a.source != QuotaSource::Observed) {
                b
            } else {
                a
            }
        }
    }
}

/// 判斷一個帳號目前的額度。`latest` 是每個（來源、視窗）最新的一筆觀測，順序不拘。
pub fn assess(latest: &[QuotaObservation], now: Timestamp) -> QuotaAssessment {
    let newest = |origin: ObservationOrigin, window: Option<QuotaWindow>| {
        latest
            .iter()
            .filter(|o| o.origin == origin && o.is_current(now))
            .filter(|o| window.is_none_or(|w| o.effective_window() == w))
            .max_by_key(|o| o.observed_at)
            .cloned()
    };

    let windows: Vec<WindowQuota> = QuotaWindow::ALL
        .into_iter()
        .map(|window| {
            let observed = newest(ObservationOrigin::Adapter, Some(window));
            let estimated = newest(ObservationOrigin::Estimate, Some(window));
            let effective = [observed.as_ref(), estimated.as_ref()]
                .into_iter()
                .flatten()
                .filter(|o| o.judged_used().is_some())
                .fold(None, |best, o| Some(more_conservative(best, o)))
                .cloned();
            WindowQuota { window, label: window.label().to_string(), effective, observed, estimated }
        })
        .collect();
    let external = newest(ObservationOrigin::Observer, None);

    let chosen = windows
        .iter()
        .filter_map(|w| w.effective.as_ref())
        .chain(external.iter().filter(|o| o.judged_used().is_some()))
        .fold(None, |best, o| Some(more_conservative(best, o)));

    let external_complete = external.as_ref().is_some_and(|o| o.source == QuotaSource::Observed && o.used.is_some());
    let missing: Vec<&str> = windows
        .iter()
        .filter(|w| w.observed.as_ref().is_none_or(|o| o.judged_used().is_none()))
        .map(|w| w.window.label())
        .collect();
    let complete = external_complete || missing.is_empty();
    let incomplete_reason = (!complete).then(|| {
        format!(
            "觀測不完整：{}視窗沒有來自 Claude Code 的已觀測用量；推估只計入這台電腦上 Baton 派出的 Run，\
             其他裝置與互動式使用的用量看不到",
            missing.join("、")
        )
    });

    QuotaAssessment {
        used: chosen.and_then(QuotaObservation::judged_used),
        source: chosen.map(|o| o.source),
        confidence: chosen.map_or(Confidence::Unknown, |o| o.confidence),
        reset_at: chosen.and_then(|o| o.reset_at),
        observed_at: chosen.map(|o| o.observed_at),
        complete,
        incomplete_reason,
        windows,
        external,
    }
}

#[cfg(test)]
mod tests;
