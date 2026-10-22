//! 方法登錄、Playbook 版本、Workflow 與 stage attempt 的持久化（票 43）。
//!
//! - 方法與 Playbook 版本只追加（資料表以 trigger 禁止更新與刪除）；識別是 `<前綴>_<key>_v<版本>`。
//! - Workflow 的每次變更在單一交易中完成：載入 Workflow、固定的 Playbook 版本與先前的 attempt → 查驗證據 →
//!   Domain 判定 → 寫回 Workflow（以 revision 條件更新）、追加 attempt 與事件。
//! - 證據由這裡在同一個交易中查驗（成果、裁定、成果接受、雙軸審查），不採信 Controller 自報。
//! - Gate 失效與重驗（票 50）：build 由 Runtime 在交易外觀察（受驗目錄的 revision，[`Builds`]），通過時隨 attempt 保存；
//!   [`Store::revalidate_gates`] 在一個交易中重新查驗所有未完成 Workflow 已通過的 Gate，失效時追加 `invalidated` attempt、
//!   退回最早失效的 Stage，驗證證據失效時自動排入重驗的驗證要求，並以工作事件通知 Controller。

use baton_domain::authority::Matter;
use baton_domain::context::StageItem;
use baton_domain::decision::{ActionContent, ControllerOutcome, DecisionCommand, DecisionOutcome};
use baton_domain::workdir::TaskKind;
use baton_domain::workflow::{
    self, AttemptOutcome, BudgetDecision, CheckedEvidence, ClaimedOutcome, Evidence, EvidenceRef, EvidenceStatus, MethodDefinition,
    MethodVersion, Origin, PlaybookDefinition, PlaybookVersion, Recorder, Situation, StageAttempt, StageResultInput,
    StartInput, WorkflowChange, WorkflowError, WorkflowInstance, WorkflowNote, WorkflowStatus,
};
use rusqlite::OptionalExtension;

use super::*;

/// Runtime 在交易外觀察到的 build：受驗目錄 → 它目前的 revision（不是 Git repo 時為 `None`）。
/// 沒有出現在這裡的目錄視為觀察不到，不比較 build。
pub type Builds = std::collections::HashMap<String, Option<String>>;

/// 一個 Workflow 的 Gate 失效（給 Runtime 記錄日誌）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevalidatedWorkflow {
    pub workspace_id: String,
    pub workflow_id: String,
    /// 失效的 Stage。
    pub stages: Vec<String>,
    /// 自動排入的重驗驗證要求。
    pub verification_requests: Vec<String>,
}

/// 套用 Workflow 變更的失敗：Domain 拒絕、找不到綁定的 Goal／Task，或儲存層錯誤。
#[derive(Debug, thiserror::Error)]
pub enum WorkflowApplyError {
    #[error(transparent)]
    Rejected(#[from] WorkflowError),
    #[error("找不到 Task {0}")]
    TaskNotFound(String),
    #[error("找不到 Goal {0}")]
    GoalNotFound(String),
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl From<rusqlite::Error> for WorkflowApplyError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error.into())
    }
}

/// Workflow 綁定的對象：Task（Goal 由 Task 決定），或只有 Goal。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowBinding {
    Task(String),
    Goal(String),
}

/// 一個 Workflow 與檢視它需要的全部資料。
#[derive(Debug, Clone, PartialEq)]
pub struct WorkflowDetail {
    pub workflow: WorkflowInstance,
    /// 固定的 Playbook 版本。
    pub playbook: PlaybookVersion,
    /// 依 Stage、attempt 排序。
    pub attempts: Vec<StageAttempt>,
    pub task_title: Option<String>,
    /// 這個 Playbook 目前的最新版本號。
    pub latest_playbook_version: i64,
}

const PLAYBOOK_COLUMNS: &str = "id, key, version, definition, content_hash, origin, created_at";
const METHOD_COLUMNS: &str = "id, key, version, definition, content_hash, origin, created_at";
const WORKFLOW_COLUMNS: &str = "w.id, w.workspace_id, w.goal_id, w.task_id, w.playbook_version_id, p.key, p.version, w.situation, \
     w.route_reason, w.status, w.current_stage, w.blocked_reason, w.revision, w.created_by, w.created_at, w.updated_at, w.resume_stage";
const ATTEMPT_COLUMNS: &str =
    "id, workflow_id, stage_key, attempt, outcome, evidence, missing, note, recorded_by, profile_id, created_at, build";

fn unknown(index: usize, what: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Text, format!("未知的{what}").into())
}

fn origin_of(index: usize, raw: String) -> rusqlite::Result<Origin> {
    Origin::parse(&raw).ok_or_else(|| unknown(index, "版本來源"))
}

fn playbook_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<PlaybookVersion> {
    Ok(PlaybookVersion {
        id: r.get(0)?,
        key: r.get(1)?,
        version: r.get(2)?,
        definition: json_column(3, &r.get::<_, String>(3)?)?,
        content_hash: r.get(4)?,
        origin: origin_of(5, r.get(5)?)?,
        created_at: Timestamp::from_millis(r.get(6)?),
    })
}

fn method_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<MethodVersion> {
    Ok(MethodVersion {
        id: r.get(0)?,
        key: r.get(1)?,
        version: r.get(2)?,
        definition: json_column(3, &r.get::<_, String>(3)?)?,
        content_hash: r.get(4)?,
        origin: origin_of(5, r.get(5)?)?,
        created_at: Timestamp::from_millis(r.get(6)?),
    })
}

fn workflow_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<WorkflowInstance> {
    let situation: String = r.get(7)?;
    let status: String = r.get(9)?;
    Ok(WorkflowInstance {
        id: r.get(0)?,
        workspace_id: r.get(1)?,
        goal_id: r.get(2)?,
        task_id: r.get(3)?,
        playbook_version_id: r.get(4)?,
        playbook_key: r.get(5)?,
        playbook_version: r.get(6)?,
        situation: Situation::parse(&situation).ok_or_else(|| unknown(7, "情境"))?,
        route_reason: r.get(8)?,
        status: WorkflowStatus::parse(&status).ok_or_else(|| unknown(9, "Workflow 狀態"))?,
        current_stage: r.get(10)?,
        blocked_reason: r.get(11)?,
        revision: r.get(12)?,
        created_by: r.get(13)?,
        created_at: Timestamp::from_millis(r.get(14)?),
        updated_at: Timestamp::from_millis(r.get(15)?),
        resume_stage: r.get(16)?,
    })
}

fn attempt_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<StageAttempt> {
    let outcome: String = r.get(4)?;
    Ok(StageAttempt {
        id: r.get(0)?,
        workflow_id: r.get(1)?,
        stage_key: r.get(2)?,
        attempt: r.get(3)?,
        outcome: AttemptOutcome::parse(&outcome).ok_or_else(|| unknown(4, "attempt 結果"))?,
        evidence: json_column(5, &r.get::<_, String>(5)?)?,
        missing: json_column(6, &r.get::<_, String>(6)?)?,
        note: r.get(7)?,
        recorded_by: r.get(8)?,
        profile_id: r.get(9)?,
        created_at: Timestamp::from_millis(r.get(10)?),
        build: r.get(11)?,
    })
}

fn load_playbook_versions(conn: &Connection) -> rusqlite::Result<Vec<PlaybookVersion>> {
    conn.prepare(&format!("SELECT {PLAYBOOK_COLUMNS} FROM playbook_version ORDER BY key, version"))?
        .query_map([], playbook_row)?
        .collect()
}

fn load_method_versions(conn: &Connection) -> rusqlite::Result<Vec<MethodVersion>> {
    conn.prepare(&format!("SELECT {METHOD_COLUMNS} FROM method_version ORDER BY key, version"))?
        .query_map([], method_row)?
        .collect()
}

/// 每個 key 的最新版本（輸入依 key、版本排序）。
fn latest_of<T: Clone>(versions: &[T], key: impl Fn(&T) -> &str) -> Vec<T> {
    let mut latest: Vec<T> = Vec::new();
    for version in versions {
        match latest.last_mut() {
            Some(last) if key(last) == key(version) => *last = version.clone(),
            _ => latest.push(version.clone()),
        }
    }
    latest
}

fn method_keys(conn: &Connection) -> rusqlite::Result<Vec<String>> {
    conn.prepare("SELECT DISTINCT key FROM method_version ORDER BY key")?.query_map([], |r| r.get(0))?.collect()
}

fn insert_playbook(tx: &Transaction<'_>, playbook: &PlaybookVersion) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO playbook_version (id, key, version, situation, definition, content_hash, origin, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        rusqlite::params![
            playbook.id,
            playbook.key,
            playbook.version,
            playbook.definition.situation.as_str(),
            serde_json::to_string(&playbook.definition).expect("Playbook 一定可以序列化"),
            playbook.content_hash,
            playbook.origin.as_str(),
            playbook.created_at.as_millis(),
        ],
    )?;
    Ok(())
}

fn playbook_id(key: &str, version: i64) -> String {
    format!("pbv_{key}_v{version}")
}

/// 發布一個 Playbook 版本（內容與最新版本相同時不追加），回傳 (目前的最新版本, 是否新增)。
/// 使用者發布的版本追加事件；啟動時登錄的內建版本不追加（版本列本身就是不可變的紀錄）。
fn publish_in(
    tx: &Transaction<'_>,
    definition: PlaybookDefinition,
    origin: Origin,
    now: Timestamp,
) -> Result<(PlaybookVersion, bool), WorkflowApplyError> {
    let versions = load_playbook_versions(tx)?;
    let latest = versions.iter().filter(|p| p.key == definition.key).max_by_key(|p| p.version);
    let keys = method_keys(tx)?;
    let keys: Vec<&str> = keys.iter().map(String::as_str).collect();
    let id = playbook_id(&definition.key, latest.map_or(1, |l| l.version + 1));
    match workflow::publish_playbook(latest, definition, &keys, origin, id, now)? {
        None => Ok((latest.cloned().expect("沒有新增表示已有最新版本"), false)),
        Some((playbook, event)) => {
            insert_playbook(tx, &playbook)?;
            if origin == Origin::User {
                append_event(tx, &event)?;
            }
            Ok((playbook, true))
        }
    }
}

fn load_workflow(conn: &Connection, workspace_id: &str, workflow_id: &str) -> rusqlite::Result<Option<WorkflowInstance>> {
    conn.query_row(
        &format!(
            "SELECT {WORKFLOW_COLUMNS} FROM workflow_instance w JOIN playbook_version p ON p.id = w.playbook_version_id
             WHERE w.workspace_id = ?1 AND w.id = ?2"
        ),
        [workspace_id, workflow_id],
        workflow_row,
    )
    .optional()
}

fn load_playbook(conn: &Connection, id: &str) -> rusqlite::Result<PlaybookVersion> {
    conn.query_row(&format!("SELECT {PLAYBOOK_COLUMNS} FROM playbook_version WHERE id = ?1"), [id], playbook_row)
}

fn load_attempts(conn: &Connection, workflow_id: &str) -> rusqlite::Result<Vec<StageAttempt>> {
    conn.prepare(&format!("SELECT {ATTEMPT_COLUMNS} FROM stage_attempt WHERE workflow_id = ?1 ORDER BY created_at, rowid"))?
        .query_map([workflow_id], attempt_row)?
        .collect()
}

/// 綁定 Task 目前未完成的 Workflow。
fn open_for_task(conn: &Connection, task_id: &str) -> rusqlite::Result<Option<WorkflowInstance>> {
    conn.query_row(
        &format!(
            "SELECT {WORKFLOW_COLUMNS} FROM workflow_instance w JOIN playbook_version p ON p.id = w.playbook_version_id
             WHERE w.task_id = ?1 AND w.status <> 'completed'"
        ),
        [task_id],
        workflow_row,
    )
    .optional()
}

/// 受驗目錄：Workflow 綁定的 Task 已登記的工作目錄，否則是 Workspace 目錄（與驗證要求相同，票 45）。
fn subject_dir(conn: &Connection, workspace_id: &str, task_id: Option<&str>) -> rusqlite::Result<String> {
    if let Some(task_id) = task_id {
        let workdir: Option<String> = conn
            .query_row("SELECT path FROM task_workdir WHERE task_id = ?1 AND status = 'ready'", [task_id], |r| r.get(0))
            .optional()?;
        if let Some(dir) = workdir {
            return Ok(dir);
        }
    }
    conn.query_row("SELECT path FROM workspace WHERE id = ?1", [workspace_id], |r| r.get(0))
}

/// Workflow 的 Task 依據的假設已被推翻時的說明（票 50）。已完成或取消的 Task 不算：它不能再改契約，
/// 「先更新契約」永遠做不到，Workflow 會退回後卡住（R151 第 12 項）。
fn basis_overturned(conn: &Connection, workspace_id: &str, task_id: Option<&str>) -> rusqlite::Result<Option<String>> {
    let Some(task_id) = task_id else { return Ok(None) };
    let loaded = super::tasks::load(conn, workspace_id)?;
    if loaded.tasks.iter().any(|t| t.id == task_id && t.status.is_terminal()) {
        return Ok(None);
    }
    Ok(loaded.ctx(workspace_id).overturned_basis(task_id))
}

/// 在交易中查驗一項證據。`workflow_task` 是 Workflow 綁定的 Task（審查必須審這個 Task）；`builds` 是 Runtime 觀察到的
/// 目前 build（驗證證據的 build 必須是受驗目錄目前的 revision，票 50）。
fn check_evidence(
    conn: &Connection,
    workspace_id: &str,
    workflow_task: Option<&str>,
    builds: &Builds,
    evidence: Evidence,
) -> rusqlite::Result<CheckedEvidence> {
    let mut reviewers = None;
    let status = match &evidence.reference {
        EvidenceRef::Artifact { artifact_id } => {
            let found: Option<i64> = conn
                .query_row("SELECT 1 FROM artifact WHERE id = ?1 AND workspace_id = ?2", [artifact_id, workspace_id], |r| r.get(0))
                .optional()?;
            if found.is_some() { EvidenceStatus::Valid } else { EvidenceStatus::NotFound }
        }
        EvidenceRef::Decision { decision_request_id } => {
            let status: Option<String> = conn
                .query_row(
                    "SELECT status FROM decision_request WHERE id = ?1 AND workspace_id = ?2",
                    [decision_request_id, workspace_id],
                    |r| r.get(0),
                )
                .optional()?;
            match status.as_deref() {
                Some("answered") => EvidenceStatus::Valid,
                Some("pending") => EvidenceStatus::Pending,
                // 被取代或合併的請求不是有效的裁定
                _ => EvidenceStatus::NotFound,
            }
        }
        // 成果接受只看 Workflow 綁定的 Task（review-notes #21）
        EvidenceRef::TaskAccepted { task_id } if workflow_task.is_some_and(|bound| bound != task_id) => EvidenceStatus::OtherSubject,
        EvidenceRef::TaskAccepted { task_id } => {
            let found: Option<i64> = conn
                .query_row(
                    "SELECT contract_version FROM task WHERE id = ?1 AND workspace_id = ?2",
                    [task_id, workspace_id],
                    |r| r.get(0),
                )
                .optional()?;
            // 成果接受必須依據目前的契約（完成條件）：契約改版後，舊契約下的接受不算數（票 50）
            let accepted: Option<i64> = conn
                .query_row(
                    "SELECT 1 FROM task_verification t JOIN task ON task.id = t.task_id
                     WHERE t.task_id = ?1 AND t.outcome = 'passed' AND t.contract_version = task.contract_version",
                    [task_id],
                    |r| r.get(0),
                )
                .optional()?;
            match (found, accepted) {
                (None, _) => EvidenceStatus::NotFound,
                (Some(_), None) => EvidenceStatus::NotAccepted,
                (Some(_), Some(_)) => EvidenceStatus::Valid,
            }
        }
        EvidenceRef::Review { review_id } => {
            let current = builds.get(&subject_dir(conn, workspace_id, workflow_task)?).cloned().flatten();
            let (status, by) = super::reviews::check_review(conn, workspace_id, review_id, workflow_task, current.as_deref())?;
            reviewers = by;
            status
        }
        EvidenceRef::Verification { evidence_bundle_id } => {
            super::verification_profiles::verification_evidence_status(conn, workspace_id, evidence_bundle_id, workflow_task, builds)?
        }
    };
    Ok(CheckedEvidence { evidence, status, reviewers })
}

/// Stage 預算用盡後，證據齊全的回報要前進前依決策權限表判定「預算用盡後的下一步」（review-notes #20）：在記錄的同一個
/// 交易中由 Controller 的這一輪送出（同一個 Stage 第一次建立裁定請求，之後回到同一個請求；核准在這次回報用掉）。
/// 裁定請求必須掛在 Task 上，只綁定 Goal 的 Workflow 無法送出。
fn budget_decision(
    tx: &Transaction<'_>,
    workflow: &WorkflowInstance,
    playbook: &PlaybookVersion,
    stage: &str,
    controller_run_id: &str,
    attempt_id: &str,
    now: Timestamp,
) -> StoreResult<BudgetDecision> {
    let Some(task_id) = workflow.task_id.as_deref() else {
        return Ok(BudgetDecision::Unavailable { reason: "這個 Workflow 只綁定 Goal，沒有可以掛裁定請求的 Task".to_string() });
    };
    let title = playbook.definition.stage(stage).map_or(stage, |s| s.title.as_str());
    let summary = format!("讓 Workflow {} 的 Stage「{title}」在用盡預算後繼續", workflow.id);
    let reason = "這個 Stage 已用盡失敗預算而阻擋；Controller 已附齊證據，要繼續必須由你裁定";
    let content = match ActionContent::new(Matter::NextStepAfterBudget, Some(task_id), &summary, reason, Vec::new(), None) {
        Ok(content) => content.with_subject(&format!("stage:{}:{stage}", workflow.id)),
        Err(rejected) => return Ok(BudgetDecision::Unavailable { reason: rejected.to_string() }),
    };
    let command = DecisionCommand::ControllerGate {
        request_id: format!("decision_{attempt_id}"),
        audit_id: format!("authz_{attempt_id}"),
        controller_run_id: controller_run_id.to_string(),
        content,
    };
    Ok(match super::decisions::apply_in(tx, &workflow.workspace_id, command, now)? {
        Ok(DecisionOutcome::Controller { outcome, .. }) => match outcome {
            ControllerOutcome::Allowed | ControllerOutcome::Approved { .. } => BudgetDecision::Approved,
            ControllerOutcome::RequiresDecision { request_id, .. } => BudgetDecision::Pending { request_id },
            ControllerOutcome::Denied { request_id, .. } => BudgetDecision::Denied { request_id },
            ControllerOutcome::HumanOnly => {
                BudgetDecision::Unavailable { reason: "決策權限表把這個事項設為只能由使用者發起".to_string() }
            }
        },
        Ok(other) => unreachable!("ControllerGate 只會得到 Controller 結果：{other:?}"),
        Err(rejected) => BudgetDecision::Unavailable { reason: rejected.to_string() },
    })
}

/// 寫回 Workflow 的變更：新 Workflow 插入，既有的以先前的 revision 條件更新。
fn save_change(tx: &Transaction<'_>, change: &WorkflowChange, previous_revision: Option<i64>) -> rusqlite::Result<()> {
    let w = &change.workflow;
    match previous_revision {
        None => {
            tx.execute(
                "INSERT INTO workflow_instance
                    (id, workspace_id, goal_id, task_id, playbook_version_id, situation, route_reason, status, current_stage,
                     blocked_reason, revision, created_by, created_at, updated_at, resume_stage)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
                rusqlite::params![
                    w.id,
                    w.workspace_id,
                    w.goal_id,
                    w.task_id,
                    w.playbook_version_id,
                    w.situation.as_str(),
                    w.route_reason,
                    w.status.as_str(),
                    w.current_stage,
                    w.blocked_reason,
                    w.revision,
                    w.created_by,
                    w.created_at.as_millis(),
                    w.updated_at.as_millis(),
                    w.resume_stage,
                ],
            )?;
        }
        Some(previous) => {
            let updated = tx.execute(
                "UPDATE workflow_instance SET status = ?1, current_stage = ?2, blocked_reason = ?3, revision = ?4, updated_at = ?5,
                    resume_stage = ?8
                 WHERE id = ?6 AND revision = ?7",
                rusqlite::params![
                    w.status.as_str(),
                    w.current_stage,
                    w.blocked_reason,
                    w.revision,
                    w.updated_at.as_millis(),
                    w.id,
                    previous,
                    w.resume_stage,
                ],
            )?;
            assert_eq!(updated, 1, "同一個交易中讀到的 revision 一定還在");
        }
    }
    if let Some(a) = &change.attempt {
        insert_attempt(tx, a)?;
    }
    for event in &change.events {
        append_event(tx, event)?;
    }
    Ok(())
}

fn insert_attempt(tx: &Transaction<'_>, a: &StageAttempt) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO stage_attempt
            (id, workflow_id, stage_key, attempt, outcome, evidence, missing, note, recorded_by, profile_id, created_at, build)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        rusqlite::params![
            a.id,
            a.workflow_id,
            a.stage_key,
            a.attempt,
            a.outcome.as_str(),
            serde_json::to_string(&a.evidence).expect("證據一定可以序列化"),
            serde_json::to_string(&a.missing).expect("缺項一定可以序列化"),
            a.note,
            a.recorded_by,
            a.profile_id,
            a.created_at.as_millis(),
            a.build,
        ],
    )?;
    Ok(())
}

/// 未完成的 Workflow（全部 Workspace），依建立順序。
fn open_workflows(conn: &Connection) -> rusqlite::Result<Vec<WorkflowInstance>> {
    conn.prepare(&format!(
        "SELECT {WORKFLOW_COLUMNS} FROM workflow_instance w JOIN playbook_version p ON p.id = w.playbook_version_id
         JOIN workspace ws ON ws.id = w.workspace_id
         WHERE w.status <> 'completed' AND ws.status = 'active' ORDER BY w.created_at, w.rowid"
    ))?
    .query_map([], workflow_row)?
    .collect()
}

/// 證據引用的 Evidence Bundle 的受驗目錄。
fn bundle_dirs(conn: &Connection, workspace_id: &str, evidence: &[Evidence]) -> rusqlite::Result<Vec<String>> {
    let mut dirs = Vec::new();
    for item in evidence {
        if let EvidenceRef::Verification { evidence_bundle_id } = &item.reference {
            if let Some(bundle) = super::verification_profiles::bundle(conn, workspace_id, evidence_bundle_id)? {
                dirs.push(bundle.subject_dir);
            }
        }
    }
    Ok(dirs)
}

/// 一個失效的驗證 Gate 的重驗：以原本那份 Bundle 的旅程，對同一個 Task 排入新的驗證要求（已經有排隊中的就不重複）。
fn queue_reverification(
    tx: &Transaction<'_>,
    workflow: &WorkflowInstance,
    passed: &StageAttempt,
    id: String,
    now: Timestamp,
) -> rusqlite::Result<Option<String>> {
    let mut journeys = Vec::new();
    for item in &passed.evidence {
        if let EvidenceRef::Verification { evidence_bundle_id } = &item.evidence.reference {
            if let Some(bundle) = super::verification_profiles::bundle(tx, &workflow.workspace_id, evidence_bundle_id)? {
                journeys.extend(super::verification_profiles::request_journeys(tx, &bundle.request_id)?);
            }
        }
    }
    // 只供診斷的重現或修復驗證不算：它的 Bundle 不能作為 Gate 的驗證證據，沿用的話重驗等於被吃掉（R151 第 11 項）
    let queued: Option<String> = tx
        .query_row(
            "SELECT id FROM verification_request r WHERE workspace_id = ?1 AND task_id IS ?2 AND status = 'queued'
               AND NOT EXISTS (SELECT 1 FROM exploration_attempt e
                               WHERE e.request_id = r.id AND e.purpose IN ('reproduce', 'verify_fix'))",
            rusqlite::params![workflow.workspace_id, workflow.task_id],
            |r| r.get(0),
        )
        .optional()?;
    if queued.is_some() {
        return Ok(queued);
    }
    let requested_by = format!("{}:gate_invalidated:{}", workflow::RECORDED_BY_RUNTIME, workflow.id);
    super::verification_profiles::queue_reverification(tx, workflow, journeys, requested_by, id, now)
}

/// 重新查驗一個 Workflow 已通過的 Gate；失效時寫入並回傳摘要。
fn revalidate_one(
    tx: &Transaction<'_>,
    workflow: WorkflowInstance,
    builds: &Builds,
    now: Timestamp,
) -> rusqlite::Result<Option<RevalidatedWorkflow>> {
    let playbook = load_playbook(tx, &workflow.playbook_version_id)?;
    let attempts = load_attempts(tx, &workflow.id)?;
    let passed: Vec<StageAttempt> = workflow::passed_attempts(&workflow, &playbook, &attempts).into_iter().cloned().collect();
    if passed.is_empty() {
        return Ok(None);
    }
    let task = workflow.task_id.as_deref();
    let mut rechecked = Vec::new();
    for attempt in &passed {
        let evidence = attempt
            .evidence
            .iter()
            .map(|e| check_evidence(tx, &workflow.workspace_id, task, builds, e.evidence.clone()))
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rechecked.push((attempt.stage_key.clone(), evidence));
    }
    let dir = subject_dir(tx, &workflow.workspace_id, task)?;
    let facts = workflow::GateFacts {
        rechecked,
        current_build: builds.get(&dir).cloned().flatten(),
        basis_overturned: basis_overturned(tx, &workflow.workspace_id, task)?,
    };
    let Some(invalidation) = workflow::revalidate(&workflow, &playbook, &attempts, &facts, now) else { return Ok(None) };
    let change = WorkflowChange { workflow: invalidation.workflow.clone(), attempt: None, events: vec![invalidation.event.clone()] };
    save_change(tx, &change, Some(workflow.revision))?;
    let mut requests = Vec::new();
    let mut lines = Vec::new();
    for attempt in &invalidation.attempts {
        insert_attempt(tx, attempt)?;
        let stage = playbook.definition.stage(&attempt.stage_key);
        let title = stage.map_or(attempt.stage_key.clone(), |s| s.title.clone());
        lines.push(format!("Stage「{title}」：{}", attempt.note));
        let verifies = stage.is_some_and(|s| s.exit_checks.iter().any(|c| c.requires == workflow::EvidenceKind::Verification));
        let previous = passed.iter().find(|p| p.stage_key == attempt.stage_key);
        if let (true, Some(previous)) = (verifies, previous) {
            if let Some(request) = queue_reverification(tx, &workflow, previous, format!("vreq_{}", attempt.id), now)? {
                requests.push(request);
            }
        }
    }
    let target = task_title(tx, task)?.map(|t| format!("Task「{t}」")).unwrap_or_else(|| "Goal".to_string());
    let back = invalidation
        .workflow
        .current_stage
        .as_deref()
        .and_then(|key| playbook.definition.stage(key))
        .map(|s| format!("Workflow 目前在 Stage「{}」（revision {}）", s.title, invalidation.workflow.revision))
        .unwrap_or_default();
    let queued = if requests.is_empty() {
        String::new()
    } else {
        format!("已自動排入重驗的驗證要求：{}，完成後以新的 Evidence Bundle 回報。", requests.join("、"))
    };
    let content = format!(
        "Gate 失效，需要重驗（Workflow {}，{target}）：{}。{back}；請重新取得證據後以 record_stage_result 回報，\
         其他仍有效的 Gate 會被沿用。{queued}",
        workflow.id,
        lines.join("；"),
    );
    let source = format!("{}:{}:{}", workflow::EVENT_GATE_INVALIDATED, workflow.id, invalidation.workflow.revision);
    super::conversations::enqueue_notice(tx, &workflow.workspace_id, &source, &content, now)?;
    Ok(Some(RevalidatedWorkflow {
        workspace_id: workflow.workspace_id.clone(),
        workflow_id: workflow.id.clone(),
        stages: invalidation.attempts.iter().map(|a| a.stage_key.clone()).collect(),
        verification_requests: requests,
    }))
}

fn task_title(conn: &Connection, task_id: Option<&str>) -> rusqlite::Result<Option<String>> {
    let Some(task_id) = task_id else { return Ok(None) };
    conn.query_row("SELECT title FROM task WHERE id = ?1", [task_id], |r| r.get(0)).optional()
}

fn detail(conn: &Connection, workflow: WorkflowInstance, latest: &[PlaybookVersion]) -> rusqlite::Result<WorkflowDetail> {
    let playbook = load_playbook(conn, &workflow.playbook_version_id)?;
    let attempts = load_attempts(conn, &workflow.id)?;
    let task_title = task_title(conn, workflow.task_id.as_deref())?;
    let latest_playbook_version =
        latest.iter().find(|p| p.key == playbook.key).map_or(playbook.version, |p| p.version);
    Ok(WorkflowDetail { workflow, playbook, attempts, task_title, latest_playbook_version })
}

fn load_details(conn: &Connection, workspace_id: &str, open_only: bool) -> rusqlite::Result<Vec<WorkflowDetail>> {
    let filter = if open_only { "AND w.status <> 'completed'" } else { "" };
    let workflows = conn
        .prepare(&format!(
            "SELECT {WORKFLOW_COLUMNS} FROM workflow_instance w JOIN playbook_version p ON p.id = w.playbook_version_id
             WHERE w.workspace_id = ?1 {filter} ORDER BY w.created_at DESC, w.rowid DESC"
        ))?
        .query_map([workspace_id], workflow_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let latest = latest_of(&load_playbook_versions(conn)?, |p| p.key.as_str());
    workflows.into_iter().map(|w| detail(conn, w, &latest)).collect()
}

/// 給 Controller 的背景：這個 Workspace 未完成的 Workflow 與目前 Stage（依建立時間）。
pub(crate) fn workflow_notes(conn: &Connection, workspace_id: &str) -> rusqlite::Result<Vec<WorkflowNote>> {
    let mut notes: Vec<WorkflowNote> = load_details(conn, workspace_id, true)?
        .into_iter()
        .filter_map(|d| WorkflowNote::from_parts(&d.workflow, &d.playbook, &d.attempts, d.task_title))
        .collect();
    notes.reverse();
    Ok(notes)
}

/// 給 Worker 的 Stage 說明（Context Builder 的必要項目）：這個 Task 未完成的 Workflow 的目前 Stage。
pub(crate) fn worker_stage_item(conn: &Connection, task_id: &str) -> rusqlite::Result<Option<StageItem>> {
    let Some(workflow) = open_for_task(conn, task_id)? else { return Ok(None) };
    let playbook = load_playbook(conn, &workflow.playbook_version_id)?;
    let Some(text) = workflow::worker_stage_note(&workflow, &playbook) else { return Ok(None) };
    let title = workflow
        .current_stage
        .as_deref()
        .and_then(|key| playbook.definition.stage(key))
        .map(|stage| format!("Stage「{}」", stage.title))
        .unwrap_or_default();
    Ok(Some(StageItem { workflow_id: workflow.id, revision: workflow.revision, title, text }))
}

impl Store {
    /// 啟動時登錄內建的方法與 Playbook：內容與上一個內建版本不同時追加新版本，不覆寫舊版本（不追加事件）。
    pub async fn seed_builtin_methods_and_playbooks(&self, now: Timestamp) -> StoreResult<()> {
        self.write(move |tx| {
            let methods = load_method_versions(tx)?;
            for definition in workflow::builtin_methods() {
                let of_key: Vec<&MethodVersion> = methods.iter().filter(|m| m.key == definition.key).collect();
                let latest = of_key.iter().max_by_key(|m| m.version).copied();
                let id = format!("mtv_{}_v{}", definition.key, latest.map_or(1, |l| l.version + 1));
                if let Some((method, _)) = publish_method_checked(latest, definition, id, now) {
                    tx.execute(
                        "INSERT INTO method_version (id, key, version, definition, content_hash, origin, created_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                        rusqlite::params![
                            method.id,
                            method.key,
                            method.version,
                            serde_json::to_string(&method.definition).expect("方法一定可以序列化"),
                            method.content_hash,
                            method.origin.as_str(),
                            method.created_at.as_millis(),
                        ],
                    )?;
                }
            }
            let playbooks = load_playbook_versions(tx)?;
            for definition in workflow::builtin_playbooks() {
                // 與上一個內建版本相同時不動（使用者發布的較新版本仍是最新）
                let builtin_hash = serde_json::to_vec(&definition).map(|b| baton_domain::artifact::sha256_hex(&b)).ok();
                let unchanged = playbooks
                    .iter()
                    .filter(|p| p.key == definition.key && p.origin == Origin::Builtin)
                    .max_by_key(|p| p.version)
                    .is_some_and(|p| Some(&p.content_hash) == builtin_hash.as_ref());
                if !unchanged {
                    publish_in(tx, definition, Origin::Builtin, now).map_err(|e| match e {
                        WorkflowApplyError::Store(store) => store,
                        other => panic!("內建 Playbook 一定有效：{other}"),
                    })?;
                }
            }
            Ok(())
        })
        .await
    }

    /// 全部的 Playbook 與方法版本（依 key、版本排序）。
    pub async fn playbook_catalog(&self) -> StoreResult<(Vec<PlaybookVersion>, Vec<MethodVersion>)> {
        self.read(|conn| Ok((load_playbook_versions(conn)?, load_method_versions(conn)?))).await
    }

    /// 每個 Playbook 的最新版本（依 key 排序）。
    pub async fn latest_playbooks(&self) -> StoreResult<Vec<PlaybookVersion>> {
        self.read(|conn| Ok(latest_of(&load_playbook_versions(conn)?, |p| p.key.as_str()))).await
    }

    /// 使用者發布 Playbook 的新版本（只能升級既有的 Playbook）。回傳 (目前的最新版本, 是否新增)。
    pub async fn publish_playbook(
        &self,
        definition: PlaybookDefinition,
        now: Timestamp,
    ) -> Result<(PlaybookVersion, bool), WorkflowApplyError> {
        let outcome = self
            .write(move |tx| {
                let exists: Option<i64> = tx
                    .query_row("SELECT 1 FROM playbook_version WHERE key = ?1 LIMIT 1", [&definition.key], |r| r.get(0))
                    .optional()?;
                if exists.is_none() {
                    return Ok(Err(WorkflowApplyError::Rejected(WorkflowError::PlaybookNotFound(definition.key.clone()))));
                }
                match publish_in(tx, definition, Origin::User, now) {
                    Err(WorkflowApplyError::Store(store)) => Err(store),
                    other => Ok(other),
                }
            })
            .await?;
        outcome
    }

    /// 依情境開始一個 Workflow：解析綁定、Router 選 Playbook 最新版本、固定版本並記錄理由。
    #[allow(clippy::too_many_arguments)]
    pub async fn start_workflow(
        &self,
        workspace_id: &str,
        binding: WorkflowBinding,
        situation: Situation,
        rationale: String,
        created_by: &str,
        id: String,
        now: Timestamp,
    ) -> Result<WorkflowDetail, WorkflowApplyError> {
        let (workspace_id, created_by) = (workspace_id.to_string(), created_by.to_string());
        let outcome = self
            .write(move |tx| {
                let (goal_id, task_id, kind) = match binding {
                    WorkflowBinding::Task(task_id) => {
                        let found: Option<(String, String)> = tx
                            .query_row(
                                "SELECT goal_id, kind FROM task WHERE id = ?1 AND workspace_id = ?2",
                                [&task_id, &workspace_id],
                                |r| Ok((r.get(0)?, r.get(1)?)),
                            )
                            .optional()?;
                        let Some((goal_id, kind)) = found else { return Ok(Err(WorkflowApplyError::TaskNotFound(task_id))) };
                        (goal_id, Some(task_id), TaskKind::parse(&kind))
                    }
                    WorkflowBinding::Goal(goal_id) => {
                        let found: Option<i64> = tx
                            .query_row("SELECT 1 FROM goal WHERE id = ?1 AND workspace_id = ?2", [&goal_id, &workspace_id], |r| {
                                r.get(0)
                            })
                            .optional()?;
                        if found.is_none() {
                            return Ok(Err(WorkflowApplyError::GoalNotFound(goal_id)));
                        }
                        (goal_id, None, None)
                    }
                };
                let latest = latest_of(&load_playbook_versions(tx)?, |p| p.key.as_str());
                let route = match workflow::route(&latest, situation, kind, &rationale) {
                    Ok(route) => route,
                    Err(rejected) => return Ok(Err(rejected.into())),
                };
                let playbook = latest.iter().find(|p| p.id == route.playbook_version_id).expect("Router 選的版本一定存在");
                let open = match &task_id {
                    Some(task_id) => open_for_task(tx, task_id)?,
                    None => None,
                };
                let input = StartInput { workspace_id: workspace_id.clone(), goal_id, task_id, route, created_by };
                let change = match workflow::start(id, input, playbook, open.as_ref(), now) {
                    Ok(change) => change,
                    Err(rejected) => return Ok(Err(rejected.into())),
                };
                save_change(tx, &change, None)?;
                Ok(Ok(detail(tx, change.workflow, &latest)?))
            })
            .await?;
        outcome
    }

    /// 回報前 Runtime 要觀察 build 的目錄：Workflow 的受驗目錄，以及證據引用的 Evidence Bundle 的受驗目錄（票 50）。
    /// 找不到 Workflow 時是空的（回報會因此被拒絕）。
    pub async fn stage_result_build_dirs(
        &self,
        workspace_id: &str,
        workflow_id: &str,
        evidence: Vec<Evidence>,
    ) -> StoreResult<Vec<String>> {
        let (workspace_id, workflow_id) = (workspace_id.to_string(), workflow_id.to_string());
        self.read(move |conn| {
            let Some(workflow) = load_workflow(conn, &workspace_id, &workflow_id)? else { return Ok(Vec::new()) };
            let mut dirs = vec![subject_dir(conn, &workspace_id, workflow.task_id.as_deref())?];
            dirs.extend(bundle_dirs(conn, &workspace_id, &evidence)?);
            dirs.sort();
            dirs.dedup();
            Ok(dirs)
        })
        .await
    }

    /// 重新查驗 Gate 前 Runtime 要觀察 build 的目錄：每個未完成 Workflow 的受驗目錄與已通過 Gate 引用的 Bundle 的受驗目錄。
    pub async fn gate_build_dirs(&self) -> StoreResult<Vec<String>> {
        self.read(|conn| {
            let mut dirs = Vec::new();
            for workflow in open_workflows(conn)? {
                dirs.push(subject_dir(conn, &workflow.workspace_id, workflow.task_id.as_deref())?);
                let playbook = load_playbook(conn, &workflow.playbook_version_id)?;
                let attempts = load_attempts(conn, &workflow.id)?;
                for passed in workflow::passed_attempts(&workflow, &playbook, &attempts) {
                    let evidence: Vec<Evidence> = passed.evidence.iter().map(|e| e.evidence.clone()).collect();
                    dirs.extend(bundle_dirs(conn, &workflow.workspace_id, &evidence)?);
                }
            }
            dirs.sort();
            dirs.dedup();
            Ok(dirs)
        })
        .await
    }

    /// 重新查驗每個未完成 Workflow 已通過的 Gate（票 50）：證據不再有效、build 改變或 Task 依據的假設被推翻時 Gate 失效，
    /// Workflow 退回最早失效的 Stage；驗證 Gate 失效時自動排入重驗的驗證要求，並以工作事件通知 Controller。
    /// 全部在一個交易中完成。
    pub async fn revalidate_gates(&self, builds: Builds, now: Timestamp) -> StoreResult<Vec<RevalidatedWorkflow>> {
        self.write(move |tx| {
            let mut done = Vec::new();
            for workflow in open_workflows(tx)? {
                if let Some(summary) = revalidate_one(tx, workflow, &builds, now)? {
                    done.push(summary);
                }
            }
            Ok(done)
        })
        .await
    }

    /// 以證據回報目前 Stage 的結果。記錄者是 Controller 的一輪（帳號由那一輪決定）。`builds` 是回報前觀察到的 build
    /// （[`Store::stage_result_build_dirs`] 的目錄）：通過時隨 attempt 保存，驗證證據必須是目前的 build（票 50）。
    #[allow(clippy::too_many_arguments)]
    pub async fn record_stage_result(
        &self,
        workspace_id: &str,
        workflow_id: &str,
        stage: String,
        expected_revision: i64,
        outcome: ClaimedOutcome,
        evidence: Vec<Evidence>,
        note: String,
        controller_run_id: &str,
        attempt_id: String,
        builds: Builds,
        now: Timestamp,
    ) -> Result<(StageAttempt, WorkflowDetail), WorkflowApplyError> {
        let (workspace_id, workflow_id, controller_run_id) =
            (workspace_id.to_string(), workflow_id.to_string(), controller_run_id.to_string());
        let result = self
            .write(move |tx| {
                let Some(workflow) = load_workflow(tx, &workspace_id, &workflow_id)? else {
                    return Ok(Err(WorkflowError::WorkflowNotFound(workflow_id).into()));
                };
                let playbook = load_playbook(tx, &workflow.playbook_version_id)?;
                let prior = load_attempts(tx, &workflow.id)?;
                let task = workflow.task_id.as_deref();
                let checked = evidence
                    .into_iter()
                    .map(|e| check_evidence(tx, &workspace_id, task, &builds, e))
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                let build = builds.get(&subject_dir(tx, &workspace_id, task)?).cloned().flatten();
                let basis_overturned = basis_overturned(tx, &workspace_id, task)?;
                let profile_id: Option<String> = tx
                    .query_row("SELECT profile_id FROM controller_run WHERE id = ?1", [&controller_run_id], |r| r.get(0))
                    .optional()?;
                let mut input = StageResultInput {
                    stage,
                    expected_revision,
                    outcome,
                    evidence: checked,
                    note,
                    build,
                    basis_overturned,
                    budget_decision: BudgetDecision::NotRequested,
                };
                match workflow::needs_budget_decision(&workflow, &playbook, &prior, &input) {
                    Ok(false) => {}
                    Ok(true) => {
                        input.budget_decision =
                            budget_decision(tx, &workflow, &playbook, &input.stage, &controller_run_id, &attempt_id, now)?;
                    }
                    Err(rejected) => return Ok(Err(rejected.into())),
                }
                let recorder = Recorder { recorded_by: controller_run_id, profile_id };
                let change = match workflow::record_result(&workflow, &playbook, &prior, input, attempt_id, recorder, now) {
                    Ok(change) => change,
                    Err(rejected) => return Ok(Err(rejected.into())),
                };
                save_change(tx, &change, Some(workflow.revision))?;
                let attempt = change.attempt.clone().expect("回報一定追加 attempt");
                let latest = latest_of(&load_playbook_versions(tx)?, |p| p.key.as_str());
                Ok(Ok((attempt, detail(tx, change.workflow, &latest)?)))
            })
            .await?;
        result
    }

    /// 使用者跳過目前的 Stage（附原因；裁定與安全 Gate 不能跳過）。
    pub async fn skip_stage(
        &self,
        workspace_id: &str,
        workflow_id: &str,
        stage: String,
        expected_revision: i64,
        reason: String,
        attempt_id: String,
        now: Timestamp,
    ) -> Result<WorkflowDetail, WorkflowApplyError> {
        let (workspace_id, workflow_id) = (workspace_id.to_string(), workflow_id.to_string());
        let result = self
            .write(move |tx| {
                let Some(workflow) = load_workflow(tx, &workspace_id, &workflow_id)? else {
                    return Ok(Err(WorkflowError::WorkflowNotFound(workflow_id).into()));
                };
                let playbook = load_playbook(tx, &workflow.playbook_version_id)?;
                let prior = load_attempts(tx, &workflow.id)?;
                let change =
                    match workflow::skip_stage(&workflow, &playbook, &prior, &stage, expected_revision, &reason, attempt_id, now) {
                        Ok(change) => change,
                        Err(rejected) => return Ok(Err(rejected.into())),
                    };
                save_change(tx, &change, Some(workflow.revision))?;
                let latest = latest_of(&load_playbook_versions(tx)?, |p| p.key.as_str());
                Ok(Ok(detail(tx, change.workflow, &latest)?))
            })
            .await?;
        result
    }

    /// 某個 Workspace 的全部 Workflow（最新的在前）。
    pub async fn workflows(&self, workspace_id: &str) -> StoreResult<Vec<WorkflowDetail>> {
        let workspace_id = workspace_id.to_string();
        self.read(move |conn| Ok(load_details(conn, &workspace_id, false)?)).await
    }
}

/// 內建方法的內容一定有效；與最新版本相同時回 `None`。
fn publish_method_checked(
    latest: Option<&MethodVersion>,
    definition: MethodDefinition,
    id: String,
    now: Timestamp,
) -> Option<(MethodVersion, DomainEvent)> {
    workflow::publish_method(latest, definition, Origin::Builtin, id, now).expect("內建方法一定有效")
}
