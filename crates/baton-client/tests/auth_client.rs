//! 桌面 App 端的憑證行為：自動讀取、重啟後自動重連、驗證失敗時顯示原因且不降級（票 04，spec D-22）。

use std::cell::Cell;
use std::sync::Arc;
use std::time::Duration;

use baton_client::{ensure_runtime, ClientError, RuntimeClient};
use baton_credential::write_desktop_credential;
use baton_harness::fakes::FakeSecretProtector;
use baton_harness::Harness;

fn app_client(harness: &Harness) -> RuntimeClient {
    RuntimeClient::new(harness.addr(), harness.data_dir().to_path_buf(), Arc::new(FakeSecretProtector))
}

#[tokio::test]
async fn app_connects_with_the_credential_it_reads_from_the_protected_file() {
    let harness = Harness::start().await;

    let health = app_client(&harness).health().await.unwrap();

    assert_eq!(health.status, "ok");
}

#[tokio::test]
async fn app_reconnects_with_the_new_credential_after_runtime_restart() {
    let mut harness = Harness::start().await;
    let client = app_client(&harness);
    assert_eq!(client.health().await.unwrap().boot_count, 1);

    harness.restart().await;
    // 重啟後埠會改變；用戶端換成新的位址，但憑證完全不需要人工處理
    let client = RuntimeClient::new(harness.addr(), harness.data_dir().to_path_buf(), Arc::new(FakeSecretProtector));

    assert_eq!(client.health().await.unwrap().boot_count, 2);
}

#[tokio::test]
async fn stale_credential_is_reported_with_its_reason_and_never_downgraded_to_unauthenticated() {
    let harness = Harness::start().await;
    let client = app_client(&harness);
    write_desktop_credential(harness.data_dir(), &FakeSecretProtector, &"0".repeat(64)).unwrap();

    let error = client.health().await.unwrap_err();

    let ClientError::Rejected { code, message } = &error else { panic!("應為 Rejected：{error:?}") };
    assert_eq!(code, "credential_invalid");
    assert!(!message.is_empty());
    assert!(error.to_string().contains("credential_invalid"), "錯誤訊息要讓使用者看到原因：{error}");
    // Runtime 只看到帶了（錯誤）憑證的請求；沒有任何不帶憑證的重試
    let log = harness.log_text();
    assert!(log.contains("credential_invalid"));
    assert!(!log.contains("credential_missing"), "不可降級為免驗證：\n{log}");
}

#[tokio::test]
async fn missing_credential_file_is_reported_without_contacting_the_runtime() {
    let harness = Harness::start().await;
    let client = app_client(&harness);
    std::fs::remove_file(baton_credential::desktop_credential_path(harness.data_dir())).unwrap();
    let requests_before = harness.log_text().lines().count();

    let error = client.health().await.unwrap_err();

    assert!(matches!(error, ClientError::Credential(_)), "{error:?}");
    assert_eq!(harness.log_text().lines().count(), requests_before, "沒有憑證就不該送出請求");
}

#[tokio::test]
async fn error_messages_never_contain_the_credential() {
    let harness = Harness::start().await;
    let token = harness.desktop_credential();
    // 讓 Runtime 認得憑證、但請求被拒（Agent 角色），訊息仍不得帶出 token
    let agent = harness.issue_credential(baton_domain::auth::Role::Agent, None).await;
    write_desktop_credential(harness.data_dir(), &FakeSecretProtector, &agent).unwrap();

    let error = app_client(&harness).health().await.unwrap_err();

    for secret in [&token, &agent] {
        assert!(!error.to_string().contains(secret.as_str()));
        assert!(!format!("{error:?}").contains(secret.as_str()));
    }
}

#[tokio::test]
async fn ensure_runtime_surfaces_a_rejected_credential_instead_of_starting_another_runtime() {
    let harness = Harness::start().await;
    let client = app_client(&harness);
    write_desktop_credential(harness.data_dir(), &FakeSecretProtector, &"0".repeat(64)).unwrap();
    let started = Cell::new(0);

    let result = ensure_runtime(
        &client,
        || {
            started.set(started.get() + 1);
            Ok(())
        },
        Duration::from_secs(5),
    )
    .await;

    assert!(matches!(result, Err(ClientError::Rejected { .. })), "{result:?}");
    assert_eq!(started.get(), 0, "Runtime 已在執行，只是憑證被拒；不該再啟動新的");
}
