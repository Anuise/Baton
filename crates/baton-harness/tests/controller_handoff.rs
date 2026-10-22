//! 情境測試：Controller 跨帳號交接（票 30），經由 Runtime harness 的真實 HTTP API 驗證。
//!
//! 額度由 fake QuotaObserver 的樣本或 Controller 一輪自己的 CLI 限流訊號（`RateLimited`）產生；舊程序停不停得下來由
//! fake ProcessControl 決定；Worker 以真實的 Agent API（範圍憑證）保存檢查點與送出裁定請求，使用者以桌面 API 裁定。
//! 只斷言外部行為：對話 API（turn 狀態、Controller Run 的帳號、交接與結束原因、等待原因）、Session API、
//! 交給 Adapter 的 Prompt 與啟動參數。
//!
//! 每個測試名稱對應票 30 的驗收條件：
//! - Controller 換帳號後對話連續、待處理訊息不遺失也不重複：
//!   `the_next_turn_moves_to_another_account_with_a_session_rebuilt_from_conversation_decisions_and_checkpoints`、
//!   `a_turn_running_when_its_account_drains_is_stopped_at_the_safe_boundary_and_its_inputs_continue_once_elsewhere`、
//!   `a_sudden_limit_during_a_turn_hands_its_inputs_to_another_account_without_backoff`、
//!   `a_handoff_survives_a_runtime_restart_without_losing_or_repeating_inputs`
//! - Controller 無可用帳號時事件被保存，UI 顯示等待原因，Runtime 其餘功能正常：
//!   `without_an_available_account_the_controller_waits_with_a_visible_reason_while_the_runtime_keeps_working`

use baton_domain::agent::{TOOL_REPORT_PROGRESS, TOOL_REQUEST_DECISION, TOOL_SAVE_CHECKPOINT};
use baton_domain::quota::{LimitStatus, QuotaWindow};
use baton_domain::Timestamp;
use baton_harness::fakes::FakeStop;
use baton_harness::Harness;
use baton_ports::{AdapterEvent, Clock, Confidence, ProcessIdentity, QuotaSample, QuotaSource, StopResult};
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

const WORKSPACES: &str = "/api/v1/workspaces";
const CLOCK_START: Timestamp = Timestamp::from_millis(1_700_000_000_123);
const SECOND: i64 = 1000;
const MINUTE: i64 = 60 * SECOND;
/// 合併視窗（spec D-31）。
const WINDOW_MS: i64 = 5 * SECOND;
/// 額度觀測器的輪詢間隔（spec D-31）。
const QUOTA_POLL_MS: i64 = 5 * MINUTE;
/// 帳號進入 draining 後，進行中的一輪最多再等多久才被要求停止（與 Worker 的交接相同）。
const HANDOFF_WAIT_MS: i64 = 2 * MINUTE;

// --- 共用輔助 ---

struct World {
    harness: Harness,
    /// 依建立順序：帳號 A、B……
    accounts: Vec<String>,
    ws: String,
    goal: String,
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
    for (index, name) in ["A", "B"].into_iter().take(accounts).enumerate() {
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
    World { harness, accounts: ids, ws, goal }
}

impl World {
    fn h(&self) -> &Harness {
        &self.harness
    }

    fn advance(&self, millis: i64) {
        self.h().clock.advance_millis(millis);
    }

    /// 建立 ready 的 Task、派工並讓 Supervisor 啟動它，回傳 (task_id, run_id)。
    async fn running_worker(&self, title: &str) -> (String, String) {
        let body = json!({ "goal_id": self.goal, "title": title, "contract": contract() });
        let (status, task) = self.h().send_json(Method::POST, &format!("{WORKSPACES}/{}/tasks", self.ws), Some(body)).await;
        assert_eq!(status, StatusCode::CREATED, "{task}");
        let tid = task["id"].as_str().unwrap().to_string();
        let path = format!("{WORKSPACES}/{}/tasks/{tid}", self.ws);
        let (status, ready) = self.h().send_json(Method::POST, &format!("{path}/ready"), None).await;
        assert_eq!(status, StatusCode::OK, "{ready}");
        let (status, run) = self.h().send_json(Method::POST, &format!("{path}/dispatch"), None).await;
        assert_eq!(status, StatusCode::CREATED, "{run}");
        self.h().supervise().await;
        (tid, run["id"].as_str().unwrap().to_string())
    }

    fn token(&self, rid: &str) -> String {
        self.h().agent_credential(rid).expect("Run 已啟動，應該有 Agent 憑證")
    }

    async fn task_status(&self, tid: &str) -> String {
        let (status, detail) = self.h().send_json(Method::GET, &format!("{WORKSPACES}/{}/tasks/{tid}", self.ws), None).await;
        assert_eq!(status, StatusCode::OK, "{detail}");
        detail["task"]["status"].as_str().unwrap().to_string()
    }

    /// fake QuotaObserver 回報某個帳號的用量；等輪詢到期後 Supervisor 處理一次。
    async fn observe(&self, profile: &str, used: f64) {
        let sample = QuotaSample {
            source: QuotaSource::Observed,
            observed_at: self.h().clock.now(),
            used: Some(used),
            reset_at: None,
            confidence: Confidence::Observed,
        };
        self.h().quota_observer.set_sample(profile, sample);
        self.advance(QUOTA_POLL_MS);
        self.h().supervise().await;
    }

    async fn send(&self, text: &str) -> String {
        let path = format!("{WORKSPACES}/{}/conversation/messages", self.ws);
        let (status, body) = self.h().send_json(Method::POST, &path, Some(json!({ "text": text }))).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        body["turn"]["id"].as_str().unwrap().to_string()
    }

    async fn conversation(&self) -> Value {
        let (status, body) = self.h().send_json(Method::GET, &format!("{WORKSPACES}/{}/conversation", self.ws), None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body
    }

    async fn controller_runs(&self) -> Vec<Value> {
        self.conversation().await["runs"].as_array().unwrap().clone()
    }

    async fn latest_run(&self) -> Value {
        self.controller_runs().await.last().cloned().expect("沒有 Controller Run")
    }

    async fn turn(&self, id: &str) -> Value {
        let view = self.conversation().await;
        view["turns"].as_array().unwrap().iter().find(|t| t["id"] == id).cloned().unwrap_or_else(|| panic!("找不到 turn {id}"))
    }

    /// 讓一輪 Controller 回覆並成功結束，Supervisor 處理一次。
    async fn reply_and_exit(&self, run_id: &str, native: &str, reply: &str) {
        let adapter = &self.h().agent_adapter;
        adapter.emit(run_id, AdapterEvent::Started { native_session_id: Some(native.into()) });
        adapter.emit(run_id, AdapterEvent::Message { text: reply.into() });
        adapter.emit(run_id, AdapterEvent::Exited { success: true });
        self.h().supervise().await;
    }

    /// 送出一則訊息並等合併視窗到期，回傳 (turn 識別, 這一輪的 Controller Run)。
    async fn send_and_wake(&self, text: &str) -> (String, Value) {
        let turn = self.send(text).await;
        self.advance(WINDOW_MS);
        self.h().supervise().await;
        (turn, self.latest_run().await)
    }

    fn spawn_args(&self, run_id: &str) -> Vec<String> {
        self.h().process_control.spawned().into_iter().find(|s| s.args[0] == run_id).expect("已啟動").args
    }

    fn identity_of(&self, run: &Value) -> ProcessIdentity {
        self.h().process_control.identity_of(run["command_id"].as_str().unwrap()).unwrap()
    }

    async fn sessions(&self) -> Vec<Value> {
        let (status, body) = self.h().send_json(Method::GET, &format!("{WORKSPACES}/{}/sessions", self.ws), None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body["sessions"].as_array().unwrap().clone()
    }
}

fn id_of(value: &Value) -> String {
    value["id"].as_str().unwrap().to_string()
}

/// Prompt 的背景區段與「本輪要處理」區段。
fn split_prompt(run: &Value) -> (String, String) {
    let prompt = run["prompt"].as_str().unwrap();
    let (context, section) = prompt.split_once("## 本輪要處理").unwrap_or_else(|| panic!("Prompt 沒有本輪區段：{prompt}"));
    (context.to_string(), section.to_string())
}

/// 對話中某種 turn 的內容，依順序。
fn contents(view: &Value, kind: &str) -> Vec<String> {
    view["turns"].as_array().unwrap().iter().filter(|t| t["kind"] == kind).map(|t| t["content"].as_str().unwrap().to_string()).collect()
}

// --- 驗收條件：換帳號後對話連續、待處理訊息不遺失也不重複 ---

#[tokio::test]
async fn the_next_turn_moves_to_another_account_with_a_session_rebuilt_from_conversation_decisions_and_checkpoints() {
    let w = world(2).await;
    let (a, b) = (w.accounts[0].clone(), w.accounts[1].clone());

    // Worker 保存了檢查點、使用者裁定了一個問題
    let (_, worker) = w.running_worker("整理競品資料").await;
    let token = w.token(&worker);
    let (status, body) = w.h().call_tool(&token, TOOL_SAVE_CHECKPOINT, json!({ "summary": "已完成三家產品的比較", "details": "{}" })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let request = json!({
        "question": "比較表要用哪種格式？",
        "reason": "取決於使用者的閱讀習慣",
        "options": [{ "id": "md", "label": "Markdown 表格" }, { "id": "csv", "label": "CSV" }],
        "recommendation": "md",
        "evidence": ["既有文件都是 Markdown"],
        "urgency": "normal",
    });
    let (status, body) = w.h().call_tool(&token, TOOL_REQUEST_DECISION, request).await;
    assert!(status.is_success(), "{body}");
    let rid = body["decision_request_id"].as_str().unwrap().to_string();
    let path = format!("{WORKSPACES}/{}/decisions/{rid}", w.ws);
    let (_, view) = w.h().send_json(Method::GET, &path, None).await;
    let answer = json!({ "option_id": "md", "expected_revision": view["revision"] });
    let (status, body) = w.h().send_json(Method::POST, &format!("{path}/answer"), Some(answer)).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // 第一輪在帳號 A
    let (u1, first) = w.send_and_wake("第一則指示：先建立 Goal").await;
    assert_eq!(first["profile_id"], a.as_str());
    assert!(first["handoff"].is_null());
    let conversation_id = w.conversation().await["conversation_id"].clone();
    w.reply_and_exit(&id_of(&first), "native-ctl-a", "Goal 建好了").await;

    // A 接近上限（draining）：下一輪換到 B，建立新的 Controller Session
    w.observe(&a, 0.9).await;
    let (u2, second) = w.send_and_wake("第二則指示：開始派工").await;
    assert_eq!(second["profile_id"], b.as_str());
    assert_eq!(second["conversation_id"], conversation_id, "conversation_id 不變");
    assert_eq!(second["handoff"], json!({ "from_run_id": first["id"], "from_profile_id": a, "reason": "quota_draining" }));
    assert!(second["resume_native_session_id"].is_null(), "原生 Session 不能跨帳號接續");
    assert_eq!(w.spawn_args(&id_of(&second)).len(), 1, "新的 Session 不 --resume");
    let sessions = w.sessions().await;
    let successor = sessions.iter().find(|s| s["id"] == second["agent_session_id"]).unwrap();
    assert_eq!(successor["role"], "controller");
    assert_eq!(successor["conversation_id"], conversation_id);
    assert_eq!(successor["profile_id"], b.as_str());
    assert_eq!(successor["predecessor_session_id"], first["agent_session_id"]);
    assert_eq!(successor["replacement_reason"], "account_changed");

    // 從持久化的對話、決策與檢查點重建；已處理的只當背景，未處理的指示只列一次
    let (context, section) = split_prompt(&second);
    assert!(context.contains("第一則指示") && context.contains("Goal 建好了"), "{context}");
    assert!(context.contains("不要重新執行"), "{context}");
    assert!(context.contains("比較表要用哪種格式？ → Markdown 表格"), "決策：{context}");
    assert!(context.contains("整理競品資料") && context.contains("已完成三家產品的比較"), "檢查點：{context}");
    assert!(section.contains("第二則指示") && !section.contains("第一則指示"), "{section}");

    w.reply_and_exit(&id_of(&second), "native-ctl-b", "開始派工了").await;
    let second = w.latest_run().await;
    assert_eq!(second["status"], "succeeded", "承接的一輪保存了結果（交接確認）");
    for (input, run) in [(&u1, &first), (&u2, &second)] {
        let t = w.turn(input).await;
        assert_eq!((t["status"].as_str(), t["attempts"].as_i64()), (Some("processed"), Some(1)), "{t}");
        assert_eq!(t["controller_run_id"], run["id"]);
    }

    // 換過帳號之後留在 B，原生接續 B 上的新 Session（不再換回 A）
    let (_, third) = w.send_and_wake("第三則").await;
    assert_eq!(third["profile_id"], b.as_str());
    assert!(third["handoff"].is_null());
    assert_eq!(third["resume_native_session_id"], "native-ctl-b");
}

#[tokio::test]
async fn a_turn_running_when_its_account_drains_is_stopped_at_the_safe_boundary_and_its_inputs_continue_once_elsewhere() {
    let w = world(2).await;
    let (a, b) = (w.accounts[0].clone(), w.accounts[1].clone());
    let (u1, first) = w.send_and_wake("第一則：盤點進度").await;
    let first_id = id_of(&first);
    assert_eq!(first["profile_id"], a.as_str());
    let identity = w.identity_of(&first);

    // 這一輪的 CLI 回報接近上限：A 進入 draining。忙碌時送出的訊息只保存
    w.h().agent_adapter.emit(
        &first_id,
        AdapterEvent::RateLimited { window: Some(QuotaWindow::FiveHour), status: LimitStatus::Warning, used: Some(0.9), reset_at_millis: None, raw: "{}".into() },
    );
    w.h().supervise().await;
    let u2 = w.send("第二則：忙碌時送出").await;
    w.advance(WINDOW_MS);
    w.h().supervise().await;
    assert_eq!(w.turn(&u2).await["status"], "pending");

    // 安全邊界之前讓這一輪自己結束的機會：不要求停止
    w.advance(HANDOFF_WAIT_MS - WINDOW_MS - SECOND);
    w.h().supervise().await;
    assert!(w.h().process_control.stop_calls().is_empty(), "等待期間不停止進行中的一輪");
    assert_eq!(w.latest_run().await["status"], "running");

    // 等滿之後要求停止；停不下來就不交接、不開第二輪（B1）
    w.h().process_control.script_stop(&identity, FakeStop::Result(StopResult::StillAlive));
    w.advance(SECOND);
    w.h().supervise().await;
    w.h().supervise().await;
    let stops = w.h().process_control.stop_calls();
    assert!(!stops.is_empty() && stops.iter().all(|s| *s == identity), "{stops:?}");
    assert_eq!(w.controller_runs().await.len(), 1, "舊程序沒有確認停止，不建立下一輪");
    assert_eq!(w.latest_run().await["status"], "running");

    // 確認停止：這一輪以 handed_off 結束，輸入依序在 B 上交付（不退避）
    w.h().process_control.script_stop(&identity, FakeStop::Result(StopResult::ForceKilled));
    w.h().supervise().await;
    w.h().supervise().await;
    let runs = w.controller_runs().await;
    assert_eq!(runs.len(), 2, "{runs:?}");
    assert_eq!(runs[0]["status"], "interrupted");
    assert!(runs[0]["end_reason"].as_str().unwrap().starts_with("handed_off"), "{}", runs[0]);
    let second = runs[1].clone();
    assert_eq!(second["profile_id"], b.as_str());
    assert_eq!(second["handoff"]["from_profile_id"], a.as_str());
    assert_eq!(second["handoff"]["from_run_id"], first_id.as_str());
    assert_eq!(second["handoff"]["reason"], "quota_draining");
    assert_eq!(second["input_turn_ids"], json!([u1, u2]), "依到達順序");
    let (_, section) = split_prompt(&second);
    assert_eq!(section.matches("第一則：盤點進度").count(), 1, "{section}");
    assert_eq!(section.matches("第二則：忙碌時送出").count(), 1, "{section}");
    assert!(section.contains("可能已部分處理"), "被中斷的指示註明可能已部分處理：{section}");

    w.reply_and_exit(&id_of(&second), "native-ctl-b", "兩則都處理了").await;
    let view = w.conversation().await;
    assert_eq!(contents(&view, "user_message").len(), 2, "訊息不重複");
    assert_eq!(contents(&view, "controller_reply"), ["兩則都處理了"]);
    assert_eq!(w.turn(&u1).await["attempts"], 2);
    assert_eq!(w.turn(&u2).await["attempts"], 1);
    for id in [&u1, &u2] {
        assert_eq!(w.turn(id).await["status"], "processed");
    }
}

#[tokio::test]
async fn a_sudden_limit_during_a_turn_hands_its_inputs_to_another_account_without_backoff() {
    let w = world(2).await;
    let (a, b) = (w.accounts[0].clone(), w.accounts[1].clone());
    let (u1, first) = w.send_and_wake("請評估目前進度").await;
    let first_id = id_of(&first);
    assert_eq!(first["profile_id"], a.as_str());

    // 突然限流：CLI 回報已達上限並以失敗結束
    let adapter = &w.h().agent_adapter;
    adapter.emit(&first_id, AdapterEvent::Started { native_session_id: Some("native-ctl-a".into()) });
    adapter.emit(
        &first_id,
        AdapterEvent::RateLimited { window: Some(QuotaWindow::FiveHour), status: LimitStatus::Exhausted, used: Some(1.0), reset_at_millis: None, raw: "{}".into() },
    );
    adapter.emit(&first_id, AdapterEvent::Error { message: "usage limit reached".into() });
    adapter.emit(&first_id, AdapterEvent::Exited { success: false });
    w.h().supervise().await;

    // 不是 Controller 的失敗：不退避，同一次處理就在 B 上接手
    let runs = w.controller_runs().await;
    assert_eq!(runs.len(), 2, "{runs:?}");
    assert_eq!(runs[0]["status"], "interrupted");
    assert!(runs[0]["end_reason"].as_str().unwrap().starts_with("handed_off"), "{}", runs[0]);
    let second = runs[1].clone();
    assert_eq!(second["profile_id"], b.as_str());
    assert_eq!(second["handoff"]["reason"], "quota_draining");
    assert_eq!(second["input_turn_ids"], json!([u1]));
    w.reply_and_exit(&id_of(&second), "native-ctl-b", "進度良好").await;
    let t = w.turn(&u1).await;
    assert_eq!((t["status"].as_str(), t["controller_run_id"].as_str()), (Some("processed"), second["id"].as_str()));
}

#[tokio::test]
async fn a_handoff_survives_a_runtime_restart_without_losing_or_repeating_inputs() {
    let mut w = world(2).await;
    let b = w.accounts[1].clone();
    let (u1, first) = w.send_and_wake("第一則").await;
    let first_id = id_of(&first);
    w.h().agent_adapter.emit(
        &first_id,
        AdapterEvent::RateLimited { window: Some(QuotaWindow::SevenDay), status: LimitStatus::Warning, used: Some(0.95), reset_at_millis: None, raw: "{}".into() },
    );
    w.h().supervise().await;
    w.advance(HANDOFF_WAIT_MS);

    // 舊程序已停止、結束還沒寫入時 Runtime 當掉：重建後依持久化狀態找回，仍以交接結束、不退避
    w.h().process_control.script_stop(&w.identity_of(&first), FakeStop::HangAfterStop);
    let supervise = w.h().supervise();
    assert!(tokio::time::timeout(std::time::Duration::from_millis(500), supervise).await.is_err(), "停止的呼叫卡住");
    w.harness.crash_restart().await;
    for _ in 0..3 {
        w.h().supervise().await;
        w.advance(61 * SECOND);
    }
    w.h().supervise().await;
    let runs = w.controller_runs().await;
    assert_eq!(runs.len(), 2, "{runs:?}");
    assert!(runs[0]["end_reason"].as_str().unwrap().starts_with("handed_off"), "{}", runs[0]);
    assert_eq!(runs[1]["profile_id"], b.as_str());
    assert_eq!(runs[1]["input_turn_ids"], json!([u1]));
    let starts = w.h().process_control.spawned().iter().filter(|s| s.args[0] == runs[1]["id"].as_str().unwrap()).count();
    assert_eq!(starts, 1, "承接的一輪只啟動一次");
}

// --- 驗收條件：無可用帳號時事件被保存、UI 顯示等待原因、Runtime 其餘功能正常 ---

#[tokio::test]
async fn without_an_available_account_the_controller_waits_with_a_visible_reason_while_the_runtime_keeps_working() {
    let w = world(1).await;
    let a = w.accounts[0].clone();
    let (task, worker) = w.running_worker("撰寫文件").await;
    let (_, first) = w.send_and_wake("開始吧").await;
    w.reply_and_exit(&id_of(&first), "native-ctl-a", "好").await;

    // 唯一的帳號達停止派工門檻：使用者訊息保存為待處理，不喚起 Controller，UI 顯示等待原因
    w.observe(&a, 0.8).await;
    let (u2, _) = w.send_and_wake("請回報進度").await;
    w.h().supervise().await;
    assert_eq!(w.controller_runs().await.len(), 1);
    let view = w.conversation().await;
    assert_eq!(view["busy"], false);
    assert_eq!(w.turn(&u2).await["status"], "pending");
    let waiting = &view["waiting"];
    assert_eq!(waiting["code"], "quota_near_limit", "{view}");
    assert!(waiting["message"].as_str().unwrap().contains("帳號 A"), "{waiting}");
    let since = waiting["since"].as_i64().unwrap();

    // Runtime 其餘功能照常：Worker 回報與結束、Task 前進，Worker 的成果也保存為待處理的工作事件
    let (status, ack) = w.h().call_tool(&w.token(&worker), TOOL_REPORT_PROGRESS, json!({ "summary": "寫完第一節" })).await;
    assert_eq!(status, StatusCode::OK, "{ack}");
    w.h().agent_adapter.emit(&worker, AdapterEvent::Exited { success: true });
    w.h().supervise().await;
    assert_ne!(w.task_status(&task).await, "running");
    let view = w.conversation().await;
    let events: Vec<&Value> = view["turns"].as_array().unwrap().iter().filter(|t| t["kind"] == "work_event").collect();
    assert_eq!(events.len(), 1, "{view}");
    assert_eq!(events[0]["status"], "pending");
    let work_event = id_of(events[0]);
    w.advance(10 * MINUTE);
    w.h().supervise().await;
    let view = w.conversation().await;
    assert_eq!(view["waiting"]["since"], since, "持續等待時保留開始時間");
    assert_eq!(w.controller_runs().await.len(), 1, "等待期間不喚起");

    // 帳號恢復：先處理使用者訊息、再處理工作事件，各一次；等待原因清除
    w.observe(&a, 0.5).await;
    let second = w.latest_run().await;
    assert_eq!(second["input_turn_ids"], json!([u2]));
    assert!(second["handoff"].is_null(), "同一個帳號恢復不是換帳號");
    assert!(w.conversation().await["waiting"].is_null());
    w.reply_and_exit(&id_of(&second), "native-ctl-a", "進度如下").await;
    w.advance(WINDOW_MS);
    w.h().supervise().await;
    let third = w.latest_run().await;
    assert_eq!(third["input_turn_ids"], json!([work_event]));
    w.reply_and_exit(&id_of(&third), "native-ctl-a", "已評估").await;
    for id in [&u2, &work_event] {
        let t = w.turn(id).await;
        assert_eq!((t["status"].as_str(), t["attempts"].as_i64()), (Some("processed"), Some(1)), "{t}");
    }
}
