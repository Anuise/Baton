//! 知識條目與教訓的持久化與搜尋（票 37，spec D-26、D-28）。
//!
//! - 每個變更在單一交易中完成：載入目前狀態 → Domain 判定 → 寫回條目、追加版本與事件、更新全文檢索索引。
//! - 全文檢索索引 `knowledge_fts`（FTS5 + trigram）是可重建的衍生資料：每次開啟時從 `knowledge_entry.search_text` 整個重建，
//!   所以降級期間（FTS5 不可用）寫入的條目在恢復後一樣搜得到。
//! - 搜尋：FTS5 可用時，3 個字元以上的關鍵字走 trigram 索引（MATCH），2 個字元的關鍵字（trigram 索引無法涵蓋）
//!   與 FTS5 不可用時一律以 LIKE 逐筆比對 `search_text`；兩種方式的結果相同。

use baton_domain::auth::AgentScope;
use baton_domain::knowledge::{
    self, Audience, KnowledgeChange, KnowledgeCommand, KnowledgeContent, KnowledgeEntry, KnowledgeError,
    KnowledgeEvent, KnowledgeKind, KnowledgeScope, KnowledgeSource, KnowledgeStatus, KnowledgeVersion, NewKnowledge,
    ScopeKind, SearchMode, RESULT_LIMIT,
};
use baton_domain::run::{self, RunError};

use super::tasks::load;
use super::*;

/// 套用知識變更的失敗：Domain 拒絕、Agent 的 Run 已不是寫入所有者（或找不到）、或儲存層錯誤。
#[derive(Debug, thiserror::Error)]
pub enum KnowledgeApplyError {
    #[error(transparent)]
    Rejected(#[from] KnowledgeError),
    #[error(transparent)]
    Run(RunError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl From<rusqlite::Error> for KnowledgeApplyError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error.into())
    }
}

/// 使用者指定的適用範圍（尚未核對是否存在）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeRequest {
    pub kind: ScopeKind,
    pub workspace_id: Option<String>,
    pub goal_id: Option<String>,
    pub task_id: Option<String>,
}

/// 使用者建立條目的輸入；內容已由 Domain 驗證。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewKnowledgeRequest {
    pub id: String,
    pub kind: KnowledgeKind,
    pub scope: ScopeRequest,
    pub content: KnowledgeContent,
    pub valid_from: Option<Timestamp>,
    pub valid_until: Option<Timestamp>,
    pub supersedes: Option<String>,
}

/// 搜尋者；見 [`Audience`]。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KnowledgeAudience {
    Desktop { workspace_id: Option<String> },
    Run { workspace_id: String, goal_id: String, task_id: String },
}

impl KnowledgeAudience {
    fn as_domain(&self) -> Audience<'_> {
        match self {
            KnowledgeAudience::Desktop { workspace_id } => Audience::Desktop { workspace_id: workspace_id.as_deref() },
            KnowledgeAudience::Run { workspace_id, goal_id, task_id } => {
                Audience::Run { workspace_id, goal_id, task_id }
            }
        }
    }

    fn workspace_id(&self) -> Option<&str> {
        match self {
            KnowledgeAudience::Desktop { workspace_id } => workspace_id.as_deref(),
            KnowledgeAudience::Run { workspace_id, .. } => Some(workspace_id),
        }
    }
}

/// 建立（或確認既有的）全文檢索索引並從條目重建；FTS5 或 trigram tokenizer 不可用時回報錯誤（呼叫端改用 LIKE）。
pub(crate) fn prepare_full_text(conn: &mut Connection) -> StoreResult<()> {
    run_in_transaction(conn, |tx| {
        tx.execute_batch(
            "CREATE VIRTUAL TABLE IF NOT EXISTS knowledge_fts USING fts5(entry_id UNINDEXED, content, tokenize = 'trigram');
             DELETE FROM knowledge_fts;
             INSERT INTO knowledge_fts (entry_id, content) SELECT id, search_text FROM knowledge_entry;",
        )?;
        Ok(())
    })
}

impl Store {
    /// 目前的搜尋方式。
    pub fn search_mode(&self) -> SearchMode {
        if self.inner.full_text {
            SearchMode::FullText
        } else {
            SearchMode::LikeFallback
        }
    }

    /// 使用者建立條目（已驗證）；指定 `supersedes` 時同一個交易把被取代的條目轉為「被取代」。
    pub async fn create_knowledge(
        &self,
        input: NewKnowledgeRequest,
        now: Timestamp,
    ) -> Result<KnowledgeEntry, KnowledgeApplyError> {
        let full_text = self.inner.full_text;
        let outcome = self
            .write(move |tx| {
                let scope = match resolve_scope(tx, &input.scope)? {
                    Ok(scope) => scope,
                    Err(rejected) => return Ok(Err(rejected)),
                };
                let old = match &input.supersedes {
                    None => None,
                    Some(id) => match load_entry(tx, id)? {
                        Some(old) => Some(old),
                        None => return Ok(Err(KnowledgeError::NotFound(id.clone()))),
                    },
                };
                let new = NewKnowledge {
                    id: input.id,
                    kind: input.kind,
                    scope,
                    content: input.content,
                    source: KnowledgeSource::User,
                    valid_from: input.valid_from,
                    valid_until: input.valid_until,
                };
                match knowledge::create(new, old.as_ref(), now) {
                    Err(rejected) => Ok(Err(rejected)),
                    Ok(change) => Ok(Ok(save_change(tx, change, full_text)?)),
                }
            })
            .await?;
        Ok(outcome?)
    }

    /// Agent 提交候選教訓。範圍一律以憑證所屬 Run 的 Task 解析（`scope_kind` 只能是 Workspace、Goal 或 Task）；
    /// Run 已不是寫入所有者時回報 `RunError::StaleReport`，什麼都不寫。
    pub async fn record_lesson_candidate(
        &self,
        agent: &AgentScope,
        id: String,
        scope_kind: ScopeKind,
        content: KnowledgeContent,
        now: Timestamp,
    ) -> Result<KnowledgeEntry, KnowledgeApplyError> {
        let agent = agent.clone();
        let full_text = self.inner.full_text;
        let outcome = self
            .write(move |tx| {
                let loaded = load(tx, &agent.workspace_id)?;
                let Some(run) = loaded.runs.iter().find(|r| r.id == agent.run_id) else {
                    return Ok(Err(KnowledgeApplyError::Run(RunError::RunNotFound(agent.run_id))));
                };
                let lease = loaded.leases.iter().find(|l| l.run_id == run.id);
                if !run::is_current_writer(run, lease, agent.generation) {
                    return Ok(Err(KnowledgeApplyError::Run(RunError::StaleReport)));
                }
                let Some(task) = loaded.tasks.iter().find(|t| t.id == run.task_id) else {
                    return Ok(Err(KnowledgeApplyError::Run(RunError::TaskNotFound(run.task_id.clone()))));
                };
                let scope = match lesson_scope(scope_kind, &agent.workspace_id, &task.goal_id, &task.id) {
                    Ok(scope) => scope,
                    Err(rejected) => return Ok(Err(rejected.into())),
                };
                let new = NewKnowledge {
                    id,
                    kind: KnowledgeKind::Lesson,
                    scope,
                    content,
                    source: KnowledgeSource::Agent {
                        run_id: run.id.clone(),
                        generation: agent.generation,
                        task_id: task.id.clone(),
                    },
                    valid_from: None,
                    valid_until: None,
                };
                match knowledge::create(new, None, now) {
                    Err(rejected) => Ok(Err(rejected.into())),
                    Ok(change) => Ok(Ok(save_change(tx, change, full_text)?)),
                }
            })
            .await?;
        outcome
    }

    /// Controller 提交候選教訓（票 54，spec D-05）。Controller 沒有自己的 Task，所以由它指定教訓來源的 Task：
    /// 在同一個交易中確認 Task 屬於 Controller 的 Workspace（否則回報 `RunError::TaskNotFound`，什麼都不寫），
    /// 範圍一律以這個 Task 解析（`scope_kind` 只能是 Workspace、Goal 或 Task）。
    pub async fn record_controller_lesson_candidate(
        &self,
        controller: &ControllerScope,
        id: String,
        task_id: &str,
        scope_kind: ScopeKind,
        content: KnowledgeContent,
        now: Timestamp,
    ) -> Result<KnowledgeEntry, KnowledgeApplyError> {
        let controller = controller.clone();
        let task_id = task_id.to_string();
        let full_text = self.inner.full_text;
        let outcome = self
            .write(move |tx| {
                let goal_id = tx
                    .query_row(
                        "SELECT goal_id FROM task WHERE id = ?1 AND workspace_id = ?2",
                        [&task_id, &controller.workspace_id],
                        |r| r.get::<_, String>(0),
                    )
                    .map(Some)
                    .or_else(|e| match e {
                        rusqlite::Error::QueryReturnedNoRows => Ok(None),
                        other => Err(other),
                    })?;
                let Some(goal_id) = goal_id else {
                    return Ok(Err(KnowledgeApplyError::Run(RunError::TaskNotFound(task_id))));
                };
                let scope = match lesson_scope(scope_kind, &controller.workspace_id, &goal_id, &task_id) {
                    Ok(scope) => scope,
                    Err(rejected) => return Ok(Err(rejected.into())),
                };
                let new = NewKnowledge {
                    id,
                    kind: KnowledgeKind::Lesson,
                    scope,
                    content,
                    source: KnowledgeSource::Controller { controller_run_id: controller.controller_run_id, task_id },
                    valid_from: None,
                    valid_until: None,
                };
                match knowledge::create(new, None, now) {
                    Err(rejected) => Ok(Err(rejected.into())),
                    Ok(change) => Ok(Ok(save_change(tx, change, full_text)?)),
                }
            })
            .await?;
        outcome
    }

    /// 使用者對既有條目的操作（確認、修正、撤銷、淘汰），以 expected_revision 校驗。
    pub async fn apply_knowledge_command(
        &self,
        id: &str,
        expected_revision: i64,
        command: KnowledgeCommand,
        now: Timestamp,
    ) -> Result<KnowledgeEntry, KnowledgeApplyError> {
        let id = id.to_string();
        let full_text = self.inner.full_text;
        let outcome = self
            .write(move |tx| {
                let Some(current) = load_entry(tx, &id)? else {
                    return Ok(Err(KnowledgeError::NotFound(id)));
                };
                match knowledge::decide(&current, expected_revision, command, now) {
                    Err(rejected) => Ok(Err(rejected)),
                    Ok(change) => Ok(Ok(save_change(tx, change, full_text)?)),
                }
            })
            .await?;
        Ok(outcome?)
    }

    /// 搜尋或列出條目：範圍由 `audience` 決定；`terms` 全部都要出現（空的表示不過濾文字）；
    /// `include_inactive` 為 false 時只回傳現在有效的條目。依最後更新時間由新到舊，最多 [`RESULT_LIMIT`] 筆。
    pub async fn knowledge_search(
        &self,
        audience: KnowledgeAudience,
        terms: Vec<String>,
        include_inactive: bool,
        now: Timestamp,
    ) -> StoreResult<Vec<KnowledgeEntry>> {
        let full_text = self.inner.full_text;
        self.read(move |conn| {
            let mut sql = format!(
                "SELECT {ENTRY_COLUMNS} FROM knowledge_entry
                 WHERE (workspace_id IS NULL OR workspace_id = ?1)"
            );
            let mut params: Vec<String> = vec![audience.workspace_id().unwrap_or_default().to_string()];
            for term in &terms {
                params.push(if full_text && term.chars().count() >= 3 {
                    sql.push_str(&format!(
                        " AND id IN (SELECT entry_id FROM knowledge_fts WHERE knowledge_fts MATCH ?{})",
                        params.len() + 1
                    ));
                    // 以片語查詢：trigram 會比對子字串；雙引號以重複一次跳脫
                    format!("\"{}\"", term.replace('"', "\"\""))
                } else {
                    sql.push_str(&format!(" AND search_text LIKE ?{} ESCAPE '\\'", params.len() + 1));
                    format!("%{}%", escape_like(term))
                });
            }
            sql.push_str(" ORDER BY updated_at DESC, rowid DESC");
            let domain_audience = audience.as_domain();
            let entries = conn
                .prepare(&sql)?
                .query_map(rusqlite::params_from_iter(params.iter()), entry_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?
                .into_iter()
                .filter(|entry| domain_audience.can_see(&entry.scope))
                .filter(|entry| include_inactive || entry.is_effective_at(now))
                .take(RESULT_LIMIT)
                .collect();
            Ok(entries)
        })
        .await
    }

    /// 單一條目：目前狀態、全部內容版本（由舊到新）與變更歷史（由舊到新）。
    pub async fn knowledge_detail(
        &self,
        id: &str,
    ) -> Result<(KnowledgeEntry, Vec<KnowledgeVersion>, Vec<KnowledgeEvent>), KnowledgeApplyError> {
        let id = id.to_string();
        let result = self
            .read(move |conn| {
                let Some(entry) = load_entry(conn, &id)? else {
                    return Ok(Err(KnowledgeError::NotFound(id)));
                };
                let versions = conn
                    .prepare(
                        "SELECT version, content, note, created_at FROM knowledge_entry_version
                         WHERE entry_id = ?1 ORDER BY version",
                    )?
                    .query_map([&id], |r| {
                        Ok(KnowledgeVersion {
                            version: r.get(0)?,
                            content: json_column(1, &r.get::<_, String>(1)?)?,
                            note: r.get(2)?,
                            created_at: Timestamp::from_millis(r.get(3)?),
                        })
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                let history = conn
                    .prepare(
                        "SELECT aggregate_version, event_type, occurred_at, payload FROM event
                         WHERE aggregate_type = ?1 AND aggregate_id = ?2 ORDER BY seq",
                    )?
                    .query_map([knowledge::AGGREGATE, id.as_str()], |r| {
                        Ok(KnowledgeEvent {
                            revision: r.get(0)?,
                            event_type: r.get(1)?,
                            occurred_at: Timestamp::from_millis(r.get(2)?),
                            payload: json_column(3, &r.get::<_, String>(3)?)?,
                        })
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(Ok((entry, versions, history)))
            })
            .await?;
        Ok(result?)
    }
}

/// Agent（Worker 或 Controller）提交的教訓的範圍：以來源 Task 解析；全域一律拒絕。
fn lesson_scope(kind: ScopeKind, workspace_id: &str, goal_id: &str, task_id: &str) -> Result<KnowledgeScope, KnowledgeError> {
    let workspace_id = Some(workspace_id.to_string());
    Ok(match kind {
        ScopeKind::Global => {
            return Err(KnowledgeError::ScopeInvalid("Agent 只能提交 Workspace、Goal 或 Task 範圍的教訓".to_string()));
        }
        ScopeKind::Workspace => KnowledgeScope { kind, workspace_id, goal_id: None, task_id: None },
        ScopeKind::Goal => KnowledgeScope { kind, workspace_id, goal_id: Some(goal_id.to_string()), task_id: None },
        ScopeKind::Task => {
            KnowledgeScope { kind, workspace_id, goal_id: Some(goal_id.to_string()), task_id: Some(task_id.to_string()) }
        }
    })
}

fn escape_like(term: &str) -> String {
    term.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

/// 核對使用者指定的範圍確實存在、彼此一致；Task 範圍的 Goal 一律取自 Task。
fn resolve_scope(conn: &Connection, request: &ScopeRequest) -> rusqlite::Result<Result<KnowledgeScope, KnowledgeError>> {
    let invalid = |message: &str| Ok(Err(KnowledgeError::ScopeInvalid(message.to_string())));
    let workspace_id = match (request.kind, &request.workspace_id) {
        (ScopeKind::Global, None) if request.goal_id.is_none() && request.task_id.is_none() => {
            return Ok(Ok(KnowledgeScope::global()));
        }
        (ScopeKind::Global, _) => return invalid("全域條目不能指定 Workspace、Goal 或 Task"),
        (_, None) => return invalid("Workspace、Goal 與 Task 範圍必須指定 Workspace"),
        (_, Some(id)) => id.clone(),
    };
    let exists: bool =
        conn.query_row("SELECT EXISTS (SELECT 1 FROM workspace WHERE id = ?1)", [&workspace_id], |r| r.get(0))?;
    if !exists {
        return invalid(&format!("找不到 Workspace {workspace_id}"));
    }
    let lookup = |sql: &str, id: &str| -> rusqlite::Result<Option<String>> {
        conn.query_row(sql, [id, workspace_id.as_str()], |r| r.get(0)).map(Some).or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(other),
        })
    };
    let scope = |goal_id, task_id| KnowledgeScope { kind: request.kind, workspace_id: Some(workspace_id.clone()), goal_id, task_id };
    match request.kind {
        ScopeKind::Global => unreachable!("全域已在上面處理"),
        ScopeKind::Workspace if request.goal_id.is_none() && request.task_id.is_none() => Ok(Ok(scope(None, None))),
        ScopeKind::Workspace => invalid("Workspace 範圍不能指定 Goal 或 Task"),
        ScopeKind::Goal => {
            let (Some(goal_id), None) = (&request.goal_id, &request.task_id) else {
                return invalid("Goal 範圍必須指定 Goal，且不能指定 Task");
            };
            match lookup("SELECT id FROM goal WHERE id = ?1 AND workspace_id = ?2", goal_id)? {
                Some(goal_id) => Ok(Ok(scope(Some(goal_id), None))),
                None => invalid(&format!("這個 Workspace 沒有 Goal {goal_id}")),
            }
        }
        ScopeKind::Task => {
            let Some(task_id) = &request.task_id else {
                return invalid("Task 範圍必須指定 Task");
            };
            match lookup("SELECT goal_id FROM task WHERE id = ?1 AND workspace_id = ?2", task_id)? {
                Some(goal_id) if request.goal_id.as_ref().is_none_or(|g| *g == goal_id) => {
                    Ok(Ok(scope(Some(goal_id), Some(task_id.clone()))))
                }
                Some(_) => invalid("指定的 Goal 與 Task 所屬的 Goal 不一致"),
                None => invalid(&format!("這個 Workspace 沒有 Task {task_id}")),
            }
        }
    }
}

const ENTRY_COLUMNS: &str = "id, kind, scope_kind, workspace_id, goal_id, task_id, status, source, valid_from, valid_until,
    version, revision, supersedes, superseded_by, status_reason, created_at, updated_at,
    (SELECT content FROM knowledge_entry_version v WHERE v.entry_id = knowledge_entry.id AND v.version = knowledge_entry.version)";

fn unknown(index: usize, what: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Text, format!("未知的{what}").into())
}

fn entry_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<KnowledgeEntry> {
    let kind: String = r.get(1)?;
    let scope_kind: String = r.get(2)?;
    let status: String = r.get(6)?;
    Ok(KnowledgeEntry {
        id: r.get(0)?,
        kind: KnowledgeKind::parse(&kind).ok_or_else(|| unknown(1, "知識種類"))?,
        scope: KnowledgeScope {
            kind: ScopeKind::parse(&scope_kind).ok_or_else(|| unknown(2, "範圍"))?,
            workspace_id: r.get(3)?,
            goal_id: r.get(4)?,
            task_id: r.get(5)?,
        },
        status: KnowledgeStatus::parse(&status).ok_or_else(|| unknown(6, "可信狀態"))?,
        source: json_column(7, &r.get::<_, String>(7)?)?,
        valid_from: Timestamp::from_millis(r.get(8)?),
        valid_until: r.get::<_, Option<i64>>(9)?.map(Timestamp::from_millis),
        version: r.get(10)?,
        revision: r.get(11)?,
        supersedes: r.get(12)?,
        superseded_by: r.get(13)?,
        status_reason: r.get(14)?,
        created_at: Timestamp::from_millis(r.get(15)?),
        updated_at: Timestamp::from_millis(r.get(16)?),
        content: json_column(17, &r.get::<_, String>(17)?)?,
    })
}

/// 全域與指定 Workspace 中已驗證的決策與教訓（Context Builder 用；範圍與有效期間由 Domain 再判定）。
pub(crate) fn load_verified_guidance(conn: &Connection, workspace_id: &str) -> rusqlite::Result<Vec<KnowledgeEntry>> {
    conn.prepare(&format!(
        "SELECT {ENTRY_COLUMNS} FROM knowledge_entry
         WHERE (workspace_id IS NULL OR workspace_id = ?1) AND status = 'verified' AND kind IN ('decision', 'lesson')
         ORDER BY rowid"
    ))?
    .query_map([workspace_id], entry_row)?
    .collect()
}

pub(crate) fn load_entry(conn: &Connection, id: &str) -> rusqlite::Result<Option<KnowledgeEntry>> {
    conn.query_row(&format!("SELECT {ENTRY_COLUMNS} FROM knowledge_entry WHERE id = ?1"), [id], entry_row)
        .map(Some)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(other),
        })
}

/// 寫回一次變更：先寫條目（含索引），再追加版本（參照條目）與事件。回傳第一個條目（主要對象）。
fn save_change(tx: &Transaction<'_>, change: KnowledgeChange, full_text: bool) -> StoreResult<KnowledgeEntry> {
    for entry in &change.entries {
        save_entry(tx, entry, full_text)?;
    }
    if let Some((entry_id, version)) = &change.new_version {
        tx.execute(
            "INSERT INTO knowledge_entry_version (entry_id, version, content, note, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                entry_id,
                version.version,
                serde_json::to_string(&version.content).expect("內容一定可以序列化"),
                version.note,
                version.created_at.as_millis(),
            ],
        )?;
    }
    for event in &change.events {
        append_event(tx, event)?;
    }
    Ok(change.entries.into_iter().next().expect("每個變更至少有一個條目"))
}

fn save_entry(tx: &Transaction<'_>, entry: &KnowledgeEntry, full_text: bool) -> rusqlite::Result<()> {
    let search_text = entry.content.search_text();
    tx.execute(
        "INSERT INTO knowledge_entry (id, kind, scope_kind, workspace_id, goal_id, task_id, status, source, valid_from,
            valid_until, version, revision, supersedes, superseded_by, status_reason, search_text, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)
         ON CONFLICT(id) DO UPDATE SET
           status = excluded.status, valid_until = excluded.valid_until, version = excluded.version,
           revision = excluded.revision, superseded_by = excluded.superseded_by,
           status_reason = excluded.status_reason, search_text = excluded.search_text, updated_at = excluded.updated_at",
        rusqlite::params![
            entry.id,
            entry.kind.as_str(),
            entry.scope.kind.as_str(),
            entry.scope.workspace_id,
            entry.scope.goal_id,
            entry.scope.task_id,
            entry.status.as_str(),
            serde_json::to_string(&entry.source).expect("來源一定可以序列化"),
            entry.valid_from.as_millis(),
            entry.valid_until.map(|t| t.as_millis()),
            entry.version,
            entry.revision,
            entry.supersedes,
            entry.superseded_by,
            entry.status_reason,
            search_text,
            entry.created_at.as_millis(),
            entry.updated_at.as_millis(),
        ],
    )?;
    if full_text {
        tx.execute("DELETE FROM knowledge_fts WHERE entry_id = ?1", [&entry.id])?;
        tx.execute("INSERT INTO knowledge_fts (entry_id, content) VALUES (?1, ?2)", [&entry.id, &search_text])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
