//! 情境測試：Context Builder 與 context receipt（票 39，spec D-07、D-27、使用者故事 61），經由 Runtime harness 的真實 HTTP API 驗證。
//!
//! 派工時 Context Builder 依相關性與 token 預算組裝 Worker 的 Prompt（Task 契約、交接說明或最近檢查點、適用決策、
//! 相關教訓、未讀訊息摘要、證據索引），並保存 context receipt。「Task 開始前被載入」以 fake AgentAdapter 收到的
//! 啟動 Prompt 觀察；receipt 以 Run 詳情的 context 端點觀察（桌面憑證）。

use baton_domain::agent::{TOOL_SAVE_CHECKPOINT, TOOL_SEND_MESSAGE};
use baton_domain::artifact::{sha256_hex, TOOL_REGISTER_ARTIFACT};
use baton_domain::context::DEFAULT_BUDGET_TOKENS;
use baton_domain::knowledge::TOOL_RECORD_LESSON_CANDIDATE;
use baton_domain::Timestamp;
use baton_harness::Harness;
use baton_ports::AdapterEvent;
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

const WORKSPACES: &str = "/api/v1/workspaces";
const KNOWLEDGE: &str = "/api/v1/knowledge";
const CLOCK_START: Timestamp = Timestamp::from_millis(1_700_000_000_000);

// --- 共用輔助 ---

async fn harness() -> Harness {
    let harness = Harness::builder().clock_at(CLOCK_START).start().await;
    // 派工需要一個可派工的帳號（票 19）
    harness.add_account("主要帳號", "main@example.com").await;
    harness
}

/// 建立 Workspace 與 Goal，回傳 (workspace_id, goal_id)。
async fn setup(harness: &Harness) -> (String, String) {
    let dir = harness.data_dir().join("projects").join("app");
    std::fs::create_dir_all(&dir).unwrap();
    let (status, ws) = harness.send_json(Method::POST, WORKSPACES, Some(json!({ "path": dir.to_string_lossy() }))).await;
    assert_eq!(status, StatusCode::CREATED, "{ws}");
    let ws = ws["id"].as_str().unwrap().to_string();
    let (status, goal) =
        harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/goals"), Some(json!({ "title": "上線" }))).await;
    assert_eq!(status, StatusCode::CREATED, "{goal}");
    (ws, goal["id"].as_str().unwrap().to_string())
}

fn filled(text: &str) -> Value {
    json!({ "status": "filled", "text": text })
}

/// 建立並標記 ready 的 Task，回傳 task_id。
async fn ready_task(harness: &Harness, ws: &str, goal: &str, title: &str) -> String {
    let contract = json!({
        "objective": filled("部署新版"),
        "inputs": filled("既有設定"),
        "scope": filled("正式環境"),
        "deliverables": filled("新版上線"),
        "done_criteria": filled("新版上線且可回滾"),
        "blocker_handling": filled("卡住時回報"),
        "required_capabilities": [],
    });
    let body = json!({ "goal_id": goal, "title": title, "contract": contract });
    let (status, task) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks"), Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    let tid = task["id"].as_str().unwrap().to_string();
    let (status, ready) =
        harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks/{tid}/ready"), None).await;
    assert_eq!(status, StatusCode::OK, "{ready}");
    tid
}

/// 派工並讓 Supervisor 啟動它，回傳 (run_id, Worker 的 Agent 憑證)。
async fn start_run(harness: &Harness, ws: &str, tid: &str) -> (String, String) {
    let (status, run) =
        harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks/{tid}/dispatch"), None).await;
    assert_eq!(status, StatusCode::CREATED, "{run}");
    let rid = run["id"].as_str().unwrap().to_string();
    harness.supervise().await;
    let token = harness.agent_credential(&rid).expect("Run 啟動時應該把 Agent 憑證放進程序的環境變數");
    (rid, token)
}

/// Worker 程序啟動時實際收到的 Prompt。
fn launched_prompt(harness: &Harness, rid: &str) -> String {
    harness.agent_adapter.launched().into_iter().find(|spec| spec.run_id == rid).expect("Run 已啟動").prompt
}

/// Run 詳情的 context receipt 與 Prompt。
async fn run_context(harness: &Harness, ws: &str, rid: &str) -> (Value, String) {
    let (status, body) = harness.send_json(Method::GET, &format!("{WORKSPACES}/{ws}/runs/{rid}/context"), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    (body["receipt"].clone(), body["prompt"].as_str().unwrap().to_string())
}

fn items<'a>(receipt: &'a Value, status: &str) -> Vec<&'a Value> {
    receipt["items"].as_array().unwrap().iter().filter(|i| i["status"] == status).collect()
}

fn ref_ids(items: &[&Value]) -> Vec<String> {
    items.iter().map(|i| i["ref_id"].as_str().unwrap().to_string()).collect()
}

/// 使用者在知識分頁建立的教訓（直接是已驗證），回傳條目。
async fn lesson(harness: &Harness, scope: Value, title: &str, resolution: &str) -> Value {
    let body = json!({
        "kind": "lesson",
        "scope": scope,
        "title": title,
        "lesson": { "trigger": "部署", "root_cause": "沒有退路", "resolution": resolution },
    });
    let (status, entry) = harness.send_json(Method::POST, KNOWLEDGE, Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{entry}");
    entry
}

fn workspace_scope(ws: &str) -> Value {
    json!({ "kind": "workspace", "workspace_id": ws })
}

fn task_scope(ws: &str, tid: &str) -> Value {
    json!({ "kind": "task", "workspace_id": ws, "task_id": tid })
}

async fn knowledge_action(harness: &Harness, entry: &Value, action: &str, body: Value) -> Value {
    let id = entry["id"].as_str().unwrap();
    let (status, updated) = harness.send_json(Method::POST, &format!("{KNOWLEDGE}/{id}/{action}"), Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    updated
}

/// receipt 與實際載入的內容一致：API 的 Prompt 就是 Worker 收到的 Prompt、雜湊相符；
/// 每個載入的知識條目以它記錄的版本出現在 Prompt 中，省略的不出現。
fn assert_receipt_matches(receipt: &Value, prompt: &str, launched: &str) {
    assert_eq!(prompt, launched, "Run 詳情的 Prompt 就是 Worker 啟動時收到的 Prompt");
    assert_eq!(receipt["prompt_sha256"].as_str().unwrap(), sha256_hex(prompt.as_bytes()));
    for item in receipt["items"].as_array().unwrap() {
        if !matches!(item["kind"].as_str().unwrap(), "decision" | "lesson") {
            continue;
        }
        let marker = format!("〔{} v{}〕{}", item["ref_id"].as_str().unwrap(), item["version"], item["title"].as_str().unwrap());
        assert_eq!(prompt.contains(&marker), item["status"] == "loaded", "{marker}：{item}");
    }
}

// --- 情境 ---

#[tokio::test]
async fn verified_lessons_that_apply_are_loaded_before_the_task_starts_and_candidates_are_not() {
    let harness = harness().await;
    // 這個情境同時有三個 Run，每個帳號最多兩個（spec D-14）
    harness.add_account("第二帳號", "second@example.com").await;
    let (ws, goal) = setup(&harness).await;
    let verified = lesson(&harness, workspace_scope(&ws), "部署前要寫回滾計畫", "先寫回滾步驟").await;
    let revoked = lesson(&harness, workspace_scope(&ws), "部署要關掉監控", "關掉告警").await;
    knowledge_action(&harness, &revoked, "revoke", json!({ "expected_revision": revoked["revision"], "reason": "錯的" })).await;

    // 另一個 Task 的 Worker 提交候選教訓：未經使用者確認不會被載入
    let other = ready_task(&harness, &ws, &goal, "盤點設定").await;
    let (_, token) = start_run(&harness, &ws, &other).await;
    let candidate = json!({ "title": "部署前先備份資料庫", "resolution": "備份", "scope": "workspace" });
    let (status, recorded) = harness.call_tool(&token, TOOL_RECORD_LESSON_CANDIDATE, candidate).await;
    assert!(status.is_success(), "{recorded}");
    let other_task_lesson = lesson(&harness, task_scope(&ws, &other), "盤點只看正式環境", "只看正式").await;

    let tid = ready_task(&harness, &ws, &goal, "部署").await;
    let (rid, _) = start_run(&harness, &ws, &tid).await;

    let prompt = launched_prompt(&harness, &rid);
    assert!(prompt.contains("部署前要寫回滾計畫") && prompt.contains("先寫回滾步驟"), "{prompt}");
    for absent in ["部署要關掉監控", "部署前先備份資料庫", "盤點只看正式環境"] {
        assert!(!prompt.contains(absent), "{absent} 不應被載入：{prompt}");
    }
    let (receipt, api_prompt) = run_context(&harness, &ws, &rid).await;
    assert_eq!(receipt["run_id"], rid.as_str());
    assert_eq!(ref_ids(&items(&receipt, "loaded")), [tid.clone(), verified["id"].as_str().unwrap().to_string()]);
    assert!(items(&receipt, "omitted").is_empty(), "不適用的條目不是被省略：{receipt}");
    assert!(!receipt.to_string().contains(other_task_lesson["id"].as_str().unwrap()));
    assert_receipt_matches(&receipt, &api_prompt, &prompt);

    // 使用者確認候選後，之後開始的 Task 才會載入它
    assert_eq!(recorded["status"], "candidate", "{recorded}");
    let candidate = json!({ "id": recorded["entry_id"], "revision": recorded["revision"] });
    knowledge_action(&harness, &candidate, "confirm", json!({ "expected_revision": recorded["revision"] })).await;
    let next = ready_task(&harness, &ws, &goal, "部署第二版").await;
    let (next_rid, _) = start_run(&harness, &ws, &next).await;
    assert!(launched_prompt(&harness, &next_rid).contains("部署前先備份資料庫"));
    assert!(!launched_prompt(&harness, &rid).contains("部署前先備份資料庫"), "已啟動的 Run 的 Prompt 不變");
}

#[tokio::test]
async fn items_left_out_for_lack_of_budget_are_recorded_in_the_receipt() {
    let harness = harness().await;
    let (ws, goal) = setup(&harness).await;
    let tid = ready_task(&harness, &ws, &goal, "部署").await;
    // 每則大教訓約 2,000 token：Task 範圍的最相關、優先放入；三則 Workspace 範圍的依新到舊只放得進兩則
    let big = |c: char| c.to_string().repeat(2_000);
    let task_lesson = lesson(&harness, task_scope(&ws, &tid), "這個 Task 的關鍵教訓", &big('甲')).await;
    let older = lesson(&harness, workspace_scope(&ws), "最舊的 Workspace 教訓", &big('乙')).await;
    harness.clock.advance_millis(1_000);
    let middle = lesson(&harness, workspace_scope(&ws), "中間的 Workspace 教訓", &big('丙')).await;
    harness.clock.advance_millis(1_000);
    let newer = lesson(&harness, workspace_scope(&ws), "最新的 Workspace 教訓", &big('丁')).await;
    harness.clock.advance_millis(1_000);
    let small = lesson(&harness, workspace_scope(&ws), "小教訓", "短").await;

    let (rid, _) = start_run(&harness, &ws, &tid).await;
    let (receipt, prompt) = run_context(&harness, &ws, &rid).await;
    let id = |e: &Value| e["id"].as_str().unwrap().to_string();
    assert_eq!(ref_ids(&items(&receipt, "loaded")), [tid.clone(), id(&task_lesson), id(&small), id(&newer), id(&middle)]);
    let omitted = items(&receipt, "omitted");
    assert_eq!(ref_ids(&omitted), [id(&older)]);
    assert_eq!(omitted[0]["omitted_reason"], "budget_exceeded");
    assert_eq!(omitted[0]["kind"], "lesson");
    assert_eq!(omitted[0]["version"], 1);
    assert!(omitted[0]["tokens"].as_u64().unwrap() > 2_000);
    assert_eq!(receipt["budget_tokens"].as_u64(), Some(DEFAULT_BUDGET_TOKENS as u64));
    assert!(receipt["used_tokens"].as_u64().unwrap() <= DEFAULT_BUDGET_TOKENS as u64, "{}", receipt["used_tokens"]);
    assert!(!prompt.contains("最舊的 Workspace 教訓"));
    assert_receipt_matches(&receipt, &prompt, &launched_prompt(&harness, &rid));
}

#[tokio::test]
async fn the_receipt_keeps_the_versions_that_were_actually_loaded() {
    let harness = harness().await;
    let (ws, goal) = setup(&harness).await;
    let entry = lesson(&harness, workspace_scope(&ws), "部署前要寫回滾計畫", "第一版做法").await;
    let correct = |entry: &Value, resolution: &str| {
        json!({
            "expected_revision": entry["revision"],
            "title": "部署前要寫回滾計畫",
            "lesson": { "trigger": "部署", "root_cause": "沒有退路", "resolution": resolution },
            "note": "修正做法",
        })
    };
    let v2 = knowledge_action(&harness, &entry, "correct", correct(&entry, "第二版做法")).await;
    assert_eq!(v2["version"], 2, "{v2}");

    let first = ready_task(&harness, &ws, &goal, "部署").await;
    let (first_rid, _) = start_run(&harness, &ws, &first).await;
    knowledge_action(&harness, &v2, "correct", correct(&v2, "第三版做法")).await;
    let second = ready_task(&harness, &ws, &goal, "部署第二版").await;
    let (second_rid, _) = start_run(&harness, &ws, &second).await;

    let version_of = |receipt: &Value| {
        receipt["items"].as_array().unwrap().iter().find(|i| i["ref_id"] == entry["id"]).unwrap()["version"].clone()
    };
    let (receipt, prompt) = run_context(&harness, &ws, &first_rid).await;
    assert_eq!(version_of(&receipt), 2, "之後的修正不改變已保存的 receipt");
    assert!(prompt.contains("第二版做法") && !prompt.contains("第一版做法") && !prompt.contains("第三版做法"), "{prompt}");
    assert_receipt_matches(&receipt, &prompt, &launched_prompt(&harness, &first_rid));
    let contract = &receipt["items"][0];
    assert_eq!((contract["kind"].as_str(), contract["ref_id"].as_str(), contract["version"].as_i64()), (Some("contract"), Some(first.as_str()), Some(1)));

    let (receipt, prompt) = run_context(&harness, &ws, &second_rid).await;
    assert_eq!(version_of(&receipt), 3);
    assert!(prompt.contains("第三版做法") && !prompt.contains("第二版做法"));
    assert_receipt_matches(&receipt, &prompt, &launched_prompt(&harness, &second_rid));

    // 重啟 Runtime 後 receipt 仍在、內容不變
    let mut harness = harness;
    harness.restart().await;
    assert_eq!(run_context(&harness, &ws, &first_rid).await, run_context(&harness, &ws, &first_rid).await);
    assert_eq!(version_of(&run_context(&harness, &ws, &first_rid).await.0), 2);
}

#[tokio::test]
async fn a_retried_run_gets_the_latest_checkpoint_unread_messages_and_an_index_of_registered_evidence() {
    let harness = harness().await;
    let (ws, goal) = setup(&harness).await;
    let tid = ready_task(&harness, &ws, &goal, "部署").await;
    let sender = ready_task(&harness, &ws, &goal, "後端").await;

    let (first, token) = start_run(&harness, &ws, &tid).await;
    let first_prompt = launched_prompt(&harness, &first);
    assert!(!first_prompt.contains("最近的檢查點") && !first_prompt.contains("證據索引"), "第一次沒有可延續的東西");
    let (status, body) =
        harness.call_tool(&token, TOOL_SAVE_CHECKPOINT, json!({ "summary": "完成步驟二", "details": "設定已改好" })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, artifact) = harness
        .call_tool(&token, TOOL_REGISTER_ARTIFACT, json!({ "name": "plan.md", "description": "回滾計畫", "content": "步驟" }))
        .await;
    assert!(status.is_success(), "{artifact}");
    harness.agent_adapter.emit(&first, AdapterEvent::Exited { success: false });
    harness.supervise().await;

    let (_, sender_token) = start_run(&harness, &ws, &sender).await;
    let (status, sent) =
        harness.call_tool(&sender_token, TOOL_SEND_MESSAGE, json!({ "to": [tid], "subject": "介面改了", "body": "細節" })).await;
    assert!(status.is_success(), "{sent}");

    let (second, _) = start_run(&harness, &ws, &tid).await;
    let prompt = launched_prompt(&harness, &second);
    let at = |text: &str| prompt.find(text).unwrap_or_else(|| panic!("缺少「{text}」：{prompt}"));
    assert!(at("完成步驟二") < at("未讀訊息：1 則") && at("未讀訊息：1 則") < at("證據索引"));
    assert!(prompt.contains("plan.md") && prompt.contains("回滾計畫"));

    let (receipt, api_prompt) = run_context(&harness, &ws, &second).await;
    let kinds: Vec<&str> = items(&receipt, "loaded").iter().map(|i| i["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["contract", "checkpoint", "messages", "evidence"]);
    let checkpoint = items(&receipt, "loaded")[1];
    assert_eq!((checkpoint["ref_id"].as_str(), checkpoint["version"].as_i64()), (Some(first.as_str()), Some(1)));
    let evidence = items(&receipt, "loaded")[3];
    assert_eq!(evidence["version"], 1);
    assert_receipt_matches(&receipt, &api_prompt, &prompt);
}

#[tokio::test]
async fn the_context_endpoint_rejects_unknown_runs() {
    let harness = harness().await;
    let (ws, _) = setup(&harness).await;
    let (status, body) = harness.send_json(Method::GET, &format!("{WORKSPACES}/{ws}/runs/run_missing/context"), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["error"], "run_not_found");
}
