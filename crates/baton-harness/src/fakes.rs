//! 全部 Port 的 fake。
//!
//! 每個 fake 都可由測試腳本化（設定回應、事件、存活狀態），並記錄被呼叫的方式，
//! 讓測試能斷言「沒有第二次 spawn」這類外部行為。

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Poll, Waker};
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

/// 模擬 Runtime 在 `spawn` 前後的某一點當掉：到了那一點就永遠不回來（呼叫端的 future 被丟棄即等同崩潰）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HangPoint {
    /// 程序尚未啟動。
    BeforeSpawn,
    /// 程序已啟動並登記，但回執沒有交還給呼叫端。
    AfterSpawn,
}

/// 可腳本化存活狀態的程序控制；不會真的啟動任何程序。
pub struct FakeProcessControl {
    next_pid: AtomicU32,
    state: Mutex<ProcessState>,
}

struct ProcessState {
    spawned: Vec<SpawnSpec>,
    liveness: HashMap<ProcessIdentity, Liveness>,
    by_command: HashMap<String, ProcessIdentity>,
    stop_result: StopResult,
    stop_calls: Vec<ProcessIdentity>,
    inspect_count: usize,
    hang: Option<HangPoint>,
    fail_spawn: Option<String>,
}

impl Default for FakeProcessControl {
    fn default() -> Self {
        Self {
            next_pid: AtomicU32::new(1000),
            state: Mutex::new(ProcessState {
                spawned: Vec::new(),
                liveness: HashMap::new(),
                by_command: HashMap::new(),
                stop_result: StopResult::StoppedGracefully,
                stop_calls: Vec::new(),
                inspect_count: 0,
                hang: None,
                fail_spawn: None,
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

    /// 下一次 `spawn` 在指定的點永遠不回來（只生效一次）。
    pub fn hang_next_spawn_at(&self, point: HangPoint) {
        lock(&self.state).hang = Some(point);
    }

    /// 下一次 `spawn` 失敗（沒有啟動任何程序），只生效一次。
    pub fn fail_next_spawn(&self, reason: &str) {
        lock(&self.state).fail_spawn = Some(reason.to_string());
    }

    pub fn spawn_count(&self) -> usize {
        lock(&self.state).spawned.len()
    }

    pub fn spawned(&self) -> Vec<SpawnSpec> {
        lock(&self.state).spawned.clone()
    }

    /// 被要求停止的程序身分，依呼叫順序。
    pub fn stop_calls(&self) -> Vec<ProcessIdentity> {
        lock(&self.state).stop_calls.clone()
    }

    pub fn inspect_count(&self) -> usize {
        lock(&self.state).inspect_count
    }

    /// 某個 command_id 啟動出來的程序身分。
    pub fn identity_of(&self, command_id: &str) -> Option<ProcessIdentity> {
        lock(&self.state).by_command.get(command_id).cloned()
    }

    fn take_hang(&self, point: HangPoint) -> bool {
        let mut state = lock(&self.state);
        if state.hang == Some(point) {
            state.hang = None;
            return true;
        }
        false
    }
}

#[async_trait]
impl ProcessControl for FakeProcessControl {
    async fn spawn(&self, spec: SpawnSpec) -> PortResult<Receipt> {
        if self.take_hang(HangPoint::BeforeSpawn) {
            std::future::pending::<()>().await;
        }
        if let Some(reason) = lock(&self.state).fail_spawn.take() {
            return Err(PortError::Failed(reason));
        }
        let pid = self.next_pid.fetch_add(1, Ordering::SeqCst);
        let identity =
            ProcessIdentity { pid, created_at_millis: i64::from(pid), command_hash: command_hash(&spec) };
        {
            let mut state = lock(&self.state);
            state.by_command.insert(spec.command_id.clone(), identity.clone());
            state.spawned.push(spec);
            state.liveness.insert(identity.clone(), Liveness::Alive);
        }
        if self.take_hang(HangPoint::AfterSpawn) {
            std::future::pending::<()>().await;
        }
        Ok(Receipt { identity })
    }

    /// 與正式版一致：程序已經全部結束就找不到。
    async fn find(&self, spec: &SpawnSpec) -> PortResult<Option<Receipt>> {
        let state = lock(&self.state);
        Ok(state
            .by_command
            .get(&spec.command_id)
            .filter(|identity| state.liveness.get(*identity).copied().unwrap_or(Liveness::Dead) != Liveness::Dead)
            .map(|identity| Receipt { identity: identity.clone() }))
    }

    async fn inspect(&self, identity: &ProcessIdentity) -> PortResult<Liveness> {
        let mut state = lock(&self.state);
        state.inspect_count += 1;
        Ok(state.liveness.get(identity).copied().unwrap_or(Liveness::Dead))
    }

    async fn stop(&self, identity: &ProcessIdentity, _grace: Duration) -> PortResult<StopResult> {
        let mut state = lock(&self.state);
        state.stop_calls.push(identity.clone());
        let result = state.stop_result;
        if result != StopResult::StillAlive {
            state.liveness.insert(identity.clone(), Liveness::Dead);
        }
        Ok(result)
    }
}

/// 單一 Run 的事件管道：測試隨時 `emit`，Supervisor 隨時取用；Runtime 重建後再接上同一條管道，沒取走的事件還在。
#[derive(Default)]
struct EventChannel {
    queue: VecDeque<AdapterEvent>,
    waker: Option<Waker>,
    /// 已經交出 `Exited`：之後串流結束。
    ended: bool,
}

type SharedChannel = Arc<Mutex<EventChannel>>;

/// 以 run_id 為鍵、事件可腳本化也可即時推送的 Agent Adapter。
pub struct FakeAgentAdapter {
    state: Mutex<AdapterState>,
}

struct AdapterState {
    channels: HashMap<String, SharedChannel>,
    /// 下一個（尚未見過的）Run 一啟動就會自動推送的事件，先進先出。
    next_scripts: VecDeque<Vec<AdapterEvent>>,
    launched: Vec<RunSpec>,
    capability: Capability,
}

impl Default for FakeAgentAdapter {
    fn default() -> Self {
        Self {
            state: Mutex::new(AdapterState {
                channels: HashMap::new(),
                next_scripts: VecDeque::new(),
                launched: Vec::new(),
                capability: Capability::Resumable,
            }),
        }
    }
}

impl FakeAgentAdapter {
    fn channel(&self, run_id: &str) -> SharedChannel {
        lock(&self.state).channels.entry(run_id.to_string()).or_default().clone()
    }

    /// 事先放入某個 run 的事件（run_id 已知時使用）。
    pub fn script_events(&self, run_id: &str, events: Vec<AdapterEvent>) {
        for event in events {
            self.emit(run_id, event);
        }
    }

    /// 下一個被啟動的 Run 一啟動就收到這些事件（Run 識別由 Runtime 產生，測試事先不知道）。
    pub fn script_next_run(&self, events: Vec<AdapterEvent>) {
        lock(&self.state).next_scripts.push_back(events);
    }

    /// 即時推送一個事件給某個 Run。
    pub fn emit(&self, run_id: &str, event: AdapterEvent) {
        let channel = self.channel(run_id);
        let mut channel = lock(&channel);
        channel.queue.push_back(event);
        if let Some(waker) = channel.waker.take() {
            waker.wake();
        }
    }

    pub fn set_capability(&self, capability: Capability) {
        lock(&self.state).capability = capability;
    }

    /// 每次 `launch`（含 Runtime 重建後的重新接上）收到的 RunSpec。
    pub fn launched(&self) -> Vec<RunSpec> {
        lock(&self.state).launched.clone()
    }
}

#[async_trait]
impl AgentAdapter for FakeAgentAdapter {
    fn spawn_spec(&self, spec: &RunSpec) -> SpawnSpec {
        SpawnSpec {
            program: "fake-agent".into(),
            args: vec![spec.run_id.clone()],
            working_dir: spec.working_dir.clone(),
            command_id: spec.command_id.clone(),
            env: Vec::new(),
        }
    }

    async fn launch(&self, spec: RunSpec) -> PortResult<AgentHandle> {
        let handle = AgentHandle(spec.run_id.clone());
        let first_time = !lock(&self.state).launched.iter().any(|s| s.run_id == spec.run_id);
        let script = if first_time { lock(&self.state).next_scripts.pop_front() } else { None };
        lock(&self.state).launched.push(spec);
        if let Some(events) = script {
            self.script_events(&handle.0, events);
        }
        Ok(handle)
    }

    fn events(&self, handle: &AgentHandle) -> BoxStream<'static, AdapterEvent> {
        let channel = self.channel(&handle.0);
        Box::pin(futures::stream::poll_fn(move |cx| {
            let mut channel = lock(&channel);
            if channel.ended {
                return Poll::Ready(None);
            }
            match channel.queue.pop_front() {
                Some(event) => {
                    channel.ended = matches!(event, AdapterEvent::Exited { .. });
                    Poll::Ready(Some(event))
                }
                None => {
                    channel.waker = Some(cx.waker().clone());
                    Poll::Pending
                }
            }
        }))
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

/// 模擬 Runtime 在成果寫入的某一點當掉：到了那一點就永遠不回來（呼叫端的 future 被丟棄即等同崩潰）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactWritePoint {
    /// 暫存檔只寫了一半（沒有 fsync、沒有改名）。
    MidWrite,
    /// 暫存檔已完整寫入並 fsync，但還沒改名。
    BeforeRename,
    /// 已經原子改名，但呼叫端還沒拿到結果（所以 manifest 不會被提交）。
    AfterRename,
}

/// 在暫存目錄中運作的 WorkspaceFs：成果寫入是真實的原子寫入（步驟與正式版相同），
/// worktree 只在記憶體中追蹤（不依賴 git）。
#[derive(Default)]
pub struct FakeWorkspaceFs {
    worktrees: Mutex<HashSet<PathBuf>>,
    dirty: Mutex<HashSet<PathBuf>>,
    artifact_hang: Mutex<Option<ArtifactWritePoint>>,
}

impl FakeWorkspaceFs {
    pub fn mark_dirty(&self, target: &Path) {
        lock(&self.dirty).insert(target.to_path_buf());
    }

    pub fn has_worktree(&self, target: &Path) -> bool {
        lock(&self.worktrees).contains(target)
    }

    /// 下一次成果寫入在指定的點永遠不回來（只生效一次）。
    pub fn hang_next_artifact_write_at(&self, point: ArtifactWritePoint) {
        *lock(&self.artifact_hang) = Some(point);
    }

    /// 若下一次成果寫入被設定在這一點當掉，這裡就永遠不回來。
    async fn crash_point(&self, point: ArtifactWritePoint) {
        let hang = {
            let mut armed = lock(&self.artifact_hang);
            (*armed == Some(point)).then(|| armed.take())
        };
        if hang.is_some() {
            std::future::pending::<()>().await;
        }
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
        // 與正式版相同的步驟：先寫暫存檔並 fsync，再改名，讀取端不會看到寫到一半的目標檔；各步驟之間可以模擬崩潰
        let temp = baton_ports::partial_path(path);
        let mut file = tokio::fs::File::create(&temp).await.map_err(io)?;
        let (first, rest) = bytes.split_at(bytes.len() / 2);
        file.write_all(first).await.map_err(io)?;
        file.flush().await.map_err(io)?;
        self.crash_point(ArtifactWritePoint::MidWrite).await;
        file.write_all(rest).await.map_err(io)?;
        file.sync_all().await.map_err(io)?;
        drop(file);
        self.crash_point(ArtifactWritePoint::BeforeRename).await;
        tokio::fs::rename(&temp, path).await.map_err(io)?;
        self.crash_point(ArtifactWritePoint::AfterRename).await;
        Ok(())
    }

    async fn read_artifact(&self, path: &Path) -> PortResult<Option<Vec<u8>>> {
        baton_ports::read_file_if_exists(path)
    }

    async fn list_artifact_files(&self, dir: &Path) -> PortResult<Vec<FileEntry>> {
        baton_ports::list_files_recursive(dir)
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
