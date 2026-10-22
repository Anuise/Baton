//! 本機 API 的身分驗證與網頁存取防護情境（票 04，spec D-22、D-23）。
//!
//! 全部經由 Runtime harness 的真實 HTTP API 驗證；harness 取得桌面憑證的方式
//! 與桌面 App 相同（讀取 DPAPI 保護檔，這裡以 fake 保護器取代）。

use std::time::Duration;

use baton_api::{CLIENT_HEADER, CLIENT_HEADER_VALUE, EVENTS_PATH, HEALTH_PATH};
use baton_domain::auth::Role;
use baton_harness::Harness;
use reqwest::header::{AUTHORIZATION, HOST, ORIGIN};
use reqwest::{Method, RequestBuilder, Response, StatusCode};

/// 與桌面 App 一樣：帶桌面憑證與固定自訂標頭。
fn authed(harness: &Harness, path: &str) -> RequestBuilder {
    with_token(harness.request(Method::GET, path), &harness.desktop_credential())
}

fn with_token(request: RequestBuilder, token: &str) -> RequestBuilder {
    request.header(AUTHORIZATION, format!("Bearer {token}")).header(CLIENT_HEADER, CLIENT_HEADER_VALUE)
}

async fn error_code(response: Response) -> String {
    let body: serde_json::Value = response.json().await.expect("錯誤回應應為 JSON");
    body["error"].as_str().expect("錯誤回應應有 error 欄位").to_string()
}

#[tokio::test]
async fn request_with_desktop_credential_is_accepted() {
    let harness = Harness::start().await;

    let response = authed(&harness, HEALTH_PATH).send().await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn request_without_credential_is_rejected() {
    let harness = Harness::start().await;

    let response =
        harness.request(Method::GET, HEALTH_PATH).header(CLIENT_HEADER, CLIENT_HEADER_VALUE).send().await.unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(error_code(response).await, "credential_missing");
}

#[tokio::test]
async fn request_with_wrong_token_is_rejected() {
    let harness = Harness::start().await;

    let response = with_token(harness.request(Method::GET, HEALTH_PATH), &"0".repeat(64)).send().await.unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(error_code(response).await, "credential_invalid");
}

#[tokio::test]
async fn non_bearer_authorization_is_rejected() {
    let harness = Harness::start().await;
    let token = harness.desktop_credential();

    let response = harness
        .request(Method::GET, HEALTH_PATH)
        .header(AUTHORIZATION, format!("Basic {token}"))
        .header(CLIENT_HEADER, CLIENT_HEADER_VALUE)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(error_code(response).await, "credential_missing");
}

#[tokio::test]
async fn token_in_the_url_is_not_a_credential() {
    let harness = Harness::start().await;
    let token = harness.desktop_credential();

    let response = harness
        .request(Method::GET, &format!("{HEALTH_PATH}?token={token}&access_token={token}"))
        .header(CLIENT_HEADER, CLIENT_HEADER_VALUE)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn expired_credential_is_rejected_but_works_before_expiry() {
    let harness = Harness::start().await;
    let token = harness.issue_credential(Role::Desktop, Some(Duration::from_secs(60))).await;

    let before = with_token(harness.request(Method::GET, HEALTH_PATH), &token).send().await.unwrap();
    harness.clock.advance_millis(60_000);
    let after = with_token(harness.request(Method::GET, HEALTH_PATH), &token).send().await.unwrap();

    assert_eq!(before.status(), StatusCode::OK);
    assert_eq!(after.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(error_code(after).await, "credential_expired");
}

#[tokio::test]
async fn request_without_the_fixed_custom_header_is_rejected() {
    let harness = Harness::start().await;
    let token = harness.desktop_credential();

    let response = harness
        .request(Method::GET, HEALTH_PATH)
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(error_code(response).await, "client_header_missing");
}

#[tokio::test]
async fn agent_role_credential_cannot_use_the_management_api() {
    let harness = Harness::start().await;
    let token = harness.issue_credential(Role::Agent, None).await;

    let response = with_token(harness.request(Method::GET, HEALTH_PATH), &token).send().await.unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(error_code(response).await, "role_not_allowed");
}

#[tokio::test]
async fn unknown_paths_are_guarded_too_and_do_not_reveal_themselves() {
    let harness = Harness::start().await;

    let anonymous = harness.request(Method::GET, "/api/v1/nope").send().await.unwrap();
    let wrong_token = with_token(harness.request(Method::GET, "/api/v1/nope"), &"0".repeat(64)).send().await.unwrap();
    let authenticated = authed(&harness, "/api/v1/nope").send().await.unwrap();

    assert_eq!(anonymous.status(), StatusCode::FORBIDDEN, "沒有自訂標頭先被擋下，不是 404");
    assert_eq!(wrong_token.status(), StatusCode::UNAUTHORIZED, "未通過驗證前不是 404");
    assert_eq!(authenticated.status(), StatusCode::NOT_FOUND);
}

// ---- 串流（SSE）----

#[tokio::test]
async fn stream_without_credential_is_rejected() {
    let harness = Harness::start().await;

    let response =
        harness.request(Method::GET, EVENTS_PATH).header(CLIENT_HEADER, CLIENT_HEADER_VALUE).send().await.unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn stream_with_wrong_or_expired_credential_is_rejected() {
    let harness = Harness::start().await;
    let expiring = harness.issue_credential(Role::Desktop, Some(Duration::from_secs(1))).await;
    harness.clock.advance_millis(1_000);

    let wrong = with_token(harness.request(Method::GET, EVENTS_PATH), &"f".repeat(64)).send().await.unwrap();
    let expired = with_token(harness.request(Method::GET, EVENTS_PATH), &expiring).send().await.unwrap();

    assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(expired.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn stream_with_valid_credential_delivers_events() {
    let harness = Harness::start().await;

    let mut response = authed(&harness, EVENTS_PATH).send().await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    let first = response.chunk().await.unwrap().expect("串流應先送出 ready 事件");
    assert!(String::from_utf8_lossy(&first).contains("event: ready"));
}

#[tokio::test]
async fn stream_rejects_web_origins_and_foreign_hosts_like_any_other_endpoint() {
    let harness = Harness::start().await;

    let origin = authed(&harness, EVENTS_PATH).header(ORIGIN, "https://evil.example").send().await.unwrap();
    let host = authed(&harness, EVENTS_PATH).header(HOST, "evil.example").send().await.unwrap();

    assert_eq!(origin.status(), StatusCode::FORBIDDEN);
    assert_eq!(host.status(), StatusCode::FORBIDDEN);
}

// ---- 網頁存取防護：Origin、Host、CORS ----

#[tokio::test]
async fn arbitrary_web_origin_is_rejected_even_with_a_valid_credential() {
    let harness = Harness::start().await;

    for origin in ["https://evil.example", "http://localhost:5173", "null", "http://127.0.0.1"] {
        let response = authed(&harness, HEALTH_PATH).header(ORIGIN, origin).send().await.unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN, "Origin {origin}");
        assert_eq!(error_code(response).await, "origin_not_allowed");
    }
}

#[tokio::test]
async fn tauri_app_origins_are_accepted() {
    let harness = Harness::start().await;

    for origin in ["http://tauri.localhost", "https://tauri.localhost", "tauri://localhost"] {
        let response = authed(&harness, HEALTH_PATH).header(ORIGIN, origin).send().await.unwrap();

        assert_eq!(response.status(), StatusCode::OK, "Origin {origin}");
    }
}

#[tokio::test]
async fn non_loopback_host_is_rejected_even_with_a_valid_credential() {
    let harness = Harness::start().await;
    let port = harness.addr().port();

    for host in [
        "evil.example".to_string(),
        format!("evil.example:{port}"),
        format!("127.0.0.1.evil.example:{port}"),
        format!("192.168.1.10:{port}"),
        // 既定埠以外的 Host 也不接受（防止 DNS rebinding 搭配其他服務的埠）
        format!("127.0.0.1:{}", port.wrapping_add(1)),
        "127.0.0.1".to_string(),
    ] {
        let response = authed(&harness, HEALTH_PATH).header(HOST, &host).send().await.unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN, "Host {host}");
        assert_eq!(error_code(response).await, "host_not_allowed");
    }
}

#[tokio::test]
async fn cors_preflight_gets_no_cors_headers_and_is_not_successful() {
    let harness = Harness::start().await;
    let token = harness.desktop_credential();

    for credentials in [false, true] {
        let mut request = harness
            .request(Method::OPTIONS, HEALTH_PATH)
            .header(ORIGIN, "http://tauri.localhost")
            .header("access-control-request-method", "GET")
            .header("access-control-request-headers", "authorization,x-baton-client");
        if credentials {
            request = with_token(request, &token);
        }

        let response = request.send().await.unwrap();

        assert!(!response.status().is_success(), "預檢不應成功：{}", response.status());
        let leaked: Vec<_> =
            response.headers().keys().filter(|k| k.as_str().starts_with("access-control-")).collect();
        assert!(leaked.is_empty(), "不應回應任何 CORS 標頭：{leaked:?}");
    }
}

#[tokio::test]
async fn responses_never_carry_cors_headers() {
    let harness = Harness::start().await;

    let ok = authed(&harness, HEALTH_PATH).header(ORIGIN, "http://tauri.localhost").send().await.unwrap();
    let denied = authed(&harness, HEALTH_PATH).header(ORIGIN, "https://evil.example").send().await.unwrap();

    for response in [ok, denied] {
        assert!(response.headers().keys().all(|k| !k.as_str().starts_with("access-control-")));
    }
}

// ---- loopback 綁定 ----

#[tokio::test]
async fn runtime_listens_only_on_loopback_addresses() {
    let harness = Harness::start().await;

    let addrs = harness.local_addrs();

    assert!(!addrs.is_empty());
    assert!(addrs.iter().all(|a| a.ip().is_loopback()), "非 loopback 位址：{addrs:?}");
    assert!(addrs.iter().all(|a| a.port() == harness.addr().port()), "IPv4 與 IPv6 應使用同一個埠");
}

#[tokio::test]
async fn ipv6_loopback_is_served_with_the_same_protections() {
    let harness = Harness::start().await;
    let Some(v6) = harness.addr_v6() else {
        eprintln!("本機沒有 IPv6 loopback，略過");
        return;
    };
    let url = format!("http://{v6}{HEALTH_PATH}");
    let token = harness.desktop_credential();
    let http = reqwest::Client::new();

    let ok = with_token(http.get(&url), &token).send().await.unwrap();
    let missing = http.get(&url).header(CLIENT_HEADER, CLIENT_HEADER_VALUE).send().await.unwrap();

    assert_eq!(ok.status(), StatusCode::OK);
    assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);
}

// ---- 輪替與重啟 ----

#[tokio::test]
async fn restart_rotates_the_desktop_credential_and_old_one_stops_working() {
    let mut harness = Harness::start().await;
    let old = harness.desktop_credential();

    harness.restart().await;
    let new = harness.desktop_credential();

    assert_ne!(old, new, "每次啟動都要輪替桌面憑證");
    let with_old = with_token(harness.request(Method::GET, HEALTH_PATH), &old).send().await.unwrap();
    let with_new = with_token(harness.request(Method::GET, HEALTH_PATH), &new).send().await.unwrap();
    assert_eq!(with_old.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(error_code(with_old).await, "credential_invalid");
    assert_eq!(with_new.status(), StatusCode::OK);
}

#[tokio::test]
async fn restart_rotates_only_the_desktop_role() {
    let mut harness = Harness::start().await;
    let agent = harness.issue_credential(Role::Agent, None).await;

    harness.restart().await;

    // Agent 憑證仍在資料庫中（失效時機由後續的票依 Run 狀態決定），但仍不能用於管理 API
    let response = with_token(harness.request(Method::GET, HEALTH_PATH), &agent).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn desktop_credential_file_is_protected_not_plaintext() {
    let harness = Harness::start().await;
    let token = harness.desktop_credential();

    let raw = std::fs::read(baton_credential::desktop_credential_path(harness.data_dir())).unwrap();

    assert!(!raw.windows(token.len()).any(|w| w == token.as_bytes()));
}

#[tokio::test]
async fn database_never_contains_a_plaintext_token() {
    let harness = Harness::start().await;
    let desktop = harness.desktop_credential();
    let agent = harness.issue_credential(Role::Agent, None).await;

    // 資料庫與 WAL 檔都不含明文 token（只有雜湊）
    for name in ["control.sqlite", "control.sqlite-wal"] {
        let Ok(bytes) = std::fs::read(harness.data_dir().join(name)) else { continue };
        for token in [&desktop, &agent] {
            assert!(!bytes.windows(token.len()).any(|w| w == token.as_bytes()), "{name} 含有明文 token");
        }
    }
}

// ---- 憑證不外洩 ----

#[tokio::test]
async fn credentials_never_appear_in_logs_urls_or_error_messages() {
    let harness = Harness::start().await;
    let desktop = harness.desktop_credential();
    let agent = harness.issue_credential(Role::Agent, None).await;
    let expired = harness.issue_credential(Role::Desktop, Some(Duration::from_secs(1))).await;
    harness.clock.advance_millis(1_000);
    let guessed = "d".repeat(64);
    let secrets = [&desktop, &agent, &expired, &guessed];

    // 各種拒絕與成功路徑，包含把 token 放進 URL、Origin、Host 的惡意請求
    let responses = vec![
        authed(&harness, HEALTH_PATH).send().await.unwrap(),
        with_token(harness.request(Method::GET, HEALTH_PATH), &guessed).send().await.unwrap(),
        with_token(harness.request(Method::GET, HEALTH_PATH), &agent).send().await.unwrap(),
        with_token(harness.request(Method::GET, HEALTH_PATH), &expired).send().await.unwrap(),
        with_token(harness.request(Method::GET, &format!("{HEALTH_PATH}?token={desktop}")), &guessed)
            .send()
            .await
            .unwrap(),
        authed(&harness, HEALTH_PATH).header(ORIGIN, format!("https://{desktop}.evil.example")).send().await.unwrap(),
        authed(&harness, HEALTH_PATH).header(HOST, format!("{desktop}.evil.example")).send().await.unwrap(),
        authed(&harness, &format!("/api/v1/{desktop}")).send().await.unwrap(),
        harness.request(Method::GET, &format!("/api/v1/{desktop}")).send().await.unwrap(),
        authed(&harness, EVENTS_PATH).send().await.unwrap(),
    ];

    let mut surfaces = Vec::new();
    for response in responses {
        surfaces.push(format!("{:?}", response.headers()));
        if response.headers().get("content-type").is_some_and(|c| c == "application/json") {
            surfaces.push(response.text().await.unwrap());
        }
    }
    let log = harness.log_text();

    assert!(log.lines().count() >= 8, "日誌應有請求紀錄，掃描才有意義：\n{log}");
    surfaces.push(log);
    for surface in &surfaces {
        for secret in secrets {
            assert!(!surface.contains(secret.as_str()), "輸出含有憑證：{surface}");
        }
    }
}

#[tokio::test]
async fn open_stream_does_not_block_restart_and_ends_when_runtime_stops() {
    let mut harness = Harness::start().await;
    let mut stream = authed(&harness, EVENTS_PATH).send().await.unwrap();
    stream.chunk().await.unwrap().expect("ready 事件");

    tokio::time::timeout(Duration::from_secs(5), harness.restart()).await.expect("重啟被開著的串流卡住");

    // 舊 Runtime 停止後，串流跟著結束（或被重設），不會一直掛著
    let end = tokio::time::timeout(Duration::from_secs(5), async { while let Ok(Some(_)) = stream.chunk().await {} }).await;
    assert!(end.is_ok(), "串流在 Runtime 停止後仍未結束");
}

#[tokio::test]
async fn runtime_refuses_to_start_on_a_non_loopback_address() {
    use baton_domain::Timestamp;
    use baton_harness::FakePorts;
    use baton_runtime::{Runtime, RuntimeConfig, RuntimeError};

    for bind in ["0.0.0.0:0", "192.168.1.5:0", "[::]:0"] {
        let data_dir = tempfile::tempdir().unwrap();
        let config = RuntimeConfig { data_dir: data_dir.path().to_path_buf(), bind: bind.parse().unwrap() };

        let result = Runtime::start(config, FakePorts::new(Timestamp::from_millis(0)).to_ports()).await;

        assert!(matches!(result, Err(RuntimeError::NotLoopback(_))), "{bind} 應被拒絕");
    }
}
