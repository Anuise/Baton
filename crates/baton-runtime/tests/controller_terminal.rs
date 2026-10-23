//! Controller 終端機（desktop-controller-cli 票 03）：`ClaudeCli::open_controller_terminal` 開出的新主控台
//! 放進與 `WindowsProcessControl::spawn` 相同的具名 Job，能以新的 `WindowsProcessControl` 找回、查驗與結束。
//!
//! 會開出真實視窗的測試標 `#[ignore]`，手動執行：
//! `cargo test -p baton-runtime --test controller_terminal -- --ignored`
#![cfg(windows)]

use std::time::{Duration, Instant};

use baton_ports::{command_hash, ClaudeEnvironment, Liveness, ProcessControl, SpawnSpec, StopResult, TerminalLaunch};
use baton_runtime::claude_cli::ClaudeCli;
use baton_runtime::windows_process::WindowsProcessControl;

const SHORT_GRACE: Duration = Duration::from_millis(100);

fn unique_id(tag: &str) -> String {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    format!("test-{tag}-{}-{nanos}", std::process::id())
}

/// 在終端機中執行約 30 秒的 ping。
fn ping_launch(command_id: &str) -> TerminalLaunch {
    TerminalLaunch {
        spawn: SpawnSpec {
            program: "cmd.exe".to_string(),
            args: ["/c", "ping", "-n", "30", "127.0.0.1"].map(String::from).to_vec(),
            working_dir: Some(std::env::temp_dir().to_string_lossy().into_owned()),
            command_id: command_id.to_string(),
            env: vec![("BATON_CONTROLLER_TEST".to_string(), "1".to_string())],
            ..Default::default()
        },
        title: "Baton Controller（測試）".to_string(),
    }
}

#[tokio::test]
async fn a_command_id_that_already_has_a_job_is_refused_without_opening_a_window() {
    let id = unique_id("controller-dup");
    let launch = ping_launch(&id);
    // 先以背景（不開視窗）的 spawn 佔用同一個 command_id 的 Job
    let control = WindowsProcessControl::new();
    let receipt = control.spawn(launch.spawn.clone()).await.unwrap();

    let error = ClaudeCli.open_controller_terminal(&launch).await.unwrap_err().to_string();

    assert!(error.starts_with("無法開啟 Controller 終端機："), "{error}");
    assert!(error.contains("拒絕重複啟動"), "{error}");
    // 先建立的程序不受影響
    assert_eq!(control.inspect(&receipt.identity).await.unwrap(), Liveness::Alive);
    control.release(&receipt.identity, SHORT_GRACE).await.unwrap();
}

#[tokio::test]
async fn an_invalid_command_id_is_refused_with_a_readable_message() {
    let launch = ping_launch("bad\\id");

    let error = ClaudeCli.open_controller_terminal(&launch).await.unwrap_err().to_string();

    assert!(error.starts_with("無法開啟 Controller 終端機：") && error.contains("command_id"), "{error}");
}

#[tokio::test]
#[ignore = "會開出真實的主控台視窗"]
async fn the_controller_terminal_is_found_inspected_and_stopped_by_a_fresh_process_control() {
    let id = unique_id("controller-window");
    let launch = ping_launch(&id);

    let identity = ClaudeCli.open_controller_terminal(&launch).await.unwrap();
    assert_eq!(identity.command_hash, command_hash(&launch.spawn));

    // 同一個 command_id 再開一次被拒，不開第二個視窗
    let error = ClaudeCli.open_controller_terminal(&launch).await.unwrap_err().to_string();
    assert!(error.contains("拒絕重複啟動"), "{error}");

    // 開窗時沒有留下任何 handle：新的 ProcessControl（相當於 Runtime 重啟後）以 find 找回同一個宿主
    let control = WindowsProcessControl::new();
    let found = control.find(&launch.spawn).await.unwrap().expect("找得回 Controller 終端機的 Job");
    assert_eq!(found.identity, identity);
    assert_eq!(control.inspect(&identity).await.unwrap(), Liveness::Alive);

    let stopped = control.stop(&identity, SHORT_GRACE).await.unwrap();
    assert_eq!(stopped, StopResult::ForceKilled);
    // Job 內程序數歸零後，主控台程序的 handle 要稍晚（實測約十餘毫秒）才變成已結束
    let deadline = Instant::now() + Duration::from_secs(5);
    while control.inspect(&identity).await.unwrap() != Liveness::Dead {
        assert!(Instant::now() < deadline, "結束後根程序仍存活");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(control.find(&launch.spawn).await.unwrap().is_none(), "Job 內已沒有程序");
    control.release(&identity, SHORT_GRACE).await.unwrap();
}
