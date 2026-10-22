//! 桌面 App 的 Runtime 用戶端：連回既有的 Runtime，沒有就啟動一個。
//!
//! 單實例由 Runtime 本身用具名 mutex 保證；這裡先連線、連不上才啟動，
//! 即使兩個 App 同時啟動，多出來的 Runtime 也會因取不到 mutex 而自行結束。

use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use baton_api::{HealthResponse, PlatformResponse, HEALTH_PATH, PLATFORM_PATH};

/// 連線方式：連回既有的，或由本次呼叫啟動。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Connection {
    AlreadyRunning,
    Started,
}

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("無法連線到 Runtime：{0}")]
    Unreachable(String),
    #[error("無法啟動 Runtime：{0}")]
    Spawn(std::io::Error),
    #[error("Runtime 在 {0:?} 內沒有就緒")]
    StartTimeout(Duration),
}

/// 查詢 Runtime 的健康狀態。
pub async fn fetch_health(addr: SocketAddr) -> Result<HealthResponse, ClientError> {
    get_json(addr, HEALTH_PATH).await
}

/// 查詢 Runtime 的平台狀態。
pub async fn fetch_platform(addr: SocketAddr) -> Result<PlatformResponse, ClientError> {
    get_json(addr, PLATFORM_PATH).await
}

async fn get_json<T: serde::de::DeserializeOwned>(addr: SocketAddr, path: &str) -> Result<T, ClientError> {
    let unreachable = |e: reqwest::Error| ClientError::Unreachable(e.to_string());
    // Windows 連到未監聽的 loopback 埠要等約 2 秒才失敗；連線逾時設短一點，連不上時才能快速判斷
    reqwest::Client::builder()
        .connect_timeout(Duration::from_millis(300))
        .build()
        .map_err(unreachable)?
        .get(format!("http://{addr}{path}"))
        .timeout(Duration::from_secs(2))
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(unreachable)?
        .json()
        .await
        .map_err(unreachable)
}

/// 確保 Runtime 可用：已在執行就連回，否則呼叫 `start` 啟動並等到就緒（最多 `wait`）。
pub async fn ensure_runtime(
    addr: SocketAddr,
    start: impl FnOnce() -> std::io::Result<()>,
    wait: Duration,
) -> Result<(Connection, HealthResponse), ClientError> {
    if let Ok(health) = fetch_health(addr).await {
        return Ok((Connection::AlreadyRunning, health));
    }

    start().map_err(ClientError::Spawn)?;

    let deadline = tokio::time::Instant::now() + wait;
    while tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if let Ok(health) = fetch_health(addr).await {
            return Ok((Connection::Started, health));
        }
    }
    Err(ClientError::StartTimeout(wait))
}

/// 以脫離本程序的方式啟動 Runtime 執行檔；關閉 App 後 Runtime 仍會繼續執行。
pub fn spawn_runtime_process(exe: &Path, envs: &[(String, String)]) -> std::io::Result<std::process::Child> {
    use std::process::{Command, Stdio};

    let mut command = Command::new(exe);
    command.envs(envs.iter().map(|(k, v)| (k, v))).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
    }
    command.spawn()
}
