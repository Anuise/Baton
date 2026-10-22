//! 修正與重試預算（票 36，spec D-16）：每個 Task 三種互不影響的計數。
//!
//! 純邏輯、無 I/O。幾條貫穿全檔的規則：
//! - **三種計數分開**：實作失敗修正 3 次（Worker 以失敗結束、確定性檢查未通過）、驗證不完整重驗 2 次（檢查無法取得
//!   證據）、基礎設施重試 5 次（啟動失敗、程序意外消失；指數退避，上限 10 分鐘）。一種用完不影響其他兩種。
//! - **不計入任何一種**：額度交接（含突然限流而結束的 Run）、恢復嘗試（Runtime 重建後找不到程序、完全退出與還原的中斷）
//!   與使用者取消。哪一種結束計入哪一種預算，由 `run` 與 `verification` 模組在 Run 結束或驗證結論的轉換中決定。
//! - **預算內**照原本的轉換繼續（基礎設施重試另外排回派工佇列並等待退避期限）；**用盡**時 Task 進入 `blocked`，
//!   並建立「預算用盡後的下一步」裁定請求（spec D-15）。使用者核准才重設用盡的那一種計數、讓 Task 繼續。

use serde::{Deserialize, Serialize};

use crate::event::DomainEvent;
use crate::task::Task;
use crate::Timestamp;

pub const BUDGET_AGGREGATE: &str = "task_budget";
pub const EVENT_BUDGET_CHARGED: &str = "task_budget_charged";
pub const EVENT_BUDGET_EXHAUSTED: &str = "task_budget_exhausted";
pub const EVENT_BUDGET_RESET: &str = "task_budget_reset";

/// 基礎設施重試的退避：第一次 1 分鐘，之後每次加倍。
pub const BACKOFF_BASE_MS: i64 = 60_000;
/// 退避上限 10 分鐘（spec D-16）。
pub const BACKOFF_MAX_MS: i64 = 10 * 60_000;

/// 預算的種類；宣告順序就是顯示順序。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetKind {
    /// 實作失敗修正。
    Implementation,
    /// 驗證不完整重驗。
    Verification,
    /// 基礎設施重試。
    Infrastructure,
}

impl BudgetKind {
    pub const ALL: [BudgetKind; 3] = [BudgetKind::Implementation, BudgetKind::Verification, BudgetKind::Infrastructure];

    pub fn as_str(self) -> &'static str {
        match self {
            BudgetKind::Implementation => "implementation",
            BudgetKind::Verification => "verification",
            BudgetKind::Infrastructure => "infrastructure",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == raw)
    }

    pub fn label(self) -> &'static str {
        match self {
            BudgetKind::Implementation => "實作失敗修正",
            BudgetKind::Verification => "驗證不完整重驗",
            BudgetKind::Infrastructure => "基礎設施重試",
        }
    }

    /// 每一輪可以用幾次（spec D-16）。
    pub fn limit(self) -> u32 {
        match self {
            BudgetKind::Implementation => 3,
            BudgetKind::Verification => 2,
            BudgetKind::Infrastructure => 5,
        }
    }
}

/// 第 `retry` 次基礎設施重試（從 1 起）要等多久：1、2、4、8 分鐘，之後都是 10 分鐘。
pub fn backoff_millis(retry: u32) -> i64 {
    let doublings = retry.saturating_sub(1).min(16);
    (BACKOFF_BASE_MS << doublings).min(BACKOFF_MAX_MS)
}

/// 一個 Task 的預算計數；從未計入過任何一次的 Task 沒有紀錄（視同全為 0）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskBudget {
    pub task_id: String,
    pub workspace_id: String,
    /// 已用的實作失敗修正次數。
    pub implementation: u32,
    /// 已用的驗證不完整重驗次數。
    pub verification: u32,
    /// 已用的基礎設施重試次數。
    pub infrastructure: u32,
    /// 基礎設施重試的退避期限：這之前不會派工。
    pub retry_not_before: Option<Timestamp>,
    /// 用盡而等待裁定的種類，與當時建立的裁定請求。
    pub exhausted: Option<BudgetKind>,
    pub request_id: Option<String>,
    /// 每個事件加一；事件的 `aggregate_version` 與它對應。
    pub version: i64,
    pub updated_at: Option<Timestamp>,
}

impl TaskBudget {
    /// 全部為 0 的預算。
    pub fn fresh(task: &Task) -> Self {
        Self {
            task_id: task.id.clone(),
            workspace_id: task.workspace_id.clone(),
            implementation: 0,
            verification: 0,
            infrastructure: 0,
            retry_not_before: None,
            exhausted: None,
            request_id: None,
            version: 0,
            updated_at: None,
        }
    }

    /// 這個 Task 目前的預算（沒有紀錄時全為 0）。
    pub fn of(budgets: &[TaskBudget], task: &Task) -> Self {
        budgets.iter().find(|b| b.task_id == task.id).cloned().unwrap_or_else(|| Self::fresh(task))
    }

    pub fn used(&self, kind: BudgetKind) -> u32 {
        match kind {
            BudgetKind::Implementation => self.implementation,
            BudgetKind::Verification => self.verification,
            BudgetKind::Infrastructure => self.infrastructure,
        }
    }

    fn used_mut(&mut self, kind: BudgetKind) -> &mut u32 {
        match kind {
            BudgetKind::Implementation => &mut self.implementation,
            BudgetKind::Verification => &mut self.verification,
            BudgetKind::Infrastructure => &mut self.infrastructure,
        }
    }

    /// 退避期限還沒到。
    pub fn in_backoff(&self, now: Timestamp) -> bool {
        self.retry_not_before.is_some_and(|t| t > now)
    }

    fn push(&mut self, event_type: &'static str, now: Timestamp, payload: serde_json::Value) -> DomainEvent {
        self.version += 1;
        self.updated_at = Some(now);
        DomainEvent {
            aggregate_type: BUDGET_AGGREGATE,
            aggregate_id: self.task_id.clone(),
            aggregate_version: self.version,
            event_type,
            occurred_at: now,
            payload,
        }
    }
}

/// 計入一次之後的結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChargeOutcome {
    /// 還在預算內：Task 照原本的轉換繼續。基礎設施重試要排回派工佇列，並等到退避期限。
    Retry { requeue: bool },
    /// 預算用盡：Task 進入 `blocked`，並以 `request_id` 建立裁定請求。
    Exhausted { request_id: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Charged {
    pub budget: TaskBudget,
    pub outcome: ChargeOutcome,
    pub event: DomainEvent,
}

/// 這次失敗或不完整的驗證要計入 `kind`。`source` 是造成它的 Run 或驗證紀錄，`request_id` 是用盡時要建立的裁定請求。
///
/// 預算內：計數加一（基礎設施重試另外設定退避期限）。已經用了 `limit` 次：不再加，標記為用盡。
pub fn charge(
    current: &TaskBudget,
    kind: BudgetKind,
    source: &str,
    reason: &str,
    request_id: String,
    now: Timestamp,
) -> Charged {
    let mut budget = current.clone();
    let limit = kind.limit();
    let used = budget.used(kind);
    if used < limit {
        *budget.used_mut(kind) = used + 1;
        let requeue = kind == BudgetKind::Infrastructure;
        if requeue {
            budget.retry_not_before = Some(now.plus_millis(backoff_millis(used + 1)));
        }
        let event = budget.push(
            EVENT_BUDGET_CHARGED,
            now,
            serde_json::json!({
                "kind": kind,
                "used": used + 1,
                "limit": limit,
                "source": source,
                "reason": reason,
                "retry_not_before": budget.retry_not_before.filter(|_| requeue),
            }),
        );
        return Charged { budget, outcome: ChargeOutcome::Retry { requeue }, event };
    }
    budget.exhausted = Some(kind);
    budget.request_id = Some(request_id.clone());
    let event = budget.push(
        EVENT_BUDGET_EXHAUSTED,
        now,
        serde_json::json!({ "kind": kind, "limit": limit, "source": source, "reason": reason, "request_id": request_id }),
    );
    Charged { budget, outcome: ChargeOutcome::Exhausted { request_id }, event }
}

/// 使用者核准了用盡時建立的裁定請求 `request_id`：重設用盡的那一種計數（另外兩種不變）。
/// 這個請求不是目前用盡的那一個（例如之後又用盡而建立了新的請求）時回傳 `None`。
pub fn reset(current: &TaskBudget, request_id: &str, now: Timestamp) -> Option<(TaskBudget, BudgetKind, DomainEvent)> {
    let kind = current.exhausted.filter(|_| current.request_id.as_deref() == Some(request_id))?;
    let mut budget = current.clone();
    *budget.used_mut(kind) = 0;
    budget.exhausted = None;
    budget.request_id = None;
    if kind == BudgetKind::Infrastructure {
        budget.retry_not_before = None;
    }
    let event = budget.push(EVENT_BUDGET_RESET, now, serde_json::json!({ "kind": kind, "request_id": request_id }));
    Some((budget, kind, event))
}

// ---------------------------------------------------------------- 檢視

/// 一種計數。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BudgetCounter {
    pub kind: BudgetKind,
    pub label: String,
    pub used: u32,
    pub limit: u32,
}

/// Task 詳情顯示的預算：三種計數（依 `BudgetKind::ALL` 的順序）、退避期限與用盡狀態。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BudgetView {
    pub task_id: String,
    pub counters: Vec<BudgetCounter>,
    pub retry_not_before: Option<Timestamp>,
    pub exhausted: Option<BudgetKind>,
    pub decision_request_id: Option<String>,
}

pub fn view(budgets: &[TaskBudget], task: &Task) -> BudgetView {
    let budget = TaskBudget::of(budgets, task);
    BudgetView {
        task_id: task.id.clone(),
        counters: BudgetKind::ALL
            .into_iter()
            .map(|kind| BudgetCounter { kind, label: kind.label().to_string(), used: budget.used(kind), limit: kind.limit() })
            .collect(),
        retry_not_before: budget.retry_not_before,
        exhausted: budget.exhausted,
        decision_request_id: budget.request_id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::TaskStatus;
    use crate::workdir::TaskKind;

    const NOW: Timestamp = Timestamp::from_millis(1_000_000);

    fn task() -> Task {
        Task {
            id: "t1".into(),
            workspace_id: "w1".into(),
            goal_id: "g1".into(),
            title: "比較".into(),
            kind: TaskKind::Research,
            status: TaskStatus::Running,
            version: 3,
            contract_version: 1,
            created_at: NOW,
            updated_at: NOW,
        }
    }

    fn charge_times(kind: BudgetKind, times: u32) -> (TaskBudget, Vec<ChargeOutcome>) {
        let mut budget = TaskBudget::fresh(&task());
        let mut outcomes = Vec::new();
        for i in 0..times {
            let charged = charge(&budget, kind, &format!("r{i}"), "失敗", format!("dr{i}"), NOW);
            budget = charged.budget;
            outcomes.push(charged.outcome);
        }
        (budget, outcomes)
    }

    #[test]
    fn the_limits_follow_spec_d16() {
        assert_eq!(BudgetKind::Implementation.limit(), 3);
        assert_eq!(BudgetKind::Verification.limit(), 2);
        assert_eq!(BudgetKind::Infrastructure.limit(), 5);
    }

    #[test]
    fn each_kind_is_exhausted_only_after_its_own_limit() {
        for kind in BudgetKind::ALL {
            let (budget, outcomes) = charge_times(kind, kind.limit() + 1);
            let limit = kind.limit() as usize;
            assert!(outcomes[..limit].iter().all(|o| matches!(o, ChargeOutcome::Retry { .. })), "{kind:?}：{outcomes:?}");
            assert_eq!(outcomes[limit], ChargeOutcome::Exhausted { request_id: format!("dr{limit}") });
            assert_eq!(budget.used(kind), kind.limit(), "用盡時計數停在上限");
            assert_eq!(budget.exhausted, Some(kind));
            for other in BudgetKind::ALL.into_iter().filter(|o| *o != kind) {
                assert_eq!(budget.used(other), 0, "{kind:?} 不影響 {other:?}");
            }
        }
    }

    #[test]
    fn only_infrastructure_retries_requeue_with_exponential_backoff_capped_at_ten_minutes() {
        assert_eq!((1..=6).map(backoff_millis).collect::<Vec<_>>(), [60_000, 120_000, 240_000, 480_000, 600_000, 600_000]);
        let (_, outcomes) = charge_times(BudgetKind::Implementation, 1);
        assert_eq!(outcomes[0], ChargeOutcome::Retry { requeue: false });
        let mut budget = TaskBudget::fresh(&task());
        for retry in 1..=5 {
            let charged = charge(&budget, BudgetKind::Infrastructure, "r", "啟動失敗", "dr".into(), NOW);
            assert_eq!(charged.outcome, ChargeOutcome::Retry { requeue: true });
            assert_eq!(charged.budget.retry_not_before, Some(NOW.plus_millis(backoff_millis(retry))));
            budget = charged.budget;
        }
        assert!(budget.in_backoff(NOW) && !budget.in_backoff(NOW.plus_millis(BACKOFF_MAX_MS)));
    }

    #[test]
    fn approving_the_exhaustion_request_resets_only_that_kind() {
        let (mut budget, _) = charge_times(BudgetKind::Implementation, 4);
        budget = charge(&budget, BudgetKind::Infrastructure, "r", "啟動失敗", "x".into(), NOW).budget;
        assert!(reset(&budget, "另一個請求", NOW).is_none());
        let (after, kind, event) = reset(&budget, "dr3", NOW).expect("用盡時的請求可以重設");
        assert_eq!(kind, BudgetKind::Implementation);
        assert_eq!((after.implementation, after.infrastructure, after.exhausted), (0, 1, None));
        assert_eq!(event.event_type, EVENT_BUDGET_RESET);
        assert!(reset(&after, "dr3", NOW).is_none(), "重設之後同一個請求不能再用");
    }

    #[test]
    fn the_view_lists_three_counters_in_order() {
        let (budget, _) = charge_times(BudgetKind::Verification, 1);
        let view = view(&[budget], &task());
        let summary: Vec<_> = view.counters.iter().map(|c| (c.kind, c.used, c.limit)).collect();
        assert_eq!(
            summary,
            [(BudgetKind::Implementation, 0, 3), (BudgetKind::Verification, 1, 2), (BudgetKind::Infrastructure, 0, 5)]
        );
        assert_eq!(view.exhausted, None);
    }
}
