//! 情境測試：移除舊的對話頁與 Controller 自動喚醒（desktop-controller-cli 票 08，ADR-0014），經由 Runtime harness 驗證。
//!
//! Controller 改由使用者操作的 Controller CLI 擔任：Runtime 不再因使用者訊息或工作事件啟動 `claude -p` 的 Controller 輪次。
//! 升級前留下的對話資料（conversation、conversation_turn、`kind = 'turn'` 的 Controller Run）以直接寫入資料庫的方式模擬，
//! 它們必須原樣留在資料庫、不被遷移、刪除或再寫入。
//!
//! 只斷言外部行為：API 回應、交給 ProcessControl 的 spawn（Controller 的一輪以 `ctl_` 開頭）、Controller 終端機的開窗次數，
//! 以及升級前資料在資料庫中的內容（「資料仍在資料庫」本身就是驗收條件）。
//!
//! 每個測試名稱對應票 08 的驗收條件：
//! - 對話端點與使用者訊息不再喚醒 Controller：`the_conversation_endpoints_are_gone`
//! - 工作事件（Worker 結束、受阻、Controller 提出的裁定被回答）不產生自動 Controller Run：
//!   `worker_results_blockers_and_answered_controller_questions_never_start_a_controller_turn`
//! - 既有對話資料升級後不遷移、不刪除：`legacy_conversation_data_stays_untouched_across_restarts`

use baton_domain::Timestamp;
use baton_harness::Harness;
use baton_ports::AdapterEvent;
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

const WORKSPACES: &str = "/api/v1/workspaces";

/// 2023-11-14 22:13:20.123 UTC
const CLOCK_START: Timestamp = Timestamp::from_millis(1_700_000_000_123);
/// 舊的事件合併視窗是 5 秒、輸入退避最多 10 分鐘；推進一小時足以涵蓋任何舊的喚起時機。
const AN_HOUR_MS: i64 = 3_600_000;

async fn start() -> (Harness, String, String) {
    let harness = Harness::builder().clock_at(CLOCK_START).start().await;
    harness.add_account("帳號 1", "account1@example.com").await;
    let controller = harness.add_account("帳號 2", "account2@example.com").await;
    let dir = harness.data_dir().join("projects").join("main");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.to_string_lossy().into_owned();
    let (status, body) = harness.send_json(Method::POST, WORKSPACES, Some(json!({ "path": path }))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    (harness, body["id"].as_str().unwrap().to_string(), controller)
}

fn filled(text: &str) -> Value {
    json!({ "status": "filled", "text": text })
}

fn contract() -> Value {
    json!({
        "objective": filled("整理資料"),
        "inputs": filled("既有的資料"),
        "scope": filled("公開資料"),
        "deliverables": filled("比較表"),
        "done_criteria": filled("五個產品"),
        "blocker_handling": filled("卡住時回報"),
        "required_capabilities": [],
    })
}

/// 建立 Goal 與一個 ready 的 Task 並派工，回傳 Worker Run 識別。
async fn dispatched_task(harness: &Harness, ws: &str, title: &str) -> String {
    let (status, goal) =
        harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/goals"), Some(json!({ "title": "目標" }))).await;
    assert_eq!(status, StatusCode::CREATED, "{goal}");
    let body = json!({ "goal_id": goal["id"], "title": title, "contract": contract() });
    let (status, task) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks"), Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    let tid = task["id"].as_str().unwrap();
    let (status, ready) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks/{tid}/ready"), None).await;
    assert_eq!(status, StatusCode::OK, "{ready}");
    let (status, run) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks/{tid}/dispatch"), None).await;
    assert_eq!(status, StatusCode::CREATED, "{run}");
    run["id"].as_str().unwrap().to_string()
}

/// 被啟動過的 Controller 輪次（`claude -p`）數：fake spawn 的第一個參數是 Run 識別，Controller 的一輪以 `ctl_` 開頭。
fn controller_turn_spawns(harness: &Harness) -> usize {
    harness.process_control.spawned().iter().filter(|s| s.args.first().is_some_and(|a| a.starts_with("ctl_"))).count()
}

/// 讓時間經過一小時，期間 Supervisor 處理多次。
async fn let_an_hour_pass(harness: &Harness) {
    for _ in 0..12 {
        harness.clock.advance_millis(AN_HOUR_MS / 12);
        harness.supervise().await;
    }
}

/// 升級前留下的對話：一則已處理的使用者訊息與 Controller 的一輪、一則還在等待的使用者訊息。
fn write_legacy_conversation(harness: &Harness, ws: &str, profile: &str) {
    let db = rusqlite::Connection::open(harness.data_dir().join(baton_store::DB_FILE_NAME)).unwrap();
    db.execute_batch(&format!(
        "INSERT INTO conversation (id, workspace_id, version, created_at) VALUES ('conv_old', '{ws}', 3, 1);
         INSERT INTO agent_session (id, runtime_type, workspace_id, role, conversation_id, profile_id, working_directory,
           state, version, created_at, last_activity_at)
           VALUES ('ses_old', 'claude_code', '{ws}', 'controller', 'conv_old', '{profile}', 'C:/w', 'closed', 1, 1, 1);
         INSERT INTO controller_run (id, kind, conversation_id, workspace_id, profile_id, status, version, command_id, prompt,
           working_dir, input_turn_ids, agent_session_id, created_at, updated_at)
           VALUES ('ctl_old', 'turn', 'conv_old', '{ws}', '{profile}', 'succeeded', 2, 'cmd_old', '你好', 'C:/w',
                   '[\"turn_1\"]', 'ses_old', 1, 2);
         INSERT INTO conversation_turn (id, conversation_id, seq, kind, status, content, controller_run_id, attempts, source,
           created_at, processed_at)
           VALUES ('turn_1', 'conv_old', 1, 'user_message', 'processed', '你好', 'ctl_old', 1, NULL, 1, 2),
                  ('turn_2', 'conv_old', 2, 'controller_reply', 'processed', '收到', 'ctl_old', 0, NULL, 2, 2),
                  ('turn_3', 'conv_old', 3, 'user_message', 'pending', '升級前沒處理完的訊息', NULL, 0, NULL, 3, NULL);"
    ))
    .unwrap();
}

/// 升級前的對話資料，依建立順序：(識別, 種類, 狀態, 內容)。
fn legacy_turns(harness: &Harness) -> Vec<(String, String, String, String)> {
    let db = harness.db();
    let mut statement =
        db.prepare("SELECT id, kind, status, content FROM conversation_turn ORDER BY conversation_id, seq").unwrap();
    statement.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap().map(Result::unwrap).collect()
}

/// `kind = 'turn'` 的 Controller Run（舊的 `claude -p` 輪次）：(識別, 狀態, 版本)。
fn legacy_controller_runs(harness: &Harness) -> Vec<(String, String, i64)> {
    let db = harness.db();
    let mut statement =
        db.prepare("SELECT id, status, version FROM controller_run WHERE kind = 'turn' ORDER BY rowid").unwrap();
    statement.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap().map(Result::unwrap).collect()
}

fn legacy_conversations(harness: &Harness) -> Vec<(String, i64)> {
    let db = harness.db();
    let mut statement = db.prepare("SELECT id, version FROM conversation ORDER BY id").unwrap();
    statement.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect()
}

#[tokio::test]
async fn the_conversation_endpoints_are_gone() {
    let (harness, ws, _) = start().await;

    let (status, body) = harness.send_json(Method::GET, &format!("{WORKSPACES}/{ws}/conversation"), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    let (status, body) = harness
        .send_json(Method::POST, &format!("{WORKSPACES}/{ws}/conversation/messages"), Some(json!({ "text": "在嗎？" })))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    let_an_hour_pass(&harness).await;
    assert_eq!(controller_turn_spawns(&harness), 0, "使用者訊息不應喚起 Controller 的一輪");
    assert!(harness.claude_env.controller_terminals().is_empty(), "Controller CLI 只由使用者開啟");
}

#[tokio::test]
async fn worker_results_blockers_and_answered_controller_questions_never_start_a_controller_turn() {
    let (harness, ws, controller) = start().await;
    // 舊版會把工作事件寫進已存在的 conversation 並喚起 Controller；先放一個升級前的對話
    write_legacy_conversation(&harness, &ws, &controller);
    let turns_before = legacy_turns(&harness);

    // Worker 成功結束
    let done = dispatched_task(&harness, &ws, "研究競品").await;
    harness.supervise().await;
    harness.agent_adapter.emit(&done, AdapterEvent::Exited { success: true });
    harness.supervise().await;
    // Worker 回報受阻
    let blocked = dispatched_task(&harness, &ws, "撰寫文件").await;
    harness.supervise().await;
    let token = harness.agent_credential(&blocked).expect("Worker 應該已啟動");
    let (status, ack) = harness.call_tool(&token, "report_blocked", json!({ "reason": "缺少範本" })).await;
    assert!(status.is_success(), "{ack}");
    // Controller CLI 提出的裁定請求被使用者回答
    let (_, controller_token) = harness.open_controller_cli(&ws, &controller).await;
    let (status, proposed) = harness
        .call_tool(&controller_token, "propose_task", json!({ "goal_title": "記帳 App", "title": "實作第一版", "contract": contract() }))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{proposed}");
    let question = json!({
        "question": "第一版要支援哪些平台？",
        "reason": "平台會改變技術選型，屬於產品方向",
        "options": [
            { "id": "web", "label": "只做網頁版", "tradeoff": "最快上線" },
            { "id": "mobile", "label": "iOS 與 Android", "tradeoff": "工作量加倍" },
        ],
        "recommendation": "web",
        "blocking": [proposed["task"]["id"]],
    });
    let (status, asked) = harness.call_tool(&controller_token, "propose_decision_request", question).await;
    assert_eq!(status, StatusCode::CREATED, "{asked}");
    let path = format!("{WORKSPACES}/{ws}/decisions/{}", asked["decision_request_id"].as_str().unwrap());
    let (_, view) = harness.send_json(Method::GET, &path, None).await;
    let body = json!({ "option_id": "web", "expected_revision": view["revision"] });
    let (status, answered) = harness.send_json(Method::POST, &format!("{path}/answer"), Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{answered}");

    let_an_hour_pass(&harness).await;

    assert_eq!(controller_turn_spawns(&harness), 0, "工作事件不應喚起 Controller 的一輪");
    assert_eq!(harness.claude_env.controller_terminals().len(), 1, "只有使用者開的那一個 Controller CLI");
    assert_eq!(legacy_turns(&harness), turns_before, "工作事件不再寫入對話");
}

#[tokio::test]
async fn legacy_conversation_data_stays_untouched_across_restarts() {
    let (mut harness, ws, controller) = start().await;
    write_legacy_conversation(&harness, &ws, &controller);
    let turns = legacy_turns(&harness);
    let runs = legacy_controller_runs(&harness);
    let conversations = legacy_conversations(&harness);
    assert_eq!(turns.len(), 3);

    let_an_hour_pass(&harness).await;
    harness.restart().await;
    let_an_hour_pass(&harness).await;

    assert_eq!(controller_turn_spawns(&harness), 0, "升級前待處理的訊息不應喚起 Controller");
    assert_eq!(legacy_turns(&harness), turns, "對話內容與狀態原樣保留");
    assert_eq!(legacy_controller_runs(&harness), runs);
    assert_eq!(legacy_conversations(&harness), conversations);
}
