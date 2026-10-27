//! 情境測試：Controller 憑證可以使用的共同工具（spec D-05，票 17／AI-327，最終 review #16），經由 Runtime harness 的真實 HTTP API 驗證。
//!
//! Controller 是腳本化的：測試開啟 Controller CLI，以 Controller 終端機收到的 Controller 憑證呼叫 Agent API。
//! - `search_knowledge`：Controller 規劃整個 Workspace 的工作，看得到全域與這個 Workspace 內所有範圍的有效條目，
//!   看不到別的 Workspace；聲稱別的 Workspace 被拒絕；Controller CLI 結束後憑證失效。
//! - `send_message`、`read_inbox` 的情境在 `controller_messages.rs`；`record_lesson_candidate` 在 `controller_lesson_candidate.rs`（票 54）。

use baton_domain::knowledge::TOOL_SEARCH_KNOWLEDGE;
use baton_domain::Timestamp;
use baton_harness::Harness;
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

const WORKSPACES: &str = "/api/v1/workspaces";
const KNOWLEDGE: &str = "/api/v1/knowledge";
const CLOCK_START: Timestamp = Timestamp::from_millis(1_700_000_000_123);

fn filled(text: &str) -> Value {
    json!({ "status": "filled", "text": text })
}

fn error_code(body: &Value) -> &str {
    body["error"].as_str().unwrap_or_default()
}

async fn new_workspace(harness: &Harness, name: &str) -> String {
    let dir = harness.data_dir().join("projects").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let (status, body) = harness.send_json(Method::POST, WORKSPACES, Some(json!({ "path": dir.to_string_lossy() }))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["id"].as_str().unwrap().to_string()
}

/// 在 Workspace 內建立 Goal 與 Task，回傳 (goal_id, task_id)。
async fn task(harness: &Harness, ws: &str) -> (String, String) {
    let (status, goal) =
        harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/goals"), Some(json!({ "title": "目標" }))).await;
    assert_eq!(status, StatusCode::CREATED, "{goal}");
    let contract = json!({
        "objective": filled("整理資料"),
        "inputs": filled("既有的資料"),
        "scope": filled("公開資料"),
        "deliverables": filled("比較表"),
        "done_criteria": filled("五個產品"),
        "blocker_handling": filled("卡住時回報"),
        "required_capabilities": [],
    });
    let body = json!({ "goal_id": goal["id"], "title": "整理", "contract": contract });
    let (status, task) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks"), Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    (goal["id"].as_str().unwrap().to_string(), task["id"].as_str().unwrap().to_string())
}

fn titles(body: &Value) -> Vec<String> {
    let mut titles: Vec<String> =
        body["entries"].as_array().unwrap().iter().map(|e| e["content"]["title"].as_str().unwrap().to_string()).collect();
    titles.sort();
    titles
}

#[tokio::test]
async fn the_controller_searches_effective_knowledge_of_its_own_workspace_only() {
    let harness = Harness::builder().clock_at(CLOCK_START).start().await;
    harness.add_account("帳號 1", "account1@example.com").await;
    let controller = harness.add_account("Controller 帳號", "controller@example.com").await;
    let ws = new_workspace(&harness, "main").await;
    let other = new_workspace(&harness, "other").await;
    let (goal, tid) = task(&harness, &ws).await;
    let scoped = |scope: Value, title: &str| json!({ "kind": "fact", "scope": scope, "title": title });
    for body in [
        scoped(json!({ "kind": "global" }), "全域：提交訊息用繁體中文"),
        scoped(json!({ "kind": "workspace", "workspace_id": ws }), "本工作區：提交訊息附票號"),
        scoped(json!({ "kind": "goal", "workspace_id": ws, "goal_id": goal }), "本目標：提交訊息附目標"),
        scoped(json!({ "kind": "task", "workspace_id": ws, "task_id": tid }), "本工作：提交訊息附作者"),
        scoped(json!({ "kind": "workspace", "workspace_id": other }), "別的工作區：提交訊息不限"),
    ] {
        let (status, entry) = harness.send_json(Method::POST, KNOWLEDGE, Some(body)).await;
        assert_eq!(status, StatusCode::CREATED, "{entry}");
    }
    let (_, token) = harness.open_controller_cli(&ws, &controller).await;

    // 全域加上這個 Workspace 內所有範圍；別的 Workspace 的條目看不到
    let (status, body) = harness.call_tool(&token, TOOL_SEARCH_KNOWLEDGE, json!({ "query": "提交訊息" })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let mut expected = ["全域：提交訊息用繁體中文", "本工作區：提交訊息附票號", "本目標：提交訊息附目標", "本工作：提交訊息附作者"];
    expected.sort();
    assert_eq!(titles(&body), expected);

    // 不能越權：聲稱別的 Workspace 被拒絕，不會改用請求裡的值
    let (status, body) =
        harness.call_tool(&token, TOOL_SEARCH_KNOWLEDGE, json!({ "query": "提交訊息", "workspace_id": other })).await;
    assert_eq!((status, error_code(&body)), (StatusCode::FORBIDDEN, "agent_scope_mismatch"), "{body}");

    // Controller CLI 結束後憑證失效
    harness.end_controller_cli(&ws).await;
    let (status, body) = harness.call_tool(&token, TOOL_SEARCH_KNOWLEDGE, json!({ "query": "" })).await;
    assert_eq!((status, error_code(&body)), (StatusCode::UNAUTHORIZED, "credential_revoked"), "{body}");
}
