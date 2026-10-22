//! 正式版驗證命令執行器（票 13）以真實程序測試：結束碼、輸出、工作目錄、找不到程式與逾時。
#![cfg(windows)]

use std::time::Duration;

use baton_ports::{CommandOutput, CommandRequest, CommandRunner};
use baton_runtime::production_ports::LocalCommandRunner;

fn request(dir: &std::path::Path, program: &str, args: &[&str], timeout: Duration) -> CommandRequest {
    CommandRequest {
        program: program.into(),
        args: args.iter().map(|a| a.to_string()).collect(),
        working_dir: dir.to_path_buf(),
        timeout,
    }
}

#[tokio::test]
async fn a_command_reports_its_exit_code_and_output_and_runs_in_the_working_directory() {
    let dir = tempfile::tempdir().unwrap();
    let marker = "baton-verify-marker.txt";
    std::fs::write(dir.path().join(marker), "x").unwrap();

    let output = LocalCommandRunner
        .run(request(dir.path(), "cmd", &["/C", &format!("dir /B & echo 驗證輸出 & exit 3")], Duration::from_secs(30)))
        .await
        .unwrap();

    let CommandOutput::Exited { code, output } = output else { panic!("{output:?}") };
    assert_eq!(code, 3);
    assert!(output.contains(marker), "命令應該在指定的工作目錄執行：{output}");
}

#[tokio::test]
async fn a_program_that_does_not_exist_cannot_start() {
    let dir = tempfile::tempdir().unwrap();
    let result = LocalCommandRunner.run(request(dir.path(), "baton-no-such-program", &[], Duration::from_secs(5))).await;
    assert!(result.is_err(), "{result:?}");
}

#[tokio::test]
async fn a_command_that_runs_past_the_timeout_is_stopped() {
    let dir = tempfile::tempdir().unwrap();
    let started = std::time::Instant::now();

    let output = LocalCommandRunner
        .run(request(dir.path(), "ping", &["-n", "30", "127.0.0.1"], Duration::from_millis(500)))
        .await
        .unwrap();

    assert!(matches!(output, CommandOutput::TimedOut { .. }), "{output:?}");
    assert!(started.elapsed() < Duration::from_secs(10));
}
