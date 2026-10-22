//! Runtime：唯一控制平面。
//!
//! `Runtime::start` 以程序內方式啟動完整 Runtime（真實 Store、真實 HTTP API），
//! 所有外部副作用都經由注入的 `Ports`。正式執行檔與測試 harness 共用這個入口。

pub mod backup;
pub mod claude_adapter;
pub mod claude_cli;
pub mod credentials;
pub mod log;
mod maintenance;
pub mod production_ports;
pub mod single_instance;
pub mod supervisor;
pub mod update_source;
pub mod updates;
pub mod verifier;
pub mod windows_platform;
#[cfg(windows)]
pub mod windows_login;
#[cfg(windows)]
pub mod windows_process;

use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use baton_api::{ApiState, LogSink, RunSettings};
use baton_domain::lifecycle::{self, ExitTiming, FileStopView};
use baton_domain::login::LoginBoundary;
use baton_domain::recovery::{RecoveryStatus, MAX_ATTEMPTS};
use baton_ports::Ports;
use baton_store::{Store, StoreError, StoreOptions};
use tokio::net::TcpListener;
use tokio::sync::{watch, Notify};
use tokio::task::JoinHandle;

pub use baton_domain::config::{self, ConfigError, RuntimeConfig};
pub use backup::BackupScheduler;
pub use credentials::CredentialIssuer;
pub use supervisor::{LifecycleSettings, Supervisor};
pub use updates::UpdateService;

/// 隨機埠（埠 0）時，IPv6 同埠已被占用就重新挑一個的次數上限。
const BIND_ATTEMPTS: usize = 16;

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("無法監聽 {0}：{1}")]
    Bind(SocketAddr, std::io::Error),
    #[error("監聽位址 {0} 不是 loopback；Runtime 只監聽本機")]
    NotLoopback(SocketAddr),
    #[error("無法取得系統亂數：{0}")]
    Entropy(String),
    #[error("無法交付桌面憑證：{0}")]
    CredentialFile(#[from] baton_credential::CredentialFileError),
    #[error("無法開啟日誌：{0}")]
    Log(std::io::Error),
    /// 監護或登入啟動入口拉起的 Runtime 發現停止意圖仍生效（完全退出優先，票 32、33）：不提供服務、直接結束，
    /// 等使用者手動開 App。
    #[error("停止意圖仍生效（完全退出），自動啟動的 Runtime 不提供服務")]
    SuppressedByStopIntent,
}

/// 啟動 Runtime 的選項；預設值就是正式執行的行為。
#[derive(Debug, Clone)]
pub struct RuntimeOptions {
    /// 是否啟動 Supervisor 的背景迴圈。測試 harness 關閉它，改由測試在想要的時刻呼叫 `Supervisor::tick`，
    /// 才能用 fake Clock 決定性地重現任何時序。
    pub background_supervisor: bool,
    /// 是否啟動備份排程的背景迴圈（票 40）。測試 harness 關閉它，改由測試呼叫 `BackupScheduler::tick`。
    pub background_backup: bool,
    /// 要求 Run 的程序停止時給它的寬限期（spec D-11 的 30 秒）；逾時強制終止整個程序樹。
    pub stop_grace: Duration,
    /// 完全退出的寬限期與強制終止確認上限（spec D-11 的 30 + 10 秒）；測試可縮短。
    pub exit_timing: ExitTiming,
    /// 完全退出的停止流程結束後，再提供多久 API（讓 App 讀到最終的停止結果）才結束服務；
    /// `None` 表示不自行結束（測試 harness 用，由測試決定何時丟棄 Runtime）。
    pub exit_linger: Option<Duration>,
    /// 是否嘗試使用全文檢索（spec D-26）。關閉時視同 FTS5 不可用：知識搜尋退回 LIKE 並標示降級（測試降級情境用）。
    pub full_text_search: bool,
    /// 監護拉起這個 Runtime 時交付的嘗試識別（票 32；環境變數 `BATON_GUARDIAN_ATTEMPT`）。
    /// 有值時這個 Runtime 接續監護已計次的那次嘗試；停止意圖仍生效則不提供服務。使用者或 App 啟動時為 `None`。
    pub guardian_attempt: Option<String>,
    /// 由 Windows 登入時的啟動入口拉起（票 33；環境變數 `BATON_START_SOURCE=login`）。
    /// 與監護拉起的相同：停止意圖仍生效則不提供服務（完全退出優先），不會改依跨登入設定續跑。
    pub login_start: bool,
    /// 這個 Runtime 的版本（票 34）：更新檢查與維護結果以它比較。測試以它模擬「安裝程式換上了新版本」。
    pub app_version: String,
    /// 是否啟動更新檢查的背景迴圈（票 34；啟動時與每日一次）。測試 harness 關閉它，改由測試呼叫 `UpdateService::tick`。
    pub background_update_check: bool,
}

impl Default for RuntimeOptions {
    fn default() -> Self {
        Self {
            background_supervisor: true,
            background_backup: true,
            stop_grace: supervisor::DEFAULT_STOP_GRACE,
            exit_timing: ExitTiming::default(),
            exit_linger: Some(DEFAULT_EXIT_LINGER),
            full_text_search: true,
            guardian_attempt: None,
            login_start: false,
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            background_update_check: true,
        }
    }
}

/// 停止流程結束後繼續提供 API 的時間（App 每 0.5 秒輪詢一次停止進度）。
const DEFAULT_EXIT_LINGER: Duration = Duration::from_secs(5);

pub struct Runtime;

impl Runtime {
    /// 啟動 Runtime：開啟資料庫、輪替桌面憑證並交付給 App、開始提供 HTTP API。
    ///
    /// 桌面憑證在開始監聽前就已換新並寫入保護檔，所以 App 一旦連得上，讀到的就是目前有效的憑證。
    /// 任何一步失敗都讓啟動失敗，不會退回免驗證的模式。
    pub async fn start(config: RuntimeConfig, ports: Ports) -> Result<RunningRuntime, RuntimeError> {
        Self::start_with(config, ports, RuntimeOptions::default()).await
    }

    pub async fn start_with(
        config: RuntimeConfig,
        ports: Ports,
        options: RuntimeOptions,
    ) -> Result<RunningRuntime, RuntimeError> {
        if !config.bind.ip().is_loopback() {
            return Err(RuntimeError::NotLoopback(config.bind));
        }
        let started_at = ports.clock.now();
        let store_options = StoreOptions { full_text_search: options.full_text_search };
        let store = Store::open_with(&config.data_dir, started_at, store_options).await?;
        let boot_count = store.record_boot(started_at).await?;
        // 這一次啟動是否跨了 Windows 登入（票 33）：與上一次啟動記下的登入識別比較
        let login_boundary = store.note_login_session(ports.login_session.current_login()).await?;
        // 前一次啟動是否正常結束（票 31）
        let previous_unclean = store.previous_boot_ended_cleanly().await? == Some(false);
        let log = log::open_log(&config.data_dir, ports.clock.clone()).map_err(RuntimeError::Log)?;

        let credentials = CredentialIssuer::new(store.clone(), ports.clock.clone());
        let desktop_token = credentials.rotate_desktop().await?;
        baton_credential::write_desktop_credential(&config.data_dir, &*ports.secret_protector, &desktop_token)?;

        let (v4, v6) = bind_loopback(config.bind).await?;
        let addr = v4.local_addr().map_err(|e| RuntimeError::Bind(config.bind, e))?;
        let addr_v6 = v6.as_ref().and_then(|l| l.local_addr().ok());
        (log)(&format!("Runtime 啟動：第 {boot_count} 次，監聽 {addr}{}，桌面憑證已輪替", {
            addr_v6.map(|a| format!(" 與 {a}")).unwrap_or_default()
        }));
        if store.search_mode() == baton_domain::knowledge::SearchMode::LikeFallback {
            (log)("全文檢索（FTS5 + trigram）無法使用，知識搜尋已降級為逐筆 LIKE 比對");
        }

        let reopen_from = reconcile_stop_intent(&store, &config.data_dir, options.exit_timing, &ports, &log).await?;
        // 監護日誌先匯入，才做崩潰判定與任何恢復副作用（spec D-24）
        import_guardian_journal(&store, &config.data_dir, &ports, &log).await?;
        if (options.guardian_attempt.is_some() || options.login_start) && reopen_from.is_some() {
            (log)(if options.login_start {
                "登入啟動：停止意圖仍生效，完全退出優先；登入啟動的 Runtime 不提供服務，等使用者手動開 App"
            } else {
                "監護：停止意圖仍生效，完全退出優先；監護啟動的 Runtime 不提供服務，等使用者手動開 App"
            });
            write_recovery_mirror(&store, &config.data_dir, &ports, &log).await;
            store.mark_clean_shutdown(ports.clock.now()).await?;
            return Err(RuntimeError::SuppressedByStopIntent);
        }
        // 使用者手動開啟（或安裝程式代為開啟）：結束更新或解除安裝的維護，之後依 DR-01 重新開啟（票 34）
        maintenance::finish_on_open(&store, &config.data_dir, &options.app_version, &*ports.clock, &log).await?;
        note_crash_recovery(&store, previous_unclean, options.guardian_attempt.clone(), &ports, &log).await?;
        note_login_boundary(&store, login_boundary, &ports, &log).await?;
        sync_login_startup(&store, &ports, &log).await?;
        write_recovery_mirror(&store, &config.data_dir, &ports, &log).await;

        let (stop_tx, stop_rx) = watch::channel(false);
        let stop_tx = Arc::new(stop_tx);
        let supervisor_wake = Arc::new(Notify::new());
        let exit_signal = Arc::new(Notify::new());
        let runtime_url = format!("http://{addr}");
        let run_settings = Arc::new(RunSettings::new(
            store.clone(),
            ports.workspace_fs.clone(),
            config.data_dir.clone(),
            ports.clock.clone(),
        ));
        let lifecycle = LifecycleSettings {
            exit_timing: options.exit_timing,
            data_dir: config.data_dir.clone(),
            reopen_from,
        };
        let supervisor = Supervisor::new(
            store.clone(),
            ports.clone(),
            log.clone(),
            credentials.clone(),
            runtime_url,
            run_settings.clone(),
            options.stop_grace,
            config.data_dir.join(baton_domain::artifact::ARTIFACTS_DIR),
            lifecycle,
        );
        let supervising = options
            .background_supervisor
            .then(|| supervisor.spawn_background(supervisor_wake.clone(), stop_rx.clone()));
        let backups = BackupScheduler::new(store.clone(), config.data_dir.clone(), ports.clock.clone(), log.clone());
        let backing_up = options.background_backup.then(|| backups.spawn_background(stop_rx.clone()));
        let request_backup: baton_api::BackupRequestSink = {
            let backups = backups.clone();
            Arc::new(move |reason: &str| backups.request_milestone_backup(reason))
        };
        let updates = UpdateService::new(
            ports.update_source.clone(),
            ports.clock.clone(),
            log.clone(),
            config.data_dir.clone(),
            &options.app_version,
        );
        let checking_updates = options.background_update_check.then(|| updates.spawn_background(stop_rx.clone()));
        let (exits_tx, exits) = watch::channel(0u64);
        let exiting = spawn_exit_runner(
            supervisor.clone(),
            MaintenanceStep {
                store: store.clone(),
                backups: backups.clone(),
                registration: ports.startup_registration.clone(),
                clock: ports.clock.clone(),
                log: log.clone(),
            },
            exit_signal.clone(),
            exits_tx,
            stop_tx.clone(),
            stop_rx.clone(),
            options.exit_linger,
        );
        let app = baton_api::router(ApiState {
            boot_count,
            started_at,
            allowed_port: addr.port(),
            store: store.clone(),
            data_dir: config.data_dir.clone(),
            clock: ports.clock.clone(),
            platform_info: ports.platform_info.clone(),
            workspace_fs: ports.workspace_fs.clone(),
            claude_env: ports.claude_env.clone(),
            quota_observer: ports.quota_observer.clone(),
            user_settings: ports.user_settings.clone(),
            guardian_probe: ports.guardian_probe.clone(),
            startup_registration: ports.startup_registration.clone(),
            run_settings,
            log,
            supervisor_wake,
            request_backup,
            shutdown: stop_rx.clone(),
            exit_timing: options.exit_timing,
            exit_signal,
            app_version: options.app_version.clone(),
            updates: updates.clone(),
        });
        let task = tokio::spawn(async move {
            let _store = store; // 與 serve 同生命週期：停止後才釋放資料庫連線
            let serving_v6 = async {
                if let Some(listener) = v6 {
                    serve(listener, app.clone(), stop_rx.clone()).await;
                }
            };
            tokio::join!(serve(v4, app.clone(), stop_rx.clone()), serving_v6);
        });
        Ok(RunningRuntime {
            addr,
            addr_v6,
            credentials,
            supervisor,
            supervising,
            backups,
            backing_up,
            updates,
            checking_updates,
            exiting,
            exits,
            stop_tx,
            task,
        })
    }
}

/// 啟動時核對停止意圖（spec D-25）：資料庫與生命週期檔不一致時採較保守者（視為停止）。
/// 只有生命週期檔有停止意圖時補記進資料庫；只有資料庫有時把它鏡像到檔案。
/// 回傳停止意圖的版本（之後由 Supervisor 核對完程序再重新開啟）；沒有停止意圖為 `None`。
async fn reconcile_stop_intent(
    store: &Store,
    data_dir: &std::path::Path,
    timing: ExitTiming,
    ports: &Ports,
    log: &LogSink,
) -> Result<Option<i64>, RuntimeError> {
    let file = {
        let data_dir = data_dir.to_path_buf();
        tokio::task::spawn_blocking(move || baton_store::read_lifecycle_file(&data_dir))
            .await
            .unwrap_or(FileStopView::Stop { requested_at: None })
    };
    let mut state = store.lifecycle_state().await?;
    if lifecycle::conservative_stop(&state, &file).is_some() {
        state = store.adopt_file_stop(timing, ports.clock.now()).await?;
        (log)("生命週期：停止意圖只存在於生命週期檔，依較保守者視為已完全退出");
    } else if state.stop_intent_active() && !matches!(file, FileStopView::Stop { .. }) {
        let mirror = store.lifecycle_mirror(ports.clock.now()).await?;
        let data_dir = data_dir.to_path_buf();
        let written = tokio::task::spawn_blocking(move || baton_store::write_lifecycle_file(&data_dir, &mirror)).await;
        if !matches!(written, Ok(Ok(()))) {
            (log)("生命週期：無法把資料庫中的停止意圖鏡像到生命週期檔");
        }
    }
    if state.stop_intent_active() {
        (log)("生命週期：上次完全退出後重新開啟，先核對程序再依各 Workspace 設定續跑");
    }
    Ok(state.stop_intent_active().then_some(state.version))
}

/// 匯入監護日誌（票 32；spec D-24）：已匯入的嘗試識別略過，其餘在同一個交易中計入恢復事件（或記下撤回、不計的原因）。
/// 讀不到日誌時只記日誌：監護寫下的嘗試留到下次啟動再匯入，Runtime 照常以自己的判定恢復。
async fn import_guardian_journal(store: &Store, data_dir: &std::path::Path, ports: &Ports, log: &LogSink) -> Result<(), RuntimeError> {
    let dir = data_dir.to_path_buf();
    let journal = match tokio::task::spawn_blocking(move || baton_guardian::files::read_journal(&dir)).await {
        Ok(Ok(journal)) => journal,
        Ok(Err(error)) => {
            (log)(&format!("監護：無法讀取監護日誌，這次不匯入：{error}"));
            return Ok(());
        }
        Err(error) => {
            (log)(&format!("監護：無法讀取監護日誌，這次不匯入：{error}"));
            return Ok(());
        }
    };
    for (attempt_id, disposition) in store.import_guardian_journal(journal, ports.clock.now()).await? {
        (log)(&format!("監護：匯入嘗試 {attempt_id}（{}）", disposition.as_str()));
    }
    Ok(())
}

/// 把恢復狀態鏡像給監護（票 32）；失敗只記日誌（監護看到的是較舊的狀態，計次仍以控制資料庫為準）。
pub(crate) async fn write_recovery_mirror(store: &Store, data_dir: &std::path::Path, ports: &Ports, log: &LogSink) {
    let mirror = match store.recovery_mirror(ports.clock.now()).await {
        Ok(mirror) => mirror,
        Err(error) => return (log)(&format!("監護：無法讀取恢復狀態，沒有更新鏡像：{error}")),
    };
    let dir = data_dir.to_path_buf();
    match tokio::task::spawn_blocking(move || baton_guardian::files::write_recovery_mirror(&dir, &mirror)).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => (log)(&format!("監護：無法寫入恢復狀態鏡像：{error}")),
        Err(error) => (log)(&format!("監護：無法寫入恢復狀態鏡像：{error}")),
    }
}

/// 崩潰恢復的啟動判定（票 31）：停止意圖已核對之後、任何程序副作用之前。上一次沒有正常結束、也沒有停止意圖時，
/// 建立或沿用恢復事件；上一次留下的進行中嘗試記為失敗。之後由 Supervisor 在退避期滿時做下一次嘗試。
/// 監護拉起的 Runtime（`own_attempt`）接續監護已計次的那次嘗試，不把它記為失敗（票 32）。
async fn note_crash_recovery(
    store: &Store,
    previous_unclean: bool,
    own_attempt: Option<String>,
    ports: &Ports,
    log: &LogSink,
) -> Result<(), RuntimeError> {
    let event_id = supervisor::new_prefixed_id("rec").ok_or_else(|| RuntimeError::Entropy("無法產生恢復事件識別".into()))?;
    let event = store.note_runtime_start_owning(previous_unclean, own_attempt, event_id, ports.clock.now()).await?;
    match event.filter(|event| event.is_open()) {
        Some(event) if event.status == RecoveryStatus::Exhausted => (log)(&format!(
            "崩潰恢復：事件 {} 的恢復額度已用盡，不自動恢復也不派工，等使用者從恢復介面重新嘗試",
            event.id
        )),
        Some(event) => (log)(&format!(
            "崩潰恢復：事件 {}（第 {} 輪，已用 {}/{} 次，狀態 {}）",
            event.id,
            event.round,
            event.attempts_used,
            MAX_ATTEMPTS,
            event.status.as_str()
        )),
        None => {}
    }
    Ok(())
}

/// 跨登入啟動的處置（票 33）：停止意圖核對完之後、任何程序副作用之前。跨了登入時，沒有開啟「跨登入自動續跑」的
/// Workspace 暫停、等「繼續工作」；無法核對時全部暫停。停止意圖生效時不做任何事（之後依 DR-01 重新開啟）。
/// 不碰恢復事件、退避與額度阻擋：重新登入不重設它們。
async fn note_login_boundary(
    store: &Store,
    boundary: LoginBoundary,
    ports: &Ports,
    log: &LogSink,
) -> Result<(), RuntimeError> {
    let paused = store.apply_login_boundary(boundary, ports.clock.now()).await?;
    match boundary {
        LoginBoundary::NewLogin => (log)(&format!(
            "跨登入：上一次啟動在另一次 Windows 登入；{} 個 Workspace 沒有開啟跨登入自動續跑，等待「繼續工作」",
            paused.len()
        )),
        LoginBoundary::Unverified => (log)(&format!(
            "跨登入：無法核對這次啟動是否跨了 Windows 登入，不自動續跑；{} 個 Workspace 等待「繼續工作」",
            paused.len()
        )),
        LoginBoundary::FirstStart | LoginBoundary::SameLogin => {}
    }
    Ok(())
}

/// 讓 Windows 的登入啟動登記與「登入時自動啟動」的偏好一致（票 33）：偏好是依據，登記被移除或殘留時補正。
/// 失敗只記日誌（不影響工作；設定頁會顯示登記狀態）。完全退出不改變偏好，所以這裡也不因停止意圖而移除登記。
async fn sync_login_startup(store: &Store, ports: &Ports, log: &LogSink) -> Result<(), RuntimeError> {
    let enabled = store.login_startup().await?.enabled;
    let registration = ports.startup_registration.clone();
    let synced = tokio::task::spawn_blocking(move || match registration.is_registered() {
        Ok(registered) if registered == enabled => Ok(false),
        Ok(_) => registration.set_registered(enabled).map(|()| true),
        Err(error) => Err(error),
    })
    .await;
    match synced {
        Ok(Ok(true)) => (log)(&format!(
            "登入啟動：Windows 的登記與偏好不一致，已{}登記",
            if enabled { "補上" } else { "移除" }
        )),
        Ok(Ok(false)) => {}
        Ok(Err(baton_ports::PortError::NotImplemented(_))) => {}
        Ok(Err(error)) => (log)(&format!("登入啟動：無法核對 Windows 的登記：{error}")),
        Err(error) => (log)(&format!("登入啟動：無法核對 Windows 的登記：{error}")),
    }
    Ok(())
}

/// 停止流程之後的維護步驟需要的元件（票 34）。
struct MaintenanceStep {
    store: Store,
    backups: Arc<BackupScheduler>,
    registration: Arc<dyn baton_ports::StartupRegistration>,
    clock: Arc<dyn baton_ports::Clock>,
    log: LogSink,
}

/// 完全退出的執行者：API 持久化停止意圖後通知它，它請 Supervisor 停下全部 Run；
/// 更新或解除安裝的維護接著做停止之後的步驟（票 34）。
/// 停止流程結束後（`linger` 有值時）再提供一段時間 API，讓 App 讀到最終的停止結果，然後結束服務。
fn spawn_exit_runner(
    supervisor: Arc<Supervisor>,
    maintenance: MaintenanceStep,
    signal: Arc<Notify>,
    exits: watch::Sender<u64>,
    stop_tx: Arc<watch::Sender<bool>>,
    mut stop_rx: watch::Receiver<bool>,
    linger: Option<Duration>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = stop_rx.wait_for(|stopping| *stopping) => break,
                _ = signal.notified() => {}
            }
            supervisor.full_exit().await;
            maintenance::continue_after_stop(
                &maintenance.store,
                &maintenance.backups,
                maintenance.registration.clone(),
                &*maintenance.clock,
                &maintenance.log,
            )
            .await;
            exits.send_modify(|count| *count += 1);
            if let Some(linger) = linger {
                tokio::time::sleep(linger).await;
                let _ = stop_tx.send(true);
                break;
            }
        }
    })
}

async fn serve(listener: TcpListener, app: axum::Router, mut stop: watch::Receiver<bool>) {
    let _ = axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = stop.wait_for(|stopping| *stopping).await;
        })
        .await;
}

/// 綁定 IPv4 與 IPv6 loopback（同一個埠）。
///
/// 本機沒有 IPv6 時只服務 IPv4；IPv6 的埠被別的程序占用則視為錯誤（不可悄悄讓出那個位址）。
async fn bind_loopback(bind: SocketAddr) -> Result<(TcpListener, Option<TcpListener>), RuntimeError> {
    let v4_addr = SocketAddr::from(([127, 0, 0, 1], bind.port()));
    for attempt in 1..=BIND_ATTEMPTS {
        let v4 = TcpListener::bind(v4_addr).await.map_err(|e| RuntimeError::Bind(v4_addr, e))?;
        let port = v4.local_addr().map_err(|e| RuntimeError::Bind(v4_addr, e))?.port();
        let v6_addr = SocketAddr::from((IpAddr::V6(Ipv6Addr::LOCALHOST), port));
        match TcpListener::bind(v6_addr).await {
            Ok(v6) => return Ok((v4, Some(v6))),
            // 隨機埠只是剛好在 IPv6 上被占用：換一個埠再試
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse && bind.port() == 0 && attempt < BIND_ATTEMPTS => {}
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => return Err(RuntimeError::Bind(v6_addr, e)),
            Err(_) => return Ok((v4, None)),
        }
    }
    unreachable!("最後一次嘗試一定會在迴圈內回傳")
}

/// 執行中的 Runtime 實例。
pub struct RunningRuntime {
    addr: SocketAddr,
    addr_v6: Option<SocketAddr>,
    credentials: CredentialIssuer,
    supervisor: Arc<Supervisor>,
    supervising: Option<JoinHandle<()>>,
    backups: Arc<BackupScheduler>,
    backing_up: Option<JoinHandle<()>>,
    updates: Arc<UpdateService>,
    checking_updates: Option<JoinHandle<()>>,
    /// 完全退出的執行者（票 25）。
    exiting: JoinHandle<()>,
    /// 這個實例已完成的完全退出次數（停止流程結束就加一）。
    exits: watch::Receiver<u64>,
    stop_tx: Arc<watch::Sender<bool>>,
    task: JoinHandle<()>,
}

impl RunningRuntime {
    /// IPv4 loopback 的監聽位址。
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    /// IPv6 loopback 的監聽位址；本機沒有 IPv6 時為 `None`。
    pub fn local_addr_v6(&self) -> Option<SocketAddr> {
        self.addr_v6
    }

    /// 簽發憑證的入口（Run 的 Agent 憑證由 Supervisor 在准入時簽發）。
    pub fn credentials(&self) -> &CredentialIssuer {
        &self.credentials
    }

    /// Supervisor；測試 harness 以它主動驅動一次處理（`tick`）。
    pub fn supervisor(&self) -> &Arc<Supervisor> {
        &self.supervisor
    }

    /// 備份排程；重要裁定或 Stage 完成時以它要求備份，測試 harness 以它主動驅動一次處理（`tick`）。
    pub fn backups(&self) -> &Arc<BackupScheduler> {
        &self.backups
    }

    /// 更新檢查；測試 harness 以它主動驅動一次檢查（`tick`）。
    pub fn updates(&self) -> &Arc<UpdateService> {
        &self.updates
    }

    /// 完全退出的停止流程完成次數；測試以它等待「停止流程已結束」。
    pub fn exits_completed(&self) -> watch::Receiver<u64> {
        self.exits.clone()
    }

    /// 停止服務並等到資料庫連線釋放；之後可用同一資料目錄重新啟動。
    /// 這是有秩序的停止：記下這一次啟動正常結束，下次啟動不視為意外終止（票 31）。
    pub async fn shutdown(self) {
        self.stop(true).await
    }

    /// 模擬意外終止（測試 harness 用）：停止服務但不記下正常結束，下次啟動視為崩潰並開始崩潰恢復。
    pub async fn kill(self) {
        self.stop(false).await
    }

    async fn stop(mut self, clean: bool) {
        let _ = self.stop_tx.send(true);
        let _ = (&mut self.task).await;
        if let Some(supervising) = self.supervising.take() {
            let _ = supervising.await;
        }
        if let Some(backing_up) = self.backing_up.take() {
            let _ = backing_up.await;
        }
        if let Some(checking) = self.checking_updates.take() {
            let _ = checking.await;
        }
        if clean {
            self.supervisor.mark_clean_shutdown().await;
        }
        // 停止流程進行到一半就被要求停止服務，等同當掉：下次啟動時停止意圖仍在，先核對再續跑
        self.exiting.abort();
        let _ = (&mut self.exiting).await;
    }

    /// 等到服務自行結束（正式執行檔用）。
    pub async fn wait(mut self) {
        let _ = (&mut self.task).await;
    }
}

impl Drop for RunningRuntime {
    fn drop(&mut self) {
        // 沒有明確 shutdown 就被丟棄時，直接中止服務，避免殘留背景工作
        self.task.abort();
        if let Some(supervising) = &self.supervising {
            supervising.abort();
        }
        if let Some(backing_up) = &self.backing_up {
            backing_up.abort();
        }
        if let Some(checking) = &self.checking_updates {
            checking.abort();
        }
        self.exiting.abort();
    }
}
