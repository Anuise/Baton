//! 情境測試：突然限流與全部帳號受限（票 29），經由 Runtime harness 的真實 HTTP API 驗證。
//!
//! 「突然限流」是 scripted fake AgentAdapter 送出的 CLI 已達上限訊號（`RateLimited { status: Exhausted }`）；其他帳號的額度由
//! 同樣的訊號或 fake QuotaObserver 的樣本決定；舊程序停不停得下來由 fake ProcessControl 決定；時間由 fake Clock 決定。
//! 「重啟 Runtime」= 丟棄 Runtime、保留資料目錄與全部 fake。
//!
//! 每個測試名稱對應票 29 的驗收條件：
//! - 突然限流後由另一帳號從檢查點接手：
//!   `a_sudden_limit_hands_the_task_to_another_account_from_the_latest_checkpoint`、
//!   `a_worker_that_keeps_waiting_on_a_limited_account_is_stopped_and_the_cooldown_does_not_apply`
//! - 全部受限時工作保存並等待，UI 顯示預計重評時間：
//!   `when_every_account_is_limited_the_task_waits_until_one_minute_after_the_earliest_reset`、
//!   `with_unknown_reset_times_the_wait_backs_off_and_survives_a_restart`、
//!   `cancelling_a_task_that_waits_for_resources_ends_the_wait`
//! - 資源恢復不會解除完全退出：`a_resource_recovery_never_lifts_a_full_exit`

use baton_domain::agent::{TOOL_CONFIRM_HANDOFF, TOOL_SAVE_CHECKPOINT};
use baton_domain::quota::{LimitStatus, QuotaWindow};
use baton_domain::Timestamp;
use baton_harness::Harness;
use baton_ports::{AdapterEvent, Clock, Confidence, ProcessIdentity, QuotaSample, QuotaSource};
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

const WORKSPACES: &str = "/api/v1/workspaces";
const LIFECYCLE: &str = "/api/v1/lifecycle";
const CLOCK_START: Timestamp = Timestamp::from_millis(1_700_000_000_000);
const SECOND: i64 = 1000;
const MINUTE: i64 = 60 * SECOND;

// --- 共用輔助 ---

/// 一個 Workspace、一個 ready 的 Task 與指定數量的帳號（依建立順序 A、B、C）。
struct World {
    harness: Harness,
    accounts: Vec<String>,
    ws: String,
    task: String,
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

async fn world(accounts: usize) -> World {
    let harness = Harness::builder().clock_at(CLOCK_START).start().await;
    let mut ids = Vec::new();
    for (index, name) in ["A", "B", "C"].into_iter().take(accounts).enumerate() {
        ids.push(harness.add_account(&format!("帳號 {name}"), &format!("account{index}@example.com")).await);
    }
    let dir = harness.data_dir().join("projects").join("main");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.to_string_lossy().into_owned();
    let (status, body) = harness.send_json(Method::POST, WORKSPACES, Some(json!({ "path": path }))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let ws = body["id"].as_str().unwrap().to_string();
    let (status, goal) =
        harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/goals"), Some(json!({ "title": "目標" }))).await;
    assert_eq!(status, StatusCode::CREATED, "{goal}");
    let goal = goal["id"].as_str().unwrap().to_string();
    let body = json!({ "goal_id": goal, "title": "整理資料", "contract": contract() });
    let (status, task) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks"), Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    let task = task["id"].as_str().unwrap().to_string();
    let (status, ready) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks/{task}/ready"), None).await;
    assert_eq!(status, StatusCode::OK, "{ready}");
    World { harness, accounts: ids, ws, task }
}

impl World {
    fn h(&self) -> &Harness {
        &self.harness
    }

    /// 派出 Task 並讓 Supervisor 啟動它，回傳 Run 識別。
    async fn start_run(&self) -> String {
        let (status, run) =
            self.h().send_json(Method::POST, &format!("{WORKSPACES}/{}/tasks/{}/dispatch", self.ws, self.task), None).await;
        assert_eq!(status, StatusCode::CREATED, "{run}");
        let rid = run["id"].as_str().unwrap().to_string();
        self.h().supervise().await;
        assert_eq!(self.run(&rid).await["status"], "running");
        rid
    }

    async fn runs(&self) -> Vec<Value> {
        let (status, body) = self.h().send_json(Method::GET, &format!("{WORKSPACES}/{}/runs", self.ws), None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body["runs"].as_array().unwrap().clone()
    }

    async fn run(&self, rid: &str) -> Value {
        self.runs().await.into_iter().find(|r| r["id"] == rid).unwrap()
    }

    async fn board(&self) -> Value {
        let (status, body) = self.h().send_json(Method::GET, &format!("{WORKSPACES}/{}/tasks", self.ws), None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body
    }

    async fn handoffs(&self) -> Vec<Value> {
        self.board().await["handoffs"].as_array().unwrap().clone()
    }

    async fn latest_handoff(&self) -> Value {
        self.handoffs().await.last().cloned().expect("應該有交接紀錄")
    }

    async fn task_status(&self) -> String {
        let board = self.board().await;
        let task = board["tasks"].as_array().unwrap().iter().find(|t| t["id"] == self.task.as_str()).unwrap().clone();
        task["status"].as_str().unwrap().to_string()
    }

    async fn gate_of(&self, profile: &str) -> Value {
        let (status, body) = self.h().send_json(Method::GET, "/api/v1/scheduling", None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let accounts = body["accounts"].as_array().unwrap();
        accounts.iter().find(|a| a["profile_id"] == profile).unwrap()["quota_gate"].clone()
    }

    fn token(&self, rid: &str) -> String {
        self.h().agent_credential(rid).expect("Run 已啟動，應該有 Agent 憑證")
    }

    async fn checkpoint(&self, rid: &str, summary: &str, details: &str) {
        let (status, body) = self
            .h()
            .call_tool(&self.token(rid), TOOL_SAVE_CHECKPOINT, json!({ "summary": summary, "details": details }))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }

    /// CLI 回報這個 Run 的帳號已達上限（突然限流）；`reset_at` 是 CLI 給的重置時間（可能沒有）。
    fn limit_reached(&self, rid: &str, reset_at: Option<Timestamp>) {
        self.h().agent_adapter.emit(
            rid,
            AdapterEvent::RateLimited {
                window: Some(QuotaWindow::FiveHour),
                status: LimitStatus::Exhausted,
                used: None,
                reset_at_millis: reset_at.map(Timestamp::as_millis),
                raw: "{\"type\":\"rate_limit_event\",\"status\":\"rejected\"}".into(),
            },
        );
    }

    /// 帳號的外部額度觀測（下一次輪詢時生效）。
    fn observe(&self, profile: &str, used: f64, reset_at: Option<Timestamp>) {
        let sample = QuotaSample {
            source: QuotaSource::Observed,
            observed_at: self.h().clock.now(),
            used: Some(used),
            reset_at,
            confidence: Confidence::Observed,
        };
        self.h().quota_observer.set_sample(profile, sample);
    }

    fn identity_of(&self, run: &Value) -> ProcessIdentity {
        self.h().process_control.identity_of(run["command_id"].as_str().unwrap()).unwrap()
    }

    /// Run 的 Prompt（fake Adapter 接上 Run 時收到的 RunSpec；接上發生在啟動後的下一次處理）。
    async fn prompt_of(&self, rid: &str) -> String {
        self.h().supervise().await;
        self.h().agent_adapter.launched().into_iter().find(|spec| spec.run_id == rid).expect("Run 已接上").prompt
    }

    fn advance(&self, millis: i64) {
        self.h().clock.advance_millis(millis);
    }

    fn at(&self, millis: i64) -> Timestamp {
        CLOCK_START.plus_millis(millis)
    }

    async fn successor_of(&self, handoff: &Value) -> Value {
        self.run(handoff["to_run_id"].as_str().expect("已轉移的交接有 successor")).await
    }

    /// 任何時刻 Task 最多一個尚未結束的 Run（也就是最多一個 writer）。
    async fn assert_single_writer(&self) {
        let unfinished: Vec<Value> = self
            .runs()
            .await
            .into_iter()
            .filter(|r| r["task_id"] == self.task.as_str())
            .filter(|r| ["queued", "starting", "running"].contains(&r["status"].as_str().unwrap()))
            .collect();
        assert!(unfinished.len() <= 1, "同一個 Task 出現兩個 writer：{unfinished:?}");
    }

    async fn budget_used(&self) -> Vec<i64> {
        let board = self.board().await;
        let budget = board["budgets"].as_array().unwrap().iter().find(|b| b["task_id"] == self.task.as_str()).cloned();
        budget.map_or_else(Vec::new, |b| {
            b["counters"].as_array().unwrap().iter().map(|c| c["used"].as_i64().unwrap()).collect()
        })
    }

    fn event_count(&self, event_type: &str) -> i64 {
        self.h().db().query_row("SELECT COUNT(*) FROM event WHERE event_type = ?1", [event_type], |r| r.get(0)).unwrap()
    }
}

// --- 突然限流後由另一帳號從檢查點接手 ---

#[tokio::test]
async fn a_sudden_limit_hands_the_task_to_another_account_from_the_latest_checkpoint() {
    let w = world(2).await;
    let (a, b) = (w.accounts[0].clone(), w.accounts[1].clone());
    let first = w.start_run().await;
    assert_eq!(w.run(&first).await["profile_id"], a.as_str());
    w.checkpoint(&first, "完成步驟一", "已整理三個產品").await;

    // 帳號 A 突然達到上限：Worker 來不及保存任何進度或最後一輪摘要，CLI 隨即以失敗結束
    w.advance(MINUTE);
    w.limit_reached(&first, None);
    w.h().agent_adapter.emit(&first, AdapterEvent::Error { message: "usage limit reached".into() });
    w.h().agent_adapter.emit(&first, AdapterEvent::Exited { success: false });
    w.h().supervise().await;

    // 事件保存：限流訊號記為帳號 A 的已觀測額度，門檻以突然限流進入 draining
    let gate = w.gate_of(&a).await;
    assert_eq!((gate["state"].as_str(), gate["reason"].as_str()), (Some("draining"), Some("limit_reached")));
    let raw: String = w
        .h()
        .db()
        .query_row("SELECT raw FROM quota_observation WHERE raw_ref = ?1 AND status = 'exhausted'", [format!("run:{first}")], |r| r.get(0))
        .unwrap();
    assert!(raw.contains("rate_limit_event"), "{raw}");

    // 舊 Run 是交接結束，不是失敗；由帳號 B 的 successor 從最近的有效檢查點接手
    let old = w.run(&first).await;
    assert_eq!((old["status"].as_str(), old["end_reason"].as_str()), (Some("interrupted"), Some("handed_off")));
    let handoff = w.latest_handoff().await;
    assert_eq!(handoff["reason"], "limit_reached");
    assert_eq!((handoff["phase"].as_str(), handoff["stop_outcome"].as_str()), (Some("transferred"), Some("exited")));
    assert_eq!(handoff["checkpoint"]["summary"], "完成步驟一");
    let successor = w.successor_of(&handoff).await;
    assert_eq!((successor["profile_id"].as_str(), successor["status"].as_str()), (Some(b.as_str()), Some("running")));
    assert_eq!(w.task_status().await, "running");
    w.assert_single_writer().await;

    // Prompt 帶著檢查點，說明舊 Worker 沒有最後一輪摘要，並要求核對後確認接手
    let rid = successor["id"].as_str().unwrap().to_string();
    let prompt = w.prompt_of(&rid).await;
    assert!(prompt.contains("完成步驟一") && prompt.contains("已整理三個產品"), "{prompt}");
    assert!(prompt.contains("最後一輪") && prompt.contains(TOOL_CONFIRM_HANDOFF), "{prompt}");
    let body = json!({ "completed": "三個產品", "incomplete": "其餘兩個", "unknown_side_effects": "無" });
    let (status, confirmed) = w.h().call_tool(&w.token(&rid), TOOL_CONFIRM_HANDOFF, body).await;
    assert_eq!(status, StatusCode::OK, "{confirmed}");

    // 突然限流不是失敗：沒有 run_failed，三種預算都沒有計入
    assert_eq!(w.event_count("run_failed"), 0);
    assert!(w.budget_used().await.iter().all(|used| *used == 0));
}

#[tokio::test]
async fn a_worker_that_keeps_waiting_on_a_limited_account_is_stopped_and_the_cooldown_does_not_apply() {
    let w = world(3).await;
    let (b, c) = (w.accounts[1].clone(), w.accounts[2].clone());
    let first = w.start_run().await;

    // 先有一次一般交接（A 達 90%）：從 A 換到 B，之後 10 分鐘內是冷卻期
    w.h().agent_adapter.emit(
        &first,
        AdapterEvent::RateLimited { window: Some(QuotaWindow::FiveHour), status: LimitStatus::Allowed, used: Some(0.9), reset_at_millis: None, raw: "{}".into() },
    );
    w.h().supervise().await;
    w.checkpoint(&first, "A 上的進度", "").await;
    w.h().supervise().await;
    let second = w.latest_handoff().await["to_run_id"].as_str().unwrap().to_string();
    assert_eq!(w.run(&second).await["profile_id"], b.as_str());
    w.checkpoint(&second, "B 上的進度", "做到第三步").await;

    // 兩分鐘後 B 突然達到上限，但 CLI 沒有結束（在等額度恢復）：Runtime 立刻查證並停止它，不等冷卻、不等新的 checkpoint
    w.advance(2 * MINUTE);
    w.limit_reached(&second, None);
    w.h().supervise().await;

    let old = w.run(&second).await;
    assert!(w.h().process_control.stop_calls().contains(&w.identity_of(&old)), "舊程序被要求停止");
    assert_eq!((old["status"].as_str(), old["end_reason"].as_str()), (Some("interrupted"), Some("handed_off")));
    let handoff = w.latest_handoff().await;
    assert_eq!((handoff["reason"].as_str(), handoff["phase"].as_str()), (Some("limit_reached"), Some("transferred")));
    assert_eq!(handoff["stop_outcome"], "stopped_gracefully");
    assert_eq!(handoff["checkpoint"]["summary"], "B 上的進度");
    let successor = w.successor_of(&handoff).await;
    assert_eq!((successor["profile_id"].as_str(), successor["status"].as_str()), (Some(c.as_str()), Some("running")));
    assert!(w.prompt_of(successor["id"].as_str().unwrap()).await.contains("做到第三步"));
    w.assert_single_writer().await;
}

// --- 全部受限時工作保存並等待，UI 顯示預計重評時間 ---

#[tokio::test]
async fn when_every_account_is_limited_the_task_waits_until_one_minute_after_the_earliest_reset() {
    let w = world(2).await;
    let (a, b) = (w.accounts[0].clone(), w.accounts[1].clone());
    // 帳號 B 的外部觀測已達上限，兩小時後重置
    w.observe(&b, 1.0, Some(w.at(120 * MINUTE)));
    let first = w.start_run().await;
    assert_eq!(w.run(&first).await["profile_id"], a.as_str());
    w.checkpoint(&first, "完成步驟一", "已整理三個產品").await;

    // 帳號 A 突然達到上限，CLI 說 30 分鐘後重置
    w.limit_reached(&first, Some(w.at(30 * MINUTE)));
    w.h().agent_adapter.emit(&first, AdapterEvent::Exited { success: false });
    w.h().supervise().await;

    // 工作保存並等待：舊 Run 停下、Task 進入 waiting_resource，交接停在等待資源
    assert_eq!(w.task_status().await, "waiting_resource");
    let old = w.run(&first).await;
    assert_eq!((old["status"].as_str(), old["end_reason"].as_str()), (Some("interrupted"), Some("handed_off_waiting_resource")));
    let waiting = w.latest_handoff().await;
    assert_eq!((waiting["phase"].as_str(), waiting["reason"].as_str()), (Some("waiting_resource"), Some("limit_reached")));
    assert_eq!(waiting["to_profile_id"], "", "沒有承接帳號");
    assert!(waiting["to_run_id"].is_null());
    assert_eq!(waiting["checkpoint"]["summary"], "完成步驟一");
    // UI 顯示的預計重評時間：最早的重置時間（A 的 30 分鐘）之後 1 分鐘
    assert_eq!(waiting["next_check_at"], w.at(31 * MINUTE).as_millis());
    assert_eq!(waiting["wait_reset_at"], w.at(30 * MINUTE).as_millis());
    assert_eq!(waiting["wait_attempts"], 0);
    let reasons = waiting["wait_reasons"].as_array().unwrap();
    assert!(reasons.iter().any(|r| r["profile_id"] == a.as_str()) && reasons.iter().any(|r| r["profile_id"] == b.as_str()), "{reasons:?}");
    assert_eq!(w.h().process_control.spawn_count(), 1);
    assert!(w.budget_used().await.iter().all(|used| *used == 0), "等待資源不消耗預算");

    // 重評時間之前不派工
    w.advance(31 * MINUTE - SECOND);
    w.h().supervise().await;
    assert_eq!(w.task_status().await, "waiting_resource");
    assert_eq!(w.h().process_control.spawn_count(), 1);

    // 重置後 1 分鐘：A 的訊號已失效，由 successor 從檢查點接手
    w.advance(SECOND);
    w.h().supervise().await;
    let handoff = w.latest_handoff().await;
    assert_eq!(handoff["id"], waiting["id"], "同一筆交接從等待中轉移");
    assert_eq!(handoff["phase"], "transferred");
    let successor = w.successor_of(&handoff).await;
    assert_eq!((successor["profile_id"].as_str(), successor["status"].as_str()), (Some(a.as_str()), Some("running")));
    assert_eq!(successor["generation"], 2);
    assert_eq!(w.task_status().await, "running");
    let prompt = w.prompt_of(successor["id"].as_str().unwrap()).await;
    assert!(prompt.contains("完成步驟一") && prompt.contains("已整理三個產品"), "{prompt}");
    w.assert_single_writer().await;
}

#[tokio::test]
async fn with_unknown_reset_times_the_wait_backs_off_and_survives_a_restart() {
    let mut w = world(2).await;
    let b = w.accounts[1].clone();
    w.observe(&b, 1.0, None);
    let first = w.start_run().await;
    w.checkpoint(&first, "只做了一半", "").await;

    w.limit_reached(&first, None);
    w.h().agent_adapter.emit(&first, AdapterEvent::Exited { success: false });
    w.h().supervise().await;
    let waiting = w.latest_handoff().await;
    assert_eq!(waiting["phase"], "waiting_resource");
    assert!(waiting["wait_reset_at"].is_null(), "重置時間未知");
    assert_eq!(waiting["next_check_at"], w.at(5 * MINUTE).as_millis(), "退避從 5 分鐘開始");

    // 5 分鐘後仍然全部受限：下一次退避加倍為 10 分鐘
    w.advance(5 * MINUTE);
    w.h().supervise().await;
    let again = w.latest_handoff().await;
    assert_eq!((again["phase"].as_str(), again["wait_attempts"].as_i64()), (Some("waiting_resource"), Some(1)));
    assert_eq!(again["next_check_at"], w.at(15 * MINUTE).as_millis());
    assert_eq!(w.h().process_control.spawn_count(), 1);

    // 等待中重啟 Runtime：等待與排定的時間都保留
    w.harness.restart().await;
    w.h().supervise().await;
    assert_eq!(w.task_status().await, "waiting_resource");
    assert_eq!(w.latest_handoff().await["next_check_at"], w.at(15 * MINUTE).as_millis());

    // B 在 12 分鐘時恢復，但要到排定的 15 分鐘才重新評估
    w.advance(7 * MINUTE);
    w.observe(&b, 0.1, None);
    w.h().supervise().await;
    assert_eq!(w.task_status().await, "waiting_resource");
    w.advance(3 * MINUTE);
    w.h().supervise().await;
    let handoff = w.latest_handoff().await;
    assert_eq!(handoff["phase"], "transferred");
    let successor = w.successor_of(&handoff).await;
    assert_eq!((successor["profile_id"].as_str(), successor["status"].as_str()), (Some(b.as_str()), Some("running")));
    assert!(w.prompt_of(successor["id"].as_str().unwrap()).await.contains("只做了一半"));
    w.assert_single_writer().await;
}

#[tokio::test]
async fn cancelling_a_task_that_waits_for_resources_ends_the_wait() {
    let w = world(1).await;
    let first = w.start_run().await;
    w.limit_reached(&first, None);
    w.h().agent_adapter.emit(&first, AdapterEvent::Exited { success: false });
    w.h().supervise().await;
    assert_eq!(w.task_status().await, "waiting_resource");

    let (status, body) =
        w.h().send_json(Method::POST, &format!("{WORKSPACES}/{}/tasks/{}/cancel", w.ws, w.task), None).await;
    assert!(status.is_success(), "{body}");
    w.h().supervise().await;

    assert_eq!(w.task_status().await, "cancelled");
    let handoff = w.latest_handoff().await;
    assert_eq!(handoff["phase"], "abandoned");
    assert!(handoff["abandoned_reason"].as_str().unwrap().contains("cancelled"), "{handoff}");
    // 之後資源恢復也不會再派工
    w.advance(6 * 60 * MINUTE);
    w.h().supervise().await;
    assert_eq!(w.h().process_control.spawn_count(), 1);
}

// --- 資源恢復不會解除完全退出 ---

#[tokio::test]
async fn a_resource_recovery_never_lifts_a_full_exit() {
    let mut w = world(2).await;
    let b = w.accounts[1].clone();
    w.observe(&b, 1.0, None);
    let first = w.start_run().await;
    w.checkpoint(&first, "退出前的進度", "").await;
    w.limit_reached(&first, None);
    w.h().agent_adapter.emit(&first, AdapterEvent::Exited { success: false });
    w.h().supervise().await;
    assert_eq!(w.task_status().await, "waiting_resource");

    // 完全退出：停止意圖持久化
    let (status, body) = w.h().send_json(Method::POST, &format!("{LIFECYCLE}/exit"), None).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    w.h().wait_for_exit().await;

    // 資源恢復、也過了重評時間：停止意圖優先，不建立任何 Worker
    w.observe(&b, 0.1, None);
    w.advance(10 * MINUTE);
    w.h().supervise().await;
    w.advance(10 * MINUTE);
    w.h().supervise().await;
    assert_eq!(w.h().process_control.spawn_count(), 1, "資源恢復不能解除完全退出");
    assert_eq!(w.task_status().await, "waiting_resource");
    assert_eq!(w.latest_handoff().await["phase"], "waiting_resource");
    let lifecycle: Value = w.h().get_json(LIFECYCLE).await;
    assert_eq!(lifecycle["stop_intent_active"], true, "{lifecycle}");

    // 重新開啟（Workspace 預設不自動續跑）：仍等使用者按「繼續工作」
    w.harness.restart().await;
    w.h().supervise().await;
    w.advance(10 * MINUTE);
    w.h().supervise().await;
    assert_eq!(w.h().process_control.spawn_count(), 1, "等待「繼續工作」期間資源恢復也不派工");
    assert_eq!(w.task_status().await, "waiting_resource");

    // 使用者按「繼續工作」：下一次處理就由 B 從檢查點接手
    let (status, body) = w.h().send_json(Method::POST, &format!("{WORKSPACES}/{}/resume", w.ws), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    w.h().supervise().await;
    let handoff = w.latest_handoff().await;
    assert_eq!(handoff["phase"], "transferred");
    let successor = w.successor_of(&handoff).await;
    assert_eq!((successor["profile_id"].as_str(), successor["status"].as_str()), (Some(b.as_str()), Some("running")));
    assert!(w.prompt_of(successor["id"].as_str().unwrap()).await.contains("退出前的進度"));
}
