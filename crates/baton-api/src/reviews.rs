//! 雙軸 Review、finding 分類與設計偏離的端點（票 44）。
//!
//! - Worker 工具：`submit_review`（reviewer 提交雙軸審查；作者不能審查自己的工作）、`report_design_deviation`
//!   （實作偏離 Design Package 時回報）。
//! - Controller 工具：`classify_review_finding`（依證據分類為修正、駁回或需裁定；駁回與需裁定依決策權限表送裁定）、
//!   `resolve_design_deviation`（拒絕可自主處理；接受送裁定，使用者核准後追加設計的新版本）、`get_reviews`。
//! - 管理 API（桌面憑證）：Workspace 的審查與設計偏離（唯讀）。
//!
//! 沿用先前審查的某一軸時，「兩個 candidate 之間變更了哪些檔案」由這裡以 Workspace 的 Git 計算（交易外），不採信 reviewer 自報。

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use baton_domain::artifact;
use baton_domain::auth::{ControllerScope, CredentialRecord};
use baton_domain::authority::Matter;
use baton_domain::decision::{ActionContent, ControllerOutcome, DecisionOutcome, DECIDED_BY_DESKTOP};
use baton_domain::review::{
    self, Axis, Classification, DesignDeviation, DeviationInput, DeviationResolution, DeviationStatus, Finding, FindingChange,
    FindingState, ReviewError, ReviewSubmission,
};
use baton_domain::Timestamp;
use baton_store::{DecisionApplyError, ReviewApplyError, ReviewRecord, ReviewsData};
use serde::{Deserialize, Serialize};

use crate::agent::{scope_of, Claims};
use crate::artifacts::{artifact_root, storage_unavailable};
use crate::authorization::bad_json;
use crate::controller::{controller_scope, ControllerToolError};
use crate::workspaces::{new_id, ApiError};
use crate::ApiState;

pub(crate) const REVIEWS_ROUTE: &str = "/api/v1/workspaces/{id}/reviews";

pub fn reviews_path(workspace_id: &str) -> String {
    format!("/api/v1/workspaces/{workspace_id}/reviews")
}

// ---------------------------------------------------------------- 回應

/// 一軸的結果與它的 finding。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AxisView {
    pub summary: String,
    pub paths: Vec<String>,
    /// 沿用自哪一份審查。
    pub reused_from: Option<String>,
    /// 實際審查這一軸的 reviewer（Task）。
    pub reviewer_task_id: String,
    /// 這一軸是否通過（沿用，或 finding 全部經使用者同意駁回）。
    pub passed: bool,
    pub findings: Vec<Finding>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewView {
    pub id: String,
    pub workspace_id: String,
    pub subject_task_id: String,
    pub subject_task_title: Option<String>,
    pub reviewer_run_id: String,
    pub reviewer_task_id: String,
    pub reviewer_task_title: Option<String>,
    pub base_revision: String,
    pub candidate_revision: String,
    /// 對照的規格版本（被審 Task 的契約版本）。
    pub spec_version: i64,
    pub standards_source: String,
    pub standards: AxisView,
    pub spec: AxisView,
    /// 兩軸都通過。
    pub passed: bool,
    /// 已過期的原因（規格版本已改變，或已有對較新 candidate 的審查）；仍有效時為 `None`。
    pub stale_reason: Option<String>,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviationView {
    #[serde(flatten)]
    pub deviation: DesignDeviation,
    /// 設計成果的名稱。
    pub design_name: Option<String>,
    /// 回報偏離的 Task 名稱。
    pub task_title: Option<String>,
}

/// 審查與設計偏離，最新的在前。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewList {
    pub workspace_id: String,
    pub reviews: Vec<ReviewView>,
    pub deviations: Vec<DeviationView>,
}

/// `classify_review_finding` 的回應（需要裁定時改回 `requires_decision`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassifyFindingResponse {
    pub finding: Finding,
    pub review: ReviewView,
    pub message: String,
}

/// `resolve_design_deviation` 的回應（需要裁定時改回 `requires_decision`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolveDeviationResponse {
    pub deviation: DeviationView,
    pub message: String,
}

fn axis_view(record: &ReviewRecord, axis: Axis) -> AxisView {
    let result = record.review.axis(axis);
    AxisView {
        summary: result.summary.clone(),
        paths: result.paths.clone(),
        reused_from: result.reused_from.clone(),
        reviewer_task_id: result.reviewer_task_id.clone(),
        passed: review::axis_passed(&record.review, axis, &record.findings),
        findings: record.findings.iter().filter(|f| f.axis == axis).cloned().collect(),
    }
}

fn review_view(record: ReviewRecord) -> ReviewView {
    let standing = review::standing(
        &record.review,
        &record.findings,
        record.current_contract_version,
        &record.latest_candidate,
        Some(&record.review.subject_task_id),
    );
    let (standards, spec) = (axis_view(&record, Axis::Standards), axis_view(&record, Axis::Spec));
    let r = record.review;
    ReviewView {
        passed: standards.passed && spec.passed,
        stale_reason: match standing {
            review::Standing::Stale(reason) => Some(reason),
            _ => None,
        },
        id: r.id,
        workspace_id: r.workspace_id,
        subject_task_id: r.subject_task_id,
        subject_task_title: record.subject_task_title,
        reviewer_run_id: r.reviewer_run_id,
        reviewer_task_id: r.reviewer_task_id,
        reviewer_task_title: record.reviewer_task_title,
        base_revision: r.base_revision,
        candidate_revision: r.candidate_revision,
        spec_version: r.spec_version,
        standards_source: r.standards_source,
        standards,
        spec,
        created_at: r.created_at,
    }
}

fn list_view(workspace_id: String, data: ReviewsData) -> ReviewList {
    ReviewList {
        workspace_id,
        reviews: data.reviews.into_iter().map(review_view).collect(),
        deviations: data
            .deviations
            .into_iter()
            .map(|(deviation, design_name, task_title)| DeviationView { deviation, design_name, task_title })
            .collect(),
    }
}

impl From<ReviewError> for ApiError {
    fn from(error: ReviewError) -> Self {
        let status = match error {
            ReviewError::Invalid(_) => StatusCode::UNPROCESSABLE_ENTITY,
            ReviewError::ReviewerIsAuthor => StatusCode::FORBIDDEN,
            ReviewError::TaskNotFound(_)
            | ReviewError::ReviewNotFound(_)
            | ReviewError::FindingNotFound(_)
            | ReviewError::DeviationNotFound(_)
            | ReviewError::DesignNotFound(_) => StatusCode::NOT_FOUND,
            ReviewError::SpecOutdated { .. }
            | ReviewError::ReuseRejected { .. }
            | ReviewError::FindingResolved(_)
            | ReviewError::DeviationResolved(_)
            | ReviewError::DesignOutdated { .. } => StatusCode::CONFLICT,
        };
        ApiError { status, code: error.code(), message: error.to_string() }
    }
}

impl From<ReviewApplyError> for ApiError {
    fn from(error: ReviewApplyError) -> Self {
        match error {
            ReviewApplyError::Rejected(rejected) => rejected.into(),
            ReviewApplyError::Stale => ApiError {
                status: StatusCode::CONFLICT,
                code: "stale_report",
                message: "這個 Run 已不是目前的寫入所有者（Run 已結束或已被接替），沒有改變任何狀態".to_string(),
            },
            ReviewApplyError::Store(store) => store.into(),
        }
    }
}

async fn workspace_path(state: &ApiState, id: &str) -> Result<std::path::PathBuf, ApiError> {
    state
        .store
        .list_workspaces()
        .await?
        .into_iter()
        .find(|w| w.id == id)
        .map(|w| std::path::PathBuf::from(w.path))
        .ok_or_else(|| ApiError { status: StatusCode::NOT_FOUND, code: "workspace_not_found", message: format!("找不到 Workspace {id}") })
}

/// 依決策權限表判定 Controller 的動作（事項與對象由 Runtime 推導）。
#[allow(clippy::too_many_arguments)]
pub(crate) async fn decide(
    state: &ApiState,
    scope: &ControllerScope,
    matter: Matter,
    task_id: &str,
    subject: &str,
    summary: &str,
    reason: &str,
    evidence: Vec<String>,
) -> Result<ControllerOutcome, ApiError> {
    gate(state, scope, action_content(matter, task_id, subject, summary, reason, evidence)?).await
}

fn action_content(matter: Matter, task_id: &str, subject: &str, summary: &str, reason: &str, evidence: Vec<String>) -> Result<ActionContent, ApiError> {
    Ok(ActionContent::new(matter, Some(task_id), summary, reason, evidence, None).map_err(DecisionApplyError::from)?.with_subject(subject))
}

async fn gate(state: &ApiState, scope: &ControllerScope, content: ActionContent) -> Result<ControllerOutcome, ApiError> {
    let outcome = state.store.controller_gate(scope, new_id("decision")?, new_id("authz")?, content, state.clock.now()).await?;
    let DecisionOutcome::Controller { outcome, .. } = outcome else { unreachable!("ControllerGate 只會得到 Controller 結果") };
    Ok(outcome)
}

pub(crate) fn requires_decision(matter: Matter, request_id: String, existing: bool, what: &str) -> ControllerToolError {
    ControllerToolError::RequiresDecision {
        matter,
        message: format!(
            "依決策權限表，「{}」必須由使用者裁定：已建立裁定請求 {request_id}，{what}。\
             使用者回答後，背景執行的 wait 會回報「裁定已回答」事件通知你，屆時以相同內容再呼叫一次這個工具套用結果；不要自行繞過。",
            matter.label()
        ),
        decision_request_id: request_id,
        existing,
    }
}

/// 依決策權限表，這個事項只能由使用者發起：Controller 的動作被拒絕，也不建立裁定請求。
pub(crate) fn human_only(message: &str) -> ControllerToolError {
    ApiError { status: StatusCode::FORBIDDEN, code: "human_only", message: message.to_string() }.into()
}

pub(crate) fn clip(text: &str, max: usize) -> String {
    if text.chars().count() > max {
        format!("{}…", text.chars().take(max).collect::<String>())
    } else {
        text.to_string()
    }
}

// ---------------------------------------------------------------- 管理 API

pub(crate) async fn list(State(state): State<Arc<ApiState>>, UrlPath(id): UrlPath<String>) -> Result<Json<ReviewList>, ApiError> {
    workspace_path(&state, &id).await?;
    let data = state.store.reviews(&id, None).await?;
    Ok(Json(list_view(id, data)))
}

// ---------------------------------------------------------------- Worker 工具

#[derive(Debug, Deserialize)]
pub(crate) struct SubmitReviewRequest {
    #[serde(flatten)]
    submission: ReviewSubmission,
    #[serde(flatten)]
    claims: Claims,
}

pub(crate) async fn submit_review(
    State(state): State<Arc<ApiState>>,
    Extension(record): Extension<CredentialRecord>,
    body: Result<Json<SubmitReviewRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<ReviewView>), ApiError> {
    let Json(request) = body.map_err(bad_json)?;
    let scope = scope_of(&record, &request.claims)?;
    let input = request.submission;
    let candidate = input.candidate_revision.trim().to_string();

    // 要沿用的先前審查：在交易外以 Git 算出它的 candidate 到這次 candidate 之間變更的檔案
    let mut changed = HashMap::new();
    for axis_input in [&input.standards, &input.spec] {
        let Some(from) = axis_input.reuse_from.as_deref().map(str::trim).filter(|f| !f.is_empty()) else { continue };
        if changed.contains_key(from) {
            continue;
        }
        let Some(prior) = state.store.review(&scope.workspace_id, from).await? else { continue };
        let paths = if prior.candidate_revision == candidate {
            Ok(Vec::new())
        } else {
            let repo = workspace_path(&state, &scope.workspace_id).await?;
            state.workspace_fs.changed_paths(&repo, &prior.candidate_revision, &candidate).await.map_err(|e| e.to_string())
        };
        changed.insert(from.to_string(), paths);
    }
    let count = [&input.standards, &input.spec].iter().map(|a| a.findings.len()).sum::<usize>().min(review::FINDINGS_MAX * 2);
    let finding_ids = (0..count).map(|_| new_id("fnd")).collect::<Result<Vec<_>, _>>()?;
    let id = new_id("rev")?;
    // finding 數量超過上限時 Domain 會拒絕；識別不夠用之前就會被拒絕
    let record = state.store.submit_review(&scope, input, changed, id, finding_ids, state.clock.now()).await?;
    Ok((StatusCode::CREATED, Json(review_view(record))))
}

#[derive(Debug, Deserialize)]
pub(crate) struct ReportDeviationRequest {
    #[serde(flatten)]
    input: DeviationInput,
    #[serde(flatten)]
    claims: Claims,
}

async fn deviation_view(state: &ApiState, deviation: DesignDeviation) -> Result<DeviationView, ApiError> {
    let data = state.store.reviews(&deviation.workspace_id, Some(&deviation.task_id)).await?;
    let (design_name, task_title) =
        data.deviations.into_iter().find(|(d, _, _)| d.id == deviation.id).map(|(_, n, t)| (n, t)).unwrap_or_default();
    Ok(DeviationView { deviation, design_name, task_title })
}

pub(crate) async fn report_design_deviation(
    State(state): State<Arc<ApiState>>,
    Extension(record): Extension<CredentialRecord>,
    body: Result<Json<ReportDeviationRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<DeviationView>), ApiError> {
    let Json(request) = body.map_err(bad_json)?;
    let scope = scope_of(&record, &request.claims)?;
    let deviation = state.store.report_deviation(&scope, request.input, new_id("dev")?, state.clock.now()).await?;
    Ok((StatusCode::CREATED, Json(deviation_view(&state, deviation).await?)))
}

// ---------------------------------------------------------------- Controller 工具

#[derive(Debug, Deserialize)]
pub(crate) struct ClassifyFindingRequest {
    finding_id: String,
    classification: Classification,
    #[serde(default)]
    reason: String,
    #[serde(default)]
    evidence: Vec<String>,
    #[serde(flatten)]
    claims: Claims,
}

pub(crate) async fn classify_review_finding(
    State(state): State<Arc<ApiState>>,
    Extension(record): Extension<CredentialRecord>,
    body: Result<Json<ClassifyFindingRequest>, JsonRejection>,
) -> Result<Json<ClassifyFindingResponse>, ControllerToolError> {
    crate::controller::require_controller(&record)?;
    let Json(request) = body.map_err(bad_json)?;
    let scope = controller_scope(&state, &record, &request.claims).await?;
    let finding_id = request.finding_id.trim().to_string();
    let Some((finding, owner)) = state.store.finding(&scope.workspace_id, &finding_id).await.map_err(ApiError::from)? else {
        return Err(ApiError::from(ReviewError::FindingNotFound(finding_id)).into());
    };
    if finding.state == FindingState::Dismissed {
        return Err(ApiError::from(ReviewError::FindingResolved(finding.id)).into());
    }
    if request.reason.trim().is_empty() {
        return Err(ApiError::from(ReviewError::Invalid("請在 reason 說明分類的理由與證據".into())).into());
    }
    let subject_task = owner.review.subject_task_id.clone();
    let label = format!("{}finding「{}」（審查 {}）", finding.axis.label(), clip(&finding.title, 80), finding.review_id);
    let now = state.clock.now();

    let (change, decision_request_id, decided_by, message) = match request.classification {
        Classification::Fix => {
            let summary = format!("把{label}分類為修正");
            match decide(&state, &scope, Matter::ClassifyFindingAsFix, &subject_task, &finding.id, &summary, &request.reason, request.evidence).await? {
                ControllerOutcome::Allowed | ControllerOutcome::Approved { .. } => (
                    FindingChange { state: FindingState::Fix, classification: Classification::Fix, reason: request.reason.clone() },
                    None,
                    None,
                    "已分類為修正：修正後請對新的 candidate 重新審查（未受影響的軸可以沿用）".to_string(),
                ),
                ControllerOutcome::RequiresDecision { request_id, existing } => {
                    return Err(requires_decision(Matter::ClassifyFindingAsFix, request_id, existing, "分類沒有套用"));
                }
                ControllerOutcome::Denied { request_id, note } => {
                    return Err(ApiError {
                        status: StatusCode::CONFLICT,
                        code: "decision_denied",
                        message: format!(
                            "使用者沒有核准把這項 finding 分類為修正（裁定請求 {request_id}）{}",
                            note.map(|n| format!("：{n}")).unwrap_or_default()
                        ),
                    }
                    .into());
                }
                ControllerOutcome::HumanOnly => {
                    return Err(human_only("這個 Workspace 的決策權限表規定只有使用者能把 review finding 分類為修正"));
                }
            }
        }
        classification @ (Classification::Dismiss | Classification::NeedsDecision) => {
            let summary = match classification {
                Classification::Dismiss => format!("駁回{label}"),
                _ => format!("{label}需要你決定：核准即駁回（不修正），不核准即修正"),
            };
            let mut content = action_content(Matter::DismissReviewFinding, &subject_task, &finding.id, &summary, &request.reason, request.evidence)?;
            // Controller 判斷不了的 finding 一律交給使用者：駁回放寬為可自主處理時仍建立裁定請求（只能人工發起時照樣拒絕）
            if classification == Classification::NeedsDecision {
                content = content.ask_user();
            }
            match gate(&state, &scope, content).await? {
                // 使用者把駁回放寬為可自主處理：等同核准，直接駁回；駁回一定要記下裁定與裁定者，兩者都記為這個 Controller Run
                ControllerOutcome::Allowed => {
                    let run = format!("controller:{}", scope.controller_run_id);
                    (
                        FindingChange { state: FindingState::Dismissed, classification, reason: request.reason.clone() },
                        Some(run.clone()),
                        Some(run),
                        "依決策權限表自主駁回：這項 finding 已解決".to_string(),
                    )
                }
                ControllerOutcome::RequiresDecision { request_id, existing } => {
                    if finding.state != FindingState::AwaitingDecision || finding.decision_request_id.as_deref() != Some(&request_id) {
                        let change = FindingChange { state: FindingState::AwaitingDecision, classification, reason: request.reason.clone() };
                        state.store.classify_finding(&scope.workspace_id, &finding.id, change, Some(request_id.clone()), None, &scope.controller_run_id, now).await.map_err(ApiError::from)?;
                    }
                    let what = "finding 標為等待裁定，在使用者同意駁回之前這一軸不會通過";
                    if classification == Classification::Dismiss {
                        return Err(requires_decision(Matter::DismissReviewFinding, request_id, existing, what));
                    }
                    // 權限表可能已放寬，不能說成「依決策權限表必須由使用者裁定」
                    return Err(ControllerToolError::RequiresDecision {
                        matter: Matter::DismissReviewFinding,
                        message: format!(
                            "你判斷不了的 finding 一律由使用者裁定：已建立裁定請求 {request_id}，{what}。\
                             使用者回答後，背景執行的 wait 會回報「裁定已回答」事件通知你，屆時以相同內容再呼叫一次這個工具套用結果；不要自行繞過。"
                        ),
                        decision_request_id: request_id,
                        existing,
                    });
                }
                ControllerOutcome::Approved { request_id, .. } => (
                    FindingChange { state: FindingState::Dismissed, classification, reason: request.reason.clone() },
                    Some(request_id),
                    Some(DECIDED_BY_DESKTOP.to_string()),
                    "使用者同意駁回：這項 finding 已解決".to_string(),
                ),
                ControllerOutcome::Denied { request_id, note } => (
                    FindingChange {
                        state: FindingState::Fix,
                        classification: Classification::Fix,
                        reason: format!("使用者不同意駁回{}；原理由：{}", note.map(|n| format!("（{n}）")).unwrap_or_default(), request.reason),
                    },
                    Some(request_id),
                    Some(DECIDED_BY_DESKTOP.to_string()),
                    "使用者不同意駁回：已改為修正，修正後請重新審查".to_string(),
                ),
                ControllerOutcome::HumanOnly => {
                    return Err(human_only("這個 Workspace 的決策權限表規定只有使用者能駁回 review finding"));
                }
            }
        }
    };
    let (finding, owner) = state
        .store
        .classify_finding(&scope.workspace_id, &finding.id, change, decision_request_id, decided_by, &scope.controller_run_id, now)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(ClassifyFindingResponse { finding, review: review_view(owner), message }))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DeviationDecision {
    Accept,
    Reject,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ResolveDeviationRequest {
    deviation_id: String,
    resolution: DeviationDecision,
    #[serde(default)]
    reason: String,
    #[serde(flatten)]
    claims: Claims,
}

pub(crate) async fn resolve_design_deviation(
    State(state): State<Arc<ApiState>>,
    Extension(record): Extension<CredentialRecord>,
    body: Result<Json<ResolveDeviationRequest>, JsonRejection>,
) -> Result<Json<ResolveDeviationResponse>, ControllerToolError> {
    crate::controller::require_controller(&record)?;
    let Json(request) = body.map_err(bad_json)?;
    let scope = controller_scope(&state, &record, &request.claims).await?;
    let id = request.deviation_id.trim().to_string();
    let Some((deviation, design_latest)) = state.store.deviation(&scope.workspace_id, &id).await.map_err(ApiError::from)? else {
        return Err(ApiError::from(ReviewError::DeviationNotFound(id)).into());
    };
    if deviation.status.is_final() {
        return Err(ApiError::from(ReviewError::DeviationResolved(deviation.id)).into());
    }
    if request.reason.trim().is_empty() {
        return Err(ApiError::from(ReviewError::Invalid("請在 reason 說明處理的理由".into())).into());
    }
    let now = state.clock.now();
    let resolve = |status: DeviationStatus, reason: String, decision: Option<String>| DeviationResolution {
        status,
        reason,
        decision_request_id: decision,
        accepted_by: None,
        accepted_design_version: None,
    };
    let (resolved, message) = match request.resolution {
        DeviationDecision::Reject => {
            let resolved = state
                .store
                .resolve_deviation(&scope.workspace_id, &deviation.id, resolve(DeviationStatus::Rejected, request.reason, None), &scope.controller_run_id, now)
                .await
                .map_err(ApiError::from)?;
            (resolved, "已拒絕偏離：實作必須依照目前的設計".to_string())
        }
        DeviationDecision::Accept => {
            let current = design_latest.unwrap_or(0);
            if current != deviation.design_version {
                return Err(ApiError::from(ReviewError::DesignOutdated { reported: deviation.design_version, current }).into());
            }
            let summary = format!(
                "接受設計偏離 {}：{}（設計第 {} 版 → 第 {} 版）",
                deviation.id,
                clip(&deviation.deviation, 200),
                deviation.design_version,
                deviation.design_version + 1
            );
            let evidence = deviation.affected_contracts.iter().map(|c| format!("受影響契約：{}", clip(c, 400))).take(10).collect();
            match decide(&state, &scope, Matter::AcceptDesignDeviation, &deviation.task_id, &deviation.id, &summary, &request.reason, evidence).await? {
                ControllerOutcome::RequiresDecision { request_id, existing } => {
                    if deviation.decision_request_id.as_deref() != Some(&request_id) {
                        state
                            .store
                            .resolve_deviation(
                                &scope.workspace_id,
                                &deviation.id,
                                resolve(DeviationStatus::AwaitingDecision, request.reason.clone(), Some(request_id.clone())),
                                &scope.controller_run_id,
                                now,
                            )
                            .await
                            .map_err(ApiError::from)?;
                    }
                    return Err(requires_decision(Matter::AcceptDesignDeviation, request_id, existing, "偏離標為等待裁定，設計還沒有更新"));
                }
                outcome @ (ControllerOutcome::Allowed | ControllerOutcome::Approved { .. }) => {
                    // 使用者把接受偏離放寬為可自主處理時等同核准；接受的偏離一定要記下裁定與接受者，兩者都記為這個 Controller Run
                    let (decision, accepted_by, who) = match outcome {
                        ControllerOutcome::Approved { request_id, .. } => (request_id, DECIDED_BY_DESKTOP.to_string(), "使用者已接受偏離"),
                        _ => {
                            let run = format!("controller:{}", scope.controller_run_id);
                            (run.clone(), run, "依決策權限表自主接受偏離")
                        }
                    };
                    // 新設計寫成新的檔案，再在同一個交易中替設計成果追加版本
                    let bytes = deviation.proposed_design.clone().into_bytes();
                    let storage_path = artifact::blob_storage_path(&scope.workspace_id, &new_id("blob")?);
                    state
                        .workspace_fs
                        .write_artifact_atomic(&artifact_root(&state).join(&storage_path), &bytes)
                        .await
                        .map_err(storage_unavailable)?;
                    let blob = (storage_path, bytes.len() as i64, artifact::sha256_hex(&bytes));
                    let resolved = state
                        .store
                        .accept_deviation(
                            &scope.workspace_id,
                            &deviation.id,
                            request.reason,
                            decision,
                            accepted_by,
                            &scope.controller_run_id,
                            blob,
                            now,
                        )
                        .await
                        .map_err(ApiError::from)?;
                    let version = resolved.accepted_design_version.unwrap_or_default();
                    (resolved, format!("{who}：設計更新為第 {version} 版，之後派出的 Worker 會拿到新版本"))
                }
                ControllerOutcome::Denied { request_id, note } => {
                    let reason = format!("使用者不接受偏離{}；原理由：{}", note.map(|n| format!("（{n}）")).unwrap_or_default(), request.reason);
                    let resolved = state
                        .store
                        .resolve_deviation(
                            &scope.workspace_id,
                            &deviation.id,
                            resolve(DeviationStatus::Rejected, reason, Some(request_id)),
                            &scope.controller_run_id,
                            now,
                        )
                        .await
                        .map_err(ApiError::from)?;
                    (resolved, "使用者不接受偏離：實作必須依照目前的設計".to_string())
                }
                ControllerOutcome::HumanOnly => {
                    return Err(human_only("這個 Workspace 的決策權限表規定只有使用者能接受設計偏離"));
                }
            }
        }
    };
    Ok(Json(ResolveDeviationResponse { deviation: deviation_view(&state, resolved).await?, message }))
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct GetReviewsRequest {
    #[serde(default)]
    task_id: Option<String>,
    #[serde(flatten)]
    claims: Claims,
}

pub(crate) async fn get_reviews(
    State(state): State<Arc<ApiState>>,
    Extension(record): Extension<CredentialRecord>,
    body: Result<Json<GetReviewsRequest>, JsonRejection>,
) -> Result<Json<ReviewList>, ApiError> {
    crate::controller::require_controller(&record)?;
    let Json(request) = body.map_err(bad_json)?;
    let scope = controller_scope(&state, &record, &request.claims).await?;
    let task = request.task_id.as_deref().map(str::trim).filter(|t| !t.is_empty());
    let data = state.store.reviews(&scope.workspace_id, task).await?;
    Ok(Json(list_view(scope.workspace_id, data)))
}
