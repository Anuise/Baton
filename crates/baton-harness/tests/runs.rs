//! 情境測試：Supervisor、程序身分與 Worker Run 端到端（票 10），經由 Runtime harness 的真實 HTTP API 驗證。
//!
//! 全部使用 fake ProcessControl／AgentAdapter 與 fake Clock：測試決定何時讓 Supervisor 處理一次（`supervise`）、
//! 時間走多久、程序存活與否、Adapter 回報什麼事件，以及 Runtime 在 spawn 的哪一點「當掉」。
//! 只斷言外部行為：HTTP 回應、重啟後仍在、Port 被呼叫的方式（例如「沒有第二次 spawn」）；
//! `harness.db()` 只用來確認事件與 Outbox 這類必須看落地內容的事實。

use baton_api::{CLIENT_HEADER, CLIENT_HEADER_VALUE};
use baton_domain::auth::Role;
use baton_domain::run::{HEARTBEAT_INTERVAL_MS, LEASE_DURATION_MS};
use baton_domain::Timestamp;
use baton_harness::fakes::HangPoint;
use baton_harness::Harness;
use baton_ports::{AdapterEvent, Clock, Liveness, StopResult};
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

const WORKSPACES: &str = "/api/v1/workspaces";

/// 2023-11-14 22:13:20.123 UTC
const CLOCK_START: Timestamp = Timestamp::from_millis(1_700_000_000_123);

// --- 共用輔助 ---

fn plain_dir(harness: &Harness, name: &str) -> String {
    let dir = harness.data_dir().join("projects").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    dir.to_string_lossy().into_owned()
}

async fn new_workspace(harness: &Harness, name: &str) -> (String, String) {
    let path = plain_dir(harness, name);
    let (status, body) = harness.send_json(Method::POST, WORKSPACES, Some(json!({ "path": path }))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    (body["id"].as_str().unwrap().to_string(), body["path"].as_str().unwrap().to_string())
}

fn tasks_path(ws: &str) -> String {
    format!("{WORKSPACES}/{ws}/tasks")
}

fn task_path(ws: &str, tid: &str) -> String {
    format!("{WORKSPACES}/{ws}/tasks/{tid}")
}

fn run_path(ws: &str, rid: &str) -> String {
    format!("{WORKSPACES}/{ws}/runs/{rid}")
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

/// 一個 Workspace、一個 Goal，回傳 (workspace_id, workspace_path, goal_id)。
async fn setup(harness: &Harness) -> (String, String, String) {
    let (ws, path) = new_workspace(harness, "main").await;
    let (status, goal) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/goals"), Some(json!({ "title": "目標" }))).await;
    assert_eq!(status, StatusCode::CREATED, "{goal}");
    (ws, path, goal["id"].as_str().unwrap().to_string())
}

/// 建立 Task（不需要任何能力的完整契約）並讓它 ready，回傳識別。
async fn ready_task(harness: &Harness, ws: &str, goal: &str, title: &str) -> String {
    ready_task_with(harness, ws, goal, title, &[]).await
}

async fn ready_task_with(harness: &Harness, ws: &str, goal: &str, title: &str, capabilities: &[&str]) -> String {
    let body = json!({ "goal_id": goal, "title": title, "contract": contract(capabilities) });
    let (status, task) = harness.send_json(Method::POST, &tasks_path(ws), Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    let tid = task["id"].as_str().unwrap().to_string();
    let (status, ready) = harness.send_json(Method::POST, &format!("{}/ready", task_path(ws, &tid)), None).await;
    assert_eq!(status, StatusCode::OK, "{ready}");
    tid
}

async fn dispatch(harness: &Harness, ws: &str, tid: &str) -> (StatusCode, Value) {
    harness.send_json(Method::POST, &format!("{}/dispatch", task_path(ws, tid)), None).await
}

/// 派工並要求成功，回傳 Run 識別。
async fn dispatch_ok(harness: &Harness, ws: &str, tid: &str) -> String {
    let (status, run) = dispatch(harness, ws, tid).await;
    assert_eq!(status, StatusCode::CREATED, "{run}");
    run["id"].as_str().unwrap().to_string()
}

async fn run_view(harness: &Harness, ws: &str, rid: &str) -> Value {
    let (status, run) = harness.send_json(Method::GET, &run_path(ws, rid), None).await;
    assert_eq!(status, StatusCode::OK, "{run}");
    run
}

async fn task_status(harness: &Harness, ws: &str, tid: &str) -> String {
    let (status, detail) = harness.send_json(Method::GET, &task_path(ws, tid), None).await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    detail["task"]["status"].as_str().unwrap().to_string()
}

async fn cancel_run(harness: &Harness, ws: &str, rid: &str) -> (StatusCode, Value) {
    harness.send_json(Method::POST, &format!("{}/cancel", run_path(ws, rid)), None).await
}

fn status_of(run: &Value) -> &str {
    run["status"].as_str().unwrap()
}

fn error_code(body: &Value) -> &str {
    body["error"].as_str().unwrap_or_default()
}

fn event_types(harness: &Harness, aggregate_type: &str, id: &str) -> Vec<String> {
    let db = harness.db();
    let mut statement = db
        .prepare("SELECT event_type FROM event WHERE aggregate_type = ?1 AND aggregate_id = ?2 ORDER BY seq")
        .unwrap();
    statement.query_map(rusqlite::params![aggregate_type, id], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
}

/// 派工 Outbox 中尚未處理的 Run 數。
fn undelivered_outbox(harness: &Harness) -> i64 {
    harness.db().query_row("SELECT COUNT(*) FROM outbox WHERE delivered_at IS NULL", [], |r| r.get(0)).unwrap()
}

fn held_leases(harness: &Harness, task_id: &str) -> i64 {
    harness
        .db()
        .query_row("SELECT COUNT(*) FROM execution_lease WHERE task_id = ?1 AND released_at IS NULL", [task_id], |r| r.get(0))
        .unwrap()
}

/// 啟動 Harness 並建立一個可派工的帳號（票 19 之後沒有可派工的帳號就不能建立 Run）。
async fn harness() -> Harness {
    let harness = Harness::builder().clock_at(CLOCK_START).start().await;
    harness.add_account("主要帳號", "main@example.com").await;
    harness
}

/// 目前唯一的帳號 Profile 識別。
async fn only_profile_id(harness: &Harness) -> String {
    let (status, list) = harness.send_json(Method::GET, "/api/v1/profiles", None).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    list["profiles"][0]["id"].as_str().unwrap().to_string()
}

// ============================================================ 派工

#[tokio::test]
async fn dispatch_creates_a_queued_run_with_a_lease_and_starts_no_process() {
    let harness = harness().await;
    let (ws, path, goal) = setup(&harness).await;
    let tid = ready_task(&harness, &ws, &goal, "整理資料").await;

    let (status, run) = dispatch(&harness, &ws, &tid).await;

    assert_eq!(status, StatusCode::CREATED, "{run}");
    assert_eq!((status_of(&run), run["generation"].as_i64(), run["task_id"].as_str()), ("queued", Some(1), Some(tid.as_str())));
    assert_eq!(run["profile_id"].as_str(), Some(only_profile_id(&harness).await.as_str()), "Run 記錄 Scheduler 選的帳號");
    assert_eq!(run["lease"]["expires_at"].as_i64(), Some(CLOCK_START.as_millis() + LEASE_DURATION_MS));
    assert!(run["lease"]["released_at"].is_null(), "派工當下就持有寫入所有權");
    assert!(run["command_id"].is_null() && run["process"].is_null(), "launch intent 與程序身分要等 Supervisor 處理");
    assert_eq!(task_status(&harness, &ws, &tid).await, "running");
    assert_eq!(harness.process_control.spawn_count(), 0, "派工交易本身不啟動任何程序");
    assert_eq!(undelivered_outbox(&harness), 1);
    let rid = run["id"].as_str().unwrap();
    assert_eq!(event_types(&harness, "run", rid), vec!["run_created"]);
    assert_eq!(event_types(&harness, "task", &tid).last().map(String::as_str), Some("task_status_changed"));
    let _ = path;
}

#[tokio::test]
async fn a_task_that_is_not_ready_cannot_be_dispatched_and_nothing_is_written() {
    let harness = harness().await;
    let (ws, _, goal) = setup(&harness).await;
    let body = json!({ "goal_id": goal, "title": "還沒 ready", "contract": contract(&[]) });
    let (_, task) = harness.send_json(Method::POST, &tasks_path(&ws), Some(body)).await;
    let pending = task["id"].as_str().unwrap().to_string();
    let running = ready_task(&harness, &ws, &goal, "已派工").await;
    dispatch_ok(&harness, &ws, &running).await;
    let events_before: i64 = harness.db().query_row("SELECT COUNT(*) FROM event", [], |r| r.get(0)).unwrap();

    let (pending_status, pending_body) = dispatch(&harness, &ws, &pending).await;
    let (again_status, again_body) = dispatch(&harness, &ws, &running).await;
    let (missing_status, missing_body) = dispatch(&harness, &ws, "task_nope").await;

    assert_eq!((pending_status, error_code(&pending_body)), (StatusCode::CONFLICT, "task_not_ready"), "{pending_body}");
    assert_eq!((again_status, error_code(&again_body)), (StatusCode::CONFLICT, "task_not_ready"), "{again_body}");
    assert_eq!((missing_status, error_code(&missing_body)), (StatusCode::NOT_FOUND, "task_not_found"), "{missing_body}");
    let events_after: i64 = harness.db().query_row("SELECT COUNT(*) FROM event", [], |r| r.get(0)).unwrap();
    assert_eq!(events_after, events_before, "被拒絕的派工不追加任何事件");
}

#[tokio::test]
async fn dispatch_re_checks_admission_so_a_revoked_capability_queues_it_with_an_authorization_reason() {
    let harness = harness().await;
    let (ws, _, goal) = setup(&harness).await;
    confirm_template(&harness, &ws, "trusted_dev_p3").await;
    let tid = ready_task_with(&harness, &ws, &goal, "要跑建置", &["run_build_test"]).await;
    // ready 之後授權被改成不含 run_build_test
    confirm_template(&harness, &ws, "research_docs").await;

    let (status, body) = dispatch(&harness, &ws, &tid).await;

    // 票 19：授權不足不再是 409，而是排隊（202）並說明原因；Task 留在 ready，沒有 Run、沒有 Outbox 項目
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert_eq!(body["queued"], true);
    assert_eq!(body["task_id"], json!(tid));
    assert_eq!(body["reasons"][0]["category"], "authorization", "{body}");
    assert_eq!(body["reasons"][0]["code"], "capability_not_granted", "{body}");
    assert_eq!(task_status(&harness, &ws, &tid).await, "ready", "排隊時 Task 不變");
    let runs: i64 = harness.db().query_row("SELECT COUNT(*) FROM run", [], |r| r.get(0)).unwrap();
    assert_eq!(runs, 0);
    assert_eq!(undelivered_outbox(&harness), 0);
}

#[tokio::test]
async fn the_run_records_the_authorization_revision_it_was_admitted_under() {
    let harness = harness().await;
    let (ws, _, goal) = setup(&harness).await;
    confirm_template(&harness, &ws, "trusted_dev_p3").await;
    let tid = ready_task_with(&harness, &ws, &goal, "要跑建置", &["run_build_test"]).await;

    let (_, run) = dispatch(&harness, &ws, &tid).await;

    assert_eq!(run["authorization_revision"], 1);
    assert_eq!(run["contract_version"], 1);
}

#[tokio::test]
async fn a_task_with_unfinished_dependencies_is_queued_instead_of_dispatched() {
    let harness = harness().await;
    let (ws, _, goal) = setup(&harness).await;
    let first = ready_task(&harness, &ws, &goal, "先做").await;
    let second = ready_task(&harness, &ws, &goal, "後做").await;
    let (status, _) = harness
        .send_json(Method::POST, &format!("{}/dependencies", task_path(&ws, &second)), Some(json!({ "depends_on": first })))
        .await;
    assert_eq!(status, StatusCode::CREATED);

    let (blocked_status, blocked) = dispatch(&harness, &ws, &second).await;
    let (free_status, _) = dispatch(&harness, &ws, &first).await;

    // 票 19：相依未完成不再是 409，而是排隊（202）；Task 仍是 ready，沒有 Run
    assert_eq!(blocked_status, StatusCode::ACCEPTED, "{blocked}");
    assert_eq!(blocked["queued"], true);
    assert_eq!(blocked["reasons"][0]["category"], "dependency", "{blocked}");
    assert_eq!(free_status, StatusCode::CREATED);
    assert_eq!(task_status(&harness, &ws, &second).await, "ready");

    // 目前沒有任何 API 能把 Task 標為 completed（驗證屬於後續的票），所以只確認相依未完成時 Supervisor 不會提前派出它
    harness.supervise().await;
    assert_eq!(task_status(&harness, &ws, &second).await, "ready", "相依還沒完成，Supervisor 不會派出它");
}

#[tokio::test]
async fn concurrent_dispatches_of_one_task_produce_exactly_one_effective_writer() {
    let harness = harness().await;
    let (ws, _, goal) = setup(&harness).await;
    let tid = ready_task(&harness, &ws, &goal, "搶著派").await;

    let attempts = futures::future::join_all((0..8).map(|_| dispatch(&harness, &ws, &tid))).await;

    let created = attempts.iter().filter(|(s, _)| *s == StatusCode::CREATED).count();
    let refused = attempts.iter().filter(|(s, b)| *s == StatusCode::CONFLICT && error_code(b) == "task_not_ready").count();
    assert_eq!((created, refused), (1, 7), "{attempts:?}");
    assert_eq!(held_leases(&harness, &tid), 1);
    let runs: i64 = harness.db().query_row("SELECT COUNT(*) FROM run", [], |r| r.get(0)).unwrap();
    assert_eq!(runs, 1);
}

#[tokio::test]
async fn an_agent_credential_cannot_dispatch_or_cancel_runs() {
    let harness = harness().await;
    let (ws, _, goal) = setup(&harness).await;
    let tid = ready_task(&harness, &ws, &goal, "Agent 不能派工").await;
    let rid = {
        let other = ready_task(&harness, &ws, &goal, "另一個").await;
        dispatch_ok(&harness, &ws, &other).await
    };
    let token = harness.issue_credential(Role::Agent, None).await;
    let requests = [
        (Method::POST, format!("{}/dispatch", task_path(&ws, &tid))),
        (Method::GET, format!("{WORKSPACES}/{ws}/runs")),
        (Method::GET, run_path(&ws, &rid)),
        (Method::POST, format!("{}/cancel", run_path(&ws, &rid))),
    ];

    for (method, path) in requests {
        let response = harness
            .request(method.clone(), &path)
            .bearer_auth(&token)
            .header(CLIENT_HEADER, CLIENT_HEADER_VALUE)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{method} {path}");
    }

    assert_eq!(task_status(&harness, &ws, &tid).await, "ready");
    assert_eq!(status_of(&run_view(&harness, &ws, &rid).await), "queued");
}

// ============================================================ 啟動、程序身分與回執

#[tokio::test]
async fn the_supervisor_persists_intent_then_starts_the_process_once_and_records_its_identity() {
    let harness = harness().await;
    let (ws, path, goal) = setup(&harness).await;
    let tid = ready_task(&harness, &ws, &goal, "整理資料").await;
    let rid = dispatch_ok(&harness, &ws, &tid).await;

    harness.supervise().await;
    harness.supervise().await;

    let run = run_view(&harness, &ws, &rid).await;
    assert_eq!(status_of(&run), "running", "{run}");
    assert_eq!(harness.process_control.spawn_count(), 1, "再次處理不會重複啟動");
    let spawned = &harness.process_control.spawned()[0];
    assert_eq!(Some(spawned.command_id.as_str()), run["command_id"].as_str(), "啟動時帶的就是先持久化的 command_id");
    let workdir = format!(r"{path}\.baton\outputs\{tid}");
    assert_eq!(spawned.working_dir.as_deref(), Some(workdir.as_str()), "研究 Task 在自己的輸出目錄中執行（票 20）");
    let identity = harness.process_control.identity_of(&spawned.command_id).unwrap();
    assert_eq!(run["process"]["pid"].as_u64(), Some(u64::from(identity.pid)));
    assert_eq!(run["process"]["created_at_millis"].as_i64(), Some(identity.created_at_millis));
    assert_eq!(run["process"]["command_hash"], json!(identity.command_hash), "程序身分 = PID + 建立時間 + 指令雜湊");
    assert_eq!(undelivered_outbox(&harness), 0, "回執寫回後派工 Outbox 視為處理完畢");
    assert_eq!(event_types(&harness, "run", &rid), vec!["run_created", "run_launch_started", "run_started"]);
    assert_eq!(task_status(&harness, &ws, &tid).await, "running");
}

#[tokio::test]
async fn a_failed_launch_fails_the_run_and_frees_the_task() {
    let harness = harness().await;
    let (ws, _, goal) = setup(&harness).await;
    let tid = ready_task(&harness, &ws, &goal, "啟動失敗").await;
    let rid = dispatch_ok(&harness, &ws, &tid).await;
    harness.process_control.fail_next_spawn("找不到 claude");

    harness.supervise().await;

    let run = run_view(&harness, &ws, &rid).await;
    assert_eq!(status_of(&run), "failed", "{run}");
    assert!(run["end_reason"].as_str().unwrap().contains("找不到 claude"));
    assert_eq!(task_status(&harness, &ws, &tid).await, "ready");
    assert_eq!(held_leases(&harness, &tid), 0);
    assert_eq!(harness.process_control.spawn_count(), 0);
}

#[tokio::test]
async fn crash_after_spawn_before_the_receipt_is_found_again_by_command_id_without_a_second_spawn() {
    let mut harness = harness().await;
    let (ws, _, goal) = setup(&harness).await;
    let tid = ready_task(&harness, &ws, &goal, "整理資料").await;
    let rid = dispatch_ok(&harness, &ws, &tid).await;
    harness.process_control.hang_next_spawn_at(HangPoint::AfterSpawn);

    harness.supervise_until(harness.process_control.wait_for_spawn_hang()).await;
    let before = run_view(&harness, &ws, &rid).await;
    assert_eq!(status_of(&before), "starting", "launch intent 已持久化，回執還沒寫回：{before}");
    assert!(before["process"].is_null());
    assert_eq!(harness.process_control.spawn_count(), 1);

    harness.restart().await;
    harness.supervise().await;

    let after = run_view(&harness, &ws, &rid).await;
    assert_eq!(status_of(&after), "running", "{after}");
    let identity = harness.process_control.identity_of(before["command_id"].as_str().unwrap()).unwrap();
    assert_eq!(after["process"]["pid"].as_u64(), Some(u64::from(identity.pid)), "找回的是原本那個程序");
    assert_eq!(harness.process_control.spawn_count(), 1, "不重複啟動");
    assert_eq!(held_leases(&harness, &tid), 1);
    assert_eq!(task_status(&harness, &ws, &tid).await, "running");
}

#[tokio::test]
async fn crash_before_spawn_never_respawns_and_the_run_is_interrupted_for_the_user_to_redispatch() {
    let mut harness = harness().await;
    let (ws, _, goal) = setup(&harness).await;
    let tid = ready_task(&harness, &ws, &goal, "整理資料").await;
    let rid = dispatch_ok(&harness, &ws, &tid).await;
    harness.process_control.hang_next_spawn_at(HangPoint::BeforeSpawn);

    harness.supervise_until(harness.process_control.wait_for_spawn_hang()).await;
    assert_eq!(status_of(&run_view(&harness, &ws, &rid).await), "starting");
    harness.restart().await;
    harness.supervise().await;
    harness.supervise().await;

    let run = run_view(&harness, &ws, &rid).await;
    assert_eq!(status_of(&run), "interrupted", "{run}");
    assert!(run["end_reason"].as_str().unwrap().contains("launch_outcome_unknown"));
    assert_eq!(harness.process_control.spawn_count(), 0, "結果不明的啟動不自動重送");
    assert_eq!(held_leases(&harness, &tid), 0);
    assert_eq!(task_status(&harness, &ws, &tid).await, "ready");

    // 使用者重新派工：新的 Run 世代加一，正常啟動
    let second = dispatch_ok(&harness, &ws, &tid).await;
    harness.supervise().await;
    let second_run = run_view(&harness, &ws, &second).await;
    assert_eq!((status_of(&second_run), second_run["generation"].as_i64()), ("running", Some(2)));
    assert_eq!(harness.process_control.spawn_count(), 1);
}

#[tokio::test]
async fn restarting_the_runtime_while_a_run_is_running_keeps_it_and_never_starts_it_again() {
    let mut harness = harness().await;
    let (ws, _, goal) = setup(&harness).await;
    let tid = ready_task(&harness, &ws, &goal, "整理資料").await;
    let rid = dispatch_ok(&harness, &ws, &tid).await;
    harness.supervise().await;
    let before = run_view(&harness, &ws, &rid).await;

    harness.restart().await;
    harness.supervise().await;
    harness.agent_adapter.emit(&rid, AdapterEvent::Exited { success: true });
    harness.supervise().await;

    let after = run_view(&harness, &ws, &rid).await;
    assert_eq!(before["process"], after["process"], "重啟前後是同一個程序");
    assert_eq!(status_of(&after), "succeeded", "重啟後仍能接上事件並結束：{after}");
    assert_eq!(harness.process_control.spawn_count(), 1);
}

// ============================================================ 結束

#[tokio::test]
async fn a_successful_exit_ends_only_the_run_and_never_completes_the_task() {
    let harness = harness().await;
    let (ws, _, goal) = setup(&harness).await;
    let tid = ready_task(&harness, &ws, &goal, "整理資料").await;
    let rid = dispatch_ok(&harness, &ws, &tid).await;
    harness.agent_adapter.script_next_run(vec![AdapterEvent::Exited { success: true }]);

    harness.supervise().await;
    harness.supervise().await;

    let run = run_view(&harness, &ws, &rid).await;
    assert_eq!(status_of(&run), "succeeded", "{run}");
    assert!(!run["lease"]["released_at"].is_null(), "結束後釋放寫入所有權");
    let task = task_status(&harness, &ws, &tid).await;
    assert_eq!(task, "verifying", "程序正常退出只讓 Task 進入驗證，不等於完成");
    assert_ne!(task, "completed");
    assert_eq!(harness.process_control.spawn_count(), 1);
    assert!(event_types(&harness, "run", &rid).contains(&"run_succeeded".to_string()));
    assert!(!event_types(&harness, "task", &tid).contains(&"task_completed".to_string()));
}

#[tokio::test]
async fn a_failed_exit_fails_the_run_and_returns_the_task_to_ready_for_another_run() {
    let harness = harness().await;
    let (ws, _, goal) = setup(&harness).await;
    let tid = ready_task(&harness, &ws, &goal, "整理資料").await;
    let first = dispatch_ok(&harness, &ws, &tid).await;
    harness.agent_adapter.script_next_run(vec![AdapterEvent::Exited { success: false }]);

    harness.supervise().await;

    assert_eq!(status_of(&run_view(&harness, &ws, &first).await), "failed");
    assert_eq!(task_status(&harness, &ws, &tid).await, "ready");
    let second = dispatch_ok(&harness, &ws, &tid).await;
    assert_ne!(first, second);
    assert_eq!(run_view(&harness, &ws, &second).await["generation"], 2);
}

// ============================================================ heartbeat 與 lease

#[tokio::test]
async fn heartbeats_renew_the_lease_and_an_unrenewed_expired_lease_only_triggers_a_check() {
    let harness = harness().await;
    let (ws, _, goal) = setup(&harness).await;
    let tid = ready_task(&harness, &ws, &goal, "整理資料").await;
    let rid = dispatch_ok(&harness, &ws, &tid).await;
    harness.supervise().await;
    let identity = harness.process_control.spawned().first().map(|s| harness.process_control.identity_of(&s.command_id).unwrap()).unwrap();
    let lease_expiry = |run: &Value| run["lease"]["expires_at"].as_i64().unwrap();
    let started = harness.clock.now().as_millis();
    assert_eq!(lease_expiry(&run_view(&harness, &ws, &rid).await), started + LEASE_DURATION_MS);

    // 每 15 秒一次的 heartbeat：lease 一直被續約，從來不需要核對程序
    for _ in 0..8 {
        harness.clock.advance_millis(HEARTBEAT_INTERVAL_MS);
        harness.agent_adapter.emit(&rid, AdapterEvent::Heartbeat);
        harness.supervise().await;
    }
    let renewed = run_view(&harness, &ws, &rid).await;
    assert_eq!(lease_expiry(&renewed), harness.clock.now().as_millis() + LEASE_DURATION_MS);
    assert_eq!(harness.process_control.inspect_count(), 0, "lease 沒過期就不核對程序");

    // heartbeat 停了：過期的 lease 只觸發核對，程序狀態不明時什麼都不改
    harness.clock.advance_millis(LEASE_DURATION_MS + 1);
    harness.process_control.script_liveness(&identity, Liveness::Unknown);
    harness.supervise().await;
    let unsure = run_view(&harness, &ws, &rid).await;
    assert_eq!(harness.process_control.inspect_count(), 1);
    assert_eq!(status_of(&unsure), "running", "lease 過期不直接判定程序死亡：{unsure}");
    assert!(unsure["lease"]["released_at"].is_null());
    assert_eq!(task_status(&harness, &ws, &tid).await, "running");

    // 核對後確認程序還活著：續約，Run 不變
    harness.process_control.script_liveness(&identity, Liveness::Alive);
    harness.supervise().await;
    let alive = run_view(&harness, &ws, &rid).await;
    assert_eq!(status_of(&alive), "running");
    assert_eq!(lease_expiry(&alive), harness.clock.now().as_millis() + LEASE_DURATION_MS);
    assert!(event_types(&harness, "run", &rid).contains(&"run_lease_verified".to_string()));

    // 再次過期，這次核對確認程序已經不在：才中斷 Run、釋放所有權、Task 退回 ready
    harness.clock.advance_millis(LEASE_DURATION_MS + 1);
    harness.process_control.script_liveness(&identity, Liveness::Dead);
    harness.supervise().await;
    let dead = run_view(&harness, &ws, &rid).await;
    assert_eq!(status_of(&dead), "interrupted", "{dead}");
    assert!(!dead["lease"]["released_at"].is_null());
    assert_eq!(task_status(&harness, &ws, &tid).await, "ready");
    assert_eq!(harness.process_control.spawn_count(), 1, "中斷之後不會自動重新啟動");
}

#[tokio::test]
async fn a_late_heartbeat_after_the_run_ended_changes_nothing() {
    let harness = harness().await;
    let (ws, _, goal) = setup(&harness).await;
    let tid = ready_task(&harness, &ws, &goal, "整理資料").await;
    let first = dispatch_ok(&harness, &ws, &tid).await;
    harness.supervise().await;
    harness.agent_adapter.emit(&first, AdapterEvent::Exited { success: false });
    harness.supervise().await;
    let second = dispatch_ok(&harness, &ws, &tid).await;
    harness.supervise().await;
    let before = run_view(&harness, &ws, &second).await;

    // 已結束的舊 Run 又冒出 heartbeat 與結束事件：不影響新的寫入所有者
    harness.agent_adapter.emit(&first, AdapterEvent::Heartbeat);
    harness.agent_adapter.emit(&first, AdapterEvent::Exited { success: true });
    harness.clock.advance_millis(5_000);
    harness.supervise().await;

    assert_eq!(run_view(&harness, &ws, &second).await, before);
    assert_eq!(status_of(&run_view(&harness, &ws, &first).await), "failed");
    assert_eq!(task_status(&harness, &ws, &tid).await, "running");
}

// ============================================================ 取消

#[tokio::test]
async fn cancelling_a_running_run_stops_the_process_then_releases_the_task() {
    let harness = harness().await;
    let (ws, _, goal) = setup(&harness).await;
    let tid = ready_task(&harness, &ws, &goal, "整理資料").await;
    let rid = dispatch_ok(&harness, &ws, &tid).await;
    harness.supervise().await;
    let identity = harness.process_control.identity_of(run_view(&harness, &ws, &rid).await["command_id"].as_str().unwrap()).unwrap();

    let (status, requested) = cancel_run(&harness, &ws, &rid).await;

    assert_eq!(status, StatusCode::OK, "{requested}");
    assert_eq!(status_of(&requested), "running", "程序還沒確認停止，Run 不會直接變成 cancelled");
    assert!(!requested["cancel_requested_at"].is_null());
    assert!(requested["lease"]["released_at"].is_null());
    let (conflict_status, conflict) = dispatch(&harness, &ws, &tid).await;
    assert_eq!((conflict_status, error_code(&conflict)), (StatusCode::CONFLICT, "task_not_ready"), "{conflict}");
    assert!(harness.process_control.stop_calls().is_empty(), "要求取消本身不阻塞在停止上");

    harness.supervise().await;

    let cancelled = run_view(&harness, &ws, &rid).await;
    assert_eq!(status_of(&cancelled), "cancelled", "{cancelled}");
    assert_eq!(harness.process_control.stop_calls(), vec![identity]);
    assert!(!cancelled["lease"]["released_at"].is_null());
    assert_eq!(task_status(&harness, &ws, &tid).await, "ready");
    let (again_status, _) = cancel_run(&harness, &ws, &rid).await;
    assert_eq!(again_status, StatusCode::OK, "重複取消是冪等的");
    assert_eq!(harness.process_control.stop_calls().len(), 1);
}

#[tokio::test]
async fn a_process_that_does_not_stop_keeps_the_write_ownership_until_it_is_confirmed_stopped() {
    let harness = harness().await;
    let (ws, _, goal) = setup(&harness).await;
    let tid = ready_task(&harness, &ws, &goal, "整理資料").await;
    let rid = dispatch_ok(&harness, &ws, &tid).await;
    harness.supervise().await;
    cancel_run(&harness, &ws, &rid).await;
    harness.process_control.set_stop_result(StopResult::StillAlive);

    harness.supervise().await;
    harness.supervise().await;

    let stuck = run_view(&harness, &ws, &rid).await;
    assert_eq!(status_of(&stuck), "running", "舊 Worker 確認停止之前，Run 不能結束：{stuck}");
    assert!(stuck["lease"]["released_at"].is_null());
    assert_eq!(held_leases(&harness, &tid), 1);
    assert_eq!(task_status(&harness, &ws, &tid).await, "running");
    assert_eq!(dispatch(&harness, &ws, &tid).await.0, StatusCode::CONFLICT, "新的 Run 不能進入同一個 Task");
    assert_eq!(harness.process_control.stop_calls().len(), 2, "每次處理都重試停止");

    harness.process_control.set_stop_result(StopResult::ForceKilled);
    harness.supervise().await;

    assert_eq!(status_of(&run_view(&harness, &ws, &rid).await), "cancelled");
    assert_eq!(held_leases(&harness, &tid), 0);
    assert_eq!(task_status(&harness, &ws, &tid).await, "ready");
    assert!(dispatch(&harness, &ws, &tid).await.0 == StatusCode::CREATED, "確認停止之後才能再派");
}

#[tokio::test]
async fn cancelling_a_run_that_has_not_launched_cancels_it_at_once_and_no_process_ever_starts() {
    let harness = harness().await;
    let (ws, _, goal) = setup(&harness).await;
    let tid = ready_task(&harness, &ws, &goal, "整理資料").await;
    let rid = dispatch_ok(&harness, &ws, &tid).await;

    let (status, cancelled) = cancel_run(&harness, &ws, &rid).await;
    harness.supervise().await;

    assert_eq!(status, StatusCode::OK, "{cancelled}");
    assert_eq!(status_of(&cancelled), "cancelled");
    assert_eq!(harness.process_control.spawn_count(), 0);
    assert_eq!(undelivered_outbox(&harness), 0);
    assert_eq!(task_status(&harness, &ws, &tid).await, "ready");
    assert_eq!(held_leases(&harness, &tid), 0);
}

#[tokio::test]
async fn cancelling_the_task_also_stops_its_active_run_and_the_task_stays_cancelled() {
    let harness = harness().await;
    let (ws, _, goal) = setup(&harness).await;
    let tid = ready_task(&harness, &ws, &goal, "整理資料").await;
    let rid = dispatch_ok(&harness, &ws, &tid).await;
    harness.supervise().await;

    let (status, task) = harness.send_json(Method::POST, &format!("{}/cancel", task_path(&ws, &tid)), None).await;
    assert_eq!(status, StatusCode::OK, "{task}");
    assert_eq!(status_of(&run_view(&harness, &ws, &rid).await), "running", "程序確認停止之前 Run 還在");
    harness.supervise().await;

    assert_eq!(status_of(&run_view(&harness, &ws, &rid).await), "cancelled");
    assert_eq!(task_status(&harness, &ws, &tid).await, "cancelled", "Run 結束不會把使用者取消的 Task 拉回來");
    assert_eq!(harness.process_control.stop_calls().len(), 1);
    assert_eq!(held_leases(&harness, &tid), 0);
}

// ============================================================ 面板資料

#[tokio::test]
async fn the_task_board_and_run_endpoints_show_the_runs_of_the_workspace_only() {
    let harness = harness().await;
    let (ws, _, goal) = setup(&harness).await;
    let (other_ws, _) = new_workspace(&harness, "other").await;
    let tid = ready_task(&harness, &ws, &goal, "整理資料").await;
    let rid = dispatch_ok(&harness, &ws, &tid).await;
    harness.supervise().await;

    let (_, board) = harness.send_json(Method::GET, &tasks_path(&ws), None).await;
    let (list_status, list) = harness.send_json(Method::GET, &format!("{WORKSPACES}/{ws}/runs"), None).await;
    let (_, other_list) = harness.send_json(Method::GET, &format!("{WORKSPACES}/{other_ws}/runs"), None).await;
    let (wrong_status, wrong) = harness.send_json(Method::GET, &run_path(&other_ws, &rid), None).await;
    let (unknown_status, unknown) = harness.send_json(Method::GET, &format!("{WORKSPACES}/nope/runs"), None).await;

    assert_eq!(board["runs"].as_array().unwrap().len(), 1, "{board}");
    assert_eq!(board["runs"][0]["id"], json!(rid));
    assert_eq!(board["runs"][0]["status"], "running");
    assert_eq!(list_status, StatusCode::OK);
    assert_eq!(list["runs"][0]["id"], json!(rid));
    assert_eq!(other_list["runs"].as_array().unwrap().len(), 0);
    assert_eq!((wrong_status, error_code(&wrong)), (StatusCode::NOT_FOUND, "run_not_found"), "{wrong}");
    assert_eq!((unknown_status, error_code(&unknown)), (StatusCode::NOT_FOUND, "workspace_not_found"), "{unknown}");
}

// --- 授權 ---

/// 確認某個授權範本（trusted_dev_p3 需要承認風險）。
async fn confirm_template(harness: &Harness, ws: &str, template_id: &str) {
    let path = format!("{WORKSPACES}/{ws}/authorization");
    let (status, view) = harness.send_json(Method::GET, &path, None).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    let grants = view["templates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == template_id)
        .unwrap_or_else(|| panic!("沒有範本 {template_id}"))["grants"]
        .clone();
    let body = json!({ "template_id": template_id, "grants": grants, "acknowledged_risk": true, "acknowledged_expansion": true });
    let (status, revision) = harness.send_json(Method::PUT, &path, Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{revision}");
}
