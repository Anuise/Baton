//! 以真實的 baton-runtime 執行檔驗證單實例：App 啟動 Runtime、再次連回同一個、
//! 第二個 Runtime 因取不到具名 mutex 而自行結束且不影響第一個。

use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::path::Path;
use std::process::Child;
use std::sync::Arc;
use std::time::Duration;

use baton_client::{ensure_runtime, spawn_runtime_process, Connection, RuntimeClient};
use baton_credential::DpapiProtector;
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

/// 與桌面 App 相同：以真實 DPAPI 讀取憑證檔後連線。
fn app_client(data_dir: &Path, addr: SocketAddr) -> RuntimeClient {
    RuntimeClient::new(addr, data_dir.to_path_buf(), Arc::new(DpapiProtector))
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
    let client = app_client(data_dir.path(), addr);

    let (first, health) = ensure_runtime(
        &client,
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
        &client,
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
        &app_client(data_dir.path(), addr),
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
    assert!(app_client(data_dir.path(), other_addr).health().await.is_err(), "第二個 Runtime 不應開始服務");
    assert_eq!(app_client(data_dir.path(), addr).health().await.unwrap().boot_count, 1, "第一個 Runtime 不受影響");
}

#[tokio::test]
async fn real_runtime_restart_rotates_credential_and_app_reconnects_via_dpapi_file() {
    let data_dir = tempfile::tempdir().unwrap();
    let addr = unused_local_addr();
    let client = app_client(data_dir.path(), addr);
    let mut runtime = KillOnDrop(None);
    let start = |slot: &mut KillOnDrop| {
        slot.0 = Some(spawn_runtime_process(Path::new(RUNTIME_EXE), &envs(data_dir.path(), addr))?);
        Ok(())
    };

    ensure_runtime(&client, || start(&mut runtime), Duration::from_secs(20)).await.unwrap();
    let first_token = baton_credential::read_desktop_credential(data_dir.path(), &DpapiProtector).unwrap();
    // 停掉第一個 Runtime（模擬重啟）：等程序結束，具名 mutex 才會釋放
    drop(std::mem::replace(&mut runtime, KillOnDrop(None)));

    let (connection, health) =
        ensure_runtime(&client, || start(&mut runtime), Duration::from_secs(20)).await.unwrap();
    let second_token = baton_credential::read_desktop_credential(data_dir.path(), &DpapiProtector).unwrap();

    assert_eq!(connection, Connection::Started);
    assert_eq!(health.boot_count, 2);
    assert_ne!(first_token, second_token, "每次啟動都輪替桌面憑證");
    // 憑證檔是 DPAPI 密文，日誌也沒有任何憑證
    let file = std::fs::read(baton_credential::desktop_credential_path(data_dir.path())).unwrap();
    let log = std::fs::read_to_string(baton_runtime::log::log_path(data_dir.path())).unwrap();
    for token in [&first_token, &second_token] {
        assert!(!file.windows(token.len()).any(|w| w == token.as_bytes()));
        assert!(!log.contains(token.as_str()));
    }
}
