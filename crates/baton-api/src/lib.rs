//! API：Axum 路由。
//!
//! 目前有健康狀態、平台狀態、Workspace 管理與授權端點、Goal／Task 與契約端點、成果接受與驗證端點、Run（派工與取消）端點、多帳號排程總覽與容量預設端點、成果（瀏覽、預覽、下載）端點、備份狀態端點、帳號 Profile 管理（含方案上限）與首次啟動精靈端點、共同設定（匯入、版本、Run 設定快照）端點、知識與教訓端點、Guard 與重犯事件端點、時間線與觀測指標端點、生命週期（完全退出、自動續跑、繼續工作）端點與事件串流；
//! 另有只給 Agent 憑證使用的 Agent API（`/agent/v1/tools/*`，票 11）。所有端點一律經過 `guard` 的身分驗證與網頁存取防護
//! （票 04）；管理 API 只接受桌面角色，Agent API 只接受 Agent 角色。

mod agent;
mod artifacts;
mod authorization;
mod backups;
mod controller;
mod conversations;
mod decisions;
mod external_operations;
mod guard;
mod guards;
mod knowledge;
mod lifecycle;
mod messages;
mod profiles;
pub mod quota;
mod recovery;
mod runs;
mod scheduling;
mod sessions;
mod setup;
mod shared_settings;
mod tasks;
mod timeline;
mod updates;
mod understanding;
mod verification;
pub mod workdirs;
mod workspaces;

use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::{get, post};
use axum::{middleware, Json, Router};
use baton_domain::lifecycle::ExitTiming;
use baton_domain::agent::{
    TOOL_CONFIRM_HANDOFF, TOOL_CONTROLLER_ACTION, TOOL_GET_TASK_CONTRACT, TOOL_PERMISSION_PROMPT, TOOL_READ_INBOX, TOOL_REPORT_BLOCKED,
    TOOL_REPORT_PROGRESS, TOOL_REQUEST_DECISION, TOOL_SAVE_CHECKPOINT, TOOL_SEND_MESSAGE, TOOL_ADD_DEPENDENCY,
    TOOL_CANCEL_TASK, TOOL_DISPATCH_TASK, TOOL_PROPOSE_DECISION_REQUEST, TOOL_PROPOSE_TASK, TOOL_UPDATE_TASK,
};
use baton_domain::artifact::TOOL_REGISTER_ARTIFACT;
use baton_domain::external_operation::{TOOL_BEGIN_EXTERNAL_OPERATION, TOOL_RECORD_EXTERNAL_OPERATION_RESULT};
use baton_domain::guardian::GuardianStatus;
use baton_domain::storage_health::StorageHealth;
use baton_domain::knowledge::{TOOL_RECORD_LESSON_CANDIDATE, TOOL_SEARCH_KNOWLEDGE};
use baton_domain::understanding::{TOOL_GET_PROJECT_UNDERSTANDING, TOOL_RECORD_UNDERSTANDING};
use baton_domain::platform::{self, PlatformAssessment, PlatformSnapshot};
use baton_domain::{product, Timestamp};
use baton_ports::{
    ClaudeEnvironment, Clock, GuardianProbe, PlatformInfo, QuotaObserver, StartupRegistration, UserSettingsSource,
    WorkspaceFs,
};
use baton_store::Store;
use futures::stream::{self, Stream, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::{watch, Notify};

pub use agent::{tool_path, ControllerActionResponse, DecisionRequested, PermissionResponse, ReportAck, TaskContractResponse};
pub use decisions::{
    decision_answer_path, decision_authority_path, decision_links_path, decision_merge_path, decision_path,
    decisions_path, AnswerRequest, BatchItem, BatchItemResult, BatchRequest, BatchResponse, LinkRequest, MergeRequest,
    MergeResponse, PendingDecisions, UpdateAuthorityRequest, DECISIONS_BATCH_PATH, PENDING_DECISIONS_PATH,
};
pub use artifacts::{
    artifact_content_path, artifact_preview_path, artifacts_path, ArtifactList, ArtifactPreview, ArtifactRegistered,
    ArtifactView, ArtifactVersionView, SHA256_HEADER,
};
pub use backups::{BackupStatus, BackupSummary, BACKUPS_PATH};
pub use conversations::{conversation_messages_path, conversation_path, PostMessageRequest, PostedMessage};
pub use external_operations::{external_operations_path, BeginResponse, ExternalOperationList, RecordResponse};
pub use messages::{DeliveryView, InboxResponse, SendAck};
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
pub use guards::{
    guard_action_path, recurrences_path, CreateGuardRequest, GuardLesson, GuardList, GuardListQuery, GuardRevisionRequest,
    GuardView, RecurrenceList, ReportRecurrenceRequest, GUARDS_PATH,
};
pub use knowledge::{
    knowledge_action_path, knowledge_entry_path, ConfirmKnowledgeRequest, CorrectKnowledgeRequest, CreateKnowledgeRequest,
    KnowledgeDetail, KnowledgeEntryView, KnowledgeList, KnowledgeListQuery, KnowledgeReasonRequest,
    LessonCandidateRecorded, ScopeInput, KNOWLEDGE_PATH,
};
pub use lifecycle::{
    workspace_lifecycle_path, workspace_login_resume_path, workspace_resume_path, LifecycleStatus, LoginStartupView,
    UpdateLoginResumeRequest, UpdateLoginStartupRequest, UpdateWorkspaceLifecycleRequest, WorkspaceLifecycleView,
    LIFECYCLE_EXIT_PATH, LIFECYCLE_PATH, LOGIN_STARTUP_PATH,
};
pub use recovery::{RecoveryStatusView, RECOVERY_PATH, RECOVERY_RETRY_PATH};
pub use updates::{
    ApplyUpdateRequest, AvailableUpdateView, MaintenanceRequestBody, UpdateControl, UpdatePrepareError, UpdateStatusView,
    MAINTENANCE_PATH, UPDATES_PATH, UPDATE_APPLY_PATH,
};
pub use profiles::{CreateProfileRequest, LoginTerminalResponse, ProfileList, PROFILES_PATH};
pub use setup::{BackgroundNotice, SetupStatus, SETUP_ACK_PATH, SETUP_PATH, SETUP_RECHECK_PATH};
pub use sessions::{session_path, sessions_path, SessionList};
pub use runs::{
    run_cancel_path, run_context_path, run_path, runs_path, task_dispatch_path, DispatchRequest, QueuedDispatch,
    RunContextView, RunList,
};
pub use scheduling::{
    AccountRun, AccountScheduling, CapacityModeInfo, CapacityView, QueueEntryView, SchedulingOverview,
    SchedulingSettingsView, UpdateSchedulingSettingsRequest, SCHEDULING_PATH, SCHEDULING_SETTINGS_PATH,
};
pub use timeline::{metrics_path, timeline_path, MetricsResponse, ServerQuota, TimelineQuery, TimelineResponse};
pub use understanding::{
    projection_reconcile_path, understanding_path, understanding_sync_path, ReconcileRequest, RecordUnderstandingResponse,
    SourceView, UnderstandingItemView, UnderstandingView,
};
pub use verification::{task_accept_path, task_verification_path, AcceptTaskRequest};
pub use shared_settings::{
    run_settings_snapshot_path, shared_settings_version_path, AdoptRequest, ImportPreview,
    RunSettings, RunSettingsSnapshot, SharedSettingsStatus, SnapshotFileInfo, SnapshotRequest, RUN_SETTINGS_DIR, SHARED_SETTINGS_ADOPT_PATH,
    SHARED_SETTINGS_PATH, SHARED_SETTINGS_PREVIEW_PATH,
};
pub use workdirs::INTEGRATION_QUEUE_PATH;
pub use workspaces::{CreateWorkspaceRequest, WorkspaceList, WORKSPACES_PATH};

/// 健康端點路徑。
pub const HEALTH_PATH: &str = "/api/v1/health";
/// 事件串流（SSE）路徑；與其他端點受到同樣的驗證。
pub const EVENTS_PATH: &str = "/api/v1/events";

/// 請求紀錄的輸出位置；內容不含憑證、查詢字串或實際 URL。
pub type LogSink = Arc<dyn Fn(&str) + Send + Sync>;

/// 要求一次重要裁定或 Stage 完成後的備份（票 40，spec D-18）；參數是原因，只用於日誌。
/// 由 Runtime 的備份排程實作，同一小時內的要求合併為一次。
pub type BackupRequestSink = Arc<dyn Fn(&str) + Send + Sync>;

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
    /// 控制資料庫是否可寫；磁碟滿或被鎖住時新派工停止，附原因與處理方式（票 26）。舊版 Runtime 沒有這個欄位。
    #[serde(default)]
    pub storage: StorageHealth,
    /// 監護程序的狀態（票 32）：不在時工作照常，只是沒有自動重啟保護。舊版 Runtime 沒有這個欄位。
    #[serde(default)]
    pub guardian: GuardianStatus,
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
    /// 外部額度觀測器，每個帳號每 5 分鐘輪詢一次（票 27）；觀測不到的帳號不阻擋派工。
    pub quota_observer: Arc<dyn QuotaObserver>,
    /// 使用者平常使用的 Claude Code 設定（唯讀；共同設定匯入的來源）。
    pub user_settings: Arc<dyn UserSettingsSource>,
    /// 監護程序是否還在（票 32）：健康查詢回報自動重啟保護是否降級。
    pub guardian_probe: Arc<dyn GuardianProbe>,
    /// 「登入時自動啟動」的 Windows 登記（票 33）。
    pub startup_registration: Arc<dyn StartupRegistration>,
    /// Run 設定快照；與 Supervisor 共用同一個實例，同一個 Run 的快照請求才會依序處理。
    pub run_settings: Arc<RunSettings>,
    pub log: LogSink,
    /// 派工或要求取消之後通知 Supervisor 儘快處理（背景迴圈等待它；沒有背景迴圈時沒有作用）。
    pub supervisor_wake: Arc<Notify>,
    /// 正式裁定之後要求備份。
    pub request_backup: BackupRequestSink,
    /// Runtime 開始停止時變成 `true`；長連線（串流）據此結束，優雅停止才不會被它們卡住。
    pub shutdown: watch::Receiver<bool>,
    /// 完全退出的時間參數（spec D-11）：停止意圖記錄的期限由它計算。
    pub exit_timing: ExitTiming,
    /// 停止意圖持久化之後通知 Runtime 開始停止 Run（票 25）。
    pub exit_signal: Arc<Notify>,
    /// 這個 Runtime 的版本（票 34；維護記錄從哪個版本開始）。
    pub app_version: String,
    /// 更新檢查與下載（票 34）。
    pub updates: Arc<dyn UpdateControl>,
    /// 專案理解投影檔的比對與寫入一次只做一個（票 42）。
    pub projection_lock: Arc<tokio::sync::Mutex<()>>,
}

pub fn router(state: ApiState) -> Router {
    let state = Arc::new(state);
    Router::new()
        .route(HEALTH_PATH, get(health))
        .route(PLATFORM_PATH, get(platform_status))
        .route(EVENTS_PATH, get(events))
        .route(WORKSPACES_PATH, get(workspaces::list).post(workspaces::create))
        .route(workspaces::ARCHIVE_ROUTE, post(workspaces::archive))
        .route(workspaces::MODEL_OVERRIDES_ROUTE, axum::routing::put(workspaces::set_model_overrides))
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
        .route(verification::VERIFICATION_ROUTE, get(verification::view))
        .route(verification::ACCEPT_ROUTE, post(verification::accept))
        .route(decisions::DECISIONS_ROUTE, get(decisions::list))
        .route(decisions::DECISION_ROUTE, get(decisions::detail))
        .route(decisions::ANSWER_ROUTE, post(decisions::answer))
        .route(decisions::MERGE_ROUTE, post(decisions::merge))
        .route(decisions::LINKS_ROUTE, post(decisions::link))
        .route(decisions::AUTHORITY_ROUTE, get(decisions::authority).put(decisions::update_authority))
        .route(PENDING_DECISIONS_PATH, get(decisions::pending))
        .route(DECISIONS_BATCH_PATH, post(decisions::batch))
        .route(PROFILES_PATH, get(profiles::list).post(profiles::create))
        .route(profiles::VERIFY_ROUTE, post(profiles::verify))
        .route(profiles::LOGIN_TERMINAL_ROUTE, post(profiles::login_terminal))
        .route(profiles::ARCHIVE_ROUTE, post(profiles::archive))
        .route(quota::PLAN_LIMITS_ROUTE, axum::routing::put(quota::set_plan_limits))
        .route(SETUP_PATH, get(setup::status))
        .route(SETUP_RECHECK_PATH, post(setup::recheck))
        .route(SETUP_ACK_PATH, post(setup::acknowledge_background_notice))
        .route(runs::DISPATCH_ROUTE, post(runs::dispatch))
        .route(runs::RUNS_ROUTE, get(runs::list))
        .route(runs::RUN_ROUTE, get(runs::detail))
        .route(runs::CANCEL_ROUTE, post(runs::cancel))
        .route(runs::CONTEXT_ROUTE, get(runs::context))
        .route(conversations::CONVERSATION_ROUTE, get(conversations::view))
        .route(conversations::MESSAGES_ROUTE, post(conversations::post_message))
        .route(sessions::SESSIONS_ROUTE, get(sessions::list))
        .route(sessions::SESSION_ROUTE, get(sessions::detail))
        .route(external_operations::EXTERNAL_OPERATIONS_ROUTE, get(external_operations::list))
        .route(timeline::TIMELINE_ROUTE, get(timeline::timeline))
        .route(timeline::METRICS_ROUTE, get(timeline::metrics))
        .route(understanding::UNDERSTANDING_ROUTE, get(understanding::view))
        .route(understanding::SYNC_ROUTE, post(understanding::sync))
        .route(understanding::RECONCILE_ROUTE, post(understanding::reconcile))
        .route(LIFECYCLE_PATH, get(lifecycle::status))
        .route(LIFECYCLE_EXIT_PATH, post(lifecycle::request_exit))
        .route(lifecycle::WORKSPACE_LIFECYCLE_ROUTE, axum::routing::put(lifecycle::update_workspace))
        .route(lifecycle::WORKSPACE_RESUME_ROUTE, post(lifecycle::resume_workspace))
        .route(LOGIN_STARTUP_PATH, axum::routing::put(lifecycle::update_login_startup))
        .route(lifecycle::WORKSPACE_LOGIN_RESUME_ROUTE, axum::routing::put(lifecycle::update_login_resume))
        .route(UPDATES_PATH, get(updates::status))
        .route(UPDATE_APPLY_PATH, post(updates::apply))
        .route(MAINTENANCE_PATH, post(updates::request))
        .route(RECOVERY_PATH, get(recovery::status))
        .route(RECOVERY_RETRY_PATH, post(recovery::retry))
        .route(SCHEDULING_PATH, get(scheduling::view))
        .route(INTEGRATION_QUEUE_PATH, get(workdirs::queue))
        .route(SCHEDULING_SETTINGS_PATH, axum::routing::put(scheduling::update_settings))
        .route(SHARED_SETTINGS_PATH, get(shared_settings::status))
        .route(SHARED_SETTINGS_PREVIEW_PATH, post(shared_settings::preview))
        .route(SHARED_SETTINGS_ADOPT_PATH, post(shared_settings::adopt))
        .route(shared_settings::VERSION_ROUTE, get(shared_settings::version))
        .route(shared_settings::SNAPSHOT_ROUTE, post(shared_settings::snapshot))
        .route(artifacts::ARTIFACTS_ROUTE, get(artifacts::list))
        .route(artifacts::PREVIEW_ROUTE, get(artifacts::preview))
        .route(artifacts::CONTENT_ROUTE, get(artifacts::content))
        .route(BACKUPS_PATH, get(backups::status))
        .route(KNOWLEDGE_PATH, get(knowledge::list).post(knowledge::create))
        .route(knowledge::ENTRY_ROUTE, get(knowledge::detail))
        .route(knowledge::CONFIRM_ROUTE, post(knowledge::confirm))
        .route(knowledge::CORRECT_ROUTE, post(knowledge::correct))
        .route(knowledge::REVOKE_ROUTE, post(knowledge::revoke))
        .route(knowledge::RETIRE_ROUTE, post(knowledge::retire))
        .route(GUARDS_PATH, get(guards::list).post(guards::create))
        .route(guards::ENABLE_ROUTE, post(guards::enable))
        .route(guards::DISABLE_ROUTE, post(guards::disable))
        .route(guards::RECURRENCES_ROUTE, get(guards::recurrences).post(guards::report))
        // Agent API：只接受 Agent 憑證（guard 依路徑前綴決定所需角色）
        .route(&tool_path(TOOL_GET_TASK_CONTRACT), post(agent::get_task_contract))
        .route(&tool_path(TOOL_REPORT_PROGRESS), post(agent::report_progress))
        .route(&tool_path(TOOL_SAVE_CHECKPOINT), post(agent::save_checkpoint))
        .route(&tool_path(TOOL_REPORT_BLOCKED), post(agent::report_blocked))
        .route(&tool_path(TOOL_REQUEST_DECISION), post(agent::request_decision))
        .route(&tool_path(TOOL_PERMISSION_PROMPT), post(agent::permission_prompt))
        .route(&tool_path(TOOL_CONTROLLER_ACTION), post(agent::controller_action))
        .route(&tool_path(TOOL_REGISTER_ARTIFACT), post(artifacts::register).layer(artifacts::register_body_limit()))
        .route(&tool_path(TOOL_SEND_MESSAGE), post(messages::send_message))
        .route(&tool_path(TOOL_READ_INBOX), post(messages::read_inbox))
        .route(&tool_path(TOOL_CONFIRM_HANDOFF), post(agent::confirm_handoff))
        .route(&tool_path(TOOL_RECORD_LESSON_CANDIDATE), post(knowledge::record_lesson_candidate))
        .route(&tool_path(TOOL_SEARCH_KNOWLEDGE), post(knowledge::search_knowledge))
        .route(&tool_path(TOOL_BEGIN_EXTERNAL_OPERATION), post(external_operations::begin))
        .route(&tool_path(TOOL_RECORD_EXTERNAL_OPERATION_RESULT), post(external_operations::record_result))
        // Controller 專屬的拆工與派工工具（票 17）：只接受 Controller 憑證
        .route(&tool_path(TOOL_PROPOSE_TASK), post(controller::propose_task))
        .route(&tool_path(TOOL_UPDATE_TASK), post(controller::update_task))
        .route(&tool_path(TOOL_ADD_DEPENDENCY), post(controller::add_dependency))
        .route(&tool_path(TOOL_DISPATCH_TASK), post(controller::dispatch_task))
        .route(&tool_path(TOOL_CANCEL_TASK), post(controller::cancel_task))
        .route(&tool_path(TOOL_PROPOSE_DECISION_REQUEST), post(controller::propose_decision_request))
        .route(&tool_path(TOOL_RECORD_UNDERSTANDING), post(understanding::record_understanding))
        .route(&tool_path(TOOL_GET_PROJECT_UNDERSTANDING), post(understanding::get_project_understanding))
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
        version: state.app_version.clone(),
        boot_count: state.boot_count,
        started_at: state.started_at,
        pid: std::process::id(),
        storage: state.store.storage_health(),
        guardian: GuardianStatus::of(state.guardian_probe.guardian_running()),
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
