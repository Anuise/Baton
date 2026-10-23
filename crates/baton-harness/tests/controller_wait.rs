//! 情境測試：Controller 事件等待端點（desktop-controller-cli 票 06），經由 Runtime harness 的真實 HTTP API 驗證。
//!
//! Controller CLI 以 fake Claude 環境 Port 開窗，Controller 憑證取自開窗時交給宿主的環境變數；Worker 由 fake Adapter 扮演。
//! 長輪詢以 fake Clock 計時：測試以 `wait_for_sleepers` 確認端點已經開始等待，再推進時間或製造事件，不使用真實 sleep
//! （`tokio::time::timeout` 只是防止測試卡死的保險）。只斷言外部行為：API 回應的事件、游標與狀態碼。
//!
//! 每個測試名稱對應票 06 的驗收條件：
//! - 啟動時已有未確認事件就立即回傳：`an_event_that_happened_before_waiting_is_returned_at_once`
//! - 否則等到事件或逾時：`without_events_the_wait_returns_when_an_event_arrives`、`without_events_the_wait_times_out_by_the_clock`
//! - 確認後同一事件不再回傳：`acknowledged_events_are_not_returned_again`
//! - 四種事件都會觸發：`each_of_the_four_kinds_of_event_ends_the_wait`
//! - Worker 憑證不能呼叫：`a_worker_credential_cannot_wait`
//! - Controller Run 結束後憑證被拒（含等待中）：`ending_the_controller_refuses_its_credential_even_while_waiting`

use std::future::Future;
use std::time::Duration;

use baton_domain::agent::{AGENT_TOKEN_ENV, TOOL_REPORT_BLOCKED, TOOL_REQUEST_DECISION, TOOL_SEND_MESSAGE};
use baton_domain::controller_cli::HOST_ENDED_PATH;
use baton_domain::controller_wait::WAIT_PATH;
use baton_domain::Timestamp;
use baton_harness::Harness;
use baton_ports::AdapterEvent;
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

const WORKSPACES: &str = "/api/v1/workspaces";
const CLOCK_START: Timestamp = Timestamp::from_millis(1_700_000_000_123);
/// 防止測試卡死的上限（不是等待條件）。
const GUARD: Duration = Duration::from_secs(20);

// --- 共用輔助 ---

async fn harness() -> (Harness, String) {
    let harness = Harness::builder().clock_at(CLOCK_START).start().await;
    let controller = harness.add_account("Controller 帳號", "controller@example.com").await;
    harness.add_account("Worker 帳號", "worker@example.com").await;
    (harness, controller)
}

async fn guarded<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(GUARD, future).await.expect("等待端點在保險時間內沒有回應")
}

fn filled(text: &str) -> Value {
    json!({ "status": "filled", "text": text })
}

fn task_path(ws: &str, tid: &str) -> String {
    format!("{WORKSPACES}/{ws}/tasks/{tid}")
}

/// 一個 Workspace、一個目標、數個 ready 的 Task，回傳 (workspace_id, 各 Task 的 id)。
async fn workspace_with_tasks(harness: &Harness, name: &str, titles: &[&str]) -> (String, Vec<String>) {
    let dir = harness.data_dir().join("projects").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let (status, ws) = harness.send_json(Method::POST, WORKSPACES, Some(json!({ "path": dir.to_string_lossy() }))).await;
    assert_eq!(status, StatusCode::CREATED, "{ws}");
    let ws = ws["id"].as_str().unwrap().to_string();
    let (status, goal) =
        harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/goals"), Some(json!({ "title": "目標" }))).await;
    assert_eq!(status, StatusCode::CREATED, "{goal}");
    let mut tasks = Vec::new();
    for title in titles {
        let contract = json!({
            "objective": filled(&format!("{title} 的目標")),
            "inputs": filled("既有的資料"),
            "scope": filled("公開資料"),
            "deliverables": filled("比較表"),
            "done_criteria": filled("五個產品"),
            "blocker_handling": filled("卡住時回報"),
            "required_capabilities": [],
        });
        let body = json!({ "goal_id": goal["id"], "title": title, "contract": contract });
        let (status, task) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks"), Some(body)).await;
        assert_eq!(status, StatusCode::CREATED, "{task}");
        let tid = task["id"].as_str().unwrap().to_string();
        let (status, ready) = harness.send_json(Method::POST, &format!("{}/ready", task_path(&ws, &tid)), None).await;
        assert_eq!(status, StatusCode::OK, "{ready}");
        tasks.push(tid);
    }
    (ws, tasks)
}

/// 開啟 Controller CLI 並讓 Supervisor 開窗，回傳 Controller 憑證。
async fn open_controller(harness: &Harness, ws: &str, profile: &str) -> String {
    let path = format!("{WORKSPACES}/{ws}/controller-cli");
    let (status, body) = harness.send_json(Method::POST, &path, Some(json!({ "profile_id": profile }))).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    harness.supervise().await;
    let (_, view) = harness.send_json(Method::GET, &path, None).await;
    assert_eq!(view["run"]["status"], "running", "{view}");
    let launch = harness.claude_env.controller_terminals().pop().expect("應該開出 Controller 終端機");
    launch.spawn.env.iter().find(|(n, _)| n == AGENT_TOKEN_ENV).map(|(_, v)| v.clone()).expect("宿主應該收到 Controller 憑證")
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

async fn worker_exits(harness: &Harness, rid: &str) {
    harness.agent_adapter.emit(rid, AdapterEvent::Exited { success: true });
    harness.supervise().await;
}

fn wait_request(harness: &Harness, token: &str, body: Value) -> reqwest::RequestBuilder {
    harness
        .request(Method::POST, WAIT_PATH)
        .bearer_auth(token)
        .header(baton_api::CLIENT_HEADER, baton_api::CLIENT_HEADER_VALUE)
        .json(&body)
}

async fn response_of(response: reqwest::Response) -> (StatusCode, Value) {
    let status = response.status();
    (status, response.json().await.unwrap_or(Value::Null))
}

async fn wait(harness: &Harness, token: &str, body: Value) -> (StatusCode, Value) {
    response_of(guarded(wait_request(harness, token, body).send()).await.unwrap()).await
}

/// 在背景送出等待，回傳之前先確認端點已經停在等待（fake Clock 上有等待者）。
async fn wait_in_background(
    harness: &Harness,
    token: &str,
    body: Value,
) -> tokio::task::JoinHandle<(StatusCode, Value)> {
    let request = wait_request(harness, token, body);
    let handle = tokio::spawn(async move { response_of(request.send().await.unwrap()).await });
    guarded(harness.clock.wait_for_sleepers(1)).await;
    handle
}

fn kinds(response: &Value) -> Vec<&str> {
    response["events"].as_array().unwrap().iter().map(|e| e["kind"].as_str().unwrap()).collect()
}

/// 等到下一批事件並確認它們，回傳事件。
async fn take_events(harness: &Harness, token: &str) -> Vec<Value> {
    let (status, response) = wait(harness, token, json!({ "timeout_seconds": 0 })).await;
    assert_eq!(status, StatusCode::OK, "{response}");
    if response["events"].as_array().unwrap().is_empty() {
        return Vec::new();
    }
    let (status, acked) = wait(harness, token, json!({ "ack": response["cursor"], "timeout_seconds": 0 })).await;
    assert_eq!(status, StatusCode::OK, "{acked}");
    response["events"].as_array().unwrap().clone()
}

// ============================================================ 立即回傳、等待、逾時

#[tokio::test]
async fn an_event_that_happened_before_waiting_is_returned_at_once() {
    let (harness, controller) = harness().await;
    let (ws, tasks) = workspace_with_tasks(&harness, "main", &["整理文件"]).await;
    let token = open_controller(&harness, &ws, &controller).await;
    let (rid, _) = start_run(&harness, &ws, &tasks[0]).await;
    worker_exits(&harness, &rid).await;

    // 不推進時間：已有未確認事件就不等
    let (status, response) = wait(&harness, &token, json!({ "timeout_seconds": 600 })).await;

    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(response["timed_out"], false);
    assert_eq!(kinds(&response), ["run_ended"]);
    let event = &response["events"][0];
    assert_eq!((event["run_id"].as_str(), event["task_id"].as_str()), (Some(rid.as_str()), Some(tasks[0].as_str())));
    assert!(event["summary"].as_str().unwrap().contains("整理文件"), "{event}");
    assert_eq!(response["cursor"], event["cursor"]);
}

#[tokio::test]
async fn without_events_the_wait_returns_when_an_event_arrives() {
    let (harness, controller) = harness().await;
    let (ws, tasks) = workspace_with_tasks(&harness, "main", &["做介面"]).await;
    let token = open_controller(&harness, &ws, &controller).await;
    let (_, worker) = start_run(&harness, &ws, &tasks[0]).await;
    assert!(take_events(&harness, &token).await.is_empty());

    let waiting = wait_in_background(&harness, &token, json!({ "timeout_seconds": 600 })).await;
    let body = json!({ "to": ["controller"], "kind": "question", "subject": "介面要用哪一版？", "body": "請決定" });
    let (status, sent) = harness.call_tool(&worker, TOOL_SEND_MESSAGE, body).await;
    assert_eq!(status, StatusCode::OK, "{sent}");

    let (status, response) = guarded(waiting).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(kinds(&response), ["message_received"]);
    let event = &response["events"][0];
    assert_eq!(event["task_id"], tasks[0].as_str());
    assert!(event["summary"].as_str().unwrap().contains("介面要用哪一版？"), "{event}");
}

#[tokio::test]
async fn without_events_the_wait_times_out_by_the_clock() {
    let (harness, controller) = harness().await;
    let (ws, _) = workspace_with_tasks(&harness, "main", &["整理文件"]).await;
    let token = open_controller(&harness, &ws, &controller).await;

    let waiting = wait_in_background(&harness, &token, json!({ "timeout_seconds": 30 })).await;
    harness.clock.advance_millis(29_999);
    // 一個往返讓端點有機會處理推進的時間；還沒到逾時
    let _ = harness.send_json(Method::GET, WORKSPACES, None).await;
    assert!(!waiting.is_finished(), "還沒到逾時就回應了");
    harness.clock.advance_millis(1);

    let (status, response) = guarded(waiting).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(response["timed_out"], true);
    assert_eq!(response["events"], json!([]));
}

#[tokio::test]
async fn acknowledged_events_are_not_returned_again() {
    let (harness, controller) = harness().await;
    let (ws, tasks) = workspace_with_tasks(&harness, "main", &["一", "二"]).await;
    let token = open_controller(&harness, &ws, &controller).await;
    let (first, _) = start_run(&harness, &ws, &tasks[0]).await;
    worker_exits(&harness, &first).await;

    let (_, response) = wait(&harness, &token, json!({ "timeout_seconds": 0 })).await;
    assert_eq!(kinds(&response), ["run_ended"]);
    // 還沒確認：再等一次拿到同一筆
    let (_, again) = wait(&harness, &token, json!({ "timeout_seconds": 0 })).await;
    assert_eq!(again["events"], response["events"]);

    // 確認之後不再回傳；只拿到之後的新事件
    let (status, acked) = wait(&harness, &token, json!({ "ack": response["cursor"], "timeout_seconds": 0 })).await;
    assert_eq!(status, StatusCode::OK, "{acked}");
    assert_eq!((acked["timed_out"].as_bool(), acked["events"].clone()), (Some(true), json!([])));
    let (second, _) = start_run(&harness, &ws, &tasks[1]).await;
    worker_exits(&harness, &second).await;
    let (_, next) = wait(&harness, &token, json!({ "timeout_seconds": 0 })).await;
    assert_eq!(kinds(&next), ["run_ended"]);
    assert_eq!(next["events"][0]["run_id"], second.as_str());

    // 確認不會倒退，也不能超過已存在的事件
    let (status, _) = wait(&harness, &token, json!({ "ack": 0, "timeout_seconds": 0 })).await;
    assert_eq!(status, StatusCode::OK);
    let (_, still) = wait(&harness, &token, json!({ "timeout_seconds": 0 })).await;
    assert_eq!(still["events"], next["events"]);
    let beyond = next["cursor"].as_i64().unwrap() + 1_000_000;
    let (status, refused) = wait(&harness, &token, json!({ "ack": beyond, "timeout_seconds": 0 })).await;
    assert_eq!((status, refused["error"].as_str()), (StatusCode::BAD_REQUEST, Some("wait_cursor_invalid")), "{refused}");
}

#[tokio::test]
async fn each_of_the_four_kinds_of_event_ends_the_wait() {
    let (harness, controller) = harness().await;
    let (ws, tasks) = workspace_with_tasks(&harness, "main", &["會成功", "會受阻", "要裁定"]).await;
    let token = open_controller(&harness, &ws, &controller).await;

    // Run 結束
    let (done, _) = start_run(&harness, &ws, &tasks[0]).await;
    worker_exits(&harness, &done).await;
    assert_eq!(take_events(&harness, &token).await.iter().map(|e| e["kind"].clone()).collect::<Vec<_>>(), [json!("run_ended")]);

    // Task 受阻（Worker 回報受阻後結束：Run 結束、Task 受阻）
    let (blocked, worker_blocked) = start_run(&harness, &ws, &tasks[1]).await;
    let (status, ack) = harness.call_tool(&worker_blocked, TOOL_REPORT_BLOCKED, json!({ "reason": "缺少範本" })).await;
    assert!(status.is_success(), "{ack}");
    worker_exits(&harness, &blocked).await;
    let events = take_events(&harness, &token).await;
    let blocked_event = events.iter().find(|e| e["kind"] == "task_blocked").unwrap_or_else(|| panic!("{events:?}"));
    assert_eq!(blocked_event["task_id"], tasks[1].as_str());

    // 裁定被回答
    let (asking, worker_asking) = start_run(&harness, &ws, &tasks[2]).await;
    let request = json!({
        "question": "要先出貨還是等報表？",
        "reason": "取決於使用者價值",
        "options": [
            { "id": "ship", "label": "先出貨", "tradeoff": "快，但缺少報表" },
            { "id": "wait", "label": "等報表完成", "tradeoff": "晚一週" },
        ],
    });
    let (status, requested) = harness.call_tool(&worker_asking, TOOL_REQUEST_DECISION, request).await;
    assert_eq!(status, StatusCode::OK, "{requested}");
    let decision_id = requested["decision_request_id"].as_str().unwrap().to_string();
    worker_exits(&harness, &asking).await;
    let _ = take_events(&harness, &token).await;
    let waiting = wait_in_background(&harness, &token, json!({ "timeout_seconds": 600 })).await;
    let answer = json!({ "option_id": "wait", "expected_revision": 1 });
    let (status, answered) =
        harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/decisions/{decision_id}/answer"), Some(answer)).await;
    assert_eq!(status, StatusCode::OK, "{answered}");
    let (_, response) = guarded(waiting).await.unwrap();
    assert_eq!(kinds(&response), ["decision_answered"]);
    assert_eq!(response["events"][0]["decision_request_id"], decision_id.as_str());
    assert!(response["events"][0]["summary"].as_str().unwrap().contains("要先出貨還是等報表？"), "{response}");
    let _ = take_events(&harness, &token).await;

    // 收到訊息（寄件的 Run 已結束時寄不出去，所以用另一個還在執行的 Worker）
    let sender_task = workspace_task(&harness, &ws, "寄訊息").await;
    let (_, worker_sender) = start_run(&harness, &ws, &sender_task).await;
    let body = json!({ "to": ["controller"], "kind": "note", "subject": "進度", "body": "完成一半" });
    let (status, sent) = harness.call_tool(&worker_sender, TOOL_SEND_MESSAGE, body).await;
    assert_eq!(status, StatusCode::OK, "{sent}");
    let events = take_events(&harness, &token).await;
    assert_eq!(events.iter().map(|e| e["kind"].clone()).collect::<Vec<_>>(), [json!("message_received")]);
}

/// 在既有 Workspace 的第一個 Goal 下再建一個 ready 的 Task。
async fn workspace_task(harness: &Harness, ws: &str, title: &str) -> String {
    let (_, board) = harness.send_json(Method::GET, &format!("{WORKSPACES}/{ws}/tasks"), None).await;
    let goal = board["tasks"][0]["goal_id"].clone();
    let contract = json!({
        "objective": filled(title),
        "inputs": filled("既有的資料"),
        "scope": filled("公開資料"),
        "deliverables": filled("比較表"),
        "done_criteria": filled("五個產品"),
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

#[tokio::test]
async fn events_of_other_workspaces_are_not_reported() {
    let (harness, controller) = harness().await;
    let (ws, _) = workspace_with_tasks(&harness, "main", &["自己的"]).await;
    let (other, other_tasks) = workspace_with_tasks(&harness, "other", &["別人的"]).await;
    let token = open_controller(&harness, &ws, &controller).await;
    let (rid, _) = start_run(&harness, &other, &other_tasks[0]).await;
    worker_exits(&harness, &rid).await;

    assert!(take_events(&harness, &token).await.is_empty());
}

// ============================================================ 憑證

#[tokio::test]
async fn a_worker_credential_cannot_wait() {
    let (harness, controller) = harness().await;
    let (ws, tasks) = workspace_with_tasks(&harness, "main", &["整理文件"]).await;
    let _ = open_controller(&harness, &ws, &controller).await;
    let (_, worker) = start_run(&harness, &ws, &tasks[0]).await;

    let (status, refused) = wait(&harness, &worker, json!({ "timeout_seconds": 0 })).await;
    assert_eq!((status, refused["error"].as_str()), (StatusCode::FORBIDDEN, Some("controller_only")), "{refused}");
    // 內容不合法也一樣
    let (status, _) = wait(&harness, &worker, json!({ "timeout_seconds": "x" })).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn ending_the_controller_refuses_its_credential_even_while_waiting() {
    let (harness, controller) = harness().await;
    let (ws, _) = workspace_with_tasks(&harness, "main", &["整理文件"]).await;
    let token = open_controller(&harness, &ws, &controller).await;

    let waiting = wait_in_background(&harness, &token, json!({ "timeout_seconds": 600 })).await;
    let response = harness
        .request(Method::POST, HOST_ENDED_PATH)
        .bearer_auth(&token)
        .header(baton_api::CLIENT_HEADER, baton_api::CLIENT_HEADER_VALUE)
        .json(&json!({ "exit_code": 0 }))
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success(), "{:?}", response.status());

    let (status, refused) = guarded(waiting).await.unwrap();
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{refused}");
    let (status, _) = wait(&harness, &token, json!({ "timeout_seconds": 0 })).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn the_timeout_has_an_upper_bound() {
    let (harness, controller) = harness().await;
    let (ws, _) = workspace_with_tasks(&harness, "main", &["整理文件"]).await;
    let token = open_controller(&harness, &ws, &controller).await;

    let (status, refused) = wait(&harness, &token, json!({ "timeout_seconds": 3601 })).await;
    assert_eq!((status, refused["error"].as_str()), (StatusCode::BAD_REQUEST, Some("wait_timeout_invalid")), "{refused}");
}

// ============================================================ 換手

#[tokio::test]
async fn a_reopened_controller_continues_from_where_the_previous_one_confirmed() {
    let (harness, controller) = harness().await;
    let (ws, tasks) = workspace_with_tasks(&harness, "main", &["一", "二"]).await;
    let first_token = open_controller(&harness, &ws, &controller).await;
    let (first, _) = start_run(&harness, &ws, &tasks[0]).await;
    worker_exits(&harness, &first).await;
    assert_eq!(take_events(&harness, &first_token).await.len(), 1);
    let (status, ended) =
        harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/controller-cli/end"), None).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{ended}");
    harness.supervise().await;

    // Controller 關著時發生的事，重開後立即收到；上一個 Controller 已確認的不重複
    let (second, _) = start_run(&harness, &ws, &tasks[1]).await;
    worker_exits(&harness, &second).await;
    let token = open_controller(&harness, &ws, &controller).await;
    let events = take_events(&harness, &token).await;
    assert_eq!(events.iter().map(|e| e["run_id"].clone()).collect::<Vec<_>>(), [json!(second)]);
}
