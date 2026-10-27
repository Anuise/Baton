//! 探索章程、疑點與重現紀錄的持久化（票 47）。
//!
//! - 章程只追加版本；疑點不可刪除，回報內容與確認時的重現案例不可更改（資料表 trigger 與 CHECK 另外把關）。
//! - 重現：在同一個交易中排入驗證要求（代表旅程當對照，加上案例）並記下重現紀錄。
//! - 回歸：已修復與回歸失敗的缺陷，案例在每一個受驗目錄包含修復的一般驗證要求中都會執行（排入要求時加入，
//!   不能被排除）；修復還沒整合進來的分支不執行，以免把缺陷改回 regressed（R151 第 14 項）。
//! - 執行前（`augment_pending`）把要求中的案例加入 Profile 的旅程，診斷用的重現套用章程的步數與成本上限；
//!   回歸案例與目前的 Profile 不相容時，以無法自動化的替代旅程執行，讓驗證不能通過、也不會誤判為產品缺陷。
//! - Evidence Bundle 寫入的同一個交易中（`judge_bundle`）判定每一個重現紀錄並更新疑點。

use baton_domain::auth::AgentScope;
use baton_domain::exploration::{
    self, AttemptOutcome, AttemptPurpose, Charter, CharterInput, ExplorationError, ExplorationFinding, FindingReport,
    FindingStatus, ReproductionAttempt, ReproductionCase, TriageAction,
};
use baton_domain::run;
use baton_domain::verification_profile::{EvidenceBundle, Journey, ProfileError, RequestStatus, Verdict, VerificationRequest};
use rusqlite::OptionalExtension;

use super::tasks::load;
use super::verification_profiles::{insert_request, latest_profile, PendingVerificationRun};
use super::*;

/// 套用探索相關變更的失敗。
#[derive(Debug, thiserror::Error)]
pub enum ExplorationApplyError {
    #[error(transparent)]
    Rejected(#[from] ExplorationError),
    #[error(transparent)]
    Profile(#[from] ProfileError),
    #[error("找不到 Workspace {0}")]
    WorkspaceNotFound(String),
    #[error("Workspace「{0}」已經封存")]
    WorkspaceArchived(String),
    #[error("找不到 Task {0}")]
    TaskNotFound(String),
    #[error("這個 Run 已不是目前的寫入所有者")]
    Stale,
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl From<rusqlite::Error> for ExplorationApplyError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error.into())
    }
}

/// 一個疑點與它的重現紀錄（新的在前）。
#[derive(Debug, Clone, PartialEq)]
pub struct FindingRecord {
    pub finding: ExplorationFinding,
    pub task_title: Option<String>,
    pub attempts: Vec<ReproductionAttempt>,
}

/// 一個 Workspace 的探索：各探索 Task 目前的章程（附 Task 名稱）與全部疑點，新的在前。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ExplorationData {
    pub charters: Vec<(Charter, Option<String>)>,
    pub findings: Vec<FindingRecord>,
}

const FINDING_COLUMNS: &str = "id, workspace_id, task_id, charter_id, report, status, exclusion, unverified_reason, \
     reproduction_case, reported_by, version, created_at, updated_at";
const ATTEMPT_COLUMNS: &str = "finding_id, request_id, purpose, journey, bundle_id, verdict, reason_code, control_verdict, \
     outcome, requested_by, created_at, finished_at";

fn unknown(index: usize, what: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Text, format!("未知的{what}").into())
}

fn optional_json<T: serde::de::DeserializeOwned>(index: usize, raw: Option<String>) -> rusqlite::Result<Option<T>> {
    raw.map(|raw| json_column(index, &raw)).transpose()
}

fn finding_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<(ExplorationFinding, String)> {
    let status: String = r.get(5)?;
    let finding = ExplorationFinding {
        id: r.get(0)?,
        workspace_id: r.get(1)?,
        task_id: r.get(2)?,
        charter_id: r.get(3)?,
        charter_version: 0,
        report: json_column(4, &r.get::<_, String>(4)?)?,
        status: FindingStatus::parse(&status).ok_or_else(|| unknown(5, "疑點狀態"))?,
        exclusion: optional_json(6, r.get(6)?)?,
        unverified_reason: r.get(7)?,
        case: optional_json(8, r.get(8)?)?,
        reported_by: r.get(9)?,
        version: r.get(10)?,
        created_at: Timestamp::from_millis(r.get(11)?),
        updated_at: Timestamp::from_millis(r.get(12)?),
    };
    Ok((finding, r.get(3)?))
}

fn attempt_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ReproductionAttempt> {
    let purpose: String = r.get(2)?;
    let verdict = |index: usize, raw: Option<String>| -> rusqlite::Result<Option<Verdict>> {
        raw.map(|v| Verdict::parse(&v).ok_or_else(|| unknown(index, "判定"))).transpose()
    };
    let outcome: Option<String> = r.get(8)?;
    Ok(ReproductionAttempt {
        finding_id: r.get(0)?,
        request_id: r.get(1)?,
        purpose: AttemptPurpose::parse(&purpose).ok_or_else(|| unknown(2, "重現目的"))?,
        journey: json_column(3, &r.get::<_, String>(3)?)?,
        bundle_id: r.get(4)?,
        verdict: verdict(5, r.get(5)?)?,
        reason_code: r.get(6)?,
        control_verdict: verdict(7, r.get(7)?)?,
        outcome: outcome.map(|o| AttemptOutcome::parse(&o).ok_or_else(|| unknown(8, "重現結論"))).transpose()?,
        requested_by: r.get(9)?,
        created_at: Timestamp::from_millis(r.get(10)?),
        finished_at: r.get::<_, Option<i64>>(11)?.map(Timestamp::from_millis),
    })
}

fn charter_by_id(conn: &Connection, id: &str) -> rusqlite::Result<Option<Charter>> {
    conn.query_row("SELECT content FROM exploration_charter WHERE id = ?1", [id], |r| json_column(0, &r.get::<_, String>(0)?))
        .optional()
}

pub(crate) fn latest_charter(conn: &Connection, task_id: &str) -> rusqlite::Result<Option<Charter>> {
    conn.query_row(
        "SELECT content FROM exploration_charter WHERE task_id = ?1 ORDER BY version DESC LIMIT 1",
        [task_id],
        |r| json_column(0, &r.get::<_, String>(0)?),
    )
    .optional()
}

fn load_finding(conn: &Connection, workspace_id: &str, finding_id: &str) -> rusqlite::Result<Option<ExplorationFinding>> {
    let found = conn
        .query_row(
            &format!("SELECT {FINDING_COLUMNS} FROM exploration_finding WHERE id = ?1 AND workspace_id = ?2"),
            [finding_id, workspace_id],
            finding_row,
        )
        .optional()?;
    let Some((mut finding, charter_id)) = found else { return Ok(None) };
    finding.charter_version = charter_by_id(conn, &charter_id)?.map_or(0, |c| c.version);
    Ok(Some(finding))
}

fn load_attempts(conn: &Connection, condition: &str, param: &str) -> rusqlite::Result<Vec<ReproductionAttempt>> {
    conn.prepare(&format!("SELECT {ATTEMPT_COLUMNS} FROM exploration_attempt WHERE {condition}"))?
        .query_map([param], attempt_row)?
        .collect()
}

fn task_title(conn: &Connection, task_id: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row("SELECT title FROM task WHERE id = ?1", [task_id], |r| r.get(0)).optional()
}

fn record(conn: &Connection, finding: ExplorationFinding) -> rusqlite::Result<FindingRecord> {
    let attempts = load_attempts(conn, "finding_id = ?1 ORDER BY created_at DESC, rowid DESC", &finding.id)?;
    let task_title = task_title(conn, &finding.task_id)?;
    Ok(FindingRecord { finding, task_title, attempts })
}

/// 疑點是否有排隊中（還沒有結果）的重現或修復驗證。
fn reproduction_pending(conn: &Connection, finding_id: &str) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT 1 FROM exploration_attempt WHERE finding_id = ?1 AND outcome IS NULL
             AND purpose IN ('reproduce', 'verify_fix') LIMIT 1",
        [finding_id],
        |r| r.get::<_, i64>(0),
    )
    .optional()
    .map(|found| found.is_some())
}

/// 確認 Workspace 存在而且沒有封存。
fn active_workspace(conn: &Connection, workspace_id: &str) -> Result<(), ExplorationApplyError> {
    let found: Option<(String, String)> = conn
        .query_row("SELECT name, status FROM workspace WHERE id = ?1", [workspace_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?;
    match found {
        None => Err(ExplorationApplyError::WorkspaceNotFound(workspace_id.into())),
        Some((name, status)) if status == "archived" => Err(ExplorationApplyError::WorkspaceArchived(name)),
        Some(_) => Ok(()),
    }
}

fn task_in_workspace(conn: &Connection, workspace_id: &str, task_id: &str) -> rusqlite::Result<bool> {
    Ok(conn
        .query_row("SELECT 1 FROM task WHERE id = ?1 AND workspace_id = ?2", [task_id, workspace_id], |r| r.get::<_, i64>(0))
        .optional()?
        .is_some())
}

fn update_finding(tx: &Transaction<'_>, finding: &ExplorationFinding) -> rusqlite::Result<()> {
    tx.execute(
        "UPDATE exploration_finding SET status = ?2, exclusion = ?3, unverified_reason = ?4, reproduction_case = ?5,
             version = ?6, updated_at = ?7
         WHERE id = ?1",
        rusqlite::params![
            finding.id,
            finding.status.as_str(),
            finding.exclusion.as_ref().map(|e| serde_json::to_string(e).expect("排除原因一定可以序列化")),
            finding.unverified_reason,
            finding.case.as_ref().map(|c| serde_json::to_string(c).expect("重現案例一定可以序列化")),
            finding.version,
            finding.updated_at.as_millis(),
        ],
    )?;
    Ok(())
}

fn insert_attempt(tx: &Transaction<'_>, attempt: &ReproductionAttempt) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO exploration_attempt (request_id, finding_id, purpose, journey, requested_by, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![
            attempt.request_id,
            attempt.finding_id,
            attempt.purpose.as_str(),
            serde_json::to_string(&attempt.journey).expect("旅程一定可以序列化"),
            attempt.requested_by,
            attempt.created_at.as_millis(),
        ],
    )?;
    Ok(())
}

/// 證據識別必須是這個 Workspace 已登記的成果或 Evidence Bundle。
fn evidence_exists(conn: &Connection, workspace_id: &str, id: &str) -> rusqlite::Result<bool> {
    let found: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM artifact WHERE id = ?1 AND workspace_id = ?2
             UNION ALL SELECT 1 FROM evidence_bundle WHERE id = ?1 AND workspace_id = ?2 LIMIT 1",
            [id, workspace_id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(found.is_some())
}

/// 在一般驗證要求中加入回歸案例（已修復與回歸失敗的缺陷）：先把案例識別加進要求的旅程，寫入要求後再記下重現紀錄。
/// 使用者或 Controller 指定的旅程中若含案例識別，一律先去掉（案例由這裡加入，不能被指定或排除）。
/// 只加入受驗目錄包含修復的案例（[`subject_has_fix`]）：修復還沒整合時，其他分支的驗證不能把缺陷改回 regressed。
pub(crate) fn insert_request_with_regressions(tx: &Transaction<'_>, request: &mut VerificationRequest) -> rusqlite::Result<()> {
    let cases: Vec<(String, String)> = tx
        .prepare(
            "SELECT id, reproduction_case FROM exploration_finding
             WHERE workspace_id = ?1 AND status IN ('fixed', 'regressed') ORDER BY created_at, rowid",
        )?
        .query_map([&request.workspace_id], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let subject = request_subject(tx, &request.workspace_id, request.task_id.as_deref())?;
    let mut attempts = Vec::with_capacity(cases.len());
    for (finding_id, raw) in cases {
        if !subject_has_fix(tx, &finding_id, &subject)? {
            continue;
        }
        let case: ReproductionCase = json_column(1, &raw)?;
        request.journeys.push(exploration::case_key(&finding_id));
        attempts.push(ReproductionAttempt {
            finding_id,
            request_id: request.id.clone(),
            purpose: AttemptPurpose::Regression,
            journey: case.journey,
            bundle_id: None,
            verdict: None,
            reason_code: None,
            control_verdict: None,
            outcome: None,
            requested_by: request.requested_by.clone(),
            created_at: request.created_at,
            finished_at: None,
        });
    }
    insert_request(tx, request)?;
    for attempt in &attempts {
        insert_attempt(tx, attempt)?;
    }
    Ok(())
}

/// 驗證要求的受驗目錄（與 `pending_verification_runs` 相同：Task 已登記的工作目錄，否則是 Workspace 目錄），
/// 以及它若是平台的 worktree，第一次建立的時間（共用的 checkout 與輸出目錄為 `None`）。
fn request_subject(conn: &Connection, workspace_id: &str, task_id: Option<&str>) -> rusqlite::Result<(String, Option<i64>)> {
    if let Some(task_id) = task_id {
        let workdir: Option<(String, String, Option<i64>)> = conn
            .query_row(
                "SELECT path, kind, ready_at FROM task_workdir WHERE task_id = ?1 AND status = 'ready'",
                [task_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        if let Some((path, kind, ready_at)) = workdir {
            return Ok((path, (kind == "worktree").then(|| ready_at.unwrap_or(0))));
        }
    }
    Ok((conn.query_row("SELECT path FROM workspace WHERE id = ?1", [workspace_id], |r| r.get(0))?, None))
}

/// 受驗目錄是否包含缺陷最近一次通過修復驗證的修復（R151 第 14 項）。Store 無法查 Git 的祖先關係，改以目錄判斷：
/// - 就是修復驗證時的受驗目錄：包含（之後的提交都在修復之上）。
/// - 修復在平台 worktree 中驗證：那個 Task 的分支整合進目標分支之後，共用的 checkout 與之後才建立的 worktree 包含。
/// - 修復在共用的 checkout（或輸出目錄）中驗證：共用的 checkout 與修復之後才建立的 worktree 包含。
/// - 沒有修復驗證的紀錄（不應發生）：照舊執行。
fn subject_has_fix(conn: &Connection, finding_id: &str, subject: &(String, Option<i64>)) -> rusqlite::Result<bool> {
    let fix: Option<(Option<String>, i64)> = conn
        .query_row(
            "SELECT json_extract(b.content, '$.subject_dir'), b.created_at
             FROM exploration_attempt a JOIN evidence_bundle b ON b.id = a.bundle_id
             WHERE a.finding_id = ?1 AND a.outcome = 'fix_verified' ORDER BY a.finished_at DESC, a.rowid DESC LIMIT 1",
            [finding_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((Some(fix_dir), fixed_at)) = fix else { return Ok(true) };
    let (dir, worktree_ready_at) = subject;
    if dir.eq_ignore_ascii_case(&fix_dir) {
        return Ok(true);
    }
    let fix_task: Option<String> = conn
        .query_row("SELECT task_id FROM task_workdir WHERE path = ?1 COLLATE NOCASE AND kind = 'worktree'", [&fix_dir], |r| {
            r.get(0)
        })
        .optional()?;
    let shared_since: Option<i64> = match fix_task {
        Some(task_id) => conn.query_row(
            "SELECT MIN(finished_at) FROM integration_entry WHERE task_id = ?1 AND status = 'integrated' AND finished_at >= ?2",
            rusqlite::params![task_id, fixed_at],
            |r| r.get(0),
        )?,
        None => Some(fixed_at),
    };
    Ok(shared_since.is_some_and(|since| worktree_ready_at.is_none_or(|ready_at| ready_at >= since)))
}

/// 去掉旅程清單中的案例識別（案例只能由 Runtime 加入）。
pub(crate) fn without_cases(journeys: Vec<String>) -> Vec<String> {
    journeys.into_iter().filter(|j| exploration::finding_of_case(j).is_none()).collect()
}

/// 這個驗證要求是不是診斷用的重現（重現或修復驗證）：它的 Bundle 不能作為 Gate 的驗證證據。
pub(crate) fn is_diagnostic_request(conn: &Connection, request_id: &str) -> rusqlite::Result<bool> {
    let found: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM exploration_attempt WHERE request_id = ?1 AND purpose IN ('reproduce', 'verify_fix') LIMIT 1",
            [request_id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(found.is_some())
}

/// 執行前把要求中的案例加入 Profile 的旅程（有對應功能時也加進 Feature Map，維持需求到測試的追蹤）。
/// 診斷用的重現套用章程的步數與成本上限（取與 Profile 預算較小者）。
pub(crate) fn augment_pending(conn: &Connection, pending: &mut PendingVerificationRun) -> rusqlite::Result<()> {
    let attempts = load_attempts(conn, "request_id = ?1 ORDER BY rowid", &pending.request.id)?;
    for attempt in attempts {
        let definition = &pending.profile.definition;
        let journey = match exploration::check_case(definition, &attempt.journey) {
            Ok(()) => attempt.journey.clone(),
            Err(problem) => exploration::incompatible_case(&attempt.finding_id, &attempt.journey.title, &problem.to_string()),
        };
        let finding: Option<(String, Option<String>)> = conn
            .query_row(
                "SELECT task_id, json_extract(report, '$.feature') FROM exploration_finding WHERE id = ?1",
                [&attempt.finding_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let definition = &mut pending.profile.definition;
        if let Some(feature) = finding.as_ref().and_then(|(_, f)| f.as_deref()) {
            if let Some(entry) = definition.feature_map.iter_mut().find(|f| f.key == feature) {
                entry.journeys.push(journey.key.clone());
            }
        }
        definition.journeys.push(journey);
        if attempt.purpose.is_diagnostic() {
            if let Some(charter) = finding.as_ref().map(|(task, _)| latest_charter(conn, task)).transpose()?.flatten() {
                let budget = &mut definition.budget;
                budget.max_operations = budget.max_operations.min(charter.max_steps);
                budget.max_model_calls = budget.max_model_calls.min(charter.max_model_calls);
            }
        }
    }
    Ok(())
}

/// Evidence Bundle 寫入的同一個交易中：判定這個要求的每一個重現紀錄，補上結果並更新疑點。
pub(crate) fn judge_bundle(tx: &Transaction<'_>, bundle: &EvidenceBundle) -> rusqlite::Result<()> {
    let attempts = load_attempts(tx, "request_id = ?1 AND outcome IS NULL ORDER BY rowid", &bundle.request_id)?;
    for mut attempt in attempts {
        let Some(finding) = load_finding(tx, &bundle.workspace_id, &attempt.finding_id)? else { continue };
        let judged = exploration::judge_attempt(&finding, attempt.purpose, bundle);
        attempt.bundle_id = Some(bundle.id.clone());
        attempt.verdict = judged.verdict;
        attempt.reason_code = judged.reason_code.clone();
        attempt.control_verdict = judged.control_verdict;
        attempt.outcome = Some(judged.outcome);
        attempt.finished_at = Some(bundle.finished_at);
        tx.execute(
            "UPDATE exploration_attempt SET bundle_id = ?3, verdict = ?4, reason_code = ?5, control_verdict = ?6, outcome = ?7,
                 finished_at = ?8
             WHERE request_id = ?1 AND finding_id = ?2",
            rusqlite::params![
                attempt.request_id,
                attempt.finding_id,
                bundle.id,
                judged.verdict.map(Verdict::as_str),
                judged.reason_code,
                judged.control_verdict.map(Verdict::as_str),
                judged.outcome.as_str(),
                bundle.finished_at.as_millis(),
            ],
        )?;
        append_event(tx, &exploration::judged_event(&finding, &attempt, &judged, bundle.finished_at))?;
        // 已經不是排入時的狀態（例如回歸期間被重新確認）就只記結果，不改狀態
        let expected = match attempt.purpose {
            AttemptPurpose::Reproduce => matches!(finding.status, FindingStatus::Suspected | FindingStatus::UnverifiedRisk),
            AttemptPurpose::VerifyFix => matches!(finding.status, FindingStatus::Confirmed | FindingStatus::Regressed),
            AttemptPurpose::Regression => finding.status.in_regression(),
        };
        if let Some(change) = exploration::change_for(&judged, &attempt, bundle).filter(|_| expected) {
            let (updated, event) = exploration::apply(&finding, change, attempt.purpose.as_str(), bundle.finished_at);
            update_finding(tx, &updated)?;
            append_event(tx, &event)?;
        }
    }
    Ok(())
}

impl Store {
    /// 定義（或改版）探索章程；內容相同時不追加。回傳 (目前的章程, 是否新增)。
    pub async fn define_exploration_charter(
        &self,
        workspace_id: &str,
        input: CharterInput,
        defined_by: String,
        id: String,
        now: Timestamp,
    ) -> Result<(Charter, bool), ExplorationApplyError> {
        let workspace_id = workspace_id.to_string();
        let outcome = self
            .write(move |tx| {
                if let Err(error) = active_workspace(tx, &workspace_id) {
                    return Ok(Err(error));
                }
                let task_id = input.task_id.trim().to_string();
                if !task_in_workspace(tx, &workspace_id, &task_id)? {
                    return Ok(Err(ExplorationApplyError::TaskNotFound(task_id)));
                }
                let current = latest_charter(tx, &task_id)?;
                match exploration::define_charter(current.as_ref(), &workspace_id, input, defined_by, id, now) {
                    Err(rejected) => Ok(Err(rejected.into())),
                    Ok(None) => Ok(Ok((current.expect("內容相同表示已有章程"), false))),
                    Ok(Some((charter, event))) => {
                        tx.execute(
                            "INSERT INTO exploration_charter (id, workspace_id, task_id, version, content, created_at)
                             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                            rusqlite::params![
                                charter.id,
                                charter.workspace_id,
                                charter.task_id,
                                charter.version,
                                serde_json::to_string(&charter).expect("章程一定可以序列化"),
                                charter.created_at.as_millis(),
                            ],
                        )?;
                        append_event(tx, &event)?;
                        Ok(Ok((charter, true)))
                    }
                }
            })
            .await?;
        outcome
    }

    /// Worker 在自己 Task 的章程下回報疑點（一律先是 suspected）。對應的功能必須在目前 Profile 的 Feature Map 中，
    /// 證據必須是這個 Workspace 已登記的成果或 Evidence Bundle。
    pub async fn report_exploration_finding(
        &self,
        scope: &AgentScope,
        input: FindingReport,
        id: String,
        now: Timestamp,
    ) -> Result<FindingRecord, ExplorationApplyError> {
        let scope = scope.clone();
        let outcome = self
            .write(move |tx| {
                let loaded = load(tx, &scope.workspace_id)?;
                let Some(run) = loaded.runs.iter().find(|r| r.id == scope.run_id) else {
                    return Ok(Err(ExplorationApplyError::Stale));
                };
                let lease = loaded.leases.iter().find(|l| l.run_id == run.id);
                if !run::is_current_writer(run, lease, scope.generation) {
                    return Ok(Err(ExplorationApplyError::Stale));
                }
                let charter = latest_charter(tx, &run.task_id)?;
                let (finding, event) =
                    match exploration::report(charter.as_ref(), &scope.workspace_id, &run.task_id, input, run.id.clone(), id, now) {
                        Ok(reported) => reported,
                        Err(rejected) => return Ok(Err(rejected.into())),
                    };
                if let Some(feature) = &finding.report.feature {
                    let known = latest_profile(tx, &scope.workspace_id)?
                        .is_some_and(|p| p.definition.feature_map.iter().any(|f| &f.key == feature));
                    if !known {
                        return Ok(Err(ExplorationError::Invalid(format!(
                            "功能「{feature}」不在目前 Verification Profile 的 Feature Map 中；不確定時省略 feature"
                        ))
                        .into()));
                    }
                }
                for evidence in &finding.report.evidence {
                    if !evidence_exists(tx, &scope.workspace_id, evidence)? {
                        return Ok(Err(ExplorationError::Invalid(format!(
                            "證據「{evidence}」不是這個 Workspace 已登記的成果或 Evidence Bundle；畫面與 trace 請先以 register_artifact 登記"
                        ))
                        .into()));
                    }
                }
                tx.execute(
                    &format!("INSERT INTO exploration_finding ({FINDING_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, NULL, NULL, ?7, ?8, ?9, ?9)"),
                    rusqlite::params![
                        finding.id,
                        finding.workspace_id,
                        finding.task_id,
                        finding.charter_id,
                        serde_json::to_string(&finding.report).expect("疑點內容一定可以序列化"),
                        finding.status.as_str(),
                        finding.reported_by,
                        finding.version,
                        finding.created_at.as_millis(),
                    ],
                )?;
                append_event(tx, &event)?;
                Ok(Ok(record(tx, finding)?))
            })
            .await?;
        outcome
    }

    /// Controller 的分流；`approved_decision` 是使用者核准「排除為預期設計」的裁定請求。
    pub async fn triage_exploration_finding(
        &self,
        workspace_id: &str,
        finding_id: &str,
        action: TriageAction,
        approved_decision: Option<String>,
        now: Timestamp,
    ) -> Result<FindingRecord, ExplorationApplyError> {
        let (workspace_id, finding_id) = (workspace_id.to_string(), finding_id.to_string());
        let outcome = self
            .write(move |tx| {
                let Some(finding) = load_finding(tx, &workspace_id, &finding_id)? else {
                    return Ok(Err(ExplorationError::FindingNotFound(finding_id).into()));
                };
                // 重現排隊中不能分流：否則結果證實是缺陷時，疑點已被排除而不會升為已確認
                if reproduction_pending(tx, &finding.id)? {
                    return Ok(Err(ExplorationError::ReproductionPending(finding.id).into()));
                }
                let change = match exploration::triage(&finding, action, approved_decision) {
                    Ok(change) => change,
                    Err(rejected) => return Ok(Err(rejected.into())),
                };
                let (updated, event) = exploration::apply(&finding, change, "triaged", now);
                update_finding(tx, &updated)?;
                append_event(tx, &event)?;
                Ok(Ok(record(tx, updated)?))
            })
            .await?;
        outcome
    }

    /// 要求重現或修復驗證：依疑點的狀態決定目的與案例，以目前的 Profile 排入驗證要求（代表旅程當對照，加上案例），
    /// 並記下重現紀錄。`task_id` 指定在哪個 Task 的工作目錄中執行（預設是回報疑點的 Task）。
    #[allow(clippy::too_many_arguments)]
    pub async fn reproduce_exploration_finding(
        &self,
        workspace_id: &str,
        finding_id: &str,
        journey: Option<Journey>,
        task_id: Option<String>,
        requested_by: String,
        request_id: String,
        now: Timestamp,
    ) -> Result<(FindingRecord, ReproductionAttempt, VerificationRequest), ExplorationApplyError> {
        let (workspace_id, finding_id) = (workspace_id.to_string(), finding_id.to_string());
        let outcome = self
            .write(move |tx| {
                if let Err(error) = active_workspace(tx, &workspace_id) {
                    return Ok(Err(error));
                }
                let Some(finding) = load_finding(tx, &workspace_id, &finding_id)? else {
                    return Ok(Err(ExplorationError::FindingNotFound(finding_id).into()));
                };
                if reproduction_pending(tx, &finding.id)? {
                    return Ok(Err(ExplorationError::ReproductionPending(finding.id).into()));
                }
                let Some(profile) = latest_profile(tx, &workspace_id)? else {
                    return Ok(Err(ProfileError::NotFound.into()));
                };
                let planned = match exploration::plan_reproduction(&finding, journey.as_ref(), &profile.definition) {
                    Ok(planned) => planned,
                    Err(rejected) => return Ok(Err(rejected.into())),
                };
                let subject = task_id.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).unwrap_or(finding.task_id.clone());
                if !task_in_workspace(tx, &workspace_id, &subject)? {
                    return Ok(Err(ExplorationApplyError::TaskNotFound(subject)));
                }
                let request = VerificationRequest {
                    id: request_id,
                    workspace_id: workspace_id.clone(),
                    task_id: Some(subject),
                    profile_version_id: profile.id.clone(),
                    profile_version: profile.version,
                    journeys: exploration::reproduction_journeys(&profile.definition, &finding.id),
                    requested_by: requested_by.clone(),
                    status: RequestStatus::Queued,
                    bundle_id: None,
                    created_at: now,
                    no_live_model: false,
                };
                insert_request(tx, &request)?;
                let attempt = ReproductionAttempt {
                    finding_id: finding.id.clone(),
                    request_id: request.id.clone(),
                    purpose: planned.purpose,
                    journey: planned.journey,
                    bundle_id: None,
                    verdict: None,
                    reason_code: None,
                    control_verdict: None,
                    outcome: None,
                    requested_by,
                    created_at: now,
                    finished_at: None,
                };
                insert_attempt(tx, &attempt)?;
                append_event(tx, &exploration::requested_event(&finding, &attempt))?;
                Ok(Ok((record(tx, finding)?, attempt, request)))
            })
            .await?;
        outcome
    }

    /// 一個疑點與它的重現紀錄。
    pub async fn exploration_finding(&self, workspace_id: &str, finding_id: &str) -> StoreResult<Option<FindingRecord>> {
        let (workspace_id, finding_id) = (workspace_id.to_string(), finding_id.to_string());
        self.read(move |conn| Ok(load_finding(conn, &workspace_id, &finding_id)?.map(|f| record(conn, f)).transpose()?)).await
    }

    /// Workspace 的探索：各探索 Task 目前的章程與全部疑點（新的在前）。
    pub async fn exploration(&self, workspace_id: &str) -> StoreResult<ExplorationData> {
        let workspace_id = workspace_id.to_string();
        self.read(move |conn| {
            let charters: Vec<Charter> = conn
                .prepare(
                    "SELECT c.content FROM exploration_charter c
                     WHERE c.workspace_id = ?1
                       AND c.version = (SELECT MAX(version) FROM exploration_charter WHERE task_id = c.task_id)
                     ORDER BY c.created_at DESC, c.rowid DESC",
                )?
                .query_map([&workspace_id], |r| json_column(0, &r.get::<_, String>(0)?))?
                .collect::<rusqlite::Result<_>>()?;
            let charters = charters
                .into_iter()
                .map(|c| {
                    let title = task_title(conn, &c.task_id)?;
                    Ok((c, title))
                })
                .collect::<rusqlite::Result<_>>()?;
            let ids: Vec<String> = conn
                .prepare("SELECT id FROM exploration_finding WHERE workspace_id = ?1 ORDER BY created_at DESC, rowid DESC")?
                .query_map([&workspace_id], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            let mut findings = Vec::with_capacity(ids.len());
            for id in ids {
                if let Some(finding) = load_finding(conn, &workspace_id, &id)? {
                    findings.push(record(conn, finding)?);
                }
            }
            Ok(ExplorationData { charters, findings })
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use baton_domain::task::{ContractContent, ContractField, TaskCommand};
    use baton_domain::workdir::TaskKind;
    use baton_domain::workspace::WorkspaceCommand;

    use super::*;

    const T1: Timestamp = Timestamp::from_millis(1_000);
    const WORKSPACE_DIR: &str = "C:\\work\\app";

    fn filled(text: &str) -> Option<ContractField> {
        Some(ContractField::Filled { text: text.into(), references: vec![] })
    }

    fn worktree(task: &str) -> String {
        format!("{WORKSPACE_DIR}\\.baton\\worktrees\\{task}")
    }

    /// 一個 Workspace、三個開發 Task（fix 修復缺陷，old 在修復整合前、late 在修復整合後建立 worktree），
    /// 一份 Profile，以及一個在 fix 的 worktree 中驗證修復通過的缺陷 xf_1。
    async fn store_with_fixed_defect(dir: &std::path::Path) -> Store {
        let store = Store::open(dir, T1).await.unwrap();
        let create = WorkspaceCommand::Create { id: "w1".into(), name: None, path: WORKSPACE_DIR.into() };
        store.apply_workspace_command(create, T1).await.unwrap();
        store.apply_task_command("w1".into(), TaskCommand::CreateGoal { id: "g1".into(), title: "目標".into() }, T1).await.unwrap();
        for task in ["fix", "old", "late"] {
            let contract = ContractContent {
                objective: filled("修正"),
                inputs: Some(ContractField::NotApplicable { reason: "沒有輸入".into() }),
                scope: filled("結帳"),
                deliverables: filled("修正"),
                done_criteria: filled("通過"),
                blocker_handling: filled("回報"),
                required_capabilities: vec![],
                verification_checks: vec![],
            };
            let command = TaskCommand::CreateTask {
                id: task.into(),
                goal_id: "g1".into(),
                title: task.into(),
                kind: TaskKind::Development,
                contract,
            };
            store.apply_task_command("w1".into(), command, T1).await.unwrap();
        }
        let profile = serde_json::json!({
            "interface": "cli_api",
            "journeys": [{ "key": "browse", "title": "瀏覽", "representative": true, "steps": [{
                "key": "list",
                "operation": { "kind": "http", "method": "GET", "url": "http://127.0.0.1:8080/items" },
                "assertions": [{ "kind": "status", "equals": 200 }],
            }] }],
            "feature_map": [{ "key": "items", "title": "項目", "criterion": "可以列出項目", "journeys": ["browse"] }],
        });
        let profile = serde_json::from_value(profile).unwrap();
        store.publish_verification_profile("w1", profile, "desk".into(), T1).await.unwrap();

        let case = ReproductionCase {
            journey: serde_json::from_value(serde_json::json!({ "key": "finding.xf_1", "title": "案例", "steps": [] })).unwrap(),
            request_id: "vreq_confirm".into(),
            bundle_id: "b_confirm".into(),
            failed_step: None,
            confirmed_at: T1,
        };
        let conn = Connection::open(dir.join(DB_FILE_NAME)).unwrap();
        conn.execute_batch("PRAGMA foreign_keys = OFF;").unwrap();
        for (task, ready_at) in [("fix", 100), ("old", 50), ("late", 500)] {
            conn.execute(
                "INSERT INTO task_workdir (task_id, workspace_id, kind, path, branch, base_branch, status, version, attempted_at, ready_at)
                 VALUES (?1, 'w1', 'worktree', ?2, 'baton/' || ?1, 'main', 'ready', 1, ?3, ?3)",
                rusqlite::params![task, worktree(task), ready_at],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO exploration_finding (id, workspace_id, task_id, charter_id, report, status, reproduction_case,
               reported_by, version, created_at, updated_at)
             VALUES ('xf_1', 'w1', 'fix', 'xch', '{}', 'fixed', ?1, 'run', 3, 1, 200)",
            [serde_json::to_string(&case).unwrap()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO evidence_bundle (id, workspace_id, request_id, task_id, profile_version_id, verdict, content, created_at)
             VALUES ('b_fix', 'w1', 'vreq_fix', 'fix', 'pv', 'passed', json_object('subject_dir', ?1), 200)",
            [worktree("fix")],
        )
        .unwrap();
        conn.execute_batch(
            "INSERT INTO exploration_attempt (request_id, finding_id, purpose, journey, requested_by, created_at, bundle_id,
               verdict, outcome, finished_at)
             VALUES ('vreq_fix', 'xf_1', 'verify_fix', '{}', 'controller', 150, 'b_fix', 'passed', 'fix_verified', 200);",
        )
        .unwrap();
        store
    }

    async fn runs_regression(store: &Store, task: Option<&str>, id: &str) -> bool {
        let request = store
            .request_verification("w1", task.map(str::to_string), Vec::new(), false, "desk".into(), id.into(), T1)
            .await
            .unwrap();
        request.journeys.contains(&exploration::case_key("xf_1"))
    }

    /// R151 第 14 項：修復還沒整合時，其他分支的驗證不執行回歸案例（不會把缺陷改回 regressed）；
    /// 修復所在的 worktree 一直執行，整合之後共用的 checkout 與之後建立的 worktree 也執行。
    #[tokio::test]
    async fn regression_cases_only_run_on_subjects_that_contain_the_fix() {
        let dir = tempfile::tempdir().unwrap();
        let store = store_with_fixed_defect(dir.path()).await;

        assert!(runs_regression(&store, Some("fix"), "v1").await, "修復所在的 worktree 要執行回歸");
        assert!(!runs_regression(&store, Some("old"), "v2").await, "修復整合前建立的分支不含修復");
        assert!(!runs_regression(&store, Some("late"), "v3").await, "修復還沒整合");
        assert!(!runs_regression(&store, None, "v4").await, "修復還沒整合進共用的 checkout");

        let conn = Connection::open(dir.path().join(DB_FILE_NAME)).unwrap();
        conn.execute_batch(
            "PRAGMA foreign_keys = OFF;
             INSERT INTO integration_entry (id, workspace_id, task_id, run_id, generation, branch, target_branch,
               status, version, checks, queued_at, finished_at)
             VALUES ('int_fix', 'w1', 'fix', 'r_fix', 1, 'baton/fix', 'main', 'integrated', 5, '[]', 250, 300);",
        )
        .unwrap();

        assert!(runs_regression(&store, None, "v5").await, "整合後共用的 checkout 含修復");
        assert!(runs_regression(&store, Some("late"), "v6").await, "整合後才建立的 worktree 含修復");
        assert!(!runs_regression(&store, Some("old"), "v7").await, "整合前建立的分支仍不含修復");
        assert!(runs_regression(&store, Some("fix"), "v8").await);
    }
}
