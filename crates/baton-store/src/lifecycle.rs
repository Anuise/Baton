//! 桌面生命週期與完全退出的持久化（票 25），以及跨 Windows 登入的啟動與續跑（票 33）。
//!
//! 規則都在 Domain（`baton_domain::lifecycle`）；這裡只做「在同一個交易中載入目前狀態 → 交給 Domain 判定 →
//! 寫入新狀態與事件」，以及生命週期檔（停止意圖的鏡像，spec D-25）的原子讀寫。
//! 派工與啟動程序的阻擋（`halt_for`、`launch_halted_in`）由排程與 Run 命令在各自的交易中呼叫。

use std::io::Write as _;

use baton_domain::lifecycle::{
    self, ExitReport, ExitTiming, FileStopView, LifecycleChange, LifecycleError, LifecycleFile, LifecyclePhase,
    LifecycleState, PauseCause, WorkspaceLifecycle, RUN_STOPPED_BY_EXIT_REASON,
};
use baton_domain::login::{self, LoginBoundary, LoginStartup};
use baton_domain::run::{RunChange, RunCommand, RunError, RunStatus};
use baton_domain::scheduler::WaitReason;
use baton_domain::task::TaskStatus;

use super::runs::{decide_in, save, RunApplyError};
use super::tasks::load;
use super::*;

/// 生命週期檔名，位於資料目錄之下。
pub const LIFECYCLE_FILE_NAME: &str = baton_domain::lifecycle::LIFECYCLE_FILE_NAME;

/// 生命週期總覽的資料：Runtime 的狀態、使用中的 Workspace 與各自的設定、尚未結束的 Run 數。
#[derive(Debug, Clone, PartialEq)]
pub struct LifecycleSnapshot {
    pub state: LifecycleState,
    /// 「登入時自動啟動」的偏好（票 33）。
    pub login_startup: LoginStartup,
    pub workspaces: Vec<(Workspace, WorkspaceLifecycle)>,
    pub active_runs: usize,
}

/// 變更 Workspace 生命週期設定的失敗。
#[derive(Debug, thiserror::Error)]
pub enum LifecycleApplyError {
    #[error("找不到 Workspace {0}")]
    WorkspaceNotFound(String),
    #[error(transparent)]
    Rejected(#[from] LifecycleError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl From<rusqlite::Error> for LifecycleApplyError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error.into())
    }
}

/// Workspace 名稱、套用後的設定，以及停止意圖是否仍生效（`awaiting_resume` 的判斷用）。
pub type WorkspaceLifecycleOutcome = (String, WorkspaceLifecycle, bool);

impl Store {
    pub async fn lifecycle_state(&self) -> StoreResult<LifecycleState> {
        self.read(|conn| Ok(load_lifecycle(conn)?)).await
    }

    /// 生命週期總覽：只列使用中的 Workspace（依建立順序）。
    pub async fn lifecycle_snapshot(&self) -> StoreResult<LifecycleSnapshot> {
        self.read(|conn| {
            let state = load_lifecycle(conn)?;
            let all = load_workspace_lifecycles(conn)?;
            let workspaces = load_workspaces(conn)?
                .into_iter()
                .filter(|w| w.status == WorkspaceStatus::Active)
                .map(|w| {
                    let lifecycle = lifecycle::workspace_lifecycle_of(&all, &w.id);
                    (w, lifecycle)
                })
                .collect();
            let active_runs: i64 = conn.query_row(
                "SELECT COUNT(*) FROM run WHERE status IN ('queued', 'starting', 'running')",
                [],
                |r| r.get(0),
            )?;
            Ok(LifecycleSnapshot {
                state,
                login_startup: load_login_startup(conn)?,
                workspaces,
                active_runs: active_runs as usize,
            })
        })
        .await
    }

    /// 持久化完全退出的停止意圖，並暫停每個使用中的 Workspace（同一個交易）。已經在停止中時不寫入任何東西。
    pub async fn request_exit(&self, timing: ExitTiming, now: Timestamp) -> StoreResult<LifecycleState> {
        self.write(move |tx| {
            let state = load_lifecycle(tx)?;
            let change =
                lifecycle::request_exit(&state, &load_workspaces(tx)?, &load_workspace_lifecycles(tx)?, timing, now);
            apply_change(tx, &change)?;
            Ok(change.state.unwrap_or(state))
        })
        .await
    }

    /// 停止意圖只存在於生命週期檔（資料庫沒有）時，保守地補記成「已經完全退出」：暫停每個使用中的 Workspace，
    /// 之後與一般的重新開啟相同，先核對再依設定續跑（spec D-25）。資料庫已經是停止時不寫入任何東西。
    pub async fn adopt_file_stop(&self, timing: ExitTiming, now: Timestamp) -> StoreResult<LifecycleState> {
        self.write(move |tx| {
            let state = load_lifecycle(tx)?;
            if state.stop_intent_active() {
                return Ok(state);
            }
            let change =
                lifecycle::request_exit(&state, &load_workspaces(tx)?, &load_workspace_lifecycles(tx)?, timing, now);
            apply_change(tx, &change)?;
            let requested = change.state.unwrap_or(state);
            let mut report = requested.last_exit.clone().expect("要求退出一定有報告");
            report.notes.push("停止意圖只記錄在生命週期檔中（資料庫沒有），依較保守者視為已完全退出".to_string());
            match lifecycle::complete_exit(&requested, report, now) {
                Some((stopped, event)) => {
                    save_lifecycle(tx, &stopped)?;
                    append_event(tx, &event)?;
                    Ok(stopped)
                }
                None => Ok(requested),
            }
        })
        .await
    }

    /// 更新停止進度（每個 Run 的處理結果）；不是停止中時不寫入。
    pub async fn record_exit_progress(&self, report: ExitReport, now: Timestamp) -> StoreResult<()> {
        self.write(move |tx| {
            if let Some(next) = lifecycle::record_progress(&load_lifecycle(tx)?, report, now) {
                save_lifecycle(tx, &next)?;
            }
            Ok(())
        })
        .await
    }

    /// 停止流程結束：記下最終結果（含停止異常）並進入 `stopped`。
    pub async fn complete_exit(&self, report: ExitReport, now: Timestamp) -> StoreResult<LifecycleState> {
        self.write(move |tx| {
            let state = load_lifecycle(tx)?;
            match lifecycle::complete_exit(&state, report, now) {
                Some((next, event)) => {
                    save_lifecycle(tx, &next)?;
                    append_event(tx, &event)?;
                    Ok(next)
                }
                None => Ok(state),
            }
        })
        .await
    }

    /// 重新開啟（核對完程序之後）：解除停止意圖，只有開啟自動續跑的 Workspace 解除暫停。
    /// 啟動後又有新的退出要求（版本不是 `expected_version`）時不重新開啟，回傳 `false`。
    pub async fn reopen_after_exit(&self, expected_version: i64, now: Timestamp) -> StoreResult<bool> {
        self.write(move |tx| {
            let state = load_lifecycle(tx)?;
            match lifecycle::reopen(&state, expected_version, &load_workspace_lifecycles(tx)?, now) {
                Some(change) => {
                    apply_change(tx, &change)?;
                    Ok(true)
                }
                None => Ok(false),
            }
        })
        .await
    }

    /// 變更「完全退出後自動續跑」；只影響下次重新開啟，不會啟動或停止任何工作。
    pub async fn set_auto_resume(
        &self,
        workspace_id: &str,
        enabled: bool,
        now: Timestamp,
    ) -> Result<WorkspaceLifecycleOutcome, LifecycleApplyError> {
        let workspace_id = workspace_id.to_string();
        let outcome = self
            .write(move |tx| {
                let Some(workspace) = active_workspace(tx, &workspace_id)? else {
                    return Ok(Err(LifecycleApplyError::WorkspaceNotFound(workspace_id)));
                };
                let current = lifecycle::workspace_lifecycle_of(&load_workspace_lifecycles(tx)?, &workspace_id);
                let next = match lifecycle::set_auto_resume(&current, enabled, now) {
                    Some((next, event)) => {
                        save_workspace_lifecycle(tx, &next)?;
                        append_event(tx, &event)?;
                        next
                    }
                    None => current,
                };
                Ok(Ok((workspace.name, next, load_lifecycle(tx)?.stop_intent_active())))
            })
            .await?;
        outcome
    }

    /// 變更「跨登入自動續跑」（票 33）；只影響之後的跨登入啟動，不會啟動或停止任何工作，也不解除暫停。
    pub async fn set_login_resume(
        &self,
        workspace_id: &str,
        enabled: bool,
        now: Timestamp,
    ) -> Result<WorkspaceLifecycleOutcome, LifecycleApplyError> {
        let workspace_id = workspace_id.to_string();
        let outcome = self
            .write(move |tx| {
                let Some(workspace) = active_workspace(tx, &workspace_id)? else {
                    return Ok(Err(LifecycleApplyError::WorkspaceNotFound(workspace_id)));
                };
                let current = lifecycle::workspace_lifecycle_of(&load_workspace_lifecycles(tx)?, &workspace_id);
                let next = match login::set_login_resume(&current, enabled, now) {
                    Some((next, event)) => {
                        save_workspace_lifecycle(tx, &next)?;
                        append_event(tx, &event)?;
                        next
                    }
                    None => current,
                };
                Ok(Ok((workspace.name, next, load_lifecycle(tx)?.stop_intent_active())))
            })
            .await?;
        outcome
    }

    /// 「登入時自動啟動」的偏好（票 33）。
    pub async fn login_startup(&self) -> StoreResult<LoginStartup> {
        self.read(|conn| Ok(load_login_startup(conn)?)).await
    }

    /// 變更「登入時自動啟動」的偏好（票 33）；相同值不寫入。Windows 的登記由呼叫端先完成。
    pub async fn set_login_startup(&self, enabled: bool, now: Timestamp) -> StoreResult<LoginStartup> {
        self.write(move |tx| {
            let current = load_login_startup(tx)?;
            match login::set_login_startup(&current, enabled, now) {
                Some((next, event)) => {
                    tx.execute(
                        "INSERT INTO login_startup (id, enabled, version, updated_at) VALUES (1, ?1, ?2, ?3)
                         ON CONFLICT(id) DO UPDATE SET enabled = excluded.enabled, version = excluded.version,
                           updated_at = excluded.updated_at",
                        rusqlite::params![next.enabled, next.version, now.as_millis()],
                    )?;
                    append_event(tx, &event)?;
                    Ok(next)
                }
                None => Ok(current),
            }
        })
        .await
    }

    /// 記下這一次啟動（`record_boot` 剛寫入的那一列）的登入識別，並與上一次啟動比較（票 33）。
    pub async fn note_login_session(&self, current: Option<String>) -> StoreResult<LoginBoundary> {
        self.write(move |tx| {
            let previous = tx.query_row(
                "SELECT login_session FROM runtime_boot ORDER BY id DESC LIMIT 1 OFFSET 1",
                [],
                |r| r.get::<_, Option<String>>(0),
            );
            let previous = match previous {
                Ok(id) => Some(id),
                Err(rusqlite::Error::QueryReturnedNoRows) => None,
                Err(other) => return Err(other.into()),
            };
            tx.execute(
                "UPDATE runtime_boot SET login_session = ?1 WHERE id = (SELECT MAX(id) FROM runtime_boot)",
                [current.as_deref()],
            )?;
            Ok(login::login_boundary(previous.as_ref().map(Option::as_deref), current.as_deref()))
        })
        .await
    }

    /// 依登入邊界處置各 Workspace（票 33）：跨了登入時暫停沒有開啟「跨登入自動續跑」的 Workspace，
    /// 無法核對時全部暫停；停止意圖生效時不做任何事。回傳這次被暫停的 Workspace 與原因。
    pub async fn apply_login_boundary(
        &self,
        boundary: LoginBoundary,
        now: Timestamp,
    ) -> StoreResult<Vec<(String, PauseCause)>> {
        self.write(move |tx| {
            let change = login::cross_login(
                &load_lifecycle(tx)?,
                boundary,
                &load_workspaces(tx)?,
                &load_workspace_lifecycles(tx)?,
                now,
            );
            apply_change(tx, &change)?;
            Ok(change.workspaces.iter().filter_map(|w| Some((w.workspace_id.clone(), w.paused_by?))).collect())
        })
        .await
    }

    /// 「繼續工作」：解除這個 Workspace 在完全退出後的暫停。停止意圖仍生效時拒絕。
    pub async fn resume_workspace(
        &self,
        workspace_id: &str,
        now: Timestamp,
    ) -> Result<WorkspaceLifecycleOutcome, LifecycleApplyError> {
        let workspace_id = workspace_id.to_string();
        let outcome = self
            .write(move |tx| {
                let Some(workspace) = active_workspace(tx, &workspace_id)? else {
                    return Ok(Err(LifecycleApplyError::WorkspaceNotFound(workspace_id)));
                };
                let state = load_lifecycle(tx)?;
                let current = lifecycle::workspace_lifecycle_of(&load_workspace_lifecycles(tx)?, &workspace_id);
                let next = match lifecycle::resume_workspace(&state, &current, now) {
                    Err(rejected) => return Ok(Err(rejected.into())),
                    Ok(Some((next, event))) => {
                        save_workspace_lifecycle(tx, &next)?;
                        append_event(tx, &event)?;
                        next
                    }
                    Ok(None) => current,
                };
                Ok(Ok((workspace.name, next, state.stop_intent_active())))
            })
            .await?;
        outcome
    }

    /// 完全退出要停下的 Run：全部尚未結束的 Run（跨 Workspace，依建立順序）與當初派工時固定的啟動資訊。
    pub async fn runs_to_stop_for_exit(&self) -> StoreResult<Vec<PendingLaunch>> {
        self.read(|conn| Ok(super::runs::load_unfinished_with_payload(conn)?)).await
    }

    /// 完全退出停下了一個 Run（程序確認停止，或根本沒有程序）：Run 以 `interrupted` 結束、釋放寫入所有權，
    /// Task 退回 `ready` 並在同一個交易中重新排入派工佇列，等重新開啟後依 Workspace 設定續跑。
    /// 使用者先前已要求取消的 Run 則完成取消（Task 不會被重新排隊）。Task 不會被標為取消或完成。
    pub async fn stop_run_for_exit(
        &self,
        workspace_id: &str,
        run_id: &str,
        now: Timestamp,
    ) -> Result<RunChange, RunApplyError> {
        self.stop_run_and_requeue(workspace_id, run_id, RUN_STOPPED_BY_EXIT_REASON, now).await
    }

    /// 程序已確認停止的 Run：以 `reason` 中斷並讓 Task 重新排隊；已要求取消的則完成取消（完全退出與崩潰恢復共用）。
    pub(crate) async fn stop_run_and_requeue(
        &self,
        workspace_id: &str,
        run_id: &str,
        reason: &'static str,
        now: Timestamp,
    ) -> Result<RunChange, RunApplyError> {
        let (workspace_id, run_id) = (workspace_id.to_string(), run_id.to_string());
        let outcome = self
            .write(move |tx| {
                let loaded = load(tx, &workspace_id)?;
                let Some(run) = loaded.runs.iter().find(|r| r.id == run_id) else {
                    return Ok(Err(RunError::RunNotFound(run_id)));
                };
                let cancelling = run.cancel_requested_at.is_some() && run.status != RunStatus::Queued;
                let command = if cancelling {
                    RunCommand::ConfirmCancelled { run_id: run_id.clone() }
                } else {
                    RunCommand::Interrupt { run_id: run_id.clone(), reason: reason.to_string() }
                };
                let change = match decide_in(&loaded, &workspace_id, &loaded.tasks, command, now) {
                    Ok(change) => change,
                    Err(rejected) => return Ok(Err(rejected)),
                };
                save(tx, &change, now)?;
                if let Some(task) = change.task.as_ref().filter(|t| !cancelling && t.status == TaskStatus::Ready) {
                    tx.execute(
                        "INSERT INTO dispatch_request (task_id, workspace_id, priority, requested_at)
                         VALUES (?1, ?2, 0, ?3) ON CONFLICT(task_id) DO NOTHING",
                        rusqlite::params![task.id, workspace_id, now.as_millis()],
                    )?;
                }
                Ok(Ok(change))
            })
            .await?;
        Ok(outcome?)
    }
}

// ---------------------------------------------------------------- 阻擋判定（在呼叫端的交易中）

/// 這個 Workspace 的派工現在是否被生命週期擋住。
pub(crate) fn halt_for(conn: &Connection, workspace: Option<&Workspace>, workspace_id: &str) -> rusqlite::Result<Option<WaitReason>> {
    let state = load_lifecycle(conn)?;
    let lifecycle = lifecycle::workspace_lifecycle_of(&load_workspace_lifecycles(conn)?, workspace_id);
    let name = workspace.map(|w| w.name.as_str()).unwrap_or(workspace_id);
    match lifecycle::dispatch_halt(&state, &lifecycle, name) {
        Some(halt) => Ok(Some(halt)),
        // 崩潰恢復尚未完成或已用盡時同樣不派工（票 31）
        None => super::recovery::recovery_halt(conn),
    }
}

/// 啟動程序前的最後一道檢查。
pub(crate) fn launch_halted_in(conn: &Connection, workspace_id: &str) -> rusqlite::Result<bool> {
    let state = load_lifecycle(conn)?;
    let lifecycle = lifecycle::workspace_lifecycle_of(&load_workspace_lifecycles(conn)?, workspace_id);
    Ok(lifecycle::launch_halted(&state, &lifecycle) || super::recovery::recovery_halt(conn)?.is_some())
}

// ---------------------------------------------------------------- 讀寫

fn active_workspace(conn: &Connection, workspace_id: &str) -> rusqlite::Result<Option<Workspace>> {
    Ok(load_workspaces(conn)?.into_iter().find(|w| w.id == workspace_id && w.status == WorkspaceStatus::Active))
}

pub(crate) fn apply_change(tx: &Transaction<'_>, change: &LifecycleChange) -> rusqlite::Result<()> {
    if let Some(state) = &change.state {
        save_lifecycle(tx, state)?;
    }
    for workspace in &change.workspaces {
        save_workspace_lifecycle(tx, workspace)?;
    }
    for event in &change.events {
        append_event(tx, event)?;
    }
    Ok(())
}

pub(crate) fn load_lifecycle(conn: &Connection) -> rusqlite::Result<LifecycleState> {
    let row = conn.query_row(
        "SELECT phase, version, last_exit, updated_at FROM runtime_lifecycle WHERE id = 1",
        [],
        |r| {
            let phase: String = r.get(0)?;
            let last_exit: Option<String> = r.get(2)?;
            Ok(LifecycleState {
                phase: LifecyclePhase::parse(&phase).ok_or_else(|| {
                    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, "未知的生命週期階段".into())
                })?,
                version: r.get(1)?,
                last_exit: last_exit
                    .map(|raw| serde_json::from_str(&raw))
                    .transpose()
                    .map_err(|e| rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, Box::new(e)))?,
                updated_at: Some(Timestamp::from_millis(r.get(3)?)),
            })
        },
    );
    match row {
        Ok(state) => Ok(state),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(LifecycleState::default()),
        Err(other) => Err(other),
    }
}

fn save_lifecycle(tx: &Transaction<'_>, state: &LifecycleState) -> rusqlite::Result<()> {
    // Domain 型別只含字串與數字，序列化不會失敗
    let last_exit = state.last_exit.as_ref().map(|r| serde_json::to_string(r).expect("停止報告可序列化"));
    tx.execute(
        "INSERT INTO runtime_lifecycle (id, phase, version, last_exit, updated_at) VALUES (1, ?1, ?2, ?3, ?4)
         ON CONFLICT(id) DO UPDATE SET phase = excluded.phase, version = excluded.version,
           last_exit = excluded.last_exit, updated_at = excluded.updated_at",
        rusqlite::params![
            state.phase.as_str(),
            state.version,
            last_exit,
            state.updated_at.map(|t| t.as_millis()).unwrap_or_default()
        ],
    )?;
    Ok(())
}

pub(crate) fn load_workspace_lifecycles(conn: &Connection) -> rusqlite::Result<Vec<WorkspaceLifecycle>> {
    conn.prepare(
        "SELECT workspace_id, auto_resume_after_exit, paused_since, version, updated_at, auto_resume_across_login,
           paused_by
         FROM workspace_lifecycle ORDER BY rowid",
    )?
    .query_map([], |r| {
        let paused_by: Option<String> = r.get(6)?;
        Ok(WorkspaceLifecycle {
            workspace_id: r.get(0)?,
            auto_resume_after_exit: r.get(1)?,
            auto_resume_across_login: r.get(5)?,
            paused_since: r.get::<_, Option<i64>>(2)?.map(Timestamp::from_millis),
            paused_by: paused_by
                .map(|raw| {
                    PauseCause::parse(&raw).ok_or_else(|| {
                        rusqlite::Error::FromSqlConversionFailure(6, rusqlite::types::Type::Text, "未知的暫停原因".into())
                    })
                })
                .transpose()?,
            version: r.get(3)?,
            updated_at: Some(Timestamp::from_millis(r.get(4)?)),
        })
    })?
    .collect()
}

fn save_workspace_lifecycle(tx: &Transaction<'_>, lifecycle: &WorkspaceLifecycle) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO workspace_lifecycle (workspace_id, auto_resume_after_exit, paused_since, version, updated_at,
           auto_resume_across_login, paused_by)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(workspace_id) DO UPDATE SET auto_resume_after_exit = excluded.auto_resume_after_exit,
           paused_since = excluded.paused_since, version = excluded.version, updated_at = excluded.updated_at,
           auto_resume_across_login = excluded.auto_resume_across_login, paused_by = excluded.paused_by",
        rusqlite::params![
            lifecycle.workspace_id,
            lifecycle.auto_resume_after_exit,
            lifecycle.paused_since.map(|t| t.as_millis()),
            lifecycle.version,
            lifecycle.updated_at.map(|t| t.as_millis()).unwrap_or_default(),
            lifecycle.auto_resume_across_login,
            lifecycle.paused_by.map(PauseCause::as_str)
        ],
    )?;
    Ok(())
}

fn load_login_startup(conn: &Connection) -> rusqlite::Result<LoginStartup> {
    let row = conn.query_row("SELECT enabled, version, updated_at FROM login_startup WHERE id = 1", [], |r| {
        Ok(LoginStartup { enabled: r.get(0)?, version: r.get(1)?, updated_at: Some(Timestamp::from_millis(r.get(2)?)) })
    });
    match row {
        Ok(startup) => Ok(startup),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(LoginStartup::default()),
        Err(other) => Err(other),
    }
}

// ---------------------------------------------------------------- 生命週期檔

/// 生命週期檔的位置。
pub fn lifecycle_file_path(data_dir: &Path) -> PathBuf {
    data_dir.join(LIFECYCLE_FILE_NAME)
}

/// 原子寫入生命週期檔：先寫暫存檔並 fsync，再改名取代（Windows 上改名會取代既有檔案）。
pub fn write_lifecycle_file(data_dir: &Path, file: &LifecycleFile) -> std::io::Result<()> {
    // 每次寫入用自己的暫存檔：API 與 Supervisor 可能同時寫入，共用暫存檔會互相覆寫或被對方先改名
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let sequence = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let target = lifecycle_file_path(data_dir);
    let temp = data_dir.join(format!("{LIFECYCLE_FILE_NAME}.{}-{sequence}.tmp", std::process::id()));
    let bytes = serde_json::to_vec_pretty(file).map_err(std::io::Error::other)?;
    let written = (|| {
        let mut out = std::fs::File::create(&temp)?;
        out.write_all(&bytes)?;
        out.sync_all()?;
        drop(out);
        // 防毒、索引器等外部程式可能正短暫開著目標檔或暫存檔且不允許刪除共享，改名取代會暫時被拒
        retry_transient(|error| is_sharing_conflict(error).then_some(SHARING_RETRY), || std::fs::rename(&temp, &target))
    })();
    // 暫存檔名每次不同，失敗時要自己清掉，不能等下一次寫入覆寫
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written
}

/// 讀取生命週期檔。檔案存在卻解析不了（毀損）時保守地視為停止（spec D-25）。
pub fn read_lifecycle_file(data_dir: &Path) -> FileStopView {
    // 寫入者正在改名取代的瞬間，開檔可能短暫「存取被拒」（舊檔待刪）或「找不到檔案」，不能因此誤判成停止或沒有檔案。
    // 「找不到檔案」的上限短：檔案確實不存在時只多花這段時間
    let budget = |error: &std::io::Error| {
        if is_sharing_conflict(error) {
            Some(SHARING_RETRY)
        } else {
            (error.kind() == std::io::ErrorKind::NotFound).then_some(std::time::Duration::from_millis(50))
        }
    };
    match retry_transient(budget, || std::fs::read(lifecycle_file_path(data_dir))) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => FileStopView::Missing,
        Err(_) => FileStopView::Stop { requested_at: None },
        Ok(bytes) => match serde_json::from_slice::<LifecycleFile>(&bytes) {
            Ok(LifecycleFile { stop: Some(stop), .. }) => FileStopView::Stop { requested_at: Some(stop.requested_at) },
            Ok(_) => FileStopView::Clear,
            Err(_) => FileStopView::Stop { requested_at: None },
        },
    }
}

/// 開檔／改名衝突的重試上限。
const SHARING_RETRY: std::time::Duration = std::time::Duration::from_secs(2);

/// 重試暫時性的失敗：`budget` 回傳這種錯誤從第一次嘗試起最多重試多久（`None` 表示不重試）。
/// 間隔由 1ms 倍增、最多 50ms；超過上限回傳最後一次的結果。CPU 滿載時睡眠會被拉長，
/// 只看時間可能只試一兩次，所以至少嘗試 `MIN_ATTEMPTS` 次。
fn retry_transient<T>(
    budget: impl Fn(&std::io::Error) -> Option<std::time::Duration>,
    mut attempt: impl FnMut() -> std::io::Result<T>,
) -> std::io::Result<T> {
    const MIN_ATTEMPTS: u32 = 5;
    let started = std::time::Instant::now();
    let mut delay = std::time::Duration::from_millis(1);
    let mut attempts = 0;
    loop {
        attempts += 1;
        match attempt() {
            Err(error) if budget(&error).is_some_and(|limit| attempts < MIN_ATTEMPTS || started.elapsed() < limit) => {
                std::thread::sleep(delay);
                delay = (delay * 2).min(std::time::Duration::from_millis(50));
            }
            other => return other,
        }
    }
}

/// Windows 的開檔／改名衝突：ERROR_ACCESS_DENIED（5，含舊檔待刪）與 ERROR_SHARING_VIOLATION（32）；其他平台沒有這類衝突。
fn is_sharing_conflict(error: &std::io::Error) -> bool {
    cfg!(windows) && matches!(error.raw_os_error(), Some(5 | 32))
}

#[cfg(test)]
mod tests;
