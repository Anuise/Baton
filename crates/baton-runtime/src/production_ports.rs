//! 正式執行環境使用的 Port。
//!
//! 目前只有系統時鐘、平台辨識、DPAPI 憑證保護與目錄檢查是真實實作；其餘 Port 的正式版由後續的票提供，
//! 在那之前呼叫它們會得到 `NotImplemented`，不會悄悄做出任何副作用。

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use baton_domain::Timestamp;
use baton_ports::*;
use futures::stream::BoxStream;

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
    async fn create_worktree(&self, _repo: &Path, _branch: &str, _target: &Path) -> PortResult<()> {
        Err(PortError::NotImplemented("WorkspaceFs::create_worktree"))
    }
    async fn remove_worktree(&self, _target: &Path) -> PortResult<()> {
        Err(PortError::NotImplemented("WorkspaceFs::remove_worktree"))
    }
    async fn inspect_dirty(&self, _target: &Path) -> PortResult<DirtyState> {
        Err(PortError::NotImplemented("WorkspaceFs::inspect_dirty"))
    }
    async fn write_artifact_atomic(&self, _path: &Path, _bytes: &[u8]) -> PortResult<()> {
        Err(PortError::NotImplemented("WorkspaceFs::write_artifact_atomic"))
    }
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
    }
}
