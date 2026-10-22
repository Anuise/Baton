//! 更新與維護端點（桌面角色專用，票 34；spec D-19）。
//!
//! - `GET /api/v1/updates`：更新檢查的結果（只通知）與進行中、最近一次的維護。
//! - `POST /api/v1/updates/apply`：使用者同意更新。先下載並驗證簽章（不通過就什麼都不停），
//!   再開始維護：持久化維護與停止意圖、寫入帶維護抑制的生命週期檔、通知 Runtime 執行與完全退出相同的停止流程，
//!   之後備份並進入「準備套用」，由桌面殼執行安裝程式。
//! - `POST /api/v1/maintenance`：安裝程式（解除安裝、或不經 App 的更新安裝）要求維護；不下載任何東西。
//!
//! 規則都在 Domain（`baton_domain::maintenance`）。

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use baton_domain::maintenance::{
    Maintenance, MaintenanceKind, MaintenanceOutcome, MaintenanceRequest, UpdateCheck, UpdateCheckStatus,
};
use baton_domain::Timestamp;
use baton_store::MaintenanceApplyError;
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};

use crate::authorization::bad_json;
use crate::workspaces::ApiError;
use crate::ApiState;

/// 更新狀態的路徑（GET）。
pub const UPDATES_PATH: &str = "/api/v1/updates";
/// 同意更新的路徑（POST）。
pub const UPDATE_APPLY_PATH: &str = "/api/v1/updates/apply";
/// 安裝程式要求維護的路徑（POST）。
pub const MAINTENANCE_PATH: &str = "/api/v1/maintenance";

/// 下載安裝程式的失敗。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdatePrepareError {
    /// 要求的版本不是目前檢查到的可更新版本（例如畫面上的資訊已過時）。
    NotAvailable,
    /// 這個版本沒有設定更新來源。
    Unconfigured,
    /// 下載失敗或簽章驗證失敗（附原因）。
    Download(String),
}

/// 更新檢查與下載（由 Runtime 實作；排程在 Runtime 的背景迴圈）。
pub trait UpdateControl: Send + Sync {
    /// 目前的檢查結果。
    fn status(&self) -> UpdateCheck;

    /// 下載 `version` 的安裝程式並驗證簽章，回傳檔案路徑。
    fn prepare<'a>(&'a self, version: &'a str) -> BoxFuture<'a, Result<PathBuf, UpdatePrepareError>>;
}

/// 可更新的版本（只給使用者看的部分）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AvailableUpdateView {
    pub version: String,
    pub notes: Option<String>,
    pub published_at: Option<String>,
}

/// 更新狀態：檢查結果（只通知）與維護。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateStatusView {
    pub current_version: String,
    pub status: UpdateCheckStatus,
    pub available: Option<AvailableUpdateView>,
    pub checked_at: Option<Timestamp>,
    pub next_check_at: Option<Timestamp>,
    pub error: Option<String>,
    /// 進行中的維護（更新或解除安裝）；`phase` 是 `ready` 時桌面殼可以執行 `package_path` 的安裝程式。
    pub maintenance: Option<Maintenance>,
    /// 最近一次結束的維護（例如「已更新到 0.2.0」）。
    pub last_maintenance: Option<MaintenanceOutcome>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplyUpdateRequest {
    /// 使用者看到並同意的版本。
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MaintenanceRequestBody {
    pub kind: MaintenanceKind,
}

impl From<MaintenanceApplyError> for ApiError {
    fn from(error: MaintenanceApplyError) -> Self {
        match error {
            MaintenanceApplyError::Rejected(rejected) => {
                ApiError { status: StatusCode::CONFLICT, code: rejected.code(), message: rejected.to_string() }
            }
            MaintenanceApplyError::Store(store) => store.into(),
        }
    }
}

async fn status_of(state: &ApiState) -> Result<UpdateStatusView, ApiError> {
    let check = state.updates.status();
    let record = state.store.maintenance().await?;
    Ok(UpdateStatusView {
        current_version: check.current_version,
        status: check.status,
        available: check.available.map(|r| AvailableUpdateView {
            version: r.version,
            notes: r.notes,
            published_at: r.published_at,
        }),
        checked_at: check.checked_at,
        next_check_at: check.next_check_at,
        error: check.error,
        maintenance: record.current,
        last_maintenance: record.last,
    })
}

pub(crate) async fn status(State(state): State<Arc<ApiState>>) -> Result<Json<UpdateStatusView>, ApiError> {
    Ok(Json(status_of(&state).await?))
}

/// 使用者同意更新：先下載並驗證簽章，通過才開始維護。下載失敗時不停止任何工作。
pub(crate) async fn apply(
    State(state): State<Arc<ApiState>>,
    body: Result<Json<ApplyUpdateRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let Json(request) = body.map_err(bad_json)?;
    // 下載可能要一段時間：已經在維護或完全退出時先拒絕，不浪費下載
    reject_if_stopping(&state, MaintenanceKind::Update).await?;
    let package = state.updates.prepare(&request.version).await.map_err(|error| match error {
        UpdatePrepareError::NotAvailable => ApiError {
            status: StatusCode::CONFLICT,
            code: "update_not_available",
            message: format!("{} 不是目前可更新的版本，請重新檢查更新", request.version),
        },
        UpdatePrepareError::Unconfigured => ApiError {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "updates_unconfigured",
            message: "這個版本沒有設定更新來源".to_string(),
        },
        UpdatePrepareError::Download(reason) => {
            (state.log)(&format!("更新：無法下載或驗證 {} 的安裝程式：{reason}", request.version));
            ApiError {
                status: StatusCode::BAD_GATEWAY,
                code: "update_download_failed",
                message: format!("無法下載或驗證安裝程式（工作沒有停止）：{reason}"),
            }
        }
    })?;
    let maintenance = MaintenanceRequest::update(
        &state.app_version,
        Some(request.version.clone()),
        Some(package.display().to_string()),
    );
    start(&state, maintenance).await
}

/// 安裝程式要求維護（解除安裝，或不經 App 的更新安裝）。
pub(crate) async fn request(
    State(state): State<Arc<ApiState>>,
    body: Result<Json<MaintenanceRequestBody>, JsonRejection>,
) -> Result<Response, ApiError> {
    let Json(request) = body.map_err(bad_json)?;
    let maintenance = match request.kind {
        MaintenanceKind::Update => MaintenanceRequest::update(&state.app_version, None, None),
        MaintenanceKind::Uninstall => MaintenanceRequest::uninstall(&state.app_version),
    };
    start(&state, maintenance).await
}

async fn reject_if_stopping(state: &ApiState, kind: MaintenanceKind) -> Result<(), ApiError> {
    let record = state.store.maintenance().await?;
    let lifecycle = state.store.lifecycle_state().await?;
    let rejected = match &record.current {
        Some(current) if current.kind != kind => Some(baton_domain::maintenance::MaintenanceError::InProgress(current.kind)),
        Some(_) => None,
        None if lifecycle.stop_intent_active() => Some(baton_domain::maintenance::MaintenanceError::StopIntentActive),
        None => None,
    };
    match rejected {
        Some(rejected) => Err(MaintenanceApplyError::Rejected(rejected).into()),
        None => Ok(()),
    }
}

/// 持久化維護與停止意圖（同一個交易）→ 寫入帶維護抑制的生命週期檔 → 通知 Runtime 開始停止流程。回應 202。
async fn start(state: &ApiState, request: MaintenanceRequest) -> Result<Response, ApiError> {
    let kind = request.kind;
    state.store.begin_maintenance(request, state.exit_timing, state.clock.now()).await?;
    (state.log)(&format!("維護：已持久化{}的維護抑制與停止意圖", kind_label(kind)));
    if !crate::lifecycle::mirror_lifecycle_file(state).await {
        (state.log)("維護：無法寫入生命週期檔，交給停止流程再試");
    }
    state.exit_signal.notify_one();
    Ok((StatusCode::ACCEPTED, Json(status_of(state).await?)).into_response())
}

fn kind_label(kind: MaintenanceKind) -> &'static str {
    match kind {
        MaintenanceKind::Update => "更新",
        MaintenanceKind::Uninstall => "解除安裝",
    }
}
