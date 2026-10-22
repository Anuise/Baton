//! 跨 Windows 登入的兩個 Port 的正式實作（票 33；DR-20261021-07、ADR-0007、spec D-25）。
//!
//! **登入識別（`WindowsLoginSession`）**：取目前程序 token 的登入工作階段（AuthenticationId LUID），
//! 再以 `LsaGetLogonSessionData` 讀出該工作階段的登入時間（LogonTime，FILETIME）與終端機工作階段識別（Session），
//! 組成 `"{session}-{logon_time}"`。
//! - 不直接用 LUID：LUID 只在一次開機內唯一，重新開機後會重新從小數字配發，相同的值可能再次出現，
//!   會把「重開機後的新登入」誤判成「同一次登入」。登入時間在同一次登入內固定、每次登入都不同，才適合跨開機比較。
//! - 任何一步失敗都回 `None`（Runtime 視為「無法確認」，保守地暫停 Workspace）。
//! - **Validation Required**：以系統管理員身分提升權限執行時，token 屬於 UAC 分割出的連結工作階段（linked logon session），
//!   LUID 與 LogonTime 可能與一般權限的 token 不同，使同一次登入下以不同權限啟動的 Runtime 被判為跨登入。等實機驗證。
//!
//! **登入啟動登記（`HkcuRunRegistration`）**：`HKCU\Software\Microsoft\Windows\CurrentVersion\Run` 下的一個 REG_SZ 值，
//! 內容是登入啟動入口的命令列 `"<baton-guardian.exe>" --login --data-dir "<資料目錄>" --port <埠>`。
//! 只有值存在且命令列完全相同才算已登記，所以資料目錄或埠變了、或執行檔換了位置，Runtime 啟動時都會補正。

use std::path::Path;

use baton_domain::login::{LAUNCHER_DATA_DIR_ARG, LAUNCHER_LOGIN_ARG, LAUNCHER_PORT_ARG};
use baton_domain::product::PRODUCT_NAME;
use baton_ports::{LoginSession, PortError, PortResult, StartupRegistration};
use windows_sys::Win32::Foundation::{CloseHandle, ERROR_FILE_NOT_FOUND, ERROR_UNSUPPORTED_TYPE, LUID};
use windows_sys::Win32::Security::Authentication::Identity::{
    LsaFreeReturnBuffer, LsaGetLogonSessionData, SECURITY_LOGON_SESSION_DATA,
};
use windows_sys::Win32::Security::{GetTokenInformation, TokenStatistics, TOKEN_QUERY, TOKEN_STATISTICS};
use windows_sys::Win32::System::Registry::{
    RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// 目前程序所屬的 Windows 登入識別。
#[derive(Debug, Default, Clone, Copy)]
pub struct WindowsLoginSession;

impl LoginSession for WindowsLoginSession {
    fn current_login(&self) -> Option<String> {
        let luid = current_logon_luid()?;
        let mut data: *mut SECURITY_LOGON_SESSION_DATA = std::ptr::null_mut();
        // SAFETY：luid 與輸出指標都指向有效的區域變數；成功時 data 由 LSA 配置，下方以 LsaFreeReturnBuffer 釋放。
        let status = unsafe { LsaGetLogonSessionData(&luid, &mut data) };
        if status != 0 || data.is_null() {
            return None;
        }
        // SAFETY：data 是 LSA 剛回傳的有效結構，讀完兩個欄位後只釋放一次。
        let (session, logon_time) = unsafe { ((*data).Session, (*data).LogonTime) };
        unsafe { LsaFreeReturnBuffer(data.cast()) };
        // 登入時間是 0 代表讀不到（不能區分不同次的登入），視為無法確認
        (logon_time != 0).then(|| format!("{session}-{logon_time}"))
    }
}

/// 目前程序 token 的登入工作階段 LUID（TOKEN_STATISTICS.AuthenticationId）。
fn current_logon_luid() -> Option<LUID> {
    let mut token = std::ptr::null_mut();
    // SAFETY：GetCurrentProcess 回傳不需關閉的偽控制代碼；輸出參數指向有效的區域變數。
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return None;
    }
    let mut stats = TOKEN_STATISTICS::default();
    let mut returned = 0u32;
    // SAFETY：緩衝區是一個 TOKEN_STATISTICS，大小與傳入的長度一致；token 是剛開啟的有效控制代碼。
    let ok = unsafe {
        GetTokenInformation(
            token,
            TokenStatistics,
            (&mut stats as *mut TOKEN_STATISTICS).cast(),
            std::mem::size_of::<TOKEN_STATISTICS>() as u32,
            &mut returned,
        )
    };
    // SAFETY：token 由 OpenProcessToken 開啟，只在此關閉一次。
    unsafe { CloseHandle(token) };
    (ok != 0).then_some(stats.AuthenticationId)
}

/// 目前使用者的「登入時執行」機碼。
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

/// `HKCU\...\Run` 下的一個值：登入時由 Windows 執行登入啟動入口。
#[derive(Debug, Clone)]
pub struct HkcuRunRegistration {
    value_name: String,
    command: String,
}

impl HkcuRunRegistration {
    pub fn new(value_name: impl Into<String>, command: impl Into<String>) -> Self {
        Self { value_name: value_name.into(), command: command.into() }
    }

    /// 以產品名稱為值名稱，登記 `"<exe>" --login --data-dir "<data_dir>" --port <port>`。
    /// 資料目錄轉成絕對路徑：登入時執行的工作目錄與 Runtime 不同，相對路徑會指到別的目錄。
    pub fn for_launcher(launcher_exe: &Path, data_dir: &Path, port: u16) -> Self {
        let data_dir = std::path::absolute(data_dir).unwrap_or_else(|_| data_dir.to_path_buf());
        let command = format!(
            "{} {LAUNCHER_LOGIN_ARG} {LAUNCHER_DATA_DIR_ARG} {} {LAUNCHER_PORT_ARG} {port}",
            quote(launcher_exe),
            quote(&data_dir)
        );
        Self::new(PRODUCT_NAME, command)
    }

    /// 讀出目前登記的命令列；值不存在（或不是字串型別）時為 `None`。
    fn read(&self) -> PortResult<Option<String>> {
        let (subkey, name) = (wide(RUN_KEY), wide(&self.value_name));
        let mut bytes = 0u32;
        // SAFETY：字串皆以 NUL 結尾；第一次呼叫只查詢所需位元組數。
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                subkey.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut bytes,
            )
        };
        match status {
            0 => {}
            // 型別不是 REG_SZ 也當作「沒有我們的登記」，登記時會覆寫成正確的值
            ERROR_FILE_NOT_FOUND | ERROR_UNSUPPORTED_TYPE => return Ok(None),
            code => return Err(self.failed("讀取", code)),
        }
        let mut buffer = vec![0u16; (bytes as usize).div_ceil(2)];
        // SAFETY：緩衝區大小至少為 `bytes` 位元組。
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                subkey.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                buffer.as_mut_ptr().cast(),
                &mut bytes,
            )
        };
        match status {
            0 => {
                let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
                Ok(Some(String::from_utf16_lossy(&buffer[..end])))
            }
            ERROR_FILE_NOT_FOUND | ERROR_UNSUPPORTED_TYPE => Ok(None),
            code => Err(self.failed("讀取", code)),
        }
    }

    fn failed(&self, action: &str, code: u32) -> PortError {
        PortError::Failed(format!("無法{action}登入啟動登記（HKCU\\{RUN_KEY}\\{}）：Win32 錯誤 {code}", self.value_name))
    }
}

impl StartupRegistration for HkcuRunRegistration {
    fn is_registered(&self) -> PortResult<bool> {
        Ok(self.read()?.as_deref() == Some(self.command.as_str()))
    }

    fn set_registered(&self, enabled: bool) -> PortResult<()> {
        let (subkey, name) = (wide(RUN_KEY), wide(&self.value_name));
        let status = if enabled {
            let data = wide(&self.command);
            // SAFETY：字串皆以 NUL 結尾；資料長度含結尾 NUL（REG_SZ 的要求）。機碼不存在時由 API 建立。
            unsafe {
                RegSetKeyValueW(
                    HKEY_CURRENT_USER,
                    subkey.as_ptr(),
                    name.as_ptr(),
                    REG_SZ,
                    data.as_ptr().cast(),
                    (data.len() * 2) as u32,
                )
            }
        } else {
            // SAFETY：字串皆以 NUL 結尾。
            match unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, subkey.as_ptr(), name.as_ptr()) } {
                ERROR_FILE_NOT_FOUND => 0, // 本來就沒有登記
                status => status,
            }
        };
        match status {
            0 => Ok(()),
            code => Err(self.failed(if enabled { "寫入" } else { "移除" }, code)),
        }
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 以雙引號包住路徑。結尾的反斜線要加倍，否則 `"D:\"` 會被命令列解析成跳脫後的引號。
fn quote(path: &Path) -> String {
    let text = path.display().to_string();
    let trailing = text.len() - text.trim_end_matches('\\').len();
    format!("\"{text}{}\"", "\\".repeat(trailing))
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn login_identity_is_available_and_stable_within_this_login() {
        let first = WindowsLoginSession.current_login().expect("應取得目前的登入識別");
        let second = WindowsLoginSession.current_login().expect("應取得目前的登入識別");
        assert_eq!(first, second, "同一次登入內的識別應相同");
        let (session, logon_time) = first.split_once('-').expect("格式應為 {session}-{logon_time}");
        session.parse::<u32>().expect("Session 應是數字");
        assert!(logon_time.parse::<i64>().expect("LogonTime 應是數字") > 0);
    }

    #[test]
    fn launcher_command_line_quotes_paths_and_uses_the_product_name() {
        let registration = HkcuRunRegistration::for_launcher(
            Path::new(r"C:\Program Files\Baton\baton-guardian.exe"),
            Path::new(r"C:\Users\u\AppData\Local\Baton"),
            47821,
        );
        assert_eq!(registration.value_name, PRODUCT_NAME);
        assert_eq!(
            registration.command,
            r#""C:\Program Files\Baton\baton-guardian.exe" --login --data-dir "C:\Users\u\AppData\Local\Baton" --port 47821"#
        );
        // 磁碟根目錄的結尾反斜線加倍，命令列解析後仍是 `D:\`
        let root = HkcuRunRegistration::for_launcher(Path::new(r"C:\b\baton-guardian.exe"), Path::new(r"D:\"), 1);
        assert!(root.command.contains(r#"--data-dir "D:\\" --port 1"#), "{}", root.command);
    }

    /// 測試結束（含失敗）時刪掉測試用的值。
    struct RemoveOnDrop(HkcuRunRegistration);

    impl Drop for RemoveOnDrop {
        fn drop(&mut self) {
            let _ = self.0.set_registered(false);
        }
    }

    /// 唯一的測試值名稱：絕不可碰到名為產品名稱的真實登記。
    fn test_value_name() -> String {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let name = format!("BatonTest-{}-{nanos}", std::process::id());
        assert_ne!(name, PRODUCT_NAME);
        name
    }

    #[test]
    fn registration_writes_reads_back_overwrites_and_deletes() {
        let name = test_value_name();
        let mine = RemoveOnDrop(HkcuRunRegistration::new(&name, r#""C:\baton-test\does-not-exist.exe" --login"#));
        let other = HkcuRunRegistration::new(&name, r#""C:\baton-test\other.exe" --login"#);

        assert!(!mine.0.is_registered().unwrap(), "一開始沒有登記");
        mine.0.set_registered(true).unwrap();
        assert!(mine.0.is_registered().unwrap(), "寫入後讀回相同的命令列");
        assert_eq!(mine.0.read().unwrap().as_deref(), Some(mine.0.command.as_str()));

        assert!(!other.is_registered().unwrap(), "同名但命令列不同不算已登記");
        other.set_registered(true).unwrap();
        assert!(other.is_registered().unwrap(), "覆寫成另一個命令列");
        assert!(!mine.0.is_registered().unwrap(), "被覆寫後原本的命令列不算已登記");

        mine.0.set_registered(false).unwrap();
        assert_eq!(mine.0.read().unwrap(), None, "刪除後值不存在");
        assert!(!other.is_registered().unwrap());
        mine.0.set_registered(false).unwrap(); // 重複刪除也算成功
    }
}
