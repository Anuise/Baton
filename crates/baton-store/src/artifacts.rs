//! 成果與版本 manifest 的持久化（票 12）。
//!
//! manifest 只在檔案已經完整落地（暫存 → fsync → 原子改名）之後才提交，所以資料庫裡的版本永遠指向完整的檔案。
//! 登記在單一交易中完成：確認呼叫者仍是寫入所有者 → Domain 決定版本 → 寫入成果、版本與事件。
//! 成果與版本都是追加式資料表（觸發器禁止更新與刪除），舊版本不會被改動。

use baton_domain::artifact::{self, Artifact, ArtifactDraft, ArtifactVersion, Registered, Registration};
use baton_domain::auth::AgentScope;
use baton_domain::run::{self, RunError};
use baton_domain::Timestamp;

use super::runs::RunApplyError;
use super::tasks::load;
use super::*;

/// 呼叫者（已驗證的內容與已落地的檔案）要登記的新版本；Task 與產生者取自憑證所屬的 Run，不由呼叫者提供。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewArtifactVersion {
    /// 這個成果尚不存在時使用的識別。
    pub new_artifact_id: String,
    pub draft: ArtifactDraft,
    pub size_bytes: i64,
    pub sha256: String,
    pub storage_path: String,
}

impl Store {
    /// 登記成果的新版本。Run 已結束或已被接替（不是目前的寫入所有者）時回報 `RunError::StaleReport`，什麼都不寫。
    pub async fn register_artifact(
        &self,
        scope: &AgentScope,
        input: NewArtifactVersion,
        now: Timestamp,
    ) -> Result<Registered, RunApplyError> {
        let scope = scope.clone();
        let outcome = self
            .write(move |tx| {
                let loaded = load(tx, &scope.workspace_id)?;
                let Some(run) = loaded.runs.iter().find(|r| r.id == scope.run_id) else {
                    return Ok(Err(RunError::RunNotFound(scope.run_id)));
                };
                let lease = loaded.leases.iter().find(|l| l.run_id == run.id);
                if !run::is_current_writer(run, lease, scope.generation) {
                    return Ok(Err(RunError::StaleReport));
                }
                let existing = load_artifacts(tx, &scope.workspace_id, Some((&run.task_id, &input.draft.name)))?.pop();
                let registration = Registration {
                    new_artifact_id: input.new_artifact_id,
                    workspace_id: scope.workspace_id.clone(),
                    task_id: run.task_id.clone(),
                    run_id: run.id.clone(),
                    generation: scope.generation,
                    draft: input.draft.clone(),
                    size_bytes: input.size_bytes,
                    sha256: input.sha256,
                    storage_path: input.storage_path,
                };
                let registered = artifact::register(existing.as_ref(), registration, now);
                if registered.created {
                    tx.execute(
                        "INSERT INTO artifact (id, workspace_id, task_id, name, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                        rusqlite::params![
                            registered.artifact_id,
                            scope.workspace_id,
                            run.task_id,
                            input.draft.name,
                            now.as_millis()
                        ],
                    )?;
                }
                let v = &registered.version;
                tx.execute(
                    "INSERT INTO artifact_version
                     (artifact_id, version, description, size_bytes, sha256, storage_path, run_id, generation, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    rusqlite::params![
                        registered.artifact_id,
                        v.version,
                        v.description,
                        v.size_bytes,
                        v.sha256,
                        v.storage_path,
                        v.produced_by_run_id,
                        v.produced_by_generation,
                        v.created_at.as_millis(),
                    ],
                )?;
                for event in &registered.events {
                    append_event(tx, event)?;
                }
                Ok(Ok(registered))
            })
            .await?;
        Ok(outcome?)
    }

    /// Workspace 內的全部成果（含全部版本），依建立順序；Workspace 不存在時回報 `WorkspaceNotFound`。
    pub async fn artifact_list(&self, workspace_id: &str) -> Result<Vec<Artifact>, RunApplyError> {
        let workspace_id = workspace_id.to_string();
        let result = self
            .read(move |conn| {
                if !workspace_exists(conn, &workspace_id)? {
                    return Ok(Err(RunError::WorkspaceNotFound(workspace_id)));
                }
                Ok(Ok(load_artifacts(conn, &workspace_id, None)?))
            })
            .await?;
        Ok(result?)
    }
}

fn workspace_exists(conn: &Connection, workspace_id: &str) -> rusqlite::Result<bool> {
    conn.query_row("SELECT EXISTS (SELECT 1 FROM workspace WHERE id = ?1)", [workspace_id], |r| r.get(0))
}

/// 載入成果與版本（版本由舊到新）；`only` 指定 (task_id, 名稱) 時只載入那一個。
fn load_artifacts(conn: &Connection, workspace_id: &str, only: Option<(&str, &str)>) -> rusqlite::Result<Vec<Artifact>> {
    let mut artifacts = conn
        .prepare(
            "SELECT id, task_id, name, created_at FROM artifact
             WHERE workspace_id = ?1 AND (?2 IS NULL OR (task_id = ?2 AND name = ?3)) ORDER BY rowid",
        )?
        .query_map(rusqlite::params![workspace_id, only.map(|o| o.0), only.map(|o| o.1)], |r| {
            Ok(Artifact {
                id: r.get(0)?,
                workspace_id: workspace_id.to_string(),
                task_id: r.get(1)?,
                name: r.get(2)?,
                created_at: Timestamp::from_millis(r.get(3)?),
                versions: Vec::new(),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut statement = conn.prepare(
        "SELECT version, description, size_bytes, sha256, storage_path, run_id, generation, created_at
         FROM artifact_version WHERE artifact_id = ?1 ORDER BY version",
    )?;
    for artifact in &mut artifacts {
        artifact.versions = statement
            .query_map([&artifact.id], |r| {
                Ok(ArtifactVersion {
                    version: r.get(0)?,
                    description: r.get(1)?,
                    size_bytes: r.get(2)?,
                    sha256: r.get(3)?,
                    storage_path: r.get(4)?,
                    produced_by_run_id: r.get(5)?,
                    produced_by_generation: r.get(6)?,
                    created_at: Timestamp::from_millis(r.get(7)?),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
    }
    Ok(artifacts)
}
