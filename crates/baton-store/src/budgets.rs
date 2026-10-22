//! 修正與重試預算的持久化（票 36）。
//!
//! 規則都在 Domain（`baton_domain::budget`）；這裡只載入、寫入 Run 結束或驗證結論計入後的預算，以及在同一個交易中
//! 寫入用盡時建立的裁定請求、把基礎設施重試排回派工佇列。

use baton_domain::budget::{BudgetKind, TaskBudget};
use baton_domain::decision::DecisionRequest;
use baton_domain::task::Task;

use super::*;

/// Workspace 內全部有計入過的 Task 預算。
pub(crate) fn load_budgets(conn: &Connection, workspace_id: &str) -> rusqlite::Result<Vec<TaskBudget>> {
    conn.prepare(
        "SELECT task_id, implementation, verification, infrastructure, retry_not_before, exhausted, request_id,
                version, updated_at
         FROM task_budget WHERE workspace_id = ?1 ORDER BY rowid",
    )?
    .query_map([workspace_id], |r| {
        let exhausted = match r.get::<_, Option<String>>(5)? {
            Some(raw) => Some(BudgetKind::parse(&raw).ok_or_else(|| {
                rusqlite::Error::FromSqlConversionFailure(5, rusqlite::types::Type::Text, "未知的預算種類".into())
            })?),
            None => None,
        };
        Ok(TaskBudget {
            task_id: r.get(0)?,
            workspace_id: workspace_id.to_string(),
            implementation: r.get(1)?,
            verification: r.get(2)?,
            infrastructure: r.get(3)?,
            retry_not_before: r.get::<_, Option<i64>>(4)?.map(Timestamp::from_millis),
            exhausted,
            request_id: r.get(6)?,
            version: r.get(7)?,
            updated_at: Some(Timestamp::from_millis(r.get(8)?)),
        })
    })?
    .collect()
}

/// 寫入計入後的預算；用盡時的裁定請求要先寫入（預算列參照它）。
pub(crate) fn save_budget(
    tx: &Transaction<'_>,
    budget: &TaskBudget,
    request: Option<&DecisionRequest>,
    now: Timestamp,
) -> rusqlite::Result<()> {
    if let Some(request) = request {
        super::decisions::insert_request(tx, request)?;
    }
    tx.execute(
        "INSERT INTO task_budget (task_id, workspace_id, implementation, verification, infrastructure, retry_not_before,
             exhausted, request_id, version, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(task_id) DO UPDATE SET
           implementation = excluded.implementation, verification = excluded.verification,
           infrastructure = excluded.infrastructure, retry_not_before = excluded.retry_not_before,
           exhausted = excluded.exhausted, request_id = excluded.request_id, version = excluded.version,
           updated_at = excluded.updated_at",
        rusqlite::params![
            budget.task_id,
            budget.workspace_id,
            budget.implementation,
            budget.verification,
            budget.infrastructure,
            budget.retry_not_before.map(|t| t.as_millis()),
            budget.exhausted.map(BudgetKind::as_str),
            budget.request_id,
            budget.version,
            budget.updated_at.unwrap_or(now).as_millis(),
        ],
    )?;
    Ok(())
}

/// 基礎設施重試在預算內：把回到 `ready` 的 Task 排回派工佇列（排程會等到退避期限）。
/// 原本派工的優先序沒有保存，一律用派工 API 的預設值 0；已在佇列中的保留原本的位置與優先序。
pub(crate) fn requeue(tx: &Transaction<'_>, task: &Task, now: Timestamp) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO dispatch_request (task_id, workspace_id, priority, requested_at)
         VALUES (?1, ?2, 0, ?3)
         ON CONFLICT(task_id) DO NOTHING",
        rusqlite::params![task.id, task.workspace_id, now.as_millis()],
    )?;
    Ok(())
}
