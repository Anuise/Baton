//! 情境測試：Workspace 的建立、列出、封存（票 05），經由 Runtime harness 的真實 HTTP API 驗證，
//! 以「丟棄 Runtime、保留資料目錄後重建」確認資料持久化。

use baton_api::{ErrorBody, CLIENT_HEADER, CLIENT_HEADER_VALUE};
use baton_domain::auth::Role;
use baton_domain::Timestamp;
use baton_harness::{FakePorts, Harness};
use baton_runtime::{Runtime, RuntimeConfig};
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

const WORKSPACES: &str = "/api/v1/workspaces";

/// 2023-11-14 22:13:20.123 UTC
const CLOCK_START: Timestamp = Timestamp::from_millis(1_700_000_000_123);

fn path_str(path: &std::path::Path) -> String {
    path.to_string_lossy().into_owned()
}

/// 建立一個「不是 Git repo」的真實目錄。
fn plain_dir(harness: &Harness, name: &str) -> String {
    let dir = harness.data_dir().join("projects").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    assert!(!dir.join(".git").exists());
    path_str(&dir)
}

async fn create(harness: &Harness, body: Value) -> (StatusCode, Value) {
    harness.send_json(Method::POST, WORKSPACES, Some(body)).await
}

async fn list(harness: &Harness) -> Vec<Value> {
    let (status, body) = harness.send_json(Method::GET, WORKSPACES, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["workspaces"].as_array().expect("workspaces 必須是陣列").clone()
}

fn event_rows(harness: &Harness) -> Vec<(String, String, i64, i64)> {
    let db = harness.db();
    let mut statement = db
        .prepare("SELECT event_type, aggregate_id, aggregate_version, occurred_at FROM event ORDER BY seq")
        .unwrap();
    statement.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap().map(Result::unwrap).collect()
}

fn error_code(body: &Value) -> &str {
    body["error"].as_str().unwrap_or_default()
}

#[tokio::test]
async fn no_workspaces_exist_in_a_fresh_runtime() {
    let harness = Harness::start().await;

    assert!(list(&harness).await.is_empty());
}

#[tokio::test]
async fn creating_a_workspace_on_a_plain_directory_lists_it() {
    let harness = Harness::builder().clock_at(CLOCK_START).start().await;
    let dir = plain_dir(&harness, "notes");

    let (status, created) = create(&harness, json!({ "path": dir })).await;

    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["path"], dir);
    assert_eq!(created["name"], "notes", "名稱預設取目錄名稱");
    assert_eq!(created["status"], "active");
    assert!(created["id"].as_str().is_some_and(|id| !id.is_empty()));
    assert_eq!(list(&harness).await, vec![created]);
}

#[tokio::test]
async fn explicit_name_is_used() {
    let harness = Harness::start().await;
    let dir = plain_dir(&harness, "docs");

    let (_, created) = create(&harness, json!({ "path": dir, "name": "  研究文件  " })).await;

    assert_eq!(created["name"], "研究文件");
}

#[tokio::test]
async fn workspaces_survive_rebuilding_the_runtime() {
    let mut harness = Harness::builder().clock_at(CLOCK_START).start().await;
    let (_, first) = create(&harness, json!({ "path": plain_dir(&harness, "a") })).await;
    let (_, second) = create(&harness, json!({ "path": plain_dir(&harness, "b") })).await;
    let id = second["id"].as_str().unwrap().to_string();
    harness.send_json(Method::POST, &format!("{WORKSPACES}/{id}/archive"), None).await;
    let before = list(&harness).await;

    harness.restart().await;

    let after = list(&harness).await;
    assert_eq!(after, before);
    assert_eq!(after.len(), 2);
    assert_eq!(after[0]["id"], first["id"]);
    assert_eq!(after[1]["status"], "archived");
}

#[tokio::test]
async fn archiving_marks_the_workspace_archived_and_keeps_it_listed() {
    let harness = Harness::builder().clock_at(CLOCK_START).start().await;
    let (_, created) = create(&harness, json!({ "path": plain_dir(&harness, "old") })).await;
    harness.clock.advance_millis(5_000);

    let id = created["id"].as_str().unwrap();
    let (status, archived) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{id}/archive"), None).await;

    assert_eq!(status, StatusCode::OK, "{archived}");
    assert_eq!(archived["status"], "archived");
    assert_eq!(archived["archived_at"], CLOCK_START.as_millis() + 5_000);
    assert_eq!(archived["created_at"], CLOCK_START.as_millis());
    assert_eq!(list(&harness).await, vec![archived]);
}

#[tokio::test]
async fn archiving_an_unknown_or_already_archived_workspace_is_a_clear_error() {
    let harness = Harness::start().await;
    let (_, created) = create(&harness, json!({ "path": plain_dir(&harness, "x") })).await;
    let id = created["id"].as_str().unwrap();
    let archive_path = format!("{WORKSPACES}/{id}/archive");
    harness.send_json(Method::POST, &archive_path, None).await;

    let (again_status, again) = harness.send_json(Method::POST, &archive_path, None).await;
    let (unknown_status, unknown) =
        harness.send_json(Method::POST, &format!("{WORKSPACES}/ws_nope/archive"), None).await;

    assert_eq!((again_status, error_code(&again)), (StatusCode::CONFLICT, "workspace_already_archived"));
    assert_eq!((unknown_status, error_code(&unknown)), (StatusCode::NOT_FOUND, "workspace_not_found"));
    assert_eq!(event_rows(&harness).len(), 2, "失敗的命令不可追加事件");
}

#[tokio::test]
async fn every_state_change_appends_an_event_with_utc_millis() {
    let harness = Harness::builder().clock_at(CLOCK_START).start().await;
    let (_, created) = create(&harness, json!({ "path": plain_dir(&harness, "ev") })).await;
    let id = created["id"].as_str().unwrap().to_string();
    harness.clock.advance_millis(1_000);
    harness.send_json(Method::POST, &format!("{WORKSPACES}/{id}/archive"), None).await;

    let events = event_rows(&harness);

    assert_eq!(
        events,
        vec![
            ("workspace_created".to_string(), id.clone(), 1, CLOCK_START.as_millis()),
            ("workspace_archived".to_string(), id.clone(), 2, CLOCK_START.as_millis() + 1_000),
        ]
    );
}

#[tokio::test]
async fn timestamps_are_stored_and_returned_as_utc_epoch_millis() {
    let harness = Harness::builder().clock_at(CLOCK_START).start().await;

    let (_, created) = create(&harness, json!({ "path": plain_dir(&harness, "utc") })).await;

    // API 回傳純數字（UTC 毫秒），不帶任何時區；轉成系統時區是 UI 的工作（spec D-33）
    assert_eq!(created["created_at"], json!(1_700_000_000_123_i64));
    let stored: i64 = harness.db().query_row("SELECT created_at FROM workspace", [], |r| r.get(0)).unwrap();
    assert_eq!(stored, 1_700_000_000_123);
}

#[tokio::test]
async fn path_must_be_an_existing_directory_and_nothing_is_written_otherwise() {
    let harness = Harness::start().await;
    let missing = path_str(&harness.data_dir().join("no-such-dir"));
    let file = harness.data_dir().join("a-file.txt");
    std::fs::write(&file, "x").unwrap();

    let (missing_status, missing_body) = create(&harness, json!({ "path": missing })).await;
    let (file_status, file_body) = create(&harness, json!({ "path": path_str(&file) })).await;
    let (relative_status, relative_body) = create(&harness, json!({ "path": "notes" })).await;
    let (empty_status, empty_body) = create(&harness, json!({ "path": "   " })).await;

    assert_eq!((missing_status, error_code(&missing_body)), (StatusCode::BAD_REQUEST, "workspace_path_not_found"));
    assert_eq!((file_status, error_code(&file_body)), (StatusCode::BAD_REQUEST, "workspace_path_not_directory"));
    assert_eq!((relative_status, error_code(&relative_body)), (StatusCode::BAD_REQUEST, "workspace_path_invalid"));
    assert_eq!((empty_status, error_code(&empty_body)), (StatusCode::BAD_REQUEST, "workspace_path_invalid"));
    assert!(missing_body["message"].as_str().unwrap().contains("no-such-dir"), "訊息要指出是哪個路徑");
    assert!(list(&harness).await.is_empty());
    assert!(event_rows(&harness).is_empty());
}

#[tokio::test]
async fn invalid_name_and_malformed_requests_are_rejected_with_a_stable_code() {
    let harness = Harness::start().await;
    let dir = plain_dir(&harness, "n");

    let (long_status, long) = create(&harness, json!({ "path": dir, "name": "x".repeat(101) })).await;
    let (missing_status, missing) = create(&harness, json!({ "name": "沒有路徑" })).await;
    let raw = harness
        .request(Method::POST, WORKSPACES)
        .bearer_auth(harness.desktop_credential())
        .header(CLIENT_HEADER, CLIENT_HEADER_VALUE)
        .header("content-type", "application/json")
        .body("{ not json")
        .send()
        .await
        .unwrap();

    assert_eq!((long_status, error_code(&long)), (StatusCode::BAD_REQUEST, "workspace_name_invalid"));
    assert_eq!((missing_status, error_code(&missing)), (StatusCode::BAD_REQUEST, "bad_request"));
    assert_eq!(raw.status(), StatusCode::BAD_REQUEST);
    assert_eq!(raw.json::<ErrorBody>().await.unwrap().error, "bad_request");
    assert!(list(&harness).await.is_empty());
}

#[tokio::test]
async fn the_same_directory_cannot_be_registered_twice_until_the_first_is_archived() {
    let harness = Harness::start().await;
    let dir = plain_dir(&harness, "dup");
    let (_, first) = create(&harness, json!({ "path": dir })).await;

    let (status, body) = create(&harness, json!({ "path": dir.to_uppercase().replace('\\', "/") })).await;
    assert_eq!((status, error_code(&body)), (StatusCode::CONFLICT, "workspace_already_exists"));
    assert_eq!(event_rows(&harness).len(), 1);

    let id = first["id"].as_str().unwrap();
    harness.send_json(Method::POST, &format!("{WORKSPACES}/{id}/archive"), None).await;
    let (again_status, again) = create(&harness, json!({ "path": dir })).await;

    assert_eq!(again_status, StatusCode::CREATED, "{again}");
    assert_ne!(again["id"], first["id"]);
}

#[tokio::test]
async fn simultaneous_requests_for_the_same_directory_produce_exactly_one_workspace() {
    let harness = Harness::start().await;
    let dir = plain_dir(&harness, "race");

    let requests = (0..8).map(|_| create(&harness, json!({ "path": dir })));
    let results = futures::future::join_all(requests).await;

    let created = results.iter().filter(|(s, _)| *s == StatusCode::CREATED).count();
    let conflicts = results.iter().filter(|(s, _)| *s == StatusCode::CONFLICT).count();
    assert_eq!((created, conflicts), (1, 7), "{results:?}");
    assert_eq!(list(&harness).await.len(), 1);
    assert_eq!(event_rows(&harness).len(), 1);
}

#[tokio::test]
async fn workspace_endpoints_require_a_desktop_credential() {
    let harness = Harness::start().await;
    let dir = plain_dir(&harness, "auth");
    let agent_token = harness.issue_credential(Role::Agent, None).await;

    let anonymous = harness
        .request(Method::POST, WORKSPACES)
        .header(CLIENT_HEADER, CLIENT_HEADER_VALUE)
        .json(&json!({ "path": dir }))
        .send()
        .await
        .unwrap();
    let agent = harness
        .request(Method::GET, WORKSPACES)
        .bearer_auth(agent_token)
        .header(CLIENT_HEADER, CLIENT_HEADER_VALUE)
        .send()
        .await
        .unwrap();

    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(agent.status(), StatusCode::FORBIDDEN);
    assert!(list(&harness).await.is_empty());
}

#[tokio::test]
async fn control_database_uses_wal_and_records_migrations_after_startup() {
    let harness = Harness::builder().clock_at(CLOCK_START).start().await;

    let db = harness.db();
    let mode: String = db.pragma_query_value(None, "journal_mode", |r| r.get(0)).unwrap();
    let applied: Vec<i64> = db
        .prepare("SELECT applied_at FROM schema_migrations ORDER BY version")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();

    assert_eq!(mode, "wal");
    assert!(!applied.is_empty() && applied.iter().all(|t| *t == CLOCK_START.as_millis()), "{applied:?}");
}

#[tokio::test]
async fn runtime_refuses_to_start_on_a_database_from_a_newer_version_and_leaves_it_untouched() {
    let result = Harness::builder()
        .prepare_data_dir(|dir| {
            let db = rusqlite::Connection::open(dir.join(baton_store::DB_FILE_NAME)).unwrap();
            db.execute_batch(
                "CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, name TEXT NOT NULL, applied_at INTEGER NOT NULL);
                 INSERT INTO schema_migrations VALUES (999, 'future', 1);",
            )
            .unwrap();
        })
        .try_start()
        .await;

    let message = result.err().expect("較新的資料庫必須拒絕啟動").to_string();
    assert!(message.contains("999") && message.contains("拒絕寫入"), "{message}");
}

#[tokio::test]
async fn runtime_refuses_data_dirs_on_network_shares_or_cloud_sync_folders_and_explains_why() {
    let sandbox = tempfile::tempdir().unwrap();
    let cases = [
        ("\\\\fileserver\\share\\baton".to_string(), "網路共享"),
        (path_str(&sandbox.path().join("OneDrive").join("Baton")), "OneDrive"),
        (path_str(&sandbox.path().join("Dropbox").join("Baton")), "Dropbox"),
    ];
    for (data_dir, expected) in cases {
        let fakes = FakePorts::new(CLOCK_START);
        let config = RuntimeConfig { data_dir: data_dir.clone().into(), bind: "127.0.0.1:0".parse().unwrap() };

        let error = Runtime::start(config, fakes.to_ports()).await.err().expect("必須拒絕啟動");

        let message = error.to_string();
        assert!(message.contains(expected) && message.contains(&data_dir) && message.contains("本機磁碟"), "{message}");
    }
    assert_eq!(std::fs::read_dir(sandbox.path()).unwrap().count(), 0, "被拒絕時不可建立任何目錄或檔案");
}
