//! Agent API 需要的持久化（票 11）：依憑證範圍讀取 Run 的契約，以及 Worker 回報的套用與稽核。
//!
//! 範圍（Workspace、Run、世代）一律來自憑證，不是請求；這裡所有查詢都以它為界。
//! 來自已不是寫入所有者的世代的回報：Domain 拒絕它、不改變任何狀態，但同一個交易會把它留在稽核紀錄裡（`accepted = 0`）。

use baton_domain::agent::{ReportContent, ReportRecord};
use baton_domain::auth::AgentScope;
use baton_domain::run::{self, Run, RunChange, RunCommand, RunError};
use baton_domain::task::{Task, TaskContract};

use super::runs::{decide_in, save, RunApplyError};
use super::tasks::{load, load_contract_history};
use super::*;

/// Agent 憑證所屬 Run 的資料：Run、它的 Task，以及派工當下固定的契約版本。
#[derive(Debug, Clone, PartialEq)]
pub struct AgentContext {
    pub run: Run,
    pub task: Task,
    pub contract: TaskContract,
    /// Run 仍是這個 Task 目前的寫入所有者（尚未結束、持有 lease、世代相符）。
    pub is_current: bool,
}

impl Store {
    /// 憑證所屬 Run 的資料；範圍內找不到 Run 時回報 `RunNotFound`。
    pub async fn agent_context(&self, scope: &AgentScope) -> Result<AgentContext, RunApplyError> {
        let scope = scope.clone();
        let result = self
            .read(move |conn| {
                let loaded = load(conn, &scope.workspace_id)?;
                let Some(run) = loaded.runs.iter().find(|r| r.id == scope.run_id) else {
                    return Ok(Err(RunError::RunNotFound(scope.run_id)));
                };
                let lease = loaded.leases.iter().find(|l| l.run_id == run.id);
                let is_current = run::is_current_writer(run, lease, scope.generation);
                let Some(task) = loaded.tasks.iter().find(|t| t.id == run.task_id) else {
                    return Ok(Err(RunError::TaskNotFound(run.task_id.clone())));
                };
                let contract = load_contract_history(conn, &task.id)?.into_iter().find(|c| c.version == run.contract_version);
                Ok(match contract {
                    Some(contract) => Ok(AgentContext { run: run.clone(), task: task.clone(), contract, is_current }),
                    None => Err(RunError::TaskNotFound(task.id.clone())),
                })
            })
            .await?;
        Ok(result?)
    }

    /// 套用 Worker 的回報（世代取自憑證）。被接受時續約 lease、追加事件並留下稽核；
    /// 過期世代被拒絕（`RunError::StaleReport`），狀態不變，但回報內容仍留在稽核紀錄。
    pub async fn apply_agent_report(
        &self,
        scope: &AgentScope,
        content: ReportContent,
        now: Timestamp,
    ) -> Result<RunChange, RunApplyError> {
        let scope = scope.clone();
        let outcome = self
            .write(move |tx| {
                let loaded = load(tx, &scope.workspace_id)?;
                let command = RunCommand::Report {
                    run_id: scope.run_id.clone(),
                    generation: scope.generation,
                    content: content.clone(),
                };
                match decide_in(&loaded, &scope.workspace_id, &loaded.tasks, command, now) {
                    Err(RunError::StaleReport) => {
                        let audit = ReportRecord {
                            workspace_id: scope.workspace_id,
                            run_id: scope.run_id,
                            generation: scope.generation,
                            accepted: false,
                            content,
                            received_at: now,
                        };
                        save_report(tx, &audit)?;
                        Ok(Err(RunError::StaleReport))
                    }
                    Err(rejected) => Ok(Err(rejected)),
                    Ok(change) => {
                        save(tx, &change, now)?;
                        Ok(Ok(change))
                    }
                }
            })
            .await?;
        Ok(outcome?)
    }
}

pub(crate) fn save_report(tx: &Transaction<'_>, report: &ReportRecord) -> rusqlite::Result<()> {
    // Domain 型別只含字串與數字，序列化不會失敗
    let content = serde_json::to_string(&report.content).expect("回報內容可序列化");
    tx.execute(
        "INSERT INTO agent_report (workspace_id, run_id, generation, kind, accepted, content, received_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![
            report.workspace_id,
            report.run_id,
            report.generation,
            report.content.kind().as_str(),
            report.accepted,
            content,
            report.received_at.as_millis(),
        ],
    )?;
    Ok(())
}

/// Workspace 內每個 Run 每種回報最新一筆被接受的回報，依收到的順序。
pub(crate) fn load_latest_reports(conn: &Connection, workspace_id: &str) -> rusqlite::Result<Vec<ReportRecord>> {
    conn.prepare(
        "SELECT workspace_id, run_id, generation, content, received_at FROM agent_report
         WHERE seq IN (SELECT MAX(seq) FROM agent_report WHERE workspace_id = ?1 AND accepted = 1
                       GROUP BY run_id, kind)
         ORDER BY seq",
    )?
    .query_map([workspace_id], |r| {
        Ok(ReportRecord {
            workspace_id: r.get(0)?,
            run_id: r.get(1)?,
            generation: r.get(2)?,
            accepted: true,
            content: json_column(3, &r.get::<_, String>(3)?)?,
            received_at: Timestamp::from_millis(r.get(4)?),
        })
    })?
    .collect()
}
