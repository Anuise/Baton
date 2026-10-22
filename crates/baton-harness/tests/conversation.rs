//! 情境測試：Controller 長期對話與事件驅動（票 16），經由 Runtime harness 的真實 HTTP API 驗證。
//!
//! 全部使用 fake ProcessControl／AgentAdapter 與 fake Clock：測試決定何時讓 Supervisor 處理一次（`supervise`）、
//! Controller 的一輪回覆什麼、何時結束、原生 Session 能否接續，以及 Runtime 在喚起或結果保存的哪一點「當掉」。
//! 只斷言外部行為：對話 API 的回應（turn 狀態、Controller Run）、交給 Adapter 的 Prompt 與啟動參數、spawn 次數。
//!
//! 每個測試名稱對應票 16 的驗收條件：
//! - 忙碌時收到兩則使用者訊息與一個工作事件，順序正確且不並行：`a_busy_controller_handles_...`
//! - 無事件時不產生 Controller Run：`without_events_no_controller_run_...`
//! - 換 Session 後 conversation 與未處理指示保留且不重複執行：`a_session_replacement_keeps_...`
//! - 在喚起與結果保存邊界中斷後，未完成事件可找回：`a_crash_before_the_controller_process_...`、
//!   `a_crash_after_the_controller_process_...`、`a_crash_before_the_result_is_saved_...`、
//!   `a_controller_that_dies_without_exiting_...`（程序沒有結束事件就死亡，含 Runtime 重建）、
//!   `a_result_that_arrives_after_the_process_is_found_dead_...`、`controller_wakes_wait_while_crash_recovery_...`
//! - Controller 的帳號遵守額度門檻的遲滯（票 28）：`controller_wakes_skip_an_account_held_by_the_quota_gate`
//! - 暫停／取消不排隊（DR-20261022-01）：`cancel_takes_effect_while_the_controller_is_busy`、
//!   `full_exit_stops_the_controller_and_keeps_its_inputs_pending`

use std::time::Duration;

use baton_domain::storage_health::StorageFaultKind;
use baton_domain::Timestamp;
use baton_harness::fakes::{FakeStop, HangPoint};
use baton_harness::Harness;
use baton_ports::{AdapterEvent, Capability, Clock, Confidence, Liveness, QuotaSample, QuotaSource, StopResult};
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

const WORKSPACES: &str = "/api/v1/workspaces";

/// 2023-11-14 22:13:20.123 UTC
const CLOCK_START: Timestamp = Timestamp::from_millis(1_700_000_000_123);
/// 合併視窗（spec D-31）。
const WINDOW_MS: i64 = 5_000;
/// 上一輪沒有完成時，重新交付的第一次退避。
const BACKOFF_MS: i64 = 60_000;
/// 進行中的一輪多久沒有事件就向 ProcessControl 查證程序（沿用 execution lease 的 60 秒，spec D-31）。
const LIVENESS_CHECK_MS: i64 = 60_000;

// --- 共用輔助 ---

fn plain_dir(harness: &Harness, name: &str) -> String {
    let dir = harness.data_dir().join("projects").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    dir.to_string_lossy().into_owned()
}

fn conversation_path(ws: &str) -> String {
    format!("{WORKSPACES}/{ws}/conversation")
}

fn messages_path(ws: &str) -> String {
    format!("{}/messages", conversation_path(ws))
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

/// 啟動 Harness、建立一個帳號與一個 Workspace，回傳 (harness, workspace_id, workspace_path)。
async fn start() -> (Harness, String, String) {
    let harness = Harness::builder().clock_at(CLOCK_START).start().await;
    harness.add_account("帳號 1", "account1@example.com").await;
    let path = plain_dir(&harness, "main");
    let (status, body) = harness.send_json(Method::POST, WORKSPACES, Some(json!({ "path": path }))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let ws = body["id"].as_str().unwrap().to_string();
    let path = body["path"].as_str().unwrap().to_string();
    (harness, ws, path)
}

/// 建立 Goal 與一個 ready 的 Task 並派工，回傳 (task_id, run_id)。
async fn dispatched_task(harness: &Harness, ws: &str, title: &str) -> (String, String) {
    let (status, goal) =
        harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/goals"), Some(json!({ "title": "目標" }))).await;
    assert_eq!(status, StatusCode::CREATED, "{goal}");
    let body = json!({ "goal_id": goal["id"], "title": title, "contract": contract() });
    let (status, task) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks"), Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    let tid = task["id"].as_str().unwrap().to_string();
    let (status, ready) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks/{tid}/ready"), None).await;
    assert_eq!(status, StatusCode::OK, "{ready}");
    let (status, run) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks/{tid}/dispatch"), None).await;
    assert_eq!(status, StatusCode::CREATED, "{run}");
    (tid, run["id"].as_str().unwrap().to_string())
}

async fn send(harness: &Harness, ws: &str, text: &str) -> Value {
    let (status, body) = harness.send_json(Method::POST, &messages_path(ws), Some(json!({ "text": text }))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["turn"].clone()
}

async fn conversation(harness: &Harness, ws: &str) -> Value {
    let (status, body) = harness.send_json(Method::GET, &conversation_path(ws), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

fn advance(harness: &Harness, millis: i64) {
    harness.clock.advance_millis(millis);
}

/// 所有 Controller Run，依建立順序。
async fn controller_runs(harness: &Harness, ws: &str) -> Vec<Value> {
    conversation(harness, ws).await["runs"].as_array().unwrap().clone()
}

/// 未結束的 Controller Run 數（B1：任何時刻最多一個）。
async fn active_controller_runs(harness: &Harness, ws: &str) -> usize {
    controller_runs(harness, ws)
        .await
        .iter()
        .filter(|r| matches!(r["status"].as_str(), Some("starting" | "running")))
        .count()
}

/// 最近一個 Controller Run。
async fn latest_run(harness: &Harness, ws: &str) -> Value {
    controller_runs(harness, ws).await.last().cloned().expect("沒有 Controller Run")
}

/// 被啟動過的 Controller 程序數（fake spawn 的第一個參數是 Run 識別，Controller Run 以 `ctl_` 開頭）。
fn controller_spawns(harness: &Harness) -> usize {
    harness.process_control.spawned().iter().filter(|s| s.args.first().is_some_and(|a| a.starts_with("ctl_"))).count()
}

/// 某個 Controller Run 的 Prompt：與 launch intent 一起持久化，Adapter 收到的就是它（重建後也相同）。
async fn prompt_of(harness: &Harness, ws: &str, run_id: &str) -> String {
    let runs = controller_runs(harness, ws).await;
    let run = runs.iter().find(|r| r["id"] == run_id).unwrap_or_else(|| panic!("找不到 Controller Run {run_id}"));
    run["prompt"].as_str().unwrap().to_string()
}

/// 以 turn 識別找 turn。
fn turn<'a>(view: &'a Value, id: &str) -> &'a Value {
    view["turns"].as_array().unwrap().iter().find(|t| t["id"] == id).unwrap_or_else(|| panic!("找不到 turn {id}：{view}"))
}

fn turn_id(turn: &Value) -> String {
    turn["id"].as_str().unwrap().to_string()
}

/// 讓一輪 Controller 回覆並成功結束。
fn reply_and_exit(harness: &Harness, run_id: &str, native: &str, reply: &str) {
    harness.agent_adapter.emit(run_id, AdapterEvent::Started { native_session_id: Some(native.into()) });
    harness.agent_adapter.emit(run_id, AdapterEvent::Message { text: reply.into() });
    harness.agent_adapter.emit(run_id, AdapterEvent::Exited { success: true });
}

/// 某個 Prompt 中「本輪要處理」的區段。
fn instructions(prompt: &str) -> &str {
    prompt.split_once("## 本輪要處理").map(|(_, rest)| rest).unwrap_or_else(|| panic!("Prompt 沒有本輪區段：{prompt}"))
}

// --- 驗收條件 ---

#[tokio::test]
async fn without_events_no_controller_run_is_ever_started() {
    let (harness, ws, _) = start().await;
    let view = conversation(&harness, &ws).await;
    assert!(view["conversation_id"].is_null(), "{view}");
    assert_eq!(view["busy"], false);
    for _ in 0..5 {
        advance(&harness, 3_600_000);
        harness.supervise().await;
    }
    assert_eq!(controller_spawns(&harness), 0);

    // 還沒有對話時，Worker 的成果也不會喚醒 Controller（使用者沒有找過 Controller）
    let (_, rid) = dispatched_task(&harness, &ws, "研究").await;
    harness.supervise().await;
    harness.agent_adapter.emit(&rid, AdapterEvent::Exited { success: true });
    harness.supervise().await;
    advance(&harness, 3_600_000);
    harness.supervise().await;
    assert_eq!(controller_spawns(&harness), 0);
    assert!(conversation(&harness, &ws).await["conversation_id"].is_null());

    // 一輪處理完之後沒有新事件：不再喚醒，也沒有巡查
    let u1 = send(&harness, &ws, "你好").await;
    advance(&harness, WINDOW_MS);
    harness.supervise().await;
    let run = latest_run(&harness, &ws).await;
    reply_and_exit(&harness, run["id"].as_str().unwrap(), "native-ctl-1", "你好，我是 Controller");
    harness.supervise().await;
    assert_eq!(turn(&conversation(&harness, &ws).await, &turn_id(&u1))["status"], "processed");
    for _ in 0..5 {
        advance(&harness, 3_600_000);
        harness.supervise().await;
    }
    assert_eq!(controller_spawns(&harness), 1);
    assert_eq!(controller_runs(&harness, &ws).await.len(), 1);
}

#[tokio::test]
async fn a_busy_controller_handles_two_user_messages_before_the_work_event_one_turn_at_a_time() {
    let (harness, ws, _) = start().await;
    let (_, worker) = dispatched_task(&harness, &ws, "研究競品").await;
    harness.supervise().await;

    // 第一則訊息：合併視窗內不喚醒，到期後喚醒一輪
    let u1 = send(&harness, &ws, "第一則：先整理需求").await;
    assert_eq!(u1["status"], "pending");
    harness.supervise().await;
    assert_eq!(controller_spawns(&harness), 0, "合併視窗內不應喚醒");
    advance(&harness, WINDOW_MS);
    harness.supervise().await;
    assert_eq!(controller_spawns(&harness), 1);
    let first = latest_run(&harness, &ws).await;
    let first_id = first["id"].as_str().unwrap().to_string();
    assert_eq!(first["input_turn_ids"], json!([turn_id(&u1)]));

    // 忙碌時：兩則使用者訊息與一個工作事件（Worker 的 Run 成功結束）依序到達
    let u2 = send(&harness, &ws, "第二則：改用表格").await;
    harness.agent_adapter.emit(&worker, AdapterEvent::Exited { success: true });
    harness.supervise().await;
    advance(&harness, 1_000);
    let u3 = send(&harness, &ws, "第三則：補上價格").await;
    let view = conversation(&harness, &ws).await;
    assert_eq!(view["busy"], true);
    assert_eq!(turn(&view, &turn_id(&u1))["status"], "processing");
    assert_eq!(turn(&view, &turn_id(&u2))["status"], "pending", "忙碌時新訊息是「已收到／待處理」");
    assert_eq!(turn(&view, &turn_id(&u3))["status"], "pending");
    let event = view["turns"].as_array().unwrap().iter().find(|t| t["kind"] == "work_event").expect("沒有工作事件").clone();
    assert_eq!(event["status"], "pending");
    assert!(event["content"].as_str().unwrap().contains("研究競品"), "{event}");

    // 忙碌期間不論過多久都不會並行第二輪
    for _ in 0..3 {
        advance(&harness, 60_000);
        harness.supervise().await;
        assert_eq!(active_controller_runs(&harness, &ws).await, 1);
    }
    assert_eq!(controller_spawns(&harness), 1);

    // 第一輪結束：先處理兩則使用者訊息（同一輪、依到達順序），工作事件留到下一輪
    reply_and_exit(&harness, &first_id, "native-ctl-1", "需求整理好了");
    harness.supervise().await;
    let launched = harness.agent_adapter.launched().into_iter().find(|s| s.run_id == first_id).expect("應該接上第一輪");
    assert_eq!(launched.prompt, first["prompt"].as_str().unwrap(), "Adapter 收到的就是持久化的 Prompt");
    assert_eq!(launched.task_id, "", "Controller Run 不綁 Task");
    let view = conversation(&harness, &ws).await;
    assert_eq!(turn(&view, &turn_id(&u1))["status"], "processed");
    let second = latest_run(&harness, &ws).await;
    let second_id = second["id"].as_str().unwrap().to_string();
    assert_ne!(second_id, first_id);
    assert_eq!(second["input_turn_ids"], json!([turn_id(&u2), turn_id(&u3)]));
    let prompt = prompt_of(&harness, &ws, &second_id).await;
    let section = instructions(&prompt);
    let (at2, at3) = (section.find("第二則").unwrap(), section.find("第三則").unwrap());
    assert!(at2 < at3, "使用者訊息依到達順序：{prompt}");
    assert!(!section.contains("研究競品"), "工作事件不在這一輪：{prompt}");
    assert_eq!(turn(&view, &turn_id(&event))["status"], "pending");
    assert_eq!(active_controller_runs(&harness, &ws).await, 1);

    // 第二輪結束：處理工作事件
    reply_and_exit(&harness, &second_id, "native-ctl-1", "表格與價格都補上了");
    harness.supervise().await;
    let third = latest_run(&harness, &ws).await;
    let third_id = third["id"].as_str().unwrap().to_string();
    assert_eq!(third["input_turn_ids"], json!([turn_id(&event)]));
    assert!(instructions(&prompt_of(&harness, &ws, &third_id).await).contains("研究競品"));
    reply_and_exit(&harness, &third_id, "native-ctl-1", "成果看過了");
    harness.supervise().await;

    let view = conversation(&harness, &ws).await;
    assert_eq!(view["busy"], false);
    let processed_by = |id: &str| turn(&view, id)["controller_run_id"].as_str().unwrap().to_string();
    assert_eq!(processed_by(&turn_id(&u1)), first_id);
    assert_eq!(processed_by(&turn_id(&u2)), second_id);
    assert_eq!(processed_by(&turn_id(&u3)), second_id);
    assert_eq!(processed_by(&turn_id(&event)), third_id);
    let replies: Vec<&str> = view["turns"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["kind"] == "controller_reply")
        .map(|t| t["content"].as_str().unwrap())
        .collect();
    assert_eq!(replies, ["需求整理好了", "表格與價格都補上了", "成果看過了"]);
    assert_eq!(controller_spawns(&harness), 3);
}

#[tokio::test]
async fn a_session_replacement_keeps_the_conversation_and_runs_pending_instructions_exactly_once() {
    let (harness, ws, path) = start().await;
    let u1 = send(&harness, &ws, "第一則指示：建立 Goal").await;
    advance(&harness, WINDOW_MS);
    harness.supervise().await;
    let first = latest_run(&harness, &ws).await;
    let first_id = first["id"].as_str().unwrap().to_string();
    let conversation_id = conversation(&harness, &ws).await["conversation_id"].clone();
    assert!(conversation_id.is_string());
    reply_and_exit(&harness, &first_id, "native-ctl-1", "Goal 建好了");
    harness.supervise().await;

    // 同帳號、原生 Session 可接續：沿用同一個 Session 並以 --resume 接續
    let u2 = send(&harness, &ws, "第二則指示").await;
    advance(&harness, WINDOW_MS);
    harness.supervise().await;
    let second = latest_run(&harness, &ws).await;
    let second_id = second["id"].as_str().unwrap().to_string();
    assert_eq!(second["agent_session_id"], first["agent_session_id"]);
    assert_eq!(second["resume_native_session_id"], "native-ctl-1");
    let spawn = harness.process_control.spawned().into_iter().find(|s| s.args[0] == second_id).unwrap();
    assert_eq!(spawn.args[1..], ["--resume".to_string(), "native-ctl-1".to_string()]);
    assert_eq!(spawn.working_dir.as_deref(), Some(path.as_str()), "Controller 在 Workspace 目錄執行");
    let token = spawn.env.iter().find(|(name, _)| name == baton_domain::agent::AGENT_TOKEN_ENV).map(|(_, v)| v.as_str());
    assert_eq!(token, Some("controller-credential-not-issued"), "本票不簽發 Controller 憑證，只給佔位值");
    assert!(!prompt_of(&harness, &ws, &second_id).await.contains("第一則指示"), "接續原生 Session 時不重送已處理的對話");
    reply_and_exit(&harness, &second_id, "native-ctl-1", "第二則完成");
    harness.supervise().await;

    // 原生 Session 不可用：建立替代 Session，conversation 不變，只有待處理的指示列為本輪指示
    harness.agent_adapter.set_session_capability("native-ctl-1", Capability::NotResumable);
    let u3 = send(&harness, &ws, "第三則指示").await;
    advance(&harness, WINDOW_MS);
    harness.supervise().await;
    let third = latest_run(&harness, &ws).await;
    let third_id = third["id"].as_str().unwrap().to_string();
    assert_eq!(third["conversation_id"], conversation_id);
    assert_ne!(third["agent_session_id"], first["agent_session_id"]);
    assert!(third["resume_native_session_id"].is_null());
    let spawn = harness.process_control.spawned().into_iter().find(|s| s.args[0] == third_id).unwrap();
    assert_eq!(spawn.args.len(), 1, "替代 Session 不 --resume：{:?}", spawn.args);
    let prompt = prompt_of(&harness, &ws, &third_id).await;
    let (history, section) = prompt.split_once("## 本輪要處理").unwrap();
    assert!(history.contains("第一則指示") && history.contains("第二則指示"), "替代 Session 帶上背景：{prompt}");
    assert!(history.contains("不要重新執行"));
    assert!(section.contains("第三則指示"));
    assert!(!section.contains("第一則指示") && !section.contains("第二則指示"), "已處理的指示不再列為指示：{prompt}");

    let (status, sessions) = harness.send_json(Method::GET, &format!("{WORKSPACES}/{ws}/sessions"), None).await;
    assert_eq!(status, StatusCode::OK);
    let successor = sessions["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == third["agent_session_id"])
        .unwrap_or_else(|| panic!("找不到替代 Session：{sessions}"));
    assert_eq!(successor["role"], "controller");
    assert_eq!(successor["conversation_id"], conversation_id);
    assert_eq!(successor["predecessor_session_id"], first["agent_session_id"]);
    assert!(successor["history_gap"].is_string());

    reply_and_exit(&harness, &third_id, "native-ctl-2", "第三則完成");
    harness.supervise().await;
    advance(&harness, 3_600_000);
    harness.supervise().await;
    let view = conversation(&harness, &ws).await;
    assert_eq!(view["conversation_id"], conversation_id);
    for (input, run) in [(&u1, &first_id), (&u2, &second_id), (&u3, &third_id)] {
        let t = turn(&view, &turn_id(input));
        assert_eq!(t["status"], "processed");
        assert_eq!(t["controller_run_id"].as_str(), Some(run.as_str()));
        assert_eq!(t["attempts"], 1, "每則指示只交付一次：{t}");
    }
    assert_eq!(controller_spawns(&harness), 3);
}

#[tokio::test]
async fn a_crash_before_the_controller_process_starts_releases_the_inputs_and_redelivers_them() {
    let (mut harness, ws, _) = start().await;
    let u1 = send(&harness, &ws, "請規劃下一步").await;
    advance(&harness, WINDOW_MS);
    harness.process_control.hang_next_spawn_at(HangPoint::BeforeSpawn);
    let crashed = tokio::time::timeout(Duration::from_millis(300), harness.supervise()).await;
    assert!(crashed.is_err(), "spawn 前卡住，模擬 Runtime 當掉");
    let view = conversation(&harness, &ws).await;
    assert_eq!(turn(&view, &turn_id(&u1))["status"], "processing", "launch intent 已持久化");
    let lost = latest_run(&harness, &ws).await;
    assert_eq!(lost["status"], "starting");

    harness.restart().await;
    harness.supervise().await;
    let view = conversation(&harness, &ws).await;
    let lost = view["runs"][0].clone();
    assert_eq!(lost["status"], "interrupted", "{lost}");
    assert!(lost["end_reason"].as_str().unwrap().contains("launch_outcome_unknown"));
    let pending = turn(&view, &turn_id(&u1));
    assert_eq!(pending["status"], "pending", "未完成的輸入找回為待處理");
    assert_eq!(controller_spawns(&harness), 0);

    // 退避後重新交付，Prompt 註明可能已部分處理
    advance(&harness, BACKOFF_MS);
    harness.supervise().await;
    let retry = latest_run(&harness, &ws).await;
    let retry_id = retry["id"].as_str().unwrap().to_string();
    assert_ne!(retry_id, lost["id"].as_str().unwrap());
    assert_eq!(retry["input_turn_ids"], json!([turn_id(&u1)]));
    assert!(prompt_of(&harness, &ws, &retry_id).await.contains("可能已部分處理"));
    reply_and_exit(&harness, &retry_id, "native-ctl-1", "規劃好了");
    harness.supervise().await;
    let view = conversation(&harness, &ws).await;
    let done = turn(&view, &turn_id(&u1));
    assert_eq!(done["status"], "processed");
    assert_eq!(done["controller_run_id"].as_str(), Some(retry_id.as_str()));
    assert_eq!(controller_spawns(&harness), 1);
}

#[tokio::test]
async fn a_crash_after_the_controller_process_starts_adopts_it_without_a_second_spawn() {
    let (mut harness, ws, _) = start().await;
    let u1 = send(&harness, &ws, "請規劃下一步").await;
    advance(&harness, WINDOW_MS);
    harness.process_control.hang_next_spawn_at(HangPoint::AfterSpawn);
    let crashed = tokio::time::timeout(Duration::from_millis(300), harness.supervise()).await;
    assert!(crashed.is_err());
    assert_eq!(controller_spawns(&harness), 1);

    harness.restart().await;
    harness.supervise().await;
    let run = latest_run(&harness, &ws).await;
    assert_eq!(run["status"], "running", "以 command_id 找回原程序：{run}");
    assert_eq!(controller_spawns(&harness), 1, "不重複啟動");
    reply_and_exit(&harness, run["id"].as_str().unwrap(), "native-ctl-1", "規劃好了");
    harness.supervise().await;
    let view = conversation(&harness, &ws).await;
    assert_eq!(turn(&view, &turn_id(&u1))["status"], "processed");
    assert_eq!(turn(&view, &turn_id(&u1))["attempts"], 1);
    assert_eq!(controller_runs(&harness, &ws).await.len(), 1);
}

#[tokio::test]
async fn a_crash_before_the_result_is_saved_finds_the_unfinished_inputs_again() {
    let (mut harness, ws, _) = start().await;
    let u1 = send(&harness, &ws, "請整理目前進度").await;
    advance(&harness, WINDOW_MS);
    harness.supervise().await;
    let lost = latest_run(&harness, &ws).await;
    let lost_id = lost["id"].as_str().unwrap().to_string();

    // 程序已經結束，但結果保存的交易寫不進去，Runtime 隨即當掉
    reply_and_exit(&harness, &lost_id, "native-ctl-1", "這份回覆沒有保存");
    harness.inject_storage_fault(Some(StorageFaultKind::DiskFull));
    harness.supervise().await;
    harness.inject_storage_fault(None);
    let identity = harness.process_control.identity_of(lost["command_id"].as_str().unwrap()).unwrap();
    harness.process_control.script_liveness(&identity, Liveness::Dead);
    harness.restart().await;
    harness.supervise().await;

    let view = conversation(&harness, &ws).await;
    assert_eq!(view["runs"][0]["status"], "interrupted", "{view}");
    assert_eq!(turn(&view, &turn_id(&u1))["status"], "pending", "未完成的輸入找回為待處理");
    assert!(view["turns"].as_array().unwrap().iter().all(|t| t["kind"] != "controller_reply"), "沒有保存的回覆不出現");

    advance(&harness, BACKOFF_MS);
    harness.supervise().await;
    let retry = latest_run(&harness, &ws).await;
    let retry_id = retry["id"].as_str().unwrap().to_string();
    assert_ne!(retry_id, lost_id);
    assert!(prompt_of(&harness, &ws, &retry_id).await.contains("可能已部分處理"));
    reply_and_exit(&harness, &retry_id, "native-ctl-1", "進度整理好了");
    harness.supervise().await;
    let view = conversation(&harness, &ws).await;
    assert_eq!(turn(&view, &turn_id(&u1))["status"], "processed");
    let replies: Vec<&Value> = view["turns"].as_array().unwrap().iter().filter(|t| t["kind"] == "controller_reply").collect();
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0]["content"], "進度整理好了");
}

/// Controller 程序送出 `Started` 之後沒有 `Exited` 就死亡（崩潰、被終止、整機重開）。`restart` 時 Runtime 也重建；
/// 重新接上的串流同樣不會結束（真實 Adapter 讀到 `Exited` 之前會一直追蹤 stream 檔）。
async fn a_controller_that_dies_without_exiting_is_found_and_its_inputs_redelivered(restart: bool) {
    let (mut harness, ws, _) = start().await;
    let u1 = send(&harness, &ws, "請整理目前進度").await;
    advance(&harness, WINDOW_MS);
    harness.supervise().await;
    let lost = latest_run(&harness, &ws).await;
    let lost_id = lost["id"].as_str().unwrap().to_string();
    harness.agent_adapter.emit(&lost_id, AdapterEvent::Started { native_session_id: Some("native-ctl-1".into()) });
    harness.agent_adapter.emit(&lost_id, AdapterEvent::Message { text: "整理到一半".into() });
    harness.supervise().await;
    let identity = harness.process_control.identity_of(lost["command_id"].as_str().unwrap()).unwrap();
    if restart {
        harness.restart().await;
        harness.supervise().await;
    }

    // 無法確定程序在不在：不下結論，這一輪維持進行中
    harness.process_control.script_liveness(&identity, Liveness::Unknown);
    advance(&harness, 10 * 3_600_000);
    harness.supervise().await;
    harness.supervise().await;
    let view = conversation(&harness, &ws).await;
    assert_eq!(view["runs"][0]["status"], "running", "{view}");
    assert_eq!(turn(&view, &turn_id(&u1))["status"], "processing");

    // 確認程序已不在：串流沒有結束事件也找得回來
    harness.process_control.script_liveness(&identity, Liveness::Dead);
    advance(&harness, LIVENESS_CHECK_MS);
    harness.supervise().await;
    harness.supervise().await;
    let view = conversation(&harness, &ws).await;
    let interrupted = &view["runs"][0];
    assert_eq!(interrupted["status"], "interrupted", "{view}");
    assert!(interrupted["end_reason"].as_str().unwrap().contains("程序已不在"), "{interrupted}");
    assert_eq!(turn(&view, &turn_id(&u1))["status"], "pending", "未完成的輸入找回為待處理");
    assert_eq!(view["busy"], false);
    assert!(view["turns"].as_array().unwrap().iter().all(|t| t["kind"] != "controller_reply"), "沒有保存的回覆不出現");

    // 退避後重新交付，Prompt 註明可能已部分處理
    advance(&harness, BACKOFF_MS);
    harness.supervise().await;
    let retry = latest_run(&harness, &ws).await;
    let retry_id = retry["id"].as_str().unwrap().to_string();
    assert_ne!(retry_id, lost_id);
    assert_eq!(retry["input_turn_ids"], json!([turn_id(&u1)]));
    assert!(prompt_of(&harness, &ws, &retry_id).await.contains("可能已部分處理"));
    reply_and_exit(&harness, &retry_id, "native-ctl-2", "進度整理好了");
    harness.supervise().await;
    let view = conversation(&harness, &ws).await;
    let done = turn(&view, &turn_id(&u1));
    assert_eq!((done["status"].as_str(), done["attempts"].as_i64()), (Some("processed"), Some(2)), "{done}");
    let replies: Vec<&Value> = view["turns"].as_array().unwrap().iter().filter(|t| t["kind"] == "controller_reply").collect();
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0]["content"], "進度整理好了");
    assert_eq!(controller_spawns(&harness), 2);
}

#[tokio::test]
async fn a_controller_that_dies_without_exiting_is_interrupted_and_redelivered() {
    a_controller_that_dies_without_exiting_is_found_and_its_inputs_redelivered(false).await;
}

#[tokio::test]
async fn a_controller_that_dies_without_exiting_across_a_restart_is_interrupted_and_redelivered() {
    a_controller_that_dies_without_exiting_is_found_and_its_inputs_redelivered(true).await;
}

#[tokio::test]
async fn a_result_that_arrives_after_the_process_is_found_dead_is_still_saved() {
    let (harness, ws, _) = start().await;
    let u1 = send(&harness, &ws, "請整理目前進度").await;
    advance(&harness, WINDOW_MS);
    harness.supervise().await;
    let run = latest_run(&harness, &ws).await;
    let run_id = run["id"].as_str().unwrap().to_string();
    let identity = harness.process_control.identity_of(run["command_id"].as_str().unwrap()).unwrap();

    // 程序寫完結果就結束，但 Adapter 還沒讀到時，Supervisor 先查證到程序已不在
    harness.process_control.script_liveness(&identity, Liveness::Dead);
    advance(&harness, LIVENESS_CHECK_MS);
    harness.supervise().await;
    assert_eq!(latest_run(&harness, &ws).await["status"], "running", "確認不在之後再讀一次串流才下結論");
    reply_and_exit(&harness, &run_id, "native-ctl-1", "進度整理好了");
    harness.supervise().await;

    let view = conversation(&harness, &ws).await;
    assert_eq!(view["runs"][0]["status"], "succeeded", "{view}");
    assert_eq!(turn(&view, &turn_id(&u1))["status"], "processed");
    assert_eq!(controller_spawns(&harness), 1);
}

#[tokio::test]
async fn controller_wakes_wait_while_crash_recovery_blocks_dispatch() {
    let (mut harness, ws, _) = start().await;
    let (_, worker) = dispatched_task(&harness, &ws, "長時間研究").await;
    harness.supervise().await;
    let (_, run) = harness.send_json(Method::GET, &format!("{WORKSPACES}/{ws}/runs/{worker}"), None).await;
    let identity = harness.process_control.identity_of(run["command_id"].as_str().unwrap()).unwrap();
    harness.process_control.script_stop(&identity, FakeStop::Result(StopResult::StillAlive));
    send(&harness, &ws, "請評估進度").await;

    // 崩潰恢復無法確認舊 Worker 停止：退避期間不派工，也不喚起 Controller
    harness.crash_restart().await;
    advance(&harness, WINDOW_MS);
    harness.supervise().await;
    harness.supervise().await;
    let (_, recovery) = harness.send_json(Method::GET, "/api/v1/recovery", None).await;
    assert_eq!(recovery["blocking"]["code"], "crash_recovery_pending", "{recovery}");
    assert_eq!(controller_spawns(&harness), 0);
    assert_eq!(conversation(&harness, &ws).await["busy"], false);

    // 恢復完成（進入穩定觀察）後才喚起
    harness.process_control.script_stop(&identity, FakeStop::Result(StopResult::ForceKilled));
    advance(&harness, 30_000);
    harness.supervise().await;
    harness.supervise().await;
    assert_eq!(controller_spawns(&harness), 1);
}

#[tokio::test]
async fn controller_wakes_skip_an_account_held_by_the_quota_gate() {
    let harness = Harness::builder().clock_at(CLOCK_START).start().await;
    let account = harness.add_account("帳號 1", "account1@example.com").await;
    let path = plain_dir(&harness, "main");
    let (status, body) = harness.send_json(Method::POST, WORKSPACES, Some(json!({ "path": path }))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let ws = body["id"].as_str().unwrap().to_string();
    let observe = |used: f64| {
        let now = harness.clock.now();
        let sample =
            QuotaSample { source: QuotaSource::Observed, observed_at: now, used: Some(used), reset_at: None, confidence: Confidence::Observed };
        harness.quota_observer.set_sample(&account, sample);
    };

    // 80% 關閉門檻（paused）；降到 70% 仍在遲滯中，Controller 也不用這個帳號
    observe(0.8);
    harness.supervise().await;
    advance(&harness, 5 * 60_000);
    observe(0.7);
    send(&harness, &ws, "請評估進度").await;
    advance(&harness, WINDOW_MS);
    harness.supervise().await;
    assert_eq!(controller_spawns(&harness), 0);

    // 回到 60% 以下才恢復
    advance(&harness, 5 * 60_000);
    observe(0.5);
    harness.supervise().await;
    assert_eq!(controller_spawns(&harness), 1);
}

#[tokio::test]
async fn worker_blockers_become_work_events_once_a_conversation_exists() {
    let (harness, ws, _) = start().await;
    send(&harness, &ws, "開始吧").await;
    advance(&harness, WINDOW_MS);
    harness.supervise().await;
    let first = latest_run(&harness, &ws).await;
    reply_and_exit(&harness, first["id"].as_str().unwrap(), "native-ctl-1", "好");
    harness.supervise().await;

    let (_, worker) = dispatched_task(&harness, &ws, "撰寫文件").await;
    harness.supervise().await;
    let token = harness.agent_credential(&worker).expect("Worker 應該已啟動");
    let (status, ack) = harness.call_tool(&token, "report_blocked", json!({ "reason": "缺少範本" })).await;
    assert!(status.is_success(), "{ack}");
    let view = conversation(&harness, &ws).await;
    let events: Vec<&Value> = view["turns"].as_array().unwrap().iter().filter(|t| t["kind"] == "work_event").collect();
    assert_eq!(events.len(), 1, "{view}");
    let content = events[0]["content"].as_str().unwrap();
    assert!(content.contains("撰寫文件") && content.contains("缺少範本"), "{content}");
    assert_eq!(view["next_wake_at"], json!(CLOCK_START.as_millis() + WINDOW_MS + WINDOW_MS));

    advance(&harness, WINDOW_MS);
    harness.supervise().await;
    let second = latest_run(&harness, &ws).await;
    assert_eq!(second["input_turn_ids"], json!([events[0]["id"]]));
}

#[tokio::test]
async fn cancel_takes_effect_while_the_controller_is_busy() {
    let (harness, ws, _) = start().await;
    let (_, worker) = dispatched_task(&harness, &ws, "長時間研究").await;
    harness.supervise().await;
    send(&harness, &ws, "請評估進度").await;
    advance(&harness, WINDOW_MS);
    harness.supervise().await;
    send(&harness, &ws, "還有一則待處理").await;
    assert_eq!(conversation(&harness, &ws).await["busy"], true);

    // 取消直接由 Runtime 處理，不排在對話佇列之後，也不成為 Controller 的輸入
    let (status, cancelled) =
        harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/runs/{worker}/cancel"), None).await;
    assert_eq!(status, StatusCode::OK, "{cancelled}");
    assert!(cancelled["cancel_requested_at"].is_number(), "{cancelled}");
    harness.supervise().await;
    let (_, run) = harness.send_json(Method::GET, &format!("{WORKSPACES}/{ws}/runs/{worker}"), None).await;
    assert_eq!(run["status"], "cancelled", "{run}");
    let view = conversation(&harness, &ws).await;
    assert_eq!(view["busy"], true, "Controller 那一輪不受影響");
    assert_eq!(view["turns"].as_array().unwrap().len(), 2, "取消不成為對話輸入：{view}");
}

#[tokio::test]
async fn full_exit_stops_the_controller_and_keeps_its_inputs_pending() {
    let (harness, ws, _) = start().await;
    let u1 = send(&harness, &ws, "請評估進度").await;
    advance(&harness, WINDOW_MS);
    harness.supervise().await;
    let run = latest_run(&harness, &ws).await;
    assert_eq!(run["status"], "running");

    let (status, body) = harness.send_json(Method::POST, "/api/v1/lifecycle/exit", None).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    harness.wait_for_exit().await;
    let identity = harness.process_control.identity_of(run["command_id"].as_str().unwrap()).unwrap();
    assert!(harness.process_control.stop_calls().contains(&identity), "Controller 的程序也被停止");
    let view = conversation(&harness, &ws).await;
    assert_eq!(view["runs"][0]["status"], "interrupted", "{view}");
    assert_eq!(turn(&view, &turn_id(&u1))["status"], "pending");

    // 停止意圖生效期間不喚醒
    advance(&harness, 3_600_000);
    harness.supervise().await;
    assert_eq!(controller_spawns(&harness), 1);
}

#[tokio::test]
async fn messages_are_validated_and_scoped_to_an_existing_workspace() {
    let (harness, ws, _) = start().await;
    let (status, body) = harness.send_json(Method::POST, &messages_path(&ws), Some(json!({ "text": "   " }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "message_empty");
    let long = "字".repeat(20_001);
    let (status, body) = harness.send_json(Method::POST, &messages_path(&ws), Some(json!({ "text": long }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "message_too_long");
    let (status, body) = harness.send_json(Method::POST, &messages_path("ws_missing"), Some(json!({ "text": "嗨" }))).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    let (status, _) = harness.send_json(Method::GET, &conversation_path("ws_missing"), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // Agent 憑證不能使用對話端點
    let token = harness.issue_credential(baton_domain::auth::Role::Agent, None).await;
    let response = harness
        .request(Method::GET, &conversation_path(&ws))
        .bearer_auth(token)
        .header(baton_api::CLIENT_HEADER, baton_api::CLIENT_HEADER_VALUE)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}
