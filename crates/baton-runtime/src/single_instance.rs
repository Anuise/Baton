//! 單實例鎖：以具名 mutex 確保同一個資料目錄只有一個 Runtime（spec「單實例，具名 mutex」）。

use std::path::Path;

use baton_domain::product;

/// 持有期間代表本程序是該資料目錄的唯一 Runtime；釋放（drop）後其他程序可再取得。
#[derive(Debug)]
pub struct InstanceGuard {
    #[cfg(windows)]
    handle: windows_sys::Win32::Foundation::HANDLE,
}

// HANDLE 是原生 mutex 控制代碼，只在 drop 時關閉，跨執行緒移動是安全的
#[cfg(windows)]
unsafe impl Send for InstanceGuard {}
#[cfg(windows)]
unsafe impl Sync for InstanceGuard {}

/// 依資料目錄產生 mutex 名稱；目錄不分大小寫（Windows 檔案系統語意）。
pub fn mutex_name(data_dir: &Path) -> String {
    let normalized = data_dir.to_string_lossy().replace('/', "\\").to_lowercase();
    // FNV-1a：不依賴標準函式庫雜湊的穩定性，兩個程序一定算出相同名稱
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in normalized.trim_end_matches('\\').bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("Local\\{}.Runtime.{hash:016x}", product::PRODUCT_NAME)
}

#[cfg(windows)]
impl InstanceGuard {
    /// 取得單實例鎖；已有其他程序持有時回傳 `None`。
    pub fn acquire(data_dir: &Path) -> std::io::Result<Option<Self>> {
        use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS};
        use windows_sys::Win32::System::Threading::CreateMutexW;

        let name: Vec<u16> = mutex_name(data_dir).encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY：name 以 NUL 結尾且在呼叫期間有效；安全屬性傳 null 表示預設
        let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
        if handle.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY：緊接在 CreateMutexW 之後讀取，沒有其他 API 呼叫干擾
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            // SAFETY：handle 是剛取得、尚未交給別人的有效控制代碼
            unsafe { CloseHandle(handle) };
            return Ok(None);
        }
        Ok(Some(Self { handle }))
    }
}

#[cfg(windows)]
impl Drop for InstanceGuard {
    fn drop(&mut self) {
        // SAFETY：handle 由 acquire 建立，只在此處關閉一次
        unsafe { windows_sys::Win32::Foundation::CloseHandle(self.handle) };
    }
}

#[cfg(not(windows))]
impl InstanceGuard {
    /// 非 Windows 平台不在支援範圍；僅為了讓程式碼能編譯而永遠取得成功。
    pub fn acquire(_data_dir: &Path) -> std::io::Result<Option<Self>> {
        Ok(Some(Self {}))
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn unique_dir(tag: &str) -> PathBuf {
        PathBuf::from(format!("C:\\baton-test\\{tag}-{}", std::process::id()))
    }

    #[test]
    fn second_acquire_on_same_data_dir_is_refused_until_first_is_dropped() {
        let dir = unique_dir("same");
        let first = InstanceGuard::acquire(&dir).unwrap();
        assert!(first.is_some());
        assert!(InstanceGuard::acquire(&dir).unwrap().is_none());
        drop(first);
        assert!(InstanceGuard::acquire(&dir).unwrap().is_some());
    }

    #[test]
    fn different_data_dirs_can_run_side_by_side() {
        let a = InstanceGuard::acquire(&unique_dir("a")).unwrap();
        let b = InstanceGuard::acquire(&unique_dir("b")).unwrap();
        assert!(a.is_some() && b.is_some());
    }

    #[test]
    fn mutex_name_ignores_case_and_trailing_separator() {
        assert_eq!(mutex_name(Path::new("C:\\Data\\Baton")), mutex_name(Path::new("c:/data/baton/")));
    }
}
