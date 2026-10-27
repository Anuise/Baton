//! 跨 Windows 登入的啟動與續跑（票 33；DR-20261021-07、ADR-0007、spec D-25）。
//!
//! 純邏輯、無 I/O。幾條貫穿全檔的規則：
//! - **兩種意圖分開**：「登入時自動啟動」是目前 Windows 使用者層級的偏好，只決定登入時要不要啟動 Runtime；
//!   每個 Workspace 的「跨登入自動續跑」只決定跨登入啟動後這個 Workspace 要不要續跑。兩者都預設關閉，
//!   也都與 DR-01 的「完全退出後自動續跑」分開保存；變更設定不會立即啟停目前的工作。
//! - **完全退出優先（S1）**：停止意圖生效時不做任何跨登入處置，登入啟動入口也不啟動 Runtime；
//!   只有使用者手動開 App 才依 DR-01 重新開啟。
//! - **跨登入以登入識別判斷**：每次啟動記下目前 Windows 登入工作階段的識別，與上一次啟動比較。
//!   不論是登入啟動入口還是手動開 App 啟動，判斷都相同，所以兩者同時發生時結果不會因誰先啟動而不同。
//!   識別取不到時無法核對，不自動續跑任何 Workspace。
//! - **重新登入不重設任何阻擋**：跨登入處置只會新增暫停，不解除既有的暫停、恢復事件、退避或額度阻擋。

use serde::{Deserialize, Serialize};

use crate::event::DomainEvent;
use crate::lifecycle::{workspace_lifecycle_of, LifecycleChange, LifecycleState, PauseCause, WorkspaceLifecycle};
use crate::workspace::{Workspace, WorkspaceStatus};
use crate::Timestamp;

pub const LOGIN_STARTUP_AGGREGATE: &str = "login_startup";
pub const LOGIN_STARTUP_ID: &str = "user";

pub const EVENT_LOGIN_STARTUP_CHANGED: &str = "login_startup_changed";
pub const EVENT_WORKSPACE_LOGIN_RESUME_CHANGED: &str = "workspace_login_resume_changed";
pub const EVENT_WORKSPACE_PAUSED_BY_LOGIN: &str = "workspace_paused_by_login";

/// 登入啟動入口啟動 Runtime 時，以這個環境變數標明啟動來源。
pub const START_SOURCE_ENV: &str = "BATON_START_SOURCE";
/// `START_SOURCE_ENV` 的值：由 Windows 登入時的啟動入口啟動。
pub const START_SOURCE_LOGIN: &str = "login";
/// 啟動入口的命令列參數：`baton-guardian.exe --login`（launcher 與監護是同一個執行檔的不同模式，spec D-25）。
pub const LAUNCHER_LOGIN_ARG: &str = "--login";
/// 啟動入口的命令列參數：資料目錄（登記時寫入，與登記當下的 Runtime 一致；HKCU Run 無法帶環境變數）。
pub const LAUNCHER_DATA_DIR_ARG: &str = "--data-dir";
/// 啟動入口的命令列參數：Runtime 監聽的埠。
pub const LAUNCHER_PORT_ARG: &str = "--port";

/// 「登入時自動啟動」的偏好（只有一份）。Windows 的登記（HKCU Run 機碼）跟著它；完全退出不改變它（停止抑制另存）。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct LoginStartup {
    /// 預設關閉。
    pub enabled: bool,
    pub version: i64,
    pub updated_at: Option<Timestamp>,
}

/// 變更「登入時自動啟動」；相同值沒有任何變動。
pub fn set_login_startup(current: &LoginStartup, enabled: bool, now: Timestamp) -> Option<(LoginStartup, DomainEvent)> {
    if current.enabled == enabled {
        return None;
    }
    let mut next = current.clone();
    next.enabled = enabled;
    next.version += 1;
    next.updated_at = Some(now);
    let event = DomainEvent {
        aggregate_type: LOGIN_STARTUP_AGGREGATE,
        aggregate_id: LOGIN_STARTUP_ID.to_string(),
        aggregate_version: next.version,
        event_type: EVENT_LOGIN_STARTUP_CHANGED,
        occurred_at: now,
        payload: serde_json::json!({ "enabled": enabled }),
    };
    Some((next, event))
}

/// 變更 Workspace 的「跨登入自動續跑」。只影響之後的跨登入啟動，不會立即啟動或停止目前的工作，也不解除暫停；
/// 相同值沒有任何變動。
pub fn set_login_resume(
    lifecycle: &WorkspaceLifecycle,
    enabled: bool,
    now: Timestamp,
) -> Option<(WorkspaceLifecycle, DomainEvent)> {
    if lifecycle.auto_resume_across_login == enabled {
        return None;
    }
    let mut next = lifecycle.clone();
    next.auto_resume_across_login = enabled;
    let event = next.push(EVENT_WORKSPACE_LOGIN_RESUME_CHANGED, now, serde_json::json!({ "enabled": enabled }));
    Some((next, event))
}

/// 這次啟動與上一次啟動之間是否跨了 Windows 登入。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoginBoundary {
    /// 第一次啟動（沒有上一次）：沒有要續跑的工作。
    FirstStart,
    /// 同一次登入期間再次啟動（重新開 App、監護重啟）。
    SameLogin,
    /// 上一次啟動在另一次登入（重新登入或重新開機）。
    NewLogin,
    /// 這一次或上一次的登入識別取不到：無法核對。
    Unverified,
}

/// 比較登入識別。`previous` 是上一次啟動記下的識別：沒有上一次啟動為 `None`，上一次沒記下識別為 `Some(None)`。
pub fn login_boundary(previous: Option<Option<&str>>, current: Option<&str>) -> LoginBoundary {
    match (previous, current) {
        (None, _) => LoginBoundary::FirstStart,
        (Some(Some(before)), Some(now)) if before == now => LoginBoundary::SameLogin,
        (Some(Some(_)), Some(_)) => LoginBoundary::NewLogin,
        _ => LoginBoundary::Unverified,
    }
}

/// 啟動時（停止意圖核對完之後）依登入邊界處置各 Workspace：
/// - 停止意圖生效時不做任何事（完全退出優先；手動開 App 才依 DR-01 重新開啟）。
/// - 跨了登入：沒有開啟「跨登入自動續跑」的使用中 Workspace 暫停，等「繼續工作」；開啟的照常續跑。
/// - 無法核對：每個使用中的 Workspace 都暫停（不論設定）。
/// - 已經暫停的 Workspace 維持原本的暫停與原因；這裡永遠不解除任何暫停。
pub fn cross_login(
    state: &LifecycleState,
    boundary: LoginBoundary,
    workspaces: &[Workspace],
    existing: &[WorkspaceLifecycle],
    now: Timestamp,
) -> LifecycleChange {
    if state.stop_intent_active() {
        return LifecycleChange::default();
    }
    let cause = match boundary {
        LoginBoundary::FirstStart | LoginBoundary::SameLogin => return LifecycleChange::default(),
        LoginBoundary::NewLogin => PauseCause::NewLogin,
        LoginBoundary::Unverified => PauseCause::LoginUnverified,
    };
    let mut change = LifecycleChange::default();
    for workspace in workspaces.iter().filter(|w| w.status == WorkspaceStatus::Active) {
        let mut lifecycle = workspace_lifecycle_of(existing, &workspace.id);
        if lifecycle.paused_since.is_some() {
            continue;
        }
        if cause == PauseCause::NewLogin && lifecycle.auto_resume_across_login {
            continue;
        }
        lifecycle.paused_since = Some(now);
        lifecycle.paused_by = Some(cause);
        change
            .events
            .push(lifecycle.push(EVENT_WORKSPACE_PAUSED_BY_LOGIN, now, serde_json::json!({ "cause": cause })));
        change.workspaces.push(lifecycle);
    }
    change
}

#[cfg(test)]
mod tests;
