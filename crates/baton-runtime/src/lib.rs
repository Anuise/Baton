//! Runtime：唯一控制平面。
//!
//! `Runtime::start` 以程序內方式啟動完整 Runtime（真實 Store、真實 HTTP API），
//! 所有外部副作用都經由注入的 `Ports`。正式執行檔與測試 harness 共用這個入口。

pub mod credentials;
pub mod log;
pub mod production_ports;
pub mod single_instance;
pub mod windows_platform;

use std::net::{IpAddr, Ipv6Addr, SocketAddr};

use baton_api::ApiState;
use baton_ports::Ports;
use baton_store::{Store, StoreError};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::task::JoinHandle;

pub use baton_domain::config::{self, ConfigError, RuntimeConfig};
pub use credentials::CredentialIssuer;

/// 隨機埠（埠 0）時，IPv6 同埠已被占用就重新挑一個的次數上限。
const BIND_ATTEMPTS: usize = 16;

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("無法監聽 {0}：{1}")]
    Bind(SocketAddr, std::io::Error),
    #[error("監聽位址 {0} 不是 loopback；Runtime 只監聽本機")]
    NotLoopback(SocketAddr),
    #[error("無法取得系統亂數：{0}")]
    Entropy(String),
    #[error("無法交付桌面憑證：{0}")]
    CredentialFile(#[from] baton_credential::CredentialFileError),
    #[error("無法開啟日誌：{0}")]
    Log(std::io::Error),
}

pub struct Runtime;

impl Runtime {
    /// 啟動 Runtime：開啟資料庫、輪替桌面憑證並交付給 App、開始提供 HTTP API。
    ///
    /// 桌面憑證在開始監聽前就已換新並寫入保護檔，所以 App 一旦連得上，讀到的就是目前有效的憑證。
    /// 任何一步失敗都讓啟動失敗，不會退回免驗證的模式。
    pub async fn start(config: RuntimeConfig, ports: Ports) -> Result<RunningRuntime, RuntimeError> {
        if !config.bind.ip().is_loopback() {
            return Err(RuntimeError::NotLoopback(config.bind));
        }
        let started_at = ports.clock.now();
        let store = Store::open(&config.data_dir, started_at).await?;
        let boot_count = store.record_boot(started_at).await?;
        let log = log::open_log(&config.data_dir, ports.clock.clone()).map_err(RuntimeError::Log)?;

        let credentials = CredentialIssuer::new(store.clone(), ports.clock.clone());
        let desktop_token = credentials.rotate_desktop().await?;
        baton_credential::write_desktop_credential(&config.data_dir, &*ports.secret_protector, &desktop_token)?;

        let (v4, v6) = bind_loopback(config.bind).await?;
        let addr = v4.local_addr().map_err(|e| RuntimeError::Bind(config.bind, e))?;
        let addr_v6 = v6.as_ref().and_then(|l| l.local_addr().ok());
        (log)(&format!("Runtime 啟動：第 {boot_count} 次，監聽 {addr}{}，桌面憑證已輪替", {
            addr_v6.map(|a| format!(" 與 {a}")).unwrap_or_default()
        }));

        let (stop_tx, stop_rx) = watch::channel(false);
        let app = baton_api::router(ApiState {
            boot_count,
            started_at,
            allowed_port: addr.port(),
            store: store.clone(),
            clock: ports.clock.clone(),
            platform_info: ports.platform_info.clone(),
            workspace_fs: ports.workspace_fs.clone(),
            log,
            shutdown: stop_rx.clone(),
        });
        let task = tokio::spawn(async move {
            let _store = store; // 與 serve 同生命週期：停止後才釋放資料庫連線
            let serving_v6 = async {
                if let Some(listener) = v6 {
                    serve(listener, app.clone(), stop_rx.clone()).await;
                }
            };
            tokio::join!(serve(v4, app.clone(), stop_rx.clone()), serving_v6);
        });
        Ok(RunningRuntime { addr, addr_v6, credentials, stop_tx, task })
    }
}

async fn serve(listener: TcpListener, app: axum::Router, mut stop: watch::Receiver<bool>) {
    let _ = axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = stop.wait_for(|stopping| *stopping).await;
        })
        .await;
}

/// 綁定 IPv4 與 IPv6 loopback（同一個埠）。
///
/// 本機沒有 IPv6 時只服務 IPv4；IPv6 的埠被別的程序占用則視為錯誤（不可悄悄讓出那個位址）。
async fn bind_loopback(bind: SocketAddr) -> Result<(TcpListener, Option<TcpListener>), RuntimeError> {
    let v4_addr = SocketAddr::from(([127, 0, 0, 1], bind.port()));
    for attempt in 1..=BIND_ATTEMPTS {
        let v4 = TcpListener::bind(v4_addr).await.map_err(|e| RuntimeError::Bind(v4_addr, e))?;
        let port = v4.local_addr().map_err(|e| RuntimeError::Bind(v4_addr, e))?.port();
        let v6_addr = SocketAddr::from((IpAddr::V6(Ipv6Addr::LOCALHOST), port));
        match TcpListener::bind(v6_addr).await {
            Ok(v6) => return Ok((v4, Some(v6))),
            // 隨機埠只是剛好在 IPv6 上被占用：換一個埠再試
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse && bind.port() == 0 && attempt < BIND_ATTEMPTS => {}
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => return Err(RuntimeError::Bind(v6_addr, e)),
            Err(_) => return Ok((v4, None)),
        }
    }
    unreachable!("最後一次嘗試一定會在迴圈內回傳")
}

/// 執行中的 Runtime 實例。
pub struct RunningRuntime {
    addr: SocketAddr,
    addr_v6: Option<SocketAddr>,
    credentials: CredentialIssuer,
    stop_tx: watch::Sender<bool>,
    task: JoinHandle<()>,
}

impl RunningRuntime {
    /// IPv4 loopback 的監聽位址。
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    /// IPv6 loopback 的監聽位址；本機沒有 IPv6 時為 `None`。
    pub fn local_addr_v6(&self) -> Option<SocketAddr> {
        self.addr_v6
    }

    /// 簽發憑證的入口（Agent 憑證由後續的票使用）。
    pub fn credentials(&self) -> &CredentialIssuer {
        &self.credentials
    }

    /// 停止服務並等到資料庫連線釋放；之後可用同一資料目錄重新啟動。
    pub async fn shutdown(mut self) {
        let _ = self.stop_tx.send(true);
        let _ = (&mut self.task).await;
    }

    /// 等到服務自行結束（正式執行檔用）。
    pub async fn wait(mut self) {
        let _ = (&mut self.task).await;
    }
}

impl Drop for RunningRuntime {
    fn drop(&mut self) {
        // 沒有明確 shutdown 就被丟棄時，直接中止服務，避免殘留背景工作
        self.task.abort();
    }
}
