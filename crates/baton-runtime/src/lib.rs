//! Runtime：唯一控制平面。
//!
//! `Runtime::start` 以程序內方式啟動完整 Runtime（真實 Store、真實 HTTP API），
//! 所有外部副作用都經由注入的 `Ports`。正式執行檔與測試 harness 共用這個入口。

pub mod production_ports;
pub mod single_instance;

use std::net::SocketAddr;

use baton_api::ApiState;
use baton_ports::Ports;
use baton_store::{Store, StoreError};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

pub use baton_domain::config::{self, ConfigError, RuntimeConfig};

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("無法監聽 {0}：{1}")]
    Bind(SocketAddr, std::io::Error),
}

pub struct Runtime;

impl Runtime {
    /// 啟動 Runtime：開啟資料庫、記錄本次啟動、開始提供 HTTP API。
    pub async fn start(config: RuntimeConfig, ports: Ports) -> Result<RunningRuntime, RuntimeError> {
        let store = Store::open(&config.data_dir).await?;
        let started_at = ports.clock.now();
        let boot_count = store.record_boot(started_at).await?;

        let listener = tokio::net::TcpListener::bind(config.bind)
            .await
            .map_err(|e| RuntimeError::Bind(config.bind, e))?;
        let addr = listener.local_addr().map_err(|e| RuntimeError::Bind(config.bind, e))?;

        let app = baton_api::router(ApiState { boot_count, started_at });
        let (stop_tx, stop_rx) = oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            let _store = store; // 與 serve 同生命週期：停止後才釋放資料庫連線
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = stop_rx.await;
                })
                .await;
        });
        Ok(RunningRuntime { addr, stop_tx: Some(stop_tx), task })
    }
}

/// 執行中的 Runtime 實例。
pub struct RunningRuntime {
    addr: SocketAddr,
    stop_tx: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
}

impl RunningRuntime {
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    /// 停止服務並等到資料庫連線釋放；之後可用同一資料目錄重新啟動。
    pub async fn shutdown(mut self) {
        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.send(());
        }
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
