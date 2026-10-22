//! 探索測試到可重現缺陷（票 47，issue #3 §10.3、§15.1 第 5、9 點）。
//!
//! 純邏輯、無 I/O。
//! - 探索 Task 先定章程（charter）：區域、使用者角色、目標、禁止的副作用、步數與成本上限。章程只追加版本；
//!   沒有章程的 Task 不能回報疑點。章程交給 Worker 的 Prompt，上限也套用在 Runtime 執行的重現上。
//! - 探索結果一律先是 `suspected`（疑點），保留重現步驟、預期、實際觀察、環境與證據。
//! - 疑點只有一條路升為 `confirmed`（已確認缺陷）：Controller 提出對應原症狀的重現案例（一條與 Profile 旅程同格式的
//!   旅程），Runtime 在 Verification Profile 的環境中執行它，並同時執行代表旅程作為對照。對照全部通過、而且案例以
//!   產品行為斷言失敗（`product_failure`）才算確認。環境、授權、證據不足、預算用盡都不是產品缺陷，疑點維持原狀；
//!   案例通過（無法重現）時保留為 `unverified_risk`，不自動駁回。
//! - 已確認的缺陷不能排除、不能降為未驗風險，確認時的案例也不能更換；修復只能以同一個案例通過來證明（`fixed`），
//!   之後這個案例納入正常回歸：每一次驗證都會執行它，失敗就是 `regressed`，同樣留在回歸範圍內。
//! - 失敗中的重現案例只用於診斷：重現與修復驗證的 Bundle 不能作為 Gate 的驗證證據，未修復的案例也不加入一般驗證。
//! - 排除疑點要附原因（環境、seed data、工具限制、預期設計）；「預期設計」是產品取捨，必須由使用者裁定。
//!   排除的疑點仍保留並可重新開啟，任何疑點都不能刪除。

use serde::{Deserialize, Serialize};

use crate::event::DomainEvent;
use crate::verification_profile::{self as vp, EvidenceBundle, Journey, Operation, ProfileDefinition, Verdict};
use crate::Timestamp;

pub const TOOL_DEFINE_EXPLORATION_CHARTER: &str = "define_exploration_charter";
pub const TOOL_REPORT_EXPLORATION_FINDING: &str = "report_exploration_finding";
pub const TOOL_TRIAGE_EXPLORATION_FINDING: &str = "triage_exploration_finding";
pub const TOOL_REPRODUCE_EXPLORATION_FINDING: &str = "reproduce_exploration_finding";
pub const TOOL_GET_EXPLORATION: &str = "get_exploration";

pub const CHARTER_AGGREGATE: &str = "exploration_charter";
pub const FINDING_AGGREGATE: &str = "exploration_finding";
/// 一次重現（驗證要求 × 疑點）：排入是第 1 版，結果是第 2 版。
pub const ATTEMPT_AGGREGATE: &str = "exploration_attempt";
pub const EVENT_CHARTER_DEFINED: &str = "exploration_charter_defined";
pub const EVENT_FINDING_REPORTED: &str = "exploration_finding_reported";
pub const EVENT_FINDING_STATUS_CHANGED: &str = "exploration_finding_status_changed";
pub const EVENT_REPRODUCTION_REQUESTED: &str = "exploration_reproduction_requested";
pub const EVENT_REPRODUCTION_JUDGED: &str = "exploration_reproduction_judged";

/// 章程的步數上限最多可以設多少（Runtime 執行重現時的操作次數上限）。
pub const MAX_STEPS_LIMIT: u32 = 1_000;
/// 章程的成本上限（模型呼叫次數）最多可以設多少。
pub const MAX_MODEL_CALLS_LIMIT: u32 = 100;
/// 重現案例的旅程識別前綴；Profile 的旅程識別不能含 `.`，所以不會撞名。
pub const CASE_KEY_PREFIX: &str = "finding.";

/// 重現案例在驗證要求與 Evidence Bundle 中的旅程識別。
pub fn case_key(finding_id: &str) -> String {
    format!("{CASE_KEY_PREFIX}{finding_id}")
}

/// 旅程識別是重現案例時，回傳它所屬的疑點。
pub fn finding_of_case(key: &str) -> Option<&str> {
    key.strip_prefix(CASE_KEY_PREFIX)
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ExplorationError {
    #[error("探索紀錄不正確：{0}")]
    Invalid(String),
    #[error("Task {0} 還沒有探索章程；請先以 define_exploration_charter 定下區域、角色、目標、禁止的副作用與上限")]
    CharterRequired(String),
    #[error("找不到探索疑點 {0}")]
    FindingNotFound(String),
    #[error("{0}")]
    DefectLocked(String),
    #[error("{0}")]
    TransitionRefused(String),
    #[error("疑點 {0} 已有排隊中的重現，請等結果出來再要求重現或分流")]
    ReproductionPending(String),
    /// 排除為「預期設計」沒有使用者的裁定（API 依決策權限表先送裁定）。
    #[error("排除為預期設計必須由使用者裁定")]
    DecisionRequired,
}

impl ExplorationError {
    pub fn code(&self) -> &'static str {
        match self {
            ExplorationError::Invalid(_) => "exploration_invalid",
            ExplorationError::CharterRequired(_) => "exploration_charter_required",
            ExplorationError::FindingNotFound(_) => "exploration_finding_not_found",
            ExplorationError::DefectLocked(_) => "exploration_defect_locked",
            ExplorationError::TransitionRefused(_) => "exploration_transition_refused",
            ExplorationError::ReproductionPending(_) => "exploration_reproduction_pending",
            ExplorationError::DecisionRequired => "requires_decision",
        }
    }
}

fn invalid(message: impl Into<String>) -> ExplorationError {
    ExplorationError::Invalid(message.into())
}

/// 去掉前後空白；必填時不能空白，且不超過長度上限。
fn text(raw: &str, label: &str, required: bool) -> Result<String, ExplorationError> {
    let trimmed = raw.trim();
    if required && trimmed.is_empty() {
        return Err(invalid(format!("{label}不能空白")));
    }
    if trimmed.chars().count() > vp::TEXT_MAX_CHARS {
        return Err(invalid(format!("{label}最多 {} 個字元", vp::TEXT_MAX_CHARS)));
    }
    Ok(trimmed.to_string())
}

/// 文字清單：每一項都不能空白；`required` 時至少一項。
fn texts(raw: &[String], label: &str, required: bool) -> Result<Vec<String>, ExplorationError> {
    if required && raw.is_empty() {
        return Err(invalid(format!("{label}至少要有一項")));
    }
    if raw.len() > vp::LIST_MAX {
        return Err(invalid(format!("{label}最多 {} 項", vp::LIST_MAX)));
    }
    raw.iter().map(|item| text(item, label, true)).collect()
}

// ---------------------------------------------------------------- 章程

/// 定義（或改版）探索章程的輸入。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CharterInput {
    pub task_id: String,
    /// 要探索的區域。
    pub area: String,
    /// 扮演的使用者角色。
    pub role: String,
    /// 這次探索的目標。
    pub goal: String,
    /// 禁止的副作用（例如「不得送出真實付款」）。
    pub forbidden_side_effects: Vec<String>,
    /// 步數上限：Worker 探索的操作步數，以及 Runtime 執行這個章程下的重現時的操作次數上限。
    pub max_steps: u32,
    /// 成本上限：模型呼叫次數（Runtime 執行重現時的語意判斷次數上限）。
    pub max_model_calls: u32,
}

/// 探索章程的一個版本（只追加）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Charter {
    pub id: String,
    pub workspace_id: String,
    pub task_id: String,
    pub version: i64,
    pub area: String,
    pub role: String,
    pub goal: String,
    pub forbidden_side_effects: Vec<String>,
    pub max_steps: u32,
    pub max_model_calls: u32,
    /// 定義者：Controller 那一輪的識別。
    pub defined_by: String,
    pub created_at: Timestamp,
}

impl Charter {
    fn same_content(&self, other: &Charter) -> bool {
        (&self.area, &self.role, &self.goal, &self.forbidden_side_effects, self.max_steps, self.max_model_calls)
            == (&other.area, &other.role, &other.goal, &other.forbidden_side_effects, other.max_steps, other.max_model_calls)
    }
}

/// 定義章程：內容與目前版本相同時不追加（回 `None`）。
pub fn define_charter(
    current: Option<&Charter>,
    workspace_id: &str,
    input: CharterInput,
    defined_by: String,
    id: String,
    now: Timestamp,
) -> Result<Option<(Charter, DomainEvent)>, ExplorationError> {
    if input.max_steps == 0 || input.max_steps > MAX_STEPS_LIMIT {
        return Err(invalid(format!("步數上限必須在 1～{MAX_STEPS_LIMIT} 之間")));
    }
    if input.max_model_calls > MAX_MODEL_CALLS_LIMIT {
        return Err(invalid(format!("成本上限（模型呼叫次數）必須在 0～{MAX_MODEL_CALLS_LIMIT} 之間")));
    }
    let charter = Charter {
        id,
        workspace_id: workspace_id.to_string(),
        task_id: input.task_id.trim().to_string(),
        version: current.map_or(1, |c| c.version + 1),
        area: text(&input.area, "探索區域", true)?,
        role: text(&input.role, "使用者角色", true)?,
        goal: text(&input.goal, "探索目標", true)?,
        forbidden_side_effects: texts(&input.forbidden_side_effects, "禁止的副作用", true)?,
        max_steps: input.max_steps,
        max_model_calls: input.max_model_calls,
        defined_by,
        created_at: now,
    };
    if current.is_some_and(|c| c.same_content(&charter)) {
        return Ok(None);
    }
    let event = DomainEvent {
        aggregate_type: CHARTER_AGGREGATE,
        aggregate_id: charter.task_id.clone(),
        aggregate_version: charter.version,
        event_type: EVENT_CHARTER_DEFINED,
        occurred_at: now,
        payload: serde_json::json!({
            "charter_id": charter.id,
            "workspace_id": charter.workspace_id,
            "area": charter.area,
            "role": charter.role,
            "goal": charter.goal,
            "forbidden_side_effects": charter.forbidden_side_effects,
            "max_steps": charter.max_steps,
            "max_model_calls": charter.max_model_calls,
            "defined_by": charter.defined_by,
        }),
    };
    Ok(Some((charter, event)))
}

/// Worker Prompt 中的章程說明（Context Builder 的必要項目）。
pub fn charter_prompt(charter: &Charter) -> String {
    let forbidden: Vec<String> = charter.forbidden_side_effects.iter().map(|s| format!("\n  - {s}")).collect();
    format!(
        "\n\n探索章程（第 {} 版，必須遵守）：\n- 區域：{}\n- 使用者角色：{}\n- 目標：{}\n- 禁止的副作用：{}\n- 步數上限：{} 步；成本上限：{} 次模型呼叫\n\
         只在章程範圍內探索；發現可疑的行為時，以 report_exploration_finding 回報疑點（重現步驟、預期、實際觀察、環境與證據）。\
         疑點不是缺陷：只有 Runtime 執行重現案例、以產品行為斷言失敗後才會被確認，不要自稱已確認缺陷。",
        charter.version,
        charter.area,
        charter.role,
        charter.goal,
        forbidden.concat(),
        charter.max_steps,
        charter.max_model_calls,
    )
}

// ---------------------------------------------------------------- 疑點

/// 疑點的狀態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingStatus {
    /// 疑點：尚未經過重現。
    Suspected,
    /// 已確認缺陷：重現案例以產品行為斷言失敗。
    Confirmed,
    /// 未驗風險：無法重現（或環境一直無法驗證），不駁回、也不宣稱已確認。
    UnverifiedRisk,
    /// 已排除：環境、seed data、工具限制或預期設計；保留紀錄，可以重新開啟。
    Excluded,
    /// 已修復：確認時的案例已通過，納入正常回歸。
    Fixed,
    /// 回歸失敗：已修復的案例在一般驗證中又以產品行為斷言失敗；仍在回歸範圍內。
    Regressed,
}

impl FindingStatus {
    pub const ALL: [FindingStatus; 6] = [
        FindingStatus::Suspected,
        FindingStatus::Confirmed,
        FindingStatus::UnverifiedRisk,
        FindingStatus::Excluded,
        FindingStatus::Fixed,
        FindingStatus::Regressed,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            FindingStatus::Suspected => "suspected",
            FindingStatus::Confirmed => "confirmed",
            FindingStatus::UnverifiedRisk => "unverified_risk",
            FindingStatus::Excluded => "excluded",
            FindingStatus::Fixed => "fixed",
            FindingStatus::Regressed => "regressed",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.as_str() == raw)
    }

    /// 已經過重現確認的缺陷（含已修復與回歸失敗）。
    pub fn is_defect(self) -> bool {
        matches!(self, FindingStatus::Confirmed | FindingStatus::Fixed | FindingStatus::Regressed)
    }

    /// 案例屬於正常回歸範圍：每一次驗證都會執行。
    pub fn in_regression(self) -> bool {
        matches!(self, FindingStatus::Fixed | FindingStatus::Regressed)
    }
}

/// 排除疑點的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExclusionReason {
    /// 環境故障（不是產品行為）。
    Environment,
    /// seed data（測試資料本身造成）。
    SeedData,
    /// 工具限制（自動化或瀏覽器引擎的限制）。
    ToolLimitation,
    /// 預期設計：產品取捨，必須由使用者裁定。
    ExpectedDesign,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Exclusion {
    pub reason: ExclusionReason,
    pub rationale: String,
    /// 排除為預期設計時，使用者核准的裁定請求。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision_request_id: Option<String>,
}

/// Worker 回報的疑點內容。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FindingReport {
    pub title: String,
    /// 對應的 Feature Map 功能（需求到測試的追蹤）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feature: Option<String>,
    /// 重現步驟。
    pub steps: Vec<String>,
    pub expected: String,
    /// 實際觀察。
    pub actual: String,
    /// 環境（版本、資料、瀏覽器等）。
    #[serde(default)]
    pub environment: String,
    /// 證據：Workspace 中已登記的成果或 Evidence Bundle 的識別（畫面、trace 等）。
    #[serde(default)]
    pub evidence: Vec<String>,
}

/// 確認缺陷時的重現案例：之後修復驗證與回歸都執行同一個案例，不能更換。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReproductionCase {
    /// 案例的旅程（識別是 `finding.<疑點>`）。
    pub journey: Journey,
    /// 確認它的驗證要求與 Evidence Bundle。
    pub request_id: String,
    pub bundle_id: String,
    /// 以產品行為斷言失敗的步驟。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed_step: Option<String>,
    pub confirmed_at: Timestamp,
}

/// 探索的一個疑點（不可刪除；狀態變更都追加事件）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExplorationFinding {
    pub id: String,
    pub workspace_id: String,
    /// 回報它的探索 Task。
    pub task_id: String,
    /// 回報當時的章程版本。
    pub charter_id: String,
    pub charter_version: i64,
    #[serde(flatten)]
    pub report: FindingReport,
    pub status: FindingStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exclusion: Option<Exclusion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unverified_reason: Option<String>,
    /// 確認缺陷時的重現案例（只有已確認的缺陷有）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub case: Option<ReproductionCase>,
    /// 回報者：Worker Run。
    pub reported_by: String,
    pub version: i64,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

/// Worker 回報疑點：一律先是 `suspected`；Task 必須已有章程。
pub fn report(
    charter: Option<&Charter>,
    workspace_id: &str,
    task_id: &str,
    input: FindingReport,
    reported_by: String,
    id: String,
    now: Timestamp,
) -> Result<(ExplorationFinding, DomainEvent), ExplorationError> {
    let Some(charter) = charter else { return Err(ExplorationError::CharterRequired(task_id.to_string())) };
    let report = FindingReport {
        title: text(&input.title, "疑點標題", true)?,
        feature: input.feature.as_deref().map(str::trim).filter(|f| !f.is_empty()).map(str::to_string),
        steps: texts(&input.steps, "重現步驟", true)?,
        expected: text(&input.expected, "預期行為", true)?,
        actual: text(&input.actual, "實際觀察", true)?,
        environment: text(&input.environment, "環境", false)?,
        evidence: texts(&input.evidence, "證據", false)?,
    };
    let finding = ExplorationFinding {
        id,
        workspace_id: workspace_id.to_string(),
        task_id: task_id.to_string(),
        charter_id: charter.id.clone(),
        charter_version: charter.version,
        report,
        status: FindingStatus::Suspected,
        exclusion: None,
        unverified_reason: None,
        case: None,
        reported_by,
        version: 1,
        created_at: now,
        updated_at: now,
    };
    let event = DomainEvent {
        aggregate_type: FINDING_AGGREGATE,
        aggregate_id: finding.id.clone(),
        aggregate_version: 1,
        event_type: EVENT_FINDING_REPORTED,
        occurred_at: now,
        payload: serde_json::json!({
            "workspace_id": finding.workspace_id,
            "task_id": finding.task_id,
            "charter_id": finding.charter_id,
            "title": finding.report.title,
            "feature": finding.report.feature,
            "status": finding.status,
            "reported_by": finding.reported_by,
        }),
    };
    Ok((finding, event))
}

/// Controller 對疑點的分流。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum TriageAction {
    /// 排除（環境、seed data、工具限制、預期設計），附理由。
    Exclude { reason: ExclusionReason, rationale: String },
    /// 無法重現或無法驗證：保留為未驗風險。
    MarkUnverified { rationale: String },
    /// 重新開啟已排除的疑點。
    Reopen { rationale: String },
}

/// 疑點狀態的變更（由 `apply` 寫回）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FindingChange {
    pub status: FindingStatus,
    pub exclusion: Option<Exclusion>,
    pub unverified_reason: Option<String>,
    pub case: Option<ReproductionCase>,
}

fn defect_locked(finding: &ExplorationFinding) -> ExplorationError {
    ExplorationError::DefectLocked(format!(
        "疑點 {} 已經過重現確認為缺陷（{}）：不能排除、不能改為未驗風險，也不能更換重現案例；修復後以同一個案例驗證",
        finding.id,
        finding.status.as_str()
    ))
}

/// 分流：已確認的缺陷一律拒絕；排除為預期設計必須附使用者核准的裁定請求。
pub fn triage(
    finding: &ExplorationFinding,
    action: TriageAction,
    approved_decision: Option<String>,
) -> Result<FindingChange, ExplorationError> {
    if finding.status.is_defect() {
        return Err(defect_locked(finding));
    }
    let refused = |what: &str| {
        ExplorationError::TransitionRefused(format!("疑點 {} 目前是 {}，不能{what}", finding.id, finding.status.as_str()))
    };
    let unchanged = FindingChange { status: finding.status, exclusion: None, unverified_reason: finding.unverified_reason.clone(), case: None };
    match action {
        TriageAction::Exclude { reason, rationale } => {
            if finding.status == FindingStatus::Excluded {
                return Err(refused("再次排除"));
            }
            let rationale = text(&rationale, "排除理由", true)?;
            let decision_request_id = match reason {
                ExclusionReason::ExpectedDesign => Some(approved_decision.ok_or(ExplorationError::DecisionRequired)?),
                _ => None,
            };
            Ok(FindingChange {
                status: FindingStatus::Excluded,
                exclusion: Some(Exclusion { reason, rationale, decision_request_id }),
                ..unchanged
            })
        }
        TriageAction::MarkUnverified { rationale } => {
            if finding.status != FindingStatus::Suspected {
                return Err(refused("改為未驗風險（只有疑點可以）"));
            }
            Ok(FindingChange {
                status: FindingStatus::UnverifiedRisk,
                unverified_reason: Some(text(&rationale, "理由", true)?),
                ..unchanged
            })
        }
        TriageAction::Reopen { rationale } => {
            if finding.status != FindingStatus::Excluded {
                return Err(refused("重新開啟（只有已排除的疑點可以）"));
            }
            text(&rationale, "理由", true)?;
            Ok(FindingChange { status: FindingStatus::Suspected, unverified_reason: None, ..unchanged })
        }
    }
}

/// 寫回狀態變更：版本加一並追加事件。`cause` 是變更的來源（`triaged`、`reproduction` 等）。
pub fn apply(finding: &ExplorationFinding, change: FindingChange, cause: &str, now: Timestamp) -> (ExplorationFinding, DomainEvent) {
    let mut updated = finding.clone();
    updated.status = change.status;
    updated.exclusion = change.exclusion;
    updated.unverified_reason = change.unverified_reason;
    if updated.case.is_none() {
        updated.case = change.case;
    }
    updated.version += 1;
    updated.updated_at = now;
    let event = DomainEvent {
        aggregate_type: FINDING_AGGREGATE,
        aggregate_id: updated.id.clone(),
        aggregate_version: updated.version,
        event_type: EVENT_FINDING_STATUS_CHANGED,
        occurred_at: now,
        payload: serde_json::json!({
            "workspace_id": updated.workspace_id,
            "from": finding.status,
            "to": updated.status,
            "cause": cause,
            "exclusion": updated.exclusion,
            "unverified_reason": updated.unverified_reason,
            "case_bundle_id": updated.case.as_ref().map(|c| &c.bundle_id),
        }),
    };
    (updated, event)
}

// ---------------------------------------------------------------- 重現

/// 一次重現的目的。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptPurpose {
    /// 確認疑點（疑點或未驗風險）。
    Reproduce,
    /// 以確認時的案例驗證修復（已確認或回歸失敗的缺陷）。
    VerifyFix,
    /// 一般驗證中執行的回歸案例（已修復或回歸失敗的缺陷）。
    Regression,
}

impl AttemptPurpose {
    pub fn as_str(self) -> &'static str {
        match self {
            AttemptPurpose::Reproduce => "reproduce",
            AttemptPurpose::VerifyFix => "verify_fix",
            AttemptPurpose::Regression => "regression",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        [AttemptPurpose::Reproduce, AttemptPurpose::VerifyFix, AttemptPurpose::Regression].into_iter().find(|p| p.as_str() == raw)
    }

    /// 診斷用的重現（重現與修復驗證）：Bundle 不能作為 Gate 的驗證證據。
    pub fn is_diagnostic(self) -> bool {
        !matches!(self, AttemptPurpose::Regression)
    }
}

/// 一次重現的結論。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptOutcome {
    /// 案例以產品行為斷言失敗、對照通過：確認缺陷。
    Reproduced,
    /// 案例通過：無法重現。
    NotReproduced,
    /// 證據不足（環境、授權、預算、對照未通過、案例沒有執行）：狀態不變。
    Inconclusive,
    /// 修復驗證通過。
    FixVerified,
    /// 修復驗證時案例仍以產品行為斷言失敗。
    StillFailing,
    /// 回歸案例以產品行為斷言失敗。
    Regressed,
    /// 回歸案例通過。
    RegressionPassed,
}

impl AttemptOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            AttemptOutcome::Reproduced => "reproduced",
            AttemptOutcome::NotReproduced => "not_reproduced",
            AttemptOutcome::Inconclusive => "inconclusive",
            AttemptOutcome::FixVerified => "fix_verified",
            AttemptOutcome::StillFailing => "still_failing",
            AttemptOutcome::Regressed => "regressed",
            AttemptOutcome::RegressionPassed => "regression_passed",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        [
            AttemptOutcome::Reproduced,
            AttemptOutcome::NotReproduced,
            AttemptOutcome::Inconclusive,
            AttemptOutcome::FixVerified,
            AttemptOutcome::StillFailing,
            AttemptOutcome::Regressed,
            AttemptOutcome::RegressionPassed,
        ]
        .into_iter()
        .find(|o| o.as_str() == raw)
    }
}

/// 一次重現的紀錄（排入驗證要求時建立，Bundle 寫入時補上結果；只追加、不可刪除）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReproductionAttempt {
    pub finding_id: String,
    pub request_id: String,
    pub purpose: AttemptPurpose,
    /// 執行的案例（識別是 `finding.<疑點>`）。
    pub journey: Journey,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_id: Option<String>,
    /// 案例的判定與原因；案例沒有執行時為 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<Verdict>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<String>,
    /// 對照（代表旅程）的判定：全部通過是 `passed`，否則是第一條未通過的判定。回歸沒有對照。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_verdict: Option<Verdict>,
    /// 還沒有結果時為 `None`（驗證要求排隊中）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<AttemptOutcome>,
    pub requested_by: String,
    pub created_at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<Timestamp>,
}

/// 檢查重現案例能否在這個 Profile 的環境中執行：與 Profile 旅程相同的檢查（每個自動化步驟都有程式斷言、persona 與
/// 登入身分存在、秘密已宣告、瀏覽器操作只用於 web），並且至少有一個可自動化的步驟。
pub fn check_case(definition: &ProfileDefinition, journey: &Journey) -> Result<(), ExplorationError> {
    if !journey.steps.iter().any(|s| !matches!(s.operation, Operation::Manual { .. })) {
        return Err(invalid("重現案例至少要有一個可自動化、有程式斷言的步驟；人工步驟無法證明產品行為"));
    }
    // 案例的識別由 Runtime 指定：檢查時換成 Profile 中沒有的識別，以免與 Profile 旅程撞名
    let mut probe = journey.clone();
    probe.representative = false;
    probe.key = (0..)
        .map(|n| if n == 0 { "reproduction".to_string() } else { format!("reproduction-{n}") })
        .find(|key| definition.journey(key).is_none())
        .expect("一定找得到沒有使用的識別");
    let mut candidate = definition.clone();
    candidate.journeys.push(probe);
    vp::validate(&candidate).map_err(|error| match error {
        vp::ProfileError::Invalid(message) => invalid(format!("重現案例無法在目前的 Verification Profile 中執行：{message}")),
        other => invalid(other.to_string()),
    })
}

/// 存放與執行用的案例：識別換成 `finding.<疑點>`，不是代表旅程。
pub fn case_journey(finding_id: &str, journey: &Journey) -> Journey {
    Journey { key: case_key(finding_id), representative: false, ..journey.clone() }
}

/// 一次重現的計畫。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedReproduction {
    pub purpose: AttemptPurpose,
    pub journey: Journey,
}

/// 依疑點目前的狀態決定這次重現的目的與案例：
/// - 疑點、未驗風險：必須提出案例，目的是確認；
/// - 已確認、回歸失敗：只能以確認時的案例驗證修復（提出的案例必須與它相同）；
/// - 已修復：已在正常回歸中，不需要另外要求；已排除：要先重新開啟。
pub fn plan_reproduction(
    finding: &ExplorationFinding,
    proposed: Option<&Journey>,
    definition: &ProfileDefinition,
) -> Result<PlannedReproduction, ExplorationError> {
    match finding.status {
        FindingStatus::Suspected | FindingStatus::UnverifiedRisk => {
            let journey = proposed.ok_or_else(|| invalid("請提供重現案例（journey）：對應原症狀、以產品行為斷言預期的行為"))?;
            check_case(definition, journey)?;
            Ok(PlannedReproduction { purpose: AttemptPurpose::Reproduce, journey: case_journey(&finding.id, journey) })
        }
        FindingStatus::Confirmed | FindingStatus::Regressed => {
            let case = finding.case.as_ref().ok_or_else(|| defect_locked(finding))?;
            if proposed.is_some_and(|p| case_journey(&finding.id, p) != case.journey) {
                return Err(defect_locked(finding));
            }
            Ok(PlannedReproduction { purpose: AttemptPurpose::VerifyFix, journey: case.journey.clone() })
        }
        FindingStatus::Fixed => Err(ExplorationError::TransitionRefused(format!(
            "疑點 {} 已修復，案例已在正常回歸中：每次驗證都會執行，不需要另外要求",
            finding.id
        ))),
        FindingStatus::Excluded => Err(ExplorationError::TransitionRefused(format!(
            "疑點 {} 已排除；需要重現時先以 reopen 重新開啟",
            finding.id
        ))),
    }
}

/// 重現要執行的旅程：Profile 的代表旅程（對照）加上案例。
pub fn reproduction_journeys(definition: &ProfileDefinition, finding_id: &str) -> Vec<String> {
    definition.representative_journeys().iter().map(|j| j.key.clone()).chain(std::iter::once(case_key(finding_id))).collect()
}

/// 依 Bundle 判定一次重現的結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JudgedAttempt {
    pub outcome: AttemptOutcome,
    /// 疑點的新狀態；不變時為 `None`。
    pub status: Option<FindingStatus>,
    pub verdict: Option<Verdict>,
    pub reason_code: Option<String>,
    pub control_verdict: Option<Verdict>,
    /// 案例中以產品行為斷言失敗的步驟。
    pub failed_step: Option<String>,
}

/// 判定一次重現：只有案例以產品行為斷言失敗（`product_failure`）才是缺陷；確認與修復驗證另外要求對照（代表旅程）
/// 全部通過。其他判定（環境、授權、證據不足、預算、無法自動化），以及案例沒有出現在 Bundle 中，都是證據不足。
pub fn judge_attempt(finding: &ExplorationFinding, purpose: AttemptPurpose, bundle: &EvidenceBundle) -> JudgedAttempt {
    let key = case_key(&finding.id);
    let case = bundle.journeys.iter().find(|j| j.key == key);
    let control = (purpose != AttemptPurpose::Regression).then(|| {
        bundle
            .journeys
            .iter()
            .filter(|j| j.representative && j.key != key)
            .map(|j| j.verdict)
            .find(|v| *v != Verdict::Passed)
            .unwrap_or(Verdict::Passed)
    });
    let failed_step = bundle
        .steps
        .iter()
        .find(|s| s.journey.as_deref() == Some(&key) && s.verdict == Verdict::ProductFailure)
        .map(|s| s.key.clone());
    let verdict = case.map(|c| c.verdict);
    let healthy = control.is_none_or(|c| c == Verdict::Passed);
    let (outcome, status) = match (purpose, verdict, healthy) {
        (_, None, _) | (_, Some(_), false) => (AttemptOutcome::Inconclusive, None),
        (AttemptPurpose::Reproduce, Some(Verdict::ProductFailure), true) => (AttemptOutcome::Reproduced, Some(FindingStatus::Confirmed)),
        (AttemptPurpose::Reproduce, Some(Verdict::Passed), true) => {
            (AttemptOutcome::NotReproduced, Some(FindingStatus::UnverifiedRisk))
        }
        (AttemptPurpose::VerifyFix, Some(Verdict::Passed), true) => (AttemptOutcome::FixVerified, Some(FindingStatus::Fixed)),
        (AttemptPurpose::VerifyFix, Some(Verdict::ProductFailure), true) => (AttemptOutcome::StillFailing, None),
        (AttemptPurpose::Regression, Some(Verdict::Passed), _) => (AttemptOutcome::RegressionPassed, Some(FindingStatus::Fixed)),
        (AttemptPurpose::Regression, Some(Verdict::ProductFailure), _) => (AttemptOutcome::Regressed, Some(FindingStatus::Regressed)),
        _ => (AttemptOutcome::Inconclusive, None),
    };
    JudgedAttempt {
        outcome,
        status: status.filter(|s| *s != finding.status),
        verdict,
        reason_code: case.map(|c| c.reason_code.clone()),
        control_verdict: control,
        failed_step: if outcome == AttemptOutcome::Reproduced { failed_step } else { None },
    }
}

/// 重現結果造成的狀態變更（狀態不變時為 `None`）；確認時記下案例，之後不能更換。
pub fn change_for(
    judged: &JudgedAttempt,
    attempt: &ReproductionAttempt,
    bundle: &EvidenceBundle,
) -> Option<FindingChange> {
    let status = judged.status?;
    let case = (status == FindingStatus::Confirmed).then(|| ReproductionCase {
        journey: attempt.journey.clone(),
        request_id: attempt.request_id.clone(),
        bundle_id: bundle.id.clone(),
        failed_step: judged.failed_step.clone(),
        confirmed_at: bundle.finished_at,
    });
    let unverified_reason = (status == FindingStatus::UnverifiedRisk)
        .then(|| format!("重現案例在 Evidence Bundle {} 中通過：無法重現，保留為未驗風險", bundle.id));
    Some(FindingChange { status, exclusion: None, unverified_reason, case })
}

fn attempt_id(attempt: &ReproductionAttempt) -> String {
    format!("{}/{}", attempt.request_id, attempt.finding_id)
}

/// 重現排入驗證要求的事件。
pub fn requested_event(finding: &ExplorationFinding, attempt: &ReproductionAttempt) -> DomainEvent {
    DomainEvent {
        aggregate_type: ATTEMPT_AGGREGATE,
        aggregate_id: attempt_id(attempt),
        aggregate_version: 1,
        event_type: EVENT_REPRODUCTION_REQUESTED,
        occurred_at: attempt.created_at,
        payload: serde_json::json!({
            "workspace_id": finding.workspace_id,
            "finding_id": finding.id,
            "request_id": attempt.request_id,
            "purpose": attempt.purpose,
            "requested_by": attempt.requested_by,
        }),
    }
}

/// 重現結果的事件。
pub fn judged_event(finding: &ExplorationFinding, attempt: &ReproductionAttempt, judged: &JudgedAttempt, now: Timestamp) -> DomainEvent {
    DomainEvent {
        aggregate_type: ATTEMPT_AGGREGATE,
        aggregate_id: attempt_id(attempt),
        aggregate_version: 2,
        event_type: EVENT_REPRODUCTION_JUDGED,
        occurred_at: now,
        payload: serde_json::json!({
            "workspace_id": finding.workspace_id,
            "finding_id": finding.id,
            "request_id": attempt.request_id,
            "purpose": attempt.purpose,
            "bundle_id": attempt.bundle_id,
            "outcome": judged.outcome,
            "verdict": judged.verdict,
            "reason_code": judged.reason_code,
            "control_verdict": judged.control_verdict,
        }),
    }
}

/// 回歸案例與目前的 Profile 不相容（例如引用的登入身分已不存在）時，執行的替代旅程：一個無法自動化的步驟，
/// 判定為 `automation_unsupported`，讓這次驗證不能通過，也不會把不相容誤判為產品缺陷。
pub fn incompatible_case(finding_id: &str, title: &str, problem: &str) -> Journey {
    Journey {
        key: case_key(finding_id),
        title: title.to_string(),
        representative: false,
        persona: None,
        identity: None,
        steps: vec![vp::Step {
            key: "incompatible".into(),
            title: "回歸案例與目前的 Verification Profile 不相容".into(),
            operation: Operation::Manual {
                instruction: format!("回歸案例無法在目前的 Profile 中執行，需要使用者處理：{problem}"),
            },
            assertions: Vec::new(),
            external: false,
        }],
    }
}

#[cfg(test)]
mod tests;
