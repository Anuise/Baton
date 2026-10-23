//! 互動式 Controller Run 的持久化（desktop-controller-cli 票 03）。
//!
//! 與升級前的 Controller 輪次（`kind = 'turn'`）共用 `controller_run` 資料表（`kind = 'cli'`），所以 Controller 憑證、裁定請求、訊息等
//! 既有以 `controller_run` 為範圍的機制直接適用：Run 進入 `ended` 時，以它簽發的 Controller 憑證隨之失效。
//! 規則都在 Domain（`baton_domain::controller_cli`）；這裡只做「在同一個交易中載入目前狀態 → 交給 Domain 判定 → 寫入 Run 與事件」。
//! 已要求換帳號重開的 Run 結束時，在同一個交易中以新 Profile 開新的 Run（票 05），不會有「舊的已結束、新的還沒開」的空檔。
//! 要不要開新的 Run 依交易內讀到的 Run 決定，不依呼叫端的快照：可能讓 Run 結束的命令都由呼叫端備妥新 Run 的識別（`next`），
//! 沒有要求換帳號時不使用。

use baton_domain::auth::ControllerScope;
use baton_domain::controller_cli::{self, ControllerCliChange, ControllerCliError, ControllerCliRun, ControllerCliStatus, OpenIds};
use baton_domain::run::ProcessRecord;

use super::profiles::load_profiles;
use super::*;

/// 套用 Controller CLI 命令的失敗：Domain 拒絕或儲存層錯誤。
#[derive(Debug, thiserror::Error)]
pub enum ControllerCliApplyError {
    #[error(transparent)]
    Rejected(#[from] ControllerCliError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl From<rusqlite::Error> for ControllerCliApplyError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error.into())
    }
}

const COLUMNS: &str = "id, workspace_id, profile_id, status, version, command_id, working_dir, settings_version, created_at, \
     launched_at, pid, process_created_at, command_hash, claude_pid, started_at, end_requested_at, ended_at, end_code, \
     end_reason, exit_code, continue_session, rebound_at, switch_to_profile_id, handoff_from_run_id";

impl Store {
    /// Controller 憑證是否仍然有效（票 17）：它所屬的 Controller CLI 屬於這個 Workspace，而且憑證仍有效
    /// （Domain 的 `credential_valid`：尚未結束、沒有在 Runtime 重啟後被重新綁定，desktop-controller-cli 票 07）。
    /// 舊的 Controller 輪次（`kind = 'turn'`）已不再啟動（票 08）；升級前留下的列即使停在未結束，也不再讓憑證有效。
    pub async fn controller_run_active(&self, scope: &ControllerScope) -> StoreResult<bool> {
        let scope = scope.clone();
        self.read(move |conn| Ok(controller_credential_valid(conn, &scope)?.unwrap_or(false))).await
    }

    /// 使用者要求開啟：在同一個交易中確認 Workspace、Profile 與「這個 Workspace 沒有開著的 Controller CLI」，
    /// 並固定這一刻採用的共同設定版本。
    pub async fn open_controller_cli(
        &self,
        workspace_id: &str,
        profile_id: &str,
        ids: OpenIds,
        now: Timestamp,
    ) -> Result<ControllerCliRun, ControllerCliApplyError> {
        let (workspace_id, profile_id) = (workspace_id.to_string(), profile_id.to_string());
        let result = self
            .write(move |tx| {
                let Some(workspace) = load_workspaces(tx)?.into_iter().find(|w| w.id == workspace_id) else {
                    return Ok(Err(ControllerCliError::WorkspaceNotFound(workspace_id)));
                };
                let profiles = load_profiles(tx)?;
                let runs = load(tx, "workspace_id = ?1", [&workspace_id])?;
                let settings_version = latest_settings_version(tx)?;
                match controller_cli::open(&workspace, &profile_id, &profiles, &runs, settings_version, ids, now) {
                    Err(rejected) => Ok(Err(rejected)),
                    Ok(change) => {
                        save(tx, &change, now)?;
                        Ok(Ok(change.run))
                    }
                }
            })
            .await?;
        Ok(result?)
    }

    /// 這個 Workspace 最近一次的 Controller Run（Controller 頁簽顯示它）。
    pub async fn latest_controller_cli_run(&self, workspace_id: &str) -> StoreResult<Option<ControllerCliRun>> {
        let workspace_id = workspace_id.to_string();
        self.read(move |conn| Ok(load(conn, "workspace_id = ?1", [&workspace_id])?.pop())).await
    }

    /// 尚未結束的 Controller Run（跨 Workspace，依建立順序）；Supervisor 開窗、核對與查驗存活用。
    pub async fn open_controller_cli_runs(&self) -> StoreResult<Vec<ControllerCliRun>> {
        self.read(|conn| Ok(load(conn, "status IN ('starting', 'running')", [])?)).await
    }

    /// 寫下 launch intent（開窗之前）；Profile 已不可用、或使用者已要求結束時改為結束（已要求換帳號時以 `next` 開新的 Run）。
    /// `continuable` 為 false 時不接續原生 Session（見 Domain）。
    pub async fn begin_controller_cli_launch(
        &self,
        run_id: &str,
        continuable: bool,
        next: Option<OpenIds>,
        now: Timestamp,
    ) -> Result<ControllerCliRun, ControllerCliApplyError> {
        self.apply_cli(run_id, move |tx, run| Ok(controller_cli::begin_launch(run, &load_profiles(tx)?, continuable, now)), next, now)
            .await
    }

    /// 開窗回執。
    pub async fn record_controller_cli_receipt(
        &self,
        run_id: &str,
        host: ProcessRecord,
        now: Timestamp,
    ) -> Result<ControllerCliRun, ControllerCliApplyError> {
        self.apply_cli(run_id, move |_, run| Ok(controller_cli::record_receipt(run, host, now)), None, now).await
    }

    /// 結束（宿主消失、使用者結束、開窗失敗或結果不明）。已要求換帳號重開時，同一個交易中以 `next` 開新的 Run。
    pub async fn end_controller_cli(
        &self,
        run_id: &str,
        code: &str,
        reason: &str,
        exit_code: Option<i32>,
        next: Option<OpenIds>,
        now: Timestamp,
    ) -> Result<ControllerCliRun, ControllerCliApplyError> {
        let (code, reason) = (code.to_string(), reason.to_string());
        self.apply_cli(run_id, move |_, run| Ok(controller_cli::end(run, &code, &reason, exit_code, now)), next, now).await
    }

    /// 使用者按「換帳號重開」：記下要改用的 Profile 並要求結束，由 Supervisor 結束宿主；結束時開新的 Run。
    pub async fn request_controller_cli_switch(
        &self,
        workspace_id: &str,
        profile_id: &str,
        now: Timestamp,
    ) -> Result<ControllerCliRun, ControllerCliApplyError> {
        let (workspace_id, profile_id) = (workspace_id.to_string(), profile_id.to_string());
        let result = self
            .write(move |tx| {
                if !load_workspaces(tx)?.iter().any(|w| w.id == workspace_id) {
                    return Ok(Err(ControllerCliError::WorkspaceNotFound(workspace_id)));
                }
                let Some(run) = load(tx, "workspace_id = ?1 AND status IN ('starting', 'running')", [&workspace_id])?.pop() else {
                    return Ok(Err(ControllerCliError::NotOpen));
                };
                match controller_cli::request_switch(&run, &profile_id, &load_profiles(tx)?, now) {
                    Err(rejected) => Ok(Err(rejected)),
                    Ok(change) => {
                        save(tx, &change, now)?;
                        Ok(Ok(change.run))
                    }
                }
            })
            .await?;
        Ok(result?)
    }

    /// Runtime 重啟後查驗宿主已不在：中斷（票 07）。已要求換帳號重開時，同一個交易中以 `next` 開新的 Run（換帳號的要求不因當掉而遺失）。
    pub async fn interrupt_controller_cli(
        &self,
        run_id: &str,
        reason: &str,
        next: Option<OpenIds>,
        now: Timestamp,
    ) -> Result<ControllerCliRun, ControllerCliApplyError> {
        let reason = reason.to_string();
        self.apply_cli(run_id, move |_, run| Ok(controller_cli::interrupt(run, &reason, now)), next, now).await
    }

    /// Runtime 重啟後查驗宿主仍存活：重新綁定，以這個 Run 簽發的 Controller 憑證從此失效（票 07）。已重新綁定過不改變。
    pub async fn rebind_controller_cli(&self, run_id: &str, now: Timestamp) -> Result<ControllerCliRun, ControllerCliApplyError> {
        let run_id = run_id.to_string();
        let result = self
            .write(move |tx| {
                let Some(run) = load(tx, "id = ?1", [&run_id])?.pop() else {
                    return Ok(Err(ControllerCliError::RunNotFound(run_id)));
                };
                match controller_cli::rebind(&run, now) {
                    Err(rejected) => Ok(Err(rejected)),
                    Ok(None) => Ok(Ok(run)),
                    Ok(Some(change)) => {
                        save(tx, &change, now)?;
                        Ok(Ok(change.run))
                    }
                }
            })
            .await?;
        Ok(result?)
    }

    /// 使用者按「結束」：記下要求，由 Supervisor 結束宿主。
    pub async fn request_controller_cli_end(&self, workspace_id: &str, now: Timestamp) -> Result<ControllerCliRun, ControllerCliApplyError> {
        let workspace_id = workspace_id.to_string();
        let result = self
            .write(move |tx| {
                if !load_workspaces(tx)?.iter().any(|w| w.id == workspace_id) {
                    return Ok(Err(ControllerCliError::WorkspaceNotFound(workspace_id)));
                }
                let Some(run) = load(tx, "workspace_id = ?1 AND status IN ('starting', 'running')", [&workspace_id])?.pop() else {
                    return Ok(Err(ControllerCliError::NotOpen));
                };
                match controller_cli::request_end(&run, now) {
                    Err(rejected) => Ok(Err(rejected)),
                    Ok(None) => Ok(Ok(run)),
                    Ok(Some(change)) => {
                        save(tx, &change, now)?;
                        Ok(Ok(change.run))
                    }
                }
            })
            .await?;
        Ok(result?)
    }

    /// 宿主回報 `claude` 已啟動（憑證範圍就是這個 Run）。
    pub async fn controller_cli_host_started(
        &self,
        scope: &ControllerScope,
        claude_pid: u32,
        now: Timestamp,
    ) -> Result<ControllerCliRun, ControllerCliApplyError> {
        let run_id = scoped_run(self, scope).await?;
        self.apply_cli(&run_id, move |_, run| Ok(controller_cli::host_started(run, claude_pid, now)), None, now).await
    }

    /// 宿主回報 `claude` 已結束：Run 結束，憑證隨之失效（已要求換帳號重開時以 `next` 開新的 Run）。
    pub async fn controller_cli_host_ended(
        &self,
        scope: &ControllerScope,
        exit_code: Option<i32>,
        next: Option<OpenIds>,
        now: Timestamp,
    ) -> Result<ControllerCliRun, ControllerCliApplyError> {
        let run_id = scoped_run(self, scope).await?;
        let reason = match exit_code {
            Some(code) => format!("Controller CLI 已在終端機結束（結束碼 {code}）"),
            None => "Controller CLI 已在終端機結束".to_string(),
        };
        let end = move |_: &Transaction<'_>, run: &ControllerCliRun| {
            Ok(controller_cli::end(run, controller_cli::END_HOST_EXITED, &reason, exit_code, now))
        };
        self.apply_cli(&run_id, end, next, now).await
    }

    /// 在一個交易中載入 Run、交給 Domain 判定並寫入。Run 因此結束、而它（交易內讀到的最新狀態）已要求換帳號重開時，
    /// 同一個交易中以 `next` 開新的 Run；`next` 為 `None` 時只結束（例如完全退出）。回傳這個 Run 的新狀態。
    async fn apply_cli(
        &self,
        run_id: &str,
        decide: impl FnOnce(&Transaction<'_>, &ControllerCliRun) -> rusqlite::Result<Result<ControllerCliChange, ControllerCliError>>
            + Send
            + 'static,
        next: Option<OpenIds>,
        now: Timestamp,
    ) -> Result<ControllerCliRun, ControllerCliApplyError> {
        let run_id = run_id.to_string();
        let result = self
            .write(move |tx| {
                let Some(run) = load(tx, "id = ?1", [&run_id])?.pop() else {
                    return Ok(Err(ControllerCliError::RunNotFound(run_id)));
                };
                let changed = match decide(tx, &run)? {
                    Err(rejected) => return Ok(Err(rejected)),
                    Ok(change) => {
                        save(tx, &change, now)?;
                        change.run
                    }
                };
                if let (true, Some(ids)) = (changed.status.is_terminal(), next) {
                    open_switched(tx, &changed, ids, now)?;
                }
                Ok(Ok(changed))
            })
            .await?;
        Ok(result?)
    }
}

/// 從備份還原時，把備份當下尚未結束的 Controller CLI 一律中斷（不開新的 Run）：還原後的 Runtime 不替使用者開窗，
/// 也不沿用備份當下的宿主；使用者要時再自行開啟。回傳中斷的數量。
pub(crate) fn interrupt_open_controller_clis(tx: &Transaction<'_>, reason: &str, now: Timestamp) -> rusqlite::Result<usize> {
    let runs = load(tx, "status IN ('starting', 'running')", [])?;
    for run in &runs {
        let change = controller_cli::interrupt(run, reason, now).expect("尚未結束的 Controller CLI 可以中斷");
        save(tx, &change, now)?;
    }
    Ok(runs.len())
}

/// 已要求換帳號重開的 Run 結束後，以新 Profile 開新的 Run；沒有要求換帳號時不做任何事。
/// Workspace 已封存等無法開新 Run 的情況只結束舊的（頁簽顯示已結束，使用者可再手動開啟）。
fn open_switched(tx: &Transaction<'_>, ended: &ControllerCliRun, ids: OpenIds, now: Timestamp) -> rusqlite::Result<()> {
    let Some(workspace) = load_workspaces(tx)?.into_iter().find(|w| w.id == ended.workspace_id) else { return Ok(()) };
    let profiles = load_profiles(tx)?;
    if let Ok(Some(change)) = controller_cli::open_switched(&workspace, ended, &profiles, latest_settings_version(tx)?, ids, now) {
        save(tx, &change, now)?;
    }
    Ok(())
}

/// 以這個範圍簽發的 Controller 憑證是否仍有效；範圍所指的不是這個 Workspace 的 Controller CLI 時為 `None`。
/// 憑證檢查與「Controller 寄件者是否仍是目前的 Controller」（訊息）共用這一份規則。
pub(crate) fn controller_credential_valid(conn: &Connection, scope: &ControllerScope) -> rusqlite::Result<Option<bool>> {
    let run = load(conn, "id = ?1 AND workspace_id = ?2", [&scope.controller_run_id, &scope.workspace_id])?.pop();
    Ok(run.as_ref().map(controller_cli::credential_valid))
}

/// 憑證範圍所指的 Controller Run 必須是這個 Workspace 的 Controller CLI（升級前的 Controller 輪次不能冒用宿主的端點）。
async fn scoped_run(store: &Store, scope: &ControllerScope) -> Result<String, ControllerCliApplyError> {
    let scope = scope.clone();
    let found = store
        .read(move |conn| {
            Ok(load(conn, "id = ?1 AND workspace_id = ?2", [&scope.controller_run_id, &scope.workspace_id])?
                .pop()
                .map(|run| run.id)
                .ok_or(ControllerCliError::RunNotFound(scope.controller_run_id)))
        })
        .await?;
    Ok(found?)
}

/// 開著的 Controller CLI（`starting`／`running`）的 Controller 帳號，不重複（Scheduler 不派新的 Worker 給它們，票 05）。
pub(crate) fn load_controller_accounts(conn: &Connection) -> rusqlite::Result<Vec<String>> {
    conn.prepare("SELECT DISTINCT profile_id FROM controller_run WHERE kind = 'cli' AND status IN ('starting', 'running') ORDER BY profile_id")?
        .query_map([], |r| r.get(0))?
        .collect()
}

/// 開啟時採用的共同設定版本：目前最新的版本；還沒有任何版本為 `None`。
fn latest_settings_version(conn: &Connection) -> rusqlite::Result<Option<i64>> {
    conn.query_row("SELECT MAX(version) FROM shared_settings_version", [], |r| r.get(0))
}

fn load(conn: &Connection, condition: &str, params: impl rusqlite::Params) -> rusqlite::Result<Vec<ControllerCliRun>> {
    conn.prepare(&format!("SELECT {COLUMNS} FROM controller_run WHERE kind = 'cli' AND ({condition}) ORDER BY rowid"))?
        .query_map(params, row)?
        .collect()
}

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ControllerCliRun> {
    let status: String = r.get(3)?;
    let millis = |index: usize| -> rusqlite::Result<Option<Timestamp>> {
        Ok(r.get::<_, Option<i64>>(index)?.map(Timestamp::from_millis))
    };
    let host = match (r.get::<_, Option<u32>>(10)?, r.get::<_, Option<i64>>(11)?, r.get::<_, Option<String>>(12)?) {
        (Some(pid), Some(created_at_millis), Some(command_hash)) => Some(ProcessRecord { pid, created_at_millis, command_hash }),
        _ => None,
    };
    Ok(ControllerCliRun {
        id: r.get(0)?,
        workspace_id: r.get(1)?,
        profile_id: r.get(2)?,
        status: ControllerCliStatus::parse(&status).ok_or_else(|| {
            rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, "未知的 Controller CLI 狀態".into())
        })?,
        version: r.get(4)?,
        command_id: r.get(5)?,
        working_dir: r.get(6)?,
        settings_version: r.get(7)?,
        opened_at: Timestamp::from_millis(r.get(8)?),
        launched_at: millis(9)?,
        host,
        claude_pid: r.get(13)?,
        running_at: millis(14)?,
        end_requested_at: millis(15)?,
        ended_at: millis(16)?,
        end_code: r.get(17)?,
        end_reason: r.get(18)?,
        exit_code: r.get(19)?,
        continue_session: r.get(20)?,
        rebound_at: millis(21)?,
        switch_to_profile_id: r.get(22)?,
        switched_from_run_id: r.get(23)?,
    })
}

fn save(tx: &Transaction<'_>, change: &ControllerCliChange, now: Timestamp) -> rusqlite::Result<()> {
    let run = &change.run;
    let millis = |t: Option<Timestamp>| t.map(|t| t.as_millis());
    let host = run.host.as_ref();
    tx.execute(
        "INSERT INTO controller_run
            (id, kind, workspace_id, profile_id, status, version, command_id, working_dir, settings_version, created_at,
             updated_at, launched_at, pid, process_created_at, command_hash, claude_pid, started_at, end_requested_at,
             ended_at, end_code, end_reason, exit_code, continue_session, rebound_at, switch_to_profile_id, handoff_from_run_id)
         VALUES (?1, 'cli', ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25)
         ON CONFLICT(id) DO UPDATE SET
           status = excluded.status, version = excluded.version, updated_at = excluded.updated_at,
           launched_at = excluded.launched_at, pid = excluded.pid, process_created_at = excluded.process_created_at,
           command_hash = excluded.command_hash, claude_pid = excluded.claude_pid, started_at = excluded.started_at,
           end_requested_at = excluded.end_requested_at, ended_at = excluded.ended_at, end_code = excluded.end_code,
           end_reason = excluded.end_reason, exit_code = excluded.exit_code, rebound_at = excluded.rebound_at,
           continue_session = excluded.continue_session, switch_to_profile_id = excluded.switch_to_profile_id",
        rusqlite::params![
            run.id,
            run.workspace_id,
            run.profile_id,
            run.status.as_str(),
            run.version,
            run.command_id,
            run.working_dir,
            run.settings_version,
            run.opened_at.as_millis(),
            now.as_millis(),
            millis(run.launched_at),
            host.map(|h| h.pid),
            host.map(|h| h.created_at_millis),
            host.map(|h| h.command_hash.as_str()),
            run.claude_pid,
            millis(run.running_at),
            millis(run.end_requested_at),
            millis(run.ended_at),
            run.end_code,
            run.end_reason,
            run.exit_code,
            run.continue_session,
            millis(run.rebound_at),
            run.switch_to_profile_id,
            run.switched_from_run_id,
        ],
    )?;
    for event in &change.events {
        append_event(tx, event)?;
    }
    Ok(())
}
