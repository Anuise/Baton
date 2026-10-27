//! 桌面殼經由 RuntimeClient 讀取設定頁的備份狀態：現有備份與最新一次還原演練（票 40）。

use std::sync::Arc;

use baton_client::RuntimeClient;
use baton_domain::backup::{BackupTrigger, RetentionReason};
use baton_harness::fakes::FakeSecretProtector;
use baton_harness::Harness;

fn app_client(harness: &Harness) -> RuntimeClient {
    RuntimeClient::new(harness.addr(), harness.data_dir().to_path_buf(), Arc::new(FakeSecretProtector))
}

#[tokio::test]
async fn app_reads_backups_and_the_latest_drill_result() {
    let harness = Harness::start().await;
    let client = app_client(&harness);

    let before = client.backups().await.unwrap();
    assert!(before.backups.is_empty());
    assert_eq!(before.latest_drill, None);

    harness.run_backups().await;
    let after = client.backups().await.unwrap();

    assert_eq!(after.backups.len(), 1);
    assert_eq!(after.backups[0].trigger, BackupTrigger::Daily);
    assert!(after.backups[0].retained_as.contains(&RetentionReason::Daily));
    let drill = after.latest_drill.expect("第一份備份之後就會做第一次還原演練");
    assert!(drill.passed, "{:?}", drill.checks);
    assert_eq!(drill.backup_id.as_deref(), Some(after.backups[0].id.as_str()));
}
