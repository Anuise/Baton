//! 桌面殼經由 RuntimeClient 管理帳號 Profile 與首次啟動精靈（票 07）：與 App 實際使用的路徑相同（讀憑證檔、帶標頭）。

use std::sync::Arc;

use baton_api::CreateProfileRequest;
use baton_client::{ClientError, RuntimeClient};
use baton_domain::setup::ToolStatus;
use baton_harness::fakes::FakeSecretProtector;
use baton_harness::Harness;

fn app_client(harness: &Harness) -> RuntimeClient {
    RuntimeClient::new(harness.addr(), harness.data_dir().to_path_buf(), Arc::new(FakeSecretProtector))
}

fn request(name: &str) -> CreateProfileRequest {
    CreateProfileRequest { name: name.to_string(), expected_account: None }
}

#[tokio::test]
async fn app_adds_a_profile_logs_in_through_the_terminal_and_sees_it_become_dispatchable() {
    let harness = Harness::start().await;
    harness.claude_env.script_terminal_login("me@example.com");
    let client = app_client(&harness);

    let created = client.create_profile(&request("主要帳號")).await.unwrap();
    assert!(!created.dispatchable);
    assert!(client.open_login_terminal(&created.profile.id).await.unwrap().opened);
    let verified = client.verify_profile(&created.profile.id).await.unwrap();

    assert!(verified.dispatchable, "{:?}", verified.blockers);
    assert_eq!(client.profiles().await.unwrap().profiles, vec![verified.clone()]);
    assert_eq!(client.profiles().await.unwrap().limit, 8);

    let archived = client.archive_profile(&created.profile.id).await.unwrap();
    assert!(!archived.dispatchable);
}

#[tokio::test]
async fn app_runs_the_wizard_checks_rechecks_and_acknowledges_the_background_notice() {
    let harness = Harness::start().await;
    let client = app_client(&harness);

    let status = client.setup().await.unwrap();
    assert_eq!(status.claude_code.status, ToolStatus::Ok);
    assert!(status.background_notice.acknowledged_at.is_none());

    assert!(client.recheck_setup().await.unwrap().profiles.is_empty());
    let acknowledged = client.acknowledge_background_notice().await.unwrap();
    assert!(acknowledged.background_notice.acknowledged_at.is_some());
}

#[tokio::test]
async fn rejection_reasons_reach_the_app_with_their_stable_code() {
    let harness = Harness::start().await;
    let client = app_client(&harness);
    for n in 1..=8 {
        client.create_profile(&request(&format!("帳號 {n}"))).await.unwrap();
    }

    let ninth = client.create_profile(&request("帳號 9")).await.unwrap_err();
    let unknown = client.verify_profile("prof_nope").await.unwrap_err();

    let ClientError::Rejected { code, message } = &ninth else { panic!("{ninth:?}") };
    assert_eq!(code, "profile_limit_reached");
    assert!(message.contains('8'));
    assert!(matches!(&unknown, ClientError::Rejected { code, .. } if code == "profile_not_found"));
}
