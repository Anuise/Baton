//! 情境測試：多帳號並行排程與容量（票 19），經由 Runtime harness 的真實 HTTP API 驗證。
//!
//! 全部使用 fake ProcessControl／AgentAdapter／QuotaObserver／ClaudeEnvironment 與 fake Clock：
//! 測試決定何時讓 Supervisor 處理一次（`supervise`）、額度樣本是多少、Run 何時結束。
//! 只斷言外部行為：HTTP 回應（派工 201／202 與原因）、排程總覽、Port 被呼叫的方式（spawn 次數）、重啟後仍在；
//! `harness.db()` 只用來確認「容量設定的事件有追加（或沒有追加）」這類必須看落地內容的事實。
//!
//! 每個測試名稱對應票 19 的驗收條件：
//! - 兩個帳號同時執行不同 Task，Run 與帳號關聯正確：`two_accounts_run_different_tasks_...`
//! - 達到每帳號或全域上限時新 Task 排隊並顯示原因：`an_account_at_its_limit_...`、`the_global_limit_...`、佇列的持久性與優先序
//! - 交接保留槽不被一般派工佔用：`the_handoff_slot_...`、`with_only_two_accounts_...`、`switching_to_higher_concurrency_...`
//! - 阻擋原因區分額度、授權、相依與狀態不明：`a_blocked_*`、`quota_*`、`..._reason_categories_...`

use baton_domain::profile::AuthSource;
use baton_domain::Timestamp;
use baton_harness::Harness;
use baton_ports::{AdapterEvent, Confidence, QuotaSample, QuotaSource, StopResult};
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

const WORKSPACES: &str = "/api/v1/workspaces";
const PROFILES: &str = "/api/v1/profiles";
const SCHEDULING: &str = "/api/v1/scheduling";
const SCHEDULING_SETTINGS: &str = "/api/v1/scheduling/settings";

/// 2023-11-14 22:13:20.123 UTC
const CLOCK_START: Timestamp = Timestamp::from_millis(1_700_000_000_123);

// --- 共用輔助 ---

fn plain_dir(harness: &Harness, name: &str) -> String {
    let dir = harness.data_dir().join("projects").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    dir.to_string_lossy().into_owned()
}

fn tasks_path(ws: &str) -> String {
    format!("{WORKSPACES}/{ws}/tasks")
}

fn task_path(ws: &str, tid: &str) -> String {
    format!("{WORKSPACES}/{ws}/tasks/{tid}")
}

fn run_path(ws: &str, rid: &str) -> String {
    format!("{WORKSPACES}/{ws}/runs/{rid}")
}

fn filled(text: &str) -> Value {
    json!({ "status": "filled", "text": text })
}

fn contract_with(objective: &str, capabilities: &[&str]) -> Value {
    json!({
        "objective": filled(objective),
        "inputs": filled("既有的資料"),
        "scope": filled("公開資料"),
        "deliverables": filled("比較表"),
        "done_criteria": filled("五個產品"),
        "blocker_handling": filled("卡住時回報"),
        "required_capabilities": capabilities,
    })
}

fn error_code(body: &Value) -> &str {
    body["error"].as_str().unwrap_or_default()
}

/// 啟動 Harness 並建立 `accounts` 個可派工的帳號（依建立順序），回傳帳號識別。
async fn start_with_accounts(accounts: usize) -> (Harness, Vec<String>) {
    let harness = Harness::builder().clock_at(CLOCK_START).start().await;
    let mut ids = Vec::new();
    for n in 1..=accounts {
        ids.push(harness.add_account(&format!("帳號 {n}"), &format!("account{n}@example.com")).await);
    }
    (harness, ids)
}

/// 一個 Workspace、一個 Goal，回傳 (workspace_id, goal_id)。
async fn setup(harness: &Harness) -> (String, String) {
    let path = plain_dir(harness, "main");
    let (status, body) = harness.send_json(Method::POST, WORKSPACES, Some(json!({ "path": path }))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let ws = body["id"].as_str().unwrap().to_string();
    let (status, goal) =
        harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/goals"), Some(json!({ "title": "目標" }))).await;
    assert_eq!(status, StatusCode::CREATED, "{goal}");
    (ws, goal["id"].as_str().unwrap().to_string())
}

/// 建立 Task（不需要任何能力的完整契約）並讓它 ready，回傳識別。
async fn ready_task(harness: &Harness, ws: &str, goal: &str, title: &str) -> String {
    ready_task_with(harness, ws, goal, title, &[]).await
}

async fn ready_task_with(harness: &Harness, ws: &str, goal: &str, title: &str, capabilities: &[&str]) -> String {
    let body = json!({ "goal_id": goal, "title": title, "contract": contract_with("整理資料", capabilities) });
    let (status, task) = harness.send_json(Method::POST, &tasks_path(ws), Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    let tid = task["id"].as_str().unwrap().to_string();
    let (status, ready) = harness.send_json(Method::POST, &format!("{}/ready", task_path(ws, &tid)), None).await;
    assert_eq!(status, StatusCode::OK, "{ready}");
    tid
}

async fn dispatch(harness: &Harness, ws: &str, tid: &str) -> (StatusCode, Value) {
    harness.send_json(Method::POST, &format!("{}/dispatch", task_path(ws, tid)), None).await
}

async fn dispatch_with_priority(harness: &Harness, ws: &str, tid: &str, priority: i32) -> (StatusCode, Value) {
    harness
        .send_json(Method::POST, &format!("{}/dispatch", task_path(ws, tid)), Some(json!({ "priority": priority })))
        .await
}

/// 派工並要求成功（201），回傳 Run 識別。
async fn dispatch_ok(harness: &Harness, ws: &str, tid: &str) -> String {
    let (status, run) = dispatch(harness, ws, tid).await;
    assert_eq!(status, StatusCode::CREATED, "{run}");
    run["id"].as_str().unwrap().to_string()
}

/// 派工並要求排隊（202），回傳回應本文。
async fn dispatch_queued(harness: &Harness, ws: &str, tid: &str) -> Value {
    let (status, body) = dispatch(harness, ws, tid).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert_eq!(body["queued"], true, "{body}");
    assert_eq!(body["task_id"], json!(tid), "{body}");
    body
}

/// 建立 `count` 個 ready 的 Task 並全部派工成功，回傳 (task_id, run_id)。
async fn fill(harness: &Harness, ws: &str, goal: &str, count: usize) -> Vec<(String, String)> {
    let mut started = Vec::new();
    for n in 1..=count {
        let tid = ready_task(harness, ws, goal, &format!("已派工 {n}")).await;
        let rid = dispatch_ok(harness, ws, &tid).await;
        started.push((tid, rid));
    }
    started
}

async fn task_status(harness: &Harness, ws: &str, tid: &str) -> String {
    let (status, detail) = harness.send_json(Method::GET, &task_path(ws, tid), None).await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    detail["task"]["status"].as_str().unwrap().to_string()
}

async fn run_view(harness: &Harness, ws: &str, rid: &str) -> Value {
    let (status, run) = harness.send_json(Method::GET, &run_path(ws, rid), None).await;
    assert_eq!(status, StatusCode::OK, "{run}");
    run
}

async fn overview(harness: &Harness) -> Value {
    let (status, body) = harness.send_json(Method::GET, SCHEDULING, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

async fn set_mode(harness: &Harness, mode: &str) -> Value {
    let (status, body) = harness.send_json(Method::PUT, SCHEDULING_SETTINGS, Some(json!({ "mode": mode }))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

/// Run 結束（程式正常退出）：推送 Adapter 事件並讓 Supervisor 處理一次（含釋出名額後的排程）。
async fn finish(harness: &Harness, rid: &str) {
    harness.agent_adapter.emit(rid, AdapterEvent::Exited { success: true });
    harness.supervise().await;
}

/// 排程總覽中某個帳號的資料。
fn account<'a>(overview: &'a Value, profile_id: &str) -> &'a Value {
    overview["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["profile_id"] == profile_id)
        .unwrap_or_else(|| panic!("總覽中沒有帳號 {profile_id}：{overview}"))
}

fn runs_of<'a>(overview: &'a Value, profile_id: &str) -> Vec<&'a Value> {
    account(overview, profile_id)["runs"].as_array().unwrap().iter().collect()
}

/// 各帳號進行中的 Run 數（依總覽的帳號順序）。
fn running_counts(overview: &Value) -> Vec<u64> {
    overview["accounts"].as_array().unwrap().iter().map(|a| a["running"].as_u64().unwrap()).collect()
}

/// 佇列中的 Task 識別（依佇列順序）。
fn queued_task_ids(overview: &Value) -> Vec<String> {
    overview["queue"].as_array().unwrap().iter().map(|e| e["task_id"].as_str().unwrap().to_string()).collect()
}

/// 派工回應或佇列項目的原因，整理成 (類別, 代碼)。
fn reasons(body: &Value) -> Vec<(String, String)> {
    body["reasons"]
        .as_array()
        .unwrap_or_else(|| panic!("沒有 reasons：{body}"))
        .iter()
        .map(|r| (r["category"].as_str().unwrap().to_string(), r["code"].as_str().unwrap().to_string()))
        .collect()
}

fn pair(category: &str, code: &str) -> (String, String) {
    (category.to_string(), code.to_string())
}

fn categories(body: &Value) -> Vec<String> {
    let mut found: Vec<String> = reasons(body).into_iter().map(|(category, _)| category).collect();
    found.sort();
    found.dedup();
    found
}

/// 在排程總覽中找出某個 Task 目前進行中的 Run 識別。
fn run_of_task(overview: &Value, task_id: &str) -> Option<String> {
    overview["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|a| a["runs"].as_array().unwrap().iter())
        .find(|r| r["task_id"] == task_id)
        .map(|r| r["run_id"].as_str().unwrap().to_string())
}

/// 容量設定的事件（`aggregate_type = 'scheduler_settings'`）：(版本, 事件種類)。
fn settings_events(harness: &Harness) -> Vec<(i64, String)> {
    let db = harness.db();
    let mut statement = db
        .prepare("SELECT aggregate_version, event_type FROM event WHERE aggregate_type = 'scheduler_settings' ORDER BY seq")
        .unwrap();
    statement.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect()
}

/// 額度觀測器的輪詢間隔（spec D-31）：改了樣本之後要等到下一次輪詢，Runtime 才會看到新的值（票 27）。
const QUOTA_POLL_MILLIS: i64 = 5 * 60 * 1000;

/// 腳本化某個帳號的額度樣本（用量比例）。
fn set_quota(harness: &Harness, profile_id: &str, used: f64) {
    harness.quota_observer.set_sample(
        profile_id,
        QuotaSample {
            source: QuotaSource::Estimated,
            observed_at: CLOCK_START,
            used: Some(used),
            reset_at: None,
            confidence: Confidence::Estimated,
        },
    );
}

/// 確認某個授權範本（trusted_dev_p3 需要承認風險）。
async fn confirm_template(harness: &Harness, ws: &str, template_id: &str) {
    let path = format!("{WORKSPACES}/{ws}/authorization");
    let (status, view) = harness.send_json(Method::GET, &path, None).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    let grants = view["templates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == template_id)
        .unwrap_or_else(|| panic!("沒有範本 {template_id}"))["grants"]
        .clone();
    let body = json!({ "template_id": template_id, "grants": grants, "acknowledged_risk": true, "acknowledged_expansion": true });
    let (status, revision) = harness.send_json(Method::PUT, &path, Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{revision}");
}

/// 建立一個 Profile 但不做任何登入或核對，回傳 (識別, 設定目錄)。
async fn create_profile(harness: &Harness, name: &str) -> (String, String) {
    let (status, created) = harness.send_json(Method::POST, PROFILES, Some(json!({ "name": name }))).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    (created["id"].as_str().unwrap().to_string(), created["config_dir"].as_str().unwrap().to_string())
}

async fn verify_profile(harness: &Harness, id: &str) -> Value {
    let (status, body) = harness.send_json(Method::POST, &format!("{PROFILES}/{id}/verify"), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

// ============================================================ 兩個帳號同時執行不同 Task

#[tokio::test]
async fn two_accounts_run_different_tasks_at_the_same_time_and_each_run_belongs_to_its_account() {
    let (harness, accounts) = start_with_accounts(2).await;
    let (ws, goal) = setup(&harness).await;
    let first_task = ready_task(&harness, &ws, &goal, "整理甲").await;
    let second_task = ready_task(&harness, &ws, &goal, "整理乙").await;

    let (first_status, first) = dispatch(&harness, &ws, &first_task).await;
    let (second_status, second) = dispatch(&harness, &ws, &second_task).await;

    assert_eq!((first_status, second_status), (StatusCode::CREATED, StatusCode::CREATED), "{first} {second}");
    assert_eq!(first["profile_id"], json!(accounts[0]), "第一個 Task 給較早建立的帳號");
    assert_eq!(second["profile_id"], json!(accounts[1]), "第二個 Task 給另一個帳號，而不是排在同一個帳號後面");
    assert_ne!(first["profile_id"], second["profile_id"]);
    // 查 Run 時帳號關聯仍在
    let first_rid = first["id"].as_str().unwrap();
    let second_rid = second["id"].as_str().unwrap();
    assert_eq!(run_view(&harness, &ws, first_rid).await["profile_id"], json!(accounts[0]));
    assert_eq!(run_view(&harness, &ws, second_rid).await["profile_id"], json!(accounts[1]));

    // 總覽：每個帳號各有一個進行中的 Run，指向正確的 Task
    let before = overview(&harness).await;
    let first_runs = runs_of(&before, &accounts[0]);
    let second_runs = runs_of(&before, &accounts[1]);
    assert_eq!((first_runs.len(), second_runs.len()), (1, 1), "{before}");
    assert_eq!(first_runs[0]["run_id"], json!(first_rid));
    assert_eq!(first_runs[0]["task_id"], json!(first_task));
    assert_eq!(first_runs[0]["task_title"], "整理甲");
    assert_eq!(first_runs[0]["workspace_id"], json!(ws));
    assert_eq!((first_runs[0]["status"].as_str(), first_runs[0]["generation"].as_i64()), (Some("queued"), Some(1)));
    assert_eq!(first_runs[0]["cancel_requested"], false);
    assert_eq!(second_runs[0]["run_id"], json!(second_rid));
    assert_eq!(second_runs[0]["task_id"], json!(second_task));
    assert_eq!(before["capacity"]["running"], 2);
    assert!(before["queue"].as_array().unwrap().is_empty(), "兩個都派出了，佇列是空的");

    // 兩個程序都啟動了
    harness.supervise().await;
    assert_eq!(harness.process_control.spawn_count(), 2, "兩個 Run 同時各自啟動一個程序");
    assert_eq!(run_view(&harness, &ws, first_rid).await["status"], "running");
    assert_eq!(run_view(&harness, &ws, second_rid).await["status"], "running");
    assert_eq!(running_counts(&overview(&harness).await), [1, 1]);

    // 各自完成後各帳號進行中的數量歸零
    finish(&harness, first_rid).await;
    let after_first = overview(&harness).await;
    assert_eq!(running_counts(&after_first), [0, 1], "只有結束的那一個帳號釋出名額");
    assert_eq!(run_view(&harness, &ws, first_rid).await["status"], "succeeded");
    finish(&harness, second_rid).await;
    let after_both = overview(&harness).await;
    assert_eq!(running_counts(&after_both), [0, 0]);
    assert_eq!(after_both["capacity"]["running"], 0);
    assert_eq!(run_view(&harness, &ws, second_rid).await["status"], "succeeded");
    assert_eq!(harness.process_control.spawn_count(), 2, "結束不會再啟動任何程序");
}

#[tokio::test]
async fn new_tasks_go_to_the_account_with_the_fewest_active_runs() {
    let (harness, accounts) = start_with_accounts(2).await;
    let (ws, goal) = setup(&harness).await;

    let runs = fill(&harness, &ws, &goal, 3).await;

    let after = overview(&harness).await;
    assert_eq!(running_counts(&after), [2, 1], "第 1、3 個給帳號 1，第 2 個給帳號 2");
    let third = run_view(&harness, &ws, &runs[2].1).await;
    assert_eq!(third["profile_id"], json!(accounts[0]), "兩個帳號同數時取較早建立的");
}

// ============================================================ 每帳號上限

#[tokio::test]
async fn an_account_at_its_limit_queues_the_next_task_with_a_reason_until_a_run_finishes() {
    let (harness, accounts) = start_with_accounts(1).await;
    let (ws, goal) = setup(&harness).await;
    let started = fill(&harness, &ws, &goal, 2).await;
    harness.supervise().await;
    let waiting_task = ready_task(&harness, &ws, &goal, "等名額的 Task").await;

    let (status, body) = dispatch(&harness, &ws, &waiting_task).await;

    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert_eq!(body["queued"], true);
    assert_eq!(body["task_id"], json!(waiting_task));
    assert_eq!(body["priority"], 0);
    assert_eq!(reasons(&body), [pair("capacity", "account_capacity_full")], "{body}");
    assert_eq!(body["reasons"][0]["profile_id"], json!(accounts[0]), "原因指出是哪個帳號滿了");
    assert!(!body["reasons"][0]["message"].as_str().unwrap().is_empty());
    assert_eq!(task_status(&harness, &ws, &waiting_task).await, "ready", "排隊的 Task 維持 ready");
    let queued = overview(&harness).await;
    assert_eq!(queued_task_ids(&queued), [waiting_task.clone()]);
    assert_eq!(queued["queue"][0]["title"], "等名額的 Task");
    assert_eq!(queued["queue"][0]["position"], 1);
    assert_eq!(queued["queue"][0]["priority"], 0);
    assert_eq!(reasons(&queued["queue"][0]), [pair("capacity", "account_capacity_full")], "總覽顯示同樣的原因");
    let account_view = account(&queued, &accounts[0]);
    assert_eq!((account_view["running"].as_u64(), account_view["limit"].as_u64()), (Some(2), Some(2)));
    assert_eq!(queued["capacity"]["per_account_limit"], 2);
    assert_eq!(harness.process_control.spawn_count(), 2);

    // 名額還沒空出來，Supervisor 處理再多次也不會派出它
    harness.supervise().await;
    assert_eq!(task_status(&harness, &ws, &waiting_task).await, "ready");
    assert_eq!(harness.process_control.spawn_count(), 2);

    // 其中一個 Run 結束後，排隊的 Task 在同一次處理中取得名額並啟動
    finish(&harness, &started[0].1).await;

    assert_eq!(task_status(&harness, &ws, &waiting_task).await, "running");
    let after = overview(&harness).await;
    assert!(after["queue"].as_array().unwrap().is_empty(), "取得名額後離開佇列");
    let new_run = run_of_task(&after, &waiting_task).expect("排隊的 Task 應該已有進行中的 Run");
    let new_view = run_view(&harness, &ws, &new_run).await;
    assert_eq!((new_view["profile_id"].as_str(), new_view["status"].as_str()), (Some(accounts[0].as_str()), Some("running")));
    assert_eq!(harness.process_control.spawn_count(), 3, "同一次處理中就啟動了新的程序");
    assert_eq!(running_counts(&after), [2]);
}

// ============================================================ 全域上限

#[tokio::test]
async fn the_global_limit_queues_the_ninth_task_with_a_global_reason_in_higher_concurrency_mode() {
    let (harness, _) = start_with_accounts(3).await;
    let (ws, goal) = setup(&harness).await;
    set_mode(&harness, "higher_concurrency").await;
    let started = fill(&harness, &ws, &goal, 8).await;
    harness.supervise().await;
    assert_eq!(harness.process_control.spawn_count(), 8);
    let full = overview(&harness).await;
    let mut counts = running_counts(&full);
    counts.sort_unstable();
    assert_eq!(counts, [2, 3, 3], "8 個 Run 平均分配，每帳號最多 3 個");
    assert_eq!(full["capacity"]["global_limit"], 8);
    assert_eq!(full["capacity"]["per_account_limit"], 3);
    assert_eq!(full["capacity"]["handoff_reserve"], 0);
    assert_eq!(full["capacity"]["normal_limit"], 8);
    assert_eq!(full["capacity"]["running"], 8);
    let ninth = ready_task(&harness, &ws, &goal, "第九個").await;

    let body = dispatch_queued(&harness, &ws, &ninth).await;

    let found = reasons(&body);
    assert_eq!(found[0], pair("capacity", "global_capacity_full"), "{body}");
    assert!(body["reasons"][0]["profile_id"].is_null(), "全域名額不屬於任何帳號：{body}");
    assert!(found.iter().all(|(category, _)| category == "capacity"), "只有容量類原因：{body}");
    assert!(!found.contains(&pair("capacity", "handoff_slot_reserved")), "較高並行不保留交接槽：{body}");
    assert_eq!(task_status(&harness, &ws, &ninth).await, "ready");
    assert_eq!(queued_task_ids(&overview(&harness).await), [ninth.clone()]);

    // 任何一個 Run 結束，全域名額就空出來給它
    finish(&harness, &started[0].1).await;
    assert_eq!(task_status(&harness, &ws, &ninth).await, "running");
    assert!(overview(&harness).await["queue"].as_array().unwrap().is_empty());
    assert_eq!(harness.process_control.spawn_count(), 9);
}

// ============================================================ 交接保留槽

#[tokio::test]
async fn the_handoff_slot_is_not_taken_by_normal_dispatch_when_three_accounts_are_usable() {
    let (harness, _) = start_with_accounts(3).await;
    let (ws, goal) = setup(&harness).await;
    let started = fill(&harness, &ws, &goal, 5).await;
    harness.supervise().await;
    let before = overview(&harness).await;
    assert_eq!(before["settings"]["mode"], "reserve_handoff", "預設保留交接容量");
    assert_eq!(before["capacity"]["handoff_reserve"], 1);
    assert_eq!(before["capacity"]["normal_limit"], 5);
    assert_eq!(before["capacity"]["global_limit"], 6);
    assert_eq!(before["capacity"]["usable_accounts"], 3);
    assert_eq!(before["capacity"]["running"], 5);
    let sixth = ready_task(&harness, &ws, &goal, "第六個").await;

    let body = dispatch_queued(&harness, &ws, &sixth).await;

    let found = reasons(&body);
    assert!(found.contains(&pair("capacity", "handoff_slot_reserved")), "{body}");
    assert!(!found.iter().any(|(_, code)| code == "global_capacity_full"), "全域還沒滿，是保留槽擋住：{body}");
    assert_eq!(categories(&body), ["capacity"], "{body}");
    assert_eq!(task_status(&harness, &ws, &sixth).await, "ready");
    assert_eq!(overview(&harness).await["capacity"]["running"], 5, "第 6 個全域名額留著，一般派工沒有佔用");
    harness.supervise().await;
    assert_eq!(harness.process_control.spawn_count(), 5, "Supervisor 也不會用一般派工佔用保留槽");

    // 一個 Run 結束後，排隊的 Task 取得第 5 個一般名額
    finish(&harness, &started[0].1).await;
    assert_eq!(task_status(&harness, &ws, &sixth).await, "running");
    assert_eq!(overview(&harness).await["capacity"]["running"], 5);
}

#[tokio::test]
async fn the_handoff_slot_is_the_only_reason_when_every_account_still_has_room() {
    let (harness, _) = start_with_accounts(5).await;
    let (ws, goal) = setup(&harness).await;
    fill(&harness, &ws, &goal, 5).await;
    let overview_before = overview(&harness).await;
    assert_eq!(running_counts(&overview_before), [1, 1, 1, 1, 1]);
    assert_eq!(overview_before["capacity"]["normal_limit"], 5);
    let sixth = ready_task(&harness, &ws, &goal, "第六個").await;

    let body = dispatch_queued(&harness, &ws, &sixth).await;

    assert_eq!(reasons(&body), [pair("capacity", "handoff_slot_reserved")], "{body}");
    assert!(body["reasons"][0]["profile_id"].is_null());
}

#[tokio::test]
async fn with_only_two_accounts_no_handoff_slot_is_reserved_but_the_per_account_limit_still_applies() {
    let (harness, _) = start_with_accounts(2).await;
    let (ws, goal) = setup(&harness).await;
    let before = overview(&harness).await;
    assert_eq!(before["capacity"]["handoff_reserve"], 0, "可用帳號不到 3 個不保留");
    assert_eq!(before["capacity"]["normal_limit"], 6);
    assert_eq!(before["capacity"]["usable_accounts"], 2);
    fill(&harness, &ws, &goal, 4).await;
    let fifth = ready_task(&harness, &ws, &goal, "第五個").await;

    let body = dispatch_queued(&harness, &ws, &fifth).await;

    let found = reasons(&body);
    assert_eq!(found, [pair("capacity", "account_capacity_full"), pair("capacity", "account_capacity_full")], "{body}");
    assert!(!found.iter().any(|(_, code)| code == "handoff_slot_reserved"), "{body}");
    let profile_ids: Vec<&str> = body["reasons"].as_array().unwrap().iter().map(|r| r["profile_id"].as_str().unwrap()).collect();
    assert_ne!(profile_ids[0], profile_ids[1], "兩個帳號各有一個原因");
}

#[tokio::test]
async fn a_third_account_that_cannot_be_dispatched_to_does_not_count_toward_the_reserve() {
    let (harness, _) = start_with_accounts(2).await;
    let (unverified_id, _) = create_profile(&harness, "還沒核對的帳號").await;
    let (ws, goal) = setup(&harness).await;
    let before = overview(&harness).await;
    assert_eq!(before["capacity"]["usable_accounts"], 2);
    assert_eq!(before["capacity"]["handoff_reserve"], 0, "三個 Profile 但只有兩個可用，不保留");
    assert_eq!(account(&before, &unverified_id)["dispatchable"], false);
    fill(&harness, &ws, &goal, 4).await;

    let after = overview(&harness).await;

    assert_eq!(after["capacity"]["normal_limit"], 6);
}

#[tokio::test]
async fn switching_to_higher_concurrency_dispatches_queued_tasks_immediately_and_the_response_is_the_new_overview() {
    let (harness, _) = start_with_accounts(3).await;
    let (ws, goal) = setup(&harness).await;
    fill(&harness, &ws, &goal, 5).await;
    let sixth = ready_task(&harness, &ws, &goal, "第六個").await;
    dispatch_queued(&harness, &ws, &sixth).await;
    assert_eq!(queued_task_ids(&overview(&harness).await), [sixth.clone()]);

    let switched = set_mode(&harness, "higher_concurrency").await;

    // 回應就是新的總覽：模式已切換、排隊的 Task 已經派出
    assert_eq!(switched["settings"]["mode"], "higher_concurrency", "{switched}");
    assert_eq!(switched["capacity"]["global_limit"], 8);
    assert_eq!(switched["capacity"]["per_account_limit"], 3);
    assert_eq!(switched["capacity"]["handoff_reserve"], 0);
    assert!(switched["queue"].as_array().unwrap().is_empty(), "容量變大，佇列立刻消化：{switched}");
    assert_eq!(switched["capacity"]["running"], 6);
    assert!(run_of_task(&switched, &sixth).is_some());
    assert_eq!(task_status(&harness, &ws, &sixth).await, "running");
    harness.supervise().await;
    assert_eq!(harness.process_control.spawn_count(), 6, "新派出的 Run 由 Supervisor 啟動");
}

#[tokio::test]
async fn switching_back_to_reserve_handoff_does_not_stop_runs_that_are_already_running() {
    let (harness, _) = start_with_accounts(3).await;
    let (ws, goal) = setup(&harness).await;
    set_mode(&harness, "higher_concurrency").await;
    fill(&harness, &ws, &goal, 8).await;
    harness.supervise().await;

    let switched = set_mode(&harness, "reserve_handoff").await;

    assert_eq!(switched["capacity"]["running"], 8, "已在執行的 Run 不受影響");
    assert_eq!(harness.process_control.spawn_count(), 8);
    let extra = ready_task(&harness, &ws, &goal, "多出來的").await;
    let body = dispatch_queued(&harness, &ws, &extra).await;
    assert_eq!(reasons(&body)[0], pair("capacity", "global_capacity_full"), "{body}");
}

// ============================================================ 阻擋原因區分

#[tokio::test]
async fn a_blocked_task_with_unfinished_dependencies_waits_under_the_dependency_category() {
    let (harness, _) = start_with_accounts(1).await;
    let (ws, goal) = setup(&harness).await;
    let first = ready_task(&harness, &ws, &goal, "先做").await;
    let second = ready_task(&harness, &ws, &goal, "後做").await;
    let (status, _) = harness
        .send_json(Method::POST, &format!("{}/dependencies", task_path(&ws, &second)), Some(json!({ "depends_on": first })))
        .await;
    assert_eq!(status, StatusCode::CREATED);

    let body = dispatch_queued(&harness, &ws, &second).await;

    assert_eq!(reasons(&body), [pair("dependency", "dependencies_unfinished")], "{body}");
    assert!(body["reasons"][0]["profile_id"].is_null(), "相依不屬於任何帳號");
    assert!(body["reasons"][0]["message"].as_str().unwrap().contains(first.as_str()), "訊息指出還沒完成的相依：{body}");
    assert_eq!(task_status(&harness, &ws, &second).await, "ready");
    let queued = overview(&harness).await;
    assert_eq!(reasons(&queued["queue"][0]), [pair("dependency", "dependencies_unfinished")]);

    // 相依的 Task 只是在執行中（還沒完成），仍然不會派出
    let first_run = dispatch_ok(&harness, &ws, &first).await;
    harness.supervise().await;
    finish(&harness, &first_run).await;
    assert_eq!(task_status(&harness, &ws, &first).await, "verifying", "Run 成功只讓 Task 進入驗證");
    assert_eq!(task_status(&harness, &ws, &second).await, "ready");
    assert_eq!(queued_task_ids(&overview(&harness).await), [second]);
}

#[tokio::test]
async fn a_blocked_task_whose_capability_was_revoked_waits_under_the_authorization_category_until_it_is_granted_again() {
    let (harness, _) = start_with_accounts(1).await;
    let (ws, goal) = setup(&harness).await;
    confirm_template(&harness, &ws, "trusted_dev_p3").await;
    let tid = ready_task_with(&harness, &ws, &goal, "要跑建置", &["run_build_test"]).await;
    confirm_template(&harness, &ws, "research_docs").await;

    let body = dispatch_queued(&harness, &ws, &tid).await;

    assert_eq!(reasons(&body), [pair("authorization", "capability_not_granted")], "{body}");
    assert!(body["reasons"][0]["profile_id"].is_null(), "授權屬於 Workspace，不屬於帳號");
    let queued = overview(&harness).await;
    assert_eq!(reasons(&queued["queue"][0]), [pair("authorization", "capability_not_granted")]);
    assert_eq!(queued["accounts"][0]["dispatchable"], true, "帳號本身沒有問題");
    harness.supervise().await;
    assert_eq!(task_status(&harness, &ws, &tid).await, "ready", "授權不足時 Supervisor 不會派出");
    assert_eq!(harness.process_control.spawn_count(), 0);

    // 重新授權後，排隊的 Task 在下一次處理時派出
    confirm_template(&harness, &ws, "trusted_dev_p3").await;
    harness.supervise().await;

    assert_eq!(task_status(&harness, &ws, &tid).await, "running");
    assert_eq!(harness.process_control.spawn_count(), 1);
    assert!(overview(&harness).await["queue"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn quota_near_the_limit_blocks_the_account_and_dispatch_resumes_when_the_sample_drops() {
    let (harness, accounts) = start_with_accounts(1).await;
    let (ws, goal) = setup(&harness).await;
    set_quota(&harness, &accounts[0], 0.8);
    let tid = ready_task(&harness, &ws, &goal, "額度吃緊").await;

    let body = dispatch_queued(&harness, &ws, &tid).await;

    assert_eq!(reasons(&body), [pair("quota", "quota_near_limit")], "{body}");
    assert_eq!(body["reasons"][0]["profile_id"], json!(accounts[0]));
    assert!(body["reasons"][0]["message"].as_str().unwrap().contains("80%"), "訊息帶出目前的用量：{body}");
    let queued = overview(&harness).await;
    assert_eq!(account(&queued, &accounts[0])["quota_used"], 0.8, "帳號頁顯示已知的用量");
    assert_eq!(reasons(&queued["queue"][0]), [pair("quota", "quota_near_limit")]);
    harness.supervise().await;
    assert_eq!(task_status(&harness, &ws, &tid).await, "ready", "額度沒降下來之前不派");
    assert_eq!(harness.process_control.spawn_count(), 0);

    // 樣本降到門檻以下：下一次輪詢之後的處理就派出
    set_quota(&harness, &accounts[0], 0.5);
    harness.clock.advance_millis(QUOTA_POLL_MILLIS);
    harness.supervise().await;

    assert_eq!(task_status(&harness, &ws, &tid).await, "running");
    assert_eq!(harness.process_control.spawn_count(), 1);
    assert!(overview(&harness).await["queue"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn quota_blocks_from_seventy_five_percent_and_not_below() {
    let (harness, accounts) = start_with_accounts(1).await;
    let (ws, goal) = setup(&harness).await;
    let tid = ready_task(&harness, &ws, &goal, "邊界").await;

    // 85% 以上另進入 draining（票 28），同樣是額度類的等待原因
    for (blocked, code) in [(0.75, "quota_near_limit"), (0.9, "quota_draining"), (1.0, "quota_draining")] {
        set_quota(&harness, &accounts[0], blocked);
        harness.clock.advance_millis(QUOTA_POLL_MILLIS);
        let body = dispatch_queued(&harness, &ws, &tid).await;
        assert_eq!(reasons(&body), [pair("quota", code)], "用量 {blocked}：{body}");
    }
    assert_eq!(queued_task_ids(&overview(&harness).await), [tid.clone()], "重複派工仍只有一筆佇列項目");

    // 遲滯（spec D-13，票 28）：門檻關閉後要回到 60% 以下才恢復；74% 仍不派
    set_quota(&harness, &accounts[0], 0.74);
    harness.clock.advance_millis(QUOTA_POLL_MILLIS);
    let body = dispatch_queued(&harness, &ws, &tid).await;
    assert_eq!(reasons(&body), [pair("quota", "quota_draining")], "{body}");

    set_quota(&harness, &accounts[0], 0.59);
    harness.clock.advance_millis(QUOTA_POLL_MILLIS);
    let (status, run) = dispatch(&harness, &ws, &tid).await;

    assert_eq!(status, StatusCode::CREATED, "{run}");
}

#[tokio::test]
async fn an_account_without_a_quota_sample_is_not_blocked() {
    let (harness, accounts) = start_with_accounts(1).await;
    let (ws, goal) = setup(&harness).await;
    let tid = ready_task(&harness, &ws, &goal, "沒有樣本").await;

    let (status, run) = dispatch(&harness, &ws, &tid).await;

    assert_eq!(status, StatusCode::CREATED, "未觀測不阻擋：{run}");
    let after = overview(&harness).await;
    assert!(account(&after, &accounts[0])["quota_used"].is_null(), "未觀測顯示為 null，不當作 0：{after}");
}

#[tokio::test]
async fn an_account_near_its_quota_is_skipped_for_another_account_with_room() {
    let (harness, accounts) = start_with_accounts(2).await;
    let (ws, goal) = setup(&harness).await;
    set_quota(&harness, &accounts[0], 0.9);
    let tid = ready_task(&harness, &ws, &goal, "改派給另一個帳號").await;

    let (status, run) = dispatch(&harness, &ws, &tid).await;

    assert_eq!(status, StatusCode::CREATED, "{run}");
    assert_eq!(run["profile_id"], json!(accounts[1]), "較早建立的帳號額度吃緊，改派第二個");
}

#[tokio::test]
async fn a_blocked_account_that_was_never_verified_waits_under_the_state_unknown_category() {
    let harness = Harness::builder().clock_at(CLOCK_START).start().await;
    let (profile_id, config_dir) = create_profile(&harness, "剛建立的帳號").await;
    let (ws, goal) = setup(&harness).await;
    let tid = ready_task(&harness, &ws, &goal, "等核對").await;

    let body = dispatch_queued(&harness, &ws, &tid).await;

    assert_eq!(reasons(&body), [pair("state_unknown", "profile_unverified")], "{body}");
    assert_eq!(body["reasons"][0]["profile_id"], json!(profile_id));
    let queued = overview(&harness).await;
    assert_eq!(queued["capacity"]["usable_accounts"], 0);
    assert_eq!(account(&queued, &profile_id)["dispatchable"], false);
    assert_eq!(account(&queued, &profile_id)["blockers"][0]["code"], "profile_unverified");
    assert_eq!(reasons(&queued["queue"][0]), [pair("state_unknown", "profile_unverified")]);

    // 登入並核對後，下一次處理就派出
    harness.claude_env.log_in(std::path::Path::new(&config_dir), "new@example.com");
    assert_eq!(verify_profile(&harness, &profile_id).await["dispatchable"], true);
    harness.supervise().await;

    assert_eq!(task_status(&harness, &ws, &tid).await, "running");
    assert_eq!(harness.process_control.spawn_count(), 1);
}

#[tokio::test]
async fn a_profile_that_is_not_logged_in_is_an_account_problem_not_an_unknown_state() {
    let harness = Harness::builder().clock_at(CLOCK_START).start().await;
    let (profile_id, _) = create_profile(&harness, "還沒登入的帳號").await;
    verify_profile(&harness, &profile_id).await;
    let (ws, goal) = setup(&harness).await;
    let tid = ready_task(&harness, &ws, &goal, "等登入").await;

    let body = dispatch_queued(&harness, &ws, &tid).await;

    assert_eq!(reasons(&body), [pair("account", "profile_not_logged_in")], "{body}");
    assert_eq!(body["reasons"][0]["profile_id"], json!(profile_id));
    assert_eq!(categories(&body), ["account"], "核對過、確定沒登入，不是「狀態不明」");
}

#[tokio::test]
async fn an_overridden_auth_source_blocks_the_account_until_the_override_is_removed() {
    let (harness, accounts) = start_with_accounts(1).await;
    let (ws, goal) = setup(&harness).await;
    harness.claude_env.set_env_override(Some((AuthSource::ApiKey, Some("ANTHROPIC_API_KEY"))));
    assert_eq!(verify_profile(&harness, &accounts[0]).await["dispatchable"], false);
    let tid = ready_task(&harness, &ws, &goal, "認證被覆蓋").await;

    let body = dispatch_queued(&harness, &ws, &tid).await;

    assert_eq!(reasons(&body), [pair("account", "auth_source_overridden")], "{body}");
    assert_eq!(body["reasons"][0]["profile_id"], json!(accounts[0]));
    assert!(body["reasons"][0]["message"].as_str().unwrap().contains("ANTHROPIC_API_KEY"), "{body}");

    harness.claude_env.set_env_override(None);
    assert_eq!(verify_profile(&harness, &accounts[0]).await["dispatchable"], true);
    harness.supervise().await;

    assert_eq!(task_status(&harness, &ws, &tid).await, "running");
}

#[tokio::test]
async fn without_any_account_a_task_waits_with_the_no_account_reason() {
    let harness = Harness::builder().clock_at(CLOCK_START).start().await;
    let (ws, goal) = setup(&harness).await;
    let tid = ready_task(&harness, &ws, &goal, "沒有帳號").await;

    let body = dispatch_queued(&harness, &ws, &tid).await;

    assert_eq!(reasons(&body), [pair("account", "no_account")], "{body}");
    assert!(body["reasons"][0]["profile_id"].is_null());
    let queued = overview(&harness).await;
    assert!(queued["accounts"].as_array().unwrap().is_empty());
    assert_eq!(queued["capacity"]["usable_accounts"], 0);
    assert_eq!(queued_task_ids(&queued), [tid.clone()]);
    harness.supervise().await;
    assert_eq!(task_status(&harness, &ws, &tid).await, "ready");
    assert_eq!(harness.process_control.spawn_count(), 0);

    // 新增並登入帳號後就能派出
    harness.add_account("第一個帳號", "first@example.com").await;
    harness.supervise().await;
    assert_eq!(task_status(&harness, &ws, &tid).await, "running");
}

#[tokio::test]
async fn reasons_from_different_blockers_keep_their_own_categories_and_accounts() {
    let (harness, accounts) = start_with_accounts(1).await;
    let (unverified_id, _) = create_profile(&harness, "還沒核對的帳號").await;
    let (ws, goal) = setup(&harness).await;
    fill(&harness, &ws, &goal, 2).await;
    let tid = ready_task(&harness, &ws, &goal, "多重原因").await;

    let body = dispatch_queued(&harness, &ws, &tid).await;

    let mut found: Vec<(String, String, Option<String>)> = body["reasons"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (r["category"].as_str().unwrap().to_string(), r["code"].as_str().unwrap().to_string(), r["profile_id"].as_str().map(str::to_string))
        })
        .collect();
    found.sort();
    let mut expected = vec![
        ("capacity".to_string(), "account_capacity_full".to_string(), Some(accounts[0].clone())),
        ("state_unknown".to_string(), "profile_unverified".to_string(), Some(unverified_id.clone())),
    ];
    expected.sort();
    assert_eq!(found, expected, "容量與狀態不明各自歸類、各自指向正確的帳號：{body}");
    assert_eq!(categories(&body), ["capacity", "state_unknown"]);
}

#[tokio::test]
async fn a_task_that_is_not_ready_or_does_not_exist_is_rejected_and_never_queued() {
    let (harness, _) = start_with_accounts(1).await;
    let (ws, goal) = setup(&harness).await;
    let body = json!({ "goal_id": goal, "title": "還沒 ready", "contract": contract_with("整理資料", &[]) });
    let (_, task) = harness.send_json(Method::POST, &tasks_path(&ws), Some(body)).await;
    let pending = task["id"].as_str().unwrap().to_string();
    let running = ready_task(&harness, &ws, &goal, "已派工").await;
    dispatch_ok(&harness, &ws, &running).await;

    let (pending_status, pending_body) = dispatch(&harness, &ws, &pending).await;
    let (running_status, running_body) = dispatch(&harness, &ws, &running).await;
    let (missing_status, missing_body) = dispatch(&harness, &ws, "task_nope").await;

    assert_eq!((pending_status, error_code(&pending_body)), (StatusCode::CONFLICT, "task_not_ready"), "{pending_body}");
    assert_eq!((running_status, error_code(&running_body)), (StatusCode::CONFLICT, "task_not_ready"), "{running_body}");
    assert_eq!((missing_status, error_code(&missing_body)), (StatusCode::NOT_FOUND, "task_not_found"), "{missing_body}");
    assert!(overview(&harness).await["queue"].as_array().unwrap().is_empty(), "被拒絕的要求不進佇列");
}

// ============================================================ 優先序

#[tokio::test]
async fn a_later_task_with_a_higher_priority_gets_the_freed_slot_first() {
    let (harness, _) = start_with_accounts(1).await;
    let (ws, goal) = setup(&harness).await;
    let started = fill(&harness, &ws, &goal, 2).await;
    harness.supervise().await;
    let early = ready_task(&harness, &ws, &goal, "先要求、優先序低").await;
    let late = ready_task(&harness, &ws, &goal, "後要求、優先序高").await;
    let (early_status, early_body) = dispatch_with_priority(&harness, &ws, &early, 0).await;
    let (late_status, late_body) = dispatch_with_priority(&harness, &ws, &late, 5).await;
    assert_eq!((early_status, late_status), (StatusCode::ACCEPTED, StatusCode::ACCEPTED), "{early_body} {late_body}");
    assert_eq!(late_body["priority"], 5);

    let queued = overview(&harness).await;
    assert_eq!(queued_task_ids(&queued), [late.clone(), early.clone()], "優先序高者排在前面");
    assert_eq!(queued["queue"][0]["position"], 1);
    assert_eq!(queued["queue"][0]["priority"], 5);
    assert_eq!(queued["queue"][1]["position"], 2);
    assert_eq!(queued["queue"][1]["priority"], 0);

    finish(&harness, &started[0].1).await;

    assert_eq!(task_status(&harness, &ws, &late).await, "running", "釋出的名額給優先序較高者");
    assert_eq!(task_status(&harness, &ws, &early).await, "ready");
    assert_eq!(queued_task_ids(&overview(&harness).await), [early.clone()]);
    assert_eq!(overview(&harness).await["queue"][0]["position"], 1);

    finish(&harness, &started[1].1).await;

    assert_eq!(task_status(&harness, &ws, &early).await, "running", "下一個名額輪到先要求的那個");
}

#[tokio::test]
async fn tasks_with_the_same_priority_get_slots_in_the_order_they_were_requested() {
    let (harness, _) = start_with_accounts(1).await;
    let (ws, goal) = setup(&harness).await;
    let started = fill(&harness, &ws, &goal, 2).await;
    harness.supervise().await;
    let first = ready_task(&harness, &ws, &goal, "先要求").await;
    let second = ready_task(&harness, &ws, &goal, "後要求").await;
    dispatch_queued(&harness, &ws, &first).await;
    harness.clock.advance_millis(1_000);
    dispatch_queued(&harness, &ws, &second).await;
    assert_eq!(queued_task_ids(&overview(&harness).await), [first.clone(), second.clone()]);

    finish(&harness, &started[0].1).await;

    assert_eq!(task_status(&harness, &ws, &first).await, "running");
    assert_eq!(task_status(&harness, &ws, &second).await, "ready");
}

// ============================================================ 取消中的 Run 仍佔名額

#[tokio::test]
async fn a_run_that_was_asked_to_cancel_keeps_its_slot_until_the_process_is_confirmed_stopped() {
    let (harness, _) = start_with_accounts(1).await;
    let (ws, goal) = setup(&harness).await;
    let started = fill(&harness, &ws, &goal, 2).await;
    harness.supervise().await;
    let waiting_task = ready_task(&harness, &ws, &goal, "等名額").await;
    dispatch_queued(&harness, &ws, &waiting_task).await;
    harness.process_control.set_stop_result(StopResult::StillAlive);

    let (status, cancelled) = harness
        .send_json(Method::POST, &format!("{}/cancel", run_path(&ws, &started[0].1)), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{cancelled}");
    harness.supervise().await;

    let stuck = overview(&harness).await;
    let stuck_run = runs_of(&stuck, stuck["accounts"][0]["profile_id"].as_str().unwrap())
        .into_iter()
        .find(|r| r["run_id"] == json!(started[0].1))
        .cloned()
        .expect("停不掉的 Run 仍在進行中");
    assert_eq!(stuck_run["cancel_requested"], true);
    assert_eq!(stuck["capacity"]["running"], 2, "程序還沒確認停止，名額沒有釋出");
    assert_eq!(queued_task_ids(&stuck), [waiting_task.clone()]);
    assert_eq!(task_status(&harness, &ws, &waiting_task).await, "ready");

    // 確認停止之後才釋出名額，排隊的 Task 取得它
    harness.process_control.set_stop_result(StopResult::ForceKilled);
    harness.supervise().await;

    assert_eq!(run_view(&harness, &ws, &started[0].1).await["status"], "cancelled");
    assert_eq!(task_status(&harness, &ws, &waiting_task).await, "running");
    assert!(overview(&harness).await["queue"].as_array().unwrap().is_empty());
}

// ============================================================ 容量設定

#[tokio::test]
async fn capacity_defaults_to_reserve_handoff_and_a_switch_survives_a_runtime_restart() {
    let (mut harness, _) = start_with_accounts(1).await;
    let initial = overview(&harness).await;
    assert_eq!(initial["settings"]["mode"], "reserve_handoff", "預設保留交接容量");
    assert_eq!(initial["settings"]["mode_label"], "保留交接容量");
    let modes: Vec<(String, u64, u64, u64)> = initial["settings"]["modes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| {
            (
                m["mode"].as_str().unwrap().to_string(),
                m["per_account_limit"].as_u64().unwrap(),
                m["global_limit"].as_u64().unwrap(),
                m["handoff_reserve"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        modes,
        [("reserve_handoff".to_string(), 2, 6, 1), ("higher_concurrency".to_string(), 3, 8, 0)],
        "設定頁的兩組預設與各自的數字"
    );
    assert_eq!(initial["capacity"]["per_account_limit"], 2);
    assert_eq!(initial["capacity"]["global_limit"], 6);
    assert!(settings_events(&harness).is_empty(), "沒改過就沒有設定事件");

    let switched = set_mode(&harness, "higher_concurrency").await;
    assert_eq!(switched["settings"]["mode"], "higher_concurrency");
    assert_eq!(switched["settings"]["mode_label"], "較高並行");

    harness.restart().await;

    let after = overview(&harness).await;
    assert_eq!(after["settings"]["mode"], "higher_concurrency", "重啟後仍保持");
    assert_eq!(after["capacity"]["per_account_limit"], 3);
    assert_eq!(after["capacity"]["global_limit"], 8);
}

#[tokio::test]
async fn switching_to_the_current_mode_again_is_idempotent_and_appends_no_event() {
    let (harness, _) = start_with_accounts(1).await;

    // 切到目前的模式（預設）：沒有任何變動
    set_mode(&harness, "reserve_handoff").await;
    assert!(settings_events(&harness).is_empty(), "相同模式不追加事件");

    set_mode(&harness, "higher_concurrency").await;
    assert_eq!(settings_events(&harness), [(1, "scheduler_settings_changed".to_string())]);
    set_mode(&harness, "higher_concurrency").await;
    set_mode(&harness, "higher_concurrency").await;
    assert_eq!(settings_events(&harness).len(), 1, "重複切換同一個模式不新增事件");

    set_mode(&harness, "reserve_handoff").await;
    assert_eq!(
        settings_events(&harness),
        [(1, "scheduler_settings_changed".to_string()), (2, "scheduler_settings_changed".to_string())],
        "真的切換才追加事件，版本連續遞增"
    );
}

#[tokio::test]
async fn an_unknown_capacity_mode_is_rejected_and_changes_nothing() {
    let (harness, _) = start_with_accounts(1).await;

    let (status, body) =
        harness.send_json(Method::PUT, SCHEDULING_SETTINGS, Some(json!({ "mode": "unlimited" }))).await;

    assert_eq!((status, error_code(&body)), (StatusCode::BAD_REQUEST, "bad_request"), "{body}");
    assert_eq!(overview(&harness).await["settings"]["mode"], "reserve_handoff");
    assert!(settings_events(&harness).is_empty());
}

// ============================================================ 佇列的持久性

#[tokio::test]
async fn the_queue_survives_a_runtime_restart_and_queued_tasks_are_dispatched_afterwards() {
    let (mut harness, accounts) = start_with_accounts(1).await;
    let (ws, goal) = setup(&harness).await;
    let started = fill(&harness, &ws, &goal, 2).await;
    harness.supervise().await;
    let waiting_task = ready_task(&harness, &ws, &goal, "重啟前排隊").await;
    dispatch_with_priority(&harness, &ws, &waiting_task, 3).await;

    harness.restart().await;

    let after_restart = overview(&harness).await;
    assert_eq!(queued_task_ids(&after_restart), [waiting_task.clone()], "排隊中的 Task 重啟後仍在佇列");
    assert_eq!(after_restart["queue"][0]["priority"], 3, "優先序也保留");
    assert_eq!(reasons(&after_restart["queue"][0]), [pair("capacity", "account_capacity_full")]);
    assert_eq!(task_status(&harness, &ws, &waiting_task).await, "ready");
    harness.supervise().await;
    assert_eq!(harness.process_control.spawn_count(), 2, "重啟不會重複啟動程序，也不會越過上限");
    assert_eq!(account(&overview(&harness).await, &accounts[0])["running"], 2);

    // 名額空出來之後仍會被派出
    finish(&harness, &started[0].1).await;

    assert_eq!(task_status(&harness, &ws, &waiting_task).await, "running");
    assert_eq!(harness.process_control.spawn_count(), 3);
    assert!(overview(&harness).await["queue"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn a_queued_task_that_is_cancelled_leaves_the_queue_and_is_never_dispatched() {
    let (harness, _) = start_with_accounts(1).await;
    let (ws, goal) = setup(&harness).await;
    let started = fill(&harness, &ws, &goal, 2).await;
    harness.supervise().await;
    let cancelled_task = ready_task(&harness, &ws, &goal, "排隊後取消").await;
    let kept_task = ready_task(&harness, &ws, &goal, "仍要執行").await;
    dispatch_queued(&harness, &ws, &cancelled_task).await;
    dispatch_queued(&harness, &ws, &kept_task).await;
    assert_eq!(queued_task_ids(&overview(&harness).await), [cancelled_task.clone(), kept_task.clone()]);

    let (status, body) = harness.send_json(Method::POST, &format!("{}/cancel", task_path(&ws, &cancelled_task)), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    assert_eq!(queued_task_ids(&overview(&harness).await), [kept_task.clone()], "取消的 Task 立刻離開佇列");
    finish(&harness, &started[0].1).await;
    assert_eq!(task_status(&harness, &ws, &cancelled_task).await, "cancelled");
    assert_eq!(task_status(&harness, &ws, &kept_task).await, "running", "名額給了還在排隊的那個");
    finish(&harness, &started[1].1).await;
    assert_eq!(task_status(&harness, &ws, &cancelled_task).await, "cancelled", "之後也不會被派出");
    assert_eq!(harness.process_control.spawn_count(), 3, "沒有為已取消的 Task 啟動程序");
    assert!(overview(&harness).await["queue"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn a_queued_task_whose_contract_changes_returns_to_pending_and_leaves_the_queue() {
    let (harness, _) = start_with_accounts(1).await;
    let (ws, goal) = setup(&harness).await;
    let started = fill(&harness, &ws, &goal, 2).await;
    harness.supervise().await;
    let tid = ready_task(&harness, &ws, &goal, "排隊後改契約").await;
    dispatch_queued(&harness, &ws, &tid).await;

    let (status, body) = harness
        .send_json(
            Method::PUT,
            &format!("{}/contract", task_path(&ws, &tid)),
            Some(json!({ "contract": contract_with("改過的目標", &[]) })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    assert_eq!(task_status(&harness, &ws, &tid).await, "pending", "ready 的 Task 改契約會退回 pending");
    assert!(overview(&harness).await["queue"].as_array().unwrap().is_empty(), "不再是 ready，離開佇列");
    finish(&harness, &started[0].1).await;
    assert_eq!(task_status(&harness, &ws, &tid).await, "pending", "名額空出來也不會被派出");
    assert_eq!(harness.process_control.spawn_count(), 2);
}

// ============================================================ 重複派工

#[tokio::test]
async fn dispatching_a_queued_task_again_is_idempotent_and_keeps_a_single_queue_entry() {
    let (harness, _) = start_with_accounts(1).await;
    let (ws, goal) = setup(&harness).await;
    let started = fill(&harness, &ws, &goal, 2).await;
    harness.supervise().await;
    let tid = ready_task(&harness, &ws, &goal, "重複要求").await;
    let other = ready_task(&harness, &ws, &goal, "另一個排隊的").await;
    dispatch_queued(&harness, &ws, &tid).await;
    let first_overview = overview(&harness).await;
    let requested_at = first_overview["queue"][0]["requested_at"].clone();

    harness.clock.advance_millis(5_000);
    let again = dispatch_queued(&harness, &ws, &tid).await;
    dispatch_queued(&harness, &ws, &other).await;
    let third = dispatch_queued(&harness, &ws, &tid).await;

    assert_eq!(reasons(&again), [pair("capacity", "account_capacity_full")]);
    assert_eq!(reasons(&third), reasons(&again));
    let queued = overview(&harness).await;
    assert_eq!(queued_task_ids(&queued), [tid.clone(), other.clone()], "仍各只有一筆，且沒有因重複要求而插隊或被擠到後面");
    assert_eq!(queued["queue"][0]["requested_at"], requested_at, "重複要求不改變最初要求的時間");
    assert_eq!(task_status(&harness, &ws, &tid).await, "ready");

    // 名額空出來時只會派出一個 Run
    finish(&harness, &started[0].1).await;

    assert_eq!(task_status(&harness, &ws, &tid).await, "running");
    assert_eq!(harness.process_control.spawn_count(), 3);
    let (status, body) = dispatch(&harness, &ws, &tid).await;
    assert_eq!((status, error_code(&body)), (StatusCode::CONFLICT, "task_not_ready"), "已派出的 Task 再派是 409：{body}");
    assert_eq!(queued_task_ids(&overview(&harness).await), [other.clone()]);
}
