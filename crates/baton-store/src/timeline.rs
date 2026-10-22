//! 事件時間線的讀取（票 41）：從追加式事件表讀出時間線上的事件，解析每筆事件相關的 Task 與 Run。
//!
//! 分類規則在 Domain（`baton_domain::timeline`）。Task 的歸屬依聚合決定：Run、交接、裁定請求看各自的資料表；
//! Task 與 worktree 清理的聚合識別就是 Task；外部操作、重犯、用量看事件內容裡的 `task_id`／`run_id`
//! （外部操作重試時 `task_id` 會換成新的 Task，所以以事件當時的 Run 為準）。
//! 恢復事件屬於整個 Runtime，出現在每個 Workspace 的時間線上，關聯到它任何一次嘗試處置過、屬於這個 Workspace 的 Task。
//! 不屬於任何 Task 的事件依它記下的 Workspace 歸屬：沒有指定 Task 的重犯、Controller Run 的用量（票 16）。
//! 裁定請求除了發起的 Task，也關聯到它阻擋的每個 Task（`decision_request_block`）。
//! 不屬於這個 Workspace 的事件在 SQL 端就濾掉，不讀出、不解析內容。

use std::collections::{HashMap, HashSet};

use baton_domain::decision::DECISION_AGGREGATE;

use baton_domain::recovery::{AffectedRun, RECOVERY_AGGREGATE};
use baton_domain::timeline::{self, TimelineEntry, TIMELINE_AGGREGATES};
use rusqlite::OptionalExtension;

use super::*;

/// 讀取時間線的失敗。
#[derive(Debug, thiserror::Error)]
pub enum TimelineError {
    #[error("找不到 Workspace {0}")]
    WorkspaceNotFound(String),
    #[error("Workspace 中沒有 Task {0}")]
    TaskNotFound(String),
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl From<rusqlite::Error> for TimelineError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error.into())
    }
}

impl Store {
    /// Workspace 的時間線，依事件提交順序；指定 Task 時只留下與它有關的事件。
    pub async fn timeline(&self, workspace_id: &str, task_id: Option<&str>) -> Result<Vec<TimelineEntry>, TimelineError> {
        let workspace_id = workspace_id.to_string();
        let task_id = task_id.map(str::to_string);
        self.read(move |conn| Ok(read_timeline(conn, &workspace_id, task_id.as_deref())))
            .await?
    }
}

fn read_timeline(conn: &Connection, workspace_id: &str, task_id: Option<&str>) -> Result<Vec<TimelineEntry>, TimelineError> {
    let exists = conn.query_row("SELECT 1 FROM workspace WHERE id = ?1", [workspace_id], |_| Ok(())).optional()?;
    if exists.is_none() {
        return Err(TimelineError::WorkspaceNotFound(workspace_id.to_string()));
    }
    let workspace_tasks: HashSet<String> = conn
        .prepare("SELECT id FROM task WHERE workspace_id = ?1")?
        .query_map([workspace_id], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    if let Some(task_id) = task_id {
        if !workspace_tasks.contains(task_id) {
            return Err(TimelineError::TaskNotFound(task_id.to_string()));
        }
    }
    let in_workspace = |task: &String| workspace_tasks.contains(task);

    // 裁定請求 → 它阻擋的 Task（依加入阻擋範圍的順序）
    let mut blocked: HashMap<String, Vec<String>> = HashMap::new();
    let mut blocks = conn.prepare(
        "SELECT b.request_id, b.task_id FROM decision_request_block b
         JOIN decision_request d ON d.id = b.request_id
         WHERE d.workspace_id = ?1 ORDER BY b.rowid",
    )?;
    for row in blocks.query_map([workspace_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
        let (request_id, task) = row?;
        blocked.entry(request_id).or_default().push(task);
    }

    // 恢復事件 → 它處置過的 Task（所有輪次、所有嘗試）
    let mut recovered: HashMap<String, Vec<String>> = HashMap::new();
    let mut attempts = conn.prepare("SELECT event_id, affected FROM recovery_attempt ORDER BY started_at, id")?;
    for row in attempts.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
        let (event_id, affected) = row?;
        let affected: Vec<AffectedRun> = json_column(1, &affected)?;
        let tasks = recovered.entry(event_id).or_default();
        for run in affected {
            if !tasks.contains(&run.task_id) {
                tasks.push(run.task_id);
            }
        }
    }

    let aggregates = TIMELINE_AGGREGATES.map(|a| format!("'{a}'")).join(", ");
    // 與下面逐筆判斷的歸屬相同：恢復事件一律讀出；其餘只讀出 Task 屬於這個 Workspace、或沒有 Task 但記下這個 Workspace 的事件
    let sql = format!(
        "SELECT seq, aggregate_type, aggregate_id, event_type, occurred_at, payload, task_id, run_id, owner_workspace FROM (
         SELECT e.seq, e.aggregate_type, e.aggregate_id, e.event_type, e.occurred_at, e.payload,
                COALESCE(r.task_id, h.task_id, d.task_id,
                         CASE WHEN e.aggregate_type IN ('task', 'worktree_cleanup') THEN e.aggregate_id END,
                         json_extract(e.payload, '$.task_id'), pr.task_id, x.task_id) AS task_id,
                COALESCE(CASE WHEN e.aggregate_type = 'run' THEN e.aggregate_id END, h.from_run_id, d.run_id,
                         json_extract(e.payload, '$.run_id'), x.run_id) AS run_id,
                COALESCE(lr.workspace_id, cr.workspace_id) AS owner_workspace
         FROM event e
         LEFT JOIN run r ON e.aggregate_type = 'run' AND r.id = e.aggregate_id
         LEFT JOIN handoff h ON e.aggregate_type = 'handoff' AND h.id = e.aggregate_id
         LEFT JOIN decision_request d ON e.aggregate_type = 'decision_request' AND d.id = e.aggregate_id
         LEFT JOIN external_operation x ON e.aggregate_type = 'external_operation' AND x.id = e.aggregate_id
         LEFT JOIN lesson_recurrence lr ON e.aggregate_type = 'lesson_recurrence' AND lr.id = e.aggregate_id
         LEFT JOIN controller_run cr ON e.aggregate_type = 'run_usage' AND cr.id = e.aggregate_id
         LEFT JOIN run pr ON pr.id = json_extract(e.payload, '$.run_id')
         WHERE e.aggregate_type IN ({aggregates}))
         WHERE aggregate_type = '{RECOVERY_AGGREGATE}'
            OR task_id IN (SELECT id FROM task WHERE workspace_id = ?1)
            OR (task_id IS NULL AND owner_workspace = ?1)
         ORDER BY seq"
    );
    let mut statement = conn.prepare(&sql)?;
    let rows = statement.query_map([workspace_id], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, i64>(4)?,
            r.get::<_, String>(5)?,
            r.get::<_, Option<String>>(6)?,
            r.get::<_, Option<String>>(7)?,
            r.get::<_, Option<String>>(8)?,
        ))
    })?;
    let mut entries = Vec::new();
    for row in rows {
        let (seq, aggregate_type, aggregate_id, event_type, occurred_at, payload, task, run_id, owner_workspace) = row?;
        let Some(category) = timeline::category_of(&aggregate_type, &event_type) else { continue };
        let task_ids: Vec<String> = if aggregate_type == RECOVERY_AGGREGATE {
            recovered.get(&aggregate_id).into_iter().flatten().filter(|t| in_workspace(t)).cloned().collect()
        } else {
            // 屬於別的 Workspace 的事件不上這個時間線；沒有 Task 的事件看它記下的 Workspace
            match task {
                Some(task) if in_workspace(&task) => {
                    let mut tasks = vec![task];
                    if aggregate_type == DECISION_AGGREGATE {
                        for other in blocked.get(&aggregate_id).into_iter().flatten() {
                            if !tasks.contains(other) && in_workspace(other) {
                                tasks.push(other.clone());
                            }
                        }
                    }
                    tasks
                }
                Some(_) => continue,
                None if owner_workspace.as_deref() == Some(workspace_id) => Vec::new(),
                None => continue,
            }
        };
        entries.push(TimelineEntry {
            seq,
            occurred_at: Timestamp::from_millis(occurred_at),
            category,
            aggregate_type,
            aggregate_id,
            event_type,
            task_ids,
            run_id,
            payload: json_column(5, &payload)?,
        });
    }
    Ok(match task_id {
        Some(task_id) => timeline::for_task(entries, task_id),
        None => entries,
    })
}
