//! 桌面殼經由 RuntimeClient 開啟／結束 Controller CLI（desktop-controller-cli 票 03）：與 App 實際使用的路徑相同（讀憑證檔、帶標頭）。
//!
//! 測試不呼叫 supervise，所以 Run 停在 `starting`（沒有真的開窗）。

use std::sync::Arc;

use baton_api::CreateWorkspaceRequest;
use baton_client::{ClientError, RuntimeClient};
use baton_domain::controller_cli::ControllerCliStatus;
use baton_harness::fakes::FakeSecretProtector;
use baton_harness::Harness;

fn app_client(harness: &Harness) -> RuntimeClient {
    RuntimeClient::new(harness.addr(), harness.data_dir().to_path_buf(), Arc::new(FakeSecretProtector))
}

async fn new_workspace(harness: &Harness, client: &RuntimeClient) -> String {
    let dir = harness.data_dir().join("app");
    std::fs::create_dir_all(&dir).unwrap();
    client
        .create_workspace(&CreateWorkspaceRequest { path: dir.to_string_lossy().into_owned(), name: None })
        .await
        .unwrap()
        .id
}

fn rejected_code(error: ClientError) -> String {
    match error {
        ClientError::Rejected { code, .. } => code,
        other => panic!("應該被拒絕並帶代碼：{other:?}"),
    }
}

#[tokio::test]
async fn app_opens_the_controller_cli_and_sees_it_starting() {
    let harness = Harness::start().await;
    let client = app_client(&harness);
    let workspace_id = new_workspace(&harness, &client).await;
    let profile_id = harness.add_account("Controller 帳號", "ctl@example.com").await;

    let empty = client.controller_cli(&workspace_id).await.unwrap();
    assert_eq!(empty.workspace_id, workspace_id);
    assert_eq!(empty.run, None);

    let opened = client.open_controller_cli(&workspace_id, &profile_id).await.unwrap();
    let run = opened.run.clone().expect("開啟後應該有 Run");
    assert_eq!(run.status, ControllerCliStatus::Starting);
    assert_eq!(run.profile_id, profile_id);
    assert_eq!(run.workspace_id, workspace_id);
    assert_eq!(client.controller_cli(&workspace_id).await.unwrap(), opened);

    let again = client.open_controller_cli(&workspace_id, &profile_id).await.unwrap_err();
    assert_eq!(rejected_code(again), "controller_cli_already_open");
}

#[tokio::test]
async fn app_ends_the_open_controller_cli_and_ending_without_one_is_rejected() {
    let harness = Harness::start().await;
    let client = app_client(&harness);
    let workspace_id = new_workspace(&harness, &client).await;
    let profile_id = harness.add_account("Controller 帳號", "ctl@example.com").await;

    let not_open = client.end_controller_cli(&workspace_id).await.unwrap_err();
    assert_eq!(rejected_code(not_open), "controller_cli_not_open");

    client.open_controller_cli(&workspace_id, &profile_id).await.unwrap();
    let ended = client.end_controller_cli(&workspace_id).await.unwrap();
    let run = ended.run.expect("結束後應該仍回傳 Run");
    assert!(run.end_requested_at.is_some());
    assert_eq!(client.controller_cli(&workspace_id).await.unwrap().run.unwrap().end_requested_at, run.end_requested_at);
}

#[tokio::test]
async fn app_switches_the_controller_cli_to_another_profile_and_bad_switches_are_rejected() {
    let harness = Harness::start().await;
    let client = app_client(&harness);
    let workspace_id = new_workspace(&harness, &client).await;
    let first = harness.add_account("Controller 帳號", "ctl@example.com").await;
    let second = harness.add_account("備用帳號", "spare@example.com").await;

    let not_open = client.switch_controller_cli(&workspace_id, &second).await.unwrap_err();
    assert_eq!(rejected_code(not_open), "controller_cli_not_open");

    client.open_controller_cli(&workspace_id, &first).await.unwrap();
    let same = client.switch_controller_cli(&workspace_id, &first).await.unwrap_err();
    assert_eq!(rejected_code(same), "controller_cli_same_profile");

    let switched = client.switch_controller_cli(&workspace_id, &second).await.unwrap();
    let run = switched.run.clone().expect("換帳號後應該仍回傳目前的 Run");
    assert_eq!(run.profile_id, first);
    assert_eq!(run.switch_to_profile_id.as_deref(), Some(second.as_str()));
    assert!(run.end_requested_at.is_some());
    assert_eq!(client.controller_cli(&workspace_id).await.unwrap(), switched);

    let again = client.switch_controller_cli(&workspace_id, &second).await.unwrap_err();
    assert_eq!(rejected_code(again), "controller_cli_ending");
}
