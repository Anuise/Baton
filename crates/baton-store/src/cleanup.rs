//! worktree 自動清理的持久化（票 21）。
//!
//! 規則都在 Domain（`baton_domain::cleanup`）；這裡只做「找出該登記待清理的 worktree 並在交易中登記」
//! 「找出現在要處理的清理」「在同一個交易中載入登記 → 交給 Domain 判定 → 寫入登記與事件」與唯讀查詢。
//! 核對與移除本身是外部副作用，由 Supervisor 在交易外經由 WorkspaceFs 完成；每一次嘗試都先持久化「清理中」，
//! 移除之後、記錄結果之前崩潰的話，重建後從「清理中」接續核對（移除是冪等的）。

use baton_domain::cleanup::{self, CleanupChange, CleanupCommand, CleanupError, CleanupStatus, WorktreeCleanup};

use super::tasks::load;
use super::*;

/// 套用清理命令的失敗：Domain 拒絕或儲存層錯誤。
#[derive(Debug, thiserror::Error)]
pub enum CleanupApplyError {
    #[error(transparent)]
    Rejected(#[from] CleanupError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl From<rusqlite::Error> for CleanupApplyError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error.into())
    }
}

/// 現在要處理的一個清理，附上它的 Workspace（repo 位置與暫放目錄由它決定）。
#[derive(Debug, Clone, PartialEq)]
pub struct CleanupWork {
    pub cleanup: WorktreeCleanup,
    pub workspace: Workspace,
}

impl Store {
    /// 把符合條件的 worktree 登記為待清理（同一個交易中寫入登記與事件），回傳這次新登記的清理。
    /// 只看使用中、沒有被生命週期擋住的 Workspace：完全退出中或等待「繼續工作」時不觸發清理。
    pub async fn request_cleanups(&self, now: Timestamp) -> StoreResult<Vec<WorktreeCleanup>> {
        self.write(move |tx| {
            let mut requested = Vec::new();
            for workspace in load_workspaces(tx)? {
                if workspace.status != WorkspaceStatus::Active
                    || super::lifecycle::halt_for(tx, Some(&workspace), &workspace.id)?.is_some()
                {
                    continue;
                }
                let loaded = load(tx, &workspace.id)?;
                let existing = load_cleanups(tx, Some(&workspace.id))?;
                for workdir in &loaded.workdirs {
                    let Some(task) = loaded.tasks.iter().find(|t| t.id == workdir.task_id) else { continue };
                    let current = existing.iter().find(|c| c.task_id == workdir.task_id);
                    if !cleanup::should_request(workdir, task, &loaded.runs, &loaded.integrations, current) {
                        continue;
                    }
                    let (registered, event) = cleanup::request(workdir, now);
                    insert_cleanup(tx, &registered)?;
                    append_event(tx, &event)?;
                    requested.push(registered);
                }
            }
            Ok(requested)
        })
        .await
    }

    /// 現在要處理的清理：到了重試時間的待清理與停在清理中的（崩潰後接續），依登記順序。
    /// 被生命週期擋住的 Workspace 暫不處理（不在這時執行 git）。
    pub async fn due_cleanups(&self, now: Timestamp) -> StoreResult<Vec<CleanupWork>> {
        self.read(move |conn| {
            let workspaces = load_workspaces(conn)?;
            let mut found = Vec::new();
            for cleanup in load_cleanups(conn, None)? {
                if !cleanup::is_due(&cleanup, now) {
                    continue;
                }
                let Some(workspace) = workspaces.iter().find(|w| w.id == cleanup.workspace_id) else { continue };
                if super::lifecycle::halt_for(conn, Some(workspace), &workspace.id)?.is_some() {
                    continue;
                }
                found.push(CleanupWork { cleanup, workspace: workspace.clone() });
            }
            Ok(found)
        })
        .await
    }

    /// 套用一個清理命令：Domain 拒絕時什麼都不寫。
    pub async fn apply_cleanup(
        &self,
        command: CleanupCommand,
        now: Timestamp,
    ) -> Result<CleanupChange, CleanupApplyError> {
        let outcome = self
            .write(move |tx| {
                let task_id = command.task_id().to_string();
                let Some(current) = load_cleanup(tx, &task_id)? else {
                    return Ok(Err(CleanupError::NotFound(task_id)));
                };
                match cleanup::decide(&current, command, now) {
                    Err(rejected) => Ok(Err(rejected)),
                    Ok(change) => {
                        update_cleanup(tx, &change.cleanup)?;
                        for event in &change.events {
                            append_event(tx, event)?;
                        }
                        Ok(Ok(change))
                    }
                }
            })
            .await?;
        Ok(outcome?)
    }
}

const COLUMNS: &str =
    "task_id, workspace_id, path, branch, status, version, attempts, failure, requested_at, next_attempt_at, finished_at";

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<WorktreeCleanup> {
    let status: String = r.get(4)?;
    Ok(WorktreeCleanup {
        task_id: r.get(0)?,
        workspace_id: r.get(1)?,
        path: r.get(2)?,
        branch: r.get(3)?,
        status: CleanupStatus::parse(&status).ok_or_else(|| {
            rusqlite::Error::FromSqlConversionFailure(4, rusqlite::types::Type::Text, "未知的清理狀態".into())
        })?,
        version: r.get(5)?,
        attempts: r.get(6)?,
        failure: r.get(7)?,
        requested_at: Timestamp::from_millis(r.get(8)?),
        next_attempt_at: r.get::<_, Option<i64>>(9)?.map(Timestamp::from_millis),
        finished_at: r.get::<_, Option<i64>>(10)?.map(Timestamp::from_millis),
    })
}

/// 清理登記，依登記順序；`workspace_id` 為 `None` 時是全部 Workspace。
pub(crate) fn load_cleanups(conn: &Connection, workspace_id: Option<&str>) -> rusqlite::Result<Vec<WorktreeCleanup>> {
    let sql = format!("SELECT {COLUMNS} FROM worktree_cleanup WHERE ?1 IS NULL OR workspace_id = ?1 ORDER BY rowid");
    conn.prepare(&sql)?.query_map([workspace_id], row)?.collect()
}

/// 一個 Task 的清理登記。
pub(crate) fn load_cleanup(conn: &Connection, task_id: &str) -> rusqlite::Result<Option<WorktreeCleanup>> {
    let sql = format!("SELECT {COLUMNS} FROM worktree_cleanup WHERE task_id = ?1");
    conn.prepare(&sql)?.query_map([task_id], row)?.next().transpose()
}

fn insert_cleanup(tx: &Transaction<'_>, cleanup: &WorktreeCleanup) -> rusqlite::Result<()> {
    tx.execute(
        &format!("INSERT INTO worktree_cleanup ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)"),
        rusqlite::params![
            cleanup.task_id,
            cleanup.workspace_id,
            cleanup.path,
            cleanup.branch,
            cleanup.status.as_str(),
            cleanup.version,
            cleanup.attempts,
            cleanup.failure,
            cleanup.requested_at.as_millis(),
            cleanup.next_attempt_at.map(Timestamp::as_millis),
            cleanup.finished_at.map(Timestamp::as_millis),
        ],
    )?;
    Ok(())
}

fn update_cleanup(tx: &Transaction<'_>, cleanup: &WorktreeCleanup) -> rusqlite::Result<()> {
    tx.execute(
        "UPDATE worktree_cleanup SET status = ?2, version = ?3, attempts = ?4, failure = ?5, next_attempt_at = ?6,
           finished_at = ?7
         WHERE task_id = ?1",
        rusqlite::params![
            cleanup.task_id,
            cleanup.status.as_str(),
            cleanup.version,
            cleanup.attempts,
            cleanup.failure,
            cleanup.next_attempt_at.map(Timestamp::as_millis),
            cleanup.finished_at.map(Timestamp::as_millis),
        ],
    )?;
    Ok(())
}
