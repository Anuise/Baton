//! 裁定請求與裁定（Decision）的狀態機：最小裁定中心（票 22）與完整裁定中心（票 35），spec D-08、D-15、使用者故事 49–57。
//!
//! 純邏輯、無 I/O。輸入「目前狀態（`DecisionContext`）+ 命令 + 時間」，輸出「新狀態 + 事件 + 喚醒 Outbox」，
//! 由 Store 在同一個交易中套用。幾條貫穿全檔的規則：
//! - **只有桌面憑證能裁定**：`Answer` 命令只能由桌面 API 發出，裁定記下 `decided_by`；Agent 憑證只能提出（`Request`、
//!   `PermissionPrompt`、`ControllerAction`）與查詢。資料庫另有 CHECK 約束，`decided_by` 只能是桌面。
//! - **只阻擋受影響的 Task**：請求的阻擋範圍是明列的 Task 識別；只有這些 Task 進入 `waiting_decision`，
//!   請求結束後也只有它們恢復（`ready`），而且要等**所有**阻擋它們的請求都結束。其他 Task 完全不受影響。
//! - **逾時與延後都不是同意**：期限（`deadline_at`）只用來標示「已逾期」，延後（`deferred_until`）只改變顯示順序；
//!   沒有任何命令會因時間流逝而回答請求、放行權限提示或讓 Task 恢復。
//! - **權限提示的單次批准（E1，票 23）**：範圍外的權限提示建立裁定請求並回「等待裁定」；請求建立時就算好它綁定的
//!   邏輯操作（`approval::ApprovalBinding`：操作識別、目標資源、重要參數、要求雜湊、成果版本）。使用者回答
//!   「允許這一次」之後，同一個 Task（任何一個 Run、任何一個帳號）再次提出時**逐項核對綁定**，全部相符才消耗這個批准
//!   （一次）；任何一項不同就不能沿用，改建新的請求並說明哪一項變了。用過之後再提出同一個操作，Runtime 不知道上一次的
//!   結果，所以新的請求會要求使用者先查證再決定。單次批准從不改變 Workspace 授權。
//! - **Controller 動作的單次核准（票 35）**：Controller 的「必須送裁定」動作建立裁定請求；使用者核准之後，
//!   同一個 Task 的同一種事項再次提出時消耗這個核准（一次），再提出就要重新裁定。
//! - **版本校驗（票 35）**：請求記下建立時的目標版本（目標 Task 與它的契約版本）；目標版本變了或 Task 已結束，
//!   請求標為 `superseded`，不能再套用。權限提示另外記下判定當時的授權 revision 供查閱，但授權變更不會讓它過期：
//!   修改授權與回答請求是兩回事，單次批准在執行時逐項核對綁定（票 23）。使用者提交時必須帶 `expected_revision`，請求在檢視後有任何變動就拒絕。
//! - **每一項各自保存（票 35）**：一筆裁定只對應一個請求；批次處理也是逐項套用，不形成概括授權。
//! - **答案推翻假設（票 50）**：Task 可以在契約中以 `decision` 引用一個請求，照它的建議先做。使用者的裁定與建議不同時，
//!   假設被推翻：依據它的 Task 尚未結束的裁定請求在同一個交易中標為 `superseded`（之後重新整理時也會）；
//!   與它無關的請求與 Task 不受影響。

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::approval::{self, ApprovalBinding, ArtifactMark, Mismatch};
use crate::authority::{Authority, AuthorityPolicy, Matter};
use crate::authorization::{AuthorizationRevision, Capability};
use crate::budget::{self, BudgetKind, TaskBudget};
use crate::event::DomainEvent;
use crate::permission::{self, Verdict};
use crate::run::{self, ExecutionLease, Run};
use crate::task::{Task, TaskContext, TaskStatus, Versioned};
use crate::Timestamp;

pub const DECISION_AGGREGATE: &str = "decision_request";
pub const PERMISSION_AUDIT_AGGREGATE: &str = "permission_audit";
/// Controller 動作經權限表判定的稽核（自主處理或只能人工發起，不建立請求的那些）。
pub const AUTHORITY_AUDIT_AGGREGATE: &str = "authority_audit";
pub const EVENT_DECISION_REQUESTED: &str = "decision_requested";
pub const EVENT_DECISION_ANSWERED: &str = "decision_answered";
pub const EVENT_DECISION_DEFERRED: &str = "decision_deferred";
pub const EVENT_DECISION_SUPERSEDED: &str = "decision_superseded";
pub const EVENT_DECISION_MERGED: &str = "decision_merged";
pub const EVENT_DECISION_MERGE_RECEIVED: &str = "decision_merge_received";
pub const EVENT_DECISION_LINKED: &str = "decision_linked";
pub const EVENT_PERMISSION_APPROVAL_USED: &str = "permission_approval_used";
pub const EVENT_PERMISSION_AUTO_ALLOWED: &str = "permission_auto_allowed";
pub const EVENT_CONTROLLER_ACTION_ALLOWED: &str = "controller_action_allowed";
pub const EVENT_CONTROLLER_ACTION_REFUSED: &str = "controller_action_refused";

/// 喚醒 Outbox 項目的種類前綴；完整的種類是 `前綴 + 裁定請求識別`（Outbox 的唯一鍵是種類加 Run，
/// 同一個 Run 可以提出多個請求，所以種類要帶上請求識別）。
pub const OUTBOX_WAKE_PREFIX: &str = "decision_wake:";

/// 裁定者：目前只有桌面管理憑證。
pub const DECIDED_BY_DESKTOP: &str = "desktop";

/// 權限提示裁定請求的兩個固定選項。
pub const OPTION_ALLOW_ONCE: &str = "allow_once";
pub const OPTION_DENY: &str = "deny";
/// Controller 動作裁定請求的兩個固定選項。
pub const OPTION_APPROVE: &str = "approve";
pub const OPTION_REJECT: &str = "reject";

pub const QUESTION_MAX_CHARS: usize = 500;
pub const REASON_MAX_CHARS: usize = 1_000;
pub const OPTION_ID_MAX_CHARS: usize = 40;
pub const OPTION_LABEL_MAX_CHARS: usize = 200;
pub const TRADEOFF_MAX_CHARS: usize = 500;
pub const EVIDENCE_MAX_ITEMS: usize = 10;
pub const EVIDENCE_MAX_CHARS: usize = 500;
pub const NOTE_MAX_CHARS: usize = 1_000;
/// Controller 動作摘要的上限。
pub const SUMMARY_MAX_CHARS: usize = 500;
pub const OPTIONS_MIN: usize = 2;
pub const OPTIONS_MAX: usize = 8;
/// 期限與延後最長 30 天（以分鐘計）。
pub const DEADLINE_MAX_MINUTES: i64 = 30 * 24 * 60;
/// 延後沒有指定時間時延後一天。
pub const DEFER_DEFAULT_MINUTES: i64 = 24 * 60;

pub fn wake_kind(request_id: &str) -> String {
    format!("{OUTBOX_WAKE_PREFIX}{request_id}")
}

// ---------------------------------------------------------------- 實體

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionKind {
    /// Agent 以 `request_decision` 提出。
    Agent,
    /// 權限提示超出授權範圍而建立。
    Permission,
    /// Controller 嘗試處理決策權限表中「必須送裁定」的事項，被拒絕而建立（票 35）。
    Authority,
}

impl DecisionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            DecisionKind::Agent => "agent",
            DecisionKind::Permission => "permission",
            DecisionKind::Authority => "authority",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        [DecisionKind::Agent, DecisionKind::Permission, DecisionKind::Authority].into_iter().find(|k| k.as_str() == raw)
    }

    /// 這種請求可以用哪些方式處理。權限提示只有「允許這一次／拒絕」的意義，不能修改條件、回答或要求調查。
    pub fn allowed_actions(self) -> &'static [DecisionAction] {
        use DecisionAction::*;
        match self {
            DecisionKind::Permission => &[Accept, Defer, Reject],
            DecisionKind::Agent | DecisionKind::Authority => &[Accept, Modify, Answer, Investigate, Defer, Reject],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionStatus {
    Pending,
    Answered,
    /// 目標版本已變更，請求不能再套用。
    Superseded,
    /// 已合併到另一個請求（`merged_into`），由那個請求代為裁定。
    Merged,
}

impl DecisionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            DecisionStatus::Pending => "pending",
            DecisionStatus::Answered => "answered",
            DecisionStatus::Superseded => "superseded",
            DecisionStatus::Merged => "merged",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        [DecisionStatus::Pending, DecisionStatus::Answered, DecisionStatus::Superseded, DecisionStatus::Merged]
            .into_iter()
            .find(|s| s.as_str() == raw)
    }
}

/// 使用者處理一個請求的方式（使用者故事 51）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionAction {
    /// 接受某個方案。
    Accept,
    /// 接受某個方案，但修改條件（`note` 是修改後的條件）。
    Modify,
    /// 直接回答（`note` 是回答）。
    Answer,
    /// 先要求調查（`note` 是要調查的事）。
    Investigate,
    /// 延後：請求仍待處理，工作仍等待。
    Defer,
    /// 駁回。
    Reject,
}

impl DecisionAction {
    pub fn as_str(self) -> &'static str {
        match self {
            DecisionAction::Accept => "accept",
            DecisionAction::Modify => "modify",
            DecisionAction::Answer => "answer",
            DecisionAction::Investigate => "investigate",
            DecisionAction::Defer => "defer",
            DecisionAction::Reject => "reject",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        use DecisionAction::*;
        [Accept, Modify, Answer, Investigate, Defer, Reject].into_iter().find(|a| a.as_str() == raw)
    }

    pub fn label(self) -> &'static str {
        match self {
            DecisionAction::Accept => "接受方案",
            DecisionAction::Modify => "修改條件",
            DecisionAction::Answer => "回答",
            DecisionAction::Investigate => "要求調查",
            DecisionAction::Defer => "延後",
            DecisionAction::Reject => "駁回",
        }
    }

    /// 這種處理方式要不要選一個選項。
    fn needs_option(self) -> bool {
        matches!(self, DecisionAction::Accept | DecisionAction::Modify)
    }

    /// 這種處理方式要不要附文字（修改後的條件、回答、要調查的事）。
    fn needs_note(self) -> bool {
        matches!(self, DecisionAction::Modify | DecisionAction::Answer | DecisionAction::Investigate)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Urgency {
    Low,
    Normal,
    High,
}

impl Urgency {
    pub fn as_str(self) -> &'static str {
        match self {
            Urgency::Low => "low",
            Urgency::Normal => "normal",
            Urgency::High => "high",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        [Urgency::Low, Urgency::Normal, Urgency::High].into_iter().find(|u| u.as_str() == raw)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionOption {
    pub id: String,
    pub label: String,
    /// 這個選項的取捨（可省略）。
    #[serde(default)]
    pub tradeoff: Option<String>,
}

/// 權限提示的細節（只有 `kind = permission` 的請求有）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionDetail {
    pub tool_name: String,
    pub input: Value,
    /// 這個操作需要的能力；無法歸類時為 `None`。
    pub capability: Option<Capability>,
    pub summary: String,
    pub fingerprint: String,
    /// 單次批准綁定的邏輯操作（票 23）。票 23 之前建立的請求沒有綁定，它們的「允許這一次」不能再被使用。
    #[serde(default)]
    pub binding: Option<ApprovalBinding>,
    /// 同一個操作先前已用掉一次批准、結果未知：使用者要先查證再回答。
    #[serde(default)]
    pub prior_use: Option<PriorUse>,
    /// 先前有一個單次批准，但這次的操作與它的綁定不符而不能沿用。
    #[serde(default)]
    pub rejected_approval: Option<RejectedApproval>,
}

/// 先前已被使用的單次批准。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PriorUse {
    pub request_id: String,
    pub run_id: String,
    pub used_at: Timestamp,
}

/// 不能沿用的單次批准與原因。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RejectedApproval {
    pub request_id: String,
    pub reason: Mismatch,
}

/// Controller 動作的細節（只有 `kind = authority` 的請求有）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorityDetail {
    pub matter: Matter,
    pub summary: String,
    /// `事項:目標 Task`：同一個 Task 的同一種事項視為同一個操作。
    pub fingerprint: String,
}

/// 裁定請求的提出者（票 17）。Controller 提出的請求沒有 Worker 的 Run：回答或過期時不寫喚醒 Outbox；
/// 被回答時由 Controller CLI 的事件等待（`wait`）回報「裁定已回答」。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionOrigin {
    #[default]
    Worker,
    Controller,
}

impl DecisionOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            DecisionOrigin::Worker => "worker",
            DecisionOrigin::Controller => "controller",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "worker" => Some(DecisionOrigin::Worker),
            "controller" => Some(DecisionOrigin::Controller),
            _ => None,
        }
    }
}

/// 請求建立時的目標版本；之後任一項改變，請求就不能再套用（`superseded`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionTarget {
    pub task_id: String,
    pub contract_version: i64,
    /// 權限提示判定當時的授權 revision（尚未確認授權時為 `None`；其他種類一律為 `None`）。只供查閱，不影響過期判定。
    pub authorization_revision: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionRequest {
    pub id: String,
    pub workspace_id: String,
    /// 提出請求的 Run 與 Task。Controller 提出的請求（票 17）`run_id` 是 Controller CLI 的 Run（`controller_run`），
    /// `task_id` 是目標 Task。
    pub run_id: String,
    pub task_id: String,
    /// 提出者是 Worker 還是 Controller（票 17）。
    #[serde(default)]
    pub origin: DecisionOrigin,
    pub kind: DecisionKind,
    pub status: DecisionStatus,
    pub question: String,
    /// 為什麼 Agent 無法自行決定。
    pub reason: String,
    pub options: Vec<DecisionOption>,
    /// 建議的選項識別。
    pub recommendation: Option<String>,
    pub evidence: Vec<String>,
    pub urgency: Urgency,
    pub deadline_at: Option<Timestamp>,
    pub permission: Option<PermissionDetail>,
    pub authority: Option<AuthorityDetail>,
    /// 外部操作結果未知時由 Runtime 建立的查證請求：對應的外部操作（票 26）。只能以查證選項作答，見 `allowed_actions`。
    #[serde(default)]
    pub external_operation_id: Option<String>,
    /// 阻擋範圍：受影響的 Task（不在這裡的 Task 從頭到尾不受阻擋）。
    pub blocking: Vec<String>,
    pub target: DecisionTarget,
    /// 使用者看到的內容版本：從 1 起，延後、合併、過期、回答時加一。提交時以 `expected_revision` 校驗。
    pub revision: i64,
    /// 這個請求聚合目前最後一個事件的版本（事件表以它保證順序與唯一）。
    pub event_version: i64,
    pub deferred_until: Option<Timestamp>,
    pub merged_into: Option<String>,
    /// 狀態的原因（例如為什麼過期）。
    pub status_reason: Option<String>,
    /// 互相連結的請求（雙向）。
    pub links: Vec<String>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

impl DecisionRequest {
    pub fn is_pending(&self) -> bool {
        self.status == DecisionStatus::Pending
    }

    pub fn is_overdue(&self, now: Timestamp) -> bool {
        self.is_pending() && self.deadline_at.is_some_and(|d| now > d)
    }

    pub fn is_deferred(&self, now: Timestamp) -> bool {
        self.is_pending() && self.deferred_until.is_some_and(|d| d > now)
    }

    /// 這個請求可以用哪些處理方式。外部操作的查證請求只能接受「已生效／沒有生效」其中一個選項，或延後：
    /// 其他方式會讓請求結案、外部操作卻一直停在結果未知，同一個冪等鍵就再也無法處理（票 26 整合）。
    pub fn allowed_actions(&self) -> &'static [DecisionAction] {
        if self.external_operation_id.is_some() {
            return &[DecisionAction::Accept, DecisionAction::Defer];
        }
        self.kind.allowed_actions()
    }

    fn option(&self, id: &str) -> Option<&DecisionOption> {
        self.options.iter().find(|o| o.id == id)
    }

    /// 相似問題的比對鍵：同種類、正規化後相同的問題（權限提示是同一個操作）視為相似。
    pub fn similarity_key(&self) -> String {
        match (&self.permission, &self.authority) {
            (Some(p), _) => format!("permission:{}", p.fingerprint),
            (_, Some(a)) => format!("authority:{}:{}", a.matter.as_str(), normalize_question(&a.summary)),
            _ => format!("{}:{}", self.kind.as_str(), normalize_question(&self.question)),
        }
    }

    /// 改動可變欄位前呼叫：內容版本加一。
    fn bump(&mut self, now: Timestamp) {
        self.revision += 1;
        self.updated_at = now;
    }

    /// 這個請求聚合的下一個事件。
    fn event(&mut self, event_type: &'static str, now: Timestamp, payload: Value) -> DomainEvent {
        self.event_version += 1;
        event(DECISION_AGGREGATE, &self.id, self.event_version, event_type, now, payload)
    }
}

/// 問題文字的正規化：只保留字母、數字與各種文字（含中文），英文字母轉小寫。
pub fn normalize_question(text: &str) -> String {
    text.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// 使用者對一個請求的正式裁定（追加式）。延後不是裁定，不會產生這筆紀錄。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decision {
    pub request_id: String,
    pub action: DecisionAction,
    /// `accept`／`modify` 選的選項；其他處理方式為 `None`。
    pub option_id: Option<String>,
    /// 修改後的條件、回答、要調查的事或備註。
    pub note: Option<String>,
    pub decided_by: String,
    pub decided_at: Timestamp,
    /// 批次處理時的批次識別；只用於稽核，不代表任何概括授權。
    pub batch_id: Option<String>,
}

impl Decision {
    /// 這筆裁定是否核准了單次批准的操作（權限提示或 Controller 動作）。
    fn approves(&self, approve_option: &str) -> bool {
        matches!(self.action, DecisionAction::Accept | DecisionAction::Modify)
            && self.option_id.as_deref() == Some(approve_option)
    }
}

/// 單次批准已被消耗的紀錄。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalUse {
    pub request_id: String,
    pub run_id: String,
    pub used_at: Timestamp,
}

/// 要追加的喚醒 Outbox 項目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeItem {
    pub kind: String,
    pub run_id: String,
    pub payload: Value,
    pub created_at: Timestamp,
}

// ---------------------------------------------------------------- Agent 提出請求的內容

fn clean_text(name: &str, raw: &str, max: usize) -> Result<String, DecisionError> {
    let value = raw.trim();
    if value.is_empty() {
        return Err(DecisionError::Invalid(format!("{name}不可空白")));
    }
    if value.chars().count() > max {
        return Err(DecisionError::Invalid(format!("{name}不可超過 {max} 個字元")));
    }
    Ok(value.to_string())
}

fn clean_evidence(evidence: Vec<String>) -> Result<Vec<String>, DecisionError> {
    if evidence.len() > EVIDENCE_MAX_ITEMS {
        return Err(DecisionError::Invalid(format!("證據最多 {EVIDENCE_MAX_ITEMS} 項")));
    }
    evidence.iter().map(|e| clean_text("證據", e, EVIDENCE_MAX_CHARS)).collect()
}

/// Agent 以 `request_decision` 提出的內容（已驗證）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestContent {
    pub question: String,
    pub reason: String,
    pub options: Vec<DecisionOption>,
    pub recommendation: Option<String>,
    pub evidence: Vec<String>,
    pub urgency: Urgency,
    pub deadline_minutes: Option<i64>,
}

impl RequestContent {
    /// 驗證並整理（去除前後空白）；任何一項無效都拒絕整個請求，不寫入任何東西。
    pub fn new(
        question: &str,
        reason: &str,
        options: Vec<DecisionOption>,
        recommendation: Option<&str>,
        evidence: Vec<String>,
        urgency: Option<Urgency>,
        deadline_minutes: Option<i64>,
    ) -> Result<Self, DecisionError> {
        let invalid = |message: String| DecisionError::Invalid(message);
        let question = clean_text("問題", question, QUESTION_MAX_CHARS)?;
        let reason = clean_text("原因（為什麼無法自行決定）", reason, REASON_MAX_CHARS)?;
        if !(OPTIONS_MIN..=OPTIONS_MAX).contains(&options.len()) {
            return Err(invalid(format!("選項必須有 {OPTIONS_MIN} 到 {OPTIONS_MAX} 個")));
        }
        let mut cleaned: Vec<DecisionOption> = Vec::with_capacity(options.len());
        for option in options {
            let id = clean_text("選項識別", &option.id, OPTION_ID_MAX_CHARS)?;
            if !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
                return Err(invalid(format!("選項識別「{id}」只能使用英數字、底線與連字號")));
            }
            if cleaned.iter().any(|o| o.id == id) {
                return Err(invalid(format!("選項識別「{id}」重複")));
            }
            let label = clean_text("選項名稱", &option.label, OPTION_LABEL_MAX_CHARS)?;
            let tradeoff = match option.tradeoff.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
                Some(t) => Some(clean_text("選項的取捨", t, TRADEOFF_MAX_CHARS)?),
                None => None,
            };
            cleaned.push(DecisionOption { id, label, tradeoff });
        }
        let recommendation = match recommendation.map(str::trim).filter(|r| !r.is_empty()) {
            Some(id) if cleaned.iter().any(|o| o.id == id) => Some(id.to_string()),
            Some(id) => return Err(invalid(format!("建議的選項「{id}」不在選項之中"))),
            None => None,
        };
        let evidence = clean_evidence(evidence)?;
        if deadline_minutes.is_some_and(|m| !(1..=DEADLINE_MAX_MINUTES).contains(&m)) {
            return Err(invalid(format!("期限必須是 1 到 {DEADLINE_MAX_MINUTES} 分鐘")));
        }
        Ok(Self { question, reason, options: cleaned, recommendation, evidence, urgency: urgency.unwrap_or(Urgency::Normal), deadline_minutes })
    }
}

/// Controller 以 `controller_action` 提出的動作內容（已驗證）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionContent {
    pub matter: Matter,
    /// 目標 Task；省略時是呼叫者 Run 的 Task。
    pub task_id: Option<String>,
    pub summary: String,
    pub reason: String,
    pub evidence: Vec<String>,
    pub urgency: Urgency,
    /// 同一個 Task 內的操作對象（例如某個 review finding、某個設計偏離，票 44）；有的話只有同一個對象才視為同一個操作，
    /// 核准不會被同一個 Task 的其他對象用掉。
    pub subject: Option<String>,
}

impl ActionContent {
    pub fn new(
        matter: Matter,
        task_id: Option<&str>,
        summary: &str,
        reason: &str,
        evidence: Vec<String>,
        urgency: Option<Urgency>,
    ) -> Result<Self, DecisionError> {
        Ok(Self {
            matter,
            task_id: task_id.map(str::trim).filter(|t| !t.is_empty()).map(str::to_string),
            summary: clean_text("動作摘要", summary, SUMMARY_MAX_CHARS)?,
            reason: clean_text("理由", reason, REASON_MAX_CHARS)?,
            evidence: clean_evidence(evidence)?,
            urgency: urgency.unwrap_or(Urgency::Normal),
            subject: None,
        })
    }

    /// 指定操作對象（見 `subject`）。
    pub fn with_subject(mut self, subject: &str) -> Self {
        self.subject = Some(subject.to_string());
        self
    }
}

/// 使用者對一個請求的處理（單筆或批次中的一項）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
    pub action: DecisionAction,
    pub option_id: Option<String>,
    pub note: Option<String>,
    /// 只有延後用到；省略時延後一天。
    pub defer_minutes: Option<i64>,
}

// ---------------------------------------------------------------- 命令、結果與錯誤

#[derive(Debug, Clone, PartialEq)]
pub enum DecisionCommand {
    /// Agent 提出裁定請求（`request_decision`）。`request_id` 由呼叫端產生。
    Request { request_id: String, run_id: String, generation: i64, content: RequestContent },
    /// Worker 的權限提示（`permission_prompt`）。`request_id`、`audit_id` 由呼叫端產生，用不到時丟棄。
    PermissionPrompt {
        request_id: String,
        audit_id: String,
        run_id: String,
        generation: i64,
        tool_name: String,
        input: Value,
    },
    /// Controller 嘗試一個動作（`controller_action`），依決策權限表判定。`request_id`、`audit_id` 由呼叫端產生。
    ControllerAction { request_id: String, audit_id: String, run_id: String, generation: i64, content: ActionContent },
    /// Controller（票 17）向使用者提問，`blocking` 是依賴這個答案的 Task（只能是 pending 或 ready）。
    /// 呼叫端已確認那個 Controller CLI 的 Run 還沒結束。
    ControllerQuestion { request_id: String, controller_run_id: String, content: RequestContent, blocking: Vec<String> },
    /// Controller 的工具在執行變更前由 Runtime 依實際變更推導出事項（票 17），依決策權限表判定；規則同 `ControllerAction`，
    /// 但提出者是 Controller CLI 的 Run、目標 Task 必填。呼叫端已確認那個 Run 還沒結束。
    ControllerGate { request_id: String, audit_id: String, controller_run_id: String, content: ActionContent },
    /// 使用者（桌面憑證）處理一個請求。
    Answer {
        request_id: String,
        expected_revision: i64,
        resolution: Resolution,
        decided_by: String,
        batch_id: Option<String>,
    },
    /// 使用者把 `request_id` 合併到 `into`（由 `into` 代為裁定）。
    Merge { request_id: String, expected_revision: i64, into: String, into_expected_revision: i64 },
    /// 使用者把兩個請求互相連結。
    Link { request_id: String, other_id: String },
    /// 把目標版本已變更的待處理請求標為 `superseded`（開啟裁定中心時）。
    Refresh,
}

/// 等待裁定時，附帶說明先前的單次批准為什麼不能用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalCheck {
    /// 綁定不符（參數、目標、成果版本…改變）。
    Mismatch { request_id: String, reason: Mismatch },
    /// 同一個操作已經用掉一次批准，結果未知，要先查證。
    UsedResultUnknown { request_id: String },
}

impl ApprovalCheck {
    pub fn request_id(&self) -> &str {
        match self {
            ApprovalCheck::Mismatch { request_id, .. } | ApprovalCheck::UsedResultUnknown { request_id } => request_id,
        }
    }

    /// 穩定的機器可讀代碼。
    pub fn code(&self) -> &'static str {
        match self {
            ApprovalCheck::Mismatch { reason, .. } => reason.as_str(),
            ApprovalCheck::UsedResultUnknown { .. } => "used_result_unknown",
        }
    }
}

/// 權限提示給 Worker 的結果。
#[derive(Debug, Clone, PartialEq)]
pub enum PermissionOutcome {
    /// 在授權範圍內：自動允許並已記錄。
    Allowed { capability: Capability, revision: i64 },
    /// 範圍外：等待裁定（Worker 應保存 checkpoint 並以 blocked 結束，或改做其他步驟）。
    /// `check` 說明先前的單次批准為什麼不能用（沒有相關的批准時為 `None`）。
    Waiting { request_id: String, message: String, check: Option<ApprovalCheck> },
    /// 使用者已裁定「允許這一次」，這次提示消耗了它。
    Approved { request_id: String },
    /// 使用者已裁定拒絕（這次提示消耗了它）。
    Denied { request_id: String },
}

/// Controller 動作經決策權限表判定的結果。
#[derive(Debug, Clone, PartialEq)]
pub enum ControllerOutcome {
    /// 可自主處理。
    Allowed,
    /// 必須送裁定：動作沒有執行，已建立（或回到既有的）裁定請求。
    RequiresDecision { request_id: String, existing: bool },
    /// 使用者已核准這一次（這次呼叫消耗了它）；`conditions` 是使用者修改後的條件。
    Approved { request_id: String, conditions: Option<String> },
    /// 使用者沒有核准（這次呼叫消耗了它）。
    Denied { request_id: String, note: Option<String> },
    /// 只能由使用者發起，不建立請求。
    HumanOnly,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DecisionOutcome {
    /// 請求已建立（`existing` 為真表示同一個請求仍在等待，沒有重複建立）。
    Requested { request_id: String, existing: bool },
    Permission(PermissionOutcome),
    Controller { matter: Matter, authority: Authority, outcome: ControllerOutcome },
    /// 請求已有裁定（同樣的處理重送時 `resumed_task_ids` 為空）。
    Answered { resumed_task_ids: Vec<String> },
    /// 延後到某個時間；工作仍在等待。
    Deferred,
    /// 提交時發現目標版本已變更：請求已標為 `superseded`（這個標記要寫入），API 回報不能套用。
    Superseded { request_id: String, reason: String, resumed_task_ids: Vec<String> },
    Merged,
    Linked,
    /// 開啟裁定中心時的重新整理：被標為 `superseded` 的請求。
    Refreshed { superseded: Vec<String> },
}

/// 命令成功後的結果；沒有任何變動（冪等）時各欄位都是空的。
#[derive(Debug, Clone, PartialEq)]
pub struct DecisionChange {
    /// 新建立的請求。
    pub created: Option<DecisionRequest>,
    /// 可變欄位（狀態、內容版本、延後、合併、原因）改變的既有請求，內容是改變後的樣子。
    pub updated: Vec<DecisionRequest>,
    /// 新增的阻擋範圍 `(請求, Task)`（合併時）。
    pub new_blocks: Vec<(String, String)>,
    /// 新增的連結（兩個請求識別，較小者在前）。
    pub new_links: Vec<(String, String)>,
    pub decision: Option<Decision>,
    pub approval_use: Option<ApprovalUse>,
    /// 因此改變的 Task。
    pub tasks: Vec<Task>,
    /// 恢復排程（回到 `ready`）的 Task；Store 要在同一個交易把它們排回派工佇列。
    pub resumed: Vec<String>,
    /// 因核准「預算用盡後的下一步」而重設的預算（票 36）。
    pub budgets: Vec<TaskBudget>,
    pub outbox: Vec<WakeItem>,
    pub events: Vec<DomainEvent>,
    pub outcome: DecisionOutcome,
}

impl DecisionChange {
    fn new(outcome: DecisionOutcome) -> Self {
        Self {
            created: None,
            updated: Vec::new(),
            new_blocks: Vec::new(),
            new_links: Vec::new(),
            decision: None,
            approval_use: None,
            tasks: Vec::new(),
            resumed: Vec::new(),
            budgets: Vec::new(),
            outbox: Vec::new(),
            events: Vec::new(),
            outcome,
        }
    }

    pub fn changed(&self) -> bool {
        self.created.is_some()
            || !self.updated.is_empty()
            || !self.new_blocks.is_empty()
            || !self.new_links.is_empty()
            || self.decision.is_some()
            || self.approval_use.is_some()
            || !self.tasks.is_empty()
            || !self.outbox.is_empty()
            || !self.events.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DecisionError {
    #[error("{0}")]
    Invalid(String),
    #[error("找不到 Workspace {0}")]
    WorkspaceNotFound(String),
    #[error("找不到 Run {0}")]
    RunNotFound(String),
    #[error("找不到 Task {0}")]
    TaskNotFound(String),
    #[error("找不到裁定請求 {0}")]
    NotFound(String),
    #[error("這個 Run 已經結束或已被接替，不能再提出裁定請求")]
    StaleRun,
    #[error("選項「{0}」不在這個裁定請求的選項之中")]
    OptionUnknown(String),
    #[error("這個裁定請求已經回答過了（{0}），不能改成別的答案")]
    AlreadyAnswered(String),
    #[error("只有桌面管理憑證提交的裁定才算正式裁定")]
    NotDesktop,
    #[error("這個裁定請求在你檢視之後已經變更（目前 revision {current}），請重新整理後再處理")]
    RevisionMismatch { current: i64 },
    #[error("目標版本已變更，這個裁定請求已過期，不能套用：{0}")]
    Superseded(String),
    #[error("這個裁定請求已合併到 {0}，請到那一筆處理")]
    Merged(String),
    #[error("這種裁定請求不能用「{0}」處理")]
    ActionNotAllowed(&'static str),
    #[error("{0}")]
    MergeInvalid(String),
}

impl DecisionError {
    /// 穩定的機器可讀代碼，API 與 UI 依它判斷錯誤種類。
    pub fn code(&self) -> &'static str {
        match self {
            DecisionError::Invalid(_) => "decision_invalid",
            DecisionError::WorkspaceNotFound(_) => "workspace_not_found",
            DecisionError::RunNotFound(_) => "run_not_found",
            DecisionError::TaskNotFound(_) => "task_not_found",
            DecisionError::NotFound(_) => "decision_not_found",
            DecisionError::StaleRun => "run_report_stale",
            DecisionError::OptionUnknown(_) => "decision_option_unknown",
            DecisionError::AlreadyAnswered(_) => "decision_already_answered",
            DecisionError::NotDesktop => "decision_not_desktop",
            DecisionError::RevisionMismatch { .. } => "decision_revision_mismatch",
            DecisionError::Superseded(_) => "decision_superseded",
            DecisionError::Merged(_) => "decision_merged",
            DecisionError::ActionNotAllowed(_) => "decision_action_not_allowed",
            DecisionError::MergeInvalid(_) => "decision_merge_invalid",
        }
    }
}

// ---------------------------------------------------------------- 判定

/// 目前狀態的唯讀快照（同一個 Workspace 內）。Store 在交易中載入。
pub struct DecisionContext<'a> {
    pub task: &'a TaskContext<'a>,
    pub runs: &'a [Run],
    pub leases: &'a [ExecutionLease],
    pub requests: &'a [DecisionRequest],
    pub decisions: &'a [Decision],
    pub uses: &'a [ApprovalUse],
    /// 這個 Workspace 的決策權限表設定。
    pub authority: &'a AuthorityPolicy,
    /// 各 Task 成果的最新版本（單次批准綁定的成果版本，票 23）。
    pub artifacts: &'a [ArtifactMark],
    /// Workspace 的全部授權 revision（由舊到新）：權限提示依「Run 准入時固定的 revision」判定（R1-a，票 24）。
    pub authorizations: &'a [AuthorizationRevision],
    /// 各 Task 的修正與重試預算（票 36）：核准用盡時建立的請求要重設它。
    pub budgets: &'a [TaskBudget],
}

impl DecisionContext<'_> {
    fn decision_of(&self, request_id: &str) -> Option<&Decision> {
        self.decisions.iter().find(|d| d.request_id == request_id)
    }

    fn approval_used(&self, request_id: &str) -> bool {
        self.uses.iter().any(|u| u.request_id == request_id)
    }

    fn use_of(&self, request_id: &str) -> Option<&ApprovalUse> {
        self.uses.iter().find(|u| u.request_id == request_id)
    }

    /// Run 沿用的授權：准入時固定的 revision，不是 Workspace 目前生效的 revision（R1-a）。
    /// 准入時還沒有任何授權的 Run 是 `None`：之後才確認的授權不會回頭套用到它身上。
    fn authorization_of(&self, run: &Run) -> Option<&AuthorizationRevision> {
        run.authorization_revision.and_then(|number| self.authorizations.iter().find(|r| r.revision == number))
    }

    fn request(&self, id: &str) -> Result<&DecisionRequest, DecisionError> {
        self.requests.iter().find(|r| r.id == id).ok_or_else(|| DecisionError::NotFound(id.to_string()))
    }

    /// 確認 Run 仍是目前的寫入所有者，回傳 Run 與它的 Task。
    fn current_run(&self, run_id: &str, generation: i64) -> Result<(&Run, &Task), DecisionError> {
        let run = self.runs.iter().find(|r| r.id == run_id).ok_or_else(|| DecisionError::RunNotFound(run_id.to_string()))?;
        let lease = self.leases.iter().find(|l| l.run_id == run.id);
        if !run::is_current_writer(run, lease, generation) {
            return Err(DecisionError::StaleRun);
        }
        let task = self.task.task(&run.task_id).ok_or_else(|| DecisionError::TaskNotFound(run.task_id.clone()))?;
        Ok((run, task))
    }

    /// 目標版本已變更的原因；仍有效回傳 `None`。
    fn stale_reason(&self, request: &DecisionRequest) -> Option<String> {
        // 外部操作有沒有生效與 Task 的契約、狀態無關：查證請求不會過期，否則外部操作會一直停在結果未知（票 26 整合）
        if request.external_operation_id.is_some() {
            return None;
        }
        // 請求提出之後，它的 Task 依據的假設被使用者推翻：請求的前提已改變（票 50）
        for task_id in std::iter::once(&request.target.task_id).chain(&request.blocking) {
            if let Some(why) = self.task.overturned_basis_after(task_id, request.created_at) {
                return Some(format!("{}依據的假設已被推翻：{why}", self.task.title_of(task_id)));
            }
        }
        // Controller 的提問（票 17）問的是使用者的決定，不是某個契約版本：Controller 依答案補契約是預期中的事，
        // 所以契約變更不會讓它過期；只有依賴它的 Task 全部結束（或不存在）時才過期
        if request.origin == DecisionOrigin::Controller && request.kind == DecisionKind::Agent {
            let open = request.blocking.iter().any(|id| self.task.task(id).is_some_and(|t| !t.status.is_terminal()));
            return (!open).then(|| "依賴這個答案的 Task 都已結束".to_string());
        }
        let target = &request.target;
        let Some(task) = self.task.task(&target.task_id) else {
            return Some("目標 Task 已不存在".to_string());
        };
        if task.status.is_terminal() {
            return Some(format!("目標 Task「{}」已結束（{}）", task.title, task.status.as_str()));
        }
        // 預算用盡的裁定（票 36）問的是「用盡之後怎麼做」，與契約版本無關：使用者常會先修改契約再核准重設，
        // 過期的話就再也無法重設，Task 卡在阻擋（R151 第 10 項）。只限目前用盡的那一個請求
        let budget = TaskBudget::of(self.budgets, task);
        if budget.exhausted.is_some() && budget.request_id.as_deref() == Some(request.id.as_str()) {
            return None;
        }
        if task.contract_version != target.contract_version {
            return Some(format!(
                "目標 Task「{}」的契約已從版本 {} 改為版本 {}",
                task.title, target.contract_version, task.contract_version
            ));
        }
        None
    }
}

fn event(
    aggregate_type: &'static str,
    aggregate_id: &str,
    aggregate_version: i64,
    event_type: &'static str,
    now: Timestamp,
    payload: Value,
) -> DomainEvent {
    DomainEvent { aggregate_type, aggregate_id: aggregate_id.to_string(), aggregate_version, event_type, occurred_at: now, payload }
}

pub fn decide(ctx: &DecisionContext<'_>, command: DecisionCommand, now: Timestamp) -> Result<DecisionChange, DecisionError> {
    if ctx.task.workspace.is_none() {
        return Err(DecisionError::WorkspaceNotFound(ctx.task.workspace_id.to_string()));
    }
    match command {
        DecisionCommand::Request { request_id, run_id, generation, content } => {
            request(ctx, request_id, &run_id, generation, content, now)
        }
        DecisionCommand::PermissionPrompt { request_id, audit_id, run_id, generation, tool_name, input } => {
            permission_prompt(ctx, request_id, audit_id, &run_id, generation, &tool_name, input, now)
        }
        DecisionCommand::ControllerAction { request_id, audit_id, run_id, generation, content } => {
            let (run, caller) = ctx.current_run(&run_id, generation)?;
            controller_action(ctx, request_id, audit_id, Requester::Worker(run), caller, content, now)
        }
        DecisionCommand::ControllerQuestion { request_id, controller_run_id, content, blocking } => {
            controller_question(ctx, request_id, &controller_run_id, content, blocking, now)
        }
        DecisionCommand::ControllerGate { request_id, audit_id, controller_run_id, content } => {
            let task_id = content.task_id.clone().ok_or_else(|| DecisionError::Invalid("必須指定目標 Task".into()))?;
            let target = ctx.task.task(&task_id).ok_or_else(|| DecisionError::TaskNotFound(task_id.clone()))?;
            let requester = Requester::Controller { run_id: &controller_run_id, workspace_id: ctx.task.workspace_id };
            controller_action(ctx, request_id, audit_id, requester, target, content, now)
        }
        DecisionCommand::Answer { request_id, expected_revision, resolution, decided_by, batch_id } => {
            answer(ctx, &request_id, expected_revision, resolution, &decided_by, batch_id, now)
        }
        DecisionCommand::Merge { request_id, expected_revision, into, into_expected_revision } => {
            merge(ctx, &request_id, expected_revision, &into, into_expected_revision, now)
        }
        DecisionCommand::Link { request_id, other_id } => link(ctx, &request_id, &other_id, now),
        DecisionCommand::Refresh => refresh(ctx, now),
    }
}

/// 請求的提出者：Worker 的 Run，或 Controller CLI 的 Run（票 17）。
#[derive(Clone, Copy)]
enum Requester<'a> {
    Worker(&'a Run),
    Controller { run_id: &'a str, workspace_id: &'a str },
}

impl Requester<'_> {
    fn run_id(&self) -> &str {
        match self {
            Requester::Worker(run) => &run.id,
            Requester::Controller { run_id, .. } => run_id,
        }
    }

    fn workspace_id(&self) -> &str {
        match self {
            Requester::Worker(run) => &run.workspace_id,
            Requester::Controller { workspace_id, .. } => workspace_id,
        }
    }

    fn origin(&self) -> DecisionOrigin {
        match self {
            Requester::Worker(_) => DecisionOrigin::Worker,
            Requester::Controller { .. } => DecisionOrigin::Controller,
        }
    }

    /// 權限提示依 Run 准入時固定的 revision 判定（R1-a，票 24）；Controller 沒有准入的 revision。
    fn authorization_revision(&self) -> Option<i64> {
        match self {
            Requester::Worker(run) => run.authorization_revision,
            Requester::Controller { .. } => None,
        }
    }
}

/// 新請求的共同欄位；`blocking` 只有目標 Task。
#[allow(clippy::too_many_arguments)]
fn new_request(
    id: String,
    requester: &Requester<'_>,
    caller: &Task,
    target: &Task,
    kind: DecisionKind,
    question: String,
    reason: String,
    now: Timestamp,
) -> DecisionRequest {
    DecisionRequest {
        id,
        workspace_id: requester.workspace_id().to_string(),
        run_id: requester.run_id().to_string(),
        task_id: caller.id.clone(),
        origin: requester.origin(),
        kind,
        status: DecisionStatus::Pending,
        question,
        reason,
        options: Vec::new(),
        recommendation: None,
        evidence: Vec::new(),
        urgency: Urgency::Normal,
        deadline_at: None,
        permission: None,
        authority: None,
        external_operation_id: None,
        blocking: vec![target.id.clone()],
        target: DecisionTarget {
            task_id: target.id.clone(),
            contract_version: target.contract_version,
            // 權限提示依 Run 准入時固定的 revision 判定（R1-a，票 24），記下的就是那一版
            authorization_revision: (kind == DecisionKind::Permission).then_some(requester.authorization_revision()).flatten(),
        },
        revision: 1,
        event_version: 0,
        deferred_until: None,
        merged_into: None,
        status_reason: None,
        links: Vec::new(),
        created_at: now,
        updated_at: now,
    }
}

/// 請求建立的事件（請求聚合的第 1 版）；外部操作結果未知時由 Runtime 建立的請求也用它（票 26）。
pub fn requested_event(request: &mut DecisionRequest, linked: &[String], now: Timestamp) -> DomainEvent {
    let payload = serde_json::json!({
        "kind": request.kind,
        "run_id": request.run_id,
        "task_id": request.task_id,
        "question": request.question,
        "blocking": request.blocking,
        "target": request.target,
        "linked": linked,
    });
    request.event(EVENT_DECISION_REQUESTED, now, payload)
}

/// 建立請求、讓目標 Task 進入 `waiting_decision`（只有 `running` 的 Task 會轉換），並和相似的待處理請求互相連結。
fn open_request(
    ctx: &DecisionContext<'_>,
    mut request: DecisionRequest,
    task: &Task,
    now: Timestamp,
    outcome: DecisionOutcome,
) -> Result<DecisionChange, DecisionError> {
    let mut change = DecisionChange::new(outcome);
    let key = request.similarity_key();
    let similar: Vec<String> = ctx
        .requests
        .iter()
        .filter(|r| r.is_pending() && r.id != request.id && r.similarity_key() == key)
        .map(|r| r.id.clone())
        .collect();
    let requested = requested_event(&mut request, &similar, now);
    change.events.push(requested);
    for other in similar {
        change.new_links.push(link_pair(&request.id, &other));
        request.links.push(other);
    }
    if task.status == TaskStatus::Running {
        let mut moved = task.clone();
        let mut versioned = Versioned { task: &mut moved, events: Vec::new(), now };
        versioned
            .change_status(TaskStatus::WaitingDecision, EVENT_DECISION_REQUESTED)
            .map_err(|_| DecisionError::TaskNotFound(task.id.clone()))?;
        change.events.extend(versioned.events);
        change.tasks.push(moved);
    }
    change.created = Some(request);
    Ok(change)
}

fn link_pair(a: &str, b: &str) -> (String, String) {
    if a < b {
        (a.to_string(), b.to_string())
    } else {
        (b.to_string(), a.to_string())
    }
}

fn request(
    ctx: &DecisionContext<'_>,
    request_id: String,
    run_id: &str,
    generation: i64,
    content: RequestContent,
    now: Timestamp,
) -> Result<DecisionChange, DecisionError> {
    let (run, task) = ctx.current_run(run_id, generation)?;
    // 同一個 Run 重複提出同一個問題：回到仍在等待的那一個，不重複建立
    if let Some(existing) = ctx
        .requests
        .iter()
        .find(|r| r.is_pending() && r.kind == DecisionKind::Agent && r.run_id == run.id && r.question == content.question)
    {
        return Ok(DecisionChange::new(DecisionOutcome::Requested { request_id: existing.id.clone(), existing: true }));
    }
    let mut record =
        new_request(request_id.clone(), &Requester::Worker(run), task, task, DecisionKind::Agent, content.question, content.reason, now);
    record.options = content.options;
    record.recommendation = content.recommendation;
    record.evidence = content.evidence;
    record.urgency = content.urgency;
    record.deadline_at = content.deadline_minutes.map(|m| now.plus_millis(m * 60_000));
    open_request(ctx, record, task, now, DecisionOutcome::Requested { request_id, existing: false })
}

/// 同一個操作先前已裁定、還沒被消耗的單次批准（權限提示或 Controller 動作共用）。
fn unused_answer<'a>(
    ctx: &'a DecisionContext<'_>,
    same_operation: impl Fn(&DecisionRequest) -> bool,
) -> Option<(&'a DecisionRequest, &'a Decision)> {
    ctx.requests
        .iter()
        .filter(|r| same_operation(r) && r.status == DecisionStatus::Answered && !ctx.approval_used(&r.id))
        .find_map(|r| ctx.decision_of(&r.id).map(|d| (r, d)))
}

/// 消耗一次單次批准：紀錄與事件。
fn consume(
    answered: &DecisionRequest,
    decision: &Decision,
    run_id: &str,
    now: Timestamp,
    outcome: DecisionOutcome,
    detail: Value,
) -> DecisionChange {
    let mut change = DecisionChange::new(outcome);
    change.approval_use = Some(ApprovalUse { request_id: answered.id.clone(), run_id: run_id.to_string(), used_at: now });
    let mut payload = serde_json::json!({ "run_id": run_id, "action": decision.action, "option_id": decision.option_id });
    if let (Some(target), Value::Object(extra)) = (payload.as_object_mut(), detail) {
        target.extend(extra);
    }
    change.events.push(event(
        DECISION_AGGREGATE,
        &answered.id,
        answered.event_version + 1,
        EVENT_PERMISSION_APPROVAL_USED,
        now,
        payload,
    ));
    change
}

const WAITING_MESSAGE: &str = "等待裁定：這個操作超出 Workspace 授權範圍，已建立裁定請求交給使用者。\
請先保存 checkpoint，再以 report_blocked 結束，或改做不需要這個操作的其他步驟；不要重試相同操作來繞過。";

#[allow(clippy::too_many_arguments)]
fn permission_prompt(
    ctx: &DecisionContext<'_>,
    request_id: String,
    audit_id: String,
    run_id: &str,
    generation: i64,
    tool_name: &str,
    input: Value,
    now: Timestamp,
) -> Result<DecisionChange, DecisionError> {
    if tool_name.trim().is_empty() {
        return Err(DecisionError::Invalid("權限提示必須帶有工具名稱 tool_name".to_string()));
    }
    let (run, task) = ctx.current_run(run_id, generation)?;
    let workspace = ctx.task.workspace.ok_or_else(|| DecisionError::WorkspaceNotFound(ctx.task.workspace_id.to_string()))?;
    let authorization = ctx.authorization_of(run);
    let summary = permission::summarize(tool_name, &input);

    let (capability, reason) = match permission::evaluate(tool_name, &input, &workspace.path, authorization) {
        Verdict::Allow { capability, revision } => {
            let mut change = DecisionChange::new(DecisionOutcome::Permission(PermissionOutcome::Allowed { capability, revision }));
            change.events.push(event(
                PERMISSION_AUDIT_AGGREGATE,
                &audit_id,
                1,
                EVENT_PERMISSION_AUTO_ALLOWED,
                now,
                serde_json::json!({
                    "run_id": run.id,
                    "task_id": task.id,
                    "tool_name": tool_name,
                    "summary": summary,
                    "capability": capability,
                    "authorization_revision": revision,
                }),
            ));
            return Ok(change);
        }
        Verdict::OutOfScope { capability, reason } => (capability, reason),
    };

    let fingerprint = permission::fingerprint(tool_name, &input);
    let current = approval::bind(tool_name, &input, approval::artifact_version_of(&task.id, ctx.artifacts));
    let is_permission_of_task = |r: &&DecisionRequest| r.kind == DecisionKind::Permission && r.task_id == task.id;
    // 同一個操作＝要求雜湊相同（說明欄位不算）；沒有綁定的舊請求以原本的指紋比對
    let same_operation = |r: &&DecisionRequest| {
        is_permission_of_task(r)
            && r.permission.as_ref().is_some_and(|p| match &p.binding {
                Some(binding) => binding.request_hash == current.request_hash,
                None => p.fingerprint == fingerprint,
            })
    };
    if let Some(pending) = ctx.requests.iter().filter(same_operation).find(|r| r.is_pending()) {
        return Ok(DecisionChange::new(DecisionOutcome::Permission(PermissionOutcome::Waiting {
            request_id: pending.id.clone(),
            message: WAITING_MESSAGE.to_string(),
            check: None,
        })));
    }

    // 已回答、還沒被使用的裁定：由新到舊，第一個適用的被這次提示消耗。
    // 拒絕（含駁回）一律適用；「允許這一次」要在執行時逐項核對綁定，任何一項不同就不能沿用
    let mut check: Option<ApprovalCheck> = None;
    let unused = ctx.requests.iter().filter(same_operation).filter(|r| !ctx.approval_used(&r.id)).rev();
    for answered in unused {
        let Some(decision) = ctx.decision_of(&answered.id) else { continue };
        let allows = decision.approves(OPTION_ALLOW_ONCE);
        let usable = !allows
            || match answered.permission.as_ref().and_then(|p| p.binding.as_ref()) {
                Some(binding) => match approval::check(binding, &current) {
                    Ok(()) => true,
                    Err(reason) => {
                        check.get_or_insert(ApprovalCheck::Mismatch { request_id: answered.id.clone(), reason });
                        false
                    }
                },
                // 票 23 之前的批准沒有綁定可以核對：保守地不使用
                None => false,
            };
        if !usable {
            continue;
        }
        let outcome = if allows {
            PermissionOutcome::Approved { request_id: answered.id.clone() }
        } else {
            PermissionOutcome::Denied { request_id: answered.id.clone() }
        };
        return Ok(consume(
            answered,
            decision,
            &run.id,
            now,
            DecisionOutcome::Permission(outcome),
            serde_json::json!({ "tool_name": tool_name, "summary": summary, "binding": current }),
        ));
    }

    // 參數或目標改變：同一個 Task 對同一種操作還有未使用的「允許這一次」，但要求不同
    if check.is_none() {
        check = ctx
            .requests
            .iter()
            .filter(is_permission_of_task)
            .filter(|r| !ctx.approval_used(&r.id))
            .filter(|r| ctx.decision_of(&r.id).is_some_and(|d| d.approves(OPTION_ALLOW_ONCE)))
            .rev()
            .find_map(|r| {
                let binding = r.permission.as_ref()?.binding.as_ref()?;
                if binding.operation != current.operation {
                    return None;
                }
                approval::check(binding, &current).err().map(|reason| ApprovalCheck::Mismatch { request_id: r.id.clone(), reason })
            });
    }
    // 同一個操作已經用掉一次批准：Runtime 看不到上一次的結果，要先查證再決定
    let prior_use = ctx
        .requests
        .iter()
        .filter(same_operation)
        .filter(|r| ctx.decision_of(&r.id).is_some_and(|d| d.approves(OPTION_ALLOW_ONCE)))
        .filter_map(|r| ctx.use_of(&r.id))
        .next_back()
        .map(|u| PriorUse { request_id: u.request_id.clone(), run_id: u.run_id.clone(), used_at: u.used_at });
    if let Some(prior) = &prior_use {
        check.get_or_insert(ApprovalCheck::UsedResultUnknown { request_id: prior.request_id.clone() });
    }

    let mut evidence = vec![format!("工具：{tool_name}"), format!("輸入：{}", truncate(&input.to_string(), 300))];
    if let Some(ApprovalCheck::Mismatch { request_id, reason }) = &check {
        evidence.push(format!("先前的單次批准（{request_id}）不能沿用：{}", reason.label()));
    }
    let question = match &prior_use {
        Some(_) => format!("「{summary}」先前已批准並放行過一次，結果未知。查證之後，要再允許一次嗎？"),
        None => format!("允許 Worker 執行「{summary}」嗎？"),
    };
    let mut reason_text = match capability {
        Some(capability) => format!("這個操作需要「{}」，但目前的 Workspace 授權不涵蓋它：{reason}", capability.label()),
        None => format!("Runtime 無法確認這個操作在授權範圍內，所以不自動允許：{reason}"),
    };
    if let Some(prior) = &prior_use {
        evidence.push(format!("前一次批准（{}）已由 Run {} 使用，Runtime 無法得知執行結果", prior.request_id, prior.run_id));
        reason_text.push_str("。同一個操作先前已經放行過一次，Runtime 無法確認上一次是否已經生效；請先查證，避免重複執行");
    }
    let (allow_label, deny_label) = match &prior_use {
        Some(_) => ("已查證，允許再執行一次", "拒絕（不要重送）"),
        None => ("允許本次操作", "拒絕"),
    };
    let rejected_approval = match &check {
        Some(ApprovalCheck::Mismatch { request_id, reason }) => {
            Some(RejectedApproval { request_id: request_id.clone(), reason: *reason })
        }
        _ => None,
    };
    let mut record =
        new_request(request_id.clone(), &Requester::Worker(run), task, task, DecisionKind::Permission, question, reason_text, now);
    record.options = vec![
        DecisionOption {
            id: OPTION_ALLOW_ONCE.to_string(),
            label: allow_label.to_string(),
            tradeoff: Some(
                "只允許這個 Task 的這一個操作（同樣的目標、參數與成果版本），用過一次就失效；不會修改 Workspace 授權"
                    .to_string(),
            ),
        },
        DecisionOption {
            id: OPTION_DENY.to_string(),
            label: deny_label.to_string(),
            tradeoff: Some("Worker 會被告知這個操作不被允許，需要改用其他做法".to_string()),
        },
    ];
    // 高風險、無法歸類或結果未知的操作建議拒絕；其餘不給建議，交給使用者判斷
    record.recommendation =
        (capability.is_none_or(Capability::is_high_risk) || prior_use.is_some()).then(|| OPTION_DENY.to_string());
    record.evidence = evidence;
    record.permission = Some(PermissionDetail {
        tool_name: tool_name.to_string(),
        input,
        capability,
        summary,
        fingerprint,
        binding: Some(current),
        prior_use,
        rejected_approval,
    });
    let message = match &check {
        Some(ApprovalCheck::Mismatch { reason, .. }) => {
            format!("先前的單次批准不能沿用（{}），已建立新的裁定請求。{WAITING_MESSAGE}", reason.label())
        }
        Some(ApprovalCheck::UsedResultUnknown { .. }) => format!(
            "這個操作先前已經用掉一次批准，Runtime 無法確認上一次的結果；不要直接重送，先查證上一次是否已經生效。\
             已建立新的裁定請求。{WAITING_MESSAGE}"
        ),
        None => WAITING_MESSAGE.to_string(),
    };
    open_request(ctx, record, task, now, DecisionOutcome::Permission(PermissionOutcome::Waiting { request_id, message, check }))
}

/// `caller` 是提出者的 Task：Worker 是自己的 Task，Controller 沒有自己的 Task，就是目標 Task。
fn controller_action(
    ctx: &DecisionContext<'_>,
    request_id: String,
    audit_id: String,
    requester: Requester<'_>,
    caller: &Task,
    content: ActionContent,
    now: Timestamp,
) -> Result<DecisionChange, DecisionError> {
    let target = match &content.task_id {
        Some(id) => ctx.task.task(id).ok_or_else(|| DecisionError::TaskNotFound(id.clone()))?,
        None => caller,
    };
    let matter = content.matter;
    let authority = ctx.authority.effective(matter);
    let decided = |outcome: ControllerOutcome| DecisionOutcome::Controller { matter, authority, outcome };
    let audit = |event_type: &'static str| {
        event(
            AUTHORITY_AUDIT_AGGREGATE,
            &audit_id,
            1,
            event_type,
            now,
            serde_json::json!({
                "run_id": requester.run_id(),
                "origin": requester.origin(),
                "task_id": target.id,
                "matter": matter,
                "authority": authority,
                "summary": content.summary,
                "policy_revision": ctx.authority.revision,
            }),
        )
    };
    match authority {
        Authority::Autonomous => {
            let mut change = DecisionChange::new(decided(ControllerOutcome::Allowed));
            change.events.push(audit(EVENT_CONTROLLER_ACTION_ALLOWED));
            return Ok(change);
        }
        Authority::HumanOnly => {
            let mut change = DecisionChange::new(decided(ControllerOutcome::HumanOnly));
            change.events.push(audit(EVENT_CONTROLLER_ACTION_REFUSED));
            return Ok(change);
        }
        Authority::RequiresDecision => {}
    }

    let fingerprint = match &content.subject {
        Some(subject) => format!("{}:{}:{subject}", matter.as_str(), target.id),
        None => format!("{}:{}", matter.as_str(), target.id),
    };
    // 提出者也要相同：Worker 與 Controller 的核准不能互相用掉，回答時也只會喚醒原本的提出者（R151 第 8 項）
    let origin = requester.origin();
    let same_operation = |r: &DecisionRequest| {
        r.kind == DecisionKind::Authority
            && r.origin == origin
            && r.authority.as_ref().is_some_and(|a| a.fingerprint == fingerprint)
    };
    if let Some(pending) = ctx.requests.iter().find(|r| same_operation(r) && r.is_pending()) {
        let outcome = ControllerOutcome::RequiresDecision { request_id: pending.id.clone(), existing: true };
        return Ok(DecisionChange::new(decided(outcome)));
    }
    if let Some((answered, decision)) = unused_answer(ctx, same_operation) {
        let outcome = if decision.approves(OPTION_APPROVE) {
            let conditions = (decision.action == DecisionAction::Modify).then(|| decision.note.clone()).flatten();
            ControllerOutcome::Approved { request_id: answered.id.clone(), conditions }
        } else {
            ControllerOutcome::Denied { request_id: answered.id.clone(), note: decision.note.clone() }
        };
        return Ok(consume(
            answered,
            decision,
            requester.run_id(),
            now,
            decided(outcome),
            serde_json::json!({ "matter": matter, "summary": content.summary }),
        ));
    }

    let question = format!("Controller 要「{}」：{}，核准嗎？", matter.label(), content.summary);
    let reason = format!("依決策權限表，「{}」必須由你裁定，Controller 不能自行處理。Controller 的理由：{}", matter.label(), content.reason);
    let mut record = new_request(request_id.clone(), &requester, caller, target, DecisionKind::Authority, question, reason, now);
    // 超出授權（票 17）：裁定只記錄使用者的決定，不會擴大 Workspace 授權；要派出這個 Task 必須另外修改授權
    let approve_tradeoff = if matter == Matter::ExceedAuthorization {
        "核准不會擴大 Workspace 授權：請另外到授權頁加入這個 Task 需要的能力，Controller 才能派出它；用過一次就失效"
    } else {
        "只對這個 Task 的這一次動作有效，用過一次就失效；不會改變決策權限表"
    };
    record.options = vec![
        DecisionOption {
            id: OPTION_APPROVE.to_string(),
            label: "核准這一次".to_string(),
            tradeoff: Some(approve_tradeoff.to_string()),
        },
        DecisionOption {
            id: OPTION_REJECT.to_string(),
            label: "不核准".to_string(),
            tradeoff: Some("Controller 會被告知不能這麼做，需要改用其他做法或先問你".to_string()),
        },
    ];
    record.evidence = content.evidence;
    record.urgency = content.urgency;
    record.authority = Some(AuthorityDetail { matter, summary: content.summary, fingerprint });
    open_request(ctx, record, target, now, decided(ControllerOutcome::RequiresDecision { request_id, existing: false }))
}

/// Controller 向使用者提問（票 17，ADR-0010「邊釐清、邊調查」）：`blocking` 列出依賴這個答案的 Task，
/// 它們在使用者回答之前不能進入 ready 或派工（`task::admission_report` 的 `decision_pending`）；狀態不變。
/// 同一個問題（正規化後相同）仍在等待時回到既有的請求，新列出的 Task 一併加入阻擋範圍。
fn controller_question(
    ctx: &DecisionContext<'_>,
    request_id: String,
    controller_run_id: &str,
    content: RequestContent,
    blocking: Vec<String>,
    now: Timestamp,
) -> Result<DecisionChange, DecisionError> {
    if blocking.is_empty() || blocking.len() > crate::agent::BLOCKING_MAX_TASKS {
        return Err(DecisionError::Invalid(format!(
            "blocking 必須列出 1 到 {} 個依賴這個答案的 Task",
            crate::agent::BLOCKING_MAX_TASKS
        )));
    }
    let mut tasks: Vec<&Task> = Vec::new();
    for id in &blocking {
        let task = ctx.task.task(id).ok_or_else(|| DecisionError::TaskNotFound(id.clone()))?;
        if !matches!(task.status, TaskStatus::Pending | TaskStatus::Ready) {
            return Err(DecisionError::Invalid(format!(
                "Task「{}」（{}）目前是 {}：只有還沒派工的 Task（pending、ready）能標為依賴這個答案",
                task.title,
                task.id,
                task.status.as_str()
            )));
        }
        if !tasks.iter().any(|t| t.id == task.id) {
            tasks.push(task);
        }
    }
    let key = normalize_question(&content.question);
    if let Some(existing) = ctx.requests.iter().find(|r| {
        r.is_pending()
            && r.origin == DecisionOrigin::Controller
            && r.kind == DecisionKind::Agent
            && normalize_question(&r.question) == key
    }) {
        let mut change =
            DecisionChange::new(DecisionOutcome::Requested { request_id: existing.id.clone(), existing: true });
        change.new_blocks = tasks
            .iter()
            .filter(|t| !existing.blocking.contains(&t.id))
            .map(|t| (existing.id.clone(), t.id.clone()))
            .collect();
        return Ok(change);
    }
    let target = tasks[0];
    let requester = Requester::Controller { run_id: controller_run_id, workspace_id: ctx.task.workspace_id };
    let mut record =
        new_request(request_id.clone(), &requester, target, target, DecisionKind::Agent, content.question, content.reason, now);
    record.blocking = tasks.iter().map(|t| t.id.clone()).collect();
    record.options = content.options;
    record.recommendation = content.recommendation;
    record.evidence = content.evidence;
    record.urgency = content.urgency;
    record.deadline_at = content.deadline_minutes.map(|m| now.plus_millis(m * 60_000));
    open_request(ctx, record, target, now, DecisionOutcome::Requested { request_id, existing: false })
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        format!("{}…", text.chars().take(max).collect::<String>())
    }
}

/// 預算用盡（票 36）：Runtime 建立「預算用盡後的下一步」裁定請求（spec D-15 必須送裁定的事項）。
/// 呼叫端已在同一個交易中把 Task 轉為 `blocked`；請求的阻擋範圍是這個 Task，指向用盡預算的 Run。
pub fn budget_exhausted_request(
    request_id: String,
    run: &Run,
    task: &Task,
    kind: BudgetKind,
    evidence: Vec<String>,
    now: Timestamp,
) -> (DecisionRequest, DomainEvent) {
    let matter = Matter::NextStepAfterBudget;
    let label = kind.label();
    let limit = kind.limit();
    let summary = format!("「{}」的{label}預算（{limit} 次）已用完", task.title);
    let mut request = DecisionRequest {
        id: request_id,
        workspace_id: task.workspace_id.clone(),
        run_id: run.id.clone(),
        task_id: task.id.clone(),
        origin: DecisionOrigin::Worker,
        kind: DecisionKind::Authority,
        status: DecisionStatus::Pending,
        question: truncate(&format!("Task「{}」的{label}預算（{limit} 次）已用完，下一步怎麼做？", task.title), QUESTION_MAX_CHARS),
        reason: format!(
            "Runtime 已停止自動重試並把 Task 設為阻擋。依決策權限表，「{}」必須由你裁定（spec D-16、D-15）。",
            matter.label()
        ),
        options: vec![
            DecisionOption {
                id: OPTION_APPROVE.to_string(),
                label: format!("再給一輪{label}預算（{limit} 次），讓 Task 繼續"),
                tradeoff: Some("只重設這一種計數，另外兩種不變；可能再次用盡".to_string()),
            },
            DecisionOption {
                id: OPTION_REJECT.to_string(),
                label: "維持阻擋，由我處理".to_string(),
                tradeoff: Some("Task 留在阻擋，可以修改契約、取消，或之後手動標為可派工".to_string()),
            },
        ],
        recommendation: None,
        evidence: evidence
            .iter()
            .map(|e| e.trim())
            .filter(|e| !e.is_empty())
            .take(EVIDENCE_MAX_ITEMS)
            .map(|e| truncate(e, EVIDENCE_MAX_CHARS))
            .collect(),
        urgency: Urgency::Normal,
        deadline_at: None,
        permission: None,
        authority: Some(AuthorityDetail {
            matter,
            fingerprint: format!("{}:{}", matter.as_str(), task.id),
            summary: truncate(&summary, SUMMARY_MAX_CHARS),
        }),
        external_operation_id: None,
        blocking: vec![task.id.clone()],
        target: DecisionTarget { task_id: task.id.clone(), contract_version: task.contract_version, authorization_revision: None },
        revision: 1,
        event_version: 0,
        deferred_until: None,
        merged_into: None,
        status_reason: None,
        links: Vec::new(),
        created_at: now,
        updated_at: now,
    };
    let requested = request.event(
        EVENT_DECISION_REQUESTED,
        now,
        serde_json::json!({
            "kind": request.kind,
            "run_id": request.run_id,
            "task_id": request.task_id,
            "question": request.question,
            "blocking": request.blocking,
            "target": request.target,
            "budget": kind,
            "linked": [],
        }),
    );
    (request, requested)
}

/// 核准了用盡預算時建立的請求（票 36）：重設用盡的那一種計數，阻擋中的 Task 繼續——驗證不完整回到 `verifying` 重驗，
/// 其他回到 `ready` 並排回派工佇列。核准在這裡就用掉（記為已使用），Controller 之後不能再拿它當單次核准。
/// 不是目前用盡的那個請求（例如之後又用盡而建立了新請求）時只記下裁定，什麼都不改。
fn continue_after_budget(
    ctx: &DecisionContext<'_>,
    request: &DecisionRequest,
    change: &mut DecisionChange,
    now: Timestamp,
) -> Result<Vec<String>, DecisionError> {
    let Some(task) = ctx.task.task(&request.target.task_id) else { return Ok(Vec::new()) };
    let Some((budget, kind, event)) = budget::reset(&TaskBudget::of(ctx.budgets, task), &request.id, now) else {
        return Ok(Vec::new());
    };
    change.events.push(event);
    change.budgets.push(budget);
    change.approval_use = Some(ApprovalUse { request_id: request.id.clone(), run_id: request.run_id.clone(), used_at: now });
    if task.status != TaskStatus::Blocked {
        return Ok(Vec::new());
    }
    let to = if kind == BudgetKind::Verification { TaskStatus::Verifying } else { TaskStatus::Ready };
    let mut moved = task.clone();
    let mut versioned = Versioned { task: &mut moved, events: Vec::new(), now };
    versioned.change_status(to, budget::EVENT_BUDGET_RESET).map_err(|_| DecisionError::TaskNotFound(task.id.clone()))?;
    change.events.extend(versioned.events);
    change.tasks.push(moved);
    if to != TaskStatus::Ready {
        return Ok(Vec::new());
    }
    change.resumed.push(task.id.clone());
    Ok(vec![task.id.clone()])
}

/// 請求結束（回答或過期）後，阻擋範圍中可以恢復的 Task：必須還在等裁定，而且沒有別的待處理請求也阻擋它。
/// `closing` 是這次一併結束的請求（重新整理時可能一次結束好幾個）。
fn resume_tasks(
    ctx: &DecisionContext<'_>,
    closing: &[&DecisionRequest],
    change: &mut DecisionChange,
    reason: &'static str,
    now: Timestamp,
) -> Result<Vec<String>, DecisionError> {
    let closing_ids: Vec<&str> = closing.iter().map(|r| r.id.as_str()).collect();
    let mut resumed = Vec::new();
    for task_id in closing.iter().flat_map(|r| r.blocking.iter()) {
        if resumed.contains(task_id) {
            continue;
        }
        let still_blocked = ctx
            .requests
            .iter()
            .any(|other| !closing_ids.contains(&other.id.as_str()) && other.is_pending() && other.blocking.contains(task_id));
        let Some(task) = ctx.task.task(task_id).filter(|t| t.status == TaskStatus::WaitingDecision && !still_blocked) else {
            continue;
        };
        let mut moved = task.clone();
        let mut versioned = Versioned { task: &mut moved, events: Vec::new(), now };
        versioned.change_status(TaskStatus::Ready, reason).map_err(|_| DecisionError::TaskNotFound(task.id.clone()))?;
        change.events.extend(versioned.events);
        change.tasks.push(moved);
        resumed.push(task.id.clone());
    }
    change.resumed.extend(resumed.iter().cloned());
    Ok(resumed)
}

/// 喚醒提出者：請求本身與被合併進來的請求都要叫醒。Worker 提出的寫喚醒 Outbox（各自的 Run）；
/// Controller 提出的（票 17）不寫 Outbox，被回答時由 Controller CLI 的事件等待（`wait`）回報。
fn wake(ctx: &DecisionContext<'_>, request: &DecisionRequest, payload: &Value, change: &mut DecisionChange, now: Timestamp) {
    let mut wake_one = |target: &DecisionRequest, payload: Value| {
        if target.origin == DecisionOrigin::Worker {
            change.outbox.push(WakeItem { kind: wake_kind(&target.id), run_id: target.run_id.clone(), payload, created_at: now });
        }
    };
    wake_one(request, payload.clone());
    for source in ctx.requests.iter().filter(|r| r.merged_into.as_deref() == Some(request.id.as_str())) {
        let mut payload = payload.clone();
        payload["request_id"] = Value::String(source.id.clone());
        payload["merged_into"] = Value::String(request.id.clone());
        wake_one(source, payload);
    }
}

/// 把一批待處理請求標為 `superseded`，恢復只被它們阻擋的 Task，並叫醒提出者。
fn supersede(
    ctx: &DecisionContext<'_>,
    stale: Vec<(&DecisionRequest, String)>,
    change: &mut DecisionChange,
    now: Timestamp,
) -> Result<Vec<String>, DecisionError> {
    let closing: Vec<&DecisionRequest> = stale.iter().map(|(r, _)| *r).collect();
    let resumed = resume_tasks(ctx, &closing, change, EVENT_DECISION_SUPERSEDED, now)?;
    mark_superseded(ctx, stale, &resumed, change, now);
    Ok(resumed)
}

/// 標為 `superseded` 並叫醒提出者（Task 的恢復由呼叫端處理）。
fn mark_superseded(
    ctx: &DecisionContext<'_>,
    stale: Vec<(&DecisionRequest, String)>,
    resumed: &[String],
    change: &mut DecisionChange,
    now: Timestamp,
) {
    for (request, reason) in stale {
        let mut updated = request.clone();
        updated.status = DecisionStatus::Superseded;
        updated.status_reason = Some(reason.clone());
        updated.bump(now);
        let superseded = updated.event(EVENT_DECISION_SUPERSEDED, now, serde_json::json!({ "reason": reason, "revision": updated.revision }));
        change.events.push(superseded);
        let payload = serde_json::json!({
            "workspace_id": request.workspace_id,
            "request_id": request.id,
            "superseded": true,
            "reason": reason,
            "task_ids": request.blocking,
            "resumed_task_ids": resumed.iter().filter(|t| request.blocking.contains(t)).collect::<Vec<_>>(),
        });
        wake(ctx, request, &payload, change, now);
        change.updated.push(updated);
    }
}

/// 使用者的裁定是否推翻了請求的建議（假設）：只有「接受」建議的那個選項才是確認；沒有建議時任何裁定都改變了前提（票 50）。
pub fn overturns_assumption(recommendation: Option<&str>, action: DecisionAction, option_id: Option<&str>) -> bool {
    !(action == DecisionAction::Accept && recommendation.is_some() && option_id == recommendation)
}

/// 推翻假設的說明（給受影響的 Task 與請求看）；裁定確認了建議時為 `None`。
pub fn overturn_reason(request: &DecisionRequest, decision: &Decision) -> Option<String> {
    describe_overturn(
        &request.question,
        &request.options,
        request.recommendation.as_deref(),
        decision.action,
        decision.option_id.as_deref(),
    )
}

/// 同 [`overturn_reason`]，以請求與裁定的欄位計算（Store 載入假設狀態時用）。
pub fn describe_overturn(
    question: &str,
    options: &[DecisionOption],
    recommendation: Option<&str>,
    action: DecisionAction,
    option_id: Option<&str>,
) -> Option<String> {
    if !overturns_assumption(recommendation, action, option_id) {
        return None;
    }
    let label = |id: &str| options.iter().find(|o| o.id == id).map_or(id.to_string(), |o| o.label.clone());
    let chosen = match option_id {
        Some(option) => format!("{}「{}」", action.label(), label(option)),
        None => action.label().to_string(),
    };
    Some(match recommendation {
        Some(recommended) => format!("使用者對「{question}」的裁定是{chosen}，不是建議的「{}」", label(recommended)),
        None => format!("使用者對「{question}」的裁定是{chosen}"),
    })
}

fn refresh(ctx: &DecisionContext<'_>, now: Timestamp) -> Result<DecisionChange, DecisionError> {
    let stale: Vec<(&DecisionRequest, String)> =
        ctx.requests.iter().filter(|r| r.is_pending()).filter_map(|r| ctx.stale_reason(r).map(|why| (r, why))).collect();
    let ids = stale.iter().map(|(r, _)| r.id.clone()).collect();
    let mut change = DecisionChange::new(DecisionOutcome::Refreshed { superseded: ids });
    supersede(ctx, stale, &mut change, now)?;
    Ok(change)
}

fn answer(
    ctx: &DecisionContext<'_>,
    request_id: &str,
    expected_revision: i64,
    resolution: Resolution,
    decided_by: &str,
    batch_id: Option<String>,
    now: Timestamp,
) -> Result<DecisionChange, DecisionError> {
    if decided_by != DECIDED_BY_DESKTOP {
        return Err(DecisionError::NotDesktop);
    }
    let request = ctx.request(request_id)?;
    let action = resolution.action;
    let option_id = resolution.option_id.as_deref().map(str::trim).filter(|o| !o.is_empty()).map(str::to_string);
    match request.status {
        DecisionStatus::Pending => {}
        DecisionStatus::Answered => {
            // 同樣的處理重送是冪等的；不同的處理不能改寫既有的裁定
            let existing = ctx.decision_of(&request.id).ok_or_else(|| DecisionError::NotFound(request.id.clone()))?;
            return if existing.action == action && existing.option_id == option_id {
                Ok(DecisionChange::new(DecisionOutcome::Answered { resumed_task_ids: Vec::new() }))
            } else {
                let what = match &existing.option_id {
                    Some(option) => format!("{}「{option}」", existing.action.label()),
                    None => existing.action.label().to_string(),
                };
                Err(DecisionError::AlreadyAnswered(what))
            };
        }
        DecisionStatus::Superseded => {
            return Err(DecisionError::Superseded(request.status_reason.clone().unwrap_or_default()));
        }
        DecisionStatus::Merged => return Err(DecisionError::Merged(request.merged_into.clone().unwrap_or_default())),
    }
    // 目標版本已變更：標為過期（這個標記要寫入），不能套用
    if let Some(reason) = ctx.stale_reason(request) {
        let mut change = DecisionChange::new(DecisionOutcome::Answered { resumed_task_ids: Vec::new() });
        let resumed = supersede(ctx, vec![(request, reason.clone())], &mut change, now)?;
        change.outcome = DecisionOutcome::Superseded { request_id: request.id.clone(), reason, resumed_task_ids: resumed };
        return Ok(change);
    }
    if expected_revision != request.revision {
        return Err(DecisionError::RevisionMismatch { current: request.revision });
    }
    if !request.allowed_actions().contains(&action) {
        return Err(DecisionError::ActionNotAllowed(action.label()));
    }
    let option_id = if action.needs_option() {
        let id = option_id.ok_or_else(|| DecisionError::Invalid(format!("「{}」必須選一個選項", action.label())))?;
        if request.option(&id).is_none() {
            return Err(DecisionError::OptionUnknown(id));
        }
        Some(id)
    } else {
        None
    };
    let note = match resolution.note.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) if n.chars().count() > NOTE_MAX_CHARS => {
            return Err(DecisionError::Invalid(format!("文字不可超過 {NOTE_MAX_CHARS} 個字元")))
        }
        Some(n) => Some(n.to_string()),
        None if action.needs_note() => {
            let what = match action {
                DecisionAction::Modify => "修改後的條件",
                DecisionAction::Answer => "回答",
                _ => "要調查的事",
            };
            return Err(DecisionError::Invalid(format!("「{}」必須寫下{what}", action.label())));
        }
        None => None,
    };

    if action == DecisionAction::Defer {
        let minutes = resolution.defer_minutes.unwrap_or(DEFER_DEFAULT_MINUTES);
        if !(1..=DEADLINE_MAX_MINUTES).contains(&minutes) {
            return Err(DecisionError::Invalid(format!("延後時間必須是 1 到 {DEADLINE_MAX_MINUTES} 分鐘")));
        }
        let mut updated = request.clone();
        updated.deferred_until = Some(now.plus_millis(minutes * 60_000));
        updated.bump(now);
        let deferred = updated.event(
            EVENT_DECISION_DEFERRED,
            now,
            serde_json::json!({ "deferred_until": updated.deferred_until, "note": note, "revision": updated.revision }),
        );
        let mut change = DecisionChange::new(DecisionOutcome::Deferred);
        change.events.push(deferred);
        change.updated.push(updated);
        return Ok(change);
    }

    let mut change = DecisionChange::new(DecisionOutcome::Answered { resumed_task_ids: Vec::new() });
    let mut updated = request.clone();
    updated.status = DecisionStatus::Answered;
    updated.bump(now);
    change.decision = Some(Decision {
        request_id: request.id.clone(),
        action,
        option_id: option_id.clone(),
        note: note.clone(),
        decided_by: decided_by.to_string(),
        decided_at: now,
        batch_id: batch_id.clone(),
    });
    // 答案推翻了建議（假設）：依據它的 Task 尚未結束的請求一起過期（票 50）
    let decision = change.decision.as_ref().expect("剛剛建立");
    let overturned = overturn_reason(request, decision);
    let built_on: Vec<&str> = match &overturned {
        Some(_) => ctx
            .task
            .contracts
            .iter()
            .filter(|c| c.content.relies_on_decision(&request.id))
            .filter(|c| ctx.task.task(&c.task_id).is_some_and(|t| c.version == t.contract_version))
            .map(|c| c.task_id.as_str())
            .collect(),
        None => Vec::new(),
    };
    let dependents: Vec<(&DecisionRequest, String)> = ctx
        .requests
        .iter()
        .filter(|r| r.id != request.id && r.is_pending() && r.external_operation_id.is_none())
        .filter(|r| std::iter::once(&r.target.task_id).chain(&r.blocking).any(|t| built_on.contains(&t.as_str())))
        .map(|r| (r, format!("依據的假設已被推翻：{}（裁定請求 {}）", overturned.clone().unwrap_or_default(), request.id)))
        .collect();
    let mut closing = vec![request];
    closing.extend(dependents.iter().map(|(r, _)| *r));
    let mut resumed = resume_tasks(ctx, &closing, &mut change, EVENT_DECISION_ANSWERED, now)?;
    mark_superseded(ctx, dependents, &resumed, &mut change, now);
    if action.needs_option() && option_id.as_deref() == Some(OPTION_APPROVE) {
        resumed.extend(continue_after_budget(ctx, request, &mut change, now)?);
    }
    let answered = updated.event(
        EVENT_DECISION_ANSWERED,
        now,
        serde_json::json!({
            "action": action,
            "option_id": option_id,
            "decided_by": decided_by,
            "note": note,
            "batch_id": batch_id,
            "revision": updated.revision,
            "resumed_task_ids": resumed,
        }),
    );
    change.events.push(answered);
    let payload = serde_json::json!({
        "workspace_id": request.workspace_id,
        "request_id": request.id,
        "action": action,
        "option_id": option_id,
        "note": note,
        "task_ids": request.blocking,
        "resumed_task_ids": resumed,
    });
    wake(ctx, request, &payload, &mut change, now);
    change.updated.push(updated);
    change.outcome = DecisionOutcome::Answered { resumed_task_ids: resumed };
    Ok(change)
}

fn merge(
    ctx: &DecisionContext<'_>,
    request_id: &str,
    expected_revision: i64,
    into: &str,
    into_expected_revision: i64,
    now: Timestamp,
) -> Result<DecisionChange, DecisionError> {
    let source = ctx.request(request_id)?;
    let target = ctx.request(into)?;
    let invalid = |why: &str| Err(DecisionError::MergeInvalid(why.to_string()));
    if source.id == target.id {
        return invalid("不能把裁定請求合併到它自己");
    }
    if !source.is_pending() || !target.is_pending() {
        return invalid("只能合併兩個都還在等待裁定的請求");
    }
    if source.kind != target.kind {
        return invalid("只能合併同一種類的裁定請求");
    }
    // Controller 的提問以「待處理」阻擋 Task 的准入（票 17），Worker 的請求以 waiting_decision 阻擋：兩者不能互相合併
    if source.origin != target.origin {
        return invalid("Controller 與 Worker 提出的裁定請求不能合併；可以改用連結");
    }
    if source.kind == DecisionKind::Permission {
        return invalid("權限提示的單次批准各自綁定 Task 與操作，不能合併；可以改用連結");
    }
    if source.external_operation_id.is_some() || target.external_operation_id.is_some() {
        return invalid("外部操作的查證請求各自對應一個外部操作，不能合併；可以改用連結");
    }
    if source.revision != expected_revision {
        return Err(DecisionError::RevisionMismatch { current: source.revision });
    }
    if target.revision != into_expected_revision {
        return Err(DecisionError::RevisionMismatch { current: target.revision });
    }
    let mut change = DecisionChange::new(DecisionOutcome::Merged);
    let mut merged = source.clone();
    merged.status = DecisionStatus::Merged;
    merged.merged_into = Some(target.id.clone());
    merged.bump(now);
    let merged_event = merged.event(EVENT_DECISION_MERGED, now, serde_json::json!({ "into": target.id, "revision": merged.revision }));
    change.events.push(merged_event);

    let mut receiving = target.clone();
    let added: Vec<String> = source.blocking.iter().filter(|t| !target.blocking.contains(t)).cloned().collect();
    receiving.blocking.extend(added.iter().cloned());
    receiving.bump(now);
    let received = receiving.event(
        EVENT_DECISION_MERGE_RECEIVED,
        now,
        serde_json::json!({ "from": source.id, "added_blocking": added, "revision": receiving.revision }),
    );
    change.events.push(received);
    change.new_blocks = added.into_iter().map(|t| (target.id.clone(), t)).collect();
    if !source.links.contains(&target.id) {
        change.new_links.push(link_pair(&source.id, &target.id));
    }
    change.updated.push(merged);
    change.updated.push(receiving);
    Ok(change)
}

fn link(ctx: &DecisionContext<'_>, request_id: &str, other_id: &str, now: Timestamp) -> Result<DecisionChange, DecisionError> {
    let request = ctx.request(request_id)?;
    let other = ctx.request(other_id)?;
    if request.id == other.id {
        return Err(DecisionError::Invalid("不能把裁定請求連結到它自己".to_string()));
    }
    let mut change = DecisionChange::new(DecisionOutcome::Linked);
    if request.links.contains(&other.id) {
        return Ok(change);
    }
    change.new_links.push(link_pair(&request.id, &other.id));
    change.events.push(event(
        DECISION_AGGREGATE,
        &request.id,
        request.event_version + 1,
        EVENT_DECISION_LINKED,
        now,
        serde_json::json!({ "other_id": other.id }),
    ));
    Ok(change)
}

// ---------------------------------------------------------------- 投影（API／面板）

/// 阻擋範圍中的一個 Task。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockedTask {
    pub task_id: String,
    pub task_title: String,
    pub task_status: Option<TaskStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionResult {
    pub action: DecisionAction,
    pub option_id: Option<String>,
    pub option_label: Option<String>,
    pub note: Option<String>,
    pub decided_by: String,
    pub decided_at: Timestamp,
    pub batch_id: Option<String>,
}

/// 連結或合併來的另一個請求（只列摘要）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelatedRequest {
    pub id: String,
    pub question: String,
    pub status: DecisionStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorityView {
    pub matter: Matter,
    pub matter_label: String,
    pub summary: String,
}

/// 裁定請求的完整投影：問題、原因、選項、建議、證據、阻擋範圍、目標版本與（已回答時的）裁定。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionView {
    pub id: String,
    pub workspace_id: String,
    pub run_id: String,
    pub task_id: String,
    /// 提出者是 Worker 還是 Controller（票 17）。
    #[serde(default)]
    pub origin: DecisionOrigin,
    pub task_title: String,
    pub kind: DecisionKind,
    pub status: DecisionStatus,
    pub revision: i64,
    pub question: String,
    pub reason: String,
    pub options: Vec<DecisionOption>,
    pub recommendation: Option<String>,
    pub evidence: Vec<String>,
    pub urgency: Urgency,
    pub deadline_at: Option<Timestamp>,
    /// 已逾期仍未回答；只是標示，不代表同意，也不會改變任何狀態。
    pub overdue: bool,
    pub deferred_until: Option<Timestamp>,
    /// 目前仍在延後期間；只影響顯示順序，工作仍在等待。
    pub deferred: bool,
    pub merged_into: Option<String>,
    pub status_reason: Option<String>,
    pub blocking: Vec<BlockedTask>,
    pub target: DecisionTarget,
    pub links: Vec<RelatedRequest>,
    pub merged_from: Vec<RelatedRequest>,
    pub permission: Option<PermissionView>,
    pub authority: Option<AuthorityView>,
    /// 目前可以用的處理方式；已結束的請求為空。
    pub allowed_actions: Vec<DecisionAction>,
    pub created_at: Timestamp,
    pub decision: Option<DecisionResult>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionView {
    pub tool_name: String,
    pub input: Value,
    pub capability: Option<Capability>,
    pub capability_label: Option<String>,
    pub summary: String,
    /// 「允許本次操作」綁定的邏輯操作；票 23 之前的請求沒有。
    pub binding: Option<ApprovalBinding>,
    /// 同一個操作先前已用掉一次批准、結果未知（要先查證）。
    pub prior_use: Option<PriorUse>,
    /// 先前不能沿用的單次批准與原因。
    pub rejected_approval: Option<RejectedApproval>,
    /// 這個請求的裁定已被某個 Run 使用（單次批准用過即失效）。
    pub approval_use: Option<ApprovalUse>,
}

pub fn view(
    request: &DecisionRequest,
    requests: &[DecisionRequest],
    decisions: &[Decision],
    uses: &[ApprovalUse],
    tasks: &[Task],
    now: Timestamp,
) -> DecisionView {
    let title_of = |id: &str| tasks.iter().find(|t| t.id == id).map_or_else(|| id.to_string(), |t| t.title.clone());
    let related = |r: &DecisionRequest| RelatedRequest { id: r.id.clone(), question: r.question.clone(), status: r.status };
    let decision = decisions.iter().find(|d| d.request_id == request.id);
    DecisionView {
        id: request.id.clone(),
        workspace_id: request.workspace_id.clone(),
        run_id: request.run_id.clone(),
        origin: request.origin,
        task_id: request.task_id.clone(),
        task_title: title_of(&request.task_id),
        kind: request.kind,
        status: request.status,
        revision: request.revision,
        question: request.question.clone(),
        reason: request.reason.clone(),
        options: request.options.clone(),
        recommendation: request.recommendation.clone(),
        evidence: request.evidence.clone(),
        urgency: request.urgency,
        deadline_at: request.deadline_at,
        overdue: request.is_overdue(now),
        deferred_until: request.deferred_until,
        deferred: request.is_deferred(now),
        merged_into: request.merged_into.clone(),
        status_reason: request.status_reason.clone(),
        blocking: request
            .blocking
            .iter()
            .map(|id| BlockedTask {
                task_id: id.clone(),
                task_title: title_of(id),
                task_status: tasks.iter().find(|t| &t.id == id).map(|t| t.status),
            })
            .collect(),
        target: request.target.clone(),
        links: request.links.iter().filter_map(|id| requests.iter().find(|r| &r.id == id)).map(related).collect(),
        merged_from: requests.iter().filter(|r| r.merged_into.as_deref() == Some(request.id.as_str())).map(related).collect(),
        permission: request.permission.as_ref().map(|p| PermissionView {
            tool_name: p.tool_name.clone(),
            input: p.input.clone(),
            capability: p.capability,
            capability_label: p.capability.map(|c| c.label().to_string()),
            summary: p.summary.clone(),
            binding: p.binding.clone(),
            prior_use: p.prior_use.clone(),
            rejected_approval: p.rejected_approval.clone(),
            approval_use: uses.iter().find(|u| u.request_id == request.id).cloned(),
        }),
        authority: request.authority.as_ref().map(|a| AuthorityView {
            matter: a.matter,
            matter_label: a.matter.label().to_string(),
            summary: a.summary.clone(),
        }),
        allowed_actions: if request.is_pending() { request.allowed_actions().to_vec() } else { Vec::new() },
        created_at: request.created_at,
        decision: decision.map(|d| DecisionResult {
            action: d.action,
            option_id: d.option_id.clone(),
            option_label: d
                .option_id
                .as_deref()
                .map(|id| request.option(id).map_or_else(|| id.to_string(), |o| o.label.clone())),
            note: d.note.clone(),
            decided_by: d.decided_by.clone(),
            decided_at: d.decided_at,
            batch_id: d.batch_id.clone(),
        }),
    }
}

#[cfg(test)]
mod tests;
