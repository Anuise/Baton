//! 情境測試：Worker 間訊息（票 18、spec D-27），經由 Runtime harness 的真實 HTTP API 驗證。
//!
//! 寄件與收件都用 Worker 的 Agent 憑證（取自 fake ProcessControl 記錄的啟動環境變數），
//! 與真實 Worker 的 MCP shim 拿到的是同一個值；全部使用 fake Clock 與 fake Port。
//! 只斷言外部行為：HTTP 回應、Run／Task 的可觀察狀態、Port 被呼叫的方式（Worker 的 Prompt）；
//! `harness.db()` 只用來確認落地內容：交付狀態、訊息有沒有被儲存、給 Controller 的事件。

use baton_domain::agent::{TOOL_GET_TASK_CONTRACT, TOOL_READ_INBOX, TOOL_SEND_MESSAGE};
use baton_domain::Timestamp;
use baton_harness::Harness;
use baton_mcp_shim::{serve, ShimConfig};
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};
use tokio::io::{duplex, AsyncBufReadExt, AsyncWriteExt, BufReader};

const WORKSPACES: &str = "/api/v1/workspaces";

/// 2023-11-14 22:13:20.123 UTC
const CLOCK_START: Timestamp = Timestamp::from_millis(1_700_000_000_123);

// --- 共用輔助 ---

async fn harness() -> Harness {
    let harness = Harness::builder().clock_at(CLOCK_START).start().await;
    // 派工需要可派工的帳號（票 19）；每個帳號並行上限 2，部分情境同時有 3 個 Run，所以建立兩個
    harness.add_account("主要帳號", "main@example.com").await;
    harness.add_account("第二帳號", "second@example.com").await;
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

fn run_path(ws: &str, rid: &str) -> String {
    format!("{WORKSPACES}/{ws}/runs/{rid}")
}

/// 一個 Workspace、一個目標、數個 ready 的 Task（標題依序給定），回傳 (workspace_id, 各 Task 的 id)。
async fn workspace_with_tasks(harness: &Harness, name: &str, titles: &[&str]) -> (String, Vec<String>) {
    let ws = new_workspace(harness, name).await;
    let (status, goal) =
        harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/goals"), Some(json!({ "title": "目標" }))).await;
    assert_eq!(status, StatusCode::CREATED, "{goal}");
    let mut tasks = Vec::new();
    for title in titles {
        let body = json!({ "goal_id": goal["id"], "title": title, "contract": contract(&format!("{title} 的目標")) });
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

/// 取消 Run 並讓 Supervisor 確認停止；Task 回到 ready，可以再派工。
async fn cancel_run(harness: &Harness, ws: &str, rid: &str) {
    let (status, body) = harness.send_json(Method::POST, &format!("{}/cancel", run_path(ws, rid)), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    harness.supervise().await;
    let (status, run) = harness.send_json(Method::GET, &run_path(ws, rid), None).await;
    assert_eq!(status, StatusCode::OK, "{run}");
    assert_eq!(run["status"], "cancelled");
}

async fn task_detail(harness: &Harness, ws: &str, tid: &str) -> Value {
    let (status, detail) = harness.send_json(Method::GET, &task_path(ws, tid), None).await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    detail
}

fn error_code(body: &Value) -> &str {
    body["error"].as_str().unwrap_or_default()
}

/// 交給 Adapter 的 Prompt：Run 啟動後 Adapter 才收到 RunSpec，所以先讓 Supervisor 動作一次。
async fn prompt_of(harness: &Harness, run_id: &str) -> String {
    harness.supervise().await;
    harness
        .agent_adapter
        .launched()
        .into_iter()
        .find(|spec| spec.run_id == run_id)
        .expect("Adapter 應該已經收到這個 Run 的 RunSpec")
        .prompt
}

/// 送出訊息並要求被接受（不論是新訊息或重複），回傳回應。
async fn send_ok(harness: &Harness, token: &str, body: Value) -> Value {
    let (status, response) = harness.call_tool(token, TOOL_SEND_MESSAGE, body).await;
    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(response["accepted"], true, "{response}");
    response
}

async fn read_inbox(harness: &Harness, token: &str, body: Value) -> Value {
    let (status, response) = harness.call_tool(token, TOOL_READ_INBOX, body).await;
    assert_eq!(status, StatusCode::OK, "{response}");
    response
}

fn message_ids(inbox: &Value) -> Vec<&str> {
    inbox["messages"].as_array().unwrap().iter().map(|m| m["message_id"].as_str().unwrap()).collect()
}

/// 訊息表中的訊息數。
fn message_count(harness: &Harness) -> i64 {
    harness.db().query_row("SELECT COUNT(*) FROM message", [], |r| r.get(0)).unwrap()
}

fn delivery_count(harness: &Harness) -> i64 {
    harness.db().query_row("SELECT COUNT(*) FROM message_delivery", [], |r| r.get(0)).unwrap()
}

/// 某則訊息的各筆交付：(收件 Task, delivered_at, read_at)，依收件 Task 排序。
fn deliveries_of(harness: &Harness, message_id: &str) -> Vec<(String, Option<i64>, Option<i64>)> {
    let db = harness.db();
    let mut statement = db
        .prepare(
            "SELECT recipient_task_id, delivered_at, read_at FROM message_delivery \
             WHERE message_id = ?1 ORDER BY recipient_task_id",
        )
        .unwrap();
    statement
        .query_map([message_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// 給 Controller 的「嘗試用訊息改範圍」事件的 payload，依發生順序。
fn scope_change_events(harness: &Harness) -> Vec<Value> {
    let db = harness.db();
    let mut statement = db
        .prepare(
            "SELECT payload FROM event WHERE aggregate_type = 'message' \
             AND event_type = 'message_scope_change_ignored' ORDER BY seq",
        )
        .unwrap();
    statement
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|payload| serde_json::from_str(&payload.unwrap()).expect("事件 payload 是 JSON"))
        .collect()
}

// ============================================================ 寄送與收取

#[tokio::test]
async fn a_message_to_another_task_is_read_by_that_task_exactly_once() {
    let harness = harness().await;
    let (ws, tasks) = workspace_with_tasks(&harness, "main", &["任務甲", "任務乙"]).await;
    let (a, b) = (&tasks[0], &tasks[1]);
    let (_, token_a) = start_run(&harness, &ws, a).await;
    let (_, token_b) = start_run(&harness, &ws, b).await;

    harness.clock.advance_millis(5_000);
    let sent = send_ok(
        &harness,
        &token_a,
        json!({ "to": [b], "kind": "question", "subject": "輸出格式？", "body": "請問結果用 JSON 還是 CSV？" }),
    )
    .await;

    let message_id = sent["message_id"].as_str().unwrap().to_string();
    assert!(message_id.starts_with("msg_"), "沒給 message_id 時由 Runtime 產生：{sent}");
    assert_eq!(sent["ignored"], false);
    assert_eq!(sent["duplicate"], false);
    assert_eq!(sent["thread_id"], message_id.as_str(), "新對話串的 thread_id 等於 message_id");
    assert_eq!(sent["correlation_id"], message_id.as_str());
    assert_eq!(sent["deliveries"], json!([{ "recipient_task_id": b, "state": "pending" }]));
    assert!(sent["reason"].is_null());
    assert_eq!(deliveries_of(&harness, &message_id), [(b.clone(), None, None)], "還沒有人看到：pending");

    // 收件人讀取：欄位與內容正確
    let inbox = read_inbox(&harness, &token_b, json!({})).await;
    assert_eq!(inbox["task_id"], b.as_str());
    assert_eq!(inbox["unread_remaining"], 0);
    let messages = inbox["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1, "{inbox}");
    let message = &messages[0];
    assert_eq!(message["message_id"], message_id.as_str());
    assert_eq!(message["thread_id"], message_id.as_str());
    assert_eq!(message["correlation_id"], message_id.as_str());
    assert!(message["in_reply_to"].is_null());
    assert_eq!(message["kind"], "question");
    assert_eq!(message["subject"], "輸出格式？");
    assert_eq!(message["body"], "請問結果用 JSON 還是 CSV？");
    assert_eq!(message["from_task_id"], a.as_str());
    assert_eq!(message["from_task_title"], "任務甲");
    assert_eq!(message["sent_at"], CLOCK_START.as_millis() + 5_000);
    assert_eq!(message["already_read"], false);

    // 取走之後：再讀是空的，include_read 才看得到，而且標為已讀
    harness.clock.advance_millis(1_000);
    let again = read_inbox(&harness, &token_b, json!({})).await;
    assert_eq!((again["messages"].as_array().unwrap().len(), &again["unread_remaining"]), (0, &json!(0)), "{again}");
    let with_read = read_inbox(&harness, &token_b, json!({ "include_read": true })).await;
    assert_eq!(message_ids(&with_read), [message_id.as_str()]);
    assert_eq!(with_read["messages"][0]["already_read"], true);

    let rows = deliveries_of(&harness, &message_id);
    assert_eq!(rows.len(), 1);
    assert!(rows[0].2.is_some(), "read_inbox 取走後 read_at 有值：{rows:?}");

    // 自己寄出的訊息不會出現在自己的收件匣
    let own = read_inbox(&harness, &token_a, json!({ "include_read": true })).await;
    assert!(own["messages"].as_array().unwrap().is_empty(), "{own}");
    assert_eq!(own["task_id"], a.as_str());
}

#[tokio::test]
async fn read_inbox_returns_messages_oldest_first_and_honors_the_limit() {
    let harness = harness().await;
    let (ws, tasks) = workspace_with_tasks(&harness, "main", &["任務甲", "任務乙"]).await;
    let (_, token_a) = start_run(&harness, &ws, &tasks[0]).await;
    let (_, token_b) = start_run(&harness, &ws, &tasks[1]).await;
    for n in 1..=5 {
        harness.clock.advance_millis(10);
        send_ok(&harness, &token_a, json!({ "to": [tasks[1]], "subject": format!("第 {n} 則"), "message_id": format!("seq-{n}") })).await;
    }

    let first = read_inbox(&harness, &token_b, json!({ "limit": 2 })).await;
    assert_eq!(message_ids(&first), ["seq-1", "seq-2"], "依送出順序，舊到新");
    assert_eq!(first["unread_remaining"], 3);
    // 只有回傳的那些被標為已讀
    assert!(deliveries_of(&harness, "seq-1")[0].2.is_some() && deliveries_of(&harness, "seq-2")[0].2.is_some());
    assert!(deliveries_of(&harness, "seq-3")[0].2.is_none());

    let rest = read_inbox(&harness, &token_b, json!({})).await;
    assert_eq!(message_ids(&rest), ["seq-3", "seq-4", "seq-5"]);
    assert_eq!(rest["unread_remaining"], 0);
    assert!(rest["messages"].as_array().unwrap().iter().all(|m| m["already_read"] == false));

    let everything = read_inbox(&harness, &token_b, json!({ "include_read": true })).await;
    assert_eq!(message_ids(&everything), ["seq-1", "seq-2", "seq-3", "seq-4", "seq-5"]);
    assert!(everything["messages"].as_array().unwrap().iter().all(|m| m["already_read"] == true));
}

// ============================================================ 去重

#[tokio::test]
async fn resending_the_same_message_is_delivered_only_once() {
    let harness = harness().await;
    let (ws, tasks) = workspace_with_tasks(&harness, "main", &["任務甲", "任務乙", "任務丙"]).await;
    let (a, b, c) = (&tasks[0], &tasks[1], &tasks[2]);
    let (_, token_a) = start_run(&harness, &ws, a).await;
    let (_, token_b) = start_run(&harness, &ws, b).await;
    let (_, token_c) = start_run(&harness, &ws, c).await;

    // 多收件人：每個收件人各有一份
    let body = json!({ "to": [b, c], "kind": "result", "subject": "調查完成", "body": "結論在附件", "message_id": "result:1" });
    let first = send_ok(&harness, &token_a, body.clone()).await;
    assert_eq!(first["duplicate"], false);
    assert_eq!(first["message_id"], "result:1");
    assert_eq!(first["deliveries"].as_array().unwrap().len(), 2, "{first}");
    assert_eq!(deliveries_of(&harness, "result:1").len(), 2);

    // 完全相同的內容重送（收件人順序不同也一樣）：重複，不新增任何東西
    let before_messages = message_count(&harness);
    let before_deliveries = delivery_count(&harness);
    let mut reordered = body.clone();
    reordered["to"] = json!([c, b]);
    for retry in [body.clone(), reordered] {
        let again = send_ok(&harness, &token_a, retry).await;
        assert_eq!((again["duplicate"].as_bool(), again["message_id"].as_str()), (Some(true), Some("result:1")), "{again}");
        assert_eq!(again["ignored"], false);
    }
    assert_eq!((message_count(&harness), delivery_count(&harness)), (before_messages, before_deliveries));
    for token in [&token_b, &token_c] {
        let inbox = read_inbox(&harness, token, json!({})).await;
        assert_eq!(message_ids(&inbox), ["result:1"], "每個收件人只有一份：{inbox}");
    }

    // 同一個 message_id 但內容不同（主旨、本文、類型、收件人、回覆對象）：衝突
    let variants = [
        json!({ "to": [b, c], "kind": "result", "subject": "調查完成（改）", "body": "結論在附件", "message_id": "result:1" }),
        json!({ "to": [b, c], "kind": "result", "subject": "調查完成", "body": "結論改了", "message_id": "result:1" }),
        json!({ "to": [b, c], "kind": "note", "subject": "調查完成", "body": "結論在附件", "message_id": "result:1" }),
        json!({ "to": [b], "kind": "result", "subject": "調查完成", "body": "結論在附件", "message_id": "result:1" }),
    ];
    for variant in variants {
        let (status, response) = harness.call_tool(&token_a, TOOL_SEND_MESSAGE, variant.clone()).await;
        assert_eq!((status, error_code(&response)), (StatusCode::CONFLICT, "message_id_conflict"), "{variant}：{response}");
    }

    // 另一個 Task 用同一個 id（即使內容一樣）：衝突，而且原訊息不被覆蓋
    let (status, response) = harness.call_tool(&token_b, TOOL_SEND_MESSAGE, json!({ "to": [a], "kind": "result", "subject": "調查完成", "body": "結論在附件", "message_id": "result:1" })).await;
    assert_eq!((status, error_code(&response)), (StatusCode::CONFLICT, "message_id_conflict"), "{response}");

    assert_eq!((message_count(&harness), delivery_count(&harness)), (before_messages, before_deliveries), "被拒絕的送出什麼都不儲存");
    let a_inbox = read_inbox(&harness, &token_a, json!({ "include_read": true })).await;
    assert!(a_inbox["messages"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn a_resend_from_a_newer_generation_of_the_sender_is_still_a_duplicate() {
    let harness = harness().await;
    let (ws, tasks) = workspace_with_tasks(&harness, "main", &["任務甲", "任務乙"]).await;
    let (a, b) = (&tasks[0], &tasks[1]);
    let (run_a1, token_a1) = start_run(&harness, &ws, a).await;
    let (_, token_b) = start_run(&harness, &ws, b).await;
    let body = json!({ "to": [b], "subject": "我的中間成果", "body": "放在 notes.md", "message_id": "handoff-note" });
    send_ok(&harness, &token_a1, body.clone()).await;

    // A 的 Run 換了世代，新的 Run 不知道前一個送過沒有，重送一次
    cancel_run(&harness, &ws, &run_a1).await;
    let (_, token_a2) = start_run(&harness, &ws, a).await;
    let again = send_ok(&harness, &token_a2, body).await;

    assert_eq!((again["duplicate"].as_bool(), again["message_id"].as_str()), (Some(true), Some("handoff-note")), "{again}");
    assert_eq!(message_count(&harness), 1);
    assert_eq!(delivery_count(&harness), 1);
    let inbox = read_inbox(&harness, &token_b, json!({})).await;
    assert_eq!(message_ids(&inbox), ["handoff-note"]);
}

// ============================================================ 範圍

#[tokio::test]
async fn a_worker_can_only_read_messages_addressed_to_its_own_task() {
    let harness = harness().await;
    let (ws, tasks) = workspace_with_tasks(&harness, "main", &["任務甲", "任務乙", "任務丙"]).await;
    let (a, b, c) = (&tasks[0], &tasks[1], &tasks[2]);
    let (run_a, token_a) = start_run(&harness, &ws, a).await;
    let (run_b, token_b) = start_run(&harness, &ws, b).await;
    let (run_c, token_c) = start_run(&harness, &ws, c).await;
    send_ok(&harness, &token_a, json!({ "to": [b], "subject": "只給乙看的機密", "body": "機密內容", "message_id": "private-1" })).await;

    // C 的收件匣沒有它，連 include_read 也沒有
    for request in [json!({}), json!({ "include_read": true, "limit": 100 })] {
        let inbox = read_inbox(&harness, &token_c, request).await;
        assert_eq!(inbox["task_id"], c.as_str());
        assert!(inbox["messages"].as_array().unwrap().is_empty(), "{inbox}");
        assert_eq!(inbox["unread_remaining"], 0);
    }

    // 聲稱是 B 的 Workspace／Run：被拒絕，而且不會改用請求裡的值
    let claims = [
        json!({ "run_id": run_b }),
        json!({ "workspace_id": ws, "run_id": run_b }),
        json!({ "workspace_id": "ws_other", "run_id": run_c }),
        json!({ "workspace_id": ws, "run_id": run_a }),
    ];
    for claim in &claims {
        let mut request = json!({ "include_read": true });
        request.as_object_mut().unwrap().extend(claim.as_object().unwrap().clone());
        let (status, response) = harness.call_tool(&token_c, TOOL_READ_INBOX, request).await;
        assert_eq!((status, error_code(&response)), (StatusCode::FORBIDDEN, "agent_scope_mismatch"), "{claim}：{response}");
        assert!(!response.to_string().contains("機密"));
    }
    // 聲稱別的 Task：不論 Runtime 忽略或拒絕，都讀不到別人的訊息
    let (_, response) = harness.call_tool(&token_c, TOOL_READ_INBOX, json!({ "task_id": b, "include_read": true })).await;
    assert!(!response.to_string().contains("機密"), "{response}");

    // 寄件也一樣不接受別人的歸屬
    let (status, response) = harness
        .call_tool(&token_c, TOOL_SEND_MESSAGE, json!({ "to": [a], "subject": "冒名", "run_id": run_b }))
        .await;
    assert_eq!((status, error_code(&response)), (StatusCode::FORBIDDEN, "agent_scope_mismatch"), "{response}");
    assert_eq!(message_count(&harness), 1);

    // 被拒絕的請求沒有動到 B 的未讀狀態；B 自己照常讀得到
    assert!(deliveries_of(&harness, "private-1")[0].2.is_none());
    let inbox = read_inbox(&harness, &token_b, json!({})).await;
    assert_eq!(message_ids(&inbox), ["private-1"]);
    assert_eq!(inbox["messages"][0]["already_read"], false);
}

#[tokio::test]
async fn messages_never_cross_workspaces() {
    let harness = harness().await;
    let (ws, tasks) = workspace_with_tasks(&harness, "main", &["任務甲", "任務乙"]).await;
    let (other_ws, other_tasks) = workspace_with_tasks(&harness, "other", &["外部甲", "外部乙"]).await;
    let (_, token_a) = start_run(&harness, &ws, &tasks[0]).await;
    let (_, token_x) = start_run(&harness, &other_ws, &other_tasks[0]).await;

    // 寄給別的 Workspace 的 Task、或根本不存在的 Task：同樣是 recipient_not_found，不洩漏有沒有那個 Task
    let mut responses = Vec::new();
    for target in [other_tasks[1].as_str(), other_tasks[0].as_str(), "task_does_not_exist"] {
        let (status, response) = harness.call_tool(&token_a, TOOL_SEND_MESSAGE, json!({ "to": [target], "subject": "跨界" })).await;
        assert_eq!((status, error_code(&response)), (StatusCode::NOT_FOUND, "recipient_not_found"), "{target}：{response}");
        responses.push(response);
    }
    assert!(!responses[0].to_string().contains("外部乙"), "回應不可洩漏別的 Workspace 的資料");
    assert_eq!(responses[0]["error"], responses[2]["error"]);
    // 收件人有一個不存在就整則不寄：同一則訊息裡合法的收件人也沒有收到
    let (status, response) = harness
        .call_tool(&token_a, TOOL_SEND_MESSAGE, json!({ "to": [tasks[1], other_tasks[1]], "subject": "混著寄" }))
        .await;
    assert_eq!((status, error_code(&response)), (StatusCode::NOT_FOUND, "recipient_not_found"), "{response}");
    assert_eq!((message_count(&harness), delivery_count(&harness)), (0, 0));

    // 別的 Workspace 的 Worker 收件匣不受影響
    send_ok(&harness, &token_a, json!({ "to": [tasks[1]], "subject": "本工作區的訊息" })).await;
    let inbox = read_inbox(&harness, &token_x, json!({ "include_read": true })).await;
    assert_eq!(inbox["task_id"], other_tasks[0].as_str());
    assert!(inbox["messages"].as_array().unwrap().is_empty(), "{inbox}");
}

#[tokio::test]
async fn the_desktop_credential_cannot_use_the_messaging_tools() {
    let harness = harness().await;
    let (ws, tasks) = workspace_with_tasks(&harness, "main", &["任務甲", "任務乙"]).await;
    let (_, token_a) = start_run(&harness, &ws, &tasks[0]).await;
    let (_, token_b) = start_run(&harness, &ws, &tasks[1]).await;
    send_ok(&harness, &token_a, json!({ "to": [tasks[1]], "subject": "給乙", "message_id": "m-1" })).await;
    let desktop = harness.desktop_credential();

    let (status, response) =
        harness.call_tool(&desktop, TOOL_SEND_MESSAGE, json!({ "to": [tasks[1]], "subject": "桌面冒充 Worker" })).await;
    assert_eq!((status, error_code(&response)), (StatusCode::FORBIDDEN, "role_not_allowed"), "{response}");
    let (status, response) = harness.call_tool(&desktop, TOOL_READ_INBOX, json!({})).await;
    assert_eq!((status, error_code(&response)), (StatusCode::FORBIDDEN, "role_not_allowed"), "{response}");

    assert_eq!(message_count(&harness), 1);
    assert!(deliveries_of(&harness, "m-1")[0].2.is_none(), "被拒絕的讀取不改變已讀狀態");
    assert_eq!(message_ids(&read_inbox(&harness, &token_b, json!({})).await), ["m-1"]);
}

// ============================================================ 訊息不能改變範圍或歸屬

/// 一個試圖改範圍的訊息：(kind, 額外欄位)。
type Case<'a> = (Option<&'a str>, Vec<(&'a str, Value)>);

#[tokio::test]
async fn a_message_that_tries_to_change_scope_or_ownership_is_ignored_and_reported_to_the_controller() {
    let harness = harness().await;
    let (ws, tasks) = workspace_with_tasks(&harness, "main", &["任務甲", "任務乙"]).await;
    let (a, b) = (&tasks[0], &tasks[1]);
    let (run_a, token_a) = start_run(&harness, &ws, a).await;
    let (_, token_b) = start_run(&harness, &ws, b).await;
    let b_contract_before = harness.call_tool(&token_b, TOOL_GET_TASK_CONTRACT, json!({})).await;
    let a_contract_before = harness.call_tool(&token_a, TOOL_GET_TASK_CONTRACT, json!({})).await;
    let (a_detail_before, b_detail_before) = (task_detail(&harness, &ws, a).await, task_detail(&harness, &ws, b).await);
    let long_body = "範圍".repeat(300); // 600 個字元，事件裡最多留 500 個

    // (kind, 額外欄位)：五種 kind 與八種欄位各一次，另外加一個 kind 與欄位並用的
    let mut cases: Vec<Case> = ["scope_change", "reassign", "reassign_task", "change_scope", "update_contract"]
        .into_iter()
        .map(|kind| (Some(kind), vec![]))
        .collect();
    let fields: [(&str, Value); 8] = [
        ("task_id", json!(b)),
        ("contract", contract("被偷換的目標")),
        ("scope", json!("全部網路")),
        ("assignee", json!(a)),
        ("assign_to", json!(a)),
        ("reassign_to", json!(a)),
        ("new_scope", json!("全部網路")),
        ("owner", json!(a)),
    ];
    cases.extend(fields.iter().map(|(name, value)| (None, vec![(*name, value.clone())])));
    cases.push((Some("reassign"), vec![("task_id", json!(b)), ("owner", json!(a))]));

    for (index, (kind, extra)) in cases.iter().enumerate() {
        let mut body = json!({ "to": [b], "subject": format!("請改範圍 {index}"), "body": long_body, "message_id": format!("scope-{index}") });
        if let Some(kind) = kind {
            body["kind"] = json!(kind);
        }
        for (name, value) in extra {
            body[*name] = value.clone();
        }

        let (status, response) = harness.call_tool(&token_a, TOOL_SEND_MESSAGE, body).await;

        assert_eq!(status, StatusCode::OK, "{kind:?} {extra:?}：{response}");
        assert_eq!(
            response,
            json!({
                "accepted": false, "ignored": true, "duplicate": false, "message_id": null, "thread_id": null,
                "correlation_id": null, "deliveries": [], "reason": "scope_change_not_allowed",
            }),
            "{kind:?} {extra:?}"
        );
        let events = scope_change_events(&harness);
        assert_eq!(events.len(), index + 1, "每次嘗試追加一筆給 Controller 的事件");
        let payload = events.last().unwrap();
        let mut attempted: Vec<&str> = payload["attempted_fields"].as_array().unwrap().iter().map(|f| f.as_str().unwrap()).collect();
        attempted.sort_unstable();
        let mut expected: Vec<&str> = extra.iter().map(|(name, _)| *name).collect();
        expected.sort_unstable();
        assert_eq!(attempted, expected, "{payload}");
        assert_eq!(payload["attempted_kind"], json!(kind));
        assert_eq!(payload["workspace_id"], ws.as_str());
        assert_eq!(payload["run_id"], run_a.as_str());
        assert_eq!(payload["task_id"], a.as_str());
        assert_eq!(payload["generation"], 1);
        assert_eq!(payload["recipients"], json!([b]));
        assert_eq!(payload["subject"], format!("請改範圍 {index}"));
        assert_eq!(payload["body_excerpt"].as_str().unwrap().chars().count(), 500);
        assert!(long_body.starts_with(payload["body_excerpt"].as_str().unwrap()));
        assert_eq!(payload["reason"], "scope_change_not_allowed");
    }

    // 訊息沒有被儲存或交付；B 的收件匣、契約、狀態與 A 的都沒有變
    assert_eq!((message_count(&harness), delivery_count(&harness)), (0, 0));
    let inbox = read_inbox(&harness, &token_b, json!({ "include_read": true })).await;
    assert!(inbox["messages"].as_array().unwrap().is_empty(), "{inbox}");
    assert_eq!(harness.call_tool(&token_b, TOOL_GET_TASK_CONTRACT, json!({})).await, b_contract_before);
    assert_eq!(harness.call_tool(&token_a, TOOL_GET_TASK_CONTRACT, json!({})).await, a_contract_before);
    assert_eq!(task_detail(&harness, &ws, a).await, a_detail_before);
    assert_eq!(task_detail(&harness, &ws, b).await, b_detail_before);

    // 正常的訊息仍然可以寄，而且寄完之後歸屬、契約與狀態一樣沒變
    send_ok(&harness, &token_a, json!({ "to": [b], "kind": "result", "subject": "一般的結論", "message_id": "normal-1" })).await;
    assert_eq!(message_ids(&read_inbox(&harness, &token_b, json!({})).await), ["normal-1"]);
    assert_eq!(harness.call_tool(&token_b, TOOL_GET_TASK_CONTRACT, json!({})).await, b_contract_before);
    assert_eq!(task_detail(&harness, &ws, a).await, a_detail_before);
    assert_eq!(task_detail(&harness, &ws, b).await, b_detail_before);
    assert_eq!(scope_change_events(&harness).len(), cases.len(), "正常的訊息不產生這種事件");
}

// ============================================================ Run 啟動時的未讀摘要

#[tokio::test]
async fn a_run_start_prompt_lists_unread_messages_without_their_bodies() {
    let harness = harness().await;
    let (ws, tasks) = workspace_with_tasks(&harness, "main", &["任務甲", "任務乙"]).await;
    let (a, b) = (&tasks[0], &tasks[1]);
    let (_, token_a) = start_run(&harness, &ws, a).await;

    // B 還沒有 Run 的時候先收到訊息
    for (n, kind) in [(1, "question"), (2, "note"), (3, "result")] {
        send_ok(
            &harness,
            &token_a,
            json!({ "to": [b], "kind": kind, "subject": format!("主旨-{n}"), "body": format!("祕密本文-{n}"), "message_id": format!("pre-{n}") }),
        )
        .await;
    }
    assert!(deliveries_of(&harness, "pre-1")[0].1.is_none(), "還沒有 Run 啟動：pending");

    let (run_b1, token_b) = start_run(&harness, &ws, b).await;
    let prompt = prompt_of(&harness, &run_b1).await;

    assert!(prompt.contains("未讀訊息：3 則"), "{prompt}");
    assert!(prompt.contains("read_inbox"));
    let lines: Vec<&str> = prompt.lines().filter(|l| l.starts_with("- [")).collect();
    assert_eq!(
        lines,
        [
            "- [question] 主旨-1（來自 Task「任務甲」，pre-1）",
            "- [note] 主旨-2（來自 Task「任務甲」，pre-2）",
            "- [result] 主旨-3（來自 Task「任務甲」，pre-3）",
        ]
    );
    assert!(!prompt.contains("祕密本文"), "摘要不含訊息本文");
    assert!(!prompt.contains("另有"), "沒超過上限就沒有「另有」");
    for n in 1..=3 {
        let rows = deliveries_of(&harness, &format!("pre-{n}"));
        assert!(rows[0].1.is_some() && rows[0].2.is_none(), "出現在摘要：delivered，還沒讀：{rows:?}");
    }

    // 派工之後才送來的訊息不在這次的 Prompt 裡，也還是 pending
    send_ok(&harness, &token_a, json!({ "to": [b], "subject": "主旨-遲到", "message_id": "post-1" })).await;
    assert!(!prompt_of(&harness, &run_b1).await.contains("主旨-遲到"));
    assert!(deliveries_of(&harness, "post-1")[0].1.is_none());

    // B 自己拉取：四則全文，read_at 設定
    let inbox = read_inbox(&harness, &token_b, json!({})).await;
    assert_eq!(message_ids(&inbox), ["pre-1", "pre-2", "pre-3", "post-1"]);
    assert_eq!(inbox["messages"][0]["body"], "祕密本文-1");
    for id in ["pre-1", "pre-2", "pre-3", "post-1"] {
        assert!(deliveries_of(&harness, id)[0].2.is_some(), "{id} 應為 read");
    }

    // 全部讀完之後換一個新的 Run：Prompt 沒有未讀訊息
    cancel_run(&harness, &ws, &run_b1).await;
    let (run_b2, _) = start_run(&harness, &ws, b).await;
    let prompt = prompt_of(&harness, &run_b2).await;
    assert!(!prompt.contains("未讀訊息"), "{prompt}");
}

#[tokio::test]
async fn a_new_run_only_lists_the_messages_the_previous_run_left_unread() {
    let harness = harness().await;
    let (ws, tasks) = workspace_with_tasks(&harness, "main", &["任務甲", "任務乙"]).await;
    let (a, b) = (&tasks[0], &tasks[1]);
    let (_, token_a) = start_run(&harness, &ws, a).await;
    for n in 1..=3 {
        send_ok(&harness, &token_a, json!({ "to": [b], "subject": format!("主旨-{n}"), "message_id": format!("part-{n}") })).await;
    }
    let (run_b1, token_b) = start_run(&harness, &ws, b).await;
    assert!(prompt_of(&harness, &run_b1).await.contains("未讀訊息：3 則"));

    // 第一個 Run 只讀了前兩則就被取消（交接）
    let inbox = read_inbox(&harness, &token_b, json!({ "limit": 2 })).await;
    assert_eq!(message_ids(&inbox), ["part-1", "part-2"]);
    assert_eq!(inbox["unread_remaining"], 1);
    cancel_run(&harness, &ws, &run_b1).await;
    let (run_b2, token_b2) = start_run(&harness, &ws, b).await;
    let prompt = prompt_of(&harness, &run_b2).await;

    assert!(prompt.contains("未讀訊息：1 則"), "{prompt}");
    assert!(prompt.contains("主旨-3") && !prompt.contains("主旨-1") && !prompt.contains("主旨-2"), "{prompt}");
    // 收件匣屬於 Task：新世代拿得到沒讀完的那一則，而且不會再看到讀過的
    let inbox = read_inbox(&harness, &token_b2, json!({})).await;
    assert_eq!(message_ids(&inbox), ["part-3"]);
    assert_eq!(inbox["unread_remaining"], 0);
}

#[tokio::test]
async fn the_start_up_summary_lists_at_most_ten_messages_and_counts_the_rest() {
    let harness = harness().await;
    let (ws, tasks) = workspace_with_tasks(&harness, "main", &["任務甲", "任務乙"]).await;
    let (_, token_a) = start_run(&harness, &ws, &tasks[0]).await;
    for n in 1..=13 {
        send_ok(&harness, &token_a, json!({ "to": [tasks[1]], "subject": format!("大量-{n:02}"), "message_id": format!("bulk-{n:02}") })).await;
    }

    let (run_b, token_b) = start_run(&harness, &ws, &tasks[1]).await;
    let prompt = prompt_of(&harness, &run_b).await;

    assert!(prompt.contains("未讀訊息：13 則"), "{prompt}");
    let listed: Vec<&str> = prompt.lines().filter(|l| l.starts_with("- [")).collect();
    assert_eq!(listed.len(), 10, "{prompt}");
    assert_eq!(prompt.lines().last().unwrap(), "- …另有 3 則");
    // 列在摘要裡的訊息都是 delivered
    for line in &listed {
        let id = line.rsplit('，').next().unwrap().trim_end_matches('）');
        assert!(deliveries_of(&harness, id)[0].1.is_some(), "{id} 出現在摘要，delivered_at 應有值");
    }
    // 摘要只是摘要：全部 13 則都還讀得到
    let inbox = read_inbox(&harness, &token_b, json!({})).await;
    assert_eq!(inbox["messages"].as_array().unwrap().len(), 13);
}

// ============================================================ 對話串

#[tokio::test]
async fn a_reply_joins_the_thread_and_keeps_the_correlation_of_the_question() {
    let harness = harness().await;
    let (ws, tasks) = workspace_with_tasks(&harness, "main", &["任務甲", "任務乙", "任務丙"]).await;
    let (a, b, c) = (&tasks[0], &tasks[1], &tasks[2]);
    let (_, token_a) = start_run(&harness, &ws, a).await;
    let (_, token_b) = start_run(&harness, &ws, b).await;
    let (_, token_c) = start_run(&harness, &ws, c).await;
    send_ok(&harness, &token_a, json!({ "to": [b], "kind": "question", "subject": "格式？", "message_id": "q-1" })).await;

    // B 回覆：同一個對話串，correlation 沿用，且是新的訊息
    let answer = send_ok(&harness, &token_b, json!({ "to": [a], "kind": "answer", "subject": "用 JSON", "in_reply_to": "q-1" })).await;
    let answer_id = answer["message_id"].as_str().unwrap();
    assert!(answer_id.starts_with("msg_") && answer_id != "q-1");
    assert_eq!((answer["thread_id"].as_str(), answer["correlation_id"].as_str()), (Some("q-1"), Some("q-1")), "{answer}");
    let inbox = read_inbox(&harness, &token_a, json!({})).await;
    assert_eq!(inbox["messages"][0]["in_reply_to"], "q-1");
    assert_eq!(inbox["messages"][0]["thread_id"], "q-1");
    assert_eq!(inbox["messages"][0]["kind"], "answer");
    assert_eq!(inbox["messages"][0]["from_task_id"], b.as_str());

    // 對回覆再回覆：仍是最初的對話串；明確指定 correlation_id 時以指定的為準
    let follow_up = send_ok(&harness, &token_a, json!({ "to": [b], "subject": "謝謝", "in_reply_to": answer_id, "correlation_id": "corr:custom" })).await;
    assert_eq!((follow_up["thread_id"].as_str(), follow_up["correlation_id"].as_str()), (Some("q-1"), Some("corr:custom")), "{follow_up}");

    // 外人（C）既不是寄件人也不是收件人：不能回覆這串訊息，也不能確認它存在
    let (status, response) = harness.call_tool(&token_c, TOOL_SEND_MESSAGE, json!({ "to": [a], "subject": "插嘴", "in_reply_to": "q-1" })).await;
    assert_eq!((status, error_code(&response)), (StatusCode::NOT_FOUND, "reply_target_not_found"), "{response}");
    let (status, response) = harness.call_tool(&token_b, TOOL_SEND_MESSAGE, json!({ "to": [a], "subject": "回覆不存在的", "in_reply_to": "no-such-message" })).await;
    assert_eq!((status, error_code(&response)), (StatusCode::NOT_FOUND, "reply_target_not_found"), "{response}");
    assert_eq!(message_count(&harness), 3, "被拒絕的回覆什麼都不儲存");
}

#[tokio::test]
async fn invalid_messages_are_rejected_and_change_nothing() {
    let harness = harness().await;
    let (ws, tasks) = workspace_with_tasks(&harness, "main", &["任務甲", "任務乙"]).await;
    let (a, b) = (&tasks[0], &tasks[1]);
    let (_, token_a) = start_run(&harness, &ws, a).await;
    let (_, token_b) = start_run(&harness, &ws, b).await;
    let eleven: Vec<String> = (0..11).map(|n| format!("task_{n}")).collect();

    let invalid = [
        ("空主旨", json!({ "to": [b], "subject": "   " })),
        ("主旨超過 200 字元", json!({ "to": [b], "subject": "字".repeat(201) })),
        ("未知的 kind", json!({ "to": [b], "subject": "x", "kind": "shout" })),
        ("沒有收件人", json!({ "to": [], "subject": "x" })),
        ("收件人超過 10 個", json!({ "to": eleven, "subject": "x" })),
        ("寄給自己的 Task", json!({ "to": [a], "subject": "x" })),
        ("message_id 字元不合法", json!({ "to": [b], "subject": "x", "message_id": "bad id!" })),
        ("message_id 過長", json!({ "to": [b], "subject": "x", "message_id": "m".repeat(129) })),
        ("correlation_id 字元不合法", json!({ "to": [b], "subject": "x", "correlation_id": "bad/id" })),
        ("in_reply_to 字元不合法", json!({ "to": [b], "subject": "x", "in_reply_to": "bad id" })),
        ("本文超過 16 KiB", json!({ "to": [b], "subject": "x", "body": "x".repeat(16 * 1024 + 1) })),
    ];
    for (label, body) in invalid {
        let (status, response) = harness.call_tool(&token_a, TOOL_SEND_MESSAGE, body).await;
        assert_eq!((status, error_code(&response)), (StatusCode::BAD_REQUEST, "message_invalid"), "{label}：{response}");
    }
    // JSON 欄位型別錯誤是既有的 bad_request
    for body in [json!({ "to": "不是陣列", "subject": "x" }), json!({ "to": [b], "subject": 5 })] {
        let (status, response) = harness.call_tool(&token_a, TOOL_SEND_MESSAGE, body).await;
        assert_eq!((status, error_code(&response)), (StatusCode::BAD_REQUEST, "bad_request"), "{response}");
    }
    // read_inbox 的 limit 型別錯誤也是
    let (status, response) = harness.call_tool(&token_b, TOOL_READ_INBOX, json!({ "limit": "很多" })).await;
    assert_eq!((status, error_code(&response)), (StatusCode::BAD_REQUEST, "bad_request"), "{response}");

    assert_eq!((message_count(&harness), delivery_count(&harness)), (0, 0));
    let inbox = read_inbox(&harness, &token_b, json!({ "include_read": true })).await;
    assert!(inbox["messages"].as_array().unwrap().is_empty(), "{inbox}");
    assert_eq!(scope_change_events(&harness).len(), 0, "一般的驗證失敗不是改範圍的嘗試");

    // 邊界值本身是合法的：主旨 200 個字元（不是位元組）、本文 16 KiB、10 個收件人以內
    let boundary = send_ok(&harness, &token_a, json!({ "to": [b], "subject": "字".repeat(200), "body": "x".repeat(16 * 1024) })).await;
    assert_eq!(boundary["duplicate"], false);
    let inbox = read_inbox(&harness, &token_b, json!({})).await;
    assert_eq!(inbox["messages"][0]["subject"].as_str().unwrap().chars().count(), 200);
    assert_eq!(inbox["messages"][0]["body"].as_str().unwrap().len(), 16 * 1024);
}

// ============================================================ 世代與憑證失效

#[tokio::test]
async fn a_superseded_run_can_neither_send_nor_read_and_the_new_run_inherits_the_inbox() {
    let harness = harness().await;
    let (ws, tasks) = workspace_with_tasks(&harness, "main", &["任務甲", "任務乙"]).await;
    let (a, b) = (&tasks[0], &tasks[1]);
    let (run_a1, old_token) = start_run(&harness, &ws, a).await;
    let (_, token_b) = start_run(&harness, &ws, b).await;
    // B 寄給 A 的訊息，A 的第一個 Run 來不及讀
    send_ok(&harness, &token_b, json!({ "to": [a], "subject": "給甲的交接事項", "message_id": "for-a" })).await;
    send_ok(&harness, &old_token, json!({ "to": [b], "subject": "第一世代的訊息", "message_id": "gen1-msg" })).await;

    cancel_run(&harness, &ws, &run_a1).await;
    let (run_a2, new_token) = start_run(&harness, &ws, a).await;
    let messages_before = message_count(&harness);

    // 舊世代：寄不出去、讀不到，也不會改變已讀狀態
    let (status, response) =
        harness.call_tool(&old_token, TOOL_SEND_MESSAGE, json!({ "to": [b], "subject": "舊世代遲到的訊息", "message_id": "stale-1" })).await;
    assert_eq!((status, error_code(&response)), (StatusCode::CONFLICT, "run_report_stale"), "{response}");
    let (status, response) = harness.call_tool(&old_token, TOOL_READ_INBOX, json!({})).await;
    assert_eq!((status, error_code(&response)), (StatusCode::UNAUTHORIZED, "credential_revoked"), "{response}");
    assert_eq!(message_count(&harness), messages_before);
    assert!(deliveries_of(&harness, "for-a")[0].2.is_none());

    // 新世代照常：拿得到前一個 Run 沒讀的訊息，也寄得出去
    let inbox = read_inbox(&harness, &new_token, json!({})).await;
    assert_eq!(message_ids(&inbox), ["for-a"]);
    let sent = send_ok(&harness, &new_token, json!({ "to": [b], "subject": "第二世代的訊息" })).await;
    assert_eq!(sent["duplicate"], false);
    let rows: Vec<(String, i64)> = {
        let db = harness.db();
        let mut statement = db.prepare("SELECT sender_run_id, sender_generation FROM message ORDER BY created_at, rowid").unwrap();
        statement.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect()
    };
    assert!(rows.contains(&(run_a1.clone(), 1)) && rows.contains(&(run_a2.clone(), 2)), "訊息記下寄件的 Run 與世代：{rows:?}");
}

#[tokio::test]
async fn the_messaging_tools_stop_working_when_the_run_has_ended() {
    let harness = harness().await;
    let (ws, tasks) = workspace_with_tasks(&harness, "main", &["任務甲", "任務乙"]).await;
    let (a, b) = (&tasks[0], &tasks[1]);
    let (_, token_b) = start_run(&harness, &ws, b).await;
    harness.agent_adapter.script_next_run(vec![baton_ports::AdapterEvent::Exited { success: true }]);
    let (run_a, token_a) = start_run(&harness, &ws, a).await;
    send_ok(&harness, &token_b, json!({ "to": [a], "subject": "給甲", "message_id": "for-a" })).await;
    harness.supervise().await; // 收到 Adapter 的結束事件，Run 結束
    let (_, run) = harness.send_json(Method::GET, &run_path(&ws, &run_a), None).await;
    assert_eq!(run["status"], "succeeded");

    let (status, response) = harness.call_tool(&token_a, TOOL_SEND_MESSAGE, json!({ "to": [b], "subject": "結束後才說" })).await;
    assert_eq!((status, error_code(&response)), (StatusCode::CONFLICT, "run_report_stale"), "{response}");
    let (status, response) = harness.call_tool(&token_a, TOOL_READ_INBOX, json!({})).await;
    assert_eq!((status, error_code(&response)), (StatusCode::UNAUTHORIZED, "credential_revoked"), "{response}");

    assert_eq!(message_count(&harness), 1, "只有 B 先前寄的那一則");
    assert!(deliveries_of(&harness, "for-a")[0].2.is_none());
    let inbox = read_inbox(&harness, &token_b, json!({ "include_read": true })).await;
    assert!(inbox["messages"].as_array().unwrap().is_empty());
}

// ============================================================ MCP shim

/// 以 `tokio::io::duplex` 當 stdio，在程序內跑真正的 shim，把每一行請求送進去並收回每一行回應。
async fn talk_to_shim(harness: &Harness, token: &str, requests: &[Value]) -> Vec<Value> {
    let (client, shim_side) = duplex(64 * 1024);
    let (shim_read, shim_write) = tokio::io::split(shim_side);
    let config = ShimConfig::new(harness.runtime_url(), token.to_string());
    let server = tokio::spawn(async move { serve(BufReader::new(shim_read), shim_write, config).await });

    let (client_read, mut client_write) = tokio::io::split(client);
    for request in requests {
        client_write.write_all(format!("{request}\n").as_bytes()).await.unwrap();
    }
    client_write.shutdown().await.unwrap(); // 關閉 stdin：shim 處理完後乾淨結束
    let mut lines = BufReader::new(client_read).lines();
    let mut responses = Vec::new();
    while let Some(line) = lines.next_line().await.unwrap() {
        responses.push(serde_json::from_str(&line).expect("shim 的輸出每一行都是 JSON-RPC 訊息"));
    }
    server.await.unwrap().unwrap();
    responses
}

fn tool_call(id: u64, name: &str, arguments: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call", "params": { "name": name, "arguments": arguments } })
}

fn initialize() -> Value {
    json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "claude-code", "version": "0" } } })
}

#[tokio::test]
async fn workers_exchange_messages_through_the_mcp_shim() {
    let harness = harness().await;
    let (ws, tasks) = workspace_with_tasks(&harness, "main", &["任務甲", "任務乙"]).await;
    let (a, b) = (&tasks[0], &tasks[1]);
    let (_, token_a) = start_run(&harness, &ws, a).await;
    let (_, token_b) = start_run(&harness, &ws, b).await;

    // Worker A 經 shim 列出工具、寄出一則正常的與一則空主旨的訊息
    let from_a = talk_to_shim(
        &harness,
        &token_a,
        &[
            initialize(),
            json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
            json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
            tool_call(3, TOOL_SEND_MESSAGE, json!({ "to": [b], "kind": "question", "subject": "透過 shim 的提問", "body": "請問 API 的版本？", "message_id": "shim-1" })),
            tool_call(4, TOOL_SEND_MESSAGE, json!({ "to": [b], "subject": "" })),
        ],
    )
    .await;

    assert_eq!(from_a.len(), 4, "通知不產生回應");
    let tools: Vec<&str> = from_a[1]["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(tools.contains(&TOOL_SEND_MESSAGE) && tools.contains(&TOOL_READ_INBOX), "{tools:?}");
    for tool in from_a[1]["result"]["tools"].as_array().unwrap() {
        if tool["name"] == TOOL_SEND_MESSAGE || tool["name"] == TOOL_READ_INBOX {
            assert!(tool["inputSchema"].is_object(), "工具要有輸入結構：{tool}");
        }
    }
    assert_eq!(from_a[2]["result"]["isError"], false, "{}", from_a[2]);
    assert!(from_a[2]["result"]["content"][0]["text"].as_str().unwrap().contains("shim-1"));
    assert_eq!(from_a[3]["result"]["isError"], true, "Runtime 拒絕時是工具錯誤，不是協定錯誤");
    assert!(from_a[3]["result"]["content"][0]["text"].as_str().unwrap().contains("message_invalid"));
    assert!(!from_a.iter().any(|r| r.to_string().contains(&token_a)), "shim 的輸出不含憑證");
    assert_eq!(message_count(&harness), 1);

    // Worker B 經 shim 讀到，之後再讀沒有新的
    let from_b = talk_to_shim(
        &harness,
        &token_b,
        &[
            initialize(),
            tool_call(2, TOOL_READ_INBOX, json!({})),
            tool_call(3, TOOL_READ_INBOX, json!({})),
        ],
    )
    .await;

    assert_eq!(from_b[1]["result"]["isError"], false, "{}", from_b[1]);
    let first_text = from_b[1]["result"]["content"][0]["text"].as_str().unwrap();
    assert!(first_text.contains("透過 shim 的提問") && first_text.contains("請問 API 的版本？") && first_text.contains("任務甲"), "{first_text}");
    let second_text = from_b[2]["result"]["content"][0]["text"].as_str().unwrap();
    assert!(!second_text.contains("透過 shim 的提問"), "已讀的不會再回傳：{second_text}");
    assert!(!from_b.iter().any(|r| r.to_string().contains(&token_b)));
    assert!(!harness.log_text().contains(&token_a) && !harness.log_text().contains(&token_b));
}

// ============================================================ 重啟

#[tokio::test]
async fn messages_and_their_read_state_survive_a_runtime_restart() {
    let mut harness = harness().await;
    let (ws, tasks) = workspace_with_tasks(&harness, "main", &["任務甲", "任務乙"]).await;
    let (a, b) = (&tasks[0], &tasks[1]);
    let (_, token_a) = start_run(&harness, &ws, a).await;
    let (_, token_b) = start_run(&harness, &ws, b).await;
    for n in 1..=2 {
        send_ok(&harness, &token_a, json!({ "to": [b], "subject": format!("重啟-{n}"), "body": format!("本文-{n}"), "message_id": format!("restart-{n}") })).await;
    }
    let before = read_inbox(&harness, &token_b, json!({ "limit": 1 })).await;
    assert_eq!(message_ids(&before), ["restart-1"]);

    harness.restart().await;

    // 沒讀的還在、讀過的不會再回來；內容完整
    let after = read_inbox(&harness, &token_b, json!({})).await;
    assert_eq!(message_ids(&after), ["restart-2"], "{after}");
    assert_eq!(after["messages"][0]["body"], "本文-2");
    assert_eq!(after["messages"][0]["already_read"], false);
    let everything = read_inbox(&harness, &token_b, json!({ "include_read": true })).await;
    assert_eq!(message_ids(&everything), ["restart-1", "restart-2"]);
    // 重啟後重送同樣的內容仍然是重複
    let again = send_ok(&harness, &token_a, json!({ "to": [b], "subject": "重啟-1", "body": "本文-1", "message_id": "restart-1" })).await;
    assert_eq!(again["duplicate"], true, "{again}");
    assert_eq!(message_count(&harness), 2);
}

#[tokio::test]
async fn read_inbox_rejects_a_limit_outside_the_allowed_range_without_marking_anything_read() {
    let harness = harness().await;
    let (ws, tasks) = workspace_with_tasks(&harness, "main", &["任務甲", "任務乙"]).await;
    let (_, token_a) = start_run(&harness, &ws, &tasks[0]).await;
    let (_, token_b) = start_run(&harness, &ws, &tasks[1]).await;
    send_ok(&harness, &token_a, json!({ "to": [tasks[1]], "subject": "一則", "message_id": "m-1" })).await;

    for limit in [0, 101] {
        let (status, response) = harness.call_tool(&token_b, TOOL_READ_INBOX, json!({ "limit": limit })).await;
        assert_eq!((status, error_code(&response)), (StatusCode::BAD_REQUEST, "message_invalid"), "limit={limit}：{response}");
    }

    assert_eq!(message_ids(&read_inbox(&harness, &token_b, json!({})).await), ["m-1"]);
}
