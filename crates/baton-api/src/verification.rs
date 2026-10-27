//! 成果接受與驗證的端點（桌面角色專用，票 13）：檢視一個 Task 的驗證狀況、使用者手動接受成果。
//!
//! 規則都在 Domain（`baton_domain::verification`）；端點只做「解析請求 → 交給 Store → 對應成穩定的錯誤代碼」。
//! 手動接受的接受者取自 guard 已驗證的桌面憑證（只記錄指紋），請求不能指定；Agent 憑證在 guard 就被拒絕。

use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use baton_domain::auth::CredentialRecord;
use baton_domain::authority::Matter;
use baton_domain::decision::{ControllerOutcome, EVIDENCE_MAX_CHARS, EVIDENCE_MAX_ITEMS, SUMMARY_MAX_CHARS};
use baton_domain::task::TaskStatus;
use baton_domain::verification::{AcceptanceBasis, AcceptanceCommand, AcceptanceError, VerificationView};
use baton_store::AcceptanceApplyError;
use serde::{Deserialize, Serialize};

use crate::agent::Claims;
use crate::authorization::bad_json;
use crate::controller::{controller_scope, ControllerToolError};
use crate::reviews::{clip, decide, requires_decision};
use crate::workspaces::{new_id, ApiError};
use crate::ApiState;

pub(crate) const VERIFICATION_ROUTE: &str = "/api/v1/workspaces/{id}/tasks/{task_id}/verification";
pub(crate) const ACCEPT_ROUTE: &str = "/api/v1/workspaces/{id}/tasks/{task_id}/verification/accept";

/// 接受者紀錄使用的憑證指紋長度（憑證雜湊的前幾個十六進位字元）。
const FINGERPRINT_CHARS: usize = 16;

/// 路徑；Runtime 與用戶端共用。
pub fn task_verification_path(workspace_id: &str, task_id: &str) -> String {
    format!("{}/verification", crate::tasks::task_path(workspace_id, task_id))
}

pub fn task_accept_path(workspace_id: &str, task_id: &str) -> String {
    format!("{}/accept", task_verification_path(workspace_id, task_id))
}

/// 手動接受的請求：`basis` 必須原樣送回檢視時拿到的接受基礎。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcceptTaskRequest {
    pub basis: AcceptanceBasis,
    #[serde(default)]
    pub note: Option<String>,
}

fn rejected(error: AcceptanceError) -> ApiError {
    let status = match &error {
        AcceptanceError::WorkspaceNotFound(_) | AcceptanceError::TaskNotFound(_) => StatusCode::NOT_FOUND,
        AcceptanceError::NoteTooLong => StatusCode::BAD_REQUEST,
        AcceptanceError::WorkspaceArchived(_)
        | AcceptanceError::TaskNotVerifying(_)
        | AcceptanceError::RunStale { .. }
        | AcceptanceError::ContractStale { .. }
        | AcceptanceError::ManifestStale
        | AcceptanceError::ResultsMismatch
        | AcceptanceError::AlreadyQueuedForIntegration
        | AcceptanceError::WorktreeMissing => StatusCode::CONFLICT,
    };
    ApiError { status, code: error.code(), message: error.to_string() }
}

impl From<AcceptanceApplyError> for ApiError {
    fn from(error: AcceptanceApplyError) -> Self {
        match error {
            AcceptanceApplyError::Rejected(error) => rejected(error),
            AcceptanceApplyError::Store(store) => store.into(),
        }
    }
}

pub(crate) async fn view(
    State(state): State<Arc<ApiState>>,
    UrlPath((id, task_id)): UrlPath<(String, String)>,
) -> Result<Json<VerificationView>, ApiError> {
    Ok(Json(state.store.task_verification(&id, &task_id).await?))
}

/// 使用者手動接受：成果接受交易同時校驗 Run 世代、契約版本與成果 manifest，通過後回傳最新的驗證狀況。
pub(crate) async fn accept(
    State(state): State<Arc<ApiState>>,
    Extension(record): Extension<CredentialRecord>,
    UrlPath((id, task_id)): UrlPath<(String, String)>,
    body: Result<Json<AcceptTaskRequest>, JsonRejection>,
) -> Result<Json<VerificationView>, ApiError> {
    let Json(request) = body.map_err(bad_json)?;
    let accepted_by = record.token_hash.chars().take(FINGERPRINT_CHARS).collect();
    let command =
        AcceptanceCommand::AcceptManually { task_id: task_id.clone(), basis: request.basis, accepted_by, note: request.note };
    state.store.apply_acceptance(id.clone(), command, new_id("ver")?, state.clock.now()).await?;
    Ok(Json(state.store.task_verification(&id, &task_id).await?))
}

// ---------------------------------------------------------------- Controller 工具：accept_artifact（票 54）

#[derive(Debug, Deserialize)]
pub(crate) struct AcceptArtifactRequest {
    task_id: String,
    #[serde(default)]
    reason: String,
    #[serde(default)]
    note: Option<String>,
    #[serde(flatten)]
    claims: Claims,
}

fn tool_error(status: StatusCode, code: &'static str, message: String) -> ControllerToolError {
    ApiError { status, code, message }.into()
}

/// Controller 接受一個 Task 的成果（spec D-05，仍受驗證 Gate 約束）。
///
/// - 契約宣告了確定性檢查（Runtime 會自動執行）：只有檢查全部通過才接受，Controller 不能以接受取代檢查。
/// - 沒有宣告檢查（原本只能由使用者手動接受）：Controller 的接受等同略過驗證 Gate，依決策權限表一定要送裁定；
///   使用者核准後以與手動接受相同的接受交易完成，所以 Run 世代、契約版本與成果 manifest 一樣會被校驗。
///   裁定對象綁定接受基礎，核准只能用在使用者看到的那一份成果。
pub(crate) async fn accept_artifact(
    State(state): State<Arc<ApiState>>,
    Extension(record): Extension<CredentialRecord>,
    body: Result<Json<AcceptArtifactRequest>, JsonRejection>,
) -> Result<Json<VerificationView>, ControllerToolError> {
    crate::controller::require_controller(&record)?;
    let Json(request) = body.map_err(bad_json)?;
    let scope = controller_scope(&state, &record, &request.claims).await?;
    let task_id = request.task_id.trim().to_string();
    // 只在這個 Controller 的 Workspace 內查找：別的 Workspace 的 Task 拿不到也動不到
    let view = state.store.task_verification(&scope.workspace_id, &task_id).await.map_err(ApiError::from)?;
    let basis = match (view.status, &view.basis) {
        (TaskStatus::Verifying, Some(basis)) => basis.clone(),
        (status, _) => return Err(rejected(AcceptanceError::TaskNotVerifying(status)).into()),
    };
    if view.automatic {
        return Err(tool_error(
            StatusCode::CONFLICT,
            "verification_gate_pending",
            "這個 Task 的契約宣告了確定性檢查：Runtime 會執行檢查，全部通過才接受成果；\
             Controller 不能以接受取代檢查。請等待驗證結果（未通過時 Task 會回到可修正狀態）。"
                .to_string(),
        ));
    }

    let title = state.store.task_detail(&scope.workspace_id, &task_id).await?.task.task.title;
    let names = view.artifacts.iter().map(|a| a.name.as_str()).collect::<Vec<_>>();
    let subject = format!(
        "accept:{}:g{}:c{}:{}",
        basis.run_id,
        basis.generation,
        basis.contract_version,
        view.artifacts.iter().map(|a| format!("{}@{}", a.name, a.sha256)).collect::<Vec<_>>().join(",")
    );
    let summary = format!(
        "接受 Task「{}」的成果（Run {} 第 {} 代，成果：{}）",
        clip(&title, 80),
        basis.run_id,
        basis.generation,
        if names.is_empty() { "（沒有登記成果）".to_string() } else { names.join("、") }
    );
    let summary = clip(&summary, SUMMARY_MAX_CHARS - 1);
    let evidence = view
        .artifacts
        .iter()
        .take(EVIDENCE_MAX_ITEMS)
        .map(|a| clip(&format!("成果「{}」第 {} 版，SHA-256 為 {}", a.name, a.version, a.sha256), EVIDENCE_MAX_CHARS - 1))
        .collect();
    let matter = Matter::SkipStageOrGate;
    let accepted_by = match decide(&state, &scope, matter, &task_id, &subject, &summary, &request.reason, evidence).await? {
        ControllerOutcome::Allowed => format!("controller:{}", scope.controller_run_id),
        ControllerOutcome::Approved { request_id, .. } => format!("decision:{request_id}"),
        ControllerOutcome::RequiresDecision { request_id, existing } => {
            return Err(requires_decision(matter, request_id, existing, "成果沒有接受，Task 仍在驗證中"));
        }
        ControllerOutcome::Denied { request_id, note } => {
            return Err(tool_error(
                StatusCode::CONFLICT,
                "decision_denied",
                format!(
                    "使用者沒有核准接受這個 Task 的成果（裁定請求 {request_id}）{}；Task 仍在驗證中",
                    note.map(|n| format!("：{n}")).unwrap_or_default()
                ),
            ));
        }
        ControllerOutcome::HumanOnly => {
            return Err(tool_error(StatusCode::FORBIDDEN, "human_only", "接受成果只能由使用者從桌面發起".to_string()));
        }
    };
    let note = request.note.filter(|n| !n.trim().is_empty()).or(Some(request.reason));
    let command = AcceptanceCommand::AcceptManually { task_id: task_id.clone(), basis, accepted_by, note };
    state
        .store
        .apply_acceptance(scope.workspace_id.clone(), command, new_id("ver")?, state.clock.now())
        .await
        .map_err(ApiError::from)?;
    Ok(Json(state.store.task_verification(&scope.workspace_id, &task_id).await.map_err(ApiError::from)?))
}
