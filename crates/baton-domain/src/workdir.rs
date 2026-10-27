//! Task 的工作目錄（票 20）：開發 Task 在平台建立並登記的獨立 Git worktree 中執行，
//! 研究與文件 Task 在獨立的輸出目錄中執行。
//!
//! 純邏輯、無 I/O。幾條貫穿全檔的規則：
//! - **目錄在派工之前準備好**：Runtime 在交易外經由 WorkspaceFs 建立目錄，成功或失敗都登記下來；
//!   派工只認「已登記且可用」的目錄（`run::check_dispatch`），不可用時不派工，等待原因就是登記的失敗原因。
//! - **一個 Task 一個目錄，跨 Run 沿用**：同一個 Task 的後續 Run 回到同一個目錄（原生 Session 以工作目錄
//!   區分專案，換目錄就接不回原 Session）；不同 Task 的目錄一定不同。
//! - **位置固定在 Workspace 目錄內的 `.baton`**：Workspace 授權的目標是 Workspace 目錄，
//!   放在裡面才不會讓 Worker 的每一次編輯都落在授權範圍外；`.baton` 內放一份忽略全部內容的 `.gitignore`，
//!   不會讓主 checkout 出現未追蹤的檔案。

use serde::{Deserialize, Serialize};

use crate::event::DomainEvent;
use crate::task::Task;
use crate::workspace::Workspace;
use crate::Timestamp;

pub const AGGREGATE_TYPE: &str = "task_workdir";
pub const EVENT_PREPARED: &str = "task_workdir_prepared";
pub const EVENT_FAILED: &str = "task_workdir_failed";

/// Workspace 目錄內，平台專用的目錄名稱。
pub const BATON_DIR: &str = ".baton";
/// `.baton` 內的 `.gitignore` 檔名與內容：忽略整個 `.baton`（含它自己）。
pub const GITIGNORE_NAME: &str = ".gitignore";
pub const GITIGNORE_CONTENT: &str = "# Baton 的工作目錄（worktree、輸出目錄、整合目錄），不納入版本控制\n*\n";
/// 開發 Task 的 worktree 放在 `.baton\worktrees\<task_id>`。
pub const WORKTREES_DIR: &str = "worktrees";
/// 研究與文件 Task 的輸出目錄放在 `.baton\outputs\<task_id>`。
pub const OUTPUTS_DIR: &str = "outputs";
/// 平台的分支名稱前綴：`baton/<task_id>`。
pub const BRANCH_PREFIX: &str = "baton/";

/// 準備失敗後，自動重試的最短間隔（使用者再次要求派工時立即重試）。
pub const PREPARE_RETRY_MS: i64 = 60_000;

// ---------------------------------------------------------------- Task 種類

/// Task 的種類：決定它在哪一種工作目錄中執行。建立時決定，之後不變。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    /// 程式開發：在平台建立並登記的獨立 worktree 中執行，成果經整合佇列合併。
    Development,
    /// 研究（預設）：在獨立輸出目錄中執行。
    #[default]
    Research,
    /// 文件：在獨立輸出目錄中執行。
    Document,
}

impl TaskKind {
    pub const ALL: [TaskKind; 3] = [TaskKind::Development, TaskKind::Research, TaskKind::Document];

    pub fn as_str(self) -> &'static str {
        match self {
            TaskKind::Development => "development",
            TaskKind::Research => "research",
            TaskKind::Document => "document",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == raw)
    }

    pub fn label(self) -> &'static str {
        match self {
            TaskKind::Development => "程式開發",
            TaskKind::Research => "研究",
            TaskKind::Document => "文件",
        }
    }

    /// 這種 Task 需要的工作目錄。
    pub fn workdir_kind(self) -> WorkdirKind {
        match self {
            TaskKind::Development => WorkdirKind::Worktree,
            TaskKind::Research | TaskKind::Document => WorkdirKind::OutputDir,
        }
    }
}

// ---------------------------------------------------------------- 登記

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkdirKind {
    Worktree,
    OutputDir,
}

impl WorkdirKind {
    pub fn as_str(self) -> &'static str {
        match self {
            WorkdirKind::Worktree => "worktree",
            WorkdirKind::OutputDir => "output_dir",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        [WorkdirKind::Worktree, WorkdirKind::OutputDir].into_iter().find(|k| k.as_str() == raw)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkdirStatus {
    /// 已建立，可以派工。
    Ready,
    /// 最近一次準備失敗；`failure` 是原因，派工會等待並顯示它。
    Failed,
}

impl WorkdirStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            WorkdirStatus::Ready => "ready",
            WorkdirStatus::Failed => "failed",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        [WorkdirStatus::Ready, WorkdirStatus::Failed].into_iter().find(|k| k.as_str() == raw)
    }
}

/// 平台登記的一個 Task 工作目錄。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workdir {
    pub task_id: String,
    pub workspace_id: String,
    pub kind: WorkdirKind,
    /// 絕對路徑（Windows 形式）。
    pub path: String,
    /// worktree 簽出的平台分支；輸出目錄是 `None`。
    pub branch: Option<String>,
    /// 整合目標：建立 worktree 當時 Workspace 簽出的分支；輸出目錄是 `None`。
    pub base_branch: Option<String>,
    pub status: WorkdirStatus,
    /// 最近一次準備失敗的原因（給使用者看）。
    pub failure: Option<String>,
    /// 每次登記（成功或失敗）加一；事件的 `aggregate_version` 與它對應。
    pub version: i64,
    /// 最近一次嘗試準備的時間。
    pub attempted_at: Timestamp,
    /// 第一次成功建立的時間。
    pub ready_at: Option<Timestamp>,
}

impl Workdir {
    pub fn is_ready(&self) -> bool {
        self.status == WorkdirStatus::Ready
    }
}

/// 面板與 API 看到的工作目錄：登記內容加上曾在其中執行的 Run（依建立順序）與 worktree 的清理狀態（票 21）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkdirView {
    #[serde(flatten)]
    pub workdir: Workdir,
    pub run_ids: Vec<String>,
    /// 清理登記；還沒有登記（Task 尚未完成並整合，或是輸出目錄）時為 `None`。
    #[serde(default)]
    pub cleanup: Option<crate::cleanup::WorktreeCleanup>,
}

// ---------------------------------------------------------------- 計畫

/// 要為一個 Task 準備的目錄。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkdirPlan {
    pub task_id: String,
    pub workspace_id: String,
    pub kind: WorkdirKind,
    pub path: String,
    pub branch: Option<String>,
    /// Workspace 目錄（worktree 的來源 repo）。
    pub repo: String,
    /// `.baton\.gitignore` 的路徑。
    pub gitignore: String,
}

/// 依 Task 種類決定目錄位置與分支。
pub fn plan(workspace: &Workspace, task: &Task) -> WorkdirPlan {
    let kind = task.kind.workdir_kind();
    let sub = match kind {
        WorkdirKind::Worktree => WORKTREES_DIR,
        WorkdirKind::OutputDir => OUTPUTS_DIR,
    };
    WorkdirPlan {
        task_id: task.id.clone(),
        workspace_id: workspace.id.clone(),
        kind,
        path: join(&workspace.path, &[BATON_DIR, sub, &task.id]),
        branch: (kind == WorkdirKind::Worktree).then(|| branch_name(&task.id)),
        repo: workspace.path.clone(),
        gitignore: join(&workspace.path, &[BATON_DIR, GITIGNORE_NAME]),
    }
}

/// 平台分支名稱。
pub fn branch_name(task_id: &str) -> String {
    format!("{BRANCH_PREFIX}{task_id}")
}

/// 以反斜線連接（Workspace 路徑已正規化為反斜線、沒有結尾分隔線，磁碟根目錄除外）。
pub(crate) fn join(base: &str, parts: &[&str]) -> String {
    let mut path = base.trim_end_matches('\\').to_string();
    for part in parts {
        path.push('\\');
        path.push_str(part);
    }
    path
}

/// 現在要不要為這個 Task 準備目錄：還沒有登記、或最近一次失敗且已過重試間隔（`explicit` 為使用者剛要求派工，立即重試）。
/// 已可用的目錄不再重做。
pub fn needs_preparation(existing: Option<&Workdir>, now: Timestamp, explicit: bool) -> bool {
    match existing {
        None => true,
        Some(w) if w.is_ready() => false,
        Some(w) => explicit || now.as_millis() - w.attempted_at.as_millis() >= PREPARE_RETRY_MS,
    }
}

/// 準備失敗的原因（給使用者看的文字由這裡統一產生）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrepareFailure {
    /// 開發 Task 的 Workspace 不是 Git repo。
    NotGitRepository,
    /// Workspace 目前不在任何分支上，無法決定整合目標。
    DetachedHead,
    /// 建立目錄或 worktree 時，Port 回報的錯誤。
    Io(String),
}

impl PrepareFailure {
    pub fn message(&self, kind: WorkdirKind) -> String {
        match self {
            PrepareFailure::NotGitRepository => {
                "這個 Workspace 不是 Git repo；程式開發 Task 需要在獨立的 worktree 中執行，請改用研究或文件 Task，或把 Workspace 目錄初始化為 Git repo".into()
            }
            PrepareFailure::DetachedHead => {
                "Workspace 目前不在任何分支上（detached HEAD），無法決定整合目標；請在 Workspace 簽出要整合的分支".into()
            }
            PrepareFailure::Io(reason) => match kind {
                WorkdirKind::Worktree => format!("無法建立 worktree：{reason}"),
                WorkdirKind::OutputDir => format!("無法建立輸出目錄：{reason}"),
            },
        }
    }
}

/// 登記準備成功：第一次成功記下 `ready_at`。`base_branch` 是 worktree 的整合目標（輸出目錄為 `None`）。
pub fn record_prepared(
    existing: Option<&Workdir>,
    plan: &WorkdirPlan,
    base_branch: Option<String>,
    now: Timestamp,
) -> (Workdir, DomainEvent) {
    let workdir = Workdir {
        task_id: plan.task_id.clone(),
        workspace_id: plan.workspace_id.clone(),
        kind: plan.kind,
        path: plan.path.clone(),
        branch: plan.branch.clone(),
        base_branch,
        status: WorkdirStatus::Ready,
        failure: None,
        version: existing.map_or(0, |w| w.version) + 1,
        attempted_at: now,
        ready_at: Some(now),
    };
    let event = event(&workdir, EVENT_PREPARED, now);
    (workdir, event)
}

/// 登記準備失敗。
pub fn record_failed(
    existing: Option<&Workdir>,
    plan: &WorkdirPlan,
    failure: &PrepareFailure,
    now: Timestamp,
) -> (Workdir, DomainEvent) {
    let workdir = Workdir {
        task_id: plan.task_id.clone(),
        workspace_id: plan.workspace_id.clone(),
        kind: plan.kind,
        path: plan.path.clone(),
        branch: plan.branch.clone(),
        base_branch: None,
        status: WorkdirStatus::Failed,
        failure: Some(failure.message(plan.kind)),
        version: existing.map_or(0, |w| w.version) + 1,
        attempted_at: now,
        ready_at: None,
    };
    let event = event(&workdir, EVENT_FAILED, now);
    (workdir, event)
}

fn event(workdir: &Workdir, event_type: &'static str, now: Timestamp) -> DomainEvent {
    DomainEvent {
        aggregate_type: AGGREGATE_TYPE,
        aggregate_id: workdir.task_id.clone(),
        aggregate_version: workdir.version,
        event_type,
        occurred_at: now,
        payload: serde_json::json!({
            "kind": workdir.kind,
            "path": workdir.path,
            "branch": workdir.branch,
            "base_branch": workdir.base_branch,
            "failure": workdir.failure,
        }),
    }
}

#[cfg(test)]
mod tests;
