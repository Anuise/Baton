//! 開 App 時只補缺少的元件（票 32，ADR-0006 M1）：Runtime 在就連回、不重啟 Runtime，
//! 監護不在才補啟動監護；健康查詢回報的監護狀態就是補不補的依據。

use std::cell::Cell;
use std::sync::Arc;
use std::time::Duration;

use baton_api::HealthResponse;
use baton_client::{ensure_guardian, ensure_runtime, ClientError, Connection, RuntimeClient};
use baton_domain::guardian::{GuardianState, GuardianStatus, DEGRADED_MESSAGE};
use baton_harness::fakes::FakeSecretProtector;
use baton_harness::Harness;

fn app_client(harness: &Harness) -> RuntimeClient {
    RuntimeClient::new(harness.addr(), harness.data_dir().to_path_buf(), Arc::new(FakeSecretProtector))
}

/// App 開啟時的連線流程：連回既有的 Runtime，沒有才啟動；回傳連線方式、健康狀態與啟動 Runtime 的次數。
async fn open_app(client: &RuntimeClient) -> (Connection, HealthResponse, usize) {
    let started = Cell::new(0);
    let (connection, health) = ensure_runtime(
        client,
        || {
            started.set(started.get() + 1);
            Ok(())
        },
        Duration::from_millis(500),
    )
    .await
    .unwrap();
    (connection, health, started.get())
}

#[tokio::test]
async fn reopening_with_runtime_alive_but_guardian_gone_starts_only_the_guardian() {
    let harness = Harness::start().await;
    harness.guardian_probe.set_running(false);
    let client = app_client(&harness);

    let (connection, health, spawned_runtimes) = open_app(&client).await;
    assert_eq!(connection, Connection::AlreadyRunning);
    assert_eq!(spawned_runtimes, 0, "Runtime 還在，不重啟 Runtime");

    let guardians_started = Cell::new(0);
    let started = ensure_guardian(&health, || {
        guardians_started.set(guardians_started.get() + 1);
        Ok(())
    })
    .unwrap();
    assert!(started);
    assert_eq!(guardians_started.get(), 1, "只補啟動缺少的監護");
}

#[tokio::test]
async fn a_running_guardian_is_left_alone() {
    let harness = Harness::start().await;
    harness.guardian_probe.set_running(true);
    let client = app_client(&harness);

    let (_, health, _) = open_app(&client).await;
    let guardians_started = Cell::new(0);
    let started = ensure_guardian(&health, || {
        guardians_started.set(guardians_started.get() + 1);
        Ok(())
    })
    .unwrap();
    assert!(!started);
    assert_eq!(guardians_started.get(), 0, "監護已在執行，不啟動第二個");
}

#[tokio::test]
async fn health_reports_the_guardian_state() {
    let harness = Harness::start().await;
    let client = app_client(&harness);

    harness.guardian_probe.set_running(false);
    let degraded = client.health().await.unwrap().guardian;
    assert_eq!(degraded.state, GuardianState::Unavailable);
    assert_eq!(degraded.message.as_deref(), Some(DEGRADED_MESSAGE));
    assert_eq!(DEGRADED_MESSAGE, "工作繼續，自動重啟保護不可用");

    harness.guardian_probe.set_running(true);
    let active = client.health().await.unwrap().guardian;
    assert_eq!(active, GuardianStatus { state: GuardianState::Active, message: None });
}

#[tokio::test]
async fn an_older_runtime_without_guardian_status_gets_a_guardian() {
    let harness = Harness::start().await;
    let mut health = app_client(&harness).health().await.unwrap();
    // 舊版 Runtime 的健康回應沒有監護欄位，反序列化後是 Unknown
    health.guardian = GuardianStatus::default();
    assert_eq!(health.guardian.state, GuardianState::Unknown);

    let guardians_started = Cell::new(0);
    let started = ensure_guardian(&health, || {
        guardians_started.set(guardians_started.get() + 1);
        Ok(())
    })
    .unwrap();
    assert!(started);
    assert_eq!(guardians_started.get(), 1);
}

#[tokio::test]
async fn a_guardian_that_cannot_be_spawned_is_reported() {
    let harness = Harness::start().await;
    harness.guardian_probe.set_running(false);
    let health = app_client(&harness).health().await.unwrap();

    let result = ensure_guardian(&health, || Err(std::io::Error::new(std::io::ErrorKind::NotFound, "找不到執行檔")));
    assert!(matches!(result, Err(ClientError::Spawn(_))));
}
