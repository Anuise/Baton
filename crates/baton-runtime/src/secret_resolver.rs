//! 正式的 SecretResolver（票 46）：秘密參照 `env:NAME` 解析為 Runtime 環境變數 `NAME` 的值。
//!
//! Profile 與資料庫只保存參照；值只在 Runner 執行操作的當下取得，錯誤訊息只提參照、不提值。

use async_trait::async_trait;
use baton_ports::{PortError, PortResult, SecretResolver};

#[derive(Debug, Default)]
pub struct EnvSecretResolver;

#[async_trait]
impl SecretResolver for EnvSecretResolver {
    async fn resolve(&self, reference: &str) -> PortResult<String> {
        let Some(name) = reference.strip_prefix("env:") else {
            return Err(PortError::Failed(format!("不支援的秘密參照「{reference}」；目前只支援 env:<環境變數名稱>")));
        };
        match std::env::var(name) {
            Ok(value) if !value.is_empty() => Ok(value),
            _ => Err(PortError::Failed(format!("Runtime 的環境中沒有設定 {name}（秘密參照 {reference}）"))),
        }
    }
}
