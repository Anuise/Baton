//! 探索測試到可重現缺陷的端點（票 47，issue #3 §10.3）。
//!
//! - 管理 API（桌面憑證）：Workspace 的探索概況（唯讀）：各探索 Task 目前的章程，以及疑點、已確認缺陷、未驗風險、
//!   已排除與已修復的疑點和它們的重現紀錄。
//! - Controller 工具：`define_exploration_charter`（定下或改版章程）、`triage_exploration_finding`（排除、改為未驗風險、
//!   重新開啟；排除為預期設計依決策權限表送裁定）、`reproduce_exploration_finding`（提出重現案例或驗證修復，
//!   由 Runtime 排入驗證要求）、`get_exploration`。
//! - Worker 工具：`report_exploration_finding`（在自己 Task 的章程下回報疑點，一律先是 suspected）。

use baton_domain::exploration::{Charter, ExplorationFinding, ReproductionAttempt};
use baton_domain::verification_profile::VerificationRequest;
use serde::{Deserialize, Serialize};

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
