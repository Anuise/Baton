//! Controller 宿主（`controller-host` 子命令）：在外部終端機裡啟動互動式 `claude`，並向 Runtime 回報開始與結束，
//! 讓 Runtime 不必是終端機程序的父程序也能追蹤 Controller CLI 的生命週期。
//!
//! 所有訊息走 stderr（終端機上與 `claude` 共用畫面）；憑證只出現在送往 Runtime 的 `Authorization` 標頭。

use std::ffi::OsString;

use baton_domain::controller_cli::{HOST_ENDED_PATH, HOST_STARTED_PATH};
use baton_domain::controller_wait::SHIM_PATH_ENV;
use serde_json::{json, Value};
use tokio::process::Command;

use crate::{ShimConfig, CLIENT_HEADER, CLIENT_HEADER_VALUE, REQUEST_TIMEOUT};

const PREFIX: &str = "baton-mcp-shim controller-host";
/// 缺少設定或參數。
const EXIT_USAGE: i32 = 2;
/// 平台已結束這個 Controller（啟動回報時憑證已失效）。
const EXIT_REVOKED: i32 = 3;
/// 啟動失敗或取不到子程序結束碼。
const EXIT_FAILURE: i32 = 1;

/// 回報 Runtime 的結果。
enum Report {
    Accepted,
    /// 401：憑證已失效，Runtime 已經把這個 Controller Run 結束了。
    Revoked,
    /// 其他失敗（連線、逾時、非 2xx）；說明不含憑證。
    Failed(String),
}

/// 執行宿主流程：`args` 是 `controller-host` 之後的參數（`<program> [args...]`）。回傳宿主應使用的結束碼。
pub async fn run_controller_host(args: &[OsString], lookup: impl Fn(&str) -> Option<String>) -> i32 {
    let Some((program, program_args)) = args.split_first() else {
        eprintln!("{PREFIX}：缺少要啟動的程式");
        return EXIT_USAGE;
    };
    // 錯誤訊息只含變數名稱，不含憑證
    let config = match ShimConfig::from_lookup(lookup) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("{PREFIX}：{error}");
            return EXIT_USAGE;
        }
    };
    let client = match reqwest::Client::builder().timeout(REQUEST_TIMEOUT).no_proxy().build() {
        Ok(client) => client,
        Err(_) => {
            eprintln!("{PREFIX}：無法建立 HTTP 用戶端");
            return EXIT_FAILURE;
        }
    };
    // 持有到宿主結束：期間 Ctrl+C／Ctrl+Break 不會結束宿主
    let _interrupts = platform::ignore_console_interrupts();
    if let Err(error) = platform::join_kill_on_close_job() {
        eprintln!("{PREFIX}：警告：無法建立程序群組（{error}），宿主結束時可能留下子程序");
    }

    // 繼承標準輸入輸出錯誤、環境變數與目前目錄（tokio 的預設）
    let mut command = Command::new(program);
    command.args(program_args);
    // 讓 Controller 能以 "$BATON_MCP_SHIM" wait 找到 shim
    match std::env::current_exe() {
        Ok(path) => {
            command.env(SHIM_PATH_ENV, path);
        }
        Err(error) => eprintln!("{PREFIX}：警告：無法取得 shim 執行檔路徑（{error}），Controller 將無法執行 wait"),
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            eprintln!("{PREFIX}：無法啟動 {}：{error}", program.to_string_lossy());
            warn_unless_accepted(report(&client, &config, HOST_ENDED_PATH, json!({ "exit_code": null })).await, "結束");
            eprintln!("按 Enter 關閉視窗");
            // 標準輸入關閉時 read_line 立即回傳 0
            let _ = std::io::stdin().read_line(&mut String::new());
            return EXIT_FAILURE;
        }
    };

    if let Some(pid) = child.id() {
        match report(&client, &config, HOST_STARTED_PATH, json!({ "claude_pid": pid })).await {
            Report::Accepted => {}
            Report::Revoked => {
                eprintln!("{PREFIX}：平台已結束這個 Controller，關閉中");
                // 直接子程序先結束；其餘子孫在宿主結束、Job handle 關閉時一併終止
                let _ = child.start_kill();
                let _ = child.wait().await;
                return EXIT_REVOKED;
            }
            Report::Failed(reason) => eprintln!("{PREFIX}：警告：無法回報 Controller 已啟動（{reason}），繼續執行"),
        }
    }

    let exit_code = match child.wait().await {
        Ok(status) => status.code(),
        Err(error) => {
            eprintln!("{PREFIX}：警告：無法取得結束碼（{error}）");
            None
        }
    };
    warn_unless_accepted(report(&client, &config, HOST_ENDED_PATH, json!({ "exit_code": exit_code })).await, "結束");
    exit_code.unwrap_or(EXIT_FAILURE)
}

fn warn_unless_accepted(outcome: Report, what: &str) {
    let reason = match outcome {
        Report::Accepted => return,
        Report::Revoked => "憑證已失效".to_string(),
        Report::Failed(reason) => reason,
    };
    eprintln!("{PREFIX}：警告：無法回報 Controller 已{what}（{reason}）");
}

async fn report(client: &reqwest::Client, config: &ShimConfig, path: &str, body: Value) -> Report {
    let url = format!("{}{path}", config.runtime_url.trim_end_matches('/'));
    let response = client
        .post(url)
        .bearer_auth(&config.token)
        .header(CLIENT_HEADER, CLIENT_HEADER_VALUE)
        .json(&body)
        .send()
        .await;
    match response {
        Ok(response) if response.status().is_success() => Report::Accepted,
        Ok(response) if response.status() == reqwest::StatusCode::UNAUTHORIZED => Report::Revoked,
        Ok(response) => Report::Failed(format!("HTTP {}", response.status().as_u16())),
        Err(error) if error.is_timeout() => Report::Failed("Runtime 回應逾時".to_string()),
        Err(_) => Report::Failed("無法連線到 Runtime".to_string()),
    }
}

#[cfg(windows)]
mod platform {
    use std::io;
    use std::os::windows::io::{FromRawHandle, IntoRawHandle, OwnedHandle};

    use tokio::signal::windows::{ctrl_break, ctrl_c, CtrlBreak, CtrlC};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    /// 註冊主控台事件處理函式並持有接收端：tokio 的處理函式在有接收端時回報「已處理」，宿主就不會被結束。
    /// 處理函式不會被子程序繼承（不同於 `SetConsoleCtrlHandler(NULL, TRUE)`），`claude` 照常收到 Ctrl+C。
    /// 接收端不必輪詢；註冊失敗時宿主維持預設行為。
    pub(super) fn ignore_console_interrupts() -> (Option<CtrlC>, Option<CtrlBreak>) {
        (ctrl_c().ok(), ctrl_break().ok())
    }

    /// 把宿主自己放進設了 `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` 的匿名 Job；之後啟動的子孫程序自動在 Job 內。
    /// Job handle 刻意保持開啟到宿主結束：宿主不論以何種方式結束（正常、被終止、視窗關閉），
    /// handle 一關閉整棵子程序樹就一併終止（`cmd.exe /c call claude.cmd` 時只殺 cmd.exe 不夠）。
    pub(super) fn join_kill_on_close_job() -> io::Result<()> {
        // SAFETY：不具名、使用預設安全性；失敗回傳 NULL。
        let raw = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY：raw 是剛建立、由這裡獨占的有效 handle；中途失敗時由 OwnedHandle 關閉。
        let job = unsafe { OwnedHandle::from_raw_handle(raw) };
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY：job 是有效 handle，結構與大小符合 JobObjectExtendedLimitInformation。
        let ok = unsafe {
            SetInformationJobObject(
                raw,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const std::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY：job 有效；GetCurrentProcess 回傳的是不需關閉的虛擬 handle。
        if unsafe { AssignProcessToJobObject(raw, GetCurrentProcess()) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // 宿主已在 Job 內：此時關閉 handle 會連宿主一起終止，所以交由作業系統在宿主結束時關閉
        let _ = job.into_raw_handle();
        Ok(())
    }
}

#[cfg(not(windows))]
mod platform {
    use std::io;

    // Controller CLI 只在 Windows 上使用；其他平台維持預設行為
    pub(super) fn ignore_console_interrupts() {}

    pub(super) fn join_kill_on_close_job() -> io::Result<()> {
        Ok(())
    }
}
