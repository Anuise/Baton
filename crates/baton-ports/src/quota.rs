use async_trait::async_trait;
pub use baton_domain::quota::{Confidence, QuotaSource};
use baton_domain::Timestamp;

use crate::PortResult;

/// account_profile 的參照。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProfileRef(pub String);

#[derive(Debug, Clone, PartialEq)]
pub struct QuotaSample {
    pub source: QuotaSource,
    pub observed_at: Timestamp,
    /// 已用比例（0.0–1.0）；未知為 `None`。
    pub used: Option<f64>,
    pub reset_at: Option<Timestamp>,
    pub confidence: Confidence,
}

/// 外部額度觀測器（spec D-12 的擴充點，例如 CLI 直接的額度查詢）：Runtime 每 5 分鐘輪詢一次（spec D-31）。
/// 「已觀測」（CLI 限流訊號）與「推估」（本機 Run 用量）兩個來源由 Runtime 自己從 Adapter 事件累積，不經過這個 Port。
/// 回傳 `PortError::NotImplemented` 代表沒有外部觀測器，不留下任何觀測；其他錯誤記為一筆未知的觀測。
#[async_trait]
pub trait QuotaObserver: Send + Sync {
    async fn observe(&self, profile: &ProfileRef) -> PortResult<QuotaSample>;
}
