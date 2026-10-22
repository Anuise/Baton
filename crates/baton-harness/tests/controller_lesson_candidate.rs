//! 情境測試：Controller 提交候選教訓（票 54，spec D-05、D-28），經由 Runtime harness 的真實 HTTP API 驗證。
//!
//! Controller 是腳本化的：測試在 Controller 的一輪進行中，以那一輪收到的 Controller 憑證呼叫 `record_lesson_candidate`。
//! - Controller 沒有自己的 Task，必須以 `task_id` 指定教訓來源的 Task（必須在它的 Workspace 內），範圍以這個 Task 解析；
//!   來源記為 `controller`（帶 Controller Run 與 Task），狀態一律是候選，使用者確認前不是有效知識。
//! - 依決策權限表判定「把候選教訓提交給使用者」：內建可自主處理；Workspace 收窄為必須送裁定時先建立裁定請求，
//!   核准後以相同內容再呼叫才寫入。
//! - Worker 版行為不變：範圍一律以自己的 Task 解析，送 `task_id` 也不能把教訓掛到別的 Task。

use baton_domain::knowledge::{TOOL_RECORD_LESSON_CANDIDATE, TOOL_SEARCH_KNOWLEDGE};
use baton_domain::Timestamp;
use baton_harness::Harness;
use baton_ports::AdapterEvent;
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

const WORKSPACES: &str = "/api/v1/workspaces";
const KNOWLEDGE: &str = "/api/v1/knowledge";
const CLOCK_START: Timestamp = Timestamp::from_millis(1_700_000_000_123);
/// 合併視窗（spec D-31）。
const WINDOW_MS: i64 = 5_000;

fn filled(text: &str) -> Value {
    json!({ "status": "filled", "text": text })
}

fn error_code(body: &Value) -> &str {
    body["error"].as_str().unwrap_or_default()
}

async fn harness() -> Harness {
    let harness = Harness::builder().clock_at(CLOCK_START).start().await;
    harness.add_account("帳號 1", "account1@example.com").await;
    harness
}

async fn new_workspace(harness: &Harness, name: &str) -> String {
    let dir = harness.data_dir().join("projects").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let (status, body) = harness.send_json(Method::POST, WORKSPACES, Some(json!({ "path": dir.to_string_lossy() }))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["id"].as_str().unwrap().to_string()
}

/// 在 Workspace 內建立 Goal 與 Task，回傳 (goal_id, task_id)。
async fn task(harness: &Harness, ws: &str) -> (String, String) {
    let (status, goal) =
        harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/goals"), Some(json!({ "title": "目標" }))).await;
    assert_eq!(status, StatusCode::CREATED, "{goal}");
    let contract = json!({
        "objective": filled("整理資料"),
        "inputs": filled("既有的資料"),
        "scope": filled("公開資料"),
        "deliverables": filled("比較表"),
        "done_criteria": filled("五個產品"),
        "blocker_handling": filled("卡住時回報"),
        "required_capabilities": [],
    });
    let body = json!({ "goal_id": goal["id"], "title": "整理", "contract": contract });
    let (status, task) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks"), Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    (goal["id"].as_str().unwrap().to_string(), task["id"].as_str().unwrap().to_string())
}

/// 使用者送出一則對話訊息，合併視窗過後喚起 Controller，回傳這一輪的 (Controller Run 識別, Controller 憑證)。
async fn wake_controller(harness: &Harness, ws: &str) -> (String, String) {
    let path = format!("{WORKSPACES}/{ws}/conversation/messages");
    let (status, body) = harness.send_json(Method::POST, &path, Some(json!({ "text": "開始" }))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    harness.clock.advance_millis(WINDOW_MS);
    harness.supervise().await;
    let (status, view) = harness.send_json(Method::GET, &format!("{WORKSPACES}/{ws}/conversation"), None).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    let run_id = view["active_run"]["id"].as_str().unwrap_or_else(|| panic!("Controller 沒有被喚起：{view}")).to_string();
    let token = harness.agent_credential(&run_id).expect("Controller 程序啟動時應該收到 Controller 憑證");
    (run_id, token)
}

/// 讓這一輪 Controller 回覆並成功結束。
async fn finish_turn(harness: &Harness, run_id: &str) {
    harness.agent_adapter.emit(run_id, AdapterEvent::Started { native_session_id: Some("native-1".into()) });
    harness.agent_adapter.emit(run_id, AdapterEvent::Message { text: "好了".into() });
    harness.agent_adapter.emit(run_id, AdapterEvent::Exited { success: true });
    harness.supervise().await;
}

fn lesson(task_id: &str, title: &str) -> Value {
    json!({
        "task_id": task_id,
        "title": title,
        "body": "快取殘留會讓整合測試誤判為通過",
        "trigger": "整合測試在第二次執行時才通過",
        "root_cause": "建置快取沒有失效",
        "resolution": "測試前清除快取目錄",
        "tags": ["測試"],
        "evidence": ["run 紀錄：第二次才通過"],
    })
}

async fn detail(harness: &Harness, id: &str) -> Value {
    let (status, body) = harness.send_json(Method::GET, &format!("{KNOWLEDGE}/{id}"), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["entry"].clone()
}

fn knowledge_rows(harness: &Harness) -> i64 {
    harness.db().query_row("SELECT COUNT(*) FROM knowledge_entry", [], |r| r.get(0)).unwrap()
}

fn titles(body: &Value) -> Vec<String> {
    body["entries"].as_array().unwrap().iter().map(|e| e["content"]["title"].as_str().unwrap().to_string()).collect()
}

/// 提交，預期 201，回傳條目識別。
async fn record(harness: &Harness, token: &str, body: Value) -> String {
    let (status, recorded) = harness.call_tool(token, TOOL_RECORD_LESSON_CANDIDATE, body).await;
    assert_eq!(status, StatusCode::CREATED, "{recorded}");
    assert_eq!(recorded["status"], "candidate", "{recorded}");
    assert_eq!(recorded["revision"], 1, "{recorded}");
    recorded["entry_id"].as_str().unwrap().to_string()
}

async fn answer(harness: &Harness, ws: &str, request_id: &str, option: &str) {
    let path = format!("{WORKSPACES}/{ws}/decisions/{request_id}");
    let (status, view) = harness.send_json(Method::GET, &path, None).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    let body = json!({ "option_id": option, "expected_revision": view["revision"] });
    let (status, answered) = harness.send_json(Method::POST, &format!("{path}/answer"), Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{answered}");
}

// ============================================================ 驗收條件

#[tokio::test]
async fn the_controller_records_a_candidate_lesson_for_a_task_of_its_workspace() {
    let harness = harness().await;
    let ws = new_workspace(&harness, "main").await;
    let (goal, tid) = task(&harness, &ws).await;
    let (controller_run, token) = wake_controller(&harness, &ws).await;

    // 預設 Task 範圍；請求指定的狀態與種類不被採信
    let mut body = lesson(&tid, "測試前先清除快取");
    body["status"] = json!("verified");
    body["kind"] = json!("decision");
    let id = record(&harness, &token, body).await;
    let entry = detail(&harness, &id).await;
    assert_eq!((entry["kind"].as_str(), entry["status"].as_str(), entry["effective"].as_bool()), (Some("lesson"), Some("candidate"), Some(false)));
    assert_eq!(entry["source"], json!({ "kind": "controller", "controller_run_id": controller_run, "task_id": tid }));
    assert_eq!(entry["scope"], json!({ "kind": "task", "workspace_id": ws, "goal_id": goal, "task_id": tid }));
    assert_eq!(entry["content"]["lesson"]["resolution"], "測試前清除快取目錄");
    assert_eq!(entry["content"]["evidence"], json!(["run 紀錄：第二次才通過"]));

    // Goal 與 Workspace 範圍一律以指定的 Task 解析
    let mut body = lesson(&tid, "目標範圍的教訓");
    body["scope"] = json!("goal");
    let goal_entry = detail(&harness, &record(&harness, &token, body).await).await;
    assert_eq!(goal_entry["scope"], json!({ "kind": "goal", "workspace_id": ws, "goal_id": goal, "task_id": null }));
    let mut body = lesson(&tid, "工作區範圍的教訓");
    body["scope"] = json!("workspace");
    let ws_entry = detail(&harness, &record(&harness, &token, body).await).await;
    assert_eq!(ws_entry["scope"], json!({ "kind": "workspace", "workspace_id": ws, "goal_id": null, "task_id": null }));

    // 使用者確認前不是有效知識：Controller 搜不到
    let (status, found) = harness.call_tool(&token, TOOL_SEARCH_KNOWLEDGE, json!({ "query": "快取" })).await;
    assert_eq!(status, StatusCode::OK, "{found}");
    assert!(titles(&found).is_empty(), "候選教訓不能被當作有效知識：{found}");

    // 使用者確認後才搜得到
    let (status, confirmed) =
        harness.send_json(Method::POST, &format!("{KNOWLEDGE}/{id}/confirm"), Some(json!({ "expected_revision": 1 }))).await;
    assert_eq!(status, StatusCode::OK, "{confirmed}");
    let (status, found) = harness.call_tool(&token, TOOL_SEARCH_KNOWLEDGE, json!({ "query": "快取" })).await;
    assert_eq!(status, StatusCode::OK, "{found}");
    assert_eq!(titles(&found), ["測試前先清除快取"]);
}

#[tokio::test]
async fn the_controller_cannot_record_lessons_outside_its_workspace_or_without_a_source_task() {
    let harness = harness().await;
    let ws = new_workspace(&harness, "main").await;
    let other = new_workspace(&harness, "other").await;
    let (_, tid) = task(&harness, &ws).await;
    let (_, foreign_task) = task(&harness, &other).await;
    let (controller_run, token) = wake_controller(&harness, &ws).await;
    let before = knowledge_rows(&harness);

    // 別的 Workspace 的 Task：找不到，什麼都不寫
    let (status, body) = harness.call_tool(&token, TOOL_RECORD_LESSON_CANDIDATE, lesson(&foreign_task, "越界")).await;
    assert_eq!((status, error_code(&body)), (StatusCode::NOT_FOUND, "task_not_found"), "{body}");

    // 全域範圍不能由 Agent 提交
    let mut global = lesson(&tid, "全域");
    global["scope"] = json!("global");
    let (status, body) = harness.call_tool(&token, TOOL_RECORD_LESSON_CANDIDATE, global).await;
    assert_eq!((status, error_code(&body)), (StatusCode::BAD_REQUEST, "knowledge_scope_invalid"), "{body}");

    // Controller 沒有自己的 Task，必須指定來源 Task
    let mut missing = lesson(&tid, "沒有來源");
    missing.as_object_mut().unwrap().remove("task_id");
    let (status, body) = harness.call_tool(&token, TOOL_RECORD_LESSON_CANDIDATE, missing).await;
    assert_eq!((status, error_code(&body)), (StatusCode::BAD_REQUEST, "knowledge_scope_invalid"), "{body}");

    // 聲稱別的 Workspace 被拒絕，不會改用請求裡的值
    let mut claimed = lesson(&foreign_task, "冒充");
    claimed["workspace_id"] = json!(other);
    let (status, body) = harness.call_tool(&token, TOOL_RECORD_LESSON_CANDIDATE, claimed).await;
    assert_eq!((status, error_code(&body)), (StatusCode::FORBIDDEN, "agent_scope_mismatch"), "{body}");

    assert_eq!(knowledge_rows(&harness), before, "被拒絕的請求沒有寫入任何條目");

    // 這一輪結束後憑證失效
    finish_turn(&harness, &controller_run).await;
    let (status, body) = harness.call_tool(&token, TOOL_RECORD_LESSON_CANDIDATE, lesson(&tid, "結束後")).await;
    assert_eq!((status, error_code(&body)), (StatusCode::UNAUTHORIZED, "credential_revoked"), "{body}");
    assert_eq!(knowledge_rows(&harness), before);
}

#[tokio::test]
async fn a_workspace_can_require_a_decision_before_the_controller_submits_a_lesson() {
    let harness = harness().await;
    let ws = new_workspace(&harness, "main").await;
    let (_, tid) = task(&harness, &ws).await;
    let (_, token) = wake_controller(&harness, &ws).await;

    // 收窄：把候選教訓提交給使用者必須送裁定
    let narrow = json!({ "expected_revision": 0, "entries": [{ "matter": "submit_lesson_candidate", "effective": "requires_decision" }] });
    let (status, table) = harness.send_json(Method::PUT, &format!("{WORKSPACES}/{ws}/decision-authority"), Some(narrow)).await;
    assert_eq!(status, StatusCode::OK, "{table}");
    let before = knowledge_rows(&harness);

    let (status, body) = harness.call_tool(&token, TOOL_RECORD_LESSON_CANDIDATE, lesson(&tid, "測試前先清除快取")).await;
    assert_eq!((status, error_code(&body)), (StatusCode::CONFLICT, "requires_decision"), "{body}");
    assert_eq!(body["matter"], "submit_lesson_candidate");
    let request_id = body["decision_request_id"].as_str().unwrap().to_string();
    assert_eq!(knowledge_rows(&harness), before, "裁定前不寫入");

    // 等待裁定期間再呼叫一次：沿用同一個請求
    let (status, again) = harness.call_tool(&token, TOOL_RECORD_LESSON_CANDIDATE, lesson(&tid, "測試前先清除快取")).await;
    assert_eq!((status, error_code(&again)), (StatusCode::CONFLICT, "requires_decision"), "{again}");
    assert_eq!((again["decision_request_id"].as_str(), again["existing"].as_bool()), (Some(request_id.as_str()), Some(true)));

    // 核准後以相同內容再呼叫才寫入
    answer(&harness, &ws, &request_id, "approve").await;
    let id = record(&harness, &token, lesson(&tid, "測試前先清除快取")).await;
    assert_eq!(detail(&harness, &id).await["source"]["kind"], "controller");
    assert_eq!(knowledge_rows(&harness), before + 1);

    // 使用者不核准：不寫入，回報被拒絕
    let (status, body) = harness.call_tool(&token, TOOL_RECORD_LESSON_CANDIDATE, lesson(&tid, "另一個教訓")).await;
    assert_eq!((status, error_code(&body)), (StatusCode::CONFLICT, "requires_decision"), "{body}");
    answer(&harness, &ws, body["decision_request_id"].as_str().unwrap(), "reject").await;
    let (status, body) = harness.call_tool(&token, TOOL_RECORD_LESSON_CANDIDATE, lesson(&tid, "另一個教訓")).await;
    assert_eq!((status, error_code(&body)), (StatusCode::CONFLICT, "decision_denied"), "{body}");
    assert_eq!(knowledge_rows(&harness), before + 1);
}

#[tokio::test]
async fn a_worker_cannot_attach_its_lesson_to_another_task_by_naming_it() {
    let harness = harness().await;
    let ws = new_workspace(&harness, "main").await;
    let (goal, own) = task(&harness, &ws).await;
    let (_, other) = task(&harness, &ws).await;
    let (status, ready) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks/{own}/ready"), None).await;
    assert_eq!(status, StatusCode::OK, "{ready}");
    let (status, run) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks/{own}/dispatch"), None).await;
    assert_eq!(status, StatusCode::CREATED, "{run}");
    let rid = run["id"].as_str().unwrap().to_string();
    harness.supervise().await;
    let token = harness.agent_credential(&rid).expect("Run 啟動時應該收到 Agent 憑證");

    // Worker 送的 task_id 不被採信：範圍與來源一律是自己的 Task
    let entry = detail(&harness, &record(&harness, &token, lesson(&other, "測試前先清除快取")).await).await;
    assert_eq!(entry["source"], json!({ "kind": "agent", "run_id": rid, "generation": 1, "task_id": own }));
    assert_eq!(entry["scope"], json!({ "kind": "task", "workspace_id": ws, "goal_id": goal, "task_id": own }));
}
