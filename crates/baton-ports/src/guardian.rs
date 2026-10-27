//! 監護程序的狀態（票 32）。

/// 監護程序是否還在。只用來顯示自動重啟保護是否降級，不影響任何工作。
pub trait GuardianProbe: Send + Sync {
    fn guardian_running(&self) -> bool;
}
