//! 專案理解與投影檔狀態的持久化（票 42）。
//!
//! - 項目的每次變更在單一交易中完成：載入既有項目 → Domain 判定 → 寫回並追加事件。
//! - 投影的比對與寫檔在 API 層（需要 WorkspaceFs）；這裡只以「預期的 revision」寫回投影狀態，
//!   別人先改過就不寫（回 `false`），並在同一個交易中把新開的協調送進 Controller 對話成為工作事件。

use baton_domain::knowledge::{KnowledgeEntry, KnowledgeKind, KnowledgeStatus, ScopeKind};
use baton_domain::understanding::{
    self, ItemStatus, Projection, ProjectionKind, ProjectionPlan, ProjectionStatus, RecordInput, Section,
    UnderstandingError, UnderstandingItem,
};

use rusqlite::OptionalExtension;

use super::*;

/// 套用專案理解變更的失敗：Domain 拒絕或儲存層錯誤。
#[derive(Debug, thiserror::Error)]
pub enum UnderstandingApplyError {
    #[error(transparent)]
    Rejected(#[from] UnderstandingError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl From<rusqlite::Error> for UnderstandingApplyError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error.into())
    }
}

const ITEM_COLUMNS: &str =
    "id, workspace_id, section, key, text, status, sources, revision, recorded_by, created_at, updated_at";

const PROJECTION_COLUMNS: &str = "workspace_id, path, kind, version, body_hash, baseline_hash, status, external_hash, \
     detected_at, revision, updated_at";

fn unknown(index: usize, what: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Text, format!("未知的{what}").into())
}

fn item_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<UnderstandingItem> {
    let section: String = r.get(2)?;
    let status: String = r.get(5)?;
    Ok(UnderstandingItem {
        id: r.get(0)?,
        workspace_id: r.get(1)?,
        section: Section::parse(&section).ok_or_else(|| unknown(2, "文件別"))?,
        key: r.get(3)?,
        text: r.get(4)?,
        status: ItemStatus::parse(&status).ok_or_else(|| unknown(5, "可信狀態"))?,
        sources: json_column(6, &r.get::<_, String>(6)?)?,
        revision: r.get(7)?,
        recorded_by: r.get(8)?,
        created_at: Timestamp::from_millis(r.get(9)?),
        updated_at: Timestamp::from_millis(r.get(10)?),
    })
}

fn projection_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Projection> {
    let kind: String = r.get(2)?;
    let status: String = r.get(6)?;
    Ok(Projection {
        workspace_id: r.get(0)?,
        path: r.get(1)?,
        kind: ProjectionKind::parse(&kind).ok_or_else(|| unknown(2, "投影種類"))?,
        version: r.get(3)?,
        body_hash: r.get(4)?,
        baseline_hash: r.get(5)?,
        status: ProjectionStatus::parse(&status).ok_or_else(|| unknown(6, "投影狀態"))?,
        external_hash: r.get(7)?,
        detected_at: r.get::<_, Option<i64>>(8)?.map(Timestamp::from_millis),
        revision: r.get(9)?,
        updated_at: Timestamp::from_millis(r.get(10)?),
    })
}

fn load_items(conn: &Connection, workspace_id: &str) -> rusqlite::Result<Vec<UnderstandingItem>> {
    conn.prepare(&format!(
        "SELECT {ITEM_COLUMNS} FROM understanding_item WHERE workspace_id = ?1 ORDER BY section, key"
    ))?
    .query_map([workspace_id], item_row)?
    .collect()
}

fn load_projection(conn: &Connection, workspace_id: &str, path: &str) -> rusqlite::Result<Option<Projection>> {
    conn.query_row(
        &format!("SELECT {PROJECTION_COLUMNS} FROM understanding_projection WHERE workspace_id = ?1 AND path = ?2"),
        [workspace_id, path],
        projection_row,
    )
    .optional()
}

impl Store {
    /// 某個 Workspace 的全部專案理解項目（含已移除），依文件別、名稱排序。
    pub async fn understanding_items(&self, workspace_id: &str) -> StoreResult<Vec<UnderstandingItem>> {
        let workspace_id = workspace_id.to_string();
        self.read(move |conn| Ok(load_items(conn, &workspace_id)?)).await
    }

    /// 某個 Workspace 的全部投影檔狀態，依路徑排序。
    pub async fn understanding_projections(&self, workspace_id: &str) -> StoreResult<Vec<Projection>> {
        let workspace_id = workspace_id.to_string();
        self.read(move |conn| {
            Ok(conn
                .prepare(&format!(
                    "SELECT {PROJECTION_COLUMNS} FROM understanding_projection WHERE workspace_id = ?1 ORDER BY path"
                ))?
                .query_map([&workspace_id], projection_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?)
        })
        .await
    }

    /// 投影成 ADR 的決策：這個 Workspace 範圍內、使用者建立（非候選）的決策條目，依建立順序。
    pub async fn workspace_decisions(&self, workspace_id: &str) -> StoreResult<Vec<KnowledgeEntry>> {
        let workspace_id = workspace_id.to_string();
        self.read(move |conn| {
            let ids = conn
                .prepare(
                    "SELECT id FROM knowledge_entry WHERE workspace_id = ?1 AND scope_kind = ?2 AND kind = ?3 AND status <> ?4
                     ORDER BY created_at, rowid",
                )?
                .query_map(
                    [
                        workspace_id.as_str(),
                        ScopeKind::Workspace.as_str(),
                        KnowledgeKind::Decision.as_str(),
                        KnowledgeStatus::Candidate.as_str(),
                    ],
                    |r| r.get::<_, String>(0),
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let mut entries = Vec::new();
            for id in ids {
                entries.extend(super::knowledge::load_entry(conn, &id)?);
            }
            Ok(entries)
        })
        .await
    }

    /// 建立或更新一項（以 Workspace、文件別、名稱識別）。回傳目前的項目，以及這次是否有變更。
    pub async fn record_understanding(
        &self,
        workspace_id: &str,
        new_id: String,
        input: RecordInput,
        recorded_by: &str,
        now: Timestamp,
    ) -> Result<(UnderstandingItem, bool), UnderstandingApplyError> {
        let (workspace_id, recorded_by) = (workspace_id.to_string(), recorded_by.to_string());
        let outcome = self
            .write(move |tx| {
                let existing = tx
                    .query_row(
                        &format!(
                            "SELECT {ITEM_COLUMNS} FROM understanding_item WHERE workspace_id = ?1 AND section = ?2 AND key = ?3"
                        ),
                        [workspace_id.as_str(), input.section.as_str(), input.key.trim()],
                        item_row,
                    )
                    .optional()?;
                let change = match understanding::record(existing.as_ref(), new_id, &workspace_id, input, &recorded_by, now) {
                    Err(rejected) => return Ok(Err(rejected)),
                    Ok(None) => return Ok(Ok((existing.expect("沒有變更表示項目已存在"), false))),
                    Ok(Some(change)) => change,
                };
                let item = &change.item;
                tx.execute(
                    "INSERT INTO understanding_item
                        (id, workspace_id, section, key, text, status, sources, revision, recorded_by, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                     ON CONFLICT(id) DO UPDATE SET
                        text = excluded.text, status = excluded.status, sources = excluded.sources,
                        revision = excluded.revision, recorded_by = excluded.recorded_by, updated_at = excluded.updated_at",
                    rusqlite::params![
                        item.id,
                        item.workspace_id,
                        item.section.as_str(),
                        item.key,
                        item.text,
                        item.status.as_str(),
                        serde_json::to_string(&item.sources).expect("來源一定可以序列化"),
                        item.revision,
                        item.recorded_by,
                        item.created_at.as_millis(),
                        item.updated_at.as_millis(),
                    ],
                )?;
                append_event(tx, &change.event)?;
                Ok(Ok((change.item, true)))
            })
            .await?;
        Ok(outcome?)
    }

    /// 目前的投影狀態（單一路徑）。
    pub async fn understanding_projection(&self, workspace_id: &str, path: &str) -> StoreResult<Option<Projection>> {
        let (workspace_id, path) = (workspace_id.to_string(), path.to_string());
        self.read(move |conn| Ok(load_projection(conn, &workspace_id, &path)?)).await
    }

    /// 寫回一次投影比對或協調的結果。投影狀態必須仍是比對時讀到的那一版（`expected_revision`，沒有紀錄時為 0），
    /// 否則什麼都不寫並回 `false`。`notice` 有值時（新開的協調）在同一個交易中成為 Controller 對話的工作事件。
    pub async fn save_projection(
        &self,
        plan: ProjectionPlan,
        expected_revision: i64,
        notice: Option<(String, String)>,
    ) -> StoreResult<bool> {
        let Some(projection) = plan.projection else { return Ok(true) };
        self.write(move |tx| {
            let current = load_projection(tx, &projection.workspace_id, &projection.path)?;
            if current.map_or(0, |p| p.revision) != expected_revision {
                return Ok(false);
            }
            tx.execute(
                "INSERT INTO understanding_projection
                    (workspace_id, path, kind, version, body_hash, baseline_hash, status, external_hash, detected_at, revision, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                 ON CONFLICT(workspace_id, path) DO UPDATE SET
                    version = excluded.version, body_hash = excluded.body_hash, baseline_hash = excluded.baseline_hash,
                    status = excluded.status, external_hash = excluded.external_hash, detected_at = excluded.detected_at,
                    revision = excluded.revision, updated_at = excluded.updated_at",
                rusqlite::params![
                    projection.workspace_id,
                    projection.path,
                    projection.kind.as_str(),
                    projection.version,
                    projection.body_hash,
                    projection.baseline_hash,
                    projection.status.as_str(),
                    projection.external_hash,
                    projection.detected_at.map(|t| t.as_millis()),
                    projection.revision,
                    projection.updated_at.as_millis(),
                ],
            )?;
            for event in &plan.events {
                append_event(tx, event)?;
            }
            if let Some((source, content)) = notice {
                super::conversations::enqueue_notice(tx, &projection.workspace_id, &source, &content, projection.updated_at)?;
            }
            Ok(true)
        })
        .await
    }
}
