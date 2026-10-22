//! Run、execution lease 與派工 Outbox 的持久化。
//!
//! 規則都在 Domain（`baton_domain::run`）；這裡只做「在同一個交易中載入目前狀態 → 交給 Domain 判定 →
//! 寫入新狀態、lease、Outbox、Task 與事件」，以及給 Supervisor 與面板的唯讀查詢。範圍一律以 Workspace 為界；
//! Supervisor 的跨 Workspace 查詢只回傳它需要處理的 Run。

use baton_domain::run::{
    self, ExecutionLease, LaunchPayload, OutboxItem, ProcessRecord, Run, RunChange, RunCommand, RunContext, RunError,
    RunStatus, RunView,
};
use baton_domain::task::{Task, TaskContext};

use super::tasks::{load, save_task, Loaded};
use super::*;

/// 套用 Run 命令或查詢的失敗：Domain 拒絕（使用者可修正）或儲存層錯誤。
#[derive(Debug, thiserror::Error)]
pub enum RunApplyError {
    #[error(transparent)]
    Rejected(#[from] RunError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl From<rusqlite::Error> for RunApplyError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error.into())
    }
}

/// 派工 Outbox 中尚未處理的一個啟動要求，附上 Run 的目前狀態。
#[derive(Debug, Clone, PartialEq)]
pub struct PendingLaunch {
    pub run: Run,
    pub payload: LaunchPayload,
}

/// 已經啟動（starting、running）的 Run、它的 lease，以及當初派工時固定下來的啟動資訊。
#[derive(Debug, Clone, PartialEq)]
pub struct ExecutingRun {
    pub view: RunView,
    pub payload: LaunchPayload,
}

impl Store {
    /// 套用一個 Run 命令：Domain 拒絕或沒有任何變動（冪等）時不寫入任何東西。
    pub async fn apply_run_command(
        &self,
        workspace_id: String,
        command: RunCommand,
        now: Timestamp,
    ) -> Result<RunChange, RunApplyError> {
        let outcome = self
            .write(move |tx| {
                let loaded = load(tx, &workspace_id)?;
                match decide_in(&loaded, &workspace_id, &loaded.tasks, command, now) {
                    Err(rejected) => Ok(Err(rejected)),
                    Ok(change) => {
                        if change.changed() {
                            save(tx, &change, now)?;
                        }
                        Ok(Ok(change))
                    }
                }
            })
            .await?;
        Ok(outcome?)
    }

    /// Workspace 內的全部 Run（含已結束），依建立順序。
    pub async fn run_list(&self, workspace_id: &str) -> Result<Vec<RunView>, RunApplyError> {
        let workspace_id = workspace_id.to_string();
        let result = self
            .read(move |conn| {
                let loaded = load(conn, &workspace_id)?;
                if loaded.workspace.is_none() {
                    return Ok(Err(RunError::WorkspaceNotFound(workspace_id)));
                }
                Ok(Ok(views(&loaded)))
            })
            .await?;
        Ok(result?)
    }

    pub async fn run_detail(&self, workspace_id: &str, run_id: &str) -> Result<RunView, RunApplyError> {
        let (workspace_id, run_id) = (workspace_id.to_string(), run_id.to_string());
        let result = self
            .read(move |conn| {
                let loaded = load(conn, &workspace_id)?;
                if loaded.workspace.is_none() {
                    return Ok(Err(RunError::WorkspaceNotFound(workspace_id)));
                }
                Ok(views(&loaded).into_iter().find(|v| v.run.id == run_id).ok_or(RunError::RunNotFound(run_id)))
            })
            .await?;
        Ok(result?)
    }

    /// 派工 Outbox 中尚未處理的啟動要求（跨 Workspace，依派工順序）。
    pub async fn pending_launches(&self) -> StoreResult<Vec<PendingLaunch>> {
        self.read(|conn| {
            let sql = format!(
                "SELECT {RUN_COLUMNS_QUALIFIED}, o.payload FROM outbox o JOIN run r ON r.id = o.run_id
                 WHERE o.kind = ?1 AND o.delivered_at IS NULL ORDER BY o.id"
            );
            let mut statement = conn.prepare(&sql)?;
            let rows = statement.query_map([run::OUTBOX_LAUNCH_RUN], |r| {
                Ok(PendingLaunch { run: run_row(r)?, payload: json_column(RUN_COLUMN_COUNT, &r.get::<_, String>(RUN_COLUMN_COUNT)?)? })
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
        .await
    }

    /// 已經啟動（starting、running）的 Run（跨 Workspace，依建立順序）。
    pub async fn executing_runs(&self) -> StoreResult<Vec<ExecutingRun>> {
        self.read(|conn| {
            let sql = format!(
                "SELECT {RUN_COLUMNS_QUALIFIED}, o.payload FROM run r
                 JOIN outbox o ON o.run_id = r.id AND o.kind = ?1
                 WHERE r.status IN ('starting', 'running') ORDER BY r.rowid"
            );
            let rows = conn
                .prepare(&sql)?
                .query_map([run::OUTBOX_LAUNCH_RUN], |r| {
                    Ok((run_row(r)?, json_column::<LaunchPayload>(RUN_COLUMN_COUNT, &r.get::<_, String>(RUN_COLUMN_COUNT)?)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let mut executing = Vec::with_capacity(rows.len());
            for (run, payload) in rows {
                let lease = load_lease(conn, &run.id)?;
                executing.push(ExecutingRun { view: RunView { run, lease }, payload });
            }
            Ok(executing)
        })
        .await
    }
}

/// 在已載入的狀態上判定一個 Run 命令；`tasks` 允許呼叫端換成「Task 已經先改變」的版本（取消連動）。
pub(crate) fn decide_in(
    loaded: &Loaded,
    workspace_id: &str,
    tasks: &[Task],
    command: RunCommand,
    now: Timestamp,
) -> Result<RunChange, RunError> {
    let ctx = loaded.ctx(workspace_id);
    let task_ctx = TaskContext { tasks, ..ctx };
    run::decide(&RunContext { task: &task_ctx, runs: &loaded.runs, leases: &loaded.leases }, command, now)
}

/// 取消 Task 之後，同一個交易中要求取消它還在進行的 Run。
/// `task` 是取消之後的 Task：Run 結束時看到 Task 已經不是 `running`，就不會把它拉回 `ready`。
pub(crate) fn cascade_cancel(
    tx: &Transaction<'_>,
    loaded: &Loaded,
    task: &Task,
    task_id: &str,
    now: Timestamp,
) -> rusqlite::Result<()> {
    let Some(active) = loaded.runs.iter().find(|r| r.task_id == task_id && !r.status.is_terminal()) else {
        return Ok(());
    };
    let tasks: Vec<Task> =
        loaded.tasks.iter().map(|t| if t.id == task.id { task.clone() } else { t.clone() }).collect();
    let command = RunCommand::RequestCancel { run_id: active.id.clone() };
    // 取消一個進行中的 Run 不會被 Domain 拒絕；真的被拒絕時 Task 取消本身仍然成立
    if let Ok(change) = decide_in(loaded, &task.workspace_id, &tasks, command, now) {
        if change.changed() {
            save(tx, &change, now)?;
        }
    }
    Ok(())
}

pub(crate) fn views(loaded: &Loaded) -> Vec<RunView> {
    loaded
        .runs
        .iter()
        .map(|run| RunView { run: run.clone(), lease: loaded.leases.iter().find(|l| l.run_id == run.id).cloned() })
        .collect()
}

const RUN_COLUMNS: &str = "id, workspace_id, task_id, status, version, generation, contract_version, \
     authorization_revision, command_id, pid, process_created_at, command_hash, cancel_requested_at, end_reason, \
     created_at, updated_at, started_at, ended_at, profile_id";
const RUN_COLUMNS_QUALIFIED: &str = "r.id, r.workspace_id, r.task_id, r.status, r.version, r.generation, \
     r.contract_version, r.authorization_revision, r.command_id, r.pid, r.process_created_at, r.command_hash, \
     r.cancel_requested_at, r.end_reason, r.created_at, r.updated_at, r.started_at, r.ended_at, r.profile_id";
const RUN_COLUMN_COUNT: usize = 19;

const LEASE_COLUMNS: &str =
    "run_id, task_id, generation, acquired_at, last_heartbeat_at, expires_at, released_at, release_reason";

fn run_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Run> {
    let status: String = r.get(3)?;
    let pid: Option<u32> = r.get(9)?;
    let process_created_at: Option<i64> = r.get(10)?;
    let command_hash: Option<String> = r.get(11)?;
    let timestamp = |index: usize| r.get::<_, Option<i64>>(index).map(|v| v.map(Timestamp::from_millis));
    Ok(Run {
        id: r.get(0)?,
        workspace_id: r.get(1)?,
        task_id: r.get(2)?,
        status: RunStatus::parse(&status).ok_or_else(|| {
            rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, "未知的 Run 狀態".into())
        })?,
        version: r.get(4)?,
        generation: r.get(5)?,
        contract_version: r.get(6)?,
        authorization_revision: r.get(7)?,
        command_id: r.get(8)?,
        process: match (pid, process_created_at, command_hash) {
            (Some(pid), Some(created_at_millis), Some(command_hash)) => {
                Some(ProcessRecord { pid, created_at_millis, command_hash })
            }
            _ => None,
        },
        cancel_requested_at: timestamp(12)?,
        end_reason: r.get(13)?,
        created_at: Timestamp::from_millis(r.get(14)?),
        updated_at: Timestamp::from_millis(r.get(15)?),
        started_at: timestamp(16)?,
        ended_at: timestamp(17)?,
        // 票 19 之前建立的 Run 沒有帳號
        profile_id: r.get::<_, Option<String>>(18)?.unwrap_or_default(),
    })
}

fn lease_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ExecutionLease> {
    let timestamp = |index: usize| r.get::<_, Option<i64>>(index).map(|v| v.map(Timestamp::from_millis));
    Ok(ExecutionLease {
        run_id: r.get(0)?,
        task_id: r.get(1)?,
        generation: r.get(2)?,
        acquired_at: Timestamp::from_millis(r.get(3)?),
        last_heartbeat_at: Timestamp::from_millis(r.get(4)?),
        expires_at: Timestamp::from_millis(r.get(5)?),
        released_at: timestamp(6)?,
        release_reason: r.get(7)?,
    })
}

/// Workspace 內的全部 Run，依建立順序。
pub(crate) fn load_runs(conn: &Connection, workspace_id: &str) -> rusqlite::Result<Vec<Run>> {
    conn.prepare(&format!("SELECT {RUN_COLUMNS} FROM run WHERE workspace_id = ?1 ORDER BY rowid"))?
        .query_map([workspace_id], run_row)?
        .collect()
}

/// 全部 Workspace 中進行中（還沒結束）的 Run；容量是全域的，所以不分 Workspace。
pub(crate) fn load_active_runs(conn: &Connection) -> rusqlite::Result<Vec<Run>> {
    conn.prepare(&format!(
        "SELECT {RUN_COLUMNS} FROM run WHERE status IN ('queued', 'starting', 'running') ORDER BY rowid"
    ))?
    .query_map([], run_row)?
    .collect()
}

pub(crate) fn load_leases(conn: &Connection, workspace_id: &str) -> rusqlite::Result<Vec<ExecutionLease>> {
    // JOIN 之後欄位名稱會有歧義，所以每個欄位都加上 lease 的前綴
    let columns = LEASE_COLUMNS.split(", ").map(|c| format!("l.{c}")).collect::<Vec<_>>().join(", ");
    conn.prepare(&format!(
        "SELECT {columns} FROM execution_lease l JOIN run r ON r.id = l.run_id
         WHERE r.workspace_id = ?1 ORDER BY r.rowid"
    ))?
    .query_map([workspace_id], lease_row)?
    .collect()
}

fn load_lease(conn: &Connection, run_id: &str) -> rusqlite::Result<Option<ExecutionLease>> {
    conn.prepare(&format!("SELECT {LEASE_COLUMNS} FROM execution_lease WHERE run_id = ?1"))?
        .query_map([run_id], lease_row)?
        .next()
        .transpose()
}

/// 把 Domain 的結果寫進同一個交易：Run、lease、Outbox、Task 與事件。
pub(crate) fn save(tx: &Transaction<'_>, change: &RunChange, now: Timestamp) -> rusqlite::Result<()> {
    if let Some(run) = &change.run {
        let (pid, process_created_at, command_hash) = match &run.process {
            Some(p) => (Some(p.pid), Some(p.created_at_millis), Some(p.command_hash.as_str())),
            None => (None, None, None),
        };
        let millis = |t: Option<Timestamp>| t.map(|t| t.as_millis());
        tx.execute(
            &format!(
                "INSERT INTO run ({RUN_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)
                 ON CONFLICT(id) DO UPDATE SET
                   status = excluded.status, version = excluded.version, command_id = excluded.command_id,
                   pid = excluded.pid, process_created_at = excluded.process_created_at,
                   command_hash = excluded.command_hash, cancel_requested_at = excluded.cancel_requested_at,
                   end_reason = excluded.end_reason, updated_at = excluded.updated_at,
                   started_at = excluded.started_at, ended_at = excluded.ended_at"
            ),
            rusqlite::params![
                run.id,
                run.workspace_id,
                run.task_id,
                run.status.as_str(),
                run.version,
                run.generation,
                run.contract_version,
                run.authorization_revision,
                run.command_id,
                pid,
                process_created_at,
                command_hash,
                millis(run.cancel_requested_at),
                run.end_reason,
                run.created_at.as_millis(),
                run.updated_at.as_millis(),
                millis(run.started_at),
                millis(run.ended_at),
                run.profile_id,
            ],
        )?;
    }
    if let Some(lease) = &change.lease {
        tx.execute(
            "INSERT INTO execution_lease (run_id, task_id, generation, acquired_at, last_heartbeat_at, expires_at,
                released_at, release_reason) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(run_id) DO UPDATE SET
               last_heartbeat_at = excluded.last_heartbeat_at, expires_at = excluded.expires_at,
               released_at = excluded.released_at, release_reason = excluded.release_reason",
            rusqlite::params![
                lease.run_id,
                lease.task_id,
                lease.generation,
                lease.acquired_at.as_millis(),
                lease.last_heartbeat_at.as_millis(),
                lease.expires_at.as_millis(),
                lease.released_at.map(|t| t.as_millis()),
                lease.release_reason,
            ],
        )?;
    }
    if let Some(item) = &change.outbox {
        save_outbox(tx, item)?;
    }
    if let Some(run_id) = &change.outbox_delivered {
        tx.execute(
            "UPDATE outbox SET delivered_at = ?3 WHERE kind = ?1 AND run_id = ?2 AND delivered_at IS NULL",
            rusqlite::params![run::OUTBOX_LAUNCH_RUN, run_id, now.as_millis()],
        )?;
    }
    if let Some(task) = &change.task {
        save_task(tx, task)?;
    }
    for event in &change.events {
        append_event(tx, event)?;
    }
    Ok(())
}

fn save_outbox(tx: &Transaction<'_>, item: &OutboxItem) -> rusqlite::Result<()> {
    // Domain 型別只含字串與數字，序列化不會失敗
    let payload = serde_json::to_string(&item.payload).expect("Outbox 內容可序列化");
    tx.execute(
        "INSERT INTO outbox (kind, run_id, payload, created_at, delivered_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![item.kind, item.run_id, payload, item.created_at.as_millis(), item.delivered_at.map(|t| t.as_millis())],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests;
