//! 情境測試：Worktree 完成後自動清理（票 21），經由 Runtime harness 的真實 HTTP API 驗證。
//!
//! Git 的狀態由 fake WorkspaceFs 腳本化，目錄本身真的建立在暫存目錄中；移除 worktree 與正式版步驟相同
//! （改名到 `.baton\trash\<Task>` 再刪除），可以腳本化失敗（例如 Windows 的檔案占用）或在改名之後當掉。
//! 開發 Task 以確定性檢查通過驗證、經整合佇列整合後完成，Supervisor 在同一次 `tick` 中登記待清理並移除 worktree。
//! 使用者以桌面憑證查看工作面板（Task 狀態與清理狀態分開顯示）。

use std::path::{Path, PathBuf};

use baton_domain::artifact::TOOL_REGISTER_ARTIFACT;
use baton_domain::Timestamp;
use baton_harness::Harness;
use baton_ports::{AdapterEvent, StopResult};
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

const WORKSPACES: &str = "/api/v1/workspaces";
const LIFECYCLE: &str = "/api/v1/lifecycle";
const CLOCK_START: Timestamp = Timestamp::from_millis(1_700_000_000_000);

// --- 共用輔助 ---

async fn harness() -> Harness {
    let harness = Harness::builder().clock_at(CLOCK_START).start().await;
    harness.add_account("主要帳號", "main@example.com").await;
    harness.add_account("第二帳號", "second@example.com").await;
    harness.command_runner.script_exit("cargo", 0, "test result: ok");
    harness
}

/// 建立簽出 main 的 Git Workspace（目錄在資料目錄下）與 Goal。
async fn setup(harness: &Harness) -> (String, PathBuf, String) {
    let dir = harness.data_dir().join("projects").join("app");
    std::fs::create_dir_all(&dir).unwrap();
    let (status, ws) = harness.send_json(Method::POST, WORKSPACES, Some(json!({ "path": dir.to_string_lossy() }))).await;
    assert_eq!(status, StatusCode::CREATED, "{ws}");
    let dir = PathBuf::from(ws["path"].as_str().unwrap());
    harness.workspace_fs.init_repo(&dir, "main");
    let ws = ws["id"].as_str().unwrap().to_string();
    let (status, goal) =
        harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/goals"), Some(json!({ "title": "目標" }))).await;
    assert_eq!(status, StatusCode::CREATED, "{goal}");
    (ws, dir, goal["id"].as_str().unwrap().to_string())
}

fn filled(text: &str) -> Value {
    json!({ "status": "filled", "text": text })
}

/// 宣告一項確定性檢查（`cargo test` 結束碼 0）的完整契約，Runtime 會自動驗證與整合。
fn contract() -> Value {
    json!({
        "objective": filled("修正登入流程"),
        "inputs": filled("既有程式碼"),
        "scope": filled("登入模組"),
        "deliverables": filled("修正與測試"),
        "done_criteria": filled("測試通過"),
        "blocker_handling": filled("卡住時回報"),
        "required_capabilities": [],
        "verification_checks": [{ "kind": "command", "program": "cargo", "args": ["test"], "expected_exit_code": 0 }],
    })
}

fn task_path(ws: &str, tid: &str) -> String {
    format!("{WORKSPACES}/{ws}/tasks/{tid}")
}

async fn ready_task(harness: &Harness, ws: &str, goal: &str, title: &str, kind: &str) -> String {
    let body = json!({ "goal_id": goal, "title": title, "kind": kind, "contract": contract() });
    let (status, task) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks"), Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    let tid = task["id"].as_str().unwrap().to_string();
    let (status, ready) = harness.send_json(Method::POST, &format!("{}/ready", task_path(ws, &tid)), None).await;
    assert_eq!(status, StatusCode::OK, "{ready}");
    tid
}

/// 派工並讓 Supervisor 啟動它，回傳 Run 識別。
async fn start_run(harness: &Harness, ws: &str, tid: &str) -> String {
    let (status, run) = harness.send_json(Method::POST, &format!("{}/dispatch", task_path(ws, tid)), None).await;
    assert_eq!(status, StatusCode::CREATED, "{run}");
    harness.supervise().await;
    run["id"].as_str().unwrap().to_string()
}

/// Worker 程序結束，讓 Supervisor 處理（驗證、整合與清理）。
async fn worker_exits(harness: &Harness, rid: &str, success: bool) {
    harness.agent_adapter.emit(rid, AdapterEvent::Exited { success });
    harness.supervise().await;
}

async fn board(harness: &Harness, ws: &str) -> Value {
    let (status, board) = harness.send_json(Method::GET, &format!("{WORKSPACES}/{ws}/tasks"), None).await;
    assert_eq!(status, StatusCode::OK, "{board}");
    board
}

async fn task_status(harness: &Harness, ws: &str, tid: &str) -> String {
    let (status, detail) = harness.send_json(Method::GET, &task_path(ws, tid), None).await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    detail["task"]["status"].as_str().unwrap().to_string()
}

/// 工作面板上這個 Task 的清理狀態（還沒有登記時是 `null`）。
async fn cleanup_of(harness: &Harness, ws: &str, tid: &str) -> Value {
    let shown = board(harness, ws).await;
    let workdir = shown["workdirs"].as_array().unwrap().iter().find(|w| w["task_id"] == tid).expect("Task 應該有登記的工作目錄");
    workdir["cleanup"].clone()
}

fn worktree(dir: &Path, tid: &str) -> PathBuf {
    dir.join(".baton").join("worktrees").join(tid)
}

fn trash(dir: &Path, tid: &str) -> PathBuf {
    dir.join(".baton").join("trash").join(tid)
}

/// 完成一個開發 Task（派工 → Worker 成功結束 → 驗證通過 → 整合），回傳（Task、Run）。
async fn completed_task(harness: &Harness, ws: &str, goal: &str, title: &str) -> (String, String) {
    let tid = ready_task(harness, ws, goal, title, "development").await;
    let rid = start_run(harness, ws, &tid).await;
    worker_exits(harness, &rid, true).await;
    (tid, rid)
}

fn event_types(harness: &Harness, task_id: &str) -> Vec<String> {
    harness
        .db()
        .prepare("SELECT event_type FROM event WHERE aggregate_type = 'worktree_cleanup' AND aggregate_id = ?1 ORDER BY rowid")
        .unwrap()
        .query_map([task_id], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

// --- 只清理平台登記的 worktree ---

#[tokio::test]
async fn a_completed_and_integrated_task_has_only_its_registered_worktree_removed() {
    let harness = harness().await;
    let (ws, dir, goal) = setup(&harness).await;
    // 原始 checkout 與 Workspace 中的其他目錄
    std::fs::write(dir.join("README.md"), "原始 checkout 的檔案").unwrap();
    std::fs::create_dir_all(dir.join("docs")).unwrap();
    std::fs::write(dir.join("docs").join("keep.md"), "其他目錄").unwrap();
    // 另一個仍在執行的開發 Task 與一個研究 Task
    let other = ready_task(&harness, &ws, &goal, "修正登出", "development").await;
    start_run(&harness, &ws, &other).await;
    let research = ready_task(&harness, &ws, &goal, "整理競品", "research").await;
    start_run(&harness, &ws, &research).await;
    // 要清理的 Task：Worker 登記一份正式成果後結束
    let tid = ready_task(&harness, &ws, &goal, "修正登入", "development").await;
    let rid = start_run(&harness, &ws, &tid).await;
    let token = harness.agent_credential(&rid).unwrap();
    let (status, body) =
        harness.call_tool(&token, TOOL_REGISTER_ARTIFACT, json!({ "name": "report.md", "content": "修正說明" })).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert!(worktree(&dir, &tid).is_dir());

    worker_exits(&harness, &rid, true).await;

    // Task 已完成，清理狀態另外顯示為已清理
    assert_eq!(task_status(&harness, &ws, &tid).await, "completed");
    let cleanup = cleanup_of(&harness, &ws, &tid).await;
    assert_eq!(cleanup["status"], "cleaned", "{cleanup}");
    assert_eq!(cleanup["attempts"], 1);
    assert_eq!(cleanup["path"].as_str(), Some(worktree(&dir, &tid).to_string_lossy().as_ref()));
    assert_eq!(event_types(&harness, &tid), ["worktree_cleanup_requested", "worktree_cleanup_started", "worktree_cleaned"]);
    assert!(!worktree(&dir, &tid).exists(), "平台登記的 worktree 被移除");
    assert!(!trash(&dir, &tid).exists(), "暫放目錄也被刪除");
    assert_eq!(harness.workspace_fs.removals(), vec![worktree(&dir, &tid)], "只移除這一個目錄");

    // 原始 checkout、其他目錄、其他 Task 的工作目錄都不受影響
    assert_eq!(std::fs::read_to_string(dir.join("README.md")).unwrap(), "原始 checkout 的檔案");
    assert_eq!(std::fs::read_to_string(dir.join("docs").join("keep.md")).unwrap(), "其他目錄");
    assert!(worktree(&dir, &other).is_dir(), "仍在執行的 Task 的 worktree 不受影響");
    assert!(cleanup_of(&harness, &ws, &other).await.is_null());
    assert!(dir.join(".baton").join("outputs").join(&research).is_dir(), "研究 Task 的輸出目錄不清理");
    assert!(cleanup_of(&harness, &ws, &research).await.is_null());

    // 不刪除 Git 分支、歷史或正式 Artifact
    assert_eq!(harness.workspace_fs.branch_tip(&dir, &format!("baton/{tid}")).as_deref(), Some("base"), "平台分支仍在");
    let merge = harness.workspace_fs.merges()[0].commit.clone();
    assert_eq!(harness.workspace_fs.branch_tip(&dir, "main"), merge, "整合結果仍在目標分支上");
    let (status, listing) = harness.send_json(Method::GET, &format!("{WORKSPACES}/{ws}/artifacts"), None).await;
    assert_eq!(status, StatusCode::OK, "{listing}");
    let artifact = &listing["artifacts"][0];
    assert_eq!(artifact["task_id"].as_str(), Some(tid.as_str()));
    let (status, downloaded) = harness
        .send_json(Method::GET, &format!("{WORKSPACES}/{ws}/artifacts/{}/versions/1/preview", artifact["id"].as_str().unwrap()), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{downloaded}");
    assert!(downloaded.to_string().contains("修正說明"), "成果仍可讀取：{downloaded}");

    // 已清理的登記不再處理
    harness.clock.advance_millis(3_600_000);
    harness.supervise().await;
    assert_eq!(harness.workspace_fs.removals().len(), 1);
}

// --- 移除失敗：保留待清理並重試，Task 仍是已完成 ---

#[tokio::test]
async fn a_locked_file_keeps_the_cleanup_pending_and_it_is_retried_while_the_task_stays_completed() {
    let harness = harness().await;
    let (ws, dir, goal) = setup(&harness).await;
    harness.workspace_fs.fail_next_removal("無法改名 worktree：檔案可能被其他程式占用，請關閉使用這個目錄的程式");

    let (tid, _) = completed_task(&harness, &ws, &goal, "修正登入").await;

    assert_eq!(task_status(&harness, &ws, &tid).await, "completed", "清理失敗不會改變 Task 的完成狀態");
    let cleanup = cleanup_of(&harness, &ws, &tid).await;
    assert_eq!(cleanup["status"], "pending", "{cleanup}");
    assert!(cleanup["failure"].as_str().unwrap().contains("被其他程式占用"), "顯示可行動的原因：{cleanup}");
    let retry_at = CLOCK_START.as_millis() + 60_000;
    assert_eq!(cleanup["next_attempt_at"], retry_at, "1 分鐘後重試");
    assert!(worktree(&dir, &tid).is_dir(), "移除失敗時 worktree 保留");

    // 還沒到重試時間：不再嘗試
    harness.clock.advance_millis(30_000);
    harness.supervise().await;
    assert_eq!(harness.workspace_fs.removals().len(), 1);

    // 到了重試時間：重新核對後移除
    harness.clock.advance_millis(30_000);
    harness.supervise().await;
    let cleanup = cleanup_of(&harness, &ws, &tid).await;
    assert_eq!(cleanup["status"], "cleaned", "{cleanup}");
    assert_eq!(cleanup["attempts"], 2);
    assert!(cleanup["failure"].is_null());
    assert!(!worktree(&dir, &tid).exists());
    assert_eq!(task_status(&harness, &ws, &tid).await, "completed");
    assert_eq!(
        event_types(&harness, &tid),
        [
            "worktree_cleanup_requested",
            "worktree_cleanup_started",
            "worktree_cleanup_retry_scheduled",
            "worktree_cleanup_started",
            "worktree_cleaned"
        ]
    );
}

#[tokio::test]
async fn cleanup_fails_after_limited_retries_and_keeps_the_worktree_and_the_completed_task() {
    let harness = harness().await;
    let (ws, dir, goal) = setup(&harness).await;
    for _ in 0..6 {
        harness.workspace_fs.fail_next_removal("檔案被占用");
    }

    let (tid, _) = completed_task(&harness, &ws, &goal, "修正登入").await;
    // 第一次失敗後再重試 5 次（1、2、4、8、10 分鐘後）
    for delay in [60_000, 120_000, 240_000, 480_000, 600_000] {
        assert_eq!(cleanup_of(&harness, &ws, &tid).await["status"], "pending");
        harness.clock.advance_millis(delay);
        harness.supervise().await;
    }

    let cleanup = cleanup_of(&harness, &ws, &tid).await;
    assert_eq!(cleanup["status"], "failed", "{cleanup}");
    assert_eq!(cleanup["attempts"], 6);
    assert_eq!(cleanup["failure"], "無法移除 worktree ".to_string() + &worktree(&dir, &tid).to_string_lossy() + "：檔案被占用");
    assert_eq!(task_status(&harness, &ws, &tid).await, "completed", "清理失敗時 Task 仍是已完成");
    assert!(worktree(&dir, &tid).is_dir(), "清理失敗時 worktree 保留");

    // 不再自動重試
    harness.clock.advance_millis(24 * 3_600_000);
    harness.supervise().await;
    assert_eq!(harness.workspace_fs.removals().len(), 6);
    assert_eq!(cleanup_of(&harness, &ws, &tid).await["status"], "failed");
}

// --- 未保存變更或歸屬不明的檔案阻擋刪除 ---

#[tokio::test]
async fn unsaved_changes_block_removal_and_safety_is_rechecked_on_every_retry() {
    let harness = harness().await;
    let (ws, dir, goal) = setup(&harness).await;
    let tid = ready_task(&harness, &ws, &goal, "修正登入", "development").await;
    let rid = start_run(&harness, &ws, &tid).await;
    harness.workspace_fs.mark_unsaved(&worktree(&dir, &tid), &["src/login.rs", "notes.txt"]);

    worker_exits(&harness, &rid, true).await;

    assert_eq!(task_status(&harness, &ws, &tid).await, "completed");
    let cleanup = cleanup_of(&harness, &ws, &tid).await;
    assert_eq!(cleanup["status"], "pending", "{cleanup}");
    let failure = cleanup["failure"].as_str().unwrap();
    assert!(failure.contains("未保存的變更") && failure.contains("src/login.rs") && failure.contains("notes.txt"), "{failure}");
    assert!(harness.workspace_fs.removals().is_empty(), "安全條件不符時根本不移除");
    assert!(worktree(&dir, &tid).is_dir());

    // 重試時仍有未保存的變更：再次擋下
    harness.clock.advance_millis(60_000);
    harness.supervise().await;
    assert_eq!(cleanup_of(&harness, &ws, &tid).await["attempts"], 2);
    assert!(harness.workspace_fs.removals().is_empty());

    // 使用者處理掉之後，下一次重試重新核對、移除
    harness.workspace_fs.clear_unsaved(&worktree(&dir, &tid));
    harness.clock.advance_millis(120_000);
    harness.supervise().await;
    assert_eq!(cleanup_of(&harness, &ws, &tid).await["status"], "cleaned");
    assert!(!worktree(&dir, &tid).exists());
}

#[tokio::test]
async fn a_directory_that_is_no_longer_the_registered_worktree_is_never_deleted() {
    let harness = harness().await;
    let (ws, dir, goal) = setup(&harness).await;
    harness.workspace_fs.fail_next_removal("檔案被占用");
    let (tid, _) = completed_task(&harness, &ws, &goal, "修正登入").await;
    assert_eq!(cleanup_of(&harness, &ws, &tid).await["status"], "pending");

    // 使用者在重試前自行移除了 worktree，同一路徑又放了自己的檔案（已不是 Git 登記的 worktree）
    let path = worktree(&dir, &tid);
    std::fs::remove_dir_all(&path).unwrap();
    harness.workspace_fs.forget_worktree(&path);
    std::fs::create_dir_all(&path).unwrap();
    std::fs::write(path.join("mine.txt"), "使用者的檔案").unwrap();

    harness.clock.advance_millis(60_000);
    harness.supervise().await;

    let cleanup = cleanup_of(&harness, &ws, &tid).await;
    assert_eq!(cleanup["status"], "pending", "{cleanup}");
    assert!(cleanup["failure"].as_str().unwrap().contains("歸屬不明"), "{cleanup}");
    assert_eq!(std::fs::read_to_string(path.join("mine.txt")).unwrap(), "使用者的檔案", "歸屬不明的檔案不被刪除");
    assert_eq!(harness.workspace_fs.removals().len(), 1, "核對不通過時不呼叫移除");
}

// --- 清理中崩潰：重建 Runtime 後核對、完成清理且不誤刪 ---

#[tokio::test]
async fn a_runtime_crash_mid_cleanup_is_reconciled_after_restart_and_finishes_without_deleting_anything_else() {
    let mut harness = harness().await;
    let (ws, dir, goal) = setup(&harness).await;
    std::fs::write(dir.join("README.md"), "原始 checkout 的檔案").unwrap();
    let other = ready_task(&harness, &ws, &goal, "修正登出", "development").await;
    start_run(&harness, &ws, &other).await;
    let tid = ready_task(&harness, &ws, &goal, "修正登入", "development").await;
    let rid = start_run(&harness, &ws, &tid).await;

    // worktree 已改名到暫放目錄、還沒刪除時 Runtime 當掉：處理這次 tick 的 future 被丟棄
    let _never = harness.workspace_fs.hold_next_removal();
    harness.agent_adapter.emit(&rid, AdapterEvent::Exited { success: true });
    harness.supervise_until(harness.workspace_fs.wait_for_held_removal()).await;
    let cleanup = cleanup_of(&harness, &ws, &tid).await;
    assert_eq!(cleanup["status"], "removing", "這一次嘗試已經持久化：{cleanup}");
    assert_eq!(task_status(&harness, &ws, &tid).await, "completed");
    assert!(!worktree(&dir, &tid).exists() && trash(&dir, &tid).is_dir(), "崩潰時留下平台自己的殘留");

    harness.restart().await;
    harness.supervise().await;

    // 核對後接續同一次嘗試：收拾殘留、完成清理
    let cleanup = cleanup_of(&harness, &ws, &tid).await;
    assert_eq!(cleanup["status"], "cleaned", "{cleanup}");
    assert_eq!(cleanup["attempts"], 1, "崩潰後接續不算另一次嘗試");
    assert!(!trash(&dir, &tid).exists());
    assert_eq!(
        event_types(&harness, &tid),
        ["worktree_cleanup_requested", "worktree_cleanup_started", "worktree_cleanup_started", "worktree_cleaned"]
    );
    // 不誤刪：原始 checkout、其他 Task 的 worktree、平台分支都還在
    assert_eq!(std::fs::read_to_string(dir.join("README.md")).unwrap(), "原始 checkout 的檔案");
    assert!(worktree(&dir, &other).is_dir());
    assert!(cleanup_of(&harness, &ws, &other).await.is_null());
    assert_eq!(harness.workspace_fs.branch_tip(&dir, &format!("baton/{tid}")).as_deref(), Some("base"));
    assert_eq!(task_status(&harness, &ws, &tid).await, "completed");
}

// --- 暫停、失敗、取消、完全退出不觸發清理 ---

#[tokio::test]
async fn failing_cancelling_or_failing_integration_never_triggers_cleanup() {
    let harness = harness().await;
    let (ws, dir, goal) = setup(&harness).await;

    // Run 失敗：Task 回到 ready
    let failed = ready_task(&harness, &ws, &goal, "Run 失敗", "development").await;
    let failed_run = start_run(&harness, &ws, &failed).await;
    worker_exits(&harness, &failed_run, false).await;
    assert_eq!(task_status(&harness, &ws, &failed).await, "ready");

    // 取消
    let cancelled = ready_task(&harness, &ws, &goal, "取消", "development").await;
    start_run(&harness, &ws, &cancelled).await;
    harness.process_control.set_stop_result(StopResult::StoppedGracefully);
    let (status, body) = harness.send_json(Method::POST, &format!("{}/cancel", task_path(&ws, &cancelled)), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    harness.supervise().await;
    assert_eq!(task_status(&harness, &ws, &cancelled).await, "cancelled");

    // 驗證通過但整合未通過（合併衝突）：Task 回到 ready
    harness.workspace_fs.conflict_next_merge("src/login.rs");
    let (conflicted, _) = completed_task(&harness, &ws, &goal, "整合衝突").await;
    assert_eq!(task_status(&harness, &ws, &conflicted).await, "ready");

    harness.clock.advance_millis(3_600_000);
    harness.supervise().await;
    for tid in [&failed, &cancelled, &conflicted] {
        assert!(cleanup_of(&harness, &ws, tid).await.is_null(), "{tid} 不應有清理登記");
        assert!(worktree(&dir, tid).is_dir(), "{tid} 的 worktree 保留");
    }
    assert!(harness.workspace_fs.removals().is_empty());
}

#[tokio::test]
async fn full_exit_never_triggers_cleanup_and_pending_cleanups_wait_until_work_resumes() {
    let mut harness = harness().await;
    let (ws, dir, goal) = setup(&harness).await;
    // 一個已完成、清理暫時失敗（待重試）的 Task，與一個執行中的 Task
    harness.workspace_fs.fail_next_removal("檔案被占用");
    let (done, _) = completed_task(&harness, &ws, &goal, "已完成").await;
    assert_eq!(cleanup_of(&harness, &ws, &done).await["status"], "pending");
    let running = ready_task(&harness, &ws, &goal, "執行中", "development").await;
    start_run(&harness, &ws, &running).await;
    harness.process_control.set_stop_result(StopResult::StoppedGracefully);

    let (status, body) = harness.send_json(Method::POST, &format!("{LIFECYCLE}/exit"), None).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    harness.wait_for_exit().await;
    harness.clock.advance_millis(3_600_000);
    harness.restart().await;
    harness.supervise().await;

    // 完全退出停下的 Task 不觸發清理；重新開啟後等待「繼續工作」期間，待清理的也不執行（不在這時執行 git）
    assert_eq!(task_status(&harness, &ws, &running).await, "ready");
    assert!(cleanup_of(&harness, &ws, &running).await.is_null());
    assert!(worktree(&dir, &running).is_dir());
    assert_eq!(cleanup_of(&harness, &ws, &done).await["status"], "pending");
    assert_eq!(harness.workspace_fs.removals().len(), 1);
    assert!(worktree(&dir, &done).is_dir());

    // 使用者「繼續工作」後，待清理的接著處理
    let (status, body) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/resume"), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    harness.supervise().await;
    assert_eq!(cleanup_of(&harness, &ws, &done).await["status"], "cleaned");
    assert!(!worktree(&dir, &done).exists());
    assert!(cleanup_of(&harness, &ws, &running).await.is_null(), "停下的 Task 仍不清理");
    assert!(worktree(&dir, &running).is_dir());
}
