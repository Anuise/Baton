//! UI E2E「裁定中心批次處理」（spec Testing Decisions 3，票 53）。
//!
//! 前置（Workspace、授權、Task、派工）直接以 API 建立，「派工到完成」旅程已在 UI 涵蓋；
//! 裁定請求由 scripted fake Worker／Controller 以 Agent 憑證呼叫 Agent 工具產生（真實產品只能這樣建立）；
//! 裁定的處理一律經 Web UI 的裁定中心。時間全由 fake Clock 決定，等待只靠瀏覽器的等待。

use baton_domain::agent::{TOOL_CONTROLLER_ACTION, TOOL_PERMISSION_PROMPT, TOOL_REQUEST_DECISION};
use baton_harness::Harness;
use baton_ports::AdapterEvent;
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

use crate::browser::{nth, role, text, Browser};
use crate::UiE2e;

const WORKSPACES: &str = "/api/v1/workspaces";
const PENDING: &str = "/api/v1/decisions/pending";
/// 範圍外的遠端寫入（「可信專案開發」範本沒有遠端寫入），每次都要裁定。
const PUSH: &str = "git push origin main";
const FORCE_PUSH: &str = "git push --force origin main";
/// 決策權限表內建「必須送裁定」的事項。
const MATTER: &str = "modify_done_criteria";

// --- API 前置與 fake Worker／Controller（沿用 decision_center_full.rs 的寫法） ---

async fn api(harness: &Harness, method: Method, path: &str, body: Option<Value>, expected: StatusCode) -> Value {
    let (status, response) = harness.send_json(method.clone(), path, body).await;
    assert_eq!(status, expected, "{method} {path}：{response}");
    response
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

/// 建立 Workspace 並確認「可信專案開發」授權，回傳 (識別, 名稱)。
async fn workspace_with_dev_authorization(harness: &Harness) -> (String, String) {
    let dir = harness.data_dir().join("projects").join("demo");
    std::fs::create_dir_all(&dir).unwrap();
    let created =
        api(harness, Method::POST, WORKSPACES, Some(json!({ "path": dir.to_string_lossy() })), StatusCode::CREATED).await;
    let ws = created["id"].as_str().unwrap().to_string();
    let authorization = format!("{WORKSPACES}/{ws}/authorization");
    let view = api(harness, Method::GET, &authorization, None, StatusCode::OK).await;
    let template = view["templates"].as_array().unwrap().iter().find(|t| t["id"] == "trusted_dev_p3").unwrap().clone();
    let body = json!({ "template_id": "trusted_dev_p3", "grants": template["grants"], "acknowledged_risk": true });
    api(harness, Method::PUT, &authorization, Some(body), StatusCode::CREATED).await;
    (ws, created["name"].as_str().unwrap().to_string())
}

/// 建立一個 ready 的 Task、派工並讓 Supervisor 啟動它，回傳 (task_id, run_id, Worker 的 Agent 憑證)。
async fn running_task(harness: &Harness, ws: &str, goal: &str, title: &str) -> (String, String, String) {
    let body = json!({ "goal_id": goal, "title": title, "contract": contract(title) });
    let task = api(harness, Method::POST, &format!("{WORKSPACES}/{ws}/tasks"), Some(body), StatusCode::CREATED).await;
    let tid = task["id"].as_str().unwrap().to_string();
    api(harness, Method::POST, &format!("{}/ready", task_path(ws, &tid)), None, StatusCode::OK).await;
    let run = api(harness, Method::POST, &format!("{}/dispatch", task_path(ws, &tid)), None, StatusCode::CREATED).await;
    let rid = run["id"].as_str().unwrap().to_string();
    harness.supervise().await;
    let token = harness.agent_credential(&rid).expect("Run 啟動時應該把 Agent 憑證放進程序的環境變數");
    (tid, rid, token)
}

async fn prompt(harness: &Harness, token: &str, command: &str) -> Value {
    let input = json!({ "tool_name": "Bash", "input": { "command": command } });
    let (status, response) = harness.call_tool(token, TOOL_PERMISSION_PROMPT, input).await;
    assert_eq!(status, StatusCode::OK, "{response}");
    response
}

/// fake Worker 的權限提示被攔下送裁定，回傳裁定請求識別。
async fn prompt_waiting(harness: &Harness, token: &str, command: &str) -> String {
    let response = prompt(harness, token, command).await;
    assert_eq!(response["behavior"], "deny", "{response}");
    assert_eq!(response["status"], "waiting_decision", "{response}");
    response["decision_request_id"].as_str().unwrap().to_string()
}

/// fake Worker 以 `request_decision` 問使用者一個問題，回傳裁定請求識別。
async fn ask(harness: &Harness, token: &str, question: &str) -> String {
    let body = json!({
        "question": question,
        "reason": "兩個方案的取捨取決於使用者價值，不是我能決定的",
        "options": [
            { "id": "ship", "label": "先出貨", "tradeoff": "快，但缺少報表" },
            { "id": "wait", "label": "等報表完成", "tradeoff": "晚一週" },
        ],
        "recommendation": "wait",
        "evidence": ["報表目前完成 60%"],
        "urgency": "high",
    });
    let (status, response) = harness.call_tool(token, TOOL_REQUEST_DECISION, body).await;
    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(response["status"], "waiting_decision", "{response}");
    response["decision_request_id"].as_str().unwrap().to_string()
}

/// fake Controller 想自行修改完成條件（權限表規定必須送裁定），回傳工具回應。
async fn controller(harness: &Harness, token: &str) -> Value {
    let body = json!({
        "matter": MATTER,
        "summary": "把完成條件從五個產品改成三個產品",
        "reason": "第五個產品的公開資料不足",
        "evidence": ["只找到三個產品的價格表"],
        "urgency": "high",
    });
    let (status, response) = harness.call_tool(token, TOOL_CONTROLLER_ACTION, body).await;
    assert_eq!(status, StatusCode::OK, "{response}");
    response
}

async fn controller_waiting(harness: &Harness, token: &str) -> String {
    let response = controller(harness, token).await;
    assert_eq!(response["status"], "requires_decision", "{response}");
    response["decision_request_id"].as_str().unwrap().to_string()
}

/// Worker 結束（依裁定中心的約定：保存 checkpoint、report_blocked 之後結束）。
async fn worker_exits(harness: &Harness, rid: &str) {
    harness.agent_adapter.emit(rid, AdapterEvent::Exited { success: true });
    harness.supervise().await;
}

async fn decision(harness: &Harness, ws: &str, rid: &str) -> Value {
    api(harness, Method::GET, &format!("{WORKSPACES}/{ws}/decisions/{rid}"), None, StatusCode::OK).await
}

async fn task_status(harness: &Harness, ws: &str, tid: &str) -> String {
    let detail = api(harness, Method::GET, &task_path(ws, tid), None, StatusCode::OK).await;
    detail["task"]["status"].as_str().unwrap().to_string()
}

async fn pending_count(harness: &Harness) -> i64 {
    api(harness, Method::GET, PENDING, None, StatusCode::OK).await["pending_count"].as_i64().unwrap()
}

/// 某個 Task 目前執行中的 Run 與它的 Agent 憑證。
async fn running_run(harness: &Harness, ws: &str, tid: &str) -> (String, String) {
    let list = api(harness, Method::GET, &format!("{WORKSPACES}/{ws}/runs"), None, StatusCode::OK).await;
    let runs: Vec<&Value> = list["runs"].as_array().unwrap().iter().filter(|r| r["task_id"] == tid).collect();
    let run = runs.iter().rev().find(|r| r["status"] == "running").unwrap_or_else(|| panic!("{tid} 應該已重新派工：{runs:?}"));
    let run_id = run["id"].as_str().unwrap().to_string();
    let token = harness.agent_credential(&run_id).expect("新的 Run 應該有 Agent 憑證");
    (run_id, token)
}

/// 畫面上的選項文字：建議的選項後面加「（建議）」（與 DecisionCenter 的呈現相同）。
fn option_label(view: &Value, option_id: &str) -> String {
    let option = view["options"].as_array().unwrap().iter().find(|o| o["id"] == option_id).unwrap_or_else(|| panic!("{view}"));
    let label = option["label"].as_str().unwrap();
    if view["recommendation"] == option_id {
        format!("{label}（建議）")
    } else {
        label.to_string()
    }
}

// --- UI 操作 ---

fn center() -> Value {
    role("region", "裁定中心")
}

/// Workspace 功能頁簽。
fn tab(name: &str) -> [Value; 2] {
    [role("tablist", "Workspace 功能"), role("tab", name)]
}

/// 在側欄選擇 Workspace（已選中時再點一次無害），切到「裁定」頁簽開啟裁定中心（從別的頁簽切過來時會重新讀取清單）。
async fn open_center(browser: &Browser, workspace: &str) {
    browser.click(&[role("button", &format!("選擇 {workspace}"))]).await;
    browser.click(&tab("裁定")).await;
    browser.wait_visible(&[center(), role("heading", &format!("裁定：{workspace}"))]).await;
}

/// 批次面板中的一項：以問題（fieldset 的 legend）找到它；問題相同時以 `index` 區分。
fn batch_item(question: &str, index: usize) -> Vec<Value> {
    vec![center(), role("form", "批次處理"), role("group", question), nth(index)]
}

/// 在批次面板中替一項選「接受方案」與指定的選項。
async fn accept_in_batch(browser: &Browser, question: &str, index: usize, option: &str) {
    let item = batch_item(question, index);
    browser.check(&[item.clone(), vec![role("radio", "接受方案")]].concat()).await;
    browser.check(&[item, vec![role("radio", option)]].concat()).await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "UI E2E：需要 Playwright，以 node scripts/test-e2e.mjs 執行"]
async fn a_batch_in_the_decision_center_saves_each_item_and_resumes_the_tasks_without_blanket_authorization() {
    let e2e = UiE2e::start().await;
    let (harness, browser) = (&e2e.harness, &e2e.browser);

    // --- 前置（API）：三個 Task 各有一個執行中的 Worker；第三個 Run 需要另一個帳號的並行名額 ---
    harness.add_account("第二帳號", "second@example.com").await;
    let (ws, ws_name) = workspace_with_dev_authorization(harness).await;
    let goal = api(harness, Method::POST, &format!("{WORKSPACES}/{ws}/goals"), Some(json!({ "title": "目標" })), StatusCode::CREATED)
        .await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let titles = ["推送修正", "推送文件", "整理比較表"];
    let (a, run_a, token_a) = running_task(harness, &ws, &goal, titles[0]).await;
    let (b, run_b, token_b) = running_task(harness, &ws, &goal, titles[1]).await;
    let (c, run_c, token_c) = running_task(harness, &ws, &goal, titles[2]).await;

    // --- scripted fake Worker／Controller 產生四筆裁定請求 ---
    let ra = prompt_waiting(harness, &token_a, PUSH).await;
    let rb = prompt_waiting(harness, &token_b, PUSH).await;
    let rq = ask(harness, &token_c, "要先出貨還是等報表？").await;
    let rc = controller_waiting(harness, &token_c).await;
    assert_eq!(pending_count(harness).await, 4);
    let (view_a, view_b, view_q, view_c) = (
        decision(harness, &ws, &ra).await,
        decision(harness, &ws, &rb).await,
        decision(harness, &ws, &rq).await,
        decision(harness, &ws, &rc).await,
    );
    let push_question = view_a["question"].as_str().unwrap().to_string();
    assert_eq!(view_b["question"], push_question, "兩個 Task 的同一個操作：問題相同，但是各自一個請求");
    let q_question = view_q["question"].as_str().unwrap().to_string();
    let c_question = view_c["question"].as_str().unwrap().to_string();
    // Worker 依約定在送出請求後結束
    for rid in [&run_a, &run_b, &run_c] {
        worker_exits(harness, rid).await;
    }

    // --- UI：開啟裁定中心，四筆都在「可處理」 ---
    // 側欄只在載入時讀取 Workspace 清單：重新載入 Web UI
    browser.goto(&e2e.server.url("/")).await;
    browser.wait_visible(&[text("Runtime 運作中")]).await;
    browser.wait_visible(&[text("待裁定：4")]).await;
    browser.wait_visible(&[text("待裁定 4")]).await;
    open_center(browser, &ws_name).await;
    browser.wait_visible(&[center(), role("heading", "可處理（4）")]).await;

    // 逐項勾選進批次
    for index in [0, 1] {
        browser.check(&[center(), role("checkbox", &format!("批次選取 {push_question}")), nth(index)]).await;
    }
    browser.check(&[center(), role("checkbox", &format!("批次選取 {q_question}"))]).await;
    browser.check(&[center(), role("checkbox", &format!("批次選取 {c_question}"))]).await;
    browser.wait_visible(&[center(), role("form", "批次處理"), role("heading", "批次處理（4 項）")]).await;
    let note = browser.text(&[center(), role("form", "批次處理")]).await;
    assert!(note.contains("每一項各自保存，不會形成概括授權"), "{note}");

    // 每一項各自選擇處理方式：兩筆權限提示各允許本次操作、Agent 問題選「先出貨」、Controller 的動作核准這一次
    accept_in_batch(browser, &push_question, 0, &option_label(&view_a, "allow_once")).await;
    accept_in_batch(browser, &push_question, 1, &option_label(&view_b, "allow_once")).await;
    accept_in_batch(browser, &q_question, 0, &option_label(&view_q, "ship")).await;
    accept_in_batch(browser, &c_question, 0, &option_label(&view_c, "approve")).await;
    browser.click(&[center(), role("button", "送出批次（4 項）")]).await;

    // --- UI：每一筆各自的結果、待裁定歸零 ---
    browser.wait_visible(&[center(), role("region", "批次結果")]).await;
    let results = browser.text(&[center(), role("region", "批次結果")]).await;
    for question in [&push_question, &q_question, &c_question] {
        assert!(results.contains(&format!("{question}：已保存")), "{results}");
    }
    assert_eq!(results.matches("：已保存").count(), 4, "四筆各自一行結果：{results}");
    assert!(!results.contains("失敗"), "{results}");
    browser.wait_hidden(&[center(), role("form", "批次處理")]).await;
    browser.wait_visible(&[center(), role("heading", "可處理（0）")]).await;
    browser.wait_visible(&[center(), role("heading", "已結束（4）")]).await;
    browser.wait_visible(&[text("待裁定：0")]).await;
    browser.wait_hidden(&[text("待裁定 4")]).await;
    // 已結束的每一筆都顯示自己的裁定結果，並標示是批次中各自保存的一項
    for (question, index, chosen) in [
        (&push_question, 0, option_label(&view_a, "allow_once")),
        (&push_question, 1, option_label(&view_b, "allow_once")),
        (&q_question, 0, option_label(&view_q, "ship")),
        (&c_question, 0, option_label(&view_c, "approve")),
    ] {
        browser.click(&[center(), role("button", &format!("查看裁定 {question}")), nth(index)]).await;
        let outcome = [center(), role("article", "裁定詳情"), role("region", "裁定結果")];
        browser.wait_visible(&outcome).await;
        let shown = browser.text(&outcome).await;
        let chosen = chosen.trim_end_matches("（建議）");
        assert!(shown.contains(&format!("選擇：{chosen}")), "{shown}");
        assert!(shown.contains("批次處理（此項各自保存）"), "{shown}");
    }

    // --- API：每一筆各自保存了自己的 decision，同屬一個批次 ---
    let mut batch_ids = Vec::new();
    for (rid, option) in [(&ra, "allow_once"), (&rb, "allow_once"), (&rq, "ship"), (&rc, "approve")] {
        let view = decision(harness, &ws, rid).await;
        assert_eq!(view["status"], "answered", "{view}");
        assert_eq!(view["decision"]["action"], "accept", "{view}");
        assert_eq!(view["decision"]["option_id"], option, "{view}");
        assert_eq!(view["decision"]["decided_by"], "desktop", "{view}");
        assert!(view["decision"]["batch_id"].is_string(), "{view}");
        batch_ids.push(view["decision"]["batch_id"].clone());
    }
    assert!(batch_ids.iter().all(|id| *id == batch_ids[0]), "同一個批次：{batch_ids:?}");
    let saved: i64 = harness.db().query_row("SELECT COUNT(*) FROM decision", [], |r| r.get(0)).unwrap();
    assert_eq!(saved, 4, "每一項各自一筆裁定，不是一筆概括的裁定");
    assert_eq!(pending_count(harness).await, 0);
    for tid in [&a, &b, &c] {
        assert_eq!(task_status(harness, &ws, tid).await, "ready", "受影響的 Task 已恢復並排入派工佇列");
    }

    // --- 受影響的 Task 恢復：Supervisor 重新派工，UI 的工作面板顯示三個都在執行中 ---
    harness.supervise().await;
    let (_, token_a2) = running_run(harness, &ws, &a).await;
    let (_, token_b2) = running_run(harness, &ws, &b).await;
    let (_, token_c2) = running_run(harness, &ws, &c).await;
    browser.click(&tab("工作")).await;
    let running = [role("region", "工作面板"), role("region", "執行中（3）")];
    browser.wait_visible(&running).await;
    for title in titles {
        browser.wait_visible(&[running.to_vec(), vec![role("article", title)]].concat()).await;
    }

    // --- 不形成概括授權：每一項批准只放行它自己的那一次 ---
    // A：同一個操作消耗自己那一次批准；再做一次又要裁定
    let approved = prompt(harness, &token_a2, PUSH).await;
    assert_eq!(approved["behavior"], "allow", "{approved}");
    assert_eq!(approved["decision_request_id"], ra);
    let again_a = prompt_waiting(harness, &token_a2, PUSH).await;
    // B：自己那一次批准放行同一個操作；不同的操作沒有被批准
    let approved = prompt(harness, &token_b2, PUSH).await;
    assert_eq!(approved["behavior"], "allow", "{approved}");
    assert_eq!(approved["decision_request_id"], rb);
    let other_b = prompt_waiting(harness, &token_b2, FORCE_PUSH).await;
    // C：Controller 的核准放行一次；第二次又要裁定
    let approved = controller(harness, &token_c2).await;
    assert_eq!(approved["status"], "approved", "{approved}");
    let again_c = controller_waiting(harness, &token_c2).await;
    for (fresh, old) in [(&again_a, &ra), (&other_b, &rb), (&again_c, &rc)] {
        assert!(![&ra, &rb, &rq, &rc].contains(&fresh), "{fresh} 是新的請求，不沿用 {old}");
    }
    assert_eq!(pending_count(harness).await, 3);
    for tid in [&a, &b, &c] {
        assert_eq!(task_status(harness, &ws, tid).await, "waiting_decision");
    }

    // --- UI：裁定中心出現三筆新的待裁定請求；先前的批准已被各自的 Run 用掉 ---
    open_center(browser, &ws_name).await;
    browser.wait_visible(&[center(), role("heading", "可處理（3）")]).await;
    browser.wait_visible(&[text("待裁定：3")]).await;
    for rid in [&again_a, &other_b, &again_c] {
        let question = decision(harness, &ws, rid).await["question"].as_str().unwrap().to_string();
        // Controller 的新請求與已結束的那筆問題相同：「可處理」排在最前面，取第一個並確認它不是已結束的
        let listed = [center(), role("button", &format!("查看裁定 {question}")), nth(0)];
        browser.wait_visible(&listed).await;
        let shown = browser.text(&listed).await;
        assert!(!shown.contains("已回答"), "新的請求待處理：{shown}");
    }
    for index in [0, 1] {
        browser.click(&[center(), role("button", &format!("查看裁定 {push_question}")), nth(index)]).await;
        let outcome = [center(), role("article", "裁定詳情"), role("region", "裁定結果")];
        browser.wait_visible(&outcome).await;
        let shown = browser.text(&outcome).await;
        assert!(shown.contains("使用，已失效"), "單次批准用過一次就失效：{shown}");
    }

    e2e.finish().await;
}
