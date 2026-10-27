//! 正式執行環境使用的 Port。
//!
//! 目前只有系統時鐘、平台辨識、DPAPI 憑證保護、目錄檢查與建立、成果與工作目錄檔案讀寫、Claude Code／Git 環境探測、驗證命令執行、
//! Claude Code Adapter、worktree 建立與整合合併（git CLI，票 20）、Windows 登入識別與登入啟動登記（票 33）、
//! 簽章更新的來源（票 34；建置時設定才啟用）是真實實作；其餘 Port 的正式版由後續的票提供，
//! 在那之前呼叫它們會得到 `NotImplemented`，不會悄悄做出任何副作用。

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use baton_domain::Timestamp;
use baton_ports::*;

use crate::claude_adapter::ClaudeCodeAdapter;
use crate::claude_cli::ClaudeCli;
use crate::windows_platform::WindowsPlatformInfo;

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        let millis = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        Timestamp::from_millis(millis as i64)
    }

    fn sleep_until(&self, deadline: Timestamp) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        let remaining = (deadline.as_millis() - self.now().as_millis()).max(0) as u64;
        Box::pin(tokio::time::sleep(Duration::from_millis(remaining)))
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
impl QuotaObserver for Unavailable {
    async fn observe(&self, _profile: &ProfileRef) -> PortResult<QuotaSample> {
        Err(PortError::NotImplemented("QuotaObserver"))
    }
}

impl LoginSession for Unavailable {
    fn current_login(&self) -> Option<String> {
        None
    }
}

impl StartupRegistration for Unavailable {
    fn is_registered(&self) -> PortResult<bool> {
        Err(PortError::NotImplemented("StartupRegistration"))
    }
    fn set_registered(&self, _enabled: bool) -> PortResult<()> {
        Err(PortError::NotImplemented("StartupRegistration"))
    }
}

#[async_trait]
impl UpdateSource for Unavailable {
    async fn latest(&self) -> PortResult<baton_domain::maintenance::ReleaseInfo> {
        Err(PortError::NotImplemented("UpdateSource"))
    }
    async fn download(
        &self,
        _release: &baton_domain::maintenance::ReleaseInfo,
        _dest_dir: &Path,
    ) -> PortResult<std::path::PathBuf> {
        Err(PortError::NotImplemented("UpdateSource"))
    }
}

/// 本機檔案系統的 WorkspaceFs：worktree 的移除與未保存變更檢查由清理的票（票 21）補上，其餘是真實實作。
pub struct LocalWorkspaceFs;

#[async_trait]
impl WorkspaceFs for LocalWorkspaceFs {
    async fn inspect_directory(&self, path: &Path) -> PortResult<DirectoryState> {
        baton_ports::directory_state(path)
    }
    async fn ensure_directory(&self, path: &Path) -> PortResult<()> {
        tokio::fs::create_dir_all(path).await.map_err(|e| PortError::Failed(format!("無法建立目錄 {}：{e}", path.display())))
    }
    async fn inspect_git(&self, path: &Path) -> PortResult<GitState> {
        let path = path.to_path_buf();
        blocking(move || git_inspect(&path)).await
    }
    async fn create_worktree(&self, repo: &Path, branch: &str, target: &Path) -> PortResult<()> {
        let (repo, branch, target) = (repo.to_path_buf(), branch.to_string(), target.to_path_buf());
        blocking(move || git_create_worktree(&repo, &branch, &target)).await
    }
    async fn merge_for_integration(
        &self,
        repo: &Path,
        target_branch: &str,
        branch: &str,
        integration_dir: &Path,
    ) -> PortResult<MergeOutcome> {
        let (repo, target_branch, branch, dir) =
            (repo.to_path_buf(), target_branch.to_string(), branch.to_string(), integration_dir.to_path_buf());
        blocking(move || git_merge_for_integration(&repo, &target_branch, &branch, &dir)).await
    }
    async fn advance_branch(
        &self,
        repo: &Path,
        target_branch: &str,
        expected_tip: &str,
        commit: &str,
    ) -> PortResult<AdvanceOutcome> {
        let (repo, target_branch, expected_tip, commit) =
            (repo.to_path_buf(), target_branch.to_string(), expected_tip.to_string(), commit.to_string());
        blocking(move || git_advance_branch(&repo, &target_branch, &expected_tip, &commit)).await
    }
    async fn inspect_worktree(&self, repo: &Path, target: &Path) -> PortResult<WorktreeState> {
        let (repo, target) = (repo.to_path_buf(), target.to_path_buf());
        blocking(move || git_inspect_worktree(&repo, &target)).await
    }
    async fn remove_worktree(&self, repo: &Path, target: &Path, trash: &Path) -> PortResult<()> {
        let (repo, target, trash) = (repo.to_path_buf(), target.to_path_buf(), trash.to_path_buf());
        blocking(move || git_remove_worktree(&repo, &target, &trash)).await
    }
    async fn changed_paths(&self, repo: &Path, from: &str, to: &str) -> PortResult<Vec<String>> {
        let (repo, from, to) = (repo.to_path_buf(), from.to_string(), to.to_string());
        blocking(move || git_changed_paths(&repo, &from, &to)).await
    }
    async fn write_artifact_atomic(&self, path: &Path, bytes: &[u8]) -> PortResult<()> {
        let (path, bytes) = (path.to_path_buf(), bytes.to_vec());
        blocking(move || write_file_atomic(&path, &bytes)).await
    }
    async fn write_readonly_files(&self, dir: &Path, files: &[(String, Vec<u8>)]) -> PortResult<()> {
        let (dir, files) = (dir.to_path_buf(), files.to_vec());
        blocking(move || baton_ports::write_readonly_tree(&dir, &files)).await
    }
    async fn create_file_if_absent(&self, path: &Path, bytes: &[u8]) -> PortResult<bool> {
        let (path, bytes) = (path.to_path_buf(), bytes.to_vec());
        blocking(move || baton_ports::create_file_if_absent(&path, &bytes)).await
    }
    async fn read_artifact(&self, path: &Path) -> PortResult<Option<Vec<u8>>> {
        let path = path.to_path_buf();
        blocking(move || read_file_if_exists(&path)).await
    }
    async fn read_workspace_file(&self, path: &Path) -> PortResult<Option<Vec<u8>>> {
        let path = path.to_path_buf();
        blocking(move || read_file_if_exists(&path)).await
    }
    async fn read_file_within(&self, root: &Path, relative: &Path) -> PortResult<ContainedFile> {
        let (root, relative) = (root.to_path_buf(), relative.to_path_buf());
        blocking(move || baton_ports::read_file_within(&root, &relative)).await
    }
    async fn list_artifact_files(&self, dir: &Path) -> PortResult<Vec<FileEntry>> {
        let dir = dir.to_path_buf();
        blocking(move || list_files_recursive(&dir)).await
    }
}

/// 以子程序執行驗證命令（不經過 shell）。
///
/// Windows 上命令以暫停狀態建立、放進專屬的 Job Object 後才開始執行；命令結束或逾時都終止整個 Job，
/// 命令自己再開的子孫程序不會殘留，Runtime 結束時 Job 也隨之關閉並終止整棵樹（審查 R152）。
/// 等的是命令本身結束（不是輸出管道關閉），輸出由背景執行緒分開讀取，逾時時保留已讀到的部分。
pub struct LocalCommandRunner;

/// 保留的輸出結尾長度（位元組）；證據只需要結尾的失敗原因。
const OUTPUT_TAIL_BYTES: usize = 16 * 1024;
/// 等待命令結束的輪詢間隔。
const EXIT_POLL: Duration = Duration::from_millis(25);
/// 命令結束（並終止 Job）之後，等讀取執行緒讀完剩餘輸出的上限。
const OUTPUT_DRAIN: Duration = Duration::from_secs(2);

#[async_trait]
impl CommandRunner for LocalCommandRunner {
    async fn run(&self, request: CommandRequest) -> PortResult<CommandOutput> {
        let mut command = std::process::Command::new(&request.program);
        command
            .args(&request.args)
            .current_dir(&request.working_dir)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        // 背景的 Runtime 執行主控台程式時不要彈出視窗；以暫停狀態建立，放進 Job 之後才執行
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000 | 0x0000_0004); // CREATE_NO_WINDOW | CREATE_SUSPENDED
        }
        let mut child = command.spawn().map_err(|e| PortError::Failed(format!("無法啟動「{}」：{e}", request.program)))?;
        #[cfg(windows)]
        let job = match crate::windows_process::KillOnCloseJob::new().and_then(|job| job.adopt_suspended(&child).map(|()| job)) {
            Ok(job) => job,
            Err(error) => {
                let _ = child.kill();
                return Err(PortError::Failed(format!("無法把「{}」放進 Job Object：{error}", request.program)));
            }
        };
        let stdout = TailReader::spawn(child.stdout.take());
        let stderr = TailReader::spawn(child.stderr.take());

        let deadline = tokio::time::Instant::now() + request.timeout;
        let waited = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Ok(Some(status)),
                Ok(None) if tokio::time::Instant::now() >= deadline => break Ok(None),
                Ok(None) => tokio::time::sleep(EXIT_POLL).await,
                Err(e) => break Err(PortError::Failed(format!("等待「{}」結束時失敗：{e}", request.program))),
            }
        };
        // 命令結束或逾時：終止整個 Job，仍在執行的子孫程序一併結束，它們持有的輸出管道隨之關閉
        #[cfg(windows)]
        job.terminate();
        if !matches!(waited, Ok(Some(_))) {
            let _ = child.kill();
        }
        let status = waited?;
        let output = TailReader::collect(stdout, stderr).await;
        Ok(match status {
            None => CommandOutput::TimedOut { output },
            Some(status) => match status.code() {
                Some(code) => CommandOutput::Exited { code, output },
                None => CommandOutput::Terminated { output },
            },
        })
    }
}

/// 在背景執行緒把一條輸出管道讀到關閉，只保留結尾；讀到的部分隨時可以取用。
struct TailReader {
    buffer: Arc<std::sync::Mutex<Vec<u8>>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl TailReader {
    fn spawn(pipe: Option<impl std::io::Read + Send + 'static>) -> Self {
        let buffer = Arc::new(std::sync::Mutex::new(Vec::new()));
        let thread = pipe.map(|mut pipe| {
            let buffer = buffer.clone();
            std::thread::spawn(move || {
                let mut chunk = [0u8; 8192];
                while let Ok(read @ 1..) = pipe.read(&mut chunk) {
                    let mut kept = buffer.lock().unwrap_or_else(|e| e.into_inner());
                    kept.extend_from_slice(&chunk[..read]);
                    if kept.len() > 2 * OUTPUT_TAIL_BYTES {
                        let excess = kept.len() - OUTPUT_TAIL_BYTES;
                        kept.drain(..excess);
                    }
                }
            })
        });
        Self { buffer, thread }
    }

    /// 等兩個讀取執行緒讀完（最多 `OUTPUT_DRAIN`），回傳標準輸出接著標準錯誤的結尾。
    async fn collect(stdout: Self, stderr: Self) -> String {
        let handles: Vec<_> = [stdout.thread, stderr.thread].into_iter().flatten().collect();
        let joined = tokio::task::spawn_blocking(move || handles.into_iter().for_each(|h| drop(h.join())));
        let _ = tokio::time::timeout(OUTPUT_DRAIN, joined).await;
        let mut bytes = stdout.buffer.lock().unwrap_or_else(|e| e.into_inner()).clone();
        bytes.extend_from_slice(&stderr.buffer.lock().unwrap_or_else(|e| e.into_inner()));
        String::from_utf8_lossy(&bytes[bytes.len().saturating_sub(OUTPUT_TAIL_BYTES)..]).into_owned()
    }
}

/// 檔案系統操作是阻塞的（含 fsync），放到阻塞執行緒池，不卡住 Runtime 的非同步工作。
async fn blocking<T: Send + 'static>(work: impl FnOnce() -> PortResult<T> + Send + 'static) -> PortResult<T> {
    tokio::task::spawn_blocking(work).await.map_err(|e| PortError::Failed(format!("檔案工作被中斷：{e}")))?
}

/// 登入啟動入口的執行檔（與 Runtime 放在同一個資料夾，票 33）。
#[cfg(windows)]
const LAUNCHER_EXE: &str = "baton-guardian.exe";

/// 組出正式執行用的 Port 組合；`data_dir` 是 Runtime 的資料目錄（Run 的啟動檔案與輸出放在其下），
/// `port` 是 Runtime 的埠（寫進登入啟動登記的命令列）。
pub fn production_ports(data_dir: &Path, port: u16) -> Ports {
    let unavailable = Arc::new(Unavailable);
    // Windows 上使用真實的 Job Object 實作；其他平台維持尚未實作
    #[cfg(windows)]
    let process_control: Arc<dyn ProcessControl> = Arc::new(crate::windows_process::WindowsProcessControl::new());
    #[cfg(not(windows))]
    let process_control: Arc<dyn ProcessControl> = unavailable.clone();
    // 登入識別與登入啟動登記（票 33）：Windows 上是真實實作；找不到本程序的執行檔就無法登記
    #[cfg(windows)]
    let (login_session, startup_registration): (Arc<dyn LoginSession>, Arc<dyn StartupRegistration>) = (
        Arc::new(crate::windows_login::WindowsLoginSession),
        match std::env::current_exe() {
            Ok(exe) => Arc::new(crate::windows_login::HkcuRunRegistration::for_launcher(
                &exe.with_file_name(LAUNCHER_EXE),
                data_dir,
                port,
            )),
            Err(_) => unavailable.clone(),
        },
    );
    #[cfg(not(windows))]
    let (login_session, startup_registration): (Arc<dyn LoginSession>, Arc<dyn StartupRegistration>) = {
        let _ = port;
        (unavailable.clone(), unavailable.clone())
    };
    Ports {
        clock: Arc::new(SystemClock),
        process_control,
        agent_adapter: Arc::new(ClaudeCodeAdapter::for_current_process(data_dir)),
        quota_observer: unavailable.clone(),
        workspace_fs: Arc::new(LocalWorkspaceFs),
        platform_info: Arc::new(WindowsPlatformInfo),
        // DPAPI 是真實實作：桌面憑證檔一定要受保護
        secret_protector: Arc::new(baton_credential::DpapiProtector),
        claude_env: Arc::new(ClaudeCli),
        command_runner: Arc::new(LocalCommandRunner),
        user_settings: Arc::new(user_settings_source()),
        guardian_probe: Arc::new(GuardianInstanceProbe { data_dir: data_dir.to_path_buf() }),
        login_session,
        startup_registration,
        // 建置時設定了更新清單與簽章公鑰才連線檢查；否則是「未設定」（票 34）
        update_source: match crate::update_source::configured() {
            Some(source) => Arc::new(source),
            None => unavailable.clone(),
        },
        verification_driver: Arc::new(crate::verification_driver::LocalVerificationDriver::default()),
        browser_driver: Arc::new(crate::browser_driver::LocalBrowserDriver::default()),
        semantic_judge: Arc::new(crate::semantic_judge::ClaudeSemanticJudge::default()),
        secret_resolver: Arc::new(crate::secret_resolver::EnvSecretResolver),
        model_directory: Arc::new(crate::model_directory::AnthropicModelDirectory::default()),
    }
}

/// 以監護記下的程序身分判斷監護是否還在（票 32）。
struct GuardianInstanceProbe {
    data_dir: std::path::PathBuf,
}

impl GuardianProbe for GuardianInstanceProbe {
    fn guardian_running(&self) -> bool {
        baton_guardian::files::instance_alive(&baton_guardian::files::guardian_instance_path(&self.data_dir))
    }
}

/// 使用者平常使用的 Claude Code 設定位置：設定了 `CLAUDE_CONFIG_DIR` 就用該目錄（`.claude.json` 在其中），
/// 否則是使用者家目錄下的 `.claude` 與 `.claude.json`。
fn user_settings_source() -> DirUserSettingsSource {
    let from_env = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty()).map(std::path::PathBuf::from);
    if let Some(dir) = from_env("CLAUDE_CONFIG_DIR") {
        return DirUserSettingsSource::new(dir.clone(), dir.join(".claude.json"));
    }
    let home = from_env("USERPROFILE").or_else(|| from_env("HOME")).unwrap_or_default();
    DirUserSettingsSource::new(home.join(".claude"), home.join(".claude.json"))
}
