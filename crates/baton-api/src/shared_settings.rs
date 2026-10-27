//! 共同設定端點（桌面角色專用）：預覽匯入、採用成版本、列出與讀取版本、為 Run 產生唯讀設定快照。
//!
//! 規則都在 Domain（`baton_domain::shared_settings`）；端點只做「經 Port 唯讀取得來源檔 → 交給 Domain →
//! 交給 Store 或 WorkspaceFs」。預覽不寫入任何東西；採用必須帶預覽時的內容雜湊，來源在預覽後改變就拒絕，
//! 所以使用者採用的一定是他看過的內容。使用者的原始設定檔只被讀取，從不被修改。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use axum::Json;
use baton_domain::shared_settings::{
    self, DiffEntry, Exclusion, ImportCandidate, ImportError, RunSettingsPin, SettingsBundle, SettingsVersion,
    SettingsVersionSummary, SourceReport,
};
use baton_ports::{Clock, WorkspaceFs};
use baton_store::Store;
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
    let (snapshot, created) = state.run_settings.materialize(&run_id, request.version, admitted).await?;
    Ok((if created { StatusCode::CREATED } else { StatusCode::OK }, Json(snapshot)))
}

/// 產生並核對 Run 專屬的設定快照（D-09）。API 端點與 Supervisor 的啟動流程共用同一個實例：
/// 同一個 Run 的請求依序處理（不會兩個請求同時寫同一個目錄），已綁定的快照每次都依版本內容與
/// `manifest.json` 核對目錄，不一致就拒絕，不宣稱設定已固定。
pub struct RunSettings {
    store: Store,
    workspace_fs: Arc<dyn WorkspaceFs>,
    data_dir: PathBuf,
    clock: Arc<dyn Clock>,
    /// 每個 Run 一把鎖；Run 數量有限，鎖用完不回收。
    locks: std::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl RunSettings {
    pub fn new(store: Store, workspace_fs: Arc<dyn WorkspaceFs>, data_dir: PathBuf, clock: Arc<dyn Clock>) -> Self {
        Self { store, workspace_fs, data_dir, clock, locks: std::sync::Mutex::new(HashMap::new()) }
    }

    fn lock_of(&self, run_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self.locks.lock().unwrap_or_else(|e| e.into_inner());
        locks.entry(run_id.to_string()).or_default().clone()
    }

    /// Run 啟動前（准入之後）呼叫：使用准入時固定的版本 `admitted`（票 24）；已綁定就核對後沿用。
    /// 准入時還沒有採用任何版本時回傳 `None`（此時 Run 不帶共同設定啟動，之後採用的版本不會套用到它）。
    /// 錯誤訊息可直接作為 Run 的失敗原因。
    pub async fn for_launch(&self, run_id: &str, admitted: Option<i64>) -> Result<Option<RunSettingsSnapshot>, String> {
        let Some(version) = admitted else { return Ok(None) };
        self.materialize(run_id, Some(version), Some(admitted))
            .await
            .map(|(snapshot, _)| Some(snapshot))
            .map_err(|error| format!("{}：{}", error.code, error.message))
    }

    /// 取得指定（或最新）版本的快照並寫成 Run 專屬唯讀目錄；回傳快照與是否為這次新建立。
    /// `admitted` 是已准入 Run 准入時固定的版本（`Some(None)` 代表准入時還沒有採用任何版本）；尚未准入為 `None`。
    pub(crate) async fn materialize(
        &self,
        run_id: &str,
        requested: Option<i64>,
        admitted: Option<Option<i64>>,
    ) -> Result<(RunSettingsSnapshot, bool), ApiError> {
        let lock = self.lock_of(run_id);
        let _serial = lock.lock().await;
        if let Some(pin) = self.store.run_settings_pin(run_id).await? {
            if requested.is_some_and(|v| v != pin.version) {
                return Err(api_error(
                    StatusCode::CONFLICT,
                    "settings_snapshot_pinned",
                    format!("Run {run_id} 已固定使用共同設定版本 {}，不能改用其他版本", pin.version),
                ));
            }
            let version = self.pinned_version(&pin).await?;
            self.verify(&pin, &version).await?;
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
            Some(number) => self.store.settings_version(number).await?.ok_or_else(|| version_not_found(number))?,
            None => self.store.latest_settings_version().await?.ok_or_else(|| {
                api_error(StatusCode::CONFLICT, "shared_settings_not_adopted", "還沒有採用任何共同設定版本，無法產生 Run 的設定快照")
            })?,
        };
        let directory = self.data_dir.join(RUN_SETTINGS_DIR).join(run_id);
        self.write(&directory, &version).await?;
        let pin = RunSettingsPin {
            run_id: run_id.to_string(),
            version: version.version,
            content_hash: version.content_hash.clone(),
            directory: directory.to_string_lossy().into_owned(),
            created_at: self.clock.now(),
        };
        let pin = self.store.pin_run_settings(pin).await?;
        Ok((describe(&pin, &version), true))
    }

    async fn pinned_version(&self, pin: &RunSettingsPin) -> Result<SettingsVersion, ApiError> {
        self.store.settings_version(pin.version).await?.ok_or_else(|| version_not_found(pin.version))
    }

    async fn write(&self, directory: &Path, version: &SettingsVersion) -> Result<(), ApiError> {
        let files: Vec<(String, Vec<u8>)> =
            shared_settings::snapshot_files(version).into_iter().map(|f| (f.name, f.bytes)).collect();
        self.workspace_fs.write_readonly_files(directory, &files).await.map_err(|error| {
            api_error(StatusCode::INTERNAL_SERVER_ERROR, "settings_snapshot_failed", format!("無法寫入設定快照目錄 {}：{error}", directory.display()))
        })
    }

    /// 目錄中的檔案必須與版本渲染出的檔案完全相同（不多不少、位元組一致），
    /// 而且 `manifest.json` 記錄的版本與內容雜湊就是綁定的那一個。
    async fn verify(&self, pin: &RunSettingsPin, version: &SettingsVersion) -> Result<(), ApiError> {
        let directory = Path::new(&pin.directory);
        let mismatch = |detail: String| {
            api_error(
                StatusCode::CONFLICT,
                "settings_snapshot_mismatch",
                format!("Run {} 的設定快照目錄 {} 與綁定的版本 {} 不一致（{detail}），不以這份設定啟動", pin.run_id, pin.directory, pin.version),
            )
        };
        let read_failed = |error: baton_ports::PortError| {
            api_error(StatusCode::INTERNAL_SERVER_ERROR, "settings_snapshot_failed", format!("無法讀取設定快照目錄 {}：{error}", pin.directory))
        };
        let expected = shared_settings::snapshot_files(version);
        let listed = self.workspace_fs.list_artifact_files(directory).await.map_err(read_failed)?;
        let mut names: Vec<String> = listed
            .iter()
            .map(|entry| entry.path.strip_prefix(directory).unwrap_or(&entry.path).to_string_lossy().replace('\\', "/"))
            .collect();
        names.sort();
        let mut expected_names: Vec<String> = expected.iter().map(|f| f.name.clone()).collect();
        expected_names.sort();
        if names != expected_names {
            return Err(mismatch(format!("檔案為 {names:?}，應為 {expected_names:?}")));
        }
        for file in &expected {
            let actual = self.workspace_fs.read_artifact(&directory.join(&file.name)).await.map_err(read_failed)?;
            if actual.as_deref() != Some(file.bytes.as_slice()) {
                return Err(mismatch(format!("{} 的內容被改變", file.name)));
            }
        }
        // 檔案與版本一致之後，manifest 的版本與雜湊也必須就是綁定的那一個
        if version.content_hash != pin.content_hash {
            return Err(mismatch("版本的內容雜湊與綁定時不同".into()));
        }
        Ok(())
    }
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
