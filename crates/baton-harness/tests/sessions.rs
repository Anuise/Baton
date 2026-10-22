//! 情境測試：Session 持久化、恢復與替換（票 15），經由 Runtime harness 的真實 HTTP API 驗證。
//!
//! 全部使用 fake ProcessControl／AgentAdapter 與 fake Clock：測試決定何時讓 Supervisor 處理一次（`supervise`）、
//! Adapter 回報哪個原生 Session、原生 Session 能否接續，以及 Runtime 在 spawn 的哪一點「當掉」。
//! 只斷言外部行為：HTTP 回應（Session、Run）、重啟後仍在、Port 被呼叫的方式（spawn 參數、接續能力查驗、停止）；
//! `harness.db()` 只用來確認事件 payload 這類必須看落地內容的事實；transcript 檔案則直接讀成果儲存。
//!
//! 每個測試名稱對應票 15 的行為：
//! - 第一次派工建立 Session、綁定與暫停、重啟後仍在：`the_first_launch_creates_a_session_...`
//! - 同帳號續跑沿用 Session 並以原生方式接續：`a_retry_on_the_same_account_resumes_...`
//! - 原生 Session 不可用 → 替代 Session 並標示缺口：`an_unavailable_native_session_is_replaced_...`
//! - 接續能力未知 → 替代 Session、不宣稱可續用：`an_unknown_resume_capability_...`
//! - 跨帳號 → 替代 Session、不查驗：`switching_accounts_replaces_the_session_...`
//! - 同一原生 Session 不能被兩個執行者同時驅動：`a_second_run_reporting_a_native_session_in_use_...`
//! - 原生 ID 取得前中斷：`crash_after_spawn_before_the_native_id_...`、`a_launch_that_never_bound_...`
//! - transcript 以事件位置去重、記錄缺口、遮蔽憑證：`transcript_import_...`

use std::time::Duration;

use baton_domain::artifact::sha256_hex;
use baton_domain::Timestamp;
use baton_harness::fakes::HangPoint;
use baton_harness::Harness;
use baton_ports::{AdapterEvent, Capability, SessionRef};
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

const WORKSPACES: &str = "/api/v1/workspaces";
const PROFILES: &str = "/api/v1/profiles";

/// 2023-11-14 22:13:20.123 UTC
const CLOCK_START: Timestamp = Timestamp::from_millis(1_700_000_000_123);

/// 形狀與 Agent 憑證相同的秘密：64 個十六進位字元。
const HEX_TOKEN: &str = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";
/// Anthropic API 金鑰形狀的秘密。
const API_KEY: &str = "sk-ant-api03-Zq8xW3vB9nK2mL5pR7tY1uI4oP6aS0dF-gH8jK2lZ4xC6vB";

/// Run 尚未結束的狀態。
const UNFINISHED: [&str; 3] = ["queued", "starting", "running"];

// --- 共用輔助（沿用 runs.rs／scheduling.rs 的寫法） ---

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

fn sessions_path(ws: &str) -> String {
    format!("{WORKSPACES}/{ws}/sessions")
}

fn session_path(ws: &str, sid: &str) -> String {
    format!("{WORKSPACES}/{ws}/sessions/{sid}")
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

/// 一個 Workspace、一個 Goal，回傳 (workspace_id, workspace_path, goal_id)。
async fn setup(harness: &Harness) -> (String, String, String) {
    let path = plain_dir(harness, "main");
    let (status, body) = harness.send_json(Method::POST, WORKSPACES, Some(json!({ "path": path }))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let ws = body["id"].as_str().unwrap().to_string();
    let path = body["path"].as_str().unwrap().to_string();
    let (status, goal) =
        harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/goals"), Some(json!({ "title": "目標" }))).await;
    assert_eq!(status, StatusCode::CREATED, "{goal}");
    (ws, path, goal["id"].as_str().unwrap().to_string())
}

/// 建立 Task（不需要任何能力的完整契約）並讓它 ready，回傳識別。
async fn ready_task(harness: &Harness, ws: &str, goal: &str, title: &str) -> String {
    let body = json!({ "goal_id": goal, "title": title, "contract": contract() });
    let (status, task) = harness.send_json(Method::POST, &tasks_path(ws), Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    let tid = task["id"].as_str().unwrap().to_string();
    let (status, ready) = harness.send_json(Method::POST, &format!("{}/ready", task_path(ws, &tid)), None).await;
    assert_eq!(status, StatusCode::OK, "{ready}");
    tid
}

/// 派工並要求成功（201），回傳 Run 識別。
async fn dispatch_ok(harness: &Harness, ws: &str, tid: &str) -> String {
    let (status, run) = harness.send_json(Method::POST, &format!("{}/dispatch", task_path(ws, tid)), None).await;
    assert_eq!(status, StatusCode::CREATED, "{run}");
    run["id"].as_str().unwrap().to_string()
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

fn status_of(run: &Value) -> &str {
    run["status"].as_str().unwrap()
}

/// 這個 Run 指向的平台 Session 識別（沒有時測試失敗）。
async fn session_of_run(harness: &Harness, ws: &str, rid: &str) -> String {
    let run = run_view(harness, ws, rid).await;
    run["agent_session_id"].as_str().unwrap_or_else(|| panic!("Run 沒有 agent_session_id：{run}")).to_string()
}

/// `GET /sessions/{id}` 的完整回應（session、events、segments）。
async fn session_detail(harness: &Harness, ws: &str, sid: &str) -> Value {
    let (status, detail) = harness.send_json(Method::GET, &session_path(ws, sid), None).await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    detail
}

async fn session(harness: &Harness, ws: &str, sid: &str) -> Value {
    session_detail(harness, ws, sid).await["session"].clone()
}

/// Session 事件種類，依順序。
async fn session_event_types(harness: &Harness, ws: &str, sid: &str) -> Vec<String> {
    session_detail(harness, ws, sid).await["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["event_type"].as_str().unwrap().to_string())
        .collect()
}

/// Workspace 的全部 Session（依建立順序）。
async fn sessions(harness: &Harness, ws: &str) -> Vec<Value> {
    let (status, list) = harness.send_json(Method::GET, &sessions_path(ws), None).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    list["sessions"].as_array().unwrap_or_else(|| panic!("沒有 sessions：{list}")).clone()
}

/// 屬於某個 Task 的 Session（依建立順序）。
async fn sessions_of_task(harness: &Harness, ws: &str, tid: &str) -> Vec<Value> {
    sessions(harness, ws).await.into_iter().filter(|s| s["task_id"] == tid).collect()
}

/// 某個 Task 目前尚未結束的 Run 數（經由 Run 清單 API）。
async fn unfinished_runs(harness: &Harness, ws: &str, tid: &str) -> usize {
    let (status, list) = harness.send_json(Method::GET, &format!("{WORKSPACES}/{ws}/runs"), None).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    list["runs"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["task_id"] == tid && UNFINISHED.contains(&status_of(r)))
        .count()
}

/// 某個 Run 啟動時的 spawn 參數（fake Adapter 的第一個參數是 run_id）。
fn spawn_args_of(harness: &Harness, rid: &str) -> Vec<String> {
    harness
        .process_control
        .spawned()
        .into_iter()
        .find(|spec| spec.args.first().map(String::as_str) == Some(rid))
        .unwrap_or_else(|| panic!("Run {rid} 沒有被啟動"))
        .args
}

/// fake Adapter 第一次 `launch` 這個 Run 時收到的 RunSpec。
fn launched_spec(harness: &Harness, rid: &str) -> baton_ports::RunSpec {
    harness
        .agent_adapter
        .launched()
        .into_iter()
        .find(|spec| spec.run_id == rid)
        .unwrap_or_else(|| panic!("Run {rid} 沒有被 launch"))
}

/// 某個 Run 的程序身分。
async fn identity_of_run(harness: &Harness, ws: &str, rid: &str) -> baton_ports::ProcessIdentity {
    let run = run_view(harness, ws, rid).await;
    harness.process_control.identity_of(run["command_id"].as_str().unwrap()).unwrap()
}

/// 讓 Supervisor 處理一次；若 spawn 設成會「當掉」，等一小段時間後放棄那一次處理（等同 Runtime 在那一點崩潰）。
async fn supervise_until_crash(harness: &Harness) {
    let outcome = tokio::time::timeout(Duration::from_millis(300), harness.supervise()).await;
    assert!(outcome.is_err(), "預期 Supervisor 卡在被設定的崩潰點");
}

/// 第一個 Run：啟動、綁定原生 Session `native`，以失敗結束讓 Task 回到 ready。回傳 (run_id, session_id)。
async fn first_run_bound_then_failed(harness: &Harness, ws: &str, tid: &str, native: &str) -> (String, String) {
    let rid = dispatch_ok(harness, ws, tid).await;
    harness.supervise().await;
    harness.agent_adapter.emit(&rid, AdapterEvent::Started { native_session_id: Some(native.to_string()) });
    harness.supervise().await;
    harness.agent_adapter.emit(&rid, AdapterEvent::Exited { success: false });
    harness.supervise().await;
    assert_eq!(status_of(&run_view(harness, ws, &rid).await), "failed");
    assert_eq!(task_status(harness, ws, tid).await, "ready");
    let sid = session_of_run(harness, ws, &rid).await;
    let s1 = session(harness, ws, &sid).await;
    assert_eq!((s1["state"].as_str(), s1["native_session_id"].as_str()), (Some("suspended"), Some(native)), "{s1}");
    (rid, sid)
}

fn event_payloads(harness: &Harness, aggregate_type: &str, id: &str, event_type: &str) -> Vec<Value> {
    let db = harness.db();
    let mut statement = db
        .prepare("SELECT payload FROM event WHERE aggregate_type = ?1 AND aggregate_id = ?2 AND event_type = ?3 ORDER BY seq")
        .unwrap();
    statement
        .query_map(rusqlite::params![aggregate_type, id, event_type], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|p| serde_json::from_str(&p.unwrap()).unwrap())
        .collect()
}

fn transcript(position: u64, line: &str) -> AdapterEvent {
    AdapterEvent::Transcript { position, line: line.to_string() }
}

/// Session 的 segments，依 first_position 排序。
async fn segments(harness: &Harness, ws: &str, sid: &str) -> Vec<Value> {
    let mut found = session_detail(harness, ws, sid).await["segments"]
        .as_array()
        .unwrap_or_else(|| panic!("Session 回應沒有 segments"))
        .clone();
    found.sort_by_key(|s| s["first_position"].as_u64().unwrap());
    found
}

fn ranges(segments: &[Value]) -> Vec<(u64, u64, String)> {
    segments
        .iter()
        .map(|s| {
            (
                s["first_position"].as_u64().unwrap(),
                s["last_position"].as_u64().unwrap(),
                s["availability"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

/// 讀取 stored segment 的檔案，並確認 sha256 與 byte_len 與檔案相符。
fn segment_file(harness: &Harness, segment: &Value) -> Vec<u8> {
    let relative = segment["storage_path"].as_str().unwrap_or_else(|| panic!("stored segment 沒有 storage_path：{segment}"));
    let bytes = std::fs::read(harness.artifacts_dir().join(relative)).expect("讀不到 segment 檔案");
    assert_eq!(segment["sha256"].as_str(), Some(sha256_hex(&bytes).as_str()), "sha256 是檔案內容的雜湊：{segment}");
    assert_eq!(segment["byte_len"].as_u64(), Some(bytes.len() as u64), "{segment}");
    bytes
}

// ============================================================ 第一次派工

#[tokio::test]
async fn the_first_launch_creates_a_session_that_binds_then_suspends_and_survives_a_restart() {
    let (mut harness, accounts) = start_with_accounts(1).await;
    let (ws, path, goal) = setup(&harness).await;
    let tid = ready_task(&harness, &ws, &goal, "整理資料").await;
    let rid = dispatch_ok(&harness, &ws, &tid).await;
    assert!(sessions(&harness, &ws).await.is_empty(), "派工本身不建立 Session，要等 launch intent");

    harness.supervise().await;

    let run = run_view(&harness, &ws, &rid).await;
    assert_eq!(status_of(&run), "running", "{run}");
    let sid = run["agent_session_id"].as_str().unwrap_or_else(|| panic!("Run 沒有 agent_session_id：{run}")).to_string();
    assert!(run["resume_native_session_id"].is_null(), "第一次派工不接續任何原生 Session：{run}");
    let allocating = session(&harness, &ws, &sid).await;
    assert_eq!(allocating["id"], json!(sid));
    assert_eq!(allocating["runtime_type"], "claude_code");
    assert_eq!(allocating["workspace_id"], json!(ws));
    assert_eq!(allocating["role"], "worker");
    assert_eq!(allocating["task_id"], json!(tid));
    assert!(allocating["conversation_id"].is_null());
    assert_eq!(allocating["profile_id"], json!(accounts[0]), "帳號 = Run 的 Profile");
    assert_eq!(allocating["profile_id"], run["profile_id"]);
    // 工作目錄 = 派工時固定給這個 Task 的目錄（票 20 起每個 Task 有自己的目錄，位於 Workspace 之下）
    let launched_dir = launched_spec(&harness, &rid).working_dir.expect("Run 有工作目錄");
    assert!(launched_dir.starts_with(&path), "{launched_dir}");
    assert_eq!(allocating["working_directory"], json!(launched_dir), "工作目錄 = Run 啟動時的工作目錄");
    assert!(allocating["native_session_id"].is_null(), "綁定前沒有原生識別：{allocating}");
    assert_eq!(allocating["state"], "allocating", "{allocating}");
    assert!(allocating["state_reason"].is_null());
    assert_eq!(allocating["driver_run_id"], json!(rid));
    assert!(allocating["predecessor_session_id"].is_null());
    assert!(allocating["replacement_reason"].is_null());
    assert!(allocating["history_gap"].is_null(), "沒有前任就沒有紀錄缺口：{allocating}");
    assert_eq!(allocating["created_at"].as_i64(), Some(CLOCK_START.as_millis()));
    assert_eq!(allocating["run_ids"], json!([rid]));
    let listed = sessions(&harness, &ws).await;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["id"], json!(sid));

    // Adapter 回報原生 Session：綁定並進入 active
    harness.agent_adapter.emit(&rid, AdapterEvent::Started { native_session_id: Some("N1".into()) });
    harness.supervise().await;
    let active = session(&harness, &ws, &sid).await;
    assert_eq!((active["state"].as_str(), active["native_session_id"].as_str()), (Some("active"), Some("N1")), "{active}");
    assert_eq!(active["driver_run_id"], json!(rid));

    // 程序結束：Session 暫停，沒有正在驅動它的 Run
    harness.agent_adapter.emit(&rid, AdapterEvent::Exited { success: true });
    harness.supervise().await;
    let detail = session_detail(&harness, &ws, &sid).await;
    let suspended = &detail["session"];
    assert_eq!(suspended["state"], "suspended", "{suspended}");
    assert!(suspended["driver_run_id"].is_null(), "{suspended}");
    assert_eq!(suspended["native_session_id"], "N1", "原生識別保留，之後才能接續");
    assert!(suspended["last_activity_at"].as_i64().is_some(), "{suspended}");
    let types: Vec<&str> = detail["events"].as_array().unwrap().iter().map(|e| e["event_type"].as_str().unwrap()).collect();
    assert_eq!(types, ["session_created", "session_native_bound", "session_suspended"], "{detail}");
    let versions: Vec<i64> = detail["events"].as_array().unwrap().iter().map(|e| e["aggregate_version"].as_i64().unwrap()).collect();
    assert_eq!(versions, [1, 2, 3]);
    assert_eq!(suspended["version"], 3, "版本 = 事件數");

    // 重建 Runtime 後關聯仍在
    harness.restart().await;
    assert_eq!(session_detail(&harness, &ws, &sid).await, detail, "重建後 Session 與事件不變");
    let after = run_view(&harness, &ws, &rid).await;
    assert_eq!(after["agent_session_id"], json!(sid));
    assert_eq!(sessions(&harness, &ws).await.len(), 1);
}

#[tokio::test]
async fn an_unknown_session_is_not_found() {
    let (harness, _) = start_with_accounts(1).await;
    let (ws, _, _) = setup(&harness).await;

    let (status, body) = harness.send_json(Method::GET, &session_path(&ws, "ses_nope"), None).await;

    assert_eq!((status, error_code(&body)), (StatusCode::NOT_FOUND, "session_not_found"), "{body}");
}

// ============================================================ 同帳號續跑

#[tokio::test]
async fn a_retry_on_the_same_account_resumes_the_same_session_natively() {
    let (harness, accounts) = start_with_accounts(1).await;
    let (ws, path, goal) = setup(&harness).await;
    let tid = ready_task(&harness, &ws, &goal, "整理資料").await;
    let (first, sid) = first_run_bound_then_failed(&harness, &ws, &tid, "N1").await;
    let first_spec = launched_spec(&harness, &first);

    let second = dispatch_ok(&harness, &ws, &tid).await;
    harness.supervise().await;

    let run = run_view(&harness, &ws, &second).await;
    assert_eq!(run["profile_id"], json!(accounts[0]));
    assert_eq!(run["agent_session_id"], json!(sid), "同帳號、同工作目錄：沿用同一個 Session：{run}");
    assert_eq!(run["resume_native_session_id"], "N1", "{run}");
    assert_eq!(launched_spec(&harness, &second).resume_native_session_id.as_deref(), Some("N1"));
    let args = spawn_args_of(&harness, &second);
    assert!(args.windows(2).any(|w| w == ["--resume", "N1"]), "以原生方式接續：{args:?}");
    let expected_ref = SessionRef {
        native_session_id: "N1".into(),
        config_dir: first_spec.config_dir.clone(),
        working_dir: first_spec.working_dir.clone(),
    };
    assert!(first_spec.working_dir.as_deref().is_some_and(|dir| dir.starts_with(&path)));
    assert!(expected_ref.config_dir.is_some(), "Run 有帳號的設定目錄");
    assert_eq!(harness.agent_adapter.capability_queries(), vec![expected_ref], "查驗帶帳號設定目錄與當初的工作目錄");

    harness.agent_adapter.emit(&second, AdapterEvent::Started { native_session_id: Some("N1".into()) });
    harness.supervise().await;

    let resumed = session(&harness, &ws, &sid).await;
    assert_eq!(resumed["state"], "active", "{resumed}");
    assert_eq!(resumed["native_session_id"], "N1");
    assert_eq!(resumed["driver_run_id"], json!(second));
    assert_eq!(resumed["run_ids"], json!([first, second]));
    assert_eq!(resumed["resume_capability"], "resumable", "{resumed}");
    assert!(resumed["capability_checked_at"].as_i64().is_some(), "{resumed}");
    assert!(session_event_types(&harness, &ws, &sid).await.contains(&"session_resumed".to_string()));
    assert_eq!(sessions_of_task(&harness, &ws, &tid).await.len(), 1, "沒有建立新的 Session");
}

// ============================================================ 原生 Session 不可用

#[tokio::test]
async fn an_unavailable_native_session_is_replaced_with_a_new_session_and_a_history_gap() {
    let (harness, _) = start_with_accounts(1).await;
    let (ws, _, goal) = setup(&harness).await;
    let tid = ready_task(&harness, &ws, &goal, "整理資料").await;
    let (_, s1) = first_run_bound_then_failed(&harness, &ws, &tid, "N1").await;
    harness.agent_adapter.set_session_capability("N1", Capability::NotResumable);

    let second = dispatch_ok(&harness, &ws, &tid).await;
    harness.supervise().await;

    let run = run_view(&harness, &ws, &second).await;
    let s2 = run["agent_session_id"].as_str().unwrap_or_else(|| panic!("{run}")).to_string();
    assert_ne!(s2, s1, "原生 Session 不可用：建立替代 Session");
    assert!(run["resume_native_session_id"].is_null(), "{run}");
    assert_eq!(launched_spec(&harness, &second).resume_native_session_id, None);
    assert!(!spawn_args_of(&harness, &second).contains(&"--resume".to_string()), "不宣稱接續");
    assert_eq!(harness.agent_adapter.capability_queries().len(), 1, "有查驗過 N1");

    let replacement = session(&harness, &ws, &s2).await;
    assert_eq!(replacement["predecessor_session_id"], json!(s1), "{replacement}");
    assert_eq!(replacement["replacement_reason"], "native_session_unavailable");
    let gap = replacement["history_gap"].as_str().unwrap_or_default();
    assert!(!gap.is_empty(), "替代 Session 標示紀錄缺口：{replacement}");
    assert!(replacement["native_session_id"].is_null());
    assert_eq!(replacement["driver_run_id"], json!(second));
    assert_eq!(replacement["run_ids"], json!([second]));

    let predecessor = session(&harness, &ws, &s1).await;
    assert_eq!(predecessor["state"], "unavailable", "{predecessor}");
    assert_eq!(predecessor["state_reason"], "native_session_unavailable");
    assert_eq!(predecessor["resume_capability"], "not_resumable");
    assert!(predecessor["driver_run_id"].is_null());
    assert!(session_event_types(&harness, &ws, &s1).await.contains(&"session_unavailable".to_string()));
    let ids: Vec<Value> = sessions_of_task(&harness, &ws, &tid).await.iter().map(|s| s["id"].clone()).collect();
    assert_eq!(ids, [json!(s1), json!(s2)], "依建立順序");
}

#[tokio::test]
async fn an_unknown_resume_capability_replaces_the_session_and_never_claims_it_is_resumable() {
    let (harness, _) = start_with_accounts(1).await;
    let (ws, _, goal) = setup(&harness).await;
    let tid = ready_task(&harness, &ws, &goal, "整理資料").await;
    let (_, s1) = first_run_bound_then_failed(&harness, &ws, &tid, "N1").await;
    harness.agent_adapter.set_session_capability("N1", Capability::Unknown);

    let second = dispatch_ok(&harness, &ws, &tid).await;
    harness.supervise().await;

    let run = run_view(&harness, &ws, &second).await;
    let s2 = run["agent_session_id"].as_str().unwrap_or_else(|| panic!("{run}")).to_string();
    assert_ne!(s2, s1);
    assert!(run["resume_native_session_id"].is_null(), "未知不當成可接續：{run}");
    assert!(!spawn_args_of(&harness, &second).contains(&"--resume".to_string()));
    let replacement = session(&harness, &ws, &s2).await;
    assert_eq!(replacement["predecessor_session_id"], json!(s1), "{replacement}");
    assert_eq!(replacement["replacement_reason"], "resume_capability_unknown");
    assert!(!replacement["history_gap"].as_str().unwrap_or_default().is_empty(), "{replacement}");
    let predecessor = session(&harness, &ws, &s1).await;
    assert_eq!(predecessor["state"], "closed", "不宣稱可續用：{predecessor}");
    assert_eq!(predecessor["resume_capability"], "unknown");
    assert!(session_event_types(&harness, &ws, &s1).await.contains(&"session_closed".to_string()));
}

// ============================================================ 跨帳號

#[tokio::test]
async fn switching_accounts_replaces_the_session_without_checking_the_old_native_session() {
    let (harness, accounts) = start_with_accounts(2).await;
    let (ws, _, goal) = setup(&harness).await;
    let tid = ready_task(&harness, &ws, &goal, "整理資料").await;
    let (first, s1) = first_run_bound_then_failed(&harness, &ws, &tid, "N1").await;
    assert_eq!(run_view(&harness, &ws, &first).await["profile_id"], json!(accounts[0]), "第一個 Run 用較早建立的帳號");
    // 帳號 A 不能再用：封存
    let (status, archived) =
        harness.send_json(Method::POST, &format!("{PROFILES}/{}/archive", accounts[0]), None).await;
    assert_eq!(status, StatusCode::OK, "{archived}");

    let second = dispatch_ok(&harness, &ws, &tid).await;
    harness.supervise().await;

    let run = run_view(&harness, &ws, &second).await;
    assert_eq!(run["profile_id"], json!(accounts[1]), "{run}");
    let s2 = run["agent_session_id"].as_str().unwrap_or_else(|| panic!("{run}")).to_string();
    assert_ne!(s2, s1, "跨帳號不沿用 Session");
    assert!(run["resume_native_session_id"].is_null(), "{run}");
    assert!(!spawn_args_of(&harness, &second).contains(&"--resume".to_string()));
    assert!(harness.agent_adapter.capability_queries().is_empty(), "跨帳號不對舊的原生 Session 查驗接續能力");
    let replacement = session(&harness, &ws, &s2).await;
    assert_eq!(replacement["profile_id"], json!(accounts[1]), "{replacement}");
    assert_eq!(replacement["predecessor_session_id"], json!(s1));
    assert_eq!(replacement["replacement_reason"], "account_changed");
    assert!(!replacement["history_gap"].as_str().unwrap_or_default().is_empty(), "{replacement}");
    let predecessor = session(&harness, &ws, &s1).await;
    assert_eq!(predecessor["state"], "closed", "{predecessor}");
    assert_eq!(predecessor["state_reason"], "account_changed");
    assert_eq!(predecessor["profile_id"], json!(accounts[0]));
    assert!(predecessor["capability_checked_at"].is_null(), "沒有查驗：{predecessor}");
}

// ============================================================ 同一原生 Session 只有一個執行者

#[tokio::test]
async fn a_second_run_reporting_a_native_session_in_use_is_cancelled_and_never_bound() {
    let (harness, _) = start_with_accounts(1).await;
    let (ws, _, goal) = setup(&harness).await;
    let task_a = ready_task(&harness, &ws, &goal, "甲").await;
    let task_b = ready_task(&harness, &ws, &goal, "乙").await;
    let run_a = dispatch_ok(&harness, &ws, &task_a).await;
    let run_b = dispatch_ok(&harness, &ws, &task_b).await;
    harness.supervise().await;
    harness.agent_adapter.emit(&run_a, AdapterEvent::Started { native_session_id: Some("N1".into()) });
    harness.supervise().await;
    let session_a = session_of_run(&harness, &ws, &run_a).await;
    let session_b = session_of_run(&harness, &ws, &run_b).await;
    assert_ne!(session_a, session_b);
    let identity_a = identity_of_run(&harness, &ws, &run_a).await;
    let identity_b = identity_of_run(&harness, &ws, &run_b).await;

    // A 仍在執行時，B 也回報同一個原生 Session
    harness.agent_adapter.emit(&run_b, AdapterEvent::Started { native_session_id: Some("N1".into()) });
    harness.supervise().await;

    let b = run_view(&harness, &ws, &run_b).await;
    assert!(!b["cancel_requested_at"].is_null(), "B 被要求取消：{b}");
    let payloads = event_payloads(&harness, "run", &run_b, "run_cancel_requested");
    assert_eq!(payloads.len(), 1, "{payloads:?}");
    assert_eq!(payloads[0]["reason"], "native_session_in_use", "{payloads:?}");
    let b_session = session(&harness, &ws, &session_b).await;
    assert!(b_session["native_session_id"].is_null(), "B 的 Session 沒有綁定 N1：{b_session}");
    assert!(session_event_types(&harness, &ws, &session_b).await.contains(&"session_native_conflict".to_string()));

    // 下一次處理：停止 B 的程序，A 不受影響
    harness.supervise().await;
    let stops = harness.process_control.stop_calls();
    assert!(stops.contains(&identity_b), "B 的程序被停止：{stops:?}");
    assert!(!stops.contains(&identity_a), "A 的程序不受影響：{stops:?}");
    let a = run_view(&harness, &ws, &run_a).await;
    assert_eq!(status_of(&a), "running", "{a}");
    assert!(a["cancel_requested_at"].is_null());
    let a_session = session(&harness, &ws, &session_a).await;
    assert_eq!(a_session["state"], "active", "{a_session}");
    assert_eq!(a_session["native_session_id"], "N1");
    assert_eq!(a_session["driver_run_id"], json!(run_a));
    assert!(!session_event_types(&harness, &ws, &session_a).await.contains(&"session_native_conflict".to_string()));
}

// ============================================================ 原生 ID 取得前中斷

#[tokio::test]
async fn crash_after_spawn_before_the_native_id_is_reconciled_by_command_id_without_a_second_writer() {
    let (mut harness, _) = start_with_accounts(1).await;
    let (ws, _, goal) = setup(&harness).await;
    let tid = ready_task(&harness, &ws, &goal, "整理資料").await;
    let rid = dispatch_ok(&harness, &ws, &tid).await;
    harness.process_control.hang_next_spawn_at(HangPoint::AfterSpawn);
    supervise_until_crash(&harness).await;
    assert_eq!(harness.process_control.spawn_count(), 1);

    harness.restart().await;
    harness.supervise().await;

    assert_eq!(harness.process_control.spawn_count(), 1, "以 command_id 找回原程序，不重複啟動");
    let run = run_view(&harness, &ws, &rid).await;
    assert_eq!(status_of(&run), "running", "{run}");
    let found = sessions_of_task(&harness, &ws, &tid).await;
    assert_eq!(found.len(), 1, "只有一個 Session：{found:?}");
    assert_eq!(found[0]["driver_run_id"], json!(rid));
    assert_eq!(run["agent_session_id"], found[0]["id"]);
    assert!(found[0]["native_session_id"].is_null(), "還沒取得原生識別：{}", found[0]);
    assert_eq!(unfinished_runs(&harness, &ws, &tid).await, 1);

    harness.agent_adapter.emit(&rid, AdapterEvent::Started { native_session_id: Some("N1".into()) });
    harness.supervise().await;

    let sid = found[0]["id"].as_str().unwrap();
    let bound = session(&harness, &ws, sid).await;
    assert_eq!((bound["state"].as_str(), bound["native_session_id"].as_str()), (Some("active"), Some("N1")), "{bound}");
    assert_eq!(sessions_of_task(&harness, &ws, &tid).await.len(), 1);
}

#[tokio::test]
async fn a_launch_that_never_bound_a_native_session_is_replaced_without_a_capability_check() {
    let (mut harness, _) = start_with_accounts(1).await;
    let (ws, _, goal) = setup(&harness).await;
    let tid = ready_task(&harness, &ws, &goal, "整理資料").await;
    let first = dispatch_ok(&harness, &ws, &tid).await;
    assert_eq!(unfinished_runs(&harness, &ws, &tid).await, 1);
    harness.process_control.hang_next_spawn_at(HangPoint::BeforeSpawn);
    supervise_until_crash(&harness).await;
    assert_eq!(unfinished_runs(&harness, &ws, &tid).await, 1);

    // 重建後找不到程序：啟動結果不明，Run 中斷
    harness.restart().await;
    harness.supervise().await;
    harness.supervise().await;

    let run = run_view(&harness, &ws, &first).await;
    assert_eq!(status_of(&run), "interrupted", "{run}");
    assert_eq!(unfinished_runs(&harness, &ws, &tid).await, 0);
    let s1 = run["agent_session_id"].as_str().unwrap_or_else(|| panic!("{run}")).to_string();
    let never_bound = session(&harness, &ws, &s1).await;
    assert_eq!(never_bound["state"], "unavailable", "{never_bound}");
    assert_eq!(never_bound["state_reason"], "native_session_never_bound");
    assert!(never_bound["native_session_id"].is_null());
    assert!(never_bound["driver_run_id"].is_null());

    // 使用者重新派工：新的 Session 以它為前任
    let second = dispatch_ok(&harness, &ws, &tid).await;
    assert_eq!(unfinished_runs(&harness, &ws, &tid).await, 1);
    harness.supervise().await;
    assert_eq!(unfinished_runs(&harness, &ws, &tid).await, 1);

    let rerun = run_view(&harness, &ws, &second).await;
    assert_eq!(status_of(&rerun), "running", "{rerun}");
    let s2 = rerun["agent_session_id"].as_str().unwrap_or_else(|| panic!("{rerun}")).to_string();
    assert_ne!(s2, s1);
    assert!(rerun["resume_native_session_id"].is_null(), "{rerun}");
    let replacement = session(&harness, &ws, &s2).await;
    assert_eq!(replacement["predecessor_session_id"], json!(s1), "{replacement}");
    assert_eq!(replacement["replacement_reason"], "native_session_never_bound");
    assert!(!replacement["history_gap"].as_str().unwrap_or_default().is_empty(), "{replacement}");
    assert!(harness.agent_adapter.capability_queries().is_empty(), "沒有原生識別就沒有東西可查驗");
    assert!(!spawn_args_of(&harness, &second).contains(&"--resume".to_string()));
    assert_eq!(harness.process_control.spawn_count(), 1, "只有重新派工的 Run 啟動了程序");
}

// ============================================================ transcript

#[tokio::test]
async fn transcript_import_deduplicates_by_position_records_gaps_and_redacts_secrets() {
    let (mut harness, _) = start_with_accounts(1).await;
    let (ws, _, goal) = setup(&harness).await;
    let tid = ready_task(&harness, &ws, &goal, "整理資料").await;
    let rid = dispatch_ok(&harness, &ws, &tid).await;
    harness.supervise().await;
    let sid = session_of_run(&harness, &ws, &rid).await;
    let lines = [
        r#"{"type":"user","text":"開始整理"}"#.to_string(),
        format!(r#"{{"type":"assistant","text":"Agent 憑證是 {HEX_TOKEN}"}}"#),
        format!(r#"{{"type":"assistant","text":"金鑰 {API_KEY} 不應該出現"}}"#),
    ];
    harness.agent_adapter.emit(&rid, AdapterEvent::Started { native_session_id: Some("N1".into()) });
    for (n, line) in lines.iter().enumerate() {
        harness.agent_adapter.emit(&rid, transcript(n as u64 + 1, line));
    }

    harness.supervise().await;

    let first = segments(&harness, &ws, &sid).await;
    assert_eq!(ranges(&first), [(1, 3, "stored".to_string())], "{first:?}");
    assert_eq!(first[0]["run_id"], json!(rid));
    assert_eq!(first[0]["entry_count"], 3);
    assert!(first[0]["redactions"].as_u64().unwrap_or(0) > 0, "有遮蔽秘密：{}", first[0]);
    let content = String::from_utf8(segment_file(&harness, &first[0])).expect("transcript 是 UTF-8");
    assert!(!content.contains(HEX_TOKEN), "64 個十六進位字元的 token 被遮蔽：{content}");
    assert!(!content.contains(API_KEY) && !content.contains("sk-ant-"), "API 金鑰被遮蔽：{content}");
    assert!(content.contains("<已遮蔽>"), "{content}");
    assert!(content.contains("開始整理"), "一般內容保留：{content}");

    // 重建後串流重播 1–3 並送出 4：只新增 4–4
    harness.restart().await;
    harness.supervise().await;
    for (n, line) in lines.iter().enumerate() {
        harness.agent_adapter.emit(&rid, transcript(n as u64 + 1, line));
    }
    harness.agent_adapter.emit(&rid, transcript(4, r#"{"type":"assistant","text":"第四筆"}"#));
    harness.supervise().await;

    let second = segments(&harness, &ws, &sid).await;
    assert_eq!(ranges(&second), [(1, 3, "stored".to_string()), (4, 4, "stored".to_string())], "{second:?}");
    assert_eq!(second[0], first[0], "已匯入的 segment 不變");
    let fourth = String::from_utf8(segment_file(&harness, &second[1])).unwrap();
    assert!(fourth.contains("第四筆") && !fourth.contains("開始整理"), "重播的位置不再匯入：{fourth}");

    // 跳過 5 直接送 6：記錄缺口 5–5 與 6–6
    harness.agent_adapter.emit(&rid, transcript(6, r#"{"type":"assistant","text":"第六筆"}"#));
    harness.supervise().await;

    let third = segments(&harness, &ws, &sid).await;
    assert_eq!(
        ranges(&third),
        [(1, 3, "stored".to_string()), (4, 4, "stored".to_string()), (5, 5, "missing".to_string()), (6, 6, "stored".to_string())],
        "{third:?}"
    );
    assert!(third[2]["storage_path"].is_null(), "缺口沒有檔案：{}", third[2]);
    assert_eq!(third[2]["run_id"], json!(rid));
    assert!(String::from_utf8(segment_file(&harness, &third[3])).unwrap().contains("第六筆"));
    for pair in third.windows(2) {
        assert!(
            pair[0]["last_position"].as_u64().unwrap() < pair[1]["first_position"].as_u64().unwrap(),
            "segment 不重疊：{third:?}"
        );
    }
}
