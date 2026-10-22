//! 崩潰恢復（票 31）：桌面殼經由 RuntimeClient 查詢恢復狀態與「重新嘗試恢復」（與 App 實際使用的路徑相同）。

use std::sync::Arc;

use baton_client::{ClientError, RuntimeClient};
use baton_harness::fakes::FakeSecretProtector;
use baton_harness::Harness;

fn app_client(harness: &Harness) -> RuntimeClient {
    RuntimeClient::new(harness.addr(), harness.data_dir().to_path_buf(), Arc::new(FakeSecretProtector))
}

#[tokio::test]
async fn without_a_crash_there_is_no_recovery_event_and_nothing_to_retry() {
    let harness = Harness::start().await;
    let client = app_client(&harness);

    let status = client.recovery().await.unwrap();
    assert!(status.event.is_none());
    assert!(!status.can_retry);
    assert!(status.blocking.is_none(), "沒有恢復事件時不擋派工");
    assert_eq!(status.max_attempts, 3);

    // 沒有任何恢復事件時不能重新嘗試恢復
    match client.retry_recovery().await {
        Err(ClientError::Rejected { code, .. }) => assert_eq!(code, "no_recovery_event"),
        other => panic!("應該被拒絕：{other:?}"),
    }
}
