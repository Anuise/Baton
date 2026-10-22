//! 情境測試：Goal、Task、六項最小派工契約與工作面板（票 09），經由 Runtime harness 的真實 HTTP API 驗證。
//!
//! 只斷言外部行為：HTTP 回應、重啟後仍在、事件有追加。`harness.db()` 只用來確認「事件有追加、
//! 被拒絕的操作不追加事件、時間以 UTC 毫秒存放」這類必須看落地內容的事實。
//! Task 進入 ready 不會觸發任何執行（Run 與 Scheduler 屬於後續的票），所以最後也驗證沒有 Worker 被 spawn。

use baton_api::{CLIENT_HEADER, CLIENT_HEADER_VALUE};
use baton_domain::auth::Role;
use baton_domain::Timestamp;
use baton_harness::Harness;
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

const WORKSPACES: &str = "/api/v1/workspaces";

/// 2023-11-14 22:13:20.123 UTC
const CLOCK_START: Timestamp = Timestamp::from_millis(1_700_000_000_123);

/// 六項最小契約欄位，依規格的順序。
const FIELDS: [&str; 6] = ["objective", "inputs", "scope", "deliverables", "done_criteria", "blocker_handling"];

// --- 共用輔助 ---

fn plain_dir(harness: &Harness, name: &str) -> String {
    let dir = harness.data_dir().join("projects").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    dir.to_string_lossy().into_owned()
}

/// 建立 Workspace，回傳它的識別。
async fn new_workspace(harness: &Harness, name: &str) -> String {
    let path = plain_dir(harness, name);
    let (status, body) = harness.send_json(Method::POST, WORKSPACES, Some(json!({ "path": path }))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["id"].as_str().unwrap().to_string()
}

fn tasks_path(ws: &str) -> String {
    format!("{WORKSPACES}/{ws}/tasks")
}

fn task_path(ws: &str, tid: &str) -> String {
    format!("{WORKSPACES}/{ws}/tasks/{tid}")
}

fn goals_path(ws: &str) -> String {
    format!("{WORKSPACES}/{ws}/goals")
}

async fn create_goal(harness: &Harness, ws: &str, title: &str) -> (StatusCode, Value) {
    harness.send_json(Method::POST, &goals_path(ws), Some(json!({ "title": title }))).await
}

async fn new_goal(harness: &Harness, ws: &str, title: &str) -> String {
    let (status, body) = create_goal(harness, ws, title).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["id"].as_str().unwrap().to_string()
}

/// 建立 Workspace 與其中一個 Goal。
async fn setup(harness: &Harness) -> (String, String) {
    let ws = new_workspace(harness, "main").await;
    let goal = new_goal(harness, &ws, "第一個目標").await;
    (ws, goal)
}

async fn create_task(harness: &Harness, ws: &str, goal: &str, title: &str, contract: Option<Value>) -> (StatusCode, Value) {
    let mut body = json!({ "goal_id": goal, "title": title });
    if let Some(contract) = contract {
        body["contract"] = contract;
    }
    harness.send_json(Method::POST, &tasks_path(ws), Some(body)).await
}

/// 建立 Task 並要求成功，回傳 TaskView。
async fn new_task(harness: &Harness, ws: &str, goal: &str, title: &str, contract: Option<Value>) -> Value {
    let (status, body) = create_task(harness, ws, goal, title, contract).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body
}

/// 建立契約完整、不需要任何能力的 Task，回傳識別。
async fn new_complete_task(harness: &Harness, ws: &str, goal: &str, title: &str) -> String {
    id_of(&new_task(harness, ws, goal, title, Some(complete_contract())).await)
}

fn id_of(value: &Value) -> String {
    value["id"].as_str().expect("缺少 id").to_string()
}

async fn detail(harness: &Harness, ws: &str, tid: &str) -> Value {
    let (status, body) = harness.send_json(Method::GET, &task_path(ws, tid), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

async fn board(harness: &Harness, ws: &str) -> Value {
    let (status, body) = harness.send_json(Method::GET, &tasks_path(ws), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

/// 看板上的某個 Task。
async fn board_task(harness: &Harness, ws: &str, tid: &str) -> Value {
    board(harness, ws)
        .await["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == json!(tid))
        .unwrap_or_else(|| panic!("看板上沒有 Task {tid}"))
        .clone()
}

async fn put_contract(harness: &Harness, ws: &str, tid: &str, contract: Value, note: Option<&str>) -> (StatusCode, Value) {
    let mut body = json!({ "contract": contract });
    if let Some(note) = note {
        body["note"] = json!(note);
    }
    harness.send_json(Method::PUT, &format!("{}/contract", task_path(ws, tid)), Some(body)).await
}

async fn add_dependency(harness: &Harness, ws: &str, tid: &str, depends_on: &str) -> (StatusCode, Value) {
    let body = json!({ "depends_on": depends_on });
    harness.send_json(Method::POST, &format!("{}/dependencies", task_path(ws, tid)), Some(body)).await
}

async fn ready(harness: &Harness, ws: &str, tid: &str) -> (StatusCode, Value) {
    harness.send_json(Method::POST, &format!("{}/ready", task_path(ws, tid)), None).await
}

async fn cancel(harness: &Harness, ws: &str, tid: &str) -> (StatusCode, Value) {
    harness.send_json(Method::POST, &format!("{}/cancel", task_path(ws, tid)), None).await
}

async fn admission_check(harness: &Harness, ws: &str, tid: &str) -> (StatusCode, Value) {
    harness.send_json(Method::POST, &format!("{}/admission-check", task_path(ws, tid)), None).await
}

async fn make_ready(harness: &Harness, ws: &str, tid: &str) -> Value {
    let (status, body) = ready(harness, ws, tid).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

/// 確認某個授權範本（原封不動；trusted_dev_p3 需要承認風險）。
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
    let acknowledged = template_id == "trusted_dev_p3";
    let body = json!({ "template_id": template_id, "grants": grants, "acknowledged_risk": acknowledged });
    let (status, revision) = harness.send_json(Method::PUT, &path, Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{revision}");
}

fn error_code(body: &Value) -> &str {
    body["error"].as_str().unwrap_or_default()
}

// --- 契約建構 ---

fn filled(text: &str) -> Value {
    json!({ "status": "filled", "text": text })
}

fn filled_with(text: &str, references: Value) -> Value {
    json!({ "status": "filled", "text": text, "references": references })
}

fn not_applicable(reason: &str) -> Value {
    json!({ "status": "not_applicable", "reason": reason })
}

/// 六項都填寫、不需要任何能力的完整契約。
fn complete_contract() -> Value {
    json!({
        "objective": filled("讓使用者能在面板建立 Task"),
        "inputs": filled("既有的 Workspace 與授權設定"),
        "scope": filled("只改 Task 相關模組"),
        "deliverables": filled("可運作的 API 與測試"),
        "done_criteria": filled("情境測試全部通過"),
        "blocker_handling": filled("卡住時回報並等待決策"),
        "required_capabilities": [],
    })
}

/// 完整契約，但把某個欄位換成指定內容。
fn contract_with(field: &str, value: Value) -> Value {
    let mut contract = complete_contract();
    contract[field] = value;
    contract
}

/// 完整契約，但整個省略某些欄位（= 未提供）。
fn contract_without(fields: &[&str]) -> Value {
    let mut contract = complete_contract();
    for field in fields {
        contract.as_object_mut().unwrap().remove(*field);
    }
    contract
}

/// 完整契約，但需要指定的 Workspace 授權能力。
fn contract_requiring(capabilities: &[&str]) -> Value {
    let mut contract = complete_contract();
    contract["required_capabilities"] = json!(capabilities);
    contract
}

/// 缺項轉成排序後的 (code, field) 清單；field 為 null 時以空字串表示。
fn missing_set(missing: &Value) -> Vec<(String, String)> {
    let mut items: Vec<(String, String)> = missing
        .as_array()
        .expect("missing 必須是陣列")
        .iter()
        .map(|m| (m["code"].as_str().unwrap().to_string(), m["field"].as_str().unwrap_or("").to_string()))
        .collect();
    items.sort();
    items
}

fn expected_missing(items: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut items: Vec<(String, String)> = items.iter().map(|(c, f)| (c.to_string(), f.to_string())).collect();
    items.sort();
    items
}

// --- 事件 ---

/// 某個聚合的事件：(event_type, aggregate_version, occurred_at, payload)，依追加順序。
fn aggregate_events(harness: &Harness, aggregate_type: &str, id: &str) -> Vec<(String, i64, i64, Value)> {
    let db = harness.db();
    let mut statement = db
        .prepare(
            "SELECT event_type, aggregate_version, occurred_at, payload FROM event
             WHERE aggregate_type = ?1 AND aggregate_id = ?2 ORDER BY seq",
        )
        .unwrap();
    statement
        .query_map(rusqlite::params![aggregate_type, id], |r| -> rusqlite::Result<(String, i64, i64, Value)> {
            let payload: String = r.get(3)?;
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, serde_json::from_str(&payload).unwrap()))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn task_events(harness: &Harness, tid: &str) -> Vec<(String, i64, i64, Value)> {
    aggregate_events(harness, "task", tid)
}

fn task_event_types(harness: &Harness, tid: &str) -> Vec<String> {
    task_events(harness, tid).into_iter().map(|e| e.0).collect()
}

fn total_events(harness: &Harness) -> i64 {
    harness.db().query_row("SELECT COUNT(*) FROM event", [], |r| r.get(0)).unwrap()
}

// ============================================================
// 1. 六項契約欄位：引用、版本、不適用、未知
// ============================================================

#[tokio::test]
async fn contract_fields_keep_their_references_and_versions_exactly() {
    let harness = Harness::start().await;
    let (ws, goal) = setup(&harness).await;
    let base = new_complete_task(&harness, &ws, &goal, "前置 Task").await;
    let references = json!([
        { "kind": "task_contract", "target": base, "version": 1 },
        { "kind": "goal", "target": goal, "version": 1 },
        { "kind": "authorization", "target": ws, "version": 3 },
    ]);
    let objective = filled_with("依據前置 Task 與目標完成", references);
    let inputs = filled_with("前置 Task 的交付", json!([{ "kind": "task_contract", "target": base, "version": 1 }]));
    let mut contract = contract_with("inputs", inputs.clone());
    contract["objective"] = objective.clone();

    let created = new_task(&harness, &ws, &goal, "有引用的 Task", Some(contract)).await;

    let tid = id_of(&created);
    assert_eq!(created["contract"]["objective"], objective);
    let read_back = detail(&harness, &ws, &tid).await;
    assert_eq!(read_back["task"]["contract"]["objective"], objective, "引用的種類、目標、版本完全一致");
    assert_eq!(read_back["task"]["contract"]["inputs"], inputs);
    assert_eq!(read_back["contract_history"][0]["objective"], objective);
    assert_eq!(read_back["task"]["contract_version"], 1);
    assert_eq!(read_back["task"]["contract"]["version"], 1);
    assert_eq!(read_back["task"]["contract"]["task_id"], json!(tid));
}

#[tokio::test]
async fn not_applicable_is_saved_with_its_reason() {
    let harness = Harness::start().await;
    let (ws, goal) = setup(&harness).await;
    let inputs = not_applicable("這是全新的工作，沒有既有輸入");

    let created = new_task(&harness, &ws, &goal, "不適用", Some(contract_with("inputs", inputs.clone()))).await;

    assert_eq!(detail(&harness, &ws, &id_of(&created)).await["task"]["contract"]["inputs"], inputs);
}

#[tokio::test]
async fn not_applicable_without_a_reason_is_rejected() {
    let harness = Harness::start().await;
    let (ws, goal) = setup(&harness).await;
    let existing = new_complete_task(&harness, &ws, &goal, "既有").await;

    let (empty_status, empty) =
        create_task(&harness, &ws, &goal, "空理由", Some(contract_with("scope", not_applicable("")))).await;
    let (blank_status, blank) =
        create_task(&harness, &ws, &goal, "空白理由", Some(contract_with("scope", not_applicable("   ")))).await;
    let (put_status, put) = put_contract(&harness, &ws, &existing, contract_with("scope", not_applicable("")), None).await;

    for (status, body) in [(empty_status, &empty), (blank_status, &blank), (put_status, &put)] {
        assert_eq!((status, error_code(body)), (StatusCode::BAD_REQUEST, "contract_invalid"), "{body}");
    }
    assert_eq!(board(&harness, &ws).await["tasks"].as_array().unwrap().len(), 1, "被拒絕的 Task 不會被建立");
    assert_eq!(detail(&harness, &ws, &existing).await["task"]["contract_version"], 1);
}

#[tokio::test]
async fn objective_and_done_criteria_cannot_be_not_applicable() {
    let harness = Harness::start().await;
    let (ws, goal) = setup(&harness).await;
    let existing = new_complete_task(&harness, &ws, &goal, "既有").await;

    for field in ["objective", "done_criteria"] {
        let contract = contract_with(field, not_applicable("我覺得不需要"));
        let (create_status, create_body) = create_task(&harness, &ws, &goal, "不可不適用", Some(contract.clone())).await;
        let (put_status, put_body) = put_contract(&harness, &ws, &existing, contract, None).await;

        assert_eq!((create_status, error_code(&create_body)), (StatusCode::BAD_REQUEST, "contract_invalid"), "{field}");
        assert_eq!((put_status, error_code(&put_body)), (StatusCode::BAD_REQUEST, "contract_invalid"), "{field}");
    }
    assert_eq!(board(&harness, &ws).await["tasks"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn unknown_is_read_back_as_unknown_and_flagged_distinctly_from_missing() {
    let harness = Harness::start().await;
    let (ws, goal) = setup(&harness).await;
    let inputs = json!({ "status": "unknown", "note": "還要問客戶" });
    let mut contract = contract_with("inputs", inputs.clone());
    contract.as_object_mut().unwrap().remove("scope");

    let created = new_task(&harness, &ws, &goal, "有未知欄位", Some(contract)).await;

    let tid = id_of(&created);
    let read_back = detail(&harness, &ws, &tid).await;
    assert_eq!(read_back["task"]["contract"]["inputs"], inputs, "未知仍是 unknown，說明被保存");
    assert_eq!(read_back["task"]["contract"]["scope"], Value::Null, "未提供讀回為 null");
    let missing = &board_task(&harness, &ws, &tid).await["missing"];
    assert_eq!(missing_set(missing), expected_missing(&[("field_unknown", "inputs"), ("field_missing", "scope")]));
}

#[tokio::test]
async fn blank_text_in_a_filled_field_is_rejected() {
    let harness = Harness::start().await;
    let (ws, goal) = setup(&harness).await;
    let existing = new_complete_task(&harness, &ws, &goal, "既有").await;

    for blank in ["", "   ", "\t\n"] {
        let contract = contract_with("deliverables", filled(blank));
        let (create_status, create_body) = create_task(&harness, &ws, &goal, "空白文字", Some(contract.clone())).await;
        let (put_status, put_body) = put_contract(&harness, &ws, &existing, contract, None).await;

        assert_eq!((create_status, error_code(&create_body)), (StatusCode::BAD_REQUEST, "contract_invalid"), "{blank:?}");
        assert_eq!((put_status, error_code(&put_body)), (StatusCode::BAD_REQUEST, "contract_invalid"), "{blank:?}");
    }
    assert_eq!(detail(&harness, &ws, &existing).await["task"]["contract_version"], 1);
}

// ============================================================
// 2. 相依：自我、循環、重複、跨 Workspace
// ============================================================

#[tokio::test]
async fn a_task_cannot_depend_on_itself() {
    let harness = Harness::start().await;
    let (ws, goal) = setup(&harness).await;
    let a = new_complete_task(&harness, &ws, &goal, "A").await;
    let events_before = total_events(&harness);

    let (status, body) = add_dependency(&harness, &ws, &a, &a).await;

    assert_eq!((status, error_code(&body)), (StatusCode::BAD_REQUEST, "dependency_self"), "{body}");
    assert_eq!(detail(&harness, &ws, &a).await["task"]["dependencies"], json!([]));
    assert_eq!(total_events(&harness), events_before);
}

#[tokio::test]
async fn a_dependency_that_closes_a_cycle_is_rejected_and_changes_nothing() {
    let harness = Harness::start().await;
    let (ws, goal) = setup(&harness).await;
    let a = new_complete_task(&harness, &ws, &goal, "A").await;
    let b = new_complete_task(&harness, &ws, &goal, "B").await;
    let c = new_complete_task(&harness, &ws, &goal, "C").await;
    assert_eq!(add_dependency(&harness, &ws, &a, &b).await.0, StatusCode::CREATED);
    assert_eq!(add_dependency(&harness, &ws, &b, &c).await.0, StatusCode::CREATED);
    let events_before = total_events(&harness);
    let board_before = board(&harness, &ws).await;

    let (status, body) = add_dependency(&harness, &ws, &c, &a).await;

    assert_eq!((status, error_code(&body)), (StatusCode::CONFLICT, "dependency_cycle"), "{body}");
    let message = body["message"].as_str().unwrap();
    for id in [&a, &b, &c] {
        assert!(message.contains(id.as_str()), "訊息要含循環路徑上的 {id}：{message}");
    }
    assert_eq!(detail(&harness, &ws, &c).await["task"]["dependencies"], json!([]));
    assert_eq!(board(&harness, &ws).await, board_before);
    assert_eq!(total_events(&harness), events_before, "被拒絕的相依不追加事件");
}

#[tokio::test]
async fn two_tasks_cannot_depend_on_each_other() {
    let harness = Harness::start().await;
    let (ws, goal) = setup(&harness).await;
    let a = new_complete_task(&harness, &ws, &goal, "A").await;
    let b = new_complete_task(&harness, &ws, &goal, "B").await;
    assert_eq!(add_dependency(&harness, &ws, &a, &b).await.0, StatusCode::CREATED);
    let events_before = total_events(&harness);

    let (status, body) = add_dependency(&harness, &ws, &b, &a).await;

    assert_eq!((status, error_code(&body)), (StatusCode::CONFLICT, "dependency_cycle"), "{body}");
    assert_eq!(detail(&harness, &ws, &b).await["task"]["dependencies"], json!([]));
    assert_eq!(detail(&harness, &ws, &a).await["task"]["dependencies"], json!([b]));
    assert_eq!(total_events(&harness), events_before);
}

#[tokio::test]
async fn adding_a_dependency_lists_it_and_the_same_edge_again_is_a_no_op() {
    let harness = Harness::start().await;
    let (ws, goal) = setup(&harness).await;
    let a = new_complete_task(&harness, &ws, &goal, "A").await;
    let b = new_complete_task(&harness, &ws, &goal, "B").await;

    let (first_status, first) = add_dependency(&harness, &ws, &a, &b).await;
    let events_after_first = total_events(&harness);
    let (again_status, again) = add_dependency(&harness, &ws, &a, &b).await;

    assert_eq!(first_status, StatusCode::CREATED, "{first}");
    assert_eq!(first["dependencies"], json!([b]));
    assert_eq!(again_status, StatusCode::OK, "{again}");
    assert_eq!(again, first, "重複新增不改變任何東西");
    assert_eq!(total_events(&harness), events_after_first);
    assert_eq!(
        task_event_types(&harness, &a).iter().filter(|t| *t == "task_dependency_added").count(),
        1
    );
}

#[tokio::test]
async fn a_dependency_across_workspaces_or_on_a_missing_task_is_not_found() {
    let harness = Harness::start().await;
    let (ws, goal) = setup(&harness).await;
    let other_ws = new_workspace(&harness, "other").await;
    let other_goal = new_goal(&harness, &other_ws, "別的目標").await;
    let local = new_complete_task(&harness, &ws, &goal, "本地").await;
    let foreign = new_complete_task(&harness, &other_ws, &other_goal, "外地").await;
    let events_before = total_events(&harness);

    let (cross_status, cross) = add_dependency(&harness, &ws, &local, &foreign).await;
    let (reverse_status, reverse) = add_dependency(&harness, &other_ws, &foreign, &local).await;
    let (missing_status, missing) = add_dependency(&harness, &ws, &local, "task_nope").await;
    let (source_status, source) = add_dependency(&harness, &ws, "task_nope", &local).await;

    for (status, body) in [(cross_status, &cross), (reverse_status, &reverse), (missing_status, &missing), (source_status, &source)] {
        assert_eq!((status, error_code(body)), (StatusCode::NOT_FOUND, "task_not_found"), "{body}");
    }
    assert_eq!(detail(&harness, &ws, &local).await["task"]["dependencies"], json!([]));
    assert_eq!(detail(&harness, &other_ws, &foreign).await["task"]["dependencies"], json!([]));
    assert_eq!(total_events(&harness), events_before);
}

#[tokio::test]
async fn a_cancelled_task_cannot_take_new_dependencies() {
    let harness = Harness::start().await;
    let (ws, goal) = setup(&harness).await;
    let a = new_complete_task(&harness, &ws, &goal, "A").await;
    let b = new_complete_task(&harness, &ws, &goal, "B").await;
    assert_eq!(cancel(&harness, &ws, &a).await.0, StatusCode::OK);

    let (status, body) = add_dependency(&harness, &ws, &a, &b).await;

    assert_eq!((status, error_code(&body)), (StatusCode::CONFLICT, "task_closed"), "{body}");
    assert_eq!(detail(&harness, &ws, &a).await["task"]["dependencies"], json!([]));
}

// ============================================================
// 3. 契約不完整的 Task 不能 ready
// ============================================================

#[tokio::test]
async fn a_task_with_an_empty_contract_is_pending_at_contract_version_one() {
    let harness = Harness::start().await;
    let (ws, goal) = setup(&harness).await;

    let created = new_task(&harness, &ws, &goal, "什麼都沒填", None).await;

    assert_eq!(created["status"], "pending");
    assert_eq!(created["contract_version"], 1);
    assert_eq!(created["version"], 1);
    assert_eq!(created["goal_id"], json!(goal));
    assert_eq!(created["workspace_id"], json!(ws));
    assert_eq!(created["title"], "什麼都沒填");
    assert_eq!(created["dependencies"], json!([]));
    assert_eq!(created["contract"]["required_capabilities"], json!([]));
    for field in FIELDS {
        assert_eq!(created["contract"][field], Value::Null, "{field}");
    }
}

#[tokio::test]
async fn a_task_missing_contract_items_cannot_become_ready_and_every_missing_item_is_listed() {
    let harness = Harness::start().await;
    let (ws, goal) = setup(&harness).await;
    let created = new_task(&harness, &ws, &goal, "什麼都沒填", None).await;
    let tid = id_of(&created);
    let events_before = task_events(&harness, &tid);

    let (status, body) = ready(&harness, &ws, &tid).await;

    assert_eq!((status, error_code(&body)), (StatusCode::CONFLICT, "task_admission_denied"), "{body}");
    let missing = body["missing"].as_array().unwrap();
    assert_eq!(missing.len(), 6);
    for field in FIELDS {
        let item = missing.iter().find(|m| m["field"] == field).unwrap_or_else(|| panic!("缺項沒有列出 {field}：{body}"));
        assert_eq!(item["code"], "field_missing", "{field}");
        assert!(!item["message"].as_str().unwrap().is_empty(), "{field} 要有說明");
    }
    assert_eq!(detail(&harness, &ws, &tid).await["task"]["status"], "pending");
    assert_eq!(task_events(&harness, &tid), events_before, "被拒絕的 ready 不追加事件");
}

#[tokio::test]
async fn the_board_shows_the_same_missing_items_for_a_pending_task() {
    let harness = Harness::start().await;
    let (ws, goal) = setup(&harness).await;
    let contract = contract_without(&["inputs", "blocker_handling"]);
    let tid = id_of(&new_task(&harness, &ws, &goal, "缺兩項", Some(contract)).await);

    let (_, denied) = ready(&harness, &ws, &tid).await;
    let on_board = board_task(&harness, &ws, &tid).await;

    let expected = expected_missing(&[("field_missing", "inputs"), ("field_missing", "blocker_handling")]);
    assert_eq!(missing_set(&denied["missing"]), expected);
    assert_eq!(on_board["status"], "pending");
    assert_eq!(missing_set(&on_board["missing"]), expected);
}

#[tokio::test]
async fn completing_the_contract_lets_the_task_become_ready() {
    let harness = Harness::start().await;
    let (ws, goal) = setup(&harness).await;
    let tid = id_of(&new_task(&harness, &ws, &goal, "補齊後", Some(contract_without(&["scope"]))).await);
    assert_eq!(ready(&harness, &ws, &tid).await.0, StatusCode::CONFLICT);

    let (put_status, put) = put_contract(&harness, &ws, &tid, complete_contract(), Some("補上範圍")).await;
    let (status, body) = ready(&harness, &ws, &tid).await;

    assert_eq!(put_status, StatusCode::CREATED, "{put}");
    assert_eq!(put["status"], "pending", "補齊契約本身不會讓 Task 變 ready");
    assert_eq!(put["missing"], json!([]));
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "ready");
    assert_eq!(board_task(&harness, &ws, &tid).await["status"], "ready");
}

#[tokio::test]
async fn an_unknown_field_keeps_the_task_from_becoming_ready_until_it_is_filled() {
    let harness = Harness::start().await;
    let (ws, goal) = setup(&harness).await;
    let contract = contract_with("inputs", json!({ "status": "unknown" }));
    let tid = id_of(&new_task(&harness, &ws, &goal, "含未知", Some(contract)).await);

    let (status, body) = ready(&harness, &ws, &tid).await;

    assert_eq!((status, error_code(&body)), (StatusCode::CONFLICT, "task_admission_denied"), "{body}");
    assert_eq!(missing_set(&body["missing"]), expected_missing(&[("field_unknown", "inputs")]));
    assert_eq!(put_contract(&harness, &ws, &tid, complete_contract(), None).await.0, StatusCode::CREATED);
    assert_eq!(ready(&harness, &ws, &tid).await.0, StatusCode::OK);
}

#[tokio::test]
async fn not_applicable_fields_with_reasons_count_as_complete() {
    let harness = Harness::start().await;
    let (ws, goal) = setup(&harness).await;
    let mut contract = complete_contract();
    for field in ["inputs", "scope", "deliverables", "blocker_handling"] {
        contract[field] = not_applicable("本次工作用不到");
    }
    let tid = id_of(&new_task(&harness, &ws, &goal, "多項不適用", Some(contract)).await);

    let (status, body) = ready(&harness, &ws, &tid).await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "ready");
}

// ============================================================
// 4. 契約版本歷史
// ============================================================

#[tokio::test]
async fn each_different_contract_creates_a_new_version_and_the_same_content_does_not() {
    let harness = Harness::start().await;
    let (ws, goal) = setup(&harness).await;
    let created = new_task(&harness, &ws, &goal, "版本", Some(complete_contract())).await;
    let tid = id_of(&created);
    let v1 = created["contract"].clone();
    let v2_input = contract_with("scope", filled("縮小後的範圍"));
    let v3_input = {
        let mut c = v2_input.clone();
        c["deliverables"] = filled("多一份文件");
        c
    };

    let (second_status, second) = put_contract(&harness, &ws, &tid, v2_input.clone(), Some("縮小範圍")).await;
    let (same_status, same) = put_contract(&harness, &ws, &tid, v2_input, None).await;
    let (third_status, third) = put_contract(&harness, &ws, &tid, v3_input, None).await;

    assert_eq!(second_status, StatusCode::CREATED, "{second}");
    assert_eq!(second["contract_version"], 2);
    assert_eq!(second["contract"]["version"], 2);
    assert_eq!(same_status, StatusCode::OK, "{same}");
    assert_eq!(same["contract_version"], 2, "內容相同不新增版本");
    assert_eq!(third_status, StatusCode::CREATED, "{third}");
    assert_eq!(third["contract_version"], 3);
    let history = detail(&harness, &ws, &tid).await["contract_history"].as_array().unwrap().clone();
    assert_eq!(history.iter().map(|c| c["version"].as_i64().unwrap()).collect::<Vec<_>>(), [1, 2, 3], "由舊到新");
    assert_eq!(history[0], v1, "舊版本內容不變");
    assert_eq!(history[0]["scope"]["text"], "只改 Task 相關模組");
    assert_eq!(history[1]["scope"]["text"], "縮小後的範圍");
    assert_eq!(history[1]["note"], "縮小範圍", "變更說明被保存");
    assert_eq!(history[0]["note"], Value::Null);
    assert_eq!(history[2]["deliverables"]["text"], "多一份文件");
    assert_eq!(history[2], third["contract"], "目前版本是歷史的最後一筆");
}

#[tokio::test]
async fn contract_history_and_current_version_survive_rebuilding_the_runtime() {
    let mut harness = Harness::start().await;
    let (ws, goal) = setup(&harness).await;
    let tid = new_complete_task(&harness, &ws, &goal, "持久化").await;
    put_contract(&harness, &ws, &tid, contract_with("scope", filled("第二版")), Some("第二版說明")).await;
    put_contract(&harness, &ws, &tid, contract_with("scope", filled("第三版")), None).await;
    let before = detail(&harness, &ws, &tid).await;

    harness.restart().await;

    let after = detail(&harness, &ws, &tid).await;
    assert_eq!(after, before);
    assert_eq!(after["contract_history"].as_array().unwrap().len(), 3);
    assert_eq!(after["task"]["contract_version"], 3);
    assert_eq!(after["contract_history"][1]["note"], "第二版說明");
}

#[tokio::test]
async fn changing_the_contract_of_a_ready_task_sends_it_back_to_pending() {
    let harness = Harness::start().await;
    let (ws, goal) = setup(&harness).await;
    let tid = new_complete_task(&harness, &ws, &goal, "改契約").await;
    make_ready(&harness, &ws, &tid).await;

    let (status, changed) = put_contract(&harness, &ws, &tid, contract_with("scope", filled("新範圍")), None).await;

    assert_eq!(status, StatusCode::CREATED, "{changed}");
    assert_eq!(changed["status"], "pending", "必須重新通過准入");
    assert_eq!(changed["contract_version"], 2);
    assert_eq!(board_task(&harness, &ws, &tid).await["status"], "pending");
    let events = task_events(&harness, &tid);
    assert_eq!(events.iter().filter(|e| e.0 == "task_contract_changed").count(), 1);
    let transitions: Vec<(String, String)> = events
        .iter()
        .filter(|e| e.0 == "task_status_changed")
        .map(|e| (e.3["from"].as_str().unwrap().to_string(), e.3["to"].as_str().unwrap().to_string()))
        .collect();
    assert_eq!(
        transitions,
        [("pending".to_string(), "ready".to_string()), ("ready".to_string(), "pending".to_string())]
    );
    assert_eq!(make_ready(&harness, &ws, &tid).await["status"], "ready", "重新准入後可再次 ready");
}

#[tokio::test]
async fn changing_the_contract_of_a_pending_task_keeps_it_pending() {
    let harness = Harness::start().await;
    let (ws, goal) = setup(&harness).await;
    let tid = new_complete_task(&harness, &ws, &goal, "仍是 pending").await;

    let (status, changed) = put_contract(&harness, &ws, &tid, contract_with("scope", filled("新範圍")), None).await;

    assert_eq!(status, StatusCode::CREATED, "{changed}");
    assert_eq!(changed["status"], "pending");
    assert!(!task_event_types(&harness, &tid).contains(&"task_status_changed".to_string()));
}

#[tokio::test]
async fn a_cancelled_task_contract_cannot_be_changed() {
    let harness = Harness::start().await;
    let (ws, goal) = setup(&harness).await;
    let tid = new_complete_task(&harness, &ws, &goal, "已取消").await;
    assert_eq!(cancel(&harness, &ws, &tid).await.0, StatusCode::OK);
    let events_before = task_events(&harness, &tid);

    let (status, body) = put_contract(&harness, &ws, &tid, contract_with("scope", filled("事後修改")), None).await;

    assert_eq!((status, error_code(&body)), (StatusCode::CONFLICT, "task_closed"), "{body}");
    let after = detail(&harness, &ws, &tid).await;
    assert_eq!(after["task"]["contract_version"], 1);
    assert_eq!(after["contract_history"].as_array().unwrap().len(), 1);
    assert_eq!(task_events(&harness, &tid), events_before);
}

// ============================================================
// 5. 准入檢查：引用、授權
// ============================================================

#[tokio::test]
async fn contract_references_are_not_checked_when_saving_but_invalid_ones_block_ready() {
    let harness = Harness::start().await;
    let (ws, goal) = setup(&harness).await;
    confirm_template(&harness, &ws, "research_docs").await;
    let base = new_complete_task(&harness, &ws, &goal, "基礎").await;
    let cancelled = new_complete_task(&harness, &ws, &goal, "已取消的引用對象").await;
    assert_eq!(cancel(&harness, &ws, &cancelled).await.0, StatusCode::OK);
    let reference = |kind: &str, target: &str, version: i64| json!([{ "kind": kind, "target": target, "version": version }]);
    let mut contract = complete_contract();
    contract["objective"] = filled_with("引用不存在的 Task", reference("task_contract", "task_nope", 1));
    contract["inputs"] = filled_with("引用不存在的契約版本", reference("task_contract", &base, 2));
    contract["scope"] = filled_with("引用已取消的 Task", reference("task_contract", &cancelled, 1));
    contract["deliverables"] = filled_with("引用不存在的 Goal", reference("goal", "goal_nope", 1));
    contract["done_criteria"] = filled_with("引用超過現有的授權 revision", reference("authorization", &ws, 2));
    contract["blocker_handling"] = filled_with(
        "這些引用都有效",
        json!([
            { "kind": "goal", "target": goal, "version": 1 },
            { "kind": "task_contract", "target": base, "version": 1 },
            { "kind": "authorization", "target": ws, "version": 1 },
        ]),
    );

    let created = new_task(&harness, &ws, &goal, "引用失效", Some(contract)).await;
    let tid = id_of(&created);
    let (status, body) = ready(&harness, &ws, &tid).await;

    assert_eq!((status, error_code(&body)), (StatusCode::CONFLICT, "task_admission_denied"), "{body}");
    assert_eq!(
        missing_set(&body["missing"]),
        expected_missing(&[
            ("reference_invalid", "objective"),
            ("reference_invalid", "inputs"),
            ("reference_invalid", "scope"),
            ("reference_invalid", "deliverables"),
            ("reference_invalid", "done_criteria"),
        ]),
        "有效的引用不會被列為缺項"
    );
    let message_of = |field: &str| {
        body["missing"].as_array().unwrap().iter().find(|m| m["field"] == field).unwrap()["message"].as_str().unwrap().to_string()
    };
    assert!(message_of("objective").contains("task_nope"), "{}", message_of("objective"));
    assert!(message_of("deliverables").contains("goal_nope"), "{}", message_of("deliverables"));
    assert_eq!(missing_set(&board_task(&harness, &ws, &tid).await["missing"]), missing_set(&body["missing"]));
    assert_eq!(detail(&harness, &ws, &tid).await["task"]["status"], "pending");
}

#[tokio::test]
async fn fixing_the_invalid_references_lets_the_task_become_ready() {
    let harness = Harness::start().await;
    let (ws, goal) = setup(&harness).await;
    let base = new_complete_task(&harness, &ws, &goal, "基礎").await;
    let broken = contract_with(
        "objective",
        filled_with("引用還不存在的 Task", json!([{ "kind": "task_contract", "target": "task_nope", "version": 1 }])),
    );
    let tid = id_of(&new_task(&harness, &ws, &goal, "先存再補", Some(broken)).await);
    assert_eq!(ready(&harness, &ws, &tid).await.0, StatusCode::CONFLICT);

    let fixed = contract_with(
        "objective",
        filled_with("引用基礎 Task", json!([{ "kind": "task_contract", "target": base, "version": 1 }])),
    );
    assert_eq!(put_contract(&harness, &ws, &tid, fixed, None).await.0, StatusCode::CREATED);
    let (status, body) = ready(&harness, &ws, &tid).await;

    assert_eq!(status, StatusCode::OK, "{body}");
}

#[tokio::test]
async fn a_referenced_contract_version_becomes_valid_once_that_version_exists() {
    let harness = Harness::start().await;
    let (ws, goal) = setup(&harness).await;
    let base = new_complete_task(&harness, &ws, &goal, "基礎").await;
    let contract = contract_with(
        "objective",
        filled_with("引用基礎 Task 的第 2 版", json!([{ "kind": "task_contract", "target": base, "version": 2 }])),
    );
    let tid = id_of(&new_task(&harness, &ws, &goal, "等待第 2 版", Some(contract)).await);
    let (_, denied) = admission_check(&harness, &ws, &tid).await;

    put_contract(&harness, &ws, &base, contract_with("scope", filled("基礎的新範圍")), None).await;
    let (status, allowed) = admission_check(&harness, &ws, &tid).await;

    assert_eq!(denied["admissible"], false);
    assert_eq!(missing_set(&denied["missing"]), expected_missing(&[("reference_invalid", "objective")]));
    assert_eq!(status, StatusCode::OK);
    assert_eq!(allowed["admissible"], true, "{allowed}");
}

#[tokio::test]
async fn a_task_that_needs_capabilities_is_refused_until_the_workspace_confirms_authorization() {
    let harness = Harness::start().await;
    let (ws, goal) = setup(&harness).await;
    let tid = id_of(&new_task(&harness, &ws, &goal, "要跑建置", Some(contract_requiring(&["run_build_test"]))).await);

    let (status, body) = ready(&harness, &ws, &tid).await;

    assert_eq!((status, error_code(&body)), (StatusCode::CONFLICT, "task_admission_denied"), "{body}");
    assert_eq!(missing_set(&body["missing"]), expected_missing(&[("authorization_unconfirmed", "")]));
    let item = &body["missing"][0];
    assert_eq!(item["field"], Value::Null);
    assert!(!item["message"].as_str().unwrap().is_empty());
    assert_eq!(detail(&harness, &ws, &tid).await["task"]["status"], "pending");
}

#[tokio::test]
async fn a_capability_the_confirmed_authorization_does_not_grant_blocks_ready_until_it_is_granted() {
    let harness = Harness::start().await;
    let (ws, goal) = setup(&harness).await;
    confirm_template(&harness, &ws, "research_docs").await;
    let tid = id_of(&new_task(&harness, &ws, &goal, "要跑建置", Some(contract_requiring(&["run_build_test"]))).await);

    let (status, body) = ready(&harness, &ws, &tid).await;
    let (_, preview) = admission_check(&harness, &ws, &tid).await;

    assert_eq!((status, error_code(&body)), (StatusCode::CONFLICT, "task_admission_denied"), "{body}");
    assert_eq!(missing_set(&body["missing"]), expected_missing(&[("capability_not_granted", "")]));
    let message = body["missing"][0]["message"].as_str().unwrap();
    assert!(message.contains("run_build_test") || message.contains("建置與測試"), "要列出缺少的能力：{message}");
    assert_eq!(preview["admissible"], false);

    confirm_template(&harness, &ws, "trusted_dev_p3").await;
    let (ready_status, ready_body) = ready(&harness, &ws, &tid).await;

    assert_eq!(ready_status, StatusCode::OK, "{ready_body}");
    assert_eq!(ready_body["status"], "ready", "改確認 P3 之後，同一個 Task 可以 ready");
}

#[tokio::test]
async fn a_task_with_no_required_capabilities_can_become_ready_without_any_authorization() {
    let harness = Harness::start().await;
    let (ws, goal) = setup(&harness).await;
    let tid = new_complete_task(&harness, &ws, &goal, "不需要能力").await;

    let (status, body) = ready(&harness, &ws, &tid).await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "ready");
}

#[tokio::test]
async fn required_capabilities_are_deduplicated_and_sorted_when_read_back() {
    let harness = Harness::start().await;
    let (ws, goal) = setup(&harness).await;
    let contract = contract_requiring(&["write_files", "read_files", "write_files"]);

    let created = new_task(&harness, &ws, &goal, "能力去重", Some(contract)).await;

    assert_eq!(created["contract"]["required_capabilities"], json!(["read_files", "write_files"]));
}

#[tokio::test]
async fn admission_check_is_a_read_only_preview_for_a_pending_task() {
    let harness = Harness::start().await;
    let (ws, goal) = setup(&harness).await;
    let incomplete = id_of(&new_task(&harness, &ws, &goal, "不完整", Some(contract_without(&["scope"]))).await);
    let complete = new_complete_task(&harness, &ws, &goal, "完整").await;
    let events_before = total_events(&harness);
    let board_before = board(&harness, &ws).await;

    let (denied_status, denied) = admission_check(&harness, &ws, &incomplete).await;
    let (allowed_status, allowed) = admission_check(&harness, &ws, &complete).await;

    assert_eq!(denied_status, StatusCode::OK, "{denied}");
    assert_eq!(denied["admissible"], false);
    assert_eq!(missing_set(&denied["missing"]), expected_missing(&[("field_missing", "scope")]));
    assert_eq!(allowed_status, StatusCode::OK, "{allowed}");
    assert_eq!(allowed["admissible"], true);
    assert_eq!(allowed["missing"], json!([]));
    assert_eq!(board(&harness, &ws).await, board_before, "預覽不改任何狀態");
    assert_eq!(total_events(&harness), events_before, "預覽不追加事件");
}

#[tokio::test]
async fn admission_check_runs_the_full_check_for_a_task_that_is_already_ready() {
    let harness = Harness::start().await;
    let (ws, goal) = setup(&harness).await;
    let tid = new_complete_task(&harness, &ws, &goal, "已 ready").await;
    make_ready(&harness, &ws, &tid).await;

    let (status, body) = admission_check(&harness, &ws, &tid).await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["admissible"], true);
    assert_eq!(detail(&harness, &ws, &tid).await["task"]["status"], "ready");
}

#[tokio::test]
async fn a_task_in_an_archived_workspace_is_not_admissible() {
    let harness = Harness::start().await;
    let (ws, goal) = setup(&harness).await;
    let tid = new_complete_task(&harness, &ws, &goal, "封存前建立").await;
    harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/archive"), None).await;

    let (status, body) = admission_check(&harness, &ws, &tid).await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["admissible"], false);
    assert!(missing_set(&body["missing"]).contains(&("workspace_archived".to_string(), String::new())), "{body}");
}

#[tokio::test]
async fn admission_check_for_an_unknown_task_is_not_found() {
    let harness = Harness::start().await;
    let (ws, _) = setup(&harness).await;

    let (status, body) = admission_check(&harness, &ws, "task_nope").await;

    assert_eq!((status, error_code(&body)), (StatusCode::NOT_FOUND, "task_not_found"));
}

// ============================================================
// 6. 狀態轉換
// ============================================================

#[tokio::test]
async fn only_a_pending_task_can_become_ready() {
    let harness = Harness::start().await;
    let (ws, goal) = setup(&harness).await;
    let tid = new_complete_task(&harness, &ws, &goal, "只能 ready 一次").await;
    make_ready(&harness, &ws, &tid).await;
    let events_before = task_events(&harness, &tid);

    let (status, body) = ready(&harness, &ws, &tid).await;

    assert_eq!((status, error_code(&body)), (StatusCode::CONFLICT, "task_transition_invalid"), "{body}");
    assert_eq!(task_events(&harness, &tid), events_before);
}

#[tokio::test]
async fn cancelling_is_final_and_a_cancelled_task_cannot_become_ready_again() {
    let harness = Harness::start().await;
    let (ws, goal) = setup(&harness).await;
    let tid = new_complete_task(&harness, &ws, &goal, "取消").await;

    let (status, cancelled) = cancel(&harness, &ws, &tid).await;
    let (again_status, again) = cancel(&harness, &ws, &tid).await;
    let (ready_status, ready_body) = ready(&harness, &ws, &tid).await;

    assert_eq!(status, StatusCode::OK, "{cancelled}");
    assert_eq!(cancelled["status"], "cancelled");
    assert_eq!((again_status, error_code(&again)), (StatusCode::CONFLICT, "task_transition_invalid"), "{again}");
    assert_eq!((ready_status, error_code(&ready_body)), (StatusCode::CONFLICT, "task_transition_invalid"), "{ready_body}");
    assert_eq!(detail(&harness, &ws, &tid).await["task"]["status"], "cancelled");
}

#[tokio::test]
async fn a_ready_task_can_be_cancelled() {
    let harness = Harness::start().await;
    let (ws, goal) = setup(&harness).await;
    let tid = new_complete_task(&harness, &ws, &goal, "ready 後取消").await;
    make_ready(&harness, &ws, &tid).await;

    let (status, body) = cancel(&harness, &ws, &tid).await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "cancelled");
}

#[tokio::test]
async fn ready_does_not_wait_for_dependencies_but_the_board_shows_what_it_waits_on() {
    let harness = Harness::start().await;
    let (ws, goal) = setup(&harness).await;
    let a = new_complete_task(&harness, &ws, &goal, "A").await;
    let b = new_complete_task(&harness, &ws, &goal, "B").await;
    add_dependency(&harness, &ws, &a, &b).await;

    let (status, body) = ready(&harness, &ws, &a).await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["dependencies"], json!([b]));
    assert_eq!(body["unfinished_dependencies"], json!([b]), "B 尚未 completed");
}

// ============================================================
// 7. 工作面板資料
// ============================================================

#[tokio::test]
async fn a_new_workspace_has_an_empty_board_with_all_twelve_capabilities() {
    let harness = Harness::start().await;
    let ws = new_workspace(&harness, "empty").await;

    let board = board(&harness, &ws).await;

    assert_eq!(board["workspace_id"], json!(ws));
    assert_eq!(board["goals"], json!([]));
    assert_eq!(board["tasks"], json!([]));
    let capabilities = board["capabilities"].as_array().unwrap();
    assert_eq!(capabilities.len(), 12);
    let read_files = capabilities.iter().find(|c| c["capability"] == "read_files").unwrap();
    assert_eq!(read_files["label"], "讀取檔案");
    assert_eq!(read_files["high_risk"], false);
    assert_eq!(capabilities.iter().filter(|c| c["high_risk"] == true).count(), 7);
}

#[tokio::test]
async fn the_board_lists_goals_and_tasks_in_creation_order() {
    let harness = Harness::builder().clock_at(CLOCK_START).start().await;
    let ws = new_workspace(&harness, "order").await;
    let first_goal = new_goal(&harness, &ws, "  第一個目標  ").await;
    harness.clock.advance_millis(1_000);
    let second_goal = new_goal(&harness, &ws, "第二個目標").await;
    let t1 = id_of(&new_task(&harness, &ws, &second_goal, "任務一", None).await);
    let t2 = id_of(&new_task(&harness, &ws, &first_goal, "任務二", None).await);
    let t3 = id_of(&new_task(&harness, &ws, &first_goal, "任務三", None).await);

    let board = board(&harness, &ws).await;

    let goals = board["goals"].as_array().unwrap();
    assert_eq!(goals.iter().map(|g| g["title"].as_str().unwrap()).collect::<Vec<_>>(), ["第一個目標", "第二個目標"]);
    assert_eq!(goals[0]["id"], json!(first_goal));
    assert_eq!(goals[0]["workspace_id"], json!(ws));
    assert_eq!(goals[0]["version"], 1);
    assert_eq!(goals[0]["created_at"], CLOCK_START.as_millis());
    assert_eq!(goals[1]["created_at"], CLOCK_START.as_millis() + 1_000);
    let tasks = board["tasks"].as_array().unwrap();
    assert_eq!(tasks.iter().map(|t| t["id"].as_str().unwrap()).collect::<Vec<_>>(), [t1.as_str(), t2.as_str(), t3.as_str()]);
    assert_eq!(tasks[0]["goal_id"], json!(second_goal));
}

#[tokio::test]
async fn tasks_in_every_status_appear_on_the_board_and_only_pending_ones_carry_missing_items() {
    let harness = Harness::start().await;
    let (ws, goal) = setup(&harness).await;
    let pending_incomplete = id_of(&new_task(&harness, &ws, &goal, "待補契約", None).await);
    let pending_complete = new_complete_task(&harness, &ws, &goal, "契約完整但還沒 ready").await;
    let ready_task = new_complete_task(&harness, &ws, &goal, "ready").await;
    make_ready(&harness, &ws, &ready_task).await;
    let cancelled_incomplete = id_of(&new_task(&harness, &ws, &goal, "不完整就取消", None).await);
    assert_eq!(cancel(&harness, &ws, &cancelled_incomplete).await.0, StatusCode::OK);

    let pending_incomplete_view = board_task(&harness, &ws, &pending_incomplete).await;
    let pending_complete_view = board_task(&harness, &ws, &pending_complete).await;
    let ready_view = board_task(&harness, &ws, &ready_task).await;
    let cancelled_view = board_task(&harness, &ws, &cancelled_incomplete).await;

    assert_eq!(pending_incomplete_view["status"], "pending");
    assert_eq!(pending_incomplete_view["missing"].as_array().unwrap().len(), 6);
    assert_eq!(pending_complete_view["status"], "pending");
    assert_eq!(pending_complete_view["missing"], json!([]), "契約完整就沒有缺項");
    assert_eq!(ready_view["status"], "ready");
    assert_eq!(ready_view["missing"], json!([]));
    assert_eq!(cancelled_view["status"], "cancelled");
    assert_eq!(cancelled_view["missing"], json!([]), "非 pending 的 Task 一律沒有 missing");
}

#[tokio::test]
async fn unfinished_dependencies_list_each_dependency_that_is_not_completed() {
    let harness = Harness::start().await;
    let (ws, goal) = setup(&harness).await;
    let a = new_complete_task(&harness, &ws, &goal, "A").await;
    let b = new_complete_task(&harness, &ws, &goal, "B").await;
    let c = new_complete_task(&harness, &ws, &goal, "C").await;
    add_dependency(&harness, &ws, &a, &b).await;
    add_dependency(&harness, &ws, &a, &c).await;
    make_ready(&harness, &ws, &b).await;
    assert_eq!(cancel(&harness, &ws, &c).await.0, StatusCode::OK);

    let view = board_task(&harness, &ws, &a).await;

    let mut dependencies: Vec<String> = view["dependencies"].as_array().unwrap().iter().map(|d| d.as_str().unwrap().to_string()).collect();
    let mut unfinished: Vec<String> =
        view["unfinished_dependencies"].as_array().unwrap().iter().map(|d| d.as_str().unwrap().to_string()).collect();
    dependencies.sort();
    unfinished.sort();
    let mut expected = vec![b, c];
    expected.sort();
    assert_eq!(dependencies, expected);
    assert_eq!(unfinished, expected, "ready 與 cancelled 都不是 completed");
    assert_eq!(board_task(&harness, &ws, &expected[0]).await["unfinished_dependencies"], json!([]));
}

#[tokio::test]
async fn the_board_content_is_unchanged_after_rebuilding_the_runtime() {
    let mut harness = Harness::builder().clock_at(CLOCK_START).start().await;
    let (ws, goal) = setup(&harness).await;
    let second_goal = new_goal(&harness, &ws, "第二個目標").await;
    let a = new_complete_task(&harness, &ws, &goal, "A").await;
    let b = id_of(&new_task(&harness, &ws, &second_goal, "B（契約不完整）", Some(contract_without(&["inputs"]))).await);
    let c = new_complete_task(&harness, &ws, &goal, "C").await;
    add_dependency(&harness, &ws, &a, &b).await;
    add_dependency(&harness, &ws, &c, &a).await;
    make_ready(&harness, &ws, &a).await;
    assert_eq!(cancel(&harness, &ws, &c).await.0, StatusCode::OK);
    let before = board(&harness, &ws).await;

    harness.restart().await;

    assert_eq!(board(&harness, &ws).await, before);
}

// ============================================================
// 8. 其他：標題驗證、範圍、封存、權限
// ============================================================

#[tokio::test]
async fn goal_titles_are_trimmed_and_limited_to_two_hundred_characters() {
    let harness = Harness::start().await;
    let ws = new_workspace(&harness, "goals").await;
    let events_before = total_events(&harness);

    let (empty_status, empty) = create_goal(&harness, &ws, "").await;
    let (blank_status, blank) = create_goal(&harness, &ws, "   ").await;
    let (long_status, long) = create_goal(&harness, &ws, &"題".repeat(201)).await;
    let (max_status, max) = create_goal(&harness, &ws, &"題".repeat(200)).await;
    let (trim_status, trimmed) = create_goal(&harness, &ws, "  前後有空白  ").await;

    for (status, body) in [(empty_status, &empty), (blank_status, &blank), (long_status, &long)] {
        assert_eq!((status, error_code(body)), (StatusCode::BAD_REQUEST, "goal_title_invalid"), "{body}");
    }
    assert_eq!(max_status, StatusCode::CREATED, "{max}");
    assert_eq!(trim_status, StatusCode::CREATED, "{trimmed}");
    assert_eq!(trimmed["title"], "前後有空白");
    assert_eq!(board(&harness, &ws).await["goals"].as_array().unwrap().len(), 2, "被拒絕的 Goal 不會出現");
    assert_eq!(total_events(&harness), events_before + 2);
}

#[tokio::test]
async fn task_titles_are_trimmed_and_limited_to_two_hundred_characters() {
    let harness = Harness::start().await;
    let (ws, goal) = setup(&harness).await;

    let (empty_status, empty) = create_task(&harness, &ws, &goal, "  ", None).await;
    let (long_status, long) = create_task(&harness, &ws, &goal, &"題".repeat(201), None).await;
    let (max_status, max) = create_task(&harness, &ws, &goal, &"題".repeat(200), None).await;
    let (trim_status, trimmed) = create_task(&harness, &ws, &goal, "  前後有空白  ", None).await;

    for (status, body) in [(empty_status, &empty), (long_status, &long)] {
        assert_eq!((status, error_code(body)), (StatusCode::BAD_REQUEST, "task_title_invalid"), "{body}");
    }
    assert_eq!(max_status, StatusCode::CREATED, "{max}");
    assert_eq!(trim_status, StatusCode::CREATED, "{trimmed}");
    assert_eq!(trimmed["title"], "前後有空白");
    assert_eq!(board(&harness, &ws).await["tasks"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn a_task_cannot_be_created_under_a_missing_goal_or_another_workspace_goal() {
    let harness = Harness::start().await;
    let (ws, _) = setup(&harness).await;
    let other_ws = new_workspace(&harness, "other").await;
    let other_goal = new_goal(&harness, &other_ws, "別的目標").await;

    let (missing_status, missing) = create_task(&harness, &ws, "goal_nope", "孤兒", None).await;
    let (foreign_status, foreign) = create_task(&harness, &ws, &other_goal, "跨 Workspace", None).await;

    assert_eq!((missing_status, error_code(&missing)), (StatusCode::NOT_FOUND, "goal_not_found"), "{missing}");
    assert_eq!((foreign_status, error_code(&foreign)), (StatusCode::NOT_FOUND, "goal_not_found"), "{foreign}");
    assert_eq!(board(&harness, &ws).await["tasks"], json!([]));
    assert_eq!(board(&harness, &other_ws).await["tasks"], json!([]));
}

#[tokio::test]
async fn an_unknown_workspace_is_not_found_for_the_board_goals_and_tasks() {
    let harness = Harness::start().await;

    let (board_status, board_body) = harness.send_json(Method::GET, &tasks_path("ws_nope"), None).await;
    let (goal_status, goal_body) = create_goal(&harness, "ws_nope", "目標").await;
    let (task_status, task_body) = create_task(&harness, "ws_nope", "goal_x", "任務", None).await;

    for (status, body) in [(board_status, &board_body), (goal_status, &goal_body), (task_status, &task_body)] {
        assert_eq!((status, error_code(body)), (StatusCode::NOT_FOUND, "workspace_not_found"), "{body}");
    }
}

#[tokio::test]
async fn tasks_and_goals_are_scoped_to_their_workspace() {
    let harness = Harness::start().await;
    let (ws, goal) = setup(&harness).await;
    let other_ws = new_workspace(&harness, "other").await;
    let tid = new_complete_task(&harness, &ws, &goal, "屬於 main").await;
    let events_before = total_events(&harness);

    let (get_status, get_body) = harness.send_json(Method::GET, &task_path(&other_ws, &tid), None).await;
    let (put_status, put_body) = put_contract(&harness, &other_ws, &tid, complete_contract(), None).await;
    let (ready_status, ready_body) = ready(&harness, &other_ws, &tid).await;
    let (cancel_status, cancel_body) = cancel(&harness, &other_ws, &tid).await;
    let (check_status, check_body) = admission_check(&harness, &other_ws, &tid).await;

    for (status, body) in [
        (get_status, &get_body),
        (put_status, &put_body),
        (ready_status, &ready_body),
        (cancel_status, &cancel_body),
        (check_status, &check_body),
    ] {
        assert_eq!((status, error_code(body)), (StatusCode::NOT_FOUND, "task_not_found"), "{body}");
    }
    let other_board = board(&harness, &other_ws).await;
    assert_eq!(other_board["goals"], json!([]));
    assert_eq!(other_board["tasks"], json!([]));
    assert_eq!(detail(&harness, &ws, &tid).await["task"]["status"], "pending");
    assert_eq!(total_events(&harness), events_before);
}

#[tokio::test]
async fn an_unknown_task_is_not_found() {
    let harness = Harness::start().await;
    let (ws, _) = setup(&harness).await;

    let (get_status, get_body) = harness.send_json(Method::GET, &task_path(&ws, "task_nope"), None).await;
    let (ready_status, ready_body) = ready(&harness, &ws, "task_nope").await;
    let (cancel_status, cancel_body) = cancel(&harness, &ws, "task_nope").await;
    let (put_status, put_body) = put_contract(&harness, &ws, "task_nope", complete_contract(), None).await;

    for (status, body) in [(get_status, &get_body), (ready_status, &ready_body), (cancel_status, &cancel_body), (put_status, &put_body)] {
        assert_eq!((status, error_code(body)), (StatusCode::NOT_FOUND, "task_not_found"), "{body}");
    }
}

#[tokio::test]
async fn an_archived_workspace_cannot_get_new_goals_or_tasks() {
    let harness = Harness::start().await;
    let (ws, goal) = setup(&harness).await;
    harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/archive"), None).await;
    let events_before = total_events(&harness);

    let (goal_status, goal_body) = create_goal(&harness, &ws, "封存後的目標").await;
    let (task_status, task_body) = create_task(&harness, &ws, &goal, "封存後的任務", None).await;

    assert_eq!((goal_status, error_code(&goal_body)), (StatusCode::CONFLICT, "workspace_archived"), "{goal_body}");
    assert_eq!((task_status, error_code(&task_body)), (StatusCode::CONFLICT, "workspace_archived"), "{task_body}");
    assert_eq!(total_events(&harness), events_before);
}

#[tokio::test]
async fn an_agent_credential_cannot_use_any_task_endpoint() {
    let harness = Harness::start().await;
    let (ws, goal) = setup(&harness).await;
    let tid = new_complete_task(&harness, &ws, &goal, "Agent 不能碰").await;
    let token = harness.issue_credential(Role::Agent, None).await;
    let events_before = total_events(&harness);
    let board_before = board(&harness, &ws).await;
    let task = task_path(&ws, &tid);
    let requests = [
        (Method::GET, tasks_path(&ws), None),
        (Method::POST, goals_path(&ws), Some(json!({ "title": "Agent 的目標" }))),
        (Method::POST, tasks_path(&ws), Some(json!({ "goal_id": goal, "title": "Agent 的任務" }))),
        (Method::GET, task.clone(), None),
        (Method::PUT, format!("{task}/contract"), Some(json!({ "contract": complete_contract() }))),
        (Method::POST, format!("{task}/dependencies"), Some(json!({ "depends_on": tid }))),
        (Method::POST, format!("{task}/admission-check"), None),
        (Method::POST, format!("{task}/ready"), None),
        (Method::POST, format!("{task}/cancel"), None),
    ];

    for (method, path, body) in requests {
        let mut request =
            harness.request(method.clone(), &path).bearer_auth(&token).header(CLIENT_HEADER, CLIENT_HEADER_VALUE);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.unwrap();
        let status = response.status();
        let body: Value = response.json().await.unwrap_or(Value::Null);

        assert_eq!((status, error_code(&body)), (StatusCode::FORBIDDEN, "role_not_allowed"), "{method} {path}");
    }
    assert_eq!(board(&harness, &ws).await, board_before);
    assert_eq!(total_events(&harness), events_before);
}

// ============================================================
// 9. 事件
// ============================================================

#[tokio::test]
async fn goal_and_task_changes_append_events_with_consecutive_versions_and_utc_millis() {
    let harness = Harness::builder().clock_at(CLOCK_START).start().await;
    let ws = new_workspace(&harness, "events").await;
    let goal = new_goal(&harness, &ws, "事件目標").await;
    let a = new_complete_task(&harness, &ws, &goal, "A").await;
    let b = new_complete_task(&harness, &ws, &goal, "B").await;
    harness.clock.advance_millis(1_000);
    put_contract(&harness, &ws, &a, contract_with("scope", filled("新範圍")), None).await;
    harness.clock.advance_millis(1_000);
    add_dependency(&harness, &ws, &a, &b).await;
    harness.clock.advance_millis(1_000);
    make_ready(&harness, &ws, &a).await;
    harness.clock.advance_millis(1_000);
    assert_eq!(cancel(&harness, &ws, &a).await.0, StatusCode::OK);

    let goal_events = aggregate_events(&harness, "goal", &goal);
    let events = task_events(&harness, &a);

    assert_eq!(goal_events.len(), 1);
    assert_eq!((goal_events[0].0.as_str(), goal_events[0].1, goal_events[0].2), ("goal_created", 1, CLOCK_START.as_millis()));
    let summary: Vec<(&str, i64, i64)> = events.iter().map(|e| (e.0.as_str(), e.1, e.2)).collect();
    assert_eq!(
        summary,
        [
            ("task_created", 1, CLOCK_START.as_millis()),
            ("task_contract_changed", 2, CLOCK_START.as_millis() + 1_000),
            ("task_dependency_added", 3, CLOCK_START.as_millis() + 2_000),
            ("task_status_changed", 4, CLOCK_START.as_millis() + 3_000),
            ("task_status_changed", 5, CLOCK_START.as_millis() + 4_000),
        ]
    );
    assert_eq!((events[3].3["from"].as_str(), events[3].3["to"].as_str()), (Some("pending"), Some("ready")));
    assert_eq!((events[4].3["from"].as_str(), events[4].3["to"].as_str()), (Some("ready"), Some("cancelled")));
    assert_eq!(detail(&harness, &ws, &a).await["task"]["version"], 5, "Task 聚合版本每個事件 +1");
}

#[tokio::test]
async fn rejected_operations_append_no_events_and_change_no_task() {
    let harness = Harness::start().await;
    let (ws, goal) = setup(&harness).await;
    let incomplete = id_of(&new_task(&harness, &ws, &goal, "不完整", None).await);
    let a = new_complete_task(&harness, &ws, &goal, "A").await;
    let b = new_complete_task(&harness, &ws, &goal, "B").await;
    let cancelled = new_complete_task(&harness, &ws, &goal, "已取消").await;
    add_dependency(&harness, &ws, &a, &b).await;
    cancel(&harness, &ws, &cancelled).await;
    let events_before = total_events(&harness);
    let board_before = board(&harness, &ws).await;

    let results = [
        ready(&harness, &ws, &incomplete).await.0,
        put_contract(&harness, &ws, &a, contract_with("objective", not_applicable("不行")), None).await.0,
        put_contract(&harness, &ws, &cancelled, complete_contract(), None).await.0,
        add_dependency(&harness, &ws, &a, &a).await.0,
        add_dependency(&harness, &ws, &b, &a).await.0,
        add_dependency(&harness, &ws, &a, "task_nope").await.0,
        ready(&harness, &ws, &cancelled).await.0,
        cancel(&harness, &ws, &cancelled).await.0,
        create_task(&harness, &ws, "goal_nope", "孤兒", None).await.0,
        create_task(&harness, &ws, &goal, "", None).await.0,
        create_goal(&harness, &ws, "").await.0,
    ];

    assert!(results.iter().all(|s| s.is_client_error()), "{results:?}");
    assert_eq!(total_events(&harness), events_before);
    assert_eq!(board(&harness, &ws).await, board_before);
}

// ============================================================
// 10. 沒有 Run、沒有 Worker
// ============================================================

#[tokio::test]
async fn a_task_becoming_ready_does_not_start_any_worker_or_run() {
    let harness = Harness::start().await;
    let (ws, goal) = setup(&harness).await;
    let tid = new_complete_task(&harness, &ws, &goal, "ready 不是執行").await;

    make_ready(&harness, &ws, &tid).await;

    assert_eq!(harness.process_control.spawn_count(), 0, "進入 ready 不會觸發任何執行");
    assert!(harness.agent_adapter.launched().is_empty());
    assert_eq!(detail(&harness, &ws, &tid).await["task"]["status"], "ready", "Task 仍停在 ready，沒有被推進");
}
