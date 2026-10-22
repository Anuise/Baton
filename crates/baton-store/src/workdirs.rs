//! 平台登記的 Task 工作目錄（票 20）。
//!
//! 規則都在 Domain（`baton_domain::workdir`）；這裡只做「找出需要準備目錄的 Task」「登記準備結果（含事件）」
//! 與唯讀查詢。建立目錄本身是外部副作用，由 Runtime 在交易外經由 WorkspaceFs 完成；
//! 建立之後、登記之前崩潰的話，下一次準備會重做（WorkspaceFs 的建立是冪等的）。

use baton_domain::task::{Task, TaskStatus};
use baton_domain::workdir::{self, Workdir, WorkdirKind, WorkdirStatus, WorkdirView};
use baton_domain::workspace::Workspace;

use super::tasks::load;
use super::*;

/// 需要準備工作目錄的 Task：它的 Workspace、Task 本身與目前的登記（若有）。
#[derive(Debug, Clone, PartialEq)]
pub struct WorkdirCandidate {
    pub workspace: Workspace,
    pub task: Task,
    pub existing: Option<Workdir>,
}

impl Store {
    /// 現在需要準備工作目錄的 Task：派工佇列中排隊的 Task，加上 `explicit`（使用者剛要求派工的 Task，失敗也立即重試）。
    /// 只考慮 `ready` 的 Task 與使用中的 Workspace；已有可用目錄的不再準備。
    pub async fn workdir_candidates(
        &self,
        explicit: Option<(String, String)>,
        now: Timestamp,
    ) -> StoreResult<Vec<WorkdirCandidate>> {
        self.read(move |conn| {
            let mut keys: Vec<(String, String)> = conn
                .prepare("SELECT workspace_id, task_id FROM dispatch_request ORDER BY seq")?
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            if let Some(key) = &explicit {
                if !keys.contains(key) {
                    keys.push(key.clone());
                }
            }
            let mut found = Vec::new();
            for (workspace_id, task_id) in keys {
                let loaded = load(conn, &workspace_id)?;
                let Some(workspace) = loaded.workspace.clone().filter(|w| w.status == WorkspaceStatus::Active) else {
                    continue;
                };
                let Some(task) = loaded.tasks.iter().find(|t| t.id == task_id && t.status == TaskStatus::Ready) else {
                    continue;
                };
                let existing = loaded.workdirs.iter().find(|w| w.task_id == task.id).cloned();
                let is_explicit = explicit.as_ref().is_some_and(|(_, t)| *t == task.id);
                if workdir::needs_preparation(existing.as_ref(), now, is_explicit) {
                    found.push(WorkdirCandidate { workspace, task: task.clone(), existing });
                }
            }
            Ok(found)
        })
        .await
    }

    /// 登記一次準備結果與它的事件。
    pub async fn record_workdir(&self, workdir: Workdir, event: DomainEvent) -> StoreResult<()> {
        self.write(move |tx| {
            tx.execute(
                "INSERT INTO task_workdir
                 (task_id, workspace_id, kind, path, branch, base_branch, status, failure, version, attempted_at, ready_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                 ON CONFLICT(task_id) DO UPDATE SET
                   kind = excluded.kind, path = excluded.path, branch = excluded.branch,
                   base_branch = excluded.base_branch, status = excluded.status, failure = excluded.failure,
                   version = excluded.version, attempted_at = excluded.attempted_at, ready_at = excluded.ready_at",
                rusqlite::params![
                    workdir.task_id,
                    workdir.workspace_id,
                    workdir.kind.as_str(),
                    workdir.path,
                    workdir.branch,
                    workdir.base_branch,
                    workdir.status.as_str(),
                    workdir.failure,
                    workdir.version,
                    workdir.attempted_at.as_millis(),
                    workdir.ready_at.map(Timestamp::as_millis),
                ],
            )?;
            append_event(tx, &event)?;
            Ok(())
        })
        .await
    }
}

pub(crate) fn load_workdirs(conn: &Connection, workspace_id: &str) -> rusqlite::Result<Vec<Workdir>> {
    conn.prepare(
        "SELECT task_id, kind, path, branch, base_branch, status, failure, version, attempted_at, ready_at
         FROM task_workdir WHERE workspace_id = ?1 ORDER BY rowid",
    )?
    .query_map([workspace_id], |r| {
        let invalid = |index: usize, what: &str| {
            rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Text, what.to_string().into())
        };
        let kind: String = r.get(1)?;
        let status: String = r.get(5)?;
        Ok(Workdir {
            task_id: r.get(0)?,
            workspace_id: workspace_id.to_string(),
            kind: WorkdirKind::parse(&kind).ok_or_else(|| invalid(1, "未知的工作目錄種類"))?,
            path: r.get(2)?,
            branch: r.get(3)?,
            base_branch: r.get(4)?,
            status: WorkdirStatus::parse(&status).ok_or_else(|| invalid(5, "未知的工作目錄狀態"))?,
            failure: r.get(6)?,
            version: r.get(7)?,
            attempted_at: Timestamp::from_millis(r.get(8)?),
            ready_at: r.get::<_, Option<i64>>(9)?.map(Timestamp::from_millis),
        })
    })?
    .collect()
}

/// 每個工作目錄加上曾在其中執行的 Run：派工 Outbox 的啟動資訊中工作目錄相同的 Run（依建立順序）。
pub(crate) fn views(conn: &Connection, workdirs: &[Workdir]) -> rusqlite::Result<Vec<WorkdirView>> {
    let mut found = Vec::with_capacity(workdirs.len());
    for workdir in workdirs {
        let run_ids = conn
            .prepare(
                "SELECT r.id FROM run r JOIN outbox o ON o.run_id = r.id AND o.kind = ?1
                 WHERE r.task_id = ?2 AND json_extract(o.payload, '$.working_dir') = ?3 ORDER BY r.rowid",
            )?
            .query_map(rusqlite::params![baton_domain::run::OUTBOX_LAUNCH_RUN, workdir.task_id, workdir.path], |r| {
                r.get::<_, String>(0)
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        found.push(WorkdirView { workdir: workdir.clone(), run_ids });
    }
    Ok(found)
}

/// 測試用：把 Task 的工作目錄登記為可用（Runtime 在交易外建立目錄後做的事），派工才會被接受。
#[cfg(test)]
pub(crate) async fn register_ready(store: &Store, workspace_id: &str, task_id: &str, now: Timestamp) {
    let (workspace_id, task_id) = (workspace_id.to_string(), task_id.to_string());
    let loaded = store.read(move |conn| Ok(load(conn, &workspace_id)?)).await.unwrap();
    let workspace = loaded.workspace.clone().unwrap();
    let task = loaded.tasks.iter().find(|t| t.id == task_id).unwrap();
    let plan = workdir::plan(&workspace, task);
    let base = plan.branch.as_ref().map(|_| "main".to_string());
    let (workdir, event) = workdir::record_prepared(None, &plan, base, now);
    store.record_workdir(workdir, event).await.unwrap();
}
