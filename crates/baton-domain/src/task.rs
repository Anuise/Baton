//! Goal 與 Task：Task 狀態機、六項最小派工契約（DR-20261022-03）、相依與准入檢查。
//!
//! 純邏輯、無 I/O。所有 Goal／Task 的變更都只能經過 `decide`：輸入「目前狀態（`TaskContext`）+ 命令 + 時間」，
//! 輸出「新狀態 + 事件」，由 Store 在一個交易中套用。
//!
//! 契約欄位齊全只代表結構完整，不代表語意正確，更不代表成果已通過驗證（ADR-0011）：
//! 欄位未提供、標為「未知」、引用失效、授權不符都不能進入 `ready`。

use serde::{Deserialize, Serialize};

use crate::authorization::{self, AdmissionDenied, AuthorizationRevision, Capability};
use crate::event::DomainEvent;
use crate::verification::VerificationCheck;
use crate::workdir::TaskKind;
use crate::workspace::{Workspace, WorkspaceStatus};
use crate::Timestamp;

pub const GOAL_AGGREGATE: &str = "goal";
pub const TASK_AGGREGATE: &str = "task";
pub const EVENT_GOAL_CREATED: &str = "goal_created";
pub const EVENT_TASK_CREATED: &str = "task_created";
pub const EVENT_CONTRACT_CHANGED: &str = "task_contract_changed";
pub const EVENT_DEPENDENCY_ADDED: &str = "task_dependency_added";
pub const EVENT_STATUS_CHANGED: &str = "task_status_changed";

/// Goal 與 Task 標題的長度上限（字元數）。
pub const MAX_TITLE_CHARS: usize = 200;

// ---------------------------------------------------------------- 狀態機

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    Ready,
    Running,
    WaitingDecision,
    WaitingResource,
    Blocked,
    Verifying,
    Completed,
    Cancelled,
}

impl TaskStatus {
    pub const ALL: [TaskStatus; 9] = [
        TaskStatus::Pending,
        TaskStatus::Ready,
        TaskStatus::Running,
        TaskStatus::WaitingDecision,
        TaskStatus::WaitingResource,
        TaskStatus::Blocked,
        TaskStatus::Verifying,
        TaskStatus::Completed,
        TaskStatus::Cancelled,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            TaskStatus::Pending => "pending",
            TaskStatus::Ready => "ready",
            TaskStatus::Running => "running",
            TaskStatus::WaitingDecision => "waiting_decision",
            TaskStatus::WaitingResource => "waiting_resource",
            TaskStatus::Blocked => "blocked",
            TaskStatus::Verifying => "verifying",
            TaskStatus::Completed => "completed",
            TaskStatus::Cancelled => "cancelled",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.as_str() == raw)
    }

    /// 已結束的狀態：不能再有任何轉換，也不能再修改契約或相依。
    pub fn is_terminal(self) -> bool {
        matches!(self, TaskStatus::Completed | TaskStatus::Cancelled)
    }

    /// 這個狀態可以直接轉到哪些狀態。目前只有 `ready`（准入檢查）與 `cancelled` 有對外的命令；
    /// 其餘轉換由 Run、裁定與驗證等後續的票透過 Domain 加入命令，但規則已集中在這張表。
    pub fn allowed_next(self) -> &'static [TaskStatus] {
        use TaskStatus::*;
        match self {
            Pending => &[Ready, Blocked, Cancelled],
            // ready → waiting_decision：Run 結束時留下結果未知的外部操作（票 26）
            Ready => &[Pending, Running, WaitingDecision, Blocked, Cancelled],
            Running => &[Ready, WaitingDecision, WaitingResource, Blocked, Verifying, Cancelled],
            WaitingDecision => &[Ready, Blocked, Cancelled],
            WaitingResource => &[Ready, Blocked, Cancelled],
            // 驗證不完整的預算用盡後，使用者核准再重驗時回到 verifying（票 36）
            Blocked => &[Pending, Ready, Verifying, Cancelled],
            Verifying => &[Ready, WaitingDecision, WaitingResource, Blocked, Completed, Cancelled],
            Completed | Cancelled => &[],
        }
    }

    pub fn can_transition_to(self, next: TaskStatus) -> bool {
        self.allowed_next().contains(&next)
    }
}

// ---------------------------------------------------------------- 六項最小契約

/// 六項最小契約的欄位；宣告順序就是顯示與檢查的標準順序。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContractFieldName {
    /// 目標
    Objective,
    /// 輸入與依據
    Inputs,
    /// 範圍與限制
    Scope,
    /// 預期成果
    Deliverables,
    /// 完成與驗證條件
    DoneCriteria,
    /// 阻礙處理
    BlockerHandling,
}

impl ContractFieldName {
    pub const ALL: [ContractFieldName; 6] = [
        ContractFieldName::Objective,
        ContractFieldName::Inputs,
        ContractFieldName::Scope,
        ContractFieldName::Deliverables,
        ContractFieldName::DoneCriteria,
        ContractFieldName::BlockerHandling,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ContractFieldName::Objective => "objective",
            ContractFieldName::Inputs => "inputs",
            ContractFieldName::Scope => "scope",
            ContractFieldName::Deliverables => "deliverables",
            ContractFieldName::DoneCriteria => "done_criteria",
            ContractFieldName::BlockerHandling => "blocker_handling",
        }
    }

    /// 給使用者看的名稱。
    pub fn label(self) -> &'static str {
        match self {
            ContractFieldName::Objective => "目標",
            ContractFieldName::Inputs => "輸入與依據",
            ContractFieldName::Scope => "範圍與限制",
            ContractFieldName::Deliverables => "預期成果",
            ContractFieldName::DoneCriteria => "完成與驗證條件",
            ContractFieldName::BlockerHandling => "阻礙處理",
        }
    }

    /// 目標與完成條件一定適用：Worker 不能在沒有目標或沒有完成標準的情況下被委託。
    pub fn allows_not_applicable(self) -> bool {
        !matches!(self, ContractFieldName::Objective | ContractFieldName::DoneCriteria)
    }
}

/// 引用的對象種類。Runtime 要能解析它，所以只有目前已存在的實體；
/// 成果、Stage、決策等實體由後續的票加入新的種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceKind {
    /// 同一個 Workspace 內另一個 Task 的某個契約版本。
    TaskContract,
    /// 同一個 Workspace 內的 Goal。
    Goal,
    /// 這個 Workspace 的授權 revision（`target` 是 Workspace 識別）。
    Authorization,
    /// 同一個 Workspace 內的裁定請求（`version` 是引用時請求的內容版本）：Task 依它的建議（假設）或答案進行（票 50）。
    /// 使用者的裁定與建議不同時，假設被推翻，這個引用就失效。
    Decision,
}

impl ReferenceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ReferenceKind::TaskContract => "task_contract",
            ReferenceKind::Goal => "goal",
            ReferenceKind::Authorization => "authorization",
            ReferenceKind::Decision => "decision",
        }
    }
}

/// 可解析的引用與適用版本（不重複抄寫被引用的內容）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reference {
    pub kind: ReferenceKind,
    pub target: String,
    pub version: i64,
}

/// 一個契約欄位的內容。欄位根本沒有提供（`None`）是第四種狀態「未提供」。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ContractField {
    Filled {
        text: String,
        #[serde(default)]
        references: Vec<Reference>,
    },
    /// 確實不適用：必須附理由。
    NotApplicable { reason: String },
    /// 明確標示為未知；不能以空值或假設冒充已確認，所以不算完整。
    Unknown {
        #[serde(default)]
        note: Option<String>,
    },
}

/// 六項最小契約的內容，外加這個 Task 需要的授權能力。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContractContent {
    #[serde(default)]
    pub objective: Option<ContractField>,
    #[serde(default)]
    pub inputs: Option<ContractField>,
    #[serde(default)]
    pub scope: Option<ContractField>,
    #[serde(default)]
    pub deliverables: Option<ContractField>,
    #[serde(default)]
    pub done_criteria: Option<ContractField>,
    #[serde(default)]
    pub blocker_handling: Option<ContractField>,
    /// 准入檢查時與 Workspace 目前生效的授權比對。
    #[serde(default)]
    pub required_capabilities: Vec<Capability>,
    /// 「完成與驗證條件」中可由 Runtime 機械判定的部分（票 13）；沒有宣告時只能由使用者手動接受。
    #[serde(default)]
    pub verification_checks: Vec<VerificationCheck>,
}

impl ContractContent {
    /// 已填寫欄位中的全部引用。
    pub fn references(&self) -> impl Iterator<Item = &Reference> {
        [&self.objective, &self.inputs, &self.scope, &self.deliverables, &self.done_criteria, &self.blocker_handling]
            .into_iter()
            .filter_map(|field| match field {
                Some(ContractField::Filled { references, .. }) => Some(references.iter()),
                _ => None,
            })
            .flatten()
    }

    /// 是否以 `decision` 引用了這個裁定請求（依它的建議或答案進行，票 50）。
    pub fn relies_on_decision(&self, request_id: &str) -> bool {
        self.references().any(|r| r.kind == ReferenceKind::Decision && r.target == request_id)
    }
}

impl ContractContent {
    pub fn field(&self, name: ContractFieldName) -> Option<&ContractField> {
        match name {
            ContractFieldName::Objective => self.objective.as_ref(),
            ContractFieldName::Inputs => self.inputs.as_ref(),
            ContractFieldName::Scope => self.scope.as_ref(),
            ContractFieldName::Deliverables => self.deliverables.as_ref(),
            ContractFieldName::DoneCriteria => self.done_criteria.as_ref(),
            ContractFieldName::BlockerHandling => self.blocker_handling.as_ref(),
        }
    }

    fn field_mut(&mut self, name: ContractFieldName) -> &mut Option<ContractField> {
        match name {
            ContractFieldName::Objective => &mut self.objective,
            ContractFieldName::Inputs => &mut self.inputs,
            ContractFieldName::Scope => &mut self.scope,
            ContractFieldName::Deliverables => &mut self.deliverables,
            ContractFieldName::DoneCriteria => &mut self.done_criteria,
            ContractFieldName::BlockerHandling => &mut self.blocker_handling,
        }
    }

    /// 整理並驗證使用者送來的內容：去掉前後空白、拒絕空白文字、缺理由的「不適用」與不允許「不適用」的欄位，
    /// 能力去重並依標準順序排列（讓「內容是否相同」可以直接比較）。
    pub fn normalized(mut self) -> Result<Self, TaskError> {
        for name in ContractFieldName::ALL {
            let invalid = |why: &str| TaskError::ContractInvalid(format!("「{}」{why}", name.label()));
            let slot = self.field_mut(name);
            let Some(field) = slot.take() else { continue };
            *slot = Some(match field {
                ContractField::Filled { text, references } => {
                    let text = text.trim().to_string();
                    if text.is_empty() {
                        return Err(invalid("填寫的內容是空白；沒有內容請不要提供這個欄位，或標為「未知」"));
                    }
                    let mut clean = Vec::with_capacity(references.len());
                    for reference in references {
                        let target = reference.target.trim().to_string();
                        if target.is_empty() || reference.version < 1 {
                            return Err(invalid("的引用必須有目標識別與大於 0 的版本"));
                        }
                        clean.push(Reference { target, ..reference });
                    }
                    ContractField::Filled { text, references: clean }
                }
                ContractField::NotApplicable { reason } => {
                    if !name.allows_not_applicable() {
                        return Err(invalid("一定適用，不能標為「不適用」"));
                    }
                    let reason = reason.trim().to_string();
                    if reason.is_empty() {
                        return Err(invalid("標為「不適用」時必須附理由"));
                    }
                    ContractField::NotApplicable { reason }
                }
                ContractField::Unknown { note } => ContractField::Unknown {
                    note: note.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()),
                },
            });
        }
        self.required_capabilities.sort();
        self.required_capabilities.dedup();
        self.verification_checks = crate::verification::normalize_checks(std::mem::take(&mut self.verification_checks))
            .map_err(|why| TaskError::ContractInvalid(format!("「{}」的確定性檢查：{why}", ContractFieldName::DoneCriteria.label())))?;
        Ok(self)
    }
}

/// 契約的一個不可變版本；Task 目前生效的契約是 `Task::contract_version` 指到的那一個。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskContract {
    pub task_id: String,
    /// 從 1 起連續遞增。
    pub version: i64,
    #[serde(flatten)]
    pub content: ContractContent,
    /// 這次變更的說明（建立時為 `None`）。
    pub note: Option<String>,
    pub created_at: Timestamp,
}

// ---------------------------------------------------------------- 實體

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Goal {
    pub id: String,
    pub workspace_id: String,
    pub title: String,
    /// 目前沒有修改 Goal 的命令，版本固定為 1；Task 契約可以引用 Goal 的版本。
    pub version: i64,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub workspace_id: String,
    pub goal_id: String,
    pub title: String,
    /// 決定 Task 在哪一種工作目錄中執行（票 20）；建立時決定，之後不變。
    #[serde(default)]
    pub kind: TaskKind,
    pub status: TaskStatus,
    /// 每個事件加一；事件的 `aggregate_version` 與它對應。
    pub version: i64,
    /// 目前生效的契約版本。
    pub contract_version: i64,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

/// 相依邊：`task_id` 依賴 `depends_on`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dependency {
    pub task_id: String,
    pub depends_on: String,
}

// ---------------------------------------------------------------- 准入檢查

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MissingCode {
    FieldMissing,
    FieldUnknown,
    ReferenceInvalid,
    AuthorizationUnconfirmed,
    CapabilityNotGranted,
    WorkspaceArchived,
    /// Controller 提出的裁定請求還沒有回答，而這個 Task 依賴它的答案（票 17，ADR-0010）。
    DecisionPending,
}

/// 准入檢查列出的一個缺項。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissingItem {
    pub code: MissingCode,
    pub field: Option<ContractFieldName>,
    pub message: String,
}

/// 目前狀態的唯讀快照（同一個 Workspace 內）。Store 在交易中載入。
pub struct TaskContext<'a> {
    pub workspace_id: &'a str,
    pub workspace: Option<&'a Workspace>,
    /// Workspace 目前生效的授權 revision；尚未確認為 `None`。
    pub authorization: Option<&'a AuthorizationRevision>,
    pub goals: &'a [Goal],
    pub tasks: &'a [Task],
    pub dependencies: &'a [Dependency],
    /// 每個 Task 目前生效的契約。
    pub contracts: &'a [TaskContract],
    /// Controller 提出、尚未回答的裁定請求所阻擋的 Task（票 17）：依賴未決答案的 Task 不能進入 ready 或派工。
    pub decision_holds: &'a [DecisionHold],
    /// 這個 Workspace 的裁定請求作為 Task 依據（假設）的狀態（票 50）。
    pub assumptions: &'a [Assumption],
}

/// 一個裁定請求作為 Task 依據（假設）的狀態（票 50）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Assumption {
    pub request_id: String,
    /// 請求目前的內容版本（引用的版本不能超過它）。
    pub revision: i64,
    /// 使用者的裁定與建議不同（假設被推翻）時的時間與說明；尚未裁定或接受了建議時為 `None`。
    pub overturned: Option<Overturn>,
}

/// 假設被使用者的答案推翻。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Overturn {
    pub decided_at: Timestamp,
    pub reason: String,
}

/// 一個 Task 正在等待 Controller 提出的裁定請求的答案（票 17）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionHold {
    pub task_id: String,
    pub request_id: String,
    pub question: String,
}

impl TaskContext<'_> {
    pub(crate) fn task(&self, id: &str) -> Option<&Task> {
        self.tasks.iter().find(|t| t.id == id)
    }

    pub(crate) fn contract_of(&self, task: &Task) -> Option<&TaskContract> {
        self.contracts.iter().find(|c| c.task_id == task.id && c.version == task.contract_version)
    }

    /// 這個 Task 目前的契約依據了哪些已被推翻的假設：(裁定請求, 推翻)。
    fn overturned_assumptions(&self, task_id: &str) -> Vec<(&str, &Overturn)> {
        let Some(contract) = self.task(task_id).and_then(|t| self.contract_of(t)) else { return Vec::new() };
        contract
            .content
            .references()
            .filter(|r| r.kind == ReferenceKind::Decision)
            .filter_map(|r| {
                let assumption = self.assumptions.iter().find(|a| a.request_id == r.target)?;
                assumption.overturned.as_ref().map(|o| (assumption.request_id.as_str(), o))
            })
            .collect()
    }

    /// 這個 Task 目前的契約依據的假設已被推翻時的說明（票 50）：受影響的 Task 不能准入，它的 Gate 不能通過。
    pub fn overturned_basis(&self, task_id: &str) -> Option<String> {
        self.overturned_basis_after(task_id, Timestamp::from_millis(i64::MIN))
    }

    /// 同 [`Self::overturned_basis`]，但只算 `after` 之後才推翻的假設。
    pub fn overturned_basis_after(&self, task_id: &str, after: Timestamp) -> Option<String> {
        let reasons: Vec<String> = self
            .overturned_assumptions(task_id)
            .into_iter()
            .filter(|(_, o)| o.decided_at > after)
            .map(|(id, o)| format!("{}（裁定請求 {id}）", o.reason))
            .collect();
        (!reasons.is_empty()).then(|| reasons.join("；"))
    }

    pub(crate) fn title_of(&self, id: &str) -> String {
        self.task(id).map_or_else(|| id.to_string(), |t| format!("「{}」（{}）", t.title, t.id))
    }

    /// 引用失效的原因；有效回傳 `None`。
    fn reference_problem(&self, reference: &Reference) -> Option<String> {
        match reference.kind {
            ReferenceKind::TaskContract => {
                let Some(target) = self.task(&reference.target) else {
                    return Some("找不到這個 Task（引用只能指向同一個 Workspace 內的 Task）".into());
                };
                if target.status == TaskStatus::Cancelled {
                    return Some(format!("{}已取消", self.title_of(&target.id)));
                }
                (reference.version > target.contract_version).then(|| {
                    format!("{}沒有契約版本 {}（目前最新是 {}）", self.title_of(&target.id), reference.version, target.contract_version)
                })
            }
            ReferenceKind::Goal => match self.goals.iter().find(|g| g.id == reference.target) {
                None => Some("找不到這個 Goal".into()),
                Some(goal) => (reference.version > goal.version)
                    .then(|| format!("Goal 沒有版本 {}（目前最新是 {}）", reference.version, goal.version)),
            },
            ReferenceKind::Authorization => {
                if reference.target != self.workspace_id {
                    return Some("授權引用的目標必須是這個 Workspace 的識別".into());
                }
                match self.authorization {
                    None => Some("這個 Workspace 還沒有確認授權".into()),
                    Some(current) => (reference.version > current.revision)
                        .then(|| format!("授權沒有 revision {}（目前生效的是 {}）", reference.version, current.revision)),
                }
            }
            ReferenceKind::Decision => match self.assumptions.iter().find(|a| a.request_id == reference.target) {
                None => Some("找不到這個裁定請求（引用只能指向同一個 Workspace 內的請求）".into()),
                Some(a) if reference.version > a.revision => {
                    Some(format!("裁定請求沒有版本 {}（目前最新是 {}）", reference.version, a.revision))
                }
                Some(Assumption { overturned: Some(o), .. }) => Some(format!("依據的假設已被使用者推翻：{}", o.reason)),
                Some(_) => None,
            },
        }
    }
}

/// 啟動准入前的完整檢查：契約欄位、引用可用性與版本、授權。回傳全部缺項；空陣列表示可以進入 ready。
/// 相依是否完成不在這裡判定（那是派工時 Scheduler 的事，面板以 `unfinished_dependencies` 顯示）。
pub fn admission_report(ctx: &TaskContext<'_>, contract: &TaskContract) -> Vec<MissingItem> {
    let item = |code, field, message: String| MissingItem { code, field, message };
    let archived = ctx.workspace.is_some_and(|w| w.status == WorkspaceStatus::Archived);
    if archived {
        return vec![item(MissingCode::WorkspaceArchived, None, "Workspace 已經封存，不能派工".into())];
    }

    let mut missing = Vec::new();
    for name in ContractFieldName::ALL {
        let label = name.label();
        match contract.content.field(name) {
            None => missing.push(item(MissingCode::FieldMissing, Some(name), format!("缺少「{label}」"))),
            Some(ContractField::Unknown { .. }) => missing.push(item(
                MissingCode::FieldUnknown,
                Some(name),
                format!("「{label}」標為未知，必須先補足才能派工"),
            )),
            Some(ContractField::NotApplicable { .. }) => {}
            Some(ContractField::Filled { references, .. }) => {
                for reference in references {
                    if let Some(problem) = ctx.reference_problem(reference) {
                        missing.push(item(
                            MissingCode::ReferenceInvalid,
                            Some(name),
                            format!(
                                "「{label}」的引用 {}:{}@{} 失效：{problem}",
                                reference.kind.as_str(),
                                reference.target,
                                reference.version
                            ),
                        ));
                    }
                }
            }
        }
    }

    // 依賴未決答案的部分保持等待（ADR-0010）：使用者回答之前不能進入 ready 或派工
    for hold in ctx.decision_holds.iter().filter(|h| h.task_id == contract.task_id) {
        missing.push(item(
            MissingCode::DecisionPending,
            None,
            format!("等待使用者回答裁定請求「{}」（{}）", hold.question, hold.request_id),
        ));
    }

    let verdict = authorization::admit(
        ctx.workspace_id,
        ctx.workspace,
        ctx.authorization,
        &contract.content.required_capabilities,
    );
    match verdict {
        Ok(_) | Err(AdmissionDenied::WorkspaceNotFound(_)) | Err(AdmissionDenied::WorkspaceArchived(_)) => {}
        Err(denied @ AdmissionDenied::AuthorizationUnconfirmed(_)) => {
            missing.push(item(MissingCode::AuthorizationUnconfirmed, None, denied.to_string()))
        }
        Err(denied @ AdmissionDenied::CapabilityNotGranted { .. }) => {
            missing.push(item(MissingCode::CapabilityNotGranted, None, denied.to_string()))
        }
    }
    missing
}

// ---------------------------------------------------------------- 看板投影

/// 工作面板上的一張卡：Task、目前契約與等待／缺項資訊。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskView {
    #[serde(flatten)]
    pub task: Task,
    pub contract: TaskContract,
    pub dependencies: Vec<String>,
    /// `dependencies` 中狀態不是 `completed` 的 Task。
    pub unfinished_dependencies: Vec<String>,
    /// 只有 `pending` 的 Task 才計算（完整准入檢查）；其他狀態是空的。
    pub missing: Vec<MissingItem>,
}

/// 把一個 Task 投影成面板上的卡；契約遺失（資料不一致）時回傳 `None`。
pub fn view_task(ctx: &TaskContext<'_>, task: &Task) -> Option<TaskView> {
    let contract = ctx.contract_of(task)?.clone();
    let dependencies: Vec<String> =
        ctx.dependencies.iter().filter(|d| d.task_id == task.id).map(|d| d.depends_on.clone()).collect();
    let unfinished_dependencies = dependencies
        .iter()
        .filter(|id| ctx.task(id).is_none_or(|t| t.status != TaskStatus::Completed))
        .cloned()
        .collect();
    let missing = if task.status == TaskStatus::Pending { admission_report(ctx, &contract) } else { Vec::new() };
    Some(TaskView { task: task.clone(), contract, dependencies, unfinished_dependencies, missing })
}

/// 全部 Task 的看板投影，依建立順序。
pub fn board(ctx: &TaskContext<'_>) -> Vec<TaskView> {
    ctx.tasks.iter().filter_map(|t| view_task(ctx, t)).collect()
}

// ---------------------------------------------------------------- 命令

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskCommand {
    /// `id` 由呼叫端產生（Domain 不產生亂數）。
    CreateGoal { id: String, title: String },
    CreateTask { id: String, goal_id: String, title: String, kind: TaskKind, contract: ContractContent },
    /// 整份取代；內容沒有變動時不產生新版本。
    UpdateContract { task_id: String, contract: ContractContent, note: Option<String> },
    AddDependency { task_id: String, depends_on: String },
    /// pending → ready：必須通過完整的准入檢查。
    MarkReady { task_id: String },
    Cancel { task_id: String },
}

/// 命令成功後的結果：要寫入的新狀態與要追加的事件。沒有任何變動（冪等）時 `events` 是空的。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TaskChange {
    pub goal: Option<Goal>,
    pub task: Option<Task>,
    /// 新增的契約版本。
    pub contract: Option<TaskContract>,
    pub dependency: Option<Dependency>,
    pub events: Vec<DomainEvent>,
}

impl TaskChange {
    pub fn changed(&self) -> bool {
        !self.events.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TaskError {
    #[error("找不到 Workspace {0}")]
    WorkspaceNotFound(String),
    #[error("Workspace「{0}」已經封存，不能再新增或修改 Goal 與 Task")]
    WorkspaceArchived(String),
    #[error("找不到 Goal {0}")]
    GoalNotFound(String),
    #[error("找不到 Task {0}")]
    TaskNotFound(String),
    #[error("Goal 標題必須是 1 到 {MAX_TITLE_CHARS} 個字元")]
    GoalTitleInvalid,
    #[error("Task 標題必須是 1 到 {MAX_TITLE_CHARS} 個字元")]
    TaskTitleInvalid,
    #[error("契約內容不正確：{0}")]
    ContractInvalid(String),
    #[error("Task 不能依賴自己（{0}）")]
    DependencySelf(String),
    #[error("這條相依會造成循環：{}", .path.join(" → "))]
    DependencyCycle { path: Vec<String> },
    #[error("Task {id} 已經{}，不能再修改", if *.status == TaskStatus::Completed { "完成" } else { "取消" })]
    TaskClosed { id: String, status: TaskStatus },
    #[error("Task 目前是 {} 狀態，不能轉換到 {}", .from.as_str(), .to.as_str())]
    TransitionInvalid { from: TaskStatus, to: TaskStatus },
    #[error("准入檢查未通過，缺少：{}", join_messages(.0))]
    AdmissionDenied(Vec<MissingItem>),
}

fn join_messages(items: &[MissingItem]) -> String {
    items.iter().map(|i| i.message.as_str()).collect::<Vec<_>>().join("；")
}

impl TaskError {
    /// 穩定的機器可讀代碼，API 與 UI 依它判斷錯誤種類。
    pub fn code(&self) -> &'static str {
        match self {
            TaskError::WorkspaceNotFound(_) => "workspace_not_found",
            TaskError::WorkspaceArchived(_) => "workspace_archived",
            TaskError::GoalNotFound(_) => "goal_not_found",
            TaskError::TaskNotFound(_) => "task_not_found",
            TaskError::GoalTitleInvalid => "goal_title_invalid",
            TaskError::TaskTitleInvalid => "task_title_invalid",
            TaskError::ContractInvalid(_) => "contract_invalid",
            TaskError::DependencySelf(_) => "dependency_self",
            TaskError::DependencyCycle { .. } => "dependency_cycle",
            TaskError::TaskClosed { .. } => "task_closed",
            TaskError::TransitionInvalid { .. } => "task_transition_invalid",
            TaskError::AdmissionDenied(_) => "task_admission_denied",
        }
    }
}

fn clean_title(raw: &str) -> Option<String> {
    let title = raw.trim();
    (!title.is_empty() && title.chars().count() <= MAX_TITLE_CHARS).then(|| title.to_string())
}

/// 目前狀態之後的下一個事件版本。
pub(crate) struct Versioned<'a> {
    pub(crate) task: &'a mut Task,
    pub(crate) events: Vec<DomainEvent>,
    pub(crate) now: Timestamp,
}

impl Versioned<'_> {
    pub(crate) fn push(&mut self, event_type: &'static str, payload: serde_json::Value) {
        self.task.version += 1;
        self.task.updated_at = self.now;
        self.events.push(DomainEvent {
            aggregate_type: TASK_AGGREGATE,
            aggregate_id: self.task.id.clone(),
            aggregate_version: self.task.version,
            event_type,
            occurred_at: self.now,
            payload,
        });
    }

    pub(crate) fn change_status(&mut self, to: TaskStatus, reason: &str) -> Result<(), TaskError> {
        let from = self.task.status;
        if !from.can_transition_to(to) {
            return Err(TaskError::TransitionInvalid { from, to });
        }
        self.task.status = to;
        self.push(EVENT_STATUS_CHANGED, serde_json::json!({ "from": from, "to": to, "reason": reason }));
        Ok(())
    }
}

/// 套用一個命令。
pub fn decide(ctx: &TaskContext<'_>, command: TaskCommand, now: Timestamp) -> Result<TaskChange, TaskError> {
    let workspace = ctx.workspace.ok_or_else(|| TaskError::WorkspaceNotFound(ctx.workspace_id.to_string()))?;
    if workspace.status == WorkspaceStatus::Archived {
        return Err(TaskError::WorkspaceArchived(workspace.name.clone()));
    }
    let find_task = |id: &str| ctx.task(id).cloned().ok_or_else(|| TaskError::TaskNotFound(id.to_string()));
    let open_task = |id: &str| {
        let task = find_task(id)?;
        if task.status.is_terminal() {
            return Err(TaskError::TaskClosed { id: task.id, status: task.status });
        }
        Ok(task)
    };

    match command {
        TaskCommand::CreateGoal { id, title } => {
            let title = clean_title(&title).ok_or(TaskError::GoalTitleInvalid)?;
            let goal = Goal { id, workspace_id: ctx.workspace_id.to_string(), title, version: 1, created_at: now };
            let event = DomainEvent {
                aggregate_type: GOAL_AGGREGATE,
                aggregate_id: goal.id.clone(),
                aggregate_version: goal.version,
                event_type: EVENT_GOAL_CREATED,
                occurred_at: now,
                payload: serde_json::json!({ "title": goal.title }),
            };
            Ok(TaskChange { goal: Some(goal), events: vec![event], ..TaskChange::default() })
        }
        TaskCommand::CreateTask { id, goal_id, title, kind, contract } => {
            let title = clean_title(&title).ok_or(TaskError::TaskTitleInvalid)?;
            if !ctx.goals.iter().any(|g| g.id == goal_id) {
                return Err(TaskError::GoalNotFound(goal_id));
            }
            let content = contract.normalized()?;
            let mut task = Task {
                id,
                workspace_id: ctx.workspace_id.to_string(),
                goal_id,
                title,
                kind,
                status: TaskStatus::Pending,
                version: 0,
                contract_version: 1,
                created_at: now,
                updated_at: now,
            };
            let contract = TaskContract { task_id: task.id.clone(), version: 1, content, note: None, created_at: now };
            let mut versioned = Versioned { task: &mut task, events: Vec::new(), now };
            versioned.push(
                EVENT_TASK_CREATED,
                serde_json::json!({
                    "goal_id": versioned.task.goal_id,
                    "title": versioned.task.title,
                    "kind": versioned.task.kind,
                    "contract_version": 1,
                }),
            );
            let events = versioned.events;
            Ok(TaskChange { task: Some(task), contract: Some(contract), events, ..TaskChange::default() })
        }
        TaskCommand::UpdateContract { task_id, contract, note } => {
            let mut task = open_task(&task_id)?;
            let content = contract.normalized()?;
            let current = ctx.contract_of(&task).ok_or_else(|| TaskError::TaskNotFound(task_id.clone()))?;
            if current.content == content {
                return Ok(TaskChange::default());
            }
            let note = note.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
            let version = task.contract_version + 1;
            task.contract_version = version;
            let new_contract =
                TaskContract { task_id: task.id.clone(), version, content, note: note.clone(), created_at: now };
            let mut versioned = Versioned { task: &mut task, events: Vec::new(), now };
            versioned.push(EVENT_CONTRACT_CHANGED, serde_json::json!({ "contract_version": version, "note": note }));
            // 契約變了，先前的准入結果不再有效：ready 退回 pending，必須重新通過准入
            if versioned.task.status == TaskStatus::Ready {
                versioned.change_status(TaskStatus::Pending, "contract_changed")?;
            }
            let events = versioned.events;
            Ok(TaskChange { task: Some(task), contract: Some(new_contract), events, ..TaskChange::default() })
        }
        TaskCommand::AddDependency { task_id, depends_on } => {
            let mut task = open_task(&task_id)?;
            let target = find_task(&depends_on)?;
            if task.id == target.id {
                return Err(TaskError::DependencySelf(ctx.title_of(&task.id)));
            }
            if ctx.dependencies.iter().any(|d| d.task_id == task.id && d.depends_on == target.id) {
                return Ok(TaskChange::default());
            }
            // 新增 task → target 會造成循環，若且唯若 target 已經（直接或間接）依賴 task
            if let Some(path) = dependency_path(ctx, &target.id, &task.id) {
                let mut cycle = vec![ctx.title_of(&task.id)];
                cycle.extend(path.iter().map(|id| ctx.title_of(id)));
                return Err(TaskError::DependencyCycle { path: cycle });
            }
            let mut versioned = Versioned { task: &mut task, events: Vec::new(), now };
            versioned.push(EVENT_DEPENDENCY_ADDED, serde_json::json!({ "depends_on": target.id }));
            let events = versioned.events;
            let dependency = Dependency { task_id: task.id.clone(), depends_on: target.id };
            Ok(TaskChange { task: Some(task), dependency: Some(dependency), events, ..TaskChange::default() })
        }
        TaskCommand::MarkReady { task_id } => {
            let mut task = find_task(&task_id)?;
            // 只有 pending 與（預算用盡等原因而）blocked 的 Task 可以標為 ready（AI-276）；進行中、驗證中或等待中的
            // Task 由各自的流程離開，不能被直接標回 ready
            if !matches!(task.status, TaskStatus::Pending | TaskStatus::Blocked) {
                return Err(TaskError::TransitionInvalid { from: task.status, to: TaskStatus::Ready });
            }
            let contract = ctx.contract_of(&task).ok_or_else(|| TaskError::TaskNotFound(task_id.clone()))?;
            let missing = admission_report(ctx, contract);
            if !missing.is_empty() {
                return Err(TaskError::AdmissionDenied(missing));
            }
            let mut versioned = Versioned { task: &mut task, events: Vec::new(), now };
            versioned.change_status(TaskStatus::Ready, "admitted")?;
            let events = versioned.events;
            Ok(TaskChange { task: Some(task), events, ..TaskChange::default() })
        }
        TaskCommand::Cancel { task_id } => {
            let mut task = find_task(&task_id)?;
            let mut versioned = Versioned { task: &mut task, events: Vec::new(), now };
            versioned.change_status(TaskStatus::Cancelled, "cancelled")?;
            let events = versioned.events;
            Ok(TaskChange { task: Some(task), events, ..TaskChange::default() })
        }
    }
}

/// 沿著「依賴」方向從 `from` 走到 `to` 的路徑（含兩端）；走不到回傳 `None`。
fn dependency_path(ctx: &TaskContext<'_>, from: &str, to: &str) -> Option<Vec<String>> {
    let mut visited = vec![from.to_string()];
    let mut stack: Vec<Vec<String>> = vec![vec![from.to_string()]];
    while let Some(path) = stack.pop() {
        let last = path.last().expect("路徑至少有一個節點");
        if last == to {
            return Some(path);
        }
        for edge in ctx.dependencies.iter().filter(|d| d.task_id == *last) {
            if !visited.contains(&edge.depends_on) {
                visited.push(edge.depends_on.clone());
                let mut next = path.clone();
                next.push(edge.depends_on.clone());
                stack.push(next);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authorization::{ConfirmOutcome, ConfirmRequest, TEMPLATE_RESEARCH_DOCS};
    use crate::workspace::{decide as decide_workspace, WorkspaceCommand};

    const T1: Timestamp = Timestamp::from_millis(1_000);
    const T2: Timestamp = Timestamp::from_millis(2_000);

    /// 測試用的世界：每次命令後把結果套回去，模擬 Store 的載入與寫入。
    struct World {
        workspace: Workspace,
        authorization: Option<AuthorizationRevision>,
        goals: Vec<Goal>,
        tasks: Vec<Task>,
        dependencies: Vec<Dependency>,
        contracts: Vec<TaskContract>,
        holds: Vec<DecisionHold>,
        assumptions: Vec<Assumption>,
        events: Vec<DomainEvent>,
    }

    impl World {
        fn new() -> Self {
            let workspace = decide_workspace(
                &[],
                WorkspaceCommand::Create { id: "ws_1".into(), name: None, path: "C:\\work\\app".into() },
                T1,
            )
            .unwrap()
            .workspace;
            Self {
                workspace,
                authorization: None,
                goals: vec![],
                tasks: vec![],
                dependencies: vec![],
                contracts: vec![],
                holds: vec![],
                assumptions: vec![],
                events: vec![],
            }
        }

        fn with_goal() -> Self {
            let mut world = Self::new();
            world.run(TaskCommand::CreateGoal { id: "g1".into(), title: "目標一".into() }, T1).unwrap();
            world
        }

        fn ctx(&self) -> TaskContext<'_> {
            // contracts 只放目前生效的版本
            TaskContext {
                workspace_id: &self.workspace.id,
                workspace: Some(&self.workspace),
                authorization: self.authorization.as_ref(),
                goals: &self.goals,
                tasks: &self.tasks,
                dependencies: &self.dependencies,
                contracts: &self.contracts,
                decision_holds: &self.holds,
                assumptions: &self.assumptions,
            }
        }

        fn run(&mut self, command: TaskCommand, now: Timestamp) -> Result<TaskChange, TaskError> {
            let change = decide(&self.ctx(), command, now)?;
            if let Some(goal) = change.goal.clone() {
                self.goals.push(goal);
            }
            if let Some(task) = change.task.clone() {
                match self.tasks.iter_mut().find(|t| t.id == task.id) {
                    Some(existing) => *existing = task,
                    None => self.tasks.push(task),
                }
            }
            if let Some(contract) = change.contract.clone() {
                self.contracts.retain(|c| c.task_id != contract.task_id);
                self.contracts.push(contract);
            }
            if let Some(dependency) = change.dependency.clone() {
                self.dependencies.push(dependency);
            }
            self.events.extend(change.events.clone());
            Ok(change)
        }

        fn task(&self, id: &str) -> &Task {
            self.tasks.iter().find(|t| t.id == id).unwrap()
        }

        fn add_task(&mut self, id: &str, contract: ContractContent) {
            self.run(
                TaskCommand::CreateTask {
                    id: id.into(),
                    goal_id: "g1".into(),
                    title: format!("任務 {id}"),
                    kind: TaskKind::Research,
                    contract,
                },
                T1,
            )
            .unwrap();
        }

        fn depend(&mut self, task: &str, on: &str) -> Result<TaskChange, TaskError> {
            self.run(TaskCommand::AddDependency { task_id: task.into(), depends_on: on.into() }, T2)
        }

        fn missing_of(&self, id: &str) -> Vec<MissingItem> {
            let task = self.task(id);
            admission_report(&self.ctx(), self.ctx().contract_of(task).unwrap())
        }

        fn confirm_research_docs(&mut self) {
            let grants = crate::authorization::templates(&self.workspace.path)
                .into_iter()
                .find(|t| t.id == TEMPLATE_RESEARCH_DOCS)
                .unwrap()
                .grants;
            let request = ConfirmRequest {
                template_id: None,
                grants,
                acknowledged_risk: false,
                acknowledged_expansion: true,
                expected_revision: None,
            };
            match crate::authorization::confirm(&self.workspace, None, request, T1).unwrap() {
                ConfirmOutcome::Created { revision, .. } => self.authorization = Some(revision),
                other => panic!("{other:?}"),
            }
        }
    }

    fn filled(text: &str) -> Option<ContractField> {
        Some(ContractField::Filled { text: text.into(), references: vec![] })
    }

    fn filled_with(text: &str, references: Vec<Reference>) -> Option<ContractField> {
        Some(ContractField::Filled { text: text.into(), references })
    }

    fn complete() -> ContractContent {
        ContractContent {
            objective: filled("整理競品資料"),
            inputs: Some(ContractField::NotApplicable { reason: "沒有既有資料".into() }),
            scope: filled("只看公開資料"),
            deliverables: filled("一份比較表"),
            done_criteria: filled("至少比較五個產品"),
            blocker_handling: filled("缺資料就回報，不自行猜測"),
            required_capabilities: vec![],
            verification_checks: vec![],
        }
    }

    fn reference(kind: ReferenceKind, target: &str, version: i64) -> Reference {
        Reference { kind, target: target.into(), version }
    }

    fn codes(items: &[MissingItem]) -> Vec<(MissingCode, Option<ContractFieldName>)> {
        items.iter().map(|i| (i.code, i.field)).collect()
    }

    #[test]
    fn new_task_is_pending_with_contract_version_one_even_when_empty() {
        let mut world = World::with_goal();
        let change = world
            .run(
                TaskCommand::CreateTask {
                    id: "t1".into(),
                    goal_id: "g1".into(),
                    title: "  整理資料  ".into(),
                    kind: TaskKind::Development,
                    contract: ContractContent::default(),
                },
                T1,
            )
            .unwrap();

        let task = change.task.unwrap();
        assert_eq!((task.title.as_str(), task.status, task.version, task.contract_version), ("整理資料", TaskStatus::Pending, 1, 1));
        assert_eq!(task.kind, TaskKind::Development, "種類在建立時決定");
        assert_eq!(change.events[0].payload["kind"], "development");
        assert_eq!(change.contract.unwrap().version, 1);
        assert_eq!(change.events.len(), 1);
        assert_eq!((change.events[0].event_type, change.events[0].aggregate_version), (EVENT_TASK_CREATED, 1));
    }

    #[test]
    fn titles_and_goal_are_validated() {
        let mut world = World::with_goal();
        let long = "x".repeat(MAX_TITLE_CHARS + 1);
        assert_eq!(
            world.run(TaskCommand::CreateGoal { id: "g2".into(), title: "  ".into() }, T1),
            Err(TaskError::GoalTitleInvalid)
        );
        for bad in ["", "   ", long.as_str()] {
            let create = TaskCommand::CreateTask {
                id: "t1".into(),
                goal_id: "g1".into(),
                title: bad.into(),
                kind: TaskKind::default(),
                contract: ContractContent::default(),
            };
            assert_eq!(world.run(create, T1), Err(TaskError::TaskTitleInvalid));
        }
        let orphan = TaskCommand::CreateTask {
            id: "t1".into(),
            goal_id: "nope".into(),
            title: "x".into(),
            kind: TaskKind::default(),
            contract: ContractContent::default(),
        };
        assert_eq!(world.run(orphan, T1), Err(TaskError::GoalNotFound("nope".into())));
    }

    #[test]
    fn contract_content_rejects_blank_text_unreasoned_not_applicable_and_not_applicable_objective() {
        let blank = ContractContent { scope: filled("   "), ..ContractContent::default() };
        let no_reason = ContractContent {
            inputs: Some(ContractField::NotApplicable { reason: "  ".into() }),
            ..ContractContent::default()
        };
        let na_objective = ContractContent {
            objective: Some(ContractField::NotApplicable { reason: "不需要".into() }),
            ..ContractContent::default()
        };
        let na_done = ContractContent {
            done_criteria: Some(ContractField::NotApplicable { reason: "不需要".into() }),
            ..ContractContent::default()
        };
        let bad_reference = ContractContent {
            inputs: filled_with("x", vec![reference(ReferenceKind::Goal, "g1", 0)]),
            ..ContractContent::default()
        };

        for content in [blank, no_reason, na_objective, na_done, bad_reference] {
            assert!(matches!(content.normalized(), Err(TaskError::ContractInvalid(_))));
        }
    }

    #[test]
    fn normalization_trims_text_sorts_capabilities_and_keeps_references_and_versions() {
        let content = ContractContent {
            inputs: filled_with(" 看這份 ", vec![reference(ReferenceKind::TaskContract, " t0 ", 3)]),
            scope: Some(ContractField::Unknown { note: Some("  ".into()) }),
            required_capabilities: vec![Capability::WriteFiles, Capability::ReadFiles, Capability::WriteFiles],
            ..ContractContent::default()
        };

        let normalized = content.normalized().unwrap();

        assert_eq!(normalized.inputs, filled_with("看這份", vec![reference(ReferenceKind::TaskContract, "t0", 3)]));
        assert_eq!(normalized.scope, Some(ContractField::Unknown { note: None }));
        assert_eq!(normalized.required_capabilities, vec![Capability::ReadFiles, Capability::WriteFiles]);
    }

    #[test]
    fn contract_json_round_trips_through_the_flattened_shape() {
        let contract = TaskContract {
            task_id: "t1".into(),
            version: 2,
            content: ContractContent {
                inputs: filled_with("x", vec![reference(ReferenceKind::Authorization, "ws_1", 1)]),
                scope: Some(ContractField::Unknown { note: None }),
                required_capabilities: vec![Capability::RunBuildTest],
                ..ContractContent::default()
            },
            note: Some("補範圍".into()),
            created_at: T2,
        };

        let json = serde_json::to_value(&contract).unwrap();

        assert_eq!(json["inputs"]["status"], "filled");
        assert_eq!(json["scope"], serde_json::json!({ "status": "unknown", "note": null }));
        assert_eq!(json["objective"], serde_json::Value::Null);
        assert_eq!(json["required_capabilities"], serde_json::json!(["run_build_test"]));
        assert_eq!(serde_json::from_value::<TaskContract>(json).unwrap(), contract);
    }

    #[test]
    fn missing_unknown_and_not_applicable_fields_are_reported_differently() {
        let mut world = World::with_goal();
        let mut content = complete();
        content.objective = None;
        content.scope = Some(ContractField::Unknown { note: None });
        world.add_task("t1", content);

        let missing = world.missing_of("t1");

        assert_eq!(
            codes(&missing),
            vec![
                (MissingCode::FieldMissing, Some(ContractFieldName::Objective)),
                (MissingCode::FieldUnknown, Some(ContractFieldName::Scope)),
            ]
        );
        assert!(missing[1].message.contains("未知"));
    }

    #[test]
    fn complete_contract_is_admitted_and_ready_event_is_appended() {
        let mut world = World::with_goal();
        world.add_task("t1", complete());

        let change = world.run(TaskCommand::MarkReady { task_id: "t1".into() }, T2).unwrap();

        assert_eq!(world.task("t1").status, TaskStatus::Ready);
        assert_eq!(world.task("t1").version, 2);
        assert_eq!((change.events[0].event_type, change.events[0].payload["to"].as_str()), (EVENT_STATUS_CHANGED, Some("ready")));
    }

    #[test]
    fn incomplete_contract_cannot_become_ready_and_nothing_changes() {
        let mut world = World::with_goal();
        world.add_task("t1", ContractContent::default());
        let events_before = world.events.len();

        let result = world.run(TaskCommand::MarkReady { task_id: "t1".into() }, T2);

        let Err(TaskError::AdmissionDenied(missing)) = result else { panic!("{result:?}") };
        assert_eq!(missing.len(), 6);
        assert_eq!((world.task("t1").status, world.events.len()), (TaskStatus::Pending, events_before));
    }

    #[test]
    fn a_task_waiting_for_an_unanswered_controller_question_cannot_become_ready() {
        let mut world = World::with_goal();
        world.add_task("t1", complete());
        world.add_task("t2", complete());
        world.holds.push(DecisionHold { task_id: "t1".into(), request_id: "dr_1".into(), question: "要支援哪些平台？".into() });

        let result = world.run(TaskCommand::MarkReady { task_id: "t1".into() }, T2);

        let Err(TaskError::AdmissionDenied(missing)) = result else { panic!("{result:?}") };
        assert_eq!(codes(&missing), vec![(MissingCode::DecisionPending, None)]);
        assert!(missing[0].message.contains("要支援哪些平台？") && missing[0].message.contains("dr_1"));
        // 不依賴這個答案的 Task 不受影響
        world.run(TaskCommand::MarkReady { task_id: "t2".into() }, T2).unwrap();
        assert_eq!((world.task("t1").status, world.task("t2").status), (TaskStatus::Pending, TaskStatus::Ready));
    }

    #[test]
    fn ready_requires_pending_state() {
        let mut world = World::with_goal();
        world.add_task("t1", complete());
        world.run(TaskCommand::MarkReady { task_id: "t1".into() }, T2).unwrap();

        let again = world.run(TaskCommand::MarkReady { task_id: "t1".into() }, T2);

        assert_eq!(again, Err(TaskError::TransitionInvalid { from: TaskStatus::Ready, to: TaskStatus::Ready }));
    }

    #[test]
    fn only_pending_and_blocked_tasks_can_be_marked_ready() {
        // 進行中、驗證中或等待中的 Task 被標成 ready 會讓舊 Run 的結果被忽略、整件工作重做（R151 第 22 項）
        for status in [TaskStatus::Running, TaskStatus::Verifying, TaskStatus::WaitingDecision, TaskStatus::WaitingResource] {
            let mut world = World::with_goal();
            world.add_task("t1", complete());
            world.tasks[0].status = status;

            let result = world.run(TaskCommand::MarkReady { task_id: "t1".into() }, T2);

            assert_eq!(result, Err(TaskError::TransitionInvalid { from: status, to: TaskStatus::Ready }));
            assert_eq!(world.task("t1").status, status);
        }
        // 預算用盡而阻擋的 Task 仍可由使用者標回可派工（AI-276）
        let mut world = World::with_goal();
        world.add_task("t1", complete());
        world.tasks[0].status = TaskStatus::Blocked;
        world.run(TaskCommand::MarkReady { task_id: "t1".into() }, T2).unwrap();
        assert_eq!(world.task("t1").status, TaskStatus::Ready);
    }

    #[test]
    fn invalid_references_are_reported_per_field_but_saving_them_is_allowed() {
        let mut world = World::with_goal();
        world.add_task("dep", complete());
        world.add_task("gone", complete());
        world.run(TaskCommand::Cancel { task_id: "gone".into() }, T2).unwrap();
        world.confirm_research_docs();
        let mut content = complete();
        content.inputs = filled_with(
            "參考",
            vec![
                reference(ReferenceKind::TaskContract, "dep", 1),      // 有效
                reference(ReferenceKind::TaskContract, "dep", 2),      // 沒有這個版本
                reference(ReferenceKind::TaskContract, "nope", 1),     // 沒有這個 Task
                reference(ReferenceKind::TaskContract, "gone", 1),     // 已取消
                reference(ReferenceKind::Goal, "g1", 1),               // 有效
                reference(ReferenceKind::Goal, "g9", 1),               // 沒有這個 Goal
                reference(ReferenceKind::Authorization, "ws_1", 1),    // 有效
                reference(ReferenceKind::Authorization, "ws_1", 2),    // 沒有這個 revision
                reference(ReferenceKind::Authorization, "ws_other", 1), // 不是這個 Workspace
            ],
        );
        world.add_task("t1", content);

        let missing = world.missing_of("t1");

        assert_eq!(missing.len(), 6, "{missing:#?}");
        assert!(missing.iter().all(|m| m.code == MissingCode::ReferenceInvalid && m.field == Some(ContractFieldName::Inputs)));
        assert!(missing[0].message.contains("task_contract:dep@2"));
    }

    #[test]
    fn a_task_built_on_an_overturned_assumption_cannot_be_admitted() {
        let mut world = World::with_goal();
        world.assumptions = vec![
            Assumption { request_id: "dr_open".into(), revision: 1, overturned: None },
            Assumption {
                request_id: "dr_rest".into(),
                revision: 2,
                overturned: Some(Overturn { decided_at: T2, reason: "使用者選了 gRPC，不是建議的 REST".into() }),
            },
        ];
        let mut content = complete();
        content.inputs = filled_with(
            "依據裁定",
            vec![
                reference(ReferenceKind::Decision, "dr_open", 1), // 還沒裁定：照假設先做
                reference(ReferenceKind::Decision, "dr_open", 2), // 沒有這個版本
                reference(ReferenceKind::Decision, "dr_none", 1), // 沒有這個請求
                reference(ReferenceKind::Decision, "dr_rest", 1), // 假設已被推翻
            ],
        );
        world.add_task("t1", content);
        world.add_task("t2", complete());

        let missing = world.missing_of("t1");
        assert_eq!(missing.len(), 3, "{missing:#?}");
        assert!(missing.iter().all(|m| m.code == MissingCode::ReferenceInvalid));
        assert!(missing[2].message.contains("decision:dr_rest@1") && missing[2].message.contains("gRPC"), "{missing:#?}");
        let ctx = world.ctx();
        assert!(ctx.overturned_basis("t1").unwrap().contains("gRPC"));
        assert_eq!(ctx.overturned_basis("t2"), None, "沒有依據這個假設的 Task 不受影響");
        assert!(world.missing_of("t2").is_empty());
    }

    #[test]
    fn authorization_mismatch_is_reported_and_follows_the_current_revision() {
        let mut world = World::with_goal();
        let mut content = complete();
        content.required_capabilities = vec![Capability::RunBuildTest];
        world.add_task("t1", content);
        world.add_task("t2", complete());

        assert_eq!(codes(&world.missing_of("t1")), vec![(MissingCode::AuthorizationUnconfirmed, None)]);
        assert!(world.missing_of("t2").is_empty(), "不需要任何能力的 Task 不需要確認授權");

        world.confirm_research_docs();

        let missing = world.missing_of("t1");
        assert_eq!(codes(&missing), vec![(MissingCode::CapabilityNotGranted, None)]);
        assert!(missing[0].message.contains("建置與測試"));
    }

    #[test]
    fn archived_workspace_rejects_every_command_and_reports_archived() {
        let mut world = World::with_goal();
        world.add_task("t1", complete());
        world.workspace.status = WorkspaceStatus::Archived;

        let create = world.run(TaskCommand::CreateGoal { id: "g2".into(), title: "x".into() }, T2);
        let ready = world.run(TaskCommand::MarkReady { task_id: "t1".into() }, T2);

        assert!(matches!(create, Err(TaskError::WorkspaceArchived(_))));
        assert!(matches!(ready, Err(TaskError::WorkspaceArchived(_))));
        assert_eq!(codes(&world.missing_of("t1")), vec![(MissingCode::WorkspaceArchived, None)]);
    }

    #[test]
    fn contract_update_creates_versions_only_when_content_changes() {
        let mut world = World::with_goal();
        world.add_task("t1", ContractContent::default());

        let v2 = world
            .run(
                TaskCommand::UpdateContract { task_id: "t1".into(), contract: complete(), note: Some(" 補齊 ".into()) },
                T2,
            )
            .unwrap();
        let same = world
            .run(TaskCommand::UpdateContract { task_id: "t1".into(), contract: complete(), note: Some("x".into()) }, T2)
            .unwrap();

        let contract = v2.contract.unwrap();
        assert_eq!((contract.version, contract.note.as_deref(), contract.created_at), (2, Some("補齊"), T2));
        assert_eq!(v2.events[0].event_type, EVENT_CONTRACT_CHANGED);
        assert!(!same.changed());
        assert_eq!(world.task("t1").contract_version, 2);
    }

    #[test]
    fn changing_the_contract_of_a_ready_task_sends_it_back_to_pending() {
        let mut world = World::with_goal();
        world.add_task("t1", complete());
        world.run(TaskCommand::MarkReady { task_id: "t1".into() }, T2).unwrap();
        let mut changed = complete();
        changed.scope = filled("改成也看付費資料");

        let change = world
            .run(TaskCommand::UpdateContract { task_id: "t1".into(), contract: changed, note: None }, T2)
            .unwrap();

        assert_eq!(world.task("t1").status, TaskStatus::Pending);
        let versions: Vec<i64> = change.events.iter().map(|e| e.aggregate_version).collect();
        assert_eq!(versions, vec![3, 4]);
        assert_eq!(change.events[1].payload["from"], "ready");
    }

    #[test]
    fn closed_tasks_reject_contract_and_dependency_changes_but_cancelling_twice_is_a_transition_error() {
        let mut world = World::with_goal();
        world.add_task("t1", complete());
        world.add_task("t2", complete());
        world.run(TaskCommand::Cancel { task_id: "t1".into() }, T2).unwrap();

        let update = world.run(TaskCommand::UpdateContract { task_id: "t1".into(), contract: complete(), note: None }, T2);
        let dependency = world.depend("t1", "t2");
        let again = world.run(TaskCommand::Cancel { task_id: "t1".into() }, T2);

        assert_eq!(update, Err(TaskError::TaskClosed { id: "t1".into(), status: TaskStatus::Cancelled }));
        assert!(matches!(dependency, Err(TaskError::TaskClosed { .. })));
        assert_eq!(again, Err(TaskError::TransitionInvalid { from: TaskStatus::Cancelled, to: TaskStatus::Cancelled }));
    }

    #[test]
    fn dependencies_reject_self_cycles_and_unknown_tasks_and_are_idempotent() {
        let mut world = World::with_goal();
        for id in ["a", "b", "c"] {
            world.add_task(id, complete());
        }

        assert!(matches!(world.depend("a", "a"), Err(TaskError::DependencySelf(_))));
        assert!(matches!(world.depend("a", "zzz"), Err(TaskError::TaskNotFound(id)) if id == "zzz"));
        assert!(world.depend("a", "b").unwrap().changed());
        assert!(world.depend("b", "c").unwrap().changed());
        assert!(!world.depend("a", "b").unwrap().changed(), "重複新增是冪等的");

        let Err(TaskError::DependencyCycle { path }) = world.depend("c", "a") else { panic!("應該是循環") };
        assert_eq!(path.len(), 4);
        assert!(path[0].contains("任務 c") && path[3].contains("任務 c"));
        assert!(matches!(world.depend("b", "a"), Err(TaskError::DependencyCycle { .. })));
        // 不是循環的邊仍然可以加（菱形）
        assert!(world.depend("a", "c").unwrap().changed());
        assert_eq!(world.dependencies.len(), 3);
    }

    #[test]
    fn board_shows_missing_only_for_pending_tasks_and_unfinished_dependencies() {
        let mut world = World::with_goal();
        world.add_task("a", complete());
        world.add_task("b", ContractContent::default());
        world.depend("b", "a").unwrap();
        world.run(TaskCommand::MarkReady { task_id: "a".into() }, T2).unwrap();

        let board = board(&world.ctx());

        assert_eq!(board.iter().map(|v| v.task.id.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        assert!(board[0].missing.is_empty() && board[0].unfinished_dependencies.is_empty());
        assert_eq!(board[1].missing.len(), 6);
        assert_eq!((board[1].dependencies.clone(), board[1].unfinished_dependencies.clone()), (vec!["a".to_string()], vec!["a".to_string()]));
    }

    #[test]
    fn transition_table_has_no_exit_from_terminal_states_and_ready_is_not_reachable_from_them() {
        assert!(TaskStatus::Completed.allowed_next().is_empty());
        assert!(TaskStatus::Cancelled.allowed_next().is_empty());
        for status in TaskStatus::ALL {
            assert_eq!(TaskStatus::parse(status.as_str()), Some(status));
            assert_eq!(status.is_terminal(), status.allowed_next().is_empty());
            if !status.is_terminal() {
                assert!(status.can_transition_to(TaskStatus::Cancelled), "{status:?} 必須可以取消");
            }
            assert!(!status.can_transition_to(status), "{status:?} 不能轉到自己");
        }
        assert!(TaskStatus::Running.can_transition_to(TaskStatus::Verifying));
        assert!(!TaskStatus::Pending.can_transition_to(TaskStatus::Running), "沒有通過准入就不能執行");
        assert!(!TaskStatus::Running.can_transition_to(TaskStatus::Completed), "必須經過驗證");
    }

    #[test]
    fn error_codes_are_stable() {
        assert_eq!(TaskError::DependencyCycle { path: vec![] }.code(), "dependency_cycle");
        assert_eq!(TaskError::AdmissionDenied(vec![]).code(), "task_admission_denied");
        assert_eq!(TaskError::ContractInvalid("x".into()).code(), "contract_invalid");
        assert_eq!(TaskError::TaskClosed { id: "t".into(), status: TaskStatus::Completed }.code(), "task_closed");
    }
}
