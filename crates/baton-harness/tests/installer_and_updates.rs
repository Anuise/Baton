//! 情境測試：更新檢查、更新與解除安裝的維護（票 34，spec D-19、D-25），經由 Runtime harness 的真實 HTTP API 驗證。
//!
//! 「更新來源」是 fake（`publish` 公布版本、`reject_signature` 讓簽章驗證失敗）；「檢查更新」= `check_updates`
//! （Runtime 背景迴圈的一次處理）。「安裝程式換上新版本並重新開啟」= `install_version` + `open_app`。
//! 「解除安裝程式的準備步驟」= `baton_runtime::installer::prepare`（NSIS hook 執行的 `baton-runtime.exe maintenance …`），
//! 透過真實的桌面用戶端連線。時間全由 fake Clock 決定。只斷言外部行為：端點、Run 與 Task、Port 被呼叫的方式與磁碟上的檔案。
//!
//! 每個測試名稱對應票 34 的行為：
//! - 啟動時與每日檢查簽章更新，只通知：`update_checks_*`、`an_unconfigured_*`、`a_download_*`
//! - 更新期間監護與登入啟動不會拉起 Runtime：`consenting_to_an_update_*`、`a_failed_pre_update_backup_*`
//! - 解除安裝後不留下登入啟動機碼；資料目錄保留：`uninstalling_*`、`the_installer_run_by_an_app_update_*`
//! - 維護與完全退出互斥：`maintenance_cannot_start_*`

use std::sync::Arc;
use std::time::Duration;

use baton_client::RuntimeClient;
use baton_domain::guardian::Suppression;
use baton_domain::maintenance::{MaintenanceKind, UPDATE_CHECK_INTERVAL_MS};
use baton_domain::Timestamp;
use baton_guardian::launcher::LoginLaunch;
use baton_guardian::{GuardianEnd, GuardianStep};
use baton_harness::{Harness, DEFAULT_APP_VERSION};
use baton_runtime::installer::{self, PrepareTiming, Prepared};
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

const WORKSPACES: &str = "/api/v1/workspaces";
const LIFECYCLE: &str = "/api/v1/lifecycle";
const LOGIN_STARTUP: &str = "/api/v1/lifecycle/login-startup";
const UPDATES: &str = "/api/v1/updates";
const UPDATE_APPLY: &str = "/api/v1/updates/apply";
const MAINTENANCE: &str = "/api/v1/maintenance";
const HEALTH: &str = "/api/v1/health";

/// 2023-11-14 22:13:20.123 UTC
const CLOCK_START: Timestamp = Timestamp::from_millis(1_700_000_000_123);
const HOUR: i64 = 60 * 60 * 1000;

// --- 共用輔助 ---

async fn start() -> Harness {
    started(Harness::builder().clock_at(CLOCK_START).start().await).await
}

async fn started(harness: Harness) -> Harness {
    harness.add_account("帳號 1", "a@example.com").await;
    harness
}

fn filled(text: &str) -> Value {
    json!({ "status": "filled", "text": text })
}

/// 一個 Workspace 與它的 Goal，回傳 (workspace_id, goal_id)。
async fn workspace(harness: &Harness, name: &str) -> (String, String) {
    let dir = harness.data_dir().join("projects").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let body = json!({ "path": dir.to_string_lossy(), "name": name });
    let (status, created) = harness.send_json(Method::POST, WORKSPACES, Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let ws = created["id"].as_str().unwrap().to_string();
    let (status, goal) =
        harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/goals"), Some(json!({ "title": "目標" }))).await;
    assert_eq!(status, StatusCode::CREATED, "{goal}");
    (ws, goal["id"].as_str().unwrap().to_string())
}

/// 建立 ready 的 Task、派工並讓 Supervisor 啟動它，回傳 (task_id, run_id)。
async fn running_task(harness: &Harness, ws: &str, goal: &str) -> (String, String) {
    let contract = json!({
        "objective": filled("整理資料"),
        "inputs": filled("既有的資料"),
        "scope": filled("公開資料"),
        "deliverables": filled("比較表"),
        "done_criteria": filled("五個產品"),
        "blocker_handling": filled("卡住時回報"),
        "required_capabilities": [],
    });
    let body = json!({ "goal_id": goal, "title": "長任務", "contract": contract });
    let (status, task) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks"), Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    let tid = task["id"].as_str().unwrap().to_string();
    let (status, ready) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks/{tid}/ready"), None).await;
    assert_eq!(status, StatusCode::OK, "{ready}");
    let (status, run) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks/{tid}/dispatch"), None).await;
    assert_eq!(status, StatusCode::CREATED, "{run}");
    let rid = run["id"].as_str().unwrap().to_string();
    harness.supervise().await;
    assert_eq!(run_view(harness, ws, &rid).await["status"], "running");
    (tid, rid)
}

async fn run_view(harness: &Harness, ws: &str, rid: &str) -> Value {
    let (status, run) = harness.send_json(Method::GET, &format!("{WORKSPACES}/{ws}/runs/{rid}"), None).await;
    assert_eq!(status, StatusCode::OK, "{run}");
    run
}

/// 這個 Task 的全部 Run，依建立順序。
async fn runs_of(harness: &Harness, ws: &str, tid: &str) -> Vec<Value> {
    let (status, list) = harness.send_json(Method::GET, &format!("{WORKSPACES}/{ws}/runs"), None).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    list["runs"].as_array().unwrap().iter().filter(|r| r["task_id"] == tid).cloned().collect()
}

async fn updates(harness: &Harness) -> Value {
    harness.get_json(UPDATES).await
}

async fn lifecycle(harness: &Harness) -> Value {
    harness.get_json(LIFECYCLE).await
}

/// 生命週期總覽中這個 Workspace 的設定與暫停狀態。
async fn workspace_view(harness: &Harness, ws: &str) -> Value {
    let status = lifecycle(harness).await;
    status["workspaces"].as_array().unwrap().iter().find(|w| w["workspace_id"] == ws).unwrap().clone()
}

async fn set_after_exit(harness: &Harness, ws: &str, enabled: bool) {
    let body = json!({ "auto_resume_after_exit": enabled });
    let (status, view) = harness.send_json(Method::PUT, &format!("{WORKSPACES}/{ws}/lifecycle"), Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{view}");
}

async fn enable_login_startup(harness: &Harness) {
    let (status, view) = harness.send_json(Method::PUT, LOGIN_STARTUP, Some(json!({ "enabled": true }))).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert!(harness.startup_registration.registered());
}

async fn apply(harness: &Harness, version: &str) -> (StatusCode, Value) {
    harness.send_json(Method::POST, UPDATE_APPLY, Some(json!({ "version": version }))).await
}

/// 生命週期檔目前的內容（JSON）；沒有檔案時為 `Null`。
fn lifecycle_file(harness: &Harness) -> Value {
    std::fs::read(harness.lifecycle_file_path())
        .map(|bytes| serde_json::from_slice(&bytes).expect("生命週期檔應是 JSON"))
        .unwrap_or(Value::Null)
}

/// 備份目錄中這個觸發原因的備份識別。
fn backups_with(harness: &Harness, trigger: &str) -> Vec<String> {
    std::fs::read_dir(harness.backups_dir())
        .map(|entries| {
            entries
                .filter_map(|e| e.ok()?.file_name().into_string().ok())
                .filter(|name| name.ends_with(&format!("-{trigger}")))
                .collect()
        })
        .unwrap_or_default()
}

fn no_log(_: &str) {}

/// 解除安裝程式的準備步驟（`baton-runtime.exe maintenance prepare-…`），以真實的桌面用戶端連到 `addr`。
async fn prepare_installer(harness: &Harness, kind: MaintenanceKind, addr: std::net::SocketAddr) -> installer::PrepareReport {
    let client = RuntimeClient::new(addr, harness.data_dir().to_path_buf(), harness.secret_protector.clone());
    let timing =
        PrepareTiming { runtime_exit: Duration::from_secs(30), processes_exit: Duration::from_secs(1), poll: Duration::from_millis(20) };
    installer::prepare(
        kind,
        harness.data_dir(),
        &client,
        &*harness.startup_registration,
        &*harness.clock,
        timing,
        &(Arc::new(no_log) as baton_api::LogSink),
    )
    .await
}

// ============================================================ 更新檢查：只通知

#[tokio::test]
async fn update_checks_run_at_startup_and_once_a_day_and_only_notify() {
    let harness = start().await;
    let (ws, goal) = workspace(&harness, "A").await;
    let (_, rid) = running_task(&harness, &ws, &goal).await;

    let before = updates(&harness).await;
    assert_eq!(before["status"], "not_checked");
    assert_eq!(before["current_version"], DEFAULT_APP_VERSION);

    // 啟動時檢查：有新版本只記為「可更新」
    harness.update_source.publish("0.2.0");
    harness.check_updates().await;
    assert_eq!(harness.update_source.checks(), 1);
    let status = updates(&harness).await;
    assert_eq!(status["status"], "available", "{status}");
    assert_eq!(status["available"]["version"], "0.2.0");
    assert_eq!(status["available"]["notes"], "Baton 0.2.0 的更新說明");
    assert!(status["available"].get("url").is_none() && status["available"].get("signature").is_none(), "{status}");
    assert_eq!(status["checked_at"], json!(CLOCK_START.as_millis()));
    assert_eq!(status["next_check_at"], json!(CLOCK_START.as_millis() + UPDATE_CHECK_INTERVAL_MS));
    assert_eq!(status["maintenance"], Value::Null);

    // 只通知：工作照常、沒有停止意圖、沒有維護抑制、沒有下載
    harness.supervise().await;
    assert_eq!(run_view(&harness, &ws, &rid).await["status"], "running");
    assert!(harness.process_control.stop_calls().is_empty());
    assert_eq!(lifecycle(&harness).await["stop_intent_active"], false);
    assert_eq!(lifecycle_file(&harness), Value::Null);
    assert!(harness.update_source.downloads().is_empty());

    // 每日一次：未滿 24 小時不再檢查
    harness.clock.advance_millis(23 * HOUR);
    harness.check_updates().await;
    assert_eq!(harness.update_source.checks(), 1);
    harness.clock.advance_millis(HOUR);
    harness.update_source.fail_next_check("連線逾時");
    harness.check_updates().await;
    assert_eq!(harness.update_source.checks(), 2);
    let failed = updates(&harness).await;
    assert_eq!(failed["status"], "failed");
    assert_eq!(failed["error"], "連線逾時");
    assert_eq!(failed["available"]["version"], "0.2.0", "之前看到的新版本仍可更新");

    // 重新啟動就重新檢查（啟動時）
    let mut harness = harness;
    harness.restart().await;
    harness.check_updates().await;
    assert_eq!(harness.update_source.checks(), 3);
    assert_eq!(updates(&harness).await["status"], "available");
}

#[tokio::test]
async fn an_unconfigured_update_source_is_reported_and_nothing_can_be_applied() {
    let harness = start().await;
    harness.check_updates().await;
    let status = updates(&harness).await;
    assert_eq!(status["status"], "unconfigured", "{status}");
    assert_eq!(status["available"], Value::Null);

    let (status, body) = apply(&harness, "0.2.0").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert_eq!(body["error"], "updates_unconfigured");
    assert_eq!(lifecycle(&harness).await["stop_intent_active"], false);
}

#[tokio::test]
async fn a_download_with_a_bad_signature_or_a_stale_version_stops_no_work() {
    let harness = start().await;
    let (ws, goal) = workspace(&harness, "A").await;
    let (_, rid) = running_task(&harness, &ws, &goal).await;
    harness.update_source.publish("0.2.0");
    harness.check_updates().await;

    // 畫面上的版本已過時
    let (status, body) = apply(&harness, "0.1.5").await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"], "update_not_available");

    // 簽章驗證失敗：不開始維護、不停止任何工作、不留下安裝程式
    harness.update_source.reject_signature(true);
    let (status, body) = apply(&harness, "0.2.0").await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    assert_eq!(body["error"], "update_download_failed");
    assert!(body["message"].as_str().unwrap().contains("簽章"), "{body}");
    assert_eq!(updates(&harness).await["maintenance"], Value::Null);
    assert_eq!(lifecycle(&harness).await["stop_intent_active"], false);
    assert_eq!(lifecycle_file(&harness), Value::Null);
    harness.supervise().await;
    assert_eq!(run_view(&harness, &ws, &rid).await["status"], "running");
    assert!(harness.process_control.stop_calls().is_empty());
}

// ============================================================ 更新期間不拉起 Runtime

#[tokio::test]
async fn consenting_to_an_update_suppresses_the_guardian_and_login_start_until_the_app_is_reopened() {
    let mut harness = start().await;
    let (auto, goal_a) = workspace(&harness, "自動續跑").await;
    let (manual, goal_b) = workspace(&harness, "等繼續工作").await;
    set_after_exit(&harness, &auto, true).await;
    let (task_a, run_a) = running_task(&harness, &auto, &goal_a).await;
    let (task_b, run_b) = running_task(&harness, &manual, &goal_b).await;
    enable_login_startup(&harness).await;
    harness.start_guardian();
    harness.update_source.publish("0.2.0");
    harness.check_updates().await;

    // 同意更新：下載並驗證 → 維護抑制 + 停止意圖（同一交易）→ 與完全退出相同的停止流程
    let (status, body) = apply(&harness, "0.2.0").await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert_eq!(body["maintenance"]["kind"], "update");
    assert_eq!(body["maintenance"]["phase"], "stopping");
    assert_eq!(body["maintenance"]["from_version"], DEFAULT_APP_VERSION);
    assert_eq!(body["maintenance"]["target_version"], "0.2.0");
    assert_eq!(harness.update_source.downloads(), vec!["0.2.0"]);
    let package = body["maintenance"]["package_path"].as_str().unwrap().to_string();
    assert!(std::path::Path::new(&package).is_file(), "已驗證的安裝程式在資料目錄下：{package}");
    let file = lifecycle_file(&harness);
    assert_eq!(file["maintenance"]["kind"], "update", "{file}");
    assert!(file["stop"].is_object(), "{file}");

    harness.wait_for_exit().await;
    // 停止流程停下了全部 Run（不是取消），之後做了更新前備份，進入「準備套用」
    assert_eq!(harness.process_control.stop_calls().len(), 2);
    for (ws, rid) in [(&auto, &run_a), (&manual, &run_b)] {
        let run = run_view(&harness, ws, rid).await;
        assert_eq!(run["status"], "interrupted", "{run}");
        assert_eq!(run["end_reason"], "stopped_by_full_exit");
    }
    let ready = updates(&harness).await;
    assert_eq!(ready["maintenance"]["phase"], "ready", "{ready}");
    let backup_id = ready["maintenance"]["backup_id"].as_str().unwrap().to_string();
    assert_eq!(backups_with(&harness, "pre_update"), vec![backup_id]);
    assert_eq!(lifecycle_file(&harness)["maintenance"]["kind"], "update", "停止流程的鏡像保留維護抑制");

    // Runtime 結束（桌面殼接著執行安裝程式）：監護看到維護抑制，不拉起 Runtime
    harness.stop_runtime_after_exit().await;
    assert_eq!(harness.guardian_step().await, GuardianStep::RuntimeExited);
    assert_eq!(
        harness.guardian_step().await,
        GuardianStep::Finished(GuardianEnd::Suppressed(Suppression::Maintenance("update".into())))
    );
    assert!(!harness.runtime_running());
    assert!(harness.guardian_starts().is_empty());

    // 安裝期間 Windows 重新登入：登入啟動入口同樣受維護抑制
    harness.sign_in_again().await;
    let login = harness.windows_login_startup().await.expect("登入啟動仍登記著");
    assert_eq!(login.launch, LoginLaunch::Suppressed(Suppression::Maintenance("update".into())));
    assert_eq!(login.runtime_outcome, None);
    assert!(!harness.runtime_running());
    let spawned = harness.process_control.spawn_count();
    assert_eq!(spawned, 2, "更新期間沒有任何 Worker 被拉起");

    // 安裝程式換上新版本並重新開啟 App：結束維護、清掉維護抑制與下載的安裝程式
    harness.install_version("0.2.0");
    harness.open_app().await;
    let health: Value = harness.get_json(HEALTH).await;
    assert_eq!(health["version"], "0.2.0");
    let done = updates(&harness).await;
    assert_eq!(done["maintenance"], Value::Null, "{done}");
    assert_eq!(done["current_version"], "0.2.0");
    assert_eq!(done["last_maintenance"]["kind"], "update");
    assert_eq!(done["last_maintenance"]["applied"], true);
    assert_eq!(done["last_maintenance"]["from_version"], DEFAULT_APP_VERSION);
    assert_eq!(done["last_maintenance"]["running_version"], "0.2.0");
    assert_eq!(lifecycle_file(&harness).get("maintenance"), None, "維護抑制已清除");
    assert!(!std::path::Path::new(&package).exists(), "用過的安裝程式已刪除");
    assert!(harness.startup_registration.registered(), "更新不改變登入啟動");

    // 依 DR-01 重新開啟：開啟完全退出後自動續跑的 Workspace 續跑，另一個等「繼續工作」
    harness.supervise().await;
    harness.supervise().await;
    assert_eq!(lifecycle(&harness).await["phase"], "running");
    assert_eq!(workspace_view(&harness, &auto).await["awaiting_resume"], false);
    assert_eq!(workspace_view(&harness, &manual).await["awaiting_resume"], true);
    let resumed = runs_of(&harness, &auto, &task_a).await;
    assert_eq!(resumed.len(), 2, "自動續跑的 Task 以新 Run 續跑：{resumed:?}");
    assert_eq!(resumed[1]["status"], "running");
    assert_eq!(runs_of(&harness, &manual, &task_b).await.len(), 1, "等「繼續工作」的 Task 沒有被派出");
    assert_eq!(harness.process_control.spawn_count(), spawned + 1);
}

#[tokio::test]
async fn a_failed_pre_update_backup_applies_nothing_and_the_next_open_follows_the_exit_rules() {
    let mut harness = start().await;
    let (ws, goal) = workspace(&harness, "A").await;
    running_task(&harness, &ws, &goal).await;
    harness.start_guardian();
    harness.update_source.publish("0.2.0");
    harness.check_updates().await;
    // 備份目錄的位置被一個檔案佔住：無法建立更新前備份
    std::fs::write(harness.backups_dir(), b"not a directory").unwrap();

    let (status, body) = apply(&harness, "0.2.0").await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    harness.wait_for_exit().await;
    let failed = updates(&harness).await;
    assert_eq!(failed["maintenance"]["phase"], "failed", "{failed}");
    assert!(failed["maintenance"]["message"].as_str().unwrap().contains("備份"), "{failed}");

    // 仍在維護中：Runtime 結束後監護不拉起它
    harness.stop_runtime_after_exit().await;
    assert_eq!(harness.guardian_step().await, GuardianStep::RuntimeExited);
    assert_eq!(
        harness.guardian_step().await,
        GuardianStep::Finished(GuardianEnd::Suppressed(Suppression::Maintenance("update".into())))
    );

    // 沒有安裝新版本就重新開啟：記下「沒有套用」，之後照完全退出的規則等「繼續工作」
    harness.open_app().await;
    let done = updates(&harness).await;
    assert_eq!(done["maintenance"], Value::Null);
    assert_eq!(done["last_maintenance"]["applied"], false, "{done}");
    harness.supervise().await;
    assert_eq!(workspace_view(&harness, &ws).await["awaiting_resume"], true);
}

// ============================================================ 解除安裝

#[tokio::test]
async fn uninstalling_stops_everything_removes_the_login_startup_registration_and_keeps_the_data_directory() {
    // Runtime 完成停止流程後與正式執行相同地自行結束服務，解除安裝程式的準備步驟才看得到它結束
    let mut harness =
        started(Harness::builder().clock_at(CLOCK_START).exit_linger(Duration::from_millis(50)).start().await).await;
    let (ws, goal) = workspace(&harness, "A").await;
    let (tid, rid) = running_task(&harness, &ws, &goal).await;
    enable_login_startup(&harness).await;
    harness.start_guardian();

    let report = prepare_installer(&harness, MaintenanceKind::Uninstall, harness.addr()).await;
    assert_eq!(report.prepared, Prepared::RuntimeStopped, "{report:?}");
    assert!(report.registration_removed);
    assert!(report.notes.is_empty(), "{report:?}");

    // 登入啟動的登記已移除（Runtime 在停止流程後移除一次，準備步驟再確認一次）
    assert!(!harness.startup_registration.registered());
    assert_eq!(harness.startup_registration.changes(), vec![true, false, false]);
    // 與完全退出相同地停下 Run（沒有取消 Task）
    let file = lifecycle_file(&harness);
    assert_eq!(file["maintenance"]["kind"], "uninstall", "{file}");
    harness.stop_runtime_after_exit().await;
    assert_eq!(harness.process_control.stop_calls().len(), 1);

    // 監護與 Windows 登入都不再拉起 Runtime
    assert_eq!(harness.guardian_step().await, GuardianStep::RuntimeExited);
    assert_eq!(
        harness.guardian_step().await,
        GuardianStep::Finished(GuardianEnd::Suppressed(Suppression::Maintenance("uninstall".into())))
    );
    harness.sign_in_again().await;
    assert_eq!(harness.windows_login_startup().await, None, "沒有登入啟動的登記，登入時不執行任何東西");
    assert!(!harness.runtime_running());

    // 資料目錄保留：控制資料庫、生命週期檔與 Workspace 的資料都還在
    assert!(harness.data_dir().join(baton_store::DB_FILE_NAME).is_file());
    let (status, name): (String, String) = harness
        .db()
        .query_row("SELECT status, name FROM workspace WHERE id = ?1", [&ws], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap();
    assert_eq!((status.as_str(), name.as_str()), ("active", "A"));

    // 之後重新安裝並開啟：沿用保留的資料，結束維護；工作依完全退出後的規則等「繼續工作」
    harness.open_app().await;
    let done = updates(&harness).await;
    assert_eq!(done["maintenance"], Value::Null);
    assert_eq!(done["last_maintenance"]["kind"], "uninstall");
    assert_eq!(done["last_maintenance"]["applied"], Value::Null);
    harness.supervise().await;
    assert_eq!(workspace_view(&harness, &ws).await["awaiting_resume"], true);
    assert_eq!(run_view(&harness, &ws, &rid).await["status"], "interrupted");
    assert_eq!(runs_of(&harness, &ws, &tid).await.len(), 1);
    // 「登入時自動啟動」的偏好保留在資料目錄中，重新安裝後的第一次開啟依偏好重新登記
    assert!(harness.startup_registration.registered());
}

#[tokio::test]
async fn uninstalling_while_no_runtime_runs_marks_the_lifecycle_file_and_removes_the_registration() {
    let mut harness = start().await;
    enable_login_startup(&harness).await;
    let addr = harness.addr();
    // 使用者先完全退出了（Runtime 不在）
    let (status, body) = harness.send_json(Method::POST, "/api/v1/lifecycle/exit", None).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    harness.wait_for_exit().await;
    harness.stop_runtime_after_exit().await;

    let report = prepare_installer(&harness, MaintenanceKind::Uninstall, addr).await;
    assert_eq!(report.prepared, Prepared::MarkedWithoutRuntime, "{report:?}");
    assert!(report.registration_removed);
    assert!(!harness.startup_registration.registered());
    let file = lifecycle_file(&harness);
    assert_eq!(file["maintenance"]["kind"], "uninstall", "{file}");
    assert!(file["stop"].is_object(), "保留原有的停止抑制：{file}");
    assert!(harness.data_dir().join(baton_store::DB_FILE_NAME).is_file());

    // 從未有過生命週期檔也一樣寫入維護抑制
    let fresh = start().await;
    let addr = fresh.addr();
    let mut fresh = fresh;
    fresh.kill_runtime().await;
    let report = prepare_installer(&fresh, MaintenanceKind::Update, addr).await;
    assert_eq!(report.prepared, Prepared::MarkedWithoutRuntime);
    assert!(!report.registration_removed, "更新不移除登入啟動登記");
    let file = lifecycle_file(&fresh);
    assert_eq!(file["maintenance"]["kind"], "update", "{file}");
    assert_eq!(file["stop"], Value::Null);
    let login = fresh.windows_login_startup().await;
    assert_eq!(login, None);
}

#[tokio::test]
async fn the_installer_run_by_an_app_update_leaves_the_running_update_and_the_registration_alone() {
    let mut harness =
        started(Harness::builder().clock_at(CLOCK_START).exit_linger(Duration::from_millis(50)).start().await).await;
    enable_login_startup(&harness).await;
    harness.update_source.publish("0.2.0");
    harness.check_updates().await;
    let (status, body) = apply(&harness, "0.2.0").await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    harness.wait_for_exit().await;
    let addr = harness.addr();
    harness.stop_runtime_after_exit().await;

    // 安裝程式的 PREINSTALL（與誤觸的解除安裝準備）看到更新的維護就不再動作
    for kind in [MaintenanceKind::Update, MaintenanceKind::Uninstall] {
        let report = prepare_installer(&harness, kind, addr).await;
        assert_eq!(report.prepared, Prepared::AlreadyUpdating, "{report:?}");
        assert!(!report.registration_removed);
    }
    assert!(harness.startup_registration.registered(), "更新保留登入啟動");
    assert_eq!(lifecycle_file(&harness)["maintenance"]["kind"], "update");
}

// ============================================================ 維護與完全退出互斥

#[tokio::test]
async fn maintenance_cannot_start_during_a_full_exit_or_another_maintenance() {
    let harness = start().await;
    harness.update_source.publish("0.2.0");
    harness.check_updates().await;

    let (status, body) = harness.send_json(Method::POST, MAINTENANCE, Some(json!({ "kind": "uninstall" }))).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert_eq!(body["maintenance"]["kind"], "uninstall");
    // 同一種維護的重複要求是冪等的
    let (status, again) = harness.send_json(Method::POST, MAINTENANCE, Some(json!({ "kind": "uninstall" }))).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{again}");
    assert_eq!(again["maintenance"]["requested_at"], body["maintenance"]["requested_at"]);
    // 另一種維護被拒絕，也不下載
    let (status, body) = apply(&harness, "0.2.0").await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"], "maintenance_in_progress");
    assert!(harness.update_source.downloads().is_empty());

    // 完全退出進行中：不能開始維護
    let other = start().await;
    let (status, body) = other.send_json(Method::POST, "/api/v1/lifecycle/exit", None).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let (status, body) = other.send_json(Method::POST, MAINTENANCE, Some(json!({ "kind": "update" }))).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"], "stop_intent_active");
}
