//! 成果端點（票 12，spec D-17）：Worker 登記成果（Agent API），以及桌面瀏覽、預覽與下載（管理 API）。
//!
//! 登記的順序是「驗證 → 確認仍是寫入所有者 → 暫存、fsync、原子改名 → 提交 manifest」：
//! manifest 只在檔案完整落地之後才存在，所以已登記的成果內容一定完整；中途中斷只會留下未登記的檔案。
//! 內容以參數內嵌傳入，Runtime 不讀取 Worker 指定的任何路徑。
//!
//! 下載與預覽都在管理 API 之下，由 `guard` 要求桌面憑證；成果一律以 Workspace 為界。
//! 讀出內容時重新核對大小與 SHA-256，與 manifest 不符就拒絕，不把不可信的內容交出去。

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::{DefaultBodyLimit, Path as UrlPath, State};
use axum::http::{header, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use base64::Engine;
use baton_domain::artifact::{
    self, Artifact, ArtifactDraft, ArtifactError, ArtifactVersion, FileStatus, Preview, StoredFile, UnregisteredFile,
    ARTIFACTS_DIR,
};
use baton_domain::auth::CredentialRecord;
use baton_domain::run::RunError;
use baton_domain::Timestamp;
use baton_store::{NewArtifactVersion, RunApplyError};
use serde::{Deserialize, Serialize};

use crate::agent::{scope_of, Claims};
use crate::authorization::bad_json;
use crate::workspaces::{new_id, ApiError};
use crate::ApiState;

pub(crate) const ARTIFACTS_ROUTE: &str = "/api/v1/workspaces/{id}/artifacts";
pub(crate) const PREVIEW_ROUTE: &str = "/api/v1/workspaces/{id}/artifacts/{artifact_id}/versions/{version}/preview";
pub(crate) const CONTENT_ROUTE: &str = "/api/v1/workspaces/{id}/artifacts/{artifact_id}/versions/{version}/content";

/// 登記請求本文的上限：內容最多 10 MiB，但 Base64 與 JSON 跳脫會讓本文變大，留足餘裕；超過內容上限的由 Domain 回 413 `artifact_too_large`。
pub(crate) const REGISTER_BODY_LIMIT: usize = 32 * 1024 * 1024;

/// 路徑；Runtime、用戶端與測試共用。
pub fn artifacts_path(workspace_id: &str) -> String {
    format!("{}/{workspace_id}/artifacts", crate::WORKSPACES_PATH)
}

fn version_path(workspace_id: &str, artifact_id: &str, version: i64) -> String {
    format!("{}/{artifact_id}/versions/{version}", artifacts_path(workspace_id))
}

pub fn artifact_preview_path(workspace_id: &str, artifact_id: &str, version: i64) -> String {
    format!("{}/preview", version_path(workspace_id, artifact_id, version))
}

pub fn artifact_content_path(workspace_id: &str, artifact_id: &str, version: i64) -> String {
    format!("{}/content", version_path(workspace_id, artifact_id, version))
}

/// 下載回應帶的標頭：內容的 SHA-256（與 manifest 一致）。
pub const SHA256_HEADER: &str = "x-baton-sha256";

// --- Agent API：register_artifact ---

#[derive(Debug, Deserialize)]
pub(crate) struct RegisterRequest {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    content_base64: Option<String>,
    #[serde(flatten)]
    claims: Claims,
}

/// 登記成功的回應。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactRegistered {
    pub artifact_id: String,
    pub version: i64,
    pub name: String,
    pub size_bytes: i64,
    pub sha256: String,
    /// 這次登記是否建立了新的成果（false 表示為既有成果追加新版本）。
    pub created: bool,
}

impl From<ArtifactError> for ApiError {
    fn from(error: ArtifactError) -> Self {
        let status = match error {
            ArtifactError::TooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            _ => StatusCode::BAD_REQUEST,
        };
        ApiError { status, code: error.code(), message: error.to_string() }
    }
}

fn error(status: StatusCode, code: &'static str, message: impl Into<String>) -> ApiError {
    ApiError { status, code, message: message.into() }
}

fn decode_content(request: &RegisterRequest) -> Result<Vec<u8>, ArtifactError> {
    match (&request.content, &request.content_base64) {
        (Some(text), None) => Ok(text.clone().into_bytes()),
        (None, Some(encoded)) => base64::engine::general_purpose::STANDARD
            .decode(encoded.trim())
            .map_err(|_| ArtifactError::ContentNotBase64),
        _ => Err(ArtifactError::ContentSource),
    }
}

pub(crate) fn artifact_root(state: &ApiState) -> PathBuf {
    state.data_dir.join(ARTIFACTS_DIR)
}

pub(crate) fn storage_unavailable(error: impl std::fmt::Display) -> ApiError {
    self::error(StatusCode::INTERNAL_SERVER_ERROR, "artifact_storage_failed", format!("成果儲存發生錯誤：{error}"))
}

pub(crate) async fn register(
    State(state): State<Arc<ApiState>>,
    Extension(record): Extension<CredentialRecord>,
    body: Result<Json<RegisterRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<ArtifactRegistered>), ApiError> {
    let Json(request) = body.map_err(bad_json)?;
    let scope = scope_of(&record, &request.claims)?;
    let draft = ArtifactDraft::new(&request.name, &request.description)?;
    let bytes = decode_content(&request)?;
    artifact::check_size(bytes.len())?;

    // 不是目前寫入所有者的 Run 不可以在儲存區留下任何檔案：先確認，才寫入（提交 manifest 的交易會再確認一次）
    if !state.store.agent_context(&scope).await?.is_current {
        return Err(RunApplyError::Rejected(RunError::StaleReport).into());
    }

    let sha256 = artifact::sha256_hex(&bytes);
    let storage_path = artifact::blob_storage_path(&scope.workspace_id, &new_id("blob")?);
    state
        .workspace_fs
        .write_artifact_atomic(&artifact_root(&state).join(&storage_path), &bytes)
        .await
        .map_err(storage_unavailable)?;

    let input = NewArtifactVersion {
        new_artifact_id: new_id("art")?,
        draft: draft.clone(),
        size_bytes: bytes.len() as i64,
        sha256,
        storage_path,
    };
    let registered = state.store.register_artifact(&scope, input, state.clock.now()).await?;
    let response = ArtifactRegistered {
        artifact_id: registered.artifact_id,
        version: registered.version.version,
        name: draft.name,
        size_bytes: registered.version.size_bytes,
        sha256: registered.version.sha256,
        created: registered.created,
    };
    Ok((StatusCode::CREATED, Json(response)))
}

/// 登記路由要的請求本文上限。
pub(crate) fn register_body_limit() -> DefaultBodyLimit {
    DefaultBodyLimit::max(REGISTER_BODY_LIMIT)
}

// --- 管理 API：瀏覽、預覽、下載 ---

/// 一個版本加上它的檔案現況。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactVersionView {
    #[serde(flatten)]
    pub version: ArtifactVersion,
    /// 檔案是否存在、大小是否與 manifest 相符（完整雜湊在預覽與下載時核對）。
    pub file_status: FileStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactView {
    pub id: String,
    pub workspace_id: String,
    pub task_id: String,
    pub name: String,
    pub created_at: Timestamp,
    pub versions: Vec<ArtifactVersionView>,
}

/// 成果瀏覽：已登記的成果，以及沒有 manifest 的檔案（寫入中途中斷留下的，不是成果）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactList {
    pub artifacts: Vec<ArtifactView>,
    pub unregistered_files: Vec<UnregisteredFile>,
}

/// 預覽回應。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactPreview {
    pub artifact_id: String,
    pub version: i64,
    pub name: String,
    pub size_bytes: i64,
    pub sha256: String,
    pub preview: Preview,
}

/// Workspace 在成果儲存內的所有檔案（`storage_path` 以 `/` 分隔，相對於成果儲存根目錄）。
async fn stored_files(state: &ApiState, workspace_id: &str) -> Result<Vec<StoredFile>, ApiError> {
    let root = artifact_root(state);
    let entries = state.workspace_fs.list_artifact_files(&root.join(workspace_id)).await.map_err(storage_unavailable)?;
    Ok(entries
        .into_iter()
        .filter_map(|entry| {
            let relative = entry.path.strip_prefix(&root).ok()?;
            let parts: Vec<_> = relative.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
            Some(StoredFile { storage_path: parts.join("/"), size_bytes: entry.size_bytes })
        })
        .collect())
}

pub(crate) async fn list(
    State(state): State<Arc<ApiState>>,
    UrlPath(workspace_id): UrlPath<String>,
) -> Result<Json<ArtifactList>, ApiError> {
    let artifacts = state.store.artifact_list(&workspace_id).await?;
    let files = stored_files(&state, &workspace_id).await?;
    let sizes: HashMap<String, u64> = files.iter().map(|f| (f.storage_path.clone(), f.size_bytes)).collect();
    let registered: HashSet<String> =
        artifacts.iter().flat_map(|a| a.versions.iter().map(|v| v.storage_path.clone())).collect();

    let artifacts = artifacts
        .into_iter()
        .map(|artifact| ArtifactView {
            id: artifact.id,
            workspace_id: artifact.workspace_id,
            task_id: artifact.task_id,
            name: artifact.name,
            created_at: artifact.created_at,
            versions: artifact
                .versions
                .into_iter()
                .map(|version| ArtifactVersionView { file_status: artifact::file_status(&version, &sizes), version })
                .collect(),
        })
        .collect();
    Ok(Json(ArtifactList { artifacts, unregistered_files: artifact::classify_stored_files(&files, &registered) }))
}

/// 找到成果的某個版本；成果不屬於這個 Workspace、或版本不存在，一律是 404（不透露別的 Workspace 有什麼）。
async fn find_version(
    state: &ApiState,
    workspace_id: &str,
    artifact_id: &str,
    version: &str,
) -> Result<(Artifact, ArtifactVersion), ApiError> {
    let not_found = || error(StatusCode::NOT_FOUND, "artifact_not_found", "找不到這個成果或版本");
    let version: i64 = version.parse().map_err(|_| not_found())?;
    let artifact =
        state.store.artifact_list(workspace_id).await?.into_iter().find(|a| a.id == artifact_id).ok_or_else(not_found)?;
    let found = artifact.versions.iter().find(|v| v.version == version).cloned().ok_or_else(not_found)?;
    Ok((artifact, found))
}

/// 讀出版本的內容並核對大小與雜湊；不符就拒絕。
async fn verified_content(state: &ApiState, version: &ArtifactVersion) -> Result<Vec<u8>, ApiError> {
    let path = artifact_root(state).join(&version.storage_path);
    let Some(bytes) = state.workspace_fs.read_artifact(&path).await.map_err(storage_unavailable)? else {
        return Err(error(StatusCode::CONFLICT, "artifact_file_missing", "成果檔案不存在，無法讀取"));
    };
    if i64::try_from(bytes.len()).ok() != Some(version.size_bytes) || artifact::sha256_hex(&bytes) != version.sha256 {
        return Err(error(
            StatusCode::CONFLICT,
            "artifact_integrity_failed",
            "成果檔案的大小或雜湊與 manifest 不一致，已拒絕提供這份內容",
        ));
    }
    Ok(bytes)
}

pub(crate) async fn preview(
    State(state): State<Arc<ApiState>>,
    UrlPath((workspace_id, artifact_id, version)): UrlPath<(String, String, String)>,
) -> Result<Json<ArtifactPreview>, ApiError> {
    let (artifact, version) = find_version(&state, &workspace_id, &artifact_id, &version).await?;
    let bytes = verified_content(&state, &version).await?;
    Ok(Json(ArtifactPreview {
        artifact_id: artifact.id,
        version: version.version,
        name: artifact.name,
        size_bytes: version.size_bytes,
        sha256: version.sha256,
        preview: artifact::preview(&bytes),
    }))
}

pub(crate) async fn content(
    State(state): State<Arc<ApiState>>,
    UrlPath((workspace_id, artifact_id, version)): UrlPath<(String, String, String)>,
) -> Result<Response, ApiError> {
    let (artifact, version) = find_version(&state, &workspace_id, &artifact_id, &version).await?;
    let bytes = verified_content(&state, &version).await?;
    let headers = [
        (header::CONTENT_TYPE, HeaderValue::from_static("application/octet-stream")),
        (header::CONTENT_DISPOSITION, content_disposition(&artifact.name)),
        (HeaderName::from_static(SHA256_HEADER), HeaderValue::from_str(&version.sha256).map_err(storage_unavailable)?),
    ];
    Ok((headers, bytes).into_response())
}

/// `attachment`：ASCII 後備檔名加上 RFC 5987 的 UTF-8 檔名。成果名稱已由 Domain 限制為不含引號與路徑分隔字元。
fn content_disposition(name: &str) -> HeaderValue {
    let fallback: String = name.chars().map(|c| if c.is_ascii() && !c.is_ascii_control() { c } else { '_' }).collect();
    let encoded: String = name
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect();
    HeaderValue::from_str(&format!("attachment; filename=\"{fallback}\"; filename*=UTF-8''{encoded}"))
        .unwrap_or_else(|_| HeaderValue::from_static("attachment"))
}
