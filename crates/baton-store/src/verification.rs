//! 驗證紀錄與成果接受交易的持久化（票 13）。
//!
//! 規則都在 Domain（`baton_domain::verification`）；這裡只做「在同一個交易中載入 Task、Run、成果 manifest →
//! 交給 Domain 校驗與判定 → 寫入 Task 新狀態、驗證紀錄與事件」，以及給 Supervisor 與面板的唯讀查詢。
//! 驗證紀錄是追加式資料表（觸發器禁止更新與刪除）；同一個 Run 最多一筆紀錄，同一個 Task 最多一筆「通過」。

use baton_domain::artifact::Artifact;
use baton_domain::run::{self, LaunchPayload};
use baton_domain::task::TaskStatus;
use baton_domain::verification::{
    self, AcceptanceChange, AcceptanceCommand, AcceptanceContext, AcceptanceError, CheckResult, ManifestEntry,
    PendingCheck, VerificationMethod, VerificationOutcome, VerificationRecord, VerificationView,
};
use serde::{Deserialize, Serialize};

use super::artifacts::load_artifacts;
use super::tasks::{load, save_task};
use super::*;

/// 套用接受命令或查詢的失敗：Domain 拒絕（過期、狀態不符等）或儲存層錯誤。
#[derive(Debug, thiserror::Error)]
pub enum AcceptanceApplyError {
    #[error(transparent)]
    Rejected(#[from] AcceptanceError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl From<rusqlite::Error> for AcceptanceApplyError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error.into())
    }
}

/// 需要 Runtime 執行確定性檢查的 Task，附上執行檢查需要的事實。
#[derive(Debug, Clone, PartialEq)]
pub struct PendingVerification {
    pub workspace_id: String,
    pub pending: PendingCheck,
    /// 受驗 Run 派工時固定下來的工作目錄（命令與檔案檢查都在這裡）。
    pub working_dir: String,
    /// 這個 Task 的成果（含各版本的儲存位置），成果檢查據此讀取檔案。
    pub artifacts: Vec<Artifact>,
}

/// `task_verification.evidence` 欄位的內容。
#[derive(Serialize, Deserialize)]
struct Evidence {
    checks: Vec<CheckResult>,
    artifacts: Vec<ManifestEntry>,
    accepted_by: Option<String>,
    note: Option<String>,
}

impl Store {
    /// 成果接受交易：在一個交易中校驗 Run 世代、契約版本與成果 manifest，通過才寫入 Task、驗證紀錄與事件；
    /// Domain 拒絕時什麼都不寫。
    pub async fn apply_acceptance(
        &self,
        workspace_id: String,
        command: AcceptanceCommand,
        record_id: String,
        now: Timestamp,
    ) -> Result<AcceptanceChange, AcceptanceApplyError> {
        let outcome = self
            .write(move |tx| {
                let loaded = load(tx, &workspace_id)?;
                let artifacts = load_artifacts(tx, &workspace_id, None)?;
                let task_ctx = loaded.ctx(&workspace_id);
                let ctx = AcceptanceContext { task: &task_ctx, runs: &loaded.runs, artifacts: &artifacts };
                match verification::decide(&ctx, command, record_id, now) {
                    Err(rejected) => Ok(Err(rejected)),
                    Ok(change) => {
                        save_task(tx, &change.task)?;
                        insert_record(tx, &change.record)?;
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

    /// 全部 Workspace 中等待 Runtime 執行確定性檢查的 Task（依 Workspace 與 Task 的建立順序）。
    pub async fn pending_verifications(&self) -> StoreResult<Vec<PendingVerification>> {
        self.read(|conn| {
            let workspaces = conn
                .prepare("SELECT DISTINCT workspace_id FROM task WHERE status = ?1 ORDER BY workspace_id")?
                .query_map([TaskStatus::Verifying.as_str()], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let mut found = Vec::new();
            for workspace_id in workspaces {
                let loaded = load(conn, &workspace_id)?;
                let artifacts = load_artifacts(conn, &workspace_id, None)?;
                let ctx = loaded.ctx(&workspace_id);
                for task in loaded.tasks.iter().filter(|t| t.status == TaskStatus::Verifying) {
                    let Some(pending) = verification::pending_check(&ctx, &loaded.runs, &artifacts, task) else {
                        continue;
                    };
                    let Some(payload) = launch_payload(conn, &pending.basis.run_id)? else { continue };
                    found.push(PendingVerification {
                        workspace_id: workspace_id.clone(),
                        working_dir: payload.working_dir,
                        artifacts: artifacts.iter().filter(|a| a.task_id == task.id).cloned().collect(),
                        pending,
                    });
                }
            }
            Ok(found)
        })
        .await
    }

    /// 一個 Task 的驗證狀況（面板檢視與手動接受用）。
    pub async fn task_verification(
        &self,
        workspace_id: &str,
        task_id: &str,
    ) -> Result<VerificationView, AcceptanceApplyError> {
        let (workspace_id, task_id) = (workspace_id.to_string(), task_id.to_string());
        let result = self
            .read(move |conn| {
                let loaded = load(conn, &workspace_id)?;
                if loaded.workspace.is_none() {
                    return Ok(Err(AcceptanceError::WorkspaceNotFound(workspace_id)));
                }
                let Some(task) = loaded.tasks.iter().find(|t| t.id == task_id) else {
                    return Ok(Err(AcceptanceError::TaskNotFound(task_id)));
                };
                let artifacts = load_artifacts(conn, &workspace_id, None)?;
                let records = load_verifications(conn, &workspace_id)?;
                Ok(Ok(verification::view(&loaded.ctx(&workspace_id), &loaded.runs, &artifacts, &records, task)))
            })
            .await?;
        Ok(result?)
    }
}

fn launch_payload(conn: &Connection, run_id: &str) -> rusqlite::Result<Option<LaunchPayload>> {
    let mut rows = conn
        .prepare("SELECT payload FROM outbox WHERE kind = ?1 AND run_id = ?2")?
        .query_map([run::OUTBOX_LAUNCH_RUN, run_id], |r| json_column::<LaunchPayload>(0, &r.get::<_, String>(0)?))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows.pop())
}

fn insert_record(tx: &Transaction<'_>, record: &VerificationRecord) -> rusqlite::Result<()> {
    let evidence = Evidence {
        checks: record.checks.clone(),
        artifacts: record.artifacts.clone(),
        accepted_by: record.accepted_by.clone(),
        note: record.note.clone(),
    };
    // Domain 型別只含字串與數字，序列化不會失敗
    let evidence = serde_json::to_string(&evidence).expect("驗證證據可序列化");
    tx.execute(
        "INSERT INTO task_verification
         (id, workspace_id, task_id, run_id, generation, contract_version, method, outcome, evidence, decided_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        rusqlite::params![
            record.id,
            record.workspace_id,
            record.task_id,
            record.run_id,
            record.generation,
            record.contract_version,
            record.method.as_str(),
            record.outcome.as_str(),
            evidence,
            record.decided_at.as_millis(),
        ],
    )?;
    Ok(())
}

/// Workspace 內的全部驗證紀錄，由舊到新。
pub(crate) fn load_verifications(conn: &Connection, workspace_id: &str) -> rusqlite::Result<Vec<VerificationRecord>> {
    conn.prepare(
        "SELECT id, task_id, run_id, generation, contract_version, method, outcome, evidence, decided_at
         FROM task_verification WHERE workspace_id = ?1 ORDER BY rowid",
    )?
    .query_map([workspace_id], |r| {
        let invalid = |index: usize, what: &str| {
            rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Text, what.to_string().into())
        };
        let method: String = r.get(5)?;
        let outcome: String = r.get(6)?;
        let evidence: Evidence = json_column(7, &r.get::<_, String>(7)?)?;
        Ok(VerificationRecord {
            id: r.get(0)?,
            workspace_id: workspace_id.to_string(),
            task_id: r.get(1)?,
            run_id: r.get(2)?,
            generation: r.get(3)?,
            contract_version: r.get(4)?,
            method: VerificationMethod::parse(&method).ok_or_else(|| invalid(5, "未知的驗證方式"))?,
            outcome: VerificationOutcome::parse(&outcome).ok_or_else(|| invalid(6, "未知的驗證結果"))?,
            checks: evidence.checks,
            artifacts: evidence.artifacts,
            accepted_by: evidence.accepted_by,
            note: evidence.note,
            decided_at: Timestamp::from_millis(r.get(8)?),
        })
    })?
    .collect()
}
