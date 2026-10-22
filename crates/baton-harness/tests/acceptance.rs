//! 情境測試：成果接受與最小完成驗證（票 13），經由 Runtime harness 的真實 HTTP API 驗證。
//!
//! Worker 以 fake AgentAdapter 的結束事件與 Agent API（`register_artifact` 等，與 MCP shim 送出的請求相同）回報；
//! 確定性檢查中的命令由 fake CommandRunner 回應，檔案與成果檢查讀取暫存目錄中的真實檔案。
//! 使用者以桌面憑證檢視驗證狀況與手動接受。`harness.db()` 只用來確認事件這類必須看落地內容的事實。

use std::time::Duration;

use baton_domain::artifact::TOOL_REGISTER_ARTIFACT;
use baton_domain::agent::TOOL_REPORT_PROGRESS;
use baton_domain::auth::Role;
use baton_domain::run::LEASE_DURATION_MS;
use baton_domain::Timestamp;
use baton_harness::Harness;
use baton_ports::{AdapterEvent, Liveness};
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

const WORKSPACES: &str = "/api/v1/workspaces";
const CLOCK_START: Timestamp = Timestamp::from_millis(1_700_000_000_123);
/// SHA-256("abc")，獨立於程式碼之外的已知答案。
const SHA256_ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

// --- 共用輔助 ---

async fn harness() -> Harness {
    let harness = Harness::builder().clock_at(CLOCK_START).start().await;
    // 派工需要一個可派工的帳號（票 19）
    harness.add_account("主要帳號", "main@example.com").await;
    harness
}

/// 建立 Workspace（目錄在資料目錄下）與 Goal，回傳 (workspace_id, workspace 目錄, goal_id)。
async fn setup(harness: &Harness) -> (String, std::path::PathBuf, String) {
    let dir = harness.data_dir().join("projects").join("main");
    std::fs::create_dir_all(&dir).unwrap();
    let (status, ws) = harness.send_json(Method::POST, WORKSPACES, Some(json!({ "path": dir.to_string_lossy() }))).await;
    assert_eq!(status, StatusCode::CREATED, "{ws}");
    let ws = ws["id"].as_str().unwrap().to_string();
    let (status, goal) =
        harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/goals"), Some(json!({ "title": "目標" }))).await;
    assert_eq!(status, StatusCode::CREATED, "{goal}");
    (ws, dir, goal["id"].as_str().unwrap().to_string())
}

fn filled(text: &str) -> Value {
    json!({ "status": "filled", "text": text })
}

fn contract(checks: Value) -> Value {
    json!({
        "objective": filled("產出比較報告"),
        "inputs": filled("既有的資料"),
        "scope": filled("公開資料"),
        "deliverables": filled("report.md"),
        "done_criteria": filled("報告存在且測試通過"),
        "blocker_handling": filled("卡住時回報"),
        "required_capabilities": [],
        "verification_checks": checks,
    })
}

fn task_path(ws: &str, tid: &str) -> String {
    format!("{WORKSPACES}/{ws}/tasks/{tid}")
}

fn verification_path(ws: &str, tid: &str) -> String {
    format!("{}/verification", task_path(ws, tid))
}

fn accept_path(ws: &str, tid: &str) -> String {
    format!("{}/verification/accept", task_path(ws, tid))
}

/// 建立契約宣告了 `checks` 的 Task 並讓它 ready。
async fn ready_task(harness: &Harness, ws: &str, goal: &str, checks: Value) -> String {
    let body = json!({ "goal_id": goal, "title": "比較報告", "contract": contract(checks) });
    let (status, task) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks"), Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    let tid = task["id"].as_str().unwrap().to_string();
    let (status, ready) = harness.send_json(Method::POST, &format!("{}/ready", task_path(ws, &tid)), None).await;
    assert_eq!(status, StatusCode::OK, "{ready}");
    tid
}

/// 派工並讓 Supervisor 啟動它，回傳 (run_id, Worker 的 Agent 憑證)。
async fn start_run(harness: &Harness, ws: &str, tid: &str) -> (String, String) {
    let (status, run) = harness.send_json(Method::POST, &format!("{}/dispatch", task_path(ws, tid)), None).await;
    assert_eq!(status, StatusCode::CREATED, "{run}");
    let rid = run["id"].as_str().unwrap().to_string();
    harness.supervise().await;
    let token = harness.agent_credential(&rid).expect("Run 啟動時應該拿到 Agent 憑證");
    (rid, token)
}

/// Worker 程序正常結束（宣稱成功），讓 Supervisor 處理。
async fn worker_exits_successfully(harness: &Harness, rid: &str) {
    harness.agent_adapter.emit(rid, AdapterEvent::Exited { success: true });
    harness.supervise().await;
}

async fn task_status(harness: &Harness, ws: &str, tid: &str) -> String {
    let (status, detail) = harness.send_json(Method::GET, &task_path(ws, tid), None).await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    detail["task"]["status"].as_str().unwrap().to_string()
}

async fn verification(harness: &Harness, ws: &str, tid: &str) -> Value {
    let (status, view) = harness.send_json(Method::GET, &verification_path(ws, tid), None).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    view
}

async fn accept(harness: &Harness, ws: &str, tid: &str, basis: &Value) -> (StatusCode, Value) {
    harness.send_json(Method::POST, &accept_path(ws, tid), Some(json!({ "basis": basis, "note": "看過報告" }))).await
}

fn error_code(body: &Value) -> &str {
    body["error"].as_str().unwrap_or_default()
}

/// 這個 Task 被記錄的驗證事件內容（payload），依發生順序。
fn verification_events(harness: &Harness, tid: &str) -> Vec<Value> {
    harness
        .db()
        .prepare(
            "SELECT payload FROM event WHERE aggregate_type = 'task' AND aggregate_id = ?1
             AND event_type = 'task_verification_recorded' ORDER BY aggregate_version",
        )
        .unwrap()
        .query_map([tid], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|p| serde_json::from_str(&p.unwrap()).unwrap())
        .collect()
}

fn command_check(program: &str, args: &[&str]) -> Value {
    json!({ "kind": "command", "program": program, "args": args, "expected_exit_code": 0 })
}

// ============================================================ 宣稱成功不等於完成

#[tokio::test]
async fn a_worker_claiming_success_and_exiting_normally_does_not_complete_the_task() {
    let harness = harness().await;
    let (ws, _, goal) = setup(&harness).await;
    let tid = ready_task(&harness, &ws, &goal, json!([])).await;
    let (rid, token) = start_run(&harness, &ws, &tid).await;

    // Worker 以文字宣稱完成，然後程序正常退出
    let (status, _) = harness.call_tool(&token, TOOL_REPORT_PROGRESS, json!({ "summary": "全部完成了，可以結案" })).await;
    assert_eq!(status, StatusCode::OK);
    worker_exits_successfully(&harness, &rid).await;
    for _ in 0..3 {
        harness.clock.advance_millis(60_000);
        harness.supervise().await;
    }

    assert_eq!(task_status(&harness, &ws, &tid).await, "verifying");
    let view = verification(&harness, &ws, &tid).await;
    assert_eq!(view["automatic"], false, "沒有宣告確定性檢查：只能由使用者手動接受：{view}");
    assert_eq!(view["basis"]["run_id"], rid.as_str());
    assert_eq!(view["records"], json!([]));
    assert!(verification_events(&harness, &tid).is_empty());
    assert!(harness.command_runner.calls().is_empty());
}

// ============================================================ 確定性檢查

#[tokio::test]
async fn passing_deterministic_checks_complete_the_task_with_evidence() {
    let harness = harness().await;
    let (ws, dir, goal) = setup(&harness).await;
    let checks = json!([
        command_check("cargo", &["test", "--workspace"]),
        { "kind": "file", "path": "out\\summary.txt", "sha256": SHA256_ABC },
        { "kind": "artifact", "name": "report.md" },
    ]);
    let tid = ready_task(&harness, &ws, &goal, checks).await;
    let (rid, token) = start_run(&harness, &ws, &tid).await;
    // Worker 的提示中列出確定性檢查，讓它知道完成標準
    let prompt = harness.agent_adapter.launched()[0].prompt.clone();
    assert!(prompt.contains("確定性檢查") && prompt.contains("cargo test --workspace"), "{prompt}");

    // Worker 的實際成果：工作目錄裡的檔案與登記的成果
    std::fs::create_dir_all(dir.join("out")).unwrap();
    std::fs::write(dir.join("out").join("summary.txt"), "abc").unwrap();
    let (status, body) = harness.call_tool(&token, TOOL_REGISTER_ARTIFACT, json!({ "name": "report.md", "content": "abc" })).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    harness.command_runner.script_exit("cargo", 0, "test result: ok. 12 passed");

    worker_exits_successfully(&harness, &rid).await;
    harness.supervise().await;

    assert_eq!(task_status(&harness, &ws, &tid).await, "completed");
    let calls = harness.command_runner.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!((calls[0].program.as_str(), calls[0].args.clone()), ("cargo", vec!["test".to_string(), "--workspace".into()]));
    assert_eq!(calls[0].working_dir, std::path::PathBuf::from(dir.to_string_lossy().into_owned()));

    let view = verification(&harness, &ws, &tid).await;
    let record = &view["records"][0];
    assert_eq!((record["method"].as_str(), record["outcome"].as_str()), (Some("checks"), Some("passed")));
    assert_eq!((record["run_id"].as_str(), record["generation"].as_i64()), (Some(rid.as_str()), Some(1)));
    let statuses: Vec<&str> = record["checks"].as_array().unwrap().iter().map(|c| c["status"].as_str().unwrap()).collect();
    assert_eq!(statuses, ["passed", "passed", "passed"]);
    assert!(record["checks"][0]["detail"].as_str().unwrap().contains("12 passed"));
    assert_eq!(record["artifacts"][0]["name"], "report.md");
    assert_eq!(record["artifacts"][0]["sha256"], SHA256_ABC);
    assert_eq!(view["basis"], Value::Null, "已完成的 Task 沒有可接受的基礎");

    let events = verification_events(&harness, &tid);
    assert_eq!(events.len(), 1);
    assert_eq!((events[0]["method"].as_str(), events[0]["outcome"].as_str()), (Some("checks"), Some("passed")));
}

#[tokio::test]
async fn a_failing_check_returns_the_task_to_a_fixable_state_and_keeps_the_evidence() {
    let harness = harness().await;
    let (ws, _, goal) = setup(&harness).await;
    let checks = json!([command_check("cargo", &["test"]), { "kind": "artifact", "name": "report.md" }]);
    let tid = ready_task(&harness, &ws, &goal, checks).await;
    let (first, token) = start_run(&harness, &ws, &tid).await;
    harness.call_tool(&token, TOOL_REGISTER_ARTIFACT, json!({ "name": "report.md", "content": "abc" })).await;
    harness.command_runner.script_exit("cargo", 101, "test parser ... FAILED\ntest result: FAILED. 1 failed");

    worker_exits_successfully(&harness, &first).await;
    harness.supervise().await;

    assert_eq!(task_status(&harness, &ws, &tid).await, "ready", "檢查未通過：Task 回到可修正、可再派工的狀態");
    let view = verification(&harness, &ws, &tid).await;
    let failed = &view["records"][0];
    assert_eq!(failed["outcome"], "failed");
    assert_eq!(failed["checks"][0]["status"], "failed");
    let detail = failed["checks"][0]["detail"].as_str().unwrap();
    assert!(detail.contains("101") && detail.contains("1 failed"), "證據要有結束碼與輸出：{detail}");
    assert_eq!(failed["checks"][1]["status"], "passed");
    let (_, board) = harness.send_json(Method::GET, &format!("{WORKSPACES}/{ws}/tasks"), None).await;
    assert_eq!(board["verifications"][0]["id"], failed["id"], "工作面板也看得到證據");

    // 修正後重新派工（世代 2），這次通過：兩次的證據都保留
    let (second, _) = start_run(&harness, &ws, &tid).await;
    harness.command_runner.script_exit("cargo", 0, "ok");
    worker_exits_successfully(&harness, &second).await;
    harness.supervise().await;

    assert_eq!(task_status(&harness, &ws, &tid).await, "completed");
    let records = verification(&harness, &ws, &tid).await["records"].as_array().unwrap().clone();
    let summary: Vec<(i64, &str)> =
        records.iter().map(|r| (r["generation"].as_i64().unwrap(), r["outcome"].as_str().unwrap())).collect();
    assert_eq!(summary, [(1, "failed"), (2, "passed")]);
}

#[tokio::test]
async fn an_artifact_whose_stored_file_no_longer_matches_its_manifest_fails_the_check() {
    let harness = harness().await;
    let (ws, _, goal) = setup(&harness).await;
    let tid = ready_task(&harness, &ws, &goal, json!([{ "kind": "artifact", "name": "report.md" }])).await;
    let (rid, token) = start_run(&harness, &ws, &tid).await;
    let (_, registered) = harness.call_tool(&token, TOOL_REGISTER_ARTIFACT, json!({ "name": "report.md", "content": "abc" })).await;
    // 登記之後檔案在成果儲存中被竄改
    let listing = harness.send_json(Method::GET, &format!("{WORKSPACES}/{ws}/artifacts"), None).await.1;
    let storage_path = listing["artifacts"][0]["versions"][0]["storage_path"].as_str().unwrap().to_string();
    std::fs::write(harness.artifacts_dir().join(storage_path), "abd").unwrap();
    assert_eq!(registered["sha256"], SHA256_ABC);

    worker_exits_successfully(&harness, &rid).await;
    harness.supervise().await;

    assert_eq!(task_status(&harness, &ws, &tid).await, "ready");
    let record = &verification(&harness, &ws, &tid).await["records"][0];
    assert!(record["checks"][0]["detail"].as_str().unwrap().contains("manifest 不符"), "{record}");
}

#[tokio::test]
async fn a_check_command_that_cannot_start_or_a_missing_file_fails_verification() {
    let harness = harness().await;
    let (ws, _, goal) = setup(&harness).await;
    let checks = json!([command_check("no-such-tool", &[]), { "kind": "file", "path": "missing.txt" }]);
    let tid = ready_task(&harness, &ws, &goal, checks).await;
    let (rid, _) = start_run(&harness, &ws, &tid).await;

    worker_exits_successfully(&harness, &rid).await;
    harness.supervise().await;

    assert_eq!(task_status(&harness, &ws, &tid).await, "ready");
    let record = &verification(&harness, &ws, &tid).await["records"][0];
    assert!(record["checks"][0]["detail"].as_str().unwrap().contains("無法執行"), "{record}");
    assert!(record["checks"][1]["detail"].as_str().unwrap().contains("不存在"), "{record}");
}

#[tokio::test]
async fn a_contract_declaring_checks_outside_the_working_directory_is_rejected() {
    let harness = harness().await;
    let (ws, _, goal) = setup(&harness).await;
    let body = json!({ "goal_id": goal, "title": "x", "contract": contract(json!([{ "kind": "file", "path": "..\\..\\secret" }])) });

    let (status, response) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks"), Some(body)).await;

    assert_eq!((status, error_code(&response)), (StatusCode::BAD_REQUEST, "contract_invalid"), "{response}");
}

#[tokio::test]
async fn a_runtime_crash_while_checks_run_reruns_them_after_restart_instead_of_guessing() {
    let mut harness = harness().await;
    let (ws, _, goal) = setup(&harness).await;
    let tid = ready_task(&harness, &ws, &goal, json!([command_check("cargo", &["test"])])).await;
    let (rid, _) = start_run(&harness, &ws, &tid).await;
    harness.command_runner.script_exit("cargo", 0, "ok");
    harness.agent_adapter.emit(&rid, AdapterEvent::Exited { success: true });

    // Run 結束、Task 進入 verifying 之後，檢查執行到一半 Runtime 當掉（這一次處理被丟棄）
    let _gate = harness.command_runner.hold_next();
    let crashed = tokio::time::timeout(Duration::from_millis(300), harness.supervise()).await;
    assert!(crashed.is_err(), "預期 Supervisor 卡在檢查中");
    harness.restart().await;
    assert_eq!(task_status(&harness, &ws, &tid).await, "verifying", "沒有結論前不會被當成通過或失敗");

    harness.supervise().await;

    assert_eq!(task_status(&harness, &ws, &tid).await, "completed");
    assert_eq!(harness.command_runner.calls().len(), 2, "重啟後重新執行檢查");
    assert_eq!(verification(&harness, &ws, &tid).await["records"].as_array().unwrap().len(), 1);
}

// ============================================================ 過期 Run

#[tokio::test]
async fn reports_from_a_superseded_run_cannot_advance_the_task() {
    let harness = harness().await;
    let (ws, _, goal) = setup(&harness).await;
    let tid = ready_task(&harness, &ws, &goal, json!([])).await;
    let (stale, stale_token) = start_run(&harness, &ws, &tid).await;
    let spawned = harness.process_control.spawned();
    let identity = harness.process_control.identity_of(&spawned[0].command_id).unwrap();

    // 世代 1 的 lease 過期且程序被確認死亡：Run 中斷，Task 回到 ready，再派出世代 2
    harness.clock.advance_millis(LEASE_DURATION_MS + 1);
    harness.process_control.script_liveness(&identity, Liveness::Dead);
    harness.supervise().await;
    assert_eq!(task_status(&harness, &ws, &tid).await, "ready");
    let (current, current_token) = start_run(&harness, &ws, &tid).await;

    // 其實還活著的世代 1 繼續回報成果與「成功結束」：全部被拒絕，Task 不動
    let (status, body) = harness.call_tool(&stale_token, TOOL_REGISTER_ARTIFACT, json!({ "name": "report.md", "content": "舊的" })).await;
    assert_eq!((status, error_code(&body)), (StatusCode::CONFLICT, "run_report_stale"), "{body}");
    worker_exits_successfully(&harness, &stale).await;
    assert_eq!(task_status(&harness, &ws, &tid).await, "running", "過期 Run 的結束不能把 Task 推進到驗證");
    let (_, listing) = harness.send_json(Method::GET, &format!("{WORKSPACES}/{ws}/artifacts"), None).await;
    assert_eq!(listing["artifacts"], json!([]), "過期 Run 的成果沒有進入 manifest");

    // 世代 2 登記成果並結束；以世代 1 為依據的接受被拒絕
    harness.call_tool(&current_token, TOOL_REGISTER_ARTIFACT, json!({ "name": "report.md", "content": "abc" })).await;
    worker_exits_successfully(&harness, &current).await;
    let basis = verification(&harness, &ws, &tid).await["basis"].clone();
    assert_eq!((basis["run_id"].as_str(), basis["generation"].as_i64()), (Some(current.as_str()), Some(2)));
    let stale_basis = json!({ "run_id": stale, "generation": 1, "contract_version": basis["contract_version"], "artifacts": basis["artifacts"] });

    let (status, body) = accept(&harness, &ws, &tid, &stale_basis).await;

    assert_eq!((status, error_code(&body)), (StatusCode::CONFLICT, "acceptance_run_stale"), "{body}");
    assert_eq!(task_status(&harness, &ws, &tid).await, "verifying");
    let (status, _) = accept(&harness, &ws, &tid, &basis).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(task_status(&harness, &ws, &tid).await, "completed");
}

// ============================================================ 手動接受

#[tokio::test]
async fn manual_acceptance_needs_the_desktop_credential_and_is_recorded_in_an_event() {
    let harness = harness().await;
    let (ws, _, goal) = setup(&harness).await;
    let tid = ready_task(&harness, &ws, &goal, json!([])).await;
    let (rid, token) = start_run(&harness, &ws, &tid).await;
    harness.call_tool(&token, TOOL_REGISTER_ARTIFACT, json!({ "name": "report.md", "content": "abc" })).await;
    worker_exits_successfully(&harness, &rid).await;
    let basis = verification(&harness, &ws, &tid).await["basis"].clone();
    let body = json!({ "basis": basis });

    // Worker 自己的 Agent 憑證、另一個 Agent 角色憑證、沒有憑證：全部被拒絕，Task 不動
    for bearer in [Some(token.clone()), Some(harness.issue_credential(Role::Agent, None).await), None] {
        let mut request = harness
            .request(Method::POST, &accept_path(&ws, &tid))
            .header(baton_api::CLIENT_HEADER, baton_api::CLIENT_HEADER_VALUE)
            .json(&body);
        if let Some(bearer) = &bearer {
            request = request.bearer_auth(bearer);
        }
        let status = request.send().await.unwrap().status();
        assert!(matches!(status, StatusCode::FORBIDDEN | StatusCode::UNAUTHORIZED), "{status}");
    }
    assert_eq!(task_status(&harness, &ws, &tid).await, "verifying");

    let (status, view) = accept(&harness, &ws, &tid, &basis).await;

    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(view["status"], "completed");
    let record = &view["records"][0];
    assert_eq!((record["method"].as_str(), record["outcome"].as_str()), (Some("manual"), Some("passed")));
    assert_eq!(record["note"], "看過報告");
    assert_eq!(record["artifacts"][0]["sha256"], SHA256_ABC);
    let accepted_by = record["accepted_by"].as_str().unwrap().to_string();
    assert!(!accepted_by.is_empty());
    assert!(!harness.desktop_credential().contains(&accepted_by), "只記錄憑證指紋，不記錄憑證本身");

    let events = verification_events(&harness, &tid);
    assert_eq!(events.len(), 1);
    assert_eq!((events[0]["method"].as_str(), events[0]["accepted_by"].as_str()), (Some("manual"), Some(accepted_by.as_str())));
    assert_eq!(events[0]["run_id"], rid.as_str());

    // 已完成的 Task 不能再被接受一次
    let (status, again) = accept(&harness, &ws, &tid, &basis).await;
    assert_eq!((status, error_code(&again)), (StatusCode::CONFLICT, "task_not_verifying"));
}

#[tokio::test]
async fn acceptance_is_rejected_when_the_contract_or_manifest_differs_from_what_was_viewed() {
    let harness = harness().await;
    let (ws, _, goal) = setup(&harness).await;
    let tid = ready_task(&harness, &ws, &goal, json!([])).await;
    let (rid, token) = start_run(&harness, &ws, &tid).await;
    harness.call_tool(&token, TOOL_REGISTER_ARTIFACT, json!({ "name": "report.md", "content": "abc" })).await;
    worker_exits_successfully(&harness, &rid).await;
    let viewed = verification(&harness, &ws, &tid).await["basis"].clone();

    let mut wrong_manifest = viewed.clone();
    wrong_manifest["artifacts"] = json!([]);
    let (status, body) = accept(&harness, &ws, &tid, &wrong_manifest).await;
    assert_eq!((status, error_code(&body)), (StatusCode::CONFLICT, "acceptance_manifest_stale"), "{body}");

    // 檢視之後契約被改版：依舊版本的接受被拒絕，依新版本的也不行（Run 是依舊契約執行的）
    let mut changed = contract(json!([]));
    changed["scope"] = filled("也看付費資料");
    let (status, _) =
        harness.send_json(Method::PUT, &format!("{}/contract", task_path(&ws, &tid)), Some(json!({ "contract": changed }))).await;
    assert_eq!(status, StatusCode::CREATED, "契約內容改變會新增版本");
    let (status, body) = accept(&harness, &ws, &tid, &viewed).await;
    assert_eq!((status, error_code(&body)), (StatusCode::CONFLICT, "acceptance_contract_stale"), "{body}");
    let fresh = verification(&harness, &ws, &tid).await["basis"].clone();
    let (status, body) = accept(&harness, &ws, &tid, &fresh).await;
    assert_eq!((status, error_code(&body)), (StatusCode::CONFLICT, "acceptance_contract_stale"), "{body}");
    assert_eq!(task_status(&harness, &ws, &tid).await, "verifying");

    // Runtime 判定這個 Run 的成果不能在新契約下被接受：Task 回到 ready、保留原因
    harness.supervise().await;
    assert_eq!(task_status(&harness, &ws, &tid).await, "ready");
    let record = &verification(&harness, &ws, &tid).await["records"][0];
    assert_eq!(record["outcome"], "failed");
    assert!(record["note"].as_str().unwrap().contains("契約"), "{record}");
}

#[tokio::test]
async fn when_checks_and_a_manual_acceptance_race_only_one_acceptance_is_recorded() {
    let harness = harness().await;
    let (ws, _, goal) = setup(&harness).await;
    let tid = ready_task(&harness, &ws, &goal, json!([command_check("cargo", &["test"])])).await;
    let (rid, _) = start_run(&harness, &ws, &tid).await;
    harness.command_runner.script_exit("cargo", 1, "失敗");
    harness.agent_adapter.emit(&rid, AdapterEvent::Exited { success: true });

    // Supervisor 收到結束事件、Task 進入 verifying 後開始執行檢查；檢查進行中，使用者手動接受
    let gate = harness.command_runner.hold_next();
    let checking = harness.supervise();
    let accepting = async {
        while harness.command_runner.calls().is_empty() {
            tokio::task::yield_now().await;
        }
        let basis = verification(&harness, &ws, &tid).await["basis"].clone();
        let outcome = accept(&harness, &ws, &tid, &basis).await;
        gate.notify_one();
        outcome
    };
    let (_, (status, body)) = tokio::join!(checking, accepting);

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(task_status(&harness, &ws, &tid).await, "completed", "較晚提交的檢查結果已過期，不能把 Task 拉回 ready");
    let records = verification(&harness, &ws, &tid).await["records"].as_array().unwrap().clone();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["method"], "manual");
}
