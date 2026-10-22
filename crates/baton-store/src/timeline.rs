//! 鈭辣??蝺?霈??蟡?41嚗?敺蕭??鈭辣銵刻??箸???銝?鈭辣嚗圾??蝑?隞嗥?? Task ??Run??
//!
//! ??閬???Domain嚗baton_domain::timeline`嚗ask ?飛撅砌???瘙箏?嚗un?漱?乓?摰?瘙?????”嚗?
//! Task ??worktree 皜??????亙停??Task嚗??冽?雿??胯??鈭辣?批捆鋆∠? `task_id`嚗run_id`
//! 嚗??冽?雿?閰行? `task_id` ?????Task嚗?隞乩誑鈭辣?嗆???Run ?箸?嚗?
//! ?Ｗ儔鈭辣撅祆?游?Runtime嚗?曉瘥?Workspace ????銝???啣?隞颱?銝甈∪?閰西?蝵桅??惇?潮?Workspace ??Task??

use std::collections::HashMap;

use baton_domain::recovery::{AffectedRun, RECOVERY_AGGREGATE};
use baton_domain::timeline::{self, TimelineEntry, TIMELINE_AGGREGATES};
use rusqlite::OptionalExtension;

use super::*;

/// 霈?????仃??
#[derive(Debug, thiserror::Error)]
pub enum TimelineError {
    #[error("?曆???Workspace {0}")]
    WorkspaceNotFound(String),
    #[error("Workspace 銝剜???Task {0}")]
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
    /// Workspace ????嚗?鈭辣?漱??嚗?摰?Task ?????????隞嗚?
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
    let task_workspace: HashMap<String, String> = conn
        .prepare("SELECT id, workspace_id FROM task")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    if let Some(task_id) = task_id {
        if task_workspace.get(task_id).map(String::as_str) != Some(workspace_id) {
            return Err(TimelineError::TaskNotFound(task_id.to_string()));
        }
    }
    let in_workspace = |task: &String| task_workspace.get(task).map(String::as_str) == Some(workspace_id);

    // ?Ｗ儔鈭辣 ??摰?蝵桅???Task嚗??憚甈～???閰佗?
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
    let sql = format!(
        "SELECT e.seq, e.aggregate_type, e.aggregate_id, e.event_type, e.occurred_at, e.payload,
                COALESCE(r.task_id, h.task_id, d.task_id,
                         CASE WHEN e.aggregate_type IN ('task', 'worktree_cleanup') THEN e.aggregate_id END,
                         json_extract(e.payload, '$.task_id'), pr.task_id, x.task_id),
                COALESCE(CASE WHEN e.aggregate_type = 'run' THEN e.aggregate_id END, h.from_run_id, d.run_id,
                         json_extract(e.payload, '$.run_id'), x.run_id),
                lr.workspace_id
         FROM event e
         LEFT JOIN run r ON e.aggregate_type = 'run' AND r.id = e.aggregate_id
         LEFT JOIN handoff h ON e.aggregate_type = 'handoff' AND h.id = e.aggregate_id
         LEFT JOIN decision_request d ON e.aggregate_type = 'decision_request' AND d.id = e.aggregate_id
         LEFT JOIN external_operation x ON e.aggregate_type = 'external_operation' AND x.id = e.aggregate_id
         LEFT JOIN lesson_recurrence lr ON e.aggregate_type = 'lesson_recurrence' AND lr.id = e.aggregate_id
         LEFT JOIN run pr ON pr.id = json_extract(e.payload, '$.run_id')
         WHERE e.aggregate_type IN ({aggregates})
         ORDER BY e.seq"
    );
    let mut statement = conn.prepare(&sql)?;
    let rows = statement.query_map([], |r| {
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
        let (seq, aggregate_type, aggregate_id, event_type, occurred_at, payload, task, run_id, recurrence_workspace) = row?;
        let Some(category) = timeline::category_of(&aggregate_type, &event_type) else { continue };
        let task_ids: Vec<String> = if aggregate_type == RECOVERY_AGGREGATE {
            recovered.get(&aggregate_id).into_iter().flatten().filter(|t| in_workspace(t)).cloned().collect()
        } else {
            // 撅祆?亦? Workspace ??隞嗡?銝???嚗???Task ???舐?摰?銝? Workspace
            match task {
                Some(task) if in_workspace(&task) => vec![task],
                Some(_) => continue,
                None if recurrence_workspace.as_deref() == Some(workspace_id) => Vec::new(),
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
