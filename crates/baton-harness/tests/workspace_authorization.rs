//! 情境測試：Workspace 授權範本、確認與 Run 准入（票 06），經由 Runtime harness 的真實 HTTP API 驗證。
//!
//! Run 還不存在（票 10），所以准入檢查以 `admission-check` 端點呈現：它與日後派工交易使用同一個 Domain 判定。

use baton_domain::auth::Role;
use baton_domain::Timestamp;
use baton_harness::Harness;
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

const WORKSPACES: &str = "/api/v1/workspaces";

/// 2023-11-14 22:13:20.123 UTC
const CLOCK_START: Timestamp = Timestamp::from_millis(1_700_000_000_123);

fn plain_dir(harness: &Harness, name: &str) -> String {
    let dir = harness.data_dir().join("projects").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    dir.to_string_lossy().into_owned()
}

/// 建立 Workspace，回傳它的識別。
async fn new_workspace(harness: &Harness, name: &str) -> String {
    let path = plain_dir(harness, name);
    let (status, body) = harness.send_json(Method::POST, WORKSPACES, Some(json!({ "path": path }))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["id"].as_str().unwrap().to_string()
}

fn authorization_path(id: &str) -> String {
    format!("{WORKSPACES}/{id}/authorization")
}

async fn view(harness: &Harness, id: &str) -> Value {
    let (status, body) = harness.send_json(Method::GET, &authorization_path(id), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

fn template(view: &Value, id: &str) -> Value {
    view["templates"].as_array().unwrap().iter().find(|t| t["id"] == id).unwrap_or_else(|| panic!("沒有範本 {id}")).clone()
}

fn capabilities_of(grants: &Value) -> Vec<String> {
    grants.as_array().unwrap().iter().map(|g| g["capability"].as_str().unwrap().to_string()).collect()
}

async fn confirm(harness: &Harness, id: &str, template_id: Option<&str>, grants: &Value, acknowledged: bool) -> (StatusCode, Value) {
    let body = json!({ "template_id": template_id, "grants": grants, "acknowledged_risk": acknowledged, "acknowledged_expansion": true });
    harness.send_json(Method::PUT, &authorization_path(id), Some(body)).await
}

/// 確認某個範本（原封不動、已承認風險），回傳確認後的 revision。
async fn confirm_template(harness: &Harness, id: &str, template_id: &str) -> Value {
    let grants = template(&view(harness, id).await, template_id)["grants"].clone();
    let (status, revision) = confirm(harness, id, Some(template_id), &grants, true).await;
    assert_eq!(status, StatusCode::CREATED, "{revision}");
    revision
}

async fn admit(harness: &Harness, id: &str, required: &[&str]) -> (StatusCode, Value) {
    let path = format!("{WORKSPACES}/{id}/admission-check");
    harness.send_json(Method::POST, &path, Some(json!({ "required_capabilities": required }))).await
}

fn authorization_events(harness: &Harness) -> Vec<(String, i64, i64)> {
    let db = harness.db();
    let mut statement = db
        .prepare(
            "SELECT aggregate_id, aggregate_version, occurred_at FROM event
             WHERE aggregate_type = 'workspace_authorization' ORDER BY seq",
        )
        .unwrap();
    statement.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap().map(Result::unwrap).collect()
}

fn error_code(body: &Value) -> &str {
    body["error"].as_str().unwrap_or_default()
}

// --- 建立後的授權設定 ---

#[tokio::test]
async fn a_new_workspace_starts_unconfirmed_and_no_template_is_applied_for_the_user() {
    let harness = Harness::start().await;
    let id = new_workspace(&harness, "fresh").await;

    let view = view(&harness, &id).await;

    assert_eq!(view["workspace_id"], json!(id));
    assert_eq!(view["status"], "unconfirmed");
    assert_eq!(view["effective"], Value::Null);
    assert_eq!(view["revisions"], json!([]));
    assert!(authorization_events(&harness).is_empty(), "選取目錄不等於授權");
    assert_eq!(view["templates"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn templates_express_capability_target_and_restrictions_for_this_workspace_directory() {
    let harness = Harness::start().await;
    let id = new_workspace(&harness, "tpl").await;
    let (_, workspaces) = harness.send_json(Method::GET, WORKSPACES, None).await;
    let path = workspaces["workspaces"][0]["path"].as_str().unwrap().to_string();

    let dev = template(&view(&harness, &id).await, "trusted_dev_p3");

    let build = dev["grants"].as_array().unwrap().iter().find(|g| g["capability"] == "run_build_test").unwrap();
    assert_eq!(build["targets"], json!([path]), "目標資源是這個 Workspace 的目錄");
    assert!(!build["restrictions"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn p3_template_contains_none_of_the_high_risk_capabilities() {
    let harness = Harness::start().await;
    let id = new_workspace(&harness, "p3").await;
    let view = view(&harness, &id).await;

    let caps = capabilities_of(&template(&view, "trusted_dev_p3")["grants"]);

    for expected in ["write_files", "run_build_test", "install_project_dependencies"] {
        assert!(caps.contains(&expected.to_string()), "{expected}：{caps:?}");
    }
    for forbidden in ["admin_install", "global_install", "remote_write", "deploy", "external_message", "payment", "production_data"] {
        assert!(!caps.contains(&forbidden.to_string()), "P3 範本不得包含 {forbidden}");
    }
    let high_risk: Vec<&str> = view["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["high_risk"] == true)
        .map(|c| c["capability"].as_str().unwrap())
        .collect();
    assert_eq!(high_risk.len(), 7, "能力清單要標出七項高風險能力：{high_risk:?}");
}

// --- 確認摘要 ---

#[tokio::test]
async fn confirmation_summary_for_p3_discloses_that_code_runs_as_the_windows_user() {
    let harness = Harness::start().await;
    let id = new_workspace(&harness, "sum").await;
    let grants = template(&view(&harness, &id).await, "trusted_dev_p3")["grants"].clone();

    let (status, summary) = harness
        .send_json(Method::POST, &format!("{}/summary", authorization_path(&id)), Some(json!({ "grants": grants })))
        .await;

    assert_eq!(status, StatusCode::OK, "{summary}");
    assert_eq!(summary["requires_risk_acknowledgement"], true);
    let risks = summary["risks"].to_string();
    assert!(risks.contains("Windows 使用者的身分執行"), "{risks}");
    assert_eq!(summary["items"].as_array().unwrap().len(), 4);
    assert!(authorization_events(&harness).is_empty(), "摘要只是預覽，不保存任何東西");
}

#[tokio::test]
async fn research_template_summary_has_no_execution_risk_to_acknowledge() {
    let harness = Harness::start().await;
    let id = new_workspace(&harness, "sumr").await;
    let grants = template(&view(&harness, &id).await, "research_docs")["grants"].clone();

    let (_, summary) = harness
        .send_json(Method::POST, &format!("{}/summary", authorization_path(&id)), Some(json!({ "grants": grants })))
        .await;

    assert_eq!(summary["requires_risk_acknowledgement"], false);
    assert_eq!(summary["risks"], json!([]));
}

// --- 未確認前不准入 ---

#[tokio::test]
async fn run_requiring_a_capability_is_refused_with_a_reason_until_the_authorization_is_confirmed() {
    let harness = Harness::start().await;
    let id = new_workspace(&harness, "gate").await;

    let (status, body) = admit(&harness, &id, &["run_build_test"]).await;
    let (read_status, _) = admit(&harness, &id, &["read_files"]).await;

    assert_eq!((status, error_code(&body)), (StatusCode::CONFLICT, "authorization_unconfirmed"));
    assert!(body["message"].as_str().unwrap().contains("還沒有確認授權"), "{body}");
    assert_eq!(read_status, StatusCode::CONFLICT, "任何需要能力的 Run 都一樣");
}

#[tokio::test]
async fn run_requiring_no_capability_does_not_need_a_confirmed_authorization() {
    let harness = Harness::start().await;
    let id = new_workspace(&harness, "nocap").await;

    let (status, body) = admit(&harness, &id, &[]).await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["authorization_revision"], Value::Null);
}

#[tokio::test]
async fn confirming_without_acknowledging_the_risk_is_refused_and_leaves_the_workspace_unconfirmed() {
    let harness = Harness::start().await;
    let id = new_workspace(&harness, "noack").await;
    let grants = template(&view(&harness, &id).await, "trusted_dev_p3")["grants"].clone();

    let (status, body) = confirm(&harness, &id, Some("trusted_dev_p3"), &grants, false).await;

    assert_eq!((status, error_code(&body)), (StatusCode::BAD_REQUEST, "risk_acknowledgement_required"));
    assert_eq!(view(&harness, &id).await["status"], "unconfirmed");
    assert_eq!(admit(&harness, &id, &["run_build_test"]).await.0, StatusCode::CONFLICT);
    assert!(authorization_events(&harness).is_empty());
}

// --- 確認後生效，並建立 revision ---

#[tokio::test]
async fn confirming_p3_creates_revision_one_and_only_then_admits_the_capabilities() {
    let harness = Harness::builder().clock_at(CLOCK_START).start().await;
    let id = new_workspace(&harness, "ok").await;

    let revision = confirm_template(&harness, &id, "trusted_dev_p3").await;

    assert_eq!(revision["revision"], 1);
    assert_eq!(revision["confirmed_at"], json!(CLOCK_START.as_millis()));
    assert_eq!(revision["source"]["kind"], "template");
    assert_eq!(revision["source"]["template_id"], "trusted_dev_p3");
    assert_eq!(revision["source"]["confirmed_by"], "desktop");
    let (status, admitted) = admit(&harness, &id, &["write_files", "run_build_test", "install_project_dependencies"]).await;
    assert_eq!(status, StatusCode::OK, "{admitted}");
    assert_eq!(admitted["authorization_revision"], 1);
    let view = view(&harness, &id).await;
    assert_eq!(view["status"], "confirmed");
    assert_eq!(view["effective"], revision);
    assert_eq!(authorization_events(&harness), vec![(id, 1, CLOCK_START.as_millis())]);
}

#[tokio::test]
async fn every_modification_creates_a_new_revision_with_its_source_and_time() {
    let harness = Harness::builder().clock_at(CLOCK_START).start().await;
    let id = new_workspace(&harness, "rev").await;
    confirm_template(&harness, &id, "trusted_dev_p3").await;
    harness.clock.advance_millis(60_000);
    // 以範本為起點，拿掉「安裝專案依賴」
    let mut grants = template(&view(&harness, &id).await, "trusted_dev_p3")["grants"].clone();
    grants.as_array_mut().unwrap().retain(|g| g["capability"] != "install_project_dependencies");

    let (status, second) = confirm(&harness, &id, Some("trusted_dev_p3"), &grants, true).await;

    assert_eq!(status, StatusCode::CREATED, "{second}");
    assert_eq!(second["revision"], 2);
    assert_eq!(second["confirmed_at"], json!(CLOCK_START.as_millis() + 60_000));
    assert_eq!(second["source"]["kind"], "template_edited");
    assert_eq!(second["source"]["base_revision"], 1);
    let view = view(&harness, &id).await;
    let history = view["revisions"].as_array().unwrap();
    assert_eq!(history.iter().map(|r| r["revision"].as_i64().unwrap()).collect::<Vec<_>>(), [1, 2]);
    assert_eq!(history[0]["source"]["kind"], "template", "舊 revision 保持原樣");
    assert_eq!(view["effective"]["revision"], 2);
    assert_eq!(
        authorization_events(&harness),
        vec![(id.clone(), 1, CLOCK_START.as_millis()), (id, 2, CLOCK_START.as_millis() + 60_000)]
    );
}

#[tokio::test]
async fn a_new_revision_takes_effect_for_later_admissions() {
    let harness = Harness::start().await;
    let id = new_workspace(&harness, "later").await;
    confirm_template(&harness, &id, "trusted_dev_p3").await;
    let mut grants = template(&view(&harness, &id).await, "trusted_dev_p3")["grants"].clone();
    grants.as_array_mut().unwrap().retain(|g| g["capability"] != "run_build_test");

    confirm(&harness, &id, Some("trusted_dev_p3"), &grants, true).await;
    let (status, body) = admit(&harness, &id, &["run_build_test"]).await;

    assert_eq!((status, error_code(&body)), (StatusCode::CONFLICT, "capability_not_granted"));
    assert!(body["message"].as_str().unwrap().contains("建置與測試"), "{body}");
    assert!(body["message"].as_str().unwrap().contains("revision 2"), "{body}");
}

#[tokio::test]
async fn confirming_identical_content_again_does_not_create_a_revision() {
    let harness = Harness::start().await;
    let id = new_workspace(&harness, "same").await;
    let first = confirm_template(&harness, &id, "research_docs").await;
    let grants = template(&view(&harness, &id).await, "research_docs")["grants"].clone();

    let (status, again) = confirm(&harness, &id, Some("research_docs"), &grants, false).await;

    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(again, first);
    assert_eq!(authorization_events(&harness).len(), 1);
}

#[tokio::test]
async fn a_confirmed_authorization_and_its_history_survive_rebuilding_the_runtime() {
    let mut harness = Harness::builder().clock_at(CLOCK_START).start().await;
    let id = new_workspace(&harness, "persist").await;
    confirm_template(&harness, &id, "trusted_dev_p3").await;
    let before = view(&harness, &id).await;

    harness.restart().await;

    assert_eq!(view(&harness, &id).await, before);
    assert_eq!(admit(&harness, &id, &["run_build_test"]).await.0, StatusCode::OK);
}

// --- 研究／文件不被強制套用開發範本 ---

#[tokio::test]
async fn a_research_workspace_is_never_forced_into_the_development_template() {
    let harness = Harness::start().await;
    let research = new_workspace(&harness, "research").await;
    let other = new_workspace(&harness, "other").await;

    let revision = confirm_template(&harness, &research, "research_docs").await;

    let caps = capabilities_of(&revision["grants"]);
    assert!(!caps.contains(&"run_build_test".to_string()) && !caps.contains(&"install_project_dependencies".to_string()), "{caps:?}");
    assert_eq!(admit(&harness, &research, &["read_files", "write_files"]).await.0, StatusCode::OK);
    let (status, denied) = admit(&harness, &research, &["run_build_test"]).await;
    assert_eq!((status, error_code(&denied)), (StatusCode::CONFLICT, "capability_not_granted"));
    assert_eq!(view(&harness, &other).await["status"], "unconfirmed", "別的 Workspace 不受影響");
}

#[tokio::test]
async fn a_custom_authorization_without_any_template_is_allowed() {
    let harness = Harness::start().await;
    let id = new_workspace(&harness, "custom").await;
    let path = plain_dir(&harness, "custom");
    let grants = json!([{ "capability": "read_files", "targets": [path], "restrictions": [] }]);

    let (status, revision) = confirm(&harness, &id, None, &grants, false).await;

    assert_eq!(status, StatusCode::CREATED, "{revision}");
    assert_eq!(revision["source"]["kind"], "custom");
    assert_eq!(revision["source"]["template_id"], Value::Null);
}

// --- 高風險能力必須明確授權 ---

#[tokio::test]
async fn a_high_risk_capability_only_enters_when_explicitly_added_and_is_flagged() {
    let harness = Harness::start().await;
    let id = new_workspace(&harness, "deploy").await;
    let grants = json!([{ "capability": "deploy", "targets": ["staging"], "restrictions": ["僅限 staging"] }]);

    let (summary_status, summary) = harness
        .send_json(Method::POST, &format!("{}/summary", authorization_path(&id)), Some(json!({ "grants": grants })))
        .await;
    let (refused_status, refused) = confirm(&harness, &id, None, &grants, false).await;
    let (ok_status, revision) = confirm(&harness, &id, None, &grants, true).await;

    assert_eq!(summary_status, StatusCode::OK);
    assert_eq!(summary["items"][0]["high_risk"], true);
    assert_eq!((refused_status, error_code(&refused)), (StatusCode::BAD_REQUEST, "risk_acknowledgement_required"));
    assert_eq!(ok_status, StatusCode::CREATED, "{revision}");
    // 沒有被授權的高風險能力，P3 確認之後仍然被拒絕
    assert_eq!(admit(&harness, &id, &["deploy"]).await.0, StatusCode::OK);
    let other = new_workspace(&harness, "deploy2").await;
    confirm_template(&harness, &other, "trusted_dev_p3").await;
    for capability in ["admin_install", "global_install", "remote_write", "deploy", "external_message", "payment", "production_data"] {
        let (status, body) = admit(&harness, &other, &[capability]).await;
        assert_eq!((status, error_code(&body)), (StatusCode::CONFLICT, "capability_not_granted"), "{capability}");
    }
}

// --- 錯誤與權限 ---

#[tokio::test]
async fn invalid_authorization_content_is_rejected_without_writing_anything() {
    let harness = Harness::start().await;
    let id = new_workspace(&harness, "bad").await;
    let read = |targets: Value| json!({ "capability": "read_files", "targets": targets, "restrictions": [] });

    let (no_target_status, no_target) = confirm(&harness, &id, None, &json!([read(json!([]))]), false).await;
    let (dup_status, dup) = confirm(&harness, &id, None, &json!([read(json!(["a"])), read(json!(["b"]))]), false).await;
    let (tpl_status, tpl) = confirm(&harness, &id, Some("nope"), &json!([]), false).await;
    let (unknown_cap_status, unknown_cap) =
        confirm(&harness, &id, None, &json!([{ "capability": "fly", "targets": ["x"] }]), false).await;

    assert_eq!((no_target_status, error_code(&no_target)), (StatusCode::BAD_REQUEST, "authorization_invalid"));
    assert_eq!((dup_status, error_code(&dup)), (StatusCode::BAD_REQUEST, "authorization_invalid"));
    assert_eq!((tpl_status, error_code(&tpl)), (StatusCode::BAD_REQUEST, "authorization_template_unknown"));
    assert_eq!((unknown_cap_status, error_code(&unknown_cap)), (StatusCode::BAD_REQUEST, "bad_request"));
    assert!(authorization_events(&harness).is_empty());
}

#[tokio::test]
async fn unknown_and_archived_workspaces_are_refused() {
    let harness = Harness::start().await;
    let id = new_workspace(&harness, "gone").await;
    harness.send_json(Method::POST, &format!("{WORKSPACES}/{id}/archive"), None).await;

    let (view_status, view_body) = harness.send_json(Method::GET, &authorization_path("ws_nope"), None).await;
    let (confirm_status, confirm_body) = confirm(&harness, "ws_nope", None, &json!([]), false).await;
    let (archived_status, archived) = confirm(&harness, &id, None, &json!([]), false).await;
    let (admit_status, admit_body) = admit(&harness, &id, &[]).await;
    let (admit_unknown_status, admit_unknown) = admit(&harness, "ws_nope", &["read_files"]).await;

    assert_eq!((view_status, error_code(&view_body)), (StatusCode::NOT_FOUND, "workspace_not_found"));
    assert_eq!((confirm_status, error_code(&confirm_body)), (StatusCode::NOT_FOUND, "workspace_not_found"));
    assert_eq!((archived_status, error_code(&archived)), (StatusCode::CONFLICT, "workspace_archived"));
    assert_eq!((admit_status, error_code(&admit_body)), (StatusCode::CONFLICT, "workspace_archived"));
    assert_eq!((admit_unknown_status, error_code(&admit_unknown)), (StatusCode::NOT_FOUND, "workspace_not_found"));
}

#[tokio::test]
async fn an_agent_credential_cannot_confirm_authorization() {
    let harness = Harness::start().await;
    let id = new_workspace(&harness, "agent").await;
    let token = harness.issue_credential(Role::Agent, None).await;
    let grants = template(&view(&harness, &id).await, "research_docs")["grants"].clone();

    let confirm_response = harness
        .request(Method::PUT, &authorization_path(&id))
        .bearer_auth(&token)
        .header(baton_api::CLIENT_HEADER, baton_api::CLIENT_HEADER_VALUE)
        .json(&json!({ "template_id": "research_docs", "grants": grants, "acknowledged_risk": true }))
        .send()
        .await
        .unwrap();

    assert_eq!(confirm_response.status(), StatusCode::FORBIDDEN);
    assert_eq!(view(&harness, &id).await["status"], "unconfirmed", "Agent 不能替自己擴權");
}

/// 模擬一個視窗：以內容取得確認摘要，再帶著摘要的 `changes.base_revision` 確認（Web UI 的流程）。
async fn summarize(harness: &Harness, id: &str, grants: &Value) -> Value {
    let (status, summary) = harness
        .send_json(Method::POST, &format!("{}/summary", authorization_path(id)), Some(json!({ "grants": grants })))
        .await;
    assert_eq!(status, StatusCode::OK, "{summary}");
    summary
}

async fn confirm_from_summary(harness: &Harness, id: &str, grants: &Value, summary: &Value) -> (StatusCode, Value) {
    let body = json!({
        "template_id": "trusted_dev_p3",
        "grants": grants,
        "acknowledged_risk": true,
        "acknowledged_expansion": true,
        "expected_revision": summary["changes"]["base_revision"],
    });
    harness.send_json(Method::PUT, &authorization_path(id), Some(body)).await
}

#[tokio::test]
async fn when_two_windows_edit_concurrently_the_later_confirmation_is_refused_without_overwriting() {
    let harness = Harness::start().await;
    let id = new_workspace(&harness, "two-windows").await;
    confirm_template(&harness, &id, "trusted_dev_p3").await;
    let base = template(&view(&harness, &id).await, "trusted_dev_p3")["grants"].clone();
    // 兩個視窗都從 revision 1 開始編輯，各自拿掉不同的能力並檢視摘要
    let without = |capability: &str| {
        let mut grants = base.clone();
        grants.as_array_mut().unwrap().retain(|g| g["capability"] != capability);
        grants
    };
    let (first_grants, second_grants) = (without("install_project_dependencies"), without("run_build_test"));
    let first_summary = summarize(&harness, &id, &first_grants).await;
    let second_summary = summarize(&harness, &id, &second_grants).await;
    assert_eq!(first_summary["changes"]["base_revision"], 1);
    assert_eq!(second_summary["changes"]["base_revision"], 1);

    let (first_status, first) = confirm_from_summary(&harness, &id, &first_grants, &first_summary).await;
    let (second_status, second) = confirm_from_summary(&harness, &id, &second_grants, &second_summary).await;

    assert_eq!(first_status, StatusCode::CREATED, "{first}");
    assert_eq!(first["revision"], 2);
    assert_eq!((second_status, error_code(&second)), (StatusCode::CONFLICT, "authorization_revision_conflict"), "{second}");
    assert!(second["message"].as_str().unwrap().contains("目前 revision 2，編輯時是 revision 1"), "{second}");
    let view = view(&harness, &id).await;
    assert_eq!(view["effective"]["revision"], 2, "後送出者不覆蓋先確認的授權");
    assert_eq!(view["effective"]["grants"], first_grants);
    assert_eq!(authorization_events(&harness).len(), 2, "被拒絕的確認不寫入任何東西");
}
