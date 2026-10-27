//! 桌面 App 連回既有 Runtime、或啟動不成功時的行為。

use std::cell::Cell;
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::sync::Arc;
use std::time::Duration;

use baton_client::{ensure_runtime, ClientError, Connection, RuntimeClient};
use baton_harness::fakes::FakeSecretProtector;
use baton_harness::Harness;

fn unused_local_addr() -> SocketAddr {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    listener.local_addr().unwrap()
}

/// 沒有任何 Runtime 的資料目錄：用戶端讀不到憑證，也連不上。
fn offline_client() -> RuntimeClient {
    let data_dir = std::env::temp_dir().join("baton-offline-test-no-such-data-dir");
    RuntimeClient::new(unused_local_addr(), data_dir, Arc::new(FakeSecretProtector))
}

#[tokio::test]
async fn connects_to_running_runtime_without_starting_another() {
    let harness = Harness::start().await;
    let started = Cell::new(0);
    let client = RuntimeClient::new(harness.addr(), harness.data_dir().to_path_buf(), Arc::new(FakeSecretProtector));

    let (connection, health) = ensure_runtime(
        &client,
        || {
            started.set(started.get() + 1);
            Ok(())
        },
        Duration::from_millis(200),
    )
    .await
    .unwrap();

    assert_eq!(connection, Connection::AlreadyRunning);
    assert_eq!(health.status, "ok");
    assert_eq!(started.get(), 0, "已有 Runtime 在執行時不應啟動新的");
}

#[tokio::test]
async fn starts_runtime_once_and_times_out_if_it_never_becomes_healthy() {
    let started = Cell::new(0);

    let result = ensure_runtime(
        &offline_client(),
        || {
            started.set(started.get() + 1);
            Ok(())
        },
        Duration::from_millis(300),
    )
    .await;

    assert!(matches!(result, Err(ClientError::StartTimeout(_))));
    assert_eq!(started.get(), 1);
}

#[tokio::test]
async fn reports_start_failure_without_waiting() {
    let result = ensure_runtime(
        &offline_client(),
        || Err(std::io::Error::new(std::io::ErrorKind::NotFound, "找不到執行檔")),
        Duration::from_secs(30),
    )
    .await;

    assert!(matches!(result, Err(ClientError::Spawn(_))));
}
