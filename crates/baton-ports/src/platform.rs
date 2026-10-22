//! PlatformInfo Port：回報平台事實；支援狀態由 Domain 的 `platform::assess` 判定。

pub use baton_domain::platform::{OsFamily, PlatformSnapshot};

pub trait PlatformInfo: Send + Sync {
    fn snapshot(&self) -> PlatformSnapshot;
}
