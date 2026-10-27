//! 本機憑證的純邏輯：角色、token 格式、雜湊與到期判斷（spec D-22）。
//!
//! 隨機位元組由呼叫端提供（Domain 不碰 I/O）；資料庫與記憶體只保存 `hash_token` 的結果。

use sha2::{Digest, Sha256};

use crate::Timestamp;

/// token 的原始熵位元數（256 位元）。
pub const TOKEN_ENTROPY_BYTES: usize = 32;

/// 憑證角色；管理 API 只接受桌面角色，Agent API 只接受 Agent 角色。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Desktop,
    Agent,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Desktop => "desktop",
            Role::Agent => "agent",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "desktop" => Some(Role::Desktop),
            "agent" => Some(Role::Agent),
            _ => None,
        }
    }
}

/// Agent 憑證的範圍：Run 准入時簽發，綁定 Workspace、Run 與世代（spec D-22）。
/// Runtime 一律以它判定資源歸屬，不信任請求內填寫的 Workspace 或 Run。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentScope {
    pub workspace_id: String,
    pub run_id: String,
    /// 簽發時 Run 取得寫入所有權的世代。
    pub generation: i64,
}

/// Controller 憑證的範圍（票 17）：Controller 的一輪（`controller_run`）喚起時簽發，綁定 Workspace 與那一輪。
/// 有效與否取決於那一輪是否還沒結束；它只能使用 Controller 專屬的工具，不能冒充任何 Worker 的 Run。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControllerScope {
    pub workspace_id: String,
    pub controller_run_id: String,
}

/// 一筆已簽發的憑證；只含 token 的雜湊，不含 token 本身。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialRecord {
    pub token_hash: String,
    pub role: Role,
    pub issued_at: Timestamp,
    /// `None` 表示沒有時間上限（桌面憑證靠「下次啟動輪替」失效；Agent 憑證的有效性取決於 Run 是否仍是寫入所有者）。
    pub expires_at: Option<Timestamp>,
    /// Worker 的 Agent 憑證才有範圍；桌面憑證與 Controller 憑證為 `None`。
    pub scope: Option<AgentScope>,
    /// Controller 的 Agent 憑證（票 17）才有；與 `scope` 不會同時存在。
    pub controller: Option<ControllerScope>,
}

impl CredentialRecord {
    /// 到期時間點本身即視為已過期。
    pub fn is_expired(&self, now: Timestamp) -> bool {
        self.expires_at.is_some_and(|expires_at| now >= expires_at)
    }
}

/// 把 256 位元的熵轉成 token（64 個小寫十六進位字元）。
pub fn token_from_entropy(entropy: &[u8; TOKEN_ENTROPY_BYTES]) -> String {
    to_hex(entropy)
}

/// token 的 SHA-256 雜湊（小寫十六進位）；token 本身是 256 位元隨機值，不需要加鹽或慢雜湊。
pub fn hash_token(token: &str) -> String {
    to_hex(&Sha256::digest(token.as_bytes()))
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_is_64_lowercase_hex_chars_derived_from_256_bit_entropy() {
        let token = token_from_entropy(&[0xAB; TOKEN_ENTROPY_BYTES]);

        assert_eq!(token.len(), 64);
        assert!(token.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)));
        assert_eq!(token, "ab".repeat(32));
    }

    #[test]
    fn hash_is_deterministic_and_differs_from_the_token() {
        let token = token_from_entropy(&[7; TOKEN_ENTROPY_BYTES]);

        assert_eq!(hash_token(&token), hash_token(&token));
        assert_ne!(hash_token(&token), token);
        assert_eq!(hash_token(&token).len(), 64);
    }

    #[test]
    fn hash_matches_the_known_sha256_vector() {
        assert_eq!(
            hash_token("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn different_tokens_have_different_hashes() {
        assert_ne!(hash_token("a"), hash_token("b"));
    }

    #[test]
    fn role_round_trips_through_its_string_form() {
        for role in [Role::Desktop, Role::Agent] {
            assert_eq!(Role::parse(role.as_str()), Some(role));
        }
        assert_eq!(Role::parse("admin"), None);
    }

    fn record(expires_at: Option<i64>) -> CredentialRecord {
        CredentialRecord {
            token_hash: "h".into(),
            role: Role::Desktop,
            issued_at: Timestamp::from_millis(0),
            expires_at: expires_at.map(Timestamp::from_millis),
            scope: None,
            controller: None,
        }
    }

    #[test]
    fn credential_without_expiry_never_expires() {
        assert!(!record(None).is_expired(Timestamp::from_millis(i64::MAX)));
    }

    #[test]
    fn credential_expires_exactly_at_its_expiry_time() {
        let record = record(Some(1_000));

        assert!(!record.is_expired(Timestamp::from_millis(999)));
        assert!(record.is_expired(Timestamp::from_millis(1_000)));
        assert!(record.is_expired(Timestamp::from_millis(1_001)));
    }
}
