//! 雙軸 Review、獨立 reviewer 與設計偏離（票 44，issue #3 §9.3）。
//!
//! - **兩軸分開保存**：一份 Review 固定 base／candidate revision、規格版本（被審 Task 的契約版本）與規範來源，
//!   Standards（規範）與 Spec（需求）各自有審查摘要、審過的範圍（paths）與 finding。任一軸有未解決的 finding，
//!   整份 Review 就不通過：程式品質好不能掩蓋需求缺漏，反之亦然。
//! - **reviewer 不能是作者**：作者是被審的 Task（它的每一個 Run 與用過的 Session）；reviewer 必須是另一個 Task 的 Run，
//!   也沒有沿用作者的 Session。作者身分跟著 Task，所以重建或替換 Session 不會讓作者變成非作者。
//! - **finding 由 Controller 依證據分類**：修正、具理由駁回、或需裁定。駁回與需裁定都必須經使用者裁定，
//!   只有使用者同意駁回的 finding 才算解決；修正要等新的 candidate 重新審查。
//! - **修正後只沿用可證明不受影響的軸**：新的 Review 可以對某一軸指定沿用先前的 Review，Runtime 只在下列條件都成立時接受：
//!   同一個 Task、先前那一軸已通過、輸入未變（需求軸的規格版本、規範軸的規範來源）、先前記錄了審過的範圍，
//!   而且 Runtime 自己算出的兩個 candidate 之間的變更檔案都不在那個範圍內。算不出變更就不能沿用。
//! - **設計偏離**：Worker 回報偏離 Design Package（某個成果的某個版本）時保存偏離、原因、受影響契約與提議的新設計；
//!   接受必須經使用者裁定，接受後在同一個成果追加新版本（設計版本前進），下一位 Worker 的 Context 拿到的就是新版本。

use serde::{Deserialize, Serialize};

use crate::event::DomainEvent;
use crate::workflow::AxisReviewers;
use crate::Timestamp;

/// reviewer（Worker）提交雙軸審查的工具名稱。
pub const TOOL_SUBMIT_REVIEW: &str = "submit_review";
/// Controller 依證據分類 finding 的工具名稱（Controller 專屬）。
pub const TOOL_CLASSIFY_FINDING: &str = "classify_review_finding";
/// Controller 讀取審查與設計偏離的工具名稱（Controller 專屬）。
pub const TOOL_GET_REVIEWS: &str = "get_reviews";
/// Worker 回報實作偏離設計的工具名稱。
pub const TOOL_REPORT_DESIGN_DEVIATION: &str = "report_design_deviation";
/// Controller 處理設計偏離（接受須經使用者裁定）的工具名稱（Controller 專屬）。
pub const TOOL_RESOLVE_DESIGN_DEVIATION: &str = "resolve_design_deviation";

pub const REVIEW_AGGREGATE: &str = "review";
pub const FINDING_AGGREGATE: &str = "review_finding";
pub const DEVIATION_AGGREGATE: &str = "design_deviation";
pub const EVENT_REVIEW_SUBMITTED: &str = "review_submitted";
pub const EVENT_FINDING_CLASSIFIED: &str = "review_finding_classified";
pub const EVENT_DEVIATION_REPORTED: &str = "design_deviation_reported";
pub const EVENT_DEVIATION_RESOLVED: &str = "design_deviation_resolved";

pub const REVISION_MAX_CHARS: usize = 200;
pub const SOURCE_MAX_CHARS: usize = 500;
pub const SUMMARY_MAX_CHARS: usize = 4_000;
pub const PATHS_MAX: usize = 50;
pub const PATH_MAX_CHARS: usize = 500;
pub const FINDINGS_MAX: usize = 30;
pub const TITLE_MAX_CHARS: usize = 200;
pub const DETAIL_MAX_CHARS: usize = 4_000;
pub const REASON_MAX_CHARS: usize = 2_000;
pub const CONTRACTS_MAX: usize = 20;
pub const DESIGN_MAX_CHARS: usize = 100_000;

/// 代表「整個 repo」的審查範圍。
pub const WHOLE_REPOSITORY: &str = ".";

// ---------------------------------------------------------------- 軸

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    /// 規範：程式碼品質、專案規則與慣例。
    Standards,
    /// 需求：是否滿足規格（被審 Task 的契約）。
    Spec,
}

impl Axis {
    pub const ALL: [Axis; 2] = [Axis::Standards, Axis::Spec];

    pub fn as_str(self) -> &'static str {
        match self {
            Axis::Standards => "standards",
            Axis::Spec => "spec",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.as_str() == raw)
    }

    pub fn label(self) -> &'static str {
        match self {
            Axis::Standards => "規範（Standards）",
            Axis::Spec => "需求（Spec）",
        }
    }
}

// ---------------------------------------------------------------- 錯誤

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReviewError {
    #[error("內容不正確：{0}")]
    Invalid(String),
    #[error("找不到 Task {0}")]
    TaskNotFound(String),
    #[error("作者不能審查自己的工作：reviewer 必須是另一個 Task 的 Worker，而且沒有沿用作者的 Session")]
    ReviewerIsAuthor,
    #[error("審查對照的是規格第 {submitted} 版，但 Task 的契約目前是第 {current} 版；請依目前的契約重新審查")]
    SpecOutdated { submitted: i64, current: i64 },
    #[error("{}不能沿用：{reason}", .axis.label())]
    ReuseRejected { axis: Axis, reason: String },
    #[error("找不到審查 {0}")]
    ReviewNotFound(String),
    #[error("找不到 finding {0}")]
    FindingNotFound(String),
    #[error("finding {0} 已經駁回（使用者已裁定），不能再改分類")]
    FindingResolved(String),
    #[error("找不到設計偏離 {0}")]
    DeviationNotFound(String),
    #[error("設計偏離 {0} 已經處理過")]
    DeviationResolved(String),
    #[error("找不到設計 {0}（必須是這個 Workspace 已登記的成果）")]
    DesignNotFound(String),
    #[error("設計已經是第 {current} 版，不是偏離時對照的第 {reported} 版；請對照目前的設計重新回報")]
    DesignOutdated { reported: i64, current: i64 },
}

impl ReviewError {
    /// 穩定的機器可讀代碼。
    pub fn code(&self) -> &'static str {
        match self {
            ReviewError::Invalid(_) => "review_invalid",
            ReviewError::TaskNotFound(_) => "task_not_found",
            ReviewError::ReviewerIsAuthor => "reviewer_is_author",
            ReviewError::SpecOutdated { .. } => "review_spec_outdated",
            ReviewError::ReuseRejected { .. } => "review_reuse_rejected",
            ReviewError::ReviewNotFound(_) => "review_not_found",
            ReviewError::FindingNotFound(_) => "finding_not_found",
            ReviewError::FindingResolved(_) => "finding_resolved",
            ReviewError::DeviationNotFound(_) => "deviation_not_found",
            ReviewError::DeviationResolved(_) => "deviation_resolved",
            ReviewError::DesignNotFound(_) => "design_not_found",
            ReviewError::DesignOutdated { .. } => "design_outdated",
        }
    }
}

fn invalid(message: impl Into<String>) -> ReviewError {
    ReviewError::Invalid(message.into())
}

/// 去掉前後空白；必填時不能空白；不能超過上限。
fn text(raw: &str, max: usize, label: &str, required: bool) -> Result<String, ReviewError> {
    let value = raw.trim();
    if required && value.is_empty() {
        return Err(invalid(format!("{label}不能空白")));
    }
    if value.chars().count() > max {
        return Err(invalid(format!("{label}最多 {max} 個字元")));
    }
    Ok(value.to_string())
}

/// revision 必填、不含空白，也不能以 `-` 開頭（避免被當成 Git 的選項）。
fn revision(raw: &str, label: &str) -> Result<String, ReviewError> {
    let value = text(raw, REVISION_MAX_CHARS, label, true)?;
    if value.starts_with('-') || value.chars().any(char::is_whitespace) {
        return Err(invalid(format!("{label}不能含空白，也不能以 - 開頭")));
    }
    Ok(value)
}

// ---------------------------------------------------------------- 審查範圍

/// 統一路徑寫法：`/` 分隔、去掉開頭的 `./` 與結尾的 `/`；空白或 `.` 代表整個 repo。
pub fn normalize_path(raw: &str) -> String {
    let mut path = raw.trim().replace('\\', "/");
    while let Some(rest) = path.strip_prefix("./") {
        path = rest.to_string();
    }
    let path = path.trim_end_matches('/');
    if path.is_empty() || path == WHOLE_REPOSITORY {
        WHOLE_REPOSITORY.to_string()
    } else {
        path.to_string()
    }
}

/// 變更的檔案是否落在審過的範圍內（不分大小寫；範圍是目錄時含其下所有檔案）。寧可多判為受影響，也不漏判。
/// 檔名含 `"`、`\`、控制字元等時 git 即使設 `core.quotepath=false` 仍會加 C 風格引號：先還原再比對，
/// 無法還原的引號路徑一律視為受影響（review-notes #22）。
fn touches(changed: &str, reviewed: &str) -> bool {
    let Some(changed) = unquote_git_path(changed) else { return true };
    let (changed, reviewed) = (normalize_path(&changed).to_lowercase(), reviewed.to_lowercase());
    reviewed == WHOLE_REPOSITORY || changed == reviewed || changed.starts_with(&format!("{reviewed}/"))
}

/// 還原 git 的 C 風格引號路徑（`"…"`，跳脫 `\"`、`\\`、`\a` `\b` `\t` `\n` `\v` `\f` `\r` 與三位八進位的位元組 `\ooo`，
/// 位元組組回 UTF-8）。沒有引號的路徑原樣傳回；格式不對或不是合法的 UTF-8 時回 `None`。
fn unquote_git_path(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if !trimmed.starts_with('"') {
        return Some(raw.to_string());
    }
    let inner = trimmed.strip_prefix('"')?.strip_suffix('"')?.as_bytes();
    let mut bytes = Vec::with_capacity(inner.len());
    let mut i = 0;
    while i < inner.len() {
        let byte = inner[i];
        i += 1;
        match byte {
            b'"' => return None,
            b'\\' => {
                let escaped = *inner.get(i)?;
                i += 1;
                bytes.push(match escaped {
                    b'"' => b'"',
                    b'\\' => b'\\',
                    b'a' => 0x07,
                    b'b' => 0x08,
                    b't' => b'\t',
                    b'n' => b'\n',
                    b'v' => 0x0b,
                    b'f' => 0x0c,
                    b'r' => b'\r',
                    b'0'..=b'3' => {
                        let digits = inner.get(i..i + 2)?;
                        if !digits.iter().all(|d| (b'0'..=b'7').contains(d)) {
                            return None;
                        }
                        i += 2;
                        (escaped - b'0') * 64 + (digits[0] - b'0') * 8 + (digits[1] - b'0')
                    }
                    _ => return None,
                });
            }
            other => bytes.push(other),
        }
    }
    String::from_utf8(bytes).ok()
}

// ---------------------------------------------------------------- 提交

/// 一項 finding 的內容。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FindingInput {
    pub title: String,
    #[serde(default)]
    pub detail: String,
    /// 位置（例如 `src/export.rs:42`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
}

/// 一軸的審查內容；指定 `reuse_from` 時沿用先前審查的這一軸，其他欄位必須留空。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AxisInput {
    /// 檢查了什麼、對照什麼。
    #[serde(default)]
    pub summary: String,
    /// 審過的檔案或目錄（相對 repo 根目錄）；沒有記錄時，這一軸之後不能被沿用。
    #[serde(default)]
    pub paths: Vec<String>,
    #[serde(default)]
    pub findings: Vec<FindingInput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reuse_from: Option<String>,
}

/// reviewer 提交的雙軸審查。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewSubmission {
    /// 被審的 Task（作者）。
    pub subject_task_id: String,
    pub base_revision: String,
    pub candidate_revision: String,
    /// 對照的規格版本：被審 Task 的契約版本，必須是目前的版本。
    pub spec_version: i64,
    /// 規範來源（例如 `AGENTS.md@<commit>`）。
    pub standards_source: String,
    pub standards: AxisInput,
    pub spec: AxisInput,
}

/// 提交審查的 reviewer：它的 Run、Task 與用過的 Session（平台 Session 與原生 Session）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reviewer {
    pub run_id: String,
    pub task_id: String,
    pub sessions: Vec<String>,
}

/// 被審的 Task：目前的契約版本，以及它的 Run 用過的全部 Session。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubjectTask {
    pub workspace_id: String,
    pub id: String,
    pub contract_version: i64,
    pub author_sessions: Vec<String>,
}

/// 要沿用的先前審查：它的 finding，以及 Runtime 算出的「先前 candidate → 這次 candidate」變更檔案（算不出時是原因）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PriorReview {
    pub review: Review,
    pub findings: Vec<Finding>,
    pub changed_paths: Result<Vec<String>, String>,
}

/// 一軸的結果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AxisResult {
    pub summary: String,
    pub paths: Vec<String>,
    /// 沿用自哪一份審查；沿用時摘要與範圍都照抄那一份。
    pub reused_from: Option<String>,
    /// 實際審查這一軸的 reviewer（Task）；沿用時是原本的 reviewer。
    pub reviewer_task_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Review {
    pub id: String,
    pub workspace_id: String,
    pub subject_task_id: String,
    pub reviewer_run_id: String,
    pub reviewer_task_id: String,
    pub base_revision: String,
    pub candidate_revision: String,
    pub spec_version: i64,
    pub standards_source: String,
    pub standards: AxisResult,
    pub spec: AxisResult,
    pub created_at: Timestamp,
}

impl Review {
    pub fn axis(&self, axis: Axis) -> &AxisResult {
        match axis {
            Axis::Standards => &self.standards,
            Axis::Spec => &self.spec,
        }
    }

    /// 兩軸各自實際審查的 reviewer（Gate 依此計算必要的審查人數）。
    pub fn reviewers(&self) -> AxisReviewers {
        AxisReviewers { standards: self.standards.reviewer_task_id.clone(), spec: self.spec.reviewer_task_id.clone() }
    }
}

/// finding 目前的處理狀態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingState {
    /// 尚未分類。
    Open,
    /// 分類為修正：要等新的 candidate 重新審查。
    Fix,
    /// 駁回或需裁定，等待使用者裁定。
    AwaitingDecision,
    /// 使用者同意駁回：已解決。
    Dismissed,
}

impl FindingState {
    pub const ALL: [FindingState; 4] =
        [FindingState::Open, FindingState::Fix, FindingState::AwaitingDecision, FindingState::Dismissed];

    pub fn as_str(self) -> &'static str {
        match self {
            FindingState::Open => "open",
            FindingState::Fix => "fix",
            FindingState::AwaitingDecision => "awaiting_decision",
            FindingState::Dismissed => "dismissed",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.as_str() == raw)
    }
}

/// Controller 的分類。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Classification {
    /// 修正（Controller 可自主處理）。
    Fix,
    /// 具理由駁回（必須送裁定）。
    Dismiss,
    /// Controller 無法判斷，交給使用者裁定。
    NeedsDecision,
}

impl Classification {
    pub const ALL: [Classification; 3] = [Classification::Fix, Classification::Dismiss, Classification::NeedsDecision];

    pub fn as_str(self) -> &'static str {
        match self {
            Classification::Fix => "fix",
            Classification::Dismiss => "dismiss",
            Classification::NeedsDecision => "needs_decision",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.as_str() == raw)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    pub id: String,
    pub review_id: String,
    pub axis: Axis,
    pub title: String,
    pub detail: String,
    pub location: Option<String>,
    pub state: FindingState,
    pub classification: Option<Classification>,
    /// Controller 分類的理由與證據說明。
    pub reason: Option<String>,
    pub decision_request_id: Option<String>,
    /// 裁定者（駁回由使用者裁定時是桌面）。
    pub decided_by: Option<String>,
    /// 最後分類的 Controller 一輪。
    pub classified_by: Option<String>,
    pub version: i64,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

/// 一軸是否通過：沿用（沿用前已確認通過）或這一軸的 finding 全部經使用者同意駁回。
pub fn axis_passed(review: &Review, axis: Axis, findings: &[Finding]) -> bool {
    review.axis(axis).reused_from.is_some()
        || findings.iter().filter(|f| f.review_id == review.id && f.axis == axis).all(|f| f.state == FindingState::Dismissed)
}

/// 兩軸都通過才通過。
pub fn review_passed(review: &Review, findings: &[Finding]) -> bool {
    Axis::ALL.into_iter().all(|axis| axis_passed(review, axis, findings))
}

/// 提交的結果：審查、它的 finding 與事件。
#[derive(Debug, Clone, PartialEq)]
pub struct Submitted {
    pub review: Review,
    pub findings: Vec<Finding>,
    pub event: DomainEvent,
}

/// 驗證並建立一份雙軸審查。`finding_id` 依序產生 finding 的識別；`priors` 是 `reuse_from` 指到的先前審查。
pub fn submit(
    id: String,
    finding_id: &mut dyn FnMut() -> String,
    input: ReviewSubmission,
    reviewer: &Reviewer,
    subject: &SubjectTask,
    priors: &[PriorReview],
    now: Timestamp,
) -> Result<Submitted, ReviewError> {
    if subject.id != input.subject_task_id.trim() {
        return Err(ReviewError::TaskNotFound(input.subject_task_id));
    }
    if reviewer.task_id == subject.id || reviewer.sessions.iter().any(|s| subject.author_sessions.contains(s)) {
        return Err(ReviewError::ReviewerIsAuthor);
    }
    let base_revision = revision(&input.base_revision, "base_revision ")?;
    let candidate_revision = revision(&input.candidate_revision, "candidate_revision ")?;
    let standards_source = text(&input.standards_source, SOURCE_MAX_CHARS, "standards_source（規範來源）", true)?;
    if input.spec_version != subject.contract_version {
        return Err(ReviewError::SpecOutdated { submitted: input.spec_version, current: subject.contract_version });
    }
    let mut findings = Vec::new();
    let mut results = Vec::new();
    for (axis, axis_input) in [(Axis::Standards, &input.standards), (Axis::Spec, &input.spec)] {
        let result = match axis_input.reuse_from.as_deref().map(str::trim).filter(|r| !r.is_empty()) {
            Some(from) => reuse_axis(axis, axis_input, from, &input, &standards_source, &candidate_revision, priors)?,
            None => {
                let label = axis.label();
                let summary = text(&axis_input.summary, SUMMARY_MAX_CHARS, &format!("{label}的 summary（檢查了什麼）"), true)?;
                if axis_input.paths.len() > PATHS_MAX || axis_input.findings.len() > FINDINGS_MAX {
                    return Err(invalid(format!("{label}最多 {PATHS_MAX} 個 paths、{FINDINGS_MAX} 項 finding")));
                }
                let mut paths = Vec::new();
                for raw in &axis_input.paths {
                    let path = normalize_path(&text(raw, PATH_MAX_CHARS, &format!("{label}的 path "), false)?);
                    if !paths.contains(&path) {
                        paths.push(path);
                    }
                }
                for raw in &axis_input.findings {
                    findings.push(Finding {
                        id: finding_id(),
                        review_id: id.clone(),
                        axis,
                        title: text(&raw.title, TITLE_MAX_CHARS, &format!("{label}finding 的 title"), true)?,
                        detail: text(&raw.detail, DETAIL_MAX_CHARS, &format!("{label}finding 的 detail"), false)?,
                        location: match &raw.location {
                            Some(l) => Some(text(l, PATH_MAX_CHARS, "finding 的 location ", false)?).filter(|l| !l.is_empty()),
                            None => None,
                        },
                        state: FindingState::Open,
                        classification: None,
                        reason: None,
                        decision_request_id: None,
                        decided_by: None,
                        classified_by: None,
                        version: 1,
                        created_at: now,
                        updated_at: now,
                    });
                }
                AxisResult { summary, paths, reused_from: None, reviewer_task_id: reviewer.task_id.clone() }
            }
        };
        results.push(result);
    }
    let spec = results.pop().expect("兩軸");
    let standards = results.pop().expect("兩軸");
    let review = Review {
        id,
        workspace_id: subject.workspace_id.clone(),
        subject_task_id: subject.id.clone(),
        reviewer_run_id: reviewer.run_id.clone(),
        reviewer_task_id: reviewer.task_id.clone(),
        base_revision,
        candidate_revision,
        spec_version: input.spec_version,
        standards_source,
        standards,
        spec,
        created_at: now,
    };
    let event = DomainEvent {
        aggregate_type: REVIEW_AGGREGATE,
        aggregate_id: review.id.clone(),
        aggregate_version: 1,
        event_type: EVENT_REVIEW_SUBMITTED,
        occurred_at: now,
        payload: serde_json::json!({
            "subject_task_id": review.subject_task_id,
            "reviewer_run_id": review.reviewer_run_id,
            "reviewer_task_id": review.reviewer_task_id,
            "base_revision": review.base_revision,
            "candidate_revision": review.candidate_revision,
            "spec_version": review.spec_version,
            "standards_source": review.standards_source,
            "standards_reused_from": review.standards.reused_from,
            "spec_reused_from": review.spec.reused_from,
            "findings": findings.iter().map(|f| serde_json::json!({ "id": f.id, "axis": f.axis, "title": f.title })).collect::<Vec<_>>(),
            "passed": review_passed(&review, &findings),
        }),
    };
    Ok(Submitted { review, findings, event })
}

/// 沿用先前審查的某一軸：只有可證明不受影響時才接受。
fn reuse_axis(
    axis: Axis,
    axis_input: &AxisInput,
    from: &str,
    input: &ReviewSubmission,
    standards_source: &str,
    candidate_revision: &str,
    priors: &[PriorReview],
) -> Result<AxisResult, ReviewError> {
    if !axis_input.summary.trim().is_empty() || !axis_input.paths.is_empty() || !axis_input.findings.is_empty() {
        return Err(invalid(format!("{}指定 reuse_from 時，summary、paths 與 findings 都要留空", axis.label())));
    }
    let prior = priors.iter().find(|p| p.review.id == from).ok_or_else(|| ReviewError::ReviewNotFound(from.to_string()))?;
    let rejected = |reason: String| ReviewError::ReuseRejected { axis, reason };
    let previous = prior.review.axis(axis);
    if prior.review.subject_task_id != input.subject_task_id.trim() {
        return Err(rejected(format!("審查 {from} 審的是另一個 Task")));
    }
    if !axis_passed(&prior.review, axis, &prior.findings) {
        return Err(rejected(format!("審查 {from} 的這一軸還有未解決的 finding")));
    }
    match axis {
        Axis::Spec if prior.review.spec_version != input.spec_version => {
            return Err(rejected(format!(
                "審查 {from} 對照的是規格版本第 {} 版，這次是第 {} 版",
                prior.review.spec_version, input.spec_version
            )));
        }
        Axis::Standards if prior.review.standards_source != standards_source => {
            return Err(rejected(format!("審查 {from} 的規範來源是「{}」，這次是「{standards_source}」", prior.review.standards_source)));
        }
        _ => {}
    }
    if previous.paths.is_empty() {
        return Err(rejected(format!("審查 {from} 的這一軸沒有記錄審過的範圍（paths），無法證明不受影響")));
    }
    if prior.review.candidate_revision != candidate_revision {
        let changed = prior.changed_paths.as_ref().map_err(|e| {
            rejected(format!("無法取得 {} 到 {candidate_revision} 的變更（{e}），無法證明不受影響", prior.review.candidate_revision))
        })?;
        let hits: Vec<String> = changed
            .iter()
            .flat_map(|c| previous.paths.iter().filter(|p| touches(c, p)).map(move |p| format!("{}（範圍 {p}）", normalize_path(c))))
            .collect();
        if !hits.is_empty() {
            return Err(rejected(format!("變更的檔案落在這一軸審過的範圍內：{}", hits.join("、"))));
        }
    }
    Ok(AxisResult {
        summary: previous.summary.clone(),
        paths: previous.paths.clone(),
        reused_from: Some(prior.review.id.clone()),
        reviewer_task_id: previous.reviewer_task_id.clone(),
    })
}

// ---------------------------------------------------------------- Gate 用的狀態

/// 一份審查能不能作為 Gate 的證據。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Standing {
    Passed,
    /// 這些軸還有未解決的 finding。
    NotPassed(Vec<Axis>),
    /// 規格版本已改變，或已有對較新 candidate 的審查。
    Stale(String),
    /// 審的不是這個 Workflow 的 Task。
    OtherSubject,
}

/// `latest_candidate` 是同一個 Task 最新一份審查的 candidate；`workflow_task` 是 Gate 所在 Workflow 綁定的 Task。
pub fn standing(
    review: &Review,
    findings: &[Finding],
    current_contract_version: i64,
    latest_candidate: &str,
    workflow_task: Option<&str>,
) -> Standing {
    if workflow_task != Some(review.subject_task_id.as_str()) {
        return Standing::OtherSubject;
    }
    if review.spec_version != current_contract_version {
        return Standing::Stale(format!(
            "需求契約已更新到第 {current_contract_version} 版（審查對照第 {} 版）",
            review.spec_version
        ));
    }
    if review.candidate_revision != latest_candidate {
        return Standing::Stale(format!("已有對較新 candidate {latest_candidate} 的審查"));
    }
    let failing: Vec<Axis> = Axis::ALL.into_iter().filter(|a| !axis_passed(review, *a, findings)).collect();
    if failing.is_empty() {
        Standing::Passed
    } else {
        Standing::NotPassed(failing)
    }
}

// ---------------------------------------------------------------- finding 的分類

/// 分類後的狀態與理由。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FindingChange {
    pub state: FindingState,
    pub classification: Classification,
    pub reason: String,
}

/// 套用一次分類：已駁回的 finding 不能再改；每次都記下理由、裁定請求、裁定者與分類的 Controller。
pub fn classify(
    finding: &Finding,
    change: FindingChange,
    decision_request_id: Option<String>,
    decided_by: Option<String>,
    classified_by: &str,
    now: Timestamp,
) -> Result<(Finding, DomainEvent), ReviewError> {
    if finding.state == FindingState::Dismissed {
        return Err(ReviewError::FindingResolved(finding.id.clone()));
    }
    let reason = text(&change.reason, REASON_MAX_CHARS, "reason（分類的理由與證據）", true)?;
    let mut next = finding.clone();
    next.state = change.state;
    next.classification = Some(change.classification);
    next.reason = Some(reason);
    next.decision_request_id = decision_request_id;
    next.decided_by = decided_by;
    next.classified_by = Some(classified_by.to_string());
    next.version += 1;
    next.updated_at = now;
    let event = DomainEvent {
        aggregate_type: FINDING_AGGREGATE,
        aggregate_id: next.id.clone(),
        aggregate_version: next.version,
        event_type: EVENT_FINDING_CLASSIFIED,
        occurred_at: now,
        payload: serde_json::json!({
            "review_id": next.review_id,
            "axis": next.axis,
            "state": next.state,
            "classification": next.classification,
            "reason": next.reason,
            "decision_request_id": next.decision_request_id,
            "decided_by": next.decided_by,
            "classified_by": next.classified_by,
        }),
    };
    Ok((next, event))
}

// ---------------------------------------------------------------- 設計偏離

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviationStatus {
    Proposed,
    /// 接受已送裁定，等待使用者。
    AwaitingDecision,
    Accepted,
    Rejected,
}

impl DeviationStatus {
    pub const ALL: [DeviationStatus; 4] =
        [DeviationStatus::Proposed, DeviationStatus::AwaitingDecision, DeviationStatus::Accepted, DeviationStatus::Rejected];

    pub fn as_str(self) -> &'static str {
        match self {
            DeviationStatus::Proposed => "proposed",
            DeviationStatus::AwaitingDecision => "awaiting_decision",
            DeviationStatus::Accepted => "accepted",
            DeviationStatus::Rejected => "rejected",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.as_str() == raw)
    }

    pub fn is_final(self) -> bool {
        matches!(self, DeviationStatus::Accepted | DeviationStatus::Rejected)
    }
}

/// Worker 回報的偏離。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviationInput {
    /// Design Package 所在的成果。
    pub design_artifact_id: String,
    /// 偏離時對照的設計版本（成果版本），必須是目前的最新版本。
    pub design_version: i64,
    /// 偏離了什麼。
    pub deviation: String,
    pub reason: String,
    /// 受影響的契約（介面、資料格式、需求條款…）。
    #[serde(default)]
    pub affected_contracts: Vec<String>,
    /// 接受後的完整設計內容（成為設計的下一個版本）。
    pub proposed_design: String,
}

/// 回報偏離的 Worker。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reporter {
    pub workspace_id: String,
    pub run_id: String,
    pub task_id: String,
    pub generation: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesignDeviation {
    pub id: String,
    pub workspace_id: String,
    pub task_id: String,
    pub reported_by_run_id: String,
    pub reported_generation: i64,
    pub design_artifact_id: String,
    pub design_version: i64,
    pub deviation: String,
    pub reason: String,
    pub affected_contracts: Vec<String>,
    pub proposed_design: String,
    pub status: DeviationStatus,
    /// Controller 處理時的理由。
    pub resolution_reason: Option<String>,
    pub decision_request_id: Option<String>,
    /// 接受者（使用者裁定，桌面）。
    pub accepted_by: Option<String>,
    /// 最後處理的 Controller 一輪。
    pub resolved_by: Option<String>,
    /// 接受後設計的新版本。
    pub accepted_design_version: Option<i64>,
    pub version: i64,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

fn deviation_event(deviation: &DesignDeviation, event_type: &'static str) -> DomainEvent {
    DomainEvent {
        aggregate_type: DEVIATION_AGGREGATE,
        aggregate_id: deviation.id.clone(),
        aggregate_version: deviation.version,
        event_type,
        occurred_at: deviation.updated_at,
        payload: serde_json::json!({
            "task_id": deviation.task_id,
            "design_artifact_id": deviation.design_artifact_id,
            "design_version": deviation.design_version,
            "status": deviation.status,
            "deviation": deviation.deviation,
            "reason": deviation.reason,
            "affected_contracts": deviation.affected_contracts,
            "resolution_reason": deviation.resolution_reason,
            "decision_request_id": deviation.decision_request_id,
            "accepted_by": deviation.accepted_by,
            "resolved_by": deviation.resolved_by,
            "accepted_design_version": deviation.accepted_design_version,
        }),
    }
}

/// 回報偏離：`design_latest` 是設計成果目前的最新版本（找不到成果時 `None`）。
pub fn report_deviation(
    id: String,
    input: DeviationInput,
    reporter: &Reporter,
    design_latest: Option<i64>,
    now: Timestamp,
) -> Result<(DesignDeviation, DomainEvent), ReviewError> {
    let Some(current) = design_latest else {
        return Err(ReviewError::DesignNotFound(input.design_artifact_id));
    };
    if input.design_version != current {
        return Err(ReviewError::DesignOutdated { reported: input.design_version, current });
    }
    if input.affected_contracts.len() > CONTRACTS_MAX {
        return Err(invalid(format!("affected_contracts 最多 {CONTRACTS_MAX} 項")));
    }
    let affected_contracts = input
        .affected_contracts
        .iter()
        .map(|c| text(c, PATH_MAX_CHARS, "affected_contracts 的每一項", true))
        .collect::<Result<Vec<_>, _>>()?;
    let deviation = DesignDeviation {
        id,
        workspace_id: reporter.workspace_id.clone(),
        task_id: reporter.task_id.clone(),
        reported_by_run_id: reporter.run_id.clone(),
        reported_generation: reporter.generation,
        design_artifact_id: input.design_artifact_id.trim().to_string(),
        design_version: input.design_version,
        deviation: text(&input.deviation, DETAIL_MAX_CHARS, "deviation（偏離了什麼）", true)?,
        reason: text(&input.reason, REASON_MAX_CHARS, "reason（偏離的原因）", true)?,
        affected_contracts,
        proposed_design: text(&input.proposed_design, DESIGN_MAX_CHARS, "proposed_design（接受後的完整設計）", true)?,
        status: DeviationStatus::Proposed,
        resolution_reason: None,
        decision_request_id: None,
        accepted_by: None,
        resolved_by: None,
        accepted_design_version: None,
        version: 1,
        created_at: now,
        updated_at: now,
    };
    let event = deviation_event(&deviation, EVENT_DEVIATION_REPORTED);
    Ok((deviation, event))
}

/// 處理偏離的結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviationResolution {
    pub status: DeviationStatus,
    pub reason: String,
    pub decision_request_id: Option<String>,
    pub accepted_by: Option<String>,
    pub accepted_design_version: Option<i64>,
}

/// 套用處理結果：已接受或已拒絕的偏離不能再處理。
pub fn resolve_deviation(
    deviation: &DesignDeviation,
    resolution: DeviationResolution,
    resolved_by: &str,
    now: Timestamp,
) -> Result<(DesignDeviation, DomainEvent), ReviewError> {
    if deviation.status.is_final() {
        return Err(ReviewError::DeviationResolved(deviation.id.clone()));
    }
    let mut next = deviation.clone();
    next.status = resolution.status;
    next.resolution_reason = Some(text(&resolution.reason, REASON_MAX_CHARS, "reason（處理的理由）", true)?);
    next.decision_request_id = resolution.decision_request_id;
    next.accepted_by = resolution.accepted_by;
    next.accepted_design_version = resolution.accepted_design_version;
    next.resolved_by = Some(resolved_by.to_string());
    next.version += 1;
    next.updated_at = now;
    let event = deviation_event(&next, EVENT_DEVIATION_RESOLVED);
    Ok((next, event))
}

#[cfg(test)]
mod tests;
