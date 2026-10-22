//! 裁定請求與裁定的持久化（票 22）。
//!
//! 規則都在 Domain（`baton_domain::decision`）；這裡只做「在同一個交易中載入目前狀態 → 交給 Domain 判定 →
//! 寫入請求、裁定、受影響的 Task、喚醒 Outbox 與事件」，以及給裁定中心與狀態列的唯讀查詢。
//! 回答一個請求時，裁定、請求狀態、Task 恢復與喚醒 Outbox 在**同一個交易**提交：不會出現「裁定已存、Task 還卡住」。

use baton_domain::approval::ArtifactMark;
use baton_domain::auth::AgentScope;
use baton_domain::decision::{
    self, ApprovalUse, Decision, DecisionChange, DecisionCommand, DecisionContext, DecisionError, DecisionKind,
    DecisionOutcome, DecisionRequest, DecisionStatus, DecisionView, Urgency,
};
use baton_domain::task::Task;
use serde::{Deserialize, Serialize};

use super::tasks::{load, save_task, Loaded};
use super::*;

/// 套用裁定命令或查詢的失敗：Domain 拒絕（使用者可修正）或儲存層錯誤。
#[derive(Debug, thiserror::Error)]
pub enum DecisionApplyError {
    #[error(transparent)]
    Rejected(#[from] DecisionError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl From<rusqlite::Error> for DecisionApplyError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error.into())
    }
}

/// 一個 Workspace 的裁定清單：待處理的在前（緊急程度高者優先、同級先建立者優先），已回答的在後（最近裁定者優先）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionList {
    pub pending_count: usize,
    pub decisions: Vec<DecisionView>,
}

/// 回答之後的結果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnswerResult {
    pub decision: DecisionView,
    /// 因這個裁定恢復排程（回到 `ready`）的 Task。
    pub resumed_task_ids: Vec<String>,
}

/// 每個使用中 Workspace 的待處理裁定數（狀態列的「待裁定數」）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingByWorkspace {
    pub workspace_id: String,
    pub pending_count: usize,
}

struct DecisionData {
    requests: Vec<DecisionRequest>,
    decisions: Vec<Decision>,
    uses: Vec<ApprovalUse>,
    artifacts: Vec<ArtifactMark>,
}

impl DecisionData {
    fn ctx<'a>(&'a self, loaded: &'a Loaded, task: &'a baton_domain::task::TaskContext<'a>) -> DecisionContext<'a> {
        DecisionContext {
            task,
            runs: &loaded.runs,
            leases: &loaded.leases,
            requests: &self.requests,
            decisions: &self.decisions,
            uses: &self.uses,
            artifacts: &self.artifacts,
        }
    }
}

impl Store {
    /// Agent 提出裁定請求（`request_decision`）；範圍取自憑證。
    pub async fn request_decision(
        &self,
        scope: &AgentScope,
        request_id: String,
        content: decision::RequestContent,
        now: Timestamp,
    ) -> Result<DecisionOutcome, DecisionApplyError> {
        let command = DecisionCommand::Request {
            request_id,
            run_id: scope.run_id.clone(),
            generation: scope.generation,
            content,
        };
        self.apply_decision_command(scope.workspace_id.clone(), command, now).await
    }

    /// Worker 的權限提示（`permission_prompt`）；範圍取自憑證。
    pub async fn permission_prompt(
        &self,
        scope: &AgentScope,
        request_id: String,
        audit_id: String,
        tool_name: String,
        input: serde_json::Value,
        now: Timestamp,
    ) -> Result<DecisionOutcome, DecisionApplyError> {
        let command = DecisionCommand::PermissionPrompt {
            request_id,
            audit_id,
            run_id: scope.run_id.clone(),
            generation: scope.generation,
            tool_name,
            input,
        };
        self.apply_decision_command(scope.workspace_id.clone(), command, now).await
    }

    /// 使用者（桌面憑證）回答一個請求；回傳更新後的請求與恢復排程的 Task。
    pub async fn answer_decision(
        &self,
        workspace_id: &str,
        request_id: &str,
        option_id: String,
        note: Option<String>,
        now: Timestamp,
    ) -> Result<AnswerResult, DecisionApplyError> {
        let command = DecisionCommand::Answer {
            request_id: request_id.to_string(),
            option_id,
            note,
            decided_by: decision::DECIDED_BY_DESKTOP.to_string(),
        };
        let outcome = self.apply_decision_command(workspace_id.to_string(), command, now).await?;
        let DecisionOutcome::Answered { resumed_task_ids } = outcome else {
            unreachable!("Answer 命令只會得到 Answered 結果")
        };
        let decision = self.decision_detail(workspace_id, request_id, now).await?;
        Ok(AnswerResult { decision, resumed_task_ids })
    }

    async fn apply_decision_command(
        &self,
        workspace_id: String,
        command: DecisionCommand,
        now: Timestamp,
    ) -> Result<DecisionOutcome, DecisionApplyError> {
        let outcome = self
            .write(move |tx| {
                let loaded = load(tx, &workspace_id)?;
                let data = load_decision_data(tx, &workspace_id)?;
                let task_ctx = loaded.ctx(&workspace_id);
                match decision::decide(&data.ctx(&loaded, &task_ctx), command, now) {
                    Err(rejected) => Ok(Err(rejected)),
                    Ok(change) => {
                        if change.changed() {
                            save(tx, &change)?;
                        }
                        Ok(Ok(change.outcome))
                    }
                }
            })
            .await?;
        Ok(outcome?)
    }

    /// Workspace 的全部裁定請求（含已回答）。
    pub async fn decision_list(&self, workspace_id: &str, now: Timestamp) -> Result<DecisionList, DecisionApplyError> {
        let workspace_id = workspace_id.to_string();
        let result = self
            .read(move |conn| {
                let loaded = load(conn, &workspace_id)?;
                if loaded.workspace.is_none() {
                    return Ok(Err(DecisionError::WorkspaceNotFound(workspace_id)));
                }
                let data = load_decision_data(conn, &workspace_id)?;
                let mut views: Vec<DecisionView> = data
                    .requests
                    .iter()
                    .map(|r| {
                        let decision = data.decisions.iter().find(|d| d.request_id == r.id);
                        let used = data.uses.iter().find(|u| u.request_id == r.id);
                        decision::view(r, decision, used, &loaded.tasks, now)
                    })
                    .collect();
                views.sort_by_key(|v| match &v.decision {
                    // 待處理：緊急程度高者在前，同級先建立者在前
                    None => (0, std::cmp::Reverse(urgency_rank(v.urgency)), v.created_at.as_millis()),
                    // 已回答：最近裁定者在前
                    Some(d) => (1, std::cmp::Reverse(0), -d.decided_at.as_millis()),
                });
                let pending_count = views.iter().filter(|v| v.status == DecisionStatus::Pending).count();
                Ok(Ok(DecisionList { pending_count, decisions: views }))
            })
            .await?;
        Ok(result?)
    }

    pub async fn decision_detail(
        &self,
        workspace_id: &str,
        request_id: &str,
        now: Timestamp,
    ) -> Result<DecisionView, DecisionApplyError> {
        let list = self.decision_list(workspace_id, now).await?;
        list.decisions
            .into_iter()
            .find(|d| d.id == request_id)
            .ok_or_else(|| DecisionError::NotFound(request_id.to_string()).into())
    }

    /// 各使用中 Workspace 的待處理裁定數（沒有待處理的 Workspace 不列出）。
    pub async fn pending_decisions_by_workspace(&self) -> StoreResult<Vec<PendingByWorkspace>> {
        self.read(|conn| {
            let rows = conn
                .prepare(
                    "SELECT r.workspace_id, COUNT(*) FROM decision_request r
                     JOIN workspace w ON w.id = r.workspace_id
                     WHERE r.status = 'pending' AND w.status = 'active'
                     GROUP BY r.workspace_id ORDER BY MIN(r.rowid)",
                )?
                .query_map([], |r| Ok(PendingByWorkspace { workspace_id: r.get(0)?, pending_count: r.get::<_, i64>(1)? as usize }))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .await
    }
}

fn urgency_rank(urgency: Urgency) -> i32 {
    match urgency {
        Urgency::Low => 0,
        Urgency::Normal => 1,
        Urgency::High => 2,
    }
}

const REQUEST_COLUMNS: &str = "id, workspace_id, run_id, task_id, kind, status, question, reason, options, \
     recommendation, evidence, urgency, deadline_at, permission, created_at";

fn load_decision_data(conn: &Connection, workspace_id: &str) -> rusqlite::Result<DecisionData> {
    let bad = |index: usize, what: &str| {
        rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Text, format!("未知的{what}").into())
    };
    let mut requests = conn
        .prepare(&format!("SELECT {REQUEST_COLUMNS} FROM decision_request WHERE workspace_id = ?1 ORDER BY rowid"))?
        .query_map([workspace_id], |r| {
            let kind: String = r.get(4)?;
            let status: String = r.get(5)?;
            let urgency: String = r.get(11)?;
            let permission: Option<String> = r.get(13)?;
            Ok(DecisionRequest {
                id: r.get(0)?,
                workspace_id: r.get(1)?,
                run_id: r.get(2)?,
                task_id: r.get(3)?,
                kind: DecisionKind::parse(&kind).ok_or_else(|| bad(4, "裁定種類"))?,
                status: DecisionStatus::parse(&status).ok_or_else(|| bad(5, "裁定狀態"))?,
                question: r.get(6)?,
                reason: r.get(7)?,
                options: json_column(8, &r.get::<_, String>(8)?)?,
                recommendation: r.get(9)?,
                evidence: json_column(10, &r.get::<_, String>(10)?)?,
                urgency: Urgency::parse(&urgency).ok_or_else(|| bad(11, "緊急程度"))?,
                deadline_at: r.get::<_, Option<i64>>(12)?.map(Timestamp::from_millis),
                permission: permission.map(|raw| json_column(13, &raw)).transpose()?,
                blocking: Vec::new(),
                created_at: Timestamp::from_millis(r.get(14)?),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let blocks = conn
        .prepare(
            "SELECT b.request_id, b.task_id FROM decision_request_block b
             JOIN decision_request r ON r.id = b.request_id WHERE r.workspace_id = ?1 ORDER BY b.rowid",
        )?
        .query_map([workspace_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (request_id, task_id) in blocks {
        if let Some(request) = requests.iter_mut().find(|r| r.id == request_id) {
            request.blocking.push(task_id);
        }
    }

    let decisions = conn
        .prepare(
            "SELECT d.request_id, d.option_id, d.note, d.decided_by, d.decided_at FROM decision d
             JOIN decision_request r ON r.id = d.request_id WHERE r.workspace_id = ?1 ORDER BY d.rowid",
        )?
        .query_map([workspace_id], |r| {
            Ok(Decision {
                request_id: r.get(0)?,
                option_id: r.get(1)?,
                note: r.get(2)?,
                decided_by: r.get(3)?,
                decided_at: Timestamp::from_millis(r.get(4)?),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let uses = conn
        .prepare(
            "SELECT u.request_id, u.run_id, u.used_at FROM decision_approval_use u
             JOIN decision_request r ON r.id = u.request_id WHERE r.workspace_id = ?1 ORDER BY u.rowid",
        )?
        .query_map([workspace_id], |r| {
            Ok(ApprovalUse { request_id: r.get(0)?, run_id: r.get(1)?, used_at: Timestamp::from_millis(r.get(2)?) })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    // 各成果的最新版本：單次批准綁定 Task 的成果版本（票 23）
    let artifacts = conn
        .prepare(
            "SELECT a.task_id, a.name, MAX(v.version) FROM artifact a
             JOIN artifact_version v ON v.artifact_id = a.id
             WHERE a.workspace_id = ?1 GROUP BY a.id ORDER BY a.rowid",
        )?
        .query_map([workspace_id], |r| Ok(ArtifactMark { task_id: r.get(0)?, name: r.get(1)?, version: r.get(2)? }))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(DecisionData { requests, decisions, uses, artifacts })
}

/// 把 Domain 的結果寫進同一個交易：請求、裁定、單次批准的消耗、受影響的 Task、喚醒 Outbox 與事件。
fn save(tx: &Transaction<'_>, change: &DecisionChange) -> rusqlite::Result<()> {
    if let Some(request) = &change.request {
        // Domain 型別只含字串與數字，序列化不會失敗
        let options = serde_json::to_string(&request.options).expect("選項可序列化");
        let evidence = serde_json::to_string(&request.evidence).expect("證據可序列化");
        let permission = request.permission.as_ref().map(|p| serde_json::to_string(p).expect("權限細節可序列化"));
        tx.execute(
            &format!(
                "INSERT INTO decision_request ({REQUEST_COLUMNS})
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)"
            ),
            rusqlite::params![
                request.id,
                request.workspace_id,
                request.run_id,
                request.task_id,
                request.kind.as_str(),
                request.status.as_str(),
                request.question,
                request.reason,
                options,
                request.recommendation,
                evidence,
                request.urgency.as_str(),
                request.deadline_at.map(|t| t.as_millis()),
                permission,
                request.created_at.as_millis(),
            ],
        )?;
        for task_id in &request.blocking {
            tx.execute(
                "INSERT INTO decision_request_block (request_id, task_id) VALUES (?1, ?2)",
                rusqlite::params![request.id, task_id],
            )?;
        }
    }
    if let Some(request_id) = &change.answered_request {
        tx.execute("UPDATE decision_request SET status = 'answered' WHERE id = ?1", [request_id])?;
    }
    if let Some(decision) = &change.decision {
        tx.execute(
            "INSERT INTO decision (request_id, option_id, note, decided_by, decided_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                decision.request_id,
                decision.option_id,
                decision.note,
                decision.decided_by,
                decision.decided_at.as_millis()
            ],
        )?;
    }
    if let Some(used) = &change.approval_use {
        tx.execute(
            "INSERT INTO decision_approval_use (request_id, run_id, used_at) VALUES (?1, ?2, ?3)",
            rusqlite::params![used.request_id, used.run_id, used.used_at.as_millis()],
        )?;
    }
    for task in &change.tasks {
        let task: &Task = task;
        save_task(tx, task)?;
    }
    // 恢復排程：回到 ready 的 Task 重新排進派工佇列，由 Scheduler 派出（舊 Run 還持有 lease 時留在佇列等待）。
    // 原本派工的優先序沒有保存，一律用派工 API 的預設值 0；已在佇列中的保留原本的位置與優先序
    if let (DecisionOutcome::Answered { resumed_task_ids }, Some(decision)) = (&change.outcome, &change.decision) {
        for task in change.tasks.iter().filter(|t| resumed_task_ids.contains(&t.id)) {
            tx.execute(
                "INSERT INTO dispatch_request (task_id, workspace_id, priority, requested_at)
                 VALUES (?1, ?2, 0, ?3)
                 ON CONFLICT(task_id) DO NOTHING",
                rusqlite::params![task.id, task.workspace_id, decision.decided_at.as_millis()],
            )?;
        }
    }
    if let Some(item) = &change.outbox {
        tx.execute(
            "INSERT INTO outbox (kind, run_id, payload, created_at, delivered_at) VALUES (?1, ?2, ?3, ?4, NULL)",
            rusqlite::params![item.kind, item.run_id, item.payload.to_string(), item.created_at.as_millis()],
        )?;
    }
    for event in &change.events {
        append_event(tx, event)?;
    }
    Ok(())
}
