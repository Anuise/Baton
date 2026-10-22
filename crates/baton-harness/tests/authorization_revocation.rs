//! 情境測試：授權撤銷與啟動准入順序（票 24，ADR-0003 的 R1-a），經由 Runtime harness 的真實 HTTP API 驗證。
//!
//! 准入 = Supervisor 持久化 launch intent 的那一個交易（`BeginLaunch`）。授權更新與准入都經過 Store 的單一寫入佇列，
//! 所以兩者的先後就是持久化順序：先准入的 Run 沿用舊授權與設定版本；排隊中、已有派工 Outbox 但尚未准入的 Run、
//! 恢復 Run 與新的工作一律用新版。全部使用 fake Port；`harness.db()` 只用來讀事件表核對持久化順序。

use baton_harness::Harness;
use baton_ports::AdapterEvent;
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

const WORKSPACES: &str = "/api/v1/workspaces";
const SCHEDULING: &str = "/api/v1/scheduling";
const TOOL_PERMISSION_PROMPT: &str = "permission_prompt";

// --- 共用輔助 ---

async fn harness() -> Harness {
    let harness = Harness::start().await;
    harness.add_account("帳號甲", "a@example.com").await;
    harness.add_account("帳號乙", "b@example.com").await;
    harness
}

fn plain_dir(harness: &Harness, name: &str) -> String {
    let dir = harness.data_dir().join("projects").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    dir.to_string_lossy().into_owned()
}

/// 建立 Workspace 與 Goal，回傳 (workspace_id, goal_id)。
async fn workspace(harness: &Harness, name: &str) -> (String, String) {
    let path = plain_dir(harness, name);
    let (status, ws) = harness.send_json(Method::POST, WORKSPACES, Some(json!({ "path": path }))).await;
    assert_eq!(status, StatusCode::CREATED, "{ws}");
    let ws = ws["id"].as_str().unwrap().to_string();
    let (status, goal) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/goals"), Some(json!({ "title": "目標" }))).await;
    assert_eq!(status, StatusCode::CREATED, "{goal}");
    (ws, goal["id"].as_str().unwrap().to_string())
}

fn authorization_path(ws: &str) -> String {
    format!("{WORKSPACES}/{ws}/authorization")
}

async fn authorization(harness: &Harness, ws: &str) -> Value {
    let (status, view) = harness.send_json(Method::GET, &authorization_path(ws), None).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    view
}

/// 以範本確認（或修改）授權，回傳新的 revision 編號。擴大既有授權時一併明確確認（AI-183）。
async fn confirm_template(harness: &Harness, ws: &str, template_id: &str) -> i64 {
    let view = authorization(harness, ws).await;
    let template = view["templates"].as_array().unwrap().iter().find(|t| t["id"] == template_id).unwrap().clone();
    let body = json!({
        "template_id": template_id,
        "grants": template["grants"],
        "acknowledged_risk": true,
        "acknowledged_expansion": true,
    });
    let (status, revision) = harness.send_json(Method::PUT, &authorization_path(ws), Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{revision}");
    revision["revision"].as_i64().unwrap()
}

/// 撤銷「建置與測試」與「安裝專案依賴」：改成「研究／文件」範本。
async fn revoke_build(harness: &Harness, ws: &str) -> i64 {
    confirm_template(harness, ws, "research_docs").await
}

fn filled(text: &str) -> Value {
    json!({ "status": "filled", "text": text })
}

/// 建立一個 ready 的 Task（契約需要 `capabilities`），回傳識別。
async fn ready_task(harness: &Harness, ws: &str, goal: &str, title: &str, capabilities: &[&str]) -> String {
    let contract = json!({
        "objective": filled(title), "inputs": filled("既有的程式"), "scope": filled("這個 repo"),
        "deliverables": filled("修正"), "done_criteria": filled("測試通過"), "blocker_handling": filled("卡住時回報"),
        "required_capabilities": capabilities,
    });
    let body = json!({ "goal_id": goal, "title": title, "contract": contract });
    let (status, task) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks"), Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    let tid = task["id"].as_str().unwrap().to_string();
    let (status, ready) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks/{tid}/ready"), None).await;
    assert_eq!(status, StatusCode::OK, "{ready}");
    tid
}

/// 派工（建立排隊中的 Run 與派工 Outbox，尚未准入），回傳 Run 識別。
async fn dispatch(harness: &Harness, ws: &str, tid: &str) -> String {
    let (status, run) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks/{tid}/dispatch"), None).await;
    assert_eq!(status, StatusCode::CREATED, "{run}");
    assert_eq!(run["status"], "queued");
    assert_eq!(run["admitted_at"], Value::Null, "派工只建立排隊中的 Run，還沒有准入");
    run["id"].as_str().unwrap().to_string()
}

/// 派工並讓 Supervisor 准入、啟動，回傳 (Run 識別, Agent 憑證)。
async fn start_run(harness: &Harness, ws: &str, tid: &str) -> (String, String) {
    let rid = dispatch(harness, ws, tid).await;
    harness.supervise().await;
    let token = harness.agent_credential(&rid).expect("准入的 Run 啟動時帶著 Agent 憑證");
    (rid, token)
}

async fn run_view(harness: &Harness, ws: &str, rid: &str) -> Value {
    let (status, run) = harness.send_json(Method::GET, &format!("{WORKSPACES}/{ws}/runs/{rid}"), None).await;
    assert_eq!(status, StatusCode::OK, "{run}");
    run
}

async fn runs_of(harness: &Harness, ws: &str, tid: &str) -> Vec<Value> {
    let (status, list) = harness.send_json(Method::GET, &format!("{WORKSPACES}/{ws}/runs"), None).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    list["runs"].as_array().unwrap().iter().filter(|r| r["task_id"] == tid).cloned().collect()
}

async fn task_status(harness: &Harness, ws: &str, tid: &str) -> String {
    let (status, detail) = harness.send_json(Method::GET, &format!("{WORKSPACES}/{ws}/tasks/{tid}"), None).await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    detail["task"]["status"].as_str().unwrap().to_string()
}

async fn prompt(harness: &Harness, token: &str, command: &str) -> (StatusCode, Value) {
    let body = json!({ "tool_name": "Bash", "input": { "command": command } });
    harness.call_tool(token, TOOL_PERMISSION_PROMPT, body).await
}

/// 事件表中的順序（`seq` 是單一寫入佇列提交的順序）。
fn seq_of(harness: &Harness, aggregate_type: &str, aggregate_id: &str, event_type: &str) -> Option<i64> {
    harness
        .db()
        .query_row(
            "SELECT seq FROM event WHERE aggregate_type = ?1 AND aggregate_id = ?2 AND event_type = ?3",
            rusqlite::params![aggregate_type, aggregate_id, event_type],
            |r| r.get(0),
        )
        .ok()
}

fn revision_seq(harness: &Harness, ws: &str, revision: i64) -> i64 {
    harness
        .db()
        .query_row(
            "SELECT seq FROM event WHERE aggregate_type = 'workspace_authorization' AND aggregate_id = ?1
             AND aggregate_version = ?2",
            rusqlite::params![ws, revision],
            |r| r.get(0),
        )
        .unwrap()
}

// ============================================================ 未准入的 Run 用新版重新檢查

#[tokio::test]
async fn a_run_already_queued_with_an_outbox_item_is_rechecked_at_admission_and_cannot_take_the_revoked_authorization() {
    let harness = harness().await;
    let (ws, goal) = workspace(&harness, "app").await;
    confirm_template(&harness, &ws, "trusted_dev_p3").await;
    let tid = ready_task(&harness, &ws, &goal, "跑建置", &["run_build_test"]).await;
    let rid = dispatch(&harness, &ws, &tid).await;
    assert_eq!(run_view(&harness, &ws, &rid).await["authorization_revision"], 1, "派工時記下的只是當時的版本");

    // 派工之後、准入之前撤權
    let revoked = revoke_build(&harness, &ws).await;
    harness.supervise().await;

    // 沒有准入：不啟動程序、不簽發憑證，Run 直接結束並說明原因
    assert_eq!(harness.process_control.spawn_count(), 0, "未准入的 Run 不能拿舊授權啟動");
    assert!(harness.agent_credential(&rid).is_none());
    let run = run_view(&harness, &ws, &rid).await;
    assert_eq!(run["status"], "interrupted", "{run}");
    assert_eq!(run["admitted_at"], Value::Null);
    assert!(run["end_reason"].as_str().unwrap().starts_with("admission_denied"), "{run}");
    assert!(run["end_reason"].as_str().unwrap().contains("建置與測試"), "{run}");
    assert!(seq_of(&harness, "run", &rid, "run_admission_denied").unwrap() > revision_seq(&harness, &ws, revoked));
    assert_eq!(seq_of(&harness, "run", &rid, "run_launch_started"), None);

    // Task 沒有被放棄：回到 ready 並重新排隊，等待原因是授權
    assert_eq!(task_status(&harness, &ws, &tid).await, "ready");
    let (_, overview) = harness.send_json(Method::GET, SCHEDULING, None).await;
    let entry = overview["queue"].as_array().unwrap().iter().find(|q| q["task_id"] == tid).cloned().unwrap();
    assert_eq!(entry["reasons"][0]["category"], "authorization", "{entry}");
    assert_eq!(entry["reasons"][0]["code"], "capability_not_granted", "{entry}");

    // 使用者重新授權後，排隊的 Task 依新版派出並准入
    let restored = confirm_template(&harness, &ws, "trusted_dev_p3").await;
    harness.supervise().await;
    let runs = runs_of(&harness, &ws, &tid).await;
    assert_eq!(runs.len(), 2, "{runs:?}");
    assert_eq!(runs[1]["status"], "running");
    assert_eq!(runs[1]["authorization_revision"], restored);
    assert_eq!(harness.process_control.spawn_count(), 1);
}

#[tokio::test]
async fn racing_authorization_updates_and_launch_admission_leave_a_verifiable_persisted_order() {
    // 每一輪：Run 已排隊並有派工 Outbox，同時送出撤權與 Supervisor 的准入。不論誰先進寫入佇列，
    // 事件表的順序都要與 Run 實際拿到的授權一致：先准入者沿用舊版，晚於撤權者一律被拒
    let mut admitted_first = 0;
    let mut revoked_first = 0;
    for round in 0..6 {
        let harness = harness().await;
        let (ws, goal) = workspace(&harness, &format!("race{round}")).await;
        let before = confirm_template(&harness, &ws, "trusted_dev_p3").await;
        let tid = ready_task(&harness, &ws, &goal, "跑建置", &["run_build_test"]).await;
        let rid = dispatch(&harness, &ws, &tid).await;

        let (revoked, _) = tokio::join!(revoke_build(&harness, &ws), harness.supervise());

        let run = run_view(&harness, &ws, &rid).await;
        let revoked_at = revision_seq(&harness, &ws, revoked);
        match seq_of(&harness, "run", &rid, "run_launch_started") {
            Some(admitted_at) => {
                admitted_first += 1;
                assert!(admitted_at < revoked_at, "准入晚於撤權卻啟動了：{run}");
                assert_eq!(run["authorization_revision"], before, "先准入的 Run 沿用撤權前的版本");
                assert_eq!(harness.process_control.spawn_count(), 1);
            }
            None => {
                revoked_first += 1;
                let denied_at = seq_of(&harness, "run", &rid, "run_admission_denied").expect("沒有准入就必須被拒");
                assert!(denied_at > revoked_at, "{run}");
                assert_eq!(run["status"], "interrupted");
                assert_eq!(harness.process_control.spawn_count(), 0);
            }
        }
    }
    assert_eq!(admitted_first + revoked_first, 6);
}

// ============================================================ 已准入的 Run 沿用舊版本並在 UI 標示

#[tokio::test]
async fn a_run_admitted_before_revocation_keeps_its_authorization_and_is_listed_with_pause_and_cancel_entries() {
    let harness = harness().await;
    let (ws, goal) = workspace(&harness, "app").await;
    confirm_template(&harness, &ws, "trusted_dev_p3").await;
    let tid = ready_task(&harness, &ws, &goal, "跑建置", &["run_build_test"]).await;
    let (rid, token) = start_run(&harness, &ws, &tid).await;
    let admitted = run_view(&harness, &ws, &rid).await;
    assert_eq!(admitted["authorization_revision"], 1);
    assert!(admitted["admitted_at"].is_i64(), "{admitted}");

    let revoked = revoke_build(&harness, &ws).await;
    harness.supervise().await;

    // 撤權不會停止或改變已准入的 Run；它的權限提示仍依准入時的 revision 判定
    let run = run_view(&harness, &ws, &rid).await;
    assert_eq!((run["status"].as_str(), run["authorization_revision"].clone()), (Some("running"), json!(1)));
    assert!(harness.process_control.stop_calls().is_empty(), "撤權不是強制停止");
    let (status, allowed) = prompt(&harness, &token, "cargo test --workspace").await;
    assert_eq!(status, StatusCode::OK, "{allowed}");
    assert_eq!((allowed["status"].as_str(), allowed["authorization_revision"].clone()), (Some("auto_allowed"), json!(1)));

    // 授權畫面：後續執行已停用，仍有 1 個 Run 沿用舊授權
    let view = authorization(&harness, &ws).await;
    assert_eq!(view["effective"]["revision"], revoked);
    let legacy = view["legacy_runs"].as_array().unwrap();
    assert_eq!(legacy.len(), 1, "{view}");
    assert_eq!(legacy[0]["run_id"], json!(rid));
    assert_eq!(legacy[0]["task_title"], "跑建置");
    assert_eq!(legacy[0]["authorization_revision"], 1);
    assert_eq!(legacy[0]["revoked_capabilities"], json!(["run_build_test", "install_project_dependencies"]));
    assert_eq!(legacy[0]["cancel_requested"], false);

    // 工作面板：Run 帶著它沿用的 revision 與准入時間，面板帶著目前的 revision，畫面據此標示
    let (_, board) = harness.send_json(Method::GET, &format!("{WORKSPACES}/{ws}/tasks"), None).await;
    assert_eq!(board["authorization_revision"], revoked);
    let on_board = board["runs"].as_array().unwrap().iter().find(|r| r["id"] == rid).unwrap();
    assert_eq!((on_board["authorization_revision"].clone(), on_board["admitted_at"].is_i64()), (json!(1), true));

    // 暫停入口（停止這個 Run）：要求後標示等待確認，程序確認停止後不再列出，Task 保留在 ready
    let (status, _) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/runs/{rid}/cancel"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(authorization(&harness, &ws).await["legacy_runs"][0]["cancel_requested"], true);
    harness.supervise().await;
    assert_eq!(run_view(&harness, &ws, &rid).await["status"], "cancelled");
    assert!(authorization(&harness, &ws).await["legacy_runs"].as_array().unwrap().is_empty());
    assert_eq!(task_status(&harness, &ws, &tid).await, "ready");
}

#[tokio::test]
async fn the_cancel_entry_cancels_the_task_of_a_run_on_the_old_authorization() {
    let harness = harness().await;
    let (ws, goal) = workspace(&harness, "app").await;
    confirm_template(&harness, &ws, "trusted_dev_p3").await;
    let tid = ready_task(&harness, &ws, &goal, "跑建置", &["run_build_test"]).await;
    let (rid, _) = start_run(&harness, &ws, &tid).await;
    revoke_build(&harness, &ws).await;

    let (status, _) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks/{tid}/cancel"), None).await;
    assert_eq!(status, StatusCode::OK);
    harness.supervise().await;

    assert_eq!(task_status(&harness, &ws, &tid).await, "cancelled");
    assert_eq!(run_view(&harness, &ws, &rid).await["status"], "cancelled");
    assert!(authorization(&harness, &ws).await["legacy_runs"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn an_expansion_alone_does_not_list_runs_as_keeping_revoked_authorization() {
    let harness = harness().await;
    let (ws, goal) = workspace(&harness, "app").await;
    confirm_template(&harness, &ws, "research_docs").await;
    let tid = ready_task(&harness, &ws, &goal, "整理文件", &["read_files"]).await;
    let (rid, _) = start_run(&harness, &ws, &tid).await;

    // 在目前的授權上只新增「建置與測試」
    let mut grants = authorization(&harness, &ws).await["effective"]["grants"].as_array().unwrap().clone();
    let target = grants[0]["targets"][0].clone();
    grants.push(json!({ "capability": "run_build_test", "targets": [target], "restrictions": ["僅執行專案內建置與測試指令"] }));
    let body = json!({ "grants": grants, "acknowledged_risk": true, "acknowledged_expansion": true });
    let (status, revision) = harness.send_json(Method::PUT, &authorization_path(&ws), Some(body)).await;
    assert_eq!((status, revision["revision"].clone()), (StatusCode::CREATED, json!(2)), "{revision}");

    assert!(authorization(&harness, &ws).await["legacy_runs"].as_array().unwrap().is_empty());
    assert_eq!(run_view(&harness, &ws, &rid).await["authorization_revision"], 1, "已准入的 Run 也不會取得新增的範圍");
}

// ============================================================ 舊 Run／Session 不能成為新工作的權限捷徑

#[tokio::test]
async fn an_old_run_is_not_a_permission_shortcut_for_new_work_recovery_runs_or_other_tasks() {
    let harness = harness().await;
    let (ws, goal) = workspace(&harness, "app").await;
    confirm_template(&harness, &ws, "trusted_dev_p3").await;
    let old_task = ready_task(&harness, &ws, &goal, "舊的工作", &[]).await;
    let (old_run, old_token) = start_run(&harness, &ws, &old_task).await;
    revoke_build(&harness, &ws).await;

    // 舊 Run 沿用撤權前的授權
    let (_, allowed) = prompt(&harness, &old_token, "cargo test").await;
    assert_eq!(allowed["status"], "auto_allowed", "{allowed}");

    // 新工作：撤權之後准入的 Run 依新版判定，同一個操作要送裁定
    let new_task = ready_task(&harness, &ws, &goal, "新的工作", &[]).await;
    let (new_run, new_token) = start_run(&harness, &ws, &new_task).await;
    assert_eq!(run_view(&harness, &ws, &new_run).await["authorization_revision"], 2);
    let (_, asked) = prompt(&harness, &new_token, "cargo test").await;
    assert_eq!((asked["behavior"].as_str(), asked["status"].as_str()), (Some("deny"), Some("waiting_decision")), "{asked}");

    // 恢復 Run：舊 Run 結束後同一個 Task 再派工，新的 Run 依新版准入，不繼承舊 Run 的授權
    harness.agent_adapter.emit(&old_run, AdapterEvent::Exited { success: false });
    harness.supervise().await;
    assert_eq!(task_status(&harness, &ws, &old_task).await, "ready");
    let (recovery, recovery_token) = start_run(&harness, &ws, &old_task).await;
    assert_eq!(run_view(&harness, &ws, &recovery).await["authorization_revision"], 2);
    let (_, asked) = prompt(&harness, &recovery_token, "cargo test").await;
    assert_eq!(asked["status"], "waiting_decision", "{asked}");

    // 舊 Run 的憑證在它結束後不能再拿來做任何權限判定
    let (status, stale) = prompt(&harness, &old_token, "cargo test").await;
    assert_ne!(status, StatusCode::OK, "{stale}");
    assert_eq!(stale["error"], "run_report_stale", "{stale}");
}

// ============================================================ 設定版本也依准入順序固定

#[tokio::test]
async fn an_admitted_run_keeps_its_settings_version_while_runs_admitted_later_use_the_newer_one() {
    let harness = harness().await;
    harness.user_settings.write_settings(r#"{"model":"opus"}"#);
    adopt_settings(&harness).await;
    let (ws, goal) = workspace(&harness, "app").await;
    let first = ready_task(&harness, &ws, &goal, "甲", &[]).await;
    let (admitted, _) = start_run(&harness, &ws, &first).await;
    let second = ready_task(&harness, &ws, &goal, "乙", &[]).await;
    let queued = dispatch(&harness, &ws, &second).await;

    // 已准入的甲沿用版本 1；派工後才准入的乙用版本 2
    harness.user_settings.write_settings(r#"{"model":"sonnet"}"#);
    adopt_settings(&harness).await;
    harness.supervise().await;

    assert_eq!(run_view(&harness, &ws, &admitted).await["settings_version"], 1);
    assert_eq!(run_view(&harness, &ws, &queued).await["settings_version"], 2);
    // Supervisor 啟動時已用准入時固定的版本產生快照（票 14），之後只回傳同一個綁定
    let (status, pinned) = snapshot(&harness, &ws, &admitted, json!({})).await;
    assert_eq!((status, pinned["version"].clone()), (StatusCode::OK, json!(1)), "{pinned}");
    let (status, refused) = snapshot(&harness, &ws, &queued, json!({ "version": 1 })).await;
    assert_eq!((status, refused["error"].clone()), (StatusCode::CONFLICT, json!("settings_snapshot_pinned")), "{refused}");
    let (status, newer) = snapshot(&harness, &ws, &queued, json!({})).await;
    assert_eq!((status, newer["version"].clone()), (StatusCode::OK, json!(2)), "{newer}");
}

async fn snapshot(harness: &Harness, ws: &str, rid: &str, body: Value) -> (StatusCode, Value) {
    harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/runs/{rid}/settings-snapshot"), Some(body)).await
}

async fn adopt_settings(harness: &Harness) {
    let (status, candidate) = harness.send_json(Method::POST, "/api/v1/shared-settings/import-preview", None).await;
    assert_eq!(status, StatusCode::OK, "{candidate}");
    let body = json!({ "expected_hash": candidate["candidate_hash"] });
    let (status, version) = harness.send_json(Method::POST, "/api/v1/shared-settings/adopt", Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{version}");
}
