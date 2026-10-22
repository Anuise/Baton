//! 情境測試：桌面生命週期與完全退出（票 25），經由 Runtime harness 的真實 HTTP API 驗證。
//!
//! 全部使用 fake ProcessControl／AgentAdapter／QuotaObserver 與 fake Clock：測試決定何時讓 Supervisor 處理一次
//! （`supervise`）、程序停不停得下來（`script_stop`）、Runtime 何時「重新開啟」（`restart`）。
//! 只斷言外部行為：HTTP 回應（生命週期總覽、排程總覽、Run 與 Task）、Port 被呼叫的方式（spawn／stop 次數與寬限期）、
//! 生命週期檔的內容；`harness.db()` 只用來確認「事件有追加」這類必須看落地內容的事實。
//!
//! 每個測試名稱對應票 25 的驗收條件（關窗與重開視窗連回同一 Runtime 的情境在 `baton-client/tests/lifecycle_client.rs`）：
//! - 停止意圖生效後，佇列重試、額度恢復與交接都不能啟動新 Run：`after_the_stop_intent_*`（交接由 Domain 單元測試涵蓋）
//! - 退出流程在 40 秒內結束，無法確認停止的程序被記錄為停止異常：`full_exit_*_forty_seconds*`、`unconfirmed_*`
//! - Workspace A 開、B 關時，重開後只有 A 的合格工作自動恢復：`only_the_workspace_with_auto_resume_*`
//! - 完全退出不會把 Task 標為取消或完成：`full_exit_never_cancels_or_completes_tasks`

use std::time::{Duration, Instant};

use baton_domain::lifecycle::ExitTiming;
use baton_domain::Timestamp;
use baton_harness::fakes::FakeStop;
use baton_harness::Harness;
use baton_ports::{AdapterEvent, Confidence, ProcessIdentity, QuotaSample, QuotaSource, StopResult};
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

const WORKSPACES: &str = "/api/v1/workspaces";
const LIFECYCLE: &str = "/api/v1/lifecycle";
const LIFECYCLE_EXIT: &str = "/api/v1/lifecycle/exit";
const SCHEDULING: &str = "/api/v1/scheduling";

/// 2023-11-14 22:13:20.123 UTC
const CLOCK_START: Timestamp = Timestamp::from_millis(1_700_000_000_123);

// --- 共用輔助 ---

fn plain_dir(harness: &Harness, name: &str) -> String {
    let dir = harness.data_dir().join("projects").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    dir.to_string_lossy().into_owned()
}

fn task_path(ws: &str, tid: &str) -> String {
    format!("{WORKSPACES}/{ws}/tasks/{tid}")
}

fn filled(text: &str) -> Value {
    json!({ "status": "filled", "text": text })
}

fn contract() -> Value {
    json!({
        "objective": filled("整理資料"),
        "inputs": filled("既有的資料"),
        "scope": filled("公開資料"),
        "deliverables": filled("比較表"),
        "done_criteria": filled("五個產品"),
        "blocker_handling": filled("卡住時回報"),
        "required_capabilities": [],
    })
}

async fn start() -> Harness {
    Harness::builder().clock_at(CLOCK_START).start().await
}

/// 完全退出的寬限期縮短成毫秒級，讓「停不下來」的情境不必真的等 40 秒。
async fn start_with_short_exit() -> Harness {
    Harness::builder()
        .clock_at(CLOCK_START)
        .exit_timing(ExitTiming { stop_grace_ms: 150, kill_confirm_ms: 150 })
        .start()
        .await
}

/// 一個 Workspace 與它的 Goal，回傳 (workspace_id, goal_id)。
async fn workspace(harness: &Harness, name: &str) -> (String, String) {
    let path = plain_dir(harness, name);
    let (status, body) =
        harness.send_json(Method::POST, WORKSPACES, Some(json!({ "path": path, "name": name }))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let ws = body["id"].as_str().unwrap().to_string();
    let (status, goal) =
        harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/goals"), Some(json!({ "title": "目標" }))).await;
    assert_eq!(status, StatusCode::CREATED, "{goal}");
    (ws, goal["id"].as_str().unwrap().to_string())
}

async fn ready_task(harness: &Harness, ws: &str, goal: &str, title: &str) -> String {
    let body = json!({ "goal_id": goal, "title": title, "contract": contract() });
    let (status, task) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks"), Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    let tid = task["id"].as_str().unwrap().to_string();
    let (status, ready) = harness.send_json(Method::POST, &format!("{}/ready", task_path(ws, &tid)), None).await;
    assert_eq!(status, StatusCode::OK, "{ready}");
    tid
}

async fn dispatch(harness: &Harness, ws: &str, tid: &str) -> (StatusCode, Value) {
    harness.send_json(Method::POST, &format!("{}/dispatch", task_path(ws, tid)), None).await
}

/// 建立 ready 的 Task、派工成功並讓 Supervisor 啟動它，回傳 (task_id, run_id)。
async fn running_task(harness: &Harness, ws: &str, goal: &str, title: &str) -> (String, String) {
    let tid = ready_task(harness, ws, goal, title).await;
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

async fn task_status(harness: &Harness, ws: &str, tid: &str) -> String {
    let (status, detail) = harness.send_json(Method::GET, &task_path(ws, tid), None).await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    detail["task"]["status"].as_str().unwrap().to_string()
}

/// 這個 Run 的程序身分（fake ProcessControl 以 command_id 登記）。
async fn identity(harness: &Harness, ws: &str, rid: &str) -> ProcessIdentity {
    let run = run_view(harness, ws, rid).await;
    harness.process_control.identity_of(run["command_id"].as_str().unwrap()).unwrap()
}

async fn lifecycle(harness: &Harness) -> Value {
    harness.get_json(LIFECYCLE).await
}

/// 完全退出：持久化停止意圖（回應 202）並等停止流程結束，回傳結束後的生命週期總覽。
async fn full_exit(harness: &Harness) -> Value {
    let (status, body) = harness.send_json(Method::POST, LIFECYCLE_EXIT, None).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    harness.wait_for_exit().await;
    lifecycle(harness).await
}

fn run_outcome<'a>(status: &'a Value, rid: &str) -> &'a Value {
    status["exit"]["runs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["run_id"] == rid)
        .unwrap_or_else(|| panic!("停止紀錄中沒有 {rid}：{status}"))
}

fn workspace_view<'a>(status: &'a Value, ws: &str) -> &'a Value {
    status["workspaces"].as_array().unwrap().iter().find(|w| w["workspace_id"] == ws).unwrap()
}

async fn queue_reasons(harness: &Harness, tid: &str) -> Vec<(String, String)> {
    let overview: Value = harness.get_json(SCHEDULING).await;
    let entry = overview["queue"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["task_id"] == tid)
        .unwrap_or_else(|| panic!("{tid} 不在佇列中：{overview}"));
    entry["reasons"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (r["category"].as_str().unwrap().to_string(), r["code"].as_str().unwrap().to_string()))
        .collect()
}

fn event_types(harness: &Harness, aggregate_type: &str) -> Vec<String> {
    harness
        .db()
        .prepare("SELECT event_type FROM event WHERE aggregate_type = ?1 ORDER BY seq")
        .unwrap()
        .query_map([aggregate_type], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

async fn set_auto_resume(harness: &Harness, ws: &str, enabled: bool) -> Value {
    let (status, body) = harness
        .send_json(
            Method::PUT,
            &format!("{WORKSPACES}/{ws}/lifecycle"),
            Some(json!({ "auto_resume_after_exit": enabled })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

fn quota(used: f64) -> QuotaSample {
    QuotaSample {
        source: QuotaSource::Estimated,
        observed_at: CLOCK_START,
        used: Some(used),
        reset_at: None,
        confidence: Confidence::Estimated,
    }
}

// ============================================================ 停止意圖先持久化

#[tokio::test]
async fn full_exit_persists_the_stop_intent_in_the_database_and_the_lifecycle_file_then_stops_runs() {
    let harness = start().await;
    harness.add_account("帳號", "a@example.com").await;
    let (ws, goal) = workspace(&harness, "A").await;
    let (tid, rid) = running_task(&harness, &ws, &goal, "研究").await;

    let before = lifecycle(&harness).await;
    assert_eq!(before["phase"], "running");
    assert_eq!(before["stop_intent_active"], false);
    assert_eq!(before["active_runs"], 1);

    let (status, accepted) = harness.send_json(Method::POST, LIFECYCLE_EXIT, None).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{accepted}");
    assert_eq!(accepted["stop_intent_active"], true);
    assert_eq!(accepted["exit"]["requested_at"], CLOCK_START.as_millis());
    assert_eq!(accepted["exit"]["deadline_at"], CLOCK_START.as_millis() + 40_000, "總上限 40 秒（spec D-11）");
    assert_eq!(accepted["timing"], json!({ "stop_grace_ms": 30_000, "kill_confirm_ms": 10_000 }));
    harness.wait_for_exit().await;

    let after = lifecycle(&harness).await;
    assert_eq!(after["phase"], "stopped");
    assert_eq!(after["stop_intent_active"], true, "停止意圖持續到重新開啟");
    assert!(after["exit"]["completed_at"].is_number(), "{after}");
    assert_eq!(run_outcome(&after, &rid)["outcome"], "stopped_gracefully");
    assert_eq!(after["active_runs"], 0);

    // 要求 Run 在 30 秒寬限期內保存並停止；剩餘程序樹由 ProcessControl 在之後 10 秒內終止並確認
    assert_eq!(harness.process_control.stop_graces(), vec![Duration::from_secs(30)]);
    let run = run_view(&harness, &ws, &rid).await;
    assert_eq!(run["status"], "interrupted");
    assert_eq!(run["end_reason"], "stopped_by_full_exit");
    assert!(run["lease"]["released_at"].is_number(), "確認停止後才釋放寫入所有權");
    assert_eq!(task_status(&harness, &ws, &tid).await, "ready");

    // 生命週期檔鏡像停止意圖，launcher 不開資料庫就能判斷（spec D-25）
    let file: Value = serde_json::from_slice(&std::fs::read(harness.lifecycle_file_path()).unwrap()).unwrap();
    assert_eq!(file["stop"]["requested_at"], CLOCK_START.as_millis(), "{file}");

    assert_eq!(event_types(&harness, "runtime_lifecycle"), ["full_exit_requested", "full_exit_completed"]);
}

#[tokio::test]
async fn repeated_full_exit_requests_run_the_stop_procedure_only_once() {
    let harness = start().await;
    harness.add_account("帳號", "a@example.com").await;
    let (ws, goal) = workspace(&harness, "A").await;
    running_task(&harness, &ws, &goal, "研究").await;

    let (first, _) = harness.send_json(Method::POST, LIFECYCLE_EXIT, None).await;
    let (second, body) = harness.send_json(Method::POST, LIFECYCLE_EXIT, None).await;
    assert_eq!((first, second), (StatusCode::ACCEPTED, StatusCode::ACCEPTED), "{body}");
    harness.wait_for_exit().await;

    assert_eq!(harness.process_control.stop_calls().len(), 1);
    assert_eq!(event_types(&harness, "runtime_lifecycle"), ["full_exit_requested", "full_exit_completed"]);
}

// ============================================================ 停止意圖生效後不啟動新 Run

#[tokio::test]
async fn after_the_stop_intent_queue_retries_and_new_dispatch_requests_start_no_runs() {
    let harness = start().await;
    harness.add_account("帳號", "a@example.com").await;
    let (ws, goal) = workspace(&harness, "A").await;
    // 每帳號最多 2 個並行：第三個排隊等名額
    let (first, _) = running_task(&harness, &ws, &goal, "一").await;
    let (second, _) = running_task(&harness, &ws, &goal, "二").await;
    let waiting = ready_task(&harness, &ws, &goal, "三").await;
    let (status, _) = dispatch(&harness, &ws, &waiting).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(harness.process_control.spawn_count(), 2);

    full_exit(&harness).await;

    // 退出停下了兩個 Run、釋出名額：佇列重試仍然不派
    harness.supervise().await;
    harness.supervise().await;
    assert_eq!(harness.process_control.spawn_count(), 2, "停止意圖生效後佇列重試不能啟動新 Run");
    for tid in [&first, &second, &waiting] {
        assert_eq!(queue_reasons(&harness, tid).await, [("lifecycle".to_string(), "runtime_stopping".to_string())]);
    }

    // 新的派工要求只會排隊並說明原因，不建立 Run
    let late = ready_task(&harness, &ws, &goal, "四").await;
    let (status, body) = dispatch(&harness, &ws, &late).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert_eq!(body["reasons"][0]["code"], "runtime_stopping", "{body}");
    harness.supervise().await;
    assert_eq!(harness.process_control.spawn_count(), 2);
}

#[tokio::test]
async fn after_the_stop_intent_quota_recovery_starts_no_runs() {
    let harness = start().await;
    let profile = harness.add_account("帳號", "a@example.com").await;
    harness.quota_observer.set_sample(&profile, quota(0.9));
    let (ws, goal) = workspace(&harness, "A").await;
    let tid = ready_task(&harness, &ws, &goal, "研究").await;
    let (status, body) = dispatch(&harness, &ws, &tid).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    // 90% 已達 85% 的交接門檻（票 28）：帳號在 draining，同樣不派新工作
    assert_eq!(body["reasons"][0]["code"], "quota_draining");

    full_exit(&harness).await;
    // 額度恢復到門檻以下：沒有停止意圖時下一次處理就會派出
    harness.quota_observer.set_sample(&profile, quota(0.1));
    harness.supervise().await;

    assert_eq!(harness.process_control.spawn_count(), 0, "停止意圖生效後額度恢復不能啟動新 Run");
    assert_eq!(queue_reasons(&harness, &tid).await, [("lifecycle".to_string(), "runtime_stopping".to_string())]);
}

#[tokio::test]
async fn workers_are_told_to_save_and_stop_once_the_stop_intent_is_active() {
    let harness = start_with_short_exit().await;
    harness.add_account("帳號", "a@example.com").await;
    let (ws, goal) = workspace(&harness, "A").await;
    let (_, rid) = running_task(&harness, &ws, &goal, "研究").await;
    let token = harness.agent_credential(&rid).unwrap();

    let (status, ack) = harness.call_tool(&token, "report_progress", json!({ "summary": "進行中" })).await;
    assert_eq!(status, StatusCode::OK, "{ack}");
    assert_eq!(ack["stop_requested"], false);

    // 程序一直不停：寬限期內 Worker 仍是寫入所有者，回報的回應告訴它該保存並停止
    harness.process_control.script_stop(&identity(&harness, &ws, &rid).await, FakeStop::Hang);
    full_exit(&harness).await;
    let (status, ack) = harness.call_tool(&token, "save_checkpoint", json!({ "summary": "保存進度" })).await;
    assert_eq!(status, StatusCode::OK, "{ack}");
    assert_eq!(ack["stop_requested"], true);
}

// ============================================================ 40 秒內結束，無法確認停止者記為停止異常

#[tokio::test]
async fn full_exit_ends_within_its_deadline_and_records_unconfirmed_processes_as_stop_anomalies() {
    let harness = start_with_short_exit().await;
    harness.add_account("帳號 1", "a@example.com").await;
    harness.add_account("帳號 2", "b@example.com").await;
    let (ws, goal) = workspace(&harness, "A").await;
    let (alive_task, alive) = running_task(&harness, &ws, &goal, "終止後仍存活").await;
    let (_, hung) = running_task(&harness, &ws, &goal, "停止呼叫不回來").await;
    let (_, failed) = running_task(&harness, &ws, &goal, "停止呼叫失敗").await;
    let (_, graceful) = running_task(&harness, &ws, &goal, "正常停止").await;
    harness.process_control.script_stop(&identity(&harness, &ws, &alive).await, FakeStop::Result(StopResult::StillAlive));
    harness.process_control.script_stop(&identity(&harness, &ws, &hung).await, FakeStop::Hang);
    harness.process_control.script_stop(&identity(&harness, &ws, &failed).await, FakeStop::Fail("拒絕存取".into()));
    harness.process_control.script_stop(
        &identity(&harness, &ws, &graceful).await,
        FakeStop::Result(StopResult::ForceKilled),
    );

    let began = Instant::now();
    let status = full_exit(&harness).await;
    // 縮短後的總上限是 300ms；停止呼叫永遠不回來也不會讓退出卡住
    assert!(began.elapsed() < Duration::from_secs(5), "退出流程花了 {:?}", began.elapsed());

    assert_eq!(status["phase"], "stopped");
    let outcome = |rid: &str| {
        let record = run_outcome(&status, rid);
        (record["outcome"].as_str().unwrap().to_string(), record["anomaly"].as_str().map(str::to_string))
    };
    assert_eq!(outcome(&alive), ("anomaly".into(), Some("still_alive".into())));
    assert_eq!(outcome(&hung), ("anomaly".into(), Some("stop_unconfirmed".into())));
    assert_eq!(outcome(&failed), ("anomaly".into(), Some("stop_failed".into())));
    assert_eq!(outcome(&graceful), ("force_killed".into(), None));
    assert!(run_outcome(&status, &failed)["message"].as_str().unwrap().contains("拒絕存取"));

    // 停不下來的 Run 仍持有寫入所有權，不假裝已停止；它的 Task 也不會被取消或完成
    let run = run_view(&harness, &ws, &alive).await;
    assert_eq!(run["status"], "running");
    assert!(run["lease"]["released_at"].is_null());
    assert_eq!(task_status(&harness, &ws, &alive_task).await, "running");

    let db = harness.db();
    let payload: String = db
        .query_row(
            "SELECT payload FROM event WHERE event_type = 'full_exit_completed'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let payload: Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(payload["anomalies"].as_array().unwrap().len(), 3, "{payload}");
}

#[tokio::test]
async fn full_exit_is_not_delayed_by_a_cancellation_stuck_in_stopping_its_process() {
    let harness = start_with_short_exit().await;
    harness.add_account("帳號 1", "a@example.com").await;
    harness.add_account("帳號 2", "b@example.com").await;
    let (ws, goal) = workspace(&harness, "A").await;
    let (stuck_task, stuck) = running_task(&harness, &ws, &goal, "取消中卡住").await;
    let (_, other) = running_task(&harness, &ws, &goal, "一般執行中").await;
    let stuck_identity = identity(&harness, &ws, &stuck).await;
    harness.process_control.script_stop(&stuck_identity, FakeStop::Hang);
    let (status, body) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/runs/{stuck}/cancel"), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Supervisor 處理取消時，停止呼叫卡住（握著處理的鎖）；這時使用者要求完全退出
    let ticking = harness.supervise();
    let exiting = async {
        while !harness.process_control.stop_calls().contains(&stuck_identity) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let began = Instant::now();
        let (status, body) = harness.send_json(Method::POST, LIFECYCLE_EXIT, None).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{body}");
        // 停止意圖在回應前就已寫進生命週期檔（spec D-25），不必等停止流程拿到處理的鎖
        let file: Value = serde_json::from_slice(&std::fs::read(harness.lifecycle_file_path()).unwrap()).unwrap();
        assert_eq!(file["stop"]["requested_at"], CLOCK_START.as_millis(), "{file}");
        harness.wait_for_exit().await;
        // 縮短後的總上限是 300ms，從要求退出的那一刻算起
        assert!(began.elapsed() < Duration::from_secs(2), "退出流程花了 {:?}", began.elapsed());
        lifecycle(&harness).await
    };
    let ((), status) = tokio::time::timeout(Duration::from_secs(10), async { tokio::join!(ticking, exiting) })
        .await
        .expect("完全退出開始後，卡在停止程序的處理也要讓出");

    assert_eq!(status["phase"], "stopped", "{status}");
    let record = run_outcome(&status, &stuck);
    assert_eq!(record["outcome"], "anomaly", "{status}");
    assert_eq!(record["anomaly"], "stop_unconfirmed", "{status}");
    assert_eq!(run_outcome(&status, &other)["outcome"], "stopped_gracefully", "{status}");

    // 卡住的 Run 不假裝已停止：仍持有寫入所有權，Task 也不會被取消或完成
    let run = run_view(&harness, &ws, &stuck).await;
    assert_eq!(run["status"], "running", "{run}");
    assert!(run["lease"]["released_at"].is_null(), "{run}");
    assert_eq!(task_status(&harness, &ws, &stuck_task).await, "running");
}

#[tokio::test]
async fn full_exit_still_ends_within_its_deadline_when_another_supervisor_step_never_returns() {
    let harness = start_with_short_exit().await;
    harness.add_account("帳號 1", "a@example.com").await;
    harness.add_account("帳號 2", "b@example.com").await;
    let (ws, goal) = workspace(&harness, "A").await;
    let (running_tid, running) = running_task(&harness, &ws, &goal, "執行中").await;
    let launching_tid = ready_task(&harness, &ws, &goal, "啟動中卡住").await;
    let (status, run) = dispatch(&harness, &ws, &launching_tid).await;
    assert_eq!(status, StatusCode::CREATED, "{run}");
    let launching = run["id"].as_str().unwrap().to_string();
    harness.process_control.hang_next_spawn_at(baton_harness::fakes::HangPoint::BeforeSpawn);

    // Supervisor 啟動程序時永遠不回來（握著處理的鎖）；這時使用者要求完全退出
    let ticking = harness.supervise();
    let exiting = async {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let began = Instant::now();
        let (status, body) = harness.send_json(Method::POST, LIFECYCLE_EXIT, None).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{body}");
        harness.wait_for_exit().await;
        assert!(began.elapsed() < Duration::from_secs(2), "退出流程花了 {:?}", began.elapsed());
        lifecycle(&harness).await
    };
    let status = tokio::select! {
        _ = ticking => panic!("啟動程序的呼叫不該回來"),
        status = exiting => status,
    };

    // 不能在另一個處理進行中停止程序：沒有要求任何程序停止，全部記為無法確認，並保留寫入所有權
    assert_eq!(status["phase"], "stopped", "{status}");
    for rid in [&running, &launching] {
        assert_eq!(run_outcome(&status, rid)["anomaly"], "stop_unconfirmed", "{status}");
    }
    assert!(!status["exit"]["notes"].as_array().unwrap().is_empty(), "{status}");
    assert!(harness.process_control.stop_calls().is_empty());
    let run = run_view(&harness, &ws, &running).await;
    assert_eq!(run["status"], "running", "{run}");
    assert!(run["lease"]["released_at"].is_null(), "{run}");
    assert_eq!(task_status(&harness, &ws, &running_tid).await, "running");
}

// ============================================================ 完全退出不改變 Task 的結論

#[tokio::test]
async fn full_exit_never_cancels_or_completes_tasks() {
    let harness = start().await;
    harness.add_account("帳號 1", "a@example.com").await;
    harness.add_account("帳號 2", "b@example.com").await;
    let (ws, goal) = workspace(&harness, "A").await;
    let (running, _) = running_task(&harness, &ws, &goal, "執行中").await;
    let (blocked, blocked_run) = running_task(&harness, &ws, &goal, "受阻").await;
    let token = harness.agent_credential(&blocked_run).unwrap();
    let (status, _) = harness.call_tool(&token, "report_blocked", json!({ "reason": "缺資料" })).await;
    assert_eq!(status, StatusCode::OK);
    let (verifying, verifying_run) = running_task(&harness, &ws, &goal, "驗證中").await;
    harness.agent_adapter.emit(&verifying_run, AdapterEvent::Exited { success: true });
    harness.supervise().await;
    let queued = ready_task(&harness, &ws, &goal, "排隊中").await;
    harness.process_control.set_stop_result(StopResult::StoppedGracefully);
    assert_eq!(task_status(&harness, &ws, &verifying).await, "verifying");

    full_exit(&harness).await;

    assert_eq!(task_status(&harness, &ws, &running).await, "ready", "被停下的工作退回 ready，等之後續跑");
    assert_eq!(task_status(&harness, &ws, &blocked).await, "blocked");
    assert_eq!(task_status(&harness, &ws, &verifying).await, "verifying");
    assert_eq!(task_status(&harness, &ws, &queued).await, "ready");
    let statuses: Vec<String> = harness
        .db()
        .prepare("SELECT status FROM task")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert!(statuses.iter().all(|s| s != "cancelled" && s != "completed"), "{statuses:?}");
}

// ============================================================ 重新開啟：先核對，再依各 Workspace 設定續跑

#[tokio::test]
async fn only_the_workspace_with_auto_resume_resumes_its_work_after_reopening() {
    let mut harness = start().await;
    harness.add_account("帳號 1", "a@example.com").await;
    harness.add_account("帳號 2", "b@example.com").await;
    let (a, goal_a) = workspace(&harness, "A").await;
    let (b, goal_b) = workspace(&harness, "B").await;
    // 設定預設關閉
    assert_eq!(workspace_view(&lifecycle(&harness).await, &b)["auto_resume_after_exit"], false);
    let view = set_auto_resume(&harness, &a, true).await;
    assert_eq!(view["auto_resume_after_exit"], true);
    let (task_a, _) = running_task(&harness, &a, &goal_a, "A 的工作").await;
    let (task_b, _) = running_task(&harness, &b, &goal_b, "B 的工作").await;
    assert_eq!(harness.process_control.spawn_count(), 2);

    full_exit(&harness).await;
    harness.clock.advance_millis(3_600_000);
    harness.restart().await;

    // 重新開啟後、核對之前：仍然是停止意圖，不續跑任何工作
    let reopened = lifecycle(&harness).await;
    assert_eq!(reopened["stop_intent_active"], true, "{reopened}");
    let (status, body) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{b}/resume"), None).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"], "stop_intent_active");

    harness.supervise().await;
    let after = lifecycle(&harness).await;
    assert_eq!(after["phase"], "running");
    assert_eq!(after["stop_intent_active"], false);
    assert_eq!(workspace_view(&after, &a)["awaiting_resume"], false);
    assert_eq!(workspace_view(&after, &b)["awaiting_resume"], true);
    assert_eq!(task_status(&harness, &a, &task_a).await, "running", "A 開啟自動續跑：合格的工作自動恢復");
    assert_eq!(task_status(&harness, &b, &task_b).await, "ready", "B 關閉：等「繼續工作」");
    assert_eq!(harness.process_control.spawn_count(), 3);
    assert_eq!(queue_reasons(&harness, &task_b).await, [("lifecycle".to_string(), "awaiting_resume".to_string())]);

    let (status, resumed) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{b}/resume"), None).await;
    assert_eq!(status, StatusCode::OK, "{resumed}");
    assert_eq!(resumed["awaiting_resume"], false);
    harness.supervise().await;
    assert_eq!(task_status(&harness, &b, &task_b).await, "running");
    assert_eq!(harness.process_control.spawn_count(), 4);
    let types = event_types(&harness, "workspace_lifecycle");
    assert!(types.contains(&"workspace_resumed".to_string()), "{types:?}");
}

#[tokio::test]
async fn reopening_reconciles_processes_the_exit_could_not_confirm_before_resuming() {
    let mut harness = start_with_short_exit().await;
    harness.add_account("帳號 1", "a@example.com").await;
    harness.add_account("帳號 2", "b@example.com").await;
    let (ws, goal) = workspace(&harness, "A").await;
    set_auto_resume(&harness, &ws, true).await;
    let (gone_task, gone) = running_task(&harness, &ws, &goal, "退出後才結束").await;
    let (alive_task, alive) = running_task(&harness, &ws, &goal, "仍在執行").await;
    let gone_identity = identity(&harness, &ws, &gone).await;
    harness.process_control.script_stop(&gone_identity, FakeStop::Result(StopResult::StillAlive));
    harness.process_control.script_stop(&identity(&harness, &ws, &alive).await, FakeStop::Result(StopResult::StillAlive));
    full_exit(&harness).await;

    // Runtime 結束後，其中一個程序自己結束了
    harness.process_control.script_liveness(&gone_identity, baton_ports::Liveness::Dead);
    harness.restart().await;
    harness.supervise().await;

    let run = run_view(&harness, &ws, &gone).await;
    assert_eq!(run["status"], "interrupted");
    assert_eq!(run["end_reason"], "process_gone_after_full_exit");
    // 結果不明：不自動重派（由使用者重新派工），Task 回到 ready
    assert_eq!(task_status(&harness, &ws, &gone_task).await, "ready");
    // 仍在執行的那個保持原狀，不會因為續跑而重複啟動 Worker
    assert_eq!(run_view(&harness, &ws, &alive).await["status"], "running");
    assert_eq!(task_status(&harness, &ws, &alive_task).await, "running");
    assert_eq!(harness.process_control.spawn_count(), 2);
}

#[tokio::test]
async fn changing_auto_resume_neither_starts_nor_stops_current_work() {
    let harness = start().await;
    harness.add_account("帳號", "a@example.com").await;
    let (ws, goal) = workspace(&harness, "A").await;
    let (tid, _) = running_task(&harness, &ws, &goal, "研究").await;

    set_auto_resume(&harness, &ws, true).await;
    set_auto_resume(&harness, &ws, true).await;
    set_auto_resume(&harness, &ws, false).await;
    harness.supervise().await;

    assert_eq!(harness.process_control.spawn_count(), 1);
    assert!(harness.process_control.stop_calls().is_empty());
    assert_eq!(task_status(&harness, &ws, &tid).await, "running");
    assert_eq!(event_types(&harness, "workspace_lifecycle").len(), 2, "相同的值不追加事件");
    let (status, body) = harness
        .send_json(Method::PUT, &format!("{WORKSPACES}/ws_missing/lifecycle"), Some(json!({ "auto_resume_after_exit": true })))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
}

#[tokio::test]
async fn a_stop_intent_found_only_in_the_lifecycle_file_is_treated_as_stopped() {
    let mut harness = start().await;
    let (ws, goal) = workspace(&harness, "A").await;
    let tid = ready_task(&harness, &ws, &goal, "研究").await;
    // 還沒有帳號：先排隊
    let (status, _) = dispatch(&harness, &ws, &tid).await;
    assert_eq!(status, StatusCode::ACCEPTED);

    // 資料庫沒有停止意圖，生命週期檔卻有（例如寫完檔案後就當掉）：採較保守者（spec D-25）
    std::fs::write(
        harness.lifecycle_file_path(),
        json!({ "version": 1, "stop": { "requested_at": CLOCK_START.as_millis(), "reason": "full_exit" }, "updated_at": CLOCK_START.as_millis() })
            .to_string(),
    )
    .unwrap();
    harness.restart().await;
    assert_eq!(lifecycle(&harness).await["stop_intent_active"], true);

    harness.add_account("帳號", "a@example.com").await;
    harness.supervise().await;
    harness.supervise().await;
    assert_eq!(harness.process_control.spawn_count(), 0, "自動續跑預設關閉：等使用者按「繼續工作」");
    assert_eq!(workspace_view(&lifecycle(&harness).await, &ws)["awaiting_resume"], true);
    let file: Value = serde_json::from_slice(&std::fs::read(harness.lifecycle_file_path()).unwrap()).unwrap();
    assert!(file["stop"].is_null(), "重新開啟後解除停止抑制：{file}");

    let (status, _) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/resume"), None).await;
    assert_eq!(status, StatusCode::OK);
    harness.supervise().await;
    assert_eq!(harness.process_control.spawn_count(), 1);
}
