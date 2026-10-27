//! Context Builder 的資料取得與 context receipt 的保存（票 39）。
//!
//! 組裝規則都在 Domain（`baton_domain::context`）；這裡只在派工的同一個交易中取得輸入、
//! 把組好的 Prompt 寫回派工 Outbox，並保存 receipt。receipt 只追加、不修改。

use baton_domain::context::{self, ContextInputs, ContextReceipt, EvidenceItem, DEFAULT_BUDGET_TOKENS};
use baton_domain::handoff::{self, Handoff};
use baton_domain::run::{RunChange, OUTBOX_LAUNCH_RUN};
use rusqlite::OptionalExtension;

use super::tasks::Loaded;
use super::*;

/// 讀取 Run 的 context receipt 失敗。
#[derive(Debug, thiserror::Error)]
pub enum ContextApplyError {
    #[error("找不到 Run：{0}")]
    RunNotFound(String),
    #[error("Run {0} 沒有 context receipt（在 Context Builder 之前建立）")]
    ReceiptNotFound(String),
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// 派工（產生派工 Outbox 的變更）時以 Context Builder 組裝完整 Prompt，並保存 context receipt。
/// `loaded` 是做出這次派工判定時載入的狀態；`handoff` 是 successor Run 承接的交接。其他變更什麼都不做。
pub(crate) fn attach_context(
    tx: &Transaction<'_>,
    loaded: &Loaded,
    change: &mut RunChange,
    handoff: Option<&Handoff>,
    now: Timestamp,
) -> rusqlite::Result<()> {
    let (Some(outbox), Some(run)) = (change.outbox.as_mut(), change.run.as_ref()) else {
        return Ok(());
    };
    let Some(task) = loaded.tasks.iter().find(|t| t.id == outbox.payload.task_id) else {
        return Ok(());
    };
    let workspace_id = &run.workspace_id;
    let checkpoint = handoff::latest_checkpoint(&loaded.reports, &loaded.runs, &task.id);
    let knowledge = super::knowledge::load_verified_guidance(tx, workspace_id)?;
    let (unread, unread_total) = super::messages::unread_summary(tx, workspace_id, &task.id)?;
    let evidence = load_evidence(tx, workspace_id, &task.id)?;
    let stage = super::workflows::worker_stage_item(tx, &task.id)?;
    let charter = super::exploration::latest_charter(tx, &task.id)?;
    let built = context::build(&ContextInputs {
        run_id: &run.id,
        task,
        contract: &outbox.payload.prompt,
        stage: stage.as_ref(),
        charter: charter.as_ref(),
        handoff,
        checkpoint: checkpoint.as_ref(),
        knowledge: &knowledge,
        unread: &unread,
        unread_total,
        evidence: &evidence,
        budget_tokens: DEFAULT_BUDGET_TOKENS,
        now,
    });
    if built.messages_loaded {
        super::messages::mark_summary_delivered(tx, workspace_id, &task.id, &unread, now)?;
    }
    outbox.payload.prompt = built.prompt;
    // run 列在同一個交易稍後才寫入：外鍵延到提交時檢查
    tx.execute(
        "INSERT INTO context_receipt (run_id, workspace_id, receipt, created_at) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![
            run.id,
            workspace_id,
            serde_json::to_string(&built.receipt).expect("receipt 一定可以序列化"),
            now.as_millis(),
        ],
    )?;
    Ok(())
}

/// 這個 Task 與它直接依賴的 Task 已登記的成果（各取最新版本）；這個 Task 自己的在前。
fn load_evidence(conn: &Connection, workspace_id: &str, task_id: &str) -> rusqlite::Result<Vec<EvidenceItem>> {
    conn.prepare(
        "SELECT a.id, a.name, t.title, v.version, v.sha256, v.description
         FROM artifact a
         JOIN task t ON t.id = a.task_id
         JOIN artifact_version v ON v.artifact_id = a.id
             AND v.version = (SELECT MAX(version) FROM artifact_version WHERE artifact_id = a.id)
         WHERE a.workspace_id = ?1
           AND (a.task_id = ?2 OR a.task_id IN (SELECT depends_on FROM task_dependency WHERE task_id = ?2))
         ORDER BY a.task_id <> ?2, a.rowid",
    )?
    .query_map(rusqlite::params![workspace_id, task_id], |r| {
        Ok(EvidenceItem {
            artifact_id: r.get(0)?,
            name: r.get(1)?,
            task_title: r.get(2)?,
            version: r.get(3)?,
            sha256: r.get(4)?,
            description: r.get(5)?,
        })
    })?
    .collect()
}

impl Store {
    /// Run 的 context receipt 與實際交給 Worker 的 Prompt（派工 Outbox 中固定的那一份）。
    pub async fn run_context(
        &self,
        workspace_id: &str,
        run_id: &str,
    ) -> Result<(ContextReceipt, String), ContextApplyError> {
        let (workspace_id, run_id) = (workspace_id.to_string(), run_id.to_string());
        let result = self
            .read(move |conn| {
                let found = conn
                    .query_row(
                        "SELECT c.receipt, o.payload FROM context_receipt c
                         JOIN outbox o ON o.run_id = c.run_id AND o.kind = ?3
                         WHERE c.run_id = ?1 AND c.workspace_id = ?2",
                        rusqlite::params![run_id, workspace_id, OUTBOX_LAUNCH_RUN],
                        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
                    )
                    .optional()?;
                let Some((receipt, payload)) = found else {
                    let exists = conn
                        .query_row(
                            "SELECT 1 FROM run WHERE id = ?1 AND workspace_id = ?2",
                            [&run_id, &workspace_id],
                            |_| Ok(()),
                        )
                        .optional()?
                        .is_some();
                    return Ok(Err(if exists {
                        ContextApplyError::ReceiptNotFound(run_id)
                    } else {
                        ContextApplyError::RunNotFound(run_id)
                    }));
                };
                let receipt: ContextReceipt = json_column(0, &receipt)?;
                let payload: baton_domain::run::LaunchPayload = json_column(1, &payload)?;
                Ok(Ok((receipt, payload.prompt)))
            })
            .await?;
        result
    }
}
