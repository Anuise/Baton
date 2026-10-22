//! Goal、Task、契約版本與相依的持久化。
//!
//! 規則都在 Domain（`baton_domain::task`）；這裡只做「在同一個交易中載入目前狀態 → 交給 Domain 判定 →
//! 寫入新狀態、契約版本、相依與事件」，以及唯讀的看板投影。範圍一律以 Workspace 為界。

use baton_domain::authorization::AuthorizationRevision;
use baton_domain::agent::ReportRecord;
use baton_domain::budget::{self, BudgetView, TaskBudget};
use baton_domain::guard::ActiveGuard;
use baton_domain::run::{ExecutionLease, Run, RunView};
use baton_domain::integration::IntegrationEntry;
use baton_domain::verification::VerificationRecord;
use baton_domain::workdir::{TaskKind, Workdir, WorkdirView};
use baton_domain::task::{
    self, ContractContent, Dependency, Goal, MissingItem, Task, TaskChange, TaskCommand, TaskContext, TaskContract,
    TaskError, TaskStatus, TaskView,
};

use super::*;

/// 套用 Task 命令或查詢的失敗：Domain 拒絕（使用者可修正）或儲存層錯誤。
#[derive(Debug, thiserror::Error)]
pub enum TaskApplyError {
    #[error(transparent)]
    Rejected(#[from] TaskError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl From<rusqlite::Error> for TaskApplyError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error.into())
    }
}

/// 工作面板的資料：全部 Goal 與每個 Task 的投影，依建立順序，外加這個 Workspace 的全部 Run。
#[derive(Debug, Clone, PartialEq)]
pub struct TaskBoardData {
    pub goals: Vec<Goal>,
    pub tasks: Vec<TaskView>,
    pub runs: Vec<RunView>,
    /// 這個 Workspace 的全部驗證紀錄，由舊到新（票 13）。
    pub verifications: Vec<VerificationRecord>,
    /// 平台登記的工作目錄與曾在其中執行的 Run（票 20）。
    pub workdirs: Vec<WorkdirView>,
    /// 這個 Workspace 的整合項目，依排入順序（票 20）。
    pub integrations: Vec<IntegrationEntry>,
    /// 每個 Task 的修正與重試預算，依 Task 的建立順序（票 36）。
    pub budgets: Vec<BudgetView>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TaskDetailData {
    pub task: TaskView,
    /// 契約的全部版本，由舊到新（含目前版本）。
    pub contract_history: Vec<TaskContract>,
    /// 修正與重試預算的三種計數（票 36）。
    pub budget: BudgetView,
}

/// 一個 Workspace 範圍內、Domain 判定需要的全部狀態。
pub(crate) struct Loaded {
    pub(crate) workspace: Option<Workspace>,
    pub(crate) authorization: Option<AuthorizationRevision>,
    pub(crate) goals: Vec<Goal>,
    pub(crate) tasks: Vec<Task>,
    pub(crate) dependencies: Vec<Dependency>,
    pub(crate) contracts: Vec<TaskContract>,
    pub(crate) runs: Vec<Run>,
    pub(crate) leases: Vec<ExecutionLease>,
    /// 每個 Run 每種回報最新一筆被接受的回報（給面板顯示用）。
    pub(crate) reports: Vec<ReportRecord>,
    /// 平台登記的工作目錄（票 20）。
    pub(crate) workdirs: Vec<Workdir>,
    /// 這個 Workspace 的整合項目，依排入順序（票 20）。
    pub(crate) integrations: Vec<IntegrationEntry>,
    /// Run 准入時要檢查的 Guard；只有派工路徑以 `guards::load_active_guards` 填入，其他路徑是空的（票 38）。
    pub(crate) guards: Vec<ActiveGuard>,
    /// 有計入過的 Task 預算（票 36）。
    pub(crate) budgets: Vec<TaskBudget>,
}

impl Loaded {
    pub(crate) fn ctx<'a>(&'a self, workspace_id: &'a str) -> TaskContext<'a> {
        TaskContext {
            workspace_id,
            workspace: self.workspace.as_ref(),
            authorization: self.authorization.as_ref(),
            goals: &self.goals,
            tasks: &self.tasks,
            dependencies: &self.dependencies,
            contracts: &self.contracts,
        }
    }
}

impl Store {
    /// 套用一個 Goal／Task 命令：Domain 拒絕或沒有任何變動（冪等）時不寫入任何東西。
    pub async fn apply_task_command(
        &self,
        workspace_id: String,
        command: TaskCommand,
        now: Timestamp,
    ) -> Result<TaskChange, TaskApplyError> {
        let outcome = self
            .write(move |tx| {
                let loaded = load(tx, &workspace_id)?;
                let cancelled = match &command {
                    TaskCommand::Cancel { task_id } => Some(task_id.clone()),
                    _ => None,
                };
                match task::decide(&loaded.ctx(&workspace_id), command, now) {
                    Err(rejected) => Ok(Err(rejected)),
                    Ok(change) => {
                        save(tx, &change)?;
                        // 取消 Task 時，它還在進行的 Run 在同一個交易中一起被要求取消（不留下沒有主人的寫入者）
                        if let (Some(task_id), Some(task)) = (cancelled, &change.task) {
                            super::runs::cascade_cancel(tx, &loaded, task, &task_id, now)?;
                        }
                        Ok(Ok(change))
                    }
                }
            })
            .await?;
        Ok(outcome?)
    }

    /// 工作面板：Workspace 內全部 Goal 與 Task（pending 的 Task 附完整准入檢查的缺項）。
    pub async fn task_board(&self, workspace_id: &str) -> Result<TaskBoardData, TaskApplyError> {
        let workspace_id = workspace_id.to_string();
        let result = self
            .read(move |conn| {
                let loaded = load(conn, &workspace_id)?;
                if loaded.workspace.is_none() {
                    return Ok(Err(TaskError::WorkspaceNotFound(workspace_id)));
                }
                let tasks = task::board(&loaded.ctx(&workspace_id));
                let runs = super::runs::views(&loaded);
                let verifications = super::verification::load_verifications(conn, &workspace_id)?;
                let workdirs = super::workdirs::views(conn, &loaded.workdirs)?;
                let budgets = loaded.tasks.iter().map(|t| budget::view(&loaded.budgets, t)).collect();
                Ok(Ok(TaskBoardData {
                    goals: loaded.goals,
                    tasks,
                    runs,
                    verifications,
                    workdirs,
                    integrations: loaded.integrations,
                    budgets,
                }))
            })
            .await?;
        Ok(result?)
    }

    /// 單一 Task 的投影與契約版本歷史。
    pub async fn task_detail(&self, workspace_id: &str, task_id: &str) -> Result<TaskDetailData, TaskApplyError> {
        let (workspace_id, task_id) = (workspace_id.to_string(), task_id.to_string());
        let result = self
            .read(move |conn| {
                let loaded = load(conn, &workspace_id)?;
                if loaded.workspace.is_none() {
                    return Ok(Err(TaskError::WorkspaceNotFound(workspace_id)));
                }
                let ctx = loaded.ctx(&workspace_id);
                let Some(view) = loaded.tasks.iter().find(|t| t.id == task_id).and_then(|t| task::view_task(&ctx, t))
                else {
                    return Ok(Err(TaskError::TaskNotFound(task_id)));
                };
                let contract_history = load_contract_history(conn, &task_id)?;
                let budget = budget::view(&loaded.budgets, &view.task);
                Ok(Ok(TaskDetailData { task: view, contract_history, budget }))
            })
            .await?;
        Ok(result?)
    }

    /// 對任何狀態的 Task 做完整的准入檢查（唯讀預覽，不改狀態）。
    pub async fn task_admission_report(
        &self,
        workspace_id: &str,
        task_id: &str,
    ) -> Result<Vec<MissingItem>, TaskApplyError> {
        let (workspace_id, task_id) = (workspace_id.to_string(), task_id.to_string());
        let result = self
            .read(move |conn| {
                let loaded = load(conn, &workspace_id)?;
                if loaded.workspace.is_none() {
                    return Ok(Err(TaskError::WorkspaceNotFound(workspace_id)));
                }
                let found = loaded
                    .tasks
                    .iter()
                    .find(|t| t.id == task_id)
                    .and_then(|t| loaded.contracts.iter().find(|c| c.task_id == t.id && c.version == t.contract_version));
                Ok(match found {
                    Some(contract) => Ok(task::admission_report(&loaded.ctx(&workspace_id), contract)),
                    None => Err(TaskError::TaskNotFound(task_id)),
                })
            })
            .await?;
        Ok(result?)
    }
}

pub(crate) fn load(conn: &Connection, workspace_id: &str) -> rusqlite::Result<Loaded> {
    let workspace = load_workspaces(conn)?.into_iter().find(|w| w.id == workspace_id);
    let authorization = load_authorization_revisions(conn, workspace_id)?.pop();

    let goals = conn
        .prepare("SELECT id, title, version, created_at FROM goal WHERE workspace_id = ?1 ORDER BY rowid")?
        .query_map([workspace_id], |r| {
            Ok(Goal {
                id: r.get(0)?,
                workspace_id: workspace_id.to_string(),
                title: r.get(1)?,
                version: r.get(2)?,
                created_at: Timestamp::from_millis(r.get(3)?),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let tasks = conn
        .prepare(
            "SELECT id, goal_id, title, status, version, contract_version, created_at, updated_at, kind
             FROM task WHERE workspace_id = ?1 ORDER BY rowid",
        )?
        .query_map([workspace_id], |r| {
            let status: String = r.get(3)?;
            Ok(Task {
                id: r.get(0)?,
                workspace_id: workspace_id.to_string(),
                goal_id: r.get(1)?,
                title: r.get(2)?,
                kind: TaskKind::parse(&r.get::<_, String>(8)?).ok_or_else(|| {
                    rusqlite::Error::FromSqlConversionFailure(8, rusqlite::types::Type::Text, "未知的 Task 種類".into())
                })?,
                status: TaskStatus::parse(&status).ok_or_else(|| {
                    rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, "未知的 Task 狀態".into())
                })?,
                version: r.get(4)?,
                contract_version: r.get(5)?,
                created_at: Timestamp::from_millis(r.get(6)?),
                updated_at: Timestamp::from_millis(r.get(7)?),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let dependencies = conn
        .prepare(
            "SELECT d.task_id, d.depends_on FROM task_dependency d
             JOIN task t ON t.id = d.task_id WHERE t.workspace_id = ?1 ORDER BY d.rowid",
        )?
        .query_map([workspace_id], |r| Ok(Dependency { task_id: r.get(0)?, depends_on: r.get(1)? }))?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    // 只載入每個 Task 目前生效的版本
    let contracts = conn
        .prepare(
            "SELECT c.task_id, c.version, c.content, c.note, c.created_at FROM task_contract_version c
             JOIN task t ON t.id = c.task_id AND t.contract_version = c.version
             WHERE t.workspace_id = ?1 ORDER BY t.rowid",
        )?
        .query_map([workspace_id], contract_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let runs = super::runs::load_runs(conn, workspace_id)?;
    let leases = super::runs::load_leases(conn, workspace_id)?;
    let reports = super::agent::load_latest_reports(conn, workspace_id)?;
    let workdirs = super::workdirs::load_workdirs(conn, workspace_id)?;
    let integrations = super::integration::load_entries(conn, Some(workspace_id))?;
    let budgets = super::budgets::load_budgets(conn, workspace_id)?;
    Ok(Loaded {
        workspace,
        authorization,
        goals,
        tasks,
        dependencies,
        contracts,
        runs,
        leases,
        reports,
        workdirs,
        integrations,
        guards: Vec::new(),
        budgets,
    })
}

fn contract_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<TaskContract> {
    Ok(TaskContract {
        task_id: r.get(0)?,
        version: r.get(1)?,
        content: json_column::<ContractContent>(2, &r.get::<_, String>(2)?)?,
        note: r.get(3)?,
        created_at: Timestamp::from_millis(r.get(4)?),
    })
}

pub(crate) fn load_contract_history(conn: &Connection, task_id: &str) -> rusqlite::Result<Vec<TaskContract>> {
    conn.prepare(
        "SELECT task_id, version, content, note, created_at FROM task_contract_version
         WHERE task_id = ?1 ORDER BY version",
    )?
    .query_map([task_id], contract_row)?
    .collect()
}

/// 寫入（或更新）一個 Task；Run 的結果改變 Task 狀態時也走這裡。
pub(crate) fn save_task(tx: &Transaction<'_>, task: &Task) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO task
         (id, workspace_id, goal_id, title, status, version, contract_version, created_at, updated_at, kind)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(id) DO UPDATE SET
           status = excluded.status, version = excluded.version,
           contract_version = excluded.contract_version, updated_at = excluded.updated_at",
        rusqlite::params![
            task.id,
            task.workspace_id,
            task.goal_id,
            task.title,
            task.status.as_str(),
            task.version,
            task.contract_version,
            task.created_at.as_millis(),
            task.updated_at.as_millis(),
            task.kind.as_str(),
        ],
    )?;
    // 離開 ready（派工、取消、被改契約退回 pending）就不再排隊
    if task.status != TaskStatus::Ready {
        tx.execute("DELETE FROM dispatch_request WHERE task_id = ?1", [&task.id])?;
    }
    Ok(())
}

/// 把 Domain 的結果寫進同一個交易：新狀態、契約版本、相依與事件。
fn save(tx: &Transaction<'_>, change: &TaskChange) -> rusqlite::Result<()> {
    if let Some(goal) = &change.goal {
        tx.execute(
            "INSERT INTO goal (id, workspace_id, title, version, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![goal.id, goal.workspace_id, goal.title, goal.version, goal.created_at.as_millis()],
        )?;
    }
    if let Some(task) = &change.task {
        save_task(tx, task)?;
    }
    if let Some(contract) = &change.contract {
        // Domain 型別只含字串與數字，序列化不會失敗
        let content = serde_json::to_string(&contract.content).expect("契約內容可序列化");
        tx.execute(
            "INSERT INTO task_contract_version (task_id, version, content, note, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![contract.task_id, contract.version, content, contract.note, contract.created_at.as_millis()],
        )?;
    }
    if let Some(dependency) = &change.dependency {
        tx.execute(
            "INSERT INTO task_dependency (task_id, depends_on) VALUES (?1, ?2)",
            rusqlite::params![dependency.task_id, dependency.depends_on],
        )?;
    }
    for event in &change.events {
        append_event(tx, event)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use baton_domain::authorization::Capability;
    use baton_domain::task::{ContractField, Reference, ReferenceKind};

    use super::*;

    const T1: Timestamp = Timestamp::from_millis(1_000);
    const T2: Timestamp = Timestamp::from_millis(2_000);

    async fn store_with_workspace(dir: &Path) -> Store {
        let store = Store::open(dir, T1).await.unwrap();
        let create = WorkspaceCommand::Create { id: "w1".into(), name: None, path: "C:\\work\\app".into() };
        store.apply_workspace_command(create, T1).await.unwrap();
        store
    }

    fn filled(text: &str) -> Option<ContractField> {
        Some(ContractField::Filled { text: text.into(), references: vec![] })
    }

    fn complete() -> ContractContent {
        ContractContent {
            objective: filled("整理資料"),
            inputs: Some(ContractField::NotApplicable { reason: "沒有輸入".into() }),
            scope: filled("公開資料"),
            deliverables: filled("比較表"),
            done_criteria: filled("五個產品"),
            blocker_handling: filled("回報"),
            required_capabilities: vec![Capability::ReadFiles],
            verification_checks: vec![],
        }
    }

    async fn apply(store: &Store, command: TaskCommand, now: Timestamp) -> Result<TaskChange, TaskApplyError> {
        store.apply_task_command("w1".into(), command, now).await
    }

    async fn goal_and_task(store: &Store, task_id: &str, contract: ContractContent) {
        let goal = TaskCommand::CreateGoal { id: "g1".into(), title: "目標".into() };
        // 第二次呼叫時 g1 已存在，忽略重複建立
        let _ = apply(store, goal, T1).await;
        let create = TaskCommand::CreateTask { id: task_id.into(), goal_id: "g1".into(), title: task_id.into(), kind: Default::default(), contract };
        apply(store, create, T1).await.unwrap();
    }

    fn raw(dir: &Path) -> Connection {
        Connection::open(dir.join(DB_FILE_NAME)).unwrap()
    }

    #[tokio::test]
    async fn goal_task_contract_and_events_are_persisted_together_and_survive_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let store = store_with_workspace(dir.path()).await;
        let mut contract = complete();
        contract.inputs = Some(ContractField::Filled {
            text: "看前一份".into(),
            references: vec![Reference { kind: ReferenceKind::Goal, target: "g1".into(), version: 1 }],
        });
        goal_and_task(&store, "t1", contract.clone()).await;
        drop(store);

        let reopened = Store::open(dir.path(), T2).await.unwrap();
        let board = reopened.task_board("w1").await.unwrap();

        assert_eq!(board.goals.len(), 1);
        let view = &board.tasks[0];
        assert_eq!((view.task.id.as_str(), view.task.status, view.contract.version), ("t1", TaskStatus::Pending, 1));
        assert_eq!(view.contract.content, contract.normalized().unwrap());
        let events: Vec<(String, String, i64)> = raw(dir.path())
            .prepare("SELECT aggregate_type, event_type, aggregate_version FROM event WHERE aggregate_type IN ('goal', 'task') ORDER BY seq")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(
            events,
            vec![("goal".into(), "goal_created".into(), 1), ("task".into(), "task_created".into(), 1)]
        );
    }

    #[tokio::test]
    async fn rejected_and_unchanged_commands_write_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let store = store_with_workspace(dir.path()).await;
        goal_and_task(&store, "t1", ContractContent::default()).await;
        let count = || -> i64 { raw(dir.path()).query_row("SELECT COUNT(*) FROM event", [], |r| r.get(0)).unwrap() };
        let before = count();

        let denied = apply(&store, TaskCommand::MarkReady { task_id: "t1".into() }, T2).await;
        let selfish = apply(&store, TaskCommand::AddDependency { task_id: "t1".into(), depends_on: "t1".into() }, T2).await;
        let same = apply(
            &store,
            TaskCommand::UpdateContract { task_id: "t1".into(), contract: ContractContent::default(), note: None },
            T2,
        )
        .await
        .unwrap();

        assert!(matches!(denied, Err(TaskApplyError::Rejected(TaskError::AdmissionDenied(_)))));
        assert!(matches!(selfish, Err(TaskApplyError::Rejected(TaskError::DependencySelf(_)))));
        assert!(!same.changed());
        assert_eq!(count(), before);
    }

    #[tokio::test]
    async fn contract_versions_accumulate_and_are_immutable() {
        let dir = tempfile::tempdir().unwrap();
        let store = store_with_workspace(dir.path()).await;
        goal_and_task(&store, "t1", ContractContent::default()).await;
        let update = |contract, note: &str| TaskCommand::UpdateContract {
            task_id: "t1".into(),
            contract,
            note: Some(note.into()),
        };
        apply(&store, update(complete(), "補齊"), T2).await.unwrap();
        let mut changed = complete();
        changed.scope = filled("加上付費資料");
        apply(&store, update(changed.clone(), "擴大範圍"), T2).await.unwrap();

        let detail = store.task_detail("w1", "t1").await.unwrap();

        let versions: Vec<(i64, Option<&str>)> =
            detail.contract_history.iter().map(|c| (c.version, c.note.as_deref())).collect();
        assert_eq!(versions, vec![(1, None), (2, Some("補齊")), (3, Some("擴大範圍"))]);
        assert_eq!(detail.task.contract.content, changed.normalized().unwrap());
        assert_eq!(detail.contract_history[1].content, complete().normalized().unwrap());
        let conn = raw(dir.path());
        assert!(conn.execute("UPDATE task_contract_version SET note = 'x'", []).is_err());
        assert!(conn.execute("DELETE FROM task_contract_version", []).is_err());
    }

    #[tokio::test]
    async fn queries_are_scoped_to_the_workspace_and_report_missing_ones() {
        let dir = tempfile::tempdir().unwrap();
        let store = store_with_workspace(dir.path()).await;
        let other = WorkspaceCommand::Create { id: "w2".into(), name: None, path: "C:\\work\\other".into() };
        store.apply_workspace_command(other, T1).await.unwrap();
        goal_and_task(&store, "t1", complete()).await;

        let in_other = store.task_board("w2").await.unwrap();
        let wrong_workspace = store.task_detail("w2", "t1").await;
        let unknown_task = store.task_admission_report("w1", "nope").await;
        let unknown_workspace = store.task_board("nope").await;

        assert!(in_other.goals.is_empty() && in_other.tasks.is_empty());
        assert!(matches!(wrong_workspace, Err(TaskApplyError::Rejected(TaskError::TaskNotFound(_)))));
        assert!(matches!(unknown_task, Err(TaskApplyError::Rejected(TaskError::TaskNotFound(_)))));
        assert!(matches!(unknown_workspace, Err(TaskApplyError::Rejected(TaskError::WorkspaceNotFound(_)))));
    }

    #[tokio::test]
    async fn dependencies_and_status_changes_are_persisted() {
        let dir = tempfile::tempdir().unwrap();
        let store = store_with_workspace(dir.path()).await;
        goal_and_task(&store, "a", complete()).await;
        goal_and_task(&store, "b", complete()).await;
        apply(&store, TaskCommand::AddDependency { task_id: "b".into(), depends_on: "a".into() }, T2).await.unwrap();
        apply(&store, TaskCommand::Cancel { task_id: "a".into() }, T2).await.unwrap();
        // 授權尚未確認，但需要的能力是 read_files，所以 b 無法 ready
        let denied = apply(&store, TaskCommand::MarkReady { task_id: "b".into() }, T2).await;

        let board = store.task_board("w1").await.unwrap();

        assert!(matches!(denied, Err(TaskApplyError::Rejected(TaskError::AdmissionDenied(_)))));
        assert_eq!(board.tasks[0].task.status, TaskStatus::Cancelled);
        assert_eq!(board.tasks[1].dependencies, vec!["a".to_string()]);
        assert_eq!(board.tasks[1].unfinished_dependencies, vec!["a".to_string()]);
        assert_eq!(board.tasks[1].task.version, 2);
        assert_eq!(board.tasks[1].task.updated_at, T2);
    }
}
