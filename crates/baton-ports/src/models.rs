//! Anthropic models API：使用者貼上 API 金鑰更新模型設定的可用清單（AI-33）。
//!
//! 金鑰只隨單次查詢傳入；實作不得保存、記錄，也不得把它放進錯誤訊息。

use async_trait::async_trait;
use serde::Deserialize;

/// Anthropic 回報的一個可用模型。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RemoteModel {
    pub id: String,
    pub display_name: String,
}

/// models API 的一頁：`has_more` 為真時以 `last_id` 當下一頁的 `after_id`。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ModelPage {
    pub data: Vec<RemoteModel>,
    pub has_more: bool,
    #[serde(default)]
    pub last_id: Option<String>,
}

/// 查詢失敗；訊息不含金鑰。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ModelDirectoryError {
    /// 金鑰無效或沒有權限（HTTP 401／403）。
    #[error("Anthropic 拒絕了這把 API 金鑰（HTTP {0}）")]
    KeyRejected(u16),
    /// 其他非成功的 HTTP 狀態。
    #[error("Anthropic 回應 HTTP {0}")]
    Status(u16),
    #[error("無法連線到 Anthropic：{0}")]
    Unreachable(String),
    #[error("無法辨識 Anthropic 的回應：{0}")]
    Invalid(String),
}

/// 向 Anthropic 查詢可用模型（一次一頁）。
#[async_trait]
pub trait ModelDirectory: Send + Sync {
    /// 以 `api_key` 取得一頁；`after_id` 是上一頁的 `last_id`（第一頁為 `None`）。
    async fn page(&self, api_key: &str, after_id: Option<&str>) -> Result<ModelPage, ModelDirectoryError>;
}
