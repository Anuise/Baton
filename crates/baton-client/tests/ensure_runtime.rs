//! 桌面 App 連回既有 Runtime、或啟動不成功時的行為。

use std::cell::Cell;
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::time::Duration;

use baton_client::{ensure_runtime, ClientError, Connection};
use baton_harness::Harness;

fn unused_local_addr() -> SocketAddr {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    listener.local_addr().unwrap()
}

#[tokio::test]
async fn connects_to_running_runtime_without_starting_another() {
    let harness = Harness::start().await;
    let started = Cell::new(0);

    let (connection, health) = ensure_runtime(
        harness.addr(),
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
        unused_local_addr(),
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
        unused_local_addr(),
        || Err(std::io::Error::new(std::io::ErrorKind::NotFound, "找不到執行檔")),
        Duration::from_secs(30),
    )
    .await;

    assert!(matches!(result, Err(ClientError::Spawn(_))));
}
