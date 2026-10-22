//! 探索測試到可重現缺陷的端點（票 47，issue #3 §10.3）。
//!
//! - 管理 API（桌面憑證）：Workspace 的探索概況（唯讀）：各探索 Task 目前的章程，以及疑點、已確認缺陷、未驗風險、
//!   已排除與已修復的疑點和它們的重現紀錄。
//! - Controller 工具：`define_exploration_charter`（定下或改版章程）、`triage_exploration_finding`（排除、改為未驗風險、
//!   重新開啟；排除為預期設計依決策權限表送裁定）、`reproduce_exploration_finding`（提出重現案例或驗證修復，
//!   由 Runtime 排入驗證要求）、`get_exploration`。
//! - Worker 工具：`report_exploration_finding`（在自己 Task 的章程下回報疑點，一律先是 suspected）。

use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use baton_domain::auth::CredentialRecord;
use baton_domain::authority::Matter;
use baton_domain::decision::ControllerOutcome;
use baton_domain::exploration::{
    self, AttemptPurpose, Charter, CharterInput, ExclusionReason, ExplorationError, ExplorationFinding, FindingReport,
    FindingStatus, ReproductionAttempt, TriageAction,
};
use baton_domain::verification_profile::{Journey, VerificationRequest};
use baton_store::{ExplorationApplyError, ExplorationData, FindingRecord};
use serde::{Deserialize, Serialize};

use crate::agent::{scope_of, Claims};
use crate::authorization::bad_json;
use crate::controller::{controller_scope, require_controller, ControllerToolError};
use crate::reviews::{clip, decide, internal, requires_decision};
use crate::workspaces::{new_id, ApiError};
use crate::ApiState;

pub(crate) const EXPLORATION_ROUTE: &str = "/api/v1/workspaces/{id}/exploration";

pub fn workspace_exploration_path(workspace_id: &str) -> String {
    format!("/api/v1/workspaces/{workspace_id}/exploration")
}

/// 一個探索 Task 目前的章程。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CharterView {
    #[serde(flatten)]
    pub charter: Charter,
    pub task_title: Option<String>,
}

/// 一個疑點與它的重現紀錄（新的在前）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FindingView {
    #[serde(flatten)]
    pub finding: ExplorationFinding,
    pub task_title: Option<String>,
    pub attempts: Vec<ReproductionAttempt>,
}

/// Workspace 的探索概況：各探索 Task 目前的章程與全部疑點（新的在前）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExplorationOverview {
    pub workspace_id: String,
    pub charters: Vec<CharterView>,
    pub findings: Vec<FindingView>,
}

/// `define_exploration_charter` 的回應：`created` 為 false 表示內容與目前版本相同，沒有新增版本。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DefineCharterResponse {
    pub charter: Charter,
    pub created: bool,
}

/// `triage_exploration_finding` 的回應（需要裁定時改回 `requires_decision`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriageFindingResponse {
    pub finding: FindingView,
    pub message: String,
}

/// `reproduce_exploration_finding` 的回應：排入的驗證要求與重現紀錄。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReproduceFindingResponse {
    pub finding: FindingView,
    pub attempt: ReproductionAttempt,
    pub request: VerificationRequest,
    pub message: String,
}

// ---------------------------------------------------------------- 錯誤與檢視

impl From<ExplorationError> for ApiError {
    fn from(error: ExplorationError) -> Self {
        let status = match error {
            ExplorationError::Invalid(_) => StatusCode::UNPROCESSABLE_ENTITY,
            ExplorationError::FindingNotFound(_) => StatusCode::NOT_FOUND,
            ExplorationError::CharterRequired(_)
            | ExplorationError::DefectLocked(_)
            | ExplorationError::TransitionRefused(_)
            | ExplorationError::ReproductionPending(_)
            | ExplorationError::DecisionRequired => StatusCode::CONFLICT,
        };
        ApiError { status, code: error.code(), message: error.to_string() }
    }
}

impl From<ExplorationApplyError> for ApiError {
    fn from(error: ExplorationApplyError) -> Self {
        let message = error.to_string();
        match error {
            ExplorationApplyError::Rejected(rejected) => rejected.into(),
            ExplorationApplyError::Profile(rejected) => rejected.into(),
            ExplorationApplyError::WorkspaceNotFound(_) => {
                ApiError { status: StatusCode::NOT_FOUND, code: "workspace_not_found", message }
            }
            ExplorationApplyError::WorkspaceArchived(_) => {
                ApiError { status: StatusCode::CONFLICT, code: "workspace_archived", message }
            }
            ExplorationApplyError::TaskNotFound(_) => ApiError { status: StatusCode::NOT_FOUND, code: "task_not_found", message },
            ExplorationApplyError::Stale => ApiError {
                status: StatusCode::CONFLICT,
                code: "stale_report",
                message: "這個 Run 已不是目前的寫入所有者（Run 已結束或已被接替），沒有改變任何狀態".to_string(),
            },
            ExplorationApplyError::Store(store) => store.into(),
        }
    }
}

fn finding_view(record: FindingRecord) -> FindingView {
    FindingView { finding: record.finding, task_title: record.task_title, attempts: record.attempts }
}

fn overview_view(workspace_id: String, data: ExplorationData) -> ExplorationOverview {
    ExplorationOverview {
        workspace_id,
        charters: data.charters.into_iter().map(|(charter, task_title)| CharterView { charter, task_title }).collect(),
        findings: data.findings.into_iter().map(finding_view).collect(),
    }
}

async fn require_workspace(state: &ApiState, id: &str) -> Result<(), ApiError> {
    if state.store.list_workspaces().await?.iter().any(|w| w.id == id) {
        return Ok(());
    }
    Err(ApiError { status: StatusCode::NOT_FOUND, code: "workspace_not_found", message: format!("找不到 Workspace {id}") })
}

async fn load_finding(state: &ApiState, workspace_id: &str, finding_id: &str) -> Result<FindingRecord, ApiError> {
    let finding_id = finding_id.trim();
    state
        .store
        .exploration_finding(workspace_id, finding_id)
        .await?
        .ok_or_else(|| ExplorationError::FindingNotFound(finding_id.to_string()).into())
}

// ---------------------------------------------------------------- 管理 API

pub(crate) async fn overview(
    State(state): State<Arc<ApiState>>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<ExplorationOverview>, ApiError> {
    require_workspace(&state, &id).await?;
    let data = state.store.exploration(&id).await?;
    Ok(Json(overview_view(id, data)))
}

// ---------------------------------------------------------------- Worker 工具

#[derive(Debug, Deserialize)]
pub(crate) struct ReportFindingRequest {
    #[serde(flatten)]
    report: FindingReport,
    #[serde(flatten)]
    claims: Claims,
}

pub(crate) async fn report_exploration_finding(
    State(state): State<Arc<ApiState>>,
    Extension(record): Extension<CredentialRecord>,
    body: Result<Json<ReportFindingRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<FindingView>), ApiError> {
    let Json(request) = body.map_err(bad_json)?;
    let scope = scope_of(&record, &request.claims)?;
    let reported = state.store.report_exploration_finding(&scope, request.report, new_id("xf")?, state.clock.now()).await?;
    Ok((StatusCode::CREATED, Json(finding_view(reported))))
}

// ---------------------------------------------------------------- Controller 工具

#[derive(Debug, Deserialize)]
pub(crate) struct DefineCharterRequest {
    #[serde(flatten)]
    input: CharterInput,
    #[serde(flatten)]
    claims: Claims,
}

pub(crate) async fn define_exploration_charter(
    State(state): State<Arc<ApiState>>,
    Extension(record): Extension<CredentialRecord>,
    body: Result<Json<DefineCharterRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<DefineCharterResponse>), ApiError> {
    require_controller(&record)?;
    let Json(request) = body.map_err(bad_json)?;
    let scope = controller_scope(&state, &record, &request.claims).await?;
    let id = new_id("xch")?;
    let (charter, created) = state
        .store
        .define_exploration_charter(&scope.workspace_id, request.input, scope.controller_run_id.clone(), id, state.clock.now())
        .await?;
    let status = if created { StatusCode::CREATED } else { StatusCode::OK };
    Ok((status, Json(DefineCharterResponse { charter, created })))
}

#[derive(Debug, Deserialize)]
pub(crate) struct TriageFindingRequest {
    finding_id: String,
    #[serde(flatten)]
    action: TriageAction,
    #[serde(flatten)]
    claims: Claims,
}

pub(crate) async fn triage_exploration_finding(
    State(state): State<Arc<ApiState>>,
    Extension(record): Extension<CredentialRecord>,
    body: Result<Json<TriageFindingRequest>, JsonRejection>,
) -> Result<Json<TriageFindingResponse>, ControllerToolError> {
    require_controller(&record)?;
    let Json(request) = body.map_err(bad_json)?;
    let scope = controller_scope(&state, &record, &request.claims).await?;
    let current = load_finding(&state, &scope.workspace_id, &request.finding_id).await?;
    let finding = &current.finding;
    // 先以 Domain 檢查這個分流是否成立（已確認的缺陷、狀態不符、理由空白都直接拒絕），不為注定被拒的分流建立裁定請求
    exploration::triage(finding, request.action.clone(), Some(String::new())).map_err(ApiError::from)?;
    // 重現排隊中不能分流，也不為此建立裁定請求（Store 會在寫入交易內再檢查一次）
    if current.attempts.iter().any(|a| a.purpose.is_diagnostic() && a.outcome.is_none()) {
        return Err(ApiError::from(ExplorationError::ReproductionPending(finding.id.clone())).into());
    }

    let mut approved = None;
    if let TriageAction::Exclude { reason: ExclusionReason::ExpectedDesign, rationale } = &request.action {
        // 「預期設計」是產品取捨（spec D-15）：依決策權限表送裁定，使用者核准後才排除
        let summary = format!("把探索疑點「{}」排除為預期設計（不是缺陷）", clip(&finding.report.title, 80));
        let evidence = finding.report.evidence.clone();
        match decide(&state, &scope, Matter::ProductDirection, &finding.task_id, &finding.id, &summary, rationale, evidence).await? {
            ControllerOutcome::Approved { request_id, .. } => approved = Some(request_id),
            ControllerOutcome::RequiresDecision { request_id, existing } => {
                return Err(requires_decision(Matter::ProductDirection, request_id, existing, "疑點維持原狀，在使用者同意之前不會排除"));
            }
            ControllerOutcome::Denied { request_id, note } => {
                return Err(ApiError {
                    status: StatusCode::CONFLICT,
                    code: "decision_denied",
                    message: format!(
                        "使用者沒有同意把這項疑點排除為預期設計（裁定請求 {request_id}）{}；疑點維持原狀，請改以重現確認",
                        note.map(|n| format!("：{n}")).unwrap_or_default()
                    ),
                }
                .into());
            }
            ControllerOutcome::Allowed | ControllerOutcome::HumanOnly => {
                return Err(internal("產品取捨一定要送裁定").into());
            }
        }
    }
    let updated = state
        .store
        .triage_exploration_finding(&scope.workspace_id, &finding.id, request.action, approved, state.clock.now())
        .await
        .map_err(ApiError::from)?;
    let message = match updated.finding.status {
        FindingStatus::Excluded => "已排除：紀錄保留在「已排除」，需要時可以 reopen",
        FindingStatus::UnverifiedRisk => "已保留為未驗風險：不駁回、也不宣稱已確認；之後仍可提出重現案例",
        _ => "已重新開啟為疑點",
    };
    Ok(Json(TriageFindingResponse { finding: finding_view(updated), message: message.to_string() }))
}

#[derive(Debug, Deserialize)]
pub(crate) struct ReproduceFindingRequest {
    finding_id: String,
    #[serde(default)]
    journey: Option<Journey>,
    #[serde(default)]
    task_id: Option<String>,
    #[serde(flatten)]
    claims: Claims,
}

pub(crate) async fn reproduce_exploration_finding(
    State(state): State<Arc<ApiState>>,
    Extension(record): Extension<CredentialRecord>,
    body: Result<Json<ReproduceFindingRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<ReproduceFindingResponse>), ApiError> {
    require_controller(&record)?;
    let Json(request) = body.map_err(bad_json)?;
    let scope = controller_scope(&state, &record, &request.claims).await?;
    let (finding, attempt, created) = state
        .store
        .reproduce_exploration_finding(
            &scope.workspace_id,
            request.finding_id.trim(),
            request.journey,
            request.task_id,
            scope.controller_run_id.clone(),
            new_id("vreq")?,
            state.clock.now(),
        )
        .await?;
    state.supervisor_wake.notify_one();
    let message = match attempt.purpose {
        AttemptPurpose::VerifyFix => "已排入修復驗證：以確認時的案例執行，通過後成為已修復並納入正常回歸",
        _ => "已排入重現：代表旅程當對照，對照通過且案例以產品行為斷言失敗才會確認為缺陷",
    };
    Ok((
        StatusCode::CREATED,
        Json(ReproduceFindingResponse { finding: finding_view(finding), attempt, request: created, message: message.to_string() }),
    ))
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct GetExplorationRequest {
    #[serde(flatten)]
    claims: Claims,
}

pub(crate) async fn get_exploration(
    State(state): State<Arc<ApiState>>,
    Extension(record): Extension<CredentialRecord>,
    body: Result<Json<GetExplorationRequest>, JsonRejection>,
) -> Result<Json<ExplorationOverview>, ApiError> {
    require_controller(&record)?;
    let Json(request) = body.map_err(bad_json)?;
    let scope = controller_scope(&state, &record, &request.claims).await?;
    let data = state.store.exploration(&scope.workspace_id).await?;
    Ok(Json(overview_view(scope.workspace_id, data)))
}
