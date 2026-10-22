//! API：Axum 路由。
//!
//! 目前有健康狀態端點與事件串流；所有端點一律經過 `guard` 的身分驗證與網頁存取防護
//! （票 04）。管理 API 與 Agent API 的分流由後續的票加入。

mod guard;

use std::convert::Infallible;
use std::sync::Arc;

use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::get;
use axum::{middleware, Json, Router};
use baton_domain::{product, Timestamp};
use baton_ports::Clock;
use baton_store::Store;
use futures::stream::{self, Stream, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

pub use guard::{Denial, CLIENT_HEADER, CLIENT_HEADER_VALUE};

/// 健康端點路徑。
pub const HEALTH_PATH: &str = "/api/v1/health";
/// 事件串流（SSE）路徑；與其他端點受到同樣的驗證。
pub const EVENTS_PATH: &str = "/api/v1/events";

/// 請求紀錄的輸出位置；內容不含憑證、查詢字串或實際 URL。
pub type LogSink = Arc<dyn Fn(&str) + Send + Sync>;

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
    /// Runtime 實際監聽的埠；Host 標頭必須帶這個埠。
    pub allowed_port: u16,
    pub store: Store,
    pub clock: Arc<dyn Clock>,
    pub log: LogSink,
    /// Runtime 開始停止時變成 `true`；長連線（串流）據此結束，優雅停止才不會被它們卡住。
    pub shutdown: watch::Receiver<bool>,
}

pub fn router(state: ApiState) -> Router {
    let state = Arc::new(state);
    Router::new()
        .route(HEALTH_PATH, get(health))
        .route(EVENTS_PATH, get(events))
        // 套在 Router 上，所以也涵蓋未知路徑與不支援的方法
        .layer(middleware::from_fn_with_state(state.clone(), guard::guard))
        .with_state(state)
}

/// 事件串流：連線後先送出 `ready`，之後只有保活訊息；實際事件由後續的票加入。
async fn events(State(state): State<Arc<ApiState>>) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let mut shutdown = state.shutdown.clone();
    let ready = stream::once(async { Ok(Event::default().event("ready").data("ok")) });
    let until_shutdown = async move {
        let _ = shutdown.wait_for(|stopping| *stopping).await;
    };
    Sse::new(ready.chain(stream::pending()).take_until(until_shutdown)).keep_alive(KeepAlive::default())
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
