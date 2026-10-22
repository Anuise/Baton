//! 方法登錄、Playbook 路由與 Stage／Gate（票 43，issue #3 §9.1、§9.2、§11.5）。
//!
//! - **Method Registry**：方法的用途、輸入、產出、適用條件、工具需求、預算、停止條件、驗證要求與來源，以版本保存。
//! - **Playbook**：一種工作情境的流程，由 Stage 組成；每個 Stage 宣告 entry_conditions、expected_outputs、allowed_actions、
//!   budget、exit_checks、failure_route 與 Gate 種類。Method 與 Playbook 版本**不可原地覆寫**：內容改變就追加新版本。
//! - **Router**：依情境（與 Task 的工作種類）選 Playbook 的最新版本，把理由與版本記在 Workflow 上；
//!   研究與文件 Task 不套用開發流程。
//! - **Workflow**：固定所選的 Playbook 版本（升級不改變進行中的 Workflow）；Stage 以 key 保持穩定身分，每次回報結果都追加一筆
//!   stage attempt。Runtime 依**證據**推進：exit check 都有經 Runtime 查驗的證據才算通過；缺證據記 incomplete，
//!   等待裁定記 blocked；只說「做完了」不推進。跳過 Stage 只能由使用者在桌面進行並保存原因，裁定與安全 Gate 不能跳過。
//! - Workflow 與 Run、帳號無關：換帳號（Controller 或 Worker 交接）後由持久化的目前 Stage 接續，不從頭跑。
//! - **Gate 失效與重驗（票 50）**：每次通過記下當時的 build；Runtime 之後重新查驗已通過 Gate 的證據與 build，
//!   依賴已改變（規格、build、Verification Profile、被推翻的假設）的 Gate 失效，Workflow 回到最早失效的 Stage 重驗；
//!   沒有受影響的 Gate 保持通過，重驗後直接沿用（見 `gate` 模組）。

use serde::{Deserialize, Serialize};

use crate::artifact::sha256_hex;
use crate::event::DomainEvent;
use crate::workdir::TaskKind;
use crate::Timestamp;

mod builtin;
mod gate;
pub use builtin::{builtin_methods, builtin_playbooks};
pub use gate::{passed_attempts, revalidate, GateFacts, GateInvalidation};

/// Controller 依情境開始一個 Workflow 的工具名稱（Controller 專屬）。
pub const TOOL_START_WORKFLOW: &str = "start_workflow";
/// Controller 以證據回報目前 Stage 結果的工具名稱（Controller 專屬）。
pub const TOOL_RECORD_STAGE_RESULT: &str = "record_stage_result";
/// Controller 讀取可用 Playbook 與這個 Workspace 的 Workflow 的工具名稱（Controller 專屬）。
pub const TOOL_GET_WORKFLOWS: &str = "get_workflows";

pub const WORKFLOW_AGGREGATE: &str = "workflow";
pub const PLAYBOOK_AGGREGATE: &str = "playbook";
pub const METHOD_AGGREGATE: &str = "method";
pub const EVENT_WORKFLOW_STARTED: &str = "workflow_started";
pub const EVENT_STAGE_ATTEMPT_RECORDED: &str = "stage_attempt_recorded";
pub const EVENT_PLAYBOOK_VERSION_PUBLISHED: &str = "playbook_version_published";
pub const EVENT_METHOD_VERSION_PUBLISHED: &str = "method_version_published";
/// 已通過的 Gate 因依賴改變而失效（票 50）。
pub const EVENT_GATE_INVALIDATED: &str = "gate_invalidated";

/// 桌面（使用者）在 attempt 上的記錄者識別。
pub const RECORDED_BY_DESKTOP: &str = "desktop";
/// Runtime 自己記錄的 attempt（Gate 失效，票 50）。
pub const RECORDED_BY_RUNTIME: &str = "runtime";

pub const KEY_MAX_CHARS: usize = 64;
pub const TITLE_MAX_CHARS: usize = 200;
pub const TEXT_MAX_CHARS: usize = 2_000;
pub const LIST_MAX: usize = 20;
pub const STAGES_MAX: usize = 20;
pub const MAX_ATTEMPTS_LIMIT: u32 = 10;
pub const EVIDENCE_MAX: usize = 20;
pub const NOTE_MAX_CHARS: usize = 4_000;
/// 一項審查 exit check 最多要求幾位 reviewer（票 44）。
pub const REVIEWERS_MAX: u32 = 5;

// ---------------------------------------------------------------- 情境

/// 工作情境（issue §9.1 的七種）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Situation {
    /// 需求仍模糊。
    Ambiguous,
    /// 功能開發。
    Feature,
    /// 缺陷處理。
    Defect,
    /// 架構或廣泛遷移。
    Migration,
    /// 效能改善。
    Performance,
    /// 調查與研究。
    Investigation,
    /// 探索測試。
    ExploratoryTesting,
}

impl Situation {
    pub const ALL: [Situation; 7] = [
        Situation::Ambiguous,
        Situation::Feature,
        Situation::Defect,
        Situation::Migration,
        Situation::Performance,
        Situation::Investigation,
        Situation::ExploratoryTesting,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Situation::Ambiguous => "ambiguous",
            Situation::Feature => "feature",
            Situation::Defect => "defect",
            Situation::Migration => "migration",
            Situation::Performance => "performance",
            Situation::Investigation => "investigation",
            Situation::ExploratoryTesting => "exploratory_testing",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.as_str() == raw)
    }

    pub fn label(self) -> &'static str {
        match self {
            Situation::Ambiguous => "需求仍模糊",
            Situation::Feature => "功能開發",
            Situation::Defect => "缺陷處理",
            Situation::Migration => "架構或廣泛遷移",
            Situation::Performance => "效能改善",
            Situation::Investigation => "調查與研究",
            Situation::ExploratoryTesting => "探索測試",
        }
    }

    /// 這個情境的流程是否是開發流程（會修改程式）。研究與文件 Task 不套用開發流程。
    pub fn is_development(self) -> bool {
        !matches!(self, Situation::Ambiguous | Situation::Investigation)
    }
}

/// 版本的來源：內建（隨 App 提供），或使用者在桌面發布。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    Builtin,
    User,
}

impl Origin {
    pub fn as_str(self) -> &'static str {
        match self {
            Origin::Builtin => "builtin",
            Origin::User => "user",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        [Origin::Builtin, Origin::User].into_iter().find(|o| o.as_str() == raw)
    }
}

// ---------------------------------------------------------------- Method Registry

/// 一個方法的內容。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MethodDefinition {
    pub key: String,
    pub title: String,
    /// 用途。
    pub purpose: String,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    /// 適用條件。
    pub applicability: String,
    /// 工具需求。
    pub tool_requirements: Vec<String>,
    /// 預算（例如嘗試次數、時間）。
    pub budget: String,
    pub stop_conditions: Vec<String>,
    /// 驗證要求。
    pub verification: String,
    /// 來源（概念出處）。
    pub source: String,
}

/// 方法的一個不可變版本。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MethodVersion {
    pub id: String,
    pub key: String,
    pub version: i64,
    pub definition: MethodDefinition,
    pub content_hash: String,
    pub origin: Origin,
    pub created_at: Timestamp,
}

// ---------------------------------------------------------------- Playbook 與 Stage

/// Stage 的 Gate 種類：裁定與安全 Gate 不能跳過。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateKind {
    /// 一般 Stage：使用者可以附原因跳過。
    None,
    /// 使用者裁定：必須有已回答的裁定作為證據。
    Decision,
    /// 安全門檻（驗證、審查、整合）：不能跳過。
    Safety,
}

impl GateKind {
    pub fn skippable(self) -> bool {
        self == GateKind::None
    }

    pub fn label(self) -> &'static str {
        match self {
            GateKind::None => "一般",
            GateKind::Decision => "裁定 Gate",
            GateKind::Safety => "安全 Gate",
        }
    }
}

/// exit check 需要的證據種類；每一種都由 Runtime 查驗，不接受文字宣告。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    /// 這個 Workspace 已登記的成果（Artifact）。
    Artifact,
    /// 這個 Workspace 已回答的裁定。
    Decision,
    /// 已通過成果接受（最小完成驗證）的 Task。
    TaskAccepted,
    /// 非作者 reviewer 的雙軸審查（票 44）：兩軸都通過、對照目前的規格版本、是這個 Task 最新 candidate 的審查。
    Review,
    /// 判定為 passed、而且依據目前 Profile 版本與需求版本的 Evidence Bundle（票 45）。
    Verification,
}

/// 離開 Stage 前必須滿足的一項檢查。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExitCheck {
    pub key: String,
    pub description: String,
    pub requires: EvidenceKind,
    /// 審查 exit check 需要幾位不同的非作者 reviewer（每一軸各自計算）；省略為 1。只用於 `review`（票 44）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewers: Option<u32>,
}

impl ExitCheck {
    /// 審查 exit check 需要的 reviewer 人數。
    pub fn required_reviewers(&self) -> usize {
        self.reviewers.unwrap_or(1) as usize
    }
}

/// 回報失敗時的去向。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FailureRoute {
    /// 留在這個 Stage 重試（在預算內）。
    Retry,
    /// 回到指定的 Stage。
    ReturnTo { stage: String },
    /// 阻擋 Workflow，等待使用者決定下一步。
    Block,
}

/// Stage 的預算。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StageBudget {
    /// 這個 Stage 最多可以失敗幾次；用盡時 Workflow 被阻擋。
    pub max_attempts: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StageDefinition {
    /// 在 Playbook 內穩定的識別。
    pub key: String,
    pub title: String,
    /// 這個 Stage 採用的方法（Method Registry 的 key）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    pub entry_conditions: Vec<String>,
    pub expected_outputs: Vec<String>,
    pub allowed_actions: Vec<String>,
    pub budget: StageBudget,
    pub exit_checks: Vec<ExitCheck>,
    pub failure_route: FailureRoute,
    pub gate: GateKind,
}

/// 一個 Playbook 的內容。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaybookDefinition {
    pub key: String,
    pub title: String,
    pub situation: Situation,
    /// 適用條件（Router 的理由會引用）。
    pub applicability: String,
    pub stages: Vec<StageDefinition>,
}

impl PlaybookDefinition {
    pub fn stage(&self, key: &str) -> Option<&StageDefinition> {
        self.stages.iter().find(|s| s.key == key)
    }

    fn stage_index(&self, key: &str) -> Option<usize> {
        self.stages.iter().position(|s| s.key == key)
    }
}

/// Playbook 的一個不可變版本。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaybookVersion {
    pub id: String,
    pub key: String,
    pub version: i64,
    pub definition: PlaybookDefinition,
    pub content_hash: String,
    pub origin: Origin,
    pub created_at: Timestamp,
}

// ---------------------------------------------------------------- 錯誤

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WorkflowError {
    #[error("內容不正確：{0}")]
    Invalid(String),
    #[error("Playbook 內容不正確：{0}")]
    PlaybookInvalid(String),
    #[error("{0}")]
    NotApplicable(String),
    #[error("找不到 Playbook {0}")]
    PlaybookNotFound(String),
    #[error("找不到 Workflow {0}")]
    WorkflowNotFound(String),
    #[error("Stage {stage} 不是目前的 Stage（目前是 {current}）；已通過的 Stage 不重跑，請回報目前 Stage 的結果")]
    StageNotCurrent { stage: String, current: String },
    #[error("Workflow 的版本是 {actual}，不是 {expected}；請重新讀取後再回報")]
    RevisionMismatch { expected: i64, actual: i64 },
    #[error("Stage {stage} 是{gate}，不能跳過")]
    NotSkippable { stage: String, gate: &'static str },
    #[error("Workflow {0} 已完成")]
    Completed(String),
    #[error("Task {task_id} 已經有進行中的 Workflow {workflow_id}")]
    TaskHasWorkflow { task_id: String, workflow_id: String },
}

impl WorkflowError {
    /// 穩定的機器可讀代碼。
    pub fn code(&self) -> &'static str {
        match self {
            WorkflowError::Invalid(_) => "workflow_invalid",
            WorkflowError::PlaybookInvalid(_) => "playbook_invalid",
            WorkflowError::NotApplicable(_) => "playbook_not_applicable",
            WorkflowError::PlaybookNotFound(_) => "playbook_not_found",
            WorkflowError::WorkflowNotFound(_) => "workflow_not_found",
            WorkflowError::StageNotCurrent { .. } => "stage_not_current",
            WorkflowError::RevisionMismatch { .. } => "workflow_revision_mismatch",
            WorkflowError::NotSkippable { .. } => "stage_not_skippable",
            WorkflowError::Completed(_) => "workflow_completed",
            WorkflowError::TaskHasWorkflow { .. } => "workflow_exists",
        }
    }
}

fn invalid(message: impl Into<String>) -> WorkflowError {
    WorkflowError::Invalid(message.into())
}

fn bounded(raw: &str, max: usize, label: &str) -> Result<String, WorkflowError> {
    let text = raw.trim();
    if text.chars().count() > max {
        return Err(invalid(format!("{label}最多 {max} 個字元")));
    }
    Ok(text.to_string())
}

// ---------------------------------------------------------------- 版本

fn hash_of<T: Serialize>(value: &T) -> String {
    sha256_hex(&serde_json::to_vec(value).expect("內容一定可以序列化"))
}

fn playbook_invalid(message: impl Into<String>) -> WorkflowError {
    WorkflowError::PlaybookInvalid(message.into())
}

fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.chars().count() <= KEY_MAX_CHARS
        && key.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn check_list(list: &[String], label: &str) -> Result<(), WorkflowError> {
    if list.len() > LIST_MAX {
        return Err(playbook_invalid(format!("{label}最多 {LIST_MAX} 項")));
    }
    if list.iter().any(|t| t.trim().is_empty() || t.chars().count() > TEXT_MAX_CHARS) {
        return Err(playbook_invalid(format!("{label}的每一項都不能空白，且最多 {TEXT_MAX_CHARS} 個字元")));
    }
    Ok(())
}

/// 發布前的檢查：結構完整、Stage 與 exit check 識別不重複、失敗去向指到存在的 Stage、方法已登錄、預算在範圍內，
/// 每個 Stage 至少一項 exit check（Stage 只能依證據推進），裁定 Gate 至少一項要求裁定證據。
pub fn validate_playbook(definition: &PlaybookDefinition, method_keys: &[&str]) -> Result<(), WorkflowError> {
    if !valid_key(&definition.key) {
        return Err(playbook_invalid("key 只能是 1～64 個小寫英文字母、數字或底線"));
    }
    if definition.title.trim().is_empty() || definition.title.chars().count() > TITLE_MAX_CHARS {
        return Err(playbook_invalid(format!("title 不能空白，且最多 {TITLE_MAX_CHARS} 個字元")));
    }
    if definition.applicability.chars().count() > TEXT_MAX_CHARS {
        return Err(playbook_invalid(format!("applicability 最多 {TEXT_MAX_CHARS} 個字元")));
    }
    if definition.stages.is_empty() || definition.stages.len() > STAGES_MAX {
        return Err(playbook_invalid(format!("Playbook 需要 1～{STAGES_MAX} 個 Stage")));
    }
    let mut seen = Vec::new();
    for stage in &definition.stages {
        let at = format!("Stage {}", stage.key);
        if !valid_key(&stage.key) {
            return Err(playbook_invalid("Stage 的 key 只能是 1～64 個小寫英文字母、數字或底線"));
        }
        if seen.contains(&stage.key.as_str()) {
            return Err(playbook_invalid(format!("{at} 重複")));
        }
        seen.push(&stage.key);
        if stage.title.trim().is_empty() || stage.title.chars().count() > TITLE_MAX_CHARS {
            return Err(playbook_invalid(format!("{at} 的 title 不能空白，且最多 {TITLE_MAX_CHARS} 個字元")));
        }
        if let Some(method) = &stage.method {
            if !method_keys.contains(&method.as_str()) {
                return Err(playbook_invalid(format!("{at} 引用的方法 {method} 不在 Method Registry 中")));
            }
        }
        check_list(&stage.entry_conditions, &format!("{at} 的 entry_conditions"))?;
        check_list(&stage.expected_outputs, &format!("{at} 的 expected_outputs"))?;
        check_list(&stage.allowed_actions, &format!("{at} 的 allowed_actions"))?;
        if stage.budget.max_attempts == 0 || stage.budget.max_attempts > MAX_ATTEMPTS_LIMIT {
            return Err(playbook_invalid(format!("{at} 的 budget.max_attempts 必須是 1～{MAX_ATTEMPTS_LIMIT}")));
        }
        if stage.exit_checks.is_empty() || stage.exit_checks.len() > LIST_MAX {
            return Err(playbook_invalid(format!("{at} 需要 1～{LIST_MAX} 項 exit check（Stage 只能依證據推進）")));
        }
        let mut checks = Vec::new();
        for check in &stage.exit_checks {
            if !valid_key(&check.key) || checks.contains(&check.key.as_str()) {
                return Err(playbook_invalid(format!("{at} 的 exit check key 必須是不重複的小寫英文字母、數字或底線")));
            }
            checks.push(&check.key);
            if check.description.trim().is_empty() || check.description.chars().count() > TEXT_MAX_CHARS {
                return Err(playbook_invalid(format!("{at} 的 exit check {} 需要說明", check.key)));
            }
            if let Some(n) = check.reviewers {
                if check.requires != EvidenceKind::Review || n == 0 || n > REVIEWERS_MAX {
                    return Err(playbook_invalid(format!(
                        "{at} 的 exit check {}：reviewers 只能用在 review，而且是 1～{REVIEWERS_MAX}",
                        check.key
                    )));
                }
            }
        }
        if stage.gate == GateKind::Decision && !stage.exit_checks.iter().any(|c| c.requires == EvidenceKind::Decision) {
            return Err(playbook_invalid(format!("{at} 是裁定 Gate，至少一項 exit check 要求已回答的裁定")));
        }
    }
    for stage in &definition.stages {
        if let FailureRoute::ReturnTo { stage: target } = &stage.failure_route {
            if target == &stage.key || definition.stage(target).is_none() {
                return Err(playbook_invalid(format!("Stage {} 的 failure_route 指到不存在（或自己）的 Stage {target}", stage.key)));
            }
        }
    }
    Ok(())
}

/// 發布 Playbook 的新版本：與同一個 key 的最新版本內容相同時不產生新版本（回 `None`）。
/// 使用者只能升級既有的 Playbook，不能改它的情境（每種情境一個 Playbook，Router 才有唯一的選擇）。
pub fn publish_playbook(
    latest: Option<&PlaybookVersion>,
    definition: PlaybookDefinition,
    method_keys: &[&str],
    origin: Origin,
    id: String,
    now: Timestamp,
) -> Result<Option<(PlaybookVersion, DomainEvent)>, WorkflowError> {
    validate_playbook(&definition, method_keys)?;
    if let Some(latest) = latest {
        if latest.definition.situation != definition.situation {
            return Err(playbook_invalid(format!(
                "Playbook {} 的情境是 {}，新版本不能改成 {}",
                definition.key,
                latest.definition.situation.as_str(),
                definition.situation.as_str()
            )));
        }
    }
    let content_hash = hash_of(&definition);
    if latest.is_some_and(|l| l.content_hash == content_hash) {
        return Ok(None);
    }
    let version = latest.map_or(1, |l| l.version + 1);
    let playbook = PlaybookVersion { id, key: definition.key.clone(), version, definition, content_hash, origin, created_at: now };
    let event = DomainEvent {
        aggregate_type: PLAYBOOK_AGGREGATE,
        aggregate_id: playbook.key.clone(),
        aggregate_version: version,
        event_type: EVENT_PLAYBOOK_VERSION_PUBLISHED,
        occurred_at: now,
        payload: serde_json::json!({
            "playbook_version_id": playbook.id,
            "key": playbook.key,
            "version": version,
            "content_hash": playbook.content_hash,
            "origin": origin.as_str(),
        }),
    };
    Ok(Some((playbook, event)))
}

/// 發布方法的新版本；內容與最新版本相同時不產生新版本。
pub fn publish_method(
    latest: Option<&MethodVersion>,
    definition: MethodDefinition,
    origin: Origin,
    id: String,
    now: Timestamp,
) -> Result<Option<(MethodVersion, DomainEvent)>, WorkflowError> {
    if !valid_key(&definition.key) || definition.title.trim().is_empty() {
        return Err(playbook_invalid("方法需要 key（小寫英文字母、數字或底線）與 title"));
    }
    let content_hash = hash_of(&definition);
    if latest.is_some_and(|l| l.content_hash == content_hash) {
        return Ok(None);
    }
    let version = latest.map_or(1, |l| l.version + 1);
    let method = MethodVersion { id, key: definition.key.clone(), version, definition, content_hash, origin, created_at: now };
    let event = DomainEvent {
        aggregate_type: METHOD_AGGREGATE,
        aggregate_id: method.key.clone(),
        aggregate_version: version,
        event_type: EVENT_METHOD_VERSION_PUBLISHED,
        occurred_at: now,
        payload: serde_json::json!({
            "method_version_id": method.id,
            "key": method.key,
            "version": version,
            "content_hash": method.content_hash,
            "origin": origin.as_str(),
        }),
    };
    Ok(Some((method, event)))
}

// ---------------------------------------------------------------- Router

/// Router 的選擇：所選 Playbook 的版本與理由。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Route {
    pub playbook_version_id: String,
    pub playbook_key: String,
    pub playbook_version: i64,
    pub situation: Situation,
    pub reason: String,
}

/// 依情境選 Playbook：`latest` 是每個 Playbook 的最新版本。
/// 綁定的 Task 是研究或文件工作時，不套用開發流程（功能、缺陷、遷移、效能、探索測試），改由 Controller 選調查或釐清流程。
pub fn route(
    latest: &[PlaybookVersion],
    situation: Situation,
    task_kind: Option<TaskKind>,
    rationale: &str,
) -> Result<Route, WorkflowError> {
    let rationale = bounded(rationale, TEXT_MAX_CHARS, "rationale ")?;
    if rationale.is_empty() {
        return Err(invalid("請在 rationale 說明為什麼判斷是這個情境"));
    }
    if let Some(kind) = task_kind {
        if kind != TaskKind::Development && situation.is_development() {
            return Err(WorkflowError::NotApplicable(format!(
                "Task 是{}工作，不套用開發流程（{}）；研究與文件工作請用 investigation（調查與研究）或 ambiguous（需求仍模糊）",
                kind.label(),
                situation.label()
            )));
        }
    }
    let playbook = latest
        .iter()
        .filter(|p| p.definition.situation == situation)
        .max_by_key(|p| (p.created_at, p.version))
        .ok_or_else(|| WorkflowError::PlaybookNotFound(situation.as_str().to_string()))?;
    let reason = format!(
        "情境「{}」→ {}（{}）第 {} 版。適用：{}。Controller 的判斷：{}",
        situation.label(),
        playbook.definition.title,
        playbook.key,
        playbook.version,
        playbook.definition.applicability,
        rationale
    );
    Ok(Route {
        playbook_version_id: playbook.id.clone(),
        playbook_key: playbook.key.clone(),
        playbook_version: playbook.version,
        situation,
        reason,
    })
}

// ---------------------------------------------------------------- Workflow

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowStatus {
    Active,
    /// 等待裁定、預算用盡、失敗去向是阻擋，或 Controller 回報阻擋。
    Blocked,
    Completed,
}

impl WorkflowStatus {
    pub const ALL: [WorkflowStatus; 3] = [WorkflowStatus::Active, WorkflowStatus::Blocked, WorkflowStatus::Completed];

    pub fn as_str(self) -> &'static str {
        match self {
            WorkflowStatus::Active => "active",
            WorkflowStatus::Blocked => "blocked",
            WorkflowStatus::Completed => "completed",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.as_str() == raw)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowInstance {
    pub id: String,
    pub workspace_id: String,
    pub goal_id: String,
    #[serde(default)]
    pub task_id: Option<String>,
    /// 固定的 Playbook 版本；之後發布的新版本不影響這個 Workflow。
    pub playbook_version_id: String,
    pub playbook_key: String,
    pub playbook_version: i64,
    pub situation: Situation,
    pub route_reason: String,
    pub status: WorkflowStatus,
    /// 目前的 Stage；完成後為 `None`。
    pub current_stage: Option<String>,
    pub blocked_reason: Option<String>,
    /// 每次變更遞增；回報 Stage 結果與跳過都要帶上讀到的版本。
    pub revision: i64,
    /// Gate 失效使 Workflow 退回較早的 Stage 時，失效前的目前 Stage（票 50）：重驗通過後，這之前仍有效通過的 Stage 直接沿用。
    #[serde(default)]
    pub resume_stage: Option<String>,
    pub created_by: String,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

/// 一項證據的引用（由 Controller 提供，Runtime 查驗）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EvidenceRef {
    Artifact { artifact_id: String },
    Decision { decision_request_id: String },
    TaskAccepted { task_id: String },
    Review { review_id: String },
    Verification { evidence_bundle_id: String },
}

impl EvidenceRef {
    pub fn kind(&self) -> EvidenceKind {
        match self {
            EvidenceRef::Artifact { .. } => EvidenceKind::Artifact,
            EvidenceRef::Decision { .. } => EvidenceKind::Decision,
            EvidenceRef::TaskAccepted { .. } => EvidenceKind::TaskAccepted,
            EvidenceRef::Review { .. } => EvidenceKind::Review,
            EvidenceRef::Verification { .. } => EvidenceKind::Verification,
        }
    }
}

/// 工具請求中的一項證據：對應的 exit check，以及剛好一個引用。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceInput {
    pub check: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision_request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_bundle_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evidence {
    pub check: String,
    #[serde(flatten)]
    pub reference: EvidenceRef,
}

/// Runtime 查驗一項證據的結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceStatus {
    /// 查驗通過。
    Valid,
    /// 這個 Workspace 沒有這個成果、裁定或 Task。
    NotFound,
    /// 裁定還沒有回答。
    Pending,
    /// Task 還沒有通過成果接受。
    NotAccepted,
    /// 審查還有未解決的 finding（任一軸）；或 Evidence Bundle 的判定不是 passed，或它依據的 Profile 版本、需求版本已經不是目前的版本（票 45）。
    NotPassed,
    /// 審查已過期：規格版本改變，或已有對較新 candidate 的審查。
    Stale,
    /// 審查審的不是這個 Workflow 的 Task。
    OtherSubject,
    /// Evidence Bundle 驗證的 build（candidate revision）已不是受驗目錄目前的 revision（票 50）。
    BuildChanged,
}

/// 一份審查兩軸各自實際審查的 reviewer（Task 識別）；沿用的軸是原本的 reviewer。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AxisReviewers {
    pub standards: String,
    pub spec: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckedEvidence {
    #[serde(flatten)]
    pub evidence: Evidence,
    pub status: EvidenceStatus,
    /// 審查證據的 reviewer（Gate 依此計算人數）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewers: Option<AxisReviewers>,
}

/// 整理工具請求中的證據：每項剛好一個引用、數量有上限。
pub fn normalize_evidence(inputs: &[EvidenceInput]) -> Result<Vec<Evidence>, WorkflowError> {
    if inputs.len() > EVIDENCE_MAX {
        return Err(invalid(format!("evidence 最多 {EVIDENCE_MAX} 項")));
    }
    inputs
        .iter()
        .map(|input| {
            let check = input.check.trim().to_string();
            if check.is_empty() {
                return Err(invalid("每項 evidence 都要指明 check（exit check 的 key）"));
            }
            let present = |v: &Option<String>| v.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
            let reference = match (
                present(&input.artifact_id),
                present(&input.decision_request_id),
                present(&input.task_id),
                present(&input.review_id),
                present(&input.evidence_bundle_id),
            ) {
                (Some(artifact_id), None, None, None, None) => EvidenceRef::Artifact { artifact_id },
                (None, Some(decision_request_id), None, None, None) => EvidenceRef::Decision { decision_request_id },
                (None, None, Some(task_id), None, None) => EvidenceRef::TaskAccepted { task_id },
                (None, None, None, Some(review_id), None) => EvidenceRef::Review { review_id },
                (None, None, None, None, Some(evidence_bundle_id)) => EvidenceRef::Verification { evidence_bundle_id },
                _ => {
                    return Err(invalid(format!(
                        "evidence {check} 必須剛好提供 artifact_id、decision_request_id、task_id、review_id 或 evidence_bundle_id 其中一個"
                    )))
                }
            };
            Ok(Evidence { check, reference })
        })
        .collect()
}

/// Controller 回報的結果。只說「通過」不夠：Runtime 依證據判定是 passed、incomplete 或 blocked。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimedOutcome {
    Passed,
    Failed,
    Blocked,
}

/// 一次 stage attempt 的結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptOutcome {
    /// 每項 exit check 都有查驗通過的證據。
    Passed,
    /// 缺少必要證據（或證據查驗不通過）：留在這個 Stage。
    Incomplete,
    /// 等待裁定，或 Controller 回報無法繼續（例如沒有獨立 reviewer）。
    Blocked,
    /// 這個 Stage 的工作失敗，依 failure_route 處理。
    Failed,
    /// 使用者附原因跳過。
    Skipped,
    /// 先前的通過因依賴改變而失效，需要重驗（Runtime 記錄，票 50）。
    Invalidated,
}

impl AttemptOutcome {
    pub const ALL: [AttemptOutcome; 6] = [
        AttemptOutcome::Passed,
        AttemptOutcome::Incomplete,
        AttemptOutcome::Blocked,
        AttemptOutcome::Failed,
        AttemptOutcome::Skipped,
        AttemptOutcome::Invalidated,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            AttemptOutcome::Passed => "passed",
            AttemptOutcome::Incomplete => "incomplete",
            AttemptOutcome::Blocked => "blocked",
            AttemptOutcome::Failed => "failed",
            AttemptOutcome::Skipped => "skipped",
            AttemptOutcome::Invalidated => "invalidated",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|o| o.as_str() == raw)
    }
}

/// 沒有被滿足的 exit check 與原因。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissingCheck {
    pub check: String,
    pub reason: String,
}

/// 一個 Stage 的一次嘗試（只追加）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StageAttempt {
    pub id: String,
    pub workflow_id: String,
    pub stage_key: String,
    /// 同一個 Stage 內從 1 起遞增。
    pub attempt: i64,
    pub outcome: AttemptOutcome,
    pub evidence: Vec<CheckedEvidence>,
    pub missing: Vec<MissingCheck>,
    pub note: String,
    /// Controller 那一輪的識別，或 [`RECORDED_BY_DESKTOP`]。
    pub recorded_by: String,
    /// 記錄者所用的帳號（Controller 那一輪的帳號；桌面為 `None`）。
    pub profile_id: Option<String>,
    /// 記錄當下 Runtime 觀察到的 build（受驗目錄的 revision；不是 Git repo 或觀察不到時為 `None`，票 50）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<String>,
    pub created_at: Timestamp,
}

/// Workflow 的變更：新狀態、追加的 attempt 與事件。
#[derive(Debug, Clone, PartialEq)]
pub struct WorkflowChange {
    pub workflow: WorkflowInstance,
    pub attempt: Option<StageAttempt>,
    pub events: Vec<DomainEvent>,
}

/// 開始 Workflow 的輸入（綁定已由 API 解析）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartInput {
    pub workspace_id: String,
    pub goal_id: String,
    pub task_id: Option<String>,
    pub route: Route,
    pub created_by: String,
}

fn workflow_event(workflow: &WorkflowInstance, event_type: &'static str, payload: serde_json::Value) -> DomainEvent {
    DomainEvent {
        aggregate_type: WORKFLOW_AGGREGATE,
        aggregate_id: workflow.id.clone(),
        aggregate_version: workflow.revision,
        event_type,
        occurred_at: workflow.updated_at,
        payload,
    }
}

/// 開始 Workflow：固定 Router 選的 Playbook 版本，目前 Stage 是第一個 Stage。
/// `open_for_task` 是綁定的 Task 目前未完成的 Workflow（有就拒絕：同一個 Task 同時只走一個流程）。
pub fn start(
    id: String,
    input: StartInput,
    playbook: &PlaybookVersion,
    open_for_task: Option<&WorkflowInstance>,
    now: Timestamp,
) -> Result<WorkflowChange, WorkflowError> {
    if let (Some(task_id), Some(open)) = (&input.task_id, open_for_task) {
        return Err(WorkflowError::TaskHasWorkflow { task_id: task_id.clone(), workflow_id: open.id.clone() });
    }
    let first = playbook.definition.stages.first().ok_or_else(|| playbook_invalid("Playbook 沒有 Stage"))?;
    let workflow = WorkflowInstance {
        id,
        workspace_id: input.workspace_id,
        goal_id: input.goal_id,
        task_id: input.task_id,
        playbook_version_id: playbook.id.clone(),
        playbook_key: playbook.key.clone(),
        playbook_version: playbook.version,
        situation: input.route.situation,
        route_reason: input.route.reason,
        status: WorkflowStatus::Active,
        current_stage: Some(first.key.clone()),
        blocked_reason: None,
        revision: 1,
        resume_stage: None,
        created_by: input.created_by,
        created_at: now,
        updated_at: now,
    };
    let event = workflow_event(
        &workflow,
        EVENT_WORKFLOW_STARTED,
        serde_json::json!({
            "workspace_id": workflow.workspace_id,
            "goal_id": workflow.goal_id,
            "task_id": workflow.task_id,
            "playbook_version_id": workflow.playbook_version_id,
            "playbook_key": workflow.playbook_key,
            "playbook_version": workflow.playbook_version,
            "situation": workflow.situation.as_str(),
            "route_reason": workflow.route_reason,
            "stage": first.key,
        }),
    );
    Ok(WorkflowChange { workflow, attempt: None, events: vec![event] })
}

/// 回報目前 Stage 結果的輸入（證據已由 Runtime 查驗）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageResultInput {
    pub stage: String,
    pub expected_revision: i64,
    pub outcome: ClaimedOutcome,
    pub evidence: Vec<CheckedEvidence>,
    pub note: String,
    /// Runtime 在記錄前觀察到的 build（受驗目錄的 revision），隨 attempt 保存（票 50）。
    pub build: Option<String>,
    /// Workflow 的 Task 依據的假設已被使用者推翻的說明（票 50）：有值時不能通過，必須先更新契約。
    pub basis_overturned: Option<String>,
}

/// 這次變更的記錄者。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recorder {
    pub recorded_by: String,
    pub profile_id: Option<String>,
}

/// 確認可以對 `stage` 記錄結果：Workflow 未完成、版本相符、`stage` 是目前的 Stage。
fn current_stage<'a>(
    workflow: &WorkflowInstance,
    playbook: &'a PlaybookVersion,
    stage: &str,
    expected_revision: i64,
) -> Result<&'a StageDefinition, WorkflowError> {
    let Some(current) = workflow.current_stage.as_deref() else {
        return Err(WorkflowError::Completed(workflow.id.clone()));
    };
    if workflow.revision != expected_revision {
        return Err(WorkflowError::RevisionMismatch { expected: expected_revision, actual: workflow.revision });
    }
    if stage.trim() != current {
        return Err(WorkflowError::StageNotCurrent { stage: stage.trim().to_string(), current: current.to_string() });
    }
    playbook.definition.stage(current).ok_or_else(|| playbook_invalid(format!("Playbook 版本中沒有 Stage {current}")))
}

/// 依證據評估 exit check：全部有查驗通過的證據 → passed；有裁定還沒回答、或缺少必要的非作者 reviewer → blocked
/// （第三個值是阻擋原因）；其餘 → incomplete。
fn evaluate(stage: &StageDefinition, evidence: &[CheckedEvidence]) -> (AttemptOutcome, Vec<MissingCheck>, Option<String>) {
    let mut missing = Vec::new();
    let mut blocked = None;
    for check in &stage.exit_checks {
        let offered: Vec<&CheckedEvidence> = evidence.iter().filter(|e| e.evidence.check == check.key).collect();
        if check.requires == EvidenceKind::Review {
            if let Some((reason, absent)) = review_shortfall(check, &offered) {
                if absent && blocked.is_none() {
                    blocked = Some(format!("Stage「{}」：{reason}", stage.title));
                }
                missing.push(MissingCheck { check: check.key.clone(), reason });
            }
            continue;
        }
        if offered.iter().any(|e| e.status == EvidenceStatus::Valid) {
            continue;
        }
        let reason = if offered.is_empty() {
            format!("缺少證據：{}", check.description)
        } else if offered.iter().any(|e| e.status == EvidenceStatus::Pending) {
            blocked = Some(format!("Stage「{}」等待裁定", stage.title));
            "裁定還沒有回答".to_string()
        } else if offered.iter().any(|e| e.status == EvidenceStatus::NotAccepted) {
            "Task 還沒有通過成果接受".to_string()
        } else if offered.iter().any(|e| e.status == EvidenceStatus::NotPassed) {
            "驗證沒有通過（證據不足、環境或授權阻礙都不算通過），或依據的 Profile、需求版本已經改變".to_string()
        } else if offered.iter().any(|e| e.status == EvidenceStatus::BuildChanged) {
            "Evidence Bundle 驗證的 build 已不是受驗目錄目前的 revision，請對目前的 build 重新驗證".to_string()
        } else {
            "引用的證據在這個 Workspace 中不存在".to_string()
        };
        missing.push(MissingCheck { check: check.key.clone(), reason });
    }
    let outcome = match (missing.is_empty(), blocked.is_some()) {
        (true, _) => AttemptOutcome::Passed,
        (false, true) => AttemptOutcome::Blocked,
        (false, false) => AttemptOutcome::Incomplete,
    };
    (outcome, missing, blocked)
}

/// 審查 exit check 還差什麼：每一軸要有 `reviewers` 位不同的 reviewer 通過。沒有任何待處理的審查（只是 reviewer 不夠）
/// 回 `(原因, true)`，Gate 保持關閉並阻擋；有審查但未通過、過期或不適用時回 `(原因, false)`，需要修正後重新審查。
fn review_shortfall(check: &ExitCheck, offered: &[&CheckedEvidence]) -> Option<(String, bool)> {
    let need = check.required_reviewers();
    let valid: Vec<&AxisReviewers> =
        offered.iter().filter(|e| e.status == EvidenceStatus::Valid).filter_map(|e| e.reviewers.as_ref()).collect();
    let distinct = |pick: fn(&AxisReviewers) -> &str| {
        let mut names: Vec<&str> = valid.iter().map(|r| pick(r)).collect();
        names.sort_unstable();
        names.dedup();
        names.len()
    };
    let have = distinct(|r| &r.standards).min(distinct(|r| &r.spec));
    if have >= need {
        return None;
    }
    let reason = match offered.iter().map(|e| e.status).find(|s| *s != EvidenceStatus::Valid) {
        Some(EvidenceStatus::NotPassed) => "審查還有未解決的 finding：Standards 與 Spec 兩軸都要通過".to_string(),
        Some(EvidenceStatus::Stale) => "審查已過期（規格版本已改變，或已有對較新 candidate 的審查），請重新審查".to_string(),
        Some(EvidenceStatus::OtherSubject) => "審查審的不是這個 Workflow 的 Task".to_string(),
        Some(_) => "引用的審查在這個 Workspace 中不存在".to_string(),
        None => {
            return Some((format!("缺少非作者 reviewer 的雙軸審查：每一軸需要 {need} 位，目前 {have} 位"), true));
        }
    };
    Some((reason, false))
}

fn next_attempt_no(prior: &[StageAttempt], stage: &str) -> i64 {
    prior.iter().filter(|a| a.stage_key == stage).map(|a| a.attempt).max().unwrap_or(0) + 1
}

/// 這個 Stage 最後一次通過、跳過或失效的紀錄是否是有效的通過（或跳過）。
fn validly_passed(attempts: &[StageAttempt], stage: &str) -> bool {
    attempts
        .iter()
        .filter(|a| a.stage_key == stage)
        .filter(|a| matches!(a.outcome, AttemptOutcome::Passed | AttemptOutcome::Skipped | AttemptOutcome::Invalidated))
        .max_by_key(|a| a.attempt)
        .is_some_and(|a| a.outcome != AttemptOutcome::Invalidated)
}

/// 這個 Stage 的 Gate 已失效、還沒重新通過或跳過（票 50）。
pub fn reverification_pending(attempts: &[StageAttempt], stage: &str) -> bool {
    attempts
        .iter()
        .filter(|a| a.stage_key == stage)
        .filter(|a| matches!(a.outcome, AttemptOutcome::Passed | AttemptOutcome::Skipped | AttemptOutcome::Invalidated))
        .max_by_key(|a| a.attempt)
        .is_some_and(|a| a.outcome == AttemptOutcome::Invalidated)
}

/// 通過或跳過之後前進到下一個 Stage；最後一個 Stage 之後 Workflow 完成。
/// Gate 失效後的重驗途中（`resume_stage`），失效前的目前 Stage 之前仍有效通過的 Stage 直接沿用、不重跑（票 50）。
fn advance(workflow: &mut WorkflowInstance, playbook: &PlaybookVersion, prior: &[StageAttempt], stage: &str) {
    let index = playbook.definition.stage_index(stage).expect("目前的 Stage 一定在 Playbook 中");
    let mut next = index + 1;
    if let Some(resume) = workflow.resume_stage.as_deref().and_then(|r| playbook.definition.stage_index(r)) {
        while next < resume && validly_passed(prior, &playbook.definition.stages[next].key) {
            next += 1;
        }
        if next >= resume {
            workflow.resume_stage = None;
        }
    }
    match playbook.definition.stages.get(next) {
        Some(next) => {
            workflow.current_stage = Some(next.key.clone());
            workflow.status = WorkflowStatus::Active;
        }
        None => {
            workflow.current_stage = None;
            workflow.status = WorkflowStatus::Completed;
        }
    }
    workflow.blocked_reason = None;
}

fn attempt_event(workflow: &WorkflowInstance, attempt: &StageAttempt) -> DomainEvent {
    workflow_event(
        workflow,
        EVENT_STAGE_ATTEMPT_RECORDED,
        serde_json::json!({
            "workspace_id": workflow.workspace_id,
            "task_id": workflow.task_id,
            "attempt_id": attempt.id,
            "stage": attempt.stage_key,
            "attempt": attempt.attempt,
            "outcome": attempt.outcome.as_str(),
            "missing": attempt.missing,
            "note": attempt.note,
            "recorded_by": attempt.recorded_by,
            "profile_id": attempt.profile_id,
            "status": workflow.status.as_str(),
            "current_stage": workflow.current_stage,
        }),
    )
}

/// 回報目前 Stage 的結果。`prior` 是這個 Workflow 先前的全部 attempt。
///
/// - `passed`：依證據判定 passed／incomplete／blocked；只有 passed 才前進。
/// - `failed`：需要說明；累計失敗達預算時 Workflow 被阻擋，否則依 failure_route（重試、回到某個 Stage、阻擋）。
/// - `blocked`：需要說明（例如沒有獨立 reviewer）；Workflow 被阻擋，之後仍可對同一個 Stage 回報結果。
pub fn record_result(
    workflow: &WorkflowInstance,
    playbook: &PlaybookVersion,
    prior: &[StageAttempt],
    input: StageResultInput,
    attempt_id: String,
    recorder: Recorder,
    now: Timestamp,
) -> Result<WorkflowChange, WorkflowError> {
    let stage = current_stage(workflow, playbook, &input.stage, input.expected_revision)?;
    let note = bounded(&input.note, NOTE_MAX_CHARS, "note ")?;
    for offered in &input.evidence {
        let check = stage.exit_checks.iter().find(|c| c.key == offered.evidence.check).ok_or_else(|| {
            invalid(format!("Stage {} 沒有 exit check {}", stage.key, offered.evidence.check))
        })?;
        if check.requires != offered.evidence.reference.kind() {
            return Err(invalid(format!(
                "exit check {} 需要的證據種類是 {}",
                check.key,
                serde_json::to_value(check.requires).expect("可以序列化").as_str().unwrap_or_default()
            )));
        }
    }
    let mut next = workflow.clone();
    let (outcome, missing, blocked_by) = match input.outcome {
        // 依據的假設已被推翻：舊成果不能在錯誤的前提下通過，先更新契約（票 50）
        ClaimedOutcome::Passed if input.basis_overturned.is_some() => {
            let why = input.basis_overturned.clone().unwrap_or_default();
            let reason = format!("Stage「{}」：Task 依據的假設已被推翻（{why}），請先依使用者的答案更新契約", stage.title);
            (AttemptOutcome::Blocked, Vec::new(), Some(reason))
        }
        ClaimedOutcome::Passed => evaluate(stage, &input.evidence),
        ClaimedOutcome::Failed | ClaimedOutcome::Blocked if note.is_empty() => {
            return Err(invalid("回報失敗或阻擋時，請在 note 說明原因"));
        }
        ClaimedOutcome::Failed => (AttemptOutcome::Failed, Vec::new(), None),
        ClaimedOutcome::Blocked => (AttemptOutcome::Blocked, Vec::new(), None),
    };
    let failures = prior.iter().filter(|a| a.stage_key == stage.key && a.outcome == AttemptOutcome::Failed).count() as u32
        + u32::from(outcome == AttemptOutcome::Failed);
    let exhausted = failures >= stage.budget.max_attempts;
    match outcome {
        AttemptOutcome::Passed => advance(&mut next, playbook, prior, &stage.key),
        AttemptOutcome::Blocked => {
            next.status = WorkflowStatus::Blocked;
            next.blocked_reason = Some(blocked_by.unwrap_or_else(|| format!("Stage「{}」：{note}", stage.title)));
        }
        AttemptOutcome::Failed if exhausted => {
            next.status = WorkflowStatus::Blocked;
            next.blocked_reason = Some(format!(
                "Stage「{}」已失敗 {failures} 次，用盡預算（{} 次）；需要使用者決定下一步",
                stage.title, stage.budget.max_attempts
            ));
        }
        AttemptOutcome::Failed => match &stage.failure_route {
            FailureRoute::Retry => {
                next.status = WorkflowStatus::Active;
                next.blocked_reason = None;
            }
            FailureRoute::ReturnTo { stage: target } => {
                next.current_stage = Some(target.clone());
                next.status = WorkflowStatus::Active;
                next.blocked_reason = None;
                // 退回重做：之後的 Stage 照常重跑，不沿用 Gate 失效前的通過
                next.resume_stage = None;
            }
            FailureRoute::Block => {
                next.status = WorkflowStatus::Blocked;
                next.blocked_reason = Some(format!("Stage「{}」失敗：{note}", stage.title));
            }
        },
        // 缺證據：留在這個 Stage；預算已用盡的阻擋不因此解除
        AttemptOutcome::Incomplete if workflow.status == WorkflowStatus::Blocked && exhausted => {}
        AttemptOutcome::Incomplete => {
            next.status = WorkflowStatus::Active;
            next.blocked_reason = None;
        }
        AttemptOutcome::Skipped | AttemptOutcome::Invalidated => unreachable!("跳過只經由 skip_stage，失效只經由 revalidate"),
    }
    next.revision += 1;
    next.updated_at = now;
    let attempt = StageAttempt {
        id: attempt_id,
        workflow_id: workflow.id.clone(),
        stage_key: stage.key.clone(),
        attempt: next_attempt_no(prior, &stage.key),
        outcome,
        evidence: input.evidence,
        missing,
        note,
        recorded_by: recorder.recorded_by,
        profile_id: recorder.profile_id,
        build: input.build,
        created_at: now,
    };
    let event = attempt_event(&next, &attempt);
    Ok(WorkflowChange { workflow: next, attempt: Some(attempt), events: vec![event] })
}

/// 使用者跳過目前的 Stage：必須附原因；裁定與安全 Gate 不能跳過。
pub fn skip_stage(
    workflow: &WorkflowInstance,
    playbook: &PlaybookVersion,
    prior: &[StageAttempt],
    stage: &str,
    expected_revision: i64,
    reason: &str,
    attempt_id: String,
    now: Timestamp,
) -> Result<WorkflowChange, WorkflowError> {
    let definition = current_stage(workflow, playbook, stage, expected_revision)?;
    if !definition.gate.skippable() {
        return Err(WorkflowError::NotSkippable { stage: definition.key.clone(), gate: definition.gate.label() });
    }
    let reason = bounded(reason, NOTE_MAX_CHARS, "reason ")?;
    if reason.is_empty() {
        return Err(invalid("跳過 Stage 必須說明原因"));
    }
    let mut next = workflow.clone();
    advance(&mut next, playbook, prior, &definition.key);
    next.revision += 1;
    next.updated_at = now;
    let attempt = StageAttempt {
        id: attempt_id,
        workflow_id: workflow.id.clone(),
        stage_key: definition.key.clone(),
        attempt: next_attempt_no(prior, &definition.key),
        outcome: AttemptOutcome::Skipped,
        evidence: Vec::new(),
        missing: Vec::new(),
        note: reason,
        recorded_by: RECORDED_BY_DESKTOP.to_string(),
        profile_id: None,
        build: None,
        created_at: now,
    };
    let event = attempt_event(&next, &attempt);
    Ok(WorkflowChange { workflow: next, attempt: Some(attempt), events: vec![event] })
}

// ---------------------------------------------------------------- 檢視

/// 一個 Stage 在 Workflow 中的狀態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StageState {
    Pending,
    Current,
    Blocked,
    Passed,
    Skipped,
    /// 先前通過，但依賴已改變而失效，等待重驗（票 50）。
    Invalidated,
}

/// 依目前 Stage 與 attempt 推出每個 Stage 的狀態：以最後一次通過、跳過或失效的紀錄為準。目前 Stage 之後的是待進行，
/// 除非 Gate 失效後正在重驗（`resume_stage`）：失效前的目前 Stage 之前的通過與失效照樣顯示（仍有效的重驗後沿用）。
pub fn stage_states(workflow: &WorkflowInstance, playbook: &PlaybookVersion, attempts: &[StageAttempt]) -> Vec<StageState> {
    let stages = &playbook.definition.stages;
    let current = workflow.current_stage.as_deref().and_then(|c| playbook.definition.stage_index(c)).unwrap_or(stages.len());
    let resume = workflow.resume_stage.as_deref().and_then(|r| playbook.definition.stage_index(r)).unwrap_or(current);
    stages
        .iter()
        .enumerate()
        .map(|(index, stage)| {
            if index == current {
                return if workflow.status == WorkflowStatus::Blocked { StageState::Blocked } else { StageState::Current };
            }
            if index >= resume.max(current) {
                return StageState::Pending;
            }
            let last = attempts
                .iter()
                .filter(|a| a.stage_key == stage.key)
                .filter(|a| matches!(a.outcome, AttemptOutcome::Passed | AttemptOutcome::Skipped | AttemptOutcome::Invalidated))
                .max_by_key(|a| a.attempt);
            match last.map(|a| a.outcome) {
                Some(AttemptOutcome::Skipped) => StageState::Skipped,
                Some(AttemptOutcome::Invalidated) => StageState::Invalidated,
                Some(_) => StageState::Passed,
                None if index < current => StageState::Passed,
                None => StageState::Pending,
            }
        })
        .collect()
}

fn bullet_list(items: &[String]) -> String {
    if items.is_empty() {
        "（無）".to_string()
    } else {
        items.join("；")
    }
}

/// 給 Controller 的背景：進行中的 Workflow 與目前 Stage（換帳號、換 Session 後從這裡接續）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowNote {
    pub workflow_id: String,
    pub revision: i64,
    pub playbook_title: String,
    pub playbook_key: String,
    pub playbook_version: i64,
    pub task_title: Option<String>,
    pub status: WorkflowStatus,
    pub current_stage: String,
    pub current_stage_title: String,
    pub blocked_reason: Option<String>,
    /// 已通過或跳過的 Stage 名稱（依 Playbook 順序）。
    pub done: Vec<String>,
    /// 已失效、等待重驗的 Stage 名稱（票 50）。
    pub reverify: Vec<String>,
}

impl WorkflowNote {
    pub fn from_parts(
        workflow: &WorkflowInstance,
        playbook: &PlaybookVersion,
        attempts: &[StageAttempt],
        task_title: Option<String>,
    ) -> Option<Self> {
        let current = workflow.current_stage.clone()?;
        let states = stage_states(workflow, playbook, attempts);
        let titles = |wanted: &[StageState]| {
            playbook
                .definition
                .stages
                .iter()
                .zip(&states)
                .filter(|(_, s)| wanted.contains(s))
                .map(|(stage, _)| stage.title.clone())
                .collect::<Vec<_>>()
        };
        let done = titles(&[StageState::Passed, StageState::Skipped]);
        // 失效後還沒重新通過的 Gate（含退回後的目前 Stage）
        let reverify = playbook
            .definition
            .stages
            .iter()
            .filter(|s| reverification_pending(attempts, &s.key))
            .map(|s| s.title.clone())
            .collect();
        Some(WorkflowNote {
            workflow_id: workflow.id.clone(),
            revision: workflow.revision,
            playbook_title: playbook.definition.title.clone(),
            playbook_key: playbook.key.clone(),
            playbook_version: playbook.version,
            task_title,
            status: workflow.status,
            current_stage_title: playbook.definition.stage(&current).map(|s| s.title.clone()).unwrap_or_default(),
            current_stage: current,
            blocked_reason: workflow.blocked_reason.clone(),
            done,
            reverify,
        })
    }

    /// Controller Prompt 中的一行。
    pub fn render(&self) -> String {
        let target = self.task_title.as_deref().map(|t| format!("Task「{t}」")).unwrap_or_else(|| "Goal".to_string());
        let done = if self.done.is_empty() { "（無）".to_string() } else { self.done.join("、") };
        let blocked = self.blocked_reason.as_deref().map(|r| format!("；已阻擋：{r}")).unwrap_or_default();
        let reverify = if self.reverify.is_empty() {
            String::new()
        } else {
            format!("；Gate 已失效、需要重驗：{}", self.reverify.join("、"))
        };
        format!(
            "- Workflow {}（{target}，{} {} 第 {} 版，revision {}）：目前 Stage「{}」（{}）{blocked}{reverify}；已完成：{done}",
            self.workflow_id,
            self.playbook_title,
            self.playbook_key,
            self.playbook_version,
            self.revision,
            self.current_stage_title,
            self.current_stage,
        )
    }
}

/// 給 Worker 的 Stage 說明（Context Builder 的必要項目）：Run 換帳號交接後，successor 收到的是同一個 Stage。
pub fn worker_stage_note(workflow: &WorkflowInstance, playbook: &PlaybookVersion) -> Option<String> {
    let stage = playbook.definition.stage(workflow.current_stage.as_deref()?)?;
    let checks: Vec<String> = stage
        .exit_checks
        .iter()
        .map(|c| match c.requires {
            EvidenceKind::Review => format!("{}（由非作者的 reviewer 以 submit_review 提交；作者不能審查自己的工作）", c.description),
            _ => c.description.clone(),
        })
        .collect();
    Some(format!(
        "\n\n目前的流程階段（Workflow {}，{} 第 {} 版）：Stage「{}」（{}）。\
         這個 Task 已完成的階段不要重做，只做這個階段的工作。\n\
         進入條件：{}\n預期產出：{}\n允許的動作：{}\n離開前需要的證據（以 register_artifact 登記成果）：{}",
        workflow.id,
        playbook.definition.title,
        playbook.version,
        stage.title,
        stage.key,
        bullet_list(&stage.entry_conditions),
        bullet_list(&stage.expected_outputs),
        bullet_list(&stage.allowed_actions),
        bullet_list(&checks),
    ))
}

#[cfg(test)]
mod tests;
