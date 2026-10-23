//! 事件時間線（票 41）：把追加式事件表中與工作有關的事件分類、關聯到 Task，依提交順序呈現。
//!
//! 純邏輯、無 I/O。Store 依 `TIMELINE_AGGREGATES` 讀出事件並解析每筆事件相關的 Task 與 Run，
//! 這裡決定事件屬於哪一類（不在任何一類的事件不上時間線）與依 Task 過濾。觀測指標（`metrics`）只由時間線事件計算。

use serde::{Deserialize, Serialize};

use crate::{
    cleanup, controller_cli, decision, external_operation, guard, handoff, quota, recovery, run, task, verification, Timestamp,
};

/// 時間線的事件類別。前六類是票面要求的「派工、交接、裁定、恢復、驗證與清理」；
/// 其後四類是計算觀測指標需要的事件（Task 狀態、外部操作、教訓重犯、用量），同樣可在時間線上追溯；
/// 最後是 Controller CLI 的開啟、結束與帳號（desktop-controller-cli 票 03）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimelineCategory {
    Dispatch,
    Handoff,
    Decision,
    Recovery,
    Verification,
    Cleanup,
    Task,
    ExternalOperation,
    Lesson,
    Usage,
    Controller,
}

impl TimelineCategory {
    pub const ALL: [TimelineCategory; 11] = [
        TimelineCategory::Dispatch,
        TimelineCategory::Handoff,
        TimelineCategory::Decision,
        TimelineCategory::Recovery,
        TimelineCategory::Verification,
        TimelineCategory::Cleanup,
        TimelineCategory::Task,
        TimelineCategory::ExternalOperation,
        TimelineCategory::Lesson,
        TimelineCategory::Usage,
        TimelineCategory::Controller,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            TimelineCategory::Dispatch => "dispatch",
            TimelineCategory::Handoff => "handoff",
            TimelineCategory::Decision => "decision",
            TimelineCategory::Recovery => "recovery",
            TimelineCategory::Verification => "verification",
            TimelineCategory::Cleanup => "cleanup",
            TimelineCategory::Task => "task",
            TimelineCategory::ExternalOperation => "external_operation",
            TimelineCategory::Lesson => "lesson",
            TimelineCategory::Usage => "usage",
            TimelineCategory::Controller => "controller",
        }
    }
}

/// 可能出現在時間線上的聚合種類；Store 只讀這些聚合的事件。
pub const TIMELINE_AGGREGATES: [&str; 11] = [
    run::RUN_AGGREGATE,
    handoff::HANDOFF_AGGREGATE,
    decision::DECISION_AGGREGATE,
    recovery::RECOVERY_AGGREGATE,
    task::TASK_AGGREGATE,
    cleanup::AGGREGATE_TYPE,
    external_operation::EXTERNAL_OPERATION_AGGREGATE,
    external_operation::EXTERNAL_OPERATION_DUPLICATE_AGGREGATE,
    guard::RECURRENCE_AGGREGATE,
    quota::USAGE_AGGREGATE,
    controller_cli::CONTROLLER_CLI_AGGREGATE,
];

/// Run 的派工生命週期事件。進度、checkpoint、lease 續約與原生 Session 綁定量大且不是派工的轉折，不上時間線。
const DISPATCH_EVENTS: [&str; 9] = [
    run::EVENT_RUN_CREATED,
    run::EVENT_RUN_ADMISSION_DENIED,
    run::EVENT_RUN_LAUNCH_STARTED,
    run::EVENT_RUN_STARTED,
    run::EVENT_RUN_SUCCEEDED,
    run::EVENT_RUN_FAILED,
    run::EVENT_RUN_INTERRUPTED,
    run::EVENT_RUN_CANCEL_REQUESTED,
    run::EVENT_RUN_CANCELLED,
];

/// 裁定請求的事件（權限與決策權限表的稽核事件屬於其他聚合，不在這裡）。
const DECISION_EVENTS: [&str; 7] = [
    decision::EVENT_DECISION_REQUESTED,
    decision::EVENT_DECISION_ANSWERED,
    decision::EVENT_DECISION_DEFERRED,
    decision::EVENT_DECISION_SUPERSEDED,
    decision::EVENT_DECISION_MERGED,
    decision::EVENT_DECISION_MERGE_RECEIVED,
    decision::EVENT_DECISION_LINKED,
];

/// 事件屬於哪一類；不上時間線的事件為 `None`。
pub fn category_of(aggregate_type: &str, event_type: &str) -> Option<TimelineCategory> {
    use TimelineCategory as C;
    match aggregate_type {
        run::RUN_AGGREGATE if event_type == run::EVENT_RUN_HANDED_OFF => Some(C::Handoff),
        run::RUN_AGGREGATE => DISPATCH_EVENTS.contains(&event_type).then_some(C::Dispatch),
        handoff::HANDOFF_AGGREGATE => Some(C::Handoff),
        decision::DECISION_AGGREGATE => DECISION_EVENTS.contains(&event_type).then_some(C::Decision),
        recovery::RECOVERY_AGGREGATE => Some(C::Recovery),
        task::TASK_AGGREGATE => match event_type {
            verification::EVENT_TASK_VERIFICATION_RECORDED => Some(C::Verification),
            task::EVENT_TASK_CREATED | task::EVENT_STATUS_CHANGED => Some(C::Task),
            _ => None,
        },
        cleanup::AGGREGATE_TYPE => Some(C::Cleanup),
        external_operation::EXTERNAL_OPERATION_AGGREGATE | external_operation::EXTERNAL_OPERATION_DUPLICATE_AGGREGATE => {
            Some(C::ExternalOperation)
        }
        guard::RECURRENCE_AGGREGATE => Some(C::Lesson),
        quota::USAGE_AGGREGATE => Some(C::Usage),
        controller_cli::CONTROLLER_CLI_AGGREGATE => Some(C::Controller),
        _ => None,
    }
}

/// 時間線上的一筆事件。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimelineEntry {
    /// 事件表的提交序號；時間線依它排序（同一毫秒內的事件也有確定的先後）。
    pub seq: i64,
    /// UTC（spec D-33）。
    pub occurred_at: Timestamp,
    pub category: TimelineCategory,
    pub aggregate_type: String,
    pub aggregate_id: String,
    pub event_type: String,
    /// 相關的 Task。恢復事件關聯到它任何一次嘗試處置過的 Task（可能多個，也可能沒有）；其他事件最多一個。
    pub task_ids: Vec<String>,
    pub run_id: Option<String>,
    pub payload: serde_json::Value,
}

impl TimelineEntry {
    pub fn concerns(&self, task_id: &str) -> bool {
        self.task_ids.iter().any(|t| t == task_id)
    }
}

/// 依 Task 過濾：保留與它有關的事件，順序不變。
pub fn for_task(entries: Vec<TimelineEntry>, task_id: &str) -> Vec<TimelineEntry> {
    entries.into_iter().filter(|e| e.concerns(task_id)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_required_kind_of_event_has_its_category() {
        use TimelineCategory as C;
        let cases = [
            (run::RUN_AGGREGATE, run::EVENT_RUN_CREATED, C::Dispatch),
            (run::RUN_AGGREGATE, run::EVENT_RUN_SUCCEEDED, C::Dispatch),
            (run::RUN_AGGREGATE, run::EVENT_RUN_HANDED_OFF, C::Handoff),
            (handoff::HANDOFF_AGGREGATE, handoff::EVENT_HANDOFF_TRANSFERRED, C::Handoff),
            (decision::DECISION_AGGREGATE, decision::EVENT_DECISION_ANSWERED, C::Decision),
            (recovery::RECOVERY_AGGREGATE, recovery::EVENT_RUNTIME_CRASH_DETECTED, C::Recovery),
            (task::TASK_AGGREGATE, verification::EVENT_TASK_VERIFICATION_RECORDED, C::Verification),
            (cleanup::AGGREGATE_TYPE, cleanup::EVENT_CLEANED, C::Cleanup),
            (task::TASK_AGGREGATE, task::EVENT_STATUS_CHANGED, C::Task),
            (external_operation::EXTERNAL_OPERATION_AGGREGATE, external_operation::EVENT_EXTERNAL_OPERATION_BEGUN, C::ExternalOperation),
            (
                external_operation::EXTERNAL_OPERATION_DUPLICATE_AGGREGATE,
                external_operation::EVENT_EXTERNAL_OPERATION_DUPLICATE_BLOCKED,
                C::ExternalOperation,
            ),
            (guard::RECURRENCE_AGGREGATE, guard::EVENT_RECURRENCE_RECORDED, C::Lesson),
            (quota::USAGE_AGGREGATE, quota::EVENT_RUN_USAGE_REPORTED, C::Usage),
        ];
        for (aggregate, event, expected) in cases {
            assert_eq!(category_of(aggregate, event), Some(expected), "{aggregate}/{event}");
            assert!(TIMELINE_AGGREGATES.contains(&aggregate));
        }
    }

    #[test]
    fn noisy_or_unrelated_events_stay_off_the_timeline() {
        assert_eq!(category_of(run::RUN_AGGREGATE, run::EVENT_RUN_PROGRESS_REPORTED), None);
        assert_eq!(category_of(run::RUN_AGGREGATE, run::EVENT_RUN_LEASE_VERIFIED), None);
        assert_eq!(category_of(task::TASK_AGGREGATE, task::EVENT_CONTRACT_CHANGED), None);
        assert_eq!(category_of("workspace", "workspace_created"), None);
        assert_eq!(category_of(decision::PERMISSION_AUDIT_AGGREGATE, decision::EVENT_PERMISSION_AUTO_ALLOWED), None);
    }

    #[test]
    fn filtering_by_task_keeps_only_its_events_in_order() {
        let entry = |seq: i64, tasks: &[&str]| TimelineEntry {
            seq,
            occurred_at: Timestamp::from_millis(seq),
            category: TimelineCategory::Dispatch,
            aggregate_type: "run".into(),
            aggregate_id: format!("r{seq}"),
            event_type: "run_created".into(),
            task_ids: tasks.iter().map(|t| t.to_string()).collect(),
            run_id: None,
            payload: serde_json::json!({}),
        };
        let all = vec![entry(1, &["a"]), entry(2, &["b"]), entry(3, &["a", "b"]), entry(4, &[]), entry(5, &["a"])];
        let seqs: Vec<i64> = for_task(all, "a").iter().map(|e| e.seq).collect();
        assert_eq!(seqs, [1, 3, 5]);
    }
}
