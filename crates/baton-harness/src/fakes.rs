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

/// 某個程序被要求停止時的腳本化行為。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FakeStop {
    /// 回傳這個結果（`StillAlive` 時程序仍存活）。
    Result(StopResult),
    /// 停止的呼叫永遠不回來（模擬卡住的停止）。
    Hang,
    /// 停止的呼叫失敗。
    Fail(String),
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
    stop_scripts: HashMap<ProcessIdentity, FakeStop>,
    stop_calls: Vec<ProcessIdentity>,
    stop_graces: Vec<Duration>,
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
                stop_scripts: HashMap::new(),
                stop_calls: Vec::new(),
                stop_graces: Vec::new(),
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

    async fn stop(&self, identity: &ProcessIdentity, grace: Duration) -> PortResult<StopResult> {
        let behavior = {
            let mut state = lock(&self.state);
            state.stop_calls.push(identity.clone());
            state.stop_graces.push(grace);
            state.stop_scripts.get(identity).cloned().unwrap_or(FakeStop::Result(state.stop_result))
        };
        match behavior {
            FakeStop::Hang => std::future::pending().await,
            FakeStop::Fail(reason) => Err(PortError::Failed(reason)),
            FakeStop::Result(result) => {
                if result != StopResult::StillAlive {
                    lock(&self.state).liveness.insert(identity.clone(), Liveness::Dead);
                }
                Ok(result)
            }
        }
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
#[derive(Default)]
pub struct FakeWorkspaceFs {
    worktrees: Mutex<HashSet<PathBuf>>,
    dirty: Mutex<HashSet<PathBuf>>,
    artifact_hang: Mutex<Option<ArtifactWritePoint>>,
    git: Mutex<FakeGit>,
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

    /// 至今的全部整合合併，依順序。
    pub fn merges(&self) -> Vec<FakeMerge> {
        lock(&self.git).merges.clone()
    }

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

    async fn read_workspace_file(&self, path: &Path) -> PortResult<Option<Vec<u8>>> {
        baton_ports::read_file_if_exists(path)
    }

    async fn list_artifact_files(&self, dir: &Path) -> PortResult<Vec<FileEntry>> {
        baton_ports::list_files_recursive(dir)
    }

    async fn write_readonly_files(&self, dir: &Path, files: &[(String, Vec<u8>)]) -> PortResult<()> {
        baton_ports::write_readonly_tree(dir, files)
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
    pub command_runner: Arc<FakeCommandRunner>,
    pub user_settings: Arc<FakeUserSettings>,
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
            command_runner: Arc::default(),
            user_settings: Arc::default(),
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
        }
    }
}
