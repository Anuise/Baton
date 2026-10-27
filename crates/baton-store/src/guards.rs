//! Guard 與重犯事件的持久化（票 38）。
//!
//! 規則都在 Domain（`baton_domain::guard`）；這裡只做「在同一個交易中載入 Guard 與來源教訓 → Domain 判定 → 寫回 Guard 與事件」，
//! 以及派工路徑需要的「現在要檢查哪些 Guard」與排程時記錄被攔下的重犯。

use baton_domain::guard::{
    self, ActiveGuard, Guard, GuardCommand, GuardError, GuardRule, GuardStatus, Recurrence,
    RecurrenceKind,
};
use baton_domain::knowledge::{Audience, KnowledgeEntry, KnowledgeError};
use baton_domain::scheduler::WaitReason;
use baton_domain::task::Task;

use super::knowledge::load_entry;
use super::*;

/// 套用 Guard 變更的失敗：Domain 拒絕、來源教訓不存在、或儲存層錯誤。
#[derive(Debug, thiserror::Error)]
pub enum GuardApplyError {
    #[error(transparent)]
    Rejected(#[from] GuardError),
    #[error(transparent)]
    Knowledge(#[from] KnowledgeError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl From<rusqlite::Error> for GuardApplyError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error.into())
    }
}

/// 列表上的一個 Guard：本身、來源教訓（目前狀態）與成效。
#[derive(Debug, Clone, PartialEq)]
pub struct GuardOverview {
    pub guard: Guard,
    pub lesson: KnowledgeEntry,
    /// 被這個 Guard 攔下的重犯次數。
    pub prevented_count: i64,
    pub last_prevented_at: Option<Timestamp>,
    /// Guard 建立之後，同一個教訓沒被攔下、由使用者回報的重犯次數。
    pub reported_count: i64,
}

impl Store {
    /// 使用者把教訓轉成 Guard（建立即啟用）。
    pub async fn create_guard(
        &self,
        id: String,
        lesson_id: &str,
        rule: GuardRule,
        now: Timestamp,
    ) -> Result<GuardOverview, GuardApplyError> {
        let lesson_id = lesson_id.to_string();
        let outcome = self
            .write(move |tx| {
                let Some(lesson) = load_entry(tx, &lesson_id)? else {
                    return Ok(Err(KnowledgeError::NotFound(lesson_id).into()));
                };
                match guard::create(id, &lesson, rule, now) {
                    Err(rejected) => Ok(Err(rejected.into())),
                    Ok(change) => {
                        save_guard(tx, &change.guard)?;
                        append_event(tx, &change.event)?;
                        Ok(Ok(overview(tx, change.guard, lesson)?))
                    }
                }
            })
            .await?;
        outcome
    }

    /// 啟用或停用（以 expected_revision 校驗）；已經是目標狀態時不寫入。
    pub async fn apply_guard_command(
        &self,
        id: &str,
        expected_revision: i64,
        command: GuardCommand,
        now: Timestamp,
    ) -> Result<GuardOverview, GuardApplyError> {
        let id = id.to_string();
        let outcome = self
            .write(move |tx| {
                let Some(current) = load_guard(tx, &id)? else {
                    return Ok(Err(GuardError::NotFound(id).into()));
                };
                let lesson = load_entry(tx, &current.lesson_id)?.expect("Guard 的來源教訓有外鍵約束，一定存在");
                match guard::decide(&current, &lesson, expected_revision, command, now) {
                    Err(rejected) => Ok(Err(rejected.into())),
                    Ok(None) => Ok(Ok(overview(tx, current, lesson)?)),
                    Ok(Some(change)) => {
                        save_guard(tx, &change.guard)?;
                        append_event(tx, &change.event)?;
                        Ok(Ok(overview(tx, change.guard, lesson)?))
                    }
                }
            })
            .await?;
        outcome
    }

    /// 知識分頁看得到的 Guard：來源教訓是全域，或屬於指定的 Workspace；依建立順序。
    pub async fn guard_list(&self, workspace_id: Option<String>) -> StoreResult<Vec<GuardOverview>> {
        self.read(move |conn| {
            let audience = Audience::Desktop { workspace_id: workspace_id.as_deref() };
            let mut list = Vec::new();
            for guard in load_guards(conn, "", rusqlite::params![])? {
                let lesson = load_entry(conn, &guard.lesson_id)?.expect("Guard 的來源教訓有外鍵約束，一定存在");
                if audience.can_see(&lesson.scope) {
                    list.push(overview(conn, guard, lesson)?);
                }
            }
            Ok(list)
        })
        .await
    }

    /// 一個教訓的全部重犯事件，新到舊。
    pub async fn recurrences_of(&self, lesson_id: &str) -> Result<Vec<Recurrence>, GuardApplyError> {
        let lesson_id = lesson_id.to_string();
        let outcome = self
            .read(move |conn| {
                if load_entry(conn, &lesson_id)?.is_none() {
                    return Ok(Err(KnowledgeError::NotFound(lesson_id).into()));
                }
                let recurrences = conn
                    .prepare(&format!(
                        "SELECT {RECURRENCE_COLUMNS} FROM lesson_recurrence WHERE lesson_id = ?1
                         ORDER BY occurred_at DESC, rowid DESC"
                    ))?
                    .query_map([&lesson_id], recurrence_row)?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(Ok(recurrences))
            })
            .await?;
        outcome
    }

    /// 使用者回報一次沒被攔下的重犯：`scope` 是相關的 Workspace 與（選填的）其中一個 Task。
    pub async fn report_recurrence(
        &self,
        id: String,
        lesson_id: &str,
        scope: Option<(String, Option<String>)>,
        note: String,
        now: Timestamp,
    ) -> Result<Recurrence, GuardApplyError> {
        let lesson_id = lesson_id.to_string();
        let outcome = self
            .write(move |tx| {
                let Some(lesson) = load_entry(tx, &lesson_id)? else {
                    return Ok(Err(KnowledgeError::NotFound(lesson_id).into()));
                };
                let (workspace_id, task) = match scope {
                    None => (None, None),
                    Some((workspace_id, task_id)) => {
                        let loaded = super::tasks::load(tx, &workspace_id)?;
                        if loaded.workspace.is_none() {
                            return Ok(Err(GuardError::Invalid(format!("找不到 Workspace {workspace_id}")).into()));
                        }
                        let task = match task_id {
                            None => None,
                            Some(task_id) => match loaded.tasks.into_iter().find(|t| t.id == task_id) {
                                Some(task) => Some(task),
                                None => {
                                    let message = format!("Workspace {workspace_id} 沒有 Task {task_id}");
                                    return Ok(Err(GuardError::Invalid(message).into()));
                                }
                            },
                        };
                        (Some(workspace_id), task)
                    }
                };
                match guard::report(id, &lesson, workspace_id.as_deref(), task.as_ref(), &note, now) {
                    Err(rejected) => Ok(Err(rejected.into())),
                    Ok((recurrence, event)) => {
                        insert_recurrence(tx, &recurrence)?;
                        append_event(tx, &event)?;
                        Ok(Ok(recurrence))
                    }
                }
            })
            .await?;
        outcome
    }
}

/// 派工路徑（Run 准入）要檢查的 Guard：啟用、來源教訓現在有效，而且教訓是全域或屬於這個 Workspace。
/// 是否涵蓋某個 Task（Goal、Task 範圍）由 Domain 的 `guard::violations` 判定。
pub(crate) fn load_active_guards(conn: &Connection, workspace_id: &str, now: Timestamp) -> rusqlite::Result<Vec<ActiveGuard>> {
    let mut active = Vec::new();
    for guard in load_guards(
        conn,
        "JOIN knowledge_entry k ON k.id = g.lesson_id
         WHERE g.status = 'enabled' AND (k.workspace_id IS NULL OR k.workspace_id = ?1)",
        [workspace_id],
    )? {
        let lesson = load_entry(conn, &guard.lesson_id)?.expect("Guard 的來源教訓有外鍵約束，一定存在");
        if guard::inactive_reason(&guard, &lesson, now).is_none() {
            active.push(ActiveGuard { guard, lesson });
        }
    }
    Ok(active)
}

/// 排程時被 Guard 擋下：每個 Guard 原因記一筆「已攔下」的重犯（同一個 Guard、Task 與契約版本只記一次，事件也只追加一次）。
pub(crate) fn record_prevented(tx: &Transaction<'_>, task: &Task, reasons: &[WaitReason], now: Timestamp) -> rusqlite::Result<()> {
    for reason in reasons {
        let Some(source) = &reason.guard else { continue };
        let (recurrence, event) = guard::prevented(source, &reason.message, task, now);
        if insert_recurrence(tx, &recurrence)? {
            append_event(tx, &event)?;
        }
    }
    Ok(())
}

/// 寫入一筆重犯事件；同一個識別已存在時不寫（回傳 false）。
fn insert_recurrence(tx: &Transaction<'_>, recurrence: &Recurrence) -> rusqlite::Result<bool> {
    let inserted = tx.execute(
        "INSERT INTO lesson_recurrence (id, lesson_id, guard_id, workspace_id, task_id, contract_version, kind, message, occurred_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) ON CONFLICT(id) DO NOTHING",
        rusqlite::params![
            recurrence.id,
            recurrence.lesson_id,
            recurrence.guard_id,
            recurrence.workspace_id,
            recurrence.task_id,
            recurrence.contract_version,
            recurrence.kind.as_str(),
            recurrence.message,
            recurrence.occurred_at.as_millis(),
        ],
    )?;
    Ok(inserted == 1)
}

fn overview(conn: &Connection, guard: Guard, lesson: KnowledgeEntry) -> rusqlite::Result<GuardOverview> {
    let (prevented_count, last_prevented_at): (i64, Option<i64>) = conn.query_row(
        "SELECT COUNT(*), MAX(occurred_at) FROM lesson_recurrence WHERE guard_id = ?1",
        [&guard.id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let reported_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM lesson_recurrence WHERE lesson_id = ?1 AND kind = 'reported' AND occurred_at >= ?2",
        rusqlite::params![guard.lesson_id, guard.created_at.as_millis()],
        |r| r.get(0),
    )?;
    Ok(GuardOverview {
        guard,
        lesson,
        prevented_count,
        last_prevented_at: last_prevented_at.map(Timestamp::from_millis),
        reported_count,
    })
}

fn save_guard(tx: &Transaction<'_>, guard: &Guard) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO guard (id, lesson_id, rule, status, revision, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(id) DO UPDATE SET status = excluded.status, revision = excluded.revision, updated_at = excluded.updated_at",
        rusqlite::params![
            guard.id,
            guard.lesson_id,
            serde_json::to_string(&guard.rule).expect("規則一定可以序列化"),
            guard.status.as_str(),
            guard.revision,
            guard.created_at.as_millis(),
            guard.updated_at.as_millis(),
        ],
    )?;
    Ok(())
}

const GUARD_COLUMNS: &str = "g.id, g.lesson_id, g.rule, g.status, g.revision, g.created_at, g.updated_at";

/// `filter` 是接在 `FROM guard g` 之後的 JOIN／WHERE 子句；依建立順序。
fn load_guards<P: rusqlite::Params>(conn: &Connection, filter: &str, params: P) -> rusqlite::Result<Vec<Guard>> {
    conn.prepare(&format!("SELECT {GUARD_COLUMNS} FROM guard g {filter} ORDER BY g.rowid"))?
        .query_map(params, guard_row)?
        .collect()
}

fn load_guard(conn: &Connection, id: &str) -> rusqlite::Result<Option<Guard>> {
    Ok(load_guards(conn, "WHERE g.id = ?1", [id])?.pop())
}

fn guard_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Guard> {
    let status: String = r.get(3)?;
    Ok(Guard {
        id: r.get(0)?,
        lesson_id: r.get(1)?,
        rule: json_column(2, &r.get::<_, String>(2)?)?,
        status: GuardStatus::parse(&status).ok_or_else(|| {
            rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, "未知的 Guard 狀態".into())
        })?,
        revision: r.get(4)?,
        created_at: Timestamp::from_millis(r.get(5)?),
        updated_at: Timestamp::from_millis(r.get(6)?),
    })
}

const RECURRENCE_COLUMNS: &str =
    "id, lesson_id, guard_id, workspace_id, task_id, contract_version, kind, message, occurred_at";

fn recurrence_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Recurrence> {
    let kind: String = r.get(6)?;
    Ok(Recurrence {
        id: r.get(0)?,
        lesson_id: r.get(1)?,
        guard_id: r.get(2)?,
        workspace_id: r.get(3)?,
        task_id: r.get(4)?,
        contract_version: r.get(5)?,
        kind: RecurrenceKind::parse(&kind).ok_or_else(|| {
            rusqlite::Error::FromSqlConversionFailure(6, rusqlite::types::Type::Text, "未知的重犯種類".into())
        })?,
        message: r.get(7)?,
        occurred_at: Timestamp::from_millis(r.get(8)?),
    })
}

#[cfg(test)]
mod tests;
