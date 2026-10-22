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
                os_build: "fake".into(),
                edition: "Pro".into(),
                native_arch: "x64".into(),
                support_status: SupportStatus::Supported,
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
        }
    }
}
