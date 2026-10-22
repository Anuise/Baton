//! 專案理解：Project Brief、Glossary 與 Project Map（票 42），以及 repo 內 GLOSSARY／ADR 的輸出投影。
//!
//! - Controller 以 `record_understanding` 逐項建立與維護；每一項以（文件別, 名稱）識別，附來源位置、版本與查驗時間。
//! - **未知資訊不被寫成已確認事實**：「已確認」至少要有一個經 Runtime 查驗的來源；檔案來源由 Runtime 讀取 Workspace 內的檔案、
//!   自行計算版本（內容 SHA-256）並記下查驗時間，不採信 Agent 自報；找不到的來源一律拒絕。沒有來源的描述只能是假說或未知。
//! - 投影：GLOSSARY.md 與 docs/adr/ 是由專案理解與 Workspace 的決策產生、帶版本的輸出。Runtime 記住最後寫入（或採用）的內容雜湊；
//!   檔案被外部修改、刪除，或接入前就已存在時，不覆寫，而是開啟協調（conflict）並產生事件，由使用者選擇覆寫或保留外部版本。

use serde::{Deserialize, Serialize};

use crate::artifact::sha256_hex;
use crate::event::DomainEvent;
use crate::knowledge::{KnowledgeEntry, KnowledgeStatus};
use crate::Timestamp;

/// Controller 建立或更新一項專案理解的工具名稱（Controller 專屬）。
pub const TOOL_RECORD_UNDERSTANDING: &str = "record_understanding";
/// Controller 讀取目前專案理解的工具名稱（Controller 專屬）。
pub const TOOL_GET_PROJECT_UNDERSTANDING: &str = "get_project_understanding";

pub const ITEM_AGGREGATE: &str = "understanding_item";
pub const PROJECTION_AGGREGATE: &str = "understanding_projection";
pub const EVENT_ITEM_RECORDED: &str = "understanding_item_recorded";
pub const EVENT_PROJECTION_WRITTEN: &str = "understanding_projection_written";
pub const EVENT_PROJECTION_CONFLICT_DETECTED: &str = "understanding_projection_conflict_detected";
pub const EVENT_PROJECTION_RECONCILED: &str = "understanding_projection_reconciled";

pub const KEY_MAX_CHARS: usize = 200;
pub const TEXT_MAX_CHARS: usize = 4_000;
pub const SOURCES_MAX: usize = 20;
pub const REFERENCE_MAX_CHARS: usize = 1_000;
pub const VERSION_MAX_CHARS: usize = 200;

/// Glossary 投影的位置（Workspace 根目錄內）。
pub const GLOSSARY_PATH: &str = "GLOSSARY.md";

/// 決策投影的位置：每個 Workspace 範圍的決策一個 ADR 檔。
pub fn adr_path(entry_id: &str) -> String {
    format!("docs/adr/baton-{entry_id}.md")
}

/// 專案理解的文件別。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Section {
    /// Project Brief：產品目標、使用者、成功條件、排除範圍、系統現況、外部整合、風險與限制。
    Brief,
    /// Glossary：名詞與定義。
    Glossary,
    /// Project Map：模組責任、介面、使用路徑、資料流、測試入口與它們之間的關係。
    Map,
}

impl Section {
    pub const ALL: [Section; 3] = [Section::Brief, Section::Glossary, Section::Map];

    pub fn as_str(self) -> &'static str {
        match self {
            Section::Brief => "brief",
            Section::Glossary => "glossary",
            Section::Map => "map",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.as_str() == raw)
    }
}

/// 一項描述的可信狀態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemStatus {
    /// 已確認：至少一個經 Runtime 查驗的來源支持。
    Confirmed,
    /// 假說：推測或尚未查證的描述與關係。
    Hypothesis,
    /// 未知：記下的缺口（不知道、待查）。
    Unknown,
    /// 已移除：不再適用，保留歷史，不出現在投影中。
    Retired,
}

impl ItemStatus {
    pub const ALL: [ItemStatus; 4] = [ItemStatus::Confirmed, ItemStatus::Hypothesis, ItemStatus::Unknown, ItemStatus::Retired];

    pub fn as_str(self) -> &'static str {
        match self {
            ItemStatus::Confirmed => "confirmed",
            ItemStatus::Hypothesis => "hypothesis",
            ItemStatus::Unknown => "unknown",
            ItemStatus::Retired => "retired",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.as_str() == raw)
    }
}

/// Agent 提供的來源（尚未查驗）：Workspace 內的檔案（`path`，可附行號），或外部來源（`reference` 與它的 `version`）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// 整理過、待查驗的來源。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceRequest {
    /// Workspace 內的相對路徑（以 `/` 分隔）。
    File { path: String, line: Option<u32> },
    External { reference: String, version: String },
}

/// 已查驗的來源：位置、版本與查驗時間。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Source {
    /// Workspace 內的檔案；`version` 是 Runtime 查驗當時檔案內容的 SHA-256。
    File { path: String, line: Option<u32>, version: String, checked_at: Timestamp },
    /// 外部來源（網址、文件、對話）；`version` 由提供者標明（例如修訂、日期），Runtime 無法讀取內容。
    External { reference: String, version: String, checked_at: Timestamp },
}

/// Runtime 查驗一個來源的結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceCheck {
    Found(Source),
    /// Workspace 內找不到這個檔案。
    Missing { path: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnderstandingItem {
    pub id: String,
    pub workspace_id: String,
    pub section: Section,
    /// 文件內的名稱：Brief 的段落、Glossary 的名詞、Map 的模組或關係（例如「Runtime → Store」）。
    pub key: String,
    pub text: String,
    pub status: ItemStatus,
    pub sources: Vec<Source>,
    /// 每次變更遞增；每一版的完整內容保存在事件中。
    pub revision: i64,
    /// 最後記錄者（Controller 那一輪的識別）。
    pub recorded_by: String,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UnderstandingError {
    #[error("內容不正確：{0}")]
    Invalid(String),
    #[error("「已確認」至少需要一個可查驗的來源（Workspace 內的檔案，或附版本的外部來源）；沒有來源的描述請標為 hypothesis 或 unknown")]
    UnverifiedClaim,
    #[error("來源 {0} 在 Workspace 中不存在，無法查驗；請修正路徑，或不附這個來源並把描述標為 hypothesis")]
    SourceNotFound(String),
    #[error("找不到要移除的項目「{0}」")]
    NotFound(String),
    #[error("找不到投影檔 {0}")]
    ProjectionNotFound(String),
    #[error("投影檔 {path} 目前是「{status}」，不能{action}")]
    ProjectionTransitionInvalid { path: String, status: &'static str, action: &'static str },
    #[error("投影檔 {0} 在你檢視之後又被修改過，請重新整理後再決定")]
    ProjectionChanged(String),
}

impl UnderstandingError {
    /// 穩定的機器可讀代碼。
    pub fn code(&self) -> &'static str {
        match self {
            UnderstandingError::Invalid(_) => "understanding_invalid",
            UnderstandingError::UnverifiedClaim => "understanding_unverified_claim",
            UnderstandingError::SourceNotFound(_) => "understanding_source_not_found",
            UnderstandingError::NotFound(_) => "understanding_not_found",
            UnderstandingError::ProjectionNotFound(_) => "projection_not_found",
            UnderstandingError::ProjectionTransitionInvalid { .. } => "projection_transition_invalid",
            UnderstandingError::ProjectionChanged(_) => "projection_changed",
        }
    }
}

fn invalid(message: impl Into<String>) -> UnderstandingError {
    UnderstandingError::Invalid(message.into())
}

fn bounded(raw: &str, max: usize, label: &str) -> Result<String, UnderstandingError> {
    let text = raw.trim();
    if text.chars().count() > max {
        return Err(invalid(format!("{label}最多 {max} 個字元")));
    }
    Ok(text.to_string())
}

/// 整理 Agent 提供的來源：檔案與外部來源二擇一；外部來源必須標明版本。
pub fn normalize_sources(inputs: &[SourceInput]) -> Result<Vec<SourceRequest>, UnderstandingError> {
    if inputs.len() > SOURCES_MAX {
        return Err(invalid(format!("來源最多 {SOURCES_MAX} 個")));
    }
    let mut sources = Vec::new();
    for input in inputs {
        let reference = input.reference.as_deref().map(str::trim).filter(|r| !r.is_empty());
        let source = match (&input.path, reference) {
            (Some(path), None) => {
                let path = crate::verification::relative_path(path)
                    .ok_or_else(|| invalid(format!("來源路徑「{path}」必須是 Workspace 內的相對路徑")))?;
                SourceRequest::File { path, line: input.line }
            }
            (None, Some(reference)) => {
                let reference = bounded(reference, REFERENCE_MAX_CHARS, "外部來源")?;
                let version = bounded(input.version.as_deref().unwrap_or_default(), VERSION_MAX_CHARS, "來源版本")?;
                if version.is_empty() {
                    return Err(invalid(format!("外部來源「{reference}」必須標明版本（例如修訂、發布日期）")));
                }
                SourceRequest::External { reference, version }
            }
            _ => return Err(invalid("每個來源必須剛好提供 path（Workspace 內的檔案）或 reference（外部來源）其中一個")),
        };
        if !sources.contains(&source) {
            sources.push(source);
        }
    }
    Ok(sources)
}

/// 一次記錄的輸入（來源已由 Runtime 查驗）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordInput {
    pub section: Section,
    pub key: String,
    pub text: String,
    pub status: ItemStatus,
    pub sources: Vec<SourceCheck>,
}

/// 一次變更：寫回的項目與事件。
#[derive(Debug, Clone, PartialEq)]
pub struct ItemChange {
    pub item: UnderstandingItem,
    pub event: DomainEvent,
}

/// 建立或更新一項。內容與來源都沒變時回 `None`（不產生新版本）。
///
/// 規則：找不到的來源一律拒絕；「已確認」至少要有一個已查驗的來源；
/// 「已移除」只能用於既有項目，沿用它原本的描述與來源。
pub fn record(
    existing: Option<&UnderstandingItem>,
    new_id: String,
    workspace_id: &str,
    input: RecordInput,
    recorded_by: &str,
    now: Timestamp,
) -> Result<Option<ItemChange>, UnderstandingError> {
    let key = bounded(&input.key, KEY_MAX_CHARS, "名稱")?;
    if key.is_empty() {
        return Err(invalid("名稱不能是空的"));
    }
    let mut sources = Vec::new();
    for check in input.sources {
        match check {
            SourceCheck::Found(source) => sources.push(source),
            SourceCheck::Missing { path } => return Err(UnderstandingError::SourceNotFound(path)),
        }
    }
    let (text, sources) = if input.status == ItemStatus::Retired {
        let Some(existing) = existing else { return Err(UnderstandingError::NotFound(key)) };
        (existing.text.clone(), existing.sources.clone())
    } else {
        let text = bounded(&input.text, TEXT_MAX_CHARS, "描述")?;
        if text.is_empty() {
            return Err(invalid("描述不能是空的；未知的事項請寫下哪裡未知並標為 unknown"));
        }
        if input.status == ItemStatus::Confirmed && sources.is_empty() {
            return Err(UnderstandingError::UnverifiedClaim);
        }
        (text, sources)
    };
    if let Some(existing) = existing {
        if existing.text == text && existing.status == input.status && existing.sources == sources {
            return Ok(None);
        }
    }
    let item = match existing {
        Some(existing) => UnderstandingItem {
            text,
            status: input.status,
            sources,
            revision: existing.revision + 1,
            recorded_by: recorded_by.to_string(),
            updated_at: now,
            ..existing.clone()
        },
        None => UnderstandingItem {
            id: new_id,
            workspace_id: workspace_id.to_string(),
            section: input.section,
            key,
            text,
            status: input.status,
            sources,
            revision: 1,
            recorded_by: recorded_by.to_string(),
            created_at: now,
            updated_at: now,
        },
    };
    let event = DomainEvent {
        aggregate_type: ITEM_AGGREGATE,
        aggregate_id: item.id.clone(),
        aggregate_version: item.revision,
        event_type: EVENT_ITEM_RECORDED,
        occurred_at: now,
        payload: serde_json::to_value(&item).expect("項目一定可以序列化"),
    };
    Ok(Some(ItemChange { item, event }))
}

// ---------------------------------------------------------------- 投影

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectionKind {
    Glossary,
    Adr,
}

impl ProjectionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ProjectionKind::Glossary => "glossary",
            ProjectionKind::Adr => "adr",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        [ProjectionKind::Glossary, ProjectionKind::Adr].into_iter().find(|k| k.as_str() == raw)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectionStatus {
    /// 檔案就是 Baton 最後寫入（或採用）的內容。
    InSync,
    /// 偵測到外部編輯（修改、刪除，或接入前就已存在），等待使用者協調；期間不寫入。
    Conflict,
    /// 使用者選擇保留外部版本：Baton 停止寫入這個檔，直到使用者改為覆寫。
    Detached,
}

impl ProjectionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            ProjectionStatus::InSync => "in_sync",
            ProjectionStatus::Conflict => "conflict",
            ProjectionStatus::Detached => "detached",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        [ProjectionStatus::InSync, ProjectionStatus::Conflict, ProjectionStatus::Detached]
            .into_iter()
            .find(|s| s.as_str() == raw)
    }
}

/// 一個投影檔的狀態。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Projection {
    pub workspace_id: String,
    /// Workspace 內的相對路徑。
    pub path: String,
    pub kind: ProjectionKind,
    /// 投影內容的版本：Baton 產生的內容改變時遞增，寫在檔頭。
    pub version: i64,
    /// 這個版本的內文（不含檔頭）雜湊，用來判斷內容是否改變。
    pub body_hash: String,
    /// Baton 最後寫入（或採用）的整個檔案雜湊；`None` 表示還沒有基準（例如接入前就存在的檔案）。
    pub baseline_hash: Option<String>,
    pub status: ProjectionStatus,
    /// 協調中時檔案目前的雜湊；`None` 表示檔案被刪除。
    pub external_hash: Option<String>,
    pub detected_at: Option<Timestamp>,
    pub revision: i64,
    pub updated_at: Timestamp,
}

/// 要產生的一個投影檔：位置、種類與內文（不含檔頭）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectionTarget {
    pub path: String,
    pub kind: ProjectionKind,
    pub body: String,
}

/// 整個檔案內容：檔頭標明由 Baton 產生、版本，以及直接修改的後果。
pub fn projection_content(kind: ProjectionKind, version: i64, body: &str) -> String {
    let label = match kind {
        ProjectionKind::Glossary => "Glossary",
        ProjectionKind::Adr => "ADR",
    };
    format!(
        "<!-- Baton 投影：{label} 第 {version} 版。本檔由 Baton 依專案理解產生；直接修改會被偵測為外部編輯並送交協調，不會被靜默覆寫。 -->\n\n{body}"
    )
}

/// 一次比對的結果：要寫入的內容（`None` 表示不寫）、要寫回的投影狀態與事件，以及是否新開了協調。
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectionPlan {
    pub write: Option<String>,
    pub projection: Option<Projection>,
    pub events: Vec<DomainEvent>,
    pub conflict_opened: bool,
}

impl ProjectionPlan {
    fn unchanged() -> Self {
        Self { write: None, projection: None, events: Vec::new(), conflict_opened: false }
    }
}

fn projection_event(projection: &Projection, event_type: &'static str, now: Timestamp, payload: serde_json::Value) -> DomainEvent {
    DomainEvent {
        aggregate_type: PROJECTION_AGGREGATE,
        aggregate_id: format!("{}:{}", projection.workspace_id, projection.path),
        aggregate_version: projection.revision,
        event_type,
        occurred_at: now,
        payload,
    }
}

/// 下一個內容版本：內文改變時遞增。
fn next_version(record: Option<&Projection>, body_hash: &str) -> i64 {
    match record {
        None => 1,
        Some(record) if record.body_hash == body_hash => record.version,
        Some(record) => record.version + 1,
    }
}

/// 比對目前的檔案（`current`，`None` 表示不存在）與 Baton 要產生的內容，決定寫入、略過或開啟協調。
///
/// - 從未投影過：檔案不存在就寫入；內容剛好相同就採用為基準；其他內容（接入前就存在）開啟協調。
/// - 同步中：檔案仍是基準（沒被動過）才寫入新內容；檔案已是要產生的內容就只更新基準；其他情況（修改、刪除）開啟協調。
/// - 協調中：不寫入；檔案又被改成別的內容時更新協調資訊並再產生一次事件；被改成與要產生的內容相同時視為已同步。
/// - 保留外部版本：什麼都不做。
pub fn plan_projection(
    workspace_id: &str,
    record: Option<&Projection>,
    target: &ProjectionTarget,
    current: Option<&[u8]>,
    now: Timestamp,
) -> ProjectionPlan {
    if record.is_some_and(|r| r.status == ProjectionStatus::Detached) {
        return ProjectionPlan::unchanged();
    }
    let body_hash = sha256_hex(target.body.as_bytes());
    let version = next_version(record, &body_hash);
    let content = projection_content(target.kind, version, &target.body);
    let desired_hash = sha256_hex(content.as_bytes());
    let current_hash = current.map(sha256_hex);
    let base = record.cloned().unwrap_or_else(|| Projection {
        workspace_id: workspace_id.to_string(),
        path: target.path.clone(),
        kind: target.kind,
        version: 0,
        body_hash: String::new(),
        baseline_hash: None,
        status: ProjectionStatus::InSync,
        external_hash: None,
        detected_at: None,
        revision: 0,
        updated_at: now,
    });
    let synced = Projection {
        version,
        body_hash: body_hash.clone(),
        baseline_hash: Some(desired_hash.clone()),
        status: ProjectionStatus::InSync,
        external_hash: None,
        detected_at: None,
        revision: base.revision + 1,
        updated_at: now,
        ..base.clone()
    };
    let written = |projection: Projection, write: Option<String>, adopted: bool| {
        let payload = serde_json::json!({ "path": projection.path, "version": projection.version, "adopted": adopted });
        let events = vec![projection_event(&projection, EVENT_PROJECTION_WRITTEN, now, payload)];
        ProjectionPlan { write, projection: Some(projection), events, conflict_opened: false }
    };
    let conflict = |opened: bool| {
        let projection = Projection {
            status: ProjectionStatus::Conflict,
            external_hash: current_hash.clone(),
            detected_at: Some(now),
            revision: base.revision + 1,
            updated_at: now,
            ..base.clone()
        };
        let payload = serde_json::json!({
            "path": projection.path,
            "external_hash": projection.external_hash,
            "deleted": current_hash.is_none(),
            "pending_version": version,
        });
        let events = vec![projection_event(&projection, EVENT_PROJECTION_CONFLICT_DETECTED, now, payload)];
        ProjectionPlan { write: None, projection: Some(projection), events, conflict_opened: opened }
    };

    if current_hash.as_deref() == Some(desired_hash.as_str()) {
        // 檔案已經是要產生的內容：只在狀態不同時更新（採用為基準）
        let same = record.is_some_and(|r| {
            r.status == ProjectionStatus::InSync && r.baseline_hash.as_deref() == Some(desired_hash.as_str()) && r.version == version
        });
        return if same { ProjectionPlan::unchanged() } else { written(synced, None, true) };
    }
    match record {
        None if current.is_none() => written(synced, Some(content), false),
        None => conflict(true),
        Some(r) if r.status == ProjectionStatus::InSync => {
            if r.baseline_hash.is_some() && current_hash == r.baseline_hash {
                written(synced, Some(content), false)
            } else {
                conflict(true)
            }
        }
        // 協調中
        Some(r) if r.external_hash != current_hash => conflict(false),
        Some(_) => ProjectionPlan::unchanged(),
    }
}

/// 使用者對投影檔的協調。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReconcileAction {
    /// 以 Baton 產生的內容覆寫（協調中或已保留外部版本時都可以）。
    Overwrite,
    /// 保留外部版本：Baton 停止寫入這個檔（只限協調中）。
    KeepExternal,
}

/// 套用使用者的協調。`expected_external_hash` 是使用者檢視時檔案的雜湊（`None` 表示當時檔案不存在），
/// 與目前的檔案不同時拒絕，避免覆寫使用者沒看過的內容。
pub fn reconcile(
    record: &Projection,
    action: ReconcileAction,
    expected_external_hash: Option<&str>,
    target: &ProjectionTarget,
    current: Option<&[u8]>,
    now: Timestamp,
) -> Result<ProjectionPlan, UnderstandingError> {
    let allowed = match action {
        ReconcileAction::Overwrite => matches!(record.status, ProjectionStatus::Conflict | ProjectionStatus::Detached),
        ReconcileAction::KeepExternal => record.status == ProjectionStatus::Conflict,
    };
    if !allowed {
        return Err(UnderstandingError::ProjectionTransitionInvalid {
            path: record.path.clone(),
            status: record.status.as_str(),
            action: match action {
                ReconcileAction::Overwrite => "覆寫",
                ReconcileAction::KeepExternal => "保留外部版本",
            },
        });
    }
    let current_hash = current.map(sha256_hex);
    if current_hash.as_deref() != expected_external_hash {
        return Err(UnderstandingError::ProjectionChanged(record.path.clone()));
    }
    let (projection, write) = match action {
        ReconcileAction::Overwrite => {
            let body_hash = sha256_hex(target.body.as_bytes());
            let version = next_version(Some(record), &body_hash);
            let content = projection_content(target.kind, version, &target.body);
            let projection = Projection {
                version,
                body_hash,
                baseline_hash: Some(sha256_hex(content.as_bytes())),
                status: ProjectionStatus::InSync,
                external_hash: None,
                detected_at: None,
                revision: record.revision + 1,
                updated_at: now,
                ..record.clone()
            };
            (projection, Some(content))
        }
        ReconcileAction::KeepExternal => {
            let projection = Projection {
                baseline_hash: current_hash.clone(),
                status: ProjectionStatus::Detached,
                external_hash: None,
                detected_at: None,
                revision: record.revision + 1,
                updated_at: now,
                ..record.clone()
            };
            (projection, None)
        }
    };
    let payload = serde_json::json!({ "path": projection.path, "action": action, "version": projection.version });
    let events = vec![projection_event(&projection, EVENT_PROJECTION_RECONCILED, now, payload)];
    Ok(ProjectionPlan { write, projection: Some(projection), events, conflict_opened: false })
}

// ---------------------------------------------------------------- 產生內容

/// UTC 時間的顯示形式（`2023-11-14 22:13 UTC`）；投影檔內一律用 UTC（spec D-33）。
pub fn format_utc(at: Timestamp) -> String {
    let seconds = at.as_millis().div_euclid(1_000);
    let (days, secs) = (seconds.div_euclid(86_400), seconds.rem_euclid(86_400));
    // Howard Hinnant 的 civil_from_days
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02} UTC", secs / 3_600, secs % 3_600 / 60)
}

fn short(version: &str) -> &str {
    version.get(..12).unwrap_or(version)
}

/// 來源的一行說明：位置、版本與查驗時間。
pub fn describe_source(source: &Source) -> String {
    match source {
        Source::File { path, line, version, checked_at } => {
            let at = line.map(|l| format!("{path}:{l}")).unwrap_or_else(|| path.clone());
            format!("`{at}`（版本 {}，查驗於 {}）", short(version), format_utc(*checked_at))
        }
        Source::External { reference, version, checked_at } => {
            format!("{reference}（版本 {version}，查驗於 {}）", format_utc(*checked_at))
        }
    }
}

/// GLOSSARY.md 的內文：已確認與假說的名詞依名稱排序，假說明確標示；未知的名詞另列「待確認」，不寫成定義。
pub fn render_glossary(items: &[UnderstandingItem]) -> String {
    let mut terms: Vec<&UnderstandingItem> =
        items.iter().filter(|i| i.section == Section::Glossary && i.status != ItemStatus::Retired).collect();
    terms.sort_by(|a, b| a.key.cmp(&b.key));
    let mut out = String::from("# Glossary\n");
    for item in terms.iter().filter(|i| i.status != ItemStatus::Unknown) {
        out.push_str(&format!("\n## {}\n\n{}\n\n", item.key, item.text));
        match item.status {
            ItemStatus::Confirmed => out.push_str("- 狀態：已確認\n"),
            _ => out.push_str("- 狀態：假說（未確認）\n"),
        }
        for source in &item.sources {
            out.push_str(&format!("- 來源：{}\n", describe_source(source)));
        }
    }
    let unknown: Vec<&&UnderstandingItem> = terms.iter().filter(|i| i.status == ItemStatus::Unknown).collect();
    if !unknown.is_empty() {
        out.push_str("\n## 待確認（未知）\n\n");
        for item in unknown {
            out.push_str(&format!("- **{}**：{}\n", item.key, item.text));
        }
    }
    out
}

/// 一個決策的 ADR 內文：標題、狀態、生效時間、內容與證據。只有已驗證的決策標為「已採用」。
pub fn render_adr(entry: &KnowledgeEntry) -> String {
    let status = match entry.status {
        KnowledgeStatus::Verified => "已採用".to_string(),
        KnowledgeStatus::Candidate => "候選（未確認）".to_string(),
        KnowledgeStatus::Revoked => "已撤銷".to_string(),
        KnowledgeStatus::Retired => "已淘汰".to_string(),
        KnowledgeStatus::Superseded => match &entry.superseded_by {
            Some(by) => format!("已被取代（{by}）"),
            None => "已被取代".to_string(),
        },
    };
    let mut out = format!(
        "# {}\n\n- 狀態：{status}\n- 來源：Baton Knowledge Entry {}（第 {} 版）\n- 生效：{}\n",
        entry.content.title,
        entry.id,
        entry.version,
        format_utc(entry.valid_from)
    );
    if let Some(reason) = &entry.status_reason {
        out.push_str(&format!("- 狀態原因：{reason}\n"));
    }
    if !entry.content.body.is_empty() {
        out.push_str(&format!("\n{}\n", entry.content.body));
    }
    if !entry.content.evidence.is_empty() {
        out.push_str("\n## 證據\n\n");
        for evidence in &entry.content.evidence {
            out.push_str(&format!("- {evidence}\n"));
        }
    }
    out
}

#[cfg(test)]
mod tests;
