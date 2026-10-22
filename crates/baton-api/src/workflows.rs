//! 方法登錄、Playbook 路由與 Stage／Gate 的端點（票 43）。
//!
//! - Controller 工具（只接受 Controller 憑證）：`start_workflow` 依情境開始 Workflow（Router 選 Playbook 版本並記理由），
//!   `record_stage_result` 以證據回報目前 Stage（證據由 Runtime 查驗），`get_workflows` 讀取可用 Playbook 與 Workflow。
//! - 管理 API（桌面憑證）：Playbook 與方法目錄、發布 Playbook 新版本、Workspace 的流程視圖、使用者附原因跳過 Stage
//!   （spec D-15：跳過 Stage 或 Gate 必須由使用者決定；裁定與安全 Gate 不能跳過）。

use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use baton_domain::auth::CredentialRecord;
use baton_domain::workflow::{
    self, ClaimedOutcome, EvidenceInput, MethodVersion, PlaybookDefinition, PlaybookVersion, Situation, StageAttempt,
    StageDefinition, StageState, WorkflowError, WorkflowInstance,
};
use baton_store::{WorkflowApplyError, WorkflowBinding, WorkflowDetail};
use serde::{Deserialize, Serialize};

use crate::agent::Claims;
use crate::authorization::bad_json;
use crate::controller::{controller_scope, require_controller};
use crate::workspaces::{new_id, ApiError};
use crate::ApiState;

pub const PLAYBOOKS_PATH: &str = "/api/v1/playbooks";
pub(crate) const WORKFLOWS_ROUTE: &str = "/api/v1/workspaces/{id}/workflows";
pub(crate) const SKIP_ROUTE: &str = "/api/v1/workspaces/{id}/workflows/{workflow_id}/skip";

pub fn workflows_path(workspace_id: &str) -> String {
    format!("/api/v1/workspaces/{workspace_id}/workflows")
}

pub fn workflow_skip_path(workspace_id: &str, workflow_id: &str) -> String {
    format!("{}/{workflow_id}/skip", workflows_path(workspace_id))
}

/// Playbook 與方法的全部版本（依 key、版本排序）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaybookCatalog {
    pub playbooks: Vec<PlaybookVersion>,
    pub methods: Vec<MethodVersion>,
}

/// 發布 Playbook 新版本的結果：`created` 為 false 表示內容與最新版本相同，沒有新增版本。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishPlaybookResponse {
    pub playbook: PlaybookVersion,
    pub created: bool,
}

/// 一個 Stage 的定義、在這個 Workflow 中的狀態與 attempt 歷史。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StageView {
    #[serde(flatten)]
    pub definition: StageDefinition,
    pub state: StageState,
    pub attempts: Vec<StageAttempt>,
}

/// 流程視圖中的一個 Workflow。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowView {
    #[serde(flatten)]
    pub workflow: WorkflowInstance,
    pub playbook_title: String,
    /// 這個 Playbook 目前的最新版本；大於 `playbook_version` 表示已有新版，但這個 Workflow 維持原版本。
    pub latest_playbook_version: i64,
    pub task_title: Option<String>,
    /// 依 Playbook 順序。
    pub stages: Vec<StageView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowList {
    pub workspace_id: String,
    /// 最新的在前。
    pub workflows: Vec<WorkflowView>,
}

/// 使用者跳過目前的 Stage。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkipStageRequest {
    pub stage: String,
    pub expected_revision: i64,
    pub reason: String,
}

/// `get_workflows` 回應中可選用的 Playbook（每個情境的最新版本）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaybookSummary {
    pub key: String,
    pub title: String,
    pub situation: Situation,
    pub applicability: String,
    pub version: i64,
    /// 是否是開發流程（研究與文件 Task 不能使用）。
    pub development: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowsForController {
    pub playbooks: Vec<PlaybookSummary>,
    pub workflows: Vec<WorkflowView>,
}

/// `record_stage_result` 的回應：這次的 attempt 與更新後的 Workflow。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StageResultResponse {
    pub attempt: StageAttempt,
    pub workflow: WorkflowView,
}

#[derive(Debug, Deserialize)]
pub(crate) struct StartWorkflowRequest {
    situation: Situation,
    #[serde(default)]
    task_id: Option<String>,
    #[serde(default)]
    goal_id: Option<String>,
    rationale: String,
    #[serde(flatten)]
    claims: Claims,
}

#[derive(Debug, Deserialize)]
pub(crate) struct RecordStageResultRequest {
    workflow_id: String,
    stage: String,
    expected_revision: i64,
    outcome: ClaimedOutcome,
    #[serde(default)]
    evidence: Vec<EvidenceInput>,
    #[serde(default)]
    note: String,
    #[serde(flatten)]
    claims: Claims,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct GetWorkflowsRequest {
    #[serde(flatten)]
    claims: Claims,
}

impl From<WorkflowError> for ApiError {
    fn from(error: WorkflowError) -> Self {
        let status = match error {
            WorkflowError::Invalid(_) | WorkflowError::PlaybookInvalid(_) | WorkflowError::NotApplicable(_) => {
                StatusCode::UNPROCESSABLE_ENTITY
            }
            WorkflowError::PlaybookNotFound(_) | WorkflowError::WorkflowNotFound(_) => StatusCode::NOT_FOUND,
            WorkflowError::StageNotCurrent { .. }
            | WorkflowError::RevisionMismatch { .. }
            | WorkflowError::NotSkippable { .. }
            | WorkflowError::Completed(_)
            | WorkflowError::TaskHasWorkflow { .. } => StatusCode::CONFLICT,
        };
        ApiError { status, code: error.code(), message: error.to_string() }
    }
}

impl From<WorkflowApplyError> for ApiError {
    fn from(error: WorkflowApplyError) -> Self {
        match error {
            WorkflowApplyError::Rejected(rejected) => rejected.into(),
            WorkflowApplyError::TaskNotFound(id) => {
                ApiError { status: StatusCode::NOT_FOUND, code: "task_not_found", message: format!("找不到 Task {id}") }
            }
            WorkflowApplyError::GoalNotFound(id) => {
                ApiError { status: StatusCode::NOT_FOUND, code: "goal_not_found", message: format!("找不到 Goal {id}") }
            }
            WorkflowApplyError::Store(store) => store.into(),
        }
    }
}

fn view(detail: WorkflowDetail) -> WorkflowView {
    let states = workflow::stage_states(&detail.workflow, &detail.playbook, &detail.attempts);
    let stages = detail
        .playbook
        .definition
        .stages
        .iter()
        .zip(states)
        .map(|(definition, state)| StageView {
            definition: definition.clone(),
            state,
            attempts: detail.attempts.iter().filter(|a| a.stage_key == definition.key).cloned().collect(),
        })
        .collect();
    WorkflowView {
        playbook_title: detail.playbook.definition.title.clone(),
        latest_playbook_version: detail.latest_playbook_version,
        task_title: detail.task_title,
        workflow: detail.workflow,
        stages,
    }
}

async fn require_workspace(state: &ApiState, id: &str) -> Result<(), ApiError> {
    if state.store.list_workspaces().await?.iter().any(|w| w.id == id) {
        return Ok(());
    }
    Err(ApiError { status: StatusCode::NOT_FOUND, code: "workspace_not_found", message: format!("找不到 Workspace {id}") })
}

// ---------------------------------------------------------------- 管理 API

pub(crate) async fn catalog(State(state): State<Arc<ApiState>>) -> Result<Json<PlaybookCatalog>, ApiError> {
    let (playbooks, methods) = state.store.playbook_catalog().await?;
    Ok(Json(PlaybookCatalog { playbooks, methods }))
}

/// 發布既有 Playbook 的新版本：驗證後追加，不覆寫舊版本；進行中的 Workflow 維持原版本，之後開始的 Workflow 才選用新版本。
pub(crate) async fn publish(
    State(state): State<Arc<ApiState>>,
    body: Result<Json<PlaybookDefinition>, JsonRejection>,
) -> Result<(StatusCode, Json<PublishPlaybookResponse>), ApiError> {
    let Json(definition) = body.map_err(bad_json)?;
    let (playbook, created) = state.store.publish_playbook(definition, state.clock.now()).await?;
    let status = if created { StatusCode::CREATED } else { StatusCode::OK };
    Ok((status, Json(PublishPlaybookResponse { playbook, created })))
}

pub(crate) async fn list(
    State(state): State<Arc<ApiState>>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<WorkflowList>, ApiError> {
    require_workspace(&state, &id).await?;
    let workflows = state.store.workflows(&id).await?.into_iter().map(view).collect();
    Ok(Json(WorkflowList { workspace_id: id, workflows }))
}

pub(crate) async fn skip(
    State(state): State<Arc<ApiState>>,
    UrlPath((id, workflow_id)): UrlPath<(String, String)>,
    body: Result<Json<SkipStageRequest>, JsonRejection>,
) -> Result<Json<WorkflowView>, ApiError> {
    let Json(request) = body.map_err(bad_json)?;
    require_workspace(&state, &id).await?;
    let detail = state
        .store
        .skip_stage(&id, &workflow_id, request.stage, request.expected_revision, request.reason, new_id("att")?, state.clock.now())
        .await?;
    Ok(Json(view(detail)))
}

// ---------------------------------------------------------------- Controller 工具

pub(crate) async fn start_workflow(
    State(state): State<Arc<ApiState>>,
    Extension(record): Extension<CredentialRecord>,
    body: Result<Json<StartWorkflowRequest>, JsonRejection>,
) -> Result<Json<WorkflowView>, ApiError> {
    require_controller(&record)?;
    let Json(request) = body.map_err(bad_json)?;
    let scope = controller_scope(&state, &record, &request.claims).await?;
    let present = |v: Option<String>| v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let binding = match (present(request.task_id), present(request.goal_id)) {
        (Some(task_id), _) => WorkflowBinding::Task(task_id),
        (None, Some(goal_id)) => WorkflowBinding::Goal(goal_id),
        (None, None) => return Err(WorkflowError::Invalid("請指定 task_id 或 goal_id".into()).into()),
    };
    let detail = state
        .store
        .start_workflow(
            &scope.workspace_id,
            binding,
            request.situation,
            request.rationale,
            &scope.controller_run_id,
            new_id("wf")?,
            state.clock.now(),
        )
        .await?;
    Ok(Json(view(detail)))
}

pub(crate) async fn record_stage_result(
    State(state): State<Arc<ApiState>>,
    Extension(record): Extension<CredentialRecord>,
    body: Result<Json<RecordStageResultRequest>, JsonRejection>,
) -> Result<Json<StageResultResponse>, ApiError> {
    require_controller(&record)?;
    let Json(request) = body.map_err(bad_json)?;
    let scope = controller_scope(&state, &record, &request.claims).await?;
    let evidence = workflow::normalize_evidence(&request.evidence)?;
    let (attempt, detail) = state
        .store
        .record_stage_result(
            &scope.workspace_id,
            &request.workflow_id,
            request.stage,
            request.expected_revision,
            request.outcome,
            evidence,
            request.note,
            &scope.controller_run_id,
            new_id("att")?,
            state.clock.now(),
        )
        .await?;
    Ok(Json(StageResultResponse { attempt, workflow: view(detail) }))
}

pub(crate) async fn get_workflows(
    State(state): State<Arc<ApiState>>,
    Extension(record): Extension<CredentialRecord>,
    body: Result<Json<GetWorkflowsRequest>, JsonRejection>,
) -> Result<Json<WorkflowsForController>, ApiError> {
    require_controller(&record)?;
    let Json(request) = body.map_err(bad_json)?;
    let scope = controller_scope(&state, &record, &request.claims).await?;
    let playbooks = state
        .store
        .latest_playbooks()
        .await?
        .into_iter()
        .map(|p| PlaybookSummary {
            development: p.definition.situation.is_development(),
            key: p.key,
            title: p.definition.title,
            situation: p.definition.situation,
            applicability: p.definition.applicability,
            version: p.version,
        })
        .collect();
    let workflows = state.store.workflows(&scope.workspace_id).await?.into_iter().map(view).collect();
    Ok(Json(WorkflowsForController { playbooks, workflows }))
}
