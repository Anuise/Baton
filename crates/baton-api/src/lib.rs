//! API：Axum 路由。
//!
//! 目前只有健康狀態端點；身分驗證與網頁存取防護由票 04 加入，
//! 管理 API 與 Agent API 的分流由後續的票加入。

use std::sync::Arc;

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use baton_domain::platform::{self, PlatformAssessment, PlatformSnapshot};
use baton_domain::{product, Timestamp};
use baton_ports::{Clock, PlatformInfo};
use serde::{Deserialize, Serialize};

/// 健康端點路徑。
pub const HEALTH_PATH: &str = "/api/v1/health";

/// 平台狀態端點路徑。
pub const PLATFORM_PATH: &str = "/api/v1/platform";

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

/// 平台狀態端點的回應內容：診斷事實、支援判定與支援期限資料來源。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformResponse {
    pub platform: PlatformSnapshot,
    pub app_version: String,
    pub assessment: PlatformAssessment,
    /// 支援期限表的資料來源與查閱日期。
    pub support_table_source: String,
    pub support_table_checked_on: String,
}

/// Router 需要的執行期狀態。
#[derive(Clone)]
pub struct ApiState {
    pub boot_count: u64,
    pub started_at: Timestamp,
    pub clock: Arc<dyn Clock>,
    pub platform_info: Arc<dyn PlatformInfo>,
}

pub fn router(state: ApiState) -> Router {
    Router::new()
        .route(HEALTH_PATH, get(health))
        .route(PLATFORM_PATH, get(platform_status))
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

/// 每次請求都以目前的時間重新判定，支援期限到期後不必重啟 Runtime。
async fn platform_status(State(state): State<Arc<ApiState>>) -> Json<PlatformResponse> {
    let snapshot = state.platform_info.snapshot();
    let assessment = platform::assess(&snapshot, state.clock.now());
    Json(PlatformResponse {
        platform: snapshot,
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        assessment,
        support_table_source: platform::SUPPORT_TABLE_SOURCE.to_string(),
        support_table_checked_on: platform::SUPPORT_TABLE_CHECKED_ON.to_string(),
    })
}
