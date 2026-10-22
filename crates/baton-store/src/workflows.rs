//! 方法登錄、Playbook 版本、Workflow 與 stage attempt 的持久化（票 43）。
//!
//! - 方法與 Playbook 版本只追加（資料表以 trigger 禁止更新與刪除）；識別是 `<前綴>_<key>_v<版本>`。
//! - Workflow 的每次變更在單一交易中完成：載入 Workflow、固定的 Playbook 版本與先前的 attempt → 查驗證據 →
//!   Domain 判定 → 寫回 Workflow（以 revision 條件更新）、追加 attempt 與事件。
//! - 證據由這裡在同一個交易中查驗（成果、裁定、成果接受），不採信 Controller 自報。

use baton_domain::context::StageItem;
use baton_domain::workdir::TaskKind;
use baton_domain::workflow::{
    self, AttemptOutcome, CheckedEvidence, ClaimedOutcome, Evidence, EvidenceRef, EvidenceStatus, MethodDefinition,
    MethodVersion, Origin, PlaybookDefinition, PlaybookVersion, Recorder, Situation, StageAttempt, StageResultInput,
    StartInput, WorkflowChange, WorkflowError, WorkflowInstance, WorkflowNote, WorkflowStatus,
};
use rusqlite::OptionalExtension;

use super::*;

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
     w.route_reason, w.status, w.current_stage, w.blocked_reason, w.revision, w.created_by, w.created_at, w.updated_at";
const ATTEMPT_COLUMNS: &str =
    "id, workflow_id, stage_key, attempt, outcome, evidence, missing, note, recorded_by, profile_id, created_at";

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
            append_event(tx, &event)?;
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

/// 在交易中查驗一項證據。
fn check_evidence(conn: &Connection, workspace_id: &str, evidence: Evidence) -> rusqlite::Result<CheckedEvidence> {
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
        EvidenceRef::TaskAccepted { task_id } => {
            let found: Option<i64> = conn
                .query_row("SELECT 1 FROM task WHERE id = ?1 AND workspace_id = ?2", [task_id, workspace_id], |r| r.get(0))
                .optional()?;
            let accepted: Option<i64> = conn
                .query_row("SELECT 1 FROM task_verification WHERE task_id = ?1 AND outcome = 'passed'", [task_id], |r| r.get(0))
                .optional()?;
            match (found, accepted) {
                (None, _) => EvidenceStatus::NotFound,
                (Some(_), None) => EvidenceStatus::NotAccepted,
                (Some(_), Some(_)) => EvidenceStatus::Valid,
            }
        }
    };
    Ok(CheckedEvidence { evidence, status })
}

/// 寫回 Workflow 的變更：新 Workflow 插入，既有的以先前的 revision 條件更新。
fn save_change(tx: &Transaction<'_>, change: &WorkflowChange, previous_revision: Option<i64>) -> rusqlite::Result<()> {
    let w = &change.workflow;
    match previous_revision {
        None => {
            tx.execute(
                "INSERT INTO workflow_instance
                    (id, workspace_id, goal_id, task_id, playbook_version_id, situation, route_reason, status, current_stage,
                     blocked_reason, revision, created_by, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
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
                ],
            )?;
        }
        Some(previous) => {
            let updated = tx.execute(
                "UPDATE workflow_instance SET status = ?1, current_stage = ?2, blocked_reason = ?3, revision = ?4, updated_at = ?5
                 WHERE id = ?6 AND revision = ?7",
                rusqlite::params![
                    w.status.as_str(),
                    w.current_stage,
                    w.blocked_reason,
                    w.revision,
                    w.updated_at.as_millis(),
                    w.id,
                    previous,
                ],
            )?;
            assert_eq!(updated, 1, "同一個交易中讀到的 revision 一定還在");
        }
    }
    if let Some(a) = &change.attempt {
        tx.execute(
            "INSERT INTO stage_attempt
                (id, workflow_id, stage_key, attempt, outcome, evidence, missing, note, recorded_by, profile_id, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
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
            ],
        )?;
    }
    for event in &change.events {
        append_event(tx, event)?;
    }
    Ok(())
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
    /// 啟動時登錄內建的方法與 Playbook：內容與上一個內建版本不同時追加新版本，不覆寫舊版本。
    pub async fn seed_builtin_methods_and_playbooks(&self, now: Timestamp) -> StoreResult<()> {
        self.write(move |tx| {
            let methods = load_method_versions(tx)?;
            for definition in workflow::builtin_methods() {
                let of_key: Vec<&MethodVersion> = methods.iter().filter(|m| m.key == definition.key).collect();
                let latest = of_key.iter().max_by_key(|m| m.version).copied();
                let id = format!("mtv_{}_v{}", definition.key, latest.map_or(1, |l| l.version + 1));
                if let Some((method, event)) = publish_method_checked(latest, definition, id, now) {
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
                    append_event(tx, &event)?;
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

    /// 以證據回報目前 Stage 的結果。記錄者是 Controller 的一輪（帳號由那一輪決定）。
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
                let checked =
                    evidence.into_iter().map(|e| check_evidence(tx, &workspace_id, e)).collect::<rusqlite::Result<Vec<_>>>()?;
                let profile_id: Option<String> = tx
                    .query_row("SELECT profile_id FROM controller_run WHERE id = ?1", [&controller_run_id], |r| r.get(0))
                    .optional()?;
                let input = StageResultInput { stage, expected_revision, outcome, evidence: checked, note };
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
