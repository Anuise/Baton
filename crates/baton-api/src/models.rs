//! 可選模型清單端點：提供模型設定的下拉選單選項（桌面角色專用）。
//!
//! 清單是 Runtime 內建的：Baton 不讀 Claude Code 的登入 token，無法以帳號憑證查詢 Anthropic 的模型清單；
//! Claude Code CLI 也沒有不開 session 就能列出模型的指令。清單包含 CLI 接受的別名與完整模型名稱。

use axum::Json;
use serde::{Deserialize, Serialize};

/// 可選模型清單路徑：GET 取得。
pub const MODELS_PATH: &str = "/api/v1/models";

/// 一個可選的模型：`id` 是交給 `claude --model` 的值，`label` 是給使用者看的名稱。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelOption {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelCatalog {
    pub models: Vec<ModelOption>,
}

/// 內建清單：先列別名（永遠指向最新版），再列完整模型名稱。
const BUILTIN_MODELS: [(&str, &str); 8] = [
    ("fable", "Fable（最新版）"),
    ("opus", "Opus（最新版）"),
    ("sonnet", "Sonnet（最新版）"),
    ("haiku", "Haiku（最新版）"),
    ("claude-fable-5-1", "Fable 5.1"),
    ("claude-opus-5-5", "Opus 5.5"),
    ("claude-sonnet-5-5", "Sonnet 5.5"),
    ("claude-haiku-5-5", "Haiku 5.5"),
];

pub(crate) async fn list() -> Json<ModelCatalog> {
    let models = BUILTIN_MODELS.iter().map(|(id, label)| ModelOption { id: (*id).into(), label: (*label).into() }).collect();
    Json(ModelCatalog { models })
}
