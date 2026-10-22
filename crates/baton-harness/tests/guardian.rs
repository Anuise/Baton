//! 情境測試：監護程序、有限自動重啟與降級（票 32，ADR-0006、spec D-24），經由 Runtime harness 的真實 HTTP API 驗證。
//!
//! 監護在測試程序內執行（`start_guardian`／`guardian_step`），它看到的 Runtime 就是 harness 目前的 Runtime：
//! `kill_runtime` 是 Runtime 被殺掉而沒有人重啟它；監護啟動的 Runtime 帶著監護寫下的嘗試識別。
//! 時間全由 fake Clock 決定。只斷言外部行為：恢復狀態與健康端點、Run 與 Task、Port 被呼叫的方式，以及監護寫在磁碟上的日誌。
//!
//! 每個測試名稱對應票 32 的驗收條件：
//! - 關閉視窗後殺掉 Runtime，監護自行重啟並經過完整恢復核對：`killing_the_runtime_*`
//! - 監護日誌與資料庫銜接不重複扣次；啟動早期失敗也計次：`journal_and_database_*`、`early_start_failures_*`
//! - 完全退出與監護啟動競態時不拉起 Runtime：`full_exit_*`、`a_stop_intent_*`、`maintenance_*`
//! - 殺掉監護：工作繼續、UI 顯示降級，重開 App 只補監護：`killing_the_guardian_*`

use std::time::Duration;

use baton_domain::guardian::{JournalRecord, Suppression};
use baton_domain::lifecycle::{LifecycleFile, MaintenanceSuppression, LIFECYCLE_FILE_NAME, LIFECYCLE_FILE_VERSION};
use baton_domain::Timestamp;
use baton_guardian::{GuardianEnd, GuardianStep};
use baton_harness::Harness;
use baton_ports::ProcessIdentity;
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

const WORKSPACES: &str = "/api/v1/workspaces";
const RECOVERY: &str = "/api/v1/recovery";
const HEALTH: &str = "/api/v1/health";

/// 2023-11-14 22:13:20.123 UTC
const CLOCK_START: Timestamp = Timestamp::from_millis(1_700_000_000_123);

// --- 共用輔助 ---

async fn start() -> Harness {
    let harness = Harness::builder().clock_at(CLOCK_START).start().await;
    harness.add_account("帳號 1", "a@example.com").await;
    harness
}

fn filled(text: &str) -> Value {
    json!({ "status": "filled", "text": text })
}

/// 一個 Workspace 與它的 Goal，回傳 (workspace_id, goal_id)。
async fn workspace(harness: &Harness, name: &str) -> (String, String) {
    let dir = harness.data_dir().join("projects").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let body = json!({ "path": dir.to_string_lossy(), "name": name });
    let (status, created) = harness.send_json(Method::POST, WORKSPACES, Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let ws = created["id"].as_str().unwrap().to_string();
    let (status, goal) =
        harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/goals"), Some(json!({ "title": "目標" }))).await;
    assert_eq!(status, StatusCode::CREATED, "{goal}");
    (ws, goal["id"].as_str().unwrap().to_string())
}

/// 建立 ready 的 Task、派工並讓 Supervisor 啟動它，回傳 (task_id, run_id)。
async fn running_task(harness: &Harness, ws: &str, goal: &str) -> (String, String) {
    let contract = json!({
        "objective": filled("整理資料"),
        "inputs": filled("既有的資料"),
        "scope": filled("公開資料"),
        "deliverables": filled("比較表"),
        "done_criteria": filled("五個產品"),
        "blocker_handling": filled("卡住時回報"),
        "required_capabilities": [],
    });
    let body = json!({ "goal_id": goal, "title": "長任務", "contract": contract });
    let (status, task) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks"), Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    let tid = task["id"].as_str().unwrap().to_string();
    let (status, ready) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks/{tid}/ready"), None).await;
    assert_eq!(status, StatusCode::OK, "{ready}");
    let (status, run) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks/{tid}/dispatch"), None).await;
    assert_eq!(status, StatusCode::CREATED, "{run}");
    let rid = run["id"].as_str().unwrap().to_string();
    harness.supervise().await;
    assert_eq!(run_view(harness, ws, &rid).await["status"], "running");
    (tid, rid)
}

async fn run_view(harness: &Harness, ws: &str, rid: &str) -> Value {
    let (status, run) = harness.send_json(Method::GET, &format!("{WORKSPACES}/{ws}/runs/{rid}"), None).await;
    assert_eq!(status, StatusCode::OK, "{run}");
    run
}

/// 這個 Task 的全部 Run，依建立順序。
async fn runs_of(harness: &Harness, ws: &str, tid: &str) -> Vec<Value> {
    let (status, list) = harness.send_json(Method::GET, &format!("{WORKSPACES}/{ws}/runs"), None).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    list["runs"].as_array().unwrap().iter().filter(|r| r["task_id"] == tid).cloned().collect()
}

async fn identity(harness: &Harness, ws: &str, rid: &str) -> ProcessIdentity {
    let run = run_view(harness, ws, rid).await;
    harness.process_control.identity_of(run["command_id"].as_str().unwrap()).unwrap()
}

async fn recovery(harness: &Harness) -> Value {
    harness.get_json(RECOVERY).await
}

async fn guardian_status(harness: &Harness) -> Value {
    let health: Value = harness.get_json(HEALTH).await;
    health["guardian"].clone()
}

/// 監護處理一次，期望它重新啟動了 Runtime；回傳寫下的嘗試。
async fn expect_restart(harness: &mut Harness) -> baton_domain::guardian::GuardianAttempt {
    match harness.guardian_step().await {
        GuardianStep::Restarted(attempt) => attempt,
        other => panic!("監護應該重新啟動 Runtime：{other:?}"),
    }
}

/// Runtime 被殺掉後，監護確認它已結束。
async fn kill_and_observe(harness: &mut Harness) {
    harness.kill_runtime().await;
    assert_eq!(harness.guardian_step().await, GuardianStep::RuntimeExited);
}

fn now(harness: &Harness) -> Timestamp {
    use baton_ports::Clock;
    harness.clock.now()
}

fn write_lifecycle_file(harness: &Harness, stop: bool, maintenance: Option<&str>) {
    let file = LifecycleFile {
        version: LIFECYCLE_FILE_VERSION,
        stop: stop.then(|| baton_domain::lifecycle::StopSuppression { requested_at: CLOCK_START, reason: "full_exit".into() }),
        maintenance: maintenance.map(|kind| MaintenanceSuppression { kind: kind.into(), requested_at: CLOCK_START }),
        updated_at: CLOCK_START,
    };
    std::fs::write(harness.lifecycle_file_path(), serde_json::to_vec(&file).unwrap()).unwrap();
}

// ============================================================ 自動重啟與完整恢復核對

#[tokio::test]
async fn killing_the_runtime_after_the_window_closes_lets_the_guardian_restart_it_through_full_recovery_checks() {
    let mut harness = start().await;
    let (ws, goal) = workspace(&harness, "A").await;
    let (tid, old) = running_task(&harness, &ws, &goal).await;
    let old_identity = identity(&harness, &ws, &old).await;
    harness.start_guardian();
    assert_eq!(guardian_status(&harness).await, json!({ "state": "active", "message": null }));
    // 關閉視窗不影響 Runtime：監護只是持續監看
    assert_eq!(harness.guardian_step().await, GuardianStep::Watching);

    kill_and_observe(&mut harness).await;
    assert!(!harness.runtime_running());
    let attempt = expect_restart(&mut harness).await;
    assert_eq!((attempt.round, attempt.seq), (1, 1));
    assert!(harness.runtime_running(), "監護啟動了 Runtime");
    assert_eq!(harness.guardian_journal(), vec![JournalRecord::Attempt(attempt.clone())], "啟動前已寫下這次嘗試");

    // Runtime 匯入監護日誌並接續同一次嘗試：還沒處置前不派工、沒碰任何程序
    let pending = recovery(&harness).await;
    let event = &pending["event"];
    assert_eq!(event["id"], attempt.event_id.as_str());
    assert_eq!((event["status"].as_str(), event["attempts_used"].as_i64()), (Some("attempting"), Some(1)));
    assert_eq!(event["attempts"][0]["id"], attempt.attempt_id.as_str());
    assert_eq!(pending["blocking"]["code"], "crash_recovery_pending");
    assert!(harness.process_control.stop_calls().is_empty());

    harness.supervise().await;

    // 完整恢復核對：受控停止身分已確認的舊程序、以新 Run 恢復；不另扣一次
    assert_eq!(harness.process_control.stop_calls(), [old_identity.clone()]);
    let stopped = run_view(&harness, &ws, &old).await;
    assert_eq!((stopped["status"].as_str(), stopped["end_reason"].as_str()), (Some("interrupted"), Some("stopped_by_crash_recovery")));
    let runs = runs_of(&harness, &ws, &tid).await;
    assert_eq!((runs.len(), runs[1]["status"].as_str()), (2, Some("running")), "{runs:?}");
    let alive = harness.process_control.alive();
    assert_eq!(alive.len(), 1, "同一時間只有一個 writer");
    assert_ne!(alive[0], old_identity);
    let observing = recovery(&harness).await;
    assert_eq!(observing["event"]["status"], "observing");
    assert_eq!(observing["event"]["attempts_used"], 1, "監護的那一次就是 Runtime 做的那一次");
    assert_eq!(observing["event"]["attempts"].as_array().unwrap().len(), 1);

    // 監護繼續監看新的 Runtime
    assert_eq!(harness.guardian_step().await, GuardianStep::Watching);
    assert_eq!(guardian_status(&harness).await["state"], "active");
}

// ============================================================ 監護日誌與資料庫的銜接

#[tokio::test]
async fn journal_and_database_share_one_count_without_double_counting() {
    let mut harness = start().await;
    let (ws, goal) = workspace(&harness, "A").await;
    running_task(&harness, &ws, &goal).await;
    harness.start_guardian();

    kill_and_observe(&mut harness).await;
    let first = expect_restart(&mut harness).await;
    harness.supervise().await;
    assert_eq!(recovery(&harness).await["event"]["status"], "observing");

    // 同一份監護日誌再被讀一次（Runtime 重建）：以嘗試識別去重，不重複扣次
    harness.restart().await;
    let after_restart = recovery(&harness).await;
    assert_eq!(after_restart["event"]["attempts_used"], 1, "{after_restart}");
    assert_eq!(after_restart["event"]["attempts"].as_array().unwrap().len(), 1);
    assert_eq!(after_restart["event"]["status"], "observing", "正常重建不打斷觀察");

    // 觀察中又被殺掉：監護從鏡像與日誌算出這是第 2 次（不是第 3 次），並等退避
    kill_and_observe(&mut harness).await;
    let exited_at = now(&harness);
    let until = exited_at.plus_millis(30_000);
    assert_eq!(harness.guardian_step().await, GuardianStep::Waiting { until });
    harness.clock.advance_millis(29_999);
    assert_eq!(harness.guardian_step().await, GuardianStep::Waiting { until }, "退避未滿不啟動");
    assert!(!harness.runtime_running());
    harness.clock.advance_millis(1);
    let second = expect_restart(&mut harness).await;
    assert_eq!((second.event_id.as_str(), second.seq), (first.event_id.as_str(), 2), "沿用原事件");

    let event = recovery(&harness).await["event"].clone();
    assert_eq!((event["attempts_used"].as_i64(), event["status"].as_str()), (Some(2), Some("attempting")));
    let ids: Vec<&str> = event["attempts"].as_array().unwrap().iter().map(|a| a["id"].as_str().unwrap()).collect();
    assert_eq!(ids, [first.attempt_id.as_str(), second.attempt_id.as_str()]);
    assert_eq!(event["attempts"][0]["outcome"], "failed");
}

#[tokio::test]
async fn early_start_failures_are_counted_and_three_of_them_exhaust_the_round() {
    let mut harness = start().await;
    let (ws, goal) = workspace(&harness, "A").await;
    let (tid, _) = running_task(&harness, &ws, &goal).await;
    harness.start_guardian();
    let spawned_before = harness.process_control.spawn_count();

    kill_and_observe(&mut harness).await;
    harness.fail_next_guardian_start_early();
    let first = expect_restart(&mut harness).await;
    assert!(!harness.runtime_running(), "Runtime 在匯入監護日誌之前就結束了");
    assert_eq!(harness.guardian_step().await, GuardianStep::RuntimeExited);
    let GuardianStep::Waiting { until } = harness.guardian_step().await else { panic!("第二次前要退避 30 秒") };
    assert_eq!(until, now(&harness).plus_millis(30_000));
    harness.clock.advance_millis(30_000);

    harness.fail_next_guardian_start_early();
    let second = expect_restart(&mut harness).await;
    assert_eq!(second.seq, 2, "早期失敗的第一次也計次");
    assert_eq!(harness.guardian_step().await, GuardianStep::RuntimeExited);
    let GuardianStep::Waiting { until } = harness.guardian_step().await else { panic!("第三次前要退避 2 分鐘") };
    assert_eq!(until, now(&harness).plus_millis(120_000));
    harness.clock.advance_millis(120_000);

    harness.fail_next_guardian_start_early();
    assert_eq!(expect_restart(&mut harness).await.seq, 3);
    assert_eq!(harness.guardian_step().await, GuardianStep::RuntimeExited);
    harness.clock.advance_millis(600_000);
    assert_eq!(harness.guardian_step().await, GuardianStep::Finished(GuardianEnd::Exhausted), "三次用盡就不再啟動");
    assert_eq!(harness.guardian_starts().len(), 3);
    assert!(!harness.runtime_running());

    // 兩者都不在：等使用者開 App。Runtime 匯入三筆早期失敗，用盡、不派工、不產生第四次
    harness.open_app().await;
    harness.supervise().await;
    let status = recovery(&harness).await;
    let event = &status["event"];
    assert_eq!((event["status"].as_str(), event["attempts_used"].as_i64()), (Some("exhausted"), Some(3)), "{status}");
    assert_eq!(event["id"], first.event_id.as_str());
    assert_eq!(event["attempts"].as_array().unwrap().len(), 3);
    assert_eq!(status["blocking"]["code"], "crash_recovery_exhausted");
    assert_eq!(harness.process_control.spawn_count(), spawned_before, "用盡後不恢復工作");
    assert_eq!(runs_of(&harness, &ws, &tid).await.len(), 1);
    assert!(harness.process_control.stop_calls().is_empty(), "沒有任何一次嘗試真的動到程序");
}

// ============================================================ 完全退出、更新、解除安裝與競態

#[tokio::test]
async fn full_exit_suppresses_the_restart_and_the_guardian_ends() {
    let mut harness = start().await;
    let (ws, goal) = workspace(&harness, "A").await;
    running_task(&harness, &ws, &goal).await;
    harness.start_guardian();
    let spawned_before = harness.process_control.spawn_count();

    let (status, body) = harness.send_json(Method::POST, "/api/v1/lifecycle/exit", None).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    harness.wait_for_exit().await;
    harness.stop_runtime_after_exit().await;

    assert_eq!(harness.guardian_step().await, GuardianStep::RuntimeExited);
    assert_eq!(harness.guardian_step().await, GuardianStep::Finished(GuardianEnd::Suppressed(Suppression::FullExit)));
    assert!(!harness.runtime_running());
    assert!(harness.guardian_journal().is_empty(), "受抑制時什麼都不寫、不計次");
    assert!(harness.guardian_starts().is_empty());
    assert_eq!(harness.process_control.spawn_count(), spawned_before);
}

#[tokio::test]
async fn a_stop_intent_that_only_reached_the_database_keeps_the_guardian_started_runtime_from_serving() {
    let mut harness = start().await;
    let (ws, goal) = workspace(&harness, "A").await;
    let (_, rid) = running_task(&harness, &ws, &goal).await;
    harness.start_guardian();
    let spawned_before = harness.process_control.spawn_count();

    // 停止意圖寫進資料庫，但生命週期檔沒寫成（之後 Runtime 就被殺掉）：監護從檔案看不出已完全退出
    let blocker = harness.data_dir().join(format!("{LIFECYCLE_FILE_NAME}.tmp"));
    std::fs::create_dir(&blocker).unwrap();
    let (status, body) = harness.send_json(Method::POST, "/api/v1/lifecycle/exit", None).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    harness.wait_for_exit().await;
    assert!(!harness.lifecycle_file_path().exists());
    std::fs::remove_dir(&blocker).unwrap();
    kill_and_observe(&mut harness).await;

    // 監護啟動了程序，但 Runtime 看到資料庫的停止意圖：完全退出優先，不提供服務、不續跑工作，並把停止意圖補進檔案
    let attempt = expect_restart(&mut harness).await;
    let starts = harness.guardian_starts();
    assert_eq!(starts.len(), 1);
    assert_eq!(starts[0].attempt_id, attempt.attempt_id);
    assert!(starts[0].outcome.as_ref().is_err_and(|e| e.contains("停止意圖")), "{starts:?}");
    assert!(!harness.runtime_running());
    assert_eq!(harness.process_control.spawn_count(), spawned_before, "沒有任何 Worker 被拉起");

    // 監護接著從檔案看到停止意圖，不再重試
    assert_eq!(harness.guardian_step().await, GuardianStep::RuntimeExited);
    assert_eq!(harness.guardian_step().await, GuardianStep::Finished(GuardianEnd::Suppressed(Suppression::FullExit)));
    assert_eq!(harness.guardian_starts().len(), 1);

    // 使用者手動開 App 才依完全退出後的規則重新開啟；那次監護嘗試沒有計入任何恢復事件
    harness.open_app().await;
    harness.supervise().await;
    assert_eq!(recovery(&harness).await["event"], Value::Null);
    assert_ne!(run_view(&harness, &ws, &rid).await["status"], "running");
}

#[tokio::test]
async fn maintenance_that_appears_right_after_journaling_withdraws_the_attempt_and_nothing_starts() {
    let mut harness = start().await;
    harness.start_guardian();
    kill_and_observe(&mut harness).await;

    // 監護寫下嘗試之後、啟動之前，更新流程寫入維護抑制
    harness.on_next_runtime_probe(|dir| {
        let file = LifecycleFile {
            version: LIFECYCLE_FILE_VERSION,
            stop: None,
            maintenance: Some(MaintenanceSuppression { kind: "update".into(), requested_at: CLOCK_START }),
            updated_at: CLOCK_START,
        };
        std::fs::write(dir.join(LIFECYCLE_FILE_NAME), serde_json::to_vec(&file).unwrap()).unwrap();
    });
    assert_eq!(
        harness.guardian_step().await,
        GuardianStep::Finished(GuardianEnd::Suppressed(Suppression::Maintenance("update".into())))
    );
    assert!(!harness.runtime_running());
    assert!(harness.guardian_starts().is_empty(), "最後核對發現抑制就不啟動");
    let journal = harness.guardian_journal();
    let [JournalRecord::Attempt(attempt), JournalRecord::Withdrawn { attempt_id, .. }] = journal.as_slice() else {
        panic!("應先寫下嘗試、再寫下撤回：{journal:?}")
    };
    assert_eq!(&attempt.attempt_id, attempt_id);

    // 之後使用者手動開 App：撤回的嘗試不計次（Runtime 自己判定崩潰，尚未做任何嘗試）
    write_lifecycle_file(&harness, false, None);
    harness.open_app().await;
    let event = recovery(&harness).await["event"].clone();
    assert_eq!(event["attempts_used"], 0, "{event}");
    assert!(event["attempts"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn maintenance_during_backoff_also_stops_the_guardian() {
    let mut harness = start().await;
    let (ws, goal) = workspace(&harness, "A").await;
    running_task(&harness, &ws, &goal).await;
    harness.start_guardian();
    kill_and_observe(&mut harness).await;
    expect_restart(&mut harness).await;
    harness.supervise().await;
    kill_and_observe(&mut harness).await;
    assert!(matches!(harness.guardian_step().await, GuardianStep::Waiting { .. }));

    write_lifecycle_file(&harness, false, Some("uninstall"));
    harness.clock.advance_millis(30_000);
    assert_eq!(
        harness.guardian_step().await,
        GuardianStep::Finished(GuardianEnd::Suppressed(Suppression::Maintenance("uninstall".into())))
    );
    assert_eq!(harness.guardian_starts().len(), 1, "解除安裝期間不再拉起 Runtime");
    assert!(!harness.runtime_running());
}

// ============================================================ 監護降級

#[tokio::test]
async fn killing_the_guardian_keeps_work_running_shows_degradation_and_reopening_the_app_only_adds_the_guardian() {
    let mut harness = start().await;
    let (ws, goal) = workspace(&harness, "A").await;
    let (tid, _) = running_task(&harness, &ws, &goal).await;
    harness.start_guardian();
    // 先經歷一次由監護重啟的恢復，讓次數與觀察期有值，才看得出「不重設」
    kill_and_observe(&mut harness).await;
    expect_restart(&mut harness).await;
    harness.supervise().await;
    let before = recovery(&harness).await;
    assert_eq!(before["event"]["status"], "observing");
    let health_before: Value = harness.get_json(HEALTH).await;
    let stops_before = harness.process_control.stop_calls().len();
    let runs_before = runs_of(&harness, &ws, &tid).await;

    harness.kill_guardian();

    // 工作繼續：Run 照常、沒有任何停止或重派；UI 顯示降級
    assert_eq!(
        guardian_status(&harness).await,
        json!({ "state": "unavailable", "message": "工作繼續，自動重啟保護不可用" })
    );
    harness.supervise().await;
    harness.clock.advance_millis(60_000);
    harness.supervise().await;
    assert_eq!(runs_of(&harness, &ws, &tid).await, runs_before);
    assert_eq!(runs_before.last().unwrap()["status"], "running");
    assert_eq!(harness.process_control.stop_calls().len(), stops_before);

    // 重開 App：Runtime 還在，所以只補監護（Runtime 沒有重啟，次數、觀察期都不重設）
    harness.open_app().await;
    harness.start_guardian();
    let health_after: Value = harness.get_json(HEALTH).await;
    assert_eq!(health_after["boot_count"], health_before["boot_count"], "沒有重啟 Runtime");
    assert_eq!(health_after["guardian"]["state"], "active");
    let after = recovery(&harness).await;
    assert_eq!(after["event"], before["event"], "不重設次數、退避與阻擋");
    assert_eq!(harness.guardian_step().await, GuardianStep::Watching);

    // 補上的監護之後照常運作：再被殺掉就沿用同一個事件的計數
    tokio::time::timeout(Duration::from_secs(10), kill_and_observe(&mut harness)).await.unwrap();
    assert!(matches!(harness.guardian_step().await, GuardianStep::Waiting { .. }), "第 2 次前仍要退避");
}

#[tokio::test]
async fn when_both_are_gone_nothing_restarts_until_the_user_opens_the_app() {
    let mut harness = start().await;
    let (ws, goal) = workspace(&harness, "A").await;
    running_task(&harness, &ws, &goal).await;
    harness.start_guardian();
    harness.kill_guardian();
    harness.kill_runtime().await;
    harness.clock.advance_millis(3_600_000);
    assert!(!harness.runtime_running(), "沒有監護就沒有人自動重啟");

    harness.open_app().await;
    let status = recovery(&harness).await;
    assert_eq!(status["event"]["status"], "waiting", "開 App 才判定崩潰並開始恢復：{status}");
    assert_eq!(guardian_status(&harness).await["state"], "unavailable");
}
