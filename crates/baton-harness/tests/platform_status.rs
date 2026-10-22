//! 情境測試：Windows 平台辨識與支援狀態（DR-20261021-08），經由 Runtime harness 的真實 HTTP API 驗證。

use baton_api::{PlatformResponse, PLATFORM_PATH};
use baton_domain::platform::{PlatformIssueCode, SupportStatus};
use baton_domain::Timestamp;
use baton_harness::Harness;
use baton_ports::{OsFamily, PlatformSnapshot};

/// 2027-10-12 00:00:00 UTC：25H2 Home／Pro 的服務結束日。
const END_OF_25H2: Timestamp = Timestamp::from_millis(1_823_299_200_000);

fn snapshot(family: OsFamily, build: &str, feature: &str, edition: &str, native: &str, process: &str) -> PlatformSnapshot {
    PlatformSnapshot {
        os_family: family,
        os_build: build.into(),
        feature_version: feature.into(),
        edition: edition.into(),
        native_arch: native.into(),
        process_arch: process.into(),
    }
}

async fn platform_of(harness: &Harness) -> PlatformResponse {
    harness.get_json(PLATFORM_PATH).await
}

fn codes(response: &PlatformResponse) -> Vec<PlatformIssueCode> {
    response.assessment.issues.iter().map(|i| i.code).collect()
}

#[tokio::test]
async fn the_four_supported_combinations_show_as_officially_supported() {
    let harness = Harness::start().await;

    for (build, feature) in [("26200", "25H2"), ("26300", "26H2")] {
        for edition in ["Home", "Pro"] {
            harness.platform_info.set(snapshot(OsFamily::Windows11, build, feature, edition, "x64", "x64"));

            let response = platform_of(&harness).await;

            assert_eq!(response.assessment.status, SupportStatus::Supported, "{feature} {edition}");
            assert!(response.assessment.issues.is_empty(), "{feature} {edition}");
        }
    }
}

#[tokio::test]
async fn windows_10_server_and_arm_show_as_unofficial_without_blocking_dispatch() {
    let harness = Harness::start().await;
    let cases = [
        ("Windows 10", snapshot(OsFamily::Windows10, "19045", "22H2", "Pro", "x64", "x64"), PlatformIssueCode::UnsupportedOs),
        (
            "Windows Server",
            snapshot(OsFamily::WindowsServer, "26100", "24H2", "Pro", "x64", "x64"),
            PlatformIssueCode::UnsupportedOs,
        ),
        (
            "ARM64 原生",
            snapshot(OsFamily::Windows11, "26200", "25H2", "Pro", "arm64", "arm64"),
            PlatformIssueCode::UnsupportedArchitecture,
        ),
        (
            "ARM 上的 x64 模擬",
            snapshot(OsFamily::Windows11, "26300", "26H2", "Home", "arm64", "x64"),
            PlatformIssueCode::UnsupportedArchitecture,
        ),
    ];

    for (name, platform, expected_code) in cases {
        harness.platform_info.set(platform);

        let response = platform_of(&harness).await;

        assert_eq!(response.assessment.status, SupportStatus::Unsupported, "{name}");
        assert!(codes(&response).contains(&expected_code), "{name}");
        assert!(!response.assessment.blocks_dispatch, "{name}");
    }
}

#[tokio::test]
async fn an_ended_version_shows_an_l1_warning_and_still_does_not_block_dispatch() {
    let harness = Harness::builder().clock_at(END_OF_25H2.plus_millis(-1)).start().await;
    harness.platform_info.set(snapshot(OsFamily::Windows11, "26200", "25H2", "Pro", "x64", "x64"));
    let before = platform_of(&harness).await;
    assert_eq!(before.assessment.status, SupportStatus::Supported);
    assert_eq!(before.assessment.support_ends_on.as_deref(), Some("2027-10-12"));

    harness.clock.set(END_OF_25H2);
    let after = platform_of(&harness).await;

    assert_eq!(after.assessment.status, SupportStatus::Unsupported);
    assert_eq!(codes(&after), vec![PlatformIssueCode::SupportEnded]);
    assert!(after.assessment.issues[0].message.contains("L1"));
    assert!(!after.assessment.blocks_dispatch);
}

#[tokio::test]
async fn diagnostics_include_os_build_edition_native_arch_and_app_version() {
    let harness = Harness::start().await;
    harness.platform_info.set(snapshot(OsFamily::Windows11, "26300", "26H2", "Home", "arm64", "x64"));

    let response = platform_of(&harness).await;

    assert_eq!(response.platform.os_build, "26300");
    assert_eq!(response.platform.feature_version, "26H2");
    assert_eq!(response.platform.edition, "Home");
    assert_eq!(response.platform.native_arch, "arm64");
    assert_eq!(response.platform.process_arch, "x64");
    assert_eq!(response.app_version, env!("CARGO_PKG_VERSION"));
}

#[tokio::test]
async fn the_support_table_states_its_source_and_lookup_date() {
    let harness = Harness::start().await;

    let response = platform_of(&harness).await;

    assert!(response.support_table_source.starts_with("https://"));
    assert_eq!(response.support_table_checked_on, "2026-10-22");
}

#[tokio::test]
async fn the_status_survives_a_runtime_rebuild() {
    let mut harness = Harness::start().await;
    harness.platform_info.set(snapshot(OsFamily::Windows10, "19045", "22H2", "Pro", "x64", "x64"));

    harness.restart().await;

    let response = platform_of(&harness).await;
    assert_eq!(response.assessment.status, SupportStatus::Unsupported);
}
