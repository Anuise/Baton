//! 可選模型清單端點：提供模型設定的下拉選單選項（桌面角色專用）。
//!
//! - `GET /api/v1/models`：使用者更新過就回傳保存的清單與更新時間，否則回傳 Runtime 內建的清單（AI-32）。
//! - `POST /api/v1/models/refresh`：使用者貼上 Anthropic API 金鑰，Runtime 以它查詢 models API（處理分頁），
//!   成功才保存新清單；失敗時保存的清單不變（AI-33）。
//!
//! 金鑰只用於這一次查詢：不寫入資料庫、檔案、日誌或事件，錯誤訊息中出現時也會遮蔽。
//! Baton 不讀 Claude Code 的登入 token；Claude Code CLI 也沒有不開 session 就能列出模型的指令。

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use baton_domain::Timestamp;
use baton_ports::{ModelDirectoryError, RemoteModel};
use serde::{Deserialize, Serialize};

use crate::workspaces::ApiError;
use crate::ApiState;

/// 可選模型清單路徑：GET 取得。
pub const MODELS_PATH: &str = "/api/v1/models";
/// 以使用者提供的 API 金鑰更新清單：POST。
pub const MODELS_REFRESH_PATH: &str = "/api/v1/models/refresh";

/// 更新後的清單保存在資料目錄下的這個檔案（只有模型與更新時間，沒有金鑰）。
const SAVED_CATALOG_FILE: &str = "model-catalog.json";
/// 分頁上限：避免異常回應（一直 has_more）讓請求不會結束。
const MAX_PAGES: usize = 50;

/// 一個可選的模型：`id` 是交給 `claude --model` 的值，`label` 是給使用者看的名稱。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelOption {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelCatalog {
    pub models: Vec<ModelOption>,
    /// 清單來源：`None` 是內建清單；有值是使用者以 API 金鑰更新的時間。
    pub updated_at: Option<Timestamp>,
}

/// 更新清單的請求；`Debug` 不印出金鑰。
#[derive(Clone, Serialize, Deserialize)]
pub struct RefreshModelsRequest {
    pub api_key: String,
}

impl fmt::Debug for RefreshModelsRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RefreshModelsRequest").field("api_key", &"<已遮蔽>").finish()
    }
}

/// CLI 接受的別名（永遠指向最新版）；更新後的清單也保留在最前面（AI-48）。
const ALIASES: [(&str, &str); 4] =
    [("fable", "Fable（最新版）"), ("opus", "Opus（最新版）"), ("sonnet", "Sonnet（最新版）"), ("haiku", "Haiku（最新版）")];

/// 內建的完整模型名稱，接在別名之後。
const BUILTIN_FULL_NAMES: [(&str, &str); 4] = [
    ("claude-fable-5-1", "Fable 5.1"),
    ("claude-opus-5-5", "Opus 5.5"),
    ("claude-sonnet-5-5", "Sonnet 5.5"),
    ("claude-haiku-5-5", "Haiku 5.5"),
];

fn options(pairs: &[(&str, &str)]) -> Vec<ModelOption> {
    pairs.iter().map(|(id, label)| ModelOption { id: (*id).into(), label: (*label).into() }).collect()
}

fn builtin() -> ModelCatalog {
    let mut models = options(&ALIASES);
    models.extend(options(&BUILTIN_FULL_NAMES));
    ModelCatalog { models, updated_at: None }
}

fn saved_catalog_path(data_dir: &Path) -> PathBuf {
    data_dir.join(SAVED_CATALOG_FILE)
}

/// 保存的清單；從未更新或檔案讀不了時退回內建清單。
fn current_catalog(data_dir: &Path) -> ModelCatalog {
    std::fs::read(saved_catalog_path(data_dir))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<ModelCatalog>(&bytes).ok())
        .filter(|catalog| catalog.updated_at.is_some() && !catalog.models.is_empty())
        .unwrap_or_else(builtin)
}

/// 先寫暫存檔再改名，中途失敗不會留下半個檔案。
fn save_catalog(data_dir: &Path, catalog: &ModelCatalog) -> std::io::Result<()> {
    let path = saved_catalog_path(data_dir);
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, serde_json::to_vec_pretty(catalog)?)?;
    std::fs::rename(&temp, &path)
}

pub(crate) async fn list(State(state): State<Arc<ApiState>>) -> Json<ModelCatalog> {
    Json(current_catalog(&state.data_dir))
}

pub(crate) async fn refresh(
    State(state): State<Arc<ApiState>>,
    body: Result<Json<RefreshModelsRequest>, JsonRejection>,
) -> Result<Json<ModelCatalog>, ApiError> {
    // 解析錯誤的細節可能引用請求內容，所以只回固定說明
    let Json(request) = body.map_err(|_| ApiError {
        status: StatusCode::BAD_REQUEST,
        code: "bad_request",
        message: "請求內容不是有效的更新要求（需要 api_key）".into(),
    })?;
    let api_key = request.api_key.trim();
    if api_key.is_empty() {
        return Err(ApiError { status: StatusCode::BAD_REQUEST, code: "model_api_key_missing", message: "請貼上 API 金鑰".into() });
    }
    let remote = fetch_all(&state, api_key).await.map_err(|error| {
        let (status, code) = match error {
            ModelDirectoryError::KeyRejected(_) => (StatusCode::BAD_REQUEST, "model_api_key_rejected"),
            _ => (StatusCode::BAD_GATEWAY, "model_directory_unavailable"),
        };
        // 保險：實作若把金鑰帶進錯誤訊息，回給使用者之前遮掉
        ApiError { status, code, message: format!("無法更新可用模型清單：{}", error.to_string().replace(api_key, "<已遮蔽>")) }
    })?;
    let mut models = options(&ALIASES);
    for model in remote {
        if !models.iter().any(|m| m.id == model.id) {
            let label = if model.display_name.trim().is_empty() { model.id.clone() } else { model.display_name };
            models.push(ModelOption { id: model.id, label });
        }
    }
    let catalog = ModelCatalog { models, updated_at: Some(state.clock.now()) };
    save_catalog(&state.data_dir, &catalog).map_err(|e| ApiError {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        code: "internal_error",
        message: format!("無法保存可用模型清單：{e}"),
    })?;
    Ok(Json(catalog))
}

/// 依 `has_more`／`last_id` 取完全部分頁；沒有任何模型視為無法辨識的回應（保留原清單）。
async fn fetch_all(state: &ApiState, api_key: &str) -> Result<Vec<RemoteModel>, ModelDirectoryError> {
    let mut models = Vec::new();
    let mut after_id: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let page = state.model_directory.page(api_key, after_id.as_deref()).await?;
        models.extend(page.data);
        if !page.has_more {
            if models.is_empty() {
                return Err(ModelDirectoryError::Invalid("沒有回傳任何模型".into()));
            }
            return Ok(models);
        }
        match page.last_id {
            Some(last_id) if after_id.as_deref() != Some(last_id.as_str()) => after_id = Some(last_id),
            _ => return Err(ModelDirectoryError::Invalid("分頁沒有前進（has_more 為真但 last_id 缺少或重複）".into())),
        }
    }
    Err(ModelDirectoryError::Invalid(format!("分頁超過 {MAX_PAGES} 頁")))
}
