//! Domain 事件：狀態變更的追加式稽核紀錄（issue #3 §11.2 的 events）。

use serde::Serialize;

use crate::Timestamp;

/// 一筆業務事件；Domain 只產出，由 Store 與狀態變更在同一個交易中追加。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DomainEvent {
    pub aggregate_type: &'static str,
    pub aggregate_id: String,
    /// 該聚合在這個事件之後的版本；同一聚合的事件版本連續遞增。
    pub aggregate_version: i64,
    pub event_type: &'static str,
    /// UTC（spec D-33）。
    pub occurred_at: Timestamp,
    pub payload: serde_json::Value,
}
