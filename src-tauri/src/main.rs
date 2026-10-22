//! 桌面殼：啟動時連回或啟動 Runtime，並把健康狀態提供給 Web UI。

// release 版不顯示額外的主控台視窗
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use baton_api::{
    AcceptTaskRequest, AdmissionReport, ArtifactList, ArtifactPreview, AuthorizationView, ConfirmAuthorizationRequest, CreateProfileRequest, CreateTaskRequest,
    CreateWorkspaceRequest, HealthResponse, LoginTerminalResponse, PlatformResponse, ProfileList, SetupStatus,
    SchedulingOverview, SummaryRequest, TaskBoard, TaskDetail, WorkspaceList,
};
use baton_client::{ensure_runtime, spawn_runtime_process, Connection, DispatchOutcome, RuntimeClient};
use baton_credential::DpapiProtector;
use baton_domain::config::{RuntimeConfig, ENV_DATA_DIR, ENV_PORT};
use baton_domain::profile::ProfileView;
use baton_domain::authorization::{AuthorizationRevision, ConfirmationSummary, Grant};
use baton_domain::run::RunView;
use baton_domain::verification::VerificationView;
use baton_domain::scheduler::CapacityMode;
use baton_domain::task::{ContractContent, Goal, TaskView};
use baton_domain::workspace::Workspace;
use base64::Engine;
use serde::Serialize;
use tauri::{Manager, State};
use tokio::sync::OnceCell;

/// 等待 Runtime 就緒的上限。
const START_WAIT: Duration = Duration::from_secs(20);

/// Runtime 執行檔與桌面殼放在同一個資料夾。
const RUNTIME_EXE: &str = "baton-runtime.exe";

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
                ensure_runtime(
                    &self.client,
                    || spawn_runtime_process(&exe, &self.envs).map(drop),
                    START_WAIT,
                )
                .await
                .map(|(connection, _health)| connection)
                .map_err(|e| e.to_string())
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
) -> Result<AuthorizationRevision, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client
        .confirm_authorization(&id, &ConfirmAuthorizationRequest { template_id, grants, acknowledged_risk })
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
    contract: Option<ContractContent>,
) -> Result<TaskView, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client
        .create_task(&workspace_id, &CreateTaskRequest { goal_id, title, contract: contract.unwrap_or_default() })
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

/// Web UI 切換容量預設（`reserve_handoff` 或 `higher_concurrency`）。
#[tauri::command]
async fn scheduling_set_mode(link: State<'_, RuntimeLink>, mode: CapacityMode) -> Result<SchedulingOverview, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    link.client.set_scheduling_mode(mode).await.map_err(|e| e.to_string())
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
            profile_list,
            profile_create,
            profile_verify,
            profile_login_terminal,
            profile_archive,
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
            scheduling_overview,
            scheduling_set_mode,
            artifact_list,
            artifact_preview,
            artifact_download
        ])
        .setup(|app| {
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
