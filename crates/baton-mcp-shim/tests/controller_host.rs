//! Controller 宿主子命令的真實程序測試：啟動 bin 的 `controller-host`，Runtime 用程序內的 stub HTTP server。
#![cfg(windows)]

use std::io::Read;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use axum::extract::Request;
use axum::http::header::{AUTHORIZATION, CONTENT_TYPE};
use axum::http::StatusCode;
use axum::Router;
use baton_domain::agent::{AGENT_TOKEN_ENV, RUNTIME_URL_ENV};
use baton_domain::controller_cli::{HOST_ENDED_PATH, HOST_STARTED_PATH, HOST_SUBCOMMAND};
use serde_json::{json, Value};

const TOKEN: &str = "tok-HOST-SECRET-5d81";
/// 宿主必須在這段時間內結束，否則視為卡住。
const HOST_DEADLINE: Duration = Duration::from_secs(20);
/// stub 回 401 前的延遲。
const REVOKE_DELAY: Duration = Duration::from_millis(1500);

struct Recorded {
    path: String,
    authorization: Option<String>,
    client: Option<String>,
    body: Value,
}

type Log = Arc<Mutex<Vec<Recorded>>>;

/// 在獨立執行緒起 stub Runtime：started 回 `started_status`，ended 一律接受。回傳 base URL 與請求紀錄。
fn spawn_runtime(started_status: u16) -> (String, Log) {
    let log: Log = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new().fallback({
        let log = log.clone();
        move |request: Request| {
            let log = log.clone();
            async move {
                let (parts, body_in) = request.into_parts();
                let bytes = axum::body::to_bytes(body_in, usize::MAX).await.unwrap();
                let header = |name: &str| parts.headers.get(name).and_then(|v| v.to_str().ok()).map(str::to_string);
                let path = parts.uri.path().to_string();
                log.lock().unwrap().push(Recorded {
                    path: path.clone(),
                    authorization: header(AUTHORIZATION.as_str()),
                    client: header("x-baton-client"),
                    body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
                });
                let (status, body) = match (path.as_str(), started_status) {
                    (HOST_STARTED_PATH, 401) => {
                        // 延後回應，讓 cmd.exe 先開出孫程序，才驗得到整棵樹都被終止
                        tokio::time::sleep(REVOKE_DELAY).await;
                        (401, json!({"error":"credential_revoked","message":"這個 Controller 已結束"}))
                    }
                    (HOST_STARTED_PATH, status) => (status, json!({"accepted": true})),
                    _ => (200, json!({"accepted": true})),
                };
                (StatusCode::from_u16(status).unwrap(), [(CONTENT_TYPE, "application/json")], body.to_string())
            }
        }
    });
    let (url_tx, url_rx) = mpsc::channel();
    thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        runtime.block_on(async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            url_tx.send(format!("http://{}", listener.local_addr().unwrap())).unwrap();
            axum::serve(listener, app).await.unwrap();
        });
    });
    (url_rx.recv().unwrap(), log)
}

/// 已關閉的埠：連線一定失敗。
fn closed_url() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    format!("http://{}", listener.local_addr().unwrap())
}

struct HostOutput {
    status: ExitStatus,
    stdout: String,
    stderr: String,
    /// 從啟動到 stdout／stderr 都關閉（含子孫程序都結束）的時間。
    elapsed: Duration,
}

fn host(url: Option<&str>, program: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_baton-mcp-shim"));
    command.arg(HOST_SUBCOMMAND).args(program).env(AGENT_TOKEN_ENV, TOKEN);
    match url {
        Some(url) => command.env(RUNTIME_URL_ENV, url),
        None => command.env_remove(RUNTIME_URL_ENV),
    };
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    command
}

/// 執行宿主並等它結束（逾時就終止並失敗）；同時確認輸出不含憑證。
fn run(mut command: Command) -> HostOutput {
    let started = Instant::now();
    let mut child = command.spawn().unwrap();
    let drain = |mut pipe: Box<dyn Read + Send>| {
        thread::spawn(move || {
            // 子程序（如 ping）的輸出是主控台字碼頁，不一定是 UTF-8
            let mut bytes = Vec::new();
            pipe.read_to_end(&mut bytes).unwrap();
            String::from_utf8_lossy(&bytes).into_owned()
        })
    };
    let stdout = drain(Box::new(child.stdout.take().unwrap()));
    let stderr = drain(Box::new(child.stderr.take().unwrap()));
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if started.elapsed() > HOST_DEADLINE {
            let _ = child.kill();
            panic!("宿主在 {HOST_DEADLINE:?} 內沒有結束");
        }
        thread::sleep(Duration::from_millis(50));
    };
    let output = HostOutput { status, stdout: stdout.join().unwrap(), stderr: stderr.join().unwrap(), elapsed: started.elapsed() };
    assert!(!output.stdout.contains(TOKEN), "stdout 不應含憑證：{}", output.stdout);
    assert!(!output.stderr.contains(TOKEN), "stderr 不應含憑證：{}", output.stderr);
    output
}

fn assert_authenticated(request: &Recorded) {
    assert_eq!(request.authorization.as_deref(), Some(format!("Bearer {TOKEN}").as_str()));
    assert_eq!(request.client.as_deref(), Some("baton-desktop"));
}

#[test]
fn reports_started_and_ended_and_exits_with_the_child_code() {
    let (url, log) = spawn_runtime(200);

    let output = run(host(Some(&url), &["cmd.exe", "/c", "exit", "3"]));

    assert_eq!(output.status.code(), Some(3), "stderr：{}", output.stderr);
    let log = log.lock().unwrap();
    assert_eq!(log.iter().map(|r| r.path.as_str()).collect::<Vec<_>>(), [HOST_STARTED_PATH, HOST_ENDED_PATH]);
    assert!(log[0].body["claude_pid"].as_u64().unwrap() > 0, "{}", log[0].body);
    assert_eq!(log[1].body, json!({"exit_code": 3}));
    log.iter().for_each(assert_authenticated);
}

#[test]
fn revoked_credential_on_started_terminates_the_child_tree_without_reporting_ended() {
    let (url, log) = spawn_runtime(401);

    // ping 本身約 29 秒；宿主結束後若孫程序還活著，stdout／stderr 會一直不關閉
    let output = run(host(Some(&url), &["cmd.exe", "/c", "ping", "-n", "30", "127.0.0.1"]));

    assert_eq!(output.status.code(), Some(3), "stderr：{}", output.stderr);
    assert!(output.elapsed < Duration::from_secs(15), "應在幾秒內結束：{:?}", output.elapsed);
    assert!(output.stderr.contains("平台已結束這個 Controller"), "{}", output.stderr);
    let log = log.lock().unwrap();
    assert_eq!(log.iter().map(|r| r.path.as_str()).collect::<Vec<_>>(), [HOST_STARTED_PATH]);
    assert_authenticated(&log[0]);
}

#[test]
fn launch_failure_reports_ended_without_exit_code_and_exits_with_1() {
    let (url, log) = spawn_runtime(200);

    let output = run(host(Some(&url), &["no-such-program-xyz.exe"]));

    assert_eq!(output.status.code(), Some(1), "stderr：{}", output.stderr);
    assert!(output.stderr.contains("按 Enter 關閉視窗"), "{}", output.stderr);
    let log = log.lock().unwrap();
    assert_eq!(log.iter().map(|r| r.path.as_str()).collect::<Vec<_>>(), [HOST_ENDED_PATH]);
    assert_eq!(log[0].body, json!({"exit_code": null}));
    assert_authenticated(&log[0]);
}

#[test]
fn exits_with_code_2_when_a_variable_or_the_program_is_missing() {
    let output = run(host(None, &["cmd.exe", "/c", "exit", "0"]));
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stderr.contains(RUNTIME_URL_ENV), "{}", output.stderr);

    let output = run(host(Some(&closed_url()), &[]));
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn unreachable_runtime_does_not_stop_the_child() {
    let output = run(host(Some(&closed_url()), &["cmd.exe", "/c", "exit", "0"]));
    assert_eq!(output.status.code(), Some(0), "stderr：{}", output.stderr);
}

#[test]
fn passes_the_shim_path_to_the_child() {
    let (url, _) = spawn_runtime(200);

    let output = run(host(Some(&url), &["cmd.exe", "/c", "echo", "%BATON_MCP_SHIM%"]));

    assert_eq!(output.status.code(), Some(0), "stderr：{}", output.stderr);
    let shim = std::path::Path::new(env!("CARGO_BIN_EXE_baton-mcp-shim"));
    let file_name = shim.file_name().unwrap().to_string_lossy().to_lowercase();
    let stdout = output.stdout.to_lowercase();
    // 變數沒設定時 cmd.exe 會原樣印出 %BATON_MCP_SHIM%
    assert!(!stdout.contains("%baton_mcp_shim%"), "{}", output.stdout);
    assert!(stdout.contains(&file_name), "stdout 應含 shim 路徑 {}：{}", shim.display(), output.stdout);
}
