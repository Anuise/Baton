//! DPAPI 實作：以目前使用者範圍保護（`CryptProtectData` 不加 `CRYPTPROTECT_LOCAL_MACHINE`）。

use baton_ports::{PortError, PortResult, SecretProtector};

pub struct DpapiProtector;

#[cfg(windows)]
mod imp {
    use super::*;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    fn blob(bytes: &[u8]) -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB { cbData: bytes.len() as u32, pbData: bytes.as_ptr() as *mut u8 }
    }

    /// 取出 DPAPI 配置的輸出並釋放。
    fn take(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
        // SAFETY：呼叫成功後 pbData 指向 cbData 個有效位元組，且需以 LocalFree 釋放
        unsafe {
            let bytes = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
            LocalFree(out.pbData as _);
            bytes
        }
    }

    impl SecretProtector for DpapiProtector {
        fn protect(&self, plaintext: &[u8]) -> PortResult<Vec<u8>> {
            let input = blob(plaintext);
            let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
            // SAFETY：input 在呼叫期間有效；其餘選用參數傳 null
            let ok = unsafe {
                CryptProtectData(
                    &input,
                    std::ptr::null(),
                    std::ptr::null(),
                    std::ptr::null(),
                    std::ptr::null(),
                    CRYPTPROTECT_UI_FORBIDDEN,
                    &mut out,
                )
            };
            if ok == 0 {
                return Err(PortError::Failed(format!("CryptProtectData 失敗：{}", std::io::Error::last_os_error())));
            }
            Ok(take(out))
        }

        fn unprotect(&self, protected: &[u8]) -> PortResult<Vec<u8>> {
            let input = blob(protected);
            let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
            // SAFETY：同 protect
            let ok = unsafe {
                CryptUnprotectData(
                    &input,
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    std::ptr::null(),
                    std::ptr::null(),
                    CRYPTPROTECT_UI_FORBIDDEN,
                    &mut out,
                )
            };
            if ok == 0 {
                return Err(PortError::Failed(format!("CryptUnprotectData 失敗：{}", std::io::Error::last_os_error())));
            }
            Ok(take(out))
        }
    }
}

#[cfg(not(windows))]
impl SecretProtector for DpapiProtector {
    fn protect(&self, _plaintext: &[u8]) -> PortResult<Vec<u8>> {
        Err(PortError::NotImplemented("DPAPI 只支援 Windows"))
    }

    fn unprotect(&self, _protected: &[u8]) -> PortResult<Vec<u8>> {
        Err(PortError::NotImplemented("DPAPI 只支援 Windows"))
    }
}
