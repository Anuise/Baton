//! 端到端（Windows）：真實 Runtime + 真實 WindowsProcessControl + 真實 HTTP，取消 Run 會終止整個程序樹。
//!
//! 只有 AgentAdapter（它把「Worker」換成一棵長時間存活的 cmd → cmd → ping 程序樹）與 ClaudeEnvironment
//! （不呼叫真的 `claude`，讓帳號 Profile 能被核對為可派工）是測試用的；
//! 派工、Supervisor 啟動、程序身分回執與取消都走正式路徑。
#![cfg(windows)]

mod support;

use std::ffi::c_void;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use baton_api::{CreateProfileRequest, CreateTaskRequest, CreateWorkspaceRequest};
use baton_client::{DispatchOutcome, RuntimeClient};
use baton_domain::run::RunStatus;
use baton_domain::task::{ContractContent, ContractField, TaskStatus};
use baton_ports::*;

/// 這個測試不涉及監護（票 32）。
struct NoGuardian;

impl GuardianProbe for NoGuardian {
    fn guardian_running(&self) -> bool {
        false
    }
}

/// 一直是同一次登入（票 33）。
struct SameLogin;

impl baton_ports::LoginSession for SameLogin {
    fn current_login(&self) -> Option<String> {
        Some("login".into())
    }
}

/// 不登記登入自動啟動（票 33）。
struct NoStartup;

impl baton_ports::StartupRegistration for NoStartup {
    fn is_registered(&self) -> baton_ports::PortResult<bool> {
        Ok(false)
    }
    fn set_registered(&self, _enabled: bool) -> baton_ports::PortResult<()> {
        Ok(())
    }
}
struct NoUpdates;

#[async_trait::async_trait]
impl baton_ports::UpdateSource for NoUpdates {
    async fn latest(&self) -> baton_ports::PortResult<baton_domain::maintenance::ReleaseInfo> {
        Err(baton_ports::PortError::NotImplemented("UpdateSource"))
    }
    async fn download(
        &self,
        _release: &baton_domain::maintenance::ReleaseInfo,
        _dest_dir: &std::path::Path,
    ) -> baton_ports::PortResult<std::path::PathBuf> {
        Err(baton_ports::PortError::NotImplemented("UpdateSource"))
    }
}
use baton_runtime::production_ports::{LocalCommandRunner, LocalWorkspaceFs, SystemClock};
use baton_runtime::windows_platform::WindowsPlatformInfo;
use baton_runtime::windows_process::WindowsProcessControl;
use baton_runtime::{Runtime, RuntimeConfig, RuntimeOptions};
use futures::stream::BoxStream;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::JobObjects::{
    JobObjectBasicProcessIdList, OpenJobObjectW, QueryInformationJobObject, TerminateJobObject,
    JOBOBJECT_BASIC_PROCESS_ID_LIST,
};

const JOB_OBJECT_TERMINATE: u32 = 0x0008;
const JOB_OBJECT_QUERY: u32 = 0x0004;

/// 把 Worker 換成一棵三層程序樹：cmd → cmd → ping（ping 約 60 秒才會自己結束）。
struct TreeAdapter;

#[async_trait]
impl AgentAdapter for TreeAdapter {
    fn spawn_spec(&self, spec: &RunSpec) -> SpawnSpec {
        SpawnSpec {
            program: "cmd.exe".into(),
            args: ["/c", "cmd.exe", "/c", "ping", "-n", "60", "127.0.0.1"].map(String::from).to_vec(),
            working_dir: spec.working_dir.clone(),
            command_id: spec.command_id.clone(),
            env: Vec::new(),
            ..Default::default()
        }
    }

    fn controller_spawn_spec(&self, spec: &RunSpec) -> SpawnSpec {
        self.spawn_spec(spec)
    }

    async fn launch(&self, spec: RunSpec) -> PortResult<AgentHandle> {
        Ok(AgentHandle(spec.run_id))
    }

    fn events(&self, _handle: &AgentHandle) -> BoxStream<'static, AdapterEvent> {
        Box::pin(futures::stream::pending())
    }

    async fn resume_capability(&self, _session: &SessionRef) -> PortResult<Capability> {
        Ok(Capability::Unknown)
    }
}

/// Worker 啟動背景的 ping 後立刻結束，約一秒後回報成功結束；ping 留在 Run 的 Job 內。
struct OrphanAdapter;

#[async_trait]
impl AgentAdapter for OrphanAdapter {
    fn spawn_spec(&self, spec: &RunSpec) -> SpawnSpec {
        SpawnSpec {
            program: "cmd.exe".into(),
            args: ["/c", "start", "/b", "ping", "-n", "60", "127.0.0.1"].map(String::from).to_vec(),
            working_dir: spec.working_dir.clone(),
            command_id: spec.command_id.clone(),
            env: Vec::new(),
            ..Default::default()
        }
    }

    fn controller_spawn_spec(&self, spec: &RunSpec) -> SpawnSpec {
        self.spawn_spec(spec)
    }

    async fn launch(&self, spec: RunSpec) -> PortResult<AgentHandle> {
        Ok(AgentHandle(spec.run_id))
    }

    fn events(&self, _handle: &AgentHandle) -> BoxStream<'static, AdapterEvent> {
        Box::pin(futures::stream::once(async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            AdapterEvent::Exited { success: true }
        }))
    }

    async fn resume_capability(&self, _session: &SessionRef) -> PortResult<Capability> {
        Ok(Capability::Unknown)
    }
}

/// 測試用的 Claude 環境：任何設定目錄都回報原生登入，帳號以目錄名稱區分（不會呼叫真的 `claude`）。
struct NativeLoginEnv;

#[async_trait]
impl ClaudeEnvironment for NativeLoginEnv {
    async fn probe_claude(&self) -> PortResult<ToolProbe> {
        Ok(ToolProbe::Found { version_output: Some("2.1.307 (Claude Code)".into()) })
    }

    async fn probe_git(&self) -> PortResult<ToolProbe> {
        Ok(ToolProbe::Found { version_output: Some("git version 2.54.0.windows.1".into()) })
    }

    async fn auth_status(&self, config_dir: &Path) -> PortResult<AuthReport> {
        let name = config_dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        Ok(AuthReport {
            logged_in: true,
            source: AuthSource::NativeLogin,
            source_detail: None,
            account: Some(format!("{name}@example.com")),
            config_dir: Some(config_dir.to_string_lossy().into_owned()),
        })
    }

    async fn open_login_terminal(&self, _config_dir: &Path) -> PortResult<()> {
        Ok(())
    }
}

struct NoQuota;

#[async_trait]
impl QuotaObserver for NoQuota {
    async fn observe(&self, _profile: &ProfileRef) -> PortResult<QuotaSample> {
        Err(PortError::NotImplemented("QuotaObserver"))
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn open_job(command_id: &str) -> Option<HANDLE> {
    let name = wide(&format!("Local\\Baton-Run-{command_id}"));
    // SAFETY：name 以 NUL 結尾。
    let handle = unsafe { OpenJobObjectW(JOB_OBJECT_QUERY | JOB_OBJECT_TERMINATE, 0, name.as_ptr()) };
    (!handle.is_null()).then_some(handle)
}

/// 該 Run 的 Job 內存活的程序數；Job 不存在回傳 0。
fn job_process_count(command_id: &str) -> usize {
    let Some(job) = open_job(command_id) else { return 0 };
    let mut buffer = vec![0u64; 1 + 256];
    // SAFETY：job 有效，buffer 對齊且大小與傳入的位元組數一致。
    let ok = unsafe {
        QueryInformationJobObject(
            job,
            JobObjectBasicProcessIdList,
            buffer.as_mut_ptr() as *mut c_void,
            (buffer.len() * 8) as u32,
            std::ptr::null_mut(),
        )
    };
    let count = if ok != 0 {
        let list = buffer.as_ptr() as *const JOBOBJECT_BASIC_PROCESS_ID_LIST;
        // SAFETY：查詢成功，結構的第一個欄位就是清單內的程序數。
        unsafe { (*list).NumberOfProcessIdsInList as usize }
    } else {
        0
    };
    // SAFETY：job 是這裡開啟、尚未關閉的 handle。
    unsafe {
        CloseHandle(job);
    }
    count
}

/// 測試結束（含失敗）時終止 Job，確保不留下孤兒程序。
struct JobGuard(String);

impl Drop for JobGuard {
    fn drop(&mut self) {
        if let Some(job) = open_job(&self.0) {
            // SAFETY：job 是這裡開啟、尚未關閉的 handle。
            unsafe {
                TerminateJobObject(job, 1);
                CloseHandle(job);
            }
        }
    }
}

fn filled(text: &str) -> Option<ContractField> {
    Some(ContractField::Filled { text: text.into(), references: vec![] })
}

fn complete() -> ContractContent {
    ContractContent {
        objective: filled("整理資料"),
        inputs: Some(ContractField::NotApplicable { reason: "沒有輸入".into() }),
        scope: filled("公開資料"),
        deliverables: filled("比較表"),
        done_criteria: filled("五個產品"),
        blocker_handling: filled("回報"),
        required_capabilities: vec![],
        verification_checks: vec![],
    }
}

fn ports(agent_adapter: Arc<dyn AgentAdapter>, protector: Arc<dyn SecretProtector>) -> Ports {
    Ports {
        clock: Arc::new(SystemClock),
        process_control: Arc::new(WindowsProcessControl::new()),
        agent_adapter,
        quota_observer: Arc::new(NoQuota),
        workspace_fs: Arc::new(LocalWorkspaceFs),
        platform_info: Arc::new(WindowsPlatformInfo),
        secret_protector: protector.clone(),
        claude_env: Arc::new(NativeLoginEnv),
        command_runner: Arc::new(LocalCommandRunner),
        user_settings: Arc::new(baton_ports::DirUserSettingsSource::new("unused", "unused")),
        guardian_probe: Arc::new(NoGuardian),
        login_session: Arc::new(SameLogin),
        startup_registration: Arc::new(NoStartup),
        update_source: Arc::new(NoUpdates),
        verification_driver: Arc::new(baton_runtime::verification_driver::LocalVerificationDriver::default()),
        browser_driver: Arc::new(baton_runtime::browser_driver::LocalBrowserDriver::default()),
        semantic_judge: Arc::new(baton_runtime::semantic_judge::ClaudeSemanticJudge::default()),
        secret_resolver: Arc::new(baton_runtime::secret_resolver::EnvSecretResolver),
        model_directory: Arc::new(baton_runtime::model_directory::AnthropicModelDirectory::default()),
    }
}

/// 以真實 Runtime 派出一個 Run 並讓 Supervisor 啟動它；回傳 Runtime、用戶端、Workspace 與 Run 的識別。
async fn running_run(
    data_dir: &Path,
    agent_adapter: Arc<dyn AgentAdapter>,
) -> (baton_runtime::RunningRuntime, RuntimeClient, String, baton_domain::run::RunView) {
    let workspace_dir = data_dir.join("work");
    std::fs::create_dir_all(&workspace_dir).unwrap();
    let protector: Arc<dyn SecretProtector> = Arc::new(baton_credential::DpapiProtector);
    let ports = ports(agent_adapter, protector.clone());
    let config = RuntimeConfig { data_dir: data_dir.to_path_buf(), bind: "127.0.0.1:0".parse().unwrap() };
    let runtime = Runtime::start_with(
        config,
        ports,
        RuntimeOptions { background_supervisor: false, background_backup: false, stop_grace: Duration::from_millis(200), ..RuntimeOptions::default() },
    )
    .await
    .unwrap();
    let client = RuntimeClient::new(runtime.local_addr(), data_dir.to_path_buf(), protector);

    // 派工需要可派工的帳號：經由 HTTP 建立 Profile 並核對（測試用的 Claude 環境回報原生登入）
    let profile = client.create_profile(&CreateProfileRequest { name: "測試帳號".into(), expected_account: None }).await.unwrap();
    let verified = client.verify_profile(&profile.profile.id).await.unwrap();
    assert!(verified.dispatchable, "{verified:?}");

    let workspace = client
        .create_workspace(&CreateWorkspaceRequest { path: workspace_dir.to_string_lossy().into_owned(), name: None })
        .await
        .unwrap();
    let goal = client.create_goal(&workspace.id, "目標").await.unwrap();
    let task = client
        .create_task(
            &workspace.id,
            &CreateTaskRequest { goal_id: goal.id, title: "跑一棵程序樹".into(), kind: Default::default(), contract: complete() },
        )
        .await
        .unwrap();
    client.mark_task_ready(&workspace.id, &task.task.id).await.unwrap();

    let DispatchOutcome::Started(queued) = client.dispatch_task(&workspace.id, &task.task.id).await.unwrap() else {
        panic!("有可派工的帳號與名額，應該直接建立 Run");
    };
    runtime.supervisor().tick().await;
    let running = client.task_board(&workspace.id).await.unwrap().runs.remove(0);
    assert_eq!((queued.run.status, running.run.status), (RunStatus::Queued, RunStatus::Running), "{running:?}");
    (runtime, client, workspace.id, running)
}

#[tokio::test(flavor = "multi_thread")]
async fn cancelling_a_run_through_the_runtime_terminates_the_whole_process_tree() {
    let data_dir = tempfile::tempdir().unwrap();
    let (runtime, client, workspace_id, running) = running_run(data_dir.path(), Arc::new(TreeAdapter)).await;
    let command_id = running.run.command_id.clone().expect("launch intent 已持久化");
    let _cleanup = JobGuard(command_id.clone());

    let process = running.run.process.as_ref().expect("回執已寫回");
    assert!(process.pid > 0 && !process.command_hash.is_empty());
    let deadline = Instant::now() + Duration::from_secs(10);
    while job_process_count(&command_id) < 2 {
        assert!(Instant::now() < deadline, "程序樹沒有在 10 秒內長出子孫程序");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    client.cancel_run(&workspace_id, &running.run.id).await.unwrap();
    runtime.supervisor().tick().await;

    let board = client.task_board(&workspace_id).await.unwrap();
    assert_eq!(board.runs[0].run.status, RunStatus::Cancelled, "{:?}", board.runs[0]);
    assert_eq!(board.tasks[0].task.status, TaskStatus::Ready);
    assert_eq!(job_process_count(&command_id), 0, "整個程序樹（含孫程序）都已被終止");
    runtime.shutdown().await;
    // 被終止的程序樹與 Runtime 的背景工作都放掉檔案之後，資料目錄（含 Workspace 的 `.baton`）才刪得掉
    support::remove_temp_dir(data_dir).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_finished_worker_leaves_nothing_running_in_its_job() {
    let data_dir = tempfile::tempdir().unwrap();
    let (runtime, client, workspace_id, running) = running_run(data_dir.path(), Arc::new(OrphanAdapter)).await;
    let command_id = running.run.command_id.clone().expect("launch intent 已持久化");
    let _cleanup = JobGuard(command_id.clone());

    // Worker 回報結束：Supervisor 結束 Run 之前終止它留在背景的 ping
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        runtime.supervisor().tick().await;
        let board = client.task_board(&workspace_id).await.unwrap();
        if board.runs[0].run.status != RunStatus::Running {
            assert_eq!(board.runs[0].run.status, RunStatus::Succeeded, "{:?}", board.runs[0]);
            break;
        }
        assert!(Instant::now() < deadline, "Run 沒有在 20 秒內結束");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(job_process_count(&command_id), 0, "Run 結束時 Job 內不能留下任何程序");
    runtime.shutdown().await;
}
