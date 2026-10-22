use crate::PortResult;

/// 以目前使用者身分保護小段祕密（Windows 上為 DPAPI，spec D-22）。
///
/// 保護後的位元組只有同一個使用者帳號能還原；用於把桌面憑證交給 App。
pub trait SecretProtector: Send + Sync {
    fn protect(&self, plaintext: &[u8]) -> PortResult<Vec<u8>>;
    fn unprotect(&self, protected: &[u8]) -> PortResult<Vec<u8>>;
}
