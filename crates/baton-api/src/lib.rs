//! API：Axum 路由。
//!
//! 目前有健康狀態、平台狀態、Workspace 管理與授權端點、Goal／Task 與契約端點、Run（派工與取消）端點、帳號 Profile 管理與首次啟動精靈端點與事件串流；
//! 另有只給 Agent 憑證使用的 Agent API（`/agent/v1/tools/*`，票 11）。所有端點一律經過 `guard` 的身分驗證與網頁存取防護
//! （票 04）；管理 API 只接受桌面角色，Agent API 只接受 Agent 角色。

mod agent;
mod authorization;
mod decisions;
mod guard;
mod profiles;
mod setup;
mod runs;
mod tasks;
mod workspaces;

use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::{get, post};
use axum::{middleware, Json, Router};
use baton_domain::agent::{
    TOOL_GET_TASK_CONTRACT, TOOL_PERMISSION_PROMPT, TOOL_REPORT_BLOCKED, TOOL_REPORT_PROGRESS, TOOL_REQUEST_DECISION,
    TOOL_SAVE_CHECKPOINT,
};
use baton_domain::platform::{self, PlatformAssessment, PlatformSnapshot};
use baton_domain::{product, Timestamp};
use baton_ports::{ClaudeEnvironment, Clock, PlatformInfo, WorkspaceFs};
use baton_store::Store;
use futures::stream::{self, Stream, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::{watch, Notify};

pub use agent::{tool_path, DecisionRequested, PermissionResponse, ReportAck, TaskContractResponse};
pub use decisions::{
    decision_answer_path, decision_path, decisions_path, AnswerRequest, PendingDecisions, PENDING_DECISIONS_PATH,
};
pub use authorization::{
    admission_check_path, authorization_path, authorization_summary_path, AdmissionCheckRequest, AuthorizationStatus,
    AuthorizationView, CapabilityInfo, ConfirmAuthorizationRequest, SummaryRequest,
};
pub use tasks::{
    goals_path, task_admission_check_path, task_cancel_path, task_contract_path, task_dependencies_path, task_path,
    task_ready_path, tasks_path, AddDependencyRequest, AdmissionReport, CreateGoalRequest, CreateTaskRequest, TaskBoard,
    TaskDetail, UpdateContractRequest,
};
pub use guard::{Denial, CLIENT_HEADER, CLIENT_HEADER_VALUE};
pub use profiles::{CreateProfileRequest, LoginTerminalResponse, ProfileList, PROFILES_PATH};
pub use setup::{BackgroundNotice, SetupStatus, SETUP_ACK_PATH, SETUP_PATH, SETUP_RECHECK_PATH};
pub use runs::{run_cancel_path, run_path, runs_path, task_dispatch_path, RunList};
pub use workspaces::{CreateWorkspaceRequest, WorkspaceList, WORKSPACES_PATH};

/// 健康端點路徑。
pub const HEALTH_PATH: &str = "/api/v1/health";
/// 事件串流（SSE）路徑；與其他端點受到同樣的驗證。
pub const EVENTS_PATH: &str = "/api/v1/events";

/// 請求紀錄的輸出位置；內容不含憑證、查詢字串或實際 URL。
pub type LogSink = Arc<dyn Fn(&str) + Send + Sync>;

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

/// 請求被拒絕時的回應內容；Runtime 與用戶端共用。`error` 是穩定的機器可讀代碼。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorBody {
    pub error: String,
    pub message: String,
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
    /// Runtime 實際監聽的埠；Host 標頭必須帶這個埠。
    pub allowed_port: u16,
    pub store: Store,
    /// Runtime 的資料目錄；Profile 的原生設定目錄建立在其下。
    pub data_dir: PathBuf,
    pub clock: Arc<dyn Clock>,
    pub platform_info: Arc<dyn PlatformInfo>,
    pub workspace_fs: Arc<dyn WorkspaceFs>,
    pub claude_env: Arc<dyn ClaudeEnvironment>,
    pub log: LogSink,
    /// 派工或要求取消之後通知 Supervisor 儘快處理（背景迴圈等待它；沒有背景迴圈時沒有作用）。
    pub supervisor_wake: Arc<Notify>,
    /// Runtime 開始停止時變成 `true`；長連線（串流）據此結束，優雅停止才不會被它們卡住。
    pub shutdown: watch::Receiver<bool>,
}

pub fn router(state: ApiState) -> Router {
    let state = Arc::new(state);
    Router::new()
        .route(HEALTH_PATH, get(health))
        .route(PLATFORM_PATH, get(platform_status))
        .route(EVENTS_PATH, get(events))
        .route(WORKSPACES_PATH, get(workspaces::list).post(workspaces::create))
        .route(workspaces::ARCHIVE_ROUTE, post(workspaces::archive))
        .route(authorization::AUTHORIZATION_ROUTE, get(authorization::view).put(authorization::confirm))
        .route(authorization::SUMMARY_ROUTE, post(authorization::summary))
        .route(authorization::ADMISSION_ROUTE, post(authorization::admission_check))
        .route(tasks::GOALS_ROUTE, post(tasks::create_goal))
        .route(tasks::TASKS_ROUTE, get(tasks::board).post(tasks::create_task))
        .route(tasks::TASK_ROUTE, get(tasks::detail))
        .route(tasks::CONTRACT_ROUTE, axum::routing::put(tasks::update_contract))
        .route(tasks::DEPENDENCIES_ROUTE, post(tasks::add_dependency))
        .route(tasks::ADMISSION_ROUTE, post(tasks::admission_check))
        .route(tasks::READY_ROUTE, post(tasks::mark_ready))
        .route(tasks::CANCEL_ROUTE, post(tasks::cancel))
        .route(decisions::DECISIONS_ROUTE, get(decisions::list))
        .route(decisions::DECISION_ROUTE, get(decisions::detail))
        .route(decisions::ANSWER_ROUTE, post(decisions::answer))
        .route(PENDING_DECISIONS_PATH, get(decisions::pending))
        .route(PROFILES_PATH, get(profiles::list).post(profiles::create))
        .route(profiles::VERIFY_ROUTE, post(profiles::verify))
        .route(profiles::LOGIN_TERMINAL_ROUTE, post(profiles::login_terminal))
        .route(profiles::ARCHIVE_ROUTE, post(profiles::archive))
        .route(SETUP_PATH, get(setup::status))
        .route(SETUP_RECHECK_PATH, post(setup::recheck))
        .route(SETUP_ACK_PATH, post(setup::acknowledge_background_notice))
        .route(runs::DISPATCH_ROUTE, post(runs::dispatch))
        .route(runs::RUNS_ROUTE, get(runs::list))
        .route(runs::RUN_ROUTE, get(runs::detail))
        .route(runs::CANCEL_ROUTE, post(runs::cancel))
        // Agent API：只接受 Agent 憑證（guard 依路徑前綴決定所需角色）
        .route(&tool_path(TOOL_GET_TASK_CONTRACT), post(agent::get_task_contract))
        .route(&tool_path(TOOL_REPORT_PROGRESS), post(agent::report_progress))
        .route(&tool_path(TOOL_SAVE_CHECKPOINT), post(agent::save_checkpoint))
        .route(&tool_path(TOOL_REPORT_BLOCKED), post(agent::report_blocked))
        .route(&tool_path(TOOL_REQUEST_DECISION), post(agent::request_decision))
        .route(&tool_path(TOOL_PERMISSION_PROMPT), post(agent::permission_prompt))
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
