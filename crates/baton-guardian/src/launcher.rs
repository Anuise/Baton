//! 登入啟動入口（票 33；DR-20261021-07、ADR-0007、spec D-25）。
//!
//! 「登入時自動啟動」開啟時，Windows 登入後由 HKCU Run 機碼執行 `baton-guardian.exe --login`。它是監護執行檔的另一個模式：
//! 1. 先讀生命週期檔（不開資料庫）：完全退出、維護或停止意圖不明時直接結束，不啟動 Runtime、監護或任何工作。
//! 2. 已有 Runtime 在執行（使用者剛手動開 App）就不再啟動；否則以「登入啟動」的身分啟動 Runtime。
//!    兩者同時發生時，多出來的 Runtime 會因單實例鎖自行結束，所以只有一個控制平面。
//! 3. 之後同一個程序繼續當監護（由呼叫端接續）。
//!
//! 資料庫裡有停止意圖、生命週期檔卻沒有（寫入失敗）的情形，由登入啟動的 Runtime 自己核對後拒絕提供服務。

use std::path::Path;

use baton_domain::guardian::{self, Suppression};
use baton_ports::Clock;

use crate::{files, RuntimeHost};

/// 登入啟動入口做了什麼。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginLaunch {
    /// 受抑制：不啟動 Runtime、監護或任何工作。
    Suppressed(Suppression),
    /// 已有 Runtime 在執行：不再啟動，改為監護它。
    AlreadyRunning,
    /// 已以「登入啟動」的身分啟動 Runtime 程序。
    Started,
    /// 無法啟動 Runtime 程序。
    StartFailed(String),
}

/// 登入後執行一次。
pub async fn launch_at_login(data_dir: &Path, clock: &dyn Clock, host: &mut dyn RuntimeHost) -> LoginLaunch {
    if let Some(suppression) = guardian::suppression_of(&files::read_lifecycle(data_dir)) {
        files::append_log(data_dir, clock.now(), &format!("登入啟動：受抑制，不啟動 Runtime：{suppression:?}"));
        return LoginLaunch::Suppressed(suppression);
    }
    if host.runtime_running().await {
        files::append_log(data_dir, clock.now(), "登入啟動：已有 Runtime 在執行，不再啟動");
        return LoginLaunch::AlreadyRunning;
    }
    match host.start_runtime_at_login().await {
        Ok(()) => {
            files::append_log(data_dir, clock.now(), "登入啟動：已啟動 Runtime");
            LoginLaunch::Started
        }
        Err(error) => {
            files::append_log(data_dir, clock.now(), &format!("登入啟動：無法啟動 Runtime：{error}"));
            LoginLaunch::StartFailed(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use baton_domain::lifecycle::{LifecycleFile, MaintenanceSuppression, StopSuppression, LIFECYCLE_FILE_NAME};
    use baton_domain::Timestamp;

    struct FixedClock;

    impl Clock for FixedClock {
        fn now(&self) -> Timestamp {
            Timestamp::from_millis(1_000)
        }
    }

    #[derive(Default)]
    struct Host {
        running: bool,
        fail: Option<String>,
        login_starts: usize,
    }

    #[async_trait]
    impl RuntimeHost for Host {
        async fn runtime_running(&mut self) -> bool {
            self.running
        }

        async fn start_runtime(&mut self, _attempt_id: &str) -> Result<(), String> {
            panic!("登入啟動不帶監護的嘗試識別")
        }

        async fn start_runtime_at_login(&mut self) -> Result<(), String> {
            self.login_starts += 1;
            match self.fail.take() {
                Some(error) => Err(error),
                None => {
                    self.running = true;
                    Ok(())
                }
            }
        }
    }

    fn write_file(dir: &Path, file: &LifecycleFile) {
        std::fs::write(dir.join(LIFECYCLE_FILE_NAME), serde_json::to_vec(file).unwrap()).unwrap();
    }

    fn file(stop: bool, maintenance: bool) -> LifecycleFile {
        LifecycleFile {
            version: 1,
            stop: stop.then(|| StopSuppression { requested_at: Timestamp::from_millis(1), reason: "full_exit".into() }),
            maintenance: maintenance
                .then(|| MaintenanceSuppression { kind: "update".into(), requested_at: Timestamp::from_millis(1) }),
            updated_at: Timestamp::from_millis(1),
        }
    }

    #[tokio::test]
    async fn starts_the_runtime_as_a_login_start_when_nothing_suppresses_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut host = Host::default();
        assert_eq!(launch_at_login(dir.path(), &FixedClock, &mut host).await, LoginLaunch::Started, "沒有生命週期檔");
        assert_eq!(host.login_starts, 1);

        write_file(dir.path(), &file(false, false));
        let mut host = Host::default();
        assert_eq!(launch_at_login(dir.path(), &FixedClock, &mut host).await, LoginLaunch::Started);
    }

    #[tokio::test]
    async fn full_exit_maintenance_or_an_unreadable_file_suppress_the_launch() {
        let dir = tempfile::tempdir().unwrap();
        let cases = [
            (Some(file(true, false)), Suppression::FullExit),
            (Some(file(false, true)), Suppression::Maintenance("update".into())),
            (None, Suppression::Unknown),
        ];
        for (content, expected) in cases {
            match content {
                Some(content) => write_file(dir.path(), &content),
                None => std::fs::write(dir.path().join(LIFECYCLE_FILE_NAME), b"{broken").unwrap(),
            }
            let mut host = Host::default();
            assert_eq!(
                launch_at_login(dir.path(), &FixedClock, &mut host).await,
                LoginLaunch::Suppressed(expected)
            );
            assert_eq!(host.login_starts, 0, "受抑制時不啟動 Runtime");
        }
    }

    #[tokio::test]
    async fn an_already_running_runtime_is_not_started_again() {
        let dir = tempfile::tempdir().unwrap();
        let mut host = Host { running: true, ..Host::default() };
        assert_eq!(launch_at_login(dir.path(), &FixedClock, &mut host).await, LoginLaunch::AlreadyRunning);
        assert_eq!(host.login_starts, 0);
    }

    #[tokio::test]
    async fn a_start_failure_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let mut host = Host { fail: Some("找不到執行檔".into()), ..Host::default() };
        assert_eq!(
            launch_at_login(dir.path(), &FixedClock, &mut host).await,
            LoginLaunch::StartFailed("找不到執行檔".into())
        );
    }
}
