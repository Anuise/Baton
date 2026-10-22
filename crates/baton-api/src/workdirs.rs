//! Task 工作目錄的準備與整合佇列的端點（票 20）。
//!
//! 準備工作目錄是外部副作用：在交易外經由 WorkspaceFs 建立 worktree 或輸出目錄，成功或失敗都登記到 Store。
//! 派工（`run::check_dispatch`）只認已登記且可用的目錄，所以目錄不可用時不派工，等待原因就是登記的失敗原因。
//! 使用者要求派工時（`runs::dispatch`）與 Supervisor 排程前都會先準備；兩者都可能重做，建立是冪等的。

use std::path::Path;
use std::sync::Arc;

use axum::extract::State;
use axum::Json;
use baton_domain::integration::IntegrationQueue;
use baton_domain::workdir::{self, PrepareFailure, WorkdirKind, WorkdirPlan};
use baton_domain::Timestamp;
use baton_ports::{GitState, WorkspaceFs};
use baton_store::{Store, StoreError};

use crate::workspaces::ApiError;
use crate::ApiState;

/// 全域整合佇列的路徑。
pub const INTEGRATION_QUEUE_PATH: &str = "/api/v1/integration-queue";

/// 同一時間只有一次準備在進行：同一個 Task 同時被要求派工好幾次時，只有第一次真的建立目錄，
/// 之後的看到已登記的結果就不再重做（登記的版本也不會互相衝突）。
static PREPARING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// 為需要的 Task 準備工作目錄並登記結果；`explicit` 是使用者剛要求派工的（Workspace、Task），失敗也立即重試。
/// 回傳這次嘗試準備的數量。個別目錄的失敗只會被登記，不是這個函式的錯誤。
pub async fn prepare(
    store: &Store,
    fs: &dyn WorkspaceFs,
    now: Timestamp,
    explicit: Option<(String, String)>,
) -> Result<usize, StoreError> {
    let _serial = PREPARING.lock().await;
    let candidates = store.workdir_candidates(explicit, now).await?;
    for candidate in &candidates {
        let plan = workdir::plan(&candidate.workspace, &candidate.task);
        let existing = candidate.existing.as_ref();
        let (registered, event) = match prepare_one(fs, &plan).await {
            Ok(base_branch) => workdir::record_prepared(existing, &plan, base_branch, now),
            Err(failure) => workdir::record_failed(existing, &plan, &failure, now),
        };
        store.record_workdir(registered, event).await?;
    }
    Ok(candidates.len())
}

/// 建立一個目錄；成功時回傳 worktree 的整合目標分支（輸出目錄為 `None`）。
async fn prepare_one(fs: &dyn WorkspaceFs, plan: &WorkdirPlan) -> Result<Option<String>, PrepareFailure> {
    let io = |e: baton_ports::PortError| PrepareFailure::Io(e.to_string());
    let base_branch = match plan.kind {
        WorkdirKind::Worktree => match fs.inspect_git(Path::new(&plan.repo)).await.map_err(io)? {
            GitState::NotRepository => return Err(PrepareFailure::NotGitRepository),
            GitState::Repository { branch: None } => return Err(PrepareFailure::DetachedHead),
            GitState::Repository { branch: Some(branch) } => Some(branch),
        },
        WorkdirKind::OutputDir => None,
    };
    // `.baton` 內放一份忽略全部內容的 .gitignore：平台的目錄不會讓主 checkout 出現未追蹤的檔案
    let gitignore = Path::new(&plan.gitignore);
    if fs.read_workspace_file(gitignore).await.map_err(io)?.is_none() {
        fs.write_artifact_atomic(gitignore, workdir::GITIGNORE_CONTENT.as_bytes()).await.map_err(io)?;
    }
    match (&plan.kind, &plan.branch) {
        (WorkdirKind::Worktree, Some(branch)) => {
            fs.create_worktree(Path::new(&plan.repo), branch, Path::new(&plan.path)).await.map_err(io)?;
        }
        _ => fs.ensure_directory(Path::new(&plan.path)).await.map_err(io)?,
    }
    Ok(base_branch)
}

/// 全域整合佇列：處理中的項目與全部項目（依排入順序）。
pub(crate) async fn queue(State(state): State<Arc<ApiState>>) -> Result<Json<IntegrationQueue>, ApiError> {
    Ok(Json(state.store.integration_queue().await?))
}
