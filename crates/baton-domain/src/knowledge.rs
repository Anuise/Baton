//! 知識與教訓（Knowledge Entry／Lesson）的純邏輯（票 37，spec D-26、D-28）。
//!
//! - 決策、教訓與一般知識都是 Knowledge Entry，以 `kind` 區分；依全域、Workspace、Goal、Task 範圍保存，
//!   附來源、證據、可信狀態、有效期間、版本與取代關係。
//! - **狀態由來源決定，呼叫者不能指定**：使用者在「知識」分頁建立的直接是已驗證；Agent（Controller）提交的一律是候選教訓，
//!   必須由使用者確認才升為已驗證（D-28）。
//! - 修正只追加新版本（舊版本內容保留）；撤銷、淘汰、被取代都只改變狀態並留下事件，不刪除任何東西。
//! - 只有「已驗證且在有效期間內」的條目是有效的；候選、撤銷、淘汰、被取代與過期的條目都不會被當作有效知識載入或搜尋到。

use serde::{Deserialize, Serialize};

use crate::event::DomainEvent;
use crate::Timestamp;

/// Agent 提交候選教訓的工具名稱（spec D-05 的 Controller 專屬工具）。
pub const TOOL_RECORD_LESSON_CANDIDATE: &str = "record_lesson_candidate";
/// Agent 搜尋有效知識的工具名稱（spec D-05 的共同工具）。
pub const TOOL_SEARCH_KNOWLEDGE: &str = "search_knowledge";

pub const AGGREGATE: &str = "knowledge_entry";
pub const EVENT_CREATED: &str = "knowledge_created";
pub const EVENT_CONFIRMED: &str = "knowledge_confirmed";
pub const EVENT_CORRECTED: &str = "knowledge_corrected";
pub const EVENT_REVOKED: &str = "knowledge_revoked";
pub const EVENT_RETIRED: &str = "knowledge_retired";
pub const EVENT_SUPERSEDED: &str = "knowledge_superseded";

/// 標題、內文、教訓欄位、標籤、證據、原因的上限。
pub const TITLE_MAX_CHARS: usize = 200;
pub const BODY_MAX_CHARS: usize = 10_000;
pub const LESSON_FIELD_MAX_CHARS: usize = 2_000;
pub const TAG_MAX_CHARS: usize = 50;
pub const TAGS_MAX: usize = 20;
pub const EVIDENCE_MAX_CHARS: usize = 1_000;
pub const EVIDENCE_MAX: usize = 20;
pub const REASON_MAX_CHARS: usize = 1_000;
/// 搜尋關鍵字：每個詞至少 2 個字元（繁體中文兩字），整個查詢最多 200 字元、8 個詞。
pub const QUERY_TERM_MIN_CHARS: usize = 2;
pub const QUERY_MAX_CHARS: usize = 200;
pub const QUERY_MAX_TERMS: usize = 8;
/// 一次搜尋或列表最多回傳的條目數（依最後更新時間由新到舊）。
pub const RESULT_LIMIT: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeKind {
    /// 決策：已定案的判斷與理由。
    Decision,
    /// 教訓：從事件與證據得出、可防止重犯的經驗。
    Lesson,
    /// 一般知識：事實、約定或背景資訊。
    Fact,
}

impl KnowledgeKind {
    pub const ALL: [KnowledgeKind; 3] = [KnowledgeKind::Decision, KnowledgeKind::Lesson, KnowledgeKind::Fact];

    pub fn as_str(self) -> &'static str {
        match self {
            KnowledgeKind::Decision => "decision",
            KnowledgeKind::Lesson => "lesson",
            KnowledgeKind::Fact => "fact",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == raw)
    }
}

/// 可信狀態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeStatus {
    /// 候選：Agent 提交、尚未經使用者確認。
    Candidate,
    /// 已驗證：使用者建立或確認。
    Verified,
    /// 撤銷：內容是錯的。
    Revoked,
    /// 淘汰：曾經正確，但已不適用。
    Retired,
    /// 被另一個條目取代。
    Superseded,
}

impl KnowledgeStatus {
    pub const ALL: [KnowledgeStatus; 5] = [
        KnowledgeStatus::Candidate,
        KnowledgeStatus::Verified,
        KnowledgeStatus::Revoked,
        KnowledgeStatus::Retired,
        KnowledgeStatus::Superseded,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            KnowledgeStatus::Candidate => "candidate",
            KnowledgeStatus::Verified => "verified",
            KnowledgeStatus::Revoked => "revoked",
            KnowledgeStatus::Retired => "retired",
            KnowledgeStatus::Superseded => "superseded",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.as_str() == raw)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeKind {
    Global,
    Workspace,
    Goal,
    Task,
}

impl ScopeKind {
    pub const ALL: [ScopeKind; 4] = [ScopeKind::Global, ScopeKind::Workspace, ScopeKind::Goal, ScopeKind::Task];

    pub fn as_str(self) -> &'static str {
        match self {
            ScopeKind::Global => "global",
            ScopeKind::Workspace => "workspace",
            ScopeKind::Goal => "goal",
            ScopeKind::Task => "task",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.as_str() == raw)
    }
}

/// 適用範圍。全域時三個識別都是空的；Task 範圍同時帶它的 Goal（由 Runtime 依 Task 補上，不信任呼叫者填寫）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnowledgeScope {
    pub kind: ScopeKind,
    pub workspace_id: Option<String>,
    pub goal_id: Option<String>,
    pub task_id: Option<String>,
}

impl KnowledgeScope {
    pub fn global() -> Self {
        Self { kind: ScopeKind::Global, workspace_id: None, goal_id: None, task_id: None }
    }
}

/// 條目的來源：使用者，或提交它的 Agent Run（含世代與當時的 Task）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum KnowledgeSource {
    User,
    Agent { run_id: String, generation: i64, task_id: String },
}

/// 教訓特有的內容；每一欄都可以是空字串。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LessonDetail {
    /// 觸發條件：什麼情況下會再犯。
    #[serde(default)]
    pub trigger: String,
    /// 根因或待驗假說。
    #[serde(default)]
    pub root_cause: String,
    /// 有效解法或禁用做法。
    #[serde(default)]
    pub resolution: String,
}

/// 條目某一個版本的內容（已驗證、已去除前後空白）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnowledgeContent {
    pub title: String,
    pub body: String,
    pub tags: Vec<String>,
    /// 證據：Artifact、Run、事件或外部來源的參照與說明。
    pub evidence: Vec<String>,
    /// 只有教訓有；其他種類一律為空。
    pub lesson: Option<LessonDetail>,
}

/// 不可變的版本紀錄：修正只追加新版本。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnowledgeVersion {
    pub version: i64,
    pub content: KnowledgeContent,
    /// 修正說明；第一版為空。
    pub note: Option<String>,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnowledgeEntry {
    pub id: String,
    pub kind: KnowledgeKind,
    pub scope: KnowledgeScope,
    pub status: KnowledgeStatus,
    pub source: KnowledgeSource,
    pub valid_from: Timestamp,
    pub valid_until: Option<Timestamp>,
    /// 目前內容的版本號（修正時遞增）。
    pub version: i64,
    /// 聚合版本：每次變更都遞增，提交變更時以它做 expected_revision 校驗。
    pub revision: i64,
    /// 這個條目取代了哪一個條目。
    pub supersedes: Option<String>,
    /// 這個條目被哪一個條目取代。
    pub superseded_by: Option<String>,
    /// 撤銷或淘汰的原因。
    pub status_reason: Option<String>,
    /// 目前版本的內容。
    pub content: KnowledgeContent,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

impl KnowledgeEntry {
    /// 有效 = 已驗證，且 `now` 在有效期間內（`valid_until` 不含）。
    pub fn is_effective_at(&self, now: Timestamp) -> bool {
        self.status == KnowledgeStatus::Verified
            && self.valid_from <= now
            && self.valid_until.is_none_or(|until| now < until)
    }
}

/// 搜尋方式（spec D-26）：FTS5 + trigram 全文檢索；FTS5 不可用時退回逐筆 LIKE 比對（降級，結果相同但較慢）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchMode {
    FullText,
    LikeFallback,
}

/// 條目歷史中的一筆變更（取自追加式事件）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KnowledgeEvent {
    /// 這次變更之後的聚合版本。
    pub revision: i64,
    pub event_type: String,
    pub occurred_at: Timestamp,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KnowledgeError {
    #[error("內容不正確：{0}")]
    Invalid(String),
    #[error("搜尋關鍵字不正確：{0}")]
    QueryInvalid(String),
    #[error("找不到知識條目 {0}")]
    NotFound(String),
    #[error("適用範圍不正確：{0}")]
    ScopeInvalid(String),
    #[error("這個條目已經被修改過（目前是第 {current} 次變更），請重新整理後再操作")]
    RevisionConflict { current: i64 },
    #[error("條目目前是「{}」，不能{action}", .status.as_str())]
    TransitionInvalid { status: KnowledgeStatus, action: &'static str },
    #[error("不能取代這個條目：{0}")]
    SupersedeInvalid(String),
}

impl KnowledgeError {
    /// 穩定的機器可讀代碼，API 與 UI 依它判斷錯誤種類。
    pub fn code(&self) -> &'static str {
        match self {
            KnowledgeError::Invalid(_) => "knowledge_invalid",
            KnowledgeError::QueryInvalid(_) => "knowledge_query_invalid",
            KnowledgeError::NotFound(_) => "knowledge_not_found",
            KnowledgeError::ScopeInvalid(_) => "knowledge_scope_invalid",
            KnowledgeError::RevisionConflict { .. } => "knowledge_revision_conflict",
            KnowledgeError::TransitionInvalid { .. } => "knowledge_transition_invalid",
            KnowledgeError::SupersedeInvalid(_) => "knowledge_supersede_invalid",
        }
    }
}

fn invalid(message: impl Into<String>) -> KnowledgeError {
    KnowledgeError::Invalid(message.into())
}

fn bounded(raw: &str, max: usize, label: &str) -> Result<String, KnowledgeError> {
    let text = raw.trim();
    if text.chars().count() > max {
        return Err(invalid(format!("{label}最多 {max} 個字元")));
    }
    Ok(text.to_string())
}

fn list(raw: &[String], max_items: usize, max_chars: usize, label: &str) -> Result<Vec<String>, KnowledgeError> {
    let mut items: Vec<String> = Vec::new();
    for item in raw {
        let item = bounded(item, max_chars, label)?;
        if !item.is_empty() && !items.contains(&item) {
            items.push(item);
        }
    }
    if items.len() > max_items {
        return Err(invalid(format!("{label}最多 {max_items} 項")));
    }
    Ok(items)
}

impl KnowledgeContent {
    /// 驗證並正規化內容。教訓欄位只有教訓可以有；教訓沒給時視為空白的教訓欄位。
    pub fn new(
        kind: KnowledgeKind,
        title: &str,
        body: &str,
        tags: &[String],
        evidence: &[String],
        lesson: Option<LessonDetail>,
    ) -> Result<Self, KnowledgeError> {
        let title = bounded(title, TITLE_MAX_CHARS, "標題")?;
        if title.is_empty() {
            return Err(invalid("標題不能是空的"));
        }
        let body = bounded(body, BODY_MAX_CHARS, "內文")?;
        let tags = list(tags, TAGS_MAX, TAG_MAX_CHARS, "標籤")?;
        let evidence = list(evidence, EVIDENCE_MAX, EVIDENCE_MAX_CHARS, "證據")?;
        let lesson = match (kind, lesson) {
            (KnowledgeKind::Lesson, detail) => {
                let detail = detail.unwrap_or_default();
                Some(LessonDetail {
                    trigger: bounded(&detail.trigger, LESSON_FIELD_MAX_CHARS, "觸發條件")?,
                    root_cause: bounded(&detail.root_cause, LESSON_FIELD_MAX_CHARS, "根因")?,
                    resolution: bounded(&detail.resolution, LESSON_FIELD_MAX_CHARS, "解法")?,
                })
            }
            (_, Some(_)) => return Err(invalid("只有教訓可以填寫觸發條件、根因與解法")),
            (_, None) => None,
        };
        Ok(Self { title, body, tags, evidence, lesson })
    }

    /// 全文檢索與退回 LIKE 時比對的文字：標題、內文、教訓欄位、標籤與證據，以換行分隔。
    pub fn search_text(&self) -> String {
        let mut parts = vec![self.title.as_str(), self.body.as_str()];
        if let Some(lesson) = &self.lesson {
            parts.extend([lesson.trigger.as_str(), lesson.root_cause.as_str(), lesson.resolution.as_str()]);
        }
        parts.extend(self.tags.iter().map(String::as_str));
        parts.extend(self.evidence.iter().map(String::as_str));
        parts.into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join("\n")
    }
}

/// 解析搜尋關鍵字：以空白分詞，每個詞至少 2 個字元；全部的詞都要出現（AND）。空白查詢回傳空清單（不過濾文字）。
pub fn parse_query(raw: &str) -> Result<Vec<String>, KnowledgeError> {
    let raw = raw.trim();
    if raw.chars().count() > QUERY_MAX_CHARS {
        return Err(KnowledgeError::QueryInvalid(format!("最多 {QUERY_MAX_CHARS} 個字元")));
    }
    let mut terms: Vec<String> = Vec::new();
    for term in raw.split_whitespace() {
        if term.chars().count() < QUERY_TERM_MIN_CHARS {
            return Err(KnowledgeError::QueryInvalid(format!(
                "每個關鍵字至少 {QUERY_TERM_MIN_CHARS} 個字元（「{term}」太短）"
            )));
        }
        if !terms.iter().any(|t| t == term) {
            terms.push(term.to_string());
        }
    }
    if terms.len() > QUERY_MAX_TERMS {
        return Err(KnowledgeError::QueryInvalid(format!("最多 {QUERY_MAX_TERMS} 個關鍵字")));
    }
    Ok(terms)
}

/// 誰在看：決定哪些範圍的條目可見。
#[derive(Debug, Clone, Copy)]
pub enum Audience<'a> {
    /// 桌面（知識分頁）：全域條目，加上指定 Workspace 內所有範圍的條目；沒指定 Workspace 時只有全域。
    Desktop { workspace_id: Option<&'a str> },
    /// Agent Run：全域，加上它的 Workspace、它的 Goal、它的 Task 範圍的條目。
    Run { workspace_id: &'a str, goal_id: &'a str, task_id: &'a str },
}

impl Audience<'_> {
    pub fn can_see(&self, scope: &KnowledgeScope) -> bool {
        if scope.kind == ScopeKind::Global {
            return true;
        }
        let same = |field: &Option<String>, id: &str| field.as_deref() == Some(id);
        match *self {
            Audience::Desktop { workspace_id } => workspace_id.is_some_and(|ws| same(&scope.workspace_id, ws)),
            Audience::Run { workspace_id, goal_id, task_id } => {
                same(&scope.workspace_id, workspace_id)
                    && match scope.kind {
                        ScopeKind::Global | ScopeKind::Workspace => true,
                        ScopeKind::Goal => same(&scope.goal_id, goal_id),
                        ScopeKind::Task => same(&scope.task_id, task_id),
                    }
            }
        }
    }
}

/// 建立新條目的輸入（內容已驗證、範圍已由 Store 依現有資料解析）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewKnowledge {
    pub id: String,
    pub kind: KnowledgeKind,
    pub scope: KnowledgeScope,
    pub content: KnowledgeContent,
    pub source: KnowledgeSource,
    /// 沒給時從建立時間起生效。
    pub valid_from: Option<Timestamp>,
    pub valid_until: Option<Timestamp>,
}

/// 一次變更的結果：要寫回的條目（可能兩個：新條目與被它取代的舊條目）、新增的版本與事件。
#[derive(Debug, Clone, PartialEq)]
pub struct KnowledgeChange {
    pub entries: Vec<KnowledgeEntry>,
    pub new_version: Option<(String, KnowledgeVersion)>,
    pub events: Vec<DomainEvent>,
}

fn event(entry: &KnowledgeEntry, event_type: &'static str, now: Timestamp, payload: serde_json::Value) -> DomainEvent {
    DomainEvent {
        aggregate_type: AGGREGATE,
        aggregate_id: entry.id.clone(),
        aggregate_version: entry.revision,
        event_type,
        occurred_at: now,
        payload,
    }
}

fn check_validity(from: Timestamp, until: Option<Timestamp>) -> Result<(), KnowledgeError> {
    if until.is_some_and(|until| until <= from) {
        return Err(invalid("有效期間的結束時間必須晚於開始時間"));
    }
    Ok(())
}

/// 建立條目。狀態由來源決定：使用者建立的是已驗證；Agent 只能提交教訓，而且一律是候選。
/// 指定 `supersedes` 時（只限使用者），被取代的條目必須是同種類、已驗證，並在同一個變更中轉為「被取代」。
pub fn create(
    new: NewKnowledge,
    supersedes: Option<&KnowledgeEntry>,
    now: Timestamp,
) -> Result<KnowledgeChange, KnowledgeError> {
    let status = match &new.source {
        KnowledgeSource::User => KnowledgeStatus::Verified,
        KnowledgeSource::Agent { .. } if new.kind == KnowledgeKind::Lesson => KnowledgeStatus::Candidate,
        KnowledgeSource::Agent { .. } => return Err(invalid("Agent 只能提交候選教訓")),
    };
    if new.content.lesson.is_some() != (new.kind == KnowledgeKind::Lesson) {
        return Err(invalid("只有教訓可以填寫觸發條件、根因與解法"));
    }
    let valid_from = new.valid_from.unwrap_or(now);
    check_validity(valid_from, new.valid_until)?;

    let mut entries = Vec::new();
    let mut events = Vec::new();
    if let Some(old) = supersedes {
        if new.source != KnowledgeSource::User {
            return Err(KnowledgeError::SupersedeInvalid("只有使用者可以指定取代關係".into()));
        }
        if old.kind != new.kind {
            return Err(KnowledgeError::SupersedeInvalid("只能取代同種類的條目".into()));
        }
        if old.status != KnowledgeStatus::Verified {
            return Err(KnowledgeError::SupersedeInvalid(format!("被取代的條目目前是「{}」，必須是已驗證", old.status.as_str())));
        }
        let mut old = old.clone();
        old.status = KnowledgeStatus::Superseded;
        old.superseded_by = Some(new.id.clone());
        old.revision += 1;
        old.updated_at = now;
        events.push(event(&old, EVENT_SUPERSEDED, now, serde_json::json!({ "superseded_by": new.id })));
        entries.push(old);
    }

    let entry = KnowledgeEntry {
        id: new.id,
        kind: new.kind,
        scope: new.scope,
        status,
        source: new.source,
        valid_from,
        valid_until: new.valid_until,
        version: 1,
        revision: 1,
        supersedes: supersedes.map(|old| old.id.clone()),
        superseded_by: None,
        status_reason: None,
        content: new.content,
        created_at: now,
        updated_at: now,
    };
    let version = KnowledgeVersion { version: 1, content: entry.content.clone(), note: None, created_at: now };
    // 新條目的建立事件排在前面，事件序就是「建立 → 取代舊條目」
    events.insert(
        0,
        event(
            &entry,
            EVENT_CREATED,
            now,
            serde_json::json!({
                "kind": entry.kind,
                "status": entry.status,
                "scope": entry.scope,
                "source": entry.source,
                "supersedes": entry.supersedes,
            }),
        ),
    );
    let new_version = Some((entry.id.clone(), version));
    entries.insert(0, entry);
    Ok(KnowledgeChange { entries, new_version, events })
}

/// 使用者對既有條目的操作；每個都帶 expected_revision。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KnowledgeCommand {
    /// 候選 → 已驗證。
    Confirm,
    /// 修正內容與有效期限：追加新版本，狀態不變（候選仍是候選）。
    Correct { content: KnowledgeContent, valid_until: Option<Timestamp>, note: Option<String> },
    /// 撤銷（內容是錯的）：候選或已驗證 → 撤銷。
    Revoke { reason: String },
    /// 淘汰（已不適用）：已驗證 → 淘汰。
    Retire { reason: String },
}

fn reason(raw: &str) -> Result<String, KnowledgeError> {
    let text = bounded(raw, REASON_MAX_CHARS, "原因")?;
    if text.is_empty() {
        return Err(invalid("必須說明原因"));
    }
    Ok(text)
}

/// 套用使用者的操作。先校驗 expected_revision，再檢查目前狀態是否允許。
pub fn decide(
    current: &KnowledgeEntry,
    expected_revision: i64,
    command: KnowledgeCommand,
    now: Timestamp,
) -> Result<KnowledgeChange, KnowledgeError> {
    if current.revision != expected_revision {
        return Err(KnowledgeError::RevisionConflict { current: current.revision });
    }
    let mut entry = current.clone();
    let require = |allowed: &[KnowledgeStatus], action: &'static str| {
        if allowed.contains(&current.status) {
            Ok(())
        } else {
            Err(KnowledgeError::TransitionInvalid { status: current.status, action })
        }
    };
    entry.revision += 1;
    entry.updated_at = now;
    let (event_type, payload, new_version) = match command {
        KnowledgeCommand::Confirm => {
            require(&[KnowledgeStatus::Candidate], "確認為已驗證")?;
            entry.status = KnowledgeStatus::Verified;
            (EVENT_CONFIRMED, serde_json::json!({}), None)
        }
        KnowledgeCommand::Correct { content, valid_until, note } => {
            require(&[KnowledgeStatus::Candidate, KnowledgeStatus::Verified], "修正")?;
            if content.lesson.is_some() != (entry.kind == KnowledgeKind::Lesson) {
                return Err(invalid("只有教訓可以填寫觸發條件、根因與解法"));
            }
            check_validity(entry.valid_from, valid_until)?;
            if content == entry.content && valid_until == entry.valid_until {
                return Err(invalid("內容與有效期限都沒有變更"));
            }
            let note = note.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
            if note.as_ref().is_some_and(|n| n.chars().count() > REASON_MAX_CHARS) {
                return Err(invalid(format!("修正說明最多 {REASON_MAX_CHARS} 個字元")));
            }
            let content_changed = content != entry.content;
            if content_changed {
                entry.version += 1;
                entry.content = content;
            }
            entry.valid_until = valid_until;
            let version = content_changed.then(|| {
                (entry.id.clone(), KnowledgeVersion { version: entry.version, content: entry.content.clone(), note: note.clone(), created_at: now })
            });
            (
                EVENT_CORRECTED,
                serde_json::json!({ "version": entry.version, "valid_until": entry.valid_until, "note": note }),
                version,
            )
        }
        KnowledgeCommand::Revoke { reason: raw } => {
            require(&[KnowledgeStatus::Candidate, KnowledgeStatus::Verified], "撤銷")?;
            let reason = reason(&raw)?;
            entry.status = KnowledgeStatus::Revoked;
            entry.status_reason = Some(reason.clone());
            (EVENT_REVOKED, serde_json::json!({ "from": current.status, "reason": reason }), None)
        }
        KnowledgeCommand::Retire { reason: raw } => {
            require(&[KnowledgeStatus::Verified], "淘汰")?;
            let reason = reason(&raw)?;
            entry.status = KnowledgeStatus::Retired;
            entry.status_reason = Some(reason.clone());
            (EVENT_RETIRED, serde_json::json!({ "reason": reason }), None)
        }
    };
    let events = vec![event(&entry, event_type, now, payload)];
    Ok(KnowledgeChange { entries: vec![entry], new_version, events })
}

#[cfg(test)]
mod tests;
