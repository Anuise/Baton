//! 情境測試：Outbox 交付、程序核對與外部操作追蹤（票 26），經由 Runtime harness 的真實 HTTP API 驗證。
//!
//! 在程序啟動、回執、外部回應等邊界注入故障（fake ProcessControl 的崩潰點與失敗、Runtime 重建、
//! Store 的寫入故障注入），只斷言外部行為：HTTP 回應、Port 被呼叫的方式（例如「沒有第二次 spawn」）、
//! Worker 透過 Agent API 看到的結果。`harness.db()` 只用來確認必須看落地內容的事實。

use std::time::Duration;

use baton_domain::external_operation::{
    OPTION_VERIFIED_DONE, OPTION_VERIFIED_NOT_DONE, TOOL_BEGIN_EXTERNAL_OPERATION, TOOL_RECORD_EXTERNAL_OPERATION_RESULT,
};
use baton_domain::run::LEASE_DURATION_MS;
use baton_domain::storage_health::StorageFaultKind;
use baton_domain::Timestamp;
use baton_harness::fakes::HangPoint;
use baton_harness::Harness;
use baton_ports::{AdapterEvent, Liveness};
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

const WORKSPACES: &str = "/api/v1/workspaces";

/// 2023-11-14 22:13:20.123 UTC
const CLOCK_START: Timestamp = Timestamp::from_millis(1_700_000_000_123);

// --- 共用輔助 ---

async fn harness() -> Harness {
    let harness = Harness::builder().clock_at(CLOCK_START).start().await;
    harness.add_account("主要帳號", "main@example.com").await;
    harness
}

/// 一個 Workspace 與一個 Goal，回傳 (workspace_id, goal_id)。
async fn setup(harness: &Harness) -> (String, String) {
    let dir = harness.data_dir().join("projects").join("main");
    std::fs::create_dir_all(&dir).unwrap();
    let (status, ws) =
        harness.send_json(Method::POST, WORKSPACES, Some(json!({ "path": dir.to_string_lossy() }))).await;
    assert_eq!(status, StatusCode::CREATED, "{ws}");
    let ws = ws["id"].as_str().unwrap().to_string();
    let (status, goal) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/goals"), Some(json!({ "title": "目標" }))).await;
    assert_eq!(status, StatusCode::CREATED, "{goal}");
    (ws, goal["id"].as_str().unwrap().to_string())
}

fn filled(text: &str) -> Value {
    json!({ "status": "filled", "text": text })
}

async fn ready_task(harness: &Harness, ws: &str, goal: &str, title: &str, capabilities: &[&str]) -> String {
    let contract = json!({
        "objective": filled("部署新版本"),
        "inputs": filled("建置好的版本"),
        "scope": filled("正式環境"),
        "deliverables": filled("上線的新版本"),
        "done_criteria": filled("健康檢查通過"),
        "blocker_handling": filled("卡住時回報"),
        "required_capabilities": capabilities,
    });
    let body = json!({ "goal_id": goal, "title": title, "contract": contract });
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

async fn dispatch_ok(harness: &Harness, ws: &str, tid: &str) -> String {
    let (status, run) = dispatch(harness, ws, tid).await;
    assert_eq!(status, StatusCode::CREATED, "{run}");
    run["id"].as_str().unwrap().to_string()
}

async fn run_view(harness: &Harness, ws: &str, rid: &str) -> Value {
    let (status, run) = harness.send_json(Method::GET, &format!("{WORKSPACES}/{ws}/runs/{rid}"), None).await;
    assert_eq!(status, StatusCode::OK, "{run}");
    run
}

async fn task_status(harness: &Harness, ws: &str, tid: &str) -> String {
    let (status, detail) = harness.send_json(Method::GET, &format!("{WORKSPACES}/{ws}/tasks/{tid}"), None).await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    detail["task"]["status"].as_str().unwrap().to_string()
}

/// Task 最新（世代最大）的 Run 識別。
async fn latest_run(harness: &Harness, ws: &str, tid: &str) -> String {
    let (status, list) = harness.send_json(Method::GET, &format!("{WORKSPACES}/{ws}/runs"), None).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    list["runs"].as_array().unwrap().iter().rfind(|r| r["task_id"] == json!(tid)).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string()
}

async fn external_operations(harness: &Harness, ws: &str) -> Vec<Value> {
    let (status, body) = harness.send_json(Method::GET, &format!("{WORKSPACES}/{ws}/external-operations"), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["operations"].as_array().unwrap().clone()
}

async fn health(harness: &Harness) -> Value {
    let (status, body) = harness.send_json(Method::GET, "/api/v1/health", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

fn status_of(run: &Value) -> &str {
    run["status"].as_str().unwrap()
}

fn error_code(body: &Value) -> &str {
    body["error"].as_str().unwrap_or("")
}

async fn supervise_until_crash(harness: &Harness) {
    let outcome = tokio::time::timeout(Duration::from_millis(300), harness.supervise()).await;
    assert!(outcome.is_err(), "預期 Supervisor 卡在被設定的崩潰點");
}

fn deploy_request(key: &str) -> Value {
    json!({ "idempotency_key": key, "operation": "deploy", "target": "production", "request": { "version": "1.2.3" } })
}

/// Worker 登記外部操作（與 MCP shim 送出的請求相同）。
async fn begin(harness: &Harness, rid: &str, key: &str) -> (StatusCode, Value) {
    let token = harness.agent_credential(rid).expect("Run 已啟動，Worker 有 Agent 憑證");
    harness.call_tool(&token, TOOL_BEGIN_EXTERNAL_OPERATION, deploy_request(key)).await
}

async fn record(harness: &Harness, rid: &str, body: Value) -> (StatusCode, Value) {
    let token = harness.agent_credential(rid).expect("Run 已啟動，Worker 有 Agent 憑證");
    harness.call_tool(&token, TOOL_RECORD_EXTERNAL_OPERATION_RESULT, body).await
}

async fn decision(harness: &Harness, ws: &str, request_id: &str) -> Value {
    let (status, decision) = harness.send_json(Method::GET, &format!("{WORKSPACES}/{ws}/decisions/{request_id}"), None).await;
    assert_eq!(status, StatusCode::OK, "{decision}");
    decision
}

/// 以使用者看到的內容版本處理裁定請求（票 35 起 `expected_revision` 必填）。
async fn try_answer(harness: &Harness, ws: &str, request_id: &str, mut body: Value) -> (StatusCode, Value) {
    body["expected_revision"] = decision(harness, ws, request_id).await["revision"].clone();
    let path = format!("{WORKSPACES}/{ws}/decisions/{request_id}/answer");
    harness.send_json(Method::POST, &path, Some(body)).await
}

async fn answer(harness: &Harness, ws: &str, request_id: &str, option: &str) -> Value {
    let (status, body) = try_answer(harness, ws, request_id, json!({ "action": "accept", "option_id": option })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

/// 讓 Run 的程序死掉、lease 過期，Supervisor 核對後把 Run 標為中斷。
async fn kill_run(harness: &Harness, ws: &str, rid: &str) {
    let command_id = run_view(harness, ws, rid).await["command_id"].as_str().unwrap().to_string();
    let identity = harness.process_control.identity_of(&command_id).unwrap();
    harness.process_control.script_liveness(&identity, Liveness::Dead);
    harness.clock.advance_millis(LEASE_DURATION_MS + 1);
}

// ============================================================ 程序啟動與回執的邊界

#[tokio::test]
async fn a_spawn_that_starts_the_process_but_loses_the_receipt_is_found_by_command_id_and_never_started_twice() {
    let mut harness = harness().await;
    let (ws, goal) = setup(&harness).await;
    let tid = ready_task(&harness, &ws, &goal, "部署", &[]).await;
    let rid = dispatch_ok(&harness, &ws, &tid).await;
    harness.process_control.fail_next_spawn_after_start("回執在傳回途中遺失");

    harness.supervise().await;

    let run = run_view(&harness, &ws, &rid).await;
    assert_eq!(status_of(&run), "running", "以 command_id 找回已啟動的程序：{run}");
    let identity = harness.process_control.identity_of(run["command_id"].as_str().unwrap()).unwrap();
    assert_eq!(run["process"]["pid"].as_u64(), Some(u64::from(identity.pid)));
    harness.restart().await;
    harness.supervise().await;
    harness.supervise().await;
    assert_eq!(harness.process_control.spawn_count(), 1, "不重複啟動");
    assert_eq!(status_of(&run_view(&harness, &ws, &rid).await), "running");
}

#[tokio::test]
async fn a_launch_that_cannot_be_proven_stays_in_reconciliation_and_is_never_redispatched() {
    let mut harness = harness().await;
    let (ws, goal) = setup(&harness).await;
    let tid = ready_task(&harness, &ws, &goal, "部署", &[]).await;
    let rid = dispatch_ok(&harness, &ws, &tid).await;
    harness.process_control.hang_next_spawn_at(HangPoint::AfterSpawn);
    supervise_until_crash(&harness).await;

    // 重建之後 ProcessControl 暫時無法回答「這個 command_id 的程序在不在」
    harness.restart().await;
    harness.process_control.fail_finds(Some("Job Object 查詢失敗"));
    for _ in 0..3 {
        harness.supervise().await;
    }
    let waiting = run_view(&harness, &ws, &rid).await;
    assert_eq!(status_of(&waiting), "starting", "無法證明是否已啟動：留在核對，不中斷也不重派：{waiting}");
    assert_eq!(harness.process_control.spawn_count(), 1, "沒有第二次 spawn");
    assert_eq!(task_status(&harness, &ws, &tid).await, "running", "Task 不會回到 ready 被重新派出");
    let (status, body) = dispatch(&harness, &ws, &tid).await;
    assert_eq!(status, StatusCode::CONFLICT, "仍持有寫入所有權，不能派第二個：{body}");

    // 可以查證之後補寫回執
    harness.process_control.fail_finds(None);
    harness.supervise().await;
    let run = run_view(&harness, &ws, &rid).await;
    assert_eq!(status_of(&run), "running", "{run}");
    assert_eq!(harness.process_control.spawn_count(), 1);
}

#[tokio::test]
async fn a_run_whose_authorization_is_revoked_before_launch_ends_without_starting_a_process() {
    let harness = harness().await;
    let (ws, goal) = setup(&harness).await;
    confirm_template(&harness, &ws, "trusted_dev_p3").await;
    let tid = ready_task(&harness, &ws, &goal, "要跑建置", &["run_build_test"]).await;
    let rid = dispatch_ok(&harness, &ws, &tid).await;
    // 派工之後、Supervisor 啟動之前，授權被改成不含 run_build_test
    confirm_template(&harness, &ws, "research_docs").await;

    harness.supervise().await;

    let run = run_view(&harness, &ws, &rid).await;
    // 准入被拒的結束方式沿用票 24（R1-a）：interrupted、admission_denied（票 26 整合，AI-261）
    assert_eq!(status_of(&run), "interrupted", "{run}");
    assert!(run["end_reason"].as_str().unwrap().starts_with("admission_denied"), "{run}");
    assert!(run["command_id"].is_null(), "沒有寫入 launch intent");
    assert_eq!(harness.process_control.spawn_count(), 0, "准入不成立就不啟動程序");
    assert!(harness.agent_credential(&rid).is_none(), "也沒有簽發 Agent 憑證給任何程序");
    let undelivered: i64 =
        harness.db().query_row("SELECT COUNT(*) FROM outbox WHERE delivered_at IS NULL", [], |r| r.get(0)).unwrap();
    assert_eq!(undelivered, 0, "Outbox 項目處理完畢，不會一直重試");
}

async fn confirm_template(harness: &Harness, ws: &str, template_id: &str) {
    let path = format!("{WORKSPACES}/{ws}/authorization");
    let (status, view) = harness.send_json(Method::GET, &path, None).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    let grants = view["templates"].as_array().unwrap().iter().find(|t| t["id"] == template_id).unwrap()["grants"].clone();
    let body = json!({ "template_id": template_id, "grants": grants, "acknowledged_risk": true, "acknowledged_expansion": true });
    let (status, revision) = harness.send_json(Method::PUT, &path, Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{revision}");
}

// ============================================================ 外部操作

#[tokio::test]
async fn a_crash_after_an_external_operation_was_sent_never_resends_it_and_holds_only_the_related_task() {
    let mut harness = harness().await;
    let (ws, goal) = setup(&harness).await;
    let deploy = ready_task(&harness, &ws, &goal, "部署", &[]).await;
    let docs = ready_task(&harness, &ws, &goal, "寫文件", &[]).await;
    let deploy_run = dispatch_ok(&harness, &ws, &deploy).await;
    let docs_run = dispatch_ok(&harness, &ws, &docs).await;
    harness.supervise().await;

    // Worker 登記後送出部署，回報結果之前 Runtime 當掉、Worker 的程序也死了
    let (status, begun) = begin(&harness, &deploy_run, "deploy-web-1.2.3").await;
    assert_eq!((status, begun["status"].as_str()), (StatusCode::OK, Some("proceed")), "{begun}");
    harness.restart().await;
    kill_run(&harness, &ws, &deploy_run).await;
    // 另一個 Task 的 Worker 還活著，持續回報存活
    harness.agent_adapter.emit(&docs_run, AdapterEvent::Heartbeat);
    harness.supervise().await;

    assert_eq!(status_of(&run_view(&harness, &ws, &deploy_run).await), "interrupted");
    let operation = external_operations(&harness, &ws).await.remove(0);
    assert_eq!(operation["status"], "unknown", "{operation}");
    let request_id = operation["decision_request_id"].as_str().unwrap().to_string();
    assert_eq!(task_status(&harness, &ws, &deploy).await, "waiting_decision", "結果未知：相關 Task 阻擋");
    let (status, body) = dispatch(&harness, &ws, &deploy).await;
    assert_ne!(status, StatusCode::CREATED, "查證之前不會被重新派出：{body}");
    // 獨立的 Task 繼續：照常執行並結束
    assert_eq!(status_of(&run_view(&harness, &ws, &docs_run).await), "running");
    harness.agent_adapter.emit(&docs_run, AdapterEvent::Exited { success: true });
    harness.supervise().await;
    assert_eq!(status_of(&run_view(&harness, &ws, &docs_run).await), "succeeded");

    // 裁定請求說明結果未知、不自動重送，並提供查證選項
    let (status, decision) = harness.send_json(Method::GET, &format!("{WORKSPACES}/{ws}/decisions/{request_id}"), None).await;
    assert_eq!(status, StatusCode::OK, "{decision}");
    assert!(decision["question"].as_str().unwrap().contains("結果未知"), "{decision}");
    let options: Vec<&str> = decision["options"].as_array().unwrap().iter().map(|o| o["id"].as_str().unwrap()).collect();
    assert_eq!(options, vec![OPTION_VERIFIED_DONE, OPTION_VERIFIED_NOT_DONE]);

    // 使用者查證已生效：Task 恢復並重新排隊，新的 Worker 用同一個鍵登記時被告知不要重送
    let answered = answer(&harness, &ws, &request_id, OPTION_VERIFIED_DONE).await;
    assert_eq!(answered["resumed_task_ids"], json!([deploy]));
    // 程序意外消失也計入基礎設施重試：等過退避期限才再派（票 36）
    harness.clock.advance_millis(baton_domain::budget::backoff_millis(1));
    harness.supervise().await;
    let second_run = latest_run(&harness, &ws, &deploy).await;
    assert_ne!(second_run, deploy_run);
    assert_eq!(status_of(&run_view(&harness, &ws, &second_run).await), "running");
    let (status, again) = begin(&harness, &second_run, "deploy-web-1.2.3").await;
    assert_eq!((status, again["status"].as_str()), (StatusCode::OK, Some("already_succeeded")), "{again}");
    let operation = external_operations(&harness, &ws).await.remove(0);
    assert_eq!((operation["status"].as_str(), operation["attempt"].as_i64()), (Some("succeeded"), Some(1)));
}

#[tokio::test]
async fn an_unknown_result_from_the_worker_blocks_its_task_while_independent_tasks_are_still_dispatched() {
    let harness = harness().await;
    let (ws, goal) = setup(&harness).await;
    let deploy = ready_task(&harness, &ws, &goal, "部署", &[]).await;
    let docs = ready_task(&harness, &ws, &goal, "寫文件", &[]).await;
    let deploy_run = dispatch_ok(&harness, &ws, &deploy).await;
    harness.supervise().await;
    begin(&harness, &deploy_run, "deploy-1").await;

    // 外部回應逾時：Worker 不知道部署有沒有生效
    let (status, recorded) =
        record(&harness, &deploy_run, json!({ "idempotency_key": "deploy-1", "result": "unknown", "reason": "部署 API 逾時" })).await;

    assert_eq!((status, recorded["status"].as_str()), (StatusCode::OK, Some("unknown")), "{recorded}");
    let request_id = recorded["decision_request_id"].as_str().unwrap().to_string();
    assert_eq!(task_status(&harness, &ws, &deploy).await, "waiting_decision");
    // 同一個 Worker 想直接重送：被擋下
    let (_, retry) = begin(&harness, &deploy_run, "deploy-1").await;
    assert_eq!(retry["status"], "result_unknown", "{retry}");
    assert_eq!(retry["decision_request_id"], json!(request_id));
    // 獨立的 Task 照常派出並啟動
    let docs_run = dispatch_ok(&harness, &ws, &docs).await;
    harness.supervise().await;
    assert_eq!(status_of(&run_view(&harness, &ws, &docs_run).await), "running");

    // 查證沒有生效：恢復後的新 Run 可以再送一次（第二次嘗試）
    harness.agent_adapter.emit(&deploy_run, AdapterEvent::Exited { success: false });
    harness.supervise().await;
    answer(&harness, &ws, &request_id, OPTION_VERIFIED_NOT_DONE).await;
    harness.supervise().await;
    let second_run = latest_run(&harness, &ws, &deploy).await;
    let (_, retried) = begin(&harness, &second_run, "deploy-1").await;
    assert_eq!((retried["status"].as_str(), retried["attempt"].as_i64()), (Some("proceed"), Some(2)), "{retried}");
}

#[tokio::test]
async fn an_unknown_operation_can_only_be_closed_by_verifying_whether_it_took_effect() {
    let harness = harness().await;
    let (ws, goal) = setup(&harness).await;
    let deploy = ready_task(&harness, &ws, &goal, "部署", &[]).await;
    let rid = dispatch_ok(&harness, &ws, &deploy).await;
    harness.supervise().await;
    begin(&harness, &rid, "deploy-1").await;
    let (_, recorded) =
        record(&harness, &rid, json!({ "idempotency_key": "deploy-1", "result": "unknown", "reason": "部署 API 逾時" })).await;
    let request_id = recorded["decision_request_id"].as_str().unwrap().to_string();

    // 裁定中心只提供「接受查證選項」與「延後」
    assert_eq!(decision(&harness, &ws, &request_id).await["allowed_actions"], json!(["accept", "defer"]));
    // 其他處理方式會讓請求結案、操作卻停在結果未知：一律拒絕，單筆與批次都一樣
    for body in [
        json!({ "action": "reject" }),
        json!({ "action": "answer", "note": "應該好了" }),
        json!({ "action": "investigate", "note": "去查部署紀錄" }),
        json!({ "action": "modify", "option_id": OPTION_VERIFIED_DONE, "note": "條件" }),
    ] {
        let (status, refused) = try_answer(&harness, &ws, &request_id, body.clone()).await;
        assert_eq!((status, error_code(&refused)), (StatusCode::BAD_REQUEST, "decision_action_not_allowed"), "{body}：{refused}");
    }
    let revision = decision(&harness, &ws, &request_id).await["revision"].clone();
    let item = json!({ "workspace_id": ws, "request_id": request_id, "expected_revision": revision, "action": "reject" });
    let (status, batch) = harness.send_json(Method::POST, "/api/v1/decisions/batch", Some(json!({ "items": [item] }))).await;
    assert_eq!(status, StatusCode::OK, "{batch}");
    assert_eq!((&batch["results"][0]["ok"], &batch["results"][0]["error"]), (&json!(false), &json!("decision_action_not_allowed")));
    // 延後：請求仍待處理，Task 仍等待
    let (status, deferred) = try_answer(&harness, &ws, &request_id, json!({ "action": "defer" })).await;
    assert_eq!(status, StatusCode::OK, "{deferred}");
    assert_eq!(decision(&harness, &ws, &request_id).await["status"], "pending");
    assert_eq!(external_operations(&harness, &ws).await[0]["status"], "unknown");
    assert_eq!(task_status(&harness, &ws, &deploy).await, "waiting_decision");

    // 查證之後才結案
    answer(&harness, &ws, &request_id, OPTION_VERIFIED_DONE).await;
    assert_eq!(external_operations(&harness, &ws).await[0]["status"], "succeeded");
}

#[tokio::test]
async fn a_lost_response_to_a_reported_result_is_safe_to_retry_and_a_succeeded_operation_is_never_sent_again() {
    let mut harness = harness().await;
    let (ws, goal) = setup(&harness).await;
    let tid = ready_task(&harness, &ws, &goal, "部署", &[]).await;
    let rid = dispatch_ok(&harness, &ws, &tid).await;
    harness.supervise().await;
    let (_, first) = begin(&harness, &rid, "deploy-1").await;
    let (_, repeated_begin) = begin(&harness, &rid, "deploy-1").await;
    assert_eq!((repeated_begin["status"].as_str(), repeated_begin["existing"].as_bool()), (Some("proceed"), Some(true)));
    assert_eq!(repeated_begin["operation_id"], first["operation_id"]);

    let result = json!({ "idempotency_key": "deploy-1", "result": "succeeded", "receipt": "release #42" });
    let (status, _) = record(&harness, &rid, result.clone()).await;
    assert_eq!(status, StatusCode::OK);
    // 回應遺失，Runtime 重建之後 Worker 重送同一份結果
    harness.restart().await;
    harness.supervise().await;
    let (status, again) = record(&harness, &rid, result).await;
    assert_eq!((status, again["status"].as_str()), (StatusCode::OK, Some("succeeded")), "{again}");
    let (status, conflicting) =
        record(&harness, &rid, json!({ "idempotency_key": "deploy-1", "result": "failed", "reason": "改口" })).await;
    assert_eq!((status, error_code(&conflicting)), (StatusCode::CONFLICT, "external_operation_already_recorded"));

    let (_, after) = begin(&harness, &rid, "deploy-1").await;
    assert_eq!((after["status"].as_str(), after["receipt"].as_str()), (Some("already_succeeded"), Some("release #42")));
    // Run 結束不影響已有結果的操作
    harness.agent_adapter.emit(&rid, AdapterEvent::Exited { success: true });
    harness.supervise().await;
    assert_eq!(external_operations(&harness, &ws).await[0]["status"], "succeeded");
    assert_eq!(task_status(&harness, &ws, &tid).await, "verifying");
}

#[tokio::test]
async fn reusing_an_idempotency_key_for_a_different_request_is_refused() {
    let harness = harness().await;
    let (ws, goal) = setup(&harness).await;
    let tid = ready_task(&harness, &ws, &goal, "部署", &[]).await;
    let rid = dispatch_ok(&harness, &ws, &tid).await;
    harness.supervise().await;
    begin(&harness, &rid, "deploy-1").await;
    let token = harness.agent_credential(&rid).unwrap();

    let other = json!({ "idempotency_key": "deploy-1", "operation": "deploy", "target": "staging" });
    let (status, body) = harness.call_tool(&token, TOOL_BEGIN_EXTERNAL_OPERATION, other).await;
    let (missing_status, missing) =
        record(&harness, &rid, json!({ "idempotency_key": "never-begun", "result": "succeeded", "receipt": "x" })).await;

    assert_eq!((status, error_code(&body)), (StatusCode::CONFLICT, "idempotency_key_conflict"), "{body}");
    assert_eq!((missing_status, error_code(&missing)), (StatusCode::NOT_FOUND, "external_operation_not_found"));
    let operations = external_operations(&harness, &ws).await;
    assert_eq!((operations.len(), operations[0]["target"].as_str()), (1, Some("production")));
}

// ============================================================ 磁碟滿與資料庫忙碌

#[tokio::test]
async fn a_full_disk_halts_new_dispatch_and_the_ui_shows_a_reason_it_can_act_on() {
    let harness = harness().await;
    let (ws, goal) = setup(&harness).await;
    let first = ready_task(&harness, &ws, &goal, "已派出", &[]).await;
    let second = ready_task(&harness, &ws, &goal, "之後才派", &[]).await;
    let rid = dispatch_ok(&harness, &ws, &first).await;
    harness.inject_storage_fault(Some(StorageFaultKind::DiskFull));

    harness.supervise().await;
    harness.supervise().await;

    assert_eq!(harness.process_control.spawn_count(), 0, "磁碟滿時不啟動任何程序");
    let (status, refused) = dispatch(&harness, &ws, &second).await;
    assert_eq!((status, error_code(&refused)), (StatusCode::SERVICE_UNAVAILABLE, "storage_disk_full"), "{refused}");
    assert!(refused["message"].as_str().unwrap().contains("釋放"), "訊息說明怎麼處理：{refused}");
    let storage = health(&harness).await["storage"].clone();
    assert_eq!(storage["dispatch_halted"], true, "{storage}");
    assert_eq!(storage["fault"]["kind"], "disk_full");
    assert!(storage["fault"]["summary"].as_str().unwrap().contains("磁碟空間不足"), "{storage}");
    assert!(storage["fault"]["action"].as_str().unwrap().contains("自動恢復派工"), "{storage}");

    // 空間恢復：下一次處理試寫成功，解除停止並照常啟動
    harness.inject_storage_fault(None);
    harness.supervise().await;
    assert_eq!(health(&harness).await["storage"]["dispatch_halted"], false);
    assert_eq!(status_of(&run_view(&harness, &ws, &rid).await), "running");
    assert_eq!(harness.process_control.spawn_count(), 1);
    assert_eq!(dispatch(&harness, &ws, &second).await.0, StatusCode::CREATED);
}

#[tokio::test]
async fn a_database_busy_fault_is_shown_with_its_own_reason() {
    let harness = harness().await;
    let (ws, goal) = setup(&harness).await;
    let tid = ready_task(&harness, &ws, &goal, "派工", &[]).await;
    harness.inject_storage_fault(Some(StorageFaultKind::Busy));

    let (status, refused) = dispatch(&harness, &ws, &tid).await;

    assert_eq!((status, error_code(&refused)), (StatusCode::SERVICE_UNAVAILABLE, "storage_busy"), "{refused}");
    let storage = health(&harness).await["storage"].clone();
    assert_eq!((storage["dispatch_halted"].as_bool(), storage["fault"]["kind"].as_str()), (Some(true), Some("busy")));
    assert!(storage["fault"]["action"].as_str().unwrap().contains("關閉"), "{storage}");
}

#[tokio::test]
async fn a_disk_that_fills_up_right_after_spawn_loses_no_process_the_receipt_is_written_once_space_returns() {
    let harness = harness().await;
    let (ws, goal) = setup(&harness).await;
    let tid = ready_task(&harness, &ws, &goal, "部署", &[]).await;
    let rid = dispatch_ok(&harness, &ws, &tid).await;
    // 程序啟動之後、回執寫回之前磁碟滿了
    let store = harness.store();
    harness.process_control.run_on_next_spawn(move || store.inject_write_fault(Some(StorageFaultKind::DiskFull)));

    harness.supervise().await;
    assert_eq!(status_of(&run_view(&harness, &ws, &rid).await), "starting", "回執沒寫進去，launch intent 還在");
    assert_eq!(health(&harness).await["storage"]["dispatch_halted"], true);

    harness.inject_storage_fault(None);
    harness.supervise().await;

    let run = run_view(&harness, &ws, &rid).await;
    assert_eq!(status_of(&run), "running", "以 command_id 找回程序並補寫回執：{run}");
    assert_eq!(harness.process_control.spawn_count(), 1, "沒有第二次 spawn");
}
