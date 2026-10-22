use async_trait::async_trait;
use baton_domain::Timestamp;
use serde::{Deserialize, Serialize};

use crate::PortResult;

/// account_profile 的參照。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProfileRef(pub String);

/// 額度資料來源（spec D-12）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum QuotaSource {
    Observed,
    Estimated,
}

/// 可信度；觀測不到時為 Unknown，不填零。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Confidence {
    Observed,
    Estimated,
    Unknown,
}

#[derive(Debug, Clone, PartialEq)]
pub struct QuotaSample {
    pub source: QuotaSource,
    pub observed_at: Timestamp,
    /// 已用比例（0.0–1.0）；未知為 `None`。
    pub used: Option<f64>,
    pub reset_at: Option<Timestamp>,
    pub confidence: Confidence,
}

#[async_trait]
pub trait QuotaObserver: Send + Sync {
    async fn observe(&self, profile: &ProfileRef) -> PortResult<QuotaSample>;
}
