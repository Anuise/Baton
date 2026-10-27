//! 情境測試：Controller 使用訊息工具（spec D-05 共同工具、D-27，票 54），經由 Runtime harness 的真實 HTTP API 驗證。
//!
//! Controller 沒有 Task：它在訊息中的地址是 Workspace 層級的保留地址 `controller`。
//! - Controller 可以寄給自己 Workspace 內的任何 Task；Worker 以 `to: ["controller"]` 寄給自己 Workspace 的 Controller。
//! - Controller 的收件匣屬於 Workspace（不屬於某一次 Controller CLI），所以下一次開啟的 Controller CLI 接得到上一次沒讀的訊息；
//!   別的 Workspace 的訊息一律看不到、寄不到。
//! - Worker 的既有範圍不放寬：仍只讀得到寄給自己 Task 的訊息，只能回覆自己寄過或收到的訊息。
//!
//! Controller 憑證由 Controller CLI 取得（以專用的 Controller 帳號開啟）。`harness.db()` 只用來確認落地內容（訊息有沒有被儲存）。

use baton_domain::agent::{TOOL_READ_INBOX, TOOL_SEND_MESSAGE};
use baton_domain::Timestamp;
use baton_harness::Harness;
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

const WORKSPACES: &str = "/api/v1/workspaces";
const CLOCK_START: Timestamp = Timestamp::from_millis(1_700_000_000_123);

// --- 共用輔助 ---

/// 回傳 (harness, Controller 帳號的 Profile 識別)。
async fn harness() -> (Harness, String) {
    let harness = Harness::builder().clock_at(CLOCK_START).start().await;
    // 每個帳號並行上限 2；兩個 Workspace 各有 Worker，建立兩個帳號
    harness.add_account("帳號 1", "account1@example.com").await;
    harness.add_account("帳號 2", "account2@example.com").await;
    // Controller CLI 專用的帳號，不拿來派 Worker
    let controller = harness.add_account("Controller 帳號", "controller@example.com").await;
    (harness, controller)
}

fn filled(text: &str) -> Value {
    json!({ "status": "filled", "text": text })
}

fn error_code(body: &Value) -> &str {
    body["error"].as_str().unwrap_or_default()
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

/// 派工並讓 Supervisor 啟動它，回傳 (run_id, Worker 的 Agent 憑證)。
async fn start_run(harness: &Harness, ws: &str, tid: &str) -> (String, String) {
    let (status, run) = harness.send_json(Method::POST, &format!("{}/dispatch", task_path(ws, tid)), None).await;
    assert_eq!(status, StatusCode::CREATED, "{run}");
    let rid = run["id"].as_str().unwrap().to_string();
    harness.supervise().await;
    let token = harness.agent_credential(&rid).expect("Run 啟動時應該把 Agent 憑證放進程序的環境變數");
    (rid, token)
}

/// 以 Controller 帳號開啟 Workspace 的 Controller CLI，回傳 Controller 憑證。
async fn open_controller(harness: &Harness, ws: &str, profile: &str) -> String {
    harness.open_controller_cli(ws, profile).await.1
}

async fn send_ok(harness: &Harness, token: &str, body: Value) -> Value {
    let (status, response) = harness.call_tool(token, TOOL_SEND_MESSAGE, body).await;
    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(response["accepted"], true, "{response}");
    response
}

async fn read_inbox(harness: &Harness, token: &str) -> Value {
    let (status, response) = harness.call_tool(token, TOOL_READ_INBOX, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{response}");
    response
}

fn subjects(inbox: &Value) -> Vec<&str> {
    inbox["messages"].as_array().unwrap().iter().map(|m| m["subject"].as_str().unwrap()).collect()
}

fn message_count(harness: &Harness) -> i64 {
    harness.db().query_row("SELECT COUNT(*) FROM message", [], |r| r.get(0)).unwrap()
}

/// 交給 Adapter 的 Prompt。
fn prompt_of(harness: &Harness, run_id: &str) -> String {
    harness
        .agent_adapter
        .launched()
        .into_iter()
        .find(|spec| spec.run_id == run_id)
        .expect("Adapter 應該已經收到這個 Run 的 RunSpec")
        .prompt
}

// ============================================================ Controller 與 Worker 往來

#[tokio::test]
async fn the_controller_and_the_workers_of_its_workspace_exchange_messages() {
    let (harness, controller_profile) = harness().await;
    let (ws, tasks) = workspace_with_tasks(&harness, "main", &["任務甲", "任務乙"]).await;
    let (a, b) = (&tasks[0], &tasks[1]);
    let (_, worker_a) = start_run(&harness, &ws, a).await;
    let controller = open_controller(&harness, &ws, &controller_profile).await;

    // Controller 寄給 Task：收件人是 Task，寄件地址是 controller
    let sent = send_ok(
        &harness,
        &controller,
        json!({ "to": [a], "kind": "question", "subject": "比較表要幾欄？", "body": "請先列出欄位" }),
    )
    .await;
    assert_eq!(sent["deliveries"], json!([{ "recipient_task_id": a, "state": "pending" }]));
    let question_id = sent["message_id"].as_str().unwrap().to_string();

    let inbox = read_inbox(&harness, &worker_a).await;
    assert_eq!(subjects(&inbox), ["比較表要幾欄？"]);
    let message = &inbox["messages"][0];
    assert_eq!((message["from_task_id"].as_str(), message["from_task_title"].as_str()), (Some("controller"), Some("Controller")));

    // Worker 回覆 Controller：同一個討論串
    let reply = send_ok(
        &harness,
        &worker_a,
        json!({ "to": ["controller"], "kind": "answer", "subject": "五欄", "in_reply_to": question_id }),
    )
    .await;
    assert_eq!(reply["thread_id"], question_id.as_str());
    assert_eq!(reply["deliveries"], json!([{ "recipient_task_id": "controller", "state": "pending" }]));

    // Controller 的收件匣：只有寄給 Controller 的訊息，讀過就標為已讀
    let inbox = read_inbox(&harness, &controller).await;
    assert_eq!(inbox["task_id"], "controller");
    assert_eq!(subjects(&inbox), ["五欄"]);
    let message = &inbox["messages"][0];
    assert_eq!((message["from_task_id"].as_str(), message["from_task_title"].as_str()), (Some(a.as_str()), Some("任務甲")));
    assert_eq!((message["in_reply_to"].as_str(), message["thread_id"].as_str()), (Some(question_id.as_str()), Some(question_id.as_str())));
    assert_eq!(inbox["unread_remaining"], 0);
    assert_eq!(subjects(&read_inbox(&harness, &controller).await), Vec::<&str>::new());

    // 還沒派工的 Task 在 Run 啟動時的未讀摘要中看到 Controller 的訊息
    send_ok(&harness, &controller, json!({ "to": [b], "subject": "先看甲的比較表" })).await;
    let (run_b, _) = start_run(&harness, &ws, b).await;
    harness.supervise().await;
    let prompt = prompt_of(&harness, &run_b);
    assert!(prompt.contains("先看甲的比較表") && prompt.contains("來自 Controller"), "{prompt}");
}

#[tokio::test]
async fn the_controller_cannot_reach_other_workspaces_and_workers_keep_their_limits() {
    let (harness, controller_profile) = harness().await;
    let (ws, tasks) = workspace_with_tasks(&harness, "main", &["任務甲", "任務乙"]).await;
    let (other, other_tasks) = workspace_with_tasks(&harness, "other", &["別的任務"]).await;
    let (a, b, x) = (&tasks[0], &tasks[1], &other_tasks[0]);
    let (_, worker_a) = start_run(&harness, &ws, a).await;
    let (_, worker_x) = start_run(&harness, &other, x).await;
    let controller = open_controller(&harness, &ws, &controller_profile).await;

    // 別的 Workspace 的 Task 寄不到；聲稱別的 Workspace 被拒絕；寄給自己不行；都沒有儲存任何訊息
    let (status, body) = harness.call_tool(&controller, TOOL_SEND_MESSAGE, json!({ "to": [x], "subject": "越界" })).await;
    assert_eq!((status, error_code(&body)), (StatusCode::NOT_FOUND, "recipient_not_found"), "{body}");
    let (status, body) = harness
        .call_tool(&controller, TOOL_SEND_MESSAGE, json!({ "to": [a], "subject": "越界", "workspace_id": other }))
        .await;
    assert_eq!((status, error_code(&body)), (StatusCode::FORBIDDEN, "agent_scope_mismatch"), "{body}");
    let (status, body) = harness.call_tool(&controller, TOOL_READ_INBOX, json!({ "workspace_id": other })).await;
    assert_eq!((status, error_code(&body)), (StatusCode::FORBIDDEN, "agent_scope_mismatch"), "{body}");
    let (status, body) =
        harness.call_tool(&controller, TOOL_SEND_MESSAGE, json!({ "to": ["controller"], "subject": "自己" })).await;
    assert_eq!((status, error_code(&body)), (StatusCode::BAD_REQUEST, "message_invalid"), "{body}");
    assert_eq!(message_count(&harness), 0);

    // 訊息不能改變範圍：Controller 要改 Task 請用 update_task，這種訊息被忽略、不儲存
    let (status, body) = harness
        .call_tool(&controller, TOOL_SEND_MESSAGE, json!({ "to": [a], "subject": "改範圍", "contract": { "scope": "全部" } }))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!((body["accepted"].as_bool(), body["ignored"].as_bool()), (Some(false), Some(true)), "{body}");
    assert_eq!(message_count(&harness), 0);

    // 別的 Workspace 的 Worker 寄給「controller」只會到它自己 Workspace 的 Controller
    send_ok(&harness, &worker_x, json!({ "to": ["controller"], "subject": "別的工作區的回報" })).await;
    assert_eq!(subjects(&read_inbox(&harness, &controller).await), Vec::<&str>::new());

    // Worker 的既有範圍不放寬：讀不到 Controller 的收件匣，也不能回覆 Controller 與別的 Task 之間的訊息
    send_ok(&harness, &worker_a, json!({ "to": ["controller"], "subject": "甲的回報" })).await;
    let to_b = send_ok(&harness, &controller, json!({ "to": [b], "subject": "給乙的指示" })).await;
    assert_eq!(subjects(&read_inbox(&harness, &worker_a).await), Vec::<&str>::new());
    let (status, body) = harness
        .call_tool(&worker_a, TOOL_SEND_MESSAGE, json!({ "to": ["controller"], "subject": "插話", "in_reply_to": to_b["message_id"] }))
        .await;
    assert_eq!((status, error_code(&body)), (StatusCode::NOT_FOUND, "reply_target_not_found"), "{body}");
    assert_eq!(subjects(&read_inbox(&harness, &controller).await), ["甲的回報"]);
}

#[tokio::test]
async fn the_controller_inbox_belongs_to_the_workspace_and_an_ended_cli_loses_access() {
    let (harness, controller_profile) = harness().await;
    let (ws, tasks) = workspace_with_tasks(&harness, "main", &["任務甲"]).await;
    let (_, worker) = start_run(&harness, &ws, &tasks[0]).await;
    let first = open_controller(&harness, &ws, &controller_profile).await;
    send_ok(&harness, &worker, json!({ "to": ["controller"], "subject": "第一次沒讀的回報" })).await;
    harness.end_controller_cli(&ws).await;

    // 結束的 Controller CLI：憑證失效，不能再收發
    for (tool, body) in [(TOOL_READ_INBOX, json!({})), (TOOL_SEND_MESSAGE, json!({ "to": [tasks[0]], "subject": "晚了" }))] {
        let (status, response) = harness.call_tool(&first, tool, body).await;
        assert_eq!((status, error_code(&response)), (StatusCode::UNAUTHORIZED, "credential_revoked"), "{tool}：{response}");
    }

    // 下一次開啟的 Controller CLI 接得到上一次沒讀的訊息
    let second = open_controller(&harness, &ws, &controller_profile).await;
    assert_eq!(subjects(&read_inbox(&harness, &second).await), ["第一次沒讀的回報"]);
}
