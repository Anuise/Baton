//! 成果接受與最小完成驗證（票 13）：契約中宣告的確定性檢查、檢查結果的判定、成果接受交易的校驗。
//!
//! 純邏輯、無 I/O。Task 只有在「完成與驗證條件」有對應證據時才進入 `completed`：
//! - Worker 宣稱成功或程序正常退出（Run `succeeded`）只讓 Task 進入 `verifying`（見 `run` 模組）。
//! - 之後只有兩條路能完成驗證：Runtime 執行契約宣告的確定性檢查（命令結束碼、檔案存在與雜湊、成果存在與雜湊）
//!   並全部通過，或使用者以桌面憑證手動接受。
//! - 成果接受交易同時校驗三件事：受驗的 Run 仍是該 Task 最新的世代、成果 manifest 與觀察當時相同、契約版本未變。
//!   任何一項不符就拒絕，什麼都不寫；檢查未通過時 Task 回到 `ready`（可修正狀態），證據保存在驗證紀錄中。
//! - 開發 Task（票 20）通過時還不算完成：同一個交易把平台分支排入整合佇列，Task 停在 `verifying`，
//!   由整合佇列合併並通過整合驗證後才完成（見 `integration` 模組）。

use serde::{Deserialize, Serialize};

use crate::artifact::{self, Artifact, ArtifactDraft};
use crate::event::DomainEvent;
use crate::integration::{self, IntegrationEntry};
use crate::run::{Run, RunStatus};
use crate::task::{Task, TaskContext, TaskStatus, Versioned};
use crate::workdir::{TaskKind, Workdir, WorkdirKind};
use crate::workspace::WorkspaceStatus;
use crate::Timestamp;

pub const EVENT_TASK_VERIFICATION_RECORDED: &str = "task_verification_recorded";

/// 一份契約最多宣告幾項確定性檢查。
pub const MAX_CHECKS: usize = 20;
/// 命令檢查的逾時（毫秒）；逾時視為未通過。
pub const COMMAND_TIMEOUT_MS: u64 = 10 * 60 * 1000;
/// 每項檢查保存的證據文字上限（字元數，保留結尾）。
pub const DETAIL_MAX_CHARS: usize = 4_000;
/// 手動接受附註的上限（字元數）。
pub const NOTE_MAX_CHARS: usize = 1_000;

// ---------------------------------------------------------------- 確定性檢查（契約的一部分）

/// 契約中宣告的一項確定性檢查；隨契約版本保存。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum VerificationCheck {
    /// 在 Run 的工作目錄執行命令（不經過 shell），結束碼必須等於 `expected_exit_code`。
    Command {
        program: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        expected_exit_code: i32,
    },
    /// 工作目錄下的相對路徑必須是存在的檔案；有 `sha256` 時內容雜湊必須相符。
    File {
        path: String,
        #[serde(default)]
        sha256: Option<String>,
    },
    /// 這個 Task 必須有這個名稱的成果，最新版本的檔案與 manifest 一致；有 `sha256` 時必須等於它。
    Artifact {
        name: String,
        #[serde(default)]
        sha256: Option<String>,
    },
}

impl VerificationCheck {
    /// 給人看的一行描述（證據與 Worker 提示共用）。
    pub fn describe(&self) -> String {
        let hash = |sha: &Option<String>| sha.as_ref().map(|s| format!("，SHA-256 為 {s}")).unwrap_or_default();
        match self {
            VerificationCheck::Command { program, args, expected_exit_code } => {
                let line = std::iter::once(program.as_str()).chain(args.iter().map(String::as_str)).collect::<Vec<_>>();
                format!("命令「{}」的結束碼為 {expected_exit_code}", line.join(" "))
            }
            VerificationCheck::File { path, sha256 } => format!("檔案「{path}」存在{}", hash(sha256)),
            VerificationCheck::Artifact { name, sha256 } => format!("成果「{name}」已登記且檔案完整{}", hash(sha256)),
        }
    }

    fn normalized(self) -> Result<Self, String> {
        match self {
            VerificationCheck::Command { program, args, expected_exit_code } => {
                let program = program.trim().to_string();
                if program.is_empty() {
                    return Err("命令檢查必須指定要執行的程式".into());
                }
                Ok(VerificationCheck::Command { program, args, expected_exit_code })
            }
            VerificationCheck::File { path, sha256 } => {
                let path = relative_path(&path)
                    .ok_or_else(|| format!("檔案檢查的路徑「{path}」必須是工作目錄內的相對路徑（不可含「..」、磁碟代號或從根目錄開始）"))?;
                Ok(VerificationCheck::File { path, sha256: normalized_sha(sha256)? })
            }
            VerificationCheck::Artifact { name, sha256 } => {
                let name = ArtifactDraft::new(&name, "").map_err(|e| format!("成果檢查的名稱不正確：{e}"))?.name;
                Ok(VerificationCheck::Artifact { name, sha256: normalized_sha(sha256)? })
            }
        }
    }
}

/// 整理並驗證契約中的檢查清單（`ContractContent::normalized` 呼叫）；錯誤是給使用者看的原因。
pub fn normalize_checks(checks: Vec<VerificationCheck>) -> Result<Vec<VerificationCheck>, String> {
    if checks.len() > MAX_CHECKS {
        return Err(format!("確定性檢查最多 {MAX_CHECKS} 項"));
    }
    checks.into_iter().map(VerificationCheck::normalized).collect()
}

/// 工作目錄內的相對路徑，統一以 `/` 分隔；不可是絕對路徑、不可含磁碟代號（`:`）、`.`、`..` 或空的段落。
fn relative_path(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() || raw.starts_with(['/', '\\']) || raw.contains(':') || raw.chars().any(char::is_control) {
        return None;
    }
    let parts: Vec<&str> = raw.split(['/', '\\']).collect();
    if parts.iter().any(|p| p.is_empty() || *p == "." || *p == "..") {
        return None;
    }
    Some(parts.join("/"))
}

fn normalized_sha(sha: Option<String>) -> Result<Option<String>, String> {
    let Some(sha) = sha else { return Ok(None) };
    let sha = sha.trim().to_ascii_lowercase();
    if sha.len() != 64 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("「{sha}」不是 SHA-256（64 個十六進位字元）"));
    }
    Ok(Some(sha))
}

// ---------------------------------------------------------------- 檢查結果（證據）

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Passed,
    Failed,
}

/// 一項檢查的結果與證據。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckResult {
    pub check: VerificationCheck,
    pub status: CheckStatus,
    /// 證據：實際的結束碼與輸出結尾、實際雜湊、找不到的原因等。
    pub detail: String,
}

/// Runtime 實際觀察到的事實（Domain 不依賴 Port，由 Runtime 把 Port 的結果轉成這個型別）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Observation {
    /// 命令執行的結果。
    Command(CommandObservation),
    /// 檔案內容；`Ok(None)` 是不存在，`Err` 是讀不到的原因。
    File(Result<Option<Vec<u8>>, String>),
    /// 成果：manifest 中這個名稱的最新版本（沒有則 `None`）與它的檔案內容。
    Artifact { latest: Option<ManifestEntry>, content: Result<Option<Vec<u8>>, String> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandObservation {
    Exited { code: i32, output: String },
    /// 被強制結束而沒有結束碼。
    Terminated { output: String },
    TimedOut { output: String },
    /// 無法啟動（例如找不到程式）。
    NotStarted { reason: String },
}

/// 依觀察到的事實判定一項檢查。
pub fn judge(check: &VerificationCheck, observation: Observation) -> CheckResult {
    let (status, detail) = match (check, observation) {
        (VerificationCheck::Command { expected_exit_code, .. }, Observation::Command(observed)) => match observed {
            CommandObservation::Exited { code, output } => {
                let status = if code == *expected_exit_code { CheckStatus::Passed } else { CheckStatus::Failed };
                (status, with_output(format!("結束碼 {code}（預期 {expected_exit_code}）"), &output))
            }
            CommandObservation::Terminated { output } => {
                (CheckStatus::Failed, with_output("程序被終止，沒有結束碼".into(), &output))
            }
            CommandObservation::TimedOut { output } => (
                CheckStatus::Failed,
                with_output(format!("超過 {} 分鐘沒有結束，已終止", COMMAND_TIMEOUT_MS / 60_000), &output),
            ),
            CommandObservation::NotStarted { reason } => (CheckStatus::Failed, format!("無法執行：{reason}")),
        },
        (VerificationCheck::File { sha256, .. }, Observation::File(read)) => match read {
            Err(reason) => (CheckStatus::Failed, format!("無法讀取：{reason}")),
            Ok(None) => (CheckStatus::Failed, "檔案不存在".into()),
            Ok(Some(bytes)) => compare_hash(sha256.as_deref(), &artifact::sha256_hex(&bytes), bytes.len()),
        },
        (VerificationCheck::Artifact { sha256, .. }, Observation::Artifact { latest, content }) => match latest {
            None => (CheckStatus::Failed, "這個 Task 沒有登記這個名稱的成果".into()),
            Some(entry) => {
                let label = format!("版本 {}", entry.version);
                match content {
                    Err(reason) => (CheckStatus::Failed, format!("{label} 無法讀取：{reason}")),
                    Ok(None) => (CheckStatus::Failed, format!("{label} 的檔案遺失")),
                    Ok(Some(bytes)) => {
                        let actual = artifact::sha256_hex(&bytes);
                        if actual != entry.sha256 {
                            (CheckStatus::Failed, format!("{label} 的檔案與 manifest 不符（manifest {}，實際 {actual}）", entry.sha256))
                        } else {
                            let (status, detail) = compare_hash(sha256.as_deref(), &actual, bytes.len());
                            (status, format!("{label}：{detail}"))
                        }
                    }
                }
            }
        },
        (_, other) => (CheckStatus::Failed, format!("觀察結果與檢查種類不符：{other:?}")),
    };
    CheckResult { check: check.clone(), status, detail: tail_chars(&detail, DETAIL_MAX_CHARS) }
}

fn compare_hash(expected: Option<&str>, actual: &str, size: usize) -> (CheckStatus, String) {
    match expected {
        Some(expected) if expected != actual => {
            (CheckStatus::Failed, format!("SHA-256 不符（預期 {expected}，實際 {actual}）"))
        }
        _ => (CheckStatus::Passed, format!("存在，{size} 位元組，SHA-256 {actual}")),
    }
}

fn with_output(summary: String, output: &str) -> String {
    let output = output.trim_end();
    if output.is_empty() {
        summary
    } else {
        format!("{summary}\n輸出結尾：\n{output}")
    }
}

/// 保留結尾的 `max` 個字元（輸出的結尾通常才是失敗原因）。
fn tail_chars(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_string();
    }
    let tail: String = text.chars().skip(count - max).collect();
    format!("…{tail}")
}

// ---------------------------------------------------------------- manifest 與接受的基礎

/// 驗證時看到的成果 manifest 中的一項：某個成果的最新版本。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestEntry {
    pub artifact_id: String,
    pub name: String,
    pub version: i64,
    pub sha256: String,
}

/// 接受請求引用的成果版本（使用者看到、Runtime 檢查過的那一版）。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ManifestRef {
    pub artifact_id: String,
    pub version: i64,
}

/// 一個 Task 目前的成果 manifest：每個成果的最新版本，依成果識別排序。
pub fn manifest_of(artifacts: &[Artifact], task_id: &str) -> Vec<ManifestEntry> {
    let mut entries: Vec<ManifestEntry> = artifacts
        .iter()
        .filter(|a| a.task_id == task_id)
        .filter_map(|a| {
            a.latest_version().map(|v| ManifestEntry {
                artifact_id: a.id.clone(),
                name: a.name.clone(),
                version: v.version,
                sha256: v.sha256.clone(),
            })
        })
        .collect();
    entries.sort_by(|a, b| a.artifact_id.cmp(&b.artifact_id));
    entries
}

fn refs_of(entries: &[ManifestEntry]) -> Vec<ManifestRef> {
    entries.iter().map(|e| ManifestRef { artifact_id: e.artifact_id.clone(), version: e.version }).collect()
}

/// 成果接受的基礎：觀察（檢查或使用者檢視）當時的受驗 Run、世代、契約版本與成果 manifest。
/// 接受交易提交時三者都必須仍與目前狀態相同。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcceptanceBasis {
    pub run_id: String,
    pub generation: i64,
    pub contract_version: i64,
    pub artifacts: Vec<ManifestRef>,
}

/// 受驗的 Run：這個 Task 世代最大的 Run，而且它已經成功結束。其他情況（還在執行、失敗）沒有可驗證的對象。
pub fn subject_run<'a>(runs: &'a [Run], task_id: &str) -> Option<&'a Run> {
    runs.iter().filter(|r| r.task_id == task_id).max_by_key(|r| r.generation).filter(|r| r.status == RunStatus::Succeeded)
}

/// 目前的接受基礎；Task 不是 `verifying`、沒有受驗 Run，或受驗 Run 已經排入整合（已接受過）時為 `None`。
pub fn current_basis(
    task: &Task,
    runs: &[Run],
    artifacts: &[Artifact],
    integrations: &[IntegrationEntry],
) -> Option<AcceptanceBasis> {
    if task.status != TaskStatus::Verifying {
        return None;
    }
    let run = subject_run(runs, &task.id)?;
    if integration::has_entry_for_run(integrations, &run.id) {
        return None;
    }
    Some(AcceptanceBasis {
        run_id: run.id.clone(),
        generation: run.generation,
        contract_version: task.contract_version,
        artifacts: refs_of(&manifest_of(artifacts, &task.id)),
    })
}

/// 需要 Runtime 執行確定性檢查的 Task。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingCheck {
    pub task_id: String,
    pub basis: AcceptanceBasis,
    /// 要執行的檢查；契約已在 Run 派工後變更時是空的（那種情況直接判定未通過）。
    pub checks: Vec<VerificationCheck>,
    pub manifest: Vec<ManifestEntry>,
}

/// 這個 Task 是否需要 Runtime 執行檢查：`verifying`、有受驗 Run，而且契約宣告了檢查，或契約版本已與 Run 不同。
/// 沒有宣告任何檢查時回傳 `None`：只能由使用者手動接受。Workspace 已封存時也是 `None`：提交一定被拒絕。
/// 受驗 Run 已經排入整合（開發 Task 已通過）時也是 `None`：接下來由整合佇列處理。
pub fn pending_check(
    ctx: &TaskContext<'_>,
    runs: &[Run],
    artifacts: &[Artifact],
    integrations: &[IntegrationEntry],
    task: &Task,
) -> Option<PendingCheck> {
    if ctx.workspace.is_none_or(|w| w.status == WorkspaceStatus::Archived) {
        return None;
    }
    let basis = current_basis(task, runs, artifacts, integrations)?;
    let run = subject_run(runs, &task.id)?;
    let contract = ctx.contract_of(task)?;
    let mismatch = run.contract_version != task.contract_version;
    let checks = &contract.content.verification_checks;
    if !mismatch && checks.is_empty() {
        return None;
    }
    Some(PendingCheck {
        task_id: task.id.clone(),
        basis,
        checks: if mismatch { Vec::new() } else { checks.clone() },
        manifest: manifest_of(artifacts, &task.id),
    })
}

// ---------------------------------------------------------------- 驗證紀錄

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationMethod {
    /// Runtime 執行契約宣告的確定性檢查。
    Checks,
    /// 使用者以桌面憑證手動接受。
    Manual,
}

impl VerificationMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            VerificationMethod::Checks => "checks",
            VerificationMethod::Manual => "manual",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        [VerificationMethod::Checks, VerificationMethod::Manual].into_iter().find(|m| m.as_str() == raw)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationOutcome {
    Passed,
    Failed,
}

impl VerificationOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            VerificationOutcome::Passed => "passed",
            VerificationOutcome::Failed => "failed",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        [VerificationOutcome::Passed, VerificationOutcome::Failed].into_iter().find(|m| m.as_str() == raw)
    }
}

/// 一次驗證的結論與證據；追加式保存，不會被修改。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerificationRecord {
    pub id: String,
    pub workspace_id: String,
    pub task_id: String,
    /// 受驗的 Run 與它的世代。
    pub run_id: String,
    pub generation: i64,
    pub contract_version: i64,
    pub method: VerificationMethod,
    pub outcome: VerificationOutcome,
    /// 每項檢查的結果（手動接受時是空的）。
    pub checks: Vec<CheckResult>,
    /// 驗證時的成果 manifest。
    pub artifacts: Vec<ManifestEntry>,
    /// 手動接受者：桌面憑證的指紋（憑證雜湊的前 16 個字元，不是憑證本身）。
    pub accepted_by: Option<String>,
    /// 手動接受時使用者的附註；未通過時是原因摘要。
    pub note: Option<String>,
    pub decided_at: Timestamp,
}

// ---------------------------------------------------------------- 成果接受交易

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcceptanceCommand {
    /// Runtime 執行完確定性檢查，提交結果（`results` 依契約宣告的順序，一項對一項）。
    RecordChecks { task_id: String, basis: AcceptanceBasis, results: Vec<CheckResult> },
    /// 使用者以桌面憑證手動接受；`accepted_by` 由 API 從已驗證的憑證取得，請求不能指定。
    AcceptManually { task_id: String, basis: AcceptanceBasis, accepted_by: String, note: Option<String> },
}

/// 判定需要的目前狀態（同一個 Workspace 內），Store 在交易中載入。
pub struct AcceptanceContext<'a> {
    pub task: &'a TaskContext<'a>,
    pub runs: &'a [Run],
    pub artifacts: &'a [Artifact],
    /// 平台登記的工作目錄：開發 Task 的整合項目取它的平台分支與整合目標（票 20）。
    pub workdirs: &'a [Workdir],
    /// Workspace 的整合項目：受驗 Run 已排入整合時不能再接受。
    pub integrations: &'a [IntegrationEntry],
}

/// 接受交易的結果：Task 的新狀態、驗證紀錄與事件，在同一個交易中寫入。
#[derive(Debug, Clone, PartialEq)]
pub struct AcceptanceChange {
    pub task: Task,
    pub record: VerificationRecord,
    /// 開發 Task 通過時排入整合佇列的項目（票 20）；此時 Task 仍是 `verifying`。
    pub integration: Option<IntegrationEntry>,
    pub events: Vec<DomainEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AcceptanceError {
    #[error("找不到 Workspace {0}")]
    WorkspaceNotFound(String),
    #[error("Workspace「{0}」已經封存，不能再接受成果")]
    WorkspaceArchived(String),
    #[error("找不到 Task {0}")]
    TaskNotFound(String),
    #[error("Task 目前是 {} 狀態；只有驗證中（verifying）的 Task 可以接受成果", .0.as_str())]
    TaskNotVerifying(TaskStatus),
    #[error("這份接受依據的是 Run {expected}，但目前受驗的是 {}；過期 Run 的成果不能推進 Task", .current.as_deref().unwrap_or("（沒有）"))]
    RunStale { expected: String, current: Option<String> },
    #[error("契約已經變更：接受依據的是版本 {expected}，Run 依版本 {run} 執行，目前是版本 {current}")]
    ContractStale { expected: i64, run: i64, current: i64 },
    #[error("成果 manifest 已經和檢視當時不同，請重新檢視後再接受")]
    ManifestStale,
    #[error("檢查結果與契約宣告的檢查不一致")]
    ResultsMismatch,
    #[error("附註最多 {NOTE_MAX_CHARS} 個字元")]
    NoteTooLong,
    #[error("這個 Run 的成果已經接受並排入整合佇列，不能再接受一次")]
    AlreadyQueuedForIntegration,
    #[error("找不到這個開發 Task 登記的 worktree，無法排入整合佇列")]
    WorktreeMissing,
}

impl AcceptanceError {
    /// 穩定的機器可讀代碼，API 與 UI 依它判斷錯誤種類。
    pub fn code(&self) -> &'static str {
        match self {
            AcceptanceError::WorkspaceNotFound(_) => "workspace_not_found",
            AcceptanceError::WorkspaceArchived(_) => "workspace_archived",
            AcceptanceError::TaskNotFound(_) => "task_not_found",
            AcceptanceError::TaskNotVerifying(_) => "task_not_verifying",
            AcceptanceError::RunStale { .. } => "acceptance_run_stale",
            AcceptanceError::ContractStale { .. } => "acceptance_contract_stale",
            AcceptanceError::ManifestStale => "acceptance_manifest_stale",
            AcceptanceError::ResultsMismatch => "acceptance_results_mismatch",
            AcceptanceError::NoteTooLong => "acceptance_invalid",
            AcceptanceError::AlreadyQueuedForIntegration => "acceptance_already_queued",
            AcceptanceError::WorktreeMissing => "acceptance_worktree_missing",
        }
    }
}

/// 套用一個接受命令。`record_id` 由呼叫端產生（Domain 不產生亂數）。
pub fn decide(
    ctx: &AcceptanceContext<'_>,
    command: AcceptanceCommand,
    record_id: String,
    now: Timestamp,
) -> Result<AcceptanceChange, AcceptanceError> {
    let workspace = ctx.task.workspace.ok_or_else(|| AcceptanceError::WorkspaceNotFound(ctx.task.workspace_id.into()))?;
    if workspace.status == WorkspaceStatus::Archived {
        return Err(AcceptanceError::WorkspaceArchived(workspace.name.clone()));
    }
    let (task_id, basis) = match &command {
        AcceptanceCommand::RecordChecks { task_id, basis, .. } | AcceptanceCommand::AcceptManually { task_id, basis, .. } => {
            (task_id.clone(), basis.clone())
        }
    };
    let task = ctx.task.task(&task_id).ok_or_else(|| AcceptanceError::TaskNotFound(task_id.clone()))?;
    if task.status != TaskStatus::Verifying {
        return Err(AcceptanceError::TaskNotVerifying(task.status));
    }

    // 校驗一：受驗的 Run 仍是這個 Task 最新的世代
    let run = subject_run(ctx.runs, &task.id)
        .filter(|r| r.id == basis.run_id && r.generation == basis.generation)
        .ok_or_else(|| AcceptanceError::RunStale {
            expected: basis.run_id.clone(),
            current: subject_run(ctx.runs, &task.id).map(|r| r.id.clone()),
        })?;
    if integration::has_entry_for_run(ctx.integrations, &run.id) {
        return Err(AcceptanceError::AlreadyQueuedForIntegration);
    }
    // 校驗二：契約版本與觀察當時相同
    if basis.contract_version != task.contract_version {
        return Err(AcceptanceError::ContractStale {
            expected: basis.contract_version,
            run: run.contract_version,
            current: task.contract_version,
        });
    }
    // 校驗三：成果 manifest 與觀察當時相同
    let manifest = manifest_of(ctx.artifacts, &task.id);
    let mut expected_refs = basis.artifacts.clone();
    expected_refs.sort();
    if expected_refs != refs_of(&manifest) {
        return Err(AcceptanceError::ManifestStale);
    }
    let contract_matches_run = run.contract_version == task.contract_version;

    let (method, outcome, checks, accepted_by, note) = match command {
        AcceptanceCommand::AcceptManually { accepted_by, note, .. } => {
            // Run 依舊版契約執行的成果，不能在新契約下被接受
            if !contract_matches_run {
                return Err(AcceptanceError::ContractStale {
                    expected: basis.contract_version,
                    run: run.contract_version,
                    current: task.contract_version,
                });
            }
            let note = note.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
            if note.as_ref().is_some_and(|n| n.chars().count() > NOTE_MAX_CHARS) {
                return Err(AcceptanceError::NoteTooLong);
            }
            (VerificationMethod::Manual, VerificationOutcome::Passed, Vec::new(), Some(accepted_by), note)
        }
        AcceptanceCommand::RecordChecks { results, .. } => {
            if !contract_matches_run {
                let note = format!(
                    "契約在 Run 派工後由版本 {} 變更為 {}，這個 Run 的成果不能在新契約下被接受，需要重新執行",
                    run.contract_version, task.contract_version
                );
                (VerificationMethod::Checks, VerificationOutcome::Failed, Vec::new(), None, Some(note))
            } else {
                let declared = ctx.task.contract_of(task).map(|c| c.content.verification_checks.as_slice()).unwrap_or(&[]);
                let matches = !declared.is_empty()
                    && declared.len() == results.len()
                    && declared.iter().zip(&results).all(|(d, r)| *d == r.check);
                if !matches {
                    return Err(AcceptanceError::ResultsMismatch);
                }
                let failed = results.iter().filter(|r| r.status == CheckStatus::Failed).count();
                if failed == 0 {
                    (VerificationMethod::Checks, VerificationOutcome::Passed, results, None, None)
                } else {
                    let note = format!("{} 項確定性檢查中有 {failed} 項未通過", results.len());
                    (VerificationMethod::Checks, VerificationOutcome::Failed, results, None, Some(note))
                }
            }
        }
    };

    let record = VerificationRecord {
        id: record_id,
        workspace_id: task.workspace_id.clone(),
        task_id: task.id.clone(),
        run_id: run.id.clone(),
        generation: run.generation,
        contract_version: task.contract_version,
        method,
        outcome,
        checks,
        artifacts: manifest,
        accepted_by,
        note,
        decided_at: now,
    };
    let mut moved = task.clone();
    let mut versioned = Versioned { task: &mut moved, events: Vec::new(), now };
    versioned.push(
        EVENT_TASK_VERIFICATION_RECORDED,
        serde_json::json!({
            "verification_id": record.id,
            "method": record.method,
            "outcome": record.outcome,
            "run_id": record.run_id,
            "generation": record.generation,
            "contract_version": record.contract_version,
            "artifacts": record.artifacts,
            "accepted_by": record.accepted_by,
            "failed_checks": record.checks.iter().filter(|c| c.status == CheckStatus::Failed).map(|c| c.check.describe()).collect::<Vec<_>>(),
            "note": record.note,
        }),
    );
    // 通過才完成；未通過回到 ready（可修正、可重新派工），證據留在驗證紀錄。
    // 開發 Task 通過時排入整合佇列、停在 verifying：未通過整合不能宣告完成（票 20）
    let mut queued = None;
    let transition = match outcome {
        VerificationOutcome::Passed if task.kind == TaskKind::Development => {
            let worktree = ctx
                .workdirs
                .iter()
                .find(|w| w.task_id == task.id && w.kind == WorkdirKind::Worktree && w.is_ready())
                .ok_or(AcceptanceError::WorktreeMissing)?;
            let (Some(branch), Some(target)) = (worktree.branch.as_deref(), worktree.base_branch.as_deref()) else {
                return Err(AcceptanceError::WorktreeMissing);
            };
            let id = format!("int_{}", record.id);
            let (entry, event) = integration::enqueue(id, versioned.task, &run.id, run.generation, branch, target, now);
            versioned.events.push(event);
            queued = Some(entry);
            None
        }
        VerificationOutcome::Passed => Some((TaskStatus::Completed, "verification_passed")),
        VerificationOutcome::Failed => Some((TaskStatus::Ready, "verification_failed")),
    };
    if let Some((to, reason)) = transition {
        versioned.change_status(to, reason).map_err(|_| AcceptanceError::TaskNotVerifying(task.status))?;
    }
    let events = versioned.events;
    Ok(AcceptanceChange { task: moved, record, integration: queued, events })
}

// ---------------------------------------------------------------- 檢視

/// 一個 Task 的驗證狀況：目前可以接受的基礎、宣告的檢查、成果 manifest 與歷次驗證紀錄。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerificationView {
    pub task_id: String,
    pub status: TaskStatus,
    /// Task 是 `verifying` 且有受驗 Run 時才有；手動接受必須原樣送回。
    pub basis: Option<AcceptanceBasis>,
    /// 目前契約宣告的檢查。
    pub checks: Vec<VerificationCheck>,
    /// 目前的成果 manifest（每個成果的最新版本）。
    pub artifacts: Vec<ManifestEntry>,
    /// 驗證中時 Runtime 會不會自動執行檢查；`false` 表示只能由使用者手動接受。
    pub automatic: bool,
    /// 這個 Task 的歷次驗證紀錄，由舊到新。
    pub records: Vec<VerificationRecord>,
    /// 這個 Task 最新的整合項目（開發 Task 通過驗證後才有，票 20）。
    pub integration: Option<IntegrationEntry>,
}

pub fn view(
    ctx: &TaskContext<'_>,
    runs: &[Run],
    artifacts: &[Artifact],
    records: &[VerificationRecord],
    integrations: &[IntegrationEntry],
    task: &Task,
) -> VerificationView {
    VerificationView {
        task_id: task.id.clone(),
        status: task.status,
        basis: current_basis(task, runs, artifacts, integrations),
        checks: ctx.contract_of(task).map(|c| c.content.verification_checks.clone()).unwrap_or_default(),
        artifacts: manifest_of(artifacts, &task.id),
        automatic: pending_check(ctx, runs, artifacts, integrations, task).is_some(),
        records: records.iter().filter(|r| r.task_id == task.id).cloned().collect(),
        integration: integrations.iter().filter(|e| e.task_id == task.id).max_by_key(|e| e.seq).cloned(),
    }
}

#[cfg(test)]
mod tests;
