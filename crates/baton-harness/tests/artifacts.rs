//! 情境測試：Artifact 儲存與成果瀏覽（票 12），經由 Runtime harness 的真實 HTTP API 驗證。
//!
//! Worker 以 Agent 憑證呼叫 `register_artifact`（與 MCP shim 送出的請求相同），桌面以桌面憑證瀏覽、預覽、下載。
//! 中途中斷以 fake WorkspaceFs 的崩潰點模擬：請求在該點永遠不回來，測試以逾時丟棄它，再 `restart()` 重建 Runtime。
//! 除了 HTTP 行為，這裡也直接看成果儲存目錄與控制資料庫（`harness.artifacts_dir()`、`harness.db()`），
//! 因為「檔案先落地、manifest 後提交」「雜湊與檔案一致」「manifest 不可竄改」都是必須看落地內容的事實。

use std::path::PathBuf;
use std::time::Duration;

use baton_domain::agent::TOOL_GET_TASK_CONTRACT;
use baton_domain::artifact::{PREVIEW_MAX_BYTES, TOOL_REGISTER_ARTIFACT};
use baton_domain::auth::Role;
use baton_domain::Timestamp;
use baton_harness::fakes::ArtifactWritePoint;
use baton_harness::Harness;
use baton_mcp_shim::{serve, ShimConfig};
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};
use tokio::io::{duplex, AsyncBufReadExt, AsyncWriteExt, BufReader};

const WORKSPACES: &str = "/api/v1/workspaces";
/// SHA-256("abc")，獨立於程式碼之外的已知答案。
const SHA256_ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
const CLOCK_START: Timestamp = Timestamp::from_millis(1_700_000_000_123);

// --- 共用輔助 ---

async fn harness() -> Harness {
    Harness::builder().clock_at(CLOCK_START).start().await
}

async fn new_workspace(harness: &Harness, name: &str) -> String {
    let dir = harness.data_dir().join("projects").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let body = json!({ "path": dir.to_string_lossy() });
    let (status, body) = harness.send_json(Method::POST, WORKSPACES, Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["id"].as_str().unwrap().to_string()
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

/// 在 Workspace 內建立一個 ready 的 Task，回傳 task id。
async fn ready_task(harness: &Harness, ws: &str, title: &str) -> String {
    let (status, goal) =
        harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/goals"), Some(json!({ "title": "目標" }))).await;
    assert_eq!(status, StatusCode::CREATED, "{goal}");
    let body = json!({ "goal_id": goal["id"], "title": title, "contract": contract() });
    let (status, task) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks"), Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    let tid = task["id"].as_str().unwrap().to_string();
    let (status, ready) =
        harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks/{tid}/ready"), None).await;
    assert_eq!(status, StatusCode::OK, "{ready}");
    tid
}

/// 派工並讓 Supervisor 啟動它，回傳 (run_id, Worker 的 Agent 憑證)。
async fn start_run(harness: &Harness, ws: &str, tid: &str) -> (String, String) {
    let (status, run) =
        harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks/{tid}/dispatch"), None).await;
    assert_eq!(status, StatusCode::CREATED, "{run}");
    let rid = run["id"].as_str().unwrap().to_string();
    harness.supervise().await;
    let token = harness.agent_credential(&rid).expect("Run 啟動時應該把 Agent 憑證放進程序的環境變數");
    (rid, token)
}

/// 一個 Workspace、一個進行中的 Run：(workspace_id, task_id, run_id, Agent 憑證)。
async fn running(harness: &Harness) -> (String, String, String, String) {
    let ws = new_workspace(harness, "main").await;
    let tid = ready_task(harness, &ws, "整理").await;
    let (rid, token) = start_run(harness, &ws, &tid).await;
    (ws, tid, rid, token)
}

fn error_code(body: &Value) -> &str {
    body["error"].as_str().unwrap_or_default()
}

async fn register(harness: &Harness, token: &str, body: Value) -> (StatusCode, Value) {
    harness.call_tool(token, TOOL_REGISTER_ARTIFACT, body).await
}

/// 登記一個文字成果並預期成功。
async fn register_ok(harness: &Harness, token: &str, name: &str, content: &str) -> Value {
    let (status, body) = register(harness, token, json!({ "name": name, "content": content })).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body
}

fn artifacts_path(ws: &str) -> String {
    format!("{WORKSPACES}/{ws}/artifacts")
}

fn version_path(ws: &str, artifact_id: &str, version: i64, leaf: &str) -> String {
    format!("{}/{artifact_id}/versions/{version}/{leaf}", artifacts_path(ws))
}

async fn list(harness: &Harness, ws: &str) -> Value {
    let (status, body) = harness.send_json(Method::GET, &artifacts_path(ws), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

/// 下載：帶桌面憑證，回傳 (狀態碼, 位元組, `X-Baton-Sha256`)。
async fn download(harness: &Harness, ws: &str, artifact_id: &str, version: i64) -> (StatusCode, Vec<u8>, Option<String>) {
    let path = version_path(ws, artifact_id, version, "content");
    let response = harness
        .request(Method::GET, &path)
        .bearer_auth(harness.desktop_credential())
        .header(baton_api::CLIENT_HEADER, baton_api::CLIENT_HEADER_VALUE)
        .send()
        .await
        .unwrap();
    let status = response.status();
    let sha = response.headers().get(baton_api::SHA256_HEADER).map(|v| v.to_str().unwrap().to_string());
    (status, response.bytes().await.unwrap().to_vec(), sha)
}

/// 成果儲存目錄下的全部檔案（相對路徑以 `/` 分隔）與大小。
fn stored_files(harness: &Harness) -> Vec<(String, u64)> {
    fn walk(root: &std::path::Path, dir: &std::path::Path, found: &mut Vec<(String, u64)>) {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(root, &path, found);
            } else {
                let relative = path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
                found.push((relative, entry.metadata().unwrap().len()));
            }
        }
    }
    let mut found = Vec::new();
    walk(&harness.artifacts_dir(), &harness.artifacts_dir(), &mut found);
    found.sort();
    found
}

fn file_of(harness: &Harness, storage_path: &str) -> PathBuf {
    harness.artifacts_dir().join(storage_path)
}

fn first_version(listing: &Value) -> &Value {
    &listing["artifacts"][0]["versions"][0]
}

// ============================================================ 登記與 manifest

#[tokio::test]
async fn a_registered_artifact_has_a_manifest_whose_hash_and_size_match_the_stored_file() {
    let harness = harness().await;
    let (ws, tid, rid, token) = running(&harness).await;

    let registered = register_ok(&harness, &token, "report.md", "abc").await;
    assert_eq!((registered["version"].as_i64(), registered["created"].as_bool()), (Some(1), Some(true)));
    assert_eq!(registered["sha256"], SHA256_ABC);
    assert_eq!(registered["size_bytes"], 3);

    let listing = list(&harness, &ws).await;
    let artifact = &listing["artifacts"][0];
    assert_eq!(artifact["id"], registered["artifact_id"]);
    assert_eq!((artifact["name"].as_str(), artifact["task_id"].as_str()), (Some("report.md"), Some(tid.as_str())));
    let version = first_version(&listing);
    assert_eq!(version["sha256"], SHA256_ABC);
    assert_eq!(version["size_bytes"], 3);
    assert_eq!(version["produced_by_run_id"], rid.as_str());
    assert_eq!(version["produced_by_generation"], 1);
    assert_eq!(version["file_status"], "ok");
    assert_eq!(listing["unregistered_files"], json!([]));

    // 落地的檔案：manifest 指向的位置存在，內容就是登記的內容，沒有殘留暫存檔
    let on_disk = std::fs::read(file_of(&harness, version["storage_path"].as_str().unwrap())).unwrap();
    assert_eq!(on_disk, b"abc");
    assert_eq!(stored_files(&harness).len(), 1, "只有一個完整檔案，沒有暫存檔");

    let (status, bytes, sha) = download(&harness, &ws, artifact["id"].as_str().unwrap(), 1).await;
    assert_eq!((status, bytes.as_slice(), sha.as_deref()), (StatusCode::OK, &b"abc"[..], Some(SHA256_ABC)));
}

#[tokio::test]
async fn binary_content_registered_as_base64_round_trips_byte_for_byte() {
    use base64::Engine;
    let harness = harness().await;
    let (ws, _tid, _rid, token) = running(&harness).await;
    let bytes: Vec<u8> = (0..=255u8).chain([0, 0, 255]).collect();

    let body = json!({ "name": "data.bin", "content_base64": base64::engine::general_purpose::STANDARD.encode(&bytes) });
    let (status, registered) = register(&harness, &token, body).await;
    assert_eq!(status, StatusCode::CREATED, "{registered}");
    assert_eq!(registered["size_bytes"], bytes.len());

    let listing = list(&harness, &ws).await;
    let (status, downloaded, _) = download(&harness, &ws, listing["artifacts"][0]["id"].as_str().unwrap(), 1).await;
    assert_eq!((status, downloaded), (StatusCode::OK, bytes));
}

#[tokio::test]
async fn a_new_version_never_overwrites_the_old_one() {
    let harness = harness().await;
    let (ws, _tid, _rid, token) = running(&harness).await;

    let first = register_ok(&harness, &token, "report.md", "第一版").await;
    let first_listing = list(&harness, &ws).await;
    let old_path = first_version(&first_listing)["storage_path"].as_str().unwrap().to_string();
    let old_bytes = std::fs::read(file_of(&harness, &old_path)).unwrap();

    let second = register_ok(&harness, &token, "report.md", "第二版，內容不同").await;
    assert_eq!(second["artifact_id"], first["artifact_id"], "同一個 Task 內同名是同一個成果");
    assert_eq!((second["version"].as_i64(), second["created"].as_bool()), (Some(2), Some(false)));

    let listing = list(&harness, &ws).await;
    assert_eq!(listing["artifacts"].as_array().unwrap().len(), 1);
    let versions = listing["artifacts"][0]["versions"].as_array().unwrap();
    assert_eq!(versions.len(), 2);
    assert_ne!(versions[0]["storage_path"], versions[1]["storage_path"], "每個版本一個獨立檔案");
    assert_ne!(versions[0]["sha256"], versions[1]["sha256"]);

    // 舊版本的檔案與內容原封不動，兩個版本都能下載
    assert_eq!(std::fs::read(file_of(&harness, &old_path)).unwrap(), old_bytes);
    let id = first["artifact_id"].as_str().unwrap();
    assert_eq!(download(&harness, &ws, id, 1).await.1, "第一版".as_bytes());
    assert_eq!(download(&harness, &ws, id, 2).await.1, "第二版，內容不同".as_bytes());

    // 同一個 Task 不同名稱是另一個成果
    let other = register_ok(&harness, &token, "notes.md", "筆記").await;
    assert_ne!(other["artifact_id"], first["artifact_id"]);
    assert_eq!(other["version"], 1);
}

#[tokio::test]
async fn manifests_are_append_only_in_the_database() {
    let harness = harness().await;
    let (_ws, _tid, _rid, token) = running(&harness).await;
    register_ok(&harness, &token, "report.md", "abc").await;

    // 就算繞過 Runtime 直接動資料庫，觸發器也禁止改寫或刪除已登記的成果與版本
    let db = rusqlite::Connection::open(harness.data_dir().join(baton_store::DB_FILE_NAME)).unwrap();
    for sql in [
        "UPDATE artifact_version SET sha256 = '' || substr(sha256, 1, 63) || '0'",
        "DELETE FROM artifact_version",
        "UPDATE artifact SET name = 'x'",
        "DELETE FROM artifact",
    ] {
        assert!(db.execute(sql, []).is_err(), "{sql}");
    }
}

#[tokio::test]
async fn registration_appends_an_event_per_version_and_survives_a_restart() {
    let mut harness = harness().await;
    let (ws, tid, rid, token) = running(&harness).await;
    let first = register_ok(&harness, &token, "report.md", "一").await;
    register_ok(&harness, &token, "report.md", "二").await;

    harness.restart().await;

    let listing = list(&harness, &ws).await;
    assert_eq!(listing["artifacts"][0]["versions"].as_array().unwrap().len(), 2);
    assert_eq!(download(&harness, &ws, first["artifact_id"].as_str().unwrap(), 2).await.1, "二".as_bytes());

    let db = harness.db();
    let mut statement = db
        .prepare(
            "SELECT aggregate_version, event_type, payload FROM event
             WHERE aggregate_type = 'artifact' AND aggregate_id = ?1 ORDER BY aggregate_version",
        )
        .unwrap();
    let events: Vec<(i64, String, Value)> = statement
        .query_map([first["artifact_id"].as_str().unwrap()], |r| {
            Ok((r.get(0)?, r.get(1)?, serde_json::from_str(&r.get::<_, String>(2)?).unwrap()))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(events.iter().map(|e| (e.0, e.1.as_str())).collect::<Vec<_>>(), [(1, "artifact_registered"), (2, "artifact_registered")]);
    assert_eq!(events[0].2["task_id"], tid.as_str());
    assert_eq!(events[0].2["run_id"], rid.as_str());
    assert_eq!(events[0].2["sha256"], first["sha256"]);
}

// ============================================================ 中途中斷與恢復

/// 在給定的崩潰點讓登記中途中斷，重建 Runtime，回傳 (harness, ws, run_id, token, 內容)。
async fn crash_during_registration(point: ArtifactWritePoint) -> (Harness, String, String, String, String) {
    let mut harness = harness().await;
    let (ws, _tid, rid, token) = running(&harness).await;
    let content = "這份成果寫到一半 Runtime 就崩潰了，內容夠長才分得出一半。".repeat(4);

    harness.workspace_fs.hang_next_artifact_write_at(point);
    let body = json!({ "name": "report.md", "content": content });
    let hung = tokio::time::timeout(Duration::from_millis(500), register(&harness, &token, body)).await;
    assert!(hung.is_err(), "登記應該卡在崩潰點，不可能完成");

    harness.restart().await;
    (harness, ws, rid, token, content)
}

#[tokio::test]
async fn a_crash_mid_write_leaves_no_registered_artifact_and_an_identifiable_staging_file() {
    let (harness, ws, _rid, _token, content) = crash_during_registration(ArtifactWritePoint::MidWrite).await;

    let listing = list(&harness, &ws).await;
    assert_eq!(listing["artifacts"], json!([]), "沒有已登記的成果");
    let leftovers = listing["unregistered_files"].as_array().unwrap();
    assert_eq!(leftovers.len(), 1);
    assert_eq!(leftovers[0]["kind"], "staging");
    assert!(leftovers[0]["size_bytes"].as_u64().unwrap() < content.len() as u64, "暫存檔的內容不完整");
    // 未登記的檔案不是成果：沒有任何成果可以下載
    assert_eq!(stored_files(&harness).len(), 1);
}

#[tokio::test]
async fn a_crash_before_rename_leaves_a_complete_but_unregistered_staging_file() {
    let (harness, ws, _rid, _token, _content) = crash_during_registration(ArtifactWritePoint::BeforeRename).await;

    let listing = list(&harness, &ws).await;
    assert_eq!(listing["artifacts"], json!([]));
    assert_eq!(listing["unregistered_files"][0]["kind"], "staging");
}

#[tokio::test]
async fn a_crash_after_rename_but_before_the_manifest_leaves_an_unregistered_file_not_an_artifact() {
    let (harness, ws, _rid, _token, content) = crash_during_registration(ArtifactWritePoint::AfterRename).await;

    let listing = list(&harness, &ws).await;
    assert_eq!(listing["artifacts"], json!([]), "manifest 沒有提交，就不是成果");
    let leftovers = listing["unregistered_files"].as_array().unwrap();
    assert_eq!(leftovers.len(), 1);
    assert_eq!(leftovers[0]["kind"], "unregistered");
    assert_eq!(leftovers[0]["size_bytes"].as_u64().unwrap(), content.len() as u64, "檔案本身完整，只是沒登記");
}

#[tokio::test]
async fn after_a_crash_the_worker_can_register_again_and_the_leftover_stays_unregistered() {
    for point in [ArtifactWritePoint::MidWrite, ArtifactWritePoint::BeforeRename, ArtifactWritePoint::AfterRename] {
        let (harness, ws, _rid, token, content) = crash_during_registration(point).await;

        // Worker 的憑證在 Runtime 重啟後仍然有效，重新登記得到版本 1（之前那次從未登記）
        let registered = register_ok(&harness, &token, "report.md", &content).await;
        assert_eq!((registered["version"].as_i64(), registered["created"].as_bool()), (Some(1), Some(true)), "{point:?}");

        let listing = list(&harness, &ws).await;
        assert_eq!(listing["artifacts"].as_array().unwrap().len(), 1, "{point:?}");
        assert_eq!(first_version(&listing)["file_status"], "ok", "{point:?}");
        assert_eq!(listing["unregistered_files"].as_array().unwrap().len(), 1, "{point:?}：先前的殘留仍標示為未登記");
        let (status, bytes, _) = download(&harness, &ws, registered["artifact_id"].as_str().unwrap(), 1).await;
        assert_eq!((status, bytes), (StatusCode::OK, content.into_bytes()), "{point:?}");
    }
}

// ============================================================ 完整性

#[tokio::test]
async fn content_that_no_longer_matches_the_manifest_is_refused_instead_of_served() {
    let harness = harness().await;
    let (ws, _tid, _rid, token) = running(&harness).await;
    let registered = register_ok(&harness, &token, "report.md", "abc").await;
    let id = registered["artifact_id"].as_str().unwrap();
    let listing = list(&harness, &ws).await;
    let path = file_of(&harness, first_version(&listing)["storage_path"].as_str().unwrap());

    // 同樣大小、內容被改掉：列表只比對大小所以看不出來，預覽與下載會重新算雜湊而拒絕
    std::fs::write(&path, b"abd").unwrap();
    assert_eq!(first_version(&list(&harness, &ws).await)["file_status"], "ok");
    let (status, body) = harness.send_json(Method::GET, &version_path(&ws, id, 1, "preview"), None).await;
    assert_eq!((status, error_code(&body)), (StatusCode::CONFLICT, "artifact_integrity_failed"), "{body}");
    let (status, bytes, _) = download(&harness, &ws, id, 1).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(!String::from_utf8_lossy(&bytes).contains("abd"), "不可把被竄改的內容交出去");

    // 被截短：列表就會標示大小不符
    std::fs::write(&path, b"ab").unwrap();
    assert_eq!(first_version(&list(&harness, &ws).await)["file_status"], "size_mismatch");

    // 檔案不見了
    std::fs::remove_file(&path).unwrap();
    assert_eq!(first_version(&list(&harness, &ws).await)["file_status"], "missing");
    let (status, body) = harness.send_json(Method::GET, &version_path(&ws, id, 1, "preview"), None).await;
    assert_eq!((status, error_code(&body)), (StatusCode::CONFLICT, "artifact_file_missing"), "{body}");
}

// ============================================================ 預覽

#[tokio::test]
async fn preview_shows_text_cuts_long_text_and_refuses_binary() {
    use base64::Engine;
    let harness = harness().await;
    let (ws, _tid, _rid, token) = running(&harness).await;
    let short = register_ok(&harness, &token, "short.md", "你好，成果").await;
    let long_text = "a".repeat(PREVIEW_MAX_BYTES + 10);
    let long = register_ok(&harness, &token, "long.txt", &long_text).await;
    let (status, binary) = register(
        &harness,
        &token,
        json!({ "name": "image.bin", "content_base64": base64::engine::general_purpose::STANDARD.encode([0x89, b'P', 0, 1]) }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{binary}");

    let preview = |id: Value| {
        let harness = &harness;
        let ws = ws.clone();
        async move {
            let (status, body) =
                harness.send_json(Method::GET, &version_path(&ws, id.as_str().unwrap(), 1, "preview"), None).await;
            assert_eq!(status, StatusCode::OK, "{body}");
            body
        }
    };
    let short = preview(short["artifact_id"].clone()).await;
    assert_eq!(short["preview"], json!({ "kind": "text", "text": "你好，成果", "truncated": false }));
    assert_eq!((short["name"].as_str(), short["size_bytes"].as_i64()), (Some("short.md"), Some(15)));

    let long = preview(long["artifact_id"].clone()).await;
    assert_eq!(long["preview"]["truncated"], true);
    assert_eq!(long["preview"]["text"].as_str().unwrap().len(), PREVIEW_MAX_BYTES);

    assert_eq!(preview(binary["artifact_id"].clone()).await["preview"], json!({ "kind": "binary" }));
}

// ============================================================ 存取控制

#[tokio::test]
async fn unauthorized_callers_cannot_download_an_artifact() {
    let harness = harness().await;
    let (ws, _tid, _rid, token) = running(&harness).await;
    let registered = register_ok(&harness, &token, "report.md", "機密內容").await;
    let id = registered["artifact_id"].as_str().unwrap();
    let path = version_path(&ws, id, 1, "content");
    let get = |bearer: Option<String>, client_header: bool| {
        let mut request = harness.request(Method::GET, &path);
        if let Some(bearer) = bearer {
            request = request.bearer_auth(bearer);
        }
        if client_header {
            request = request.header(baton_api::CLIENT_HEADER, baton_api::CLIENT_HEADER_VALUE);
        }
        async move {
            let response = request.send().await.unwrap();
            let status = response.status();
            (status, response.text().await.unwrap())
        }
    };

    // 沒帶憑證、猜的憑證、缺少固定標頭
    let (status, body) = get(None, true).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(!body.contains("機密內容"));
    assert_eq!(get(Some("guessed-token".into()), true).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(get(Some(harness.desktop_credential()), false).await.0, StatusCode::FORBIDDEN);

    // 憑證過期
    let expiring = harness.issue_credential(Role::Desktop, Some(Duration::from_secs(60))).await;
    assert_eq!(get(Some(expiring.clone()), true).await.0, StatusCode::OK);
    harness.clock.advance_millis(60_000);
    assert_eq!(get(Some(expiring), true).await.0, StatusCode::UNAUTHORIZED);

    // Agent 憑證（包含登記這份成果的 Worker 自己）不能走管理 API 下載或預覽
    let (status, body) = get(Some(token.clone()), true).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(body.contains("role_not_allowed") && !body.contains("機密內容"));
    let response = harness
        .request(Method::GET, &artifacts_path(&ws))
        .bearer_auth(&token)
        .header(baton_api::CLIENT_HEADER, baton_api::CLIENT_HEADER_VALUE)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    // 反過來：桌面憑證不能冒充 Worker 登記成果
    let (status, body) = register(&harness, &harness.desktop_credential(), json!({ "name": "x", "content": "y" })).await;
    assert_eq!((status, error_code(&body)), (StatusCode::FORBIDDEN, "role_not_allowed"), "{body}");

    // 驗證身分的桌面憑證才下載得到
    assert_eq!(download(&harness, &ws, id, 1).await.1, "機密內容".as_bytes());
}

#[tokio::test]
async fn artifacts_are_scoped_to_their_workspace() {
    let harness = harness().await;
    let (ws, _tid, _rid, token) = running(&harness).await;
    let other = new_workspace(&harness, "other").await;
    let registered = register_ok(&harness, &token, "report.md", "abc").await;
    let id = registered["artifact_id"].as_str().unwrap();

    assert_eq!(list(&harness, &other).await["artifacts"], json!([]));
    for leaf in ["preview", "content"] {
        let (status, body) = harness.send_json(Method::GET, &version_path(&other, id, 1, leaf), None).await;
        assert_eq!((status, error_code(&body)), (StatusCode::NOT_FOUND, "artifact_not_found"), "{leaf}：{body}");
    }
    // 不存在的版本、不存在的成果、不存在的 Workspace
    for (path, code) in [
        (version_path(&ws, id, 2, "content"), "artifact_not_found"),
        (version_path(&ws, id, 0, "content"), "artifact_not_found"),
        (version_path(&ws, "art_missing", 1, "content"), "artifact_not_found"),
        (artifacts_path("ws_missing"), "workspace_not_found"),
    ] {
        let (status, body) = harness.send_json(Method::GET, &path, None).await;
        assert_eq!((status, error_code(&body)), (StatusCode::NOT_FOUND, code), "{path}：{body}");
    }
}

#[tokio::test]
async fn a_worker_cannot_register_into_another_workspace_or_run() {
    let harness = harness().await;
    let (ws, _tid, rid, token) = running(&harness).await;
    let other = new_workspace(&harness, "other").await;

    for claim in [json!({ "workspace_id": other }), json!({ "run_id": "run_someone_else" })] {
        let mut body = json!({ "name": "report.md", "content": "abc" });
        body.as_object_mut().unwrap().extend(claim.as_object().unwrap().clone());
        let (status, response) = register(&harness, &token, body).await;
        assert_eq!((status, error_code(&response)), (StatusCode::FORBIDDEN, "agent_scope_mismatch"), "{response}");
    }
    assert_eq!(list(&harness, &ws).await["artifacts"], json!([]));
    assert_eq!(list(&harness, &other).await["artifacts"], json!([]));
    assert!(stored_files(&harness).is_empty(), "被拒絕的登記不留下任何檔案");

    // 與憑證一致的聲明沒有問題
    let body = json!({ "name": "report.md", "content": "abc", "workspace_id": ws, "run_id": rid });
    assert_eq!(register(&harness, &token, body).await.0, StatusCode::CREATED);
}

#[tokio::test]
async fn a_run_that_has_ended_cannot_register_and_leaves_no_file_behind() {
    let harness = harness().await;
    let ws = new_workspace(&harness, "main").await;
    let tid = ready_task(&harness, &ws, "整理").await;
    harness.agent_adapter.script_next_run(vec![baton_ports::AdapterEvent::Exited { success: true }]);
    let (_rid, token) = start_run(&harness, &ws, &tid).await;
    harness.supervise().await; // Run 結束

    let (status, body) = register(&harness, &token, json!({ "name": "late.md", "content": "太晚了" })).await;
    assert_eq!((status, error_code(&body)), (StatusCode::CONFLICT, "run_report_stale"), "{body}");
    assert_eq!(list(&harness, &ws).await["artifacts"], json!([]));
    assert!(stored_files(&harness).is_empty(), "過期世代不可以在儲存區留下檔案");
}

// ============================================================ 輸入驗證

#[tokio::test]
async fn invalid_registrations_are_rejected_with_stable_codes_and_store_nothing() {
    use base64::Engine;
    let harness = harness().await;
    let (ws, _tid, _rid, token) = running(&harness).await;

    let too_big = "x".repeat(baton_domain::artifact::ARTIFACT_MAX_BYTES + 1);
    let cases: Vec<(Value, StatusCode, &str)> = vec![
        (json!({ "name": "", "content": "a" }), StatusCode::BAD_REQUEST, "artifact_invalid"),
        (json!({ "name": "../escape.md", "content": "a" }), StatusCode::BAD_REQUEST, "artifact_invalid"),
        (json!({ "name": "a\\b.md", "content": "a" }), StatusCode::BAD_REQUEST, "artifact_invalid"),
        (json!({ "name": "a".repeat(201), "content": "a" }), StatusCode::BAD_REQUEST, "artifact_invalid"),
        (json!({ "name": "a.md", "description": "d".repeat(1001), "content": "a" }), StatusCode::BAD_REQUEST, "artifact_invalid"),
        (json!({ "name": "a.md" }), StatusCode::BAD_REQUEST, "artifact_invalid"),
        (json!({ "name": "a.md", "content": "a", "content_base64": "YQ==" }), StatusCode::BAD_REQUEST, "artifact_invalid"),
        (json!({ "name": "a.md", "content_base64": "不是 base64!" }), StatusCode::BAD_REQUEST, "artifact_invalid"),
        (json!({ "name": "a.md", "content": too_big }), StatusCode::PAYLOAD_TOO_LARGE, "artifact_too_large"),
        (
            json!({ "name": "a.md", "content_base64": base64::engine::general_purpose::STANDARD.encode(too_big.as_bytes()) }),
            StatusCode::PAYLOAD_TOO_LARGE,
            "artifact_too_large",
        ),
        (json!({ "content": "沒有名稱" }), StatusCode::BAD_REQUEST, "bad_request"),
    ];
    for (body, expected_status, expected_code) in cases {
        let label = body.to_string().chars().take(80).collect::<String>();
        let (status, response) = register(&harness, &token, body).await;
        assert_eq!((status, error_code(&response)), (expected_status, expected_code), "{label}：{response}");
    }
    assert_eq!(list(&harness, &ws).await["artifacts"], json!([]));
    assert!(stored_files(&harness).is_empty());

    // 剛好等於上限是允許的
    let at_limit = "y".repeat(baton_domain::artifact::ARTIFACT_MAX_BYTES);
    assert_eq!(register(&harness, &token, json!({ "name": "max.txt", "content": at_limit })).await.0, StatusCode::CREATED);
}

// ============================================================ Worker 經由 MCP shim 登記

async fn talk_to_shim(harness: &Harness, token: &str, requests: &[Value]) -> Vec<Value> {
    let (client, shim_side) = duplex(64 * 1024);
    let (shim_read, shim_write) = tokio::io::split(shim_side);
    let config = ShimConfig::new(harness.runtime_url(), token.to_string());
    let server = tokio::spawn(async move { serve(BufReader::new(shim_read), shim_write, config).await });

    let (client_read, mut client_write) = tokio::io::split(client);
    for request in requests {
        client_write.write_all(format!("{request}\n").as_bytes()).await.unwrap();
    }
    client_write.shutdown().await.unwrap();
    let mut lines = BufReader::new(client_read).lines();
    let mut responses = Vec::new();
    while let Some(line) = lines.next_line().await.unwrap() {
        responses.push(serde_json::from_str(&line).expect("shim 的輸出每一行都是 JSON-RPC 訊息"));
    }
    server.await.unwrap().unwrap();
    responses
}

#[tokio::test]
async fn a_worker_registers_an_artifact_through_the_mcp_shim_and_the_ui_can_download_it() {
    let harness = harness().await;
    let (ws, tid, rid, token) = running(&harness).await;
    let call = |id: u64, name: &str, arguments: Value| {
        json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call", "params": { "name": name, "arguments": arguments } })
    };

    let responses = talk_to_shim(
        &harness,
        &token,
        &[
            json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }),
            call(2, TOOL_GET_TASK_CONTRACT, json!({})),
            call(3, TOOL_REGISTER_ARTIFACT, json!({ "name": "report.md", "description": "競品比較", "content": "abc" })),
            call(4, TOOL_REGISTER_ARTIFACT, json!({ "name": "bad/name.md", "content": "abc" })),
        ],
    )
    .await;

    let tools: Vec<&str> = responses[0]["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(tools.contains(&TOOL_REGISTER_ARTIFACT));
    assert_eq!(responses[2]["result"]["isError"], false, "{}", responses[2]);
    assert!(responses[2]["result"]["content"][0]["text"].as_str().unwrap().contains(SHA256_ABC));
    assert_eq!(responses[3]["result"]["isError"], true);
    assert!(responses[3]["result"]["content"][0]["text"].as_str().unwrap().contains("artifact_invalid"));
    assert!(!responses.iter().any(|r| r.to_string().contains(&token)), "shim 的輸出不含憑證");

    let listing = list(&harness, &ws).await;
    assert_eq!(listing["artifacts"].as_array().unwrap().len(), 1, "被拒絕的那次沒有登記");
    let artifact = &listing["artifacts"][0];
    assert_eq!((artifact["task_id"].as_str(), first_version(&listing)["produced_by_run_id"].as_str()), (Some(tid.as_str()), Some(rid.as_str())));
    assert_eq!(first_version(&listing)["description"], "競品比較");
    let (status, bytes, sha) = download(&harness, &ws, artifact["id"].as_str().unwrap(), 1).await;
    assert_eq!((status, bytes.as_slice(), sha.as_deref()), (StatusCode::OK, &b"abc"[..], Some(SHA256_ABC)));
}
