//! 桌面殼：啟動時連回或啟動 Runtime，並把健康狀態提供給 Web UI。

// release 版不顯示額外的主控台視窗
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use baton_api::{
    AuthorizationView, ConfirmAuthorizationRequest, CreateWorkspaceRequest, HealthResponse, PlatformResponse,
    SummaryRequest, WorkspaceList,
};
use baton_client::{ensure_runtime, spawn_runtime_process, Connection, RuntimeClient};
use baton_credential::DpapiProtector;
use baton_domain::config::{RuntimeConfig, ENV_DATA_DIR, ENV_PORT};
use baton_domain::authorization::{AuthorizationRevision, ConfirmationSummary, Grant};
use baton_domain::workspace::Workspace;
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
            workspace_authorization,
            workspace_authorization_summary,
            workspace_authorization_confirm
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
