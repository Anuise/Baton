//! 觀測指標（票 41，spec 使用者故事 93）：全部只由時間線事件（`timeline::TimelineEntry`）計算。
//!
//! 純函式、無 I/O：同一串事件永遠得到同一組指標，所以任何人拿 `GET …/timeline` 的事件重新計算，都會得到
//! `GET …/metrics` 顯示的數字。沒有資料時比率與平均是 `None`（顯示「無資料」），不填零。

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::{Deserialize, Serialize};

use crate::timeline::{TimelineCategory, TimelineEntry};
use crate::{decision, external_operation, guard, handoff, quota, recovery, task};

/// 分子／分母與比率；分母為 0 時比率是 `None`。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Ratio {
    pub numerator: u64,
    pub denominator: u64,
    pub rate: Option<f64>,
}

impl Ratio {
    fn of(numerator: u64, denominator: u64) -> Self {
        Self { numerator, denominator, rate: (denominator > 0).then(|| numerator as f64 / denominator as f64) }
    }
}

/// 一組耗時（毫秒）的統計；`pending` 是還沒結束、不計入統計的數量。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DurationStats {
    pub count: u64,
    pub pending: u64,
    pub average_ms: Option<i64>,
    pub max_ms: Option<i64>,
}

impl DurationStats {
    fn of(durations: &[i64], pending: u64) -> Self {
        let count = durations.len() as u64;
        Self {
            count,
            pending,
            average_ms: (count > 0).then(|| durations.iter().sum::<i64>() / count as i64),
            max_ms: durations.iter().copied().max(),
        }
    }
}

/// 工作完成率：完成的 Task ÷ 建立的 Task（不含已取消）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TaskCompletion {
    pub completion: Ratio,
    pub cancelled: u64,
}

/// 交接成功率：已轉移給承接帳號的交接 ÷ 已結束（轉移或放棄）的交接；進行中的不計入。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct HandoffMetrics {
    pub success: Ratio,
    pub abandoned: u64,
    /// 承接的 Worker 已確認接手（issue §5.2 步驟 5）的交接數。
    pub confirmed: u64,
    pub in_flight: u64,
}

/// 重複操作事件：另一個 Run 以同一個冪等鍵要求再執行一次而被擋下的次數，依擋下時的狀態分開。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DuplicateOperations {
    pub blocked: u64,
    pub already_succeeded: u64,
    pub result_unknown: u64,
    pub in_progress_elsewhere: u64,
    /// 變成結果未知的次數（可能重複執行的風險，需使用者查證）。
    pub unknown_outcomes: u64,
}

/// 教訓重犯率：有重犯紀錄的 Task ÷ 建立的 Task。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LessonRecurrence {
    pub rate: Ratio,
    pub recurrences: u64,
    /// 被 Guard 擋下的重犯。
    pub prevented: u64,
    /// Guard 沒擋下、使用者回報的重犯。
    pub reported: u64,
}

/// 一個階段的用量。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StageUsage {
    /// Stage 識別；用量事件沒有 Stage（第一版還沒有 Workflow／Stage）時為 `None`，顯示「未分階段」。
    pub stage: Option<String>,
    pub tokens: u64,
    pub runs: u64,
}

/// 每階段用量（本機 Run 回報的 token）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageMetrics {
    pub total_tokens: u64,
    pub runs: u64,
    pub by_stage: Vec<StageUsage>,
}

/// Context 使用量：每個 Run 讀入的 Context token；Adapter 沒有回報的 Run 算在 `runs_unknown`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextUsage {
    pub runs_known: u64,
    pub runs_unknown: u64,
    pub average_tokens: Option<u64>,
    pub max_tokens: Option<u64>,
}

/// 全部指標。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Metrics {
    /// 計算所用的事件數（時間線事件總數）。
    pub events: u64,
    pub task_completion: TaskCompletion,
    pub handoff: HandoffMetrics,
    /// 恢復耗時：偵測到崩潰 → 最後成功的那次嘗試完成處置（工作重新排隊）；不含 10 分鐘穩定觀察。
    /// 尚未成功（等待、嘗試中、觀察中、用盡）的恢復事件算在 `pending`。
    pub recovery: DurationStats,
    pub duplicate_operations: DuplicateOperations,
    /// 裁定等待時間：建立 → 第一次回答；尚未回答的算在 `pending`，被取代或合併的不計。
    pub decision_wait: DurationStats,
    pub lesson_recurrence: LessonRecurrence,
    pub usage: UsageMetrics,
    pub context: ContextUsage,
}

fn text<'a>(entry: &'a TimelineEntry, key: &str) -> Option<&'a str> {
    entry.payload.get(key).and_then(serde_json::Value::as_str)
}

/// 由時間線事件（依 `seq` 排序）計算全部指標。
pub fn compute(entries: &[TimelineEntry]) -> Metrics {
    let is = |e: &TimelineEntry, event_type: &str| e.event_type == event_type;

    // 工作完成率：每個 Task 的最後狀態
    let mut task_status: BTreeMap<&str, Option<&str>> = BTreeMap::new();
    for e in entries.iter().filter(|e| e.category == TimelineCategory::Task) {
        if is(e, task::EVENT_TASK_CREATED) {
            task_status.entry(&e.aggregate_id).or_insert(None);
        } else if is(e, task::EVENT_STATUS_CHANGED) {
            task_status.insert(&e.aggregate_id, text(e, "to"));
        }
    }
    let created = task_status.len() as u64;
    let count_status = |status: &str| task_status.values().filter(|s| **s == Some(status)).count() as u64;
    let cancelled = count_status("cancelled");
    let task_completion =
        TaskCompletion { completion: Ratio::of(count_status("completed"), created - cancelled), cancelled };

    // 交接成功率
    let mut handoffs: BTreeMap<&str, (bool, bool, bool)> = BTreeMap::new();
    for e in entries.iter().filter(|e| e.aggregate_type == handoff::HANDOFF_AGGREGATE) {
        let state = handoffs.entry(&e.aggregate_id).or_default();
        match e.event_type.as_str() {
            handoff::EVENT_HANDOFF_TRANSFERRED => state.0 = true,
            handoff::EVENT_HANDOFF_ABANDONED => state.1 = true,
            handoff::EVENT_HANDOFF_CONFIRMED => state.2 = true,
            _ => {}
        }
    }
    let transferred = handoffs.values().filter(|h| h.0).count() as u64;
    let abandoned = handoffs.values().filter(|h| h.1 && !h.0).count() as u64;
    let handoff = HandoffMetrics {
        success: Ratio::of(transferred, transferred + abandoned),
        abandoned,
        confirmed: handoffs.values().filter(|h| h.2).count() as u64,
        in_flight: handoffs.len() as u64 - transferred - abandoned,
    };

    // 恢復耗時
    let mut recoveries: BTreeMap<&str, Recovering> = BTreeMap::new();
    for e in entries.iter().filter(|e| e.category == TimelineCategory::Recovery) {
        let r = recoveries.entry(&e.aggregate_id).or_default();
        let at = e.occurred_at.as_millis();
        match e.event_type.as_str() {
            recovery::EVENT_RUNTIME_CRASH_DETECTED => r.detected_at = r.detected_at.or(Some(at)),
            recovery::EVENT_RECOVERY_ATTEMPT_COMPLETED => {
                if let Some(attempt) = text(e, "attempt_id") {
                    r.completed.insert(attempt.to_string(), at);
                }
            }
            recovery::EVENT_RECOVERY_SUCCEEDED => r.succeeded_attempt = text(e, "attempt_id").map(str::to_string),
            _ => {}
        }
    }
    let mut recovery_durations = Vec::new();
    let mut recovery_pending = 0;
    for r in recoveries.values() {
        let resumed = r.succeeded_attempt.as_ref().and_then(|a| r.completed.get(a));
        match (r.detected_at, resumed) {
            (Some(detected), Some(resumed)) => recovery_durations.push(resumed - detected),
            _ => recovery_pending += 1,
        }
    }

    // 重複操作事件
    let duplicates: Vec<&TimelineEntry> =
        entries.iter().filter(|e| is(e, external_operation::EVENT_EXTERNAL_OPERATION_DUPLICATE_BLOCKED)).collect();
    let duplicates_with = |status: &str| duplicates.iter().filter(|e| text(e, "status") == Some(status)).count() as u64;
    let duplicate_operations = DuplicateOperations {
        blocked: duplicates.len() as u64,
        already_succeeded: duplicates_with("already_succeeded"),
        result_unknown: duplicates_with("result_unknown"),
        in_progress_elsewhere: duplicates_with("in_progress_elsewhere"),
        unknown_outcomes: entries
            .iter()
            .filter(|e| is(e, external_operation::EVENT_EXTERNAL_OPERATION_RESULT_UNKNOWN))
            .count() as u64,
    };

    // 裁定等待時間
    let mut decisions: BTreeMap<&str, (Option<i64>, Option<i64>, bool)> = BTreeMap::new();
    for e in entries.iter().filter(|e| e.aggregate_type == decision::DECISION_AGGREGATE) {
        let d = decisions.entry(&e.aggregate_id).or_default();
        let at = e.occurred_at.as_millis();
        match e.event_type.as_str() {
            decision::EVENT_DECISION_REQUESTED => d.0 = d.0.or(Some(at)),
            decision::EVENT_DECISION_ANSWERED => d.1 = d.1.or(Some(at)),
            decision::EVENT_DECISION_SUPERSEDED | decision::EVENT_DECISION_MERGED => d.2 = true,
            _ => {}
        }
    }
    let mut decision_durations = Vec::new();
    let mut decision_pending = 0;
    for (requested, answered, closed) in decisions.values() {
        match (requested, answered) {
            (Some(requested), Some(answered)) => decision_durations.push(answered - requested),
            (Some(_), None) if !closed => decision_pending += 1,
            _ => {}
        }
    }

    // 教訓重犯率
    let recurrences: Vec<&TimelineEntry> =
        entries.iter().filter(|e| is(e, guard::EVENT_RECURRENCE_RECORDED)).collect();
    let recurring_tasks: BTreeSet<&str> = recurrences.iter().filter_map(|e| text(e, "task_id")).collect();
    let recurrences_of = |kind: &str| recurrences.iter().filter(|e| text(e, "kind") == Some(kind)).count() as u64;
    let lesson_recurrence = LessonRecurrence {
        rate: Ratio::of(recurring_tasks.len() as u64, created),
        recurrences: recurrences.len() as u64,
        prevented: recurrences_of("prevented"),
        reported: recurrences_of("reported"),
    };

    // 每階段用量與 Context 使用量（同一個 Run 的多次回報加總）
    let mut stages: BTreeMap<Option<String>, (u64, BTreeSet<&str>)> = BTreeMap::new();
    let mut context_by_run: HashMap<&str, Option<u64>> = HashMap::new();
    for e in entries.iter().filter(|e| is(e, quota::EVENT_RUN_USAGE_REPORTED)) {
        let tokens = e.payload.get("tokens").and_then(serde_json::Value::as_u64).unwrap_or(0);
        let stage = text(e, "stage").map(str::to_string);
        let bucket = stages.entry(stage).or_default();
        bucket.0 += tokens;
        bucket.1.insert(&e.aggregate_id);
        let context = e.payload.get("context_tokens").and_then(serde_json::Value::as_u64);
        let known = context_by_run.entry(&e.aggregate_id).or_insert(None);
        if let Some(context) = context {
            *known = Some(known.unwrap_or(0) + context);
        }
    }
    let by_stage: Vec<StageUsage> = stages
        .into_iter()
        .map(|(stage, (tokens, runs))| StageUsage { stage, tokens, runs: runs.len() as u64 })
        .collect();
    let usage = UsageMetrics {
        total_tokens: by_stage.iter().map(|s| s.tokens).sum(),
        runs: context_by_run.len() as u64,
        by_stage,
    };
    let known: Vec<u64> = context_by_run.values().flatten().copied().collect();
    let context = ContextUsage {
        runs_known: known.len() as u64,
        runs_unknown: (context_by_run.len() - known.len()) as u64,
        average_tokens: (!known.is_empty()).then(|| known.iter().sum::<u64>() / known.len() as u64),
        max_tokens: known.iter().copied().max(),
    };

    Metrics {
        events: entries.len() as u64,
        task_completion,
        handoff,
        recovery: DurationStats::of(&recovery_durations, recovery_pending),
        duplicate_operations,
        decision_wait: DurationStats::of(&decision_durations, decision_pending),
        lesson_recurrence,
        usage,
        context,
    }
}

#[derive(Default)]
struct Recovering {
    detected_at: Option<i64>,
    /// 嘗試識別 → 完成處置的時間。
    completed: HashMap<String, i64>,
    succeeded_attempt: Option<String>,
}

#[cfg(test)]
mod tests;
