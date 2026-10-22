//! 以真實的 baton-runtime 執行檔驗證登入啟動（票 33）：
//! - 登入啟動與使用者手動開 App 同時發生時，只有一個 Runtime 在服務，多出來的那個自行結束。
//! - 資料庫有停止意圖、生命週期檔卻不見時，登入啟動的 Runtime 不提供服務、以失敗碼結束，並把停止意圖補回生命週期檔。

use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::path::Path;
use std::process::{Child, ExitStatus};
use std::sync::Arc;
use std::time::{Duration, Instant};

use baton_client::{ensure_runtime, spawn_runtime_process, RuntimeClient};
use baton_credential::DpapiProtector;
use baton_domain::lifecycle::LIFECYCLE_FILE_NAME;
use baton_domain::login::{START_SOURCE_ENV, START_SOURCE_LOGIN};
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

impl KillOnDrop {
    /// 程序已結束時回傳結束碼（不會終止它）。
    fn exited(&mut self) -> Option<ExitStatus> {
        self.0.as_mut().and_then(|child| child.try_wait().unwrap())
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

/// 登入啟動入口拉起 Runtime 時的環境（多了啟動來源）。
fn login_envs(data_dir: &Path, addr: SocketAddr) -> Vec<(String, String)> {
    let mut envs = envs(data_dir, addr);
    envs.push((START_SOURCE_ENV.to_string(), START_SOURCE_LOGIN.to_string()));
    envs
}

#[tokio::test]
async fn login_start_racing_the_app_leaves_exactly_one_runtime_serving() {
    let data_dir = tempfile::tempdir().unwrap();
    let addr = unused_local_addr();
    let client = app_client(data_dir.path(), addr);

    // 登入啟動與 App 同時啟動同一個資料目錄、同一個埠的 Runtime
    let mut login = KillOnDrop(Some(spawn_runtime_process(Path::new(RUNTIME_EXE), &login_envs(data_dir.path(), addr)).unwrap()));
    let mut app = KillOnDrop(None);
    let (_, health) = ensure_runtime(
        &client,
        || {
            app.0 = Some(spawn_runtime_process(Path::new(RUNTIME_EXE), &envs(data_dir.path(), addr))?);
            Ok(())
        },
        Duration::from_secs(20),
    )
    .await
    .unwrap();
    assert_eq!(health.boot_count, 1, "只有一個 Runtime 開始服務");

    // App 一開始就連上登入啟動的 Runtime 時不會再啟動；否則多出來的那個應自行結束
    if app.0.is_some() {
        let deadline = Instant::now() + Duration::from_secs(20);
        let extra = loop {
            match (login.exited(), app.exited()) {
                (Some(status), None) | (None, Some(status)) => break status,
                (Some(_), Some(_)) => panic!("兩個 Runtime 都結束了"),
                (None, None) if Instant::now() < deadline => tokio::time::sleep(Duration::from_millis(100)).await,
                (None, None) => panic!("多出來的 Runtime 沒有自行結束"),
            }
        };
        assert!(extra.success(), "多出來的 Runtime 應以成功碼結束");
    }
    assert_eq!(client.health().await.unwrap().boot_count, 1, "留下的 Runtime 繼續服務");
}

#[tokio::test]
async fn login_start_refuses_to_serve_when_only_the_database_remembers_the_full_exit() {
    let data_dir = tempfile::tempdir().unwrap();
    let addr = unused_local_addr();
    let client = app_client(data_dir.path(), addr);

    // 使用者手動開 App 後完全退出：停止意圖寫入資料庫與生命週期檔
    let mut app = KillOnDrop(None);
    ensure_runtime(
        &client,
        || {
            app.0 = Some(spawn_runtime_process(Path::new(RUNTIME_EXE), &envs(data_dir.path(), addr))?);
            Ok(())
        },
        Duration::from_secs(20),
    )
    .await
    .unwrap();
    client.request_exit().await.unwrap();
    // 停止流程結束後還會多提供約 5 秒 API，才自行結束
    let deadline = Instant::now() + Duration::from_secs(60);
    let status = loop {
        if let Some(status) = app.exited() {
            break status;
        }
        assert!(Instant::now() < deadline, "完全退出後 Runtime 沒有自行結束");
        tokio::time::sleep(Duration::from_millis(200)).await;
    };
    assert!(status.success(), "完全退出後應以成功碼結束");

    // 生命週期檔寫入失敗的情形：只剩資料庫記得停止意圖
    let lifecycle_path = data_dir.path().join(LIFECYCLE_FILE_NAME);
    assert!(std::fs::read_to_string(&lifecycle_path).unwrap().contains("\"full_exit\""));
    std::fs::remove_file(&lifecycle_path).unwrap();

    let mut login = KillOnDrop(Some(spawn_runtime_process(Path::new(RUNTIME_EXE), &login_envs(data_dir.path(), addr)).unwrap()));
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = login.exited() {
            break status;
        }
        assert!(client.health().await.is_err(), "登入啟動的 Runtime 不應提供服務");
        assert!(Instant::now() < deadline, "登入啟動的 Runtime 沒有結束");
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    assert!(!status.success(), "停止意圖仍生效時登入啟動的 Runtime 應以失敗碼結束");
    assert!(client.health().await.is_err());

    let restored = std::fs::read_to_string(&lifecycle_path).expect("生命週期檔應被補回");
    assert!(restored.contains("\"full_exit\""), "補回的生命週期檔應含停止意圖：{restored}");
}
