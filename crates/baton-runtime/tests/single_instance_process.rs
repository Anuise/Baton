//! 以真實的 baton-runtime 執行檔驗證單實例：App 啟動 Runtime、再次連回同一個、
//! 第二個 Runtime 因取不到具名 mutex 而自行結束且不影響第一個。

use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::path::Path;
use std::process::Child;
use std::time::Duration;

use baton_client::{ensure_runtime, fetch_health, spawn_runtime_process, Connection};
use baton_runtime::config::{ENV_DATA_DIR, ENV_PORT};

const RUNTIME_EXE: &str = env!("CARGO_BIN_EXE_baton-runtime");

/// 測試結束時確保殘留的 Runtime 程序被終止。
struct KillOnDrop(Option<Child>);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        if let Some(child) = &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn unused_local_addr() -> SocketAddr {
    TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap().local_addr().unwrap()
}

fn envs(data_dir: &Path, addr: SocketAddr) -> Vec<(String, String)> {
    vec![
        (ENV_DATA_DIR.to_string(), data_dir.display().to_string()),
        (ENV_PORT.to_string(), addr.port().to_string()),
    ]
}

#[tokio::test]
async fn app_starts_runtime_then_reconnects_to_the_same_one() {
    let data_dir = tempfile::tempdir().unwrap();
    let addr = unused_local_addr();
    let env = envs(data_dir.path(), addr);
    let mut spawned = KillOnDrop(None);

    let (first, health) = ensure_runtime(
        addr,
        || {
            spawned.0 = Some(spawn_runtime_process(Path::new(RUNTIME_EXE), &env)?);
            Ok(())
        },
        Duration::from_secs(20),
    )
    .await
    .unwrap();
    assert_eq!(first, Connection::Started);
    assert_eq!(health.boot_count, 1);

    let (second, health) = ensure_runtime(
        addr,
        || panic!("已啟動時不應再啟動 Runtime"),
        Duration::from_secs(1),
    )
    .await
    .unwrap();
    assert_eq!(second, Connection::AlreadyRunning);
    assert_eq!(health.boot_count, 1);
}

#[tokio::test]
async fn second_runtime_on_same_data_dir_exits_and_leaves_the_first_untouched() {
    let data_dir = tempfile::tempdir().unwrap();
    let addr = unused_local_addr();
    let mut first = KillOnDrop(None);
    ensure_runtime(
        addr,
        || {
            first.0 = Some(spawn_runtime_process(Path::new(RUNTIME_EXE), &envs(data_dir.path(), addr))?);
            Ok(())
        },
        Duration::from_secs(20),
    )
    .await
    .unwrap();

    // 同一個資料目錄、不同的埠：只有 mutex 能擋下第二個
    let other_addr = unused_local_addr();
    let mut second = spawn_runtime_process(Path::new(RUNTIME_EXE), &envs(data_dir.path(), other_addr)).unwrap();
    let status = tokio::task::spawn_blocking(move || second.wait()).await.unwrap().unwrap();

    assert!(status.success(), "第二個 Runtime 應該正常結束");
    assert!(fetch_health(other_addr).await.is_err(), "第二個 Runtime 不應開始服務");
    assert_eq!(fetch_health(addr).await.unwrap().boot_count, 1, "第一個 Runtime 不受影響");
}
