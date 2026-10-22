//! 桌面 App 的 Runtime 用戶端：連回既有的 Runtime，沒有就啟動一個。
//!
//! 單實例由 Runtime 本身用具名 mutex 保證；這裡先連線、連不上才啟動，
//! 即使兩個 App 同時啟動，多出來的 Runtime 也會因取不到 mutex 而自行結束。

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use baton_api::{
    admission_check_path, authorization_path, authorization_summary_path, goals_path, task_admission_check_path,
    task_cancel_path, task_contract_path, task_dependencies_path, task_path, task_ready_path, tasks_path,
    AddDependencyRequest, AdmissionCheckRequest, AdmissionReport, AuthorizationView, ConfirmAuthorizationRequest,
    CreateGoalRequest, CreateTaskRequest, CreateWorkspaceRequest, ErrorBody, HealthResponse, PlatformResponse,
    SummaryRequest, TaskBoard, TaskDetail, UpdateContractRequest, WorkspaceList, CLIENT_HEADER, CLIENT_HEADER_VALUE,
    HEALTH_PATH, PLATFORM_PATH, WORKSPACES_PATH,
};
use baton_domain::authorization::{Admission, AuthorizationRevision, ConfirmationSummary};
use baton_domain::task::{ContractContent, Goal, TaskView};
use baton_domain::workspace::Workspace;
use baton_credential::CredentialFileError;
use baton_ports::SecretProtector;

/// 連線方式：連回既有的，或由本次呼叫啟動。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Connection {
    AlreadyRunning,
    Started,
}

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("無法連線到 Runtime：{0}")]
    Unreachable(String),
    #[error("無法啟動 Runtime：{0}")]
    Spawn(std::io::Error),
    #[error("Runtime 在 {0:?} 內沒有就緒")]
    StartTimeout(Duration),
    /// 讀不到或還原不了桌面憑證；此時不會送出任何請求。
    #[error("無法取得桌面憑證：{0}")]
    Credential(#[from] CredentialFileError),
    /// Runtime 拒絕了請求（憑證無效、過期、角色不符等）；`code` 是 Runtime 回報的穩定代碼。
    #[error("Runtime 拒絕連線（{code}）：{message}")]
    Rejected { code: String, message: String },
}

/// 桌面 App 對 Runtime 的用戶端：每次呼叫都重新讀取憑證檔並帶上 bearer 憑證與固定標頭。
///
/// 因為每次都讀檔，Runtime 重啟輪替憑證後不需要任何人工動作就會用新憑證連線；
/// 驗證失敗時回報原因，不會改用免驗證的方式重試。
pub struct RuntimeClient {
    addr: SocketAddr,
    data_dir: PathBuf,
    protector: Arc<dyn SecretProtector>,
}

impl RuntimeClient {
    pub fn new(addr: SocketAddr, data_dir: PathBuf, protector: Arc<dyn SecretProtector>) -> Self {
        Self { addr, data_dir, protector }
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// 查詢 Runtime 的健康狀態。
    pub async fn health(&self) -> Result<HealthResponse, ClientError> {
        self.get_json(HEALTH_PATH).await
    }

    /// 查詢 Runtime 的平台狀態。
    pub async fn platform(&self) -> Result<PlatformResponse, ClientError> {
        self.get_json(PLATFORM_PATH).await
    }

    /// 列出全部 Workspace（含已封存）。
    pub async fn workspaces(&self) -> Result<WorkspaceList, ClientError> {
        self.get_json(WORKSPACES_PATH).await
    }

    /// 以使用者選擇的目錄建立 Workspace。
    pub async fn create_workspace(&self, request: &CreateWorkspaceRequest) -> Result<Workspace, ClientError> {
        self.send_json(reqwest::Method::POST, WORKSPACES_PATH, Some(request)).await
    }

    pub async fn archive_workspace(&self, id: &str) -> Result<Workspace, ClientError> {
        self.send_json::<(), _>(reqwest::Method::POST, &format!("{WORKSPACES_PATH}/{id}/archive"), None).await
    }

    /// 讀取 Workspace 的授權設定：目前生效的 revision、歷史、建議範本與能力清單。
    pub async fn authorization(&self, workspace_id: &str) -> Result<AuthorizationView, ClientError> {
        self.get_json(&authorization_path(workspace_id)).await
    }

    /// 確認前的摘要（預覽，不保存）。
    pub async fn authorization_summary(
        &self,
        workspace_id: &str,
        request: &SummaryRequest,
    ) -> Result<ConfirmationSummary, ClientError> {
        self.send_json(reqwest::Method::POST, &authorization_summary_path(workspace_id), Some(request)).await
    }

    /// 確認並保存授權；內容相同時回傳既有的 revision。
    pub async fn confirm_authorization(
        &self,
        workspace_id: &str,
        request: &ConfirmAuthorizationRequest,
    ) -> Result<AuthorizationRevision, ClientError> {
        self.send_json(reqwest::Method::PUT, &authorization_path(workspace_id), Some(request)).await
    }

    /// Run 的准入檢查（被拒絕時回傳 `ClientError::Rejected`，附原因代碼）。
    pub async fn check_admission(
        &self,
        workspace_id: &str,
        request: &AdmissionCheckRequest,
    ) -> Result<Admission, ClientError> {
        self.send_json(reqwest::Method::POST, &admission_check_path(workspace_id), Some(request)).await
    }

    /// 工作面板：Workspace 內全部 Goal 與 Task。
    pub async fn task_board(&self, workspace_id: &str) -> Result<TaskBoard, ClientError> {
        self.get_json(&tasks_path(workspace_id)).await
    }

    pub async fn create_goal(&self, workspace_id: &str, title: &str) -> Result<Goal, ClientError> {
        let request = CreateGoalRequest { title: title.to_string() };
        self.send_json(reqwest::Method::POST, &goals_path(workspace_id), Some(&request)).await
    }

    pub async fn create_task(&self, workspace_id: &str, request: &CreateTaskRequest) -> Result<TaskView, ClientError> {
        self.send_json(reqwest::Method::POST, &tasks_path(workspace_id), Some(request)).await
    }

    /// 單一 Task 與契約版本歷史。
    pub async fn task_detail(&self, workspace_id: &str, task_id: &str) -> Result<TaskDetail, ClientError> {
        self.get_json(&task_path(workspace_id, task_id)).await
    }

    /// 整份取代契約；內容相同時不新增版本。
    pub async fn update_task_contract(
        &self,
        workspace_id: &str,
        task_id: &str,
        contract: ContractContent,
        note: Option<String>,
    ) -> Result<TaskView, ClientError> {
        let request = UpdateContractRequest { contract, note };
        self.send_json(reqwest::Method::PUT, &task_contract_path(workspace_id, task_id), Some(&request)).await
    }

    pub async fn add_task_dependency(
        &self,
        workspace_id: &str,
        task_id: &str,
        depends_on: &str,
    ) -> Result<TaskView, ClientError> {
        let request = AddDependencyRequest { depends_on: depends_on.to_string() };
        self.send_json(reqwest::Method::POST, &task_dependencies_path(workspace_id, task_id), Some(&request)).await
    }

    /// 准入檢查的唯讀預覽。
    pub async fn check_task_admission(&self, workspace_id: &str, task_id: &str) -> Result<AdmissionReport, ClientError> {
        self.send_json::<(), _>(reqwest::Method::POST, &task_admission_check_path(workspace_id, task_id), None).await
    }

    /// pending → ready；准入檢查未通過時回傳 `ClientError::Rejected`（`task_admission_denied`，訊息列出缺項）。
    pub async fn mark_task_ready(&self, workspace_id: &str, task_id: &str) -> Result<TaskView, ClientError> {
        self.send_json::<(), _>(reqwest::Method::POST, &task_ready_path(workspace_id, task_id), None).await
    }

    pub async fn cancel_task(&self, workspace_id: &str, task_id: &str) -> Result<TaskView, ClientError> {
        self.send_json::<(), _>(reqwest::Method::POST, &task_cancel_path(workspace_id, task_id), None).await
    }

    async fn get_json<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T, ClientError> {
        self.send_json::<(), T>(reqwest::Method::GET, path, None).await
    }

    async fn send_json<B: serde::Serialize, T: serde::de::DeserializeOwned>(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&B>,
    ) -> Result<T, ClientError> {
        let token = baton_credential::read_desktop_credential(&self.data_dir, &*self.protector)?;
        let unreachable = |e: reqwest::Error| ClientError::Unreachable(e.to_string());
        // Windows 連到未監聽的 loopback 埠要等約 2 秒才失敗；連線逾時設短一點，連不上時才能快速判斷
        let mut request = reqwest::Client::builder()
            .connect_timeout(Duration::from_millis(300))
            .build()
            .map_err(unreachable)?
            .request(method, format!("http://{}{path}", self.addr))
            .bearer_auth(token)
            .header(CLIENT_HEADER, CLIENT_HEADER_VALUE)
            .timeout(Duration::from_secs(2));
        if let Some(body) = body {
            request = request.json(body);
        }
        // reqwest 的錯誤訊息只含 URL（不含憑證，憑證在標頭裡）
        let response = request.send().await.map_err(unreachable)?;

        if response.status().is_success() {
            return response.json().await.map_err(unreachable);
        }
        let status = response.status();
        Err(match response.json::<ErrorBody>().await {
            Ok(body) => ClientError::Rejected { code: body.error, message: body.message },
            Err(_) => ClientError::Rejected { code: format!("http_{}", status.as_u16()), message: status.to_string() },
        })
    }
}

/// 確保 Runtime 可用：已在執行就連回，否則呼叫 `start` 啟動並等到就緒（最多 `wait`）。
///
/// Runtime 已在執行但拒絕憑證時，直接回報原因，不會再啟動另一個。
pub async fn ensure_runtime(
    client: &RuntimeClient,
    start: impl FnOnce() -> std::io::Result<()>,
    wait: Duration,
) -> Result<(Connection, HealthResponse), ClientError> {
    match client.health().await {
        Ok(health) => return Ok((Connection::AlreadyRunning, health)),
        Err(rejected @ ClientError::Rejected { .. }) => return Err(rejected),
        Err(_) => {}
    }

    start().map_err(ClientError::Spawn)?;

    let deadline = tokio::time::Instant::now() + wait;
    let mut stale_credential = None;
    while tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
        match client.health().await {
            Ok(health) => return Ok((Connection::Started, health)),
            // 剛讀到的是啟動前的舊憑證：Runtime 在開始監聽前才換新憑證，下一輪重讀就會對
            Err(ClientError::Rejected { code, message }) if code == "credential_invalid" => {
                stale_credential = Some(ClientError::Rejected { code, message });
            }
            Err(rejected @ ClientError::Rejected { .. }) => return Err(rejected),
            Err(_) => {}
        }
    }
    Err(stale_credential.unwrap_or(ClientError::StartTimeout(wait)))
}

/// 以脫離本程序的方式啟動 Runtime 執行檔；關閉 App 後 Runtime 仍會繼續執行。
pub fn spawn_runtime_process(exe: &Path, envs: &[(String, String)]) -> std::io::Result<std::process::Child> {
    use std::process::{Command, Stdio};

    let mut command = Command::new(exe);
    command.envs(envs.iter().map(|(k, v)| (k, v))).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
    }
    command.spawn()
}
