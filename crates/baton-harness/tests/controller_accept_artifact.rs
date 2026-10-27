//! 情境測試：Controller 專屬工具 `accept_artifact`（spec D-05，票 54），經由 Runtime harness 的真實 HTTP API 驗證。
//!
//! `accept_artifact` 仍受驗證 Gate 約束（票 13）：
//! - 契約宣告了確定性檢查的 Task 只能由 Runtime 執行檢查並全部通過才接受，Controller 不能以接受取代檢查；
//! - 沒有宣告檢查的 Task 原本只能由使用者手動接受，Controller 的接受等同略過驗證 Gate，一定要送裁定，
//!   使用者核准後才以與手動接受相同的接受交易（校驗 Run 世代、契約版本與成果 manifest）完成。
//!
//! Controller 是腳本化的：測試開啟 Controller CLI，以 Controller 終端機收到的 Controller 憑證呼叫 Agent API。

use baton_domain::artifact::TOOL_REGISTER_ARTIFACT;
use baton_domain::verification::TOOL_ACCEPT_ARTIFACT;
use baton_domain::Timestamp;
use baton_harness::Harness;
use baton_ports::AdapterEvent;
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

const WORKSPACES: &str = "/api/v1/workspaces";
const CLOCK_START: Timestamp = Timestamp::from_millis(1_700_000_000_123);
/// SHA-256("abc")，獨立於程式碼之外的已知答案。
const SHA256_ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

// --- 共用輔助 ---

/// 啟動 Harness 與兩個帳號：Worker 用的帳號 1，以及只拿來開 Controller CLI 的 Controller 帳號。回傳 (Harness, Controller 帳號)。
async fn harness() -> (Harness, String) {
    let harness = Harness::builder().clock_at(CLOCK_START).start().await;
    harness.add_account("帳號 1", "account1@example.com").await;
    let controller = harness.add_account("Controller 帳號", "controller@example.com").await;
    (harness, controller)
}

/// 建立 Workspace 與 Goal，回傳 (workspace_id, goal_id)。
async fn setup(harness: &Harness, name: &str) -> (String, String) {
    let dir = harness.data_dir().join("projects").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let (status, ws) = harness.send_json(Method::POST, WORKSPACES, Some(json!({ "path": dir.to_string_lossy() }))).await;
    assert_eq!(status, StatusCode::CREATED, "{ws}");
    let ws = ws["id"].as_str().unwrap().to_string();
    let (status, goal) =
        harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/goals"), Some(json!({ "title": "目標" }))).await;
    assert_eq!(status, StatusCode::CREATED, "{goal}");
    (ws, goal["id"].as_str().unwrap().to_string())
}

fn filled(text: &str) -> Value {
    json!({ "status": "filled", "text": text })
}

fn task_path(ws: &str, tid: &str) -> String {
    format!("{WORKSPACES}/{ws}/tasks/{tid}")
}

fn contract(checks: Value) -> Value {
    json!({
        "objective": filled("產出比較報告"),
        "inputs": filled("既有的資料"),
        "scope": filled("公開資料"),
        "deliverables": filled("report.md"),
        "done_criteria": filled("報告存在"),
        "blocker_handling": filled("卡住時回報"),
        "required_capabilities": [],
        "verification_checks": checks,
    })
}

/// 建立契約宣告了 `checks` 的 Task 並讓它 ready。
async fn ready_task(harness: &Harness, ws: &str, goal: &str, checks: Value) -> String {
    let body = json!({ "goal_id": goal, "title": "比較報告", "contract": contract(checks) });
    let (status, task) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks"), Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    let tid = task["id"].as_str().unwrap().to_string();
    let (status, ready) = harness.send_json(Method::POST, &format!("{}/ready", task_path(ws, &tid)), None).await;
    assert_eq!(status, StatusCode::OK, "{ready}");
    tid
}

/// 派工並讓 Worker 登記成果 report.md 後正常結束：Task 進入 verifying。回傳 (run_id, Worker 憑證)。
async fn run_to_verifying(harness: &Harness, ws: &str, tid: &str) -> (String, String) {
    let (status, run) = harness.send_json(Method::POST, &format!("{}/dispatch", task_path(ws, tid)), None).await;
    assert_eq!(status, StatusCode::CREATED, "{run}");
    let rid = run["id"].as_str().unwrap().to_string();
    harness.supervise().await;
    let token = harness.agent_credential(&rid).expect("Run 啟動時應該拿到 Agent 憑證");
    let (status, body) = harness.call_tool(&token, TOOL_REGISTER_ARTIFACT, json!({ "name": "report.md", "content": "abc" })).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    harness.agent_adapter.emit(&rid, AdapterEvent::Exited { success: true });
    harness.supervise().await;
    assert_eq!(task_status(harness, ws, tid).await, "verifying");
    (rid, token)
}

async fn task_status(harness: &Harness, ws: &str, tid: &str) -> String {
    let (status, detail) = harness.send_json(Method::GET, &task_path(ws, tid), None).await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    detail["task"]["status"].as_str().unwrap().to_string()
}

async fn verification(harness: &Harness, ws: &str, tid: &str) -> Value {
    let (status, view) = harness.send_json(Method::GET, &format!("{}/verification", task_path(ws, tid)), None).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    view
}

async fn decisions(harness: &Harness, ws: &str) -> Value {
    let (status, body) = harness.send_json(Method::GET, &format!("{WORKSPACES}/{ws}/decisions"), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

/// 使用者在桌面回答裁定請求。
async fn answer(harness: &Harness, ws: &str, request: &str, option: &str) {
    let path = format!("{WORKSPACES}/{ws}/decisions/{request}");
    let (_, view) = harness.send_json(Method::GET, &path, None).await;
    let body = json!({ "option_id": option, "expected_revision": view["revision"] });
    let (status, answered) = harness.send_json(Method::POST, &format!("{path}/answer"), Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{answered}");
}

async fn accept_artifact(harness: &Harness, token: &str, body: Value) -> (StatusCode, Value) {
    harness.call_tool(token, TOOL_ACCEPT_ARTIFACT, body).await
}

fn error_code(body: &Value) -> &str {
    body["error"].as_str().unwrap_or_default()
}

// ============================================================ 沒有確定性檢查：送裁定

#[tokio::test]
async fn accepting_a_task_without_checks_requires_the_users_decision_and_then_completes_it() {
    let (harness, controller) = harness().await;
    let (ws, goal) = setup(&harness, "main").await;
    let tid = ready_task(&harness, &ws, &goal, json!([])).await;
    let (rid, _) = run_to_verifying(&harness, &ws, &tid).await;
    let (controller_run, token) = harness.open_controller_cli(&ws, &controller).await;
    let request = json!({ "task_id": tid, "reason": "報告內容符合完成條件" });

    // 第一次呼叫：略過驗證 Gate 必須由使用者裁定；Task 不動
    let (status, body) = accept_artifact(&harness, &token, request.clone()).await;
    assert_eq!((status, error_code(&body)), (StatusCode::CONFLICT, "requires_decision"), "{body}");
    assert_eq!(body["matter"], "skip_stage_or_gate", "{body}");
    let decision_id = body["decision_request_id"].as_str().unwrap().to_string();
    assert_eq!(task_status(&harness, &ws, &tid).await, "verifying");
    assert_eq!(verification(&harness, &ws, &tid).await["records"], json!([]));

    // 使用者在裁定中心看到的請求：由 Controller 提出、針對這個 Task，證據列出成果與雜湊
    let list = decisions(&harness, &ws).await;
    let pending = list["decisions"].as_array().unwrap().iter().find(|d| d["id"] == decision_id.as_str()).cloned().unwrap();
    assert_eq!((pending["kind"].as_str(), pending["origin"].as_str()), (Some("authority"), Some("controller")), "{pending}");
    assert_eq!(pending["run_id"], controller_run.as_str());
    let evidence = pending["evidence"].to_string();
    assert!(evidence.contains("report.md") && evidence.contains(SHA256_ABC), "{pending}");
    assert!(pending["question"].as_str().unwrap().contains(rid.as_str()), "請求寫明接受的是哪一個 Run 的成果：{pending}");

    // 等待裁定時重送不會重複建立請求
    let (status, again) = accept_artifact(&harness, &token, request.clone()).await;
    assert_eq!((status, again["decision_request_id"].as_str(), again["existing"].as_bool()), (StatusCode::CONFLICT, Some(decision_id.as_str()), Some(true)), "{again}");

    // 使用者核准後再呼叫一次：以手動接受完成，紀錄接受者是這個裁定
    answer(&harness, &ws, &decision_id, "approve").await;
    let (status, view) = accept_artifact(&harness, &token, request.clone()).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(view["status"], "completed", "{view}");
    let record = &view["records"][0];
    assert_eq!((record["method"].as_str(), record["outcome"].as_str()), (Some("manual"), Some("passed")), "{record}");
    assert_eq!(record["accepted_by"], format!("decision:{decision_id}"));
    assert_eq!(record["note"], "報告內容符合完成條件", "沒有附註時記錄理由");
    assert_eq!((record["run_id"].as_str(), record["artifacts"][0]["sha256"].as_str()), (Some(rid.as_str()), Some(SHA256_ABC)));
    assert_eq!(task_status(&harness, &ws, &tid).await, "completed");

    // 已完成的 Task 不能再接受一次
    let (status, body) = accept_artifact(&harness, &token, request).await;
    assert_eq!((status, error_code(&body)), (StatusCode::CONFLICT, "task_not_verifying"), "{body}");
}

#[tokio::test]
async fn when_the_user_does_not_approve_the_controller_cannot_accept_the_artifact() {
    let (harness, controller) = harness().await;
    let (ws, goal) = setup(&harness, "main").await;
    let tid = ready_task(&harness, &ws, &goal, json!([])).await;
    run_to_verifying(&harness, &ws, &tid).await;
    let (_, token) = harness.open_controller_cli(&ws, &controller).await;
    let request = json!({ "task_id": tid, "reason": "看起來完成了", "note": "Controller 檢視過" });

    let (status, body) = accept_artifact(&harness, &token, request.clone()).await;
    assert_eq!((status, error_code(&body)), (StatusCode::CONFLICT, "requires_decision"), "{body}");
    answer(&harness, &ws, body["decision_request_id"].as_str().unwrap(), "reject").await;

    let (status, body) = accept_artifact(&harness, &token, request).await;

    assert_eq!((status, error_code(&body)), (StatusCode::CONFLICT, "decision_denied"), "{body}");
    assert_eq!(task_status(&harness, &ws, &tid).await, "verifying");
    assert_eq!(verification(&harness, &ws, &tid).await["records"], json!([]));
}

#[tokio::test]
async fn an_approval_only_covers_the_artifacts_the_user_saw() {
    let (harness, controller) = harness().await;
    let (ws, goal) = setup(&harness, "main").await;
    let tid = ready_task(&harness, &ws, &goal, json!([])).await;
    run_to_verifying(&harness, &ws, &tid).await;
    let (_, token) = harness.open_controller_cli(&ws, &controller).await;
    let request = json!({ "task_id": tid, "reason": "報告內容符合完成條件" });
    let (_, body) = accept_artifact(&harness, &token, request.clone()).await;
    let approved = body["decision_request_id"].as_str().unwrap().to_string();
    answer(&harness, &ws, &approved, "approve").await;

    // 核准之後、套用之前，使用者改了契約：這個 Run 的成果被判定不能接受，重新派工產生另一份成果
    let mut changed = contract(json!([]));
    changed["scope"] = filled("也看付費資料");
    let (status, body) =
        harness.send_json(Method::PUT, &format!("{}/contract", task_path(&ws, &tid)), Some(json!({ "contract": changed }))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    harness.supervise().await;
    assert_eq!(task_status(&harness, &ws, &tid).await, "ready");
    let (status, run) = harness.send_json(Method::POST, &format!("{}/dispatch", task_path(&ws, &tid)), None).await;
    assert_eq!(status, StatusCode::CREATED, "{run}");
    let rid = run["id"].as_str().unwrap().to_string();
    harness.supervise().await;
    let worker = harness.agent_credential(&rid).unwrap();
    harness.call_tool(&worker, TOOL_REGISTER_ARTIFACT, json!({ "name": "report.md", "content": "abd" })).await;
    harness.agent_adapter.emit(&rid, AdapterEvent::Exited { success: true });
    harness.supervise().await;
    assert_eq!(task_status(&harness, &ws, &tid).await, "verifying");

    // 舊的核准不能用在使用者沒看過的新成果上：要重新裁定
    let (status, body) = accept_artifact(&harness, &token, request).await;

    assert_eq!((status, error_code(&body)), (StatusCode::CONFLICT, "requires_decision"), "{body}");
    assert_ne!(body["decision_request_id"].as_str(), Some(approved.as_str()));
    assert_eq!(task_status(&harness, &ws, &tid).await, "verifying");
    let records = verification(&harness, &ws, &tid).await["records"].clone();
    assert!(records.as_array().unwrap().iter().all(|r| r["method"] != "manual"), "{records}");
}

// ============================================================ 契約宣告了確定性檢查：驗證 Gate 不能被取代

#[tokio::test]
async fn a_task_whose_contract_declares_checks_cannot_be_accepted_by_the_controller() {
    let (harness, controller) = harness().await;
    let (ws, goal) = setup(&harness, "main").await;
    let (_, token) = harness.open_controller_cli(&ws, &controller).await;
    let checks = json!([{ "kind": "command", "program": "cargo", "args": ["test"], "expected_exit_code": 0 }]);
    let tid = ready_task(&harness, &ws, &goal, checks).await;
    // 檢查命令取不到證據（無法啟動）：驗證不完整，Task 留在 verifying 等 Runtime 重驗
    harness.command_runner.script_start_failure("cargo", "找不到 cargo");
    run_to_verifying(&harness, &ws, &tid).await;
    let before = verification(&harness, &ws, &tid).await;
    assert_eq!(before["automatic"], true, "{before}");
    let pending_before = decisions(&harness, &ws).await["pending_count"].clone();

    let (status, body) = accept_artifact(&harness, &token, json!({ "task_id": tid, "reason": "我覺得可以了" })).await;

    assert_eq!((status, error_code(&body)), (StatusCode::CONFLICT, "verification_gate_pending"), "{body}");
    assert_eq!(task_status(&harness, &ws, &tid).await, "verifying");
    assert_eq!(verification(&harness, &ws, &tid).await["records"], before["records"], "沒有新增驗證紀錄");
    assert_eq!(decisions(&harness, &ws).await["pending_count"], pending_before, "不建立裁定請求");

    // 不在驗證中的 Task 沒有可接受的成果
    let idle = ready_task(&harness, &ws, &goal, json!([])).await;
    let (status, body) = accept_artifact(&harness, &token, json!({ "task_id": idle, "reason": "我覺得可以了" })).await;
    assert_eq!((status, error_code(&body)), (StatusCode::CONFLICT, "task_not_verifying"), "{body}");
    assert_eq!(task_status(&harness, &ws, &idle).await, "ready");
}

// ============================================================ 範圍與憑證

#[tokio::test]
async fn the_controller_cannot_accept_artifacts_outside_its_workspace_or_after_its_cli_ends() {
    let (harness, controller) = harness().await;
    let (ws, _) = setup(&harness, "main").await;
    let (other_ws, other_goal) = setup(&harness, "other").await;
    let foreign = ready_task(&harness, &other_ws, &other_goal, json!([])).await;
    let (_, worker_token) = run_to_verifying(&harness, &other_ws, &foreign).await;
    let (_, token) = harness.open_controller_cli(&ws, &controller).await;

    // 別的 Workspace 的 Task：找不到，也動不到
    let (status, body) = accept_artifact(&harness, &token, json!({ "task_id": foreign, "reason": "接受" })).await;
    assert_eq!((status, error_code(&body)), (StatusCode::NOT_FOUND, "task_not_found"), "{body}");
    // 聲稱別的 Workspace：拒絕，不會改用請求裡的值
    let (status, body) =
        accept_artifact(&harness, &token, json!({ "task_id": foreign, "reason": "接受", "workspace_id": other_ws })).await;
    assert_eq!((status, error_code(&body)), (StatusCode::FORBIDDEN, "agent_scope_mismatch"), "{body}");
    assert_eq!(task_status(&harness, &other_ws, &foreign).await, "verifying");
    assert_eq!(verification(&harness, &other_ws, &foreign).await["records"], json!([]));
    assert_eq!(decisions(&harness, &other_ws).await["pending_count"], 0);

    // Worker 憑證不能使用 Controller 專屬工具
    let (status, body) = accept_artifact(&harness, &worker_token, json!({ "task_id": foreign, "reason": "接受" })).await;
    assert_eq!((status, error_code(&body)), (StatusCode::FORBIDDEN, "controller_only"), "{body}");

    // Controller CLI 結束後憑證失效
    harness.end_controller_cli(&ws).await;
    let (status, body) = accept_artifact(&harness, &token, json!({ "task_id": foreign, "reason": "接受" })).await;
    assert_eq!((status, error_code(&body)), (StatusCode::UNAUTHORIZED, "credential_revoked"), "{body}");
    assert_eq!(task_status(&harness, &other_ws, &foreign).await, "verifying");
}
