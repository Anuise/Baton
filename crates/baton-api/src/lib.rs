//! API：Axum 路由。
//!
//! 目前只有健康狀態端點；身分驗證與網頁存取防護由票 04 加入，
//! 管理 API 與 Agent API 的分流由後續的票加入。

use std::sync::Arc;

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use baton_domain::{product, Timestamp};
use serde::{Deserialize, Serialize};

/// 健康端點路徑。
pub const HEALTH_PATH: &str = "/api/v1/health";

/// 健康端點的回應內容；Runtime 與用戶端（App、測試 harness）共用。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: String,
    pub product: String,
    pub version: String,
    /// 這個資料目錄累計啟動 Runtime 的次數（含目前這一次）。
    pub boot_count: u64,
    /// 本次 Runtime 的啟動時間（Clock Port 取得，UTC）。
    pub started_at: Timestamp,
    /// 執行 Runtime 的程序識別碼（診斷與冒煙測試用）。
    pub pid: u32,
}

/// Router 需要的執行期狀態。
#[derive(Clone)]
pub struct ApiState {
    pub boot_count: u64,
    pub started_at: Timestamp,
}

pub fn router(state: ApiState) -> Router {
    Router::new()
        .route(HEALTH_PATH, get(health))
        .with_state(Arc::new(state))
}

async fn health(State(state): State<Arc<ApiState>>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_string(),
        product: product::PRODUCT_NAME.to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        boot_count: state.boot_count,
        started_at: state.started_at,
        pid: std::process::id(),
    })
}
