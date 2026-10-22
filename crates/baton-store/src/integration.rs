//! 單一整合佇列的持久化（票 20）。
//!
//! 規則都在 Domain（`baton_domain::integration`）；這裡只做「在同一個交易中載入項目、它的 Task 與 Workspace →
//! 交給 Domain 判定 → 寫入項目、Task 新狀態與事件」，以及給 Supervisor 與面板的唯讀查詢。
//! 「任何時刻最多一個處理中的項目」除了 Domain 依序處理之外，還由資料表的唯一索引保證。

use baton_domain::artifact::Artifact;
use baton_domain::integration::{
    self, IntegrationChange, IntegrationCommand, IntegrationContext, IntegrationEntry, IntegrationError,
    IntegrationQueue, IntegrationStatus,
};
use baton_domain::verification::{CheckResult, ManifestEntry, VerificationCheck};

use super::artifacts::load_artifacts;
use super::tasks::{load, save_task};
use super::*;

/// 套用整合命令的失敗：Domain 拒絕或儲存層錯誤。
#[derive(Debug, thiserror::Error)]
pub enum IntegrationApplyError {
    #[error(transparent)]
    Rejected(#[from] IntegrationError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl From<rusqlite::Error> for IntegrationApplyError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error.into())
    }
}

/// 佇列中下一個要處理的項目，附上處理需要的事實。
#[derive(Debug, Clone, PartialEq)]
pub struct IntegrationWork {
    pub entry: IntegrationEntry,
    pub workspace: Workspace,
    /// Task 目前契約宣告的確定性檢查（整合驗證執行同一份清單）。
    pub checks: Vec<VerificationCheck>,
    /// Task 目前的成果 manifest 與成果（成果檢查據此讀取檔案）。
    pub manifest: Vec<ManifestEntry>,
    pub artifacts: Vec<Artifact>,
}

impl Store {
    /// 全域整合佇列：目前處理中的項目與全部項目（依排入順序）。
    pub async fn integration_queue(&self) -> StoreResult<IntegrationQueue> {
        self.read(|conn| {
            let entries = load_entries(conn, None)?;
            let active = entries.iter().find(|e| e.status.is_active()).map(|e| e.id.clone());
            Ok(IntegrationQueue { active, entries })
        })
        .await
    }

    /// 佇列中下一個要處理的項目（處理中的優先，否則最早排入的）；沒有時為 `None`。
    pub async fn next_integration(&self) -> StoreResult<Option<IntegrationWork>> {
        self.read(|conn| {
            let entries = load_entries(conn, None)?;
            let Some(entry) = integration::next(&entries).cloned() else { return Ok(None) };
            let loaded = load(conn, &entry.workspace_id)?;
            let Some(workspace) = loaded.workspace.clone() else { return Ok(None) };
            let task = loaded.tasks.iter().find(|t| t.id == entry.task_id);
            let checks = task
                .and_then(|t| loaded.contracts.iter().find(|c| c.task_id == t.id && c.version == t.contract_version))
                .map(|c| c.content.verification_checks.clone())
                .unwrap_or_default();
            let artifacts: Vec<Artifact> = load_artifacts(conn, &entry.workspace_id, None)?
                .into_iter()
                .filter(|a| a.task_id == entry.task_id)
                .collect();
            let manifest = baton_domain::verification::manifest_of(&artifacts, &entry.task_id);
            Ok(Some(IntegrationWork { entry, workspace, checks, manifest, artifacts }))
        })
        .await
    }

    /// 套用一個整合命令：Domain 拒絕時什麼都不寫。
    pub async fn apply_integration(
        &self,
        command: IntegrationCommand,
        now: Timestamp,
    ) -> Result<IntegrationChange, IntegrationApplyError> {
        let outcome = self
            .write(move |tx| {
                let id = match &command {
                    IntegrationCommand::BeginMerge { id }
                    | IntegrationCommand::RecordMerge { id, .. }
                    | IntegrationCommand::RecordChecks { id, .. }
                    | IntegrationCommand::RecordAdvance { id, .. } => id.clone(),
                };
                let Some(entry) = load_entries(tx, None)?.into_iter().find(|e| e.id == id) else {
                    return Ok(Err(IntegrationError::NotFound(id)));
                };
                let loaded = load(tx, &entry.workspace_id)?;
                let task = loaded.tasks.iter().find(|t| t.id == entry.task_id);
                let checks = task
                    .and_then(|t| loaded.contracts.iter().find(|c| c.task_id == t.id && c.version == t.contract_version))
                    .map(|c| c.content.verification_checks.clone())
                    .unwrap_or_default();
                let ctx = IntegrationContext { entry: &entry, workspace: loaded.workspace.as_ref(), task, checks: &checks };
                match integration::decide(&ctx, command, now) {
                    Err(rejected) => Ok(Err(rejected)),
                    Ok(change) => {
                        update_entry(tx, &change.entry)?;
                        if let Some(task) = &change.task {
                            save_task(tx, task)?;
                        }
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

const ENTRY_COLUMNS: &str = "seq, id, workspace_id, task_id, run_id, generation, branch, target_branch, status, version,
    merge_commit, target_tip, checks, failure, queued_at, started_at, finished_at";

/// 整合項目，依排入順序；`workspace_id` 為 `None` 時是全域佇列。
pub(crate) fn load_entries(conn: &Connection, workspace_id: Option<&str>) -> rusqlite::Result<Vec<IntegrationEntry>> {
    let sql = format!(
        "SELECT {ENTRY_COLUMNS} FROM integration_entry WHERE ?1 IS NULL OR workspace_id = ?1 ORDER BY seq"
    );
    conn.prepare(&sql)?
        .query_map([workspace_id], |r| {
            let status: String = r.get(8)?;
            Ok(IntegrationEntry {
                seq: r.get(0)?,
                id: r.get(1)?,
                workspace_id: r.get(2)?,
                task_id: r.get(3)?,
                run_id: r.get(4)?,
                generation: r.get(5)?,
                branch: r.get(6)?,
                target_branch: r.get(7)?,
                status: IntegrationStatus::parse(&status).ok_or_else(|| {
                    rusqlite::Error::FromSqlConversionFailure(8, rusqlite::types::Type::Text, "未知的整合狀態".into())
                })?,
                version: r.get(9)?,
                merge_commit: r.get(10)?,
                target_tip: r.get(11)?,
                checks: json_column::<Vec<CheckResult>>(12, &r.get::<_, String>(12)?)?,
                failure: r.get(13)?,
                queued_at: Timestamp::from_millis(r.get(14)?),
                started_at: r.get::<_, Option<i64>>(15)?.map(Timestamp::from_millis),
                finished_at: r.get::<_, Option<i64>>(16)?.map(Timestamp::from_millis),
            })
        })?
        .collect()
}

/// 寫入新排入的項目，回傳 Store 指派的排入順序。
pub(crate) fn insert_entry(tx: &Transaction<'_>, entry: &IntegrationEntry) -> rusqlite::Result<i64> {
    // Domain 型別只含字串與數字，序列化不會失敗
    let checks = serde_json::to_string(&entry.checks).expect("整合檢查結果可序列化");
    tx.execute(
        "INSERT INTO integration_entry
         (id, workspace_id, task_id, run_id, generation, branch, target_branch, status, version,
          merge_commit, target_tip, checks, failure, queued_at, started_at, finished_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
        rusqlite::params![
            entry.id,
            entry.workspace_id,
            entry.task_id,
            entry.run_id,
            entry.generation,
            entry.branch,
            entry.target_branch,
            entry.status.as_str(),
            entry.version,
            entry.merge_commit,
            entry.target_tip,
            checks,
            entry.failure,
            entry.queued_at.as_millis(),
            entry.started_at.map(Timestamp::as_millis),
            entry.finished_at.map(Timestamp::as_millis),
        ],
    )?;
    Ok(tx.last_insert_rowid())
}

fn update_entry(tx: &Transaction<'_>, entry: &IntegrationEntry) -> rusqlite::Result<()> {
    let checks = serde_json::to_string(&entry.checks).expect("整合檢查結果可序列化");
    tx.execute(
        "UPDATE integration_entry SET status = ?2, version = ?3, merge_commit = ?4, target_tip = ?5, checks = ?6,
           failure = ?7, started_at = ?8, finished_at = ?9
         WHERE id = ?1",
        rusqlite::params![
            entry.id,
            entry.status.as_str(),
            entry.version,
            entry.merge_commit,
            entry.target_tip,
            checks,
            entry.failure,
            entry.started_at.map(Timestamp::as_millis),
            entry.finished_at.map(Timestamp::as_millis),
        ],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const T1: Timestamp = Timestamp::from_millis(1_000);

    /// 直接寫入兩個項目，確認資料表層級的保證：處理中的項目最多一個。
    #[tokio::test]
    async fn the_table_allows_at_most_one_active_entry() {
        let dir = tempfile::tempdir().unwrap();
        drop(Store::open(dir.path(), T1).await.unwrap());
        let conn = Connection::open(dir.path().join(DB_FILE_NAME)).unwrap();
        conn.execute_batch(
            "PRAGMA foreign_keys = OFF;
             INSERT INTO integration_entry (id, workspace_id, task_id, run_id, generation, branch, target_branch,
               status, version, checks, queued_at)
             VALUES ('a', 'w', 't1', 'r1', 1, 'baton/t1', 'main', 'merging', 2, '[]', 1),
                    ('b', 'w', 't2', 'r2', 1, 'baton/t2', 'main', 'queued', 1, '[]', 1);",
        )
        .unwrap();

        let second = conn.execute("UPDATE integration_entry SET status = 'verifying' WHERE id = 'b'", []);
        assert!(second.is_err(), "第二個處理中的項目應被唯一索引拒絕");
        conn.execute("UPDATE integration_entry SET status = 'integrated' WHERE id = 'a'", []).unwrap();
        conn.execute("UPDATE integration_entry SET status = 'merging' WHERE id = 'b'", []).unwrap();
    }
}
