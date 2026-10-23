use std::future::Future;
use std::pin::Pin;

use baton_domain::Timestamp;

/// 時間來源；測試以 fake 控制，不使用真實 sleep。
pub trait Clock: Send + Sync {
    fn now(&self) -> Timestamp;

    /// 等到 `now()` 到達 `deadline`（可能提早醒來，呼叫端要自己再比一次 `now()`）。
    /// 預設永遠不醒：只有會被長輪詢等待使用的時鐘（Runtime 的正式時鐘、harness 的 fake）需要覆寫。
    fn sleep_until(&self, _deadline: Timestamp) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(std::future::pending())
    }
}
