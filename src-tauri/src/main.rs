//! 桌面殼：啟動時連回或啟動 Runtime，並把健康狀態提供給 Web UI。
//! 關閉視窗只隱藏到系統匣（Runtime 繼續工作）；系統匣提供狀態、重新開啟與「完全退出」。

// release 版不顯示額外的主控台視窗
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use baton_api::{
    PostedMessage, AcceptTaskRequest, AdmissionReport, AnswerRequest, ArtifactList, ArtifactPreview, AuthorizationView, BackupStatus,
    BatchRequest, BatchResponse, ConfirmAuthorizationRequest, CreateProfileRequest, CreateTaskRequest,
    CreateWorkspaceRequest, HealthResponse, ImportPreview, LifecycleStatus, LoginTerminalResponse, MergeRequest,
    MergeResponse, PendingDecisions, PlatformResponse, ProfileList, SchedulingOverview, SetupStatus, SharedSettingsStatus,
    SummaryRequest, TaskBoard, TaskDetail, UpdateAuthorityRequest, WorkspaceLifecycleView, WorkspaceList,
    ConfirmKnowledgeRequest, CorrectKnowledgeRequest, CreateKnowledgeRequest, KnowledgeDetail, KnowledgeEntryView,
    KnowledgeList, KnowledgeListQuery, KnowledgeReasonRequest,
    CreateGuardRequest, GuardList, GuardRevisionRequest, GuardView, RecurrenceList, ReportRecurrenceRequest,
    RecoveryStatusView, MetricsResponse, TimelineResponse,
    ReconcileRequest, UnderstandingView,
};
use baton_client::{
    ensure_guardian, ensure_runtime, spawn_guardian_process, spawn_runtime_process, Connection, DispatchOutcome,
    RuntimeClient,
};
use baton_credential::DpapiProtector;
use baton_store::{AnswerResult, DecisionList};
use baton_domain::config::{RuntimeConfig, ENV_DATA_DIR, ENV_PORT};
use baton_domain::authority::AuthorityTable;
use baton_domain::conversation::ConversationView;
use baton_domain::decision::DecisionView;
use baton_domain::guard::Recurrence;
use baton_domain::profile::ProfileView;
use baton_domain::quota::PlanLimits;
use baton_domain::authorization::{AuthorizationRevision, ConfirmationSummary, Grant};
use baton_domain::lifecycle::LifecyclePhase;
use baton_domain::run::RunView;
use baton_domain::verification::VerificationView;
use baton_domain::scheduler::CapacityMode;
use baton_domain::shared_settings::SettingsVersion;
use baton_domain::task::{ContractContent, Goal, TaskView};
use baton_domain::workdir::TaskKind;
use baton_domain::workspace::{ModelOverrides, Workspace};
use base64::Engine;
use serde::Serialize;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, State, WindowEvent};
use tokio::sync::OnceCell;

/// 等待 Runtime 就緒的上限。
const START_WAIT: Duration = Duration::from_secs(20);

/// 主視窗的標籤（tauri.conf.json 未指定時預設為 `main`）。
const MAIN_WINDOW: &str = "main";

/// 系統匣「完全退出」通知 Web UI 的事件；Web UI 收到後呼叫 `lifecycle_exit`、顯示停止進度，最後呼叫 `app_quit`。
const FULL_EXIT_EVENT: &str = "baton://full-exit-requested";

/// 系統匣狀態文字的更新間隔。
const TRAY_POLL: Duration = Duration::from_secs(3);

/// 後備：按下「完全退出」後若這麼久 App 仍未結束（Web UI 沒反應），殼自行要求退出並結束 App。
/// Runtime 端的停止流程上限是 40 秒（spec D-11），再留 20 秒給 Web UI 顯示結果。
const FULL_EXIT_FALLBACK: Duration = Duration::from_secs(60);

/// Runtime 執行檔與桌面殼放在同一個資料夾。
const RUNTIME_EXE: &str = "baton-runtime.exe";

/// 監護執行檔與 Runtime 放在同一個資料夾（票 32）。
const GUARDIAN_EXE: &str = "baton-guardian.exe";

struct RuntimeLink {
    /// 每次呼叫都重新讀取 DPAPI 保護的桌面憑證；憑證只在殼內處理，不交給 Web UI
    client: RuntimeClient,
    data_dir: PathBuf,
    envs: Vec<(String, String)>,
    /// 啟動流程只做一次；結果（成功或失敗原因）供之後的健康查詢使用
    startup: OnceCell<Result<Connection, String>>,
}

impl RuntimeLink {
    fn from_env() -> Result<Self, String> {
        let config = RuntimeConfig::from_lookup(|key| std::env::var(key).ok()).map_err(|e| e.to_string())?;
        // 明確把解析後的設定傳給 Runtime，兩邊對資料目錄與埠的認知一致
        let envs = vec![
            (ENV_DATA_DIR.to_string(), config.data_dir.display().to_string()),
            (ENV_PORT.to_string(), config.bind.port().to_string()),
        ];
        let data_dir = config.data_dir.clone();
        let client = RuntimeClient::new(config.bind, config.data_dir, Arc::new(DpapiProtector));
        Ok(Self { client, data_dir, envs, startup: OnceCell::new() })
    }

    /// 連回既有的 Runtime，沒有就啟動一個；只會執行一次。
    async fn ensure_started(&self) -> &Result<Connection, String> {
        self.startup
            .get_or_init(|| async {
                // 資料目錄在網路共享或雲端同步資料夾時，先把原因說清楚，而不是等 Runtime 啟動逾時（spec D-17）
                baton_store::check_data_dir(&self.data_dir).map_err(|e| e.to_string())?;
                let exe = runtime_exe_path().map_err(|e| e.to_string())?;
                let (connection, health) = ensure_runtime(
                    &self.client,
                    || spawn_runtime_process(&exe, &self.envs).map(drop),
                    START_WAIT,
                )
                .await
                .map_err(|e| e.to_string())?;
                // 只補缺少的元件：監護不在才啟動（ADR-0006 M1）。補不起來只是少了自動重啟保護，
                // 工作照常、狀態列會顯示降級，所以不讓 App 因此啟動失敗
                let guardian_exe = exe.with_file_name(GUARDIAN_EXE);
                let _ = ensure_guardian(&health, || spawn_guardian_process(&guardian_exe, &self.envs).map(drop));
                Ok(connection)
            })
            .await
    }
}

fn runtime_exe_path() -> std::io::Result<PathBuf> {
    let dir = std::env::current_exe()?.parent().map(PathBuf::from).unwrap_or_default();
    Ok(dir.join(RUNTIME_EXE))
}

/// Web UI 查詢 Runtime 狀態；啟動失敗時回報啟動失敗的原因。
#[tauri::command]
async fn runtime_health(link: State<'_, RuntimeLink>) -> Result<HealthResponse, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.health().await.map_err(|e| e.to_string())
}

/// Web UI 查詢平台支援狀態；與健康查詢一樣先確保 Runtime 已啟動。
#[tauri::command]
async fn runtime_platform(link: State<'_, RuntimeLink>) -> Result<PlatformResponse, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.platform().await.map_err(|e| e.to_string())
}

/// Web UI 列出 Workspace（含已封存）。
#[tauri::command]
async fn workspace_list(link: State<'_, RuntimeLink>) -> Result<WorkspaceList, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.workspaces().await.map_err(|e| e.to_string())
}

/// Web UI 以使用者選擇的目錄建立 Workspace。
#[tauri::command]
async fn workspace_create(
    link: State<'_, RuntimeLink>,
    path: String,
    name: Option<String>,
) -> Result<Workspace, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.create_workspace(&CreateWorkspaceRequest { path, name }).await.map_err(|e| e.to_string())
}

/// Web UI 封存 Workspace。
#[tauri::command]
async fn workspace_archive(link: State<'_, RuntimeLink>, id: String) -> Result<Workspace, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.archive_workspace(&id).await.map_err(|e| e.to_string())
}

/// Web UI 設定 Workspace 依角色的模型覆寫（只影響之後派出的 Run）。
#[tauri::command]
async fn workspace_set_model_overrides(
    link: State<'_, RuntimeLink>,
    id: String,
    overrides: ModelOverrides,
) -> Result<Workspace, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.set_model_overrides(&id, &overrides).await.map_err(|e| e.to_string())
}

/// Web UI 列出帳號 Profile 與可派工判定。
#[tauri::command]
async fn profile_list(link: State<'_, RuntimeLink>) -> Result<ProfileList, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.profiles().await.map_err(|e| e.to_string())
}

/// Web UI 新增帳號 Profile（最多 8 個）。
#[tauri::command]
async fn profile_create(
    link: State<'_, RuntimeLink>,
    name: String,
    expected_account: Option<String>,
) -> Result<ProfileView, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.create_profile(&CreateProfileRequest { name, expected_account }).await.map_err(|e| e.to_string())
}

/// Web UI 重新核對 Profile 的登入狀態、認證來源與帳號身分。
#[tauri::command]
async fn profile_verify(link: State<'_, RuntimeLink>, id: String) -> Result<ProfileView, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.verify_profile(&id).await.map_err(|e| e.to_string())
}

/// Web UI 要求開啟終端機，以該 Profile 自己的設定目錄執行原生登入。
#[tauri::command]
async fn profile_login_terminal(link: State<'_, RuntimeLink>, id: String) -> Result<LoginTerminalResponse, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.open_login_terminal(&id).await.map_err(|e| e.to_string())
}

/// Web UI 封存 Profile（釋出名額，不刪除它的登入資料目錄）。
#[tauri::command]
async fn profile_archive(link: State<'_, RuntimeLink>, id: String) -> Result<ProfileView, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.archive_profile(&id).await.map_err(|e| e.to_string())
}

/// Web UI 取得共同設定的目前狀態（已採用的版本）。
#[tauri::command]
async fn shared_settings_status(link: State<'_, RuntimeLink>) -> Result<SharedSettingsStatus, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.shared_settings().await.map_err(|e| e.to_string())
}

/// Web UI 預覽匯入使用者既有的 Claude Code 設定（不保存、不改寫來源）。
#[tauri::command]
async fn shared_settings_preview(link: State<'_, RuntimeLink>) -> Result<ImportPreview, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.preview_settings_import().await.map_err(|e| e.to_string())
}

/// Web UI 採用預覽的內容成為新版本；`expected_hash` 為預覽回傳的候選雜湊。
#[tauri::command]
async fn shared_settings_adopt(link: State<'_, RuntimeLink>, expected_hash: String) -> Result<SettingsVersion, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.adopt_settings(&expected_hash).await.map_err(|e| e.to_string())
}

/// Web UI 讀取單一共同設定版本。
#[tauri::command]
async fn shared_settings_version(link: State<'_, RuntimeLink>, version: i64) -> Result<SettingsVersion, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.settings_version(version).await.map_err(|e| e.to_string())
}

/// Web UI 取得首次啟動精靈的狀態。
#[tauri::command]
async fn setup_status(link: State<'_, RuntimeLink>) -> Result<SetupStatus, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.setup().await.map_err(|e| e.to_string())
}

/// Web UI 的「重新檢查」。
#[tauri::command]
async fn setup_recheck(link: State<'_, RuntimeLink>) -> Result<SetupStatus, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.recheck_setup().await.map_err(|e| e.to_string())
}

/// Web UI 確認已閱讀背景執行行為的告知。
#[tauri::command]
async fn setup_acknowledge_notice(link: State<'_, RuntimeLink>) -> Result<SetupStatus, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.acknowledge_background_notice().await.map_err(|e| e.to_string())
    }

/// Web UI 讀取 Workspace 的授權設定（目前授權、歷史、建議範本）。
#[tauri::command]
async fn workspace_authorization(link: State<'_, RuntimeLink>, id: String) -> Result<AuthorizationView, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.authorization(&id).await.map_err(|e| e.to_string())
}

/// Web UI 取得確認前的摘要（預覽，不保存）。
#[tauri::command]
async fn workspace_authorization_summary(
    link: State<'_, RuntimeLink>,
    id: String,
    grants: Vec<Grant>,
) -> Result<ConfirmationSummary, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.authorization_summary(&id, &SummaryRequest { grants }).await.map_err(|e| e.to_string())
}

/// Web UI 確認並保存授權（產生新的 revision）。
#[tauri::command]
async fn workspace_authorization_confirm(
    link: State<'_, RuntimeLink>,
    id: String,
    template_id: Option<String>,
    grants: Vec<Grant>,
    acknowledged_risk: bool,
    acknowledged_expansion: Option<bool>,
) -> Result<AuthorizationRevision, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client
        .confirm_authorization(&id, &ConfirmAuthorizationRequest {
            template_id,
            grants,
            acknowledged_risk,
            acknowledged_expansion: acknowledged_expansion.unwrap_or(false),
        })
        .await
        .map_err(|e| e.to_string())
}

/// Web UI 讀取工作面板：Workspace 內全部 Goal 與 Task。
#[tauri::command]
async fn task_board(
    link: State<'_, RuntimeLink>,
    workspace_id: String,
) -> Result<TaskBoard, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.task_board(&workspace_id).await.map_err(|e| e.to_string())
}

/// Web UI 建立 Goal。
#[tauri::command]
async fn goal_create(
    link: State<'_, RuntimeLink>,
    workspace_id: String,
    title: String,
) -> Result<Goal, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.create_goal(&workspace_id, &title).await.map_err(|e| e.to_string())
}

/// Web UI 手動建立 Task（可附部分契約，契約版本從 1 開始）。
#[tauri::command]
async fn task_create(
    link: State<'_, RuntimeLink>,
    workspace_id: String,
    goal_id: String,
    title: String,
    kind: Option<TaskKind>,
    contract: Option<ContractContent>,
) -> Result<TaskView, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client
        .create_task(&workspace_id, &CreateTaskRequest { goal_id, title, kind: kind.unwrap_or_default(), contract: contract.unwrap_or_default() })
        .await.map_err(|e| e.to_string())
}

/// Web UI 讀取單一 Task 與契約版本歷史。
#[tauri::command]
async fn task_detail(
    link: State<'_, RuntimeLink>,
    workspace_id: String,
    task_id: String,
) -> Result<TaskDetail, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.task_detail(&workspace_id, &task_id).await.map_err(|e| e.to_string())
}

/// Web UI 變更契約（整份取代，內容相同不新增版本）。
#[tauri::command]
async fn task_contract_update(
    link: State<'_, RuntimeLink>,
    workspace_id: String,
    task_id: String,
    contract: ContractContent,
    note: Option<String>,
) -> Result<TaskView, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.update_task_contract(&workspace_id, &task_id, contract, note).await.map_err(|e| e.to_string())
}

/// Web UI 新增相依（循環與自我相依會被拒絕）。
#[tauri::command]
async fn task_dependency_add(
    link: State<'_, RuntimeLink>,
    workspace_id: String,
    task_id: String,
    depends_on: String,
) -> Result<TaskView, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.add_task_dependency(&workspace_id, &task_id, &depends_on).await.map_err(|e| e.to_string())
}

/// Web UI 預覽准入檢查（唯讀）。
#[tauri::command]
async fn task_admission_check(
    link: State<'_, RuntimeLink>,
    workspace_id: String,
    task_id: String,
) -> Result<AdmissionReport, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.check_task_admission(&workspace_id, &task_id).await.map_err(|e| e.to_string())
}

/// Web UI 標記 Task 為 ready；准入檢查未通過時回報缺項。
#[tauri::command]
async fn task_ready(
    link: State<'_, RuntimeLink>,
    workspace_id: String,
    task_id: String,
) -> Result<TaskView, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.mark_task_ready(&workspace_id, &task_id).await.map_err(|e| e.to_string())
}

/// Web UI 取消 Task。
#[tauri::command]
async fn task_cancel(
    link: State<'_, RuntimeLink>,
    workspace_id: String,
    task_id: String,
) -> Result<TaskView, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.cancel_task(&workspace_id, &task_id).await.map_err(|e| e.to_string())
}

/// Web UI 對 ready 的 Task 派工；有名額回傳 queued 的 Run（程序由 Supervisor 之後才啟動），否則回傳排隊中與原因。
#[tauri::command]
async fn task_dispatch(
    link: State<'_, RuntimeLink>,
    workspace_id: String,
    task_id: String,
) -> Result<DispatchOutcome, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.dispatch_task(&workspace_id, &task_id).await.map_err(|e| e.to_string())
}

/// Web UI 檢視一個 Task 的驗證狀況（接受基礎、檢查、成果 manifest 與驗證紀錄）。
#[tauri::command]
async fn task_verification(
    link: State<'_, RuntimeLink>,
    workspace_id: String,
    task_id: String,
) -> Result<VerificationView, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.task_verification(&workspace_id, &task_id).await.map_err(|e| e.to_string())
}

/// Web UI 手動接受成果；`request.basis` 必須是檢視時拿到的接受基礎。
#[tauri::command]
async fn task_accept(
    link: State<'_, RuntimeLink>,
    workspace_id: String,
    task_id: String,
    request: AcceptTaskRequest,
) -> Result<VerificationView, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.accept_task(&workspace_id, &task_id, &request).await.map_err(|e| e.to_string())
}

/// Web UI 讀取排程總覽：容量、各帳號進行中的 Run、佇列與等待原因。
#[tauri::command]
async fn scheduling_overview(link: State<'_, RuntimeLink>) -> Result<SchedulingOverview, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.scheduling().await.map_err(|e| e.to_string())
}

/// Web UI 讀取設定頁的備份狀態：現有備份與最新一次還原演練（票 40）。
#[tauri::command]
async fn backup_status(link: State<'_, RuntimeLink>) -> Result<BackupStatus, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.backups().await.map_err(|e| e.to_string())
}

/// Web UI 切換容量預設（`reserve_handoff` 或 `higher_concurrency`）。
#[tauri::command]
async fn scheduling_set_mode(link: State<'_, RuntimeLink>, mode: CapacityMode) -> Result<SchedulingOverview, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.set_scheduling_mode(mode).await.map_err(|e| e.to_string())
}

/// Web UI 設定一個 Profile 的方案上限（推估的分母），回傳新的排程總覽。
#[tauri::command]
async fn profile_set_quota_limits(
    link: State<'_, RuntimeLink>,
    id: String,
    limits: PlanLimits,
) -> Result<SchedulingOverview, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.set_quota_limits(&id, &limits).await.map_err(|e| e.to_string())
}

/// Web UI 要求取消 Run；已啟動的 Run 要等程序確認停止才會變 cancelled。
#[tauri::command]
async fn run_cancel(
    link: State<'_, RuntimeLink>,
    workspace_id: String,
    run_id: String,
) -> Result<RunView, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.cancel_run(&workspace_id, &run_id).await.map_err(|e| e.to_string())
}

/// Web UI 讀取「對話」分頁：Workspace 與 Controller 的長期對話（票 16）。
#[tauri::command]
async fn conversation_view(link: State<'_, RuntimeLink>, workspace_id: String) -> Result<ConversationView, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.conversation(&workspace_id).await.map_err(|e| e.to_string())
}

/// Web UI 送出一則使用者訊息；Controller 忙碌時保存為「已收到／待處理」。
#[tauri::command]
async fn conversation_send_message(
    link: State<'_, RuntimeLink>,
    workspace_id: String,
    text: String,
) -> Result<PostedMessage, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.send_conversation_message(&workspace_id, &text).await.map_err(|e| e.to_string())
}

/// Web UI 讀取裁定中心：Workspace 內的裁定請求與待處理數。
#[tauri::command]
async fn decision_list(link: State<'_, RuntimeLink>, workspace_id: String) -> Result<DecisionList, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.decision_list(&workspace_id).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn decision_detail(
    link: State<'_, RuntimeLink>,
    workspace_id: String,
    request_id: String,
) -> Result<DecisionView, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.decision_detail(&workspace_id, &request_id).await.map_err(|e| e.to_string())
}

/// Web UI 處理一個裁定請求（只有桌面角色能做）。
#[tauri::command]
async fn decision_answer(
    link: State<'_, RuntimeLink>,
    workspace_id: String,
    request_id: String,
    request: AnswerRequest,
) -> Result<AnswerResult, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.decision_answer(&workspace_id, &request_id, &request).await.map_err(|e| e.to_string())
}

/// Web UI 批次處理裁定請求：每一項各自保存。
#[tauri::command]
async fn decisions_batch(link: State<'_, RuntimeLink>, request: BatchRequest) -> Result<BatchResponse, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.decisions_batch(&request).await.map_err(|e| e.to_string())
}

/// Web UI 把一個裁定請求合併到另一個。
#[tauri::command]
async fn decision_merge(
    link: State<'_, RuntimeLink>,
    workspace_id: String,
    request_id: String,
    request: MergeRequest,
) -> Result<MergeResponse, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.decision_merge(&workspace_id, &request_id, &request).await.map_err(|e| e.to_string())
}

/// Web UI 把兩個裁定請求互相連結。
#[tauri::command]
async fn decision_link(
    link: State<'_, RuntimeLink>,
    workspace_id: String,
    request_id: String,
    other_id: String,
) -> Result<DecisionView, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.decision_link(&workspace_id, &request_id, &other_id).await.map_err(|e| e.to_string())
}

/// Web UI 讀取 Workspace 的決策權限表。
#[tauri::command]
async fn decision_authority(link: State<'_, RuntimeLink>, workspace_id: String) -> Result<AuthorityTable, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.decision_authority(&workspace_id).await.map_err(|e| e.to_string())
}

/// Web UI 收窄 Workspace 的決策權限表。
#[tauri::command]
async fn decision_authority_update(
    link: State<'_, RuntimeLink>,
    workspace_id: String,
    request: UpdateAuthorityRequest,
) -> Result<AuthorityTable, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.update_decision_authority(&workspace_id, &request).await.map_err(|e| e.to_string())
}

/// 狀態列的全域待處理裁定數。
#[tauri::command]
async fn decisions_pending(link: State<'_, RuntimeLink>) -> Result<PendingDecisions, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.pending_decisions().await.map_err(|e| e.to_string())
}

/// Web UI 查看某個 Workspace 的成果（含全部版本與未登記的檔案）。
#[tauri::command]
async fn artifact_list(link: State<'_, RuntimeLink>, workspace_id: String) -> Result<ArtifactList, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.list_artifacts(&workspace_id).await.map_err(|e| e.to_string())
}

/// Web UI 預覽成果的某個版本（Runtime 會先核對雜湊）。
#[tauri::command]
async fn artifact_preview(
    link: State<'_, RuntimeLink>,
    workspace_id: String,
    artifact_id: String,
    version: i64,
) -> Result<ArtifactPreview, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.preview_artifact(&workspace_id, &artifact_id, version).await.map_err(|e| e.to_string())
}

/// 下載成果的內容：Tauri 指令的回傳值是 JSON，所以內容以 base64 交給 Web UI。
#[derive(Serialize)]
struct ArtifactDownloadPayload {
    sha256: String,
    content_base64: String,
}

/// Web UI 下載成果的某個版本；殼帶桌面憑證取回，內容與 manifest 不符時 Runtime 會拒絕。
#[tauri::command]
async fn artifact_download(
    link: State<'_, RuntimeLink>,
    workspace_id: String,
    artifact_id: String,
    version: i64,
) -> Result<ArtifactDownloadPayload, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    let download =
        link.client.download_artifact(&workspace_id, &artifact_id, version).await.map_err(|e| e.to_string())?;
    Ok(ArtifactDownloadPayload {
        sha256: download.sha256,
        content_base64: base64::engine::general_purpose::STANDARD.encode(download.bytes),
    })
}

/// Web UI 讀取生命週期總覽：Runtime 階段、完全退出的停止進度與異常、各 Workspace 的自動續跑設定。
#[tauri::command]
async fn lifecycle_status(link: State<'_, RuntimeLink>) -> Result<LifecycleStatus, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.lifecycle().await.map_err(|e| e.to_string())
}

/// Web UI 要求完全退出：Runtime 持久化停止意圖後立即回應，停止進度之後以 `lifecycle_status` 查詢。
#[tauri::command]
async fn lifecycle_exit(link: State<'_, RuntimeLink>) -> Result<LifecycleStatus, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.request_exit().await.map_err(|e| e.to_string())
}

/// Web UI 讀取崩潰恢復總覽：目前的恢復事件、本輪次數、退避期限、受影響工作與派工阻擋原因（票 31）。
#[tauri::command]
async fn recovery_status(link: State<'_, RuntimeLink>) -> Result<RecoveryStatusView, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.recovery().await.map_err(|e| e.to_string())
}

/// Web UI 的「重新嘗試恢復」：建立新輪次並保留歷史；只在恢復額度用盡時被接受。
#[tauri::command]
async fn recovery_retry(link: State<'_, RuntimeLink>) -> Result<RecoveryStatusView, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.retry_recovery().await.map_err(|e| e.to_string())
}

/// Web UI 變更 Workspace 的「完全退出後自動續跑」（只影響下次重新開啟）。
#[tauri::command]
async fn workspace_lifecycle_update(
    link: State<'_, RuntimeLink>,
    workspace_id: String,
    auto_resume_after_exit: bool,
) -> Result<WorkspaceLifecycleView, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.set_workspace_auto_resume(&workspace_id, auto_resume_after_exit).await.map_err(|e| e.to_string())
}

/// Web UI 的「繼續工作」：解除 Workspace 在完全退出後的暫停。
#[tauri::command]
async fn workspace_resume(
    link: State<'_, RuntimeLink>,
    workspace_id: String,
) -> Result<WorkspaceLifecycleView, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.resume_workspace(&workspace_id).await.map_err(|e| e.to_string())
}

/// 完全退出流程的最後一步：真正結束 App（關閉視窗只會隱藏到系統匣）。
#[tauri::command]
fn app_quit(app: AppHandle) {
    app.exit(0);
}

/// 顯示並聚焦主視窗（從系統匣重新開啟）。
fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// 系統匣的「完全退出」：打開主視窗讓 Web UI 接手停止流程並顯示進度。
fn request_full_exit(app: &AppHandle) {
    show_main_window(app);
    let _ = app.emit_to(MAIN_WINDOW, FULL_EXIT_EVENT, ());
    // 後備：Web UI 沒反應時也不能無限等。使用者在系統匣按下「完全退出」本身就是停止意圖，
    // 所以逾時後殼自己送出完全退出（Runtime 端是冪等的，Web UI 已送過也無妨）再結束 App。
    // Runtime 的停止流程與 App 是否在世無關，結束 App 不會中斷它。
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FULL_EXIT_FALLBACK).await;
        let _ = handle.state::<RuntimeLink>().client.request_exit().await;
        handle.exit(0);
    });
}

/// 系統匣狀態項目的文字。
fn tray_status_text(status: Result<LifecycleStatus, String>) -> String {
    match status {
        Ok(status) => match status.phase {
            LifecyclePhase::Running => format!("Runtime 運作中（進行中 Run：{}）", status.active_runs),
            LifecyclePhase::Stopping => format!("完全退出中…（進行中 Run：{}）", status.active_runs),
            LifecyclePhase::Stopped => "Runtime 已停止".to_string(),
        },
        Err(_) => "Runtime 無法連線".to_string(),
    }
}

/// 建立系統匣：狀態（停用，只顯示）、開啟 Baton、完全退出；左鍵點圖示等同開啟 Baton。
fn setup_tray(app: &tauri::App) -> tauri::Result<()> {
    let status = MenuItem::with_id(app, "status", "Runtime 連線中…", false, None::<&str>)?;
    let open = MenuItem::with_id(app, "open", "開啟 Baton", true, None::<&str>)?;
    let full_exit = MenuItem::with_id(app, "full_exit", "完全退出", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&status, &separator, &open, &full_exit])?;

    let mut tray = TrayIconBuilder::with_id("main")
        .tooltip("Baton")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => show_main_window(app),
            "full_exit" => request_full_exit(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                show_main_window(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;

    // 背景迴圈定期更新狀態文字；先等啟動流程結束，避免啟動途中誤報無法連線
    let handle = app.handle().clone();
    tauri::async_runtime::spawn(async move {
        let link = handle.state::<RuntimeLink>();
        loop {
            let result = match link.ensure_started().await {
                Ok(_) => link.client.lifecycle().await.map_err(|e| e.to_string()),
                Err(reason) => Err(reason.clone()),
            };
            let _ = status.set_text(tray_status_text(result));
            tokio::time::sleep(TRAY_POLL).await;
        }
    });
    Ok(())
}

/// Web UI 知識分頁的列表或搜尋（`include_inactive` 為 false 時只列有效條目）。
#[tauri::command]
async fn knowledge_list(
    link: State<'_, RuntimeLink>,
    workspace_id: Option<String>,
    q: Option<String>,
    include_inactive: bool,
) -> Result<KnowledgeList, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    let query = KnowledgeListQuery { workspace_id, q, include_inactive };
    link.client.list_knowledge(&query).await.map_err(|e| e.to_string())
}

/// Web UI 建立知識條目（直接是已驗證）。
#[tauri::command]
async fn knowledge_create(
    link: State<'_, RuntimeLink>,
    request: CreateKnowledgeRequest,
) -> Result<KnowledgeEntryView, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.create_knowledge(&request).await.map_err(|e| e.to_string())
}

/// Web UI 查看條目的版本與變更歷史。
#[tauri::command]
async fn knowledge_detail(link: State<'_, RuntimeLink>, id: String) -> Result<KnowledgeDetail, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.knowledge_detail(&id).await.map_err(|e| e.to_string())
}

/// Web UI 確認候選教訓。
#[tauri::command]
async fn knowledge_confirm(
    link: State<'_, RuntimeLink>,
    id: String,
    request: ConfirmKnowledgeRequest,
) -> Result<KnowledgeEntryView, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.confirm_knowledge(&id, &request).await.map_err(|e| e.to_string())
}

/// Web UI 修正條目內容（追加新版本）。
#[tauri::command]
async fn knowledge_correct(
    link: State<'_, RuntimeLink>,
    id: String,
    request: CorrectKnowledgeRequest,
) -> Result<KnowledgeEntryView, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.correct_knowledge(&id, &request).await.map_err(|e| e.to_string())
}

/// Web UI 撤銷條目。
#[tauri::command]
async fn knowledge_revoke(
    link: State<'_, RuntimeLink>,
    id: String,
    request: KnowledgeReasonRequest,
) -> Result<KnowledgeEntryView, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.revoke_knowledge(&id, &request).await.map_err(|e| e.to_string())
}

/// Web UI 淘汰條目。
#[tauri::command]
async fn knowledge_retire(
    link: State<'_, RuntimeLink>,
    id: String,
    request: KnowledgeReasonRequest,
) -> Result<KnowledgeEntryView, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.retire_knowledge(&id, &request).await.map_err(|e| e.to_string())
}

/// Web UI 知識分頁的 Guard 列表（票 38）。
#[tauri::command]
async fn guard_list(link: State<'_, RuntimeLink>, workspace_id: Option<String>) -> Result<GuardList, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.list_guards(workspace_id.as_deref()).await.map_err(|e| e.to_string())
}

/// Web UI 把教訓轉成 Guard（建立即啟用）。
#[tauri::command]
async fn guard_create(link: State<'_, RuntimeLink>, request: CreateGuardRequest) -> Result<GuardView, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.create_guard(&request).await.map_err(|e| e.to_string())
}

/// Web UI 啟用 Guard。
#[tauri::command]
async fn guard_enable(link: State<'_, RuntimeLink>, id: String, expected_revision: i64) -> Result<GuardView, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.enable_guard(&id, &GuardRevisionRequest { expected_revision }).await.map_err(|e| e.to_string())
}

/// Web UI 停用 Guard。
#[tauri::command]
async fn guard_disable(link: State<'_, RuntimeLink>, id: String, expected_revision: i64) -> Result<GuardView, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.disable_guard(&id, &GuardRevisionRequest { expected_revision }).await.map_err(|e| e.to_string())
}

/// Web UI 查看教訓的重犯事件。
#[tauri::command]
async fn knowledge_recurrences(link: State<'_, RuntimeLink>, id: String) -> Result<RecurrenceList, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.lesson_recurrences(&id).await.map_err(|e| e.to_string())
}

/// Web UI 的時間線分頁：Workspace 的事件時間線，可依 Task 過濾。
#[tauri::command]
async fn workspace_timeline(
    link: State<'_, RuntimeLink>,
    workspace_id: String,
    task_id: Option<String>,
) -> Result<TimelineResponse, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.timeline(&workspace_id, task_id.as_deref()).await.map_err(|e| e.to_string())
}

/// Web UI 的時間線分頁：由事件計算的觀測指標與服務端額度。
#[tauri::command]
async fn workspace_metrics(link: State<'_, RuntimeLink>, workspace_id: String) -> Result<MetricsResponse, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.metrics(&workspace_id).await.map_err(|e| e.to_string())
}

/// Web UI 的專案理解分頁：Project Brief、Glossary、Project Map 與投影檔狀態。
#[tauri::command]
async fn workspace_understanding(link: State<'_, RuntimeLink>, workspace_id: String) -> Result<UnderstandingView, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.project_understanding(&workspace_id).await.map_err(|e| e.to_string())
}

/// Web UI 的專案理解分頁：重新比對投影檔（偵測外部編輯）。
#[tauri::command]
async fn workspace_understanding_sync(link: State<'_, RuntimeLink>, workspace_id: String) -> Result<UnderstandingView, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.sync_understanding(&workspace_id).await.map_err(|e| e.to_string())
}

/// Web UI 的專案理解分頁：協調一個投影檔（覆寫或保留外部版本）。
#[tauri::command]
async fn workspace_projection_reconcile(
    link: State<'_, RuntimeLink>,
    workspace_id: String,
    request: ReconcileRequest,
) -> Result<UnderstandingView, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.reconcile_projection(&workspace_id, &request).await.map_err(|e| e.to_string())
}

/// Web UI 回報一次沒被 Guard 攔下的重犯。
#[tauri::command]
async fn knowledge_report_recurrence(
    link: State<'_, RuntimeLink>,
    id: String,
    request: ReportRecurrenceRequest,
) -> Result<Recurrence, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.report_recurrence(&id, &request).await.map_err(|e| e.to_string())
}

fn main() {
    let link = RuntimeLink::from_env().expect("無法讀取 Runtime 設定");
    tauri::Builder::default()
        .manage(link)
        .invoke_handler(tauri::generate_handler![
            runtime_health,
            runtime_platform,
            workspace_list,
            workspace_create,
            workspace_archive,
            workspace_set_model_overrides,
            profile_list,
            profile_create,
            profile_verify,
            profile_login_terminal,
            profile_archive,
            shared_settings_status,
            shared_settings_preview,
            shared_settings_adopt,
            shared_settings_version,
            setup_status,
            setup_recheck,
            setup_acknowledge_notice,
            workspace_authorization,
            workspace_authorization_summary,
            workspace_authorization_confirm,
            task_board,
            goal_create,
            task_create,
            task_detail,
            task_contract_update,
            task_dependency_add,
            task_admission_check,
            task_ready,
            task_cancel,
            task_dispatch,
            task_verification,
            task_accept,
            run_cancel,
            conversation_view,
            conversation_send_message,
            scheduling_overview,
            scheduling_set_mode,
            backup_status,
            decision_list,
            decision_detail,
            decision_answer,
            decisions_batch,
            decision_merge,
            decision_link,
            decision_authority,
            decision_authority_update,
            decisions_pending,
            artifact_list,
            artifact_preview,
            artifact_download,
            lifecycle_status,
            lifecycle_exit,
            workspace_lifecycle_update,
            workspace_resume,
            recovery_status,
            recovery_retry,
            app_quit,
            profile_set_quota_limits,
            knowledge_list,
            knowledge_create,
            knowledge_detail,
            knowledge_confirm,
            knowledge_correct,
            knowledge_revoke,
            knowledge_retire,
            guard_list,
            guard_create,
            guard_enable,
            guard_disable,
            knowledge_recurrences,
            knowledge_report_recurrence,
            workspace_timeline,
            workspace_metrics,
            workspace_understanding,
            workspace_understanding_sync,
            workspace_projection_reconcile
        ])
        // 關閉主視窗只隱藏到系統匣：Runtime 是獨立程序，工作繼續；只有完全退出流程的 `app_quit` 才結束 App
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == MAIN_WINDOW {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .setup(|app| {
            setup_tray(app)?;
            // 不等 Web UI 來問：App 一開啟就連回或啟動 Runtime
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let link = handle.state::<RuntimeLink>();
                let _ = link.ensure_started().await;
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("桌面 App 執行失敗");
}
