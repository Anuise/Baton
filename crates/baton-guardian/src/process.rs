//! 以程序識別碼與記錄時間確認程序是否還在（Windows）。
//!
//! 程序識別碼會被重複使用：只有「程序建立時間不晚於記錄時間」而且尚未結束，才算同一個程序還在。
//! 只用 `OpenProcess` 查詢，不持有任何具名物件，不會干擾 Runtime 與監護各自的單實例鎖。

use baton_domain::Timestamp;

#[cfg(windows)]
pub fn alive_since(pid: u32, recorded_at: Timestamp) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
    };

    /// 1601-01-01 到 1970-01-01 的毫秒數（FILETIME 與 Unix epoch 的差）。
    const FILETIME_TO_UNIX_MILLIS: i64 = 11_644_473_600_000;

    // SAFETY：OpenProcess 不需要任何前置條件，失敗回傳 NULL。
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE, 0, pid) };
    if handle.is_null() {
        return false;
    }
    let zero = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
    // SAFETY：handle 有效（PROCESS_QUERY_LIMITED_INFORMATION），四個輸出都指向本地變數。
    let times_ok = unsafe { GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) } != 0;
    // SAFETY：handle 有效（SYNCHRONIZE），逾時為 0 不會阻塞。
    let running = unsafe { WaitForSingleObject(handle, 0) } == WAIT_TIMEOUT;
    // SAFETY：handle 由 OpenProcess 取得，只在這裡關閉一次。
    unsafe { CloseHandle(handle) };
    let ticks = ((created.dwHighDateTime as u64) << 32) | created.dwLowDateTime as u64;
    let created_at = (ticks / 10_000) as i64 - FILETIME_TO_UNIX_MILLIS;
    times_ok && running && created_at <= recorded_at.as_millis()
}

/// 非 Windows 平台不在支援範圍；一律視為不在。
#[cfg(not(windows))]
pub fn alive_since(_pid: u32, _recorded_at: Timestamp) -> bool {
    false
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn now() -> Timestamp {
        Timestamp::from_millis(SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as i64)
    }

    #[test]
    fn this_process_is_alive_but_not_if_recorded_before_it_started() {
        let pid = std::process::id();
        assert!(alive_since(pid, now()));
        assert!(!alive_since(pid, Timestamp::from_millis(0)), "記錄時間早於程序建立：是被重複使用的識別碼");
    }

    #[test]
    fn an_exited_process_is_not_alive() {
        let mut child = std::process::Command::new("cmd").args(["/C", "exit 0"]).spawn().unwrap();
        let pid = child.id();
        child.wait().unwrap();
        assert!(!alive_since(pid, now()));
    }
}
