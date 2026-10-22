//! 情境測試：檢查點與主動跨帳號交接（票 28），經由 Runtime harness 的真實 HTTP API 驗證。
//!
//! 額度由 scripted fake AgentAdapter 的 CLI 限流訊號（`RateLimited`）或 fake QuotaObserver 的樣本產生；Worker 以真實的
//! Agent API（範圍憑證）保存 checkpoint 與確認接手；舊程序停不停得下來由 fake ProcessControl 決定；時間由 fake Clock 決定，
//! 測試決定何時讓 Supervisor 處理一次。「重啟 Runtime」= 丟棄 Runtime、保留資料目錄與全部 fake。
//!
//! 每個測試名稱對應票 28 的驗收條件：
//! - 模擬接近五小時與週上限，皆觸發 draining 與交接，成果延續：
//!   `nearing_the_five_hour_limit_drains_the_account_and_a_new_worker_continues_from_the_checkpoint`、
//!   `nearing_the_weekly_limit_drains_too_and_a_worker_that_exits_on_request_is_handed_off`、
//!   `a_limit_warning_without_a_number_drains_and_after_the_wait_the_latest_checkpoint_is_carried_over`
//! - 舊 writer 未確認停止前，新 Worker 不能進入同一可寫目錄：
//!   `no_successor_is_created_while_the_old_writer_is_not_confirmed_stopped`
//! - 交接中各 phase 中斷後重建，依 phase 恢復且不出現兩個 writer：
//!   `a_restart_while_requested_resumes_from_the_requested_phase`、
//!   `a_crash_while_stopping_the_old_writer_resumes_the_stop_after_a_restart`、
//!   `a_restart_after_the_old_process_already_died_transfers_without_stopping_again`、
//!   `a_crash_while_launching_the_successor_adopts_it_instead_of_starting_another`
//! - 交接不增加修正計數：`a_handoff_is_not_a_failure_and_adds_no_correction_count`
//! - D-13 的其他規則：`the_same_task_waits_ten_minutes_before_switching_accounts_again`、
//!   `a_paused_account_only_reopens_below_sixty_percent_or_after_its_reset`、
//!   `without_a_successor_account_the_run_keeps_running_on_the_draining_account`、
//!   `cancelling_the_old_run_during_a_handoff_abandons_it_without_a_successor`、
//!   `only_the_successor_can_confirm_and_the_confirmation_needs_all_three_answers`

use std::time::Duration;

use baton_domain::agent::{TOOL_CONFIRM_HANDOFF, TOOL_REPORT_PROGRESS, TOOL_SAVE_CHECKPOINT};
use baton_domain::quota::{LimitStatus, QuotaWindow};
use baton_domain::Timestamp;
use baton_harness::fakes::{FakeStop, HangPoint};
use baton_harness::Harness;
use baton_ports::{AdapterEvent, Clock, Confidence, ProcessIdentity, QuotaSample, QuotaSource, StopResult};
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

const WORKSPACES: &str = "/api/v1/workspaces";
const SCHEDULING: &str = "/api/v1/scheduling";
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
    let task = ready_task(&harness, &ws, &goal, "整理資料").await;
    World { harness, accounts: ids, ws, task }
}

async fn ready_task(harness: &Harness, ws: &str, goal: &str, title: &str) -> String {
    let body = json!({ "goal_id": goal, "title": title, "contract": contract() });
    let (status, task) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks"), Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    let tid = task["id"].as_str().unwrap().to_string();
    let (status, ready) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks/{tid}/ready"), None).await;
    assert_eq!(status, StatusCode::OK, "{ready}");
    tid
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

    /// Task 尚未結束的 Run（queued、starting、running）。
    async fn unfinished_runs(&self) -> Vec<Value> {
        self.runs()
            .await
            .into_iter()
            .filter(|r| r["task_id"] == self.task.as_str())
            .filter(|r| ["queued", "starting", "running"].contains(&r["status"].as_str().unwrap()))
            .collect()
    }

    async fn board(&self) -> Value {
        let (status, body) = self.h().send_json(Method::GET, &format!("{WORKSPACES}/{}/tasks", self.ws), None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body
    }

    async fn handoffs(&self) -> Vec<Value> {
        self.board().await["handoffs"].as_array().unwrap().clone()
    }

    async fn only_handoff(&self) -> Value {
        let handoffs = self.handoffs().await;
        assert_eq!(handoffs.len(), 1, "{handoffs:?}");
        handoffs[0].clone()
    }

    async fn task_status(&self) -> String {
        let board = self.board().await;
        let task = board["tasks"].as_array().unwrap().iter().find(|t| t["id"] == self.task.as_str()).unwrap().clone();
        task["status"].as_str().unwrap().to_string()
    }

    async fn gate_of(&self, profile: &str) -> Value {
        let (status, body) = self.h().send_json(Method::GET, SCHEDULING, None).await;
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

    /// CLI 回報這個 Run 帳號的用量（已觀測），Supervisor 處理一次。
    async fn limit_signal(&self, rid: &str, window: QuotaWindow, status: LimitStatus, used: Option<f64>) {
        self.h().agent_adapter.emit(
            rid,
            AdapterEvent::RateLimited { window: Some(window), status, used, reset_at_millis: None, raw: "{}".into() },
        );
        self.h().supervise().await;
    }

    fn identity_of(&self, run: &Value) -> ProcessIdentity {
        self.h().process_control.identity_of(run["command_id"].as_str().unwrap()).unwrap()
    }

    /// Successor 的 Prompt（fake Adapter 接上 Run 時收到的 RunSpec；接上發生在啟動後的下一次處理）。
    async fn prompt_of(&self, rid: &str) -> String {
        self.h().supervise().await;
        self.h().agent_adapter.launched().into_iter().find(|spec| spec.run_id == rid).expect("Run 已接上").prompt
    }

    fn advance(&self, millis: i64) {
        self.h().clock.advance_millis(millis);
    }

    async fn successor_of(&self, handoff: &Value) -> Value {
        self.run(handoff["to_run_id"].as_str().expect("已轉移的交接有 successor")).await
    }

    /// 任何時刻 Task 最多一個尚未結束的 Run（也就是最多一個 writer）。
    async fn assert_single_writer(&self) {
        let unfinished = self.unfinished_runs().await;
        assert!(unfinished.len() <= 1, "同一個 Task 出現兩個 writer：{unfinished:?}");
        let held: i64 = self
            .h()
            .db()
            .query_row(
                "SELECT COUNT(*) FROM execution_lease WHERE task_id = ?1 AND released_at IS NULL",
                [&self.task],
                |r| r.get(0),
            )
            .unwrap();
        assert!(held <= 1, "同一個 Task 有 {held} 個未釋放的寫入所有權");
    }
}

/// 讓 Supervisor 處理一次；停止或 spawn 設成「卡住」時，等一小段時間後放棄（等同 Runtime 在那一點崩潰）。
async fn supervise_until_crash(harness: &Harness) {
    let outcome = tokio::time::timeout(Duration::from_millis(300), harness.supervise()).await;
    assert!(outcome.is_err(), "預期 Supervisor 卡在被設定的崩潰點");
}

// --- 模擬接近五小時與週上限，皆觸發 draining 與交接，成果延續 ---

#[tokio::test]
async fn nearing_the_five_hour_limit_drains_the_account_and_a_new_worker_continues_from_the_checkpoint() {
    let w = world(2).await;
    let (a, b) = (w.accounts[0].clone(), w.accounts[1].clone());
    let first = w.start_run().await;
    assert_eq!(w.run(&first).await["profile_id"], a.as_str());
    w.checkpoint(&first, "完成步驟一", "已整理三個產品").await;

    // 五小時視窗用量 86%：帳號 A 進入 draining，Run 開始交接，承接帳號是 B
    w.advance(MINUTE);
    w.limit_signal(&first, QuotaWindow::FiveHour, LimitStatus::Allowed, Some(0.86)).await;
    let gate = w.gate_of(&a).await;
    assert_eq!((gate["state"].as_str(), gate["reason"].as_str()), (Some("draining"), Some("drain_threshold")));
    let requested = w.only_handoff().await;
    assert_eq!(requested["phase"], "requested");
    assert_eq!((requested["from_run_id"].as_str(), requested["to_profile_id"].as_str()), (Some(first.as_str()), Some(b.as_str())));
    assert_eq!(w.h().process_control.spawn_count(), 1, "安全邊界之前不停止舊程序、不建立 successor");
    assert!(w.h().process_control.stop_calls().is_empty());

    // Worker 在回報的回應中得知要保存 checkpoint 並停止
    let (status, ack) = w.h().call_tool(&w.token(&first), TOOL_REPORT_PROGRESS, json!({ "summary": "做第二步" })).await;
    assert_eq!(status, StatusCode::OK, "{ack}");
    assert_eq!((ack["stop_requested"].as_bool(), ack["handoff_requested"].as_bool()), (Some(true), Some(true)));
    w.advance(10 * SECOND);
    w.checkpoint(&first, "完成步驟二", "已整理五個產品，剩比較表").await;

    // 安全邊界：先持久化 stopping，再要求舊程序停止；確認停止後由 B 的 successor 接手
    w.h().supervise().await;
    let old = w.run(&first).await;
    assert_eq!(w.h().process_control.stop_calls(), vec![w.identity_of(&old)]);
    assert_eq!((old["status"].as_str(), old["end_reason"].as_str()), (Some("interrupted"), Some("handed_off")));
    let handoff = w.only_handoff().await;
    assert_eq!(handoff["phase"], "transferred");
    assert_eq!(handoff["stop_outcome"], "stopped_gracefully");
    assert_eq!(handoff["checkpoint"]["summary"], "完成步驟二");
    let successor = w.successor_of(&handoff).await;
    assert_eq!(successor["profile_id"], b.as_str());
    assert_eq!(successor["generation"], 2);
    assert_eq!(successor["status"], "running", "successor 在同一次處理就啟動");
    assert_eq!(w.h().process_control.spawn_count(), 2);
    w.assert_single_writer().await;

    // 成果延續：successor 的 Prompt 帶著最近的有效檢查點，並要求確認接手
    let rid = successor["id"].as_str().unwrap().to_string();
    let prompt = w.prompt_of(&rid).await;
    assert!(prompt.contains("完成步驟二") && prompt.contains("已整理五個產品，剩比較表"), "{prompt}");
    assert!(prompt.contains(TOOL_CONFIRM_HANDOFF), "{prompt}");
    let body = json!({ "completed": "五個產品已整理", "incomplete": "比較表", "unknown_side_effects": "無" });
    let (status, confirmed) = w.h().call_tool(&w.token(&rid), TOOL_CONFIRM_HANDOFF, body).await;
    assert_eq!(status, StatusCode::OK, "{confirmed}");
    assert_eq!(confirmed["handoff"]["phase"], "confirmed");
    let handoff = w.only_handoff().await;
    assert_eq!(handoff["confirmation"]["incomplete"], "比較表");

    // 舊 Worker 的憑證已不是寫入所有者
    let (status, stale) = w.h().call_tool(&w.token(&first), TOOL_REPORT_PROGRESS, json!({ "summary": "還在嗎" })).await;
    assert_eq!((status, stale["error"].as_str()), (StatusCode::CONFLICT, Some("run_report_stale")));

    // successor 完成後 Task 照常進入驗證
    w.h().agent_adapter.emit(&rid, AdapterEvent::Exited { success: true });
    w.h().supervise().await;
    assert_ne!(w.task_status().await, "ready");
    assert_eq!(w.run(&rid).await["status"], "succeeded");
}

#[tokio::test]
async fn nearing_the_weekly_limit_drains_too_and_a_worker_that_exits_on_request_is_handed_off() {
    let w = world(2).await;
    let b = w.accounts[1].clone();
    let first = w.start_run().await;
    w.checkpoint(&first, "步驟一", "").await;

    w.advance(SECOND);
    w.limit_signal(&first, QuotaWindow::SevenDay, LimitStatus::Allowed, Some(0.9)).await;
    assert_eq!(w.gate_of(&w.accounts[0]).await["state"], "draining");
    assert_eq!(w.only_handoff().await["phase"], "requested");

    // Worker 收到停止要求後自行保存並結束（結束碼不論成敗）：視同舊程序已停止，直接轉移
    w.advance(5 * SECOND);
    w.checkpoint(&first, "步驟二", "週上限前的進度").await;
    w.h().agent_adapter.emit(&first, AdapterEvent::Error { message: "收到停止要求".into() });
    w.h().agent_adapter.emit(&first, AdapterEvent::Exited { success: false });
    w.h().supervise().await;

    let old = w.run(&first).await;
    assert_eq!((old["status"].as_str(), old["end_reason"].as_str()), (Some("interrupted"), Some("handed_off")));
    let handoff = w.only_handoff().await;
    assert_eq!((handoff["phase"].as_str(), handoff["stop_outcome"].as_str()), (Some("transferred"), Some("exited")));
    let successor = w.successor_of(&handoff).await;
    assert_eq!((successor["profile_id"].as_str(), successor["status"].as_str()), (Some(b.as_str()), Some("running")));
    assert!(w.prompt_of(successor["id"].as_str().unwrap()).await.contains("週上限前的進度"));
    assert_eq!(w.task_status().await, "running", "交接不會把 Task 推進驗證");
    w.assert_single_writer().await;
}

#[tokio::test]
async fn a_limit_warning_without_a_number_drains_and_after_the_wait_the_latest_checkpoint_is_carried_over() {
    let w = world(2).await;
    let first = w.start_run().await;
    w.checkpoint(&first, "唯一的檢查點", "細節").await;

    w.advance(SECOND);
    w.limit_signal(&first, QuotaWindow::FiveHour, LimitStatus::Warning, None).await;
    let gate = w.gate_of(&w.accounts[0]).await;
    assert_eq!((gate["state"].as_str(), gate["reason"].as_str()), (Some("draining"), Some("limit_warning")));

    // Worker 沒有再保存 checkpoint：等不滿兩分鐘不停止
    w.advance(119 * SECOND);
    w.h().supervise().await;
    assert_eq!(w.only_handoff().await["phase"], "requested");
    assert_eq!(w.h().process_control.spawn_count(), 1);

    // 等滿兩分鐘：以最近的有效檢查點交接
    w.advance(SECOND);
    w.h().supervise().await;
    let handoff = w.only_handoff().await;
    assert_eq!(handoff["phase"], "transferred");
    assert_eq!(handoff["checkpoint"]["summary"], "唯一的檢查點");
    assert!(w.prompt_of(handoff["to_run_id"].as_str().unwrap()).await.contains("唯一的檢查點"));
}

// --- 舊 writer 未確認停止前，新 Worker 不能進入同一可寫目錄 ---

#[tokio::test]
async fn no_successor_is_created_while_the_old_writer_is_not_confirmed_stopped() {
    let w = world(2).await;
    let first = w.start_run().await;
    let identity = w.identity_of(&w.run(&first).await);
    w.h().process_control.script_stop(&identity, FakeStop::Result(StopResult::StillAlive));
    w.limit_signal(&first, QuotaWindow::FiveHour, LimitStatus::Allowed, Some(0.9)).await;
    w.checkpoint(&first, "保存", "").await;

    for _ in 0..3 {
        w.h().supervise().await;
        w.advance(MINUTE);
    }
    assert_eq!(w.only_handoff().await["phase"], "stopping");
    assert_eq!(w.h().process_control.stop_calls().len(), 3, "每次處理都再要求停止");
    assert_eq!(w.h().process_control.spawn_count(), 1, "舊程序沒有確認停止，不啟動新的 Worker");
    assert_eq!(w.run(&first).await["status"], "running", "舊 Run 保留寫入所有權");
    w.assert_single_writer().await;

    // 停止失敗（無法確認）也一樣
    w.h().process_control.script_stop(&identity, FakeStop::Fail("存取被拒".into()));
    w.h().supervise().await;
    assert_eq!(w.h().process_control.spawn_count(), 1);

    // 終於確認停止（強制終止程序樹）：這時才建立 successor
    w.h().process_control.script_stop(&identity, FakeStop::Result(StopResult::ForceKilled));
    w.h().supervise().await;
    let handoff = w.only_handoff().await;
    assert_eq!((handoff["phase"].as_str(), handoff["stop_outcome"].as_str()), (Some("transferred"), Some("force_killed")));
    assert_eq!(w.h().process_control.spawn_count(), 2);
    w.assert_single_writer().await;
}

// --- 交接中各 phase 中斷後重建，依 phase 恢復且不出現兩個 writer ---

#[tokio::test]
async fn a_restart_while_requested_resumes_from_the_requested_phase() {
    let mut w = world(2).await;
    let first = w.start_run().await;
    w.limit_signal(&first, QuotaWindow::FiveHour, LimitStatus::Allowed, Some(0.9)).await;
    assert_eq!(w.only_handoff().await["phase"], "requested");

    w.harness.restart().await;
    w.advance(SECOND);
    w.checkpoint(&first, "重建後保存", "").await;
    w.h().supervise().await;

    let handoff = w.only_handoff().await;
    assert_eq!(handoff["phase"], "transferred", "重建後從 requested 繼續，沒有建立第二筆交接");
    assert_eq!(handoff["checkpoint"]["summary"], "重建後保存");
    assert_eq!(w.h().process_control.spawn_count(), 2);
    w.assert_single_writer().await;
}

#[tokio::test]
async fn a_crash_while_stopping_the_old_writer_resumes_the_stop_after_a_restart() {
    let mut w = world(2).await;
    let first = w.start_run().await;
    let identity = w.identity_of(&w.run(&first).await);
    w.limit_signal(&first, QuotaWindow::FiveHour, LimitStatus::Allowed, Some(0.9)).await;
    w.checkpoint(&first, "停止前保存", "").await;

    // Runtime 在要求舊程序停止的途中崩潰：stopping 已經持久化
    w.h().process_control.script_stop(&identity, FakeStop::Hang);
    supervise_until_crash(w.h()).await;
    assert_eq!(w.only_handoff().await["phase"], "stopping");
    assert_eq!(w.h().process_control.spawn_count(), 1);

    w.harness.restart().await;
    w.h().process_control.script_stop(&identity, FakeStop::Result(StopResult::StoppedGracefully));
    w.h().supervise().await;

    let handoff = w.only_handoff().await;
    assert_eq!(handoff["phase"], "transferred");
    assert_eq!(w.h().process_control.stop_calls().last(), Some(&identity), "重建後先停下舊程序才轉移");
    assert_eq!(w.h().process_control.spawn_count(), 2, "只啟動一個 successor");
    w.assert_single_writer().await;
}

#[tokio::test]
async fn a_restart_after_the_old_process_already_died_transfers_without_stopping_again() {
    let mut w = world(2).await;
    let first = w.start_run().await;
    let identity = w.identity_of(&w.run(&first).await);
    w.limit_signal(&first, QuotaWindow::FiveHour, LimitStatus::Allowed, Some(0.9)).await;
    w.checkpoint(&first, "保存", "").await;
    w.h().process_control.script_stop(&identity, FakeStop::Hang);
    supervise_until_crash(w.h()).await;

    // 崩潰期間舊程序自己結束了
    w.h().process_control.script_liveness(&identity, baton_ports::Liveness::Dead);
    w.harness.restart().await;
    let stops_before = w.h().process_control.stop_calls().len();
    w.h().supervise().await;

    let handoff = w.only_handoff().await;
    assert_eq!((handoff["phase"].as_str(), handoff["stop_outcome"].as_str()), (Some("transferred"), Some("dead")));
    assert_eq!(w.h().process_control.stop_calls().len(), stops_before, "已確認不在的程序不再要求停止");
    w.assert_single_writer().await;
}

#[tokio::test]
async fn a_crash_while_launching_the_successor_adopts_it_instead_of_starting_another() {
    let mut w = world(2).await;
    let first = w.start_run().await;
    w.limit_signal(&first, QuotaWindow::FiveHour, LimitStatus::Allowed, Some(0.9)).await;
    w.checkpoint(&first, "保存", "").await;

    // 轉移已提交、successor 的程序已啟動，但回執寫回之前 Runtime 崩潰
    w.h().process_control.hang_next_spawn_at(HangPoint::AfterSpawn);
    supervise_until_crash(w.h()).await;
    let handoff = w.only_handoff().await;
    assert_eq!(handoff["phase"], "transferred");
    assert_eq!(w.h().process_control.spawn_count(), 2);

    w.harness.restart().await;
    w.h().supervise().await;

    let successor = w.successor_of(&handoff).await;
    assert_eq!(successor["status"], "running", "以 command_id 找回已啟動的 successor 並補寫回執");
    assert_eq!(w.h().process_control.spawn_count(), 2, "沒有再啟動第三個程序");
    w.assert_single_writer().await;
}

// --- 交接不增加修正計數 ---

#[tokio::test]
async fn a_handoff_is_not_a_failure_and_adds_no_correction_count() {
    let w = world(2).await;
    let first = w.start_run().await;
    w.limit_signal(&first, QuotaWindow::FiveHour, LimitStatus::Allowed, Some(0.9)).await;
    w.checkpoint(&first, "保存", "").await;
    w.h().supervise().await;
    assert_eq!(w.only_handoff().await["phase"], "transferred");

    // 交接結束的 Run 是「已交接」的中斷，不是失敗；Task 沒有因此回到 ready 等人重派
    let runs = w.runs().await;
    assert!(runs.iter().all(|r| r["status"] != "failed"), "{runs:?}");
    let old = w.run(&first).await;
    assert_eq!((old["status"].as_str(), old["end_reason"].as_str()), (Some("interrupted"), Some("handed_off")));
    assert_eq!(w.task_status().await, "running");
    let failures: i64 = w
        .h()
        .db()
        .query_row(
            "SELECT COUNT(*) FROM event WHERE aggregate_type = 'run' AND event_type IN ('run_failed', 'run_interrupted')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(failures, 0, "交接只留下 run_handed_off，不留下失敗或一般中斷事件");
    let handed_off: i64 = w
        .h()
        .db()
        .query_row("SELECT COUNT(*) FROM event WHERE event_type = 'run_handed_off'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(handed_off, 1);

    // 三種修正與重試預算（票 36）都沒有被計入
    let board = w.board().await;
    let budget = board["budgets"].as_array().unwrap().iter().find(|b| b["task_id"] == w.task.as_str()).unwrap().clone();
    for counter in budget["counters"].as_array().unwrap() {
        assert_eq!(counter["used"], 0, "{budget}");
    }
}

// --- D-13 的其他規則 ---

#[tokio::test]
async fn the_same_task_waits_ten_minutes_before_switching_accounts_again() {
    let w = world(3).await;
    let (b, c) = (w.accounts[1].clone(), w.accounts[2].clone());
    let first = w.start_run().await;
    w.limit_signal(&first, QuotaWindow::FiveHour, LimitStatus::Allowed, Some(0.9)).await;
    w.checkpoint(&first, "保存", "").await;
    w.h().supervise().await;
    let handoff = w.only_handoff().await;
    let second = handoff["to_run_id"].as_str().unwrap().to_string();
    assert_eq!(w.run(&second).await["profile_id"], b.as_str());

    // 一分鐘後 B 也接近上限：冷卻期內不再換帳號，successor 繼續在 B 上執行
    w.advance(MINUTE);
    w.limit_signal(&second, QuotaWindow::FiveHour, LimitStatus::Allowed, Some(0.9)).await;
    assert_eq!(w.gate_of(&b).await["state"], "draining");
    w.advance(8 * MINUTE);
    w.h().supervise().await;
    assert_eq!(w.handoffs().await.len(), 1, "距上次換帳號不到 10 分鐘");

    // 滿 10 分鐘：再交接一次，A 仍在 draining，所以由 C 接手
    w.advance(MINUTE);
    w.h().supervise().await;
    let handoffs = w.handoffs().await;
    assert_eq!(handoffs.len(), 2);
    assert_eq!((handoffs[1]["from_run_id"].as_str(), handoffs[1]["to_profile_id"].as_str()), (Some(second.as_str()), Some(c.as_str())));
}

#[tokio::test]
async fn a_paused_account_only_reopens_below_sixty_percent_or_after_its_reset() {
    let w = world(1).await;
    let a = w.accounts[0].clone();
    let sample = |used: f64, reset_at: Option<Timestamp>, now: Timestamp| QuotaSample {
        source: QuotaSource::Observed,
        observed_at: now,
        used: Some(used),
        reset_at,
        confidence: Confidence::Observed,
    };
    let dispatch = || async {
        w.h().send_json(Method::POST, &format!("{WORKSPACES}/{}/tasks/{}/dispatch", w.ws, w.task), None).await
    };

    // 80%：停止派工
    let reset = w.h().clock.now().plus_millis(60 * MINUTE);
    w.h().quota_observer.set_sample(&a, sample(0.8, Some(reset), w.h().clock.now()));
    let (status, waiting) = dispatch().await;
    assert_eq!(status, StatusCode::ACCEPTED, "{waiting}");
    assert_eq!(waiting["reasons"][0]["code"], "quota_near_limit");
    assert_eq!(w.gate_of(&a).await["state"], "paused");

    // 降到 70%（低於 75%）：遲滯，仍不派
    w.advance(5 * MINUTE);
    w.h().quota_observer.set_sample(&a, sample(0.7, None, w.h().clock.now()));
    w.h().supervise().await;
    assert_eq!(w.gate_of(&a).await["state"], "paused");
    assert!(w.unfinished_runs().await.is_empty());

    // 過了進入時記下的重置時間：恢復，佇列中的 Task 自動派出
    w.h().clock.set(reset);
    w.h().quota_observer.set_sample(&a, sample(0.7, None, w.h().clock.now()));
    w.h().supervise().await;
    let gate = w.gate_of(&a).await;
    assert_eq!((gate["state"].as_str(), gate["reason"].as_str()), (Some("open"), Some("reset_observed")));
    assert_eq!(w.unfinished_runs().await.len(), 1);
}

#[tokio::test]
async fn an_account_below_sixty_percent_reopens_without_waiting_for_a_reset() {
    let w = world(1).await;
    let a = w.accounts[0].clone();
    let set = |used: f64| {
        w.h().quota_observer.set_sample(
            &a,
            QuotaSample {
                source: QuotaSource::Observed,
                observed_at: w.h().clock.now(),
                used: Some(used),
                reset_at: None,
                confidence: Confidence::Observed,
            },
        )
    };
    set(0.8);
    w.h().supervise().await;
    assert_eq!(w.gate_of(&a).await["state"], "paused");
    w.advance(5 * MINUTE);
    set(0.55);
    w.h().supervise().await;
    let gate = w.gate_of(&a).await;
    assert_eq!((gate["state"].as_str(), gate["reason"].as_str()), (Some("open"), Some("recovered")));
}

#[tokio::test]
async fn without_a_successor_account_the_run_keeps_running_on_the_draining_account() {
    let w = world(1).await;
    let first = w.start_run().await;
    w.limit_signal(&first, QuotaWindow::FiveHour, LimitStatus::Allowed, Some(0.9)).await;
    w.checkpoint(&first, "保存", "").await;
    w.advance(5 * MINUTE);
    w.h().supervise().await;

    assert_eq!(w.gate_of(&w.accounts[0]).await["state"], "draining");
    assert!(w.handoffs().await.is_empty(), "沒有承接帳號就不開始交接");
    assert!(w.h().process_control.stop_calls().is_empty(), "不停止唯一的 writer");
    assert_eq!(w.run(&first).await["status"], "running");
}

#[tokio::test]
async fn cancelling_the_old_run_during_a_handoff_abandons_it_without_a_successor() {
    let w = world(2).await;
    let first = w.start_run().await;
    w.limit_signal(&first, QuotaWindow::FiveHour, LimitStatus::Allowed, Some(0.9)).await;
    assert_eq!(w.only_handoff().await["phase"], "requested");

    let (status, body) =
        w.h().send_json(Method::POST, &format!("{WORKSPACES}/{}/runs/{first}/cancel", w.ws), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    w.h().supervise().await;
    w.advance(3 * MINUTE);
    w.h().supervise().await;

    let handoff = w.only_handoff().await;
    assert_eq!(handoff["phase"], "abandoned");
    assert!(handoff["abandoned_reason"].as_str().unwrap().starts_with("from_run_ended"), "{handoff}");
    assert_eq!(w.h().process_control.spawn_count(), 1, "使用者取消的工作不會被交接出去");
    assert_eq!(w.run(&first).await["status"], "cancelled");
}

#[tokio::test]
async fn only_the_successor_can_confirm_and_the_confirmation_needs_all_three_answers() {
    let w = world(2).await;
    let first = w.start_run().await;
    let answers = json!({ "completed": "a", "incomplete": "b", "unknown_side_effects": "無" });
    let (status, body) = w.h().call_tool(&w.token(&first), TOOL_CONFIRM_HANDOFF, answers.clone()).await;
    assert_eq!((status, body["error"].as_str()), (StatusCode::CONFLICT, Some("handoff_not_pending")), "沒有交接時不能確認");

    w.limit_signal(&first, QuotaWindow::FiveHour, LimitStatus::Allowed, Some(0.9)).await;
    w.checkpoint(&first, "保存", "").await;
    w.h().supervise().await;
    let successor = w.only_handoff().await["to_run_id"].as_str().unwrap().to_string();

    let (status, body) = w.h().call_tool(&w.token(&first), TOOL_CONFIRM_HANDOFF, answers.clone()).await;
    assert_eq!((status, body["error"].as_str()), (StatusCode::CONFLICT, Some("run_report_stale")), "舊 Worker 不能確認");
    let blank = json!({ "completed": "a", "incomplete": " ", "unknown_side_effects": "無" });
    let (status, body) = w.h().call_tool(&w.token(&successor), TOOL_CONFIRM_HANDOFF, blank).await;
    assert_eq!((status, body["error"].as_str()), (StatusCode::BAD_REQUEST, Some("handoff_confirmation_empty")));
    assert_eq!(w.only_handoff().await["phase"], "transferred");

    let (status, _) = w.h().call_tool(&w.token(&successor), TOOL_CONFIRM_HANDOFF, answers.clone()).await;
    assert_eq!(status, StatusCode::OK);
    let (status, again) = w.h().call_tool(&w.token(&successor), TOOL_CONFIRM_HANDOFF, answers).await;
    assert_eq!((status, again["handoff"]["phase"].as_str()), (StatusCode::OK, Some("confirmed")), "重複確認是冪等的");
}
