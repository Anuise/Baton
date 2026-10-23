//! 情境測試：Controller 拆工與派工工具、漸進拆工（票 17），經由 Runtime harness 的真實 HTTP API 驗證。
//!
//! Controller 是腳本化的：測試開啟 Controller CLI，以 Controller 終端機收到的 Controller 憑證（與真實 MCP shim 拿到的
//! 同一個值）呼叫 Agent API，等同 Controller 在 CLI 中使用工具。
//! 只斷言外部行為：工具與桌面 API 的回應、工作面板、裁定中心、交給 ProcessControl 的 Worker spawn。
//!
//! 每個測試名稱對應票 17 的驗收條件：
//! - Controller 透過工具建立的 Task 與使用者手動建立的 Task 受相同准入檢查：
//!   `tasks_proposed_by_the_controller_pass_the_same_admission_check_as_tasks_created_by_the_user`、
//!   `the_controller_cannot_create_a_dependency_cycle_or_change_tasks_of_another_workspace`
//! - 超出 Workspace 授權的 Task 被拒絕並要求送裁定：
//!   `a_task_beyond_the_workspace_authorization_is_refused_and_sent_to_the_decision_center`
//! - 依賴未回答問題的 Task 不會被派出，獨立的研究 Task 可派出：
//!   `a_task_that_depends_on_an_unanswered_question_waits_while_independent_research_is_dispatched`
//! - Controller 不能直接寫資料庫或建立未受管理的 Worker：
//!   `only_the_controller_credential_can_use_controller_tools_and_it_cannot_act_as_a_worker`
//! - 依實際變更推導決策權限表的事項（AI-227）：`changing_the_done_criteria_of_a_dispatched_task_requires_a_decision`

use baton_domain::Timestamp;
use baton_harness::Harness;
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

async fn new_workspace(harness: &Harness, name: &str) -> String {
    let path = plain_dir(harness, name);
    let (status, body) = harness.send_json(Method::POST, WORKSPACES, Some(json!({ "path": path }))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["id"].as_str().unwrap().to_string()
}

/// 確認「研究／文件」範本（讀取、編輯、讀取公開網頁；不含部署）。
async fn confirm_research_template(harness: &Harness, ws: &str) {
    let path = format!("{WORKSPACES}/{ws}/authorization");
    let (status, view) = harness.send_json(Method::GET, &path, None).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    let grants = view["templates"].as_array().unwrap().iter().find(|t| t["id"] == "research_docs").unwrap()["grants"].clone();
    let body = json!({ "template_id": "research_docs", "grants": grants, "acknowledged_risk": false, "acknowledged_expansion": true });
    let (status, revision) = harness.send_json(Method::PUT, &path, Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{revision}");
}

/// 啟動 Harness、建立 Worker 用的帳號 1、只拿來開 Controller CLI 的 Controller 帳號，與一個已確認「研究／文件」授權的
/// Workspace。回傳 (Harness, Workspace, Controller 帳號)。
async fn start() -> (Harness, String, String) {
    let harness = Harness::builder().clock_at(CLOCK_START).start().await;
    harness.add_account("帳號 1", "account1@example.com").await;
    let controller = harness.add_account("Controller 帳號", "controller@example.com").await;
    let ws = new_workspace(&harness, "main").await;
    confirm_research_template(&harness, &ws).await;
    (harness, ws, controller)
}

fn filled(text: &str) -> Value {
    json!({ "status": "filled", "text": text })
}

fn contract(capabilities: &[&str]) -> Value {
    json!({
        "objective": filled("整理競品資料"),
        "inputs": filled("公開網站"),
        "scope": filled("只看公開資料"),
        "deliverables": filled("比較表"),
        "done_criteria": filled("至少五個產品"),
        "blocker_handling": filled("缺資料就回報"),
        "required_capabilities": capabilities,
    })
}

fn error_code(body: &Value) -> &str {
    body["error"].as_str().unwrap_or_default()
}

fn codes(missing: &Value) -> Vec<String> {
    missing.as_array().unwrap().iter().map(|m| m["code"].as_str().unwrap().to_string()).collect()
}

async fn board(harness: &Harness, ws: &str) -> Value {
    let (status, body) = harness.send_json(Method::GET, &format!("{WORKSPACES}/{ws}/tasks"), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

fn board_task(board: &Value, tid: &str) -> Value {
    board["tasks"].as_array().unwrap().iter().find(|t| t["id"] == tid).cloned().unwrap_or_else(|| panic!("看板上沒有 {tid}"))
}

/// 某個 Task 的 Run 數。
fn runs_of(board: &Value, tid: &str) -> usize {
    board["runs"].as_array().unwrap().iter().filter(|r| r["task_id"] == tid).count()
}

/// 被啟動過的 Worker 程序（Controller CLI 的宿主由終端機開出，不在其中）。
fn worker_spawns(harness: &Harness) -> Vec<String> {
    harness.process_control.spawned().iter().filter_map(|s| s.args.first().cloned()).collect()
}

async fn decisions(harness: &Harness, ws: &str) -> Value {
    let (status, body) = harness.send_json(Method::GET, &format!("{WORKSPACES}/{ws}/decisions"), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

async fn answer(harness: &Harness, ws: &str, rid: &str, option: &str, note: Option<&str>) {
    let path = format!("{WORKSPACES}/{ws}/decisions/{rid}");
    let (_, view) = harness.send_json(Method::GET, &path, None).await;
    let mut body = json!({ "option_id": option, "expected_revision": view["revision"] });
    if let Some(note) = note {
        body["action"] = json!("modify");
        body["note"] = json!(note);
    }
    let (status, answered) = harness.send_json(Method::POST, &format!("{path}/answer"), Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{answered}");
}

/// 以 Controller 憑證提出一個 Task，回傳 Task 識別。
async fn propose(harness: &Harness, token: &str, body: Value) -> String {
    let (status, response) = harness.call_tool(token, "propose_task", body).await;
    assert_eq!(status, StatusCode::CREATED, "{response}");
    response["task"]["id"].as_str().unwrap().to_string()
}

// --- 驗收條件 ---

#[tokio::test]
async fn tasks_proposed_by_the_controller_pass_the_same_admission_check_as_tasks_created_by_the_user() {
    let (harness, ws, controller) = start().await;
    let (_, token) = harness.open_controller_cli(&ws, &controller).await;

    // 相同的不完整契約：一個由 Controller 提出，一個由使用者在工作面板建立
    let mut incomplete = contract(&[]);
    incomplete["inputs"] = json!({ "status": "unknown", "note": "還不知道要看哪些網站" });
    let (status, proposed) = harness
        .call_tool(&token, "propose_task", json!({ "goal_title": "競品調查", "title": "整理競品", "contract": incomplete }))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{proposed}");
    let by_controller = proposed["task"]["id"].as_str().unwrap().to_string();
    let goal = proposed["goal_id"].as_str().unwrap().to_string();
    assert_eq!(proposed["task"]["status"], "pending", "Controller 提出的 Task 一律是 pending");
    assert_eq!(proposed["admission"]["admissible"], false);

    let body = json!({ "goal_id": goal, "title": "整理競品", "contract": incomplete });
    let (status, created) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks"), Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let by_user = created["id"].as_str().unwrap().to_string();

    // 准入檢查的缺項相同（工作面板的預覽與 Controller 工具的回應）
    let (_, user_check) =
        harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks/{by_user}/admission-check"), None).await;
    assert_eq!(proposed["admission"]["missing"], user_check["missing"]);
    assert_eq!(codes(&user_check["missing"]), vec!["field_unknown"]);
    let snapshot = board(&harness, &ws).await;
    assert_eq!(board_task(&snapshot, &by_controller)["missing"], board_task(&snapshot, &by_user)["missing"]);

    // 不完整就不能派工：Controller 的 dispatch_task 與使用者的「標記 ready」以同樣的缺項拒絕
    let (status, refused) = harness.call_tool(&token, "dispatch_task", json!({ "task_id": by_controller })).await;
    assert_eq!((status, error_code(&refused)), (StatusCode::CONFLICT, "task_admission_denied"), "{refused}");
    let (status, user_refused) =
        harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks/{by_user}/ready"), None).await;
    assert_eq!((status, error_code(&user_refused)), (StatusCode::CONFLICT, "task_admission_denied"));
    assert_eq!(refused["missing"], user_refused["missing"]);

    // 補齊之後兩邊都能派出
    let (status, updated) = harness
        .call_tool(&token, "update_task", json!({ "task_id": by_controller, "contract": contract(&[]), "note": "補上來源" }))
        .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!((updated["changed"].clone(), updated["admission"]["admissible"].clone()), (json!(true), json!(true)));
    let (status, dispatched) = harness.call_tool(&token, "dispatch_task", json!({ "task_id": by_controller })).await;
    assert_eq!(status, StatusCode::CREATED, "{dispatched}");
    assert_eq!(dispatched["status"], "started");
    assert_eq!(dispatched["run"]["task_id"], by_controller.as_str());

    // 派工建立的是受 Runtime 管理的 Run：Supervisor 啟動它，帶的是 Worker 自己的憑證
    harness.supervise().await;
    let run_id = dispatched["run"]["id"].as_str().unwrap();
    assert_eq!(worker_spawns(&harness), vec![run_id.to_string()]);
    assert_ne!(harness.agent_credential(run_id).unwrap(), token);

    // goal_id 與 goal_title 必須剛好一個
    let (status, both) = harness
        .call_tool(&token, "propose_task", json!({ "goal_id": goal, "goal_title": "另一個", "title": "x" }))
        .await;
    assert_eq!((status, error_code(&both)), (StatusCode::BAD_REQUEST, "goal_choice_invalid"));
    // 被拒絕的提案不會留下 Goal（Goal 與 Task 在同一個交易中建立）
    let (status, bad_title) =
        harness.call_tool(&token, "propose_task", json!({ "goal_title": "孤兒目標", "title": "  " })).await;
    assert_eq!((status, error_code(&bad_title)), (StatusCode::BAD_REQUEST, "task_title_invalid"));
    assert!(board(&harness, &ws).await["goals"].as_array().unwrap().iter().all(|g| g["title"] != "孤兒目標"));
}

#[tokio::test]
async fn the_controller_cannot_create_a_dependency_cycle_or_change_tasks_of_another_workspace() {
    let (harness, ws, controller) = start().await;
    let other = new_workspace(&harness, "other").await;
    let (status, goal) =
        harness.send_json(Method::POST, &format!("{WORKSPACES}/{other}/goals"), Some(json!({ "title": "別的" }))).await;
    assert_eq!(status, StatusCode::CREATED);
    let body = json!({ "goal_id": goal["id"], "title": "別人的 Task", "contract": contract(&[]) });
    let (_, foreign) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{other}/tasks"), Some(body)).await;
    let foreign = foreign["id"].as_str().unwrap().to_string();

    let (_, token) = harness.open_controller_cli(&ws, &controller).await;
    let a = propose(&harness, &token, json!({ "goal_title": "目標", "title": "A", "contract": contract(&[]) })).await;
    let goal_id = board(&harness, &ws).await["goals"][0]["id"].as_str().unwrap().to_string();
    let b = propose(&harness, &token, json!({ "goal_id": goal_id, "title": "B", "contract": contract(&[]) })).await;

    let (status, added) = harness.call_tool(&token, "add_dependency", json!({ "task_id": b, "depends_on": a })).await;
    assert_eq!(status, StatusCode::OK, "{added}");
    assert_eq!(added["task"]["dependencies"], json!([a]));
    let (status, cycle) = harness.call_tool(&token, "add_dependency", json!({ "task_id": a, "depends_on": b })).await;
    assert_eq!((status, error_code(&cycle)), (StatusCode::CONFLICT, "dependency_cycle"), "{cycle}");
    let (status, itself) = harness.call_tool(&token, "add_dependency", json!({ "task_id": a, "depends_on": a })).await;
    assert_eq!((status, error_code(&itself)), (StatusCode::BAD_REQUEST, "dependency_self"));

    // 相依未完成的 Task 排進佇列等待，不會派出
    let (status, queued) = harness.call_tool(&token, "dispatch_task", json!({ "task_id": b })).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{queued}");
    assert_eq!(queued["status"], "queued");
    assert_eq!(queued["reasons"][0]["category"], "dependency");

    // 只能碰自己 Workspace 的 Task：別的 Workspace 的 Task 找不到，聲稱別的 Workspace 直接拒絕
    let (status, missing) = harness.call_tool(&token, "cancel_task", json!({ "task_id": foreign })).await;
    assert_eq!((status, error_code(&missing)), (StatusCode::NOT_FOUND, "task_not_found"));
    let (status, mismatch) =
        harness.call_tool(&token, "cancel_task", json!({ "task_id": foreign, "workspace_id": other })).await;
    assert_eq!((status, error_code(&mismatch)), (StatusCode::FORBIDDEN, "agent_scope_mismatch"));

    let (status, cancelled) = harness.call_tool(&token, "cancel_task", json!({ "task_id": a })).await;
    assert_eq!(status, StatusCode::OK, "{cancelled}");
    assert_eq!(cancelled["task"]["status"], "cancelled");
}

#[tokio::test]
async fn a_task_beyond_the_workspace_authorization_is_refused_and_sent_to_the_decision_center() {
    let (harness, ws, controller) = start().await;
    let (controller_run, token) = harness.open_controller_cli(&ws, &controller).await;

    // 「研究／文件」授權不含部署：提出可以（pending，准入顯示缺少能力），派工被拒絕並轉成裁定請求
    let (status, proposed) = harness
        .call_tool(&token, "propose_task", json!({ "goal_title": "上線", "title": "部署網站", "contract": contract(&["deploy"]) }))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{proposed}");
    let tid = proposed["task"]["id"].as_str().unwrap().to_string();
    assert_eq!(codes(&proposed["admission"]["missing"]), vec!["capability_not_granted"]);

    let (status, refused) = harness.call_tool(&token, "dispatch_task", json!({ "task_id": tid })).await;
    assert_eq!((status, error_code(&refused)), (StatusCode::CONFLICT, "requires_decision"), "{refused}");
    assert_eq!((refused["matter"].clone(), refused["existing"].clone()), (json!("exceed_authorization"), json!(false)));
    let rid = refused["decision_request_id"].as_str().unwrap().to_string();
    assert!(refused["message"].as_str().unwrap().contains(&rid), "給模型看的訊息要帶裁定請求識別：{refused}");

    // 沒有建立 Run、沒有啟動任何 Worker，Task 仍是 pending
    harness.supervise().await;
    let snapshot = board(&harness, &ws).await;
    assert_eq!((runs_of(&snapshot, &tid), board_task(&snapshot, &tid)["status"].clone()), (0, json!("pending")));
    assert!(worker_spawns(&harness).is_empty());

    // 裁定中心看得到這個請求：由 Controller 提出、阻擋這個 Task、選項說明核准不會擴大授權
    let list = decisions(&harness, &ws).await;
    assert_eq!(list["pending_count"], 1);
    let request = &list["decisions"][0];
    assert_eq!((request["id"].as_str(), request["kind"].as_str(), request["origin"].as_str()), (Some(rid.as_str()), Some("authority"), Some("controller")));
    assert_eq!(request["run_id"], controller_run.as_str());
    assert_eq!(request["blocking"][0]["task_id"], tid.as_str());
    assert!(request["options"][0]["tradeoff"].as_str().unwrap().contains("不會擴大 Workspace 授權"));

    // 再試一次回到同一個請求，不重複建立；使用者手動標記 ready 同樣被拒絕（相同的准入檢查）
    let (_, again) = harness.call_tool(&token, "dispatch_task", json!({ "task_id": tid })).await;
    assert_eq!((again["decision_request_id"].as_str(), again["existing"].as_bool()), (Some(rid.as_str()), Some(true)));
    let (status, user_ready) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks/{tid}/ready"), None).await;
    assert_eq!((status, error_code(&user_ready)), (StatusCode::CONFLICT, "task_admission_denied"));
    assert_eq!(codes(&user_ready["missing"]), vec!["capability_not_granted"]);
    harness.end_controller_cli(&ws).await;

    // 使用者核准後再開 Controller CLI：核准不會擴大授權，授權仍不涵蓋時照樣不能派出
    answer(&harness, &ws, &rid, "approve", None).await;
    let (_, next_token) = harness.open_controller_cli(&ws, &controller).await;
    let (status, not_expanded) = harness.call_tool(&next_token, "dispatch_task", json!({ "task_id": tid })).await;
    assert_eq!((status, error_code(&not_expanded)), (StatusCode::CONFLICT, "authorization_not_expanded"), "{not_expanded}");
    assert_eq!(runs_of(&board(&harness, &ws).await, &tid), 0);
    assert!(worker_spawns(&harness).is_empty());
}

#[tokio::test]
async fn a_task_that_depends_on_an_unanswered_question_waits_while_independent_research_is_dispatched() {
    let (harness, ws, controller) = start().await;
    let (_, token) = harness.open_controller_cli(&ws, &controller).await;

    // 腳本化的 Controller：先派不依賴答案的研究，把依賴答案的實作掛在問題上
    let research = propose(
        &harness,
        &token,
        json!({ "goal_title": "記帳 App", "title": "調查記帳 App 的競品", "kind": "research", "contract": contract(&[]) }),
    )
    .await;
    let goal_id = board(&harness, &ws).await["goals"][0]["id"].as_str().unwrap().to_string();
    let mut implementation_contract = contract(&[]);
    implementation_contract["scope"] = json!({ "status": "unknown", "note": "等使用者決定平台" });
    let implementation = propose(
        &harness,
        &token,
        json!({ "goal_id": goal_id, "title": "實作第一版", "kind": "research", "contract": implementation_contract }),
    )
    .await;
    let question = json!({
        "question": "第一版要支援哪些平台？",
        "reason": "平台會改變技術選型與工作量，屬於產品方向",
        "options": [
            { "id": "web", "label": "只做網頁版", "tradeoff": "最快上線" },
            { "id": "mobile", "label": "iOS 與 Android", "tradeoff": "工作量加倍" },
        ],
        "recommendation": "web",
        "blocking": [implementation],
    });
    let (status, asked) = harness.call_tool(&token, "propose_decision_request", question).await;
    assert_eq!(status, StatusCode::CREATED, "{asked}");
    let rid = asked["decision_request_id"].as_str().unwrap().to_string();

    // 研究可以派出；依賴答案的實作即使契約補齊了也不會派出（等待使用者回答）
    let (status, started) = harness.call_tool(&token, "dispatch_task", json!({ "task_id": research })).await;
    assert_eq!((status, started["status"].as_str()), (StatusCode::CREATED, Some("started")), "{started}");
    let (status, refused) = harness.call_tool(&token, "dispatch_task", json!({ "task_id": implementation })).await;
    assert_eq!((status, error_code(&refused)), (StatusCode::CONFLICT, "task_admission_denied"), "{refused}");
    assert_eq!(codes(&refused["missing"]), vec!["field_unknown", "decision_pending"]);
    let mut decided_scope = implementation_contract.clone();
    decided_scope["scope"] = filled("平台待定，先做共用的資料層");
    let (status, _) =
        harness.call_tool(&token, "update_task", json!({ "task_id": implementation, "contract": decided_scope })).await;
    assert_eq!(status, StatusCode::OK);
    let (status, still) = harness.call_tool(&token, "dispatch_task", json!({ "task_id": implementation })).await;
    assert_eq!((status, codes(&still["missing"])), (StatusCode::CONFLICT, vec!["decision_pending".to_string()]), "{still}");
    // 使用者在工作面板手動標記 ready 也一樣被擋下
    let (status, user_ready) =
        harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks/{implementation}/ready"), None).await;
    assert_eq!((status, codes(&user_ready["missing"])), (StatusCode::CONFLICT, vec!["decision_pending".to_string()]));
    harness.end_controller_cli(&ws).await;

    harness.supervise().await;
    let snapshot = board(&harness, &ws).await;
    assert_eq!((runs_of(&snapshot, &research), runs_of(&snapshot, &implementation)), (1, 0));
    let research_run = snapshot["runs"].as_array().unwrap().iter().find(|r| r["task_id"] == research.as_str()).unwrap();
    assert_eq!(worker_spawns(&harness), vec![research_run["id"].as_str().unwrap().to_string()]);
    assert_eq!(board_task(&snapshot, &implementation)["status"], "pending");
    assert!(board_task(&snapshot, &implementation)["missing"][0]["message"].as_str().unwrap().contains("第一版要支援哪些平台？"));
    // 幾個小時過去、沒有新事件：不會自己派出
    harness.clock.advance_millis(3 * 3_600_000);
    harness.supervise().await;
    assert_eq!(runs_of(&board(&harness, &ws).await, &implementation), 0);

    // 使用者回答後再開 Controller CLI：依答案更新契約後派出實作
    let list = decisions(&harness, &ws).await;
    let request = list["decisions"].as_array().unwrap().iter().find(|d| d["id"] == rid.as_str()).unwrap();
    assert_eq!((request["origin"].as_str(), request["kind"].as_str()), (Some("controller"), Some("agent")));
    answer(&harness, &ws, &rid, "web", None).await;
    let (_, token2) = harness.open_controller_cli(&ws, &controller).await;
    let mut web = decided_scope.clone();
    web["scope"] = filled("只做網頁版");
    let (status, _) = harness.call_tool(&token2, "update_task", json!({ "task_id": implementation, "contract": web })).await;
    assert_eq!(status, StatusCode::OK);
    let (status, dispatched) = harness.call_tool(&token2, "dispatch_task", json!({ "task_id": implementation })).await;
    assert_eq!((status, dispatched["status"].as_str()), (StatusCode::CREATED, Some("started")), "{dispatched}");
    // 已結束的 Controller CLI 的憑證失效
    let (status, revoked) = harness.call_tool(&token, "cancel_task", json!({ "task_id": implementation })).await;
    assert_eq!((status, error_code(&revoked)), (StatusCode::UNAUTHORIZED, "credential_revoked"));
}

#[tokio::test]
async fn only_the_controller_credential_can_use_controller_tools_and_it_cannot_act_as_a_worker() {
    let (mut harness, ws, controller) = start().await;
    let (_, token) = harness.open_controller_cli(&ws, &controller).await;
    assert_ne!(token, "controller-credential-not-issued", "票 17 起 Controller 有真正的憑證");
    let tid = propose(&harness, &token, json!({ "goal_title": "目標", "title": "研究", "contract": contract(&[]) })).await;
    let (_, started) = harness.call_tool(&token, "dispatch_task", json!({ "task_id": tid })).await;
    let run_id = started["run"]["id"].as_str().unwrap().to_string();
    harness.supervise().await;
    let worker = harness.agent_credential(&run_id).unwrap();

    // Worker 不能使用 Controller 工具
    for tool in ["propose_task", "update_task", "add_dependency", "dispatch_task", "cancel_task", "propose_decision_request"] {
        let (status, body) = harness.call_tool(&worker, tool, json!({ "task_id": tid })).await;
        assert_eq!((status, error_code(&body)), (StatusCode::FORBIDDEN, "controller_only"), "{tool}：{body}");
    }
    // Controller 不能冒充 Worker 回報或寫入成果
    for tool in ["report_progress", "save_checkpoint", "report_blocked", "get_task_contract", "register_artifact", "request_decision"] {
        let (status, body) = harness.call_tool(&token, tool, json!({ "summary": "x", "reason": "x" })).await;
        assert_eq!((status, error_code(&body)), (StatusCode::FORBIDDEN, "worker_only"), "{tool}：{body}");
    }
    // Controller 的權限提示一律拒絕：不能直接執行指令、寫資料或自己啟動 claude
    for (tool, input) in [
        ("Bash", json!({ "command": "claude -p \"幫我寫程式\"" })),
        ("Write", json!({ "file_path": "C:\\data\\control.sqlite", "content": "x" })),
        ("Read", json!({ "file_path": "README.md" })),
    ] {
        let (status, body) = harness.call_tool(&token, "permission_prompt", json!({ "tool_name": tool, "input": input })).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!((body["behavior"].as_str(), body["status"].as_str()), (Some("deny"), Some("controller_denied")), "{tool}");
    }
    assert_eq!(decisions(&harness, &ws).await["pending_count"], 0, "Controller 的權限提示不會變成裁定請求");

    // Runtime 重啟後，開著的 Controller CLI 仍能使用自己的憑證；Controller CLI 結束後就失效
    harness.restart().await;
    let (status, _) = harness.call_tool(&token, "cancel_task", json!({ "task_id": "task_nope" })).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    harness.end_controller_cli(&ws).await;
    let (status, revoked) = harness.call_tool(&token, "propose_task", json!({ "goal_title": "g", "title": "t" })).await;
    assert_eq!((status, error_code(&revoked)), (StatusCode::UNAUTHORIZED, "credential_revoked"));
}

#[tokio::test]
async fn changing_the_done_criteria_of_a_dispatched_task_requires_a_decision() {
    let (harness, ws, controller) = start().await;
    let (_, token) = harness.open_controller_cli(&ws, &controller).await;

    // 還沒派工的 Task：補上或修改完成條件都是拆工的一部分，可以自主處理
    let mut draft = contract(&[]);
    draft["done_criteria"] = json!({ "status": "unknown" });
    let tid = propose(&harness, &token, json!({ "goal_title": "目標", "title": "研究", "contract": draft })).await;
    let (status, _) = harness.call_tool(&token, "update_task", json!({ "task_id": tid, "contract": contract(&[]) })).await;
    assert_eq!(status, StatusCode::OK);
    let (_, started) = harness.call_tool(&token, "dispatch_task", json!({ "task_id": tid })).await;
    assert_eq!(started["status"], "started");

    // 派工之後改其他欄位不受影響；改完成條件被拒絕並送裁定，契約不變
    let mut wording = contract(&[]);
    wording["objective"] = filled("整理競品資料（含價格）");
    let (status, _) = harness.call_tool(&token, "update_task", json!({ "task_id": tid, "contract": wording })).await;
    assert_eq!(status, StatusCode::OK);
    let mut looser = wording.clone();
    looser["done_criteria"] = filled("至少三個產品");
    let (status, refused) = harness.call_tool(&token, "update_task", json!({ "task_id": tid, "contract": looser })).await;
    assert_eq!((status, error_code(&refused)), (StatusCode::CONFLICT, "requires_decision"), "{refused}");
    assert_eq!(refused["matter"], "modify_done_criteria");
    let rid = refused["decision_request_id"].as_str().unwrap().to_string();
    let (_, detail) = harness.send_json(Method::GET, &format!("{WORKSPACES}/{ws}/tasks/{tid}"), None).await;
    assert_eq!(detail["task"]["contract"]["done_criteria"]["text"], "至少五個產品");
    harness.end_controller_cli(&ws).await;

    // 使用者核准這一次：再開 Controller CLI 時同樣的變更消耗它；再改一次又要裁定
    answer(&harness, &ws, &rid, "approve", None).await;
    let (_, token2) = harness.open_controller_cli(&ws, &controller).await;
    let mut approved = wording.clone();
    approved["done_criteria"] = filled("至少三個產品");
    let (status, applied) = harness.call_tool(&token2, "update_task", json!({ "task_id": tid, "contract": approved })).await;
    assert_eq!(status, StatusCode::OK, "{applied}");
    assert_eq!(applied["task"]["contract"]["done_criteria"]["text"], "至少三個產品");
    let mut again = approved.clone();
    again["done_criteria"] = filled("至少兩個產品");
    let (status, refused_again) = harness.call_tool(&token2, "update_task", json!({ "task_id": tid, "contract": again })).await;
    assert_eq!((status, error_code(&refused_again)), (StatusCode::CONFLICT, "requires_decision"));
    assert_ne!(refused_again["decision_request_id"].as_str(), Some(rid.as_str()));
}
