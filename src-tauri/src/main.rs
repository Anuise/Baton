//! 桌面殼：啟動時連回或啟動 Runtime，並把健康狀態提供給 Web UI。

// release 版不顯示額外的主控台視窗
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use baton_api::{HealthResponse, PlatformResponse};
use baton_client::{ensure_runtime, fetch_health, fetch_platform, spawn_runtime_process, Connection};
use baton_domain::config::{RuntimeConfig, ENV_DATA_DIR, ENV_PORT};
use tauri::{Manager, State};
use tokio::sync::OnceCell;

/// 等待 Runtime 就緒的上限。
const START_WAIT: Duration = Duration::from_secs(20);

/// Runtime 執行檔與桌面殼放在同一個資料夾。
const RUNTIME_EXE: &str = "baton-runtime.exe";

struct RuntimeLink {
    addr: SocketAddr,
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
        Ok(Self { addr: config.bind, envs, startup: OnceCell::new() })
    }

    /// 連回既有的 Runtime，沒有就啟動一個；只會執行一次。
    async fn ensure_started(&self) -> &Result<Connection, String> {
        self.startup
            .get_or_init(|| async {
                let exe = runtime_exe_path().map_err(|e| e.to_string())?;
                ensure_runtime(
                    self.addr,
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
    fetch_health(link.addr).await.map_err(|e| e.to_string())
}

/// Web UI 查詢平台支援狀態；與健康查詢一樣先確保 Runtime 已啟動。
#[tauri::command]
async fn runtime_platform(link: State<'_, RuntimeLink>) -> Result<PlatformResponse, String> {
    if let Err(reason) = link.ensure_started().await {
        return Err(reason.clone());
    }
    fetch_platform(link.addr).await.map_err(|e| e.to_string())
}

fn main() {
    let link = RuntimeLink::from_env().expect("無法讀取 Runtime 設定");
    tauri::Builder::default()
        .manage(link)
        .invoke_handler(tauri::generate_handler![runtime_health, runtime_platform])
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
