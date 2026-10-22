//! 裁定請求與裁定的持久化（票 22、票 35）。
//!
//! 規則都在 Domain（`baton_domain::decision`、`baton_domain::authority`）；這裡只做「在同一個交易中載入目前狀態 →
//! 交給 Domain 判定 → 寫入請求、裁定、受影響的 Task、喚醒 Outbox 與事件」，以及給裁定中心與狀態列的查詢。
//! 處理一個請求時，裁定、請求狀態、Task 恢復與喚醒 Outbox 在**同一個交易**提交：不會出現「裁定已存、Task 還卡住」。
//! 批次處理由呼叫端逐項呼叫 `answer_decision`：每一項各自一個交易，一項失敗不影響其他項。

use baton_domain::approval::ArtifactMark;
use baton_domain::auth::AgentScope;
use baton_domain::authority::{self, AuthorityEntryRequest, AuthorityError, AuthorityPolicy, AuthorityTable, Matter};
use baton_domain::decision::{
    self, ActionContent, ApprovalUse, AuthorityDetail, Decision, DecisionAction, DecisionChange, DecisionCommand,
    DecisionContext, DecisionError, DecisionKind, DecisionOutcome, DecisionRequest, DecisionStatus, DecisionTarget,
    DecisionView, Resolution, Urgency,
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
    Authority(#[from] AuthorityError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl From<rusqlite::Error> for DecisionApplyError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error.into())
    }
}

/// 裁定中心的總覽數字（使用者故事 49：先看總覽再逐項處理）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionOverview {
    /// 待處理（含延後中的）。
    pub pending: usize,
    /// 待處理而且沒有在延後期間。
    pub actionable: usize,
    pub deferred: usize,
    pub overdue: usize,
    /// 待處理中緊急程度為高者。
    pub high_urgency: usize,
    pub answered: usize,
    pub superseded: usize,
    pub merged: usize,
}

/// 一個 Workspace 的裁定清單：可處理的在前（緊急程度高者優先、同級先建立者優先），延後中的其次（先到期者優先），
/// 已結束的在後（最近結束者優先）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionList {
    pub pending_count: usize,
    pub overview: DecisionOverview,
    pub decisions: Vec<DecisionView>,
}

/// 處理之後的結果。
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
    policy: AuthorityPolicy,
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
            authority: &self.policy,
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

    /// Controller 嘗試一個動作（`controller_action`），依 Workspace 的決策權限表判定；範圍取自憑證。
    pub async fn controller_action(
        &self,
        scope: &AgentScope,
        request_id: String,
        audit_id: String,
        content: ActionContent,
        now: Timestamp,
    ) -> Result<DecisionOutcome, DecisionApplyError> {
        let command = DecisionCommand::ControllerAction {
            request_id,
            audit_id,
            run_id: scope.run_id.clone(),
            generation: scope.generation,
            content,
        };
        self.apply_decision_command(scope.workspace_id.clone(), command, now).await
    }

    /// 使用者（桌面憑證）處理一個請求；回傳更新後的請求與恢復排程的 Task。
    /// 目標版本已變更時，請求被標為過期（這個標記會寫入），並回傳 `decision_superseded`。
    pub async fn answer_decision(
        &self,
        workspace_id: &str,
        request_id: &str,
        expected_revision: i64,
        resolution: Resolution,
        batch_id: Option<String>,
        now: Timestamp,
    ) -> Result<AnswerResult, DecisionApplyError> {
        let command = DecisionCommand::Answer {
            request_id: request_id.to_string(),
            expected_revision,
            resolution,
            decided_by: decision::DECIDED_BY_DESKTOP.to_string(),
            batch_id,
        };
        let resumed_task_ids = match self.apply_decision_command(workspace_id.to_string(), command, now).await? {
            DecisionOutcome::Answered { resumed_task_ids } => resumed_task_ids,
            DecisionOutcome::Deferred => Vec::new(),
            DecisionOutcome::Superseded { reason, .. } => return Err(DecisionError::Superseded(reason).into()),
            other => unreachable!("Answer 命令不會得到 {other:?}"),
        };
        let decision = self.decision_detail(workspace_id, request_id, now).await?;
        Ok(AnswerResult { decision, resumed_task_ids })
    }

    /// 把 `request_id` 合併到 `into`；回傳合併後的目標請求。
    pub async fn merge_decision(
        &self,
        workspace_id: &str,
        request_id: &str,
        expected_revision: i64,
        into: &str,
        into_expected_revision: i64,
        now: Timestamp,
    ) -> Result<DecisionView, DecisionApplyError> {
        let command = DecisionCommand::Merge {
            request_id: request_id.to_string(),
            expected_revision,
            into: into.to_string(),
            into_expected_revision,
        };
        self.apply_decision_command(workspace_id.to_string(), command, now).await?;
        self.decision_detail(workspace_id, into, now).await
    }

    /// 把兩個請求互相連結；回傳 `request_id` 的最新內容。
    pub async fn link_decision(
        &self,
        workspace_id: &str,
        request_id: &str,
        other_id: &str,
        now: Timestamp,
    ) -> Result<DecisionView, DecisionApplyError> {
        let command = DecisionCommand::Link { request_id: request_id.to_string(), other_id: other_id.to_string() };
        self.apply_decision_command(workspace_id.to_string(), command, now).await?;
        self.decision_detail(workspace_id, request_id, now).await
    }

    async fn apply_decision_command(
        &self,
        workspace_id: String,
        command: DecisionCommand,
        now: Timestamp,
    ) -> Result<DecisionOutcome, DecisionApplyError> {
        let outcome = self.write(move |tx| apply_in(tx, &workspace_id, command, now)).await?;
        Ok(outcome?)
    }

    /// Workspace 的全部裁定請求（含已結束的）。開啟時先把目標版本已變更的待處理請求標為過期。
    pub async fn decision_list(&self, workspace_id: &str, now: Timestamp) -> Result<DecisionList, DecisionApplyError> {
        let workspace_id = workspace_id.to_string();
        let result = self
            .write(move |tx| {
                let loaded = load(tx, &workspace_id)?;
                if loaded.workspace.is_none() {
                    return Ok(Err(DecisionError::WorkspaceNotFound(workspace_id)));
                }
                if let Err(rejected) = apply_in(tx, &workspace_id, DecisionCommand::Refresh, now)? {
                    return Ok(Err(rejected));
                }
                let loaded = load(tx, &workspace_id)?;
                let data = load_decision_data(tx, &workspace_id)?;
                Ok(Ok(list(&loaded, &data, now)))
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

    /// 各使用中 Workspace 的待處理裁定數（沒有待處理的 Workspace 不列出）。先把目標版本已變更的請求標為過期，
    /// 狀態列才不會把已經不能套用的請求算進去。
    pub async fn pending_decisions_by_workspace(&self, now: Timestamp) -> StoreResult<Vec<PendingByWorkspace>> {
        self.write(move |tx| {
            let workspaces = tx
                .prepare(
                    "SELECT DISTINCT r.workspace_id FROM decision_request r JOIN workspace w ON w.id = r.workspace_id
                     WHERE r.status = 'pending' AND w.status = 'active'",
                )?
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for workspace_id in workspaces {
                // 重新整理只會被 Domain 拒絕於 Workspace 不存在，這裡的 Workspace 一定存在
                let _ = apply_in(tx, &workspace_id, DecisionCommand::Refresh, now)?;
            }
            let rows = tx
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

    /// Workspace 的決策權限表（spec D-15）。
    pub async fn decision_authority(&self, workspace_id: &str) -> Result<AuthorityTable, DecisionApplyError> {
        let workspace_id = workspace_id.to_string();
        let result = self
            .read(move |conn| {
                if !workspace_exists(conn, &workspace_id)? {
                    return Ok(Err(DecisionError::WorkspaceNotFound(workspace_id)));
                }
                Ok(Ok(authority::table(&load_policy(conn, &workspace_id)?)))
            })
            .await?;
        Ok(result?)
    }

    /// 以使用者送來的設定取代 Workspace 的決策權限表；只能收窄，放寬整個拒絕。
    pub async fn update_decision_authority(
        &self,
        workspace_id: &str,
        expected_revision: i64,
        entries: Vec<AuthorityEntryRequest>,
        now: Timestamp,
    ) -> Result<AuthorityTable, DecisionApplyError> {
        let workspace_id = workspace_id.to_string();
        let result = self
            .write(move |tx| {
                if !workspace_exists(tx, &workspace_id)? {
                    return Ok(Err(DecisionError::WorkspaceNotFound(workspace_id).into()));
                }
                let current = load_policy(tx, &workspace_id)?;
                let policy = match authority::replace_policy(&current, expected_revision, &entries) {
                    Ok(policy) => policy,
                    Err(rejected) => return Ok(Err(DecisionApplyError::Authority(rejected))),
                };
                let narrowed = serde_json::to_string(&policy.narrowed).expect("事項清單可序列化");
                tx.execute(
                    "INSERT INTO decision_authority_policy (workspace_id, revision, narrowed, updated_at)
                     VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(workspace_id) DO UPDATE SET revision = ?2, narrowed = ?3, updated_at = ?4",
                    rusqlite::params![workspace_id, policy.revision, narrowed, now.as_millis()],
                )?;
                append_event(
                    tx,
                    &baton_domain::event::DomainEvent {
                        aggregate_type: "decision_authority_policy",
                        aggregate_id: workspace_id.clone(),
                        aggregate_version: policy.revision,
                        event_type: "decision_authority_changed",
                        occurred_at: now,
                        payload: serde_json::json!({ "narrowed": policy.narrowed }),
                    },
                )?;
                Ok(Ok(authority::table(&policy)))
            })
            .await?;
        result
    }
}

/// 在交易中載入目前狀態、交給 Domain 判定、寫入結果。
fn apply_in(
    tx: &Transaction<'_>,
    workspace_id: &str,
    command: DecisionCommand,
    now: Timestamp,
) -> StoreResult<Result<DecisionOutcome, DecisionError>> {
    let loaded = load(tx, workspace_id)?;
    let data = load_decision_data(tx, workspace_id)?;
    let task_ctx = loaded.ctx(workspace_id);
    Ok(match decision::decide(&data.ctx(&loaded, &task_ctx), command, now) {
        Err(rejected) => Err(rejected),
        Ok(change) => {
            if change.changed() {
                save(tx, &change, now)?;
            }
            Ok(change.outcome)
        }
    })
}

fn list(loaded: &Loaded, data: &DecisionData, now: Timestamp) -> DecisionList {
    let mut requests: Vec<&DecisionRequest> = data.requests.iter().collect();
    requests.sort_by_key(|r| match r.status {
        // 可處理：緊急程度高者在前，同級先建立者在前
        DecisionStatus::Pending if !r.is_deferred(now) => {
            (0, std::cmp::Reverse(urgency_rank(r.urgency)), r.created_at.as_millis())
        }
        // 延後中：先到期者在前
        DecisionStatus::Pending => (1, std::cmp::Reverse(0), r.deferred_until.map_or(0, |t| t.as_millis())),
        // 已結束：最近結束者在前
        _ => (2, std::cmp::Reverse(0), -r.updated_at.as_millis()),
    });
    let views: Vec<DecisionView> =
        requests.into_iter().map(|r| decision::view(r, &data.requests, &data.decisions, &data.uses, &loaded.tasks, now)).collect();
    let mut overview = DecisionOverview::default();
    for view in &views {
        match view.status {
            DecisionStatus::Pending => {
                overview.pending += 1;
                if view.deferred {
                    overview.deferred += 1;
                } else {
                    overview.actionable += 1;
                }
                overview.overdue += usize::from(view.overdue);
                overview.high_urgency += usize::from(view.urgency == Urgency::High);
            }
            DecisionStatus::Answered => overview.answered += 1,
            DecisionStatus::Superseded => overview.superseded += 1,
            DecisionStatus::Merged => overview.merged += 1,
        }
    }
    DecisionList { pending_count: overview.pending, overview, decisions: views }
}

fn urgency_rank(urgency: Urgency) -> i32 {
    match urgency {
        Urgency::Low => 0,
        Urgency::Normal => 1,
        Urgency::High => 2,
    }
}

fn workspace_exists(conn: &Connection, workspace_id: &str) -> rusqlite::Result<bool> {
    conn.query_row("SELECT EXISTS (SELECT 1 FROM workspace WHERE id = ?1)", [workspace_id], |r| r.get(0))
}

fn load_policy(conn: &Connection, workspace_id: &str) -> rusqlite::Result<AuthorityPolicy> {
    let row = conn
        .prepare("SELECT revision, narrowed FROM decision_authority_policy WHERE workspace_id = ?1")?
        .query_map([workspace_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
        .next()
        .transpose()?;
    Ok(match row {
        None => AuthorityPolicy::default(),
        Some((revision, narrowed)) => AuthorityPolicy { revision, narrowed: json_column::<Vec<Matter>>(1, &narrowed)? },
    })
}

const REQUEST_COLUMNS: &str = "id, workspace_id, run_id, task_id, kind, status, question, reason, options, \
     recommendation, evidence, urgency, deadline_at, permission, authority, target_task_id, target_contract_version, \
     target_authorization_revision, revision, deferred_until, merged_into, status_reason, created_at, updated_at";

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
            let authority: Option<String> = r.get(14)?;
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
                authority: authority.map(|raw| json_column::<AuthorityDetail>(14, &raw)).transpose()?,
                blocking: Vec::new(),
                target: DecisionTarget {
                    task_id: r.get(15)?,
                    contract_version: r.get(16)?,
                    authorization_revision: r.get(17)?,
                },
                revision: r.get(18)?,
                event_version: 0,
                deferred_until: r.get::<_, Option<i64>>(19)?.map(Timestamp::from_millis),
                merged_into: r.get(20)?,
                status_reason: r.get(21)?,
                links: Vec::new(),
                created_at: Timestamp::from_millis(r.get(22)?),
                updated_at: Timestamp::from_millis(r.get(23)?),
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

    let links = conn
        .prepare(
            "SELECT l.request_id, l.linked_id FROM decision_request_link l
             JOIN decision_request r ON r.id = l.request_id WHERE r.workspace_id = ?1 ORDER BY l.rowid",
        )?
        .query_map([workspace_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (a, b) in links {
        for (from, to) in [(&a, &b), (&b, &a)] {
            if let Some(request) = requests.iter_mut().find(|r| &r.id == from) {
                request.links.push(to.clone());
            }
        }
    }

    let versions = conn
        .prepare(
            "SELECT e.aggregate_id, MAX(e.aggregate_version) FROM event e
             JOIN decision_request r ON r.id = e.aggregate_id
             WHERE e.aggregate_type = ?1 AND r.workspace_id = ?2 GROUP BY e.aggregate_id",
        )?
        .query_map(rusqlite::params![decision::DECISION_AGGREGATE, workspace_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (request_id, version) in versions {
        if let Some(request) = requests.iter_mut().find(|r| r.id == request_id) {
            request.event_version = version;
        }
    }

    let decisions = conn
        .prepare(
            "SELECT d.request_id, d.action, d.option_id, d.note, d.decided_by, d.decided_at, d.batch_id FROM decision d
             JOIN decision_request r ON r.id = d.request_id WHERE r.workspace_id = ?1 ORDER BY d.rowid",
        )?
        .query_map([workspace_id], |r| {
            let action: String = r.get(1)?;
            Ok(Decision {
                request_id: r.get(0)?,
                action: DecisionAction::parse(&action).ok_or_else(|| bad(1, "裁定處理方式"))?,
                option_id: r.get(2)?,
                note: r.get(3)?,
                decided_by: r.get(4)?,
                decided_at: Timestamp::from_millis(r.get(5)?),
                batch_id: r.get(6)?,
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
    Ok(DecisionData { requests, decisions, uses, policy: load_policy(conn, workspace_id)?, artifacts })
}

/// 把 Domain 的結果寫進同一個交易：請求、阻擋範圍、連結、裁定、單次批准的消耗、受影響的 Task、派工佇列、喚醒 Outbox 與事件。
fn save(tx: &Transaction<'_>, change: &DecisionChange, now: Timestamp) -> rusqlite::Result<()> {
    if let Some(request) = &change.created {
        // Domain 型別只含字串與數字，序列化不會失敗
        let options = serde_json::to_string(&request.options).expect("選項可序列化");
        let evidence = serde_json::to_string(&request.evidence).expect("證據可序列化");
        let permission = request.permission.as_ref().map(|p| serde_json::to_string(p).expect("權限細節可序列化"));
        let authority = request.authority.as_ref().map(|a| serde_json::to_string(a).expect("動作細節可序列化"));
        tx.execute(
            &format!(
                "INSERT INTO decision_request ({REQUEST_COLUMNS})
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24)"
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
                authority,
                request.target.task_id,
                request.target.contract_version,
                request.target.authorization_revision,
                request.revision,
                request.deferred_until.map(|t| t.as_millis()),
                request.merged_into,
                request.status_reason,
                request.created_at.as_millis(),
                request.updated_at.as_millis(),
            ],
        )?;
        for task_id in &request.blocking {
            tx.execute(
                "INSERT INTO decision_request_block (request_id, task_id) VALUES (?1, ?2)",
                rusqlite::params![request.id, task_id],
            )?;
        }
    }
    for request in &change.updated {
        tx.execute(
            "UPDATE decision_request SET status = ?2, revision = ?3, deferred_until = ?4, merged_into = ?5,
             status_reason = ?6, updated_at = ?7 WHERE id = ?1",
            rusqlite::params![
                request.id,
                request.status.as_str(),
                request.revision,
                request.deferred_until.map(|t| t.as_millis()),
                request.merged_into,
                request.status_reason,
                request.updated_at.as_millis(),
            ],
        )?;
    }
    for (request_id, task_id) in &change.new_blocks {
        tx.execute(
            "INSERT OR IGNORE INTO decision_request_block (request_id, task_id) VALUES (?1, ?2)",
            rusqlite::params![request_id, task_id],
        )?;
    }
    for (a, b) in &change.new_links {
        tx.execute(
            "INSERT OR IGNORE INTO decision_request_link (request_id, linked_id, created_at) VALUES (?1, ?2, ?3)",
            rusqlite::params![a, b, now.as_millis()],
        )?;
    }
    if let Some(decision) = &change.decision {
        tx.execute(
            "INSERT INTO decision (request_id, action, option_id, note, decided_by, batch_id, decided_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                decision.request_id,
                decision.action.as_str(),
                decision.option_id,
                decision.note,
                decision.decided_by,
                decision.batch_id,
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
    for task in change.tasks.iter().filter(|t| change.resumed.contains(&t.id)) {
        tx.execute(
            "INSERT INTO dispatch_request (task_id, workspace_id, priority, requested_at)
             VALUES (?1, ?2, 0, ?3)
             ON CONFLICT(task_id) DO NOTHING",
            rusqlite::params![task.id, task.workspace_id, now.as_millis()],
        )?;
    }
    for item in &change.outbox {
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
