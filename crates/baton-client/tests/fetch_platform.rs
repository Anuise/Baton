//! 桌面殼向 Runtime 查詢平台狀態。

use baton_client::fetch_platform;
use baton_domain::platform::{OsFamily, PlatformSnapshot, SupportStatus};
use baton_harness::Harness;

#[tokio::test]
async fn fetches_the_platform_status_from_a_running_runtime() {
    let harness = Harness::start().await;
    harness.platform_info.set(PlatformSnapshot {
        os_family: OsFamily::Windows10,
        os_build: "19045".into(),
        feature_version: "22H2".into(),
        edition: "Pro".into(),
        native_arch: "x64".into(),
        process_arch: "x64".into(),
    });

    let response = fetch_platform(harness.addr()).await.unwrap();

    assert_eq!(response.platform.os_build, "19045");
    assert_eq!(response.assessment.status, SupportStatus::Unsupported);
}
