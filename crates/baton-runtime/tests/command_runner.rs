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

/// 在工作目錄放兩個批次檔：`late.cmd` 約 5 秒後寫出 `late.txt`；`outer.cmd` 在背景啟動它（繼承輸出管道）後
/// 依 `then` 結束或繼續等待。`late.txt` 出現代表命令再開的子孫程序沒有被終止。
fn write_scripts(dir: &std::path::Path, then: &str) {
    std::fs::write(dir.join("late.cmd"), "@ping -n 6 127.0.0.1 >NUL\r\n@echo late> late.txt\r\n").unwrap();
    std::fs::write(dir.join("outer.cmd"), format!("@echo off\r\nstart /B \"\" cmd /C .\\late.cmd\r\necho partial-evidence\r\n{then}\r\n")).unwrap();
}

#[tokio::test]
async fn a_command_that_exits_while_a_descendant_still_holds_its_output_returns_at_once_and_the_descendant_is_terminated() {
    let dir = tempfile::tempdir().unwrap();
    write_scripts(dir.path(), "exit /b 4");
    let started = std::time::Instant::now();

    let output =
        LocalCommandRunner.run(request(dir.path(), "cmd", &["/C", r".\outer.cmd"], Duration::from_secs(30))).await.unwrap();

    assert!(started.elapsed() < Duration::from_secs(4), "命令結束就回報，不等子孫程序關閉管道：{:?}", started.elapsed());
    let CommandOutput::Exited { code, output } = output else { panic!("{output:?}") };
    assert_eq!(code, 4);
    assert!(output.contains("partial-evidence"), "{output}");
    tokio::time::sleep(Duration::from_secs(7)).await;
    assert!(!dir.path().join("late.txt").exists(), "命令結束時整個 Job 一併終止");
}

#[tokio::test]
async fn a_timed_out_command_keeps_the_output_read_so_far_and_its_descendants_are_terminated() {
    let dir = tempfile::tempdir().unwrap();
    write_scripts(dir.path(), "ping -n 30 127.0.0.1 >NUL");

    let output =
        LocalCommandRunner.run(request(dir.path(), "cmd", &["/C", r".\outer.cmd"], Duration::from_secs(2))).await.unwrap();

    let CommandOutput::TimedOut { output } = output else { panic!("{output:?}") };
    assert!(output.contains("partial-evidence"), "逾時仍保留已讀到的輸出：{output:?}");
    tokio::time::sleep(Duration::from_secs(7)).await;
    assert!(!dir.path().join("late.txt").exists(), "逾時時整個 Job 一併終止");
}
