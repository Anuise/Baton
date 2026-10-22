//! 登入啟動登記的舊值（票 33；review-notes #19）：登錄已有名為產品名稱、但命令列不同的值（例如舊安裝位置），
//! 偏好為關閉時 Runtime 啟動依值名稱判定歸屬並移除；偏好為開啟時覆寫成目前的命令列。

use baton_harness::Harness;
use reqwest::{Method, StatusCode};
use serde_json::json;

const LOGIN_STARTUP: &str = "/api/v1/lifecycle/login-startup";

#[tokio::test]
async fn a_stale_registration_from_an_old_install_is_removed_when_the_preference_is_off() {
    let mut harness = Harness::start().await;
    let (status, view) = harness.send_json(Method::GET, "/api/v1/lifecycle", None).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(view["login_startup"]["enabled"], false, "預設不在登入時啟動：{view}");

    harness.startup_registration.leave_stale();
    harness.restart().await;

    assert!(!harness.startup_registration.stale(), "舊安裝位置的登記應被移除，否則登入時仍會執行舊的命令列");
    assert!(!harness.startup_registration.registered());
    assert_eq!(harness.startup_registration.changes(), vec![false]);
    assert!(harness.log_text().contains("已移除登記"), "{}", harness.log_text());
}

#[tokio::test]
async fn a_stale_registration_is_overwritten_when_the_preference_is_on() {
    let mut harness = Harness::start().await;
    let (status, view) = harness.send_json(Method::PUT, LOGIN_STARTUP, Some(json!({ "enabled": true }))).await;
    assert_eq!(status, StatusCode::OK, "{view}");

    harness.startup_registration.leave_stale();
    harness.restart().await;

    assert!(harness.startup_registration.registered(), "覆寫成目前安裝位置的命令列");
    assert!(!harness.startup_registration.stale());
    assert_eq!(harness.startup_registration.changes(), vec![true, true]);
}
