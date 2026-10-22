//! 情境測試：Agent API、範圍憑證與 MCP 工具 shim（票 11），經由 Runtime harness 的真實 HTTP API 驗證。
//!
//! Worker 的憑證取自 fake ProcessControl 記錄的啟動環境變數（`Harness::agent_credential`），
//! 與真實 Worker 的 MCP shim 拿到的是同一個值；全部使用 fake Clock 與 fake Port。
//! 只斷言外部行為：HTTP 回應、Run／Task 的可觀察狀態、Port 被呼叫的方式；
//! `harness.db()` 只用來確認稽核紀錄與「憑證不落地」這類必須看落地內容的事實。

use baton_domain::agent::{
    mcp_config_json, AGENT_TOKEN_ENV, RUNTIME_URL_ENV, TOOL_GET_TASK_CONTRACT, TOOL_REPORT_BLOCKED, TOOL_REPORT_PROGRESS,
    TOOL_SAVE_CHECKPOINT,
};
use baton_domain::auth::Role;
use baton_domain::run::LEASE_DURATION_MS;
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
    Harness::builder().clock_at(CLOCK_START).start().await
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

/// 一個 Workspace、一個 ready 的 Task，回傳 (workspace_id, task_id)。
async fn workspace_with_ready_task(harness: &Harness, name: &str, objective: &str) -> (String, String) {
    let ws = new_workspace(harness, name).await;
    let (status, goal) =
        harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/goals"), Some(json!({ "title": "目標" }))).await;
    assert_eq!(status, StatusCode::CREATED, "{goal}");
    let body = json!({ "goal_id": goal["id"], "title": format!("{name} 的工作"), "contract": contract(objective) });
    let (status, task) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks"), Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    let tid = task["id"].as_str().unwrap().to_string();
    let (status, ready) = harness.send_json(Method::POST, &format!("{}/ready", task_path(&ws, &tid)), None).await;
    assert_eq!(status, StatusCode::OK, "{ready}");
    (ws, tid)
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

async fn run_view(harness: &Harness, ws: &str, rid: &str) -> Value {
    let (status, run) = harness.send_json(Method::GET, &run_path(ws, rid), None).await;
    assert_eq!(status, StatusCode::OK, "{run}");
    run
}

async fn task_status(harness: &Harness, ws: &str, tid: &str) -> String {
    let (status, detail) = harness.send_json(Method::GET, &task_path(ws, tid), None).await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    detail["task"]["status"].as_str().unwrap().to_string()
}

fn error_code(body: &Value) -> &str {
    body["error"].as_str().unwrap_or_default()
}

/// 稽核表中某個 Run 的回報：(kind, accepted, content 文字)，依收到的順序。
fn audited_reports(harness: &Harness, run_id: &str) -> Vec<(String, bool, String)> {
    let db = harness.db();
    let mut statement =
        db.prepare("SELECT kind, accepted, content FROM agent_report WHERE run_id = ?1 ORDER BY seq").unwrap();
    statement
        .query_map([run_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?, r.get::<_, String>(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn event_types(harness: &Harness, run_id: &str) -> Vec<String> {
    let db = harness.db();
    let mut statement = db
        .prepare("SELECT event_type FROM event WHERE aggregate_type = 'run' AND aggregate_id = ?1 ORDER BY seq")
        .unwrap();
    statement.query_map([run_id], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
}

/// 資料目錄下所有檔案（含 SQLite 與 WAL）的位元組中，是否出現過這段文字。
fn data_dir_contains(harness: &Harness, needle: &str) -> bool {
    fn walk(dir: &std::path::Path, needle: &[u8]) -> bool {
        std::fs::read_dir(dir).into_iter().flatten().flatten().any(|entry| {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, needle)
            } else {
                // 資料庫在 Runtime 執行中被開著，唯讀共享開啟即可
                std::fs::read(&path).is_ok_and(|bytes| bytes.windows(needle.len()).any(|w| w == needle))
            }
        })
    }
    walk(harness.data_dir(), needle.as_bytes())
}

// ============================================================ 憑證的簽發與交付

#[tokio::test]
async fn run_admission_hands_the_scoped_credential_to_the_worker_only_through_the_environment() {
    let harness = harness().await;
    let (ws, tid) = workspace_with_ready_task(&harness, "main", "整理資料").await;
    let (rid, token) = start_run(&harness, &ws, &tid).await;
    harness.supervise().await; // 接上 Adapter 事件串流，Adapter 因此收到 RunSpec

    let spawned = harness.process_control.spawned();
    assert_eq!(spawned.len(), 1);
    let env = &spawned[0].env;
    assert!(env.iter().any(|(name, value)| name == RUNTIME_URL_ENV && *value == harness.runtime_url()));
    assert_eq!(token.len(), 64, "256 位元隨機 token（64 個十六進位字元）");

    // 憑證不在指令列、Adapter 拿到的 Prompt、MCP 設定內容、Debug 輸出、日誌與資料目錄
    let all_args = spawned[0].args.join(" ");
    let launched = harness.agent_adapter.launched();
    assert!(!all_args.contains(&token));
    assert!(launched.iter().all(|spec| !spec.prompt.contains(&token) && !format!("{spec:?}").contains(&token)));
    assert!(!format!("{:?}", spawned[0]).contains(&token));
    let mcp_config = mcp_config_json("baton-mcp-shim.exe");
    assert!(!mcp_config.contains(&token));
    assert!(mcp_config.contains(&format!("${{{AGENT_TOKEN_ENV}}}")), "設定檔只放環境變數的參照");

    // 把憑證實際用過幾次（成功與被拒絕），再檢查它沒有落地到任何地方
    harness.call_tool(&token, TOOL_REPORT_PROGRESS, json!({ "summary": "開始了" })).await;
    harness.call_tool(&token, TOOL_REPORT_PROGRESS, json!({ "summary": "x", "run_id": "run_other" })).await;
    harness.call_tool("0".repeat(64).as_str(), TOOL_REPORT_PROGRESS, json!({ "summary": "x" })).await;
    assert!(!harness.log_text().contains(&token), "日誌不可含憑證");
    assert!(!data_dir_contains(&harness, &token), "資料庫、WAL 與其他檔案都不可含憑證明文");
    assert!(harness.log_text().contains("/agent/v1/tools/report_progress"), "請求有記錄，但只記路由樣板");
    let _ = rid;
}

#[tokio::test]
async fn each_run_gets_its_own_credential() {
    let harness = harness().await;
    let (ws_a, task_a) = workspace_with_ready_task(&harness, "a", "A 的目標").await;
    let (ws_b, task_b) = workspace_with_ready_task(&harness, "b", "B 的目標").await;

    let (_, token_a) = start_run(&harness, &ws_a, &task_a).await;
    let (_, token_b) = start_run(&harness, &ws_b, &task_b).await;

    assert_ne!(token_a, token_b);
}

// ============================================================ get_task_contract

#[tokio::test]
async fn get_task_contract_returns_the_contract_version_the_run_was_dispatched_with() {
    let harness = harness().await;
    let (ws, tid) = workspace_with_ready_task(&harness, "main", "整理資料").await;
    let (rid, token) = start_run(&harness, &ws, &tid).await;

    // 派工之後契約被改版：Worker 拿到的仍是派工當下固定的版本
    let (status, body) = harness
        .send_json(Method::PUT, &format!("{}/contract", task_path(&ws, &tid)), Some(json!({ "contract": contract("改過的目標") })))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let (status, response) = harness.call_tool(&token, TOOL_GET_TASK_CONTRACT, json!({})).await;

    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(response["run_id"], rid.as_str());
    assert_eq!(response["task_id"], tid.as_str());
    assert_eq!(response["generation"], 1);
    assert_eq!(response["contract_version"], 1);
    assert_eq!(response["contract"]["objective"]["text"], "整理資料");
    assert_eq!(response["contract"]["done_criteria"]["text"], "五個產品");
}

// ============================================================ 進度、checkpoint 與受阻

#[tokio::test]
async fn progress_and_checkpoint_reports_renew_the_lease_and_show_up_for_the_ui() {
    let harness = harness().await;
    let (ws, tid) = workspace_with_ready_task(&harness, "main", "整理資料").await;
    let (rid, token) = start_run(&harness, &ws, &tid).await;

    harness.clock.advance_millis(30_000);
    let (status, ack) = harness.call_tool(&token, TOOL_REPORT_PROGRESS, json!({ "summary": "  完成一半  " })).await;
    assert_eq!((status, ack["accepted"].as_bool(), ack["kind"].as_str()), (StatusCode::OK, Some(true), Some("progress")), "{ack}");
    harness.clock.advance_millis(5_000);
    let (status, _) = harness.call_tool(&token, TOOL_REPORT_PROGRESS, json!({ "summary": "完成八成" })).await;
    assert_eq!(status, StatusCode::OK);
    let (status, ack) = harness
        .call_tool(&token, TOOL_SAVE_CHECKPOINT, json!({ "summary": "第一章完成", "details": "下一步：寫第二章" }))
        .await;
    assert_eq!((status, ack["kind"].as_str()), (StatusCode::OK, Some("checkpoint")), "{ack}");

    // UI 的資料來源：工作面板的 Run 清單與單一 Run
    let board: Value = harness.get_json(&format!("{WORKSPACES}/{ws}/tasks")).await;
    let on_board = board["runs"].as_array().unwrap().iter().find(|r| r["id"] == rid.as_str()).unwrap().clone();
    let single = run_view(&harness, &ws, &rid).await;
    for view in [&on_board, &single] {
        assert_eq!(view["latest_progress"]["summary"], "完成八成", "同一種類以最後一筆為準");
        assert_eq!(view["latest_progress"]["reported_at"], CLOCK_START.as_millis() + 35_000);
        assert_eq!(view["latest_checkpoint"]["summary"], "第一章完成");
        assert_eq!(view["latest_checkpoint"]["details"], "下一步：寫第二章");
        assert!(view["blocked"].is_null());
        // 回報同時是存活訊號：lease 從最後一次回報重新計算
        assert_eq!(view["lease"]["expires_at"], CLOCK_START.as_millis() + 35_000 + LEASE_DURATION_MS);
    }
    assert_eq!(single["status"], "running");
    assert_eq!(task_status(&harness, &ws, &tid).await, "running");

    let reports = audited_reports(&harness, &rid);
    assert_eq!(reports.iter().map(|r| (r.0.as_str(), r.1)).collect::<Vec<_>>(), [("progress", true), ("progress", true), ("checkpoint", true)]);
    let events = event_types(&harness, &rid);
    assert!(events.iter().filter(|e| *e == "run_progress_reported").count() == 2 && events.contains(&"run_checkpoint_saved".to_string()));
}

#[tokio::test]
async fn a_blocked_report_shows_the_reason_and_moves_the_task_to_blocked() {
    let harness = harness().await;
    let (ws, tid) = workspace_with_ready_task(&harness, "main", "整理資料").await;
    let (rid, token) = start_run(&harness, &ws, &tid).await;

    let (status, ack) = harness.call_tool(&token, TOOL_REPORT_BLOCKED, json!({ "reason": "缺少資料來源的存取權" })).await;

    assert_eq!((status, ack["kind"].as_str()), (StatusCode::OK, Some("blocked")), "{ack}");
    let view = run_view(&harness, &ws, &rid).await;
    assert_eq!(view["blocked"]["reason"], "缺少資料來源的存取權");
    assert_eq!(view["status"], "running", "回報受阻不結束 Run，Worker 程序結束才結束");
    assert_eq!(task_status(&harness, &ws, &tid).await, "blocked");
}

#[tokio::test]
async fn invalid_reports_are_rejected_and_change_nothing() {
    let harness = harness().await;
    let (ws, tid) = workspace_with_ready_task(&harness, "main", "整理資料").await;
    let (rid, token) = start_run(&harness, &ws, &tid).await;
    let before = run_view(&harness, &ws, &rid).await;

    let too_long = "字".repeat(501);
    let cases = [
        (TOOL_REPORT_PROGRESS, json!({ "summary": "   " }), "report_invalid"),
        (TOOL_REPORT_PROGRESS, json!({ "summary": too_long }), "report_invalid"),
        (TOOL_SAVE_CHECKPOINT, json!({ "summary": "存檔", "details": "x".repeat(16 * 1024 + 1) }), "report_invalid"),
        (TOOL_REPORT_BLOCKED, json!({ "reason": "" }), "report_invalid"),
        (TOOL_REPORT_PROGRESS, json!({}), "bad_request"),
        (TOOL_REPORT_BLOCKED, json!({ "reason": 42 }), "bad_request"),
    ];
    for (tool, body, code) in cases {
        let (status, response) = harness.call_tool(&token, tool, body).await;
        assert_eq!((status, error_code(&response)), (StatusCode::BAD_REQUEST, code), "{tool}：{response}");
    }

    assert_eq!(run_view(&harness, &ws, &rid).await, before, "被拒絕的回報不改變任何狀態（含 lease）");
    assert!(audited_reports(&harness, &rid).is_empty());
    assert_eq!(task_status(&harness, &ws, &tid).await, "running");
}

// ============================================================ 資源歸屬

#[tokio::test]
async fn an_agent_cannot_reach_another_workspace_or_run() {
    let harness = harness().await;
    let (ws_a, task_a) = workspace_with_ready_task(&harness, "a", "A 的目標").await;
    let (ws_b, task_b) = workspace_with_ready_task(&harness, "b", "B 的目標：機密").await;
    let (run_a, token_a) = start_run(&harness, &ws_a, &task_a).await;
    let (run_b, _) = start_run(&harness, &ws_b, &task_b).await;
    let b_before = run_view(&harness, &ws_b, &run_b).await;

    // 自己的範圍照常可用；聲稱的歸屬與憑證一致不算違規
    let (status, own) = harness.call_tool(&token_a, TOOL_GET_TASK_CONTRACT, json!({ "workspace_id": ws_a, "run_id": run_a })).await;
    assert_eq!(status, StatusCode::OK, "{own}");
    assert_eq!(own["contract"]["objective"]["text"], "A 的目標");

    // 聲稱要讀寫別的 Workspace 或 Run：每個工具都被拒絕，而且不會改用請求裡的值
    let foreign_claims = [
        json!({ "workspace_id": ws_b, "run_id": run_b }),
        json!({ "run_id": run_b }),
        json!({ "workspace_id": ws_b }),
        json!({ "workspace_id": ws_a, "run_id": run_b }),
        json!({ "workspace_id": ws_b, "run_id": run_a }),
    ];
    for claims in &foreign_claims {
        for (tool, extra) in [
            (TOOL_GET_TASK_CONTRACT, json!({})),
            (TOOL_REPORT_PROGRESS, json!({ "summary": "寫進別人的 Run" })),
            (TOOL_SAVE_CHECKPOINT, json!({ "summary": "寫進別人的 Run", "details": "細節" })),
            (TOOL_REPORT_BLOCKED, json!({ "reason": "讓別人的 Task 受阻" })),
        ] {
            let mut body = extra.clone();
            body.as_object_mut().unwrap().extend(claims.as_object().unwrap().clone());
            let (status, response) = harness.call_tool(&token_a, tool, body).await;
            assert_eq!((status, error_code(&response)), (StatusCode::FORBIDDEN, "agent_scope_mismatch"), "{tool} {claims}：{response}");
            assert!(!response.to_string().contains("機密"), "回應不可洩漏別的 Run 的契約");
        }
    }

    // B 的一切都沒有被動到，A 也沒有留下任何回報
    assert_eq!(run_view(&harness, &ws_b, &run_b).await, b_before);
    assert_eq!(task_status(&harness, &ws_b, &task_b).await, "running");
    assert!(audited_reports(&harness, &run_b).is_empty());
    assert!(audited_reports(&harness, &run_a).is_empty());

    // 管理 API 上直接用路徑指名別人的資料，也一樣被拒絕（Agent 憑證根本不能呼叫管理 API）
    let (status, response) = harness
        .request(Method::GET, &run_path(&ws_b, &run_b))
        .bearer_auth(&token_a)
        .header(baton_api::CLIENT_HEADER, baton_api::CLIENT_HEADER_VALUE)
        .send()
        .await
        .map(|r| (r.status(), r))
        .unwrap();
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(error_code(&response.json::<Value>().await.unwrap()), "role_not_allowed");
}

// ============================================================ 角色分流

#[tokio::test]
async fn an_agent_credential_cannot_call_the_desktop_management_api() {
    let harness = harness().await;
    let (ws, tid) = workspace_with_ready_task(&harness, "main", "整理資料").await;
    let (rid, token) = start_run(&harness, &ws, &tid).await;
    let workspaces_before: Value = harness.get_json(WORKSPACES).await;

    let attempts = [
        (Method::GET, "/api/v1/health".to_string(), None),
        (Method::GET, "/api/v1/platform".to_string(), None),
        (Method::GET, "/api/v1/events".to_string(), None),
        (Method::GET, WORKSPACES.to_string(), None),
        (Method::POST, WORKSPACES.to_string(), Some(json!({ "path": harness.data_dir().to_string_lossy() }))),
        (Method::POST, format!("{WORKSPACES}/{ws}/archive"), None),
        (Method::GET, format!("{WORKSPACES}/{ws}/tasks"), None),
        (Method::POST, format!("{}/cancel", task_path(&ws, &tid)), None),
        (Method::POST, format!("{}/cancel", run_path(&ws, &rid)), None),
        (Method::PUT, format!("{WORKSPACES}/{ws}/authorization"), Some(json!({ "grants": [], "acknowledged_risk": true }))),
        (Method::GET, "/api/v1/no-such-route".to_string(), None),
    ];
    for (method, path, body) in attempts {
        let mut request = harness
            .request(method.clone(), &path)
            .bearer_auth(&token)
            .header(baton_api::CLIENT_HEADER, baton_api::CLIENT_HEADER_VALUE);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.unwrap();
        let status = response.status();
        let body: Value = response.json().await.unwrap();
        assert_eq!((status, error_code(&body)), (StatusCode::FORBIDDEN, "role_not_allowed"), "{method} {path}");
    }

    // 什麼都沒有被改變
    assert_eq!(harness.get_json::<Value>(WORKSPACES).await, workspaces_before);
    assert_eq!(run_view(&harness, &ws, &rid).await["status"], "running");
    assert!(run_view(&harness, &ws, &rid).await["cancel_requested_at"].is_null());
    assert_eq!(task_status(&harness, &ws, &tid).await, "running");
}

#[tokio::test]
async fn the_desktop_credential_cannot_call_the_agent_api() {
    let harness = harness().await;
    let (ws, tid) = workspace_with_ready_task(&harness, "main", "整理資料").await;
    let (rid, _) = start_run(&harness, &ws, &tid).await;
    let desktop = harness.desktop_credential();

    for tool in [TOOL_GET_TASK_CONTRACT, TOOL_REPORT_PROGRESS, TOOL_SAVE_CHECKPOINT, TOOL_REPORT_BLOCKED] {
        let (status, response) =
            harness.call_tool(&desktop, tool, json!({ "summary": "x", "reason": "x", "run_id": rid })).await;
        assert_eq!((status, error_code(&response)), (StatusCode::FORBIDDEN, "role_not_allowed"), "{tool}");
    }
    assert!(audited_reports(&harness, &rid).is_empty());
}

#[tokio::test]
async fn an_agent_credential_without_a_scope_or_an_unknown_token_is_rejected() {
    let harness = harness().await;
    let unscoped = harness.issue_credential(Role::Agent, None).await;

    let (status, response) = harness.call_tool(&unscoped, TOOL_GET_TASK_CONTRACT, json!({})).await;
    assert_eq!((status, error_code(&response)), (StatusCode::UNAUTHORIZED, "credential_invalid"), "{response}");

    let (status, response) = harness.call_tool(&"f".repeat(64), TOOL_GET_TASK_CONTRACT, json!({})).await;
    assert_eq!((status, error_code(&response)), (StatusCode::UNAUTHORIZED, "credential_invalid"), "{response}");
}

// ============================================================ 世代與憑證失效

#[tokio::test]
async fn reports_from_a_superseded_generation_are_kept_for_audit_but_change_nothing() {
    let harness = harness().await;
    let (ws, tid) = workspace_with_ready_task(&harness, "main", "整理資料").await;
    let (first, old_token) = start_run(&harness, &ws, &tid).await;
    let (status, _) = harness.call_tool(&old_token, TOOL_REPORT_PROGRESS, json!({ "summary": "第一世代的進度" })).await;
    assert_eq!(status, StatusCode::OK);

    // 第一個 Run 被取消並確認停止，Task 回到 ready，再派出第二個 Run（世代 2）
    let (status, _) = harness.send_json(Method::POST, &format!("{}/cancel", run_path(&ws, &first)), None).await;
    assert_eq!(status, StatusCode::OK);
    harness.supervise().await;
    assert_eq!(run_view(&harness, &ws, &first).await["status"], "cancelled");
    let (second, new_token) = start_run(&harness, &ws, &tid).await;
    assert_eq!(run_view(&harness, &ws, &second).await["generation"], 2);
    let second_before = run_view(&harness, &ws, &second).await;
    harness.clock.advance_millis(10_000);

    // 舊 Worker（程序其實還在）遲到的回報：全部被拒絕、內容保留為稽核、不改變目前狀態
    let late = [
        (TOOL_REPORT_PROGRESS, json!({ "summary": "遲到的進度" })),
        (TOOL_SAVE_CHECKPOINT, json!({ "summary": "遲到的 checkpoint", "details": "舊的細節" })),
        (TOOL_REPORT_BLOCKED, json!({ "reason": "遲到的受阻" })),
    ];
    for (tool, body) in late {
        let (status, response) = harness.call_tool(&old_token, tool, body).await;
        assert_eq!((status, error_code(&response)), (StatusCode::CONFLICT, "run_report_stale"), "{tool}：{response}");
    }

    let audit = audited_reports(&harness, &first);
    assert_eq!(audit.iter().map(|r| (r.0.as_str(), r.1)).collect::<Vec<_>>(), [("progress", true), ("progress", false), ("checkpoint", false), ("blocked", false)]);
    assert!(audit[1].2.contains("遲到的進度") && audit[2].2.contains("舊的細節") && audit[3].2.contains("遲到的受阻"));
    assert_eq!(run_view(&harness, &ws, &second).await, second_before, "目前的 Run（含 lease）完全沒有被遲到的回報影響");
    assert_eq!(task_status(&harness, &ws, &tid).await, "running", "舊世代的受阻回報不會讓目前的 Task 受阻");
    let first_view = run_view(&harness, &ws, &first).await;
    assert_eq!(first_view["latest_progress"]["summary"], "第一世代的進度", "稽核紀錄不會覆蓋面板上的最新進度");
    assert!(first_view["latest_checkpoint"].is_null() && first_view["blocked"].is_null());

    // 新世代的憑證照常可用，舊憑證也不能再讀契約
    let (status, _) = harness.call_tool(&new_token, TOOL_REPORT_PROGRESS, json!({ "summary": "第二世代的進度" })).await;
    assert_eq!(status, StatusCode::OK);
    let (status, response) = harness.call_tool(&old_token, TOOL_GET_TASK_CONTRACT, json!({})).await;
    assert_eq!((status, error_code(&response)), (StatusCode::UNAUTHORIZED, "credential_revoked"), "{response}");
}

#[tokio::test]
async fn the_credential_stops_working_when_the_run_ends() {
    let harness = harness().await;
    let (ws, tid) = workspace_with_ready_task(&harness, "main", "整理資料").await;
    harness.agent_adapter.script_next_run(vec![baton_ports::AdapterEvent::Exited { success: true }]);
    let (rid, token) = start_run(&harness, &ws, &tid).await;
    harness.supervise().await; // 收到 Adapter 的結束事件，Run 結束
    assert_eq!(run_view(&harness, &ws, &rid).await["status"], "succeeded");
    let ended = run_view(&harness, &ws, &rid).await;

    let (status, response) = harness.call_tool(&token, TOOL_GET_TASK_CONTRACT, json!({})).await;
    assert_eq!((status, error_code(&response)), (StatusCode::UNAUTHORIZED, "credential_revoked"), "{response}");
    let (status, response) = harness.call_tool(&token, TOOL_REPORT_BLOCKED, json!({ "reason": "結束後才說" })).await;
    assert_eq!((status, error_code(&response)), (StatusCode::CONFLICT, "run_report_stale"), "{response}");

    assert_eq!(run_view(&harness, &ws, &rid).await, ended);
    assert_eq!(task_status(&harness, &ws, &tid).await, "verifying", "結束後的回報不會改變 Task 狀態");
    assert_eq!(audited_reports(&harness, &rid).iter().map(|r| r.1).collect::<Vec<_>>(), [false]);
}

#[tokio::test]
async fn the_credential_survives_a_runtime_restart_because_the_worker_process_does() {
    let mut harness = harness().await;
    let (ws, tid) = workspace_with_ready_task(&harness, "main", "整理資料").await;
    let (rid, token) = start_run(&harness, &ws, &tid).await;

    harness.restart().await;
    let (status, ack) = harness.call_tool(&token, TOOL_REPORT_PROGRESS, json!({ "summary": "重啟之後還在做" })).await;

    assert_eq!(status, StatusCode::OK, "{ack}");
    assert_eq!(run_view(&harness, &ws, &rid).await["latest_progress"]["summary"], "重啟之後還在做");
}

// ============================================================ MCP shim（Adapter 契約：Worker 透過 shim 回報）

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

#[tokio::test]
async fn a_worker_reports_through_the_mcp_shim_and_the_ui_can_see_it() {
    let harness = harness().await;
    let (ws, tid) = workspace_with_ready_task(&harness, "main", "整理資料").await;
    let (rid, token) = start_run(&harness, &ws, &tid).await;

    let responses = talk_to_shim(
        &harness,
        &token,
        &[
            json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "claude-code", "version": "0" } } }),
            json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
            json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
            tool_call(3, TOOL_GET_TASK_CONTRACT, json!({})),
            tool_call(4, TOOL_REPORT_PROGRESS, json!({ "summary": "透過 shim 回報的進度" })),
            tool_call(5, TOOL_SAVE_CHECKPOINT, json!({ "summary": "透過 shim 存的 checkpoint", "details": "接手者請從第二章開始" })),
            tool_call(6, TOOL_REPORT_BLOCKED, json!({ "reason": "" })),
        ],
    )
    .await;

    assert_eq!(responses.len(), 6, "通知不產生回應");
    let tools: Vec<&str> = responses[1]["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    for tool in [TOOL_GET_TASK_CONTRACT, TOOL_REPORT_PROGRESS, TOOL_SAVE_CHECKPOINT, TOOL_REPORT_BLOCKED] {
        assert!(tools.contains(&tool), "{tool}");
    }
    let contract_text = responses[2]["result"]["content"][0]["text"].as_str().unwrap();
    assert!(contract_text.contains("整理資料") && responses[2]["result"]["isError"] == false);
    assert_eq!(responses[3]["result"]["isError"], false);
    assert_eq!(responses[4]["result"]["isError"], false);
    assert_eq!(responses[5]["result"]["isError"], true, "Runtime 拒絕時是工具錯誤，不是協定錯誤");
    assert!(responses[5]["result"]["content"][0]["text"].as_str().unwrap().contains("report_invalid"));
    assert!(!responses.iter().any(|r| r.to_string().contains(&token)), "shim 的輸出不含憑證");

    // UI 看得到：面板的 Run 帶著最新進度與 checkpoint，Task 沒有因為被拒絕的回報而受阻
    let board: Value = harness.get_json(&format!("{WORKSPACES}/{ws}/tasks")).await;
    let run = board["runs"].as_array().unwrap().iter().find(|r| r["id"] == rid.as_str()).unwrap();
    assert_eq!(run["latest_progress"]["summary"], "透過 shim 回報的進度");
    assert_eq!(run["latest_checkpoint"]["summary"], "透過 shim 存的 checkpoint");
    assert_eq!(run["latest_checkpoint"]["details"], "接手者請從第二章開始");
    assert!(run["blocked"].is_null());
    assert_eq!(task_status(&harness, &ws, &tid).await, "running");
    assert!(!harness.log_text().contains(&token));
}
