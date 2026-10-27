//! 桌面憑證檔：Runtime 每次啟動輪替桌面憑證並寫入此檔，桌面 App 讀取後連線，
//! 使用者不需要複製密鑰（spec D-22）。檔案內容經 `SecretProtector`（正式環境為 DPAPI）保護。

mod dpapi;

use std::path::{Path, PathBuf};

use baton_ports::SecretProtector;

pub use dpapi::DpapiProtector;

/// 憑證檔相對於資料目錄的位置。
const CREDENTIAL_FILE: &str = "auth/desktop-credential.bin";

/// 讀寫憑證檔的錯誤；訊息不含憑證內容。
#[derive(Debug, thiserror::Error)]
pub enum CredentialFileError {
    #[error("找不到桌面憑證檔 {0}（Runtime 尚未啟動過這個資料目錄？）")]
    NotFound(PathBuf),
    #[error("無法讀寫桌面憑證檔 {0}：{1}")]
    Io(PathBuf, std::io::Error),
    #[error("無法以目前使用者身分還原桌面憑證（檔案損毀或屬於其他使用者）：{0}")]
    Unprotect(String),
    #[error("無法保護桌面憑證：{0}")]
    Protect(String),
    #[error("桌面憑證檔的內容不是有效的憑證")]
    Malformed,
}

pub fn desktop_credential_path(data_dir: &Path) -> PathBuf {
    data_dir.join(CREDENTIAL_FILE)
}

/// 保護後寫入憑證檔；先寫暫存檔再改名，讀取端不會看到寫到一半的內容。
pub fn write_desktop_credential(
    data_dir: &Path,
    protector: &dyn SecretProtector,
    token: &str,
) -> Result<(), CredentialFileError> {
    let path = desktop_credential_path(data_dir);
    let io = |e| CredentialFileError::Io(path.clone(), e);
    let protected = protector.protect(token.as_bytes()).map_err(|e| CredentialFileError::Protect(e.to_string()))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(io)?;
    }
    let temp = path.with_extension("tmp");
    std::fs::write(&temp, protected).map_err(io)?;
    std::fs::rename(&temp, &path).map_err(io)
}

/// 讀取並還原桌面憑證。
pub fn read_desktop_credential(
    data_dir: &Path,
    protector: &dyn SecretProtector,
) -> Result<String, CredentialFileError> {
    let path = desktop_credential_path(data_dir);
    let protected = std::fs::read(&path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => CredentialFileError::NotFound(path.clone()),
        _ => CredentialFileError::Io(path.clone(), e),
    })?;
    let plain = protector.unprotect(&protected).map_err(|e| CredentialFileError::Unprotect(e.to_string()))?;
    String::from_utf8(plain).ok().filter(|t| !t.is_empty()).ok_or(CredentialFileError::Malformed)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn written_credential_can_be_read_back() {
        let dir = tempfile::tempdir().unwrap();

        write_desktop_credential(dir.path(), &DpapiProtector, TOKEN).unwrap();

        assert_eq!(read_desktop_credential(dir.path(), &DpapiProtector).unwrap(), TOKEN);
    }

    #[test]
    fn file_on_disk_does_not_contain_the_plaintext_token() {
        let dir = tempfile::tempdir().unwrap();

        write_desktop_credential(dir.path(), &DpapiProtector, TOKEN).unwrap();

        let raw = std::fs::read(desktop_credential_path(dir.path())).unwrap();
        assert!(!raw.windows(TOKEN.len()).any(|w| w == TOKEN.as_bytes()));
    }

    #[test]
    fn rewriting_replaces_the_previous_credential() {
        let dir = tempfile::tempdir().unwrap();
        write_desktop_credential(dir.path(), &DpapiProtector, "old").unwrap();

        write_desktop_credential(dir.path(), &DpapiProtector, "new").unwrap();

        assert_eq!(read_desktop_credential(dir.path(), &DpapiProtector).unwrap(), "new");
    }

    #[test]
    fn missing_file_is_reported_as_not_found() {
        let dir = tempfile::tempdir().unwrap();

        let error = read_desktop_credential(dir.path(), &DpapiProtector).unwrap_err();

        assert!(matches!(error, CredentialFileError::NotFound(_)));
    }

    #[test]
    fn corrupted_file_is_reported_as_unprotect_failure_without_leaking_content() {
        let dir = tempfile::tempdir().unwrap();
        write_desktop_credential(dir.path(), &DpapiProtector, TOKEN).unwrap();
        std::fs::write(desktop_credential_path(dir.path()), b"not a dpapi blob").unwrap();

        let error = read_desktop_credential(dir.path(), &DpapiProtector).unwrap_err();

        assert!(matches!(error, CredentialFileError::Unprotect(_)));
        assert!(!error.to_string().contains(TOKEN));
    }
}
