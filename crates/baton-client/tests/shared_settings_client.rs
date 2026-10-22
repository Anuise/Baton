//! 桌面殼經由 RuntimeClient 匯入、採用共同設定並為 Run 產生設定快照（票 08）：與 App 實際使用的路徑相同（讀憑證檔、帶標頭）。

use std::sync::Arc;

use baton_api::{CreateTaskRequest, CreateWorkspaceRequest};
use baton_client::{ClientError, DispatchOutcome, RuntimeClient};
use baton_domain::task::{ContractContent, ContractField};
use baton_harness::fakes::FakeSecretProtector;
use baton_harness::Harness;

fn app_client(harness: &Harness) -> RuntimeClient {
    RuntimeClient::new(harness.addr(), harness.data_dir().to_path_buf(), Arc::new(FakeSecretProtector))
}

fn filled(text: &str) -> Option<ContractField> {
    Some(ContractField::Filled { text: text.into(), references: vec![] })
}

fn complete() -> ContractContent {
    ContractContent {
        objective: filled("整理資料"),
        inputs: Some(ContractField::NotApplicable { reason: "沒有輸入".into() }),
        scope: filled("公開資料"),
        deliverables: filled("比較表"),
        done_criteria: filled("五個產品"),
        blocker_handling: filled("回報"),
        required_capabilities: vec![],
        verification_checks: vec![],
    }
}

/// 放入使用者的設定：偏好、指示與 MCP；env 內的登入資料不應被匯入。
fn seed_user_settings(harness: &Harness) {
    harness.user_settings.write_settings(
        r#"{"model":"opus","env":{"ANTHROPIC_API_KEY":"sk-ant-secret-value","FOO":"bar"}}"#,
    );
    harness.user_settings.write_instructions("# 全域指示\n請用繁體中文回答。\n");
    harness.user_settings.write_user_state(r#"{"mcpServers":{"docs":{"command":"docs-server"}}}"#);
}

/// 建立帳號、Workspace 與 ready 的 Task 並派工；回傳 (workspace id, run id)。
async fn dispatched_run(harness: &Harness, client: &RuntimeClient) -> (String, String) {
    harness.add_account("主要帳號", "main@example.com").await;
    let dir = harness.data_dir().join("app");
    std::fs::create_dir_all(&dir).unwrap();
    let workspace_id = client
        .create_workspace(&CreateWorkspaceRequest { path: dir.to_string_lossy().into_owned(), name: None })
        .await
        .unwrap()
        .id;
    let goal = client.create_goal(&workspace_id, "研究競品").await.unwrap();
    let created = client
        .create_task(&workspace_id, &CreateTaskRequest { goal_id: goal.id.clone(), title: "整理".into(), kind: Default::default(), contract: complete() })
        .await
        .unwrap();
    client.mark_task_ready(&workspace_id, &created.task.id).await.unwrap();
    match client.dispatch_task(&workspace_id, &created.task.id).await.unwrap() {
        DispatchOutcome::Started(view) => (workspace_id, view.run.id),
        DispatchOutcome::Queued(queued) => panic!("應該直接派出，卻排隊中：{queued:?}"),
    }
}

#[tokio::test]
async fn app_previews_adopts_lists_and_reads_a_shared_settings_version() {
    let harness = Harness::start().await;
    seed_user_settings(&harness);
    let before = harness.user_settings.files();
    let client = app_client(&harness);

    assert!(client.shared_settings().await.unwrap().current.is_none());

    let preview = client.preview_settings_import().await.unwrap();
    assert!(preview.changed);
    assert_eq!(preview.base_version, None);
    assert_eq!(preview.bundle.instructions.as_deref(), Some("# 全域指示\n請用繁體中文回答。\n"));
    assert!(preview.bundle.mcp_servers.contains_key("docs"));
    assert!(!preview.exclusions.is_empty(), "登入資料應列在排除清單");
    assert!(!format!("{preview:?}").contains("sk-ant-secret-value"));
    // 預覽不保存任何東西
    assert!(client.shared_settings().await.unwrap().versions.is_empty());

    let adopted = client.adopt_settings(&preview.candidate_hash).await.unwrap();
    assert_eq!(adopted.version, 1);
    assert_eq!(adopted.content_hash, preview.candidate_hash);

    let status = client.shared_settings().await.unwrap();
    assert_eq!(status.versions.len(), 1);
    assert_eq!(status.current.as_ref().unwrap().version, 1);
    assert_eq!(client.settings_version(1).await.unwrap(), adopted);
    assert!(!format!("{adopted:?}").contains("sk-ant-secret-value"));

    // 內容沒變時預覽顯示沒有差異；使用者的原始設定檔位元組不變
    let again = client.preview_settings_import().await.unwrap();
    assert!(!again.changed);
    assert_eq!(again.base_version, Some(1));
    assert_eq!(harness.user_settings.files(), before);
}

#[tokio::test]
async fn app_is_told_when_the_source_changed_after_the_preview_or_the_version_is_unknown() {
    let harness = Harness::start().await;
    seed_user_settings(&harness);
    let client = app_client(&harness);
    let preview = client.preview_settings_import().await.unwrap();

    let wrong_hash = client.adopt_settings("not-the-previewed-hash").await.unwrap_err();
    harness.user_settings.write_instructions("預覽之後才改的指示\n");
    let stale = client.adopt_settings(&preview.candidate_hash).await.unwrap_err();
    let missing = client.settings_version(99).await.unwrap_err();

    assert!(matches!(&wrong_hash, ClientError::Rejected { code, .. } if code == "settings_source_changed"), "{wrong_hash:?}");
    assert!(matches!(&stale, ClientError::Rejected { code, .. } if code == "settings_source_changed"), "{stale:?}");
    assert!(matches!(&missing, ClientError::Rejected { code, .. } if code == "settings_version_not_found"), "{missing:?}");
    assert!(client.shared_settings().await.unwrap().versions.is_empty());
}

#[tokio::test]
async fn app_gets_a_run_settings_snapshot_that_stays_pinned_after_a_newer_version_is_adopted() {
    let harness = Harness::start().await;
    seed_user_settings(&harness);
    let client = app_client(&harness);
    let (workspace_id, run_id) = dispatched_run(&harness, &client).await;

    // 還沒有採用任何版本
    let none = client.run_settings_snapshot(&workspace_id, &run_id, None).await.unwrap_err();
    assert!(matches!(&none, ClientError::Rejected { code, .. } if code == "shared_settings_not_adopted"), "{none:?}");

    let v1 = client.adopt_settings(&client.preview_settings_import().await.unwrap().candidate_hash).await.unwrap();
    let snapshot = client.run_settings_snapshot(&workspace_id, &run_id, None).await.unwrap();
    assert_eq!(snapshot.run_id, run_id);
    assert_eq!(snapshot.version, 1);
    assert_eq!(snapshot.content_hash, v1.content_hash);
    assert!(snapshot.files.iter().any(|f| f.name == "manifest.json"));
    assert!(std::path::Path::new(&snapshot.directory).is_dir());

    // 之後採用新版本不影響已綁定的 Run；重複要求回傳同一個綁定
    harness.user_settings.write_instructions("第二版指示\n");
    let v2 = client.adopt_settings(&client.preview_settings_import().await.unwrap().candidate_hash).await.unwrap();
    assert_eq!(v2.version, 2);
    assert_eq!(client.run_settings_snapshot(&workspace_id, &run_id, None).await.unwrap(), snapshot);
    assert_eq!(client.run_settings_snapshot(&workspace_id, &run_id, Some(1)).await.unwrap(), snapshot);
    let other = client.run_settings_snapshot(&workspace_id, &run_id, Some(2)).await.unwrap_err();
    assert!(matches!(&other, ClientError::Rejected { code, .. } if code == "settings_snapshot_pinned"), "{other:?}");
    // 舊版本內容不被修改
    assert_eq!(client.settings_version(1).await.unwrap(), v1);
}
