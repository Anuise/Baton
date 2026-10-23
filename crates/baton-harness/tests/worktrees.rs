//! 情境測試：Worktree、獨立輸出目錄與整合佇列（票 20），經由 Runtime harness 的真實 HTTP API 驗證。
//!
//! Git 的狀態（哪個目錄是 repo、分支在哪個 commit、合併結果）由 fake WorkspaceFs 腳本化，目錄本身真的建立在暫存目錄中；
//! Worker 以 fake AgentAdapter 的結束事件回報，確定性檢查的命令由 fake CommandRunner 回應（可依工作目錄給不同結果）。
//! 使用者以桌面憑證派工、查看工作面板、驗證狀況與整合佇列。

use std::path::{Path, PathBuf};
use std::time::Duration;

use baton_domain::Timestamp;
use baton_harness::Harness;
use baton_ports::AdapterEvent;
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

const WORKSPACES: &str = "/api/v1/workspaces";
const INTEGRATION_QUEUE: &str = "/api/v1/integration-queue";
const CLOCK_START: Timestamp = Timestamp::from_millis(1_700_000_000_000);

// --- 共用輔助 ---

async fn harness() -> Harness {
    let harness = Harness::builder().clock_at(CLOCK_START).start().await;
    harness.add_account("主要帳號", "main@example.com").await;
    harness.add_account("第二帳號", "second@example.com").await;
    harness
}

/// 建立 Workspace（目錄在資料目錄下）與 Goal；`git` 為真時把目錄當成簽出 main 的 Git repo。
async fn setup(harness: &Harness, git: bool) -> (String, PathBuf, String) {
    let dir = harness.data_dir().join("projects").join("app");
    std::fs::create_dir_all(&dir).unwrap();
    let (status, ws) = harness.send_json(Method::POST, WORKSPACES, Some(json!({ "path": dir.to_string_lossy() }))).await;
    assert_eq!(status, StatusCode::CREATED, "{ws}");
    // 以 Workspace 正規化後的路徑為準（fake Git 依路徑比對）
    let dir = PathBuf::from(ws["path"].as_str().unwrap());
    if git {
        harness.workspace_fs.init_repo(&dir, "main");
    }
    let ws = ws["id"].as_str().unwrap().to_string();
    let (status, goal) =
        harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/goals"), Some(json!({ "title": "目標" }))).await;
    assert_eq!(status, StatusCode::CREATED, "{goal}");
    (ws, dir, goal["id"].as_str().unwrap().to_string())
}

fn filled(text: &str) -> Value {
    json!({ "status": "filled", "text": text })
}

/// 完整契約；`checks` 為真時宣告一項確定性檢查（`cargo test` 結束碼 0），Runtime 會自動驗證。
fn contract(checks: bool) -> Value {
    let checks = if checks {
        json!([{ "kind": "command", "program": "cargo", "args": ["test"], "expected_exit_code": 0 }])
    } else {
        json!([])
    };
    json!({
        "objective": filled("修正登入流程"),
        "inputs": filled("既有程式碼"),
        "scope": filled("登入模組"),
        "deliverables": filled("修正與測試"),
        "done_criteria": filled("測試通過"),
        "blocker_handling": filled("卡住時回報"),
        "required_capabilities": [],
        "verification_checks": checks,
    })
}

fn task_path(ws: &str, tid: &str) -> String {
    format!("{WORKSPACES}/{ws}/tasks/{tid}")
}

async fn ready_task(harness: &Harness, ws: &str, goal: &str, title: &str, kind: &str, checks: bool) -> String {
    let body = json!({ "goal_id": goal, "title": title, "kind": kind, "contract": contract(checks) });
    let (status, task) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks"), Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    assert_eq!(task["kind"], kind);
    let tid = task["id"].as_str().unwrap().to_string();
    let (status, ready) = harness.send_json(Method::POST, &format!("{}/ready", task_path(ws, &tid)), None).await;
    assert_eq!(status, StatusCode::OK, "{ready}");
    tid
}

async fn dispatch(harness: &Harness, ws: &str, tid: &str) -> (StatusCode, Value) {
    harness.send_json(Method::POST, &format!("{}/dispatch", task_path(ws, tid)), None).await
}

/// 派工並讓 Supervisor 啟動它，回傳 Run 識別。
async fn start_run(harness: &Harness, ws: &str, tid: &str) -> String {
    let (status, run) = dispatch(harness, ws, tid).await;
    assert_eq!(status, StatusCode::CREATED, "{run}");
    harness.supervise().await;
    run["id"].as_str().unwrap().to_string()
}

/// 這個 Run 的程序啟動時的工作目錄。
fn spawned_dir(harness: &Harness, rid: &str) -> String {
    let spec = harness
        .process_control
        .spawned()
        .into_iter()
        .find(|spec| spec.args.first().map(String::as_str) == Some(rid))
        .expect("Run 應該已經啟動");
    spec.working_dir.expect("Worker 一定有工作目錄")
}

async fn board(harness: &Harness, ws: &str) -> Value {
    let (status, board) = harness.send_json(Method::GET, &format!("{WORKSPACES}/{ws}/tasks"), None).await;
    assert_eq!(status, StatusCode::OK, "{board}");
    board
}

fn workdir_of<'a>(board: &'a Value, tid: &str) -> &'a Value {
    board["workdirs"].as_array().unwrap().iter().find(|w| w["task_id"] == tid).expect("Task 應該有登記的工作目錄")
}

async fn task_status(harness: &Harness, ws: &str, tid: &str) -> String {
    let (status, detail) = harness.send_json(Method::GET, &task_path(ws, tid), None).await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    detail["task"]["status"].as_str().unwrap().to_string()
}

async fn queue(harness: &Harness) -> Value {
    let (status, queue) = harness.send_json(Method::GET, INTEGRATION_QUEUE, None).await;
    assert_eq!(status, StatusCode::OK, "{queue}");
    queue
}

fn entry_of<'a>(queue: &'a Value, tid: &str) -> &'a Value {
    queue["entries"].as_array().unwrap().iter().rfind(|e| e["task_id"] == tid).expect("Task 應該有整合項目")
}

fn worktree_path(dir: &Path, tid: &str) -> String {
    dir.join(".baton").join("worktrees").join(tid).to_string_lossy().into_owned()
}

fn integration_dir(dir: &Path) -> PathBuf {
    dir.join(".baton").join("integration")
}

/// Worker 程序結束，讓 Supervisor 處理（含驗證與整合）。
async fn worker_exits(harness: &Harness, rid: &str, success: bool) {
    harness.agent_adapter.emit(rid, AdapterEvent::Exited { success });
    harness.supervise().await;
}

// --- worktree 與輸出目錄 ---

#[tokio::test]
async fn different_development_tasks_each_run_in_their_own_registered_worktree_linked_to_their_runs() {
    let harness = harness().await;
    let (ws, dir, goal) = setup(&harness, true).await;
    let first = ready_task(&harness, &ws, &goal, "修正登入", "development", false).await;
    let second = ready_task(&harness, &ws, &goal, "修正登出", "development", false).await;

    let r1 = start_run(&harness, &ws, &first).await;
    let r2 = start_run(&harness, &ws, &second).await;

    // 各自的 worktree、各自的平台分支，兩者不同
    assert_eq!(spawned_dir(&harness, &r1), worktree_path(&dir, &first));
    assert_eq!(spawned_dir(&harness, &r2), worktree_path(&dir, &second));
    assert_ne!(spawned_dir(&harness, &r1), spawned_dir(&harness, &r2));
    let first_dir = PathBuf::from(worktree_path(&dir, &first));
    assert!(first_dir.is_dir(), "worktree 目錄真的被建立");
    assert_eq!(harness.workspace_fs.worktree_branch(&first_dir), Some(format!("baton/{first}")));
    // .baton 內的 .gitignore 忽略全部內容，主 checkout 不會出現未追蹤的檔案
    let gitignore = std::fs::read_to_string(dir.join(".baton").join(".gitignore")).unwrap();
    assert!(gitignore.lines().any(|line| line == "*"), "{gitignore}");

    // 平台登記：種類、路徑、分支、整合目標與在其中執行的 Run
    let shown = board(&harness, &ws).await;
    let registered = workdir_of(&shown, &first);
    assert_eq!(registered["kind"], "worktree");
    assert_eq!(registered["status"], "ready");
    assert_eq!(registered["path"].as_str(), Some(worktree_path(&dir, &first).as_str()));
    assert_eq!(registered["branch"].as_str(), Some(format!("baton/{first}").as_str()));
    assert_eq!(registered["base_branch"], "main");
    assert_eq!(registered["run_ids"], json!([r1]));
    assert_eq!(workdir_of(&shown, &second)["run_ids"], json!([r2]));

    // 第一個 Run 失敗後重新派工：同一個 Task 回到同一個 worktree（原生 Session 以工作目錄區分，才接得回來）
    worker_exits(&harness, &r1, false).await;
    assert_eq!(task_status(&harness, &ws, &first).await, "ready");
    let again = start_run(&harness, &ws, &first).await;
    assert_eq!(spawned_dir(&harness, &again), worktree_path(&dir, &first));
    let shown = board(&harness, &ws).await;
    assert_eq!(workdir_of(&shown, &first)["run_ids"], json!([r1, again]), "兩個 Run 都關聯到同一個 worktree");
    assert_eq!(workdir_of(&shown, &first)["version"], 1, "已可用的 worktree 不會重建");
}

#[tokio::test]
async fn a_research_task_in_a_non_git_workspace_uses_an_output_directory_instead_of_a_worktree() {
    let harness = harness().await;
    let (ws, dir, goal) = setup(&harness, false).await;
    let tid = ready_task(&harness, &ws, &goal, "整理競品", "research", false).await;
    let doc = ready_task(&harness, &ws, &goal, "寫使用手冊", "document", false).await;

    let rid = start_run(&harness, &ws, &tid).await;
    let doc_run = start_run(&harness, &ws, &doc).await;

    let expected = dir.join(".baton").join("outputs").join(&tid);
    assert_eq!(spawned_dir(&harness, &rid), expected.to_string_lossy());
    assert!(expected.is_dir(), "輸出目錄真的被建立");
    assert!(!harness.workspace_fs.has_worktree(&expected), "研究 Task 不建立 worktree");
    assert_ne!(spawned_dir(&harness, &doc_run), spawned_dir(&harness, &rid), "每個 Task 有自己的輸出目錄");
    let shown = board(&harness, &ws).await;
    let registered = workdir_of(&shown, &tid);
    assert_eq!(registered["kind"], "output_dir");
    assert!(registered["branch"].is_null() && registered["base_branch"].is_null(), "{registered}");
    assert_eq!(registered["run_ids"], json!([rid]));
}

#[tokio::test]
async fn a_development_task_whose_worktree_cannot_be_prepared_is_not_dispatched_and_shows_why() {
    let harness = harness().await;
    let (ws, dir, goal) = setup(&harness, false).await;
    let tid = ready_task(&harness, &ws, &goal, "修正登入", "development", false).await;

    // 不是 Git repo：不派工，排隊並顯示原因
    let (status, queued) = dispatch(&harness, &ws, &tid).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{queued}");
    let reasons = queued["reasons"].as_array().unwrap();
    assert_eq!(reasons.len(), 1, "{queued}");
    assert_eq!(reasons[0]["category"], "workdir");
    assert!(reasons[0]["message"].as_str().unwrap().contains("不是 Git repo"), "{queued}");
    harness.supervise().await;
    assert_eq!(harness.process_control.spawn_count(), 0, "目錄不可用時不派工");
    assert_eq!(task_status(&harness, &ws, &tid).await, "ready");
    let shown = board(&harness, &ws).await;
    assert!(shown["runs"].as_array().unwrap().is_empty(), "沒有建立任何 Run");
    assert_eq!(workdir_of(&shown, &tid)["status"], "failed");

    // 變成 Git repo，但建立 worktree 失敗：原因換成實際的錯誤；排程每分鐘重試一次，不會每秒重做
    harness.workspace_fs.init_repo(&dir, "main");
    harness.workspace_fs.fail_next_worktree("檔案被占用");
    let (status, queued) = dispatch(&harness, &ws, &tid).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{queued}");
    assert!(queued["reasons"][0]["message"].as_str().unwrap().contains("檔案被占用"), "{queued}");
    harness.clock.advance_millis(1_000);
    harness.supervise().await;
    assert_eq!(harness.process_control.spawn_count(), 0);

    // 重試間隔過後，Supervisor 自動重試、成功後派出
    harness.clock.advance_millis(60_000);
    harness.supervise().await;
    assert_eq!(harness.process_control.spawn_count(), 1, "目錄可用之後自動派出");
    assert_eq!(task_status(&harness, &ws, &tid).await, "running");
    let shown = board(&harness, &ws).await;
    assert_eq!(workdir_of(&shown, &tid)["status"], "ready");
}

// --- 整合佇列 ---

#[tokio::test]
async fn the_integration_queue_merges_one_branch_at_a_time_and_tasks_complete_only_after_integration() {
    let harness = harness().await;
    let (ws, dir, goal) = setup(&harness, true).await;
    harness.command_runner.script_exit("cargo", 0, "test result: ok");
    let first = ready_task(&harness, &ws, &goal, "修正登入", "development", true).await;
    let second = ready_task(&harness, &ws, &goal, "修正登出", "development", true).await;
    let r1 = start_run(&harness, &ws, &first).await;
    let r2 = start_run(&harness, &ws, &second).await;

    // 兩個 Worker 都結束、成果都通過驗證；第一個合併在途中停住，觀察佇列
    let gate = harness.workspace_fs.hold_next_merge();
    harness.agent_adapter.emit(&r1, AdapterEvent::Exited { success: true });
    harness.agent_adapter.emit(&r2, AdapterEvent::Exited { success: true });
    let observe = async {
        tokio::time::timeout(Duration::from_secs(10), harness.workspace_fs.wait_for_held_merge())
            .await
            .expect("第一個合併應該停在閘門");
        let snapshot = queue(&harness).await;
        let first_status = task_status(&harness, &ws, &first).await;
        let second_status = task_status(&harness, &ws, &second).await;
        gate.notify_one();
        (snapshot, first_status, second_status)
    };
    let ((), (snapshot, first_status, second_status)) = tokio::join!(harness.supervise(), observe);

    // 合併進行中：只有一個處理中，另一個排隊；兩個 Task 都通過驗證但都還沒完成
    let active = snapshot["active"].as_str().expect("應該有處理中的項目").to_string();
    assert_eq!(entry_of(&snapshot, &first)["id"].as_str(), Some(active.as_str()), "先排入的先處理");
    assert_eq!(entry_of(&snapshot, &first)["status"], "merging");
    assert_eq!(entry_of(&snapshot, &second)["status"], "queued", "一次只處理一個合併");
    assert_eq!((first_status.as_str(), second_status.as_str()), ("verifying", "verifying"), "未整合不能宣告完成");

    // 放行後依序整合：第二個在第一個前移後的 main 上合併
    let merges = harness.workspace_fs.merges();
    assert_eq!(merges.len(), 2, "{merges:?}");
    assert_eq!((merges[0].branch.clone(), merges[1].branch.clone()), (format!("baton/{first}"), format!("baton/{second}")));
    assert_eq!(merges[0].target_tip, "base");
    assert_eq!(Some(merges[1].target_tip.clone()), merges[0].commit, "第二個合併的基底是第一個的整合結果");
    assert_eq!(merges[0].integration_dir, integration_dir(&dir), "合併在整合目錄進行，不在主 checkout");
    assert_eq!(harness.workspace_fs.branch_tip(&dir, "main"), merges[1].commit, "main 前移到最後的整合結果");

    // 整合驗證在整合目錄中執行；兩個 Task 都完成
    let integration_checks =
        harness.command_runner.calls().iter().filter(|c| c.working_dir == integration_dir(&dir)).count();
    assert_eq!(integration_checks, 2, "每個項目合併後都執行一次整合驗證");
    assert_eq!(task_status(&harness, &ws, &first).await, "completed");
    assert_eq!(task_status(&harness, &ws, &second).await, "completed");
    let done = queue(&harness).await;
    assert!(done["active"].is_null(), "{done}");
    for tid in [&first, &second] {
        let entry = entry_of(&done, tid);
        assert_eq!(entry["status"], "integrated", "{entry}");
        assert_eq!(entry["target_branch"], "main");
        assert_eq!(entry["checks"][0]["status"], "passed", "整合驗證的證據保存在項目中");
    }
    // 工作面板與驗證檢視都看得到整合結果
    let shown = board(&harness, &ws).await;
    assert_eq!(shown["integrations"].as_array().unwrap().len(), 2);
    let (status, view) = harness.send_json(Method::GET, &format!("{}/verification", task_path(&ws, &first)), None).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(view["integration"]["status"], "integrated");
}

#[tokio::test]
async fn a_failing_integration_verification_keeps_the_task_from_completing_and_leaves_main_untouched() {
    let harness = harness().await;
    let (ws, dir, goal) = setup(&harness, true).await;
    // 在 worktree 中驗證通過，合併後在整合目錄中卻失敗
    harness.command_runner.script_exit("cargo", 0, "test result: ok");
    harness.command_runner.script_exit_in("cargo", &integration_dir(&dir), 1, "test result: FAILED. 1 failed");
    let tid = ready_task(&harness, &ws, &goal, "修正登入", "development", true).await;
    let rid = start_run(&harness, &ws, &tid).await;

    worker_exits(&harness, &rid, true).await;

    assert_eq!(task_status(&harness, &ws, &tid).await, "ready", "未通過整合：Task 不完成，回到可修正狀態");
    assert_eq!(harness.workspace_fs.branch_tip(&dir, "main").as_deref(), Some("base"), "main 沒有被未通過的合併改變");
    let entry = entry_of(&queue(&harness).await, &tid).clone();
    assert_eq!(entry["status"], "failed", "{entry}");
    assert!(entry["failure"].as_str().unwrap().contains("整合驗證未通過"), "{entry}");
    assert_eq!(entry["checks"][0]["status"], "failed");
    assert!(entry["checks"][0]["detail"].as_str().unwrap().contains("1 failed"), "證據保存在項目中：{entry}");
    // 成果本身的驗證紀錄是通過的；Task 不完成是因為整合
    let (_, view) = harness.send_json(Method::GET, &format!("{}/verification", task_path(&ws, &tid)), None).await;
    assert_eq!(view["records"][0]["outcome"], "passed");
    assert_eq!(view["integration"]["status"], "failed");
}

#[tokio::test]
async fn a_merge_conflict_fails_the_integration_without_completing_the_task() {
    let harness = harness().await;
    let (ws, dir, goal) = setup(&harness, true).await;
    harness.command_runner.script_exit("cargo", 0, "ok");
    harness.workspace_fs.conflict_next_merge("src/login.rs");
    let tid = ready_task(&harness, &ws, &goal, "修正登入", "development", true).await;
    let rid = start_run(&harness, &ws, &tid).await;

    worker_exits(&harness, &rid, true).await;

    assert_eq!(task_status(&harness, &ws, &tid).await, "ready");
    let entry = entry_of(&queue(&harness).await, &tid).clone();
    assert_eq!(entry["status"], "failed");
    assert!(entry["failure"].as_str().unwrap().contains("src/login.rs"), "{entry}");
    assert_eq!(harness.workspace_fs.branch_tip(&dir, "main").as_deref(), Some("base"));
    let integration_checks =
        harness.command_runner.calls().iter().filter(|c| c.working_dir == integration_dir(&dir)).count();
    assert_eq!(integration_checks, 0, "衝突時不執行整合驗證");
}

#[tokio::test]
async fn a_task_redone_after_a_failed_integration_can_pass_verification_again_and_complete() {
    // R151 第 1 項：第一次驗證通過、整合失敗回到 ready；重做的 Run 再通過時不能撞上「每個 Task 一筆通過」而卡在 verifying
    let harness = harness().await;
    let (ws, dir, goal) = setup(&harness, true).await;
    harness.command_runner.script_exit("cargo", 0, "ok");
    harness.workspace_fs.conflict_next_merge("src/login.rs");
    let tid = ready_task(&harness, &ws, &goal, "修正登入", "development", true).await;
    let first = start_run(&harness, &ws, &tid).await;
    worker_exits(&harness, &first, true).await;
    assert_eq!(task_status(&harness, &ws, &tid).await, "ready", "整合衝突：回到可修正狀態");

    let second = start_run(&harness, &ws, &tid).await;
    worker_exits(&harness, &second, true).await;

    assert_eq!(task_status(&harness, &ws, &tid).await, "completed", "重做的 Run 通過驗證並整合後完成");
    let (_, view) = harness.send_json(Method::GET, &format!("{}/verification", task_path(&ws, &tid)), None).await;
    let records = view["records"].as_array().unwrap();
    assert_eq!(records.len(), 2, "{view}");
    assert!(records.iter().all(|r| r["outcome"] == "passed"), "兩個 Run 各有一筆通過紀錄：{view}");
    assert_eq!(view["integration"]["status"], "integrated", "{view}");
    assert_eq!(harness.workspace_fs.branch_tip(&dir, "main"), harness.workspace_fs.merges().last().unwrap().commit);
}

#[tokio::test]
async fn a_manually_accepted_development_task_also_waits_for_integration() {
    let harness = harness().await;
    let (ws, dir, goal) = setup(&harness, true).await;
    let tid = ready_task(&harness, &ws, &goal, "修正登入", "development", false).await;
    let rid = start_run(&harness, &ws, &tid).await;
    harness.agent_adapter.emit(&rid, AdapterEvent::Exited { success: true });
    harness.supervise().await;
    let (_, view) = harness.send_json(Method::GET, &format!("{}/verification", task_path(&ws, &tid)), None).await;

    let gate = harness.workspace_fs.hold_next_merge();
    let accepted = harness
        .send_json(Method::POST, &format!("{}/verification/accept", task_path(&ws, &tid)), Some(json!({ "basis": view["basis"] })))
        .await;
    assert_eq!(accepted.0, StatusCode::OK, "{}", accepted.1);
    assert_eq!(accepted.1["status"], "verifying", "手動接受也要經過整合才完成：{}", accepted.1);
    assert_eq!(accepted.1["integration"]["status"], "queued", "{}", accepted.1);
    assert!(accepted.1["basis"].is_null(), "已排入整合，沒有可再接受的基礎：{}", accepted.1);
    // 排入整合後不能再接受一次
    let again = harness
        .send_json(Method::POST, &format!("{}/verification/accept", task_path(&ws, &tid)), Some(json!({ "basis": view["basis"] })))
        .await;
    assert_eq!(again.0, StatusCode::CONFLICT, "{}", again.1);

    gate.notify_one();
    harness.supervise().await;
    assert_eq!(task_status(&harness, &ws, &tid).await, "completed");
    assert_eq!(harness.workspace_fs.branch_tip(&dir, "main"), harness.workspace_fs.merges()[0].commit);
}

#[tokio::test]
async fn a_runtime_crash_mid_merge_resumes_after_restart_and_integrates_once() {
    let mut harness = harness().await;
    let (ws, dir, goal) = setup(&harness, true).await;
    harness.command_runner.script_exit("cargo", 0, "ok");
    let tid = ready_task(&harness, &ws, &goal, "修正登入", "development", true).await;
    let rid = start_run(&harness, &ws, &tid).await;

    // 合併途中 Runtime 當掉：處理這次 tick 的 future 被丟棄
    let _never = harness.workspace_fs.hold_next_merge();
    harness.agent_adapter.emit(&rid, AdapterEvent::Exited { success: true });
    harness.supervise_until(harness.workspace_fs.wait_for_held_merge()).await;
    assert_eq!(entry_of(&queue(&harness).await, &tid)["status"], "merging", "開始合併的意圖已經持久化");

    harness.restart().await;
    harness.supervise().await;

    // 從合併重做（整合目錄會被重設）；只前移一次、Task 完成
    assert_eq!(task_status(&harness, &ws, &tid).await, "completed");
    let merges = harness.workspace_fs.merges();
    assert_eq!(merges.len(), 1, "崩潰的那次沒有結果，重做一次：{merges:?}");
    assert_eq!(harness.workspace_fs.branch_tip(&dir, "main"), merges[0].commit);
    let entry = entry_of(&queue(&harness).await, &tid).clone();
    assert_eq!(entry["status"], "integrated", "{entry}");
}
