//! 監護狀態機的單元測試（票 32）：以假的 RuntimeHost 驗證寫日誌、最後核對與啟動的順序。
//! 與控制資料庫、真實 Runtime 的銜接由 baton-harness 的情境測試涵蓋。

use std::sync::atomic::{AtomicI64, Ordering};

use baton_domain::guardian::JournalRecord;

use super::*;

struct TestClock(AtomicI64);

impl Clock for TestClock {
    fn now(&self) -> Timestamp {
        Timestamp::from_millis(self.0.load(Ordering::SeqCst))
    }
}

#[derive(Default)]
struct FakeHost {
    running: bool,
    /// 下一次 `runtime_running` 回傳 true 之前要先回傳的 false 次數；`None` 表示照 `running`。
    appear_on_probe: Option<usize>,
    fail_start: Option<String>,
    started: Vec<String>,
}

#[async_trait]
impl RuntimeHost for FakeHost {
    async fn runtime_running(&mut self) -> bool {
        if let Some(remaining) = self.appear_on_probe.as_mut() {
            if *remaining == 0 {
                self.running = true;
                self.appear_on_probe = None;
            } else {
                *remaining -= 1;
            }
        }
        self.running
    }

    async fn start_runtime_at_login(&mut self) -> Result<(), String> {
        self.start_runtime("login").await
    }

    async fn start_runtime(&mut self, attempt_id: &str) -> Result<(), String> {
        self.started.push(attempt_id.to_string());
        match self.fail_start.take() {
            Some(error) => Err(error),
            None => {
                self.running = true;
                Ok(())
            }
        }
    }
}

fn guardian(dir: &std::path::Path) -> (Guardian, Arc<TestClock>) {
    let clock = Arc::new(TestClock(AtomicI64::new(1_000_000)));
    (Guardian::new(dir.to_path_buf(), clock.clone()), clock)
}

#[tokio::test]
async fn restarts_only_after_the_runtime_is_confirmed_gone_and_after_journaling() {
    let dir = tempfile::tempdir().unwrap();
    let (mut guardian, _clock) = guardian(dir.path());
    let mut host = FakeHost { running: true, ..Default::default() };
    assert_eq!(guardian.step(&mut host).await, GuardianStep::Watching);
    host.running = false;
    assert_eq!(guardian.step(&mut host).await, GuardianStep::RuntimeExited);
    assert!(files::read_journal(dir.path()).unwrap().is_empty(), "確認結束本身不寫日誌");
    let GuardianStep::Restarted(attempt) = guardian.step(&mut host).await else { panic!("應重啟") };
    assert_eq!(host.started, vec![attempt.attempt_id.clone()], "以寫下的嘗試識別啟動");
    assert_eq!(files::read_journal(dir.path()).unwrap(), vec![JournalRecord::Attempt(attempt)]);
    assert_eq!(guardian.step(&mut host).await, GuardianStep::Watching);
}

#[tokio::test]
async fn a_failed_spawn_still_counts_and_the_next_start_waits_the_backoff() {
    let dir = tempfile::tempdir().unwrap();
    let (mut guardian, clock) = guardian(dir.path());
    let mut host = FakeHost { fail_start: Some("找不到執行檔".into()), ..Default::default() };
    assert_eq!(guardian.step(&mut host).await, GuardianStep::RuntimeExited);
    assert!(matches!(guardian.step(&mut host).await, GuardianStep::StartFailed { .. }));
    let GuardianStep::Waiting { until } = guardian.step(&mut host).await else { panic!("第二次要先退避") };
    assert_eq!(until, clock.now().plus_millis(baton_domain::recovery::BACKOFF_BEFORE_SECOND_MS));
    clock.0.store(until.as_millis(), Ordering::SeqCst);
    let GuardianStep::Restarted(attempt) = guardian.step(&mut host).await else { panic!("退避期滿應重啟") };
    assert_eq!(attempt.seq, 2, "啟動失敗的第一次也計次");
}

#[tokio::test]
async fn a_runtime_that_appears_after_journaling_withdraws_the_attempt() {
    let dir = tempfile::tempdir().unwrap();
    let (mut guardian, _clock) = guardian(dir.path());
    let mut host = FakeHost::default();
    assert_eq!(guardian.step(&mut host).await, GuardianStep::RuntimeExited);
    host.appear_on_probe = Some(0); // 寫下日誌之後的最後核對時，使用者剛開 App 啟動了 Runtime
    let GuardianStep::Withdrawn { attempt_id } = guardian.step(&mut host).await else { panic!("應撤回") };
    assert!(host.started.is_empty(), "不啟動第二個 Runtime");
    let journal = files::read_journal(dir.path()).unwrap();
    assert!(matches!(&journal[1], JournalRecord::Withdrawn { attempt_id: id, .. } if *id == attempt_id));
    assert_eq!(guardian.step(&mut host).await, GuardianStep::Watching, "改為監看那個 Runtime");
}

#[tokio::test]
async fn an_unwritable_journal_means_no_restart() {
    let dir = tempfile::tempdir().unwrap();
    // guardian 目錄的位置被一個檔案佔住：日誌寫不進去
    std::fs::write(files::guardian_dir(dir.path()), b"").unwrap();
    let (mut guardian, _clock) = guardian(dir.path());
    let mut host = FakeHost::default();
    assert_eq!(guardian.step(&mut host).await, GuardianStep::RuntimeExited);
    assert!(matches!(guardian.step(&mut host).await, GuardianStep::Finished(GuardianEnd::JournalUnavailable(_))));
    assert!(host.started.is_empty(), "無法保存就不自動啟動");
    assert!(matches!(guardian.finished(), Some(GuardianEnd::JournalUnavailable(_))));
}
