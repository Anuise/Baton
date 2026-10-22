//! 情境測試：帳號 Profile 與首次啟動精靈（票 07），經由 Runtime harness 的真實 HTTP API 驗證。
//!
//! Claude Code、Git 與登入狀態都由 fake 的 ClaudeEnvironment Port 腳本化；
//! 其餘（Store、Domain、API）都是真實的。

use std::path::{Path, PathBuf};

use baton_domain::profile::AuthSource;
use baton_domain::setup::ToolProbe;
use baton_domain::Timestamp;
use baton_harness::Harness;
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

const PROFILES: &str = "/api/v1/profiles";
const SETUP: &str = "/api/v1/setup";
const RECHECK: &str = "/api/v1/setup/recheck";
const ACK_NOTICE: &str = "/api/v1/setup/background-notice/acknowledge";

/// 2023-11-14 22:13:20.123 UTC
const CLOCK_START: Timestamp = Timestamp::from_millis(1_700_000_000_123);

fn found(version_output: &str) -> ToolProbe {
    ToolProbe::Found { version_output: Some(version_output.to_string()) }
}

async fn add_profile(harness: &Harness, body: Value) -> (StatusCode, Value) {
    harness.send_json(Method::POST, PROFILES, Some(body)).await
}

/// 建立一個 Profile 並要求成功。
async fn add_ok(harness: &Harness, name: &str) -> Value {
    let (status, profile) = add_profile(harness, json!({ "name": name })).await;
    assert_eq!(status, StatusCode::CREATED, "{profile}");
    profile
}

async fn profiles(harness: &Harness) -> Vec<Value> {
    let (status, body) = harness.send_json(Method::GET, PROFILES, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["profiles"].as_array().expect("profiles 必須是陣列").clone()
}

async fn profile_of(harness: &Harness, id: &str) -> Value {
    profiles(harness).await.into_iter().find(|p| p["id"] == id).expect("找不到 Profile")
}

async fn verify(harness: &Harness, id: &str) -> Value {
    let (status, body) = harness.send_json(Method::POST, &format!("{PROFILES}/{id}/verify"), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

fn dir_of(profile: &Value) -> PathBuf {
    PathBuf::from(profile["config_dir"].as_str().expect("config_dir"))
}

fn blocker_codes(profile: &Value) -> Vec<String> {
    profile["blockers"].as_array().unwrap().iter().map(|b| b["code"].as_str().unwrap().to_string()).collect()
}

async fn setup(harness: &Harness) -> Value {
    let (status, body) = harness.send_json(Method::GET, SETUP, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

fn error_code(body: &Value) -> &str {
    body["error"].as_str().unwrap_or_default()
}

/// 收集資料目錄中的資料庫（含 WAL）、日誌與其他非 Profile 目錄檔案的全部位元組。
fn persisted_bytes(harness: &Harness) -> Vec<u8> {
    fn walk(dir: &Path, skip: &Path, out: &mut Vec<u8>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.starts_with(skip) {
                continue;
            }
            if path.is_dir() {
                walk(&path, skip, out);
            } else if let Ok(bytes) = std::fs::read(&path) {
                out.extend(bytes);
            }
        }
    }
    let mut out = Vec::new();
    // Profile 的原生設定目錄本來就由 Claude Code 保管登入資料，不屬於 Baton 的資料庫、日誌或匯出
    walk(harness.data_dir(), &harness.data_dir().join("profiles"), &mut out);
    out
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle.as_bytes())
}

// ───────────── 首次啟動精靈 ─────────────

#[tokio::test]
async fn wizard_reports_claude_code_and_git_found_on_path_with_versions() {
    let harness = Harness::start().await;
    harness.claude_env.set_claude(found("2.1.307 (Claude Code)"));
    harness.claude_env.set_git(found("git version 2.54.0.windows.1"));

    let status = setup(&harness).await;

    assert_eq!(status["claude_code"]["status"], "ok");
    assert_eq!(status["claude_code"]["version"], "2.1.307");
    assert_eq!(status["claude_code"]["guidance"], json!([]));
    assert_eq!(status["git"]["status"], "ok");
    assert_eq!(status["git"]["version"], "2.54.0.windows.1");
}

#[tokio::test]
async fn missing_tools_show_official_commands_and_nothing_is_downloaded_or_executed() {
    let harness = Harness::start().await;
    harness.claude_env.set_claude(ToolProbe::NotFound);
    harness.claude_env.set_git(ToolProbe::NotFound);

    let status = setup(&harness).await;

    assert_eq!(status["claude_code"]["status"], "missing");
    let commands: Vec<&str> =
        status["claude_code"]["guidance"].as_array().unwrap().iter().map(|g| g["command"].as_str().unwrap()).collect();
    assert!(commands.contains(&"irm https://claude.ai/install.ps1 | iex"), "{commands:?}");
    assert!(status["claude_code"]["message"].as_str().unwrap().contains("重新檢查"));
    assert_eq!(status["git"]["status"], "missing");
    assert_eq!(status["git"]["guidance"][0]["command"], "winget install --id Git.Git -e --source winget");
    assert_eq!(status["ready"], false);

    // 精靈只顯示指令：沒有啟動任何程序，Port 上也沒有任何安裝動作
    assert_eq!(harness.process_control.spawn_count(), 0);
    assert!(harness.claude_env.login_terminals().is_empty());
}

#[tokio::test]
async fn too_old_claude_code_is_reported_with_the_update_command() {
    let harness = Harness::start().await;
    harness.claude_env.set_claude(found("1.0.30 (Claude Code)"));

    let status = setup(&harness).await;

    assert_eq!(status["claude_code"]["status"], "too_old");
    assert_eq!(status["claude_code"]["guidance"][0]["command"], "claude update");
    assert_eq!(status["claude_code"]["minimum_version"], "2.1.307");
}

#[tokio::test]
async fn recheck_picks_up_a_tool_the_user_installed_in_the_meantime() {
    let harness = Harness::start().await;
    harness.claude_env.set_claude(ToolProbe::NotFound);
    assert_eq!(setup(&harness).await["claude_code"]["status"], "missing");

    // 使用者自己照指令安裝了
    harness.claude_env.set_claude(found("2.1.307 (Claude Code)"));
    let (status, rechecked) = harness.send_json(Method::POST, RECHECK, None).await;

    assert_eq!(status, StatusCode::OK, "{rechecked}");
    assert_eq!(rechecked["claude_code"]["status"], "ok");
    assert_eq!(harness.process_control.spawn_count(), 0);
}

#[tokio::test]
async fn a_failing_probe_is_unknown_and_does_not_break_the_wizard() {
    let harness = Harness::start().await;
    harness.claude_env.fail_tool_probes("逾時");

    let status = setup(&harness).await;

    assert_eq!(status["claude_code"]["status"], "unknown");
    assert!(status["claude_code"]["message"].as_str().unwrap().contains("逾時"));
    assert_eq!(status["ready"], false);
}

#[tokio::test]
async fn wizard_lists_each_profiles_login_state_and_is_ready_only_when_everything_works() {
    let harness = Harness::start().await;
    harness.claude_env.set_claude(found("2.1.307 (Claude Code)"));
    harness.claude_env.set_git(found("git version 2.54.0.windows.1"));
    let profile = add_ok(&harness, "主要帳號").await;
    let id = profile["id"].as_str().unwrap();
    assert_eq!(setup(&harness).await["ready"], false, "還沒有可派工的 Profile");

    harness.claude_env.log_in(&dir_of(&profile), "me@example.com");
    let (_, rechecked) = harness.send_json(Method::POST, RECHECK, None).await;

    assert_eq!(rechecked["ready"], true);
    let listed = &rechecked["profiles"][0];
    assert_eq!((listed["id"].as_str().unwrap(), listed["dispatchable"].clone()), (id, json!(true)));
    assert_eq!(listed["verification"]["account"], "me@example.com");
    assert_eq!(rechecked["profile_limit"], 8);
}

#[tokio::test]
async fn background_behavior_notice_is_shown_until_acknowledged_and_the_acknowledgement_persists() {
    let mut harness = Harness::builder().clock_at(CLOCK_START).start().await;
    let before = setup(&harness).await;
    assert_eq!(before["background_notice"]["acknowledged_at"], Value::Null);
    let text = before["background_notice"]["text"].as_str().unwrap();
    assert!(text.contains("背景") && text.contains("完全退出"), "{text}");

    let (status, acked) = harness.send_json(Method::POST, ACK_NOTICE, None).await;
    assert_eq!(status, StatusCode::OK, "{acked}");
    assert_eq!(acked["background_notice"]["acknowledged_at"], CLOCK_START.as_millis());

    // 再按一次不會改變時間，也不會多一個事件
    harness.clock.advance_millis(60_000);
    let (_, again) = harness.send_json(Method::POST, ACK_NOTICE, None).await;
    assert_eq!(again["background_notice"]["acknowledged_at"], CLOCK_START.as_millis());
    let events: i64 = harness
        .db()
        .query_row("SELECT COUNT(*) FROM event WHERE event_type = 'background_notice_acknowledged'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(events, 1);

    harness.restart().await;
    assert_eq!(setup(&harness).await["background_notice"]["acknowledged_at"], CLOCK_START.as_millis());
}

// ───────────── Profile 與原生設定目錄 ─────────────

#[tokio::test]
async fn each_profile_gets_its_own_config_dir_under_the_data_dir_and_none_are_shared() {
    let harness = Harness::start().await;

    let mut dirs = Vec::new();
    for n in 1..=3 {
        let profile = add_ok(&harness, &format!("帳號 {n}")).await;
        let dir = dir_of(&profile);
        assert!(dir.is_dir(), "設定目錄必須已建立：{}", dir.display());
        assert!(dir.starts_with(harness.data_dir().join("profiles")), "{}", dir.display());
        dirs.push(dir);
    }

    for (i, a) in dirs.iter().enumerate() {
        for b in dirs.iter().skip(i + 1) {
            assert_ne!(a, b);
            assert!(!a.starts_with(b) && !b.starts_with(a), "設定目錄不可互相包含：{a:?} {b:?}");
        }
    }
}

#[tokio::test]
async fn login_state_of_one_profile_is_independent_of_the_others() {
    let harness = Harness::start().await;
    let a = add_ok(&harness, "A").await;
    let b = add_ok(&harness, "B").await;
    harness.claude_env.log_in(&dir_of(&a), "a@example.com");

    let verified_a = verify(&harness, a["id"].as_str().unwrap()).await;
    let verified_b = verify(&harness, b["id"].as_str().unwrap()).await;

    assert_eq!(verified_a["verification"]["logged_in"], true);
    assert_eq!(verified_b["verification"]["logged_in"], false, "A 的登入不會出現在 B");
    assert_eq!(blocker_codes(&verified_b), ["profile_not_logged_in"]);
}

#[tokio::test]
async fn profile_names_must_be_valid_and_unique_and_invalid_requests_create_nothing() {
    let harness = Harness::start().await;
    add_ok(&harness, "Work").await;

    let (status, dup) = add_profile(&harness, json!({ "name": "work" })).await;
    assert_eq!((status, error_code(&dup)), (StatusCode::CONFLICT, "profile_name_taken"));
    let (status, empty) = add_profile(&harness, json!({ "name": "  " })).await;
    assert_eq!((status, error_code(&empty)), (StatusCode::BAD_REQUEST, "profile_name_invalid"));
    let (status, bad_account) = add_profile(&harness, json!({ "name": "X", "expected_account": "nope" })).await;
    assert_eq!((status, error_code(&bad_account)), (StatusCode::BAD_REQUEST, "profile_account_invalid"));
    let (status, malformed) = add_profile(&harness, json!({ "nickname": "X" })).await;
    assert_eq!((status, error_code(&malformed)), (StatusCode::BAD_REQUEST, "bad_request"));

    assert_eq!(profiles(&harness).await.len(), 1);
    assert_eq!(std::fs::read_dir(harness.data_dir().join("profiles")).unwrap().count(), 1, "被拒絕的請求不留下設定目錄");
}

#[tokio::test]
async fn the_ninth_profile_is_rejected_with_the_limit_explained_and_creates_nothing() {
    let harness = Harness::start().await;
    for n in 1..=8 {
        add_ok(&harness, &format!("帳號 {n}")).await;
    }

    let (status, rejected) = add_profile(&harness, json!({ "name": "帳號 9" })).await;

    assert_eq!(status, StatusCode::CONFLICT, "{rejected}");
    assert_eq!(error_code(&rejected), "profile_limit_reached");
    assert!(rejected["message"].as_str().unwrap().contains('8'), "{rejected}");
    assert_eq!(profiles(&harness).await.len(), 8);
    assert_eq!(std::fs::read_dir(harness.data_dir().join("profiles")).unwrap().count(), 8);
}

#[tokio::test]
async fn archiving_a_profile_frees_a_slot_but_never_reuses_its_config_dir() {
    let harness = Harness::start().await;
    let mut first = Value::Null;
    for n in 1..=8 {
        let profile = add_ok(&harness, &format!("帳號 {n}")).await;
        if n == 1 {
            first = profile;
        }
    }
    let (status, archived) =
        harness.send_json(Method::POST, &format!("{PROFILES}/{}/archive", first["id"].as_str().unwrap()), None).await;
    assert_eq!(status, StatusCode::OK, "{archived}");
    assert_eq!(archived["status"], "archived");
    assert_eq!(blocker_codes(&archived), ["profile_archived"]);

    let replacement = add_ok(&harness, "帳號 9").await;

    assert_ne!(dir_of(&replacement), dir_of(&first));
    assert!(dir_of(&first).is_dir(), "封存不刪除登入資料所在的目錄");
    let (status, again) =
        harness.send_json(Method::POST, &format!("{PROFILES}/{}/archive", first["id"].as_str().unwrap()), None).await;
    assert_eq!((status, error_code(&again)), (StatusCode::CONFLICT, "profile_already_archived"));
}

#[tokio::test]
async fn unknown_profile_endpoints_report_a_stable_not_found() {
    let harness = Harness::start().await;

    for path in ["/api/v1/profiles/prof_nope/verify", "/api/v1/profiles/prof_nope/login-terminal", "/api/v1/profiles/prof_nope/archive"] {
        let (status, body) = harness.send_json(Method::POST, path, None).await;
        assert_eq!((status, error_code(&body)), (StatusCode::NOT_FOUND, "profile_not_found"), "{path}");
    }
    assert!(harness.claude_env.login_terminals().is_empty());
}

#[tokio::test]
async fn profiles_and_verification_results_survive_rebuilding_the_runtime() {
    let mut harness = Harness::builder().clock_at(CLOCK_START).start().await;
    let profile = add_ok(&harness, "主要帳號").await;
    harness.claude_env.log_in(&dir_of(&profile), "me@example.com");
    verify(&harness, profile["id"].as_str().unwrap()).await;
    let before = profiles(&harness).await;

    harness.restart().await;

    assert_eq!(profiles(&harness).await, before);
    assert_eq!(before[0]["verification"]["checked_at"], CLOCK_START.as_millis());
}

// ───────────── 原生登入（App 開啟的終端機）與身分核對 ─────────────

#[tokio::test]
async fn login_terminal_opens_for_exactly_the_chosen_profile_and_then_verification_sees_the_login() {
    let harness = Harness::start().await;
    let a = add_ok(&harness, "A").await;
    let b = add_ok(&harness, "B").await;
    harness.claude_env.script_terminal_login("b@example.com");

    let (status, opened) =
        harness.send_json(Method::POST, &format!("{PROFILES}/{}/login-terminal", b["id"].as_str().unwrap()), None).await;

    assert_eq!(status, StatusCode::OK, "{opened}");
    assert_eq!(opened["opened"], true);
    assert_eq!(harness.claude_env.login_terminals(), vec![dir_of(&b)], "只為選定的 Profile 開啟，且使用它自己的設定目錄");
    assert_eq!(harness.process_control.spawn_count(), 0);

    let verified_b = verify(&harness, b["id"].as_str().unwrap()).await;
    let verified_a = verify(&harness, a["id"].as_str().unwrap()).await;
    assert_eq!(verified_b["verification"]["account"], "b@example.com");
    assert_eq!(verified_b["dispatchable"], true);
    assert_eq!(verified_a["dispatchable"], false);
}

#[tokio::test]
async fn first_native_login_pins_the_identity_and_later_logins_as_someone_else_are_blocked() {
    let harness = Harness::start().await;
    let profile = add_ok(&harness, "A").await;
    let id = profile["id"].as_str().unwrap();
    assert_eq!(blocker_codes(&profile_of(&harness, id).await), ["profile_unverified"], "尚未核對前不可派工");

    harness.claude_env.log_in(&dir_of(&profile), "a@example.com");
    let first = verify(&harness, id).await;
    assert_eq!(first["dispatchable"], true);
    assert_eq!(first["expected_account"], "a@example.com");

    // 使用者之後在這個 Profile 的終端機改登入了別的帳號
    harness.claude_env.log_in(&dir_of(&profile), "someone-else@example.com");
    let second = verify(&harness, id).await;

    assert_eq!(second["dispatchable"], false);
    assert_eq!(blocker_codes(&second), ["identity_mismatch"]);
    let message = second["blockers"][0]["message"].as_str().unwrap();
    assert!(message.contains("a@example.com") && message.contains("someone-else@example.com"), "{message}");
    assert_eq!(second["expected_account"], "a@example.com", "不符時不會悄悄改寫預期身分");
}

#[tokio::test]
async fn identity_declared_at_creation_is_enforced_from_the_first_verification() {
    let harness = Harness::start().await;
    let (_, profile) = add_profile(&harness, json!({ "name": "A", "expected_account": "a@example.com" })).await;
    harness.claude_env.log_in(&dir_of(&profile), "b@example.com");

    let verified = verify(&harness, profile["id"].as_str().unwrap()).await;

    assert_eq!(verified["dispatchable"], false);
    assert_eq!(blocker_codes(&verified), ["identity_mismatch"]);
}

#[tokio::test]
async fn environment_override_of_the_auth_source_is_flagged_and_blocks_dispatch_until_removed() {
    let harness = Harness::start().await;
    let profile = add_ok(&harness, "A").await;
    let id = profile["id"].as_str().unwrap();
    harness.claude_env.log_in(&dir_of(&profile), "a@example.com");
    assert_eq!(verify(&harness, id).await["dispatchable"], true);

    // 環境變數 ANTHROPIC_API_KEY 蓋過了這個 Profile 的原生登入
    harness.claude_env.set_env_override(Some((AuthSource::ApiKey, Some("ANTHROPIC_API_KEY"))));
    let overridden = verify(&harness, id).await;

    assert_eq!(overridden["dispatchable"], false);
    assert_eq!(blocker_codes(&overridden), ["auth_source_overridden"]);
    assert_eq!(overridden["verification"]["source"], "api_key");
    let message = overridden["blockers"][0]["message"].as_str().unwrap();
    assert!(message.contains("ANTHROPIC_API_KEY") && message.contains("阻擋派工"), "{message}");
    assert_eq!(overridden["expected_account"], "a@example.com");

    // 阻擋狀態可由帳號頁讀到，且重啟後仍在
    assert_eq!(blocker_codes(&profile_of(&harness, id).await), ["auth_source_overridden"]);

    harness.claude_env.set_env_override(None);
    let restored = verify(&harness, id).await;
    assert_eq!(restored["dispatchable"], true);
    assert_eq!(blocker_codes(&restored), Vec::<String>::new());
}

#[tokio::test]
async fn credential_helper_override_on_first_check_never_pins_an_identity() {
    let harness = Harness::start().await;
    let profile = add_ok(&harness, "A").await;
    harness.claude_env.set_env_override(Some((AuthSource::ApiKey, Some("apiKeyHelper"))));

    let verified = verify(&harness, profile["id"].as_str().unwrap()).await;

    assert_eq!(blocker_codes(&verified), ["auth_source_overridden"]);
    assert_eq!(verified["expected_account"], Value::Null);
}

#[tokio::test]
async fn the_same_account_cannot_be_dispatched_through_two_profiles() {
    let harness = Harness::start().await;
    let first = add_ok(&harness, "A").await;
    let second = add_ok(&harness, "B").await;
    harness.claude_env.log_in(&dir_of(&first), "me@example.com");
    harness.claude_env.log_in(&dir_of(&second), "ME@example.com");

    let verified_first = verify(&harness, first["id"].as_str().unwrap()).await;
    let verified_second = verify(&harness, second["id"].as_str().unwrap()).await;

    assert_eq!(verified_first["dispatchable"], true);
    assert_eq!(blocker_codes(&verified_second), ["duplicate_identity"]);
}

#[tokio::test]
async fn a_failing_auth_check_reports_an_error_and_keeps_the_previous_result() {
    let harness = Harness::start().await;
    let profile = add_ok(&harness, "A").await;
    let id = profile["id"].as_str().unwrap();
    harness.claude_env.log_in(&dir_of(&profile), "a@example.com");
    let good = verify(&harness, id).await;
    harness.claude_env.fail_auth_checks("claude 沒有回應");

    let (status, body) = harness.send_json(Method::POST, &format!("{PROFILES}/{id}/verify"), None).await;

    assert_eq!((status, error_code(&body)), (StatusCode::BAD_GATEWAY, "profile_check_failed"), "{body}");
    assert!(body["message"].as_str().unwrap().contains("claude 沒有回應"));
    assert_eq!(profile_of(&harness, id).await, good);
}

#[tokio::test]
async fn recheck_reverifies_every_active_profile() {
    let harness = Harness::start().await;
    harness.claude_env.set_claude(found("2.1.307 (Claude Code)"));
    harness.claude_env.set_git(found("git version 2.54.0.windows.1"));
    let a = add_ok(&harness, "A").await;
    let b = add_ok(&harness, "B").await;
    let gone = add_ok(&harness, "已封存").await;
    harness.send_json(Method::POST, &format!("{PROFILES}/{}/archive", gone["id"].as_str().unwrap()), None).await;
    harness.claude_env.log_in(&dir_of(&a), "a@example.com");
    harness.claude_env.log_in(&dir_of(&b), "b@example.com");
    harness.claude_env.reset_auth_check_count();

    let (_, rechecked) = harness.send_json(Method::POST, RECHECK, None).await;

    let dispatchable: Vec<bool> =
        rechecked["profiles"].as_array().unwrap().iter().map(|p| p["dispatchable"].as_bool().unwrap()).collect();
    assert_eq!(dispatchable, [true, true], "只列出使用中的 Profile");
    assert_eq!(harness.claude_env.auth_check_count(), 2, "已封存的 Profile 不核對");
}

// ───────────── 登入資料不進資料庫、日誌或匯出 ─────────────

#[tokio::test]
async fn login_credentials_never_reach_the_database_the_log_or_any_api_response() {
    let harness = Harness::start().await;
    let profile = add_ok(&harness, "A").await;
    let id = profile["id"].as_str().unwrap();
    let secret = harness.claude_env.log_in(&dir_of(&profile), "a@example.com");
    assert!(
        std::fs::read_to_string(dir_of(&profile).join(".credentials.json")).unwrap().contains(&secret),
        "fake 與真實的 Claude Code 一樣，把登入資料放在 Profile 自己的設定目錄"
    );

    // 走過各條路徑：核對、覆蓋、登入終端機、精靈、清單
    let mut responses = vec![verify(&harness, id).await.to_string()];
    harness.claude_env.set_env_override(Some((AuthSource::OauthToken, None)));
    responses.push(verify(&harness, id).await.to_string());
    harness.claude_env.set_env_override(None);
    responses.push(harness.send_json(Method::POST, &format!("{PROFILES}/{id}/login-terminal"), None).await.1.to_string());
    responses.push(harness.send_json(Method::POST, RECHECK, None).await.1.to_string());
    responses.push(setup(&harness).await.to_string());
    responses.push(serde_json::to_string(&profiles(&harness).await).unwrap());

    for response in &responses {
        assert!(!response.contains(&secret), "API 回應含有登入資料");
    }
    let persisted = persisted_bytes(&harness);
    assert!(contains(&persisted, "a@example.com"), "掃描確實涵蓋了資料庫（帳號身分會被保存）");
    assert!(!contains(&persisted, &secret), "資料庫、WAL 或日誌含有登入資料");
    assert!(!harness.log_text().contains(&secret));
}

#[tokio::test]
async fn profile_endpoints_require_the_desktop_credential() {
    let harness = Harness::start().await;

    for (method, path) in [
        (Method::GET, PROFILES),
        (Method::POST, PROFILES),
        (Method::POST, "/api/v1/profiles/x/verify"),
        (Method::POST, "/api/v1/profiles/x/login-terminal"),
        (Method::GET, SETUP),
        (Method::POST, RECHECK),
        (Method::POST, ACK_NOTICE),
    ] {
        let response = harness.request(method.clone(), path).send().await.unwrap();
        assert!(response.status().is_client_error(), "{method} {path} 未帶憑證卻回應 {}", response.status());
    }
    assert!(harness.claude_env.login_terminals().is_empty());
}

#[tokio::test]
async fn profile_changes_append_events_with_utc_times() {
    let harness = Harness::builder().clock_at(CLOCK_START).start().await;
    let profile = add_ok(&harness, "A").await;
    let id = profile["id"].as_str().unwrap();
    harness.claude_env.log_in(&dir_of(&profile), "a@example.com");
    verify(&harness, id).await;
    harness.send_json(Method::POST, &format!("{PROFILES}/{id}/archive"), None).await;

    let db = harness.db();
    let mut statement = db
        .prepare(
            "SELECT event_type, aggregate_version, occurred_at FROM event WHERE aggregate_type = 'account_profile' ORDER BY seq",
        )
        .unwrap();
    let rows: Vec<(String, i64, i64)> =
        statement.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap().map(Result::unwrap).collect();

    let expected = CLOCK_START.as_millis();
    assert_eq!(
        rows,
        [
            ("account_profile_created".to_string(), 1, expected),
            ("account_profile_verified".to_string(), 2, expected),
            ("account_profile_identity_pinned".to_string(), 3, expected),
            ("account_profile_archived".to_string(), 4, expected),
        ]
    );
}
