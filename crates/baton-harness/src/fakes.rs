//! 全部 Port 的 fake。
//!
//! 每個 fake 都可由測試腳本化（設定回應、事件、存活狀態），並記錄被呼叫的方式，
//! 讓測試能斷言「沒有第二次 spawn」這類外部行為。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use async_trait::async_trait;
use baton_domain::Timestamp;
use baton_ports::*;
use futures::stream::BoxStream;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// 由測試手動推進的時鐘。
pub struct FakeClock {
    now: AtomicI64,
}

impl FakeClock {
    pub fn new(start: Timestamp) -> Self {
        Self { now: AtomicI64::new(start.as_millis()) }
    }

    pub fn set(&self, to: Timestamp) {
        self.now.store(to.as_millis(), Ordering::SeqCst);
    }

    pub fn advance_millis(&self, millis: i64) {
        self.now.fetch_add(millis, Ordering::SeqCst);
    }
}

impl Clock for FakeClock {
    fn now(&self) -> Timestamp {
        Timestamp::from_millis(self.now.load(Ordering::SeqCst))
    }
}

/// 可腳本化存活狀態的程序控制；不會真的啟動任何程序。
pub struct FakeProcessControl {
    next_pid: AtomicU32,
    state: Mutex<ProcessState>,
}

struct ProcessState {
    spawned: Vec<SpawnSpec>,
    liveness: HashMap<ProcessIdentity, Liveness>,
    stop_result: StopResult,
}

impl Default for FakeProcessControl {
    fn default() -> Self {
        Self {
            next_pid: AtomicU32::new(1000),
            state: Mutex::new(ProcessState {
                spawned: Vec::new(),
                liveness: HashMap::new(),
                stop_result: StopResult::StoppedGracefully,
            }),
        }
    }
}

impl FakeProcessControl {
    pub fn script_liveness(&self, identity: &ProcessIdentity, liveness: Liveness) {
        lock(&self.state).liveness.insert(identity.clone(), liveness);
    }

    pub fn set_stop_result(&self, result: StopResult) {
        lock(&self.state).stop_result = result;
    }

    pub fn spawn_count(&self) -> usize {
        lock(&self.state).spawned.len()
    }

    pub fn spawned(&self) -> Vec<SpawnSpec> {
        lock(&self.state).spawned.clone()
    }
}

#[async_trait]
impl ProcessControl for FakeProcessControl {
    async fn spawn(&self, spec: SpawnSpec) -> PortResult<Receipt> {
        let pid = self.next_pid.fetch_add(1, Ordering::SeqCst);
        let identity = ProcessIdentity {
            pid,
            created_at_millis: i64::from(pid),
            command_hash: format!("{}:{}", spec.program, spec.args.join(" ")),
        };
        let mut state = lock(&self.state);
        state.spawned.push(spec);
        state.liveness.insert(identity.clone(), Liveness::Alive);
        Ok(Receipt { identity })
    }

    async fn inspect(&self, identity: &ProcessIdentity) -> PortResult<Liveness> {
        Ok(lock(&self.state).liveness.get(identity).copied().unwrap_or(Liveness::Dead))
    }

    async fn stop(&self, identity: &ProcessIdentity, _grace: Duration) -> PortResult<StopResult> {
        let mut state = lock(&self.state);
        let result = state.stop_result;
        if result != StopResult::StillAlive {
            state.liveness.insert(identity.clone(), Liveness::Dead);
        }
        Ok(result)
    }
}

/// 以 run_id 為鍵、事件腳本化的 Agent Adapter。
pub struct FakeAgentAdapter {
    state: Mutex<AdapterState>,
}

struct AdapterState {
    scripts: HashMap<String, Vec<AdapterEvent>>,
    launched: Vec<RunSpec>,
    capability: Capability,
}

impl Default for FakeAgentAdapter {
    fn default() -> Self {
        Self {
            state: Mutex::new(AdapterState {
                scripts: HashMap::new(),
                launched: Vec::new(),
                capability: Capability::Resumable,
            }),
        }
    }
}

impl FakeAgentAdapter {
    pub fn script_events(&self, run_id: &str, events: Vec<AdapterEvent>) {
        lock(&self.state).scripts.insert(run_id.to_string(), events);
    }

    pub fn set_capability(&self, capability: Capability) {
        lock(&self.state).capability = capability;
    }

    pub fn launched(&self) -> Vec<RunSpec> {
        lock(&self.state).launched.clone()
    }
}

#[async_trait]
impl AgentAdapter for FakeAgentAdapter {
    async fn launch(&self, spec: RunSpec) -> PortResult<AgentHandle> {
        let handle = AgentHandle(spec.run_id.clone());
        lock(&self.state).launched.push(spec);
        Ok(handle)
    }

    fn events(&self, handle: &AgentHandle) -> BoxStream<'static, AdapterEvent> {
        let events = lock(&self.state).scripts.get(&handle.0).cloned().unwrap_or_default();
        Box::pin(futures::stream::iter(events))
    }

    async fn resume_capability(&self, _session: &SessionRef) -> PortResult<Capability> {
        Ok(lock(&self.state).capability)
    }
}

/// 以 profile 為鍵、樣本腳本化的額度觀測器；沒有腳本的 profile 回傳錯誤。
#[derive(Default)]
pub struct FakeQuotaObserver {
    samples: Mutex<HashMap<String, QuotaSample>>,
}

impl FakeQuotaObserver {
    pub fn set_sample(&self, profile: &str, sample: QuotaSample) {
        lock(&self.samples).insert(profile.to_string(), sample);
    }
}

#[async_trait]
impl QuotaObserver for FakeQuotaObserver {
    async fn observe(&self, profile: &ProfileRef) -> PortResult<QuotaSample> {
        lock(&self.samples)
            .get(&profile.0)
            .cloned()
            .ok_or_else(|| PortError::Failed(format!("沒有 {} 的腳本化樣本", profile.0)))
    }
}

/// 在暫存目錄中運作的 WorkspaceFs：成果寫入是真實的原子寫入，
/// worktree 只在記憶體中追蹤（不依賴 git）。
#[derive(Default)]
pub struct FakeWorkspaceFs {
    worktrees: Mutex<HashSet<PathBuf>>,
    dirty: Mutex<HashSet<PathBuf>>,
}

impl FakeWorkspaceFs {
    pub fn mark_dirty(&self, target: &Path) {
        lock(&self.dirty).insert(target.to_path_buf());
    }

    pub fn has_worktree(&self, target: &Path) -> bool {
        lock(&self.worktrees).contains(target)
    }
}

#[async_trait]
impl WorkspaceFs for FakeWorkspaceFs {
    async fn inspect_directory(&self, path: &Path) -> PortResult<DirectoryState> {
        baton_ports::directory_state(path)
    }

    async fn ensure_directory(&self, path: &Path) -> PortResult<()> {
        tokio::fs::create_dir_all(path).await.map_err(|e| PortError::Failed(e.to_string()))
    }

    async fn create_worktree(&self, _repo: &Path, _branch: &str, target: &Path) -> PortResult<()> {
        tokio::fs::create_dir_all(target).await.map_err(|e| PortError::Failed(e.to_string()))?;
        lock(&self.worktrees).insert(target.to_path_buf());
        Ok(())
    }

    async fn remove_worktree(&self, target: &Path) -> PortResult<()> {
        lock(&self.worktrees).remove(target);
        lock(&self.dirty).remove(target);
        match tokio::fs::remove_dir_all(target).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(PortError::Failed(e.to_string())),
        }
    }

    async fn inspect_dirty(&self, target: &Path) -> PortResult<DirtyState> {
        Ok(if lock(&self.dirty).contains(target) { DirtyState::Dirty } else { DirtyState::Clean })
    }

    async fn write_artifact_atomic(&self, path: &Path, bytes: &[u8]) -> PortResult<()> {
        use tokio::io::AsyncWriteExt;
        let io = |e: std::io::Error| PortError::Failed(e.to_string());
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(io)?;
        }
        // 先寫暫存檔並 fsync，再改名，讀取端不會看到寫到一半的檔案
        let temp = path.with_extension("tmp-write");
        let mut file = tokio::fs::File::create(&temp).await.map_err(io)?;
        file.write_all(bytes).await.map_err(io)?;
        file.sync_all().await.map_err(io)?;
        drop(file);
        tokio::fs::rename(&temp, path).await.map_err(io)
    }
}

/// 可由測試設定回報內容的平台資訊。
pub struct FakePlatformInfo {
    snapshot: Mutex<PlatformSnapshot>,
}

impl Default for FakePlatformInfo {
    fn default() -> Self {
        Self {
            snapshot: Mutex::new(PlatformSnapshot {
                os_family: OsFamily::Windows11,
                os_build: "26200".into(),
                feature_version: "25H2".into(),
                edition: "Pro".into(),
                native_arch: "x64".into(),
                process_arch: "x64".into(),
            }),
        }
    }
}

impl FakePlatformInfo {
    pub fn set(&self, snapshot: PlatformSnapshot) {
        *lock(&self.snapshot) = snapshot;
    }
}

impl PlatformInfo for FakePlatformInfo {
    fn snapshot(&self) -> PlatformSnapshot {
        lock(&self.snapshot).clone()
    }
}

/// 可腳本化的 Claude Code 與 Git 環境：工具是否存在與版本、各設定目錄的原生登入、環境變數覆蓋。
///
/// 模擬真實 Claude Code 的行為：原生登入的資料（token）放在 Profile 自己的設定目錄
/// （`.credentials.json`），核對結果只回報認證來源與帳號，不含 token；環境變數覆蓋時沒有帳號。
pub struct FakeClaudeEnvironment {
    state: Mutex<ClaudeEnvState>,
}

struct ClaudeEnvState {
    claude: ToolProbe,
    git: ToolProbe,
    tool_probe_failure: Option<String>,
    auth_failure: Option<String>,
    /// 設定目錄（正規化後）→ 原生登入的帳號。
    logins: HashMap<String, String>,
    env_override: Option<(AuthSource, Option<String>)>,
    /// 開啟登入終端機時，模擬使用者在其中完成登入的帳號。
    terminal_login: Option<String>,
    terminals: Vec<PathBuf>,
    auth_checks: usize,
    secrets_issued: u32,
}

impl Default for FakeClaudeEnvironment {
    fn default() -> Self {
        Self {
            state: Mutex::new(ClaudeEnvState {
                claude: ToolProbe::Found { version_output: Some("2.1.307 (Claude Code)".into()) },
                git: ToolProbe::Found { version_output: Some("git version 2.54.0.windows.1".into()) },
                tool_probe_failure: None,
                auth_failure: None,
                logins: HashMap::new(),
                env_override: None,
                terminal_login: None,
                terminals: Vec::new(),
                auth_checks: 0,
                secrets_issued: 0,
            }),
        }
    }
}

fn dir_key(dir: &Path) -> String {
    dir.to_string_lossy().replace('/', "\\").trim_end_matches('\\').to_lowercase()
}

impl FakeClaudeEnvironment {
    pub fn set_claude(&self, probe: ToolProbe) {
        lock(&self.state).claude = probe;
    }

    pub fn set_git(&self, probe: ToolProbe) {
        lock(&self.state).git = probe;
    }

    /// 之後所有工具探測都失敗（例如逾時）。
    pub fn fail_tool_probes(&self, reason: &str) {
        lock(&self.state).tool_probe_failure = Some(reason.to_string());
    }

    /// 之後所有認證核對都失敗（例如 claude 沒有回應）。
    pub fn fail_auth_checks(&self, reason: &str) {
        lock(&self.state).auth_failure = Some(reason.to_string());
    }

    /// 在設定目錄完成原生登入：記錄帳號，並像真實的 Claude Code 一樣把登入資料寫進該目錄。
    /// 回傳寫入的登入資料（測試用來確認它不會出現在資料庫、日誌或 API 回應）。
    pub fn log_in(&self, config_dir: &Path, account: &str) -> String {
        let mut state = lock(&self.state);
        state.secrets_issued += 1;
        let secret = format!("fake-oauth-secret-{}-{:x}", state.secrets_issued, account.len() * 7919);
        std::fs::create_dir_all(config_dir).expect("無法建立設定目錄");
        std::fs::write(
            config_dir.join(".credentials.json"),
            format!(r#"{{"claudeAiOauth":{{"accessToken":"{secret}"}}}}"#),
        )
        .expect("無法寫入登入資料");
        state.logins.insert(dir_key(config_dir), account.to_string());
        secret
    }

    /// 模擬環境變數或 credential helper 覆蓋了認證來源（`None` 表示移除覆蓋）。
    pub fn set_env_override(&self, source: Option<(AuthSource, Option<&str>)>) {
        lock(&self.state).env_override = source.map(|(s, detail)| (s, detail.map(str::to_string)));
    }

    /// 之後開啟登入終端機時，模擬使用者在其中以這個帳號完成登入。
    pub fn script_terminal_login(&self, account: &str) {
        lock(&self.state).terminal_login = Some(account.to_string());
    }

    /// 已開啟的登入終端機（依序記錄各自使用的 `CLAUDE_CONFIG_DIR`）。
    pub fn login_terminals(&self) -> Vec<PathBuf> {
        lock(&self.state).terminals.clone()
    }

    pub fn auth_check_count(&self) -> usize {
        lock(&self.state).auth_checks
    }

    pub fn reset_auth_check_count(&self) {
        lock(&self.state).auth_checks = 0;
    }
}

#[async_trait]
impl ClaudeEnvironment for FakeClaudeEnvironment {
    async fn probe_claude(&self) -> PortResult<ToolProbe> {
        let state = lock(&self.state);
        match &state.tool_probe_failure {
            Some(reason) => Err(PortError::Failed(reason.clone())),
            None => Ok(state.claude.clone()),
        }
    }

    async fn probe_git(&self) -> PortResult<ToolProbe> {
        let state = lock(&self.state);
        match &state.tool_probe_failure {
            Some(reason) => Err(PortError::Failed(reason.clone())),
            None => Ok(state.git.clone()),
        }
    }

    async fn auth_status(&self, config_dir: &Path) -> PortResult<AuthReport> {
        let mut state = lock(&self.state);
        state.auth_checks += 1;
        if let Some(reason) = &state.auth_failure {
            return Err(PortError::Failed(reason.clone()));
        }
        let config_dir_reported = Some(config_dir.to_string_lossy().into_owned());
        if let Some((source, detail)) = &state.env_override {
            return Ok(AuthReport {
                logged_in: true,
                source: *source,
                source_detail: detail.clone(),
                account: None,
                config_dir: config_dir_reported,
            });
        }
        Ok(match state.logins.get(&dir_key(config_dir)) {
            Some(account) => AuthReport {
                logged_in: true,
                source: AuthSource::NativeLogin,
                source_detail: None,
                account: Some(account.clone()),
                config_dir: config_dir_reported,
            },
            None => AuthReport {
                logged_in: false,
                source: AuthSource::None,
                source_detail: None,
                account: None,
                config_dir: config_dir_reported,
            },
        })
    }

    async fn open_login_terminal(&self, config_dir: &Path) -> PortResult<()> {
        let scripted = {
            let mut state = lock(&self.state);
            state.terminals.push(config_dir.to_path_buf());
            state.terminal_login.clone()
        };
        if let Some(account) = scripted {
            self.log_in(config_dir, &account);
        }
        Ok(())
    }
}

/// 一組 fake Port 的具體型別，測試可直接操作腳本與讀取紀錄。
/// 可逆但不是明文的保護器：以固定前綴加逐位元 XOR 模擬 DPAPI，不依賴作業系統。
#[derive(Default)]
pub struct FakeSecretProtector;

const FAKE_PROTECT_PREFIX: &[u8] = b"FAKE-DPAPI:";
const FAKE_PROTECT_KEY: u8 = 0x5A;

impl SecretProtector for FakeSecretProtector {
    fn protect(&self, plaintext: &[u8]) -> PortResult<Vec<u8>> {
        let mut out = FAKE_PROTECT_PREFIX.to_vec();
        out.extend(plaintext.iter().map(|b| b ^ FAKE_PROTECT_KEY));
        Ok(out)
    }

    fn unprotect(&self, protected: &[u8]) -> PortResult<Vec<u8>> {
        let body = protected
            .strip_prefix(FAKE_PROTECT_PREFIX)
            .ok_or_else(|| PortError::Failed("不是這個保護器產生的資料".into()))?;
        Ok(body.iter().map(|b| b ^ FAKE_PROTECT_KEY).collect())
    }
}

pub struct FakePorts {
    pub clock: Arc<FakeClock>,
    pub process_control: Arc<FakeProcessControl>,
    pub agent_adapter: Arc<FakeAgentAdapter>,
    pub quota_observer: Arc<FakeQuotaObserver>,
    pub workspace_fs: Arc<FakeWorkspaceFs>,
    pub platform_info: Arc<FakePlatformInfo>,
    pub secret_protector: Arc<FakeSecretProtector>,
    pub claude_env: Arc<FakeClaudeEnvironment>,
}

impl FakePorts {
    pub fn new(clock_start: Timestamp) -> Self {
        Self {
            clock: Arc::new(FakeClock::new(clock_start)),
            process_control: Arc::default(),
            agent_adapter: Arc::default(),
            quota_observer: Arc::default(),
            workspace_fs: Arc::default(),
            platform_info: Arc::default(),
            secret_protector: Arc::default(),
            claude_env: Arc::default(),
        }
    }

    /// 轉成 Runtime 需要的 `Ports`（同一批 fake 的共用參照）。
    pub fn to_ports(&self) -> Ports {
        Ports {
            clock: self.clock.clone(),
            process_control: self.process_control.clone(),
            agent_adapter: self.agent_adapter.clone(),
            quota_observer: self.quota_observer.clone(),
            workspace_fs: self.workspace_fs.clone(),
            platform_info: self.platform_info.clone(),
            secret_protector: self.secret_protector.clone(),
            claude_env: self.claude_env.clone(),
        }
    }
}
