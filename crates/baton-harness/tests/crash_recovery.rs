//! 情境測試：Runtime 意外崩潰後的自動恢復（票 31，ADR-0005），經由 Runtime harness 的真實 HTTP API 驗證。
//!
//! 「意外崩潰」= `crash_restart`：丟棄 Runtime 而不記下正常結束，再以同一份資料目錄與同一組 fake 重新建立。
//! 要在某個邊界當掉，先讓那一步永遠不回來（`FakeStop::Hang`、`FakeStop::HangAfterStop`、`hang_next_spawn_at`），
//! 等 Supervisor 卡在那裡之後丟棄進行中的 `supervise`，再 `crash_restart`。時間全由 fake Clock 決定。
//! 只斷言外部行為：恢復狀態端點、Run 與 Task、排程的等待原因、Port 被呼叫的方式（spawn／stop 與仍存活的程序）。
//!
//! 每個測試名稱對應票 31 的驗收條件：
//! - 在嘗試保存、舊程序停止、新程序啟動等邊界中斷，不突破三次上限、不重複建立 writer：`crash_*_boundary_*`
//! - 退避期間重建 Runtime 不跳過期限：`rebuilding_during_backoff_does_not_skip_the_deadline`
//! - 觀察期內再崩潰沿用原事件與計數：`crash_during_observation_*`
//! - 三次失敗後重建或等待都不產生第四次；人工重試建立新輪次並保留歷史：`three_failures_*`、`manual_retry_*`

use std::time::Duration;

use baton_domain::Timestamp;
use baton_harness::fakes::{FakeStop, HangPoint};
use baton_harness::Harness;
use baton_ports::{ProcessIdentity, StopResult};
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

const WORKSPACES: &str = "/api/v1/workspaces";
const SCHEDULING: &str = "/api/v1/scheduling";
const RECOVERY: &str = "/api/v1/recovery";
const RECOVERY_RETRY: &str = "/api/v1/recovery/retry";

/// 2023-11-14 22:13:20.123 UTC
const CLOCK_START: Timestamp = Timestamp::from_millis(1_700_000_000_123);

// --- 共用輔助 ---

async fn start() -> Harness {
    let harness = Harness::builder().clock_at(CLOCK_START).start().await;
    harness.add_account("帳號 1", "a@example.com").await;
    harness.add_account("帳號 2", "b@example.com").await;
    harness
}

fn filled(text: &str) -> Value {
    json!({ "status": "filled", "text": text })
}

fn contract(capabilities: &[&str]) -> Value {
    json!({
        "objective": filled("整理資料"),
        "inputs": filled("既有的資料"),
        "scope": filled("公開資料"),
        "deliverables": filled("比較表"),
        "done_criteria": filled("五個產品"),
        "blocker_handling": filled("卡住時回報"),
        "required_capabilities": capabilities,
    })
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

async fn ready_task(harness: &Harness, ws: &str, goal: &str, title: &str, capabilities: &[&str]) -> String {
    let body = json!({ "goal_id": goal, "title": title, "contract": contract(capabilities) });
    let (status, task) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks"), Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    let tid = task["id"].as_str().unwrap().to_string();
    let (status, ready) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks/{tid}/ready"), None).await;
    assert_eq!(status, StatusCode::OK, "{ready}");
    tid
}

async fn dispatch(harness: &Harness, ws: &str, tid: &str) -> (StatusCode, Value) {
    harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks/{tid}/dispatch"), None).await
}

/// 建立 ready 的 Task、派工並讓 Supervisor 啟動它，回傳 (task_id, run_id)。
async fn running_task(harness: &Harness, ws: &str, goal: &str, title: &str) -> (String, String) {
    let tid = ready_task(harness, ws, goal, title, &[]).await;
    let (status, run) = dispatch(harness, ws, &tid).await;
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

/// 這個 Run 的程序身分（fake ProcessControl 以 command_id 登記）。
async fn identity(harness: &Harness, ws: &str, rid: &str) -> ProcessIdentity {
    let run = run_view(harness, ws, rid).await;
    harness.process_control.identity_of(run["command_id"].as_str().unwrap()).unwrap()
}

async fn recovery(harness: &Harness) -> Value {
    harness.get_json(RECOVERY).await
}

async fn queue_reasons(harness: &Harness, tid: &str) -> Vec<(String, String)> {
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
        .map(|r| (r["category"].as_str().unwrap().to_string(), r["code"].as_str().unwrap().to_string()))
        .collect()
}

/// 以範本確認（或修改）Workspace 授權。
async fn confirm_template(harness: &Harness, ws: &str, template_id: &str) {
    let path = format!("{WORKSPACES}/{ws}/authorization");
    let (_, view) = harness.send_json(Method::GET, &path, None).await;
    let found = view["templates"].as_array().unwrap().iter().find(|t| t["id"] == template_id).unwrap().clone();
    let body = json!({
        "template_id": template_id, "grants": found["grants"], "acknowledged_risk": true, "acknowledged_expansion": true,
    });
    let (status, revision) = harness.send_json(Method::PUT, &path, Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{revision}");
}

/// 讓 Supervisor 處理一次，直到 `stuck` 成立（它卡在注入的邊界）就丟棄進行中的處理——等同 Runtime 在那一點當掉。
async fn supervise_until_stuck(harness: &Harness, stuck: impl Fn() -> bool) {
    let tick = harness.supervise();
    tokio::pin!(tick);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        tokio::select! {
            _ = &mut tick => panic!("Supervisor 應該卡在注入的邊界"),
            _ = tokio::time::sleep(Duration::from_millis(10)) => {
                if stuck() {
                    return;
                }
                assert!(tokio::time::Instant::now() < deadline, "等不到 Supervisor 卡在注入的邊界");
            }
        }
    }
}

fn now(harness: &Harness) -> i64 {
    use baton_ports::Clock;
    harness.clock.now().as_millis()
}

// ============================================================ 正常路徑

#[tokio::test]
async fn a_crash_stops_the_identified_old_worker_and_resumes_the_task_with_a_new_run_after_checks() {
    let mut harness = start().await;
    let (ws, goal) = workspace(&harness, "A").await;
    let (tid, old) = running_task(&harness, &ws, &goal, "長任務").await;
    let old_identity = identity(&harness, &ws, &old).await;
    assert_eq!(recovery(&harness).await["event"], Value::Null, "沒有崩潰就沒有恢復事件");

    harness.crash_restart().await;

    // 啟動時先盤點：建立恢復事件，第一次嘗試完成前不派工、也還沒碰任何程序
    let pending = recovery(&harness).await;
    let event = &pending["event"];
    assert_eq!((event["status"].as_str(), event["round"].as_i64(), event["attempts_used"].as_i64()), (Some("waiting"), Some(1), Some(0)));
    assert_eq!(pending["max_attempts"], 3);
    assert_eq!(pending["blocking"]["code"], "crash_recovery_pending", "{pending}");
    assert!(harness.process_control.stop_calls().is_empty());

    harness.supervise().await;

    // 舊程序受控停止（身分已確認），舊 Run 結束；Task 以新 Run 恢復
    assert_eq!(harness.process_control.stop_calls(), [old_identity.clone()]);
    let stopped = run_view(&harness, &ws, &old).await;
    assert_eq!(stopped["status"], "interrupted", "{stopped}");
    assert_eq!(stopped["end_reason"], "stopped_by_crash_recovery");
    let runs = runs_of(&harness, &ws, &tid).await;
    assert_eq!(runs.len(), 2, "{runs:?}");
    assert_eq!(runs[1]["status"], "running");
    assert_eq!(task_status(&harness, &ws, &tid).await, "running");
    assert_eq!(harness.process_control.spawn_count(), 2);
    let alive = harness.process_control.alive();
    assert_eq!(alive.len(), 1, "同一時間只有一個 writer");
    assert_ne!(alive[0], old_identity);

    // 處置完成：穩定觀察 10 分鐘，期間可以派工；受影響工作可在恢復介面看到
    let observing = recovery(&harness).await;
    let event = &observing["event"];
    assert_eq!((event["status"].as_str(), event["attempts_used"].as_i64()), (Some("observing"), Some(1)));
    assert_eq!(event["observe_until"].as_i64(), Some(now(&harness) + 600_000));
    assert_eq!(observing["blocking"], Value::Null);
    let affected = &event["attempts"][0]["affected"];
    assert_eq!(affected, &json!([{ "run_id": old, "workspace_id": ws, "task_id": tid, "disposition": "requeued", "message": null }]));

    harness.clock.advance_millis(599_999);
    harness.supervise().await;
    assert_eq!(recovery(&harness).await["event"]["status"], "observing", "未滿 10 分鐘不算成功");
    harness.clock.advance_millis(1);
    harness.supervise().await;
    let done = recovery(&harness).await;
    assert_eq!(done["event"]["status"], "succeeded", "{done}");
    assert_eq!(done["event"]["closed_at"].as_i64(), Some(now(&harness)));
    assert_eq!(done["event"]["attempts"][0]["outcome"], "succeeded");
}

#[tokio::test]
async fn a_clean_restart_is_not_a_crash_and_keeps_the_running_worker() {
    let mut harness = start().await;
    let (ws, goal) = workspace(&harness, "A").await;
    let (_, rid) = running_task(&harness, &ws, &goal, "長任務").await;

    harness.restart().await;
    harness.supervise().await;

    assert_eq!(recovery(&harness).await["event"], Value::Null);
    assert!(harness.process_control.stop_calls().is_empty());
    assert_eq!(run_view(&harness, &ws, &rid).await["status"], "running");
}

#[tokio::test]
async fn a_stop_intent_takes_priority_and_a_crash_after_full_exit_is_not_recovered_automatically() {
    let mut harness = start().await;
    let (ws, goal) = workspace(&harness, "A").await;
    running_task(&harness, &ws, &goal, "長任務").await;
    let (status, body) = harness.send_json(Method::POST, "/api/v1/lifecycle/exit", None).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    harness.wait_for_exit().await;

    harness.crash_restart().await;
    harness.supervise().await;

    assert_eq!(recovery(&harness).await["event"], Value::Null, "完全退出的停止意圖優先：走重新開啟，不是崩潰恢復");
}

#[tokio::test]
async fn the_new_run_is_rechecked_against_the_current_authorization() {
    let mut harness = start().await;
    let (ws, goal) = workspace(&harness, "A").await;
    confirm_template(&harness, &ws, "trusted_dev_p3").await;
    let tid = ready_task(&harness, &ws, &goal, "跑建置", &["run_build_test"]).await;
    let (status, run) = dispatch(&harness, &ws, &tid).await;
    assert_eq!(status, StatusCode::CREATED, "{run}");
    harness.supervise().await;
    assert_eq!(harness.process_control.spawn_count(), 1);

    harness.crash_restart().await;
    // 崩潰期間使用者撤銷了建置權限（恢復完成前就生效）
    confirm_template(&harness, &ws, "research_docs").await;
    harness.supervise().await;

    assert_eq!(recovery(&harness).await["event"]["status"], "observing");
    assert_eq!(harness.process_control.spawn_count(), 1, "恢復的新 Run 依目前的授權判定，不沿用舊 Run 的准入");
    assert_eq!(task_status(&harness, &ws, &tid).await, "ready");
    assert_eq!(queue_reasons(&harness, &tid).await[0].0, "authorization");
}

#[tokio::test]
async fn a_cancel_requested_before_the_crash_is_honored_instead_of_resumed() {
    let mut harness = start().await;
    let (ws, goal) = workspace(&harness, "A").await;
    let (tid, rid) = running_task(&harness, &ws, &goal, "要取消的工作").await;
    let (status, body) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/runs/{rid}/cancel"), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    harness.crash_restart().await;
    harness.supervise().await;

    assert_eq!(run_view(&harness, &ws, &rid).await["status"], "cancelled");
    // 與一般的取消相同：Task 回到 ready，但不重新排隊，不以新 Run 恢復
    assert_eq!(task_status(&harness, &ws, &tid).await, "ready");
    let overview: Value = harness.get_json(SCHEDULING).await;
    assert!(overview["queue"].as_array().unwrap().iter().all(|e| e["task_id"] != tid), "{overview}");
    assert_eq!(harness.process_control.spawn_count(), 1, "取消意圖優先，不恢復");
    let event = recovery(&harness).await["event"].clone();
    assert_eq!(event["attempts"][0]["affected"][0]["disposition"], "cancelled", "{event}");
}

// ============================================================ 邊界中斷

#[tokio::test]
async fn crash_at_the_attempt_saved_boundary_counts_the_attempt_and_waits_thirty_seconds() {
    let mut harness = start().await;
    let (ws, goal) = workspace(&harness, "A").await;
    let (tid, old) = running_task(&harness, &ws, &goal, "長任務").await;
    let old_identity = identity(&harness, &ws, &old).await;
    harness.crash_restart().await;

    // 嘗試序號已保存、舊程序還沒停下時當掉
    harness.process_control.script_stop(&old_identity, FakeStop::Hang);
    supervise_until_stuck(&harness, || !harness.process_control.stop_calls().is_empty()).await;
    harness.crash_restart().await;

    let after = recovery(&harness).await["event"].clone();
    assert_eq!((after["status"].as_str(), after["attempts_used"].as_i64()), (Some("waiting"), Some(1)), "{after}");
    assert_eq!(after["attempts"][0]["outcome"], "failed");
    assert_eq!(after["next_attempt_at"].as_i64(), Some(now(&harness) + 30_000));
    assert_eq!(harness.process_control.spawn_count(), 1, "沒有建立新的 writer");

    // 退避期滿後的第二次嘗試完成恢復
    harness.process_control.script_stop(&old_identity, FakeStop::Result(StopResult::StoppedGracefully));
    harness.clock.advance_millis(30_000);
    harness.supervise().await;
    let event = recovery(&harness).await["event"].clone();
    assert_eq!((event["status"].as_str(), event["attempts_used"].as_i64()), (Some("observing"), Some(2)), "{event}");
    assert_eq!(event["attempts"][1]["seq"], 2);
    assert_eq!(task_status(&harness, &ws, &tid).await, "running");
    assert_eq!(harness.process_control.alive().len(), 1);
}

#[tokio::test]
async fn crash_at_the_old_worker_stopped_boundary_does_not_create_a_second_writer() {
    let mut harness = start().await;
    let (ws, goal) = workspace(&harness, "A").await;
    let (tid, old) = running_task(&harness, &ws, &goal, "長任務").await;
    let old_identity = identity(&harness, &ws, &old).await;
    harness.crash_restart().await;

    // 舊程序已停止、結果還沒寫回時當掉
    harness.process_control.script_stop(&old_identity, FakeStop::HangAfterStop);
    supervise_until_stuck(&harness, || !harness.process_control.stop_calls().is_empty()).await;
    harness.crash_restart().await;
    assert!(harness.process_control.alive().is_empty());
    assert_eq!(run_view(&harness, &ws, &old).await["status"], "running", "資料庫還不知道它已停止");
    assert_eq!(harness.process_control.spawn_count(), 1);

    // 第二次嘗試：程序已不在，停止立即回報已停止，Run 結束後才以新 Run 恢復
    harness.process_control.script_stop(&old_identity, FakeStop::Result(StopResult::StoppedGracefully));
    harness.clock.advance_millis(30_000);
    harness.supervise().await;
    assert_eq!(run_view(&harness, &ws, &old).await["status"], "interrupted");
    let runs = runs_of(&harness, &ws, &tid).await;
    assert_eq!(runs.len(), 2, "{runs:?}");
    assert_eq!(harness.process_control.spawn_count(), 2);
    assert_eq!(harness.process_control.alive().len(), 1);
    assert_eq!(recovery(&harness).await["event"]["attempts_used"], 2);
}

#[tokio::test]
async fn crash_at_the_new_worker_started_boundary_reuses_the_event_and_never_runs_two_writers() {
    let mut harness = start().await;
    let (ws, goal) = workspace(&harness, "A").await;
    let (tid, _) = running_task(&harness, &ws, &goal, "長任務").await;
    harness.crash_restart().await;

    // 第一次嘗試完成、新 Run 的程序已啟動但回執還沒寫回時當掉
    harness.process_control.hang_next_spawn_at(HangPoint::AfterSpawn);
    supervise_until_stuck(&harness, || harness.process_control.spawn_count() == 2).await;
    let first = recovery(&harness).await["event"].clone();
    harness.crash_restart().await;

    // 觀察中再崩潰：沿用原事件與已用次數，第一次嘗試記為失敗
    let event = recovery(&harness).await["event"].clone();
    assert_eq!(event["id"], first["id"]);
    assert_eq!((event["status"].as_str(), event["attempts_used"].as_i64()), (Some("waiting"), Some(1)), "{event}");
    assert_eq!(harness.process_control.alive().len(), 1, "只有新程序存活");

    harness.clock.advance_millis(30_000);
    harness.supervise().await;

    // 第二次嘗試先以 command_id 找回啟動中的新程序，同樣受控停止後才再建立一個 Run
    let runs = runs_of(&harness, &ws, &tid).await;
    assert_eq!(runs.len(), 3, "{runs:?}");
    assert_eq!(runs[1]["status"], "interrupted");
    assert_eq!(runs[2]["status"], "running");
    assert_eq!(harness.process_control.spawn_count(), 3);
    assert_eq!(harness.process_control.alive().len(), 1, "同一時間只有一個 writer");
    let event = recovery(&harness).await["event"].clone();
    assert_eq!((event["status"].as_str(), event["attempts_used"].as_i64()), (Some("observing"), Some(2)));
}

#[tokio::test]
async fn crash_during_observation_keeps_the_event_and_count() {
    let mut harness = start().await;
    let (ws, goal) = workspace(&harness, "A").await;
    running_task(&harness, &ws, &goal, "長任務").await;
    harness.crash_restart().await;
    harness.supervise().await;
    let first = recovery(&harness).await["event"].clone();
    assert_eq!(first["status"], "observing");

    harness.clock.advance_millis(5 * 60_000);
    harness.crash_restart().await;

    let event = recovery(&harness).await["event"].clone();
    assert_eq!(event["id"], first["id"], "沿用原事件");
    assert_eq!((event["status"].as_str(), event["attempts_used"].as_i64()), (Some("waiting"), Some(1)));
    assert_eq!(event["next_attempt_at"].as_i64(), Some(now(&harness) + 30_000));
    assert_eq!(event["attempts"][0]["outcome"], "failed");
    assert!(event["attempts"][0]["failure"].as_str().unwrap().contains("穩定觀察"), "{event}");
}

#[tokio::test]
async fn rebuilding_during_backoff_does_not_skip_the_deadline() {
    let mut harness = start().await;
    let (ws, goal) = workspace(&harness, "A").await;
    let (_, old) = running_task(&harness, &ws, &goal, "長任務").await;
    let old_identity = identity(&harness, &ws, &old).await;
    harness.process_control.script_stop(&old_identity, FakeStop::Result(StopResult::StillAlive));
    harness.crash_restart().await;
    harness.supervise().await;
    let deadline = recovery(&harness).await["event"]["next_attempt_at"].as_i64().unwrap();
    assert_eq!(deadline, now(&harness) + 30_000);
    assert_eq!(harness.process_control.stop_calls().len(), 1);

    // 退避期間崩潰重建、也正常重啟一次：期限不變、不計次、不嘗試
    harness.clock.advance_millis(10_000);
    harness.crash_restart().await;
    harness.supervise().await;
    harness.clock.advance_millis(10_000);
    harness.restart().await;
    harness.supervise().await;
    harness.clock.advance_millis(9_999);
    harness.supervise().await;
    let event = recovery(&harness).await["event"].clone();
    assert_eq!((event["attempts_used"].as_i64(), event["next_attempt_at"].as_i64()), (Some(1), Some(deadline)));
    assert_eq!(harness.process_control.stop_calls().len(), 1, "期限前沒有第二次嘗試");

    harness.clock.advance_millis(1);
    harness.supervise().await;
    assert_eq!(harness.process_control.stop_calls().len(), 2);
    assert_eq!(recovery(&harness).await["event"]["attempts_used"], 2);
}

// ============================================================ 用盡與人工重試

#[tokio::test]
async fn three_failures_block_and_neither_rebuilding_nor_waiting_produces_a_fourth_attempt() {
    let mut harness = start().await;
    let (ws, goal) = workspace(&harness, "A").await;
    let (tid, old) = running_task(&harness, &ws, &goal, "停不下來的工作").await;
    let old_identity = identity(&harness, &ws, &old).await;
    harness.process_control.script_stop(&old_identity, FakeStop::Result(StopResult::StillAlive));
    harness.crash_restart().await;

    harness.supervise().await;
    harness.clock.advance_millis(30_000);
    harness.supervise().await;
    let second = recovery(&harness).await["event"].clone();
    assert_eq!(second["next_attempt_at"].as_i64(), Some(now(&harness) + 120_000), "第三次前等 2 分鐘");
    harness.clock.advance_millis(120_000);
    harness.supervise().await;

    let exhausted = recovery(&harness).await;
    let event = &exhausted["event"];
    assert_eq!((event["status"].as_str(), event["attempts_used"].as_i64()), (Some("exhausted"), Some(3)), "{exhausted}");
    assert_eq!(exhausted["can_retry"], true);
    assert_eq!(exhausted["blocking"]["code"], "crash_recovery_exhausted");
    assert!(event["last_failure"].as_str().unwrap().contains("無法確認停止"), "{event}");
    assert_eq!(event["attempts"][2]["affected"][0]["disposition"], "unconfirmed");
    assert_eq!(harness.process_control.stop_calls().len(), 3);
    assert_eq!(harness.process_control.spawn_count(), 1, "無法確認停止就不建立新 Run");
    assert_eq!(run_view(&harness, &ws, &old).await["status"], "running", "舊 Run 保留寫入所有權");

    // 重建、正常重啟、等待都不會產生第四次；新的工作也不派出
    harness.crash_restart().await;
    harness.clock.advance_millis(3_600_000);
    harness.supervise().await;
    harness.restart().await;
    harness.clock.advance_millis(3_600_000);
    harness.supervise().await;
    let other = ready_task(&harness, &ws, &goal, "新的工作", &[]).await;
    let (status, queued) = dispatch(&harness, &ws, &other).await;
    assert!(status.is_success(), "{queued}");
    harness.supervise().await;
    assert_eq!(harness.process_control.stop_calls().len(), 3);
    assert_eq!(harness.process_control.spawn_count(), 1);
    let reasons = queue_reasons(&harness, &other).await;
    assert_eq!(reasons[0], ("lifecycle".to_string(), "crash_recovery_exhausted".to_string()), "{reasons:?}");
    let event = recovery(&harness).await["event"].clone();
    assert_eq!((event["status"].as_str(), event["attempts_used"].as_i64(), event["round"].as_i64()), (Some("exhausted"), Some(3), Some(1)));
    assert_eq!(task_status(&harness, &ws, &tid).await, "running");
}

#[tokio::test]
async fn manual_retry_starts_a_new_round_and_keeps_history() {
    let mut harness = start().await;
    let (ws, goal) = workspace(&harness, "A").await;
    let (tid, old) = running_task(&harness, &ws, &goal, "停不下來的工作").await;
    let old_identity = identity(&harness, &ws, &old).await;

    // 還沒有崩潰：不能重試
    let (status, body) = harness.send_json(Method::POST, RECOVERY_RETRY, None).await;
    assert_eq!((status, body["error"].as_str()), (StatusCode::CONFLICT, Some("no_recovery_event")));

    harness.process_control.script_stop(&old_identity, FakeStop::Result(StopResult::StillAlive));
    harness.crash_restart().await;
    harness.supervise().await;
    // 還沒用盡：不能重試
    let (status, body) = harness.send_json(Method::POST, RECOVERY_RETRY, None).await;
    assert_eq!((status, body["error"].as_str()), (StatusCode::CONFLICT, Some("recovery_not_exhausted")));
    for wait in [30_000, 120_000] {
        harness.clock.advance_millis(wait);
        harness.supervise().await;
    }
    assert_eq!(recovery(&harness).await["event"]["status"], "exhausted");

    // 使用者檢視診斷後（程序終於可以停下）從恢復介面重新嘗試
    harness.process_control.script_stop(&old_identity, FakeStop::Result(StopResult::ForceKilled));
    let (status, retried) = harness.send_json(Method::POST, RECOVERY_RETRY, None).await;
    assert_eq!(status, StatusCode::OK, "{retried}");
    let event = &retried["event"];
    assert_eq!((event["status"].as_str(), event["round"].as_i64(), event["attempts_used"].as_i64()), (Some("waiting"), Some(2), Some(0)));
    assert_eq!(retried["can_retry"], false);

    harness.supervise().await;
    let event = recovery(&harness).await["event"].clone();
    assert_eq!((event["status"].as_str(), event["round"].as_i64(), event["attempts_used"].as_i64()), (Some("observing"), Some(2), Some(1)));
    let attempts = event["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 4, "第 1 輪的三次嘗試保留為歷史");
    let rounds: Vec<(i64, i64, &str)> = attempts
        .iter()
        .map(|a| (a["round"].as_i64().unwrap(), a["seq"].as_i64().unwrap(), a["outcome"].as_str().unwrap()))
        .collect();
    assert_eq!(rounds, [(1, 1, "failed"), (1, 2, "failed"), (1, 3, "failed"), (2, 1, "observing")]);
    assert_eq!(run_view(&harness, &ws, &old).await["status"], "interrupted");
    assert_eq!(task_status(&harness, &ws, &tid).await, "running");
    assert_eq!(harness.process_control.alive().len(), 1);
}
