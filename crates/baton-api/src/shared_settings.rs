//! 共同設定端點（桌面角色專用）：預覽匯入、採用成版本、列出與讀取版本、為 Run 產生唯讀設定快照。
//!
//! 規則都在 Domain（`baton_domain::shared_settings`）；端點只做「經 Port 唯讀取得來源檔 → 交給 Domain →
//! 交給 Store 或 WorkspaceFs」。預覽不寫入任何東西；採用必須帶預覽時的內容雜湊，來源在預覽後改變就拒絕，
//! 所以使用者採用的一定是他看過的內容。使用者的原始設定檔只被讀取，從不被修改。

use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use axum::Json;
use baton_domain::shared_settings::{
    self, DiffEntry, Exclusion, ImportCandidate, ImportError, RunSettingsPin, SettingsBundle, SettingsVersion,
    SettingsVersionSummary, SourceReport,
};
use serde::{Deserialize, Serialize};

use crate::authorization::bad_json;
use crate::tasks::TaskApiError;
use crate::workspaces::ApiError;
use crate::ApiState;

/// 狀態端點：GET 列出已採用的版本。
pub const SHARED_SETTINGS_PATH: &str = "/api/v1/shared-settings";
/// 預覽匯入（POST，不保存任何東西）。
pub const SHARED_SETTINGS_PREVIEW_PATH: &str = "/api/v1/shared-settings/import-preview";
/// 採用目前的匯入結果成為新版本（POST）。
pub const SHARED_SETTINGS_ADOPT_PATH: &str = "/api/v1/shared-settings/adopt";
pub(crate) const VERSION_ROUTE: &str = "/api/v1/shared-settings/versions/{version}";
pub(crate) const SNAPSHOT_ROUTE: &str = "/api/v1/workspaces/{id}/runs/{run_id}/settings-snapshot";

/// Run 專屬設定快照放在資料目錄下的子目錄（spec D-17）。
pub const RUN_SETTINGS_DIR: &str = "run-settings";

pub fn shared_settings_version_path(version: i64) -> String {
    format!("{SHARED_SETTINGS_PATH}/versions/{version}")
}

pub fn run_settings_snapshot_path(workspace_id: &str, run_id: &str) -> String {
    format!("{}/settings-snapshot", crate::run_path(workspace_id, run_id))
}

/// 共同設定的目前狀態：已採用的版本（由舊到新）與最新版本。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedSettingsStatus {
    pub current: Option<SettingsVersionSummary>,
    pub versions: Vec<SettingsVersionSummary>,
}

/// 預覽：來源、排除項目、候選內容與相對於目前版本的差異。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportPreview {
    /// 候選內容的雜湊；採用時要帶回來。
    pub candidate_hash: String,
    pub sources: Vec<SourceReport>,
    /// 沒有被匯入的項目與原因（不含值）。
    pub exclusions: Vec<Exclusion>,
    pub bundle: SettingsBundle,
    /// 比較的基準版本（目前最新版本）；還沒有版本為 `None`。
    pub base_version: Option<i64>,
    pub diff: Vec<DiffEntry>,
    /// 候選內容與目前版本不同（還沒有版本時一定是 `true`）。
    pub changed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdoptRequest {
    /// 預覽回傳的 `candidate_hash`；來源在預覽後改變會被拒絕。
    pub expected_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SnapshotRequest {
    /// 要使用的版本；省略時使用最新版本。Run 已有快照時必須省略或相同。
    #[serde(default)]
    pub version: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotFileInfo {
    pub name: String,
    pub sha256: String,
}

/// Run 專屬唯讀設定快照。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunSettingsSnapshot {
    pub run_id: String,
    pub version: i64,
    pub content_hash: String,
    /// 唯讀快照目錄；Run 啟動時以設定參數指向這裡的檔案。
    pub directory: String,
    pub files: Vec<SnapshotFileInfo>,
}

fn api_error(status: StatusCode, code: &'static str, message: impl Into<String>) -> ApiError {
    ApiError { status, code, message: message.into() }
}

impl From<ImportError> for ApiError {
    fn from(error: ImportError) -> Self {
        api_error(StatusCode::UNPROCESSABLE_ENTITY, error.code(), format!("無法匯入設定：{error}"))
    }
}

/// 經 Port 唯讀取得來源檔並交給 Domain 建立候選內容。
async fn read_candidate(state: &ApiState) -> Result<ImportCandidate, ApiError> {
    let files = state.user_settings.read().await.map_err(|error| {
        api_error(StatusCode::INTERNAL_SERVER_ERROR, "settings_source_unreadable", format!("無法讀取使用者的設定檔：{error}"))
    })?;
    Ok(shared_settings::import(&files)?)
}

pub(crate) async fn status(State(state): State<Arc<ApiState>>) -> Result<Json<SharedSettingsStatus>, ApiError> {
    let versions: Vec<SettingsVersionSummary> =
        state.store.settings_versions().await?.iter().map(SettingsVersion::summary).collect();
    Ok(Json(SharedSettingsStatus { current: versions.last().cloned(), versions }))
}

pub(crate) async fn preview(State(state): State<Arc<ApiState>>) -> Result<Json<ImportPreview>, ApiError> {
    let candidate = read_candidate(&state).await?;
    let current = state.store.latest_settings_version().await?;
    let diff = shared_settings::diff(current.as_ref().map(|v| &v.bundle), &candidate.bundle);
    let changed = current.as_ref().is_none_or(|v| v.content_hash != candidate.content_hash);
    Ok(Json(ImportPreview {
        candidate_hash: candidate.content_hash,
        sources: candidate.sources,
        exclusions: candidate.exclusions,
        bundle: candidate.bundle,
        base_version: current.map(|v| v.version),
        diff,
        changed,
    }))
}

/// 採用：重新讀取來源並確認與預覽時相同，才建立新版本（每次採用都建立新版本）。
pub(crate) async fn adopt(
    State(state): State<Arc<ApiState>>,
    body: Result<Json<AdoptRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<SettingsVersion>), ApiError> {
    let Json(request) = body.map_err(bad_json)?;
    let candidate = read_candidate(&state).await?;
    if candidate.content_hash != request.expected_hash {
        return Err(api_error(
            StatusCode::CONFLICT,
            "settings_source_changed",
            "來源設定在預覽之後已經改變；請重新預覽，確認差異後再採用",
        ));
    }
    let version = state.store.adopt_settings(candidate, state.clock.now()).await?;
    Ok((StatusCode::CREATED, Json(version)))
}

pub(crate) async fn version(
    State(state): State<Arc<ApiState>>,
    UrlPath(version): UrlPath<i64>,
) -> Result<Json<SettingsVersion>, ApiError> {
    state.store.settings_version(version).await?.map(Json).ok_or_else(|| version_not_found(version))
}

fn version_not_found(version: i64) -> ApiError {
    api_error(StatusCode::NOT_FOUND, "settings_version_not_found", format!("找不到共同設定版本 {version}"))
}

/// 為 Run 產生唯讀設定快照：第一次呼叫把指定（或最新）版本寫成 Run 專屬的唯讀目錄並綁定；
/// 之後再呼叫回傳同一個綁定（201 → 200），後來採用的新版本不影響已綁定的 Run。
/// 已准入的 Run 一律使用准入時固定的版本（票 24）：省略版本就用它，指定別的版本回 409。
pub(crate) async fn snapshot(
    State(state): State<Arc<ApiState>>,
    UrlPath((workspace_id, run_id)): UrlPath<(String, String)>,
    body: Result<Json<SnapshotRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<RunSettingsSnapshot>), TaskApiError> {
    let Json(request) = body?;
    let run = state.store.run_detail(&workspace_id, &run_id).await?.run;
    let admitted = run.admitted_at.map(|_| run.settings_version);
    let (snapshot, created) = materialize_run_settings(&state, &run_id, request.version, admitted).await?;
    Ok((if created { StatusCode::CREATED } else { StatusCode::OK }, Json(snapshot)))
}

/// 供 Run 准入使用：取得設定版本快照並寫成 Run 專屬唯讀目錄；回傳快照與是否為這次新建立。
/// `admitted` 是已准入 Run 准入時固定的版本（`Some(None)` 代表准入時還沒有採用任何版本）；尚未准入為 `None`。
pub(crate) async fn materialize_run_settings(
    state: &ApiState,
    run_id: &str,
    requested: Option<i64>,
    admitted: Option<Option<i64>>,
) -> Result<(RunSettingsSnapshot, bool), ApiError> {
    if let Some(pin) = state.store.run_settings_pin(run_id).await? {
        if requested.is_some_and(|v| v != pin.version) {
            return Err(api_error(
                StatusCode::CONFLICT,
                "settings_snapshot_pinned",
                format!("Run {run_id} 已固定使用共同設定版本 {}，不能改用其他版本", pin.version),
            ));
        }
        let version = pinned_version(state, &pin).await?;
        return Ok((describe(&pin, &version), false));
    }

    // 已准入的 Run 沿用准入時固定的版本，後來採用的新版本不影響它（R1-a）
    let requested = match admitted {
        Some(Some(fixed)) if requested.is_some_and(|v| v != fixed) => {
            return Err(api_error(
                StatusCode::CONFLICT,
                "settings_snapshot_pinned",
                format!("Run {run_id} 准入時已固定使用共同設定版本 {fixed}，不能改用其他版本"),
            ));
        }
        Some(Some(fixed)) => Some(fixed),
        Some(None) => {
            return Err(api_error(
                StatusCode::CONFLICT,
                "shared_settings_not_adopted",
                format!("Run {run_id} 准入時還沒有採用任何共同設定版本，之後採用的版本不會套用到它"),
            ));
        }
        None => requested,
    };
    let version = match requested {
        Some(number) => state.store.settings_version(number).await?.ok_or_else(|| version_not_found(number))?,
        None => state.store.latest_settings_version().await?.ok_or_else(|| {
            api_error(StatusCode::CONFLICT, "shared_settings_not_adopted", "還沒有採用任何共同設定版本，無法產生 Run 的設定快照")
        })?,
    };
    let directory = state.data_dir.join(RUN_SETTINGS_DIR).join(run_id);
    write_snapshot(state, &directory, &version).await?;
    let pin = RunSettingsPin {
        run_id: run_id.to_string(),
        version: version.version,
        content_hash: version.content_hash.clone(),
        directory: directory.to_string_lossy().into_owned(),
        created_at: state.clock.now(),
    };
    let pin = state.store.pin_run_settings(pin).await?;
    if pin.version != version.version {
        // 同一個 Run 同時有另一個請求搶先綁定了別的版本：目錄改寫成已綁定的版本
        let pinned = pinned_version(state, &pin).await?;
        write_snapshot(state, &directory, &pinned).await?;
        return Ok((describe(&pin, &pinned), false));
    }
    Ok((describe(&pin, &version), true))
}

async fn pinned_version(state: &ApiState, pin: &RunSettingsPin) -> Result<SettingsVersion, ApiError> {
    state.store.settings_version(pin.version).await?.ok_or_else(|| version_not_found(pin.version))
}

async fn write_snapshot(
    state: &ApiState,
    directory: &std::path::Path,
    version: &SettingsVersion,
) -> Result<(), ApiError> {
    let files: Vec<(String, Vec<u8>)> =
        shared_settings::snapshot_files(version).into_iter().map(|f| (f.name, f.bytes)).collect();
    state.workspace_fs.write_readonly_files(directory, &files).await.map_err(|error| {
        api_error(StatusCode::INTERNAL_SERVER_ERROR, "settings_snapshot_failed", format!("無法寫入設定快照目錄 {}：{error}", directory.display()))
    })
}

fn describe(pin: &RunSettingsPin, version: &SettingsVersion) -> RunSettingsSnapshot {
    RunSettingsSnapshot {
        run_id: pin.run_id.clone(),
        version: pin.version,
        content_hash: pin.content_hash.clone(),
        directory: pin.directory.clone(),
        files: shared_settings::snapshot_files(version)
            .into_iter()
            .map(|f| SnapshotFileInfo { sha256: shared_settings::sha256_hex(&f.bytes), name: f.name })
            .collect(),
    }
}
