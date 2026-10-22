//! Runtime harness：主要測試 seam（spec D-04）。
//!
//! 在測試中以程序內方式啟動完整 Runtime（真實 Domain、真實 Store 與暫存目錄中的 SQLite、
//! 真實 Axum HTTP API），只替換 Port。`restart` 丟棄 Runtime 實例、保留資料目錄後重建，
//! 並沿用同一組 fake，用來模擬重啟。

pub mod fakes;

use std::ops::Deref;
use std::path::Path;

use baton_domain::Timestamp;
use baton_ports::Ports;
use baton_runtime::{RunningRuntime, Runtime, RuntimeConfig};
use serde::de::DeserializeOwned;
use tempfile::TempDir;

pub use fakes::FakePorts;

/// 預設的 fake 時鐘起點（任意固定值，避免與真實時間混淆）。
const DEFAULT_CLOCK_START: Timestamp = Timestamp::from_millis(1_000_000_000_000);

pub struct HarnessBuilder {
    clock_start: Timestamp,
}

impl HarnessBuilder {
    pub fn clock_at(mut self, start: Timestamp) -> Self {
        self.clock_start = start;
        self
    }

    pub async fn start(self) -> Harness {
        let data_dir = tempfile::tempdir().expect("無法建立暫存資料目錄");
        let fakes = FakePorts::new(self.clock_start);
        let runtime = start_runtime(data_dir.path(), &fakes.to_ports()).await;
        Harness { data_dir, fakes, runtime: Some(runtime), http: reqwest::Client::new() }
    }
}

pub struct Harness {
    data_dir: TempDir,
    fakes: FakePorts,
    runtime: Option<RunningRuntime>,
    http: reqwest::Client,
}

/// 讓測試可用 `harness.clock`、`harness.process_control` 等直接取得 fake。
impl Deref for Harness {
    type Target = FakePorts;

    fn deref(&self) -> &FakePorts {
        &self.fakes
    }
}

impl Harness {
    pub fn builder() -> HarnessBuilder {
        HarnessBuilder { clock_start: DEFAULT_CLOCK_START }
    }

    /// 以預設設定啟動。
    pub async fn start() -> Self {
        Self::builder().start().await
    }

    /// 資料目錄；重建 Runtime 時保留。
    pub fn data_dir(&self) -> &Path {
        self.data_dir.path()
    }

    /// 目前注入給 Runtime 的 Port（同一批 fake）。
    pub fn ports(&self) -> Ports {
        self.fakes.to_ports()
    }

    /// 目前 Runtime 的監聽位址。
    pub fn addr(&self) -> std::net::SocketAddr {
        self.runtime.as_ref().expect("Runtime 尚未啟動").local_addr()
    }

    /// 目前 Runtime 的 URL。
    pub fn url(&self, path: &str) -> String {
        let addr = self.runtime.as_ref().expect("Runtime 尚未啟動").local_addr();
        format!("http://{addr}{path}")
    }

    /// 對目前 Runtime 發出真實 HTTP GET 並解析 JSON；非 2xx 視為測試失敗。
    pub async fn get_json<T: DeserializeOwned>(&self, path: &str) -> T {
        let response = self.http.get(self.url(path)).send().await.expect("HTTP 請求失敗");
        let status = response.status();
        assert!(status.is_success(), "GET {path} 回應 {status}");
        response.json().await.expect("回應不是預期的 JSON")
    }

    /// 模擬重啟：丟棄目前的 Runtime，保留資料目錄與 fake，重新建立。
    pub async fn restart(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown().await;
        }
        self.runtime = Some(start_runtime(self.data_dir.path(), &self.fakes.to_ports()).await);
    }
}

async fn start_runtime(data_dir: &Path, ports: &Ports) -> RunningRuntime {
    let config = RuntimeConfig {
        data_dir: data_dir.to_path_buf(),
        // 埠 0 讓作業系統配置；每個 Harness 與每次重建都不會互相衝突
        bind: "127.0.0.1:0".parse().expect("固定位址"),
    };
    Runtime::start(config, ports.clone()).await.expect("Runtime 啟動失敗")
}
