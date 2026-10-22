//! 情境測試：Guard 與執行前檢查（票 38，spec D-28、使用者故事 60、93），經由 Runtime harness 的真實 HTTP API 驗證。
//!
//! 使用者在知識分頁把已驗證的教訓轉成 Guard（建立即啟用）；Run 准入（派工）時檢查範圍涵蓋該 Task 的 Guard，
//! 違反就不建立 Run，派工回 202、等待原因是 `guard` 類別並帶來源教訓；被攔下的重犯記錄在教訓底下，
//! 沒被攔下的重犯由使用者回報；Guard 的成效（攔下次數、未攔下的回報次數）在 Guard 列表上。

use baton_api::{CLIENT_HEADER, CLIENT_HEADER_VALUE};
use baton_domain::Timestamp;
use baton_harness::Harness;
use baton_ports::AdapterEvent;
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

const WORKSPACES: &str = "/api/v1/workspaces";
const KNOWLEDGE: &str = "/api/v1/knowledge";
const GUARDS: &str = "/api/v1/guards";
const SCHEDULING: &str = "/api/v1/scheduling";
const CLOCK_START: Timestamp = Timestamp::from_millis(1_700_000_000_000);

// --- 共用輔助 ---

async fn harness() -> Harness {
    let harness = Harness::builder().clock_at(CLOCK_START).start().await;
    // 派工需要一個可派工的帳號（票 19）
    harness.add_account("主要帳號", "main@example.com").await;
    harness
}

async fn new_workspace(harness: &Harness, name: &str) -> String {
    let dir = harness.data_dir().join("projects").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let (status, body) =
        harness.send_json(Method::POST, WORKSPACES, Some(json!({ "path": dir.to_string_lossy() }))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["id"].as_str().unwrap().to_string()
}

async fn new_goal(harness: &Harness, ws: &str) -> String {
    let (status, goal) =
        harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/goals"), Some(json!({ "title": "上線" }))).await;
    assert_eq!(status, StatusCode::CREATED, "{goal}");
    goal["id"].as_str().unwrap().to_string()
}

fn filled(text: &str) -> Value {
    json!({ "status": "filled", "text": text })
}

fn contract(done_criteria: &str, capabilities: &[&str]) -> Value {
    json!({
        "objective": filled("部署新版"),
        "inputs": filled("既有設定"),
        "scope": filled("正式環境"),
        "deliverables": filled("新版上線"),
        "done_criteria": filled(done_criteria),
        "blocker_handling": filled("卡住時回報"),
        "required_capabilities": capabilities,
    })
}

fn task_path(ws: &str, tid: &str) -> String {
    format!("{WORKSPACES}/{ws}/tasks/{tid}")
}

/// 建立並標記 ready 的 Task，回傳 task_id。
async fn ready_task(harness: &Harness, ws: &str, goal: &str, title: &str, contract: Value) -> String {
    let body = json!({ "goal_id": goal, "title": title, "contract": contract });
    let (status, task) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks"), Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    let tid = task["id"].as_str().unwrap().to_string();
    mark_ready(harness, ws, &tid).await;
    tid
}

async fn mark_ready(harness: &Harness, ws: &str, tid: &str) {
    let (status, ready) = harness.send_json(Method::POST, &format!("{}/ready", task_path(ws, tid)), None).await;
    assert_eq!(status, StatusCode::OK, "{ready}");
}

async fn dispatch(harness: &Harness, ws: &str, tid: &str) -> (StatusCode, Value) {
    harness.send_json(Method::POST, &format!("{}/dispatch", task_path(ws, tid)), None).await
}

/// 使用者在知識分頁建立已驗證的教訓，回傳 lesson_id。
async fn lesson(harness: &Harness, scope: Value, title: &str) -> String {
    let body = json!({
        "kind": "lesson",
        "scope": scope,
        "title": title,
        "lesson": { "trigger": "部署", "root_cause": "沒有退路", "resolution": "先寫回滾步驟" },
    });
    let (status, entry) = harness.send_json(Method::POST, KNOWLEDGE, Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{entry}");
    entry["id"].as_str().unwrap().to_string()
}

fn workspace_scope(ws: &str) -> Value {
    json!({ "kind": "workspace", "workspace_id": ws })
}

/// 把教訓轉成 Guard（建立即啟用），回傳 Guard。
async fn guard(harness: &Harness, lesson_id: &str, rule: Value) -> Value {
    let (status, guard) =
        harness.send_json(Method::POST, GUARDS, Some(json!({ "lesson_id": lesson_id, "rule": rule }))).await;
    assert_eq!(status, StatusCode::CREATED, "{guard}");
    guard
}

fn must_mention_rollback() -> Value {
    json!({ "kind": "preflight", "field": "done_criteria", "must_mention": "回滾" })
}

async fn guards_of(harness: &Harness, ws: &str) -> Vec<Value> {
    let (status, list) = harness.send_json(Method::GET, &format!("{GUARDS}?workspace_id={ws}"), None).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    list["guards"].as_array().unwrap().clone()
}

async fn recurrences(harness: &Harness, lesson_id: &str) -> Vec<Value> {
    let (status, list) =
        harness.send_json(Method::GET, &format!("{KNOWLEDGE}/{lesson_id}/recurrences"), None).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    list["recurrences"].as_array().unwrap().clone()
}

/// 排程總覽中這個 Task 的等待原因。
async fn queued_reasons(harness: &Harness, tid: &str) -> Vec<Value> {
    let (status, overview) = harness.send_json(Method::GET, SCHEDULING, None).await;
    assert_eq!(status, StatusCode::OK, "{overview}");
    overview["queue"].as_array().unwrap().iter().find(|q| q["task_id"] == tid).expect("Task 應該在佇列中")["reasons"]
        .as_array()
        .unwrap()
        .clone()
}

async fn task_status(harness: &Harness, ws: &str, tid: &str) -> String {
    let (_, detail) = harness.send_json(Method::GET, &task_path(ws, tid), None).await;
    detail["task"]["status"].as_str().unwrap().to_string()
}

async fn update_contract(harness: &Harness, ws: &str, tid: &str, contract: Value) {
    let (status, body) =
        harness.send_json(Method::PUT, &format!("{}/contract", task_path(ws, tid)), Some(json!({ "contract": contract }))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
}

async fn confirm_template(harness: &Harness, ws: &str, template_id: &str) {
    let path = format!("{WORKSPACES}/{ws}/authorization");
    let (_, view) = harness.send_json(Method::GET, &path, None).await;
    let grants = view["templates"].as_array().unwrap().iter().find(|t| t["id"] == template_id).unwrap()["grants"].clone();
    let body = json!({ "template_id": template_id, "grants": grants, "acknowledged_risk": true });
    let (status, revision) = harness.send_json(Method::PUT, &path, Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{revision}");
}

fn guard_reason(reasons: &[Value]) -> &Value {
    reasons.iter().find(|r| r["category"] == "guard").unwrap_or_else(|| panic!("沒有 Guard 原因：{reasons:?}"))
}

// --- 准入時檢查 ---

#[tokio::test]
async fn an_enabled_guard_blocks_admission_of_a_related_task_and_shows_the_source_lesson() {
    let harness = harness().await;
    let ws = new_workspace(&harness, "app").await;
    let goal = new_goal(&harness, &ws).await;
    let lesson_id = lesson(&harness, workspace_scope(&ws), "部署前要寫回滾計畫").await;
    let created = guard(&harness, &lesson_id, must_mention_rollback()).await;
    assert_eq!((created["status"].as_str(), created["active"].as_bool()), (Some("enabled"), Some(true)));
    let tid = ready_task(&harness, &ws, &goal, "部署", contract("新版上線", &[])).await;

    let (status, queued) = dispatch(&harness, &ws, &tid).await;

    // 派工沒有建立 Run：留在佇列，原因是 Guard，並帶來源教訓
    assert_eq!(status, StatusCode::ACCEPTED, "{queued}");
    let reasons = queued["reasons"].as_array().unwrap();
    let reason = guard_reason(reasons);
    assert_eq!(reason["code"], "guard_violated");
    assert_eq!(reason["guard"]["guard_id"], created["id"]);
    assert_eq!(reason["guard"]["lesson_id"], lesson_id.as_str());
    assert_eq!(reason["guard"]["lesson_title"], "部署前要寫回滾計畫");
    let message = reason["message"].as_str().unwrap();
    assert!(message.contains("完成與驗證條件") && message.contains("回滾"), "{message}");
    assert!(message.contains("部署前要寫回滾計畫"), "{message}");

    harness.supervise().await;
    assert_eq!(harness.process_control.spawn_count(), 0, "被 Guard 擋下的 Task 不能啟動任何程序");
    assert_eq!(task_status(&harness, &ws, &tid).await, "ready");
    // 工作面板的佇列也看得到同一個原因
    assert_eq!(guard_reason(&queued_reasons(&harness, &tid).await)["guard"]["lesson_id"], lesson_id.as_str());

    // 依教訓修正契約（退回 pending）後重新 ready、派工就通過
    update_contract(&harness, &ws, &tid, contract("新版上線，附回滾步驟", &[])).await;
    mark_ready(&harness, &ws, &tid).await;
    let (status, run) = dispatch(&harness, &ws, &tid).await;
    assert_eq!(status, StatusCode::CREATED, "{run}");
}

#[tokio::test]
async fn guards_only_check_tasks_within_their_lessons_scope() {
    let harness = harness().await;
    let ws = new_workspace(&harness, "app").await;
    let other_ws = new_workspace(&harness, "docs").await;
    let goal = new_goal(&harness, &ws).await;
    let other_goal = new_goal(&harness, &other_ws).await;
    let guarded = ready_task(&harness, &ws, &goal, "部署", contract("新版上線", &[])).await;
    let unguarded = ready_task(&harness, &ws, &goal, "整理文件", contract("新版上線", &[])).await;
    let elsewhere = ready_task(&harness, &other_ws, &other_goal, "別的專案", contract("新版上線", &[])).await;
    let task_scope = json!({ "kind": "task", "workspace_id": ws, "task_id": guarded });
    let lesson_id = lesson(&harness, task_scope, "這個部署要回滾計畫").await;
    guard(&harness, &lesson_id, must_mention_rollback()).await;

    assert_eq!(dispatch(&harness, &ws, &guarded).await.0, StatusCode::ACCEPTED);
    assert_eq!(dispatch(&harness, &ws, &unguarded).await.0, StatusCode::CREATED, "同 Workspace 的其他 Task 不受影響");
    assert_eq!(dispatch(&harness, &other_ws, &elsewhere).await.0, StatusCode::CREATED, "其他 Workspace 不受影響");
}

#[tokio::test]
async fn a_tool_restriction_guard_blocks_tasks_that_require_the_forbidden_capability() {
    let harness = harness().await;
    let ws = new_workspace(&harness, "app").await;
    confirm_template(&harness, &ws, "trusted_dev_p3").await;
    let goal = new_goal(&harness, &ws).await;
    let lesson_id = lesson(&harness, json!({ "kind": "global" }), "不要在這台機器上跑建置").await;
    guard(&harness, &lesson_id, json!({ "kind": "tool_restriction", "capability": "run_build_test" })).await;
    let builds = ready_task(&harness, &ws, &goal, "建置", contract("通過", &["run_build_test"])).await;
    let reads = ready_task(&harness, &ws, &goal, "閱讀", contract("通過", &["read_files"])).await;

    let (status, queued) = dispatch(&harness, &ws, &builds).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{queued}");
    assert_eq!(guard_reason(queued["reasons"].as_array().unwrap())["guard"]["lesson_id"], lesson_id.as_str());
    assert_eq!(dispatch(&harness, &ws, &reads).await.0, StatusCode::CREATED);
}

#[tokio::test]
async fn a_retry_policy_guard_stops_redispatching_after_the_contract_failed_too_often() {
    let harness = harness().await;
    let ws = new_workspace(&harness, "app").await;
    let goal = new_goal(&harness, &ws).await;
    let lesson_id = lesson(&harness, workspace_scope(&ws), "同樣做法失敗一次就該換方法").await;
    guard(&harness, &lesson_id, json!({ "kind": "retry_policy", "max_failed_runs": 1 })).await;
    let tid = ready_task(&harness, &ws, &goal, "部署", contract("新版上線", &[])).await;

    harness.agent_adapter.script_next_run(vec![AdapterEvent::Exited { success: false }]);
    let (status, run) = dispatch(&harness, &ws, &tid).await;
    assert_eq!(status, StatusCode::CREATED, "第一次沒有失敗紀錄：{run}");
    harness.supervise().await;
    harness.supervise().await;
    assert_eq!(task_status(&harness, &ws, &tid).await, "ready", "失敗的 Run 讓 Task 回到 ready");

    let (status, queued) = dispatch(&harness, &ws, &tid).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{queued}");
    assert!(guard_reason(queued["reasons"].as_array().unwrap())["message"].as_str().unwrap().contains("失敗 1 次"));
    harness.supervise().await;
    assert_eq!(harness.process_control.spawn_count(), 1, "被擋下後不再啟動新的程序");
}

// --- 啟用與停用 ---

#[tokio::test]
async fn disabling_a_guard_lets_the_blocked_queued_task_dispatch_automatically() {
    let harness = harness().await;
    let ws = new_workspace(&harness, "app").await;
    let goal = new_goal(&harness, &ws).await;
    let lesson_id = lesson(&harness, workspace_scope(&ws), "部署前要寫回滾計畫").await;
    let created = guard(&harness, &lesson_id, must_mention_rollback()).await;
    let gid = created["id"].as_str().unwrap();
    let tid = ready_task(&harness, &ws, &goal, "部署", contract("新版上線", &[])).await;
    assert_eq!(dispatch(&harness, &ws, &tid).await.0, StatusCode::ACCEPTED);

    // 舊的 revision 被拒絕（兩個畫面不會互相覆蓋）
    let (status, stale) =
        harness.send_json(Method::POST, &format!("{GUARDS}/{gid}/disable"), Some(json!({ "expected_revision": 0 }))).await;
    assert_eq!((status, stale["error"].as_str()), (StatusCode::CONFLICT, Some("guard_revision_conflict")));

    let (status, disabled) =
        harness.send_json(Method::POST, &format!("{GUARDS}/{gid}/disable"), Some(json!({ "expected_revision": 1 }))).await;
    assert_eq!(status, StatusCode::OK, "{disabled}");
    assert_eq!((disabled["status"].as_str(), disabled["active"].as_bool()), (Some("disabled"), Some(false)));
    assert_eq!(disabled["inactive_reason"], "已停用");

    harness.supervise().await;
    assert_eq!(task_status(&harness, &ws, &tid).await, "running", "停用的 Guard 不再檢查，佇列自動派出");

    // 重新啟用後，對下一個相關 Task 再次生效
    let (status, enabled) =
        harness.send_json(Method::POST, &format!("{GUARDS}/{gid}/enable"), Some(json!({ "expected_revision": 2 }))).await;
    assert_eq!(status, StatusCode::OK, "{enabled}");
    let next = ready_task(&harness, &ws, &goal, "再部署", contract("新版上線", &[])).await;
    assert_eq!(dispatch(&harness, &ws, &next).await.0, StatusCode::ACCEPTED);
}

#[tokio::test]
async fn a_guard_stops_applying_once_its_lesson_is_revoked_and_cannot_be_reenabled() {
    let harness = harness().await;
    let ws = new_workspace(&harness, "app").await;
    let goal = new_goal(&harness, &ws).await;
    let lesson_id = lesson(&harness, workspace_scope(&ws), "部署前要寫回滾計畫").await;
    let gid = guard(&harness, &lesson_id, must_mention_rollback()).await["id"].as_str().unwrap().to_string();
    let tid = ready_task(&harness, &ws, &goal, "部署", contract("新版上線", &[])).await;
    assert_eq!(dispatch(&harness, &ws, &tid).await.0, StatusCode::ACCEPTED);

    let body = json!({ "expected_revision": 1, "reason": "其實有自動回滾" });
    let (status, revoked) = harness.send_json(Method::POST, &format!("{KNOWLEDGE}/{lesson_id}/revoke"), Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{revoked}");

    let listed = guards_of(&harness, &ws).await;
    let view = listed.iter().find(|g| g["id"] == gid.as_str()).unwrap();
    assert_eq!(view["active"], false);
    assert_eq!(view["inactive_reason"], "來源教訓已撤銷");
    assert_eq!(view["lesson"]["status"], "revoked");
    harness.supervise().await;
    assert_eq!(task_status(&harness, &ws, &tid).await, "running", "錯誤的教訓不再擋工作");

    // 停用後不能再以撤銷的教訓啟用
    let disable = json!({ "expected_revision": 1 });
    harness.send_json(Method::POST, &format!("{GUARDS}/{gid}/disable"), Some(disable)).await;
    let (status, refused) =
        harness.send_json(Method::POST, &format!("{GUARDS}/{gid}/enable"), Some(json!({ "expected_revision": 2 }))).await;
    assert_eq!((status, refused["error"].as_str()), (StatusCode::CONFLICT, Some("lesson_ineligible")));
}

#[tokio::test]
async fn only_effective_verified_lessons_can_become_guards_and_only_the_user_can_create_them() {
    let harness = harness().await;
    let ws = new_workspace(&harness, "app").await;
    let goal = new_goal(&harness, &ws).await;
    let tid = ready_task(&harness, &ws, &goal, "整理", contract("完成", &[])).await;
    let (_, run) = dispatch(&harness, &ws, &tid).await;
    let rid = run["id"].as_str().unwrap().to_string();
    harness.supervise().await;
    let token = harness.agent_credential(&rid).unwrap();

    // Agent 提交的候選教訓不能轉成 Guard
    let candidate = json!({ "title": "候選教訓", "scope": "workspace" });
    let (status, recorded) = harness.call_tool(&token, "record_lesson_candidate", candidate).await;
    assert_eq!(status, StatusCode::CREATED, "{recorded}");
    let candidate_id = recorded["entry_id"].as_str().unwrap();
    let body = json!({ "lesson_id": candidate_id, "rule": must_mention_rollback() });
    let (status, refused) = harness.send_json(Method::POST, GUARDS, Some(body)).await;
    assert_eq!((status, refused["error"].as_str()), (StatusCode::CONFLICT, Some("lesson_ineligible")));

    // 決策不是教訓
    let decision = json!({ "kind": "decision", "scope": workspace_scope(&ws), "title": "採用 SQLite" });
    let (_, decision) = harness.send_json(Method::POST, KNOWLEDGE, Some(decision)).await;
    let body = json!({ "lesson_id": decision["id"], "rule": must_mention_rollback() });
    let (status, refused) = harness.send_json(Method::POST, GUARDS, Some(body)).await;
    assert_eq!((status, refused["error"].as_str()), (StatusCode::CONFLICT, Some("lesson_ineligible")));

    // 不合規的規則
    let lesson_id = lesson(&harness, workspace_scope(&ws), "部署前要寫回滾計畫").await;
    let body = json!({ "lesson_id": lesson_id, "rule": { "kind": "retry_policy", "max_failed_runs": 0 } });
    let (status, refused) = harness.send_json(Method::POST, GUARDS, Some(body)).await;
    assert_eq!((status, refused["error"].as_str()), (StatusCode::BAD_REQUEST, Some("guard_invalid")));

    // Agent 憑證不能建立或停用 Guard（D-28：Guard 一律由使用者啟用）
    let response = harness
        .request(Method::POST, GUARDS)
        .bearer_auth(&token)
        .header(CLIENT_HEADER, CLIENT_HEADER_VALUE)
        .json(&json!({ "lesson_id": lesson_id, "rule": must_mention_rollback() }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"], "role_not_allowed");
    assert!(guards_of(&harness, &ws).await.is_empty());
}

// --- 重犯事件與成效 ---

#[tokio::test]
async fn a_prevented_recurrence_is_recorded_once_and_traceable_to_the_lesson() {
    let harness = harness().await;
    let ws = new_workspace(&harness, "app").await;
    let goal = new_goal(&harness, &ws).await;
    let lesson_id = lesson(&harness, workspace_scope(&ws), "部署前要寫回滾計畫").await;
    let gid = guard(&harness, &lesson_id, must_mention_rollback()).await["id"].as_str().unwrap().to_string();
    let tid = ready_task(&harness, &ws, &goal, "部署", contract("新版上線", &[])).await;

    dispatch(&harness, &ws, &tid).await;
    // 佇列每次重試都看到同一個違反：只記一次
    harness.supervise().await;
    harness.supervise().await;

    let found = recurrences(&harness, &lesson_id).await;
    assert_eq!(found.len(), 1, "{found:?}");
    let recurrence = &found[0];
    assert_eq!(recurrence["kind"], "prevented");
    assert_eq!(recurrence["lesson_id"], lesson_id.as_str());
    assert_eq!(recurrence["guard_id"], gid.as_str());
    assert_eq!((recurrence["workspace_id"].as_str(), recurrence["task_id"].as_str()), (Some(ws.as_str()), Some(tid.as_str())));
    assert_eq!(recurrence["contract_version"], 1);
    assert!(recurrence["message"].as_str().unwrap().contains("回滾"));

    // 事件時間線上也有同一筆，指向教訓
    let db = harness.db();
    let payload: String = db
        .query_row(
            "SELECT payload FROM event WHERE aggregate_type = 'lesson_recurrence' AND event_type = 'lesson_recurrence_recorded'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let payload: Value = serde_json::from_str(&payload).unwrap();
    assert_eq!((payload["lesson_id"].as_str(), payload["guard_id"].as_str()), (Some(lesson_id.as_str()), Some(gid.as_str())));

    // 成效：攔下 1 次
    let view = guards_of(&harness, &ws).await.into_iter().find(|g| g["id"] == gid.as_str()).unwrap();
    assert_eq!(view["prevented_count"], 1);
    assert!(view["last_prevented_at"].as_i64().is_some());

    // 改了契約但仍違反：是新的一次重犯
    update_contract(&harness, &ws, &tid, contract("新版上線（第二版）", &[])).await;
    mark_ready(&harness, &ws, &tid).await;
    dispatch(&harness, &ws, &tid).await;
    harness.supervise().await;
    assert_eq!(recurrences(&harness, &lesson_id).await.len(), 2);
}

#[tokio::test]
async fn the_user_reports_a_recurrence_that_slipped_through_and_it_counts_against_the_guard() {
    let harness = harness().await;
    let ws = new_workspace(&harness, "app").await;
    let goal = new_goal(&harness, &ws).await;
    let lesson_id = lesson(&harness, workspace_scope(&ws), "部署前要寫回滾計畫").await;
    let gid = guard(&harness, &lesson_id, must_mention_rollback()).await["id"].as_str().unwrap().to_string();
    let tid = ready_task(&harness, &ws, &goal, "部署", contract("附回滾步驟", &[])).await;

    let path = format!("{KNOWLEDGE}/{lesson_id}/recurrences");
    let body = json!({ "note": "契約寫了回滾，實際卻沒做", "workspace_id": ws, "task_id": tid });
    let (status, reported) = harness.send_json(Method::POST, &path, Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{reported}");
    assert_eq!((reported["kind"].as_str(), reported["guard_id"].as_str()), (Some("reported"), None));
    assert_eq!(reported["task_id"], tid.as_str());

    let found = recurrences(&harness, &lesson_id).await;
    assert_eq!(found.len(), 1);
    assert_eq!(found[0]["lesson_id"], lesson_id.as_str());
    let view = guards_of(&harness, &ws).await.into_iter().find(|g| g["id"] == gid.as_str()).unwrap();
    assert_eq!((view["prevented_count"].as_i64(), view["reported_count"].as_i64()), (Some(0), Some(1)));

    // 說明必填；不存在的 Task 被拒絕
    let (status, refused) = harness.send_json(Method::POST, &path, Some(json!({ "note": " " }))).await;
    assert_eq!((status, refused["error"].as_str()), (StatusCode::BAD_REQUEST, Some("guard_invalid")));
    let body = json!({ "note": "x", "workspace_id": ws, "task_id": "task_missing" });
    let (status, _) = harness.send_json(Method::POST, &path, Some(body)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn guards_and_recurrences_survive_a_runtime_restart() {
    let mut harness = harness().await;
    let ws = new_workspace(&harness, "app").await;
    let goal = new_goal(&harness, &ws).await;
    let lesson_id = lesson(&harness, workspace_scope(&ws), "部署前要寫回滾計畫").await;
    guard(&harness, &lesson_id, must_mention_rollback()).await;
    let tid = ready_task(&harness, &ws, &goal, "部署", contract("新版上線", &[])).await;
    dispatch(&harness, &ws, &tid).await;

    harness.restart().await;
    harness.supervise().await;

    assert_eq!(harness.process_control.spawn_count(), 0);
    assert_eq!(task_status(&harness, &ws, &tid).await, "ready");
    assert_eq!(guard_reason(&queued_reasons(&harness, &tid).await)["guard"]["lesson_id"], lesson_id.as_str());
    assert_eq!(recurrences(&harness, &lesson_id).await.len(), 1);
}
