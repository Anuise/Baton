//! Runtime harness：主要測試 seam（spec D-04）。
//!
//! 在測試中以程序內方式啟動完整 Runtime（真實 Domain、真實 Store 與暫存目錄中的 SQLite、
//! 真實 Axum HTTP API），只替換 Port。`restart` 丟棄 Runtime 實例、保留資料目錄後重建，
//! 並沿用同一組 fake，用來模擬重啟。
//!
//! Supervisor 不在背景自行運轉：測試呼叫 `supervise` 才讓它處理一次，時間則由 fake Clock 決定。
//!
//! `get_json` 與桌面 App 一樣：每次從憑證檔讀取目前的桌面憑證再帶上標頭，
//! 所以重啟後會自動使用新憑證；要測試拒絕情境時改用 `request` 自行組裝請求。

pub mod fakes;

use std::ops::Deref;
use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use baton_api::{tool_path, CLIENT_HEADER, CLIENT_HEADER_VALUE};
use baton_domain::agent::AGENT_TOKEN_ENV;
use baton_domain::auth::Role;
use baton_domain::Timestamp;
use baton_ports::Ports;
use baton_runtime::{RunningRuntime, Runtime, RuntimeConfig, RuntimeError, RuntimeOptions};
use serde::de::DeserializeOwned;
use tempfile::TempDir;

pub use fakes::FakePorts;

/// 預設的 fake 時鐘起點（任意固定值，避免與真實時間混淆）。
const DEFAULT_CLOCK_START: Timestamp = Timestamp::from_millis(1_000_000_000_000);

/// 在 Runtime 第一次啟動之前整理資料目錄的函式（例如放入較新版本的資料庫）。
type PrepareDataDir = Box<dyn FnOnce(&Path)>;

pub struct HarnessBuilder {
    clock_start: Timestamp,
    prepare: Option<PrepareDataDir>,
}

impl HarnessBuilder {
    pub fn clock_at(mut self, start: Timestamp) -> Self {
        self.clock_start = start;
        self
    }

    /// Runtime 第一次啟動之前，先對資料目錄做準備。
    pub fn prepare_data_dir(mut self, prepare: impl FnOnce(&Path) + 'static) -> Self {
        self.prepare = Some(Box::new(prepare));
        self
    }

    pub async fn start(self) -> Harness {
        self.try_start().await.unwrap_or_else(|e| panic!("Runtime 啟動失敗：{e}"))
    }

    /// 啟動；Runtime 拒絕啟動時回傳錯誤（測試「啟動被拒絕」的情境用）。
    pub async fn try_start(self) -> Result<Harness, RuntimeError> {
        let data_dir = tempfile::tempdir().expect("無法建立暫存資料目錄");
        if let Some(prepare) = self.prepare {
            prepare(data_dir.path());
        }
        let fakes = FakePorts::new(self.clock_start);
        let runtime = try_start_runtime(data_dir.path(), &fakes.to_ports()).await?;
        Ok(Harness { data_dir, fakes, runtime: Some(runtime), http: reqwest::Client::new() })
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
        HarnessBuilder { clock_start: DEFAULT_CLOCK_START, prepare: None }
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

    /// IPv6 loopback 的監聽位址；本機沒有 IPv6 時為 `None`。
    pub fn addr_v6(&self) -> Option<SocketAddr> {
        self.runtime.as_ref().expect("Runtime 尚未啟動").local_addr_v6()
    }

    /// Runtime 目前監聽的全部位址。
    pub fn local_addrs(&self) -> Vec<SocketAddr> {
        std::iter::once(self.addr()).chain(self.addr_v6()).collect()
    }

    /// 目前 Runtime 的 URL。
    pub fn url(&self, path: &str) -> String {
        let addr = self.runtime.as_ref().expect("Runtime 尚未啟動").local_addr();
        format!("http://{addr}{path}")
    }

    /// 讀取目前的桌面憑證，方式與桌面 App 相同：讀取受保護的憑證檔。
    pub fn desktop_credential(&self) -> String {
        baton_credential::read_desktop_credential(self.data_dir.path(), &*self.fakes.secret_protector)
            .expect("無法讀取桌面憑證檔")
    }

    /// 簽發額外的憑證（例如 Agent 角色或會過期的憑證），回傳 token。
    pub async fn issue_credential(&self, role: Role, ttl: Option<Duration>) -> String {
        let runtime = self.runtime.as_ref().expect("Runtime 尚未啟動");
        runtime.credentials().issue(role, ttl).await.expect("無法簽發憑證")
    }

    /// 這個 Run 的 Worker 啟動時收到的 Agent 憑證：取自 fake ProcessControl 記錄的啟動環境變數，
    /// 與真實 Worker 的 MCP shim 拿到的是同一個值；Run 還沒有被啟動時為 `None`。
    pub fn agent_credential(&self, run_id: &str) -> Option<String> {
        self.fakes
            .process_control
            .spawned()
            .into_iter()
            .find(|spec| spec.args.first().map(String::as_str) == Some(run_id))
            .and_then(|spec| spec.env.into_iter().find(|(name, _)| name == AGENT_TOKEN_ENV))
            .map(|(_, token)| token)
    }

    /// Runtime 的 loopback 位址（MCP shim 的 `BATON_RUNTIME_URL`）。
    pub fn runtime_url(&self) -> String {
        format!("http://{}", self.addr())
    }

    /// 以給定的 bearer 憑證呼叫 Agent 工具（與 MCP shim 相同的請求），回傳狀態碼與 JSON 本文；
    /// 不論成功與否都不會讓測試失敗。
    pub async fn call_tool(
        &self,
        token: &str,
        tool: &str,
        body: serde_json::Value,
    ) -> (reqwest::StatusCode, serde_json::Value) {
        let response = self
            .request(reqwest::Method::POST, &tool_path(tool))
            .bearer_auth(token)
            .header(CLIENT_HEADER, CLIENT_HEADER_VALUE)
            .json(&body)
            .send()
            .await
            .expect("HTTP 請求失敗");
        let status = response.status();
        let bytes = response.bytes().await.expect("無法讀取回應");
        (status, serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null))
    }

    /// 日誌檔的全部內容；日誌尚未建立時為空字串。
    pub fn log_text(&self) -> String {
        std::fs::read_to_string(baton_runtime::log::log_path(self.data_dir.path())).unwrap_or_default()
    }

    /// 對目前 Runtime 組一個沒有任何驗證標頭的原始請求；測試自行決定要帶什麼。
    pub fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.http.request(method, self.url(path))
    }

    /// 對目前 Runtime 發出真實 HTTP GET 並解析 JSON；帶桌面憑證，非 2xx 視為測試失敗。
    pub async fn get_json<T: DeserializeOwned>(&self, path: &str) -> T {
        let response = self
            .request(reqwest::Method::GET, path)
            .bearer_auth(self.desktop_credential())
            .header(CLIENT_HEADER, CLIENT_HEADER_VALUE)
            .send()
            .await
            .expect("HTTP 請求失敗");
        let status = response.status();
        assert!(status.is_success(), "GET {path} 回應 {status}");
        response.json().await.expect("回應不是預期的 JSON")
    }

    /// 對目前 Runtime 發出已帶桌面憑證的請求，回傳狀態碼與 JSON 本文（無本文時為 `Null`）；
    /// 不論成功與否都不會讓測試失敗，由測試自行斷言。
    pub async fn send_json(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> (reqwest::StatusCode, serde_json::Value) {
        let mut request = self
            .request(method, path)
            .bearer_auth(self.desktop_credential())
            .header(CLIENT_HEADER, CLIENT_HEADER_VALUE);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.expect("HTTP 請求失敗");
        let status = response.status();
        let bytes = response.bytes().await.expect("無法讀取回應");
        (status, serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null))
    }

    /// 建立一個可派工的帳號 Profile：在它自己的設定目錄完成（fake 的）原生登入並核對，回傳 Profile 識別。
    /// `account` 是登入的電子郵件；同一個帳號不能綁兩個 Profile，所以每個 Profile 要用不同的帳號。
    pub async fn add_account(&self, name: &str, account: &str) -> String {
        let (status, created) =
            self.send_json(reqwest::Method::POST, "/api/v1/profiles", Some(serde_json::json!({ "name": name }))).await;
        assert!(status.is_success(), "建立 Profile 失敗：{created}");
        let id = created["id"].as_str().expect("Profile 回應沒有 id").to_string();
        let config_dir = created["config_dir"].as_str().expect("Profile 回應沒有 config_dir");
        self.fakes.claude_env.log_in(Path::new(config_dir), account);
        let (status, verified) =
            self.send_json(reqwest::Method::POST, &format!("/api/v1/profiles/{id}/verify"), None).await;
        assert!(status.is_success(), "核對 Profile 失敗：{verified}");
        assert_eq!(verified["dispatchable"], true, "Profile 應該可派工：{verified}");
        id
    }

    /// 以唯讀方式直接開啟控制資料庫，檢查實際落地的內容（不經過 Runtime）。
    pub fn db(&self) -> rusqlite::Connection {
        rusqlite::Connection::open_with_flags(
            self.data_dir.path().join(baton_store::DB_FILE_NAME),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .expect("無法開啟控制資料庫")
    }

    /// 讓 Supervisor 處理目前所有待辦一次：啟動派工 Outbox 裡的 Run、收 Adapter 事件、處理取消、核對過期的 lease。
    /// Harness 不啟動 Supervisor 的背景迴圈，所以每一步都由測試決定何時發生（搭配 fake Clock 就完全決定性）。
    pub async fn supervise(&self) {
        self.runtime.as_ref().expect("Runtime 尚未啟動").supervisor().tick().await;
    }

    /// 模擬重啟：丟棄目前的 Runtime，保留資料目錄與 fake，重新建立。
    pub async fn restart(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown().await;
        }
        let runtime = try_start_runtime(self.data_dir.path(), &self.fakes.to_ports()).await;
        self.runtime = Some(runtime.unwrap_or_else(|e| panic!("Runtime 重啟失敗：{e}")));
    }
}

async fn try_start_runtime(data_dir: &Path, ports: &Ports) -> Result<RunningRuntime, RuntimeError> {
    let config = RuntimeConfig {
        data_dir: data_dir.to_path_buf(),
        // 埠 0 讓作業系統配置；每個 Harness 與每次重建都不會互相衝突
        bind: "127.0.0.1:0".parse().expect("固定位址"),
    };
    // 測試自己決定何時讓 Supervisor 動作（`Harness::supervise`），所以不啟動背景迴圈
    Runtime::start_with(config, ports.clone(), RuntimeOptions { background_supervisor: false, ..RuntimeOptions::default() }).await
}
