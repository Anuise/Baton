//! 情境測試：修正與重試預算（票 36，spec D-16），經由 Runtime harness 的真實 HTTP API 驗證。
//!
//! 三種計數：實作失敗修正 3 次（Worker 以失敗結束、確定性檢查未通過）、驗證不完整重驗 2 次（檢查無法取得證據）、
//! 基礎設施重試 5 次（啟動失敗、程序意外消失；指數退避，上限 10 分鐘）。額度交接（突然限流而結束的 Run）、
//! 恢復嘗試與取消都不計入。超過預算時 Task 進入 blocked，並出現「預算用盡後的下一步」裁定請求。
//!
//! Worker 以 fake AgentAdapter 的事件推進，啟動失敗與程序存活由 fake ProcessControl 決定，確定性檢查的命令由
//! fake CommandRunner 回應，時間由 fake Clock 控制。只斷言外部行為：Task 詳情的計數、Task 狀態、裁定請求與排程佇列。

use baton_domain::quota::{LimitStatus, QuotaWindow};
use baton_domain::run::LEASE_DURATION_MS;
use baton_domain::Timestamp;
use baton_harness::fakes::HangPoint;
use baton_harness::Harness;
use baton_ports::{AdapterEvent, Clock, Liveness};
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

const WORKSPACES: &str = "/api/v1/workspaces";
const SCHEDULING: &str = "/api/v1/scheduling";
const CLOCK_START: Timestamp = Timestamp::from_millis(1_700_000_000_123);
const MINUTE_MS: i64 = 60_000;

// --- 共用輔助 ---

async fn harness() -> Harness {
    let harness = Harness::builder().clock_at(CLOCK_START).start().await;
    // 派工需要一個可派工的帳號（票 19）
    harness.add_account("主要帳號", "main@example.com").await;
    harness
}

/// 建立 Workspace 與 Goal，回傳 (workspace_id, goal_id)。
async fn setup(harness: &Harness) -> (String, String) {
    let dir = harness.data_dir().join("projects").join("main");
    std::fs::create_dir_all(&dir).unwrap();
    let (status, ws) = harness.send_json(Method::POST, WORKSPACES, Some(json!({ "path": dir.to_string_lossy() }))).await;
    assert_eq!(status, StatusCode::CREATED, "{ws}");
    let ws = ws["id"].as_str().unwrap().to_string();
    let (status, goal) =
        harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/goals"), Some(json!({ "title": "目標" }))).await;
    assert_eq!(status, StatusCode::CREATED, "{goal}");
    (ws, goal["id"].as_str().unwrap().to_string())
}

fn filled(text: &str) -> Value {
    json!({ "status": "filled", "text": text })
}

/// 宣告了 `cargo test` 這一項確定性檢查的 Task。
async fn ready_task(harness: &Harness, ws: &str, goal: &str) -> String {
    let contract = json!({
        "objective": filled("修好解析器"),
        "inputs": filled("既有的程式"),
        "scope": filled("解析器模組"),
        "deliverables": filled("修正後的程式"),
        "done_criteria": filled("測試通過"),
        "blocker_handling": filled("卡住時回報"),
        "required_capabilities": [],
        "verification_checks": [{ "kind": "command", "program": "cargo", "args": ["test"], "expected_exit_code": 0 }],
    });
    let body = json!({ "goal_id": goal, "title": "修好解析器", "contract": contract });
    let (status, task) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks"), Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    let tid = task["id"].as_str().unwrap().to_string();
    let (status, ready) = harness.send_json(Method::POST, &format!("{}/ready", task_path(ws, &tid)), None).await;
    assert_eq!(status, StatusCode::OK, "{ready}");
    tid
}

fn task_path(ws: &str, tid: &str) -> String {
    format!("{WORKSPACES}/{ws}/tasks/{tid}")
}

/// 派工並讓 Supervisor 啟動它，回傳 run_id。
async fn start_run(harness: &Harness, ws: &str, tid: &str) -> String {
    let (status, run) = harness.send_json(Method::POST, &format!("{}/dispatch", task_path(ws, tid)), None).await;
    assert_eq!(status, StatusCode::CREATED, "{run}");
    let rid = run["id"].as_str().unwrap().to_string();
    harness.supervise().await;
    rid
}

async fn exits(harness: &Harness, rid: &str, success: bool) {
    harness.agent_adapter.emit(rid, AdapterEvent::Exited { success });
    harness.supervise().await;
}

async fn task_detail(harness: &Harness, ws: &str, tid: &str) -> Value {
    let (status, detail) = harness.send_json(Method::GET, &task_path(ws, tid), None).await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    detail
}

async fn task_status(harness: &Harness, ws: &str, tid: &str) -> String {
    task_detail(harness, ws, tid).await["task"]["status"].as_str().unwrap().to_string()
}

/// Task 詳情中的三種計數：(實作失敗修正, 驗證不完整重驗, 基礎設施重試)。
async fn counts(harness: &Harness, ws: &str, tid: &str) -> (u64, u64, u64) {
    let budget = task_detail(harness, ws, tid).await["budget"].clone();
    let used = |kind: &str| {
        budget["counters"].as_array().unwrap().iter().find(|c| c["kind"] == kind).unwrap()["used"].as_u64().unwrap()
    };
    (used("implementation"), used("verification"), used("infrastructure"))
}

async fn runs_of(harness: &Harness, ws: &str, tid: &str) -> Vec<Value> {
    let (status, list) = harness.send_json(Method::GET, &format!("{WORKSPACES}/{ws}/runs"), None).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    list["runs"].as_array().unwrap().iter().filter(|r| r["task_id"] == tid).cloned().collect()
}

async fn latest_run(harness: &Harness, ws: &str, tid: &str) -> Value {
    runs_of(harness, ws, tid).await.pop().expect("至少有一個 Run")
}

async fn pending_decisions(harness: &Harness, ws: &str) -> Vec<Value> {
    let (status, list) = harness.send_json(Method::GET, &format!("{WORKSPACES}/{ws}/decisions"), None).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    list["decisions"].as_array().unwrap().iter().filter(|d| d["status"] == "pending").cloned().collect()
}

/// 排程佇列中這個 Task 的等待原因代碼；不在佇列中時為 `None`。
async fn queue_reasons(harness: &Harness, tid: &str) -> Option<Vec<String>> {
    let (status, overview) = harness.send_json(Method::GET, SCHEDULING, None).await;
    assert_eq!(status, StatusCode::OK, "{overview}");
    let entry = overview["queue"].as_array().unwrap().iter().find(|q| q["task_id"] == tid)?.clone();
    Some(entry["reasons"].as_array().unwrap().iter().map(|r| r["code"].as_str().unwrap().to_string()).collect())
}

async fn answer(harness: &Harness, ws: &str, request: &Value, option: &str) -> Value {
    let rid = request["id"].as_str().unwrap();
    let body = json!({ "action": "accept", "expected_revision": request["revision"], "option_id": option });
    let (status, result) =
        harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/decisions/{rid}/answer"), Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{result}");
    result
}

/// 確認 Task 因 `kind` 預算用盡而阻擋，並回傳那一個「預算用盡後的下一步」裁定請求。
async fn assert_blocked_by_budget(harness: &Harness, ws: &str, tid: &str, kind: &str) -> Value {
    assert_eq!(task_status(harness, ws, tid).await, "blocked");
    let budget = task_detail(harness, ws, tid).await["budget"].clone();
    assert_eq!(budget["exhausted"], kind, "{budget}");
    let pending = pending_decisions(harness, ws).await;
    assert_eq!(pending.len(), 1, "{pending:?}");
    let request = pending[0].clone();
    assert_eq!(request["id"], budget["decision_request_id"]);
    assert_eq!(request["kind"], "authority");
    assert_eq!(request["authority"]["matter"], "next_step_after_budget", "{request}");
    assert_eq!(request["blocking"][0]["task_id"], tid);
    assert_eq!(request["options"].as_array().unwrap().len(), 2);
    request
}

/// 讓下一次的派工在啟動程序時失敗（基礎設施），並等到退避期限之後讓 Supervisor 處理。
async fn launch_fails_after(harness: &Harness, wait_ms: i64) {
    harness.clock.advance_millis(wait_ms);
    harness.process_control.fail_next_spawn("找不到 claude");
    harness.supervise().await;
}

// --- 情境 ---

#[tokio::test]
async fn implementation_failures_are_fixed_three_times_then_the_task_is_blocked_with_a_decision_request() {
    let harness = harness().await;
    let (ws, goal) = setup(&harness).await;
    let tid = ready_task(&harness, &ws, &goal).await;
    assert_eq!(counts(&harness, &ws, &tid).await, (0, 0, 0), "尚未計入任何一次");
    let detail = task_detail(&harness, &ws, &tid).await;
    let limits: Vec<u64> = detail["budget"]["counters"].as_array().unwrap().iter().map(|c| c["limit"].as_u64().unwrap()).collect();
    assert_eq!(limits, [3, 2, 5], "spec D-16 的預算");

    // 兩次 Worker 以失敗結束、一次確定性檢查未通過：都是實作失敗，三次修正都還在預算內
    for attempt in 1..=3u64 {
        let rid = start_run(&harness, &ws, &tid).await;
        if attempt == 2 {
            harness.command_runner.script_exit("cargo", 101, "test result: FAILED. 1 failed");
            exits(&harness, &rid, true).await;
            harness.supervise().await;
        } else {
            exits(&harness, &rid, false).await;
        }
        assert_eq!(task_status(&harness, &ws, &tid).await, "ready", "第 {attempt} 次失敗後仍可修正");
        assert_eq!(counts(&harness, &ws, &tid).await, (attempt, 0, 0));
        assert!(pending_decisions(&harness, &ws).await.is_empty());
    }

    // 第四次失敗：修正預算已用完
    let rid = start_run(&harness, &ws, &tid).await;
    exits(&harness, &rid, false).await;

    let request = assert_blocked_by_budget(&harness, &ws, &tid, "implementation").await;
    assert_eq!(request["run_id"], rid.as_str(), "裁定請求指向用盡預算的那個 Run");
    assert!(request["question"].as_str().unwrap().contains("實作失敗修正"), "{request}");
    assert_eq!(counts(&harness, &ws, &tid).await, (3, 0, 0), "用盡時計數停在上限");
    // 阻擋的 Task 不會被派工
    let (status, body) = harness.send_json(Method::POST, &format!("{}/dispatch", task_path(&ws, &tid)), None).await;
    assert_ne!(status, StatusCode::CREATED, "{body}");
}

#[tokio::test]
async fn infrastructure_retries_back_off_exponentially_up_to_ten_minutes_then_block() {
    let harness = harness().await;
    let (ws, goal) = setup(&harness).await;
    let tid = ready_task(&harness, &ws, &goal).await;
    harness.process_control.fail_next_spawn("找不到 claude");
    start_run(&harness, &ws, &tid).await;
    assert_eq!(counts(&harness, &ws, &tid).await, (0, 0, 1));
    assert_eq!(task_status(&harness, &ws, &tid).await, "ready");

    // 基礎設施重試自動排回佇列，但要等退避期限：1、2、4、8、10 分鐘
    for (retry, wait_minutes) in [(1u64, 1i64), (2, 2), (3, 4), (4, 8)] {
        let failed_at = harness.clock.now().as_millis();
        let budget = task_detail(&harness, &ws, &tid).await["budget"].clone();
        assert_eq!(budget["retry_not_before"].as_i64(), Some(failed_at + wait_minutes * MINUTE_MS), "第 {retry} 次重試的退避");
        harness.clock.advance_millis(wait_minutes * MINUTE_MS - 1);
        harness.supervise().await;
        assert_eq!(queue_reasons(&harness, &tid).await, Some(vec!["retry_backoff".to_string()]), "退避期間不派工");
        assert_eq!(runs_of(&harness, &ws, &tid).await.len() as u64, retry);
        launch_fails_after(&harness, 1).await;
        assert_eq!(runs_of(&harness, &ws, &tid).await.len() as u64, retry + 1, "退避期限一到就自動重試");
        assert_eq!(counts(&harness, &ws, &tid).await, (0, 0, retry + 1));
    }
    let failed_at = harness.clock.now().as_millis();
    let budget = task_detail(&harness, &ws, &tid).await["budget"].clone();
    assert_eq!(budget["retry_not_before"].as_i64(), Some(failed_at + 10 * MINUTE_MS), "退避上限 10 分鐘");

    // 第六次（第 5 次重試之後）仍失敗：預算用盡
    launch_fails_after(&harness, 10 * MINUTE_MS).await;

    assert_blocked_by_budget(&harness, &ws, &tid, "infrastructure").await;
    assert_eq!(counts(&harness, &ws, &tid).await, (0, 0, 5));
    harness.clock.advance_millis(60 * MINUTE_MS);
    harness.supervise().await;
    assert_eq!(runs_of(&harness, &ws, &tid).await.len(), 6, "用盡之後不再自動重試");
}

#[tokio::test]
async fn the_three_counters_do_not_affect_each_other() {
    let harness = harness().await;
    let (ws, goal) = setup(&harness).await;
    let tid = ready_task(&harness, &ws, &goal).await;

    // 基礎設施：程序在 lease 過期後確認已經不在
    let rid = start_run(&harness, &ws, &tid).await;
    let spawned = harness.process_control.spawned().pop().unwrap();
    let identity = harness.process_control.identity_of(&spawned.command_id).unwrap();
    harness.clock.advance_millis(LEASE_DURATION_MS + 1);
    harness.process_control.script_liveness(&identity, Liveness::Dead);
    harness.supervise().await;
    assert_eq!(latest_run(&harness, &ws, &tid).await["id"], rid.as_str());
    assert_eq!(counts(&harness, &ws, &tid).await, (0, 0, 1));

    // 實作失敗：退避期限過後自動重試的 Run 以失敗結束
    harness.clock.advance_millis(MINUTE_MS);
    harness.supervise().await;
    let retried = latest_run(&harness, &ws, &tid).await;
    assert_eq!(retried["status"], "running", "{retried}");
    exits(&harness, retried["id"].as_str().unwrap(), false).await;
    assert_eq!(counts(&harness, &ws, &tid).await, (1, 0, 1));

    // 驗證不完整：檢查命令無法啟動，取不到證據；Task 留在驗證中重驗，兩次重驗都還在預算內
    let rid = start_run(&harness, &ws, &tid).await;
    harness.command_runner.script_start_failure("cargo", "找不到 cargo");
    // Run 結束的同一次處理就執行第一次檢查，之後每次處理重驗一次
    exits(&harness, &rid, true).await;
    for reverify in 1..=2u64 {
        assert_eq!(task_status(&harness, &ws, &tid).await, "verifying", "驗證不完整不算通過也不算產品缺陷");
        assert_eq!(counts(&harness, &ws, &tid).await, (1, reverify, 1));
        harness.supervise().await;
    }
    let (_, view) = harness.send_json(Method::GET, &format!("{}/verification", task_path(&ws, &tid)), None).await;
    let record = &view["records"].as_array().unwrap().last().unwrap().clone();
    assert_eq!(record["outcome"], "inconclusive", "{record}");
    assert_eq!(record["checks"][0]["status"], "inconclusive");

    // 第三次仍取不到證據：驗證預算用盡，另外兩種計數不變
    let request = assert_blocked_by_budget(&harness, &ws, &tid, "verification").await;
    assert!(request["question"].as_str().unwrap().contains("驗證不完整重驗"), "{request}");
    assert_eq!(counts(&harness, &ws, &tid).await, (1, 2, 1));

    // 核准後只重設驗證的計數，Task 回到驗證中重新執行檢查（這次環境修好了）
    harness.command_runner.script_exit("cargo", 0, "ok");
    answer(&harness, &ws, &request, "approve").await;
    assert_eq!(counts(&harness, &ws, &tid).await, (1, 0, 1));
    assert_eq!(task_status(&harness, &ws, &tid).await, "verifying");
    harness.supervise().await;
    assert_eq!(task_status(&harness, &ws, &tid).await, "completed");
}

#[tokio::test]
async fn quota_handoff_recovery_and_cancellation_never_increase_any_counter() {
    let mut harness = harness().await;
    harness.add_account("第二帳號", "second@example.com").await;
    let (ws, goal) = setup(&harness).await;
    let tid = ready_task(&harness, &ws, &goal).await;

    // 額度交接：帳號突然限流，Worker 隨即以失敗結束；這不是實作失敗
    let rid = start_run(&harness, &ws, &tid).await;
    let limited = AdapterEvent::RateLimited {
        window: Some(QuotaWindow::FiveHour),
        status: LimitStatus::Exhausted,
        used: None,
        reset_at_millis: None,
        raw: "{\"type\":\"rate_limit_event\"}".into(),
    };
    harness.agent_adapter.emit(&rid, limited);
    harness.agent_adapter.emit(&rid, AdapterEvent::Error { message: "usage limit reached".into() });
    exits(&harness, &rid, false).await;
    let run = runs_of(&harness, &ws, &tid).await.into_iter().find(|r| r["id"] == rid.as_str()).unwrap();
    assert_eq!(run["status"], "interrupted", "限流而結束的 Run 是中斷，不是失敗：{run}");
    assert_eq!(counts(&harness, &ws, &tid).await, (0, 0, 0), "交接不增加任何計數");

    // 由另一個帳號的 successor 自動接手（票 29），這次正常結束
    let successor = latest_run(&harness, &ws, &tid).await;
    assert_eq!(successor["status"], "running", "{successor}");
    assert_ne!(successor["profile_id"], run["profile_id"], "換到有額度的帳號");
    let second = successor["id"].as_str().unwrap().to_string();
    harness.command_runner.script_exit("cargo", 101, "FAILED");
    exits(&harness, &second, true).await;
    harness.supervise().await;
    assert_eq!(counts(&harness, &ws, &tid).await, (1, 0, 0), "檢查未通過才是實作失敗");

    // 恢復嘗試：Runtime 在啟動程序之前當掉，重建後找不到程序而中斷 Run
    harness.process_control.hang_next_spawn_at(HangPoint::BeforeSpawn);
    let (status, _) = harness.send_json(Method::POST, &format!("{}/dispatch", task_path(&ws, &tid)), None).await;
    assert_eq!(status, StatusCode::CREATED);
    harness.supervise_until(harness.process_control.wait_for_spawn_hang()).await;
    harness.restart().await;
    harness.supervise().await;
    harness.supervise().await;
    let recovered = latest_run(&harness, &ws, &tid).await;
    assert_eq!(recovered["status"], "interrupted", "{recovered}");
    assert_eq!(counts(&harness, &ws, &tid).await, (1, 0, 0), "恢復嘗試不計入");

    // 使用者取消 Run
    let rid = start_run(&harness, &ws, &tid).await;
    let (status, body) =
        harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/runs/{rid}/cancel"), None).await;
    assert!(status.is_success(), "{body}");
    harness.supervise().await;
    harness.supervise().await;
    assert_eq!(latest_run(&harness, &ws, &tid).await["status"], "cancelled");
    assert_eq!(counts(&harness, &ws, &tid).await, (1, 0, 0), "取消不計入");
    assert!(pending_decisions(&harness, &ws).await.is_empty());
}

#[tokio::test]
async fn approving_the_budget_decision_resets_only_that_counter_and_lets_the_task_continue() {
    let harness = harness().await;
    let (ws, goal) = setup(&harness).await;
    let tid = ready_task(&harness, &ws, &goal).await;
    harness.process_control.fail_next_spawn("找不到 claude");
    start_run(&harness, &ws, &tid).await;
    harness.clock.advance_millis(MINUTE_MS);
    harness.supervise().await;
    let rid = latest_run(&harness, &ws, &tid).await["id"].as_str().unwrap().to_string();
    exits(&harness, &rid, false).await;
    for _ in 0..3 {
        let rid = start_run(&harness, &ws, &tid).await;
        exits(&harness, &rid, false).await;
    }
    let request = assert_blocked_by_budget(&harness, &ws, &tid, "implementation").await;
    assert_eq!(counts(&harness, &ws, &tid).await, (3, 0, 1));

    let result = answer(&harness, &ws, &request, "approve").await;

    assert_eq!(result["resumed_task_ids"], json!([tid]), "{result}");
    let budget = task_detail(&harness, &ws, &tid).await["budget"].clone();
    assert_eq!((budget["exhausted"].clone(), budget["decision_request_id"].clone()), (Value::Null, Value::Null));
    assert_eq!(counts(&harness, &ws, &tid).await, (0, 0, 1), "只重設用盡的實作計數");
    // Task 回到 ready 並已排回派工佇列
    harness.supervise().await;
    assert_eq!(task_status(&harness, &ws, &tid).await, "running");
}

#[tokio::test]
async fn rejecting_the_budget_decision_keeps_the_task_blocked() {
    let harness = harness().await;
    let (ws, goal) = setup(&harness).await;
    let tid = ready_task(&harness, &ws, &goal).await;
    for _ in 0..4 {
        let rid = start_run(&harness, &ws, &tid).await;
        exits(&harness, &rid, false).await;
    }
    let request = assert_blocked_by_budget(&harness, &ws, &tid, "implementation").await;

    answer(&harness, &ws, &request, "reject").await;
    harness.supervise().await;

    assert_eq!(task_status(&harness, &ws, &tid).await, "blocked", "維持阻擋，由使用者自行處理");
    assert_eq!(counts(&harness, &ws, &tid).await, (3, 0, 0));
    assert!(pending_decisions(&harness, &ws).await.is_empty());
}

#[tokio::test]
async fn the_controller_cannot_cancel_a_task_whose_budget_is_exhausted_without_a_decision() {
    let harness = harness().await;
    // 只用來開 Controller CLI 的帳號（Worker 派工用主要帳號）
    let controller_account = harness.add_account("Controller 帳號", "controller@example.com").await;
    let (ws, goal) = setup(&harness).await;
    let tid = ready_task(&harness, &ws, &goal).await;
    for _ in 0..4 {
        let rid = start_run(&harness, &ws, &tid).await;
        exits(&harness, &rid, false).await;
    }
    let budget_request = assert_blocked_by_budget(&harness, &ws, &tid, "implementation").await;
    // 以 Controller CLI 取得 Controller 憑證
    let (_, controller) = harness.open_controller_cli(&ws, &controller_account).await;

    // 取消再另建相同的 Task 會拿到全新的預算：取消預算已用盡的 Task 也是「預算用盡後的下一步」，必須送裁定（R151 第 9 項）
    let (status, refused) = harness.call_tool(&controller, "cancel_task", json!({ "task_id": tid })).await;
    assert_eq!((status, refused["error"].as_str()), (StatusCode::CONFLICT, Some("requires_decision")), "{refused}");
    assert_eq!(refused["matter"], "next_step_after_budget");
    assert_ne!(refused["decision_request_id"], budget_request["id"], "Controller 的請求與 Runtime 的預算請求分開");
    assert_eq!(task_status(&harness, &ws, &tid).await, "blocked");

    // 使用者核准取消之後，Controller 才能取消
    let pending = pending_decisions(&harness, &ws).await;
    let cancel_request = pending.iter().find(|d| d["id"] == refused["decision_request_id"]).cloned().expect("Controller 的請求");
    answer(&harness, &ws, &cancel_request, "approve").await;
    let (status, body) = harness.call_tool(&controller, "cancel_task", json!({ "task_id": tid })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(task_status(&harness, &ws, &tid).await, "cancelled");
}
