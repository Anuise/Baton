//! 互動式 Controller Run 的持久化（desktop-controller-cli 票 03）。
//!
//! 與 Controller 的一輪共用 `controller_run` 資料表（`kind = 'cli'`），所以 Controller 憑證、裁定請求、訊息等
//! 既有以 `controller_run` 為範圍的機制直接適用：Run 進入 `ended` 時，以它簽發的 Controller 憑證隨之失效。
//! 規則都在 Domain（`baton_domain::controller_cli`）；這裡只做「在同一個交易中載入目前狀態 → 交給 Domain 判定 → 寫入 Run 與事件」。

use baton_domain::auth::ControllerScope;
use baton_domain::controller_cli::{self, ControllerCliChange, ControllerCliError, ControllerCliRun, ControllerCliStatus, OpenIds};
use baton_domain::run::ProcessRecord;
use rusqlite::OptionalExtension;

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
     end_reason, exit_code";

impl Store {
    /// Controller 憑證是否仍然有效（票 17）：它所屬的 Controller CLI 屬於這個 Workspace，而且還沒結束（`starting`／`running`）。
    /// 舊的 Controller 輪次（`kind = 'turn'`）已不再啟動（票 08）；升級前留下的列即使停在未結束，也不再讓憑證有效。
    pub async fn controller_run_active(&self, scope: &ControllerScope) -> StoreResult<bool> {
        let scope = scope.clone();
        self.read(move |conn| {
            let status: Option<String> = conn
                .query_row(
                    "SELECT status FROM controller_run WHERE id = ?1 AND workspace_id = ?2 AND kind = 'cli'",
                    [&scope.controller_run_id, &scope.workspace_id],
                    |r| r.get(0),
                )
                .optional()?;
            Ok(matches!(status.as_deref(), Some("starting" | "running")))
        })
        .await
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
                let settings_version: Option<i64> =
                    tx.query_row("SELECT MAX(version) FROM shared_settings_version", [], |r| r.get(0))?;
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

    /// 寫下 launch intent（開窗之前）；Profile 已不可用時改為結束。
    pub async fn begin_controller_cli_launch(&self, run_id: &str, now: Timestamp) -> Result<ControllerCliRun, ControllerCliApplyError> {
        self.apply_cli(run_id, move |tx, run| Ok(controller_cli::begin_launch(run, &load_profiles(tx)?, now)), now).await
    }

    /// 開窗回執。
    pub async fn record_controller_cli_receipt(
        &self,
        run_id: &str,
        host: ProcessRecord,
        now: Timestamp,
    ) -> Result<ControllerCliRun, ControllerCliApplyError> {
        self.apply_cli(run_id, move |_, run| Ok(controller_cli::record_receipt(run, host, now)), now).await
    }

    /// 結束（宿主消失、使用者結束、開窗失敗或結果不明）。
    pub async fn end_controller_cli(
        &self,
        run_id: &str,
        code: &str,
        reason: &str,
        exit_code: Option<i32>,
        now: Timestamp,
    ) -> Result<ControllerCliRun, ControllerCliApplyError> {
        let (code, reason) = (code.to_string(), reason.to_string());
        self.apply_cli(run_id, move |_, run| Ok(controller_cli::end(run, &code, &reason, exit_code, now)), now).await
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
        self.apply_cli(&run_id, move |_, run| Ok(controller_cli::host_started(run, claude_pid, now)), now).await
    }

    /// 宿主回報 `claude` 已結束：Run 結束，憑證隨之失效。
    pub async fn controller_cli_host_ended(
        &self,
        scope: &ControllerScope,
        exit_code: Option<i32>,
        now: Timestamp,
    ) -> Result<ControllerCliRun, ControllerCliApplyError> {
        let run_id = scoped_run(self, scope).await?;
        let reason = match exit_code {
            Some(code) => format!("Controller CLI 已在終端機結束（結束碼 {code}）"),
            None => "Controller CLI 已在終端機結束".to_string(),
        };
        self.apply_cli(
            &run_id,
            move |_, run| Ok(controller_cli::end(run, controller_cli::END_HOST_EXITED, &reason, exit_code, now)),
            now,
        )
        .await
    }

    async fn apply_cli(
        &self,
        run_id: &str,
        decide: impl FnOnce(&Transaction<'_>, &ControllerCliRun) -> rusqlite::Result<Result<ControllerCliChange, ControllerCliError>>
            + Send
            + 'static,
        now: Timestamp,
    ) -> Result<ControllerCliRun, ControllerCliApplyError> {
        let run_id = run_id.to_string();
        let result = self
            .write(move |tx| {
                let Some(run) = load(tx, "id = ?1", [&run_id])?.pop() else {
                    return Ok(Err(ControllerCliError::RunNotFound(run_id)));
                };
                match decide(tx, &run)? {
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
}

/// 憑證範圍所指的 Controller Run 必須是這個 Workspace 的 Controller CLI（Controller 的一輪不能冒用宿主的端點）。
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
             ended_at, end_code, end_reason, exit_code)
         VALUES (?1, 'cli', ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21)
         ON CONFLICT(id) DO UPDATE SET
           status = excluded.status, version = excluded.version, updated_at = excluded.updated_at,
           launched_at = excluded.launched_at, pid = excluded.pid, process_created_at = excluded.process_created_at,
           command_hash = excluded.command_hash, claude_pid = excluded.claude_pid, started_at = excluded.started_at,
           end_requested_at = excluded.end_requested_at, ended_at = excluded.ended_at, end_code = excluded.end_code,
           end_reason = excluded.end_reason, exit_code = excluded.exit_code",
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
        ],
    )?;
    for event in &change.events {
        append_event(tx, event)?;
    }
    Ok(())
}
