//! 監護程序執行檔（`baton-guardian.exe`，票 32）。由桌面 App 在 Runtime 就緒後啟動，與 `baton-runtime.exe` 放在同一個資料夾。
//!
//! 已有監護在執行就直接結束；沒有執行中的 Runtime 可監看也直接結束（監護只重啟它親眼看到結束的 Runtime）。

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use baton_domain::config::{RuntimeConfig, ENV_DATA_DIR, ENV_PORT};
use baton_domain::Timestamp;
use baton_guardian::files::{self, InstanceRecord};
use baton_guardian::host::ProcessHost;
use baton_guardian::{Guardian, GuardianStep, RuntimeHost};
use baton_ports::Clock;

/// Runtime 執行檔（與監護放在同一個資料夾）。
const RUNTIME_EXE: &str = "baton-runtime.exe";
/// 監看的間隔：只查詢程序狀態，負擔很輕。
const POLL: Duration = Duration::from_secs(1);

struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        let millis = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        Timestamp::from_millis(millis as i64)
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::process::ExitCode {
    match run().await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("監護程序結束：{message}");
            std::process::ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), String> {
    let config = RuntimeConfig::from_lookup(|key| std::env::var(key).ok()).map_err(|e| e.to_string())?;
    let data_dir = config.data_dir.clone();
    let Some(_lock) = lock::GuardianLock::acquire(&data_dir).map_err(|e| e.to_string())? else {
        return Ok(()); // 已有監護在執行
    };
    let clock = Arc::new(SystemClock);
    files::write_instance(
        &files::guardian_instance_path(&data_dir),
        &InstanceRecord { pid: std::process::id(), recorded_at: clock.now() },
    )
    .map_err(|e| format!("無法記下監護的程序身分：{e}"))?;

    let runtime_exe = std::env::current_exe().map_err(|e| e.to_string())?.with_file_name(RUNTIME_EXE);
    let envs = vec![
        (ENV_DATA_DIR.to_string(), data_dir.display().to_string()),
        (ENV_PORT.to_string(), config.bind.port().to_string()),
    ];
    let mut host = ProcessHost::new(runtime_exe, data_dir.clone(), envs);
    if !host.runtime_running().await {
        files::append_log(&data_dir, clock.now(), "沒有執行中的 Runtime 可監看，監護結束");
        return Ok(());
    }
    files::append_log(&data_dir, clock.now(), "監護開始監看 Runtime");

    let mut guardian = Guardian::new(data_dir.clone(), clock.clone());
    loop {
        match guardian.step(&mut host).await {
            GuardianStep::Watching => tokio::time::sleep(POLL).await,
            GuardianStep::Waiting { until } => {
                let remaining = (until.as_millis() - clock.now().as_millis()).clamp(0, POLL.as_millis() as i64);
                tokio::time::sleep(Duration::from_millis(remaining as u64)).await;
            }
            // 剛啟動或剛確認結束：稍等一下再看（避免空轉）
            GuardianStep::RuntimeExited
            | GuardianStep::Restarted(_)
            | GuardianStep::Withdrawn { .. }
            | GuardianStep::StartFailed { .. } => tokio::time::sleep(Duration::from_millis(200)).await,
            GuardianStep::Finished(_) => return Ok(()),
        }
    }
}

/// 監護的單實例鎖（具名 mutex）：同一個資料目錄只有一個監護。名稱與 Runtime 的不同，兩者互不干擾。
mod lock {
    use super::*;

    pub struct GuardianLock {
        #[cfg(windows)]
        handle: windows_sys::Win32::Foundation::HANDLE,
    }

    /// 依資料目錄產生 mutex 名稱（雜湊方式與 Runtime 的單實例鎖相同，只是後綴不同）。
    fn mutex_name(data_dir: &Path) -> String {
        let normalized = data_dir.to_string_lossy().replace('/', "\\").to_lowercase();
        let mut hash: u64 = 0xcbf29ce484222325;
        for byte in normalized.trim_end_matches('\\').bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        format!("Local\\{}.Guardian.{hash:016x}", baton_domain::product::PRODUCT_NAME)
    }

    #[cfg(windows)]
    impl GuardianLock {
        pub fn acquire(data_dir: &Path) -> std::io::Result<Option<Self>> {
            use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS};
            use windows_sys::Win32::System::Threading::CreateMutexW;

            let name: Vec<u16> = mutex_name(data_dir).encode_utf16().chain(std::iter::once(0)).collect();
            // SAFETY：name 以 NUL 結尾且在呼叫期間有效；安全屬性傳 null 表示預設
            let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
            if handle.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            // SAFETY：緊接在 CreateMutexW 之後讀取
            if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
                // SAFETY：handle 是剛取得的有效控制代碼
                unsafe { CloseHandle(handle) };
                return Ok(None);
            }
            Ok(Some(Self { handle }))
        }
    }

    #[cfg(windows)]
    impl Drop for GuardianLock {
        fn drop(&mut self) {
            // SAFETY：handle 由 acquire 建立，只在此處關閉一次
            unsafe { windows_sys::Win32::Foundation::CloseHandle(self.handle) };
        }
    }

    #[cfg(not(windows))]
    impl GuardianLock {
        /// 非 Windows 平台不在支援範圍；僅為了讓程式碼能編譯。
        pub fn acquire(_data_dir: &Path) -> std::io::Result<Option<Self>> {
            let _ = mutex_name;
            Ok(Some(Self {}))
        }
    }
}
