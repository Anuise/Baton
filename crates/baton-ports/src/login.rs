//! 跨 Windows 登入的啟動（票 33；DR-20261021-07、spec D-25）。

use crate::PortResult;

/// 目前 Windows 登入工作階段的識別。Runtime 每次啟動時記下它，與上一次啟動比較，判斷是否跨了登入。
pub trait LoginSession: Send + Sync {
    /// 同一次登入期間不變，重新登入（含重新開機）後不同；取不到時為 `None`（視為無法核對）。
    fn current_login(&self) -> Option<String>;
}

/// 「登入時自動啟動」在 Windows 上的登記（目前使用者的 HKCU Run 機碼，啟動 `baton-guardian.exe --login`）。
pub trait StartupRegistration: Send + Sync {
    /// 目前是否已登記（而且登記的是這個安裝位置的啟動入口）。
    fn is_registered(&self) -> PortResult<bool>;

    /// 登記或移除；重複呼叫是冪等的。
    fn set_registered(&self, enabled: bool) -> PortResult<()>;
}
