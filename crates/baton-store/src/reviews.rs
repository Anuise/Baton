//! 雙軸 Review、finding 分類與設計偏離的持久化（票 44）。
//!
//! - 審查只追加；reviewer 與作者的身分（Task、用過的 Session）、被審 Task 目前的契約版本都在同一個交易中由這裡查出，
//!   不採信 reviewer 自報。要沿用的先前審查也在交易中重新載入；變更檔案由 API 在交易外以 Git 算好後傳進來。
//! - finding 的分類與設計偏離的處理都以 Domain 判定後寫回，並追加事件。
//! - 接受設計偏離時，在同一個交易中替設計成果追加新版本（內容由 API 先寫成新的檔案）。

use std::collections::HashMap;

use baton_domain::artifact::{ArtifactVersion, EVENT_ARTIFACT_REGISTERED};
use baton_domain::auth::AgentScope;
use baton_domain::review::{
    self, AxisResult, Classification, DesignDeviation, DeviationInput, DeviationResolution, DeviationStatus, Finding,
    FindingChange, FindingState, PriorReview, Reporter, Review, ReviewError, ReviewSubmission, Reviewer, SubjectTask,
};
use baton_domain::run;
use baton_domain::workflow::{AxisReviewers, EvidenceStatus};
use rusqlite::OptionalExtension;

use super::tasks::load;
use super::*;

/// 套用審查或設計偏離的失敗：Domain 拒絕、呼叫者已不是寫入所有者，或儲存層錯誤。
#[derive(Debug, thiserror::Error)]
pub enum ReviewApplyError {
    #[error(transparent)]
    Rejected(#[from] ReviewError),
    #[error("這個 Run 已不是目前的寫入所有者")]
    Stale,
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl From<rusqlite::Error> for ReviewApplyError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error.into())
    }
}

/// 一份審查與檢視它需要的資料。
#[derive(Debug, Clone, PartialEq)]
pub struct ReviewRecord {
    pub review: Review,
    pub findings: Vec<Finding>,
    pub subject_task_title: Option<String>,
    pub reviewer_task_title: Option<String>,
    /// 被審 Task 目前的契約版本。
    pub current_contract_version: i64,
    /// 同一個 Task 最新一份審查的 candidate。
    pub latest_candidate: String,
}

/// 一個 Workspace（或一個 Task）的審查與設計偏離，最新的在前。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ReviewsData {
    pub reviews: Vec<ReviewRecord>,
    /// (偏離, 設計成果的名稱, 回報偏離的 Task 名稱)
    pub deviations: Vec<(DesignDeviation, Option<String>, Option<String>)>,
}

const REVIEW_COLUMNS: &str = "id, workspace_id, subject_task_id, reviewer_run_id, reviewer_task_id, base_revision, \
     candidate_revision, spec_version, standards_source, standards, spec, created_at";
const FINDING_COLUMNS: &str = "id, review_id, axis, title, detail, location, state, classification, reason, \
     decision_request_id, decided_by, classified_by, version, created_at, updated_at";
const DEVIATION_COLUMNS: &str = "id, workspace_id, task_id, reported_by_run_id, reported_generation, design_artifact_id, \
     design_version, deviation, reason, affected_contracts, proposed_design, status, resolution_reason, decision_request_id, \
     accepted_by, resolved_by, accepted_design_version, version, created_at, updated_at";

fn unknown(index: usize, what: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Text, format!("未知的{what}").into())
}

fn review_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Review> {
    Ok(Review {
        id: r.get(0)?,
        workspace_id: r.get(1)?,
        subject_task_id: r.get(2)?,
        reviewer_run_id: r.get(3)?,
        reviewer_task_id: r.get(4)?,
        base_revision: r.get(5)?,
        candidate_revision: r.get(6)?,
        spec_version: r.get(7)?,
        standards_source: r.get(8)?,
        standards: json_column::<AxisResult>(9, &r.get::<_, String>(9)?)?,
        spec: json_column::<AxisResult>(10, &r.get::<_, String>(10)?)?,
        created_at: Timestamp::from_millis(r.get(11)?),
    })
}

fn finding_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Finding> {
    let axis: String = r.get(2)?;
    let state: String = r.get(6)?;
    let classification: Option<String> = r.get(7)?;
    Ok(Finding {
        id: r.get(0)?,
        review_id: r.get(1)?,
        axis: review::Axis::parse(&axis).ok_or_else(|| unknown(2, "審查軸"))?,
        title: r.get(3)?,
        detail: r.get(4)?,
        location: r.get(5)?,
        state: FindingState::parse(&state).ok_or_else(|| unknown(6, "finding 狀態"))?,
        classification: match classification {
            Some(raw) => Some(Classification::parse(&raw).ok_or_else(|| unknown(7, "finding 分類"))?),
            None => None,
        },
        reason: r.get(8)?,
        decision_request_id: r.get(9)?,
        decided_by: r.get(10)?,
        classified_by: r.get(11)?,
        version: r.get(12)?,
        created_at: Timestamp::from_millis(r.get(13)?),
        updated_at: Timestamp::from_millis(r.get(14)?),
    })
}

fn deviation_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<DesignDeviation> {
    let status: String = r.get(11)?;
    Ok(DesignDeviation {
        id: r.get(0)?,
        workspace_id: r.get(1)?,
        task_id: r.get(2)?,
        reported_by_run_id: r.get(3)?,
        reported_generation: r.get(4)?,
        design_artifact_id: r.get(5)?,
        design_version: r.get(6)?,
        deviation: r.get(7)?,
        reason: r.get(8)?,
        affected_contracts: json_column(9, &r.get::<_, String>(9)?)?,
        proposed_design: r.get(10)?,
        status: DeviationStatus::parse(&status).ok_or_else(|| unknown(11, "設計偏離狀態"))?,
        resolution_reason: r.get(12)?,
        decision_request_id: r.get(13)?,
        accepted_by: r.get(14)?,
        resolved_by: r.get(15)?,
        accepted_design_version: r.get(16)?,
        version: r.get(17)?,
        created_at: Timestamp::from_millis(r.get(18)?),
        updated_at: Timestamp::from_millis(r.get(19)?),
    })
}

fn load_review(conn: &Connection, workspace_id: &str, id: &str) -> rusqlite::Result<Option<Review>> {
    conn.query_row(&format!("SELECT {REVIEW_COLUMNS} FROM review WHERE workspace_id = ?1 AND id = ?2"), [workspace_id, id], review_row)
        .optional()
}

fn load_findings(conn: &Connection, review_id: &str) -> rusqlite::Result<Vec<Finding>> {
    conn.prepare(&format!("SELECT {FINDING_COLUMNS} FROM review_finding WHERE review_id = ?1 ORDER BY rowid"))?
        .query_map([review_id], finding_row)?
        .collect()
}

fn load_deviation(conn: &Connection, workspace_id: &str, id: &str) -> rusqlite::Result<Option<DesignDeviation>> {
    conn.query_row(
        &format!("SELECT {DEVIATION_COLUMNS} FROM design_deviation WHERE workspace_id = ?1 AND id = ?2"),
        [workspace_id, id],
        deviation_row,
    )
    .optional()
}

fn contract_version(conn: &Connection, workspace_id: &str, task_id: &str) -> rusqlite::Result<Option<i64>> {
    conn.query_row("SELECT contract_version FROM task WHERE id = ?1 AND workspace_id = ?2", [task_id, workspace_id], |r| r.get(0))
        .optional()
}

/// 同一個 Task 最新一份審查的 candidate。
fn latest_candidate(conn: &Connection, subject_task_id: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT candidate_revision FROM review WHERE subject_task_id = ?1 ORDER BY created_at DESC, rowid DESC LIMIT 1",
        [subject_task_id],
        |r| r.get(0),
    )
    .optional()
}

/// 一個 Task 的 Run 用過的全部 Session（平台 Session 與原生 Session）。
fn task_sessions(conn: &Connection, task_id: &str) -> rusqlite::Result<Vec<String>> {
    conn.prepare(
        "SELECT agent_session_id FROM run WHERE task_id = ?1 AND agent_session_id IS NOT NULL
         UNION SELECT native_session_id FROM run WHERE task_id = ?1 AND native_session_id IS NOT NULL
         UNION SELECT resume_native_session_id FROM run WHERE task_id = ?1 AND resume_native_session_id IS NOT NULL",
    )?
    .query_map([task_id], |r| r.get(0))?
    .collect()
}

/// 一個 Run 用過的 Session。
fn run_sessions(conn: &Connection, run_id: &str) -> rusqlite::Result<Vec<String>> {
    let (agent, native, resume): (Option<String>, Option<String>, Option<String>) = conn.query_row(
        "SELECT agent_session_id, native_session_id, resume_native_session_id FROM run WHERE id = ?1",
        [run_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    Ok([agent, native, resume].into_iter().flatten().collect())
}

fn task_title(conn: &Connection, task_id: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row("SELECT title FROM task WHERE id = ?1", [task_id], |r| r.get(0)).optional()
}

fn record(conn: &Connection, review: Review) -> rusqlite::Result<ReviewRecord> {
    let findings = load_findings(conn, &review.id)?;
    let current_contract_version = contract_version(conn, &review.workspace_id, &review.subject_task_id)?.unwrap_or(review.spec_version);
    let latest_candidate = latest_candidate(conn, &review.subject_task_id)?.unwrap_or_else(|| review.candidate_revision.clone());
    Ok(ReviewRecord {
        subject_task_title: task_title(conn, &review.subject_task_id)?,
        reviewer_task_title: task_title(conn, &review.reviewer_task_id)?,
        findings,
        current_contract_version,
        latest_candidate,
        review,
    })
}

/// 設計成果目前的最新版本；不是這個 Workspace 的成果時 `None`。
fn design_latest(conn: &Connection, workspace_id: &str, artifact_id: &str) -> rusqlite::Result<Option<i64>> {
    conn.query_row(
        "SELECT MAX(v.version) FROM artifact a JOIN artifact_version v ON v.artifact_id = a.id WHERE a.id = ?1 AND a.workspace_id = ?2",
        [artifact_id, workspace_id],
        |r| r.get(0),
    )
}

fn save_finding(tx: &Transaction<'_>, finding: &Finding) -> rusqlite::Result<()> {
    tx.execute(
        "UPDATE review_finding SET state = ?1, classification = ?2, reason = ?3, decision_request_id = ?4, decided_by = ?5,
             classified_by = ?6, version = ?7, updated_at = ?8
         WHERE id = ?9",
        rusqlite::params![
            finding.state.as_str(),
            finding.classification.map(Classification::as_str),
            finding.reason,
            finding.decision_request_id,
            finding.decided_by,
            finding.classified_by,
            finding.version,
            finding.updated_at.as_millis(),
            finding.id,
        ],
    )?;
    Ok(())
}

fn save_deviation_status(tx: &Transaction<'_>, d: &DesignDeviation) -> rusqlite::Result<()> {
    tx.execute(
        "UPDATE design_deviation SET status = ?1, resolution_reason = ?2, decision_request_id = ?3, accepted_by = ?4,
             resolved_by = ?5, accepted_design_version = ?6, version = ?7, updated_at = ?8
         WHERE id = ?9",
        rusqlite::params![
            d.status.as_str(),
            d.resolution_reason,
            d.decision_request_id,
            d.accepted_by,
            d.resolved_by,
            d.accepted_design_version,
            d.version,
            d.updated_at.as_millis(),
            d.id,
        ],
    )?;
    Ok(())
}

/// Gate 查驗審查證據（票 44）：審查存在、審的是 Workflow 的 Task、對照目前的規格版本、是最新 candidate 的審查、兩軸都通過。
pub(crate) fn check_review(
    conn: &Connection,
    workspace_id: &str,
    review_id: &str,
    workflow_task: Option<&str>,
) -> rusqlite::Result<(EvidenceStatus, Option<AxisReviewers>)> {
    let Some(review) = load_review(conn, workspace_id, review_id)? else {
        return Ok((EvidenceStatus::NotFound, None));
    };
    let record = record(conn, review)?;
    let standing = review::standing(
        &record.review,
        &record.findings,
        record.current_contract_version,
        &record.latest_candidate,
        workflow_task,
    );
    let status = match standing {
        review::Standing::Passed => EvidenceStatus::Valid,
        review::Standing::NotPassed(_) => EvidenceStatus::NotPassed,
        review::Standing::Stale(_) => EvidenceStatus::Stale,
        review::Standing::OtherSubject => EvidenceStatus::OtherSubject,
    };
    Ok((status, Some(record.review.reviewers())))
}

impl Store {
    /// 一份審查（沿用前由 API 取得先前的 candidate，以便在交易外計算變更檔案）。
    pub async fn review(&self, workspace_id: &str, review_id: &str) -> StoreResult<Option<Review>> {
        let (workspace_id, review_id) = (workspace_id.to_string(), review_id.to_string());
        self.read(move |conn| Ok(load_review(conn, &workspace_id, &review_id)?)).await
    }

    /// reviewer（Worker）提交雙軸審查。`changed_paths` 是每份要沿用的先前審查「其 candidate → 這次 candidate」的變更檔案
    /// （算不出時是原因）；`finding_ids` 依序給新的 finding 使用。
    pub async fn submit_review(
        &self,
        scope: &AgentScope,
        input: ReviewSubmission,
        changed_paths: HashMap<String, Result<Vec<String>, String>>,
        id: String,
        finding_ids: Vec<String>,
        now: Timestamp,
    ) -> Result<ReviewRecord, ReviewApplyError> {
        let scope = scope.clone();
        let outcome = self
            .write(move |tx| {
                let loaded = load(tx, &scope.workspace_id)?;
                let Some(run) = loaded.runs.iter().find(|r| r.id == scope.run_id) else {
                    return Ok(Err(ReviewApplyError::Stale));
                };
                let lease = loaded.leases.iter().find(|l| l.run_id == run.id);
                if !run::is_current_writer(run, lease, scope.generation) {
                    return Ok(Err(ReviewApplyError::Stale));
                }
                let subject_id = input.subject_task_id.trim().to_string();
                let Some(version) = contract_version(tx, &scope.workspace_id, &subject_id)? else {
                    return Ok(Err(ReviewError::TaskNotFound(subject_id).into()));
                };
                let subject = SubjectTask {
                    workspace_id: scope.workspace_id.clone(),
                    id: subject_id.clone(),
                    contract_version: version,
                    author_sessions: task_sessions(tx, &subject_id)?,
                };
                let reviewer = Reviewer { run_id: run.id.clone(), task_id: run.task_id.clone(), sessions: run_sessions(tx, &run.id)? };
                let mut priors = Vec::new();
                for (prior_id, changed) in changed_paths {
                    if let Some(prior) = load_review(tx, &scope.workspace_id, &prior_id)? {
                        let findings = load_findings(tx, &prior.id)?;
                        priors.push(PriorReview { review: prior, findings, changed_paths: changed });
                    }
                }
                let mut ids = finding_ids.into_iter();
                let mut next_id = || ids.next().expect("API 依 finding 數量產生識別");
                let submitted = match review::submit(id, &mut next_id, input, &reviewer, &subject, &priors, now) {
                    Ok(submitted) => submitted,
                    Err(rejected) => return Ok(Err(rejected.into())),
                };
                let r = &submitted.review;
                tx.execute(
                    &format!("INSERT INTO review ({REVIEW_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)"),
                    rusqlite::params![
                        r.id,
                        r.workspace_id,
                        r.subject_task_id,
                        r.reviewer_run_id,
                        r.reviewer_task_id,
                        r.base_revision,
                        r.candidate_revision,
                        r.spec_version,
                        r.standards_source,
                        serde_json::to_string(&r.standards).expect("審查軸一定可以序列化"),
                        serde_json::to_string(&r.spec).expect("審查軸一定可以序列化"),
                        r.created_at.as_millis(),
                    ],
                )?;
                for f in &submitted.findings {
                    tx.execute(
                        &format!(
                            "INSERT INTO review_finding ({FINDING_COLUMNS})
                             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, NULL, NULL, NULL, NULL, ?8, ?9, ?9)"
                        ),
                        rusqlite::params![
                            f.id,
                            f.review_id,
                            f.axis.as_str(),
                            f.title,
                            f.detail,
                            f.location,
                            f.state.as_str(),
                            f.version,
                            f.created_at.as_millis(),
                        ],
                    )?;
                }
                append_event(tx, &submitted.event)?;
                Ok(Ok(record(tx, submitted.review)?))
            })
            .await?;
        outcome
    }

    /// 某個 Workspace 的審查與設計偏離（指定 Task 時只取審這個 Task 的審查與這個 Task 回報的偏離），最新的在前。
    pub async fn reviews(&self, workspace_id: &str, task_id: Option<&str>) -> StoreResult<ReviewsData> {
        let (workspace_id, task_id) = (workspace_id.to_string(), task_id.map(str::to_string));
        self.read(move |conn| {
            let reviews = conn
                .prepare(&format!(
                    "SELECT {REVIEW_COLUMNS} FROM review WHERE workspace_id = ?1 AND (?2 IS NULL OR subject_task_id = ?2)
                     ORDER BY created_at DESC, rowid DESC"
                ))?
                .query_map(rusqlite::params![workspace_id, task_id], review_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let reviews = reviews.into_iter().map(|r| record(conn, r)).collect::<rusqlite::Result<Vec<_>>>()?;
            let deviations = conn
                .prepare(&format!(
                    "SELECT {DEVIATION_COLUMNS} FROM design_deviation WHERE workspace_id = ?1 AND (?2 IS NULL OR task_id = ?2)
                     ORDER BY created_at DESC, rowid DESC"
                ))?
                .query_map(rusqlite::params![workspace_id, task_id], deviation_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let mut with_names = Vec::new();
            for d in deviations {
                let design: Option<String> =
                    conn.query_row("SELECT name FROM artifact WHERE id = ?1", [&d.design_artifact_id], |r| r.get(0)).optional()?;
                let task = task_title(conn, &d.task_id)?;
                with_names.push((d, design, task));
            }
            Ok(ReviewsData { reviews, deviations: with_names })
        })
        .await
    }

    /// 一項 finding 與它所屬的審查。
    pub async fn finding(&self, workspace_id: &str, finding_id: &str) -> StoreResult<Option<(Finding, ReviewRecord)>> {
        let (workspace_id, finding_id) = (workspace_id.to_string(), finding_id.to_string());
        self.read(move |conn| {
            let found = conn
                .query_row(&format!("SELECT {FINDING_COLUMNS} FROM review_finding WHERE id = ?1"), [&finding_id], finding_row)
                .optional()?;
            let Some(finding) = found else { return Ok(None) };
            let Some(review) = load_review(conn, &workspace_id, &finding.review_id)? else { return Ok(None) };
            Ok(Some((finding, record(conn, review)?)))
        })
        .await
    }

    /// 套用 finding 的分類，回傳更新後的 finding 與它所屬的審查。
    #[allow(clippy::too_many_arguments)]
    pub async fn classify_finding(
        &self,
        workspace_id: &str,
        finding_id: &str,
        change: FindingChange,
        decision_request_id: Option<String>,
        decided_by: Option<String>,
        classified_by: &str,
        now: Timestamp,
    ) -> Result<(Finding, ReviewRecord), ReviewApplyError> {
        let (workspace_id, finding_id, classified_by) =
            (workspace_id.to_string(), finding_id.to_string(), classified_by.to_string());
        let outcome = self
            .write(move |tx| {
                let found = tx
                    .query_row(&format!("SELECT {FINDING_COLUMNS} FROM review_finding WHERE id = ?1"), [&finding_id], finding_row)
                    .optional()?;
                let review = match &found {
                    Some(f) => load_review(tx, &workspace_id, &f.review_id)?,
                    None => None,
                };
                let (Some(finding), Some(review)) = (found, review) else {
                    return Ok(Err(ReviewError::FindingNotFound(finding_id).into()));
                };
                let (next, event) = match review::classify(&finding, change, decision_request_id, decided_by, &classified_by, now) {
                    Ok(changed) => changed,
                    Err(rejected) => return Ok(Err(rejected.into())),
                };
                save_finding(tx, &next)?;
                append_event(tx, &event)?;
                Ok(Ok((next, record(tx, review)?)))
            })
            .await?;
        outcome
    }

    /// Worker 回報實作偏離設計。設計必須是這個 Workspace 的成果，而且對照的是它目前的最新版本。
    pub async fn report_deviation(
        &self,
        scope: &AgentScope,
        input: DeviationInput,
        id: String,
        now: Timestamp,
    ) -> Result<DesignDeviation, ReviewApplyError> {
        let scope = scope.clone();
        let outcome = self
            .write(move |tx| {
                let loaded = load(tx, &scope.workspace_id)?;
                let Some(run) = loaded.runs.iter().find(|r| r.id == scope.run_id) else {
                    return Ok(Err(ReviewApplyError::Stale));
                };
                let lease = loaded.leases.iter().find(|l| l.run_id == run.id);
                if !run::is_current_writer(run, lease, scope.generation) {
                    return Ok(Err(ReviewApplyError::Stale));
                }
                let latest = design_latest(tx, &scope.workspace_id, input.design_artifact_id.trim())?;
                let reporter = Reporter {
                    workspace_id: scope.workspace_id.clone(),
                    run_id: run.id.clone(),
                    task_id: run.task_id.clone(),
                    generation: scope.generation,
                };
                let (d, event) = match review::report_deviation(id, input, &reporter, latest, now) {
                    Ok(reported) => reported,
                    Err(rejected) => return Ok(Err(rejected.into())),
                };
                tx.execute(
                    &format!(
                        "INSERT INTO design_deviation ({DEVIATION_COLUMNS})
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, NULL, NULL, NULL, NULL, NULL, ?13, ?14, ?14)"
                    ),
                    rusqlite::params![
                        d.id,
                        d.workspace_id,
                        d.task_id,
                        d.reported_by_run_id,
                        d.reported_generation,
                        d.design_artifact_id,
                        d.design_version,
                        d.deviation,
                        d.reason,
                        serde_json::to_string(&d.affected_contracts).expect("字串清單一定可以序列化"),
                        d.proposed_design,
                        d.status.as_str(),
                        d.version,
                        d.created_at.as_millis(),
                    ],
                )?;
                append_event(tx, &event)?;
                Ok(Ok(d))
            })
            .await?;
        outcome
    }

    /// 一項設計偏離與設計成果目前的最新版本。
    pub async fn deviation(&self, workspace_id: &str, id: &str) -> StoreResult<Option<(DesignDeviation, Option<i64>)>> {
        let (workspace_id, id) = (workspace_id.to_string(), id.to_string());
        self.read(move |conn| {
            let Some(d) = load_deviation(conn, &workspace_id, &id)? else { return Ok(None) };
            let latest = design_latest(conn, &workspace_id, &d.design_artifact_id)?;
            Ok(Some((d, latest)))
        })
        .await
    }

    /// 不涉及新設計版本的處理：送裁定（等待中）或拒絕。
    pub async fn resolve_deviation(
        &self,
        workspace_id: &str,
        id: &str,
        resolution: DeviationResolution,
        resolved_by: &str,
        now: Timestamp,
    ) -> Result<DesignDeviation, ReviewApplyError> {
        let (workspace_id, id, resolved_by) = (workspace_id.to_string(), id.to_string(), resolved_by.to_string());
        let outcome = self
            .write(move |tx| {
                let Some(d) = load_deviation(tx, &workspace_id, &id)? else {
                    return Ok(Err(ReviewError::DeviationNotFound(id).into()));
                };
                let (next, event) = match review::resolve_deviation(&d, resolution, &resolved_by, now) {
                    Ok(resolved) => resolved,
                    Err(rejected) => return Ok(Err(rejected.into())),
                };
                save_deviation_status(tx, &next)?;
                append_event(tx, &event)?;
                Ok(Ok(next))
            })
            .await?;
        outcome
    }

    /// 使用者已同意接受偏離：替設計成果追加新版本（`storage_path` 等是 API 已寫好的新檔案），偏離記為接受並記下接受者。
    /// 設計在回報之後已經有新版本時拒絕（新設計必須建立在目前的版本上）。
    #[allow(clippy::too_many_arguments)]
    pub async fn accept_deviation(
        &self,
        workspace_id: &str,
        id: &str,
        reason: String,
        decision_request_id: String,
        accepted_by: String,
        resolved_by: &str,
        blob: (String, i64, String),
        now: Timestamp,
    ) -> Result<DesignDeviation, ReviewApplyError> {
        let (workspace_id, id, resolved_by) = (workspace_id.to_string(), id.to_string(), resolved_by.to_string());
        let outcome = self
            .write(move |tx| {
                let Some(d) = load_deviation(tx, &workspace_id, &id)? else {
                    return Ok(Err(ReviewError::DeviationNotFound(id).into()));
                };
                if d.status.is_final() {
                    return Ok(Err(ReviewError::DeviationResolved(d.id).into()));
                }
                let current = design_latest(tx, &workspace_id, &d.design_artifact_id)?.unwrap_or(0);
                if current != d.design_version {
                    return Ok(Err(ReviewError::DesignOutdated { reported: d.design_version, current }.into()));
                }
                // 先由 Domain 判定（失敗時什麼都不寫），再追加設計的新版本
                let resolution = DeviationResolution {
                    status: DeviationStatus::Accepted,
                    reason,
                    decision_request_id: Some(decision_request_id),
                    accepted_by: Some(accepted_by),
                    accepted_design_version: Some(current + 1),
                };
                let (next, event) = match review::resolve_deviation(&d, resolution, &resolved_by, now) {
                    Ok(resolved) => resolved,
                    Err(rejected) => return Ok(Err(rejected.into())),
                };
                let (storage_path, size_bytes, sha256) = blob;
                let description: String = format!("接受設計偏離 {}：{}", d.id, d.deviation).chars().take(1_000).collect();
                let version = ArtifactVersion {
                    version: current + 1,
                    description,
                    size_bytes,
                    sha256,
                    storage_path,
                    produced_by_run_id: d.reported_by_run_id.clone(),
                    produced_by_generation: d.reported_generation,
                    created_at: now,
                };
                tx.execute(
                    "INSERT INTO artifact_version
                     (artifact_id, version, description, size_bytes, sha256, storage_path, run_id, generation, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    rusqlite::params![
                        d.design_artifact_id,
                        version.version,
                        version.description,
                        version.size_bytes,
                        version.sha256,
                        version.storage_path,
                        version.produced_by_run_id,
                        version.produced_by_generation,
                        now.as_millis(),
                    ],
                )?;
                let (task_id, name): (String, String) =
                    tx.query_row("SELECT task_id, name FROM artifact WHERE id = ?1", [&d.design_artifact_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
                append_event(
                    tx,
                    &DomainEvent {
                        aggregate_type: "artifact",
                        aggregate_id: d.design_artifact_id.clone(),
                        aggregate_version: version.version,
                        event_type: EVENT_ARTIFACT_REGISTERED,
                        occurred_at: now,
                        payload: serde_json::json!({
                            "workspace_id": workspace_id,
                            "task_id": task_id,
                            "name": name,
                            "version": version.version,
                            "size_bytes": version.size_bytes,
                            "sha256": version.sha256,
                            "run_id": version.produced_by_run_id,
                            "generation": version.produced_by_generation,
                            "design_deviation_id": d.id,
                        }),
                    },
                )?;
                save_deviation_status(tx, &next)?;
                append_event(tx, &event)?;
                Ok(Ok(next))
            })
            .await?;
        outcome
    }
}
