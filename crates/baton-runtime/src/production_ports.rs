//! 正式執行環境使用的 Port。
//!
//! 目前只有系統時鐘、平台辨識、DPAPI 憑證保護、目錄檢查與建立、成果與工作目錄檔案讀寫、Claude Code／Git 環境探測、驗證命令執行是真實實作；其餘 Port 的正式版由後續的票提供，
//! 在那之前呼叫它們會得到 `NotImplemented`，不會悄悄做出任何副作用。

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use baton_domain::Timestamp;
use baton_ports::*;
use futures::stream::BoxStream;

use crate::claude_cli::ClaudeCli;
use crate::windows_platform::WindowsPlatformInfo;

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        let millis = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        Timestamp::from_millis(millis as i64)
    }
}

struct Unavailable;

#[async_trait]
impl ProcessControl for Unavailable {
    async fn spawn(&self, _spec: SpawnSpec) -> PortResult<Receipt> {
        Err(PortError::NotImplemented("ProcessControl"))
    }
    async fn find(&self, _spec: &SpawnSpec) -> PortResult<Option<Receipt>> {
        Err(PortError::NotImplemented("ProcessControl"))
    }
    async fn inspect(&self, _identity: &ProcessIdentity) -> PortResult<Liveness> {
        Err(PortError::NotImplemented("ProcessControl"))
    }
    async fn stop(&self, _identity: &ProcessIdentity, _grace: Duration) -> PortResult<StopResult> {
        Err(PortError::NotImplemented("ProcessControl"))
    }
}

#[async_trait]
impl AgentAdapter for Unavailable {
    fn spawn_spec(&self, spec: &RunSpec) -> SpawnSpec {
        SpawnSpec {
            program: String::new(),
            args: Vec::new(),
            working_dir: spec.working_dir.clone(),
            command_id: spec.command_id.clone(),
            env: Vec::new(),
        }
    }
    async fn launch(&self, _spec: RunSpec) -> PortResult<AgentHandle> {
        Err(PortError::NotImplemented("AgentAdapter"))
    }
    fn events(&self, _handle: &AgentHandle) -> BoxStream<'static, AdapterEvent> {
        Box::pin(futures::stream::empty())
    }
    async fn resume_capability(&self, _session: &SessionRef) -> PortResult<Capability> {
        Err(PortError::NotImplemented("AgentAdapter"))
    }
}

#[async_trait]
impl QuotaObserver for Unavailable {
    async fn observe(&self, _profile: &ProfileRef) -> PortResult<QuotaSample> {
        Err(PortError::NotImplemented("QuotaObserver"))
    }
}

/// 本機檔案系統的 WorkspaceFs：目前只有目錄檢查是真實實作，其餘由後續的票補上。
pub struct LocalWorkspaceFs;

#[async_trait]
impl WorkspaceFs for LocalWorkspaceFs {
    async fn inspect_directory(&self, path: &Path) -> PortResult<DirectoryState> {
        baton_ports::directory_state(path)
    }
    async fn ensure_directory(&self, path: &Path) -> PortResult<()> {
        tokio::fs::create_dir_all(path).await.map_err(|e| PortError::Failed(format!("無法建立目錄 {}：{e}", path.display())))
    }
    async fn create_worktree(&self, _repo: &Path, _branch: &str, _target: &Path) -> PortResult<()> {
        Err(PortError::NotImplemented("WorkspaceFs::create_worktree"))
    }
    async fn remove_worktree(&self, _target: &Path) -> PortResult<()> {
        Err(PortError::NotImplemented("WorkspaceFs::remove_worktree"))
    }
    async fn inspect_dirty(&self, _target: &Path) -> PortResult<DirtyState> {
        Err(PortError::NotImplemented("WorkspaceFs::inspect_dirty"))
    }
    async fn write_artifact_atomic(&self, path: &Path, bytes: &[u8]) -> PortResult<()> {
        let (path, bytes) = (path.to_path_buf(), bytes.to_vec());
        blocking(move || write_file_atomic(&path, &bytes)).await
    }
    async fn read_artifact(&self, path: &Path) -> PortResult<Option<Vec<u8>>> {
        let path = path.to_path_buf();
        blocking(move || read_file_if_exists(&path)).await
    }
    async fn read_workspace_file(&self, path: &Path) -> PortResult<Option<Vec<u8>>> {
        let path = path.to_path_buf();
        blocking(move || read_file_if_exists(&path)).await
    }
    async fn list_artifact_files(&self, dir: &Path) -> PortResult<Vec<FileEntry>> {
        let dir = dir.to_path_buf();
        blocking(move || list_files_recursive(&dir)).await
    }
}

/// 以子程序執行驗證命令（不經過 shell）；逾時時丟棄等待，子程序隨之被終止（`kill_on_drop`）。
/// 只終止直接啟動的程序，命令自己再開的子孫程序不受控（驗證命令不放進 Job Object，AI 決策記錄於票 13）。
pub struct LocalCommandRunner;

/// 保留的輸出結尾長度（位元組）；證據只需要結尾的失敗原因。
const OUTPUT_TAIL_BYTES: usize = 16 * 1024;

#[async_trait]
impl CommandRunner for LocalCommandRunner {
    async fn run(&self, request: CommandRequest) -> PortResult<CommandOutput> {
        let mut command = tokio::process::Command::new(&request.program);
        command
            .args(&request.args)
            .current_dir(&request.working_dir)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        // 背景的 Runtime 執行主控台程式時不要彈出視窗
        #[cfg(windows)]
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        let child = command.spawn().map_err(|e| PortError::Failed(format!("無法啟動「{}」：{e}", request.program)))?;
        match tokio::time::timeout(request.timeout, child.wait_with_output()).await {
            Err(_) => Ok(CommandOutput::TimedOut { output: String::new() }),
            Ok(Err(e)) => Err(PortError::Failed(format!("等待「{}」結束時失敗：{e}", request.program))),
            Ok(Ok(output)) => {
                let mut bytes = output.stdout;
                bytes.extend_from_slice(&output.stderr);
                let tail = &bytes[bytes.len().saturating_sub(OUTPUT_TAIL_BYTES)..];
                let text = String::from_utf8_lossy(tail).into_owned();
                Ok(match output.status.code() {
                    Some(code) => CommandOutput::Exited { code, output: text },
                    None => CommandOutput::Terminated { output: text },
                })
            }
        }
    }
}

/// 檔案系統操作是阻塞的（含 fsync），放到阻塞執行緒池，不卡住 Runtime 的非同步工作。
async fn blocking<T: Send + 'static>(work: impl FnOnce() -> PortResult<T> + Send + 'static) -> PortResult<T> {
    tokio::task::spawn_blocking(work).await.map_err(|e| PortError::Failed(format!("檔案工作被中斷：{e}")))?
}

/// 組出正式執行用的 Port 組合。
pub fn production_ports() -> Ports {
    let unavailable = Arc::new(Unavailable);
    // Windows 上使用真實的 Job Object 實作；其他平台維持尚未實作
    #[cfg(windows)]
    let process_control: Arc<dyn ProcessControl> = Arc::new(crate::windows_process::WindowsProcessControl::new());
    #[cfg(not(windows))]
    let process_control: Arc<dyn ProcessControl> = unavailable.clone();
    Ports {
        clock: Arc::new(SystemClock),
        process_control,
        agent_adapter: unavailable.clone(),
        quota_observer: unavailable.clone(),
        workspace_fs: Arc::new(LocalWorkspaceFs),
        platform_info: Arc::new(WindowsPlatformInfo),
        // DPAPI 是真實實作：桌面憑證檔一定要受保護
        secret_protector: Arc::new(baton_credential::DpapiProtector),
        claude_env: Arc::new(ClaudeCli),
        command_runner: Arc::new(LocalCommandRunner),
    }
}
