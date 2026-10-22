//! 正式的 CLI／API 驗證 Port（`LocalVerificationDriver`，票 45）對真實程序、HTTP 與 Git 的行為。

use std::time::Duration;

use baton_ports::{CommandOutput, CommandRequest, HttpOutcome, HttpRequest, ServiceHandle, ServiceRequest, VerificationDriver};
use baton_runtime::verification_driver::LocalVerificationDriver;

fn http(url: String, timeout: Duration) -> HttpRequest {
    HttpRequest { method: "GET".into(), url, headers: Vec::new(), body: None, timeout }
}

#[tokio::test]
async fn commands_report_exit_codes_and_missing_programs_cannot_start() {
    let driver = LocalVerificationDriver::default();
    let dir = tempfile::tempdir().unwrap();
    let (program, args) = if cfg!(windows) { ("cmd", vec!["/C", "echo hi& exit 3"]) } else { ("sh", vec!["-c", "echo hi; exit 3"]) };
    let request = CommandRequest {
        program: program.into(),
        args: args.into_iter().map(String::from).collect(),
        working_dir: dir.path().to_path_buf(),
        timeout: Duration::from_secs(30),
    };
    match driver.run_command(request).await.unwrap() {
        CommandOutput::Exited { code, output } => assert_eq!((code, output.trim()), (3, "hi")),
        other => panic!("{other:?}"),
    }
    let missing = CommandRequest {
        program: "baton-no-such-program".into(),
        args: Vec::new(),
        working_dir: dir.path().to_path_buf(),
        timeout: Duration::from_secs(5),
    };
    assert!(driver.run_command(missing).await.is_err(), "找不到程式是錯誤（環境阻礙），不是結束碼");
}

#[tokio::test]
async fn http_reports_status_unreachable_endpoints_and_timeouts() {
    let driver = LocalVerificationDriver::default();
    // 有回應：狀態碼與本文原樣交回，不跟隨轉址
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = axum::Router::new()
        .route("/missing", axum::routing::get(|| async { (axum::http::StatusCode::NOT_FOUND, "沒有這筆") }))
        .route("/moved", axum::routing::get(|| async { axum::response::Redirect::temporary("/missing") }))
        .route("/slow", axum::routing::get(|| async { tokio::time::sleep(Duration::from_secs(5)).await; "late" }));
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let outcome = driver.send_http(http(format!("http://{addr}/missing"), Duration::from_secs(5))).await.unwrap();
    assert_eq!(outcome, HttpOutcome::Responded { status: 404, body: "沒有這筆".into() });
    let outcome = driver.send_http(http(format!("http://{addr}/moved"), Duration::from_secs(5))).await.unwrap();
    assert!(matches!(outcome, HttpOutcome::Responded { status: 307, .. }), "{outcome:?}");
    let outcome = driver.send_http(http(format!("http://{addr}/slow"), Duration::from_millis(200))).await.unwrap();
    assert_eq!(outcome, HttpOutcome::TimedOut);
    // 連不上：拿一個剛釋放的埠
    let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap().local_addr().unwrap();
    let outcome = driver.send_http(http(format!("http://{closed}/health"), Duration::from_secs(5))).await.unwrap();
    assert!(matches!(outcome, HttpOutcome::Unreachable { .. }), "{outcome:?}");
}

#[tokio::test]
async fn a_started_service_is_stopped_and_stopping_twice_is_harmless() {
    let driver = LocalVerificationDriver::default();
    let dir = tempfile::tempdir().unwrap();
    let (program, args) = if cfg!(windows) { ("ping", vec!["-n", "60", "127.0.0.1"]) } else { ("sleep", vec!["60"]) };
    let handle = driver
        .start_service(ServiceRequest {
            program: program.into(),
            args: args.into_iter().map(String::from).collect(),
            working_dir: dir.path().to_path_buf(),
        })
        .await
        .unwrap();
    driver.stop_service(&handle).await.unwrap();
    driver.stop_service(&handle).await.unwrap();
    let missing = ServiceRequest { program: "baton-no-such-program".into(), args: Vec::new(), working_dir: dir.path().to_path_buf() };
    assert!(driver.start_service(missing).await.is_err());
}

#[tokio::test]
async fn revision_is_the_git_head_or_none_outside_a_repository() {
    let driver = LocalVerificationDriver::default();
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(driver.revision(dir.path()).await.unwrap(), None);
    let git = |args: &[&str]| {
        let status = std::process::Command::new("git").args(args).current_dir(dir.path()).output().unwrap();
        assert!(status.status.success(), "{status:?}");
        String::from_utf8_lossy(&status.stdout).trim().to_string()
    };
    git(&["init", "-q"]);
    git(&["-c", "user.name=t", "-c", "user.email=t@example.com", "commit", "-q", "--allow-empty", "-m", "first"]);
    let head = git(&["rev-parse", "HEAD"]);
    assert_eq!(driver.revision(dir.path()).await.unwrap(), Some(head));
}

#[tokio::test]
async fn the_state_dir_is_recreated_empty_and_removal_of_a_missing_dir_is_not_an_error() {
    let driver = LocalVerificationDriver::default();
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join("verification").join("vreq_1");
    driver.prepare_state_dir(&state).await.unwrap();
    std::fs::write(state.join("leftover.txt"), "上一次中斷留下的").unwrap();
    driver.prepare_state_dir(&state).await.unwrap();
    assert_eq!(std::fs::read_dir(&state).unwrap().count(), 0, "同一個要求重新執行時從空目錄開始");
    driver.remove_state_dir(&state).await.unwrap();
    assert!(!state.exists());
    driver.remove_state_dir(&state).await.unwrap();
}

#[tokio::test]
async fn a_service_started_by_another_runtime_cannot_be_confirmed_stopped() {
    // 承接者（票 49）以前一個 Runtime 留下的識別停止服務：不是這個 Runtime 啟動的，不能回報已停止
    let driver = LocalVerificationDriver::default();
    let error = driver.stop_service(&ServiceHandle("svc-1-1-1".into())).await.unwrap_err();
    assert!(error.to_string().contains("無法確認已停止"), "{error}");
}
