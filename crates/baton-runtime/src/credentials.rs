//! 憑證簽發：產生 256 位元隨機 token，資料庫只寫入雜湊（spec D-22）。

use std::sync::Arc;
use std::time::Duration;

use baton_domain::auth::{hash_token, token_from_entropy, CredentialRecord, Role, TOKEN_ENTROPY_BYTES};
use baton_ports::Clock;
use baton_store::Store;

use crate::RuntimeError;

/// 簽發憑證的入口；Run 准入（Agent 憑證）由後續的票呼叫 `issue`。
#[derive(Clone)]
pub struct CredentialIssuer {
    store: Store,
    clock: Arc<dyn Clock>,
}

impl CredentialIssuer {
    pub(crate) fn new(store: Store, clock: Arc<dyn Clock>) -> Self {
        Self { store, clock }
    }

    /// 簽發新憑證並回傳 token；`ttl` 為 `None` 表示沒有時間上限。既有憑證不受影響。
    pub async fn issue(&self, role: Role, ttl: Option<Duration>) -> Result<String, RuntimeError> {
        let (token, record) = self.mint(role, ttl)?;
        self.store.issue_credential(record).await?;
        Ok(token)
    }

    /// 輪替桌面憑證：撤銷所有舊的桌面憑證，只留下新的。桌面憑證沒有時間上限，
    /// 失效靠下一次 Runtime 啟動時再輪替。
    pub(crate) async fn rotate_desktop(&self) -> Result<String, RuntimeError> {
        let (token, record) = self.mint(Role::Desktop, None)?;
        self.store.rotate_credential(record).await?;
        Ok(token)
    }

    fn mint(&self, role: Role, ttl: Option<Duration>) -> Result<(String, CredentialRecord), RuntimeError> {
        let mut entropy = [0u8; TOKEN_ENTROPY_BYTES];
        getrandom::fill(&mut entropy).map_err(|e| RuntimeError::Entropy(e.to_string()))?;
        let token = token_from_entropy(&entropy);
        let now = self.clock.now();
        let record = CredentialRecord {
            token_hash: hash_token(&token),
            role,
            issued_at: now,
            expires_at: ttl.map(|ttl| now.plus_millis(ttl.as_millis() as i64)),
        };
        Ok((token, record))
    }
}
