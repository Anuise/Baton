//! 情境測試：備份與還原演練（票 40，spec D-18），經由 Runtime harness 的真實 HTTP API 與 fake Port 驗證。
//!
//! 備份由測試呼叫 `run_backups`（備份排程的一次處理）觸發，時間由 fake Clock 決定。
//! 「還原」以 `restore_backup` 把備份寫進一個新的 harness 的空資料目錄，再在那個目錄上啟動全新的 Runtime（新的一組 fake），
//! 所以「還原後有沒有重做外部操作」就是看新 Runtime 的 fake ProcessControl 與 AgentAdapter 有沒有被呼叫。
//! 憑證掃描直接讀備份目錄的每個位元組，因為「不含」只能從落地的內容證明。

use std::path::{Path, PathBuf};

use baton_domain::artifact::TOOL_REGISTER_ARTIFACT;
use baton_domain::auth::hash_token;
use baton_domain::Timestamp;
use baton_harness::Harness;
use baton_store::backup::restore_backup;
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

const WORKSPACES: &str = "/api/v1/workspaces";
const BACKUPS: &str = "/api/v1/backups";
const HOUR: i64 = 60 * 60 * 1000;
const DAY: i64 = 24 * HOUR;
/// 2026-10-19（星期一）09:00 UTC。
const CLOCK_START: Timestamp = Timestamp::from_millis(1_792_400_400_000);

// --- 共用輔助 ---

async fn harness() -> Harness {
    let harness = Harness::builder().clock_at(CLOCK_START).start().await;
    harness.add_account("主要帳號", "main@example.com").await;
    harness
}

async fn new_workspace(harness: &Harness) -> String {
    let dir = harness.data_dir().join("projects").join("main");
    std::fs::create_dir_all(&dir).unwrap();
    let (status, body) =
        harness.send_json(Method::POST, WORKSPACES, Some(json!({ "path": dir.to_string_lossy() }))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["id"].as_str().unwrap().to_string()
}

fn filled(text: &str) -> Value {
    json!({ "status": "filled", "text": text })
}

async fn ready_task(harness: &Harness, ws: &str, title: &str) -> String {
    let (status, goal) =
        harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/goals"), Some(json!({ "title": "目標" }))).await;
    assert_eq!(status, StatusCode::CREATED, "{goal}");
    let contract = json!({
        "objective": filled("整理資料"),
        "inputs": filled("既有的資料"),
        "scope": filled("公開資料"),
        "deliverables": filled("比較表"),
        "done_criteria": filled("五個產品"),
        "blocker_handling": filled("卡住時回報"),
        "required_capabilities": [],
    });
    let body = json!({ "goal_id": goal["id"], "title": title, "contract": contract });
    let (status, task) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks"), Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    let tid = task["id"].as_str().unwrap().to_string();
    let (status, ready) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks/{tid}/ready"), None).await;
    assert_eq!(status, StatusCode::OK, "{ready}");
    tid
}

/// 派工；回傳 run id（還沒被 Supervisor 啟動）。
async fn dispatch(harness: &Harness, ws: &str, tid: &str) -> String {
    let (status, run) =
        harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks/{tid}/dispatch"), None).await;
    assert_eq!(status, StatusCode::CREATED, "{run}");
    run["id"].as_str().unwrap().to_string()
}

async fn register(harness: &Harness, token: &str, name: &str, content: &str) -> Value {
    let (status, body) =
        harness.call_tool(token, TOOL_REGISTER_ARTIFACT, json!({ "name": name, "content": content })).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body
}

async fn artifacts(harness: &Harness, ws: &str) -> Value {
    let (status, body) = harness.send_json(Method::GET, &format!("{WORKSPACES}/{ws}/artifacts"), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

async fn download(harness: &Harness, ws: &str, artifact_id: &str, version: i64) -> (StatusCode, Vec<u8>) {
    let path = format!("{WORKSPACES}/{ws}/artifacts/{artifact_id}/versions/{version}/content");
    let response = harness
        .request(Method::GET, &path)
        .bearer_auth(harness.desktop_credential())
        .header(baton_api::CLIENT_HEADER, baton_api::CLIENT_HEADER_VALUE)
        .send()
        .await
        .unwrap();
    (response.status(), response.bytes().await.unwrap().to_vec())
}

async fn backup_status(harness: &Harness) -> Value {
    let (status, body) = harness.send_json(Method::GET, BACKUPS, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

async fn runs(harness: &Harness, ws: &str) -> Vec<Value> {
    let (status, body) = harness.send_json(Method::GET, &format!("{WORKSPACES}/{ws}/runs"), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["runs"].as_array().unwrap().clone()
}

async fn task_status(harness: &Harness, ws: &str, tid: &str) -> String {
    let (status, body) = harness.send_json(Method::GET, &format!("{WORKSPACES}/{ws}/tasks/{tid}"), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["task"]["status"].as_str().unwrap().to_string()
}

/// 最新一份備份的目錄。
async fn latest_backup_dir(harness: &Harness) -> PathBuf {
    let status = backup_status(harness).await;
    harness.backups_dir().join(status["backups"][0]["id"].as_str().expect("應該至少有一份備份"))
}

/// 把備份還原到一個新 harness 的空資料目錄，並在其上啟動全新的 Runtime（新的一組 fake）。
async fn restored_from(backup_dir: PathBuf, now: Timestamp) -> Harness {
    Harness::builder()
        .clock_at(now)
        .prepare_data_dir(move |dir| {
            let report = restore_backup(&backup_dir, dir, now).expect("還原失敗");
            assert!(report.passed(), "還原的核對沒有通過：{:?}", report.checks);
        })
        .start()
        .await
}

/// 目錄下全部檔案的相對路徑與內容。
fn files_under(root: &Path) -> Vec<(String, Vec<u8>)> {
    fn walk(root: &Path, dir: &Path, found: &mut Vec<(String, Vec<u8>)>) {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(root, &path, found);
            } else {
                let relative = path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
                found.push((relative, std::fs::read(&path).unwrap()));
            }
        }
    }
    let mut found = Vec::new();
    walk(root, root, &mut found);
    found
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

// ============================================================ 還原演練

#[tokio::test]
async fn restore_drill_in_a_clean_directory_matches_the_data_and_artifact_hashes() {
    let harness = harness().await;
    let ws = new_workspace(&harness).await;
    let tid = ready_task(&harness, &ws, "整理").await;
    let rid = dispatch(&harness, &ws, &tid).await;
    harness.supervise().await;
    let token = harness.agent_credential(&rid).unwrap();
    register(&harness, &token, "report.md", "第一版").await;
    register(&harness, &token, "report.md", "第二版：內容更長").await;
    register(&harness, &token, "data.csv", "a,b\n1,2\n").await;

    harness.run_backups().await;

    // 每週的還原演練（第一次在第一份備份之後立刻進行）在隔離目錄還原並核對，結果顯示在設定頁
    let status = backup_status(&harness).await;
    assert_eq!(status["backups"].as_array().unwrap().len(), 1);
    assert_eq!(status["backups"][0]["trigger"], "daily");
    assert_eq!(status["backups"][0]["artifact_count"], 3);
    let drill = &status["latest_drill"];
    assert_eq!(drill["passed"], true, "{drill}");
    assert_eq!(drill["backup_id"], status["backups"][0]["id"]);
    let kinds: Vec<&str> = drill["checks"].as_array().unwrap().iter().map(|c| c["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["restore", "database_integrity", "foreign_keys", "artifact_hashes", "artifact_relations"]);
    assert!(drill["checks"].as_array().unwrap().iter().all(|c| c["passed"] == true), "{drill}");

    // 實際還原到乾淨目錄並啟動 Runtime：資料與每個成果版本的內容、雜湊都與原本一致
    let original = artifacts(&harness, &ws).await;
    let restored = restored_from(latest_backup_dir(&harness).await, CLOCK_START.plus_millis(HOUR)).await;
    let after = artifacts(&restored, &ws).await;
    assert_eq!(after["artifacts"], original["artifacts"]);
    assert_eq!(after["unregistered_files"], json!([]));
    for artifact in original["artifacts"].as_array().unwrap() {
        let id = artifact["id"].as_str().unwrap();
        for version in artifact["versions"].as_array().unwrap() {
            let v = version["version"].as_i64().unwrap();
            assert_eq!(version["file_status"], "ok");
            let (status, bytes) = download(&restored, &ws, id, v).await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(bytes, download(&harness, &ws, id, v).await.1);
            assert_eq!(baton_domain::artifact::sha256_hex(&bytes), version["sha256"].as_str().unwrap());
        }
    }
    assert_eq!(task_status(&restored, &ws, &tid).await, "ready");
}

#[tokio::test]
async fn a_damaged_artifact_fails_the_drill_and_the_settings_page_shows_it() {
    let harness = harness().await;
    let ws = new_workspace(&harness).await;
    let tid = ready_task(&harness, &ws, "整理").await;
    let rid = dispatch(&harness, &ws, &tid).await;
    harness.supervise().await;
    let token = harness.agent_credential(&rid).unwrap();
    register(&harness, &token, "report.md", "abc").await;
    let listing = artifacts(&harness, &ws).await;
    let storage_path = listing["artifacts"][0]["versions"][0]["storage_path"].as_str().unwrap();
    // 成果檔案在備份前就被改壞了（大小相同、內容不同）
    std::fs::write(harness.artifacts_dir().join(storage_path), b"xyz").unwrap();

    harness.run_backups().await;

    let status = backup_status(&harness).await;
    let drill = &status["latest_drill"];
    assert_eq!(drill["passed"], false, "{drill}");
    let failed: Vec<&Value> = drill["checks"].as_array().unwrap().iter().filter(|c| c["passed"] == false).collect();
    assert_eq!(failed.len(), 1, "{drill}");
    assert_eq!(failed[0]["kind"], "artifact_hashes");
    assert!(failed[0]["detail"].as_str().unwrap().contains(storage_path), "{drill}");
}

#[tokio::test]
async fn the_drill_runs_again_every_week() {
    let harness = harness().await;
    harness.run_backups().await;
    let first = backup_status(&harness).await["latest_drill"]["performed_at"].clone();
    assert_eq!(first, CLOCK_START.as_millis());

    for day in 1..7 {
        harness.clock.set(CLOCK_START.plus_millis(day * DAY));
        harness.run_backups().await;
        assert_eq!(backup_status(&harness).await["latest_drill"]["performed_at"], first, "第 {day} 天還不到一週");
    }
    harness.clock.set(CLOCK_START.plus_millis(7 * DAY));
    harness.run_backups().await;
    let status = backup_status(&harness).await;
    assert_eq!(status["latest_drill"]["performed_at"], CLOCK_START.plus_millis(7 * DAY).as_millis());
    assert_eq!(status["latest_drill"]["backup_id"], status["backups"][0]["id"], "演練的是最新的備份");
    assert_eq!(status["latest_drill"]["passed"], true);
}

// ============================================================ 備份不含憑證與登入資料

#[tokio::test]
async fn backups_contain_no_credentials_or_native_login_data() {
    let harness = Harness::builder().clock_at(CLOCK_START).start().await;
    let (status, profile) =
        harness.send_json(Method::POST, "/api/v1/profiles", Some(json!({ "name": "主要帳號" }))).await;
    assert_eq!(status, StatusCode::CREATED, "{profile}");
    let config_dir = PathBuf::from(profile["config_dir"].as_str().unwrap());
    let login_secret = harness.claude_env.log_in(&config_dir, "main@example.com");
    let (status, verified) = harness
        .send_json(Method::POST, &format!("/api/v1/profiles/{}/verify", profile["id"].as_str().unwrap()), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{verified}");
    let ws = new_workspace(&harness).await;
    let tid = ready_task(&harness, &ws, "整理").await;
    let rid = dispatch(&harness, &ws, &tid).await;
    harness.supervise().await;
    let agent_token = harness.agent_credential(&rid).unwrap();
    register(&harness, &agent_token, "report.md", "成果內容").await;
    let desktop_token = harness.desktop_credential();
    let credential_file = std::fs::read(harness.data_dir().join("auth").join("desktop-credential.bin")).unwrap();
    let native_login = std::fs::read(config_dir.join(".credentials.json")).unwrap();

    harness.run_backups().await;
    harness.request_milestone_backup("重要裁定").await;
    harness.clock.set(CLOCK_START.plus_millis(HOUR));
    harness.run_backups().await;

    let status = backup_status(&harness).await;
    assert_eq!(status["backups"][0]["trigger"], "milestone");
    let files = files_under(&harness.backups_dir());
    assert!(files.iter().any(|(path, _)| path.ends_with("control.sqlite")), "{:?}", files.iter().map(|f| &f.0).collect::<Vec<_>>());
    let forbidden: Vec<(&str, Vec<u8>)> = vec![
        ("原生登入的 token", login_secret.into_bytes()),
        ("原生登入檔案的內容", native_login),
        ("桌面憑證", desktop_token.clone().into_bytes()),
        ("桌面憑證的雜湊", hash_token(&desktop_token).into_bytes()),
        ("桌面憑證檔的內容", credential_file),
        ("Agent 憑證", agent_token.clone().into_bytes()),
        ("Agent 憑證的雜湊", hash_token(&agent_token).into_bytes()),
    ];
    for (path, bytes) in &files {
        for (what, needle) in &forbidden {
            assert!(!contains(bytes, needle), "備份檔案 {path} 含有{what}");
        }
        let lower = path.to_lowercase();
        assert!(!lower.contains("credential") && !lower.contains("profiles/") && !lower.contains("auth/"), "備份不該包含 {path}");
    }
    // 備份照樣完整：還原出來的成果與原本相同
    assert_eq!(status["latest_drill"]["passed"], true, "{}", status["latest_drill"]);
}

// ============================================================ 還原後不重放外部操作

#[tokio::test]
async fn after_restore_unfinished_runs_are_not_relaunched_or_resumed() {
    let harness = harness().await;
    let ws = new_workspace(&harness).await;
    let started_task = ready_task(&harness, &ws, "已啟動").await;
    let queued_task = ready_task(&harness, &ws, "尚未啟動").await;
    let started = dispatch(&harness, &ws, &started_task).await;
    harness.supervise().await;
    let agent_token = harness.agent_credential(&started).unwrap();
    // 第二個 Run 已派工、在 Outbox 等待啟動，但還沒被 Supervisor 處理
    let queued = dispatch(&harness, &ws, &queued_task).await;
    assert_eq!(harness.process_control.spawn_count(), 1);

    harness.run_backups().await;
    let restored = restored_from(latest_backup_dir(&harness).await, CLOCK_START.plus_millis(HOUR)).await;
    for _ in 0..3 {
        restored.supervise().await;
        restored.clock.advance_millis(5 * 60 * 1000);
    }

    // 新的 Runtime 沒有啟動任何程序、沒有接上任何事件串流
    assert_eq!(restored.process_control.spawn_count(), 0);
    assert!(restored.agent_adapter.launched().is_empty());
    assert_eq!(restored.process_control.inspect_count(), 0);
    // 備份當下尚未結束的 Run 都已結束：已啟動的標為中斷、還沒啟動的取消；Task 回到 ready 等使用者重新派工
    let after = runs(&restored, &ws).await;
    let run = |id: &str| after.iter().find(|r| r["id"] == id).unwrap().clone();
    assert_eq!(run(&started)["status"], "interrupted");
    assert!(run(&started)["end_reason"].as_str().unwrap().starts_with("restored_from_backup"), "{}", run(&started));
    assert_eq!(run(&queued)["status"], "cancelled");
    assert_eq!(task_status(&restored, &ws, &started_task).await, "ready");
    assert_eq!(task_status(&restored, &ws, &queued_task).await, "ready");
    // 舊 Run 的 Agent 憑證不在備份裡，對還原後的 Runtime 無效
    let (status, _) = restored.call_tool(&agent_token, TOOL_REGISTER_ARTIFACT, json!({ "name": "x", "content": "y" })).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    // 原本的 Runtime 不受影響
    assert_eq!(runs(&harness, &ws).await.iter().find(|r| r["id"] == started.as_str()).unwrap()["status"], "running");
}

#[tokio::test]
async fn the_drill_reports_how_many_unfinished_runs_a_restore_ends() {
    let harness = harness().await;
    let ws = new_workspace(&harness).await;
    let tid = ready_task(&harness, &ws, "整理").await;
    dispatch(&harness, &ws, &tid).await;
    harness.supervise().await;

    harness.run_backups().await;

    let drill = backup_status(&harness).await["latest_drill"].clone();
    assert_eq!(drill["passed"], true, "{drill}");
    assert_eq!(drill["interrupted_runs"], 1);
    // 演練只在隔離目錄進行，原本的 Run 照常執行
    assert_eq!(runs(&harness, &ws).await[0]["status"], "running");
}

// ============================================================ 觸發與保留

#[tokio::test]
async fn backups_are_taken_daily_and_after_milestones_at_most_once_an_hour() {
    let harness = harness().await;
    let at = |millis: i64| CLOCK_START.plus_millis(millis);
    // 最新一份備份的（觸發原因, 建立時間）；同一天較早的備份會被保留規則取代，所以看最新的那份
    let latest = || async {
        let status = backup_status(&harness).await;
        let newest = &status["backups"][0];
        (newest["trigger"].as_str().unwrap_or_default().to_string(), newest["created_at"].as_i64().unwrap_or_default())
    };

    harness.run_backups().await;
    assert_eq!(latest().await, ("daily".into(), at(0).as_millis()), "第一次就建立每日備份");
    harness.clock.set(at(HOUR));
    harness.run_backups().await;
    assert_eq!(latest().await, ("daily".into(), at(0).as_millis()), "沒有要求時不到 24 小時不再備份");

    // 距離上一份備份不到一小時：重要裁定與 Stage 完成的要求先等待，滿一小時後合併成一份
    harness.clock.set(at(HOUR + 10 * 60 * 1000));
    harness.request_milestone_backup("重要裁定").await;
    harness.run_backups().await;
    assert_eq!(latest().await.1, at(HOUR + 10 * 60 * 1000).as_millis(), "距離上一份已滿一小時：立刻備份");
    harness.clock.set(at(HOUR + 20 * 60 * 1000));
    harness.request_milestone_backup("Stage 完成").await;
    harness.run_backups().await;
    harness.clock.set(at(HOUR + 50 * 60 * 1000));
    harness.request_milestone_backup("重要裁定").await;
    harness.run_backups().await;
    assert_eq!(latest().await.1, at(HOUR + 10 * 60 * 1000).as_millis(), "同一小時內的要求還在等待");
    harness.clock.set(at(2 * HOUR + 10 * 60 * 1000));
    harness.run_backups().await;
    assert_eq!(latest().await, ("milestone".into(), at(2 * HOUR + 10 * 60 * 1000).as_millis()), "兩個要求合併成一份");
    harness.clock.set(at(4 * HOUR));
    harness.run_backups().await;
    assert_eq!(latest().await.1, at(2 * HOUR + 10 * 60 * 1000).as_millis(), "已涵蓋的要求不會再備份一次");

    // 每日：距離上一份備份滿 24 小時
    let last = 2 * HOUR + 10 * 60 * 1000;
    harness.clock.set(at(last + DAY - 1));
    harness.run_backups().await;
    assert_eq!(latest().await.1, at(last).as_millis());
    harness.clock.set(at(last + DAY));
    harness.run_backups().await;
    assert_eq!(latest().await, ("daily".into(), at(last + DAY).as_millis()));
}

#[tokio::test]
async fn retention_keeps_seven_daily_and_four_weekly_backups() {
    let harness = harness().await;
    for day in 0..40 {
        harness.clock.set(CLOCK_START.plus_millis(day * DAY));
        harness.run_backups().await;
    }

    let status = backup_status(&harness).await;
    let backups = status["backups"].as_array().unwrap();
    let with = |reason: &str| backups.iter().filter(|b| b["retained_as"].as_array().unwrap().iter().any(|r| r == reason)).count();
    assert_eq!(with("daily"), 7);
    assert_eq!(with("weekly"), 4);
    // 每週保留的四份中，最近兩週的那兩份同時也是每日備份
    assert_eq!(backups.len(), 9);
    assert!(backups.iter().all(|b| !b["retained_as"].as_array().unwrap().is_empty()));
    let on_disk = std::fs::read_dir(harness.backups_dir())
        .unwrap()
        .flatten()
        .filter(|e| e.path().join("manifest.json").exists())
        .count();
    assert_eq!(on_disk, 9, "沒有被保留的備份已從磁碟刪除");
}
