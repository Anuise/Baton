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
    /// 時間被推進時叫醒停在 `sleep_until` 的等待者。
    advanced: tokio::sync::Notify,
    /// 目前停在 `sleep_until` 的等待者數：測試以 `wait_for_sleepers` 確認長輪詢已經開始等待，再推進時間。
    sleepers: tokio::sync::watch::Sender<usize>,
}

impl FakeClock {
    pub fn new(start: Timestamp) -> Self {
        Self {
            now: AtomicI64::new(start.as_millis()),
            advanced: tokio::sync::Notify::new(),
            sleepers: tokio::sync::watch::Sender::new(0),
        }
    }

    pub fn set(&self, to: Timestamp) {
        self.now.store(to.as_millis(), Ordering::SeqCst);
        self.advanced.notify_waiters();
    }

    pub fn advance_millis(&self, millis: i64) {
        self.now.fetch_add(millis, Ordering::SeqCst);
        self.advanced.notify_waiters();
    }

    /// 等到至少 `count` 個等待者停在 `sleep_until`（不推進時間、不使用真實 sleep）。
    pub async fn wait_for_sleepers(&self, count: usize) {
        let mut sleepers = self.sleepers.subscribe();
        let _ = sleepers.wait_for(|n| *n >= count).await;
    }
}

/// 在 `sleep_until` 期間計入等待者數，結束（含被丟棄）時扣回。
struct Sleeper<'a>(&'a tokio::sync::watch::Sender<usize>);

impl<'a> Sleeper<'a> {
    fn enter(sleepers: &'a tokio::sync::watch::Sender<usize>) -> Self {
        sleepers.send_modify(|n| *n += 1);
        Self(sleepers)
    }
}

impl Drop for Sleeper<'_> {
    fn drop(&mut self) {
        self.0.send_modify(|n| *n -= 1);
    }
}

impl Clock for FakeClock {
    fn now(&self) -> Timestamp {
        Timestamp::from_millis(self.now.load(Ordering::SeqCst))
    }

    /// 只在測試推進時間（`set`、`advance_millis`）時醒來比對。
    fn sleep_until(&self, deadline: Timestamp) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            let _sleeper = Sleeper::enter(&self.sleepers);
            loop {
                let advanced = self.advanced.notified();
                tokio::pin!(advanced);
                // 先登記再比對時間，比對之後的推進一定叫得醒
                advanced.as_mut().enable();
                if self.now() >= deadline {
                    return;
                }
                advanced.await;
            }
        })
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

/// 某個程序被要求停止時的腳本化行為。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FakeStop {
    /// 回傳這個結果（`StillAlive` 時程序仍存活）。
    Result(StopResult),
    /// 停止的呼叫永遠不回來（模擬卡住的停止）。
    Hang,
    /// 停止的呼叫失敗。
    Fail(String),
    /// 程序確實停止了，但呼叫永遠不回來（模擬 Runtime 在「舊程序已停止、結果尚未寫回」的邊界當掉）。
    HangAfterStop,
}

/// 可腳本化存活狀態的程序控制；不會真的啟動任何程序。
pub struct FakeProcessControl {
    next_pid: AtomicU32,
    state: Mutex<ProcessState>,
    /// 收到 `stop` 或 `spawn` 走到停住點時喚醒等待中的測試（測試等可觀察的條件，不睡固定時間）
    activity: tokio::sync::Notify,
}

struct ProcessState {
    spawned: Vec<SpawnSpec>,
    liveness: HashMap<ProcessIdentity, Liveness>,
    by_command: HashMap<String, ProcessIdentity>,
    stop_result: StopResult,
    stop_scripts: HashMap<ProcessIdentity, FakeStop>,
    stop_calls: Vec<ProcessIdentity>,
    stop_graces: Vec<Duration>,
    /// `release` 的結果（預設確認整個程序樹已結束）與收到的身分，依呼叫順序。
    release_result: StopResult,
    release_calls: Vec<ProcessIdentity>,
    inspect_count: usize,
    hang: Option<HangPoint>,
    fail_spawn: Option<String>,
    /// 下一次 `spawn` 啟動並登記程序之後仍回傳失敗（回執遺失）。
    fail_spawn_after_start: Option<String>,
    /// 設定時每次 `find` 都失敗（無法查證程序在不在）。
    fail_find: Option<String>,
    /// 設定時每次 `find` 都永遠不回來。
    hang_find: bool,
    /// 下一次 `spawn` 啟動並登記程序之後、回傳之前執行（在這個邊界注入別的故障）。
    on_next_spawn: Option<Box<dyn FnOnce() + Send>>,
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
                stop_scripts: HashMap::new(),
                stop_calls: Vec::new(),
                stop_graces: Vec::new(),
                release_result: StopResult::StoppedGracefully,
                release_calls: Vec::new(),
                inspect_count: 0,
                hang: None,
                fail_spawn: None,
                fail_spawn_after_start: None,
                fail_find: None,
                hang_find: false,
                on_next_spawn: None,
            }),
            activity: tokio::sync::Notify::new(),
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

    /// 指定某個程序被要求停止時的行為（優先於 `set_stop_result`）。
    pub fn script_stop(&self, identity: &ProcessIdentity, behavior: FakeStop) {
        lock(&self.state).stop_scripts.insert(identity.clone(), behavior);
    }

    /// 每次 `stop` 收到的寬限期，依呼叫順序。
    pub fn stop_graces(&self) -> Vec<Duration> {
        lock(&self.state).stop_graces.clone()
    }

    /// 下一次 `spawn` 在指定的點永遠不回來（只生效一次）。
    pub fn hang_next_spawn_at(&self, point: HangPoint) {
        lock(&self.state).hang = Some(point);
    }

    /// 下一次 `spawn` 失敗（沒有啟動任何程序），只生效一次。
    pub fn fail_next_spawn(&self, reason: &str) {
        lock(&self.state).fail_spawn = Some(reason.to_string());
    }

    /// 下一次 `spawn` 真的啟動並登記了程序，卻回傳失敗（回執在傳回途中遺失），只生效一次。
    pub fn fail_next_spawn_after_start(&self, reason: &str) {
        lock(&self.state).fail_spawn_after_start = Some(reason.to_string());
    }

    /// `Some` 之後每次 `find` 都失敗（無法查證程序在不在），`None` 恢復。
    pub fn fail_finds(&self, reason: Option<&str>) {
        lock(&self.state).fail_find = reason.map(str::to_string);
    }

    /// `true` 之後每次 `find` 都永遠不回來（模擬卡住的查證），`false` 恢復。
    pub fn hang_finds(&self, hang: bool) {
        lock(&self.state).hang_find = hang;
    }

    /// 下一次 `spawn` 啟動並登記程序之後、回傳回執之前執行 `action`，只生效一次。
    pub fn run_on_next_spawn(&self, action: impl FnOnce() + Send + 'static) {
        lock(&self.state).on_next_spawn = Some(Box::new(action));
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

    /// 之後每次 `release` 的結果（例如 `StillAlive`：程序樹裡還有清不掉的程序）。
    pub fn set_release_result(&self, result: StopResult) {
        lock(&self.state).release_result = result;
    }

    /// Run 結束前被要求收尾的程序身分，依呼叫順序。
    pub fn release_calls(&self) -> Vec<ProcessIdentity> {
        lock(&self.state).release_calls.clone()
    }

    /// 目前仍存活（含無法判定）的程序身分；用來斷言同一時間沒有第二個 writer。
    pub fn alive(&self) -> Vec<ProcessIdentity> {
        let state = lock(&self.state);
        state.liveness.iter().filter(|(_, l)| **l != Liveness::Dead).map(|(identity, _)| identity.clone()).collect()
    }

    pub fn inspect_count(&self) -> usize {
        lock(&self.state).inspect_count
    }

    /// 登記一個不是由 `spawn` 啟動、但放在 `spec.command_id` 程序群組裡的程序（例如在終端機中開出的 Controller 宿主），
    /// 之後可以 `find`、`inspect`、`stop` 它；不計入 `spawned`。
    pub fn register_external(&self, spec: &SpawnSpec) -> ProcessIdentity {
        let pid = self.next_pid.fetch_add(1, Ordering::SeqCst);
        let identity = ProcessIdentity { pid, created_at_millis: i64::from(pid), command_hash: command_hash(spec) };
        let mut state = lock(&self.state);
        state.by_command.insert(spec.command_id.clone(), identity.clone());
        state.liveness.insert(identity.clone(), Liveness::Alive);
        identity
    }

    /// 某個 command_id 啟動出來的程序身分。
    pub fn identity_of(&self, command_id: &str) -> Option<ProcessIdentity> {
        lock(&self.state).by_command.get(command_id).cloned()
    }

    /// 等到 `identity` 被要求停止。
    pub async fn wait_for_stop_call(&self, identity: &ProcessIdentity) {
        self.wait_until(|state| state.stop_calls.contains(identity)).await;
    }

    /// 等到 `hang_next_spawn_at` 設定的停住點已被某次 `spawn` 走到（之後那次 `spawn` 永遠不回來）。
    pub async fn wait_for_spawn_hang(&self) {
        self.wait_until(|state| state.hang.is_none()).await;
    }

    /// 等到 `done` 成立；每次狀態改變（`stop`、走到停住點）都重新檢查，不會漏掉檢查與等待之間的通知。
    async fn wait_until(&self, done: impl Fn(&ProcessState) -> bool) {
        loop {
            let mut changed = std::pin::pin!(self.activity.notified());
            changed.as_mut().enable();
            if done(&lock(&self.state)) {
                return;
            }
            changed.await;
        }
    }

    fn take_hang(&self, point: HangPoint) -> bool {
        let mut state = lock(&self.state);
        if state.hang == Some(point) {
            state.hang = None;
            drop(state);
            self.activity.notify_waiters();
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
        let action = lock(&self.state).on_next_spawn.take();
        if let Some(action) = action {
            action();
        }
        if let Some(reason) = lock(&self.state).fail_spawn_after_start.take() {
            return Err(PortError::Failed(reason));
        }
        Ok(Receipt { identity })
    }

    /// 與正式版一致：程序已經全部結束就找不到。
    async fn find(&self, spec: &SpawnSpec) -> PortResult<Option<Receipt>> {
        if lock(&self.state).hang_find {
            std::future::pending::<()>().await;
        }
        let state = lock(&self.state);
        if let Some(reason) = &state.fail_find {
            return Err(PortError::Failed(reason.clone()));
        }
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

    async fn stop(&self, identity: &ProcessIdentity, grace: Duration) -> PortResult<StopResult> {
        let behavior = {
            let mut state = lock(&self.state);
            state.stop_calls.push(identity.clone());
            state.stop_graces.push(grace);
            state.stop_scripts.get(identity).cloned().unwrap_or(FakeStop::Result(state.stop_result))
        };
        self.activity.notify_waiters();
        match behavior {
            FakeStop::Hang => std::future::pending().await,
            FakeStop::HangAfterStop => {
                lock(&self.state).liveness.insert(identity.clone(), Liveness::Dead);
                std::future::pending().await
            }
            FakeStop::Fail(reason) => Err(PortError::Failed(reason)),
            FakeStop::Result(result) => {
                if result != StopResult::StillAlive {
                    lock(&self.state).liveness.insert(identity.clone(), Liveness::Dead);
                }
                Ok(result)
            }
        }
    }

    async fn release(&self, identity: &ProcessIdentity, _grace: Duration) -> PortResult<StopResult> {
        let mut state = lock(&self.state);
        state.release_calls.push(identity.clone());
        let result = state.release_result;
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
    /// 個別原生 Session 的接續能力（優先於 `capability`）。
    session_capabilities: HashMap<String, Capability>,
    /// 每次 `resume_capability` 收到的參照。
    capability_queries: Vec<SessionRef>,
    /// `continue_capability` 的回答（票 05）：預設 `Resumable`（同帳號重開照常接續）。
    continue_capability: Capability,
}

impl Default for FakeAgentAdapter {
    fn default() -> Self {
        Self {
            state: Mutex::new(AdapterState {
                channels: HashMap::new(),
                next_scripts: VecDeque::new(),
                launched: Vec::new(),
                capability: Capability::Resumable,
                session_capabilities: HashMap::new(),
                capability_queries: Vec::new(),
                continue_capability: Capability::Resumable,
            }),
        }
    }
}

impl FakeAgentAdapter {
    fn channel(&self, run_id: &str) -> SharedChannel {
        lock(&self.state).channels.entry(run_id.to_string()).or_default().clone()
    }

    /// 某個 Run 目前還沒被丟棄的事件串流數（Supervisor 不再需要時應該丟棄）。
    pub fn live_streams(&self, run_id: &str) -> usize {
        lock(&self.state).channels.get(run_id).map_or(0, |channel| Arc::strong_count(channel) - 1)
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

    /// `continue_capability` 的回答（例如 `NotResumable` 模擬「上次開啟後沒有任何對話」）。
    pub fn set_continue_capability(&self, capability: Capability) {
        lock(&self.state).continue_capability = capability;
    }

    pub fn set_capability(&self, capability: Capability) {
        lock(&self.state).capability = capability;
    }

    /// 只改變某個原生 Session 的接續能力（例如模擬原生 Session 遺失）。
    pub fn set_session_capability(&self, native_session_id: &str, capability: Capability) {
        lock(&self.state).session_capabilities.insert(native_session_id.to_string(), capability);
    }

    /// 每次查驗接續能力時收到的參照，依查驗順序。
    pub fn capability_queries(&self) -> Vec<SessionRef> {
        lock(&self.state).capability_queries.clone()
    }

    /// 每次 `launch`（含 Runtime 重建後的重新接上）收到的 RunSpec。
    pub fn launched(&self) -> Vec<RunSpec> {
        lock(&self.state).launched.clone()
    }
}

#[async_trait]
impl AgentAdapter for FakeAgentAdapter {
    /// Controller 宿主：`args` 是宿主子命令、假的 `claude` 與 Run 識別；帳號目錄以 `CLAUDE_CONFIG_DIR` 交付。
    fn controller_spawn_spec(&self, spec: &RunSpec) -> SpawnSpec {
        let args = [baton_domain::controller_cli::HOST_SUBCOMMAND, "fake-claude", spec.run_id.as_str()].map(String::from).to_vec();
        SpawnSpec {
            program: "fake-controller-host".into(),
            args,
            working_dir: spec.working_dir.clone(),
            command_id: spec.command_id.clone(),
            env: spec.config_dir.iter().map(|dir| ("CLAUDE_CONFIG_DIR".to_string(), dir.clone())).collect(),
            ..Default::default()
        }
    }

    fn spawn_spec(&self, spec: &RunSpec) -> SpawnSpec {
        let mut args = vec![spec.run_id.clone()];
        if let Some(native_session_id) = &spec.resume_native_session_id {
            args.extend(["--resume".to_string(), native_session_id.clone()]);
        }
        SpawnSpec {
            program: "fake-agent".into(),
            args,
            working_dir: spec.working_dir.clone(),
            command_id: spec.command_id.clone(),
            env: Vec::new(),
            ..Default::default()
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

    async fn resume_capability(&self, session: &SessionRef) -> PortResult<Capability> {
        let mut state = lock(&self.state);
        state.capability_queries.push(session.clone());
        Ok(state.session_capabilities.get(&session.native_session_id).copied().unwrap_or(state.capability))
    }

    async fn continue_capability(&self, _config_dir: &str, _working_dir: &str) -> PortResult<Capability> {
        Ok(lock(&self.state).continue_capability)
    }
}

/// 以 profile 為鍵、樣本腳本化的額度觀測器；沒有腳本的 profile 回傳錯誤。記錄每個 profile 被輪詢的次數。
#[derive(Default)]
pub struct FakeQuotaObserver {
    samples: Mutex<HashMap<String, QuotaSample>>,
    calls: Mutex<HashMap<String, usize>>,
}

impl FakeQuotaObserver {
    pub fn set_sample(&self, profile: &str, sample: QuotaSample) {
        lock(&self.samples).insert(profile.to_string(), sample);
    }

    /// 某個 profile 被 `observe` 的次數。
    pub fn observe_count(&self, profile: &str) -> usize {
        lock(&self.calls).get(profile).copied().unwrap_or(0)
    }
}

#[async_trait]
impl QuotaObserver for FakeQuotaObserver {
    async fn observe(&self, profile: &ProfileRef) -> PortResult<QuotaSample> {
        *lock(&self.calls).entry(profile.0.clone()).or_default() += 1;
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
/// worktree 只在記憶體中追蹤（不依賴 git）：目錄是真的建立在暫存目錄中，Git 的狀態（哪些目錄是 repo、
/// 簽出哪個分支、分支在哪個 commit、整合合併的結果）則由測試腳本化，並記錄每一次合併（票 20）。
/// 移除 worktree 與正式版步驟相同：真的把目錄改名到暫放目錄再刪除，可以腳本化失敗或在中途當掉（票 21）。
#[derive(Default)]
pub struct FakeWorkspaceFs {
    worktrees: Mutex<HashSet<PathBuf>>,
    /// worktree 目錄 → 未保存的變更與未追蹤的檔案。
    unsaved: Mutex<HashMap<PathBuf, Vec<String>>>,
    artifact_hang: Mutex<Option<ArtifactWritePoint>>,
    git: Mutex<FakeGit>,
    /// `hold_next_merge` 設定的合併走到閘門時通知（保留一次通知，先到先等都不會漏）
    merge_held: tokio::sync::Notify,
    /// `hold_next_removal` 設定的移除走到閘門時通知（同上）
    removal_held: tokio::sync::Notify,
    /// `hang_next_artifact_write_at` 設定的停住點被走到時通知（同上）
    artifact_hung: tokio::sync::Notify,
}

/// 腳本化的 Git 狀態。
#[derive(Default)]
struct FakeGit {
    /// repo 頂層目錄 → 目前簽出的分支（detached HEAD 為 `None`）。
    repos: HashMap<PathBuf, Option<String>>,
    /// （repo、分支）→ 分支所在的 commit。
    tips: HashMap<(PathBuf, String), String>,
    /// worktree 目錄 → 簽出的分支。
    worktree_branches: HashMap<PathBuf, String>,
    /// 下一次（依序）建立 worktree 的失敗原因。
    worktree_failures: VecDeque<String>,
    /// 下一次（依序）合併的衝突內容。
    merge_conflicts: VecDeque<String>,
    /// 下一次合併在開始後停住，直到閘門放行（只生效一次）；永不放行就等同 Runtime 在合併中崩潰。
    merge_hold: Option<Arc<tokio::sync::Notify>>,
    merges: Vec<FakeMerge>,
    commits: u32,
    /// 下一次（依序）移除 worktree 的失敗原因（例如檔案被占用）；失敗時什麼都不變。
    removal_failures: VecDeque<String>,
    /// 下一次移除在目錄改名之後、刪除之前停住，直到閘門放行（只生效一次）；永不放行就等同 Runtime 在清理中崩潰。
    removal_hold: Option<Arc<tokio::sync::Notify>>,
    /// 至今被要求移除的 worktree 目錄，依順序。
    removals: Vec<PathBuf>,
    /// （repo、from、to）→ 兩個 revision 之間變更的檔案（票 44）；沒有腳本化的組合視為 revision 不存在。
    diffs: HashMap<(PathBuf, String, String), Vec<String>>,
}

/// 一次整合合併的紀錄。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeMerge {
    pub repo: PathBuf,
    pub target_branch: String,
    pub branch: String,
    pub integration_dir: PathBuf,
    /// 合併當時目標分支所在的 commit。
    pub target_tip: String,
    /// 合併結果；衝突時是 `None`。
    pub commit: Option<String>,
}

impl FakeWorkspaceFs {
    /// 把目錄當成 Git repo 的頂層，簽出 `branch`（分支在 commit `base`）。
    pub fn init_repo(&self, path: &Path, branch: &str) {
        let mut git = lock(&self.git);
        git.repos.insert(path.to_path_buf(), Some(branch.to_string()));
        git.tips.insert((path.to_path_buf(), branch.to_string()), "base".to_string());
    }

    /// repo 改為 detached HEAD。
    pub fn detach_head(&self, path: &Path) {
        lock(&self.git).repos.insert(path.to_path_buf(), None);
    }

    /// 分支目前所在的 commit。
    pub fn branch_tip(&self, repo: &Path, branch: &str) -> Option<String> {
        lock(&self.git).tips.get(&(repo.to_path_buf(), branch.to_string())).cloned()
    }

    /// 模擬整合期間有人更新了分支。
    pub fn move_branch(&self, repo: &Path, branch: &str, commit: &str) {
        lock(&self.git).tips.insert((repo.to_path_buf(), branch.to_string()), commit.to_string());
    }

    /// worktree 目錄簽出的分支。
    pub fn worktree_branch(&self, target: &Path) -> Option<String> {
        lock(&self.git).worktree_branches.get(target).cloned()
    }

    /// 腳本化 `repo` 中 `from` 到 `to` 之間變更的檔案（票 44）。
    pub fn set_changed_paths(&self, repo: &Path, from: &str, to: &str, paths: &[&str]) {
        lock(&self.git)
            .diffs
            .insert((repo.to_path_buf(), from.to_string(), to.to_string()), paths.iter().map(|p| p.to_string()).collect());
    }

    /// 下一次建立 worktree 以這個原因失敗（只生效一次）。
    pub fn fail_next_worktree(&self, reason: &str) {
        lock(&self.git).worktree_failures.push_back(reason.to_string());
    }

    /// 下一次合併發生衝突（只生效一次）。
    pub fn conflict_next_merge(&self, detail: &str) {
        lock(&self.git).merge_conflicts.push_back(detail.to_string());
    }

    /// 下一次合併在開始後停住，直到回傳的 `Notify` 被 `notify_one`（只生效一次）。
    /// 不放行並丟棄呼叫端的 future，就等同 Runtime 在合併中崩潰。
    pub fn hold_next_merge(&self) -> Arc<tokio::sync::Notify> {
        let gate = Arc::new(tokio::sync::Notify::new());
        lock(&self.git).merge_hold = Some(gate.clone());
        gate
    }

    /// 等到 `hold_next_merge` 設定的合併走到閘門、停住等待放行。
    pub async fn wait_for_held_merge(&self) {
        self.merge_held.notified().await;
    }

    /// 至今的全部整合合併，依順序。
    pub fn merges(&self) -> Vec<FakeMerge> {
        lock(&self.git).merges.clone()
    }

    /// worktree 中有未保存的變更或未追蹤的檔案（清理前的核對會看到它們）。
    pub fn mark_unsaved(&self, target: &Path, files: &[&str]) {
        lock(&self.unsaved).insert(target.to_path_buf(), files.iter().map(|f| f.to_string()).collect());
    }

    /// 未保存的變更已被使用者處理掉。
    pub fn clear_unsaved(&self, target: &Path) {
        lock(&self.unsaved).remove(target);
    }

    /// 目錄不再是 Git 登記的 worktree（例如使用者手動刪除後 Git 的登記被清掉）。
    pub fn forget_worktree(&self, target: &Path) {
        lock(&self.worktrees).remove(target);
        lock(&self.git).worktree_branches.remove(target);
    }

    /// 下一次移除 worktree 以這個原因失敗（只生效一次），例如模擬 Windows 的檔案占用。
    pub fn fail_next_removal(&self, reason: &str) {
        lock(&self.git).removal_failures.push_back(reason.to_string());
    }

    /// 下一次移除在目錄改名之後、刪除之前停住，直到回傳的 `Notify` 被 `notify_one`（只生效一次）。
    /// 不放行並丟棄呼叫端的 future，就等同 Runtime 在清理中崩潰。
    pub fn hold_next_removal(&self) -> Arc<tokio::sync::Notify> {
        let gate = Arc::new(tokio::sync::Notify::new());
        lock(&self.git).removal_hold = Some(gate.clone());
        gate
    }

    /// 等到 `hold_next_removal` 設定的移除走到閘門（目錄已改名、還沒刪除）、停住等待放行。
    pub async fn wait_for_held_removal(&self) {
        self.removal_held.notified().await;
    }

    /// 至今被要求移除的 worktree 目錄（含失敗的嘗試），依順序。
    pub fn removals(&self) -> Vec<PathBuf> {
        lock(&self.git).removals.clone()
    }

    pub fn has_worktree(&self, target: &Path) -> bool {
        lock(&self.worktrees).contains(target)
    }

    /// 下一次成果寫入在指定的點永遠不回來（只生效一次）。
    pub fn hang_next_artifact_write_at(&self, point: ArtifactWritePoint) {
        *lock(&self.artifact_hang) = Some(point);
    }

    /// 等到 `hang_next_artifact_write_at` 設定的停住點已被某次成果寫入走到（之後那次寫入永遠不回來）。
    pub async fn wait_for_artifact_hang(&self) {
        self.artifact_hung.notified().await;
    }

    /// 若下一次成果寫入被設定在這一點當掉，這裡就永遠不回來。
    async fn crash_point(&self, point: ArtifactWritePoint) {
        let hang = {
            let mut armed = lock(&self.artifact_hang);
            (*armed == Some(point)).then(|| armed.take())
        };
        if hang.is_some() {
            self.artifact_hung.notify_one();
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

    async fn inspect_git(&self, path: &Path) -> PortResult<GitState> {
        Ok(match lock(&self.git).repos.get(path) {
            Some(branch) => GitState::Repository { branch: branch.clone() },
            None => GitState::NotRepository,
        })
    }

    async fn create_worktree(&self, repo: &Path, branch: &str, target: &Path) -> PortResult<()> {
        {
            let mut git = lock(&self.git);
            if let Some(reason) = git.worktree_failures.pop_front() {
                return Err(PortError::Failed(reason));
            }
            let Some(Some(head)) = git.repos.get(repo).cloned() else {
                return Err(PortError::Failed(format!("{} 不是簽出分支的 Git repo", repo.display())));
            };
            // 新分支從 repo 目前的 HEAD 建立；已存在時沿用（冪等）
            let key = (repo.to_path_buf(), branch.to_string());
            if !git.tips.contains_key(&key) {
                let base = git.tips.get(&(repo.to_path_buf(), head)).cloned().unwrap_or_else(|| "base".into());
                git.tips.insert(key, base);
            }
            git.worktree_branches.insert(target.to_path_buf(), branch.to_string());
        }
        tokio::fs::create_dir_all(target).await.map_err(|e| PortError::Failed(e.to_string()))?;
        lock(&self.worktrees).insert(target.to_path_buf());
        Ok(())
    }

    async fn merge_for_integration(
        &self,
        repo: &Path,
        target_branch: &str,
        branch: &str,
        integration_dir: &Path,
    ) -> PortResult<MergeOutcome> {
        let gate = lock(&self.git).merge_hold.take();
        if let Some(gate) = gate {
            self.merge_held.notify_one();
            gate.notified().await;
        }
        tokio::fs::create_dir_all(integration_dir).await.map_err(|e| PortError::Failed(e.to_string()))?;
        let mut git = lock(&self.git);
        let target_tip = git
            .tips
            .get(&(repo.to_path_buf(), target_branch.to_string()))
            .cloned()
            .ok_or_else(|| PortError::Failed(format!("找不到分支 {target_branch}")))?;
        let conflict = git.merge_conflicts.pop_front();
        let commit = match &conflict {
            Some(_) => None,
            None => {
                git.commits += 1;
                Some(format!("merge-{}", git.commits))
            }
        };
        git.merges.push(FakeMerge {
            repo: repo.to_path_buf(),
            target_branch: target_branch.to_string(),
            branch: branch.to_string(),
            integration_dir: integration_dir.to_path_buf(),
            target_tip: target_tip.clone(),
            commit: commit.clone(),
        });
        Ok(match (conflict, commit) {
            (Some(detail), _) => MergeOutcome::Conflict { detail },
            (None, Some(commit)) => MergeOutcome::Merged { commit, target_tip },
            (None, None) => unreachable!("沒有衝突一定有合併結果"),
        })
    }

    async fn advance_branch(
        &self,
        repo: &Path,
        target_branch: &str,
        expected_tip: &str,
        commit: &str,
    ) -> PortResult<AdvanceOutcome> {
        let mut git = lock(&self.git);
        let key = (repo.to_path_buf(), target_branch.to_string());
        let actual = git.tips.get(&key).cloned().unwrap_or_default();
        Ok(if actual == commit {
            AdvanceOutcome::Advanced
        } else if actual != expected_tip {
            AdvanceOutcome::TargetMoved { actual }
        } else {
            git.tips.insert(key, commit.to_string());
            AdvanceOutcome::Advanced
        })
    }

    async fn inspect_worktree(&self, repo: &Path, target: &Path) -> PortResult<WorktreeState> {
        if !target.exists() {
            return Ok(WorktreeState::Missing);
        }
        let registered = target != repo && lock(&self.worktrees).contains(target);
        let Some(branch) = registered.then(|| lock(&self.git).worktree_branches.get(target).cloned()).flatten() else {
            return Ok(WorktreeState::Foreign);
        };
        let unsaved = lock(&self.unsaved).get(target).cloned().unwrap_or_default();
        Ok(WorktreeState::Registered { branch: Some(branch), unsaved })
    }

    async fn changed_paths(&self, repo: &Path, from: &str, to: &str) -> PortResult<Vec<String>> {
        let git = lock(&self.git);
        if !git.repos.contains_key(repo) {
            return Err(PortError::Failed(format!("{} 不是 Git repo", repo.display())));
        }
        if from == to {
            return Ok(Vec::new());
        }
        git.diffs
            .get(&(repo.to_path_buf(), from.to_string(), to.to_string()))
            .cloned()
            .ok_or_else(|| PortError::Failed(format!("{} 中找不到 revision {from} 或 {to}", repo.display())))
    }

    async fn remove_worktree(&self, repo: &Path, target: &Path, trash: &Path) -> PortResult<()> {
        let io = |e: std::io::Error| PortError::Failed(e.to_string());
        let hold = {
            let mut git = lock(&self.git);
            git.removals.push(target.to_path_buf());
            if let Some(reason) = git.removal_failures.pop_front() {
                return Err(PortError::Failed(reason));
            }
            git.removal_hold.take()
        };
        // 與正式版相同：只移除 repo 登記的 worktree，先改名到暫放目錄（Git 登記一併清掉），再刪除；不動分支
        if target.exists() {
            if target == repo || !lock(&self.worktrees).contains(target) {
                return Err(PortError::Failed(format!("{} 不是平台登記的 worktree，拒絕移除", target.display())));
            }
            if trash.exists() {
                tokio::fs::remove_dir_all(trash).await.map_err(io)?;
            }
            if let Some(parent) = trash.parent() {
                tokio::fs::create_dir_all(parent).await.map_err(io)?;
            }
            tokio::fs::rename(target, trash).await.map_err(io)?;
        }
        lock(&self.worktrees).remove(target);
        lock(&self.unsaved).remove(target);
        lock(&self.git).worktree_branches.remove(target);
        if let Some(gate) = hold {
            self.removal_held.notify_one();
            gate.notified().await;
        }
        match tokio::fs::remove_dir_all(trash).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(io(e)),
        }
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

    async fn read_workspace_file(&self, path: &Path) -> PortResult<Option<Vec<u8>>> {
        baton_ports::read_file_if_exists(path)
    }

    async fn read_file_within(&self, root: &Path, relative: &Path) -> PortResult<baton_ports::ContainedFile> {
        baton_ports::read_file_within(root, relative)
    }

    async fn list_artifact_files(&self, dir: &Path) -> PortResult<Vec<FileEntry>> {
        baton_ports::list_files_recursive(dir)
    }

    async fn write_readonly_files(&self, dir: &Path, files: &[(String, Vec<u8>)]) -> PortResult<()> {
        baton_ports::write_readonly_tree(dir, files)
    }

    async fn create_file_if_absent(&self, path: &Path, bytes: &[u8]) -> PortResult<bool> {
        baton_ports::create_file_if_absent(path, bytes)
    }
}

/// 使用者平常使用的 Claude Code 設定：一個暫存的「家目錄」，內含 `.claude\` 與 `.claude.json`。
///
/// 讀取走與正式版相同的 `DirUserSettingsSource`（真實讀檔），所以「原始設定檔位元組不變」可以直接比對檔案。
/// 測試可以在其中放置登入資料（`.credentials.json`）來證明它永遠不會被讀取。
pub struct FakeUserSettings {
    home: tempfile::TempDir,
    source: DirUserSettingsSource,
}

impl Default for FakeUserSettings {
    fn default() -> Self {
        let home = tempfile::tempdir().expect("無法建立暫存的使用者家目錄");
        let source = DirUserSettingsSource::new(home.path().join(".claude"), home.path().join(".claude.json"));
        std::fs::create_dir_all(home.path().join(".claude")).expect("無法建立 .claude");
        Self { home, source }
    }
}

impl FakeUserSettings {
    pub fn home(&self) -> &Path {
        self.home.path()
    }

    /// 寫入 `.claude\` 之下的檔案（模擬使用者自己編輯設定）。
    pub fn write_in_config_dir(&self, name: &str, content: &str) {
        std::fs::write(self.home.path().join(".claude").join(name), content).expect("無法寫入設定檔");
    }

    pub fn write_settings(&self, json: &str) {
        self.write_in_config_dir("settings.json", json);
    }

    pub fn write_instructions(&self, text: &str) {
        self.write_in_config_dir("CLAUDE.md", text);
    }

    /// `.claude.json`（在家目錄，與 `.claude\` 同層）。
    pub fn write_user_state(&self, json: &str) {
        std::fs::write(self.home.path().join(".claude.json"), json).expect("無法寫入 .claude.json");
    }

    /// 家目錄下全部檔案的相對路徑與位元組；用來證明匯入前後原始設定檔沒有任何變化（也沒有多出檔案）。
    pub fn files(&self) -> std::collections::BTreeMap<String, Vec<u8>> {
        fn walk(root: &Path, dir: &Path, out: &mut std::collections::BTreeMap<String, Vec<u8>>) {
            for entry in std::fs::read_dir(dir).expect("無法列出目錄") {
                let path = entry.expect("目錄項目").path();
                if path.is_dir() {
                    walk(root, &path, out);
                } else {
                    let relative = path.strip_prefix(root).expect("在根目錄之下").to_string_lossy().replace('\\', "/");
                    out.insert(relative, std::fs::read(&path).expect("無法讀取檔案"));
                }
            }
        }
        let mut out = std::collections::BTreeMap::new();
        walk(self.home.path(), self.home.path(), &mut out);
        out
    }
}

#[async_trait]
impl UserSettingsSource for FakeUserSettings {
    async fn read(&self) -> PortResult<SourceFiles> {
        self.source.read().await
    }
}

/// 腳本化的驗證命令執行器：依程式名稱回傳設定好的結果（設定一次後每次都相同，可再覆寫）；
/// 沒有設定的程式視為「找不到程式」而無法啟動。記錄每一次執行要求，並可讓下一次執行停住直到測試放行。
#[derive(Default)]
pub struct FakeCommandRunner {
    results: Mutex<HashMap<String, PortResultOf<CommandOutput>>>,
    /// 只在某個工作目錄生效的結果（優先於 `results`），例如讓整合目錄中的檢查與 worktree 中的結果不同。
    results_in: Mutex<HashMap<(String, PathBuf), PortResultOf<CommandOutput>>>,
    calls: Mutex<Vec<CommandRequest>>,
    hold: Mutex<Option<Arc<tokio::sync::Notify>>>,
    /// `hold_next` 設定的執行走到閘門時通知（保留一次通知，先到先等都不會漏）
    held: tokio::sync::Notify,
}

/// `PortError` 不能複製，所以腳本以字串保存錯誤原因。
type PortResultOf<T> = Result<T, String>;

impl FakeCommandRunner {
    /// 這個程式之後每次執行都以 `code` 結束，輸出 `output`。
    pub fn script_exit(&self, program: &str, code: i32, output: &str) {
        lock(&self.results).insert(program.to_string(), Ok(CommandOutput::Exited { code, output: output.to_string() }));
    }

    /// 這個程式之後在 `dir` 執行時都以 `code` 結束（優先於 `script_exit`）。
    pub fn script_exit_in(&self, program: &str, dir: &Path, code: i32, output: &str) {
        let result = Ok(CommandOutput::Exited { code, output: output.to_string() });
        lock(&self.results_in).insert((program.to_string(), dir.to_path_buf()), result);
    }

    /// 這個程式之後每次執行都逾時。
    pub fn script_timeout(&self, program: &str) {
        lock(&self.results).insert(program.to_string(), Ok(CommandOutput::TimedOut { output: String::new() }));
    }

    /// 這個程式之後每次都無法啟動。
    pub fn script_start_failure(&self, program: &str, reason: &str) {
        lock(&self.results).insert(program.to_string(), Err(reason.to_string()));
    }

    /// 下一次執行在開始後停住，直到回傳的 `Notify` 被 `notify_one`（只生效一次）。
    pub fn hold_next(&self) -> Arc<tokio::sync::Notify> {
        let gate = Arc::new(tokio::sync::Notify::new());
        *lock(&self.hold) = Some(gate.clone());
        gate
    }

    /// 等到 `hold_next` 設定的執行走到閘門、停住等待放行。
    pub async fn wait_for_held(&self) {
        self.held.notified().await;
    }

    /// 至今的全部執行要求，依順序。
    pub fn calls(&self) -> Vec<CommandRequest> {
        lock(&self.calls).clone()
    }
}

#[async_trait]
impl CommandRunner for FakeCommandRunner {
    async fn run(&self, request: CommandRequest) -> PortResult<CommandOutput> {
        let program = request.program.clone();
        let in_dir = lock(&self.results_in).get(&(program.clone(), request.working_dir.clone())).cloned();
        lock(&self.calls).push(request);
        let gate = lock(&self.hold).take();
        if let Some(gate) = gate {
            self.held.notify_one();
            gate.notified().await;
        }
        match in_dir.or_else(|| lock(&self.results).get(&program).cloned()) {
            Some(Ok(output)) => Ok(output),
            Some(Err(reason)) => Err(PortError::Failed(reason)),
            None => Err(PortError::Failed(format!("找不到程式「{program}」"))),
        }
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
    /// 開出的 Controller 宿主登記在這裡，Runtime 才能以 ProcessControl 找回、查驗與停止它（與正式版相同的契約）。
    processes: Option<Arc<FakeProcessControl>>,
    /// 開 Controller 終端機走到停住點時喚醒等待中的測試。
    activity: tokio::sync::Notify,
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
    /// 已開啟的 Controller 終端機（依序記錄完整的啟動規格）。
    controller_terminals: Vec<TerminalLaunch>,
    /// 下一次開 Controller 終端機失敗（沒有開出任何視窗），只生效一次。
    fail_terminal: Option<String>,
    /// 下一次開 Controller 終端機：視窗與宿主已開出，但呼叫永遠不回來（模擬在開窗與回執之間當掉）。
    hang_terminal: bool,
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
                controller_terminals: Vec::new(),
                fail_terminal: None,
                hang_terminal: false,
            }),
            processes: None,
            activity: tokio::sync::Notify::new(),
        }
    }
}

fn dir_key(dir: &Path) -> String {
    dir.to_string_lossy().replace('/', "\\").trim_end_matches('\\').to_lowercase()
}

impl FakeClaudeEnvironment {
    /// 開出的 Controller 宿主登記到 `processes`（harness 的組態：與 Runtime 共用同一個 fake ProcessControl）。
    pub fn with_processes(processes: Arc<FakeProcessControl>) -> Self {
        Self { processes: Some(processes), ..Self::default() }
    }

    /// 已開啟的 Controller 終端機的啟動規格，依開啟順序。
    pub fn controller_terminals(&self) -> Vec<TerminalLaunch> {
        lock(&self.state).controller_terminals.clone()
    }

    /// 下一次開 Controller 終端機失敗（例如終端機開不起來），只生效一次。
    pub fn fail_next_controller_terminal(&self, reason: &str) {
        lock(&self.state).fail_terminal = Some(reason.to_string());
    }

    /// 下一次開 Controller 終端機時，視窗與宿主都開出了，但呼叫永遠不回來，只生效一次。
    pub fn hang_next_controller_terminal(&self) {
        lock(&self.state).hang_terminal = true;
    }

    /// 等到 `hang_next_controller_terminal` 設定的停住點已被走到。
    pub async fn wait_for_terminal_hang(&self) {
        loop {
            let mut changed = std::pin::pin!(self.activity.notified());
            changed.as_mut().enable();
            if !lock(&self.state).hang_terminal {
                return;
            }
            changed.await;
        }
    }

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

    /// 與正式版相同的契約：同一個 command_id 已有程序時拒絕；開出的宿主放進以 command_id 識別的程序群組。
    async fn open_controller_terminal(&self, launch: &TerminalLaunch) -> PortResult<ProcessIdentity> {
        let hang = {
            let mut state = lock(&self.state);
            state.controller_terminals.push(launch.clone());
            if let Some(reason) = state.fail_terminal.take() {
                return Err(PortError::Failed(reason));
            }
            std::mem::take(&mut state.hang_terminal)
        };
        let identity = match &self.processes {
            Some(processes) => {
                if processes.find(&launch.spawn).await?.is_some() {
                    return Err(PortError::Failed(format!("command_id {} 已有對應的程序，拒絕重複開窗", launch.spawn.command_id)));
                }
                processes.register_external(&launch.spawn)
            }
            None => ProcessIdentity { pid: 1, created_at_millis: 1, command_hash: command_hash(&launch.spawn) },
        };
        if hang {
            self.activity.notify_waiters();
            std::future::pending::<()>().await;
        }
        Ok(identity)
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

/// 監護程序是否還在（票 32）：Harness 的程序內監護啟動時設為 true，被殺掉時設為 false。
#[derive(Debug, Default)]
pub struct FakeGuardianProbe {
    running: std::sync::atomic::AtomicBool,
}

impl FakeGuardianProbe {
    pub fn set_running(&self, running: bool) {
        self.running.store(running, std::sync::atomic::Ordering::SeqCst);
    }
}

impl GuardianProbe for FakeGuardianProbe {
    fn guardian_running(&self) -> bool {
        self.running.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// 目前 Windows 登入工作階段的識別（票 33）。預設是固定的第一次登入；`sign_in_again` 換成新的一次登入，
/// `set` 可設成取不到（`None`）。
#[derive(Debug)]
pub struct FakeLoginSession {
    state: std::sync::Mutex<(u32, Option<String>)>,
}

impl Default for FakeLoginSession {
    fn default() -> Self {
        Self { state: std::sync::Mutex::new((1, Some("login-1".to_string()))) }
    }
}

impl FakeLoginSession {
    /// 換成新的一次登入（重新登入或重新開機後），回傳新的識別。
    pub fn sign_in_again(&self) -> String {
        let mut state = self.state.lock().unwrap();
        state.0 += 1;
        let id = format!("login-{}", state.0);
        state.1 = Some(id.clone());
        id
    }

    /// 直接設定目前的識別；`None` 表示取不到。
    pub fn set(&self, id: Option<&str>) {
        self.state.lock().unwrap().1 = id.map(str::to_string);
    }
}

impl LoginSession for FakeLoginSession {
    fn current_login(&self) -> Option<String> {
        self.state.lock().unwrap().1.clone()
    }
}

/// 「登入時自動啟動」的 Windows 登記（票 33）：記下是否已登記與每次變更；可讓下一次變更失敗。
#[derive(Debug, Default)]
pub struct FakeStartupRegistration {
    registered: std::sync::atomic::AtomicBool,
    /// 同名但命令列不同的舊登記（例如舊安裝位置留下的）。
    stale: std::sync::atomic::AtomicBool,
    fail_next: std::sync::Mutex<Option<String>>,
    changes: std::sync::Mutex<Vec<bool>>,
}

impl FakeStartupRegistration {
    pub fn registered(&self) -> bool {
        self.registered.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// 不經 Runtime 直接改變登記（例如使用者手動刪掉登錄值）。
    pub fn force(&self, registered: bool) {
        self.registered.store(registered, std::sync::atomic::Ordering::SeqCst);
    }

    /// 不經 Runtime 留下一筆同名、但命令列是舊安裝位置的登記（不算已登記）。
    pub fn leave_stale(&self) {
        self.force(false);
        self.stale.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// 是否還留著舊安裝位置的登記。
    pub fn stale(&self) -> bool {
        self.stale.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// 下一次 `set_registered` 以這個原因失敗。
    pub fn fail_next(&self, reason: &str) {
        *self.fail_next.lock().unwrap() = Some(reason.to_string());
    }

    /// 成功的 `set_registered` 呼叫，依時間順序。
    pub fn changes(&self) -> Vec<bool> {
        self.changes.lock().unwrap().clone()
    }
}

impl StartupRegistration for FakeStartupRegistration {
    fn is_registered(&self) -> PortResult<bool> {
        Ok(self.registered())
    }

    fn has_registration(&self) -> PortResult<bool> {
        Ok(self.registered() || self.stale())
    }

    fn set_registered(&self, enabled: bool) -> PortResult<()> {
        if let Some(reason) = self.fail_next.lock().unwrap().take() {
            return Err(PortError::Failed(reason));
        }
        // 同名的值只有一個：登記時覆寫、移除時刪掉
        self.stale.store(false, std::sync::atomic::Ordering::SeqCst);
        self.force(enabled);
        self.changes.lock().unwrap().push(enabled);
        Ok(())
    }
}

/// 簽章更新的來源（票 34）。預設是「未設定更新來源」；`publish` 之後回報那個版本。
#[derive(Default)]
pub struct FakeUpdateSource {
    latest: std::sync::Mutex<Option<baton_domain::maintenance::ReleaseInfo>>,
    fail_next_check: std::sync::Mutex<Option<String>>,
    reject_signature: std::sync::atomic::AtomicBool,
    checks: std::sync::atomic::AtomicUsize,
    downloads: std::sync::Mutex<Vec<String>>,
}

impl FakeUpdateSource {
    /// 更新來源公布 `version` 為最新版本。
    pub fn publish(&self, version: &str) {
        *self.latest.lock().unwrap() = Some(baton_domain::maintenance::ReleaseInfo {
            version: version.to_string(),
            notes: Some(format!("Baton {version} 的更新說明")),
            published_at: Some("2026-10-22T00:00:00Z".to_string()),
            url: format!("https://updates.example.invalid/Baton_{version}_x64-setup.exe"),
            signature: format!("fake-signature-{version}"),
        });
    }

    /// 下一次查詢以這個原因失敗（例如離線）。
    pub fn fail_next_check(&self, reason: &str) {
        *self.fail_next_check.lock().unwrap() = Some(reason.to_string());
    }

    /// 之後下載的安裝程式簽章都不正確（被竄改或公鑰不符）。
    pub fn reject_signature(&self, reject: bool) {
        self.reject_signature.store(reject, std::sync::atomic::Ordering::SeqCst);
    }

    /// 查詢最新版本的次數。
    pub fn checks(&self) -> usize {
        self.checks.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// 下載並通過簽章驗證的版本，依時間順序。
    pub fn downloads(&self) -> Vec<String> {
        self.downloads.lock().unwrap().clone()
    }
}

#[async_trait]
impl UpdateSource for FakeUpdateSource {
    async fn latest(&self) -> PortResult<baton_domain::maintenance::ReleaseInfo> {
        self.checks.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if let Some(reason) = self.fail_next_check.lock().unwrap().take() {
            return Err(PortError::Failed(reason));
        }
        self.latest.lock().unwrap().clone().ok_or(PortError::NotImplemented("UpdateSource"))
    }

    async fn download(
        &self,
        release: &baton_domain::maintenance::ReleaseInfo,
        dest_dir: &Path,
    ) -> PortResult<std::path::PathBuf> {
        if self.reject_signature.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(PortError::Failed(format!("安裝程式 {} 的簽章驗證失敗", release.version)));
        }
        std::fs::create_dir_all(dest_dir).map_err(|e| PortError::Failed(e.to_string()))?;
        let path = dest_dir.join(format!("Baton_{}_x64-setup.exe", release.version));
        std::fs::write(&path, b"fake installer").map_err(|e| PortError::Failed(e.to_string()))?;
        self.downloads.lock().unwrap().push(release.version.clone());
        Ok(path)
    }
}

/// Anthropic models API（AI-33）。回應依序取用（`respond` 排入）；沒有排入的回應時視同連不上。
/// 記下每次查詢收到的金鑰與 `after_id`，讓測試確認分頁與金鑰只用在這裡。
#[derive(Default)]
pub struct FakeModelDirectory {
    responses: Mutex<VecDeque<Result<ModelPage, ModelDirectoryError>>>,
    calls: Mutex<Vec<(String, Option<String>)>>,
}

impl FakeModelDirectory {
    /// 排入下一次查詢的回應。
    pub fn respond(&self, response: Result<ModelPage, ModelDirectoryError>) {
        self.responses.lock().unwrap().push_back(response);
    }

    /// 排入一頁成功的回應：`models` 是（id, display_name），`next` 是下一頁的 `last_id`（`None` 表示最後一頁）。
    pub fn respond_page(&self, models: &[(&str, &str)], next: Option<&str>) {
        let data = models
            .iter()
            .map(|(id, name)| RemoteModel { id: (*id).to_string(), display_name: (*name).to_string() })
            .collect();
        self.respond(Ok(ModelPage { data, has_more: next.is_some(), last_id: next.map(str::to_string) }));
    }

    /// 至今收到的查詢：（金鑰, after_id）。
    pub fn calls(&self) -> Vec<(String, Option<String>)> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait]
impl ModelDirectory for FakeModelDirectory {
    async fn page(&self, api_key: &str, after_id: Option<&str>) -> Result<ModelPage, ModelDirectoryError> {
        self.calls.lock().unwrap().push((api_key.to_string(), after_id.map(str::to_string)));
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Err(ModelDirectoryError::Unreachable("沒有預設的回應".into())))
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
    pub command_runner: Arc<FakeCommandRunner>,
    pub user_settings: Arc<FakeUserSettings>,
    pub guardian_probe: Arc<FakeGuardianProbe>,
    pub login_session: Arc<FakeLoginSession>,
    pub startup_registration: Arc<FakeStartupRegistration>,
    pub update_source: Arc<FakeUpdateSource>,
    pub verification_driver: Arc<FakeVerificationDriver>,
    pub browser_driver: Arc<FakeBrowserDriver>,
    pub semantic_judge: Arc<FakeSemanticJudge>,
    pub secret_resolver: Arc<FakeSecretResolver>,
    pub model_directory: Arc<FakeModelDirectory>,
}

impl FakePorts {
    pub fn new(clock_start: Timestamp) -> Self {
        let clock = Arc::new(FakeClock::new(clock_start));
        let process_control: Arc<FakeProcessControl> = Arc::default();
        Self {
            verification_driver: Arc::new(FakeVerificationDriver::new(clock.clone())),
            clock,
            claude_env: Arc::new(FakeClaudeEnvironment::with_processes(process_control.clone())),
            process_control,
            agent_adapter: Arc::default(),
            quota_observer: Arc::default(),
            workspace_fs: Arc::default(),
            platform_info: Arc::default(),
            secret_protector: Arc::default(),
            command_runner: Arc::default(),
            user_settings: Arc::default(),
            guardian_probe: Arc::default(),
            login_session: Arc::default(),
            startup_registration: Arc::default(),
            update_source: Arc::default(),
            browser_driver: Arc::default(),
            semantic_judge: Arc::default(),
            secret_resolver: Arc::default(),
            model_directory: Arc::default(),
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
            command_runner: self.command_runner.clone(),
            user_settings: self.user_settings.clone(),
            guardian_probe: self.guardian_probe.clone(),
            login_session: self.login_session.clone(),
            startup_registration: self.startup_registration.clone(),
            update_source: self.update_source.clone(),
            verification_driver: self.verification_driver.clone(),
            browser_driver: self.browser_driver.clone(),
            semantic_judge: self.semantic_judge.clone(),
            secret_resolver: self.secret_resolver.clone(),
            model_directory: self.model_directory.clone(),
        }
    }
}

/// 腳本化的 CLI／API 驗證 Adapter 的 Port（票 45）。
///
/// - 命令與 HTTP 的結果依序取用；只剩最後一個時之後每次都相同（例如就緒條件先失敗兩次再成功）。
///   命令先找「程式 + 引數」完全相同的腳本，再找只有程式名稱的腳本；沒有設定的命令視為找不到程式，
///   沒有設定的 HTTP 端點視為連不上。
/// - 等待（就緒重試的間隔）推進 fake 時鐘，不真的等待。
/// - 狀態目錄是暫存目錄下的真實目錄，測試可以直接檢查它是否還在。
/// - 記錄每一個操作（`operations`），讓測試斷言 teardown、服務停止等外部行為。
pub struct FakeVerificationDriver {
    clock: Arc<FakeClock>,
    commands: Mutex<HashMap<String, VecDeque<PortResultOf<CommandOutput>>>>,
    http: Mutex<HashMap<(String, String), VecDeque<HttpOutcome>>>,
    revisions: Mutex<HashMap<PathBuf, String>>,
    service_failures: Mutex<HashMap<String, String>>,
    running: Mutex<HashSet<String>>,
    next_service: AtomicU32,
    operations: Mutex<Vec<String>>,
    http_requests: Mutex<Vec<HttpRequest>>,
    hangs: Mutex<HashSet<(String, String)>>,
    /// `hang_next_http` 設定的請求送出、停住時通知（保留一次通知，先到先等都不會漏）
    http_hung: tokio::sync::Notify,
    stop_failures: Mutex<HashMap<String, String>>,
}

impl FakeVerificationDriver {
    pub fn new(clock: Arc<FakeClock>) -> Self {
        Self {
            clock,
            commands: Mutex::default(),
            http: Mutex::default(),
            revisions: Mutex::default(),
            service_failures: Mutex::default(),
            running: Mutex::default(),
            next_service: AtomicU32::new(1),
            operations: Mutex::default(),
            http_requests: Mutex::default(),
            hangs: Mutex::default(),
            http_hung: tokio::sync::Notify::new(),
            stop_failures: Mutex::default(),
        }
    }

    /// 之後停止這個服務都失敗（服務仍在執行），例如它是前一個 Runtime 啟動、無法確認的程序（票 49）。
    pub fn fail_stop(&self, handle: &str, reason: &str) {
        lock(&self.stop_failures).insert(handle.to_string(), reason.to_string());
    }

    /// 下一次送往這個端點的請求送出後永遠不回來（已記錄為送出）；測試等 `wait_for_http_hang` 後丟棄該次處理再重啟，
    /// 即等同 Runtime 在請求送出之後、拿到結果之前崩潰（票 49）。
    pub fn hang_next_http(&self, method: &str, url: &str) {
        lock(&self.hangs).insert((method.to_string(), url.to_string()));
    }

    /// 等到 `hang_next_http` 設定的請求已送出、停住。
    pub async fn wait_for_http_hang(&self) {
        self.http_hung.notified().await;
    }

    fn command_key(program: &str, args: &[&str]) -> String {
        std::iter::once(program).chain(args.iter().copied()).collect::<Vec<_>>().join(" ")
    }

    /// 這個命令（`args` 空的時候是這個程式的任何引數）依序以這些結果結束。
    pub fn script_command(&self, program: &str, args: &[&str], outputs: Vec<CommandOutput>) {
        let key = Self::command_key(program, args);
        lock(&self.commands).insert(key, outputs.into_iter().map(Ok).collect());
    }

    /// 這個命令每次都以 `code` 結束、輸出 `output`。
    pub fn script_exit(&self, program: &str, args: &[&str], code: i32, output: &str) {
        self.script_command(program, args, vec![CommandOutput::Exited { code, output: output.to_string() }]);
    }

    /// 這個命令每次都無法啟動。
    pub fn script_start_failure(&self, program: &str, args: &[&str], reason: &str) {
        lock(&self.commands).insert(Self::command_key(program, args), VecDeque::from([Err(reason.to_string())]));
    }

    /// 這個端點依序回應這些結果。
    pub fn script_http(&self, method: &str, url: &str, outcomes: Vec<HttpOutcome>) {
        lock(&self.http).insert((method.to_string(), url.to_string()), outcomes.into());
    }

    /// 這個端點每次都以 `status` 與 `body` 回應。
    pub fn script_response(&self, method: &str, url: &str, status: u16, body: &str) {
        self.script_http(method, url, vec![HttpOutcome::Responded { status, body: body.to_string() }]);
    }

    pub fn set_revision(&self, dir: &Path, commit: &str) {
        lock(&self.revisions).insert(dir.to_path_buf(), commit.to_string());
    }

    /// 這個程式之後無法以服務啟動。
    pub fn fail_service(&self, program: &str, reason: &str) {
        lock(&self.service_failures).insert(program.to_string(), reason.to_string());
    }

    /// 目前還在執行的服務。
    pub fn running_services(&self) -> Vec<String> {
        let mut running: Vec<String> = lock(&self.running).iter().cloned().collect();
        running.sort();
        running
    }

    /// 至今的全部操作，依順序（例如 `command ledger add`、`http POST http://…`、`start ledger serve`、`stop svc-1`）。
    pub fn operations(&self) -> Vec<String> {
        lock(&self.operations).clone()
    }

    /// 至今送出的全部 HTTP 請求（含標頭與本文）。
    pub fn http_requests(&self) -> Vec<HttpRequest> {
        lock(&self.http_requests).clone()
    }

    fn next<T: Clone>(queue: &mut VecDeque<T>) -> Option<T> {
        if queue.len() > 1 { queue.pop_front() } else { queue.front().cloned() }
    }
}

#[async_trait]
impl VerificationDriver for FakeVerificationDriver {
    async fn run_command(&self, request: CommandRequest) -> PortResult<CommandOutput> {
        let args: Vec<&str> = request.args.iter().map(String::as_str).collect();
        let exact = Self::command_key(&request.program, &args);
        lock(&self.operations).push(format!("command {exact}"));
        let mut commands = lock(&self.commands);
        let scripted = match commands.get_mut(&exact) {
            Some(queue) => Self::next(queue),
            None => commands.get_mut(&request.program).and_then(Self::next),
        };
        match scripted {
            Some(Ok(output)) => Ok(output),
            Some(Err(reason)) => Err(PortError::Failed(reason)),
            None => Err(PortError::Failed(format!("找不到程式「{}」", request.program))),
        }
    }

    async fn send_http(&self, request: HttpRequest) -> PortResult<HttpOutcome> {
        lock(&self.operations).push(format!("http {} {}", request.method, request.url));
        let key = (request.method.clone(), request.url.clone());
        lock(&self.http_requests).push(request);
        if lock(&self.hangs).remove(&key) {
            self.http_hung.notify_one();
            std::future::pending::<()>().await;
        }
        let outcome = lock(&self.http).get_mut(&key).and_then(Self::next);
        Ok(outcome.unwrap_or(HttpOutcome::Unreachable { reason: "連線被拒（沒有服務在這個位址）".into() }))
    }

    async fn start_service(&self, request: ServiceRequest) -> PortResult<ServiceHandle> {
        let line = std::iter::once(request.program.as_str()).chain(request.args.iter().map(String::as_str)).collect::<Vec<_>>();
        lock(&self.operations).push(format!("start {}", line.join(" ")));
        if let Some(reason) = lock(&self.service_failures).get(&request.program).cloned() {
            return Err(PortError::Failed(reason));
        }
        let id = format!("svc-{}", self.next_service.fetch_add(1, Ordering::SeqCst));
        lock(&self.running).insert(id.clone());
        Ok(ServiceHandle(id))
    }

    async fn stop_service(&self, handle: &ServiceHandle) -> PortResult<()> {
        lock(&self.operations).push(format!("stop {}", handle.0));
        if let Some(reason) = lock(&self.stop_failures).get(&handle.0).cloned() {
            return Err(PortError::Failed(reason));
        }
        lock(&self.running).remove(&handle.0);
        Ok(())
    }

    async fn pause(&self, duration: Duration) {
        self.clock.advance_millis(duration.as_millis() as i64);
    }

    async fn revision(&self, dir: &Path) -> PortResult<Option<String>> {
        Ok(lock(&self.revisions).get(dir).cloned())
    }

    async fn prepare_state_dir(&self, dir: &Path) -> PortResult<()> {
        lock(&self.operations).push(format!("prepare {}", dir.display()));
        if dir.exists() {
            std::fs::remove_dir_all(dir).map_err(|e| PortError::Failed(e.to_string()))?;
        }
        std::fs::create_dir_all(dir).map_err(|e| PortError::Failed(e.to_string()))
    }

    async fn remove_state_dir(&self, dir: &Path) -> PortResult<()> {
        lock(&self.operations).push(format!("remove {}", dir.display()));
        match std::fs::remove_dir_all(dir) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(PortError::Failed(error.to_string())),
            _ => Ok(()),
        }
    }
}

/// 腳本化網站上的一個畫面（票 46）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakePage {
    /// 導覽到這個畫面時的 HTTP 狀態碼。
    pub status: u16,
    pub title: String,
    pub text: String,
    /// 畫面上可見（也就是可以操作）的 selector。
    pub visible: Vec<String>,
}

impl FakePage {
    pub fn new(title: &str, text: &str, visible: &[&str]) -> Self {
        Self { status: 200, title: title.into(), text: text.into(), visible: visible.iter().map(|s| s.to_string()).collect() }
    }

    /// 以這個狀態碼回應的畫面（例如 403 權限不足）。
    pub fn with_status(mut self, status: u16) -> Self {
        self.status = status;
        self
    }
}

/// 畫面的內容可以依這個 Session 目前為止輸入過的值（selector, 文字）決定，例如以哪個帳號登入。
type PageFn = Arc<dyn Fn(&[(String, String)]) -> FakePage + Send + Sync>;

struct FakeBrowserSessionState {
    request: BrowserSessionRequest,
    url: Option<String>,
    status: Option<u16>,
    fills: Vec<(String, String)>,
    log: Vec<String>,
}

/// 腳本化的瀏覽器 Runner（票 46）：扮演受驗的網站。
///
/// - `script_page`：網址對應的畫面（狀態碼預設 200）；沒有腳本的網址連不上。
/// - `script_link`：點擊或按鍵於某個 selector 之後前往某個網址（例如登入按鈕）。
/// - 操作的 selector 不在目前畫面的 `visible` 中時，回報 `TimeoutError`（如同 Playwright 等不到元素）。
/// - 截圖的內容是「網址 + 畫面文字」，trace 是這個 Session 的完整操作紀錄（含輸入的文字，如同真實引擎的 trace），
///   錄影是固定內容；讓測試檢查 Runtime 是否依政策遮蔽或不保存。
/// - `commands` 記錄每一個操作（`fill` 記下實際輸入的文字），讓測試確認秘密的值確實交給了 Runner。
/// - `set_engine`：Session 回報的引擎（票 48 錄製身分的一部分），預設 `fake-browser 1`。
pub struct FakeBrowserDriver {
    pages: Mutex<HashMap<String, PageFn>>,
    links: Mutex<HashMap<String, String>>,
    open_failure: Mutex<Option<String>>,
    sessions: Mutex<HashMap<String, FakeBrowserSessionState>>,
    next_session: AtomicU32,
    commands: Mutex<Vec<String>>,
    requests: Mutex<Vec<BrowserSessionRequest>>,
    masks: Mutex<Vec<Vec<String>>>,
    engine: Mutex<Option<String>>,
}

impl Default for FakeBrowserDriver {
    fn default() -> Self {
        Self {
            pages: Mutex::default(),
            links: Mutex::default(),
            open_failure: Mutex::default(),
            sessions: Mutex::default(),
            next_session: AtomicU32::new(1),
            commands: Mutex::default(),
            requests: Mutex::default(),
            masks: Mutex::default(),
            engine: Mutex::new(Some("fake-browser 1".into())),
        }
    }
}

impl FakeBrowserDriver {
    /// 這個網址顯示固定的畫面。
    pub fn script_page(&self, url: &str, page: FakePage) {
        self.script_page_with(url, move |_| page.clone());
    }

    /// 這個網址的畫面（含狀態碼）依這個 Session 輸入過的值決定。
    pub fn script_page_with(&self, url: &str, page: impl Fn(&[(String, String)]) -> FakePage + Send + Sync + 'static) {
        lock(&self.pages).insert(url.to_string(), Arc::new(page));
    }

    /// 點擊（或按鍵於）這個 selector 之後前往 `url`。
    pub fn script_link(&self, selector: &str, url: &str) {
        lock(&self.links).insert(selector.to_string(), url.to_string());
    }

    /// 之後開啟 Session 都失敗（例如沒有安裝瀏覽器）；`None` 恢復。
    pub fn fail_open(&self, reason: Option<&str>) {
        *lock(&self.open_failure) = reason.map(str::to_string);
    }

    /// 至今的全部操作，依順序（例如 `open b1`、`goto b1 http://…`、`fill b1 #password hunter2`、`close b1`）。
    pub fn commands(&self) -> Vec<String> {
        lock(&self.commands).clone()
    }

    /// 之後的 Session 回報這個引擎；`None` 表示取不到。
    pub fn set_engine(&self, engine: Option<&str>) {
        *lock(&self.engine) = engine.map(str::to_string);
    }

    /// 至今開啟 Session 的要求。
    pub fn session_requests(&self) -> Vec<BrowserSessionRequest> {
        lock(&self.requests).clone()
    }

    /// 每次截圖時要求遮住的 selector，依順序。
    pub fn screenshot_masks(&self) -> Vec<Vec<String>> {
        lock(&self.masks).clone()
    }

    /// 還沒關閉的 Session。
    pub fn open_sessions(&self) -> usize {
        lock(&self.sessions).len()
    }

    fn render(&self, state: &FakeBrowserSessionState) -> FakePage {
        let page = state.url.as_ref().and_then(|url| lock(&self.pages).get(url).cloned());
        page.map(|f| f(&state.fills)).unwrap_or_else(|| FakePage::new("", "", &[]))
    }

    /// 前往網址；沒有腳本的網址連不上。
    fn navigate(&self, state: &mut FakeBrowserSessionState, url: &str) -> Option<BrowserOutcome> {
        if !lock(&self.pages).contains_key(url) {
            return Some(BrowserOutcome::Unreachable { reason: format!("net::ERR_CONNECTION_REFUSED at {url}") });
        }
        state.url = Some(url.to_string());
        state.status = Some(self.render(state).status);
        None
    }
}

/// 不透明的檔案內容：固定檔頭加上內容的雜湊，不含任何明文。
fn opaque(header: &[u8], content: &str) -> Vec<u8> {
    let mut bytes = header.to_vec();
    bytes.extend_from_slice(baton_domain::artifact::sha256_hex(content.as_bytes()).as_bytes());
    bytes
}

#[async_trait]
impl BrowserDriver for FakeBrowserDriver {
    async fn open(&self, request: BrowserSessionRequest) -> PortResult<BrowserSession> {
        lock(&self.requests).push(request.clone());
        if let Some(reason) = lock(&self.open_failure).clone() {
            lock(&self.commands).push("open failed".into());
            return Err(PortError::Failed(reason));
        }
        let id = format!("b{}", self.next_session.fetch_add(1, Ordering::SeqCst));
        lock(&self.commands).push(format!("open {id}"));
        let state = FakeBrowserSessionState { request, url: None, status: None, fills: Vec::new(), log: Vec::new() };
        lock(&self.sessions).insert(id.clone(), state);
        Ok(BrowserSession(id))
    }

    async fn step(&self, session: &BrowserSession, step: BrowserStep) -> PortResult<BrowserOutcome> {
        let line = match &step.command {
            BrowserCommand::Goto { url } => format!("goto {} {url}", session.0),
            BrowserCommand::Click { selector } => format!("click {} {selector}", session.0),
            BrowserCommand::Fill { selector, text } => format!("fill {} {selector} {text}", session.0),
            BrowserCommand::Press { selector, key } => format!("press {} {selector} {key}", session.0),
            BrowserCommand::WaitFor { selector } => format!("wait {} {selector}", session.0),
        };
        lock(&self.commands).push(line.clone());
        let mut sessions = lock(&self.sessions);
        let state = sessions.get_mut(&session.0).ok_or_else(|| PortError::Failed(format!("Session {} 不存在", session.0)))?;
        state.log.push(line);
        let needs = |selector: &str, state: &FakeBrowserSessionState| -> Option<BrowserOutcome> {
            let page = self.render(state);
            (!page.visible.iter().any(|v| v == selector)).then(|| BrowserOutcome::ActionFailed {
                error: "TimeoutError".into(),
                message: format!("等不到「{selector}」"),
                page: None,
            })
        };
        let failure = match &step.command {
            BrowserCommand::Goto { url } => self.navigate(state, url),
            BrowserCommand::Click { selector } | BrowserCommand::Press { selector, .. } => match needs(selector, state) {
                Some(failure) => Some(failure),
                None => match lock(&self.links).get(selector).cloned() {
                    Some(url) => self.navigate(state, &url),
                    None => None,
                },
            },
            BrowserCommand::Fill { selector, text } => match needs(selector, state) {
                Some(failure) => Some(failure),
                None => {
                    state.fills.push((selector.clone(), text.clone()));
                    None
                }
            },
            BrowserCommand::WaitFor { selector } => needs(selector, state),
        };
        if let Some(BrowserOutcome::Unreachable { reason }) = failure {
            return Ok(BrowserOutcome::Unreachable { reason });
        }
        let page = self.render(state);
        let url = state.url.clone().unwrap_or_else(|| "about:blank".into());
        let screenshot = step.screenshot_mask.map(|mask| {
            lock(&self.masks).push(mask);
            // 如同真實引擎的 PNG：畫面上的文字只是像素，位元組中找不到
            opaque(b"\x89PNG\r\n\x1a\n", &format!("{url}\n{}", page.text))
        });
        let snapshot = PageSnapshot {
            url,
            title: page.title.clone(),
            status: state.status,
            text: page.text.clone(),
            fields: Vec::new(),
            visible: step.visible.iter().map(|s| page.visible.contains(s)).collect(),
            screenshot,
        };
        // 操作失敗時如同真實引擎附上當前畫面
        match failure {
            Some(BrowserOutcome::ActionFailed { error, message, .. }) => {
                Ok(BrowserOutcome::ActionFailed { error, message, page: Some(snapshot) })
            }
            _ => Ok(BrowserOutcome::Done { page: snapshot }),
        }
    }

    async fn close(&self, session: &BrowserSession) -> PortResult<Vec<BrowserFile>> {
        lock(&self.commands).push(format!("close {}", session.0));
        let Some(state) = lock(&self.sessions).remove(&session.0) else {
            return Err(PortError::Failed(format!("Session {} 不存在", session.0)));
        };
        let mut files = Vec::new();
        if state.request.trace {
            // 如同真實引擎壓縮過的 zip：記錄的操作（含輸入的文字）在位元組中找不到
            let bytes = opaque(b"PK\x03\x04", &state.log.join("\n"));
            files.push(BrowserFile { kind: "trace".into(), name: "trace.zip".into(), bytes });
        }
        if state.request.video {
            files.push(BrowserFile { kind: "video".into(), name: "video.webm".into(), bytes: format!("WEBM {}", session.0).into_bytes() });
        }
        Ok(files)
    }

    fn engine(&self, _session: &BrowserSession) -> Option<String> {
        lock(&self.engine).clone()
    }
}

/// 腳本化的模型（票 46、48）：語意判斷依預期文字、動作規劃依目標依序回應；沒有腳本時回報無法使用。
/// 記錄每一次的輸入，`calls` 是至今實際收到的模型呼叫總數。
#[derive(Default)]
pub struct FakeSemanticJudge {
    outcomes: Mutex<HashMap<String, VecDeque<JudgeOutcome>>>,
    requests: Mutex<Vec<JudgeRequest>>,
    plans: Mutex<HashMap<String, VecDeque<ActionPlan>>>,
    plan_requests: Mutex<Vec<ActionRequest>>,
}

/// 依序取出腳本的下一個結果；只剩最後一個時之後每次都相同。
fn next_scripted<T: Clone>(scripts: &Mutex<HashMap<String, VecDeque<T>>>, key: &str) -> Option<T> {
    let mut scripts = lock(scripts);
    scripts.get_mut(key).and_then(|queue| if queue.len() > 1 { queue.pop_front() } else { queue.front().cloned() })
}

impl FakeSemanticJudge {
    /// 對這個預期依序回應這些結果；只剩最後一個時之後每次都相同。
    pub fn script(&self, expectation: &str, outcomes: Vec<JudgeOutcome>) {
        lock(&self.outcomes).insert(expectation.to_string(), outcomes.into());
    }

    /// 對這個目標依序回應這些規劃；只剩最後一個時之後每次都相同。
    pub fn script_plan(&self, goal: &str, plans: Vec<ActionPlan>) {
        lock(&self.plans).insert(goal.to_string(), plans.into());
    }

    /// 至今收到的全部判斷要求（也就是模型 Prompt 的全部輸入）。
    pub fn requests(&self) -> Vec<JudgeRequest> {
        lock(&self.requests).clone()
    }

    /// 至今收到的全部動作規劃要求。
    pub fn plan_requests(&self) -> Vec<ActionRequest> {
        lock(&self.plan_requests).clone()
    }

    /// 至今實際收到的模型呼叫（語意判斷與動作規劃）總數。
    pub fn calls(&self) -> usize {
        lock(&self.requests).len() + lock(&self.plan_requests).len()
    }
}

#[async_trait]
impl SemanticJudge for FakeSemanticJudge {
    async fn judge(&self, request: JudgeRequest) -> PortResult<JudgeOutcome> {
        let expectation = request.expectation.clone();
        lock(&self.requests).push(request);
        let outcome = next_scripted(&self.outcomes, &expectation);
        Ok(outcome.unwrap_or(JudgeOutcome::Unavailable { reason: "測試沒有設定這個預期的判斷".into() }))
    }

    async fn plan(&self, request: ActionRequest) -> PortResult<ActionPlan> {
        let goal = request.goal.clone();
        lock(&self.plan_requests).push(request);
        let plan = next_scripted(&self.plans, &goal);
        Ok(plan.unwrap_or(ActionPlan::Unavailable { reason: "測試沒有設定這個目標的規劃".into() }))
    }
}

/// 秘密參照的 fake（票 46）：參照 → 值；沒有設定的參照解析失敗。
#[derive(Default)]
pub struct FakeSecretResolver {
    values: Mutex<HashMap<String, String>>,
}

impl FakeSecretResolver {
    pub fn set(&self, reference: &str, value: &str) {
        lock(&self.values).insert(reference.to_string(), value.to_string());
    }
}

#[async_trait]
impl SecretResolver for FakeSecretResolver {
    async fn resolve(&self, reference: &str) -> PortResult<String> {
        lock(&self.values).get(reference).cloned().ok_or_else(|| PortError::Failed(format!("沒有設定秘密參照 {reference}")))
    }
}
