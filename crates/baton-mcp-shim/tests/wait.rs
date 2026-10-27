//! `wait` 子命令的測試：Runtime 用程序內的 stub HTTP server；多數以程序內呼叫 `run_wait` 並捕捉 stdout，
//! 另有幾個啟動真實執行檔，確認 main 的分派、結束碼與輸出不含憑證。

use std::ffi::OsString;
use std::io::{self, Write};
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex};

use axum::extract::Request;
use axum::http::header::{AUTHORIZATION, CONTENT_TYPE};
use axum::http::StatusCode;
use axum::Router;
use baton_domain::agent::{AGENT_TOKEN_ENV, RUNTIME_URL_ENV};
use baton_domain::controller_wait::{
    render_events, ControllerEvent, ControllerEventKind, WaitResponse, DEFAULT_TIMEOUT_SECONDS, EXIT_EVENTS,
    EXIT_FAILURE, EXIT_REVOKED, EXIT_TIMED_OUT, EXIT_USAGE, WAIT_PATH, WAIT_SUBCOMMAND,
};
use baton_domain::Timestamp;
use baton_mcp_shim::run_wait;
use serde_json::{json, Value};

const TOKEN: &str = "tok-WAIT-SECRET-3e9b";

struct Recorded {
    path: String,
    authorization: Option<String>,
    client: Option<String>,
    body: Value,
}

type Log = Arc<Mutex<Vec<Recorded>>>;

/// 起 stub Runtime：等待請求回 `wait_status`／`wait_body`，確認請求（body 帶 `ack`）回 `ack_status`。
async fn spawn_runtime(wait_status: u16, wait_body: String, ack_status: u16) -> (String, Log) {
    let log: Log = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new().fallback({
        let log = log.clone();
        move |request: Request| {
            let log = log.clone();
            let wait_body = wait_body.clone();
            async move {
                let (parts, body_in) = request.into_parts();
                let bytes = axum::body::to_bytes(body_in, usize::MAX).await.unwrap();
                let header = |name: &str| parts.headers.get(name).and_then(|v| v.to_str().ok()).map(str::to_string);
                let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                let is_ack = body.get("ack").is_some();
                log.lock().unwrap().push(Recorded {
                    path: parts.uri.path().to_string(),
                    authorization: header(AUTHORIZATION.as_str()),
                    client: header("x-baton-client"),
                    body,
                });
                let (status, body) = if is_ack {
                    (ack_status, json!({"events": [], "cursor": 0, "timed_out": true}).to_string())
                } else {
                    (wait_status, wait_body)
                };
                (StatusCode::from_u16(status).unwrap(), [(CONTENT_TYPE, "application/json")], body)
            }
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, log)
}

/// 已關閉的埠：連線一定失敗。
fn closed_url() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    format!("http://{}", listener.local_addr().unwrap())
}

fn events() -> Vec<ControllerEvent> {
    let event = |cursor, kind, summary: &str| ControllerEvent {
        cursor,
        kind,
        occurred_at: Timestamp::from_millis(1_700_000_000_000 + cursor),
        task_id: Some("task-1".to_string()),
        run_id: None,
        decision_request_id: None,
        message_id: None,
        summary: summary.to_string(),
    };
    vec![
        event(41, ControllerEventKind::RunEnded, "Run run-7 成功結束（Task「登入頁」）"),
        event(42, ControllerEventKind::TaskBlocked, "Task「登入頁」受阻：缺少 API 金鑰"),
    ]
}

fn events_body() -> String {
    serde_json::to_string(&WaitResponse { events: events(), cursor: 42, timed_out: false }).unwrap()
}

fn timed_out_body() -> String {
    json!({"events": [], "cursor": 40, "timed_out": true}).to_string()
}

fn args(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}

/// 程序內執行 `wait`，回傳結束碼與 stdout。
async fn wait_in_process(url: Option<&str>, values: &[&str]) -> (i32, String) {
    let lookup = |name: &str| match name {
        RUNTIME_URL_ENV => url.map(str::to_string),
        AGENT_TOKEN_ENV => Some(TOKEN.to_string()),
        _ => None,
    };
    let mut out = Vec::new();
    let code = run_wait(&args(values), lookup, &mut out).await;
    let stdout = String::from_utf8(out).unwrap();
    assert!(!stdout.contains(TOKEN), "stdout 不應含憑證：{stdout}");
    (code, stdout)
}

fn assert_authenticated(request: &Recorded) {
    assert_eq!(request.path, WAIT_PATH);
    assert_eq!(request.authorization.as_deref(), Some(format!("Bearer {TOKEN}").as_str()));
    assert_eq!(request.client.as_deref(), Some("baton-desktop"));
}

fn bodies(log: &Log) -> Vec<Value> {
    log.lock().unwrap().iter().map(|r| r.body.clone()).collect()
}

#[tokio::test]
async fn prints_events_then_acknowledges_the_cursor() {
    let (url, log) = spawn_runtime(200, events_body(), 200).await;

    let (code, stdout) = wait_in_process(Some(&url), &[]).await;

    assert_eq!(code, EXIT_EVENTS);
    assert_eq!(stdout, render_events(&events()));
    for event in events() {
        assert!(stdout.contains(&event.summary), "{stdout}");
    }
    assert_eq!(
        bodies(&log),
        [json!({"timeout_seconds": DEFAULT_TIMEOUT_SECONDS}), json!({"ack": 42, "timeout_seconds": 0})]
    );
    log.lock().unwrap().iter().for_each(assert_authenticated);
}

#[tokio::test]
async fn sends_the_requested_timeout() {
    let (url, log) = spawn_runtime(200, timed_out_body(), 200).await;

    let (code, _) = wait_in_process(Some(&url), &["--timeout", "5"]).await;

    assert_eq!(code, EXIT_TIMED_OUT);
    assert_eq!(bodies(&log), [json!({"timeout_seconds": 5})]);
}

#[tokio::test]
async fn timeout_exits_with_4_without_acknowledging() {
    let (url, log) = spawn_runtime(200, timed_out_body(), 200).await;

    let (code, stdout) = wait_in_process(Some(&url), &["--timeout", "0"]).await;

    assert_eq!(code, EXIT_TIMED_OUT);
    assert!(stdout.contains("等待逾時"), "{stdout}");
    assert_eq!(bodies(&log), [json!({"timeout_seconds": 0})]);
    assert_authenticated(&log.lock().unwrap()[0]);
}

#[tokio::test]
async fn revoked_credential_exits_with_3_without_acknowledging() {
    let body = json!({"error": "credential_revoked", "message": "這個 Controller 已結束"}).to_string();
    let (url, log) = spawn_runtime(401, body, 200).await;

    let (code, stdout) = wait_in_process(Some(&url), &[]).await;

    assert_eq!(code, EXIT_REVOKED);
    assert!(stdout.is_empty(), "{stdout}");
    assert_eq!(log.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn other_errors_and_unreachable_runtime_exit_with_1() {
    for (status, body) in [
        (403, json!({"error": "controller_only", "message": "只有 Controller 能等待事件"}).to_string()),
        (503, json!({"error": "unavailable", "message": "Runtime 尚未就緒"}).to_string()),
        (200, "不是 JSON".to_string()),
    ] {
        let (url, log) = spawn_runtime(status, body, 200).await;
        let (code, stdout) = wait_in_process(Some(&url), &[]).await;
        assert_eq!(code, EXIT_FAILURE, "HTTP {status}");
        assert!(stdout.is_empty(), "{stdout}");
        assert_eq!(log.lock().unwrap().len(), 1, "HTTP {status} 不應送確認");
    }

    let (code, stdout) = wait_in_process(Some(&closed_url()), &[]).await;
    assert_eq!(code, EXIT_FAILURE);
    assert!(stdout.is_empty(), "{stdout}");
}

#[tokio::test]
async fn failed_acknowledgement_still_exits_with_0() {
    let (url, log) = spawn_runtime(200, events_body(), 503).await;

    let (code, stdout) = wait_in_process(Some(&url), &[]).await;

    assert_eq!(code, EXIT_EVENTS);
    assert_eq!(stdout, render_events(&events()));
    assert_eq!(log.lock().unwrap().len(), 2);
}

struct BrokenPipe;

impl Write for BrokenPipe {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::Error::from(io::ErrorKind::BrokenPipe))
    }
    fn flush(&mut self) -> io::Result<()> {
        Err(io::Error::from(io::ErrorKind::BrokenPipe))
    }
}

#[tokio::test]
async fn unwritable_stdout_exits_with_1_without_acknowledging() {
    let (url, log) = spawn_runtime(200, events_body(), 200).await;
    let lookup = |name: &str| match name {
        RUNTIME_URL_ENV => Some(url.clone()),
        AGENT_TOKEN_ENV => Some(TOKEN.to_string()),
        _ => None,
    };

    let code = run_wait(&[], lookup, &mut BrokenPipe).await;

    assert_eq!(code, EXIT_FAILURE);
    assert_eq!(bodies(&log), [json!({"timeout_seconds": DEFAULT_TIMEOUT_SECONDS})]);
}

#[tokio::test]
async fn bad_arguments_and_missing_variables_exit_with_2_without_requests() {
    let (url, log) = spawn_runtime(200, events_body(), 200).await;
    for values in [
        &["--timeout", "abc"][..],
        &["--timeout", "99999"],
        &["--timeout", "-1"],
        &["--timeout"],
        &["--verbose"],
        &["5"],
    ] {
        let (code, stdout) = wait_in_process(Some(&url), values).await;
        assert_eq!(code, EXIT_USAGE, "{values:?}");
        assert!(stdout.is_empty(), "{stdout}");
    }

    let (code, stdout) = wait_in_process(None, &[]).await;
    assert_eq!(code, EXIT_USAGE);
    assert!(stdout.is_empty(), "{stdout}");

    assert!(log.lock().unwrap().is_empty());
}

// --- 真實執行檔 ---

/// 啟動 bin 的 `wait` 子命令（憑證只放在環境變數），等它結束；同時確認輸出不含憑證。
async fn wait_process(url: Option<&str>, values: &[&str]) -> (Output, String, String) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_baton-mcp-shim"));
    command.arg(WAIT_SUBCOMMAND).args(values).env(AGENT_TOKEN_ENV, TOKEN);
    match url {
        Some(url) => command.env(RUNTIME_URL_ENV, url),
        None => command.env_remove(RUNTIME_URL_ENV),
    };
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    // 阻塞等待放到 blocking 執行緒，stub Runtime 才能在測試的執行緒上回應
    let output = tokio::task::spawn_blocking(move || command.output().unwrap()).await.unwrap();
    let stdout = String::from_utf8(output.stdout.clone()).unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(!stdout.contains(TOKEN), "stdout 不應含憑證：{stdout}");
    assert!(!stderr.contains(TOKEN), "stderr 不應含憑證：{stderr}");
    (output, stdout, stderr)
}

#[tokio::test]
async fn process_prints_events_and_acknowledges() {
    let (url, log) = spawn_runtime(200, events_body(), 200).await;

    let (output, stdout, stderr) = wait_process(Some(&url), &["--timeout", "5"]).await;

    assert_eq!(output.status.code(), Some(EXIT_EVENTS), "stderr：{stderr}");
    assert_eq!(stdout, render_events(&events()));
    assert_eq!(bodies(&log), [json!({"timeout_seconds": 5}), json!({"ack": 42, "timeout_seconds": 0})]);
    log.lock().unwrap().iter().for_each(assert_authenticated);
}

#[tokio::test]
async fn process_exit_codes_for_timeout_revoked_failure_and_usage() {
    let (url, _) = spawn_runtime(200, timed_out_body(), 200).await;
    let (output, stdout, _) = wait_process(Some(&url), &["--timeout", "1"]).await;
    assert_eq!(output.status.code(), Some(EXIT_TIMED_OUT));
    assert!(stdout.contains("等待逾時"), "{stdout}");

    let (url, _) = spawn_runtime(401, json!({"error": "credential_revoked"}).to_string(), 200).await;
    let (output, _, stderr) = wait_process(Some(&url), &[]).await;
    assert_eq!(output.status.code(), Some(EXIT_REVOKED));
    assert!(stderr.contains("不要再執行 wait"), "{stderr}");

    let (url, _) = spawn_runtime(403, json!({"error": "controller_only"}).to_string(), 200).await;
    let (output, _, stderr) = wait_process(Some(&url), &[]).await;
    assert_eq!(output.status.code(), Some(EXIT_FAILURE));
    assert!(stderr.contains("403") && stderr.contains("controller_only"), "{stderr}");

    let (output, _, stderr) = wait_process(Some(&closed_url()), &[]).await;
    assert_eq!(output.status.code(), Some(EXIT_FAILURE));
    assert!(stderr.contains("無法連線到 Runtime"), "{stderr}");

    let (output, stdout, stderr) = wait_process(None, &[]).await;
    assert_eq!(output.status.code(), Some(EXIT_USAGE));
    assert!(stderr.contains(RUNTIME_URL_ENV), "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");

    let (output, _, _) = wait_process(Some(&closed_url()), &["--timeout", "abc"]).await;
    assert_eq!(output.status.code(), Some(EXIT_USAGE));
}
