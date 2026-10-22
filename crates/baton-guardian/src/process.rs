//! 以程序識別碼與記錄時間確認程序是否還在（Windows）。
//!
//! 程序識別碼會被重複使用：只有「程序建立時間不晚於記錄時間」而且尚未結束，才算同一個程序還在。
//! 只用 `OpenProcess` 查詢，不持有任何具名物件，不會干擾 Runtime 與監護各自的單實例鎖。

use baton_domain::Timestamp;

#[cfg(windows)]
pub fn alive_since(pid: u32, recorded_at: Timestamp) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, FILETIME, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    /// 1601-01-01 到 1970-01-01 的毫秒數（FILETIME 與 Unix epoch 的差）。
    const FILETIME_TO_UNIX_MILLIS: i64 = 11_644_473_600_000;

    // 只要求 PROCESS_QUERY_LIMITED_INFORMATION（建立時間與結束碼都只需要它），把存取被拒的機會降到最低
    // SAFETY：OpenProcess 不需要任何前置條件，失敗回傳 NULL。
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        // SAFETY：緊接在 OpenProcess 之後讀取同一執行緒的錯誤碼。
        return still_alive_after_open_failure(unsafe { GetLastError() });
    }
    let zero = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
    // SAFETY：handle 有效（PROCESS_QUERY_LIMITED_INFORMATION），四個輸出都指向本地變數。
    let times_ok = unsafe { GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) } != 0;
    let mut exit_code = 0u32;
    // SAFETY：handle 有效（PROCESS_QUERY_LIMITED_INFORMATION），輸出指向本地變數。
    let code_ok = unsafe { GetExitCodeProcess(handle, &mut exit_code) } != 0;
    // SAFETY：handle 由 OpenProcess 取得，只在這裡關閉一次。
    unsafe { CloseHandle(handle) };
    if !times_ok || !code_ok {
        return true; // 查不到：無法確認已結束
    }
    let ticks = ((created.dwHighDateTime as u64) << 32) | created.dwLowDateTime as u64;
    let created_at = (ticks / 10_000) as i64 - FILETIME_TO_UNIX_MILLIS;
    if created_at > recorded_at.as_millis() {
        return false; // 識別碼已被重複使用：記下的那個程序確定已經結束
    }
    // 結束碼恰好是 STILL_ACTIVE（259）的已結束程序也會被當成還在，偏向保守
    exit_code == STILL_ACTIVE as u32
}

/// `OpenProcess` 失敗時的判斷：只有 `ERROR_INVALID_PARAMETER`（沒有這個識別碼的程序）才確認已經結束；
/// 存取被拒等其他錯誤無法確認，視為還在（監護不重啟、維護流程繼續等），與「只認確認過的終止」一致。
#[cfg(windows)]
fn still_alive_after_open_failure(error: u32) -> bool {
    error != windows_sys::Win32::Foundation::ERROR_INVALID_PARAMETER
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

    #[test]
    fn a_missing_process_is_not_alive_but_an_open_failure_otherwise_is_unknown() {
        assert!(!alive_since(0xFFFF_FFF0, now()), "沒有這個識別碼的程序");
        assert!(!still_alive_after_open_failure(windows_sys::Win32::Foundation::ERROR_INVALID_PARAMETER));
        assert!(still_alive_after_open_failure(windows_sys::Win32::Foundation::ERROR_ACCESS_DENIED));
    }

    #[test]
    fn a_process_this_user_cannot_open_is_not_assumed_gone() {
        // 識別碼 4 是 System 程序，一般使用者開不了（存取被拒）：無法確認已結束，就不能當成已結束
        assert!(alive_since(4, now()));
    }
}
