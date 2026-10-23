//! 專案理解端點（票 42）：Project Brief、Glossary、Project Map 與 repo 內 GLOSSARY／ADR 的投影。
//!
//! - Controller 工具（只接受 Controller 憑證）：`record_understanding` 建立或更新一項，來源由 Runtime 讀取 Workspace 內的檔案查驗
//!   （版本 = 內容 SHA-256、查驗時間 = 現在），找不到就拒絕；沒有來源的描述只能是假說或未知。`get_project_understanding` 讀取目前內容。
//! - 管理 API（桌面憑證）：專案理解視圖、重新比對投影（偵測外部編輯）、協調投影（以 Baton 版本覆寫或保留外部版本）。
//! - 投影只在 Workspace 是 Git repo 時進行；比對與寫檔以一把鎖序列化，寫入前一律先讀回檔案、與最後寫入的雜湊比對，
//!   被外部修改、刪除或接入前就存在的檔案不覆寫，改為開啟協調，並讓 Controller 在對話中收到工作事件。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use baton_domain::artifact::sha256_hex;
use baton_domain::auth::CredentialRecord;
use baton_domain::knowledge::{KnowledgeEntry, KnowledgeKind, ScopeKind};
use baton_domain::understanding::{
    self, ItemStatus, Projection, ProjectionKind, ProjectionTarget, ReconcileAction, RecordInput, Section, Source,
    SourceCheck, SourceInput, SourceRequest, UnderstandingError, UnderstandingItem,
};
use baton_domain::workspace::Workspace;
use baton_domain::Timestamp;
use baton_ports::GitState;
use baton_store::UnderstandingApplyError;
use serde::{Deserialize, Serialize};

use crate::agent::Claims;
use crate::authorization::bad_json;
use crate::controller::{controller_scope, require_controller};
use crate::workspaces::{new_id, ApiError};
use crate::ApiState;

pub(crate) const UNDERSTANDING_ROUTE: &str = "/api/v1/workspaces/{id}/understanding";
pub(crate) const SYNC_ROUTE: &str = "/api/v1/workspaces/{id}/understanding/sync";
pub(crate) const RECONCILE_ROUTE: &str = "/api/v1/workspaces/{id}/understanding/projections/reconcile";

pub fn understanding_path(workspace_id: &str) -> String {
    format!("/api/v1/workspaces/{workspace_id}/understanding")
}

pub fn understanding_sync_path(workspace_id: &str) -> String {
    format!("{}/sync", understanding_path(workspace_id))
}

pub fn projection_reconcile_path(workspace_id: &str) -> String {
    format!("{}/projections/reconcile", understanding_path(workspace_id))
}

/// 來源加上「目前是否已與查驗時不同」：檔案內容改變或檔案不存在；外部來源無法讀取，一律為 `false`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceView {
    #[serde(flatten)]
    pub source: Source,
    pub changed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnderstandingItemView {
    pub id: String,
    pub workspace_id: String,
    pub section: Section,
    pub key: String,
    pub text: String,
    pub status: ItemStatus,
    pub sources: Vec<SourceView>,
    pub revision: i64,
    pub recorded_by: String,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    /// 已確認，但有檔案來源在查驗之後改變：需要重新查驗。
    pub stale: bool,
}

/// 專案理解視圖。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnderstandingView {
    pub workspace_id: String,
    /// Workspace 是 Git repo：GLOSSARY／ADR 會投影到 repo 內。
    pub projection_enabled: bool,
    /// 依文件別、名稱排序，含已移除的項目。
    pub items: Vec<UnderstandingItemView>,
    /// 依路徑排序。
    pub projections: Vec<Projection>,
}

/// 協調一個投影檔。`expected_external_hash` 是使用者檢視時檔案的雜湊（當時檔案不存在則為 `null`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReconcileRequest {
    pub path: String,
    pub action: ReconcileAction,
    #[serde(default)]
    pub expected_external_hash: Option<String>,
}

/// `record_understanding` 的請求。
#[derive(Debug, Deserialize)]
pub(crate) struct RecordUnderstandingRequest {
    section: Section,
    key: String,
    #[serde(default)]
    text: String,
    /// 沒給時是假說：沒有明說已確認的描述不會被當作事實。
    #[serde(default = "hypothesis")]
    status: ItemStatus,
    #[serde(default)]
    sources: Vec<SourceInput>,
    #[serde(flatten)]
    claims: Claims,
}

fn hypothesis() -> ItemStatus {
    ItemStatus::Hypothesis
}

/// `record_understanding` 的回應：目前的項目、這次是否有變更，以及投影檔的狀態（協調中的檔案 Baton 不會寫入）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordUnderstandingResponse {
    pub item: UnderstandingItemView,
    pub changed: bool,
    pub projections: Vec<Projection>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct GetUnderstandingRequest {
    #[serde(flatten)]
    claims: Claims,
}

impl From<UnderstandingError> for ApiError {
    fn from(error: UnderstandingError) -> Self {
        let status = match error {
            UnderstandingError::Invalid(_) | UnderstandingError::UnverifiedClaim | UnderstandingError::SourceNotFound(_) => {
                StatusCode::UNPROCESSABLE_ENTITY
            }
            UnderstandingError::NotFound(_) | UnderstandingError::ProjectionNotFound(_) => StatusCode::NOT_FOUND,
            UnderstandingError::ProjectionTransitionInvalid { .. } | UnderstandingError::ProjectionChanged(_) => {
                StatusCode::CONFLICT
            }
        };
        ApiError { status, code: error.code(), message: error.to_string() }
    }
}

impl From<UnderstandingApplyError> for ApiError {
    fn from(error: UnderstandingApplyError) -> Self {
        match error {
            UnderstandingApplyError::Rejected(rejected) => rejected.into(),
            UnderstandingApplyError::Store(store) => store.into(),
        }
    }
}

fn io_error(error: baton_ports::PortError) -> ApiError {
    ApiError { status: StatusCode::INTERNAL_SERVER_ERROR, code: "workspace_io_failed", message: error.to_string() }
}

async fn find_workspace(state: &ApiState, id: &str) -> Result<Workspace, ApiError> {
    state.store.list_workspaces().await?.into_iter().find(|w| w.id == id).ok_or_else(|| ApiError {
        status: StatusCode::NOT_FOUND,
        code: "workspace_not_found",
        message: format!("找不到 Workspace {id}"),
    })
}

fn file_path(workspace: &Workspace, relative: &str) -> PathBuf {
    Path::new(&workspace.path).join(relative)
}

/// 讀取 Workspace 內的檔案；不存在為 `None`。
async fn read(state: &ApiState, workspace: &Workspace, relative: &str) -> Result<Option<Vec<u8>>, ApiError> {
    state.workspace_fs.read_workspace_file(&file_path(workspace, relative)).await.map_err(io_error)
}

/// 查驗來源：檔案由 Runtime 讀取並自行計算版本；不是檔案（例如目錄）的路徑拒絕。
async fn check_sources(
    state: &ApiState,
    workspace: &Workspace,
    requests: Vec<SourceRequest>,
    now: Timestamp,
) -> Result<Vec<SourceCheck>, ApiError> {
    let mut checks = Vec::new();
    for request in requests {
        checks.push(match request {
            SourceRequest::File { path, line } => {
                match state.workspace_fs.read_workspace_file(&file_path(workspace, &path)).await {
                    Ok(Some(bytes)) => {
                        SourceCheck::Found(Source::File { path, line, version: sha256_hex(&bytes), checked_at: now })
                    }
                    Ok(None) => SourceCheck::Missing { path },
                    Err(_) => {
                        return Err(UnderstandingError::Invalid(format!("來源 {path} 不是可讀取的檔案（目錄請改指其中的檔案）")).into())
                    }
                }
            }
            SourceRequest::External { reference, version } => {
                SourceCheck::Found(Source::External { reference, version, checked_at: now })
            }
        });
    }
    Ok(checks)
}

async fn item_view(state: &ApiState, workspace: &Workspace, item: UnderstandingItem) -> UnderstandingItemView {
    let mut sources = Vec::new();
    for source in item.sources {
        let changed = match &source {
            Source::File { path, version, .. } => match read(state, workspace, path).await {
                Ok(Some(bytes)) => sha256_hex(&bytes) != *version,
                // 檔案不存在或讀不到：無法再支持這項描述
                _ => true,
            },
            Source::External { .. } => false,
        };
        sources.push(SourceView { source, changed });
    }
    let stale = item.status == ItemStatus::Confirmed && sources.iter().any(|s| s.changed);
    UnderstandingItemView {
        id: item.id,
        workspace_id: item.workspace_id,
        section: item.section,
        key: item.key,
        text: item.text,
        status: item.status,
        sources,
        revision: item.revision,
        recorded_by: item.recorded_by,
        created_at: item.created_at,
        updated_at: item.updated_at,
        stale,
    }
}

async fn projection_enabled(state: &ApiState, workspace: &Workspace) -> Result<bool, ApiError> {
    let git = state.workspace_fs.inspect_git(Path::new(&workspace.path)).await.map_err(io_error)?;
    Ok(matches!(git, GitState::Repository { .. }))
}

async fn build_view(state: &ApiState, workspace: &Workspace) -> Result<UnderstandingView, ApiError> {
    let mut items = Vec::new();
    for item in state.store.understanding_items(&workspace.id).await? {
        items.push(item_view(state, workspace, item).await);
    }
    Ok(UnderstandingView {
        workspace_id: workspace.id.clone(),
        projection_enabled: projection_enabled(state, workspace).await?,
        items,
        projections: state.store.understanding_projections(&workspace.id).await?,
    })
}

/// 這個 Workspace 要產生的投影檔：有過 Glossary 項目（或已投影過）就有 GLOSSARY.md；每個 Workspace 範圍的決策一個 ADR。
async fn targets(state: &ApiState, workspace: &Workspace) -> Result<Vec<ProjectionTarget>, ApiError> {
    let items = state.store.understanding_items(&workspace.id).await?;
    let projected = state.store.understanding_projections(&workspace.id).await?;
    let mut targets = Vec::new();
    if items.iter().any(|i| i.section == Section::Glossary) || projected.iter().any(|p| p.kind == ProjectionKind::Glossary) {
        targets.push(ProjectionTarget {
            path: understanding::GLOSSARY_PATH.to_string(),
            kind: ProjectionKind::Glossary,
            body: understanding::render_glossary(&items),
        });
    }
    for entry in state.store.workspace_decisions(&workspace.id).await? {
        targets.push(ProjectionTarget {
            path: understanding::adr_path(&entry.id),
            kind: ProjectionKind::Adr,
            body: understanding::render_adr(&entry),
        });
    }
    Ok(targets)
}

/// 比對並寫入這個 Workspace 的全部投影檔；不是 Git repo 時什麼都不做。
async fn sync_projections(state: &ApiState, workspace: &Workspace) -> Result<(), ApiError> {
    let _guard = state.projection_lock.lock().await;
    if !projection_enabled(state, workspace).await? {
        return Ok(());
    }
    for target in targets(state, workspace).await? {
        let record = state.store.understanding_projection(&workspace.id, &target.path).await?;
        let current = read(state, workspace, &target.path).await?;
        let plan = understanding::plan_projection(&workspace.id, record.as_ref(), &target, current.as_deref(), state.clock.now());
        if let Some(content) = &plan.write {
            state.workspace_fs.write_artifact_atomic(&file_path(workspace, &target.path), content.as_bytes()).await.map_err(io_error)?;
        }
        state.store.save_projection(plan, record.map_or(0, |r| r.revision)).await?;
    }
    Ok(())
}

/// 變更之後比對投影；失敗只記錄在日誌（變更本身已保存，下一次比對會再嘗試）。
async fn sync_after_change(state: &ApiState, workspace: &Workspace) {
    if let Err(error) = sync_projections(state, workspace).await {
        (state.log)(&format!("專案理解投影比對失敗（Workspace {}）：{}", workspace.id, error.message));
    }
}

/// 知識條目變更之後：Workspace 範圍的決策會投影成 ADR（票 42）。
pub(crate) async fn sync_after_knowledge_change(state: &ApiState, entry: &KnowledgeEntry) {
    if entry.kind != KnowledgeKind::Decision || entry.scope.kind != ScopeKind::Workspace {
        return;
    }
    let Some(workspace_id) = &entry.scope.workspace_id else { return };
    match find_workspace(state, workspace_id).await {
        Ok(workspace) => sync_after_change(state, &workspace).await,
        Err(error) => (state.log)(&format!("專案理解投影比對失敗（Workspace {workspace_id}）：{}", error.message)),
    }
}

// --- 管理 API ---

pub(crate) async fn view(
    State(state): State<Arc<ApiState>>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<UnderstandingView>, ApiError> {
    let workspace = find_workspace(&state, &id).await?;
    Ok(Json(build_view(&state, &workspace).await?))
}

/// 重新比對投影檔（偵測外部編輯）後回傳視圖。
pub(crate) async fn sync(
    State(state): State<Arc<ApiState>>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<UnderstandingView>, ApiError> {
    let workspace = find_workspace(&state, &id).await?;
    sync_projections(&state, &workspace).await?;
    Ok(Json(build_view(&state, &workspace).await?))
}

/// 使用者協調一個投影檔：以 Baton 版本覆寫，或保留外部版本（Baton 停止寫入）。
pub(crate) async fn reconcile(
    State(state): State<Arc<ApiState>>,
    UrlPath(id): UrlPath<String>,
    body: Result<Json<ReconcileRequest>, JsonRejection>,
) -> Result<Json<UnderstandingView>, ApiError> {
    let Json(request) = body.map_err(bad_json)?;
    let workspace = find_workspace(&state, &id).await?;
    {
        let _guard = state.projection_lock.lock().await;
        let Some(record) = state.store.understanding_projection(&workspace.id, &request.path).await? else {
            return Err(UnderstandingError::ProjectionNotFound(request.path).into());
        };
        let target = targets(&state, &workspace)
            .await?
            .into_iter()
            .find(|t| t.path == record.path)
            .ok_or_else(|| UnderstandingError::ProjectionNotFound(record.path.clone()))?;
        let current = read(&state, &workspace, &record.path).await?;
        let plan = understanding::reconcile(
            &record,
            request.action,
            request.expected_external_hash.as_deref(),
            &target,
            current.as_deref(),
            state.clock.now(),
        )?;
        if let Some(content) = &plan.write {
            state.workspace_fs.write_artifact_atomic(&file_path(&workspace, &record.path), content.as_bytes()).await.map_err(io_error)?;
        }
        state.store.save_projection(plan, record.revision).await?;
    }
    Ok(Json(build_view(&state, &workspace).await?))
}

// --- Controller 工具 ---

pub(crate) async fn record_understanding(
    State(state): State<Arc<ApiState>>,
    Extension(record): Extension<CredentialRecord>,
    body: Result<Json<RecordUnderstandingRequest>, JsonRejection>,
) -> Result<Json<RecordUnderstandingResponse>, ApiError> {
    require_controller(&record)?;
    let Json(request) = body.map_err(bad_json)?;
    let scope = controller_scope(&state, &record, &request.claims).await?;
    let workspace = find_workspace(&state, &scope.workspace_id).await?;
    let now = state.clock.now();
    let requests = understanding::normalize_sources(&request.sources)?;
    let sources = check_sources(&state, &workspace, requests, now).await?;
    let input = RecordInput { section: request.section, key: request.key, text: request.text, status: request.status, sources };
    let (item, changed) =
        state.store.record_understanding(&workspace.id, new_id("und")?, input, &scope.controller_run_id, now).await?;
    if changed {
        sync_after_change(&state, &workspace).await;
    }
    Ok(Json(RecordUnderstandingResponse {
        item: item_view(&state, &workspace, item).await,
        changed,
        projections: state.store.understanding_projections(&workspace.id).await?,
    }))
}

pub(crate) async fn get_project_understanding(
    State(state): State<Arc<ApiState>>,
    Extension(record): Extension<CredentialRecord>,
    body: Result<Json<GetUnderstandingRequest>, JsonRejection>,
) -> Result<Json<UnderstandingView>, ApiError> {
    require_controller(&record)?;
    let Json(request) = body.map_err(bad_json)?;
    let scope = controller_scope(&state, &record, &request.claims).await?;
    let workspace = find_workspace(&state, &scope.workspace_id).await?;
    Ok(Json(build_view(&state, &workspace).await?))
}
