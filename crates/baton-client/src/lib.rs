//! 桌面 App 的 Runtime 用戶端：連回既有的 Runtime，沒有就啟動一個。
//!
//! 單實例由 Runtime 本身用具名 mutex 保證；這裡先連線、連不上才啟動，
//! 即使兩個 App 同時啟動，多出來的 Runtime 也會因取不到 mutex 而自行結束。

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use baton_api::{
    admission_check_path, artifact_content_path,
    controller_cli_end_path, controller_cli_path, controller_cli_switch_path, ControllerCliView, OpenControllerCliRequest,
    artifact_preview_path, artifacts_path, authorization_path,
    authorization_summary_path, decision_answer_path, decision_authority_path, decision_links_path, decision_merge_path,
    decision_path, decisions_path, goals_path, run_cancel_path, run_context_path, RunContextView, BatchRequest, BatchResponse, LinkRequest, MergeRequest,
    MergeResponse, UpdateAuthorityRequest, DECISIONS_BATCH_PATH,
    ArtifactList, ArtifactPreview, SHA256_HEADER,
    task_admission_check_path, task_cancel_path, task_contract_path, task_dependencies_path, task_dispatch_path,
    quota::plan_limits_path, run_settings_snapshot_path, shared_settings_version_path, task_accept_path, task_path, task_ready_path, task_verification_path, tasks_path, workspace_lifecycle_path, workspace_resume_path, AcceptTaskRequest, LifecycleStatus,
    workspace_login_resume_path, LoginStartupView, UpdateLoginResumeRequest, UpdateLoginStartupRequest, LOGIN_STARTUP_PATH,
    QueuedDispatch, UpdateWorkspaceLifecycleRequest, WorkspaceLifecycleView, LIFECYCLE_EXIT_PATH, LIFECYCLE_PATH,
    SchedulingOverview, UpdateSchedulingSettingsRequest, SCHEDULING_PATH, SCHEDULING_SETTINGS_PATH,
    BackupStatus, BACKUPS_PATH,
    RecoveryStatusView, RECOVERY_PATH, RECOVERY_RETRY_PATH,
    ApplyUpdateRequest, MaintenanceRequestBody, UpdateStatusView, MAINTENANCE_PATH, UPDATES_PATH, UPDATE_APPLY_PATH,
    metrics_path, timeline_path, MetricsResponse, TimelineResponse,
    projection_reconcile_path, understanding_path, understanding_sync_path, ReconcileRequest, UnderstandingView,
    workflow_skip_path, workflows_path, PlaybookCatalog, SkipStageRequest, WorkflowList, WorkflowView, PLAYBOOKS_PATH,
    reviews_path, ReviewList,
    evidence_artifact_path, evidence_bundle_path, verification_profile_path, verification_runs_path,
    workspace_verification_path, EvidenceArtifactContent, PublishProfileResponse, RunVerificationRequest,
    VerificationOverviewView,
    workspace_exploration_path, ExplorationOverview,
    AddDependencyRequest, AdmissionCheckRequest, AdmissionReport, AdoptRequest, AnswerRequest, AuthorizationView,
    ConfirmAuthorizationRequest, CreateGoalRequest, CreateProfileFromDirRequest, CreateProfileRequest, CreateTaskRequest, CreateWorkspaceRequest,
    ErrorBody, HealthResponse, ImportPreview, LoginTerminalResponse, PendingDecisions, PlatformResponse, ProfileList,
    RunSettingsSnapshot, SetupStatus, SharedSettingsStatus, SnapshotRequest, SummaryRequest, TaskBoard, TaskDetail,
    knowledge_action_path, knowledge_entry_path, ConfirmKnowledgeRequest, CorrectKnowledgeRequest,
    CreateKnowledgeRequest, KnowledgeDetail, KnowledgeEntryView, KnowledgeList, KnowledgeListQuery,
    KnowledgeReasonRequest, KNOWLEDGE_PATH,
    guard_action_path, recurrences_path, CreateGuardRequest, GuardList, GuardRevisionRequest, GuardView, RecurrenceList,
    ReportRecurrenceRequest, GUARDS_PATH,
    UpdateContractRequest, WorkspaceList, CLIENT_HEADER, CLIENT_HEADER_VALUE, HEALTH_PATH, PENDING_DECISIONS_PATH,
    PLATFORM_PATH, PROFILES_FROM_EXISTING_DIR_PATH, PROFILES_PATH, SETUP_ACK_PATH, SETUP_PATH, SETUP_RECHECK_PATH, SHARED_SETTINGS_ADOPT_PATH,
    SHARED_SETTINGS_PATH, SHARED_SETTINGS_PREVIEW_PATH, WORKSPACES_PATH,
};
use baton_domain::authority::AuthorityTable;
use baton_domain::decision::DecisionView;
use baton_domain::authorization::{Admission, AuthorizationRevision, ConfirmationSummary};
use baton_domain::profile::ProfileView;
use baton_domain::quota::PlanLimits;
use baton_domain::run::RunView;
use baton_domain::scheduler::CapacityMode;
use baton_domain::shared_settings::SettingsVersion;
use baton_domain::task::{ContractContent, Goal, TaskView};
use baton_domain::verification::VerificationView;
use baton_domain::verification_profile::{EvidenceBundle, ProfileDefinition, VerificationRequest};
use baton_domain::workspace::{ModelOverrides, Workspace};
use baton_credential::CredentialFileError;
use baton_ports::SecretProtector;
use baton_store::{AnswerResult, DecisionList};

/// 派工的結果：Scheduler 找到帳號與名額就建立 Run；否則排進佇列並回報原因。
/// 序列化後與 Runtime 回應的 JSON 形狀相同（排隊中有 `queued: true`），Web UI 以它區分兩種結果。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum DispatchOutcome {
    Queued(QueuedDispatch),
    Started(Box<RunView>),
}

/// 一般請求等待回應的上限。
const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);

/// 下載成果（最多 10 MiB，仍要讀檔並重新計算雜湊）等待回應的上限。
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(60);

/// 同意更新的請求要先下載並驗證安裝程式（票 34），等待回應的上限。
const UPDATE_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// 需要在本機執行 `claude`、`git` 的請求（核對登入、精靈檢查、派工前準備工作目錄）等待回應的上限。
const PROBE_TIMEOUT: Duration = Duration::from_secs(60);

/// 下載到的成果內容，以及 Runtime 以 `X-Baton-Sha256` 回報的雜湊（Runtime 已與 manifest 核對過）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactDownload {
    pub bytes: Vec<u8>,
    pub sha256: String,
}

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
    /// 共用的 HTTP 用戶端：App 每 0.5 秒輪詢一次，重用連線池裡的連線。
    http: reqwest::Client,
}

impl RuntimeClient {
    pub fn new(addr: SocketAddr, data_dir: PathBuf, protector: Arc<dyn SecretProtector>) -> Self {
        // Windows 連到未監聽的 loopback 埠要等約 2 秒才失敗；連線逾時設短一點，連不上時才能快速判斷
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_millis(300))
            .build()
            .expect("建立 HTTP 用戶端（沒有 TLS，只有本機連線設定）不會失敗");
        Self { addr, data_dir, protector, http }
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

    /// 整組取代 Workspace 依角色的模型覆寫（`None` 表示沿用帳號的 Claude Code 預設）；只影響之後派出的 Run。
    pub async fn set_model_overrides(&self, workspace_id: &str, overrides: &ModelOverrides) -> Result<Workspace, ClientError> {
        self.send_json(reqwest::Method::PUT, &format!("{WORKSPACES_PATH}/{workspace_id}/model-overrides"), Some(overrides)).await
    }

    /// 列出全部帳號 Profile（含已封存）與可派工判定。
    pub async fn profiles(&self) -> Result<ProfileList, ClientError> {
        self.get_json(PROFILES_PATH).await
    }

    /// 新增帳號 Profile；Runtime 為它建立獨立的設定目錄。
    pub async fn create_profile(&self, request: &CreateProfileRequest) -> Result<ProfileView, ClientError> {
        self.send_json(reqwest::Method::POST, PROFILES_PATH, Some(request)).await
    }

    /// 以既有的 Claude Code 設定目錄建立 Profile；Runtime 建立後立即核對登入身分。
    pub async fn create_profile_from_dir(&self, request: &CreateProfileFromDirRequest) -> Result<ProfileView, ClientError> {
        self.send_json_within(reqwest::Method::POST, PROFILES_FROM_EXISTING_DIR_PATH, Some(request), PROBE_TIMEOUT).await
    }

    /// 重新核對 Profile 的登入狀態、認證來源與帳號身分。
    pub async fn verify_profile(&self, id: &str) -> Result<ProfileView, ClientError> {
        self.send_json_within::<(), _>(reqwest::Method::POST, &format!("{PROFILES_PATH}/{id}/verify"), None, PROBE_TIMEOUT)
            .await
    }

    /// 開啟終端機，以該 Profile 自己的設定目錄執行原生登入。
    pub async fn open_login_terminal(&self, id: &str) -> Result<LoginTerminalResponse, ClientError> {
        self.send_json::<(), _>(reqwest::Method::POST, &format!("{PROFILES_PATH}/{id}/login-terminal"), None).await
    }

    pub async fn archive_profile(&self, id: &str) -> Result<ProfileView, ClientError> {
        self.send_json::<(), _>(reqwest::Method::POST, &format!("{PROFILES_PATH}/{id}/archive"), None).await
    }

    /// 首次啟動精靈的狀態（重新探測 Claude Code 與 Git）。
    pub async fn setup(&self) -> Result<SetupStatus, ClientError> {
        self.send_json_within::<(), _>(reqwest::Method::GET, SETUP_PATH, None, PROBE_TIMEOUT).await
    }

    /// 「重新檢查」：重新探測工具並重新核對每個使用中的 Profile。
    pub async fn recheck_setup(&self) -> Result<SetupStatus, ClientError> {
        self.send_json_within::<(), _>(reqwest::Method::POST, SETUP_RECHECK_PATH, None, PROBE_TIMEOUT).await
    }

    pub async fn acknowledge_background_notice(&self) -> Result<SetupStatus, ClientError> {
        self.send_json_within::<(), _>(reqwest::Method::POST, SETUP_ACK_PATH, None, PROBE_TIMEOUT).await
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

    /// 對 ready 的 Task 派工：有帳號與名額就回傳 `queued` 的 Run（程序由 Supervisor 之後才啟動），
    /// 否則排進佇列並回傳等待原因；Task 不能派（不是 ready、契約失效）時回傳 `ClientError::Rejected`。
    pub async fn dispatch_task(&self, workspace_id: &str, task_id: &str) -> Result<DispatchOutcome, ClientError> {
        // Runtime 在回應前同步準備工作目錄（`git worktree add`、寫入並 fsync），不適用一般請求的 2 秒上限；
        // 逾時只代表 App 不再等，Runtime 端的派工照樣完成，App 會誤報連不上
        self.send_json_within::<(), _>(reqwest::Method::POST, &task_dispatch_path(workspace_id, task_id), None, PROBE_TIMEOUT)
            .await
    }

    /// 一個 Task 的驗證狀況：接受基礎、宣告的檢查、成果 manifest 與歷次驗證紀錄。
    pub async fn task_verification(&self, workspace_id: &str, task_id: &str) -> Result<VerificationView, ClientError> {
        self.get_json(&task_verification_path(workspace_id, task_id)).await
    }

    /// 使用者手動接受成果；Run 世代、契約版本或成果 manifest 與 `basis` 不符時回傳 `ClientError::Rejected`。
    pub async fn accept_task(
        &self,
        workspace_id: &str,
        task_id: &str,
        request: &AcceptTaskRequest,
    ) -> Result<VerificationView, ClientError> {
        self.send_json(reqwest::Method::POST, &task_accept_path(workspace_id, task_id), Some(request)).await
    }

    /// 排程總覽：容量、各帳號進行中的 Run、佇列與等待原因。
    pub async fn scheduling(&self) -> Result<SchedulingOverview, ClientError> {
        self.get_json(SCHEDULING_PATH).await
    }

    /// 設定頁的備份狀態：現有備份（由新到舊）與最新一次還原演練（票 40）。
    pub async fn backups(&self) -> Result<BackupStatus, ClientError> {
        self.get_json(BACKUPS_PATH).await
    }

    /// 切換容量預設（較高並行／保留交接容量），回傳切換後的排程總覽。
    pub async fn set_scheduling_mode(&self, mode: CapacityMode) -> Result<SchedulingOverview, ClientError> {
        let request = UpdateSchedulingSettingsRequest { mode };
        self.send_json(reqwest::Method::PUT, SCHEDULING_SETTINGS_PATH, Some(&request)).await
    }

    /// 設定一個 Profile 的方案上限（推估的分母，token；`None` 是未設定），回傳新的排程總覽；
    /// 上限不是正整數或 Profile 已封存時回傳 `ClientError::Rejected`。
    pub async fn set_quota_limits(&self, profile_id: &str, limits: &PlanLimits) -> Result<SchedulingOverview, ClientError> {
        self.send_json(reqwest::Method::PUT, &plan_limits_path(profile_id), Some(limits)).await
    }

    /// Controller 頁簽的狀態：這個 Workspace 最近一次的 Controller CLI Run（desktop-controller-cli 票 03）。
    pub async fn controller_cli(&self, workspace_id: &str) -> Result<ControllerCliView, ClientError> {
        self.get_json(&controller_cli_path(workspace_id)).await
    }

    /// 以指定 Profile 當 Controller 帳號開啟 Controller CLI；Run 從 `starting` 開始，由 Runtime 開窗。
    /// 已有開著的 Controller CLI、Profile 不可用或 Workspace 已封存時回傳 `ClientError::Rejected`。
    pub async fn open_controller_cli(&self, workspace_id: &str, profile_id: &str) -> Result<ControllerCliView, ClientError> {
        let request = OpenControllerCliRequest { profile_id: profile_id.to_string() };
        self.send_json(reqwest::Method::POST, &controller_cli_path(workspace_id), Some(&request)).await
    }

    /// 要求結束開著的 Controller CLI；沒有開著的時回傳 `ClientError::Rejected`。
    pub async fn end_controller_cli(&self, workspace_id: &str) -> Result<ControllerCliView, ClientError> {
        self.send_json::<(), _>(reqwest::Method::POST, &controller_cli_end_path(workspace_id), None).await
    }

    /// 換帳號重開：結束目前的 Controller CLI，Runtime 結束後以 `profile_id` 開新的 Run（不接續原生 Session）。
    /// 沒有開著的、與目前帳號相同、已在結束中或 Profile 不可用時回傳 `ClientError::Rejected`。
    pub async fn switch_controller_cli(&self, workspace_id: &str, profile_id: &str) -> Result<ControllerCliView, ClientError> {
        let request = OpenControllerCliRequest { profile_id: profile_id.to_string() };
        self.send_json(reqwest::Method::POST, &controller_cli_switch_path(workspace_id), Some(&request)).await
    }

    /// 要求取消 Run；已結束或重複取消是冪等的，回傳目前的 Run。
    pub async fn cancel_run(&self, workspace_id: &str, run_id: &str) -> Result<RunView, ClientError> {
        self.send_json::<(), _>(reqwest::Method::POST, &run_cancel_path(workspace_id, run_id), None).await
    }

    /// Run 的 context receipt 與實際交給 Worker 的 Prompt（票 39）。
    pub async fn run_context(&self, workspace_id: &str, run_id: &str) -> Result<RunContextView, ClientError> {
        self.get_json(&run_context_path(workspace_id, run_id)).await
    }

    /// 生命週期總覽：Runtime 階段、完全退出的停止進度與異常、各 Workspace 的自動續跑設定與暫停狀態。
    pub async fn lifecycle(&self) -> Result<LifecycleStatus, ClientError> {
        self.get_json(LIFECYCLE_PATH).await
    }

    /// 完全退出：Runtime 持久化停止意圖後立即回應，停止進度之後以 `lifecycle` 查詢；重複要求是冪等的。
    pub async fn request_exit(&self) -> Result<LifecycleStatus, ClientError> {
        self.send_json::<(), _>(reqwest::Method::POST, LIFECYCLE_EXIT_PATH, None).await
    }

    /// 更新狀態（票 34）：檢查結果（只通知）、進行中與最近一次的維護。
    pub async fn updates(&self) -> Result<UpdateStatusView, ClientError> {
        self.get_json(UPDATES_PATH).await
    }

    /// 同意更新到 `version`（票 34）：Runtime 先下載並驗證簽章，通過才開始維護並回應；之後以 `updates` 查詢維護進度。
    /// 下載或驗證失敗時回傳 `ClientError::Rejected`（`update_download_failed` 等），工作沒有停止。
    pub async fn apply_update(&self, version: &str) -> Result<UpdateStatusView, ClientError> {
        let request = ApplyUpdateRequest { version: version.to_string() };
        self.send_json_within(reqwest::Method::POST, UPDATE_APPLY_PATH, Some(&request), UPDATE_DOWNLOAD_TIMEOUT).await
    }

    /// 安裝程式要求維護（票 34；解除安裝、或不經 App 的更新安裝）：Runtime 持久化維護與停止意圖後立即回應。
    pub async fn request_maintenance(
        &self,
        kind: baton_domain::maintenance::MaintenanceKind,
    ) -> Result<UpdateStatusView, ClientError> {
        self.send_json(reqwest::Method::POST, MAINTENANCE_PATH, Some(&MaintenanceRequestBody { kind })).await
    }

    /// 崩潰恢復總覽：目前的恢復事件（含全部輪次的嘗試與受影響工作）、每輪上限、能否重試與派工阻擋原因（票 31）。
    pub async fn recovery(&self) -> Result<RecoveryStatusView, ClientError> {
        self.get_json(RECOVERY_PATH).await
    }

    /// 「重新嘗試恢復」：建立新輪次並保留歷史；不是恢復額度用盡時回傳 `ClientError::Rejected`
    /// （`recovery_not_exhausted`，沒有任何事件時 `no_recovery_event`）。
    pub async fn retry_recovery(&self) -> Result<RecoveryStatusView, ClientError> {
        self.send_json::<(), _>(reqwest::Method::POST, RECOVERY_RETRY_PATH, None).await
    }

    /// 變更 Workspace 的「完全退出後自動續跑」（只影響下次重新開啟）。
    pub async fn set_workspace_auto_resume(
        &self,
        workspace_id: &str,
        auto_resume_after_exit: bool,
    ) -> Result<WorkspaceLifecycleView, ClientError> {
        let request = UpdateWorkspaceLifecycleRequest { auto_resume_after_exit };
        self.send_json(reqwest::Method::PUT, &workspace_lifecycle_path(workspace_id), Some(&request)).await
    }

    /// 變更 Workspace 的「跨登入自動續跑」（票 33；只影響之後的跨登入啟動）。
    pub async fn set_workspace_login_resume(
        &self,
        workspace_id: &str,
        auto_resume_across_login: bool,
    ) -> Result<WorkspaceLifecycleView, ClientError> {
        let request = UpdateLoginResumeRequest { auto_resume_across_login };
        self.send_json(reqwest::Method::PUT, &workspace_login_resume_path(workspace_id), Some(&request)).await
    }

    /// 變更「登入時自動啟動」（票 33）；Windows 的登記失敗時回傳 `ClientError::Rejected`
    /// （`login_startup_registration_failed`），偏好不變。
    pub async fn set_login_startup(&self, enabled: bool) -> Result<LoginStartupView, ClientError> {
        let request = UpdateLoginStartupRequest { enabled };
        self.send_json(reqwest::Method::PUT, LOGIN_STARTUP_PATH, Some(&request)).await
    }

    /// 「繼續工作」：解除 Workspace 在完全退出後的暫停。
    pub async fn resume_workspace(&self, workspace_id: &str) -> Result<WorkspaceLifecycleView, ClientError> {
        self.send_json::<(), _>(reqwest::Method::POST, &workspace_resume_path(workspace_id), None).await
    }

    /// 裁定中心：Workspace 內的裁定請求（待處理優先）與待處理數。
    pub async fn decision_list(&self, workspace_id: &str) -> Result<DecisionList, ClientError> {
        self.get_json(&decisions_path(workspace_id)).await
    }

    pub async fn decision_detail(&self, workspace_id: &str, request_id: &str) -> Result<DecisionView, ClientError> {
        self.get_json(&decision_path(workspace_id, request_id)).await
    }

    /// 處理一個裁定請求（六種處理方式之一）；同樣的處理重送是冪等的，不同的處理回傳 `ClientError::Rejected`
    /// （`decision_already_answered`）。內容在檢視後變動回 `decision_revision_mismatch`，目標版本已變更回 `decision_superseded`。
    pub async fn decision_answer(
        &self,
        workspace_id: &str,
        request_id: &str,
        request: &AnswerRequest,
    ) -> Result<AnswerResult, ClientError> {
        self.send_json(reqwest::Method::POST, &decision_answer_path(workspace_id, request_id), Some(request)).await
    }

    /// 批次處理：每一項各自保存，一項失敗不影響其他項。
    pub async fn decisions_batch(&self, request: &BatchRequest) -> Result<BatchResponse, ClientError> {
        self.send_json(reqwest::Method::POST, DECISIONS_BATCH_PATH, Some(request)).await
    }

    /// 把一個請求合併到另一個（由目標代為裁定）；回傳合併後的目標。
    pub async fn decision_merge(
        &self,
        workspace_id: &str,
        request_id: &str,
        request: &MergeRequest,
    ) -> Result<MergeResponse, ClientError> {
        self.send_json(reqwest::Method::POST, &decision_merge_path(workspace_id, request_id), Some(request)).await
    }

    /// 把兩個請求互相連結。
    pub async fn decision_link(&self, workspace_id: &str, request_id: &str, other_id: &str) -> Result<DecisionView, ClientError> {
        let request = LinkRequest { other_id: other_id.to_string() };
        self.send_json(reqwest::Method::POST, &decision_links_path(workspace_id, request_id), Some(&request)).await
    }

    /// Workspace 的決策權限表（spec D-15）。
    pub async fn decision_authority(&self, workspace_id: &str) -> Result<AuthorityTable, ClientError> {
        self.get_json(&decision_authority_path(workspace_id)).await
    }

    /// 取代 Workspace 的決策權限表；任何事項都可以設為任何等級，沒列出的回到內建值。
    pub async fn update_decision_authority(
        &self,
        workspace_id: &str,
        request: &UpdateAuthorityRequest,
    ) -> Result<AuthorityTable, ClientError> {
        self.send_json(reqwest::Method::PUT, &decision_authority_path(workspace_id), Some(request)).await
    }

    /// 全域待處理裁定數（狀態列）。
    pub async fn pending_decisions(&self) -> Result<PendingDecisions, ClientError> {
        self.get_json(PENDING_DECISIONS_PATH).await
    }

    /// 共同設定的目前狀態：已採用的版本與最新版本。
    pub async fn shared_settings(&self) -> Result<SharedSettingsStatus, ClientError> {
        self.get_json(SHARED_SETTINGS_PATH).await
    }

    /// 預覽匯入：來源、排除項目與相對於目前版本的差異（不保存任何東西）。
    pub async fn preview_settings_import(&self) -> Result<ImportPreview, ClientError> {
        self.send_json::<(), _>(reqwest::Method::POST, SHARED_SETTINGS_PREVIEW_PATH, None).await
    }

    /// 採用預覽的內容成為新版本；來源在預覽後改變時回傳 `ClientError::Rejected`（`settings_source_changed`）。
    pub async fn adopt_settings(&self, expected_hash: &str) -> Result<SettingsVersion, ClientError> {
        let request = AdoptRequest { expected_hash: expected_hash.to_string() };
        self.send_json(reqwest::Method::POST, SHARED_SETTINGS_ADOPT_PATH, Some(&request)).await
    }

    /// 讀取單一共同設定版本（含完整內容）。
    pub async fn settings_version(&self, version: i64) -> Result<SettingsVersion, ClientError> {
        self.get_json(&shared_settings_version_path(version)).await
    }

    /// 為 Run 產生唯讀設定快照；`version` 省略時使用最新版本，Run 已綁定版本時回傳同一個綁定。
    pub async fn run_settings_snapshot(
        &self,
        workspace_id: &str,
        run_id: &str,
        version: Option<i64>,
    ) -> Result<RunSettingsSnapshot, ClientError> {
        let request = SnapshotRequest { version };
        self.send_json(reqwest::Method::POST, &run_settings_snapshot_path(workspace_id, run_id), Some(&request)).await
    }

    /// 成果瀏覽：已登記的成果與全部版本，以及沒有 manifest 的未登記檔案。
    pub async fn list_artifacts(&self, workspace_id: &str) -> Result<ArtifactList, ClientError> {
        self.get_json(&artifacts_path(workspace_id)).await
    }

    /// 成果某個版本的預覽（Runtime 會先核對雜湊；不符回傳 `ClientError::Rejected`）。
    pub async fn preview_artifact(
        &self,
        workspace_id: &str,
        artifact_id: &str,
        version: i64,
    ) -> Result<ArtifactPreview, ClientError> {
        self.get_json(&artifact_preview_path(workspace_id, artifact_id, version)).await
    }

    /// 下載成果某個版本；與其他呼叫一樣驗證桌面身分，內容與 manifest 不符時回傳 `ClientError::Rejected`。
    pub async fn download_artifact(
        &self,
        workspace_id: &str,
        artifact_id: &str,
        version: i64,
    ) -> Result<ArtifactDownload, ClientError> {
        let path = artifact_content_path(workspace_id, artifact_id, version);
        let response = self.request_ok::<()>(reqwest::Method::GET, &path, None, DOWNLOAD_TIMEOUT).await?;
        let sha256 = response.headers().get(SHA256_HEADER).and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
        let bytes = response.bytes().await.map_err(|e| ClientError::Unreachable(e.to_string()))?.to_vec();
        Ok(ArtifactDownload { bytes, sha256 })
    }

    /// 知識分頁的列表或搜尋：`workspace_id` 省略時只列全域條目；`include_inactive` 為 false 時只回傳有效條目。
    /// 關鍵字不合規則（例如少於 2 個字元）時回傳 `ClientError::Rejected`（`knowledge_query_invalid`）。
    pub async fn list_knowledge(&self, query: &KnowledgeListQuery) -> Result<KnowledgeList, ClientError> {
        // 以 URL 的表單編碼組查詢字串，中文等非 ASCII 字元才會正確編碼
        let mut url = reqwest::Url::parse("http://localhost").expect("固定的 URL 一定合法");
        url.set_path(KNOWLEDGE_PATH);
        {
            let mut pairs = url.query_pairs_mut();
            if let Some(workspace_id) = &query.workspace_id {
                pairs.append_pair("workspace_id", workspace_id);
            }
            if let Some(q) = &query.q {
                pairs.append_pair("q", q);
            }
            pairs.append_pair("include_inactive", if query.include_inactive { "true" } else { "false" });
        }
        self.get_json(&format!("{}?{}", url.path(), url.query().unwrap_or_default())).await
    }

    /// 使用者建立條目（直接是已驗證）。
    pub async fn create_knowledge(&self, request: &CreateKnowledgeRequest) -> Result<KnowledgeEntryView, ClientError> {
        self.send_json(reqwest::Method::POST, KNOWLEDGE_PATH, Some(request)).await
    }

    /// 單一條目：目前狀態、全部內容版本與變更歷史。
    pub async fn knowledge_detail(&self, id: &str) -> Result<KnowledgeDetail, ClientError> {
        self.get_json(&knowledge_entry_path(id)).await
    }

    /// 確認候選教訓，升為已驗證。
    pub async fn confirm_knowledge(
        &self,
        id: &str,
        request: &ConfirmKnowledgeRequest,
    ) -> Result<KnowledgeEntryView, ClientError> {
        self.send_json(reqwest::Method::POST, &knowledge_action_path(id, "confirm"), Some(request)).await
    }

    /// 修正內容（追加新版本，舊版本保留）。
    pub async fn correct_knowledge(
        &self,
        id: &str,
        request: &CorrectKnowledgeRequest,
    ) -> Result<KnowledgeEntryView, ClientError> {
        self.send_json(reqwest::Method::POST, &knowledge_action_path(id, "correct"), Some(request)).await
    }

    /// 撤銷條目（需說明原因）。
    pub async fn revoke_knowledge(
        &self,
        id: &str,
        request: &KnowledgeReasonRequest,
    ) -> Result<KnowledgeEntryView, ClientError> {
        self.send_json(reqwest::Method::POST, &knowledge_action_path(id, "revoke"), Some(request)).await
    }

    /// 淘汰條目（需說明原因）。
    pub async fn retire_knowledge(
        &self,
        id: &str,
        request: &KnowledgeReasonRequest,
    ) -> Result<KnowledgeEntryView, ClientError> {
        self.send_json(reqwest::Method::POST, &knowledge_action_path(id, "retire"), Some(request)).await
    }

    /// 知識分頁看得到的 Guard（來源教訓是全域或屬於這個 Workspace），附是否生效與成效（票 38）。
    pub async fn list_guards(&self, workspace_id: Option<&str>) -> Result<GuardList, ClientError> {
        let mut url = reqwest::Url::parse("http://localhost").expect("固定的 URL 一定合法");
        url.set_path(GUARDS_PATH);
        if let Some(workspace_id) = workspace_id {
            url.query_pairs_mut().append_pair("workspace_id", workspace_id);
        }
        match url.query() {
            Some(query) => self.get_json(&format!("{}?{query}", url.path())).await,
            None => self.get_json(url.path()).await,
        }
    }

    /// 把有效的已驗證教訓轉成 Guard（建立即啟用）。
    pub async fn create_guard(&self, request: &CreateGuardRequest) -> Result<GuardView, ClientError> {
        self.send_json(reqwest::Method::POST, GUARDS_PATH, Some(request)).await
    }

    /// 啟用 Guard（來源教訓必須仍有效）。
    pub async fn enable_guard(&self, id: &str, request: &GuardRevisionRequest) -> Result<GuardView, ClientError> {
        self.send_json(reqwest::Method::POST, &guard_action_path(id, "enable"), Some(request)).await
    }

    /// 停用 Guard。
    pub async fn disable_guard(&self, id: &str, request: &GuardRevisionRequest) -> Result<GuardView, ClientError> {
        self.send_json(reqwest::Method::POST, &guard_action_path(id, "disable"), Some(request)).await
    }

    /// 教訓的重犯事件（新到舊）。
    pub async fn lesson_recurrences(&self, lesson_id: &str) -> Result<RecurrenceList, ClientError> {
        self.get_json(&recurrences_path(lesson_id)).await
    }

    /// 時間線（票 41）：依提交順序；指定 Task 時只有與它有關的事件。
    pub async fn timeline(&self, workspace_id: &str, task_id: Option<&str>) -> Result<TimelineResponse, ClientError> {
        // 以 URL 的表單編碼組查詢字串，Task 識別中的特殊字元才會正確編碼
        let mut url = reqwest::Url::parse("http://localhost").expect("固定的 URL 一定合法");
        url.set_path(&timeline_path(workspace_id));
        if let Some(task_id) = task_id {
            url.query_pairs_mut().append_pair("task_id", task_id);
        }
        let path = match url.query() {
            Some(query) => format!("{}?{query}", url.path()),
            None => url.path().to_string(),
        };
        self.get_json(&path).await
    }

    /// 觀測指標（票 41）：由時間線事件計算，附各帳號的服務端額度（看不到為 `None`）。
    pub async fn metrics(&self, workspace_id: &str) -> Result<MetricsResponse, ClientError> {
        self.get_json(&metrics_path(workspace_id)).await
    }

    /// 專案理解視圖（票 42）：Project Brief、Glossary、Project Map 與投影檔狀態。
    pub async fn project_understanding(&self, workspace_id: &str) -> Result<UnderstandingView, ClientError> {
        self.get_json(&understanding_path(workspace_id)).await
    }

    /// 重新比對投影檔（偵測外部編輯）後的專案理解視圖。
    pub async fn sync_understanding(&self, workspace_id: &str) -> Result<UnderstandingView, ClientError> {
        self.send_json::<(), _>(reqwest::Method::POST, &understanding_sync_path(workspace_id), None).await
    }

    /// 協調一個投影檔：以 Baton 版本覆寫，或保留外部版本。
    pub async fn reconcile_projection(
        &self,
        workspace_id: &str,
        request: &ReconcileRequest,
    ) -> Result<UnderstandingView, ClientError> {
        self.send_json(reqwest::Method::POST, &projection_reconcile_path(workspace_id), Some(request)).await
    }

    /// 流程視圖（票 43）：這個 Workspace 的 Workflow、所選 Playbook 版本、Stage 與 attempt。
    pub async fn workflows(&self, workspace_id: &str) -> Result<WorkflowList, ClientError> {
        self.get_json(&workflows_path(workspace_id)).await
    }

    /// 使用者附原因跳過目前的 Stage（裁定與安全 Gate 不能跳過）。
    pub async fn skip_stage(
        &self,
        workspace_id: &str,
        workflow_id: &str,
        request: &SkipStageRequest,
    ) -> Result<WorkflowView, ClientError> {
        self.send_json(reqwest::Method::POST, &workflow_skip_path(workspace_id, workflow_id), Some(request)).await
    }

    /// 審查視圖（票 44）：這個 Workspace 的雙軸審查、finding 與設計偏離（唯讀）。
    pub async fn reviews(&self, workspace_id: &str) -> Result<ReviewList, ClientError> {
        self.get_json(&reviews_path(workspace_id)).await
    }

    /// 驗證概況（票 45）：最新的 Verification Profile、是否已被證明可用、未覆蓋範圍、最近的要求與 Evidence Bundle。
    pub async fn verification_overview(&self, workspace_id: &str) -> Result<VerificationOverviewView, ClientError> {
        self.get_json(&workspace_verification_path(workspace_id)).await
    }

    /// 探索概況（票 47）：各探索 Task 目前的章程，以及疑點、已確認缺陷、未驗風險與它們的重現紀錄（唯讀）。
    pub async fn exploration_overview(&self, workspace_id: &str) -> Result<ExplorationOverview, ClientError> {
        self.get_json(&workspace_exploration_path(workspace_id)).await
    }

    /// 使用者發布 Verification Profile 的新版本（內容與最新版本相同時不新增）。
    pub async fn publish_verification_profile(
        &self,
        workspace_id: &str,
        definition: &ProfileDefinition,
    ) -> Result<PublishProfileResponse, ClientError> {
        self.send_json(reqwest::Method::POST, &verification_profile_path(workspace_id), Some(definition)).await
    }

    /// 使用者要求執行驗證（由 Runtime 排入，之後執行）。
    pub async fn run_verification(
        &self,
        workspace_id: &str,
        request: &RunVerificationRequest,
    ) -> Result<VerificationRequest, ClientError> {
        self.send_json(reqwest::Method::POST, &verification_runs_path(workspace_id), Some(request)).await
    }

    /// 讀取一份 Evidence Bundle。
    pub async fn evidence_bundle(&self, workspace_id: &str, bundle_id: &str) -> Result<EvidenceBundle, ClientError> {
        self.get_json(&evidence_bundle_path(workspace_id, bundle_id)).await
    }

    /// 讀取 Evidence Bundle 中第 `index` 個證據檔案的內容（票 46）。
    pub async fn evidence_artifact(
        &self,
        workspace_id: &str,
        bundle_id: &str,
        index: usize,
    ) -> Result<EvidenceArtifactContent, ClientError> {
        self.get_json(&evidence_artifact_path(workspace_id, bundle_id, index)).await
    }

    /// Playbook 與方法的全部版本。
    pub async fn playbooks(&self) -> Result<PlaybookCatalog, ClientError> {
        self.get_json(PLAYBOOKS_PATH).await
    }

    /// 回報一次沒被 Guard 攔下的重犯。
    pub async fn report_recurrence(
        &self,
        lesson_id: &str,
        request: &ReportRecurrenceRequest,
    ) -> Result<baton_domain::guard::Recurrence, ClientError> {
        self.send_json(reqwest::Method::POST, &recurrences_path(lesson_id), Some(request)).await
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
        self.send_json_within(method, path, body, REQUEST_TIMEOUT).await
    }

    /// 與 `send_json` 相同，但等待回應的上限由呼叫端決定（會去執行 claude 的端點需要較久）。
    async fn send_json_within<B: serde::Serialize, T: serde::de::DeserializeOwned>(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&B>,
        timeout: Duration,
    ) -> Result<T, ClientError> {
        let response = self.request_ok(method, path, body, timeout).await?;
        response.json().await.map_err(|e| ClientError::Unreachable(e.to_string()))
    }

    /// 送出請求（帶憑證與固定標頭）並回傳成功的回應；非 2xx 轉成 `ClientError::Rejected`。
    async fn request_ok<B: serde::Serialize>(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&B>,
        timeout: Duration,
    ) -> Result<reqwest::Response, ClientError> {
        let token = baton_credential::read_desktop_credential(&self.data_dir, &*self.protector)?;
        let unreachable = |e: reqwest::Error| ClientError::Unreachable(e.to_string());
        let mut request = self
            .http
            .request(method, format!("http://{}{path}", self.addr))
            .bearer_auth(token)
            .header(CLIENT_HEADER, CLIENT_HEADER_VALUE)
            .timeout(timeout);
        if let Some(body) = body {
            request = request.json(body);
        }
        // reqwest 的錯誤訊息只含 URL（不含憑證，憑證在標頭裡）
        let response = request.send().await.map_err(unreachable)?;

        if response.status().is_success() {
            return Ok(response);
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

/// 確保監護程序在執行（票 32，ADR-0006 M1）：開 App 時只補缺少的元件。
///
/// 依 `ensure_runtime` 回傳的健康狀態判斷：監護回報 `Active` 就不做事（回 `Ok(false)`）；
/// `Unavailable` 或舊版 Runtime 的 `Unknown` 才呼叫 `start_guardian` 補啟動監護（回 `Ok(true)`）。
/// 不會重啟已在執行的 Runtime，也不重設任何次數；已有監護時多啟動的那個會因具名 mutex 自行結束。
pub fn ensure_guardian(
    health: &HealthResponse,
    start_guardian: impl FnOnce() -> std::io::Result<()>,
) -> Result<bool, ClientError> {
    if health.guardian.state == baton_domain::guardian::GuardianState::Active {
        return Ok(false);
    }
    start_guardian().map_err(ClientError::Spawn)?;
    Ok(true)
}

/// 以脫離本程序的方式啟動 Runtime 執行檔；關閉 App 後 Runtime 仍會繼續執行。
pub fn spawn_runtime_process(exe: &Path, envs: &[(String, String)]) -> std::io::Result<std::process::Child> {
    spawn_detached(exe, envs)
}

/// 以脫離本程序的方式啟動監護執行檔（與 Runtime 相同：無視窗、關閉 App 後仍繼續執行）。
pub fn spawn_guardian_process(exe: &Path, envs: &[(String, String)]) -> std::io::Result<std::process::Child> {
    spawn_detached(exe, envs)
}

/// 啟動與 App 脫離的背景程序：不接標準輸入輸出、獨立的程序群組、不開主控台視窗。
fn spawn_detached(exe: &Path, envs: &[(String, String)]) -> std::io::Result<std::process::Child> {
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
