use serde::{Deserialize, Serialize};

/// 平台支援狀態（spec DR-08，判斷規則由票 03 實作）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SupportStatus {
    Supported,
    Unsupported,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformSnapshot {
    pub os_build: String,
    pub edition: String,
    pub native_arch: String,
    pub support_status: SupportStatus,
}

pub trait PlatformInfo: Send + Sync {
    fn snapshot(&self) -> PlatformSnapshot;
}
