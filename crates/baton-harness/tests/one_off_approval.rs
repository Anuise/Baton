//! 情境測試：單次例外批准（E1，票 23），經由 Runtime harness 的真實 HTTP API 驗證。
//!
//! Worker 的權限提示與成果登記都以 Agent 憑證呼叫（與真實 MCP shim 送出的請求相同），使用者以桌面憑證裁定與修改授權；
//! 全部使用 fake Clock 與 fake Port。只斷言外部行為：權限提示的回應、裁定請求的投影、Task 與 Run 的可觀察狀態、授權 revision。
//!
//! 每個測試名稱對應票 23 的驗收條件：
//! - 相同操作在換帳號或 Run 後仍可沿用批准，但參數或成果版本改變時拒絕：`the_same_operation_keeps_its_approval_...`、
//!   `changed_parameters_or_target_...`、`a_new_artifact_version_...`
//! - 同一批准不能用於第二次操作；結果未知時先查證：`an_approval_is_used_once_...`
//! - 單次批准不增加 Workspace 授權：`a_one_off_approval_never_changes_the_workspace_authorization`
//! - 介面區分「允許本次操作」與「修改 Workspace 授權」，後者須顯示新增能力與影響並明確確認：
//!   `the_request_shows_the_exact_operation_being_approved`、`modifying_the_workspace_authorization_...`

use baton_domain::agent::TOOL_PERMISSION_PROMPT;
use baton_domain::artifact::TOOL_REGISTER_ARTIFACT;
use baton_domain::Timestamp;
use baton_harness::Harness;
use baton_ports::{AdapterEvent, Confidence, QuotaSample, QuotaSource};
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

const WORKSPACES: &str = "/api/v1/workspaces";

/// 2023-11-14 22:13:20.123 UTC
const CLOCK_START: Timestamp = Timestamp::from_millis(1_700_000_000_123);
/// 額度觀測器的輪詢間隔（spec D-31）：改了樣本之後要等到下一次輪詢，Runtime 才會看到新的值（票 27）。
const QUOTA_POLL_MILLIS: i64 = 5 * 60 * 1000;

// --- 共用輔助 ---

/// 啟動 Harness 並建立兩個可派工的帳號，回傳帳號識別（依建立順序）。
async fn harness_with_two_accounts() -> (Harness, Vec<String>) {
    let harness = Harness::builder().clock_at(CLOCK_START).start().await;
    let first = harness.add_account("帳號一", "one@example.com").await;
    let second = harness.add_account("帳號二", "two@example.com").await;
    (harness, vec![first, second])
}

fn filled(text: &str) -> Value {
    json!({ "status": "filled", "text": text })
}

fn task_path(ws: &str, tid: &str) -> String {
    format!("{WORKSPACES}/{ws}/tasks/{tid}")
}

fn authorization_path(ws: &str) -> String {
    format!("{WORKSPACES}/{ws}/authorization")
}

async fn new_workspace(harness: &Harness) -> String {
    let dir = harness.data_dir().join("projects").join("demo");
    std::fs::create_dir_all(&dir).unwrap();
    let (status, body) =
        harness.send_json(Method::POST, WORKSPACES, Some(json!({ "path": dir.to_string_lossy() }))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["id"].as_str().unwrap().to_string()
}

async fn ready_task(harness: &Harness, ws: &str, goal: &str, title: &str) -> String {
    let contract = json!({
        "objective": filled(title),
        "inputs": filled("既有的程式碼"),
        "scope": filled("這個 repo"),
        "deliverables": filled("推送後的分支"),
        "done_criteria": filled("遠端有新的 commit"),
        "blocker_handling": filled("卡住時回報"),
        "required_capabilities": [],
    });
    let body = json!({ "goal_id": goal, "title": title, "contract": contract });
    let (status, task) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks"), Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    let tid = task["id"].as_str().unwrap().to_string();
    let (status, ready) = harness.send_json(Method::POST, &format!("{}/ready", task_path(ws, &tid)), None).await;
    assert_eq!(status, StatusCode::OK, "{ready}");
    tid
}

/// 確認 Workspace 的授權：「可信專案開發」範本（不含遠端寫入）。
async fn confirm_dev_template(harness: &Harness, ws: &str) {
    let (_, view) = harness.send_json(Method::GET, &authorization_path(ws), None).await;
    let template = view["templates"].as_array().unwrap().iter().find(|t| t["id"] == "trusted_dev_p3").unwrap().clone();
    let body = json!({ "template_id": "trusted_dev_p3", "grants": template["grants"], "acknowledged_risk": true });
    let (status, revision) = harness.send_json(Method::PUT, &authorization_path(ws), Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{revision}");
}

async fn authorization(harness: &Harness, ws: &str) -> Value {
    let (status, view) = harness.send_json(Method::GET, &authorization_path(ws), None).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    view
}

async fn workspace_path(harness: &Harness, ws: &str) -> String {
    let (_, list) = harness.send_json(Method::GET, WORKSPACES, None).await;
    list["workspaces"].as_array().unwrap().iter().find(|w| w["id"] == ws).unwrap()["path"].as_str().unwrap().to_string()
}

/// 派工並讓 Supervisor 啟動它，回傳 Run（含帳號）與 Worker 的 Agent 憑證。
async fn start_run(harness: &Harness, ws: &str, tid: &str) -> (Value, String) {
    let (status, run) = harness.send_json(Method::POST, &format!("{}/dispatch", task_path(ws, tid)), None).await;
    assert_eq!(status, StatusCode::CREATED, "{run}");
    harness.supervise().await;
    let token = harness.agent_credential(run["id"].as_str().unwrap()).expect("Run 啟動時應該把 Agent 憑證交給程序");
    (run, token)
}

/// Worker 依裁定中心的約定保存 checkpoint 後結束。
async fn worker_exits(harness: &Harness, run: &Value) {
    harness.agent_adapter.emit(run["id"].as_str().unwrap(), AdapterEvent::Exited { success: true });
    harness.supervise().await;
}

/// 裁定恢復 Task 之後，Scheduler 自動派出的新 Run（最新的一個）與它的憑證。
async fn resumed_run(harness: &Harness, ws: &str, tid: &str) -> (Value, String) {
    harness.supervise().await;
    let (status, list) = harness.send_json(Method::GET, &format!("{WORKSPACES}/{ws}/runs"), None).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let run = list["runs"]
        .as_array()
        .unwrap()
        .iter()
        .rfind(|r| r["task_id"] == tid && r["status"] == "running")
        .unwrap_or_else(|| panic!("裁定後應該自動派出新的 Run：{list}"))
        .clone();
    let token = harness.agent_credential(run["id"].as_str().unwrap()).unwrap();
    (run, token)
}

async fn task_status(harness: &Harness, ws: &str, tid: &str) -> String {
    let (status, detail) = harness.send_json(Method::GET, &task_path(ws, tid), None).await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    detail["task"]["status"].as_str().unwrap().to_string()
}

async fn decision(harness: &Harness, ws: &str, rid: &str) -> Value {
    let (status, view) = harness.send_json(Method::GET, &format!("{WORKSPACES}/{ws}/decisions/{rid}"), None).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    view
}

/// 以目前看到的內容版本接受某個選項（票 35 起提交必須帶 `expected_revision`）。
async fn answer(harness: &Harness, ws: &str, rid: &str, option: &str) {
    let path = format!("{WORKSPACES}/{ws}/decisions/{rid}/answer");
    let (_, view) = harness.send_json(Method::GET, &format!("{WORKSPACES}/{ws}/decisions/{rid}"), None).await;
    let body = json!({ "option_id": option, "expected_revision": view["revision"] });
    let (status, result) = harness.send_json(Method::POST, &path, Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{result}");
}

async fn prompt(harness: &Harness, token: &str, tool: &str, input: Value) -> Value {
    let (status, body) = harness.call_tool(token, TOOL_PERMISSION_PROMPT, json!({ "tool_name": tool, "input": input })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

fn push(description: &str) -> Value {
    json!({ "command": "git push origin main", "description": description })
}

fn request_id(response: &Value) -> String {
    response["decision_request_id"].as_str().unwrap_or_else(|| panic!("回應沒有裁定請求：{response}")).to_string()
}

/// 讓帳號的推估用量吃緊（≥75% 不再派新 Run，<85% 不觸發交接），並等到下一次額度輪詢。
async fn account_near_its_limit(harness: &Harness, profile_id: &str) {
    harness.quota_observer.set_sample(
        profile_id,
        QuotaSample {
            source: QuotaSource::Estimated,
            observed_at: CLOCK_START,
            used: Some(0.8),
            reset_at: None,
            confidence: Confidence::Estimated,
        },
    );
    harness.clock.advance_millis(QUOTA_POLL_MILLIS);
}

struct Approved {
    harness: Harness,
    accounts: Vec<String>,
    ws: String,
    task: String,
    other_task: String,
    rid: String,
    first_run: Value,
}

/// Task 的第一個 Run 提出 `git push origin main`（範本不含遠端寫入），結束後使用者「允許本次操作」。
/// 另有一個不相關的 Task 在執行。回傳時 Task 已回到 ready、等 Scheduler 派出新的 Run。
async fn approved_push() -> Approved {
    let (harness, accounts) = harness_with_two_accounts().await;
    let ws = new_workspace(&harness).await;
    confirm_dev_template(&harness, &ws).await;
    let (status, goal) =
        harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/goals"), Some(json!({ "title": "發佈" }))).await;
    assert_eq!(status, StatusCode::CREATED, "{goal}");
    let goal = goal["id"].as_str().unwrap().to_string();
    let task = ready_task(&harness, &ws, &goal, "推送修正").await;
    let other_task = ready_task(&harness, &ws, &goal, "另一個推送").await;

    let (first_run, token) = start_run(&harness, &ws, &task).await;
    let waiting = prompt(&harness, &token, "Bash", push("推送到 main")).await;
    assert_eq!((waiting["behavior"].as_str(), waiting["status"].as_str()), (Some("deny"), Some("waiting_decision")));
    let rid = request_id(&waiting);
    worker_exits(&harness, &first_run).await;
    answer(&harness, &ws, &rid, "allow_once").await;
    assert_eq!(task_status(&harness, &ws, &task).await, "ready");
    Approved { harness, accounts, ws, task, other_task, rid, first_run }
}

// --- 換帳號或 Run 後沿用；參數、目標或成果版本改變則拒絕 ---

#[tokio::test]
async fn the_same_operation_keeps_its_approval_on_a_new_run_with_another_account() {
    let a = approved_push().await;
    // 第一個帳號的額度吃緊，Scheduler 把恢復的 Task 派給另一個帳號
    assert_eq!(a.first_run["profile_id"], json!(a.accounts[0]));
    account_near_its_limit(&a.harness, &a.accounts[0]).await;

    let (run, token) = resumed_run(&a.harness, &a.ws, &a.task).await;
    assert_ne!(run["id"], a.first_run["id"], "是新的 Run");
    assert_eq!(run["profile_id"], json!(a.accounts[1]), "也是另一個帳號");

    // 新的 Worker 重寫了說明文字；操作本身相同，所以沿用批准
    let approved = prompt(&a.harness, &token, "Bash", push("再推送一次到 main")).await;

    assert_eq!(approved["behavior"], "allow", "{approved}");
    assert_eq!(approved["status"], "approved");
    assert_eq!(approved["decision_request_id"], json!(a.rid));
    assert_eq!(approved["updatedInput"], push("再推送一次到 main"));
    let view = decision(&a.harness, &a.ws, &a.rid).await;
    assert_eq!(view["permission"]["approval_use"]["run_id"], run["id"], "記下是哪個 Run 用掉的");
}

#[tokio::test]
async fn changed_parameters_or_target_cannot_reuse_the_approval() {
    let a = approved_push().await;
    let (_, token) = resumed_run(&a.harness, &a.ws, &a.task).await;

    let forced = prompt(&a.harness, &token, "Bash", json!({ "command": "git push --force origin main" })).await;
    assert_eq!(forced["behavior"], "deny", "{forced}");
    assert_eq!(forced["status"], "waiting_decision");
    assert_ne!(forced["decision_request_id"], json!(a.rid), "參數不同就是另一個請求");
    assert_eq!(forced["approval_check"], json!({ "decision_request_id": a.rid, "result": "params_changed" }));
    assert!(forced["message"].as_str().unwrap().contains("不能沿用"), "{forced}");
    let view = decision(&a.harness, &a.ws, &request_id(&forced)).await;
    assert_eq!(view["permission"]["rejected_approval"], json!({ "request_id": a.rid, "reason": "params_changed" }));

    let elsewhere = prompt(&a.harness, &token, "Bash", json!({ "command": "git push origin release" })).await;
    assert_eq!(elsewhere["behavior"], "deny");
    assert_eq!(elsewhere["approval_check"]["result"], "target_changed");

    // 被拒絕的嘗試沒有用掉批准：原本的操作仍然可以執行一次
    let original = prompt(&a.harness, &token, "Bash", push("推送")).await;
    assert_eq!(original["status"], "approved", "{original}");
    assert_eq!(original["decision_request_id"], json!(a.rid));
}

#[tokio::test]
async fn a_new_artifact_version_invalidates_the_approval_when_it_is_checked_at_execution() {
    let a = approved_push().await;
    let (_, token) = resumed_run(&a.harness, &a.ws, &a.task).await;
    // 批准之後，Worker 改了要推送的成果
    let body = json!({ "name": "release-notes.md", "description": "發佈說明", "content": "第二版" });
    let (status, registered) = a.harness.call_tool(&token, TOOL_REGISTER_ARTIFACT, body).await;
    assert_eq!(status, StatusCode::CREATED, "{registered}");

    let response = prompt(&a.harness, &token, "Bash", push("推送")).await;

    assert_eq!(response["behavior"], "deny", "{response}");
    assert_eq!(response["status"], "waiting_decision");
    assert_eq!(response["approval_check"], json!({ "decision_request_id": a.rid, "result": "artifact_version_changed" }));
    let renewed = decision(&a.harness, &a.ws, &request_id(&response)).await;
    assert_eq!(renewed["permission"]["binding"]["artifact_version"], "release-notes.md@1");
    assert!(renewed["evidence"].to_string().contains("成果版本已改變"), "{renewed}");
    assert!(decision(&a.harness, &a.ws, &a.rid).await["permission"]["approval_use"].is_null(), "舊批准沒有被使用");

    // 使用者對新的成果版本再允許一次，才放行
    answer(&a.harness, &a.ws, &request_id(&response), "allow_once").await;
    let (_, token) = resumed_run(&a.harness, &a.ws, &a.task).await;
    let approved = prompt(&a.harness, &token, "Bash", push("推送")).await;
    assert_eq!(approved["status"], "approved", "{approved}");
    assert_eq!(approved["decision_request_id"], renewed["id"]);
}

// --- 用過即失效；結果未知時先查證 ---

#[tokio::test]
async fn an_approval_is_used_once_and_a_repeat_asks_to_verify_the_unknown_result_first() {
    let a = approved_push().await;
    let (run, token) = resumed_run(&a.harness, &a.ws, &a.task).await;
    assert_eq!(prompt(&a.harness, &token, "Bash", push("推送")).await["status"], "approved");

    // 同一個操作第二次：批准已經用掉，而且 Runtime 不知道上一次推送的結果
    let again = prompt(&a.harness, &token, "Bash", push("推送")).await;

    assert_eq!(again["behavior"], "deny", "{again}");
    assert_eq!(again["status"], "waiting_decision");
    assert_ne!(again["decision_request_id"], json!(a.rid));
    assert_eq!(again["approval_check"], json!({ "decision_request_id": a.rid, "result": "used_result_unknown" }));
    assert!(again["message"].as_str().unwrap().contains("先查證"), "{again}");
    let view = decision(&a.harness, &a.ws, &request_id(&again)).await;
    assert!(view["question"].as_str().unwrap().contains("結果未知"), "{view}");
    assert_eq!(view["permission"]["prior_use"]["request_id"], json!(a.rid));
    assert_eq!(view["permission"]["prior_use"]["run_id"], run["id"]);
    assert_eq!(view["recommendation"], "deny", "結果未知時建議不要重送");
    assert_eq!(view["options"][0]["label"], "已查證，允許再執行一次");
    assert_eq!(task_status(&a.harness, &a.ws, &a.task).await, "waiting_decision");
}

// --- 單次批准不增加 Workspace 授權 ---

#[tokio::test]
async fn a_one_off_approval_never_changes_the_workspace_authorization() {
    let a = approved_push().await;
    let before = authorization(&a.harness, &a.ws).await;
    let (_, token) = resumed_run(&a.harness, &a.ws, &a.task).await;
    assert_eq!(prompt(&a.harness, &token, "Bash", push("推送")).await["status"], "approved");

    let after = authorization(&a.harness, &a.ws).await;
    assert_eq!(after["revisions"], before["revisions"], "沒有產生新的授權 revision");
    assert!(
        !after["effective"]["grants"].to_string().contains("remote_write"),
        "授權仍然不含遠端寫入：{}",
        after["effective"]
    );
    // 另一個 Task 的同一個操作仍然要裁定
    let (_, other_token) = start_run(&a.harness, &a.ws, &a.other_task).await;
    let other = prompt(&a.harness, &other_token, "Bash", push("推送")).await;
    assert_eq!(other["status"], "waiting_decision", "{other}");
    assert!(other["approval_check"].is_null(), "別的 Task 的批准與它無關");
}

// --- 介面區分「允許本次操作」與「修改 Workspace 授權」 ---

#[tokio::test]
async fn the_request_shows_the_exact_operation_being_approved() {
    let (harness, _) = harness_with_two_accounts().await;
    let ws = new_workspace(&harness).await;
    confirm_dev_template(&harness, &ws).await;
    let (_, goal) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/goals"), Some(json!({ "title": "g" }))).await;
    let task = ready_task(&harness, &ws, goal["id"].as_str().unwrap(), "推送").await;
    let (_, token) = start_run(&harness, &ws, &task).await;
    let waiting = prompt(&harness, &token, "Bash", json!({ "command": "git push -u origin main", "timeout": 60000 })).await;

    let view = decision(&harness, &ws, &request_id(&waiting)).await;

    let binding = &view["permission"]["binding"];
    assert_eq!(binding["operation"], "Bash:git push");
    assert_eq!(binding["target"], "origin main");
    assert_eq!(binding["params"], json!({ "flags": ["-u"], "timeout": 60000 }));
    assert_eq!(binding["request_hash"].as_str().unwrap().len(), 64);
    assert_eq!(binding["artifact_version"], "");
    let allow = view["options"].as_array().unwrap().iter().find(|o| o["id"] == "allow_once").unwrap();
    assert_eq!(allow["label"], "允許本次操作");
    assert!(allow["tradeoff"].as_str().unwrap().contains("不會修改 Workspace 授權"), "{allow}");
}

#[tokio::test]
async fn modifying_the_workspace_authorization_shows_added_capabilities_and_impact_and_needs_explicit_confirmation() {
    let (harness, _) = harness_with_two_accounts().await;
    let ws = new_workspace(&harness).await;
    confirm_dev_template(&harness, &ws).await;
    let (_, goal) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/goals"), Some(json!({ "title": "g" }))).await;
    let task = ready_task(&harness, &ws, goal["id"].as_str().unwrap(), "推送").await;
    let (_, token) = start_run(&harness, &ws, &task).await;
    let waiting = prompt(&harness, &token, "Bash", push("推送")).await;
    let rid = request_id(&waiting);
    let capability = decision(&harness, &ws, &rid).await["permission"]["capability"].clone();
    assert_eq!(capability, "remote_write");

    // 「修改 Workspace 授權」：在目前的授權上加入這個能力，先看摘要
    let mut grants = authorization(&harness, &ws).await["effective"]["grants"].as_array().unwrap().clone();
    grants.push(json!({ "capability": capability, "targets": [workspace_path(&harness, &ws).await], "restrictions": [] }));
    let summary_path = format!("{}/summary", authorization_path(&ws));
    let (status, summary) = harness.send_json(Method::POST, &summary_path, Some(json!({ "grants": grants }))).await;
    assert_eq!(status, StatusCode::OK, "{summary}");
    let changes = &summary["changes"];
    assert_eq!(changes["base_revision"], 1);
    assert_eq!(changes["expands"], true);
    let added: Vec<&Value> = changes["changes"].as_array().unwrap().iter().filter(|c| c["expands"] == true).collect();
    assert_eq!(added.len(), 1, "{changes}");
    assert_eq!((added[0]["capability"].as_str(), added[0]["new_capability"].as_bool()), (Some("remote_write"), Some(true)));
    let impact = changes["impact"].to_string();
    assert!(impact.contains("不是單次批准") && impact.contains("遠端寫入"), "{impact}");

    // 沒有明確確認擴權就不保存
    let body = json!({ "grants": grants, "acknowledged_risk": true });
    let (status, refused) = harness.send_json(Method::PUT, &authorization_path(&ws), Some(body)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert_eq!(refused["error"], "authorization_expansion_not_acknowledged");
    assert_eq!(authorization(&harness, &ws).await["revisions"].as_array().unwrap().len(), 1);

    let body = json!({ "grants": grants, "acknowledged_risk": true, "acknowledged_expansion": true });
    let (status, revision) = harness.send_json(Method::PUT, &authorization_path(&ws), Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{revision}");
    assert_eq!(revision["revision"], 2);

    // 修改授權與回答請求是兩回事：請求仍待回答，但之後的同類操作在新授權內自動允許
    assert_eq!(decision(&harness, &ws, &rid).await["status"], "pending");
    let (_, goal2) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/goals"), Some(json!({ "title": "g2" }))).await;
    let other = ready_task(&harness, &ws, goal2["id"].as_str().unwrap(), "另一個推送").await;
    let (_, other_token) = start_run(&harness, &ws, &other).await;
    let allowed = prompt(&harness, &other_token, "Bash", push("推送")).await;
    assert_eq!(allowed["status"], "auto_allowed", "{allowed}");
    assert_eq!(allowed["authorization_revision"], 2);
}
