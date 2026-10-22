//! 情境測試：跨 Windows 登入的啟動、續跑與停止抑制（票 33，ADR-0007、spec D-25），經由 Runtime harness 的真實 HTTP API 驗證。
//!
//! 「重新登入」= `sign_in_again`：同一次登入的程序全部結束（Runtime 沒有正常結束、監護也不在），換一個新的登入識別。
//! 「登入啟動」= `windows_login_startup`：只有登記了登入啟動才會執行啟動入口（`baton-guardian.exe --login` 的邏輯），
//! 它先讀生命週期檔，再以登入啟動的身分啟動 Runtime，之後接著當監護。「手動開 App」= `open_app`。
//! 時間全由 fake Clock 決定。只斷言外部行為：生命週期／恢復／排程端點、Run 與 Task、Port 被呼叫的方式與磁碟上的檔案。
//!
//! 每個測試名稱對應票 33 的驗收條件：
//! - 兩項新設定預設關閉，與 DR-01 設定分開保存與顯示：`both_new_settings_*`、`login_startup_follows_*`
//! - 完全退出後重新登入不啟動 Runtime、監護或工作：`after_full_exit_*`、`a_stop_intent_only_in_the_database_*`
//! - Workspace 跨登入設定相反的情境不會互相誤用：`signing_in_again_resumes_only_*`、`an_unverified_login_*`
//! - 登入自啟動與手動開 App 同時發生時只有一個控制平面：`login_startup_and_a_manual_app_open_*`
//! - 重新登入不重設恢復額度與阻擋：`signing_in_again_does_not_reset_*`

use baton_domain::guardian::Suppression;
use baton_domain::Timestamp;
use baton_guardian::launcher::LoginLaunch;
use baton_harness::fakes::FakeStop;
use baton_harness::Harness;
use baton_ports::{ProcessIdentity, StopResult};
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

const WORKSPACES: &str = "/api/v1/workspaces";
const LIFECYCLE: &str = "/api/v1/lifecycle";
const LOGIN_STARTUP: &str = "/api/v1/lifecycle/login-startup";
const RECOVERY: &str = "/api/v1/recovery";
const SCHEDULING: &str = "/api/v1/scheduling";
const HEALTH: &str = "/api/v1/health";

/// 2023-11-14 22:13:20.123 UTC
const CLOCK_START: Timestamp = Timestamp::from_millis(1_700_000_000_123);

// --- 共用輔助 ---

async fn start() -> Harness {
    let harness = Harness::builder().clock_at(CLOCK_START).start().await;
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

async fn task_status(harness: &Harness, ws: &str, tid: &str) -> String {
    let (status, detail) = harness.send_json(Method::GET, &format!("{WORKSPACES}/{ws}/tasks/{tid}"), None).await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    detail["task"]["status"].as_str().unwrap().to_string()
}

async fn identity(harness: &Harness, ws: &str, rid: &str) -> ProcessIdentity {
    let run = run_view(harness, ws, rid).await;
    harness.process_control.identity_of(run["command_id"].as_str().unwrap()).unwrap()
}

async fn lifecycle(harness: &Harness) -> Value {
    harness.get_json(LIFECYCLE).await
}

/// 生命週期總覽中這個 Workspace 的設定與暫停狀態。
async fn workspace_view(harness: &Harness, ws: &str) -> Value {
    let status = lifecycle(harness).await;
    status["workspaces"].as_array().unwrap().iter().find(|w| w["workspace_id"] == ws).unwrap().clone()
}

async fn set_after_exit(harness: &Harness, ws: &str, enabled: bool) -> Value {
    let body = json!({ "auto_resume_after_exit": enabled });
    let (status, view) = harness.send_json(Method::PUT, &format!("{WORKSPACES}/{ws}/lifecycle"), Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    view
}

async fn set_across_login(harness: &Harness, ws: &str, enabled: bool) -> Value {
    let body = json!({ "auto_resume_across_login": enabled });
    let (status, view) = harness.send_json(Method::PUT, &format!("{WORKSPACES}/{ws}/login-resume"), Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    view
}

async fn set_login_startup(harness: &Harness, enabled: bool) -> (StatusCode, Value) {
    harness.send_json(Method::PUT, LOGIN_STARTUP, Some(json!({ "enabled": enabled }))).await
}

async fn full_exit(harness: &mut Harness) {
    let (status, body) = harness.send_json(Method::POST, "/api/v1/lifecycle/exit", None).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    harness.wait_for_exit().await;
    harness.stop_runtime_after_exit().await;
}

async fn boot_count(harness: &Harness) -> i64 {
    let health: Value = harness.get_json(HEALTH).await;
    health["boot_count"].as_i64().unwrap()
}

/// 佇列中這個 Task 的等待原因（類別、代碼、說明）。
async fn queue_reasons(harness: &Harness, tid: &str) -> Vec<(String, String, String)> {
    let overview: Value = harness.get_json(SCHEDULING).await;
    let entry = overview["queue"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["task_id"] == tid)
        .unwrap_or_else(|| panic!("{tid} 不在佇列中：{overview}"))
        .clone();
    entry["reasons"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["category"].as_str().unwrap().to_string(),
                r["code"].as_str().unwrap().to_string(),
                r["message"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

/// 這個 Workspace 因跨登入而被暫停的事件數（直接讀控制資料庫）。
fn login_pause_events(harness: &Harness, ws: &str) -> i64 {
    harness
        .db()
        .query_row(
            "SELECT COUNT(*) FROM event WHERE aggregate_id = ?1 AND event_type = 'workspace_paused_by_login'",
            [ws],
            |r| r.get(0),
        )
        .unwrap()
}

/// 跨登入後的崩潰恢復處置完成（登出時 Runtime 被終止，下一次啟動先核對再續跑）。
async fn finish_recovery(harness: &Harness) {
    harness.supervise().await;
    let observing: Value = harness.get_json(RECOVERY).await;
    assert_eq!(observing["event"]["status"], "observing", "{observing}");
}

// ============================================================ 設定

#[tokio::test]
async fn both_new_settings_default_to_off_and_are_kept_and_shown_separately_from_the_exit_setting() {
    let harness = start().await;
    let (ws, goal) = workspace(&harness, "A").await;
    let (_, rid) = running_task(&harness, &ws, &goal).await;

    // 預設都關閉；兩個 Workspace 設定各自顯示
    let status = lifecycle(&harness).await;
    assert_eq!(status["login_startup"], json!({ "enabled": false, "registered": false }));
    let view = workspace_view(&harness, &ws).await;
    assert_eq!((view["auto_resume_after_exit"].as_bool(), view["auto_resume_across_login"].as_bool()), (Some(false), Some(false)));
    assert_eq!(view["paused_by"], Value::Null);

    // 只開跨登入：完全退出後的設定不變；只開完全退出後：跨登入設定不變
    let view = set_across_login(&harness, &ws, true).await;
    assert_eq!((view["auto_resume_after_exit"].as_bool(), view["auto_resume_across_login"].as_bool()), (Some(false), Some(true)));
    let view = set_after_exit(&harness, &ws, true).await;
    assert_eq!((view["auto_resume_after_exit"].as_bool(), view["auto_resume_across_login"].as_bool()), (Some(true), Some(true)));
    let view = set_across_login(&harness, &ws, false).await;
    assert_eq!((view["auto_resume_after_exit"].as_bool(), view["auto_resume_across_login"].as_bool()), (Some(true), Some(false)));

    // 兩個設定分開保存（各自的欄位與事件）
    let (exit_flag, login_flag): (bool, bool) = harness
        .db()
        .query_row(
            "SELECT auto_resume_after_exit, auto_resume_across_login FROM workspace_lifecycle WHERE workspace_id = ?1",
            [&ws],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((exit_flag, login_flag), (true, false));
    let login_events: i64 = harness
        .db()
        .query_row(
            "SELECT COUNT(*) FROM event WHERE aggregate_id = ?1 AND event_type = 'workspace_login_resume_changed'",
            [&ws],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(login_events, 2);

    // 變更設定不會立即啟停目前的工作
    harness.supervise().await;
    assert_eq!(run_view(&harness, &ws, &rid).await["status"], "running");
    assert!(harness.process_control.stop_calls().is_empty());
    assert_eq!(harness.process_control.spawn_count(), 1);

    // 不存在的 Workspace
    let body = json!({ "auto_resume_across_login": true });
    let (status, body) = harness.send_json(Method::PUT, &format!("{WORKSPACES}/ws_missing/login-resume"), Some(body)).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
}

#[tokio::test]
async fn login_startup_follows_the_preference_in_windows_and_a_failed_registration_keeps_the_preference() {
    let mut harness = start().await;

    let (status, view) = set_login_startup(&harness, true).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(view, json!({ "enabled": true, "registered": true }));
    assert!(harness.startup_registration.registered());
    assert_eq!(lifecycle(&harness).await["login_startup"], json!({ "enabled": true, "registered": true }));

    // 登記失敗：偏好不變，回報原因
    harness.startup_registration.fail_next("存取被拒");
    let (status, body) = set_login_startup(&harness, false).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    assert_eq!(body["error"], "login_startup_registration_failed");
    assert!(body["message"].as_str().unwrap().contains("存取被拒"), "{body}");
    assert_eq!(lifecycle(&harness).await["login_startup"]["enabled"], true);
    assert!(harness.startup_registration.registered());

    // 登記被移除（例如使用者手動刪掉登錄值）：下次啟動依偏好補回
    harness.startup_registration.force(false);
    assert_eq!(lifecycle(&harness).await["login_startup"], json!({ "enabled": true, "registered": false }));
    harness.restart().await;
    assert!(harness.startup_registration.registered());

    let (status, view) = set_login_startup(&harness, false).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(view, json!({ "enabled": false, "registered": false }));
    assert_eq!(harness.startup_registration.changes(), vec![true, true, false]);

    // 沒有登記時，Windows 登入不執行任何東西
    harness.sign_in_again().await;
    assert_eq!(harness.windows_login_startup().await, None);
    assert!(!harness.runtime_running());
}

// ============================================================ 完全退出優先

#[tokio::test]
async fn after_full_exit_signing_in_again_starts_no_runtime_guardian_or_work() {
    let mut harness = start().await;
    let (ws, goal) = workspace(&harness, "A").await;
    let (_, rid) = running_task(&harness, &ws, &goal).await;
    // 跨登入自動續跑開著、完全退出後自動續跑關著：完全退出後只能依後者
    set_across_login(&harness, &ws, true).await;
    let (status, body) = set_login_startup(&harness, true).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    harness.start_guardian();

    full_exit(&mut harness).await;
    let spawned = harness.process_control.spawn_count();
    harness.kill_guardian();
    harness.sign_in_again().await;

    // 登入啟動入口讀到停止抑制：不啟動 Runtime、不當監護
    let run = harness.windows_login_startup().await.expect("登入啟動已登記");
    assert_eq!(run.launch, LoginLaunch::Suppressed(Suppression::FullExit));
    assert_eq!(run.runtime_outcome, None, "沒有啟動任何 Runtime 程序");
    assert!(!run.guards);
    assert!(!harness.runtime_running());
    assert!(!harness.guardian_running());
    assert_eq!(harness.process_control.spawn_count(), spawned, "沒有任何 Worker 被拉起");
    assert!(harness.startup_registration.registered(), "完全退出不改變登入啟動的偏好（停止抑制另存）");

    // 再登入一次也一樣
    harness.sign_in_again().await;
    assert_eq!(harness.windows_login_startup().await.unwrap().launch, LoginLaunch::Suppressed(Suppression::FullExit));
    assert!(!harness.runtime_running());

    // 只有手動開 App 才依 DR-01：完全退出後自動續跑關著，所以等「繼續工作」，跨登入設定不能繞過
    harness.open_app().await;
    harness.supervise().await;
    let view = workspace_view(&harness, &ws).await;
    assert_eq!(view["awaiting_resume"], true, "{view}");
    assert_eq!(view["paused_by"], "full_exit");
    assert_eq!(harness.process_control.spawn_count(), spawned);
    assert_ne!(run_view(&harness, &ws, &rid).await["status"], "running");
    assert_eq!(login_pause_events(&harness, &ws), 0, "停止意圖生效時不做跨登入處置");
    assert_eq!(lifecycle(&harness).await["login_startup"]["enabled"], true);
}

#[tokio::test]
async fn a_stop_intent_only_in_the_database_still_keeps_a_login_start_from_serving() {
    let mut harness = start().await;
    let (ws, goal) = workspace(&harness, "A").await;
    running_task(&harness, &ws, &goal).await;
    set_across_login(&harness, &ws, true).await;
    set_login_startup(&harness, true).await;
    full_exit(&mut harness).await;
    // 生命週期檔遺失（例如寫入失敗）：啟動入口從檔案看不出已完全退出
    std::fs::remove_file(harness.lifecycle_file_path()).unwrap();
    let spawned = harness.process_control.spawn_count();
    harness.sign_in_again().await;

    let run = harness.windows_login_startup().await.unwrap();
    assert_eq!(run.launch, LoginLaunch::Started);
    let outcome = run.runtime_outcome.expect("啟動入口啟動了 Runtime 程序");
    assert!(outcome.as_ref().is_err_and(|e| e.contains("停止意圖")), "{outcome:?}");
    assert!(!run.guards, "沒有 Runtime 可監看，不當監護");
    assert!(!harness.runtime_running(), "完全退出優先：登入啟動的 Runtime 不提供服務");
    assert_eq!(harness.process_control.spawn_count(), spawned);
    assert!(
        std::fs::read_to_string(harness.lifecycle_file_path()).unwrap().contains("full_exit"),
        "Runtime 把資料庫的停止意圖補回生命週期檔"
    );
    // 下一次登入就由檔案直接抑制
    harness.sign_in_again().await;
    assert_eq!(harness.windows_login_startup().await.unwrap().launch, LoginLaunch::Suppressed(Suppression::FullExit));
}

#[tokio::test]
async fn maintenance_suppresses_the_login_start() {
    let mut harness = start().await;
    set_login_startup(&harness, true).await;
    harness.sign_in_again().await;
    let file = json!({
        "version": 1, "stop": null, "updated_at": 1,
        "maintenance": { "kind": "update", "requested_at": 1 },
    });
    std::fs::write(harness.lifecycle_file_path(), file.to_string()).unwrap();

    let run = harness.windows_login_startup().await.unwrap();
    assert_eq!(run.launch, LoginLaunch::Suppressed(Suppression::Maintenance("update".into())));
    assert!(!harness.runtime_running());
}

// ============================================================ 跨登入續跑

#[tokio::test]
async fn signing_in_again_resumes_only_workspaces_with_cross_login_resume() {
    let mut harness = start().await;
    // A：跨登入自動續跑開、完全退出後關；B：相反
    let (a, goal_a) = workspace(&harness, "A").await;
    let (b, goal_b) = workspace(&harness, "B").await;
    let (task_a, run_a) = running_task(&harness, &a, &goal_a).await;
    let (task_b, run_b) = running_task(&harness, &b, &goal_b).await;
    set_across_login(&harness, &a, true).await;
    set_after_exit(&harness, &b, true).await;
    set_login_startup(&harness, true).await;
    harness.start_guardian();
    let boots = boot_count(&harness).await;

    harness.sign_in_again().await;
    let run = harness.windows_login_startup().await.unwrap();
    assert_eq!(run.launch, LoginLaunch::Started);
    assert_eq!(run.runtime_outcome, Some(Ok(())));
    assert!(run.guards, "啟動入口接著當監護");
    assert!(harness.guardian_running());
    assert_eq!(boot_count(&harness).await, boots + 1);

    // 只有 B 被暫停（它的「完全退出後自動續跑」與跨登入無關）
    let view_a = workspace_view(&harness, &a).await;
    assert_eq!((view_a["awaiting_resume"].as_bool(), view_a["paused_by"].clone()), (Some(false), Value::Null));
    let view_b = workspace_view(&harness, &b).await;
    assert_eq!((view_b["awaiting_resume"].as_bool(), view_b["paused_by"].as_str()), (Some(true), Some("new_login")));

    // 先核對（停下上一次登入留下的程序），再續跑：A 以新 Run 恢復，B 的工作排隊等「繼續工作」
    finish_recovery(&harness).await;
    assert_eq!(run_view(&harness, &a, &run_a).await["status"], "interrupted");
    assert_eq!(run_view(&harness, &b, &run_b).await["status"], "interrupted");
    let runs_a = runs_of(&harness, &a, &task_a).await;
    assert_eq!(runs_a.len(), 2, "{runs_a:?}");
    assert_eq!(runs_a[1]["status"], "running");
    assert_eq!(runs_of(&harness, &b, &task_b).await.len(), 1, "B 沒有新的 Run");
    assert_eq!(task_status(&harness, &b, &task_b).await, "ready");
    let reasons = queue_reasons(&harness, &task_b).await;
    assert_eq!((reasons[0].0.as_str(), reasons[0].1.as_str()), ("lifecycle", "awaiting_resume"), "{reasons:?}");
    assert!(reasons[0].2.contains("跨登入"), "{reasons:?}");

    // 同一次登入內重新開 App（或重啟）不會再暫停任何東西
    harness.restart().await;
    harness.supervise().await;
    assert_eq!(workspace_view(&harness, &a).await["awaiting_resume"], false);
    assert_eq!(login_pause_events(&harness, &a), 0);
    assert_eq!(login_pause_events(&harness, &b), 1);

    // 使用者按「繼續工作」後 B 才續跑
    let (status, body) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{b}/resume"), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["paused_by"], Value::Null);
    harness.supervise().await;
    let runs_b = runs_of(&harness, &b, &task_b).await;
    assert_eq!(runs_b.len(), 2, "{runs_b:?}");
    assert_eq!(runs_b[1]["status"], "running");
}

#[tokio::test]
async fn manually_opening_the_app_after_signing_in_again_applies_the_same_cross_login_settings() {
    let mut harness = start().await;
    let (a, _) = workspace(&harness, "A").await;
    let (b, _) = workspace(&harness, "B").await;
    set_across_login(&harness, &a, true).await;
    // 登入啟動沒有開：重新登入後沒有東西自動啟動，使用者手動開 App
    harness.sign_in_again().await;
    assert_eq!(harness.windows_login_startup().await, None);
    assert!(!harness.runtime_running());

    harness.open_app().await;
    assert_eq!(workspace_view(&harness, &a).await["awaiting_resume"], false);
    assert_eq!(workspace_view(&harness, &b).await["paused_by"], "new_login");
}

#[tokio::test]
async fn an_unverified_login_resumes_no_workspace_regardless_of_settings() {
    let mut harness = start().await;
    let (a, _) = workspace(&harness, "A").await;
    set_across_login(&harness, &a, true).await;

    // 取不到登入識別：無法核對這次啟動是否跨了登入
    harness.login_session.set(None);
    harness.restart().await;
    let view = workspace_view(&harness, &a).await;
    assert_eq!((view["awaiting_resume"].as_bool(), view["paused_by"].as_str()), (Some(true), Some("login_unverified")));
    assert!(harness.log_text().contains("無法核對"), "{}", harness.log_text());
}

// ============================================================ 單一控制平面

#[tokio::test]
async fn login_startup_and_a_manual_app_open_at_the_same_time_leave_one_control_plane() {
    let mut harness = start().await;
    let (a, _) = workspace(&harness, "A").await;
    set_login_startup(&harness, true).await;
    let boots = boot_count(&harness).await;

    harness.sign_in_again().await;
    harness.open_app_during_next_login_start();
    let run = harness.windows_login_startup().await.unwrap();

    // 啟動入口確實啟動了程序，但 App 的 Runtime 先取得單實例鎖：多出來的那個沒開資料庫就結束
    assert_eq!(run.launch, LoginLaunch::Started);
    assert!(run.runtime_outcome.as_ref().is_some_and(|o| o.as_ref().is_err_and(|e| e.contains("單實例"))), "{run:?}");
    assert!(run.guards, "啟動入口改為監護 App 啟動的那個 Runtime");
    assert!(harness.runtime_running());
    assert_eq!(boot_count(&harness).await, boots + 1, "只啟動了一個 Runtime");
    assert_eq!(login_pause_events(&harness, &a), 1, "跨登入處置只做一次");

    // 之後 App 再連線（或再開一次）都連回同一個
    harness.open_app().await;
    assert_eq!(boot_count(&harness).await, boots + 1);
    // 已有 Runtime 與監護時，啟動入口再跑一次也不啟動第二個、不當第二個監護
    let again = harness.windows_login_startup().await.unwrap();
    assert_eq!((again.launch, again.runtime_outcome, again.guards), (LoginLaunch::AlreadyRunning, None, false));
    assert_eq!(boot_count(&harness).await, boots + 1);
}

// ============================================================ 不重設恢復額度與阻擋

#[tokio::test]
async fn signing_in_again_does_not_reset_recovery_attempts_or_blocks() {
    let mut harness = start().await;
    let (ws, goal) = workspace(&harness, "A").await;
    let (tid, old) = running_task(&harness, &ws, &goal).await;
    set_across_login(&harness, &ws, true).await;
    set_login_startup(&harness, true).await;
    // 舊程序停不下來：三次恢復嘗試都失敗，額度用盡
    let old_identity = identity(&harness, &ws, &old).await;
    harness.process_control.script_stop(&old_identity, FakeStop::Result(StopResult::StillAlive));
    harness.crash_restart().await;
    harness.supervise().await;
    harness.clock.advance_millis(30_000);
    harness.supervise().await;
    harness.clock.advance_millis(120_000);
    harness.supervise().await;
    let exhausted: Value = harness.get_json(RECOVERY).await;
    assert_eq!(exhausted["event"]["status"], "exhausted", "{exhausted}");
    let stops = harness.process_control.stop_calls().len();

    // 重新登入並由登入啟動拉起：恢復事件、次數與阻擋都照舊，不做第四次、不派工
    harness.sign_in_again().await;
    let run = harness.windows_login_startup().await.unwrap();
    assert_eq!(run.runtime_outcome, Some(Ok(())));
    harness.clock.advance_millis(3_600_000);
    harness.supervise().await;
    let after: Value = harness.get_json(RECOVERY).await;
    let event = &after["event"];
    assert_eq!(
        (event["id"].clone(), event["status"].as_str(), event["attempts_used"].as_i64(), event["round"].as_i64()),
        (exhausted["event"]["id"].clone(), Some("exhausted"), Some(3), Some(1)),
        "{after}"
    );
    assert_eq!(after["blocking"]["code"], "crash_recovery_exhausted");
    assert_eq!(harness.process_control.stop_calls().len(), stops);
    assert_eq!(harness.process_control.spawn_count(), 1);
    assert_eq!(task_status(&harness, &ws, &tid).await, "running", "舊 Run 仍持有寫入所有權");
}
