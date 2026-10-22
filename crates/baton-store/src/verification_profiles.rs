//! Verification Profile、驗證要求與 Evidence Bundle 的持久化（票 45）。
//!
//! - Profile 版本只追加（資料表以 trigger 禁止更新與刪除），識別是 `vp_<Workspace>_v<版本>`。
//! - 驗證要求在建立時固定 Profile 版本與要執行的旅程（版本還沒被證明可用時一定包含代表旅程）。
//! - Runtime 執行完一個要求時，在同一個交易中寫入 Evidence Bundle、把要求標為完成並追加事件；
//!   要求已經完成（例如重複提交）時什麼都不寫。

use baton_domain::verification_profile::{
    self, EvidenceBundle, ProfileDefinition, ProfileError, ProfileVersion, RequestStatus, Verdict, VerificationRequest,
};
use rusqlite::OptionalExtension;

use super::*;

/// 套用驗證相關變更的失敗：Domain 拒絕、找不到 Workspace／Task，或儲存層錯誤。
#[derive(Debug, thiserror::Error)]
pub enum VerificationApplyError {
    #[error(transparent)]
    Rejected(#[from] ProfileError),
    #[error("找不到 Workspace {0}")]
    WorkspaceNotFound(String),
    #[error("Workspace「{0}」已經封存")]
    WorkspaceArchived(String),
    #[error("找不到 Task {0}")]
    TaskNotFound(String),
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl From<rusqlite::Error> for VerificationApplyError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error.into())
    }
}

/// 一個 Workspace 的驗證概況。
#[derive(Debug, Clone, PartialEq)]
pub struct VerificationOverview {
    /// 最新的 Profile 版本。
    pub profile: Option<ProfileVersion>,
    /// 證明最新版本可用的 Evidence Bundle（還沒被證明時為 `None`）。
    pub proven_by: Option<String>,
    /// 最近的驗證要求（新的在前）。
    pub requests: Vec<VerificationRequest>,
    /// 最近的 Evidence Bundle（新的在前）。
    pub bundles: Vec<EvidenceBundle>,
}

/// 等待 Runtime 執行的驗證要求，附上執行需要的事實。
#[derive(Debug, Clone, PartialEq)]
pub struct PendingVerificationRun {
    pub request: VerificationRequest,
    pub profile: ProfileVersion,
    /// 受驗的目錄：Task 已登記的工作目錄，否則是 Workspace 目錄。
    pub subject_dir: String,
    /// 受驗 Task 目前的契約版本（需求版本）。
    pub requirement_version: Option<i64>,
    /// 受驗 Task 最新的 Run 與世代。
    pub run: Option<(String, i64)>,
}

/// 概況中保留的最近要求與 Bundle 數量。
const RECENT_LIMIT: i64 = 20;

const PROFILE_COLUMNS: &str = "id, workspace_id, version, definition, content_hash, published_by, created_at";
const REQUEST_COLUMNS: &str =
    "r.id, r.workspace_id, r.task_id, r.profile_version_id, p.version, r.journeys, r.requested_by, r.status, r.bundle_id, r.created_at";

fn profile_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ProfileVersion> {
    Ok(ProfileVersion {
        id: r.get(0)?,
        workspace_id: r.get(1)?,
        version: r.get(2)?,
        definition: json_column(3, &r.get::<_, String>(3)?)?,
        content_hash: r.get(4)?,
        published_by: r.get(5)?,
        created_at: Timestamp::from_millis(r.get(6)?),
    })
}

fn request_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<VerificationRequest> {
    let status: String = r.get(7)?;
    Ok(VerificationRequest {
        id: r.get(0)?,
        workspace_id: r.get(1)?,
        task_id: r.get(2)?,
        profile_version_id: r.get(3)?,
        profile_version: r.get(4)?,
        journeys: json_column(5, &r.get::<_, String>(5)?)?,
        requested_by: r.get(6)?,
        status: RequestStatus::parse(&status).ok_or_else(|| {
            rusqlite::Error::FromSqlConversionFailure(7, rusqlite::types::Type::Text, "未知的驗證要求狀態".into())
        })?,
        bundle_id: r.get(8)?,
        created_at: Timestamp::from_millis(r.get(9)?),
    })
}

fn latest_profile(conn: &Connection, workspace_id: &str) -> rusqlite::Result<Option<ProfileVersion>> {
    conn.query_row(
        &format!("SELECT {PROFILE_COLUMNS} FROM verification_profile_version WHERE workspace_id = ?1 ORDER BY version DESC LIMIT 1"),
        [workspace_id],
        profile_row,
    )
    .optional()
}

fn load_profile(conn: &Connection, id: &str) -> rusqlite::Result<ProfileVersion> {
    conn.query_row(&format!("SELECT {PROFILE_COLUMNS} FROM verification_profile_version WHERE id = ?1"), [id], profile_row)
}

fn load_bundles(conn: &Connection, condition: &str, params: impl rusqlite::Params) -> rusqlite::Result<Vec<EvidenceBundle>> {
    conn.prepare(&format!("SELECT content FROM evidence_bundle WHERE {condition}"))?
        .query_map(params, |r| json_column(0, &r.get::<_, String>(0)?))?
        .collect()
}

/// Profile 版本是否已被證明可用（有代表旅程通過的 Bundle），回傳證明它的 Bundle 識別。
fn proven(conn: &Connection, profile: &ProfileVersion) -> rusqlite::Result<Option<String>> {
    // 整體判定不是 passed 的 Bundle 也可能有代表旅程通過（例如另一條旅程是產品缺陷），所以全部都看
    let bundles = load_bundles(conn, "profile_version_id = ?1 ORDER BY created_at, rowid", [&profile.id])?;
    Ok(verification_profile::proven_by(profile, &bundles).map(|b| b.id.clone()))
}

/// 確認 Workspace 存在而且沒有封存，回傳它的目錄。
fn active_workspace(conn: &Connection, workspace_id: &str) -> Result<String, VerificationApplyError> {
    let found: Option<(String, String, String)> = conn
        .query_row("SELECT name, path, status FROM workspace WHERE id = ?1", [workspace_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .optional()?;
    match found {
        None => Err(VerificationApplyError::WorkspaceNotFound(workspace_id.into())),
        Some((name, _, status)) if status == "archived" => Err(VerificationApplyError::WorkspaceArchived(name)),
        Some((_, path, _)) => Ok(path),
    }
}

/// 在交易中查驗 Stage 的驗證證據（票 43 的 exit check）：Bundle 必須判定為 passed，依據的是這個 Workspace 目前的
/// Profile 版本，而且綁定 Task 時，那個 Task 的契約版本仍與驗證當時相同。
/// 證據為 passed 之外，Bundle 驗證的 build 也必須是受驗目錄目前的 revision（票 50；`builds` 是 Runtime 觀察到的目前 build，
/// 沒有觀察到的目錄不比較）。
pub(crate) fn verification_evidence_status(
    conn: &Connection,
    workspace_id: &str,
    bundle_id: &str,
    builds: &super::workflows::Builds,
) -> rusqlite::Result<baton_domain::workflow::EvidenceStatus> {
    use baton_domain::workflow::EvidenceStatus;
    let Some(bundle) = load_bundles(conn, "id = ?1 AND workspace_id = ?2", [bundle_id, workspace_id])?.pop() else {
        return Ok(EvidenceStatus::NotFound);
    };
    let current_profile = latest_profile(conn, workspace_id)?.map(|p| p.id);
    let current_requirement: Option<i64> = match &bundle.task_id {
        Some(task_id) => {
            conn.query_row("SELECT contract_version FROM task WHERE id = ?1", [task_id], |r| r.get(0)).optional()?
        }
        None => None,
    };
    let valid = bundle.verdict == Verdict::Passed
        && current_profile.as_deref() == Some(bundle.profile_version_id.as_str())
        && current_requirement == bundle.requirement_version;
    if !valid {
        return Ok(EvidenceStatus::NotPassed);
    }
    let current_build = builds.get(&bundle.subject_dir).cloned().flatten();
    Ok(match (&bundle.candidate_revision, current_build) {
        (Some(verified), Some(current)) if *verified != current => EvidenceStatus::BuildChanged,
        _ => EvidenceStatus::Valid,
    })
}

/// 一份 Evidence Bundle。
pub(crate) fn bundle(conn: &Connection, workspace_id: &str, bundle_id: &str) -> rusqlite::Result<Option<EvidenceBundle>> {
    Ok(load_bundles(conn, "id = ?1 AND workspace_id = ?2", [bundle_id, workspace_id])?.pop())
}

/// 一個驗證要求要執行的旅程。
pub(crate) fn request_journeys(conn: &Connection, request_id: &str) -> rusqlite::Result<Vec<String>> {
    let raw: Option<String> =
        conn.query_row("SELECT journeys FROM verification_request WHERE id = ?1", [request_id], |r| r.get(0)).optional()?;
    raw.map(|raw| json_column(0, &raw)).transpose().map(Option::unwrap_or_default)
}

/// Gate 失效時由 Runtime 排入重驗（票 50）：以目前的 Profile 版本對 Workflow 的 Task 排入驗證要求，回傳要求識別。
/// 沒有 Profile 時不排入；原本的旅程在目前版本中已不存在時改為執行全部旅程。
pub(crate) fn queue_reverification(
    tx: &Transaction<'_>,
    workflow: &baton_domain::workflow::WorkflowInstance,
    journeys: Vec<String>,
    requested_by: String,
    id: String,
    now: Timestamp,
) -> rusqlite::Result<Option<String>> {
    let Some(profile) = latest_profile(tx, &workflow.workspace_id)? else { return Ok(None) };
    let is_proven = proven(tx, &profile)?.is_some();
    let journeys = verification_profile::plan_journeys(&profile.definition, &journeys, is_proven)
        .or_else(|_| verification_profile::plan_journeys(&profile.definition, &[], is_proven))
        .unwrap_or_default();
    let request = VerificationRequest {
        id,
        workspace_id: workflow.workspace_id.clone(),
        task_id: workflow.task_id.clone(),
        profile_version_id: profile.id.clone(),
        profile_version: profile.version,
        journeys,
        requested_by,
        status: RequestStatus::Queued,
        bundle_id: None,
        created_at: now,
    };
    insert_request(tx, &request)?;
    Ok(Some(request.id))
}

fn insert_request(tx: &Transaction<'_>, request: &VerificationRequest) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO verification_request
            (id, workspace_id, task_id, profile_version_id, journeys, requested_by, status, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'queued', ?7)",
        rusqlite::params![
            request.id,
            request.workspace_id,
            request.task_id,
            request.profile_version_id,
            serde_json::to_string(&request.journeys).expect("旅程清單一定可以序列化"),
            request.requested_by,
            request.created_at.as_millis(),
        ],
    )?;
    append_event(tx, &verification_profile::requested_event(request))
}

impl Store {
    /// 發布 Profile 的新版本（內容與最新版本相同時不追加）。回傳 (目前的最新版本, 是否新增)。
    pub async fn publish_verification_profile(
        &self,
        workspace_id: &str,
        definition: ProfileDefinition,
        published_by: String,
        now: Timestamp,
    ) -> Result<(ProfileVersion, bool), VerificationApplyError> {
        let workspace_id = workspace_id.to_string();
        let outcome = self
            .write(move |tx| {
                if let Err(error) = active_workspace(tx, &workspace_id) {
                    return Ok(Err(error));
                }
                let latest = latest_profile(tx, &workspace_id)?;
                let id = format!("vp_{workspace_id}_v{}", latest.as_ref().map_or(1, |l| l.version + 1));
                match verification_profile::publish(latest.as_ref(), &workspace_id, definition, published_by, id, now) {
                    Err(rejected) => Ok(Err(rejected.into())),
                    Ok(None) => Ok(Ok((latest.expect("內容相同表示已有最新版本"), false))),
                    Ok(Some((profile, event))) => {
                        tx.execute(
                            "INSERT INTO verification_profile_version
                                (id, workspace_id, version, definition, content_hash, published_by, created_at)
                             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                            rusqlite::params![
                                profile.id,
                                profile.workspace_id,
                                profile.version,
                                serde_json::to_string(&profile.definition).expect("Profile 一定可以序列化"),
                                profile.content_hash,
                                profile.published_by,
                                profile.created_at.as_millis(),
                            ],
                        )?;
                        append_event(tx, &event)?;
                        Ok(Ok((profile, true)))
                    }
                }
            })
            .await?;
        outcome
    }

    /// 建立驗證要求：固定目前的 Profile 版本與要執行的旅程。
    pub async fn request_verification(
        &self,
        workspace_id: &str,
        task_id: Option<String>,
        journeys: Vec<String>,
        requested_by: String,
        id: String,
        now: Timestamp,
    ) -> Result<VerificationRequest, VerificationApplyError> {
        let workspace_id = workspace_id.to_string();
        let outcome = self
            .write(move |tx| {
                if let Err(error) = active_workspace(tx, &workspace_id) {
                    return Ok(Err(error));
                }
                if let Some(task_id) = &task_id {
                    let found: Option<i64> = tx
                        .query_row("SELECT 1 FROM task WHERE id = ?1 AND workspace_id = ?2", [task_id, &workspace_id], |r| r.get(0))
                        .optional()?;
                    if found.is_none() {
                        return Ok(Err(VerificationApplyError::TaskNotFound(task_id.clone())));
                    }
                }
                let Some(profile) = latest_profile(tx, &workspace_id)? else {
                    return Ok(Err(ProfileError::NotFound.into()));
                };
                let is_proven = proven(tx, &profile)?.is_some();
                let journeys = match verification_profile::plan_journeys(&profile.definition, &journeys, is_proven) {
                    Ok(journeys) => journeys,
                    Err(rejected) => return Ok(Err(rejected.into())),
                };
                let request = VerificationRequest {
                    id,
                    workspace_id: workspace_id.clone(),
                    task_id,
                    profile_version_id: profile.id.clone(),
                    profile_version: profile.version,
                    journeys,
                    requested_by,
                    status: RequestStatus::Queued,
                    bundle_id: None,
                    created_at: now,
                };
                insert_request(tx, &request)?;
                Ok(Ok(request))
            })
            .await?;
        outcome
    }

    /// 等待執行的驗證要求（依建立順序），附上受驗目錄、需求版本與受驗 Run。封存的 Workspace 不執行。
    pub async fn pending_verification_runs(&self) -> StoreResult<Vec<PendingVerificationRun>> {
        self.read(|conn| {
            let requests: Vec<VerificationRequest> = conn
                .prepare(&format!(
                    "SELECT {REQUEST_COLUMNS} FROM verification_request r
                     JOIN verification_profile_version p ON p.id = r.profile_version_id
                     JOIN workspace w ON w.id = r.workspace_id
                     WHERE r.status = 'queued' AND w.status = 'active' ORDER BY r.created_at, r.rowid"
                ))?
                .query_map([], request_row)?
                .collect::<rusqlite::Result<_>>()?;
            let mut pending = Vec::with_capacity(requests.len());
            for request in requests {
                let profile = load_profile(conn, &request.profile_version_id)?;
                let workspace_dir: String =
                    conn.query_row("SELECT path FROM workspace WHERE id = ?1", [&request.workspace_id], |r| r.get(0))?;
                let (mut subject_dir, mut requirement_version, mut run) = (workspace_dir, None, None);
                if let Some(task_id) = &request.task_id {
                    if let Some(dir) = conn
                        .query_row("SELECT path FROM task_workdir WHERE task_id = ?1 AND status = 'ready'", [task_id], |r| r.get(0))
                        .optional()?
                    {
                        subject_dir = dir;
                    }
                    requirement_version =
                        Some(conn.query_row("SELECT contract_version FROM task WHERE id = ?1", [task_id], |r| r.get(0))?);
                    run = conn
                        .query_row(
                            "SELECT id, generation FROM run WHERE task_id = ?1 ORDER BY generation DESC, rowid DESC LIMIT 1",
                            [task_id],
                            |r| Ok((r.get(0)?, r.get(1)?)),
                        )
                        .optional()?;
                }
                pending.push(PendingVerificationRun { request, profile, subject_dir, requirement_version, run });
            }
            Ok(pending)
        })
        .await
    }

    /// 保存 Evidence Bundle 並把要求與執行中的嘗試（票 49）標為完成；要求已經完成時不寫入，回傳 `false`。
    pub async fn record_evidence_bundle(&self, bundle: EvidenceBundle) -> StoreResult<bool> {
        self.write(move |tx| {
            let updated = tx.execute(
                "UPDATE verification_request SET status = 'completed', bundle_id = ?2, completed_at = ?3
                 WHERE id = ?1 AND status = 'queued'",
                rusqlite::params![bundle.request_id, bundle.id, bundle.finished_at.as_millis()],
            )?;
            if updated == 0 {
                return Ok(false);
            }
            super::verification_attempts::complete_running_attempt(tx, &bundle.request_id, bundle.finished_at)?;
            tx.execute(
                "INSERT INTO evidence_bundle (id, workspace_id, request_id, task_id, profile_version_id, verdict, content, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                rusqlite::params![
                    bundle.id,
                    bundle.workspace_id,
                    bundle.request_id,
                    bundle.task_id,
                    bundle.profile_version_id,
                    bundle.verdict.as_str(),
                    serde_json::to_string(&bundle).expect("Evidence Bundle 一定可以序列化"),
                    bundle.finished_at.as_millis(),
                ],
            )?;
            append_event(tx, &verification_profile::completed_event(&bundle))?;
            Ok(true)
        })
        .await
    }

    /// Workspace 的驗證概況：最新 Profile、是否已被證明可用、最近的要求與 Bundle。
    pub async fn verification_overview(&self, workspace_id: &str) -> StoreResult<VerificationOverview> {
        let workspace_id = workspace_id.to_string();
        self.read(move |conn| {
            let profile = latest_profile(conn, &workspace_id)?;
            let proven_by = match &profile {
                Some(profile) => proven(conn, profile)?,
                None => None,
            };
            let requests = conn
                .prepare(&format!(
                    "SELECT {REQUEST_COLUMNS} FROM verification_request r
                     JOIN verification_profile_version p ON p.id = r.profile_version_id
                     WHERE r.workspace_id = ?1 ORDER BY r.created_at DESC, r.rowid DESC LIMIT ?2"
                ))?
                .query_map(rusqlite::params![workspace_id, RECENT_LIMIT], request_row)?
                .collect::<rusqlite::Result<_>>()?;
            let bundles = load_bundles(
                conn,
                "workspace_id = ?1 ORDER BY created_at DESC, rowid DESC LIMIT ?2",
                rusqlite::params![workspace_id, RECENT_LIMIT],
            )?;
            Ok(VerificationOverview { profile, proven_by, requests, bundles })
        })
        .await
    }

    /// 一份 Evidence Bundle。
    pub async fn evidence_bundle(&self, workspace_id: &str, bundle_id: &str) -> StoreResult<Option<EvidenceBundle>> {
        let (workspace_id, bundle_id) = (workspace_id.to_string(), bundle_id.to_string());
        self.read(move |conn| Ok(load_bundles(conn, "id = ?1 AND workspace_id = ?2", [&bundle_id, &workspace_id])?.pop())).await
    }
}
