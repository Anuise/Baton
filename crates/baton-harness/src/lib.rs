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
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use baton_api::{tool_path, CLIENT_HEADER, CLIENT_HEADER_VALUE};
use baton_domain::agent::AGENT_TOKEN_ENV;
use baton_domain::auth::Role;
use baton_domain::guardian::JournalRecord;
use baton_domain::lifecycle::ExitTiming;
use baton_domain::Timestamp;
use baton_ports::{AgentAdapter, BrowserDriver, Ports, VerificationDriver};
use baton_guardian::launcher::{self, LoginLaunch};
use baton_guardian::{Guardian, GuardianStep, RuntimeHost};
use baton_runtime::{RunningRuntime, Runtime, RuntimeConfig, RuntimeError, RuntimeOptions};
use serde::de::DeserializeOwned;
use tempfile::TempDir;

pub use fakes::FakePorts;

/// Harness 中 Runtime 的預設版本（與正式建置的版本無關，測試以它比較更新）。
pub const DEFAULT_APP_VERSION: &str = "0.1.0";

/// 預設的 fake 時鐘起點（任意固定值，避免與真實時間混淆）。
const DEFAULT_CLOCK_START: Timestamp = Timestamp::from_millis(1_000_000_000_000);

/// 在 Runtime 第一次啟動之前整理資料目錄的函式（例如放入較新版本的資料庫）。
type PrepareDataDir = Box<dyn FnOnce(&Path)>;

/// 以資料目錄建立 Agent Adapter 的函式（例如真實的 Claude Code Adapter，其 Run 檔案放在資料目錄下）。
type AdapterFactory = Box<dyn FnOnce(&Path) -> Arc<dyn AgentAdapter>>;

pub struct HarnessBuilder {
    clock_start: Timestamp,
    prepare: Option<PrepareDataDir>,
    adapter: Option<AdapterFactory>,
    verification_driver: Option<Arc<dyn VerificationDriver>>,
    browser_driver: Option<Arc<dyn BrowserDriver>>,
    exit_timing: ExitTiming,
    full_text_search: bool,
    exit_linger: Option<Duration>,
}

impl HarnessBuilder {
    pub fn clock_at(mut self, start: Timestamp) -> Self {
        self.clock_start = start;
        self
    }

    /// 完全退出的寬限期與強制終止確認上限（預設即 spec D-11 的 30 + 10 秒）；
    /// 測「停不下來」的情境時縮短，免得真的等 40 秒。
    pub fn exit_timing(mut self, timing: ExitTiming) -> Self {
        self.exit_timing = timing;
        self
    }

    /// 完全退出（含更新、解除安裝的停止流程）結束後，Runtime 再提供這麼久的 API 就自行結束服務（與正式執行相同）。
    /// 預設不自行結束；只有要讓「Runtime 之外的程式」（例如解除安裝程式的準備步驟）看到 Runtime 結束時才設定。
    pub fn exit_linger(mut self, linger: Duration) -> Self {
        self.exit_linger = Some(linger);
        self
    }

    /// 視同 FTS5 不可用：知識搜尋退回 LIKE（spec D-26 的降級情境）。之後可用 `Harness::set_full_text_search` 在重啟時切換。
    pub fn without_full_text_search(mut self) -> Self {
        self.full_text_search = false;
        self
    }

    /// Runtime 第一次啟動之前，先對資料目錄做準備。
    pub fn prepare_data_dir(mut self, prepare: impl FnOnce(&Path) + 'static) -> Self {
        self.prepare = Some(Box::new(prepare));
        self
    }

    /// 以指定的 Agent Adapter 取代 fake（其餘 Port 仍是 fake）；重啟時沿用同一個 Adapter。
    pub fn agent_adapter(mut self, factory: impl FnOnce(&Path) -> Arc<dyn AgentAdapter> + 'static) -> Self {
        self.adapter = Some(Box::new(factory));
        self
    }

    /// 以指定的驗證 Adapter Port 取代 fake（例如正式的 `LocalVerificationDriver`，對真實的程序與 HTTP 服務驗證；票 45）。
    pub fn verification_driver(mut self, driver: Arc<dyn VerificationDriver>) -> Self {
        self.verification_driver = Some(driver);
        self
    }

    /// 以指定的瀏覽器 Runner 取代 fake（例如正式的 `LocalBrowserDriver`，以真實的 Playwright 驗證；票 46）。
    pub fn browser_driver(mut self, driver: Arc<dyn BrowserDriver>) -> Self {
        self.browser_driver = Some(driver);
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
        let adapter = self.adapter.map(|factory| factory(data_dir.path()));
        let mut harness = Harness {
            data_dir,
            fakes,
            adapter,
            verification_driver: self.verification_driver,
            browser_driver: self.browser_driver,
            runtime: None,
            http: reqwest::Client::new(),
            exit_timing: self.exit_timing,
            exits_seen: AtomicU64::new(0),
            full_text_search: self.full_text_search,
            guardian: None,
            guardian_script: GuardianScript::default(),
            exit_linger: self.exit_linger,
            app_version: DEFAULT_APP_VERSION.to_string(),
        };
        harness.runtime = Some(harness.launch(StartedBy::App).await?);
        Ok(harness)
    }
}

pub struct Harness {
    data_dir: TempDir,
    fakes: FakePorts,
    /// 取代 fake 的 Agent Adapter；`None` 時用 `fakes.agent_adapter`。
    adapter: Option<Arc<dyn AgentAdapter>>,
    /// 取代 fake 的驗證 Adapter Port；`None` 時用 `fakes.verification_driver`。
    verification_driver: Option<Arc<dyn VerificationDriver>>,
    /// 取代 fake 的瀏覽器 Runner；`None` 時用 `fakes.browser_driver`。
    browser_driver: Option<Arc<dyn BrowserDriver>>,
    runtime: Option<RunningRuntime>,
    http: reqwest::Client,
    exit_timing: ExitTiming,
    /// 目前這個 Runtime 實例已經等到結束的完全退出次數。
    exits_seen: AtomicU64,
    full_text_search: bool,
    /// 程序內的監護（票 32）；`None` 表示沒有監護（從未啟動或已被殺掉）。
    guardian: Option<Guardian>,
    guardian_script: GuardianScript,
    exit_linger: Option<Duration>,
    /// 下一次啟動的 Runtime 的版本（票 34）；`install_version` 模擬安裝程式換上新版本。
    app_version: String,
}

/// 監護啟動 Runtime 的一筆紀錄（票 32）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuardianStart {
    pub attempt_id: String,
    /// `Ok` 是 Runtime 啟動並開始服務；`Err` 是程序啟動後很快就結束（啟動早期失敗、或遇到停止意圖不提供服務），附原因。
    pub outcome: Result<(), String>,
}

/// 登入啟動入口在 Windows 登入時做了什麼（票 33）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginStartupRun {
    pub launch: LoginLaunch,
    /// 登入啟動的 Runtime 程序啟動後的結果：`Ok` 是開始服務；`Err` 是很快就結束（遇到停止意圖不提供服務、
    /// 或已有 Runtime 而因單實例結束），附原因。沒有啟動程序時為 `None`。
    pub runtime_outcome: Option<Result<(), String>>,
    /// 之後這個程序是否接著當監護（有 Runtime 可監看、而且還沒有監護時才會）。
    pub guards: bool,
}

/// 監護與程序內 Runtime 之間的腳本與紀錄。
#[derive(Default)]
struct GuardianScript {
    /// 下一次監護啟動 Runtime 時，程序啟動了卻在匯入監護日誌之前就結束。
    fail_next_start_early: bool,
    /// 下一次監護查詢 Runtime 是否在執行時，先執行這個動作（重現「寫下日誌之後、最後核對之前」的競態）。
    on_next_probe: Option<Box<dyn FnOnce(&Path) + Send>>,
    starts: Vec<GuardianStart>,
    /// 下一次登入啟動入口啟動 Runtime 的同一刻，使用者也手動開了 App（票 33 的競態）。
    app_opens_during_next_login_start: bool,
    /// 最近一次登入啟動的 Runtime 程序的結果。
    login_start_outcome: Option<Result<(), String>>,
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
        HarnessBuilder {
            clock_start: DEFAULT_CLOCK_START,
            prepare: None,
            adapter: None,
            verification_driver: None,
            browser_driver: None,
            exit_timing: ExitTiming::default(),
            full_text_search: true,
            exit_linger: None,
        }
    }

    /// 以預設設定啟動。
    pub async fn start() -> Self {
        Self::builder().start().await
    }

    /// 資料目錄；重建 Runtime 時保留。
    pub fn data_dir(&self) -> &Path {
        self.data_dir.path()
    }

    /// 成果儲存的根目錄（`<資料目錄>/artifacts`）；成果檔案、暫存檔與未登記的檔案都在其下。
    pub fn artifacts_dir(&self) -> std::path::PathBuf {
        self.data_dir.path().join(baton_domain::artifact::ARTIFACTS_DIR)
    }

    /// 目前注入給 Runtime 的 Port（同一批 fake；指定了 Agent Adapter 時換成它）。
    pub fn ports(&self) -> Ports {
        let mut ports = self.fakes.to_ports();
        if let Some(adapter) = &self.adapter {
            ports.agent_adapter = adapter.clone();
        }
        if let Some(driver) = &self.verification_driver {
            ports.verification_driver = driver.clone();
        }
        if let Some(driver) = &self.browser_driver {
            ports.browser_driver = driver.clone();
        }
        ports
    }

    /// 目前 Runtime 的 Store（重建 Runtime 後是新的實例）；故障注入用。
    pub fn store(&self) -> baton_store::Store {
        self.runtime.as_ref().expect("Runtime 尚未啟動").supervisor().store().clone()
    }

    /// 讓目前 Runtime 的每個資料庫寫入都以指定的儲存故障失敗（磁碟滿、被鎖住、無法寫入）；`None` 恢復。
    /// 重建 Runtime 後不再生效。
    pub fn inject_storage_fault(&self, kind: Option<baton_domain::storage_health::StorageFaultKind>) {
        self.store().inject_write_fault(kind);
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

    /// 讓備份排程處理一次：該備份就備份（並套用保留規則）、該演練就做還原演練。時間由 fake Clock 決定。
    pub async fn run_backups(&self) {
        self.runtime.as_ref().expect("Runtime 尚未啟動").backups().tick().await;
    }

    /// 重要裁定或 Stage 完成：要求備份（下一次 `run_backups` 處理）。
    pub fn request_milestone_backup(&self, reason: &str) {
        self.runtime.as_ref().expect("Runtime 尚未啟動").backups().request_milestone_backup(reason);
    }

    /// 備份根目錄（`<資料目錄>/backups`）。
    pub fn backups_dir(&self) -> std::path::PathBuf {
        baton_store::backup::backups_root(self.data_dir.path())
    }

    /// 生命週期檔（停止意圖的鏡像，spec D-25）的位置。
    pub fn lifecycle_file_path(&self) -> std::path::PathBuf {
        baton_store::lifecycle_file_path(self.data_dir.path())
    }

    /// 等到這次完全退出的停止流程結束（`POST /api/v1/lifecycle/exit` 之後呼叫）。
    /// 停止流程在 Runtime 的背景執行；最多等「總上限 + 10 秒」的真實時間，逾時視為測試失敗。
    pub async fn wait_for_exit(&self) {
        let mut exits = self.runtime.as_ref().expect("Runtime 尚未啟動").exits_completed();
        let seen = self.exits_seen.load(Ordering::SeqCst);
        let limit = Duration::from_millis(self.exit_timing.total_ms() as u64) + Duration::from_secs(10);
        let done = tokio::time::timeout(limit, exits.wait_for(|count| *count > seen)).await;
        let count = *done.expect("完全退出的停止流程沒有在時限內結束").expect("Runtime 已停止");
        self.exits_seen.store(count, Ordering::SeqCst);
    }

    /// 下一次 `restart` 是否使用全文檢索（false 視同 FTS5 不可用）。
    pub fn set_full_text_search(&mut self, enabled: bool) {
        self.full_text_search = enabled;
    }

    /// 模擬重啟：有秩序地停止目前的 Runtime（記下正常結束），保留資料目錄與 fake，重新建立。
    pub async fn restart(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown().await;
        }
        self.start_again().await;
    }

    /// 模擬 Runtime 意外終止後再次啟動（票 31）：丟棄目前的 Runtime 而不記下正常結束，保留資料目錄與 fake，重新建立。
    /// 新的 Runtime 會判定上次意外終止並開始崩潰恢復。要在某個邊界「當掉」，先讓那一步永遠不回來
    /// （例如 `FakeStop::Hang`），以逾時丟棄進行中的 `supervise`，再呼叫這個方法。
    pub async fn crash_restart(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            runtime.kill().await;
        }
        self.start_again().await;
    }

    async fn start_again(&mut self) {
        let runtime = self.launch(StartedBy::App).await;
        self.runtime = Some(runtime.unwrap_or_else(|e| panic!("Runtime 重啟失敗：{e}")));
        self.exits_seen.store(0, Ordering::SeqCst);
    }

    /// 以單實例的規則啟動一個 Runtime 程序：已有 Runtime 時新的程序取不到單實例鎖、立即結束（不開資料庫）。
    async fn start_single_instance(&mut self, by: StartedBy) -> Result<(), String> {
        if self.runtime.is_some() {
            return Err("已有 Runtime 在執行（單實例），本程序結束".to_string());
        }
        self.launch(by)
            .await
            .map(|runtime| {
                self.runtime = Some(runtime);
                self.exits_seen.store(0, Ordering::SeqCst);
            })
            .map_err(|e| e.to_string())
    }

    /// 以目前的設定啟動一個 Runtime；回傳的 future 不借用 Harness（監護的 `RuntimeHost` 需要 `Send`）。
    fn launch(&self, by: StartedBy) -> impl std::future::Future<Output = Result<RunningRuntime, RuntimeError>> + Send + 'static {
        let data_dir = self.data_dir.path().to_path_buf();
        let ports = self.ports();
        let launch = Launch {
            exit_timing: self.exit_timing,
            full_text_search: self.full_text_search,
            exit_linger: self.exit_linger,
            app_version: self.app_version.clone(),
        };
        async move { try_start_runtime(&data_dir, &ports, launch, by).await }
    }

    // ------------------------------------------------------------ 更新與解除安裝（票 34）

    /// 讓更新檢查處理一次：該檢查（啟動後第一次、之後每 24 小時）就向 fake 更新來源查詢。時間由 fake Clock 決定。
    pub async fn check_updates(&self) {
        self.runtime.as_ref().expect("Runtime 尚未啟動").updates().tick().await;
    }

    /// 模擬安裝程式把程式換成 `version`：之後啟動的 Runtime 回報這個版本。
    pub fn install_version(&mut self, version: &str) {
        self.app_version = version.to_string();
    }

    // ------------------------------------------------------------ 跨 Windows 登入（票 33）

    /// 模擬登出（或關機）後再次登入 Windows：同一次登入期間的程序全部結束（Runtime 沒有記下正常結束、監護也不在），
    /// 新的一次登入有新的登入識別。之後由 `windows_login_startup`（登入啟動）或 `open_app`（手動開 App）決定是否再啟動。
    pub async fn sign_in_again(&mut self) -> String {
        self.kill_runtime().await;
        self.kill_guardian();
        self.fakes.login_session.sign_in_again()
    }

    /// 模擬 Windows 登入後執行登入啟動登記（HKCU Run）的項目：沒有登記時什麼都不執行（回傳 `None`）；
    /// 有登記時執行登入啟動入口（`baton-guardian.exe --login`），之後有 Runtime 可監看就接著當監護。
    pub async fn windows_login_startup(&mut self) -> Option<LoginStartupRun> {
        if !self.fakes.startup_registration.registered() {
            return None;
        }
        self.guardian_script.login_start_outcome = None;
        let clock = self.fakes.clock.clone();
        let data_dir = self.data_dir.path().to_path_buf();
        let launch = launcher::launch_at_login(&data_dir, &*clock, &mut HarnessHost { harness: self }).await;
        let runtime_outcome = self.guardian_script.login_start_outcome.take();
        let watchable = matches!(launch, LoginLaunch::AlreadyRunning | LoginLaunch::Started) && self.runtime.is_some();
        // 監護的單實例：已有監護時這個程序直接結束
        let guards = watchable && !self.guardian_running();
        if guards {
            self.start_guardian();
        }
        Some(LoginStartupRun { launch, runtime_outcome, guards })
    }

    /// 下一次登入啟動入口啟動 Runtime 的同一刻，使用者也手動開了 App：App 的 Runtime 先取得單實例鎖。
    pub fn open_app_during_next_login_start(&mut self) {
        self.guardian_script.app_opens_during_next_login_start = true;
    }

    /// 監護是否在執行（程序內的監護存在而且還沒結束）。
    pub fn guardian_running(&self) -> bool {
        self.guardian.as_ref().is_some_and(|g| g.finished().is_none())
    }

    // ------------------------------------------------------------ 監護（票 32）

    /// 模擬 Runtime 意外終止（程序被殺掉）而**沒有**任何人重新啟動它：丟棄 Runtime 而不記下正常結束。
    /// 之後由監護（`guardian_step`）或使用者開 App（`open_app`）決定是否再啟動。
    pub async fn kill_runtime(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            runtime.kill().await;
        }
    }

    /// 模擬完全退出的停止流程結束後 Runtime 自行結束服務（有秩序的停止）。
    pub async fn stop_runtime_after_exit(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown().await;
        }
    }

    /// 目前有沒有 Runtime 在提供服務。
    pub fn runtime_running(&self) -> bool {
        self.runtime.is_some()
    }

    /// 模擬使用者開 App：沒有 Runtime 就以一般方式啟動（不是監護拉起的，不帶嘗試識別）。
    pub async fn open_app(&mut self) {
        if self.runtime.is_none() {
            self.start_again().await;
        }
    }

    /// 啟動程序內的監護，從「監看目前的 Runtime」開始；健康查詢會回報監護在執行。
    pub fn start_guardian(&mut self) {
        self.guardian = Some(Guardian::new(self.data_dir.path().to_path_buf(), self.fakes.clock.clone()));
        self.fakes.guardian_probe.set_running(true);
    }

    /// 模擬監護程序被殺掉：不再有人自動重啟 Runtime；健康查詢會回報降級。
    pub fn kill_guardian(&mut self) {
        self.guardian = None;
        self.fakes.guardian_probe.set_running(false);
    }

    /// 讓監護處理一次（何時發生由測試決定，時間由 fake Clock 決定）。監護啟動的 Runtime 會成為目前的 Runtime。
    pub async fn guardian_step(&mut self) -> GuardianStep {
        let mut guardian = self.guardian.take().expect("監護沒有在執行（先呼叫 start_guardian）");
        let step = guardian.step(&mut HarnessHost { harness: self }).await;
        if let GuardianStep::Finished(_) = &step {
            // 監護結束就是程序結束
            self.fakes.guardian_probe.set_running(false);
        }
        self.guardian = Some(guardian);
        step
    }

    /// 下一次監護啟動 Runtime 時，程序啟動了卻在匯入監護日誌之前就結束（啟動早期失敗）。
    pub fn fail_next_guardian_start_early(&mut self) {
        self.guardian_script.fail_next_start_early = true;
    }

    /// 下一次監護查詢「Runtime 是否在執行」時先執行 `action`（收到資料目錄）。監護在寫下嘗試之後、啟動之前會再查詢一次，
    /// 用它在那個時間點寫入生命週期檔，就能重現「完全退出／維護與監護啟動」的競態。
    pub fn on_next_runtime_probe(&mut self, action: impl FnOnce(&Path) + Send + 'static) {
        self.guardian_script.on_next_probe = Some(Box::new(action));
    }

    /// 監護啟動 Runtime 的紀錄，依時間順序。
    pub fn guardian_starts(&self) -> Vec<GuardianStart> {
        self.guardian_script.starts.clone()
    }

    /// 監護日誌目前的內容（監護寫在磁碟上的紀錄）。
    pub fn guardian_journal(&self) -> Vec<JournalRecord> {
        baton_guardian::files::read_journal(self.data_dir.path()).expect("無法讀取監護日誌")
    }
}

/// 程序內的 RuntimeHost：監護「看到」的 Runtime 就是 Harness 目前的 Runtime。
struct HarnessHost<'a> {
    harness: &'a mut Harness,
}

#[async_trait::async_trait]
impl RuntimeHost for HarnessHost<'_> {
    async fn runtime_running(&mut self) -> bool {
        if let Some(action) = self.harness.guardian_script.on_next_probe.take() {
            action(self.harness.data_dir.path());
        }
        self.harness.runtime.is_some()
    }

    async fn start_runtime(&mut self, attempt_id: &str) -> Result<(), String> {
        let harness = &mut *self.harness;
        let outcome = if std::mem::take(&mut harness.guardian_script.fail_next_start_early) {
            Err("模擬：Runtime 在匯入監護日誌之前就結束".to_string())
        } else {
            harness.start_single_instance(StartedBy::Guardian(attempt_id.to_string())).await
        };
        harness.guardian_script.starts.push(GuardianStart { attempt_id: attempt_id.to_string(), outcome });
        // 程序本身有啟動起來；之後很快結束的情形由下一次 runtime_running 發現
        Ok(())
    }

    async fn start_runtime_at_login(&mut self) -> Result<(), String> {
        let harness = &mut *self.harness;
        if std::mem::take(&mut harness.guardian_script.app_opens_during_next_login_start) {
            // 使用者手動開 App 的那個 Runtime 先取得單實例鎖
            harness.start_single_instance(StartedBy::App).await.expect("手動開 App 的 Runtime 應該啟動");
        }
        let outcome = harness.start_single_instance(StartedBy::Login).await;
        harness.guardian_script.login_start_outcome = Some(outcome);
        Ok(())
    }
}

/// 啟動 Runtime 的是誰。
enum StartedBy {
    /// 使用者（手動開 App）。
    App,
    /// 監護，帶這一次的嘗試識別（票 32）。
    Guardian(String),
    /// Windows 登入時的啟動入口（票 33）。
    Login,
}

/// 啟動 Runtime 時沿用的 harness 設定。
struct Launch {
    exit_timing: ExitTiming,
    full_text_search: bool,
    exit_linger: Option<Duration>,
    app_version: String,
}

async fn try_start_runtime(data_dir: &Path, ports: &Ports, launch: Launch, by: StartedBy) -> Result<RunningRuntime, RuntimeError> {
    let (guardian_attempt, login_start) = match by {
        StartedBy::App => (None, false),
        StartedBy::Guardian(attempt_id) => (Some(attempt_id), false),
        StartedBy::Login => (None, true),
    };
    let config = RuntimeConfig {
        data_dir: data_dir.to_path_buf(),
        // 埠 0 讓作業系統配置；每個 Harness 與每次重建都不會互相衝突
        bind: "127.0.0.1:0".parse().expect("固定位址"),
    };
    // 測試自己決定何時讓 Supervisor、備份排程與更新檢查動作（`Harness::supervise`、`Harness::run_backups`、
    // `Harness::check_updates`），所以不啟動背景迴圈；完全退出後預設也不自行結束服務，由測試決定何時丟棄或重啟 Runtime
    let options = RuntimeOptions {
        background_supervisor: false,
        background_backup: false,
        background_update_check: false,
        exit_timing: launch.exit_timing,
        exit_linger: launch.exit_linger,
        full_text_search: launch.full_text_search,
        guardian_attempt,
        login_start,
        app_version: launch.app_version,
        ..RuntimeOptions::default()
    };
    Runtime::start_with(config, ports.clone(), options).await
}
