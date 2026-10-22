//! 外部操作的持久化（票 26）。
//!
//! 規則都在 Domain（`baton_domain::external_operation`）；這裡只做「在同一個交易中載入目前狀態 → 交給 Domain 判定 →
//! 寫入操作、裁定請求、受影響的 Task 與事件」。另外兩個入口不是獨立的交易，而是掛在別的交易裡，確保不會有中間狀態：
//! - `settle_run_end`：Run 結束的同一個交易（`runs::save`），還沒回報結果的操作立刻變成結果未知、Task 停下，
//!   所以不會出現「Run 已結束、Task 回到 ready 被重新派出，操作卻還沒被標成結果未知」的空檔。
//! - `resolve_by_decision`：使用者回答裁定的同一個交易，查證結果與 Task 恢復一起提交。

use baton_domain::auth::AgentScope;
use baton_domain::decision::Decision;
use baton_domain::external_operation::{
    self, BeginOutcome, ExternalChange, ExternalCommand, ExternalContext, ExternalOperation, ExternalOperationError,
    ExternalOutcome, OperationRequest, OperationResult, OperationStatus,
};
use baton_domain::run::Run;

use super::tasks::{load, save_task};
use super::*;

/// 套用外部操作命令或查詢的失敗：Domain 拒絕（Worker 可修正）或儲存層錯誤。
#[derive(Debug, thiserror::Error)]
pub enum ExternalApplyError {
    #[error(transparent)]
    Rejected(#[from] ExternalOperationError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl From<rusqlite::Error> for ExternalApplyError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error.into())
    }
}

impl Store {
    /// Worker 送出外部操作之前登記（`begin_external_operation`）；範圍取自憑證。
    pub async fn begin_external_operation(
        &self,
        scope: &AgentScope,
        operation_id: String,
        request: OperationRequest,
        now: Timestamp,
    ) -> Result<BeginOutcome, ExternalApplyError> {
        let command = ExternalCommand::Begin {
            operation_id,
            run_id: scope.run_id.clone(),
            generation: scope.generation,
            request,
        };
        match self.apply_external_command(scope.workspace_id.clone(), command, now).await? {
            ExternalOutcome::Begin(outcome) => Ok(outcome),
            other => unreachable!("Begin 命令只會得到 Begin 結果：{other:?}"),
        }
    }

    /// Worker 回報外部操作的結果（`record_external_operation_result`）；回傳操作識別、新狀態與（結果未知時的）裁定請求。
    pub async fn record_external_operation_result(
        &self,
        scope: &AgentScope,
        idempotency_key: String,
        result: OperationResult,
        now: Timestamp,
    ) -> Result<(String, OperationStatus, Option<String>), ExternalApplyError> {
        let command = ExternalCommand::RecordResult {
            run_id: scope.run_id.clone(),
            generation: scope.generation,
            idempotency_key,
            result,
        };
        match self.apply_external_command(scope.workspace_id.clone(), command, now).await? {
            ExternalOutcome::Recorded { operation_id, status, decision_request_id } => {
                Ok((operation_id, status, decision_request_id))
            }
            other => unreachable!("RecordResult 命令只會得到 Recorded 結果：{other:?}"),
        }
    }

    /// Workspace 的全部外部操作，依建立順序。
    pub async fn external_operations(&self, workspace_id: &str) -> Result<Vec<ExternalOperation>, ExternalApplyError> {
        let workspace_id = workspace_id.to_string();
        let result = self
            .read(move |conn| {
                if !load_workspaces(conn)?.iter().any(|w| w.id == workspace_id) {
                    return Ok(Err(ExternalOperationError::WorkspaceNotFound(workspace_id)));
                }
                Ok(Ok(load_operations(conn, &workspace_id)?))
            })
            .await?;
        Ok(result?)
    }

    async fn apply_external_command(
        &self,
        workspace_id: String,
        command: ExternalCommand,
        now: Timestamp,
    ) -> Result<ExternalOutcome, ExternalApplyError> {
        let outcome = self.write(move |tx| Ok(apply_in(tx, &workspace_id, command, now)?)).await?;
        Ok(outcome?)
    }
}

/// 在目前的交易中判定並寫入；Domain 拒絕或沒有任何變動時不寫入任何東西。
fn apply_in(
    tx: &Transaction<'_>,
    workspace_id: &str,
    command: ExternalCommand,
    now: Timestamp,
) -> rusqlite::Result<Result<ExternalOutcome, ExternalOperationError>> {
    let loaded = load(tx, workspace_id)?;
    let operations = load_operations(tx, workspace_id)?;
    let pending_requests = load_pending_request_ids(tx, workspace_id)?;
    let task_ctx = loaded.ctx(workspace_id);
    let ctx = ExternalContext {
        task: &task_ctx,
        runs: &loaded.runs,
        leases: &loaded.leases,
        operations: &operations,
        pending_requests: &pending_requests,
    };
    match external_operation::decide(&ctx, command, now) {
        Err(rejected) => Ok(Err(rejected)),
        Ok(change) => {
            if change.changed() {
                save(tx, &change)?;
            }
            Ok(Ok(change.outcome))
        }
    }
}

/// Run 結束的同一個交易中：它還沒回報結果的外部操作變成結果未知（建立裁定請求、Task 停下）。
pub(crate) fn settle_run_end(tx: &Transaction<'_>, run: &Run, now: Timestamp) -> rusqlite::Result<()> {
    let unreported: i64 = tx.query_row(
        "SELECT COUNT(*) FROM external_operation WHERE run_id = ?1 AND status = 'in_progress'",
        [&run.id],
        |r| r.get(0),
    )?;
    if unreported == 0 {
        return Ok(());
    }
    // Domain 不會拒絕 RunEnded（Workspace 一定存在）；真的被拒絕時 Run 的結束本身仍然成立
    let _ = apply_in(tx, &run.workspace_id, ExternalCommand::RunEnded { run_id: run.id.clone() }, now)?;
    Ok(())
}

/// 使用者回答裁定的同一個交易中：若這是外部操作「結果未知」的請求，依查證結果結案。
pub(crate) fn resolve_by_decision(
    tx: &Transaction<'_>,
    workspace_id: &str,
    decision: &Decision,
    now: Timestamp,
) -> rusqlite::Result<()> {
    let linked: i64 = tx.query_row(
        "SELECT COUNT(*) FROM external_operation WHERE decision_request_id = ?1",
        [&decision.request_id],
        |r| r.get(0),
    )?;
    if linked == 0 {
        return Ok(());
    }
    let command = ExternalCommand::Verified {
        request_id: decision.request_id.clone(),
        option_id: decision.option_id.clone(),
        note: decision.note.clone(),
    };
    let _ = apply_in(tx, workspace_id, command, now)?;
    Ok(())
}

const OPERATION_COLUMNS: &str = "id, workspace_id, task_id, run_id, idempotency_key, operation, target, request, \
     request_hash, status, attempt, receipt, failure_reason, unknown_reason, decision_request_id, version, \
     created_at, updated_at";

fn operation_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ExternalOperation> {
    let status: String = r.get(9)?;
    Ok(ExternalOperation {
        id: r.get(0)?,
        workspace_id: r.get(1)?,
        task_id: r.get(2)?,
        run_id: r.get(3)?,
        idempotency_key: r.get(4)?,
        operation: r.get(5)?,
        target: r.get(6)?,
        request: json_column(7, &r.get::<_, String>(7)?)?,
        request_hash: r.get(8)?,
        status: OperationStatus::parse(&status).ok_or_else(|| {
            rusqlite::Error::FromSqlConversionFailure(9, rusqlite::types::Type::Text, "未知的外部操作狀態".into())
        })?,
        attempt: r.get(10)?,
        receipt: r.get(11)?,
        failure_reason: r.get(12)?,
        unknown_reason: r.get(13)?,
        decision_request_id: r.get(14)?,
        version: r.get(15)?,
        created_at: Timestamp::from_millis(r.get(16)?),
        updated_at: Timestamp::from_millis(r.get(17)?),
    })
}

fn load_operations(conn: &Connection, workspace_id: &str) -> rusqlite::Result<Vec<ExternalOperation>> {
    conn.prepare(&format!("SELECT {OPERATION_COLUMNS} FROM external_operation WHERE workspace_id = ?1 ORDER BY rowid"))?
        .query_map([workspace_id], operation_row)?
        .collect()
}

fn load_pending_request_ids(conn: &Connection, workspace_id: &str) -> rusqlite::Result<Vec<String>> {
    conn.prepare("SELECT id FROM decision_request WHERE workspace_id = ?1 AND status = 'pending' ORDER BY rowid")?
        .query_map([workspace_id], |r| r.get(0))?
        .collect()
}

/// 寫入同一個交易：先寫裁定請求（操作以外鍵引用它），再寫操作、阻擋範圍、Task 與事件。
fn save(tx: &Transaction<'_>, change: &ExternalChange) -> rusqlite::Result<()> {
    for request in &change.requests {
        super::decisions::insert_request(tx, request)?;
    }
    for operation in &change.operations {
        // Domain 型別只含字串與數字，序列化不會失敗
        let request = serde_json::to_string(&operation.request).expect("外部操作的參數可序列化");
        tx.execute(
            &format!(
                "INSERT INTO external_operation ({OPERATION_COLUMNS})
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)
                 ON CONFLICT(id) DO UPDATE SET
                   task_id = excluded.task_id, run_id = excluded.run_id, status = excluded.status,
                   attempt = excluded.attempt, receipt = excluded.receipt, failure_reason = excluded.failure_reason,
                   unknown_reason = excluded.unknown_reason, decision_request_id = excluded.decision_request_id,
                   version = excluded.version, updated_at = excluded.updated_at"
            ),
            rusqlite::params![
                operation.id,
                operation.workspace_id,
                operation.task_id,
                operation.run_id,
                operation.idempotency_key,
                operation.operation,
                operation.target,
                request,
                operation.request_hash,
                operation.status.as_str(),
                operation.attempt,
                operation.receipt,
                operation.failure_reason,
                operation.unknown_reason,
                operation.decision_request_id,
                operation.version,
                operation.created_at.as_millis(),
                operation.updated_at.as_millis(),
            ],
        )?;
    }
    for (request_id, task_id) in &change.blocks {
        tx.execute(
            "INSERT OR IGNORE INTO decision_request_block (request_id, task_id) VALUES (?1, ?2)",
            rusqlite::params![request_id, task_id],
        )?;
    }
    for task in &change.tasks {
        save_task(tx, task)?;
    }
    for event in &change.events {
        append_event(tx, event)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
