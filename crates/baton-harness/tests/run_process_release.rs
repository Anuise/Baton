//! 情境測試（最終 code-review 修正）：Run 結束前確認整個程序樹已結束、取消會告知 Worker、結束的 Run 不留下事件串流。
//!
//! - Worker 自行結束（`Exited`）或 lease 核對確認根程序已不在時，Supervisor 先要求 ProcessControl 收尾（`release`）；
//!   收尾無法確認（程序樹裡還有清不掉的程序）就不結束 Run，保留寫入所有權，避免殘留程序成為第二個 writer（D-10）。
//! - 使用者取消 Run 後，Worker 的回報回應帶 `stop_requested`，讓它在寬限期內自行保存並結束。
//! - 經 lease 核對結束的 Run，Supervisor 不再持有它的事件串流。

use baton_domain::run::LEASE_DURATION_MS;
use baton_domain::Timestamp;
use baton_harness::Harness;
use baton_ports::{AdapterEvent, Liveness, ProcessIdentity, StopResult};
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

const WORKSPACES: &str = "/api/v1/workspaces";

/// 2023-11-14 22:13:20.123 UTC
const CLOCK_START: Timestamp = Timestamp::from_millis(1_700_000_000_123);

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

async fn harness() -> Harness {
    let harness = Harness::builder().clock_at(CLOCK_START).start().await;
    harness.add_account("主要帳號", "main@example.com").await;
    harness
}

/// 建立 Workspace、Goal 與一個 ready 的 Task，派工並讓 Supervisor 啟動它；回傳 (workspace_id, task_id, run_id)。
async fn running_run(harness: &Harness) -> (String, String, String) {
    let dir = harness.data_dir().join("projects").join("main");
    std::fs::create_dir_all(&dir).unwrap();
    let (status, ws) = harness.send_json(Method::POST, WORKSPACES, Some(json!({ "path": dir.to_string_lossy() }))).await;
    assert_eq!(status, StatusCode::CREATED, "{ws}");
    let ws = ws["id"].as_str().unwrap().to_string();
    let (status, goal) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/goals"), Some(json!({ "title": "目標" }))).await;
    assert_eq!(status, StatusCode::CREATED, "{goal}");
    let body = json!({ "goal_id": goal["id"], "title": "整理資料", "contract": contract() });
    let (status, task) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks"), Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    let tid = task["id"].as_str().unwrap().to_string();
    let (status, ready) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks/{tid}/ready"), None).await;
    assert_eq!(status, StatusCode::OK, "{ready}");
    let (status, run) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks/{tid}/dispatch"), None).await;
    assert_eq!(status, StatusCode::CREATED, "{run}");
    let rid = run["id"].as_str().unwrap().to_string();
    harness.supervise().await;
    assert_eq!(run_status(harness, &ws, &rid).await, "running");
    (ws, tid, rid)
}

async fn run_view(harness: &Harness, ws: &str, rid: &str) -> Value {
    let (status, run) = harness.send_json(Method::GET, &format!("{WORKSPACES}/{ws}/runs/{rid}"), None).await;
    assert_eq!(status, StatusCode::OK, "{run}");
    run
}

async fn run_status(harness: &Harness, ws: &str, rid: &str) -> String {
    run_view(harness, ws, rid).await["status"].as_str().unwrap().to_string()
}

async fn task_status(harness: &Harness, ws: &str, tid: &str) -> String {
    let (status, detail) = harness.send_json(Method::GET, &format!("{WORKSPACES}/{ws}/tasks/{tid}"), None).await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    detail["task"]["status"].as_str().unwrap().to_string()
}

fn only_identity(harness: &Harness) -> ProcessIdentity {
    let spawned = harness.process_control.spawned();
    assert_eq!(spawned.len(), 1);
    harness.process_control.identity_of(&spawned[0].command_id).unwrap()
}

#[tokio::test]
async fn a_worker_that_exits_ends_its_run_only_after_the_whole_process_tree_is_released() {
    let harness = harness().await;
    let (ws, tid, rid) = running_run(&harness).await;
    let identity = only_identity(&harness);

    // Worker 回報結束，但它放到背景的子程序清不掉：Run 不結束，仍是寫入所有者
    harness.process_control.set_release_result(StopResult::StillAlive);
    harness.agent_adapter.emit(&rid, AdapterEvent::Exited { success: true });
    harness.supervise().await;
    assert_eq!(harness.process_control.release_calls(), [identity.clone()]);
    assert_eq!(run_status(&harness, &ws, &rid).await, "running", "程序樹還沒清空就不能結束 Run");
    assert_eq!(task_status(&harness, &ws, &tid).await, "running");

    // 下一次處理再收尾一次，這次確認整棵樹都結束了：才照 Worker 的結束事件結束 Run
    harness.process_control.set_release_result(StopResult::ForceKilled);
    harness.supervise().await;
    assert_eq!(harness.process_control.release_calls(), [identity.clone(), identity]);
    assert_eq!(run_status(&harness, &ws, &rid).await, "succeeded");
}

#[tokio::test]
async fn an_expired_lease_with_a_dead_root_is_concluded_only_after_the_leftovers_are_released() {
    let harness = harness().await;
    let (ws, tid, rid) = running_run(&harness).await;
    let identity = only_identity(&harness);

    // 根程序已經不在，但 Job 內殘留的子孫程序清不掉：不能判定程序已死、交出寫入所有權
    harness.clock.advance_millis(LEASE_DURATION_MS + 1);
    harness.process_control.script_liveness(&identity, Liveness::Dead);
    harness.process_control.set_release_result(StopResult::StillAlive);
    harness.supervise().await;
    assert_eq!(harness.process_control.release_calls(), [identity.clone()]);
    let unsure = run_view(&harness, &ws, &rid).await;
    assert_eq!(unsure["status"], "running", "{unsure}");
    assert!(unsure["lease"]["released_at"].is_null());

    // 殘留程序清掉之後才中斷 Run、Task 退回 ready
    harness.process_control.set_release_result(StopResult::ForceKilled);
    harness.supervise().await;
    assert_eq!(run_status(&harness, &ws, &rid).await, "interrupted");
    assert_eq!(task_status(&harness, &ws, &tid).await, "ready");
}

#[tokio::test]
async fn a_run_ended_by_lease_reconciliation_no_longer_holds_its_event_stream() {
    let harness = harness().await;
    let (ws, _, rid) = running_run(&harness).await;
    let identity = only_identity(&harness);
    assert_eq!(harness.agent_adapter.live_streams(&rid), 1, "執行中的 Run 由 Supervisor 讀取事件");

    harness.clock.advance_millis(LEASE_DURATION_MS + 1);
    harness.process_control.script_liveness(&identity, Liveness::Dead);
    harness.supervise().await;
    assert_eq!(run_status(&harness, &ws, &rid).await, "interrupted");
    harness.supervise().await;

    assert_eq!(harness.agent_adapter.live_streams(&rid), 0, "已結束的 Run 不再保留事件串流");
}

#[tokio::test]
async fn a_worker_is_told_to_save_and_stop_once_its_run_is_cancelled() {
    let harness = harness().await;
    let (ws, _, rid) = running_run(&harness).await;
    let token = harness.agent_credential(&rid).unwrap();

    let (status, ack) = harness.call_tool(&token, "report_progress", json!({ "summary": "進行中" })).await;
    assert_eq!(status, StatusCode::OK, "{ack}");
    assert_eq!(ack["stop_requested"], false);

    // 使用者取消、Supervisor 還沒停止程序：寬限期內 Worker 的回報回應告訴它該保存並停止
    let (status, body) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/runs/{rid}/cancel"), None).await;
    assert!(status.is_success(), "{body}");
    let (status, ack) = harness.call_tool(&token, "save_checkpoint", json!({ "summary": "保存進度" })).await;
    assert_eq!(status, StatusCode::OK, "{ack}");
    assert_eq!(ack["stop_requested"], true);
    assert_eq!(ack["handoff_requested"], false);
}
