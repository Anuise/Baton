//! 情境測試：以既有目錄建立 Account Profile（desktop-controller-cli 票 02，使用者故事 64～67），
//! 經由 Runtime harness 的真實 HTTP API 驗證。
//!
//! 「既有目錄」是資料目錄外、使用者自己登入過的 Claude Code 設定目錄（例如 `~/.claude-Work-3`）；
//! 登入狀態由 fake 的 ClaudeEnvironment Port 腳本化，其餘（Store、Domain、API）都是真實的。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use baton_harness::Harness;
use baton_runtime::claude_adapter::{ClaudeCodeAdapter, RUNS_DIR};
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

const PROFILES: &str = "/api/v1/profiles";
const FROM_EXISTING_DIR: &str = "/api/v1/profiles/from-existing-dir";
const WORKSPACES: &str = "/api/v1/workspaces";

/// 使用者家目錄的替身：放在 harness 的暫存資料目錄內（測試結束一併清除），但不在平台的 profiles 底下。
fn home(harness: &Harness) -> PathBuf {
    harness.data_dir().join("home")
}

/// 在平台外以 Claude Code 登入過的既有設定目錄。
fn logged_in_dir(harness: &Harness, name: &str, account: &str) -> PathBuf {
    let dir = home(harness).join(name);
    harness.claude_env.log_in(&dir, account);
    // 使用者在平台外的使用痕跡；封存後必須原封不動
    std::fs::write(dir.join("settings.json"), r#"{"theme":"dark"}"#).unwrap();
    std::fs::create_dir_all(dir.join("projects").join("C--repo")).unwrap();
    std::fs::write(dir.join("projects").join("C--repo").join("session.jsonl"), "{}\n").unwrap();
    dir
}

async fn from_existing_dir(harness: &Harness, name: &str, dir: &Path) -> (StatusCode, Value) {
    let body = json!({ "name": name, "config_dir": dir });
    harness.send_json(Method::POST, FROM_EXISTING_DIR, Some(body)).await
}

async fn from_existing_dir_ok(harness: &Harness, name: &str, dir: &Path) -> Value {
    let (status, profile) = from_existing_dir(harness, name, dir).await;
    assert_eq!(status, StatusCode::CREATED, "{profile}");
    profile
}

async fn profiles(harness: &Harness) -> Vec<Value> {
    let (status, body) = harness.send_json(Method::GET, PROFILES, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["profiles"].as_array().expect("profiles 必須是陣列").clone()
}

fn error_code(body: &Value) -> &str {
    body["error"].as_str().unwrap_or_default()
}

fn blocker_codes(profile: &Value) -> Vec<String> {
    profile["blockers"].as_array().unwrap().iter().map(|b| b["code"].as_str().unwrap().to_string()).collect()
}

/// 目錄底下每個檔案的相對路徑與內容，用來確認平台沒有刪除或修改任何東西。
fn snapshot(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                out.insert(path.strip_prefix(root).unwrap().to_path_buf(), Vec::new());
                walk(root, &path, out);
            } else {
                out.insert(path.strip_prefix(root).unwrap().to_path_buf(), std::fs::read(&path).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

// ───────────── 建立與立即核對 ─────────────

#[tokio::test]
async fn an_existing_logged_in_dir_becomes_a_dispatchable_external_profile_without_a_new_login() {
    let harness = Harness::start().await;
    let dir = logged_in_dir(&harness, ".claude-Work-3", "work3@example.com");
    let checks_before = harness.claude_env.auth_check_count();

    let profile = from_existing_dir_ok(&harness, "Work 3", &dir).await;

    assert_eq!(profile["name"], "Work 3");
    assert_eq!(profile["config_dir"], dir.to_string_lossy().as_ref());
    assert_eq!(profile["config_dir_origin"], "external");
    // 建立後立即核對：不必再按「重新核對」，也沒有開啟登入終端機
    assert_eq!(harness.claude_env.auth_check_count(), checks_before + 1);
    assert_eq!(profile["verification"]["logged_in"], true);
    assert_eq!(profile["verification"]["account"], "work3@example.com");
    assert_eq!(profile["expected_account"], "work3@example.com", "第一次核對到的原生登入帳號成為預期身分");
    assert_eq!(profile["dispatchable"], true, "{profile}");
    assert!(harness.claude_env.login_terminals().is_empty());
    // 帳號畫面讀到的就是同一份結果
    assert_eq!(profiles(&harness).await, vec![profile]);
}

#[tokio::test]
async fn an_existing_dir_that_is_not_logged_in_is_registered_but_shows_why_it_cannot_dispatch() {
    let harness = Harness::start().await;
    let dir = home(&harness).join(".claude-empty");
    std::fs::create_dir_all(&dir).unwrap();

    let profile = from_existing_dir_ok(&harness, "空的", &dir).await;

    assert_eq!(profile["verification"]["logged_in"], false);
    assert_eq!(profile["dispatchable"], false);
    assert_eq!(blocker_codes(&profile), ["profile_not_logged_in"]);
}

#[tokio::test]
async fn an_existing_dir_logged_in_as_an_account_another_profile_already_uses_is_flagged() {
    let harness = Harness::start().await;
    harness.add_account("平台建立", "same@example.com").await;
    let dir = logged_in_dir(&harness, ".claude-Work-1", "same@example.com");

    let profile = from_existing_dir_ok(&harness, "Work 1", &dir).await;

    assert_eq!(profile["dispatchable"], false);
    assert_eq!(blocker_codes(&profile), ["duplicate_identity"]);
}

#[tokio::test]
async fn when_the_immediate_check_fails_the_profile_is_still_registered_and_can_be_rechecked() {
    let harness = Harness::start().await;
    let dir = logged_in_dir(&harness, ".claude-Work-5", "work5@example.com");
    harness.claude_env.fail_auth_checks("claude 沒有回應");

    let profile = from_existing_dir_ok(&harness, "Work 5", &dir).await;

    assert_eq!(profile["verification"], Value::Null);
    assert_eq!(blocker_codes(&profile), ["profile_unverified"]);
    assert_eq!(profiles(&harness).await.len(), 1);
}

#[tokio::test]
async fn external_profiles_and_their_origin_survive_rebuilding_the_runtime() {
    let mut harness = Harness::start().await;
    let dir = logged_in_dir(&harness, ".claude-Work-3", "work3@example.com");
    from_existing_dir_ok(&harness, "Work 3", &dir).await;
    harness.add_account("平台建立", "managed@example.com").await;
    let before = profiles(&harness).await;

    harness.restart().await;

    assert_eq!(profiles(&harness).await, before);
    let origins: Vec<&str> = before.iter().map(|p| p["config_dir_origin"].as_str().unwrap()).collect();
    assert_eq!(origins, ["external", "managed"]);
}

// ───────────── 拒絕 ─────────────

#[tokio::test]
async fn a_missing_dir_or_a_file_is_rejected_with_the_reason_and_creates_nothing() {
    let harness = Harness::start().await;
    let missing = home(&harness).join(".claude-nope");
    let file = home(&harness).join("not-a-dir.txt");
    std::fs::create_dir_all(home(&harness)).unwrap();
    std::fs::write(&file, "x").unwrap();

    let (status, body) = from_existing_dir(&harness, "A", &missing).await;
    assert_eq!((status, error_code(&body)), (StatusCode::BAD_REQUEST, "profile_config_dir_not_found"), "{body}");
    assert!(body["message"].as_str().unwrap().contains(".claude-nope"), "{body}");
    let (status, body) = from_existing_dir(&harness, "A", &file).await;
    assert_eq!((status, error_code(&body)), (StatusCode::BAD_REQUEST, "profile_config_dir_not_directory"), "{body}");
    let (status, body) =
        harness.send_json(Method::POST, FROM_EXISTING_DIR, Some(json!({ "name": "A", "config_dir": "relative\\dir" }))).await;
    assert_eq!((status, error_code(&body)), (StatusCode::BAD_REQUEST, "profile_config_dir_invalid"), "{body}");
    let (status, body) = harness.send_json(Method::POST, FROM_EXISTING_DIR, Some(json!({ "name": "A" }))).await;
    assert_eq!((status, error_code(&body)), (StatusCode::BAD_REQUEST, "bad_request"), "{body}");

    assert!(profiles(&harness).await.is_empty());
    assert!(!missing.exists(), "平台不替使用者建立外部目錄");
    assert_eq!(harness.claude_env.auth_check_count(), 0, "被拒時不核對");
}

#[tokio::test]
async fn a_dir_that_is_the_same_as_or_nested_with_another_profiles_dir_is_rejected() {
    let harness = Harness::start().await;
    let work3 = logged_in_dir(&harness, ".claude-Work-3", "work3@example.com");
    from_existing_dir_ok(&harness, "Work 3", &work3).await;
    let nested = work3.join("projects");
    let upper_case = PathBuf::from(work3.to_string_lossy().to_uppercase());

    for overlapping in [work3.clone(), upper_case, nested, home(&harness)] {
        let (status, body) = from_existing_dir(&harness, "另一個", &overlapping).await;
        assert_eq!((status, error_code(&body)), (StatusCode::CONFLICT, "profile_config_dir_shared"), "{overlapping:?}");
        assert!(body["message"].as_str().unwrap().contains("相同或互相包含"), "{body}");
    }
    assert_eq!(profiles(&harness).await.len(), 1);
}

#[tokio::test]
async fn a_dir_under_the_platforms_profiles_dir_is_rejected() {
    let harness = Harness::start().await;
    let managed_root = harness.data_dir().join("profiles");
    let inside = managed_root.join("my-own");
    std::fs::create_dir_all(&inside).unwrap();

    for dir in [inside, managed_root, harness.data_dir().to_path_buf()] {
        let (status, body) = from_existing_dir(&harness, "A", &dir).await;
        assert_eq!((status, error_code(&body)), (StatusCode::BAD_REQUEST, "profile_config_dir_managed"), "{dir:?}: {body}");
        assert!(body["message"].as_str().unwrap().contains("profiles"), "{body}");
    }
    assert!(profiles(&harness).await.is_empty());
}

#[tokio::test]
async fn names_and_the_profile_limit_apply_to_existing_dirs_too() {
    let harness = Harness::start().await;
    for n in 1..=8 {
        let (status, body) = harness.send_json(Method::POST, PROFILES, Some(json!({ "name": format!("帳號 {n}") }))).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }
    let dir = logged_in_dir(&harness, ".claude-Work-9", "work9@example.com");

    let (status, body) = from_existing_dir(&harness, "帳號 1", &dir).await;
    assert_eq!((status, error_code(&body)), (StatusCode::CONFLICT, "profile_name_taken"));
    let (status, body) = from_existing_dir(&harness, "Work 9", &dir).await;
    assert_eq!((status, error_code(&body)), (StatusCode::CONFLICT, "profile_limit_reached"));
    assert_eq!(profiles(&harness).await.len(), 8);
}

// ───────────── 封存 ─────────────

#[tokio::test]
async fn archiving_an_external_profile_neither_deletes_nor_modifies_its_dir() {
    let harness = Harness::start().await;
    let dir = logged_in_dir(&harness, ".claude-Work-3", "work3@example.com");
    let before = snapshot(&dir);
    let profile = from_existing_dir_ok(&harness, "Work 3", &dir).await;

    let (status, archived) =
        harness.send_json(Method::POST, &format!("{PROFILES}/{}/archive", profile["id"].as_str().unwrap()), None).await;

    assert_eq!(status, StatusCode::OK, "{archived}");
    assert_eq!((archived["status"].as_str(), archived["config_dir_origin"].as_str()), (Some("archived"), Some("external")));
    assert_eq!(snapshot(&dir), before, "建立、核對、封存都不改動使用者的目錄");
    // 封存釋出名稱，但目錄仍不能再登記成另一個 Profile（登入資料還在那裡）
    let (status, again) = from_existing_dir(&harness, "Work 3", &dir).await;
    assert_eq!((status, error_code(&again)), (StatusCode::CONFLICT, "profile_config_dir_shared"));
}

#[tokio::test]
async fn archiving_a_platform_created_profile_behaves_as_before() {
    let harness = Harness::start().await;
    let id = harness.add_account("平台建立", "managed@example.com").await;
    let managed = profiles(&harness).await.into_iter().find(|p| p["id"] == id.as_str()).unwrap();
    assert_eq!(managed["config_dir_origin"], "managed");
    let dir = PathBuf::from(managed["config_dir"].as_str().unwrap());
    let before = snapshot(&dir);

    let (status, archived) = harness.send_json(Method::POST, &format!("{PROFILES}/{id}/archive"), None).await;

    assert_eq!(status, StatusCode::OK, "{archived}");
    assert_eq!(archived["status"], "archived");
    assert_eq!(snapshot(&dir), before);
}

// ───────────── 派工 ─────────────

/// 用正式的 Claude Code Adapter 組出啟動指令（程序控制仍是 fake），才能看到 Worker 實際拿到的 `CLAUDE_CONFIG_DIR`。
async fn harness_with_claude_adapter() -> Harness {
    Harness::builder()
        .agent_adapter(|data_dir| {
            Arc::new(
                ClaudeCodeAdapter::new("claude".into(), "C:\\Baton\\bin\\baton-mcp-shim.exe".into(), data_dir, Vec::new())
                    .with_poll(Duration::from_millis(5)),
            )
        })
        .start()
        .await
}

fn filled(text: &str) -> Value {
    json!({ "status": "filled", "text": text })
}

/// 建立 Workspace、Goal 與 ready 的 Task 並派工，回傳 Run 識別。
async fn dispatch_one(harness: &Harness) -> String {
    let dir = harness.data_dir().join("projects").join("main");
    std::fs::create_dir_all(&dir).unwrap();
    let (status, ws) = harness.send_json(Method::POST, WORKSPACES, Some(json!({ "path": dir }))).await;
    assert_eq!(status, StatusCode::CREATED, "{ws}");
    let ws = ws["id"].as_str().unwrap();
    let (status, goal) =
        harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/goals"), Some(json!({ "title": "目標" }))).await;
    assert_eq!(status, StatusCode::CREATED, "{goal}");
    let contract = json!({
        "objective": filled("工作"), "inputs": filled("既有的資料"), "scope": filled("公開資料"),
        "deliverables": filled("比較表"), "done_criteria": filled("五個產品"), "blocker_handling": filled("卡住時回報"),
        "required_capabilities": [],
    });
    let body = json!({ "goal_id": goal["id"], "title": "工作", "contract": contract });
    let (status, task) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks"), Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    let task_path = format!("{WORKSPACES}/{ws}/tasks/{}", task["id"].as_str().unwrap());
    let (status, ready) = harness.send_json(Method::POST, &format!("{task_path}/ready"), None).await;
    assert_eq!(status, StatusCode::OK, "{ready}");
    let (status, run) = harness.send_json(Method::POST, &format!("{task_path}/dispatch"), None).await;
    assert_eq!(status, StatusCode::CREATED, "{run}");
    run["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn a_worker_run_on_an_external_profile_uses_that_dir_as_its_claude_config_dir() {
    let harness = harness_with_claude_adapter().await;
    let dir = logged_in_dir(&harness, ".claude-Work-3", "work3@example.com");
    let profile = from_existing_dir_ok(&harness, "Work 3", &dir).await;
    assert_eq!(profile["dispatchable"], true, "{profile}");

    let run = dispatch_one(&harness).await;
    harness.supervise().await;

    let run_dir = harness.data_dir().join(RUNS_DIR).join(&run).to_string_lossy().into_owned();
    let spec = harness
        .process_control
        .spawned()
        .into_iter()
        .find(|spec| spec.stdout_file.as_deref().is_some_and(|f| f.starts_with(&run_dir)))
        .unwrap_or_else(|| panic!("Run {run} 沒有被啟動"));
    assert!(
        spec.env.contains(&("CLAUDE_CONFIG_DIR".to_string(), dir.to_string_lossy().into_owned())),
        "{:?}",
        spec.env
    );
    assert_eq!(snapshot(&dir).get(Path::new("settings.json")).map(Vec::as_slice), Some(br#"{"theme":"dark"}"#.as_slice()));
}
