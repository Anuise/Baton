//! 情境測試：共同設定匯入與版本化快照（票 08，S3 / spec D-09），經由 Runtime harness 的真實 HTTP API 驗證。
//!
//! 使用者平常的 Claude Code 設定是 harness 提供的一個暫存「家目錄」（`FakeUserSettings`，走真實讀檔），
//! 所以可以直接比對檔案位元組。只斷言外部行為：HTTP 回應、落地的快照目錄、資料庫與日誌檔的內容。

use std::collections::BTreeMap;
use std::path::PathBuf;

use baton_domain::auth::Role;
use baton_domain::shared_settings::{bundle_from_snapshot_files, sha256_hex, SettingsVersion};
use baton_harness::Harness;
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

const SETTINGS: &str = "/api/v1/shared-settings";
const PREVIEW: &str = "/api/v1/shared-settings/import-preview";
const ADOPT: &str = "/api/v1/shared-settings/adopt";
const WORKSPACES: &str = "/api/v1/workspaces";

// --- 共用輔助 ---

async fn get(harness: &Harness, path: &str) -> (StatusCode, Value) {
    harness.send_json(Method::GET, path, None).await
}

async fn post(harness: &Harness, path: &str, body: Option<Value>) -> (StatusCode, Value) {
    harness.send_json(Method::POST, path, body).await
}

async fn preview(harness: &Harness) -> Value {
    let (status, body) = post(harness, PREVIEW, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

/// 預覽後採用；回傳新版本。
async fn adopt_current(harness: &Harness) -> Value {
    let candidate = preview(harness).await;
    let (status, version) = post(harness, ADOPT, Some(json!({ "expected_hash": candidate["candidate_hash"] }))).await;
    assert_eq!(status, StatusCode::CREATED, "{version}");
    version
}

async fn status_versions(harness: &Harness) -> Vec<i64> {
    let (status, body) = get(harness, SETTINGS).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["versions"].as_array().unwrap().iter().map(|v| v["version"].as_i64().unwrap()).collect()
}

fn error_code(body: &Value) -> &str {
    body["error"].as_str().unwrap_or_default()
}

fn plain_dir(harness: &Harness, name: &str) -> String {
    let dir = harness.data_dir().join("projects").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    dir.to_string_lossy().into_owned()
}

fn filled(text: &str) -> Value {
    json!({ "status": "filled", "text": text })
}

/// 建立 Workspace 與 Goal，回傳 (workspace_id, goal_id)。
async fn workspace_with_goal(harness: &Harness, name: &str) -> (String, String) {
    let path = plain_dir(harness, name);
    let (status, ws) = post(harness, WORKSPACES, Some(json!({ "path": path }))).await;
    assert_eq!(status, StatusCode::CREATED, "{ws}");
    let ws = ws["id"].as_str().unwrap().to_string();
    let (status, goal) = post(harness, &format!("{WORKSPACES}/{ws}/goals"), Some(json!({ "title": "目標" }))).await;
    assert_eq!(status, StatusCode::CREATED, "{goal}");
    (ws, goal["id"].as_str().unwrap().to_string())
}

/// 在 Goal 下建立一個 ready 的 Task 並派工，回傳 Run 識別。
async fn run_in(harness: &Harness, ws: &str, goal: &str, title: &str) -> String {
    let contract = json!({
        "objective": filled(title), "inputs": filled("既有的資料"), "scope": filled("公開資料"),
        "deliverables": filled("比較表"), "done_criteria": filled("五個產品"), "blocker_handling": filled("卡住時回報"),
        "required_capabilities": [],
    });
    let (status, task) = post(harness, &format!("{WORKSPACES}/{ws}/tasks"), Some(json!({ "goal_id": goal, "title": title, "contract": contract }))).await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    let task_path = format!("{WORKSPACES}/{ws}/tasks/{}", task["id"].as_str().unwrap());
    let (status, ready) = post(harness, &format!("{task_path}/ready"), None).await;
    assert_eq!(status, StatusCode::OK, "{ready}");
    let (status, run) = post(harness, &format!("{task_path}/dispatch"), None).await;
    assert_eq!(status, StatusCode::CREATED, "{run}");
    run["id"].as_str().unwrap().to_string()
}

/// 一個 Workspace 與一個已派工的 Run，回傳 (workspace_id, run_id)。
async fn new_run(harness: &Harness, workspace_name: &str) -> (String, String) {
    let (ws, goal) = workspace_with_goal(harness, workspace_name).await;
    let run = run_in(harness, &ws, &goal, "工作").await;
    (ws, run)
}

fn snapshot_path(ws: &str, run: &str) -> String {
    format!("{WORKSPACES}/{ws}/runs/{run}/settings-snapshot")
}

/// 讀取快照目錄的全部檔案（檔名 → 位元組）。
fn read_snapshot_dir(directory: &str) -> BTreeMap<String, Vec<u8>> {
    std::fs::read_dir(directory)
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            (path.file_name().unwrap().to_string_lossy().into_owned(), std::fs::read(&path).unwrap())
        })
        .collect()
}

/// 放入一組一般的共同設定（沒有秘密）。
fn plant_ordinary_settings(harness: &Harness) {
    harness.user_settings.write_settings(
        r#"{"model":"opus","theme":"dark",
            "hooks":{"Stop":[{"hooks":[{"type":"command","command":"echo done"}]}]},
            "enabledPlugins":{"formatter@market":true}}"#,
    );
    harness.user_settings.write_instructions("# 全域指示\n一律使用繁體中文。\n");
    harness.user_settings.write_user_state(r#"{"mcpServers":{"docs":{"command":"npx","args":["-y","docs-mcp"]}}}"#);
}

// --- 預覽與採用 ---

#[tokio::test]
async fn import_preview_shows_sources_and_differences_and_writes_nothing() {
    let harness = Harness::start().await;
    plant_ordinary_settings(&harness);

    let (status, empty) = get(&harness, SETTINGS).await;
    assert_eq!((status, empty["current"].clone(), empty["versions"].clone()), (StatusCode::OK, Value::Null, json!([])));

    let candidate = preview(&harness).await;

    // 來源：三個檔案都找到，路徑指向使用者的設定位置
    let sources = candidate["sources"].as_array().unwrap();
    assert_eq!(sources.iter().map(|s| s["kind"].as_str().unwrap()).collect::<Vec<_>>(), ["settings", "instructions", "user_state"]);
    assert!(sources.iter().all(|s| s["found"] == json!(true) && s["sha256"].as_str().unwrap().len() == 64));
    assert!(sources[0]["path"].as_str().unwrap().ends_with("settings.json"));
    // 尚無版本：全部都是新增
    assert_eq!(candidate["base_version"], Value::Null);
    assert_eq!(candidate["changed"], json!(true));
    let diff = candidate["diff"].as_array().unwrap();
    assert!(diff.iter().all(|d| d["change"] == json!("added")));
    let categories: Vec<_> = diff.iter().map(|d| d["category"].as_str().unwrap()).collect();
    for category in ["preferences", "instructions", "hooks", "mcp", "plugins"] {
        assert!(categories.contains(&category), "差異應包含 {category}：{categories:?}");
    }
    // 預覽不採用任何東西
    assert!(status_versions(&harness).await.is_empty());
}

#[tokio::test]
async fn each_adoption_creates_a_new_version_and_earlier_versions_never_change() {
    let harness = Harness::start().await;
    plant_ordinary_settings(&harness);

    let first = adopt_current(&harness).await;
    let (_, first_as_read) = get(&harness, &format!("{SETTINGS}/versions/1")).await;
    assert_eq!(first["version"], json!(1));
    assert_eq!(first_as_read, first, "讀取版本得到的就是採用時建立的內容");

    // 使用者改了設定：預覽顯示相對於 v1 的差異，採用後是 v2
    harness.user_settings.write_settings(r#"{"model":"sonnet","enabledPlugins":{"formatter@market":true}}"#);
    let candidate = preview(&harness).await;
    assert_eq!((candidate["base_version"].clone(), candidate["changed"].clone()), (json!(1), json!(true)));
    let changed: Vec<_> = candidate["diff"].as_array().unwrap().iter().map(|d| (d["key"].as_str().unwrap(), d["change"].as_str().unwrap())).collect();
    assert!(changed.contains(&("model", "changed")) && changed.contains(&("theme", "removed")) && changed.contains(&("Stop", "removed")));
    let second = adopt_current(&harness).await;

    assert_eq!(second["version"], json!(2));
    assert_ne!(second["content_hash"], first["content_hash"]);
    let (_, v1_again) = get(&harness, &format!("{SETTINGS}/versions/1")).await;
    assert_eq!(v1_again, first, "舊版本的內容與雜湊在採用新版本後完全不變");
    assert_eq!(status_versions(&harness).await, [1, 2]);

    // 內容沒變時預覽標示沒有差異，但「每次採用都建立新版本」
    let unchanged = preview(&harness).await;
    assert_eq!((unchanged["changed"].clone(), unchanged["diff"].clone()), (json!(false), json!([])));
    let third = adopt_current(&harness).await;
    assert_eq!((third["version"].clone(), third["content_hash"].clone()), (json!(3), second["content_hash"].clone()));
    assert_eq!(status_versions(&harness).await, [1, 2, 3]);
}

#[tokio::test]
async fn adoption_must_present_the_previewed_hash_so_changed_sources_are_rejected() {
    let harness = Harness::start().await;
    plant_ordinary_settings(&harness);
    let candidate = preview(&harness).await;

    harness.user_settings.write_settings(r#"{"model":"haiku"}"#);
    let (status, body) = post(&harness, ADOPT, Some(json!({ "expected_hash": candidate["candidate_hash"] }))).await;

    assert_eq!((status, error_code(&body)), (StatusCode::CONFLICT, "settings_source_changed"));
    assert!(status_versions(&harness).await.is_empty(), "被拒絕時不建立任何版本");
    let (status, body) = post(&harness, ADOPT, Some(json!({}))).await;
    assert_eq!((status, error_code(&body)), (StatusCode::BAD_REQUEST, "bad_request"));
}

#[tokio::test]
async fn unreadable_source_files_are_reported_and_nothing_is_adopted() {
    let harness = Harness::start().await;
    harness.user_settings.write_settings("{ this is not json");

    let (status, body) = post(&harness, PREVIEW, None).await;
    assert_eq!((status, error_code(&body)), (StatusCode::UNPROCESSABLE_ENTITY, "settings_source_invalid"));
    let (status, body) = post(&harness, ADOPT, Some(json!({ "expected_hash": "x" }))).await;
    assert_eq!((status, error_code(&body)), (StatusCode::UNPROCESSABLE_ENTITY, "settings_source_invalid"));
    assert!(status_versions(&harness).await.is_empty());
}

#[tokio::test]
async fn a_machine_without_any_claude_settings_previews_as_empty() {
    let harness = Harness::start().await;

    let candidate = preview(&harness).await;

    assert!(candidate["sources"].as_array().unwrap().iter().all(|s| s["found"] == json!(false)));
    assert_eq!(candidate["diff"], json!([]));
}

#[tokio::test]
async fn shared_settings_endpoints_are_for_the_desktop_only() {
    let harness = Harness::start().await;
    let agent = harness.issue_credential(Role::Agent, None).await;

    for (method, path) in [(Method::GET, SETTINGS), (Method::POST, PREVIEW), (Method::POST, ADOPT)] {
        let response = harness
            .request(method, path)
            .bearer_auth(&agent)
            .header(baton_api::CLIENT_HEADER, baton_api::CLIENT_HEADER_VALUE)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{path}");
    }
}

#[tokio::test]
async fn adopted_versions_survive_a_runtime_restart() {
    let mut harness = Harness::start().await;
    plant_ordinary_settings(&harness);
    let first = adopt_current(&harness).await;

    harness.restart().await;

    assert_eq!(status_versions(&harness).await, [1]);
    let (_, again) = get(&harness, &format!("{SETTINGS}/versions/1")).await;
    assert_eq!(again, first);
}

// --- 登入秘密與使用者原始檔 ---

const CREDENTIAL_ACCESS: &str = "sk-ant-oat01-ACCESSTOKENVALUE0123456789abcdef";
const CREDENTIAL_REFRESH: &str = "sk-ant-ort01-REFRESHTOKENVALUE0123456789abcdef";
const ENV_API_KEY: &str = "sk-ant-api03-ENVKEYVALUE0123456789abcdef";
const KEY_HELPER: &str = "/opt/secret-helper-script-xyz.sh";
const GITHUB_TOKEN: &str = "ghp_GITHUBTOKENVALUE0123456789abcdefgh";
const BEARER_VALUE: &str = "BEARERVALUE0123456789abcdefghij";
const HOOK_SECRET: &str = "sk-ant-api03-HOOKSECRET0123456789abcdef";
const ACCOUNT_EMAIL: &str = "victim@example.com";
const ACCOUNT_UUID: &str = "uuid-VICTIM-1234-abcd";
const INSTRUCTION_SECRET: &str = "ghp_INSTRUCTIONSECRET0123456789abcdef";

const PLANTED_SECRETS: [&str; 10] = [
    CREDENTIAL_ACCESS,
    CREDENTIAL_REFRESH,
    ENV_API_KEY,
    KEY_HELPER,
    GITHUB_TOKEN,
    BEARER_VALUE,
    HOOK_SECRET,
    ACCOUNT_EMAIL,
    ACCOUNT_UUID,
    INSTRUCTION_SECRET,
];

/// 使用者的設定裡混著登入資料與秘密，同時有一般內容（用來證明掃描不是空轉）。
fn plant_settings_with_secrets(harness: &Harness) {
    harness.user_settings.write_in_config_dir(
        ".credentials.json",
        &format!(r#"{{"claudeAiOauth":{{"accessToken":"{CREDENTIAL_ACCESS}","refreshToken":"{CREDENTIAL_REFRESH}"}}}}"#),
    );
    harness.user_settings.write_settings(&format!(
        r#"{{"model":"opus","apiKeyHelper":"{KEY_HELPER}",
            "env":{{"ANTHROPIC_API_KEY":"{ENV_API_KEY}","EDITOR":"vim"}},
            "hooks":{{"Stop":[{{"hooks":[{{"type":"command","command":"echo done"}}]}}],
                      "PreToolUse":[{{"hooks":[{{"type":"command","command":"curl -H 'x-api-key: {HOOK_SECRET}' https://example.test"}}]}}]}}}}"#
    ));
    harness.user_settings.write_instructions(&format!("# 指示\n不要外洩。\n備忘：{INSTRUCTION_SECRET}\n"));
    harness.user_settings.write_user_state(&format!(
        r#"{{"oauthAccount":{{"emailAddress":"{ACCOUNT_EMAIL}","accountUuid":"{ACCOUNT_UUID}"}},
            "mcpServers":{{"docs":{{"command":"npx","args":["-y","docs-mcp"]}},
                           "gh":{{"command":"gh-mcp","env":{{"GITHUB_TOKEN":"{GITHUB_TOKEN}","REGION":"eu"}},
                                  "headers":{{"Authorization":"Bearer {BEARER_VALUE}"}}}}}}}}"#
    ));
}

/// 資料庫檔（含 WAL）與日誌檔的全部內容。
fn persisted_bytes(harness: &Harness) -> Vec<(String, Vec<u8>)> {
    let db = harness.data_dir().join(baton_store::DB_FILE_NAME);
    let mut files = vec![("control.sqlite".to_string(), std::fs::read(&db).unwrap())];
    for suffix in ["-wal", "-shm"] {
        let mut path = db.clone().into_os_string();
        path.push(suffix);
        if let Ok(bytes) = std::fs::read(PathBuf::from(path)) {
            files.push((format!("control.sqlite{suffix}"), bytes));
        }
    }
    files.push(("log".to_string(), harness.log_text().into_bytes()));
    files
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack.windows(needle.len()).any(|window| window == needle.as_bytes())
}

#[tokio::test]
async fn imported_content_never_contains_login_secrets_anywhere() {
    let harness = Harness::start().await;
    plant_settings_with_secrets(&harness);
    let (ws, run) = new_run(&harness, "w").await;

    // 走完整條路徑：預覽、採用、讀版本、列版本、產生 Run 快照；收集所有回應
    let preview_body = preview(&harness).await;
    let version = adopt_current(&harness).await;
    let (_, read_back) = get(&harness, &format!("{SETTINGS}/versions/1")).await;
    let (_, listing) = get(&harness, SETTINGS).await;
    let (status, snapshot) = post(&harness, &snapshot_path(&ws, &run), Some(json!({}))).await;
    assert_eq!(status, StatusCode::CREATED, "{snapshot}");

    // 掃描不是空轉：一般內容有被匯入，被清除的秘密有被記錄（只有項目名稱）
    let bundle = &version["bundle"];
    assert_eq!(bundle["preferences"]["model"], json!("opus"));
    assert_eq!(bundle["preferences"]["env"], json!({ "EDITOR": "vim" }));
    assert!(bundle["hooks"]["Stop"].is_array() && bundle["hooks"].get("PreToolUse").is_none());
    assert_eq!(bundle["mcp_servers"]["gh"]["env"], json!({ "REGION": "eu" }));
    assert!(bundle["mcp_servers"]["docs"].is_object());
    assert_eq!(bundle["instructions"], Value::Null);
    let excluded: Vec<_> = version["exclusions"].as_array().unwrap().iter().map(|e| e["item"].as_str().unwrap()).collect();
    for item in ["apiKeyHelper", "env.ANTHROPIC_API_KEY", "hooks.PreToolUse", "CLAUDE.md", "mcpServers.gh.env.GITHUB_TOKEN", "mcpServers.gh.headers.Authorization"] {
        assert!(excluded.contains(&item), "{item} 應在排除清單：{excluded:?}");
    }

    let mut everything: Vec<(String, Vec<u8>)> = vec![
        ("preview".into(), preview_body.to_string().into_bytes()),
        ("version".into(), version.to_string().into_bytes()),
        ("version read back".into(), read_back.to_string().into_bytes()),
        ("listing".into(), listing.to_string().into_bytes()),
        ("snapshot response".into(), snapshot.to_string().into_bytes()),
    ];
    everything.extend(read_snapshot_dir(snapshot["directory"].as_str().unwrap()).into_iter().map(|(name, bytes)| (format!("snapshot/{name}"), bytes)));
    everything.extend(persisted_bytes(&harness));
    for (place, bytes) in &everything {
        for secret in PLANTED_SECRETS {
            assert!(!contains(bytes, secret), "{place} 含有不該出現的秘密 {secret}");
        }
    }
}

#[tokio::test]
async fn the_users_original_settings_are_byte_identical_before_and_after_import() {
    let harness = Harness::start().await;
    plant_settings_with_secrets(&harness);
    let (ws, run) = new_run(&harness, "w").await;
    let before = harness.user_settings.files();
    assert!(before.contains_key(".claude/.credentials.json"));

    preview(&harness).await;
    adopt_current(&harness).await;
    harness.user_settings.write_settings(r#"{"model":"sonnet"}"#);
    let touched = harness.user_settings.files();
    adopt_current(&harness).await;
    let (status, _) = post(&harness, &snapshot_path(&ws, &run), Some(json!({}))).await;
    assert_eq!(status, StatusCode::CREATED);

    // 使用者自己改的檔案以外，匯入與採用沒有改動任何位元組、也沒有新增或刪除任何檔案
    assert_eq!(harness.user_settings.files(), touched);
    for (path, bytes) in &before {
        if path != ".claude/settings.json" {
            assert_eq!(touched.get(path), Some(bytes), "{path} 不應改變");
        }
    }
}

// --- Run 專屬唯讀設定快照 ---

#[tokio::test]
async fn run_settings_snapshot_is_a_read_only_directory_matching_the_version_hash() {
    let harness = Harness::start().await;
    plant_ordinary_settings(&harness);
    let version = adopt_current(&harness).await;
    let (ws, run) = new_run(&harness, "w").await;

    let (status, snapshot) = post(&harness, &snapshot_path(&ws, &run), Some(json!({}))).await;

    assert_eq!(status, StatusCode::CREATED, "{snapshot}");
    assert_eq!((snapshot["run_id"].clone(), snapshot["version"].clone()), (json!(run), json!(1)));
    assert_eq!(snapshot["content_hash"], version["content_hash"]);
    let directory = snapshot["directory"].as_str().unwrap();
    assert!(PathBuf::from(directory).starts_with(harness.data_dir().join("run-settings")), "{directory}");

    // 內容與版本雜湊一致：從目錄裡的檔案還原內容，雜湊就是版本的雜湊
    let files = read_snapshot_dir(directory);
    assert_eq!(files.keys().cloned().collect::<Vec<_>>(), ["CLAUDE.md", "manifest.json", "mcp.json", "settings.json"]);
    let content: Vec<(String, Vec<u8>)> = files.iter().map(|(n, b)| (n.clone(), b.clone())).collect();
    let rebuilt = bundle_from_snapshot_files(&content).unwrap();
    assert_eq!(rebuilt.content_hash(), version["content_hash"].as_str().unwrap());
    let stored: SettingsVersion = serde_json::from_value(version.clone()).unwrap();
    assert_eq!(rebuilt, stored.bundle);
    let manifest: Value = serde_json::from_slice(&files["manifest.json"]).unwrap();
    assert_eq!((manifest["version"].clone(), manifest["content_hash"].clone()), (json!(1), version["content_hash"].clone()));
    for (name, bytes) in &files {
        if name != "manifest.json" {
            assert_eq!(manifest["files"][name], json!(sha256_hex(bytes)), "{name} 的雜湊要與 manifest 一致");
        }
    }
    assert_eq!(snapshot["files"].as_array().unwrap().len(), 4);
    assert!(String::from_utf8(files["CLAUDE.md"].clone()).unwrap().contains("繁體中文"));

    // 唯讀：任何檔案都不能被開啟寫入（Windows 的唯讀屬性不阻止刪除，見 AI-98）
    for name in files.keys() {
        let path = PathBuf::from(directory).join(name);
        assert!(std::fs::OpenOptions::new().write(true).open(&path).is_err(), "{name} 不應可寫");
        assert!(std::fs::write(&path, b"tampered").is_err(), "{name} 不應可被改寫");
    }
    assert_eq!(read_snapshot_dir(directory), files);
}

#[tokio::test]
async fn a_run_stays_pinned_to_its_version_while_later_runs_use_newer_ones() {
    let harness = Harness::start().await;
    plant_ordinary_settings(&harness);
    adopt_current(&harness).await;
    let (ws, goal) = workspace_with_goal(&harness, "w").await;
    let run_a = run_in(&harness, &ws, &goal, "工作甲").await;
    let (_, first) = post(&harness, &snapshot_path(&ws, &run_a), Some(json!({}))).await;
    let before = read_snapshot_dir(first["directory"].as_str().unwrap());

    harness.user_settings.write_settings(r#"{"model":"sonnet"}"#);
    adopt_current(&harness).await;

    // 已准入的 Run：再要一次得到同一份（200），要求別的版本被拒絕；目錄內容沒有被熱重載
    let (status, again) = post(&harness, &snapshot_path(&ws, &run_a), Some(json!({}))).await;
    assert_eq!((status, again.clone()), (StatusCode::OK, first.clone()));
    let (status, rejected) = post(&harness, &snapshot_path(&ws, &run_a), Some(json!({ "version": 2 }))).await;
    assert_eq!((status, error_code(&rejected)), (StatusCode::CONFLICT, "settings_snapshot_pinned"));
    assert_eq!(read_snapshot_dir(first["directory"].as_str().unwrap()), before);

    // 之後的 Run 預設使用最新版本，也可以指定舊版本
    let run_b = run_in(&harness, &ws, &goal, "工作乙").await;
    let (status, latest) = post(&harness, &snapshot_path(&ws, &run_b), Some(json!({}))).await;
    assert_eq!((status, latest["version"].clone()), (StatusCode::CREATED, json!(2)));
    assert_ne!(latest["directory"], first["directory"], "每個 Run 有自己的目錄");
    let run_c = run_in(&harness, &ws, &goal, "工作丙").await;
    let (status, old) = post(&harness, &snapshot_path(&ws, &run_c), Some(json!({ "version": 1 }))).await;
    assert_eq!((status, old["version"].clone(), old["content_hash"].clone()), (StatusCode::CREATED, json!(1), first["content_hash"].clone()));
}

#[tokio::test]
async fn run_settings_snapshot_requires_an_adopted_version_a_real_run_and_a_known_version() {
    let harness = Harness::start().await;
    let (ws, run) = new_run(&harness, "w").await;

    let (status, body) = post(&harness, &snapshot_path(&ws, &run), Some(json!({}))).await;
    assert_eq!((status, error_code(&body)), (StatusCode::CONFLICT, "shared_settings_not_adopted"));

    plant_ordinary_settings(&harness);
    adopt_current(&harness).await;
    let (status, body) = post(&harness, &snapshot_path(&ws, &run), Some(json!({ "version": 9 }))).await;
    assert_eq!((status, error_code(&body)), (StatusCode::NOT_FOUND, "settings_version_not_found"));
    let (status, _) = post(&harness, &snapshot_path(&ws, "run_missing"), Some(json!({}))).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(!harness.data_dir().join("run-settings").join("run_missing").exists(), "找不到的 Run 不建立目錄");
    assert!(!harness.data_dir().join("run-settings").join(&run).exists(), "沒有成功的請求不留下目錄");
}

#[tokio::test]
async fn a_run_snapshot_and_its_version_binding_survive_a_runtime_restart() {
    let mut harness = Harness::start().await;
    plant_ordinary_settings(&harness);
    adopt_current(&harness).await;
    let (ws, run) = new_run(&harness, "w").await;
    let (_, first) = post(&harness, &snapshot_path(&ws, &run), Some(json!({}))).await;
    harness.user_settings.write_settings(r#"{"model":"sonnet"}"#);
    adopt_current(&harness).await;

    harness.restart().await;

    let (status, again) = post(&harness, &snapshot_path(&ws, &run), Some(json!({}))).await;
    assert_eq!((status, again), (StatusCode::OK, first));
}
