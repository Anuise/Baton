//! 情境測試：完整裁定中心與決策權限表（票 35），經由 Runtime harness 的真實 HTTP API 驗證。
//!
//! 沿用票 22（`decisions.rs`）的做法：Worker 的憑證取自 fake ProcessControl 記錄的啟動環境變數，
//! Worker 的行為以 scripted fake Adapter 推進，全部使用 fake Clock 與 fake Port。
//! 只斷言外部行為：HTTP 回應、Task／裁定請求的可觀察狀態；`harness.db()` 只用來確認「沒有寫入任何裁定」這類落地事實。

use baton_domain::agent::{TOOL_PERMISSION_PROMPT, TOOL_REQUEST_DECISION};
use baton_domain::Timestamp;
use baton_harness::Harness;
use baton_ports::AdapterEvent;
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

const WORKSPACES: &str = "/api/v1/workspaces";
const PENDING: &str = "/api/v1/decisions/pending";
const BATCH: &str = "/api/v1/decisions/batch";
const SCHEDULING: &str = "/api/v1/scheduling";

/// Controller 處理事項前先問權限的 Agent 工具（票 35 新增）。
const TOOL_CONTROLLER_ACTION: &str = "controller_action";

/// 2023-11-14 22:13:20.123 UTC
const CLOCK_START: Timestamp = Timestamp::from_millis(1_700_000_000_123);
const MINUTE_MS: i64 = 60_000;
const DAY_MS: i64 = 24 * 60 * MINUTE_MS;

/// spec D-15 的內建決策權限表。
const BUILTIN_AUTONOMOUS: [&str; 7] = [
    "arrange_tasks",
    "choose_playbook",
    "dispatch_research",
    "classify_finding_as_fix",
    "redispatch_within_budget",
    "create_investigation_task",
    "submit_lesson_candidate",
];
const BUILTIN_REQUIRES_DECISION: [&str; 9] = [
    "product_direction",
    "modify_done_criteria",
    "exceed_authorization",
    "external_write",
    "dismiss_review_finding",
    "skip_stage_or_gate",
    "unknown_outcome_operation",
    "accept_design_deviation",
    "next_step_after_budget",
];
const BUILTIN_HUMAN_ONLY: [&str; 1] = ["retry_after_recovery_failures"];

// --- 共用輔助（沿用 decisions.rs 的寫法） ---

async fn harness() -> Harness {
    let harness = Harness::builder().clock_at(CLOCK_START).start().await;
    // 派工需要一個可派工的帳號（票 19）
    harness.add_account("主要帳號", "main@example.com").await;
    harness
}

async fn new_workspace(harness: &Harness, name: &str) -> String {
    let dir = harness.data_dir().join("projects").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let body = json!({ "path": dir.to_string_lossy() });
    let (status, body) = harness.send_json(Method::POST, WORKSPACES, Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["id"].as_str().unwrap().to_string()
}

fn filled(text: &str) -> Value {
    json!({ "status": "filled", "text": text })
}

fn contract(objective: &str) -> Value {
    json!({
        "objective": filled(objective),
        "inputs": filled("既有的資料"),
        "scope": filled("公開資料"),
        "deliverables": filled("比較表"),
        "done_criteria": filled("五個產品"),
        "blocker_handling": filled("卡住時回報"),
        "required_capabilities": [],
    })
}

fn task_path(ws: &str, tid: &str) -> String {
    format!("{WORKSPACES}/{ws}/tasks/{tid}")
}

fn decisions_path(ws: &str) -> String {
    format!("{WORKSPACES}/{ws}/decisions")
}

fn answer_path(ws: &str, rid: &str) -> String {
    format!("{WORKSPACES}/{ws}/decisions/{rid}/answer")
}

fn merge_path(ws: &str, rid: &str) -> String {
    format!("{WORKSPACES}/{ws}/decisions/{rid}/merge")
}

fn links_path(ws: &str, rid: &str) -> String {
    format!("{WORKSPACES}/{ws}/decisions/{rid}/links")
}

fn authority_path(ws: &str) -> String {
    format!("{WORKSPACES}/{ws}/decision-authority")
}

async fn new_goal(harness: &Harness, ws: &str) -> String {
    let (status, goal) =
        harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/goals"), Some(json!({ "title": "目標" }))).await;
    assert_eq!(status, StatusCode::CREATED, "{goal}");
    goal["id"].as_str().unwrap().to_string()
}

/// 在既有的 Workspace 與 Goal 下建立一個 ready 的 Task。
async fn ready_task(harness: &Harness, ws: &str, goal: &str, title: &str) -> String {
    let body = json!({ "goal_id": goal, "title": title, "contract": contract(title) });
    let (status, task) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks"), Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    let tid = task["id"].as_str().unwrap().to_string();
    let (status, ready) = harness.send_json(Method::POST, &format!("{}/ready", task_path(ws, &tid)), None).await;
    assert_eq!(status, StatusCode::OK, "{ready}");
    tid
}

/// 派工並讓 Supervisor 啟動它，回傳 (run_id, Worker 的 Agent 憑證)。
async fn start_run(harness: &Harness, ws: &str, tid: &str) -> (String, String) {
    let (status, run) = harness.send_json(Method::POST, &format!("{}/dispatch", task_path(ws, tid)), None).await;
    assert_eq!(status, StatusCode::CREATED, "{run}");
    let rid = run["id"].as_str().unwrap().to_string();
    harness.supervise().await;
    let token = harness.agent_credential(&rid).expect("Run 啟動時應該把 Agent 憑證放進程序的環境變數");
    (rid, token)
}

/// Worker 結束（依裁定中心的約定：保存 checkpoint、report_blocked 之後結束）。
async fn worker_exits(harness: &Harness, rid: &str) {
    harness.agent_adapter.emit(rid, AdapterEvent::Exited { success: true });
    harness.supervise().await;
}

/// 某個 Task 的全部 Run。
async fn runs_of(harness: &Harness, ws: &str, tid: &str) -> Vec<Value> {
    let (status, list) = harness.send_json(Method::GET, &format!("{WORKSPACES}/{ws}/runs"), None).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    list["runs"].as_array().unwrap().iter().filter(|r| r["task_id"] == tid).cloned().collect()
}

/// 裁定後恢復的 Task 已在派工佇列：讓 Supervisor 處理一次，取回每個 Task 新派出的執行中 Run 與它的憑證。
async fn resumed_runs(harness: &Harness, ws: &str, tids: &[&str]) -> Vec<(String, String)> {
    harness.supervise().await;
    let mut started = Vec::new();
    for tid in tids {
        let runs = runs_of(harness, ws, tid).await;
        let run = runs.iter().rev().find(|r| r["status"] == "running").unwrap_or_else(|| panic!("{tid} 應該已重新派工：{runs:?}"));
        let run_id = run["id"].as_str().unwrap().to_string();
        let token = harness.agent_credential(&run_id).expect("新的 Run 應該有 Agent 憑證");
        started.push((run_id, token));
    }
    started
}

/// 派工佇列中的 Task（排程總覽）。
async fn queued_task_ids(harness: &Harness) -> Vec<String> {
    let (status, overview) = harness.send_json(Method::GET, SCHEDULING, None).await;
    assert_eq!(status, StatusCode::OK, "{overview}");
    overview["queue"].as_array().unwrap().iter().map(|q| q["task_id"].as_str().unwrap().to_string()).collect()
}

async fn task_detail(harness: &Harness, ws: &str, tid: &str) -> Value {
    let (status, detail) = harness.send_json(Method::GET, &task_path(ws, tid), None).await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    detail
}

async fn task_status(harness: &Harness, ws: &str, tid: &str) -> String {
    task_detail(harness, ws, tid).await["task"]["status"].as_str().unwrap().to_string()
}

async fn contract_version(harness: &Harness, ws: &str, tid: &str) -> i64 {
    task_detail(harness, ws, tid).await["task"]["contract_version"].as_i64().unwrap()
}

/// 使用者修改 Task 契約的完成條件（契約版本 +1）。
async fn change_done_criteria(harness: &Harness, ws: &str, tid: &str, text: &str) {
    let title = task_detail(harness, ws, tid).await["task"]["title"].as_str().unwrap().to_string();
    let mut changed = contract(&title);
    changed["done_criteria"] = filled(text);
    let body = json!({ "contract": changed, "note": "使用者改了完成條件" });
    let (status, view) = harness.send_json(Method::PUT, &format!("{}/contract", task_path(ws, tid)), Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{view}");
}

async fn decision_list(harness: &Harness, ws: &str) -> Value {
    let (status, list) = harness.send_json(Method::GET, &decisions_path(ws), None).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    list
}

async fn decision(harness: &Harness, ws: &str, rid: &str) -> Value {
    let (status, view) = harness.send_json(Method::GET, &format!("{}/{rid}", decisions_path(ws)), None).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    view
}

/// 列表中指定識別的那一筆。
fn listed<'a>(list: &'a Value, rid: &str) -> &'a Value {
    list["decisions"].as_array().unwrap().iter().find(|d| d["id"] == rid).unwrap_or_else(|| panic!("列表中沒有 {rid}：{list}"))
}

/// 以任意處理方式送出裁定，回傳狀態碼與本文。
async fn act(harness: &Harness, ws: &str, rid: &str, body: Value) -> (StatusCode, Value) {
    harness.send_json(Method::POST, &answer_path(ws, rid), Some(body)).await
}

/// 接受某個選項，必須成功。
async fn accept(harness: &Harness, ws: &str, rid: &str, revision: i64, option: &str) -> Value {
    let body = json!({ "action": "accept", "expected_revision": revision, "option_id": option });
    let (status, result) = act(harness, ws, rid, body).await;
    assert_eq!(status, StatusCode::OK, "{result}");
    result
}

async fn global_pending(harness: &Harness) -> Value {
    let (status, pending) = harness.send_json(Method::GET, PENDING, None).await;
    assert_eq!(status, StatusCode::OK, "{pending}");
    pending
}

fn error_code(body: &Value) -> &str {
    body["error"].as_str().unwrap_or_default()
}

fn strings(value: &Value) -> Vec<String> {
    value.as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect()
}

fn sorted_strings(value: &Value) -> Vec<String> {
    let mut all = strings(value);
    all.sort();
    all
}

fn sorted<S: AsRef<str>>(items: &[S]) -> Vec<String> {
    let mut all: Vec<String> = items.iter().map(|s| s.as_ref().to_string()).collect();
    all.sort();
    all
}

fn option_ids(view: &Value) -> Vec<String> {
    view["options"].as_array().unwrap().iter().map(|o| o["id"].as_str().unwrap().to_string()).collect()
}

fn blocking_task_ids(view: &Value) -> Vec<String> {
    let mut ids: Vec<String> =
        view["blocking"].as_array().unwrap().iter().map(|b| b["task_id"].as_str().unwrap().to_string()).collect();
    ids.sort();
    ids
}

fn link_ids(view: &Value, field: &str) -> Vec<String> {
    view[field].as_array().unwrap().iter().map(|l| l["id"].as_str().unwrap().to_string()).collect()
}

fn two_options() -> Value {
    json!([
        { "id": "ship", "label": "先出貨", "tradeoff": "快，但缺少報表" },
        { "id": "wait", "label": "等報表完成", "tradeoff": "晚一週" },
    ])
}

fn decision_request(question: &str) -> Value {
    json!({
        "question": question,
        "reason": "兩個方案的取捨取決於使用者價值，不是我能決定的",
        "options": two_options(),
        "recommendation": "wait",
        "evidence": ["報表目前完成 60%", "客戶要求週五前交付"],
        "urgency": "high",
    })
}

/// 確認 Workspace 的授權：使用「可信專案開發」範本。
async fn confirm_dev_template(harness: &Harness, ws: &str) {
    let (_, view) = harness.send_json(Method::GET, &format!("{WORKSPACES}/{ws}/authorization"), None).await;
    let template = view["templates"].as_array().unwrap().iter().find(|t| t["id"] == "trusted_dev_p3").unwrap().clone();
    let body = json!({ "template_id": "trusted_dev_p3", "grants": template["grants"], "acknowledged_risk": true });
    let (status, revision) =
        harness.send_json(Method::PUT, &format!("{WORKSPACES}/{ws}/authorization"), Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{revision}");
}

/// 兩個 Task 各自有一個執行中的 Worker。
struct Pair {
    ws: String,
    a: String,
    run_a: String,
    token_a: String,
    b: String,
    run_b: String,
    token_b: String,
}

async fn two_running_tasks(harness: &Harness) -> Pair {
    let ws = new_workspace(harness, "demo").await;
    let goal = new_goal(harness, &ws).await;
    let a = ready_task(harness, &ws, &goal, "需要裁定的工作").await;
    let b = ready_task(harness, &ws, &goal, "不相關的工作").await;
    let (run_a, token_a) = start_run(harness, &ws, &a).await;
    let (run_b, token_b) = start_run(harness, &ws, &b).await;
    Pair { ws, a, run_a, token_a, b, run_b, token_b }
}

async fn request_ok(harness: &Harness, token: &str, question: &str) -> String {
    let (status, body) = harness.call_tool(token, TOOL_REQUEST_DECISION, decision_request(question)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "waiting_decision");
    body["decision_request_id"].as_str().unwrap().to_string()
}

async fn prompt(harness: &Harness, token: &str, tool: &str, input: Value) -> (StatusCode, Value) {
    harness.call_tool(token, TOOL_PERMISSION_PROMPT, json!({ "tool_name": tool, "input": input })).await
}

/// 範圍外的權限提示（可信專案開發範本沒有「遠端寫入」），回傳裁定請求識別。
async fn prompt_waiting(harness: &Harness, token: &str, command: &str) -> String {
    let (status, response) = prompt(harness, token, "Bash", json!({ "command": command })).await;
    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(response["behavior"], "deny", "{response}");
    assert_eq!(response["status"], "waiting_decision", "{response}");
    response["decision_request_id"].as_str().unwrap().to_string()
}

fn controller_body(matter: &str) -> Value {
    json!({
        "matter": matter,
        "summary": "把完成條件從五個產品改成三個產品",
        "reason": "第五個產品的公開資料不足",
        "evidence": ["只找到三個產品的價格表"],
        "urgency": "high",
    })
}

async fn controller(harness: &Harness, token: &str, matter: &str) -> Value {
    let (status, body) = harness.call_tool(token, TOOL_CONTROLLER_ACTION, controller_body(matter)).await;
    assert_eq!(status, StatusCode::OK, "{matter}：{body}");
    assert_eq!(body["matter"], matter, "{body}");
    body
}

async fn authority_table(harness: &Harness, ws: &str) -> Value {
    let (status, table) = harness.send_json(Method::GET, &authority_path(ws), None).await;
    assert_eq!(status, StatusCode::OK, "{table}");
    table
}

async fn put_authority(harness: &Harness, ws: &str, revision: i64, entries: Value) -> (StatusCode, Value) {
    let body = json!({ "expected_revision": revision, "entries": entries });
    harness.send_json(Method::PUT, &authority_path(ws), Some(body)).await
}

fn authority_entry<'a>(table: &'a Value, matter: &str) -> &'a Value {
    table["entries"].as_array().unwrap().iter().find(|e| e["matter"] == matter).unwrap_or_else(|| panic!("權限表沒有 {matter}：{table}"))
}

/// 以 Agent 憑證打桌面端點，回傳狀態碼與本文。
async fn as_agent(harness: &Harness, token: &str, method: Method, path: &str, body: Option<Value>) -> (StatusCode, Value) {
    let mut request = harness
        .request(method, path)
        .bearer_auth(token)
        .header(baton_api::CLIENT_HEADER, baton_api::CLIENT_HEADER_VALUE);
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().await.unwrap();
    let status = response.status();
    let bytes = response.bytes().await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

fn decision_rows(harness: &Harness) -> i64 {
    harness.db().query_row("SELECT COUNT(*) FROM decision", [], |r| r.get(0)).unwrap()
}

// --- 隔天開啟：請求完整 ---

#[tokio::test]
async fn requests_are_complete_when_the_center_is_opened_the_next_day() {
    let mut harness = harness().await;
    let p = two_running_tasks(&harness).await;
    let ra = request_ok(&harness, &p.token_a, "要先出貨還是等報表？").await;
    let rb = request_ok(&harness, &p.token_b, "要不要納入付費資料？").await;

    harness.clock.advance_millis(DAY_MS);
    harness.restart().await;

    let list = decision_list(&harness, &p.ws).await;
    assert_eq!(list["pending_count"], 2);
    assert_eq!(
        list["overview"],
        json!({
            "pending": 2, "actionable": 2, "deferred": 0, "overdue": 0,
            "high_urgency": 2, "answered": 0, "superseded": 0, "merged": 0,
        })
    );
    for (rid, tid, title, question) in
        [(&ra, &p.a, "需要裁定的工作", "要先出貨還是等報表？"), (&rb, &p.b, "不相關的工作", "要不要納入付費資料？")]
    {
        let view = listed(&list, rid);
        assert_eq!(view["status"], "pending", "{view}");
        assert_eq!(view["kind"], "agent");
        assert_eq!(view["revision"], 1);
        assert_eq!(view["question"], question);
        assert!(view["reason"].as_str().unwrap().contains("使用者價值"), "{view}");
        assert_eq!(view["options"], two_options());
        assert_eq!(view["recommendation"], "wait");
        assert_eq!(view["evidence"], json!(["報表目前完成 60%", "客戶要求週五前交付"]));
        assert_eq!(view["urgency"], "high");
        assert_eq!(view["blocking"], json!([{ "task_id": tid, "task_title": title, "task_status": "waiting_decision" }]));
        assert_eq!(view["deferred"], false);
        assert!(view["deferred_until"].is_null());
        assert!(view["merged_into"].is_null());
        assert!(view["status_reason"].is_null());
        assert_eq!(view["links"], json!([]), "問題不同，不會自動連結");
        assert_eq!(view["merged_from"], json!([]));
        assert!(view["authority"].is_null());
        assert_eq!(view["target"]["task_id"], *tid);
        assert_eq!(view["target"]["contract_version"], 1);
        assert_eq!(
            sorted_strings(&view["allowed_actions"]),
            sorted(&["accept", "modify", "answer", "investigate", "defer", "reject"])
        );
        assert!(view["decision"].is_null());
    }
    // 隔天仍可直接處理
    let result = accept(&harness, &p.ws, &ra, 1, "wait").await;
    assert_eq!(result["resumed_task_ids"], json!([p.a]));
    assert_eq!(result["decision"]["revision"], 2);
}

// --- 目標版本已變更的請求不能套用 ---

#[tokio::test]
async fn a_request_whose_task_contract_changed_is_superseded_when_the_center_opens() {
    let harness = harness().await;
    let p = two_running_tasks(&harness).await;
    let ra = request_ok(&harness, &p.token_a, "要先出貨還是等報表？").await;
    let rb = request_ok(&harness, &p.token_b, "要不要納入付費資料？").await;

    change_done_criteria(&harness, &p.ws, &p.a, "三個產品").await;
    let list = decision_list(&harness, &p.ws).await;

    let stale = listed(&list, &ra);
    assert_eq!(stale["status"], "superseded", "{stale}");
    assert!(!stale["status_reason"].as_str().unwrap_or_default().trim().is_empty(), "{stale}");
    assert_eq!(stale["allowed_actions"], json!([]));
    assert_eq!(stale["revision"], 2);
    assert_eq!(listed(&list, &rb)["status"], "pending", "另一個 Task 的請求不受影響");
    assert_eq!(list["pending_count"], 1);
    assert_eq!(list["overview"]["superseded"], 1);
    assert_eq!(list["overview"]["pending"], 1);
    assert_eq!(list["decisions"][0]["id"], rb, "可處理的在前，已結束的在後");

    // 被取代的請求不能再套用
    let body = json!({ "action": "accept", "expected_revision": stale["revision"], "option_id": "wait" });
    let (status, response) = act(&harness, &p.ws, &ra, body).await;
    assert_eq!(status, StatusCode::CONFLICT, "{response}");
    assert_eq!(error_code(&response), "decision_superseded");

    // 它是 A 唯一的阻擋：A 回到 ready 並重新排隊；B 仍在等裁定
    assert_eq!(task_status(&harness, &p.ws, &p.a).await, "ready");
    assert!(queued_task_ids(&harness).await.contains(&p.a));
    assert_eq!(task_status(&harness, &p.ws, &p.b).await, "waiting_decision");
    assert_eq!(decision(&harness, &p.ws, &rb).await["status"], "pending");
    assert_eq!(decision_rows(&harness), 0, "沒有寫入任何裁定");
}

#[tokio::test]
async fn answering_a_request_whose_target_changed_is_refused_and_marks_it_superseded() {
    let harness = harness().await;
    let p = two_running_tasks(&harness).await;
    let ra = request_ok(&harness, &p.token_a, "要先出貨還是等報表？").await;
    change_done_criteria(&harness, &p.ws, &p.a, "三個產品").await;

    // 不先開列表，直接回答
    let body = json!({ "action": "accept", "expected_revision": 1, "option_id": "ship" });
    let (status, response) = act(&harness, &p.ws, &ra, body).await;

    assert_eq!(status, StatusCode::CONFLICT, "{response}");
    assert_eq!(error_code(&response), "decision_superseded");
    let view = decision(&harness, &p.ws, &ra).await;
    assert_eq!(view["status"], "superseded", "{view}");
    assert_eq!(view["revision"], 2);
    assert!(view["decision"].is_null(), "沒有套用使用者的回答");
    assert_eq!(decision_rows(&harness), 0);
    assert_eq!(task_status(&harness, &p.ws, &p.a).await, "ready");
    assert_eq!(global_pending(&harness).await["pending_count"], 0);
}

#[tokio::test]
async fn a_permission_request_is_not_superseded_by_an_authorization_change() {
    let harness = harness().await;
    let p = two_running_tasks(&harness).await;
    // 尚未確認授權時，每個操作都要問
    let (_, waiting) = prompt(&harness, &p.token_a, "Read", json!({ "file_path": "README.md" })).await;
    let rid = waiting["decision_request_id"].as_str().unwrap().to_string();

    // 權限提示的目標是那一個操作（單次批准在執行時逐項核對綁定，票 23），修改授權與回答請求是兩回事：請求仍可回答
    confirm_dev_template(&harness, &p.ws).await;
    let list = decision_list(&harness, &p.ws).await;

    let view = listed(&list, &rid);
    assert_eq!(view["kind"], "permission");
    assert_eq!(view["status"], "pending", "{view}");
    assert!(view["target"]["authorization_revision"].is_null(), "記下的是判定當時的授權（尚未確認）：{view}");
    assert_eq!(task_status(&harness, &p.ws, &p.a).await, "waiting_decision");
    let body = json!({ "action": "accept", "expected_revision": view["revision"], "option_id": "allow_once" });
    let (status, response) = act(&harness, &p.ws, &rid, body).await;
    assert_eq!(status, StatusCode::OK, "{response}");
}

#[tokio::test]
async fn a_stale_expected_revision_is_refused_without_writing_anything() {
    let harness = harness().await;
    let p = two_running_tasks(&harness).await;
    let rid = request_ok(&harness, &p.token_a, "要先出貨還是等報表？").await;

    let body = json!({ "action": "accept", "expected_revision": 5, "option_id": "ship" });
    let (status, response) = act(&harness, &p.ws, &rid, body).await;
    assert_eq!(status, StatusCode::CONFLICT, "{response}");
    assert_eq!(error_code(&response), "decision_revision_mismatch");
    // 沒有 expected_revision 的回答不被接受
    let (status, response) = act(&harness, &p.ws, &rid, json!({ "action": "accept", "option_id": "ship" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{response}");

    let view = decision(&harness, &p.ws, &rid).await;
    assert_eq!(view["status"], "pending");
    assert_eq!(view["revision"], 1);
    assert!(view["decision"].is_null());
    assert_eq!(task_status(&harness, &p.ws, &p.a).await, "waiting_decision");
    assert_eq!(decision_rows(&harness), 0);
}

// --- 六種處理方式 ---

#[tokio::test]
async fn modifying_requires_the_modified_conditions_and_keeps_them() {
    let harness = harness().await;
    let p = two_running_tasks(&harness).await;
    let rid = request_ok(&harness, &p.token_a, "要先出貨還是等報表？").await;

    let (status, response) =
        act(&harness, &p.ws, &rid, json!({ "action": "modify", "expected_revision": 1, "option_id": "ship" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{response}");
    assert_eq!(error_code(&response), "decision_invalid");
    let (status, response) = act(
        &harness,
        &p.ws,
        &rid,
        json!({ "action": "modify", "expected_revision": 1, "option_id": "ship", "note": "   " }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{response}");
    assert_eq!(error_code(&response), "decision_invalid");
    assert_eq!(decision(&harness, &p.ws, &rid).await["revision"], 1, "失敗的提交不改變 revision");

    let body = json!({ "action": "modify", "expected_revision": 1, "option_id": "ship", "note": "先出貨，但報表要在下週補上" });
    let (status, result) = act(&harness, &p.ws, &rid, body).await;

    assert_eq!(status, StatusCode::OK, "{result}");
    let view = &result["decision"];
    assert_eq!(view["status"], "answered");
    assert_eq!(view["revision"], 2);
    assert_eq!(view["allowed_actions"], json!([]));
    assert_eq!(view["decision"]["action"], "modify");
    assert_eq!(view["decision"]["option_id"], "ship");
    assert_eq!(view["decision"]["option_label"], "先出貨");
    assert_eq!(view["decision"]["note"], "先出貨，但報表要在下週補上");
    assert_eq!(view["decision"]["decided_by"], "desktop");
    assert!(view["decision"]["batch_id"].is_null());
    assert_eq!(result["resumed_task_ids"], json!([p.a]));
    assert_eq!(task_status(&harness, &p.ws, &p.a).await, "ready");
}

#[tokio::test]
async fn answering_in_words_and_asking_for_investigation_resume_the_task() {
    let harness = harness().await;
    let p = two_running_tasks(&harness).await;
    let ra = request_ok(&harness, &p.token_a, "要先出貨還是等報表？").await;
    let rb = request_ok(&harness, &p.token_b, "要不要納入付費資料？").await;

    for (rid, action) in [(&ra, "answer"), (&rb, "investigate")] {
        let (status, response) = act(&harness, &p.ws, rid, json!({ "action": action, "expected_revision": 1 })).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{action}：{response}");
        assert_eq!(error_code(&response), "decision_invalid", "{action} 必須寫內容");
    }

    let body = json!({ "action": "answer", "expected_revision": 1, "note": "兩個都不要：改成先給客戶看草稿" });
    let (status, answered) = act(&harness, &p.ws, &ra, body).await;
    assert_eq!(status, StatusCode::OK, "{answered}");
    assert_eq!(answered["decision"]["status"], "answered");
    assert_eq!(answered["decision"]["decision"]["action"], "answer");
    assert_eq!(answered["decision"]["decision"]["note"], "兩個都不要：改成先給客戶看草稿");
    assert_eq!(answered["resumed_task_ids"], json!([p.a]));

    let body = json!({ "action": "investigate", "expected_revision": 1, "note": "先查清楚付費資料的授權條款" });
    let (status, investigated) = act(&harness, &p.ws, &rb, body).await;
    assert_eq!(status, StatusCode::OK, "{investigated}");
    assert_eq!(investigated["decision"]["status"], "answered");
    assert_eq!(investigated["decision"]["decision"]["action"], "investigate");
    assert_eq!(investigated["decision"]["decision"]["note"], "先查清楚付費資料的授權條款");
    assert_eq!(investigated["resumed_task_ids"], json!([p.b]));

    assert_eq!(task_status(&harness, &p.ws, &p.a).await, "ready");
    assert_eq!(task_status(&harness, &p.ws, &p.b).await, "ready");
    let mut queued = queued_task_ids(&harness).await;
    queued.sort();
    assert_eq!(queued, sorted(&[&p.a, &p.b]));
}

#[tokio::test]
async fn rejecting_closes_the_request_and_resumes_the_task() {
    let harness = harness().await;
    let p = two_running_tasks(&harness).await;
    let rid = request_ok(&harness, &p.token_a, "要先出貨還是等報表？").await;

    let body = json!({ "action": "reject", "expected_revision": 1, "note": "這不是現在要決定的事" });
    let (status, result) = act(&harness, &p.ws, &rid, body).await;

    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(result["decision"]["status"], "answered");
    assert_eq!(result["decision"]["decision"]["action"], "reject");
    assert_eq!(result["decision"]["decision"]["note"], "這不是現在要決定的事");
    assert_eq!(result["resumed_task_ids"], json!([p.a]));
    assert_eq!(task_status(&harness, &p.ws, &p.a).await, "ready");
    // 已結束：不同的處理方式不能改寫
    let (status, again) =
        act(&harness, &p.ws, &rid, json!({ "action": "accept", "expected_revision": 2, "option_id": "ship" })).await;
    assert_eq!(status, StatusCode::CONFLICT, "{again}");
    assert_eq!(error_code(&again), "decision_already_answered");
}

#[tokio::test]
async fn deferring_keeps_the_request_pending_and_never_becomes_consent() {
    let harness = harness().await;
    let p = two_running_tasks(&harness).await;
    let ra = request_ok(&harness, &p.token_a, "要先出貨還是等報表？").await;
    let rb = request_ok(&harness, &p.token_b, "要不要納入付費資料？").await;

    for minutes in [0, 43_201] {
        let body = json!({ "action": "defer", "expected_revision": 1, "defer_minutes": minutes });
        let (status, response) = act(&harness, &p.ws, &ra, body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{minutes}：{response}");
        assert_eq!(error_code(&response), "decision_invalid");
    }

    let body = json!({ "action": "defer", "expected_revision": 1, "defer_minutes": 60 });
    let (status, result) = act(&harness, &p.ws, &ra, body).await;

    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(result["resumed_task_ids"], json!([]));
    let view = &result["decision"];
    assert_eq!(view["status"], "pending", "延後不是同意");
    assert_eq!(view["deferred"], true);
    assert_eq!(view["deferred_until"], CLOCK_START.as_millis() + 60 * MINUTE_MS);
    assert_eq!(view["revision"], 2);
    assert_eq!(task_status(&harness, &p.ws, &p.a).await, "waiting_decision");
    let list = decision_list(&harness, &p.ws).await;
    assert_eq!(list["pending_count"], 2, "延後的仍算待裁定");
    assert_eq!(list["overview"]["deferred"], 1);
    assert_eq!(list["overview"]["actionable"], 1);
    assert_eq!(list["decisions"][0]["id"], rb, "可處理的排在延後的前面");
    assert_eq!(list["decisions"][1]["id"], ra);

    // 延後期滿：回到可處理，但沒有任何自動同意
    harness.clock.advance_millis(61 * MINUTE_MS);
    harness.supervise().await;
    let view = decision(&harness, &p.ws, &ra).await;
    assert_eq!(view["status"], "pending");
    assert_eq!(view["deferred"], false);
    assert_eq!(view["revision"], 2);
    assert_eq!(task_status(&harness, &p.ws, &p.a).await, "waiting_decision");
    assert_eq!(decision_list(&harness, &p.ws).await["overview"]["actionable"], 2);
    assert_eq!(decision_rows(&harness), 0);

    // 之後照常處理（必須帶最新的 revision）
    let result = accept(&harness, &p.ws, &ra, 2, "wait").await;
    assert_eq!(result["resumed_task_ids"], json!([p.a]));
}

#[tokio::test]
async fn a_permission_request_only_offers_accept_defer_and_reject() {
    let harness = harness().await;
    let p = two_running_tasks(&harness).await;
    confirm_dev_template(&harness, &p.ws).await;
    let rid = prompt_waiting(&harness, &p.token_a, "git push origin main").await;

    let view = decision(&harness, &p.ws, &rid).await;
    assert_eq!(sorted_strings(&view["allowed_actions"]), sorted(&["accept", "defer", "reject"]));
    for action in ["modify", "answer", "investigate"] {
        let body = json!({ "action": action, "expected_revision": 1, "option_id": "allow_once", "note": "條件" });
        let (status, response) = act(&harness, &p.ws, &rid, body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{action}：{response}");
        assert_eq!(error_code(&response), "decision_action_not_allowed", "{action}");
    }
    let view = decision(&harness, &p.ws, &rid).await;
    assert_eq!(view["status"], "pending");
    assert_eq!(view["revision"], 1);
    assert_eq!(task_status(&harness, &p.ws, &p.a).await, "waiting_decision");
}

// --- 批次 ---

#[tokio::test]
async fn a_batch_saves_each_item_separately_and_one_failure_does_not_stop_the_others() {
    let harness = harness().await;
    let p = two_running_tasks(&harness).await;
    let q1 = request_ok(&harness, &p.token_a, "第一個問題").await;
    let q2 = request_ok(&harness, &p.token_a, "第二個問題").await;
    let q3 = request_ok(&harness, &p.token_b, "第三個問題").await;

    // 整批格式錯誤：什麼都不寫
    let item = json!({ "workspace_id": p.ws, "request_id": q1, "expected_revision": 1, "action": "accept", "option_id": "ship" });
    for items in [json!([]), json!([item.clone(), item.clone()])] {
        let (status, response) = harness.send_json(Method::POST, BATCH, Some(json!({ "items": items }))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{response}");
        assert_eq!(error_code(&response), "decision_batch_invalid");
    }
    assert_eq!(decision_rows(&harness), 0);

    let body = json!({ "items": [
        item,
        { "workspace_id": p.ws, "request_id": q2, "expected_revision": 9, "action": "accept", "option_id": "wait" },
        { "workspace_id": p.ws, "request_id": q3, "expected_revision": 1, "action": "modify", "option_id": "wait", "note": "等報表，但最多一週" },
    ]});
    let (status, response) = harness.send_json(Method::POST, BATCH, Some(body)).await;

    assert_eq!(status, StatusCode::OK, "{response}");
    let batch_id = response["batch_id"].as_str().unwrap().to_string();
    let results = response["results"].as_array().unwrap();
    assert_eq!(results.len(), 3);
    let order: Vec<&str> = results.iter().map(|r| r["request_id"].as_str().unwrap()).collect();
    assert_eq!(order, [q1.as_str(), q2.as_str(), q3.as_str()], "結果順序同 items");

    assert_eq!(results[0]["ok"], true, "{}", results[0]);
    assert_eq!(results[0]["workspace_id"], p.ws);
    assert_eq!(results[0]["decision"]["decision"]["action"], "accept");
    assert_eq!(results[0]["decision"]["decision"]["option_id"], "ship");
    assert_eq!(results[0]["decision"]["decision"]["batch_id"], batch_id);
    assert_eq!(results[0]["resumed_task_ids"], json!([]), "A 還有一個請求沒有處理");

    assert_eq!(results[1]["ok"], false, "{}", results[1]);
    assert_eq!(results[1]["error"], "decision_revision_mismatch");
    assert!(!results[1]["message"].as_str().unwrap_or_default().is_empty());

    assert_eq!(results[2]["ok"], true, "{}", results[2]);
    assert_eq!(results[2]["decision"]["decision"]["action"], "modify");
    assert_eq!(results[2]["decision"]["decision"]["option_id"], "wait");
    assert_eq!(results[2]["decision"]["decision"]["note"], "等報表，但最多一週");
    assert_eq!(results[2]["decision"]["decision"]["batch_id"], batch_id);
    assert_eq!(results[2]["resumed_task_ids"], json!([p.b]));

    // 每筆各自保存；失敗的那筆原封不動
    assert_eq!(decision(&harness, &p.ws, &q1).await["decision"]["option_id"], "ship");
    assert_eq!(decision(&harness, &p.ws, &q3).await["decision"]["option_id"], "wait");
    let untouched = decision(&harness, &p.ws, &q2).await;
    assert_eq!(untouched["status"], "pending");
    assert_eq!(untouched["revision"], 1);
    assert_eq!(decision_rows(&harness), 2);
    assert_eq!(task_status(&harness, &p.ws, &p.a).await, "waiting_decision");
    assert_eq!(task_status(&harness, &p.ws, &p.b).await, "ready");
    assert_eq!(global_pending(&harness).await["pending_count"], 1);
}

#[tokio::test]
async fn a_batch_of_permission_approvals_is_not_a_blanket_authorization() {
    let harness = harness().await;
    let p = two_running_tasks(&harness).await;
    confirm_dev_template(&harness, &p.ws).await;
    let push = "git push origin main";
    let ra = prompt_waiting(&harness, &p.token_a, push).await;
    let rb = prompt_waiting(&harness, &p.token_b, push).await;
    assert_ne!(ra, rb, "不同 Task 的提示各自一個請求");
    // 同一個操作、不同 Task：互相連結，revision 不變
    let view_a = decision(&harness, &p.ws, &ra).await;
    assert_eq!(link_ids(&view_a, "links"), [rb.clone()]);
    assert_eq!(view_a["revision"], 1);
    assert_eq!(link_ids(&decision(&harness, &p.ws, &rb).await, "links"), [ra.clone()]);

    worker_exits(&harness, &p.run_a).await;
    worker_exits(&harness, &p.run_b).await;
    let body = json!({ "items": [
        { "workspace_id": p.ws, "request_id": ra, "expected_revision": 1, "action": "accept", "option_id": "allow_once" },
        { "workspace_id": p.ws, "request_id": rb, "expected_revision": 1, "action": "accept", "option_id": "allow_once" },
    ]});
    let (status, response) = harness.send_json(Method::POST, BATCH, Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{response}");
    assert!(response["results"].as_array().unwrap().iter().all(|r| r["ok"] == true), "{response}");
    let resumed = resumed_runs(&harness, &p.ws, &[&p.a, &p.b]).await;
    let (token_a2, token_b2) = (&resumed[0].1, &resumed[1].1);

    // A：同一個操作消耗自己那一次批准；不同的操作沒有被批准
    let (_, approved) = prompt(&harness, token_a2, "Bash", json!({ "command": push })).await;
    assert_eq!(approved["behavior"], "allow", "{approved}");
    assert_eq!(approved["status"], "approved");
    assert_eq!(approved["decision_request_id"], ra);
    let other = prompt_waiting(&harness, token_a2, "git push --force origin main").await;
    assert!(other != ra && other != rb);
    assert_eq!(task_status(&harness, &p.ws, &p.a).await, "waiting_decision");

    // B：自己那一次批准只能用一次，第二次又要重新裁定
    let (_, approved) = prompt(&harness, token_b2, "Bash", json!({ "command": push })).await;
    assert_eq!(approved["behavior"], "allow", "{approved}");
    assert_eq!(approved["decision_request_id"], rb);
    let again = prompt_waiting(&harness, token_b2, push).await;
    assert!(again != ra && again != rb && again != other);
    assert_eq!(task_status(&harness, &p.ws, &p.b).await, "waiting_decision");

    let list = decision_list(&harness, &p.ws).await;
    assert_eq!(list["pending_count"], 2);
    assert_eq!(listed(&list, &other)["status"], "pending");
    assert_eq!(listed(&list, &again)["status"], "pending");
}

// --- 相似問題：連結與合併 ---

#[tokio::test]
async fn similar_questions_from_different_tasks_are_linked_automatically_and_manually() {
    let harness = harness().await;
    let p = two_running_tasks(&harness).await;
    let ra = request_ok(&harness, &p.token_a, "要用哪個資料來源？").await;
    let rb = request_ok(&harness, &p.token_b, "要用哪個資料來源").await;
    let rc = request_ok(&harness, &p.token_a, "報表要用什麼格式？").await;

    let view_a = decision(&harness, &p.ws, &ra).await;
    assert_eq!(view_a["links"], json!([{ "id": rb, "question": "要用哪個資料來源", "status": "pending" }]));
    assert_eq!(view_a["revision"], 1, "新增連結不改 revision");
    assert_eq!(link_ids(&decision(&harness, &p.ws, &rb).await, "links"), [ra.clone()]);
    assert_eq!(decision(&harness, &p.ws, &rc).await["links"], json!([]));

    // 手動連結：雙向可見、冪等、revision 不變
    let link = |other: &str| json!({ "other_id": other });
    let (status, linked) = harness.send_json(Method::POST, &links_path(&p.ws, &rc), Some(link(&rb))).await;
    assert_eq!(status, StatusCode::OK, "{linked}");
    assert_eq!(link_ids(&linked, "links"), [rb.clone()]);
    assert_eq!(linked["revision"], 1);
    let (status, again) = harness.send_json(Method::POST, &links_path(&p.ws, &rc), Some(link(&rb))).await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(link_ids(&again, "links"), [rb.clone()]);
    let mut from_b = link_ids(&decision(&harness, &p.ws, &rb).await, "links");
    from_b.sort();
    assert_eq!(from_b, sorted(&[&ra, &rc]));

    let (status, response) = harness.send_json(Method::POST, &links_path(&p.ws, &rc), Some(link(&rc))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{response}");
    assert_eq!(error_code(&response), "decision_invalid");
    let (status, response) = harness.send_json(Method::POST, &links_path(&p.ws, &rc), Some(link("decision_nope"))).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{response}");
    assert_eq!(error_code(&response), "decision_not_found");
}

#[tokio::test]
async fn merging_a_request_widens_the_target_and_one_answer_resumes_both_tasks() {
    let harness = harness().await;
    let p = two_running_tasks(&harness).await;
    let ra = request_ok(&harness, &p.token_a, "要用哪個資料來源？").await;
    let rb = request_ok(&harness, &p.token_b, "要用哪個資料來源").await;

    let merge = |expected: i64, into_expected: i64| {
        json!({ "into": ra, "expected_revision": expected, "into_expected_revision": into_expected })
    };
    let (status, response) = harness.send_json(Method::POST, &merge_path(&p.ws, &rb), Some(merge(1, 3))).await;
    assert_eq!(status, StatusCode::CONFLICT, "{response}");
    assert_eq!(error_code(&response), "decision_revision_mismatch");
    assert_eq!(decision(&harness, &p.ws, &rb).await["status"], "pending");

    let (status, response) = harness.send_json(Method::POST, &merge_path(&p.ws, &rb), Some(merge(1, 1))).await;

    assert_eq!(status, StatusCode::OK, "{response}");
    let target = &response["decision"];
    assert_eq!(target["id"], ra);
    assert_eq!(target["status"], "pending");
    assert_eq!(target["revision"], 2);
    assert_eq!(blocking_task_ids(target), sorted(&[&p.a, &p.b]));
    assert_eq!(link_ids(target, "merged_from"), [rb.clone()]);
    let source = decision(&harness, &p.ws, &rb).await;
    assert_eq!(source["status"], "merged");
    assert_eq!(source["merged_into"], ra);
    assert_eq!(source["revision"], 2);
    assert_eq!(source["allowed_actions"], json!([]));
    let list = decision_list(&harness, &p.ws).await;
    assert_eq!(list["pending_count"], 1);
    assert_eq!(list["overview"]["merged"], 1);

    // 已合併出去的請求不能單獨回答
    let body = json!({ "action": "accept", "expected_revision": 2, "option_id": "ship" });
    let (status, response) = act(&harness, &p.ws, &rb, body).await;
    assert_eq!(status, StatusCode::CONFLICT, "{response}");
    assert_eq!(error_code(&response), "decision_merged");

    // 回答目標：兩個 Task 都恢復
    let result = accept(&harness, &p.ws, &ra, 2, "ship").await;
    let mut resumed = strings(&result["resumed_task_ids"]);
    resumed.sort();
    assert_eq!(resumed, sorted(&[&p.a, &p.b]));
    assert_eq!(task_status(&harness, &p.ws, &p.a).await, "ready");
    assert_eq!(task_status(&harness, &p.ws, &p.b).await, "ready");
    assert_eq!(global_pending(&harness).await["pending_count"], 0);
}

#[tokio::test]
async fn permission_requests_and_mixed_kinds_cannot_be_merged() {
    let harness = harness().await;
    let p = two_running_tasks(&harness).await;
    confirm_dev_template(&harness, &p.ws).await;
    let pa = prompt_waiting(&harness, &p.token_a, "git push origin main").await;
    let pb = prompt_waiting(&harness, &p.token_b, "git push origin main").await;

    let body = json!({ "into": pa, "expected_revision": 1, "into_expected_revision": 1 });
    let (status, response) = harness.send_json(Method::POST, &merge_path(&p.ws, &pb), Some(body)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{response}");
    assert_eq!(error_code(&response), "decision_merge_invalid");

    // 種類不同（agent 併入 permission）也不行；第三個 Run 需要另一個帳號的並行名額
    harness.add_account("第二帳號", "second@example.com").await;
    let worker_question = {
        let ws = p.ws.clone();
        let goal = new_goal(&harness, &ws).await;
        let c = ready_task(&harness, &ws, &goal, "第三個工作").await;
        let (_, token_c) = start_run(&harness, &ws, &c).await;
        request_ok(&harness, &token_c, "要用哪個資料來源？").await
    };
    let body = json!({ "into": pa, "expected_revision": 1, "into_expected_revision": 1 });
    let (status, response) = harness.send_json(Method::POST, &merge_path(&p.ws, &worker_question), Some(body)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{response}");
    assert_eq!(error_code(&response), "decision_merge_invalid");

    for rid in [&pa, &pb, &worker_question] {
        let view = decision(&harness, &p.ws, rid).await;
        assert_eq!(view["status"], "pending", "{view}");
        assert_eq!(view["revision"], 1);
    }
}

// --- Controller 不能自行處理「必須送裁定」的事項 ---

#[tokio::test]
async fn a_controller_trying_to_modify_done_criteria_is_refused_and_turned_into_a_request() {
    let harness = harness().await;
    let p = two_running_tasks(&harness).await;

    let first = controller(&harness, &p.token_a, "modify_done_criteria").await;

    assert_eq!(first["status"], "requires_decision", "{first}");
    assert_eq!(first["authority"], "requires_decision");
    assert!(!first["message"].as_str().unwrap_or_default().is_empty());
    let rid = first["decision_request_id"].as_str().unwrap().to_string();
    assert_eq!(contract_version(&harness, &p.ws, &p.a).await, 1, "工具本身沒有改契約");
    assert_eq!(task_status(&harness, &p.ws, &p.a).await, "waiting_decision");
    assert_eq!(task_status(&harness, &p.ws, &p.b).await, "running");

    let view = decision(&harness, &p.ws, &rid).await;
    assert_eq!(view["kind"], "authority");
    assert_eq!(view["status"], "pending");
    assert_eq!(view["authority"]["matter"], "modify_done_criteria");
    assert!(!view["authority"]["matter_label"].as_str().unwrap_or_default().is_empty(), "{view}");
    assert_eq!(view["authority"]["summary"], "把完成條件從五個產品改成三個產品");
    assert_eq!(option_ids(&view), ["approve", "reject"]);
    assert_eq!(blocking_task_ids(&view), [p.a.clone()]);
    assert_eq!(view["target"]["task_id"], p.a);
    assert_eq!(view["target"]["contract_version"], 1);
    assert_eq!(view["evidence"], json!(["只找到三個產品的價格表"]));

    // 還在等裁定時重複嘗試：同一個請求
    let again = controller(&harness, &p.token_a, "modify_done_criteria").await;
    assert_eq!(again["status"], "requires_decision");
    assert_eq!(again["decision_request_id"], rid);
    assert_eq!(again["existing"], true);
    assert_eq!(decision_list(&harness, &p.ws).await["pending_count"], 1);

    // 使用者核准這一次：下一次嘗試被放行一次
    worker_exits(&harness, &p.run_a).await;
    let result = accept(&harness, &p.ws, &rid, 1, "approve").await;
    assert_eq!(result["resumed_task_ids"], json!([p.a]));
    let resumed = resumed_runs(&harness, &p.ws, &[&p.a]).await;
    let token_a2 = &resumed[0].1;
    let approved = controller(&harness, token_a2, "modify_done_criteria").await;
    assert_eq!(approved["status"], "approved", "{approved}");
    assert_eq!(contract_version(&harness, &p.ws, &p.a).await, 1, "核准只是放行，工具本身仍不改契約");

    // 核准已經用掉：第三次又要裁定（新的請求）
    let third = controller(&harness, token_a2, "modify_done_criteria").await;
    assert_eq!(third["status"], "requires_decision", "{third}");
    assert_ne!(third["decision_request_id"], rid.as_str());
    assert_ne!(third["existing"], true);
    assert_eq!(task_status(&harness, &p.ws, &p.a).await, "waiting_decision");
}

#[tokio::test]
async fn approving_with_modified_conditions_passes_the_conditions_back_to_the_controller() {
    let harness = harness().await;
    let p = two_running_tasks(&harness).await;
    let mut body = controller_body("modify_done_criteria");
    body["task_id"] = json!(p.b);
    let (status, first) = harness.call_tool(&p.token_b, TOOL_CONTROLLER_ACTION, body).await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(first["status"], "requires_decision");
    let rid = first["decision_request_id"].as_str().unwrap().to_string();
    assert_eq!(blocking_task_ids(&decision(&harness, &p.ws, &rid).await), [p.b.clone()]);

    worker_exits(&harness, &p.run_b).await;
    let conditions = "可以改成四個產品，但第五個要列為後續工作";
    let body = json!({ "action": "modify", "expected_revision": 1, "option_id": "approve", "note": conditions });
    let (status, result) = act(&harness, &p.ws, &rid, body).await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(result["decision"]["decision"]["action"], "modify");

    let resumed = resumed_runs(&harness, &p.ws, &[&p.b]).await;
    let approved = controller(&harness, &resumed[0].1, "modify_done_criteria").await;
    assert_eq!(approved["status"], "approved", "{approved}");
    assert_eq!(approved["conditions"], conditions);
    let _ = (&p.run_a, &p.token_a);
}

#[tokio::test]
async fn a_controller_trying_to_skip_a_gate_is_refused_and_a_rejection_is_reported_once() {
    let harness = harness().await;
    let p = two_running_tasks(&harness).await;

    // 使用者以「駁回」處理
    let first = controller(&harness, &p.token_a, "skip_stage_or_gate").await;
    assert_eq!(first["status"], "requires_decision", "{first}");
    let ra = first["decision_request_id"].as_str().unwrap().to_string();
    let view = decision(&harness, &p.ws, &ra).await;
    assert_eq!(view["kind"], "authority");
    assert_eq!(view["authority"]["matter"], "skip_stage_or_gate");
    assert_ne!(task_status(&harness, &p.ws, &p.a).await, "completed");
    worker_exits(&harness, &p.run_a).await;
    let (status, result) = act(&harness, &p.ws, &ra, json!({ "action": "reject", "expected_revision": 1 })).await;
    assert_eq!(status, StatusCode::OK, "{result}");
    let resumed = resumed_runs(&harness, &p.ws, &[&p.a]).await;
    let token_a2 = &resumed[0].1;
    let denied = controller(&harness, token_a2, "skip_stage_or_gate").await;
    assert_eq!(denied["status"], "denied", "{denied}");
    assert_eq!(task_status(&harness, &p.ws, &p.a).await, "running", "被拒絕不會讓 Task 完成");
    // 拒絕只回報一次；再試又要裁定
    let again = controller(&harness, token_a2, "skip_stage_or_gate").await;
    assert_eq!(again["status"], "requires_decision", "{again}");
    assert_ne!(again["decision_request_id"], ra.as_str());

    // 使用者以「接受」選了「不核准」選項，效果相同
    let first = controller(&harness, &p.token_b, "skip_stage_or_gate").await;
    let rb = first["decision_request_id"].as_str().unwrap().to_string();
    worker_exits(&harness, &p.run_b).await;
    accept(&harness, &p.ws, &rb, 1, "reject").await;
    let resumed = resumed_runs(&harness, &p.ws, &[&p.b]).await;
    let denied = controller(&harness, &resumed[0].1, "skip_stage_or_gate").await;
    assert_eq!(denied["status"], "denied", "{denied}");
    assert_ne!(task_status(&harness, &p.ws, &p.b).await, "completed");
}

// --- 決策權限表：可收窄、不可放寬 ---

#[tokio::test]
async fn the_builtin_authority_table_matches_the_spec() {
    let harness = harness().await;
    let ws = new_workspace(&harness, "demo").await;

    let table = authority_table(&harness, &ws).await;

    assert_eq!(table["revision"], 0);
    assert_eq!(table["entries"].as_array().unwrap().len(), 17);
    for (matters, builtin) in [
        (&BUILTIN_AUTONOMOUS[..], "autonomous"),
        (&BUILTIN_REQUIRES_DECISION[..], "requires_decision"),
        (&BUILTIN_HUMAN_ONLY[..], "human_only"),
    ] {
        for matter in matters {
            let entry = authority_entry(&table, matter);
            assert_eq!(entry["builtin"], builtin, "{entry}");
            assert_eq!(entry["effective"], builtin, "{entry}");
            assert_eq!(entry["narrowed"], false, "{entry}");
            assert!(!entry["label"].as_str().unwrap_or_default().is_empty(), "{entry}");
        }
    }
}

#[tokio::test]
async fn autonomous_matters_are_allowed_until_the_workspace_narrows_them() {
    let harness = harness().await;
    let p = two_running_tasks(&harness).await;

    let allowed = controller(&harness, &p.token_a, "arrange_tasks").await;
    assert_eq!(allowed["status"], "allowed", "{allowed}");
    assert_eq!(allowed["authority"], "autonomous");
    assert!(allowed["decision_request_id"].is_null());
    let human = controller(&harness, &p.token_a, "retry_after_recovery_failures").await;
    assert_eq!(human["status"], "human_only", "{human}");
    assert_eq!(human["authority"], "human_only");
    assert!(human["decision_request_id"].is_null());
    assert_eq!(decision_list(&harness, &p.ws).await["pending_count"], 0, "兩者都不建立請求");
    assert_eq!(task_status(&harness, &p.ws, &p.a).await, "running");

    // 收窄：arrange_tasks 改為必須送裁定
    let narrow = json!([{ "matter": "arrange_tasks", "effective": "requires_decision" }]);
    let (status, table) = put_authority(&harness, &p.ws, 0, narrow.clone()).await;
    assert_eq!(status, StatusCode::OK, "{table}");
    assert_eq!(table["revision"], 1);
    let entry = authority_entry(&table, "arrange_tasks");
    assert_eq!(entry["builtin"], "autonomous");
    assert_eq!(entry["effective"], "requires_decision");
    assert_eq!(entry["narrowed"], true);
    assert_eq!(authority_table(&harness, &p.ws).await, table);

    // 放寬或改動「只有人能做」一律拒絕，表不變
    for entries in [
        json!([{ "matter": "modify_done_criteria", "effective": "autonomous" }]),
        json!([{ "matter": "retry_after_recovery_failures", "effective": "requires_decision" }]),
        json!([{ "matter": "retry_after_recovery_failures", "effective": "autonomous" }]),
    ] {
        let (status, response) = put_authority(&harness, &p.ws, 1, entries.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{entries}：{response}");
        assert_eq!(error_code(&response), "authority_widening_refused", "{entries}");
    }
    let (status, response) = put_authority(&harness, &p.ws, 0, narrow.clone()).await;
    assert_eq!(status, StatusCode::CONFLICT, "{response}");
    assert_eq!(error_code(&response), "authority_revision_mismatch");
    let (status, response) =
        put_authority(&harness, &p.ws, 1, json!([{ "matter": "no_such_matter", "effective": "requires_decision" }])).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{response}");
    assert_eq!(authority_table(&harness, &p.ws).await, table, "被拒絕的修改沒有寫入");

    // 收窄後原本自主的事項要送裁定
    let narrowed = controller(&harness, &p.token_a, "arrange_tasks").await;
    assert_eq!(narrowed["status"], "requires_decision", "{narrowed}");
    assert_eq!(narrowed["authority"], "requires_decision");
    let rid = narrowed["decision_request_id"].as_str().unwrap().to_string();
    assert_eq!(decision(&harness, &p.ws, &rid).await["authority"]["matter"], "arrange_tasks");
    assert_eq!(task_status(&harness, &p.ws, &p.a).await, "waiting_decision");

    // 取代式：沒列出的事項回到內建值
    let (status, reset) = put_authority(&harness, &p.ws, 1, json!([])).await;
    assert_eq!(status, StatusCode::OK, "{reset}");
    assert_eq!(reset["revision"], 2);
    assert_eq!(authority_entry(&reset, "arrange_tasks")["effective"], "autonomous");
    assert_eq!(authority_entry(&reset, "arrange_tasks")["narrowed"], false);
    assert_eq!(controller(&harness, &p.token_b, "arrange_tasks").await["status"], "allowed");
}

// --- 權限邊界 ---

#[tokio::test]
async fn an_agent_credential_cannot_use_the_new_desktop_endpoints() {
    let harness = harness().await;
    let p = two_running_tasks(&harness).await;
    let ra = request_ok(&harness, &p.token_a, "要用哪個資料來源？").await;
    let rb = request_ok(&harness, &p.token_b, "報表要用什麼格式？").await;

    let attempts = [
        (
            Method::POST,
            BATCH.to_string(),
            Some(json!({ "items": [{ "workspace_id": p.ws, "request_id": ra, "expected_revision": 1, "action": "accept", "option_id": "ship" }] })),
        ),
        (
            Method::POST,
            merge_path(&p.ws, &rb),
            Some(json!({ "into": ra, "expected_revision": 1, "into_expected_revision": 1 })),
        ),
        (Method::POST, links_path(&p.ws, &rb), Some(json!({ "other_id": ra }))),
        (Method::GET, authority_path(&p.ws), None),
        (
            Method::PUT,
            authority_path(&p.ws),
            Some(json!({ "expected_revision": 0, "entries": [{ "matter": "arrange_tasks", "effective": "requires_decision" }] })),
        ),
    ];
    for token in [&p.token_a, &p.token_b] {
        for (method, path, body) in attempts.clone() {
            let (status, response) = as_agent(&harness, token, method.clone(), &path, body).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{method} {path}：{response}");
            assert_eq!(error_code(&response), "role_not_allowed", "{method} {path}");
        }
    }

    // 什麼都沒有改變
    for rid in [&ra, &rb] {
        let view = decision(&harness, &p.ws, rid).await;
        assert_eq!(view["status"], "pending");
        assert_eq!(view["revision"], 1);
        assert_eq!(view["links"], json!([]));
    }
    assert_eq!(authority_table(&harness, &p.ws).await["revision"], 0);
    assert_eq!(decision_rows(&harness), 0);
}

#[tokio::test]
async fn controller_action_rejects_foreign_tasks_invalid_bodies_and_stale_runs() {
    let harness = harness().await;
    let p = two_running_tasks(&harness).await;
    let other = new_workspace(&harness, "other").await;
    let other_goal = new_goal(&harness, &other).await;
    let foreign = ready_task(&harness, &other, &other_goal, "別的 Workspace 的工作").await;

    for task_id in [foreign.as_str(), "task_nope"] {
        let mut body = controller_body("modify_done_criteria");
        body["task_id"] = json!(task_id);
        let (status, response) = harness.call_tool(&p.token_a, TOOL_CONTROLLER_ACTION, body).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{task_id}：{response}");
        assert_eq!(error_code(&response), "task_not_found");
    }
    for field in ["summary", "reason"] {
        let mut body = controller_body("modify_done_criteria");
        body[field] = json!("   ");
        let (status, response) = harness.call_tool(&p.token_a, TOOL_CONTROLLER_ACTION, body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{field}：{response}");
        assert_eq!(error_code(&response), "decision_invalid", "{field}");
    }
    assert_eq!(decision_list(&harness, &p.ws).await["pending_count"], 0);
    assert_eq!(decision_list(&harness, &other).await["pending_count"], 0);
    assert_eq!(task_status(&harness, &p.ws, &p.a).await, "running");
    assert_eq!(task_status(&harness, &other, &foreign).await, "ready");

    worker_exits(&harness, &p.run_a).await;
    let (status, response) =
        harness.call_tool(&p.token_a, TOOL_CONTROLLER_ACTION, controller_body("modify_done_criteria")).await;
    assert_eq!(status, StatusCode::CONFLICT, "{response}");
    assert_eq!(error_code(&response), "run_report_stale");
    assert_eq!(decision_list(&harness, &p.ws).await["pending_count"], 0);
    let _ = (&p.run_b, &p.token_b);
}

// --- 狀態列的待裁定數 ---

#[tokio::test]
async fn the_status_bar_counts_only_requests_that_still_need_a_decision() {
    let harness = harness().await;
    let p = two_running_tasks(&harness).await;
    let r1 = request_ok(&harness, &p.token_a, "第一個問題").await;
    let r2 = request_ok(&harness, &p.token_a, "第二個問題").await;
    let r3 = request_ok(&harness, &p.token_b, "第三個問題").await;
    let r4 = request_ok(&harness, &p.token_b, "第四個問題").await;
    let count = |pending: &Value| pending["pending_count"].as_i64().unwrap();
    assert_eq!(count(&global_pending(&harness).await), 4);

    accept(&harness, &p.ws, &r1, 1, "ship").await;
    assert_eq!(count(&global_pending(&harness).await), 3, "已回答的不計");

    let (status, deferred) =
        act(&harness, &p.ws, &r2, json!({ "action": "defer", "expected_revision": 1, "defer_minutes": 1440 })).await;
    assert_eq!(status, StatusCode::OK, "{deferred}");
    assert_eq!(count(&global_pending(&harness).await), 3, "延後的仍計入");

    let body = json!({ "into": r3, "expected_revision": 1, "into_expected_revision": 1 });
    let (status, merged) = harness.send_json(Method::POST, &merge_path(&p.ws, &r4), Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{merged}");
    assert_eq!(count(&global_pending(&harness).await), 2, "被合併的不計");

    // B 的契約變了：開啟裁定中心時 r3 被取代
    change_done_criteria(&harness, &p.ws, &p.b, "三個產品").await;
    let list = decision_list(&harness, &p.ws).await;
    assert_eq!(listed(&list, &r3)["status"], "superseded");
    assert_eq!(list["pending_count"], 1);
    let pending = global_pending(&harness).await;
    assert_eq!(count(&pending), 1, "被取代的不計");
    assert_eq!(pending["by_workspace"], json!([{ "workspace_id": p.ws, "pending_count": 1 }]));
    assert_eq!(listed(&list, &r2)["status"], "pending");
}
