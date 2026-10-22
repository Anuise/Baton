//! 額度門檻與交接的持久化（票 28）。
//!
//! 規則都在 Domain（`baton_domain::handoff`、`run`、`scheduler`）；這裡只做「在同一個交易中載入目前狀態 → 交給 Domain
//! 判定 → 寫入」。交接的每一個 phase 各是一個交易，外部副作用（要求舊程序停止、啟動 successor）都在 phase 持久化之後
//! 才由 Supervisor 執行。轉移（步驟 3）是單一交易：舊 Run 結束、successor Run 建立、交接紀錄前進一起提交，
//! 所以任何時刻 Task 最多一個未釋放的 execution lease。
//!
//! 突然限流（票 29）沿用同一套 phase：轉移時所有合適帳號都受限，就在同一個交易中結束舊 Run、讓 Task 進入
//! `waiting_resource` 並排定重新評估（交接停在 `waiting_resource`）；資源恢復時再以單一交易讓 Task 回到 ready、
//! 派出 successor 並把交接轉移給它。

use std::collections::HashMap;

use baton_domain::auth::AgentScope;
use baton_domain::handoff::{
    self, CheckpointRef, GateState, Handoff, HandoffConfirmation, HandoffError, HandoffPhase, QuotaGate,
};
use baton_domain::run::{self, LaunchPayload, Run, RunCommand, RunError, RunStatus};
use baton_domain::scheduler::{self, Capacity, Placement, SlotKind, WaitCategory, WaitReason};
use baton_domain::task::{Task, TaskStatus};

use super::guards::load_active_guards;
use super::lifecycle::halt_for;
use super::profiles::load_profiles;
use super::quota::load_quota_status;
use super::runs::{decide_in, load_active_runs, load_unfinished_with_payload, save, RunApplyError};
use super::scheduling::{failed_check, load_settings};
use super::tasks::{load, Loaded};
use super::*;

/// 交接命令的失敗：Domain 拒絕或儲存層錯誤。
#[derive(Debug, thiserror::Error)]
pub enum HandoffApplyError {
    #[error(transparent)]
    Rejected(#[from] HandoffError),
    #[error(transparent)]
    Run(#[from] RunError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl From<rusqlite::Error> for HandoffApplyError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error.into())
    }
}

impl From<RunApplyError> for HandoffApplyError {
    fn from(error: RunApplyError) -> Self {
        match error {
            RunApplyError::Rejected(rejected) => Self::Run(rejected),
            RunApplyError::Store(store) => Self::Store(store),
        }
    }
}

/// Supervisor 處理一個進行中（requested、stopping）的交接需要的資料。
#[derive(Debug, Clone, PartialEq)]
pub struct HandoffWork {
    pub handoff: Handoff,
    /// 舊 Run 的目前狀態；已由別的路徑結束時也在這裡。
    pub from_run: Run,
    /// 舊 Run 派工時固定的啟動資訊（以 command_id 找回程序用）；舊 Run 已結束時為 `None`。
    pub payload: Option<LaunchPayload>,
    /// Task 最新一筆被接受的 checkpoint。
    pub checkpoint: Option<CheckpointRef>,
}

impl Store {
    /// 依目前的額度判斷推進每個帳號的門檻（遲滯、draining）；回傳有改變的門檻。沒有改變時不寫入。
    pub async fn refresh_quota_gates(&self, now: Timestamp) -> StoreResult<Vec<QuotaGate>> {
        self.write(move |tx| {
            let status = load_quota_status(tx, now)?;
            let current = load_quota_gates(tx)?;
            let mut changed = Vec::new();
            for profile in load_profiles(tx)? {
                let Some(account) = status.get(&profile.id) else { continue };
                if let Some((gate, event)) =
                    handoff::evaluate_gate(&profile.id, current.get(&profile.id), &account.assessment, now)
                {
                    save_gate(tx, &gate)?;
                    append_event(tx, &event)?;
                    changed.push(gate);
                }
            }
            Ok(changed)
        })
        .await
    }

    /// 全部帳號目前的門檻（從未離開 open 的帳號沒有項目）。
    pub async fn quota_gates(&self) -> StoreResult<HashMap<String, QuotaGate>> {
        self.read(|conn| Ok(load_quota_gates(conn)?)).await
    }

    /// 步驟 1：為 draining 帳號上執行中的 Run 建立交接（已選定承接帳號）。沒有承接帳號（全部受限、名額已滿）或 Task 在冷卻期的
    /// Run 留在原帳號繼續執行，下一次再判定。回傳新建立的交接。
    pub async fn request_handoffs(
        &self,
        quota_used: HashMap<String, f64>,
        now: Timestamp,
        mut new_id: impl FnMut() -> Option<String> + Send + 'static,
    ) -> StoreResult<Vec<Handoff>> {
        self.write(move |tx| {
            let gates = load_quota_gates(tx)?;
            if !gates.values().any(|g| g.state == GateState::Draining) {
                return Ok(Vec::new());
            }
            let active = load_active_runs(tx)?;
            let mut handoffs = load_handoffs_where(tx, "1 = 1", [])?;
            let candidates: Vec<(Run, String)> = handoff::candidates(&active, &gates, &handoffs, now)
                .into_iter()
                .map(|(run, reason)| (run.clone(), reason))
                .collect();
            let mut created = Vec::new();
            for (run, reason) in candidates {
                let placement = successor_placement(tx, &run.workspace_id, &quota_used, None, None)?;
                let profile_id = match placement {
                    Placement::Place { profile_id } => profile_id,
                    // 突然限流：舊 Worker 已無法工作，沒有承接帳號也要停下它（之後等待資源）；停止意圖生效時不交接
                    Placement::Wait(reasons) if handoff::is_sudden_limit(&reason) && !halted(&reasons) => String::new(),
                    Placement::Wait(_) => continue,
                };
                let Some(id) = new_id() else { continue };
                let (requested, event) = handoff::request(id, &run, &profile_id, &reason, now);
                save_handoff(tx, &requested)?;
                append_event(tx, &event)?;
                handoffs.push(requested.clone());
                created.push(requested);
            }
            Ok(created)
        })
        .await
    }

    /// 突然限流（票 29）：這個 Run 的帳號已達上限、程序已經結束（Supervisor 收到結束事件），但還沒有交接紀錄時，
    /// 為它建立交接（有承接帳號就選定，沒有則留空），讓 Supervisor 直接轉移或進入等待資源。
    /// 帳號不是突然限流、Run 已不在執行時回傳 `None`；已有進行中的交接時回傳它。
    pub async fn request_sudden_handoff(
        &self,
        run_id: &str,
        quota_used: HashMap<String, f64>,
        now: Timestamp,
        mut new_id: impl FnMut() -> Option<String> + Send + 'static,
    ) -> StoreResult<Option<Handoff>> {
        let run_id = run_id.to_string();
        self.write(move |tx| {
            let active = load_active_runs(tx)?;
            let Some(run) = active.iter().find(|r| r.id == run_id && r.status == RunStatus::Running) else {
                return Ok(None);
            };
            let gates = load_quota_gates(tx)?;
            let sudden = gates.get(&run.profile_id).filter(|g| g.state == GateState::Draining && handoff::is_sudden_limit(&g.reason));
            let Some(gate) = sudden else { return Ok(None) };
            let filter = "from_run_id = ?1 AND phase IN ('requested', 'stopping')";
            if let Some(existing) = load_handoffs_where(tx, filter, [&run.id])?.pop() {
                return Ok(Some(existing));
            }
            let to_profile_id = match successor_placement(tx, &run.workspace_id, &quota_used, None, Some(&run.id))? {
                Placement::Place { profile_id } => profile_id,
                Placement::Wait(_) => String::new(),
            };
            let Some(id) = new_id() else { return Ok(None) };
            let (requested, event) = handoff::request(id, run, &to_profile_id, &gate.reason, now);
            save_handoff(tx, &requested)?;
            append_event(tx, &event)?;
            Ok(Some(requested))
        })
        .await
    }

    /// 等待資源的交接（票 29）：Task 已不在 `waiting_resource`（例如被取消）就放棄；到了重新評估的時間就找承接帳號，
    /// 有就在同一個交易中讓 Task 回到 ready、派出 successor（Prompt 附上最近的有效 checkpoint）並把交接轉移給它，
    /// 沒有就依重置時間或退避排定下一次。停止意圖或 Workspace 等待「繼續工作」時不派工、也不重新排定
    /// （資源恢復不會解除完全退出；解除後的下一次處理就重新評估）。回傳有改變的交接。
    pub async fn resume_resource_waits(
        &self,
        quota_used: HashMap<String, f64>,
        now: Timestamp,
        mut new_run_id: impl FnMut() -> Option<String> + Send + 'static,
    ) -> StoreResult<Vec<Handoff>> {
        self.write(move |tx| {
            let mut changed = Vec::new();
            for current in load_handoffs_where(tx, "phase = 'waiting_resource'", [])? {
                let loaded = load(tx, &current.workspace_id)?;
                let Some(task) = loaded.tasks.iter().find(|t| t.id == current.task_id).cloned() else { continue };
                if task.status != TaskStatus::WaitingResource {
                    let reason = format!("task_left_waiting_resource：{}", task.status.as_str());
                    let Ok((next, event)) = handoff::abandon(&current, &reason, now) else { continue };
                    save_handoff(tx, &next)?;
                    append_event(tx, &event)?;
                    changed.push(next);
                    continue;
                }
                if current.next_check_at.is_some_and(|at| now < at) {
                    continue;
                }
                let reasons = match successor_placement(tx, &current.workspace_id, &quota_used, None, None)? {
                    Placement::Wait(reasons) if halted(&reasons) => continue,
                    Placement::Wait(reasons) => reasons,
                    Placement::Place { profile_id } => match resume_in(tx, &current, &task, profile_id, now, &mut new_run_id)? {
                        Ok(next) => {
                            changed.push(next);
                            continue;
                        }
                        Err(reason) => vec![reason],
                    },
                };
                let resets = blocking_resets(tx)?;
                let Ok((next, event)) = handoff::reschedule_wait(&current, &resets, reasons, now) else { continue };
                save_handoff(tx, &next)?;
                append_event(tx, &event)?;
                changed.push(next);
            }
            Ok(changed)
        })
        .await
    }

    /// 進行中（requested、stopping）的交接，附上舊 Run、它的啟動資訊與 Task 最新的 checkpoint。
    pub async fn in_flight_handoffs(&self) -> StoreResult<Vec<HandoffWork>> {
        self.read(|conn| {
            let in_flight = load_handoffs_where(conn, "phase IN ('requested', 'stopping')", [])?;
            if in_flight.is_empty() {
                return Ok(Vec::new());
            }
            let unfinished = load_unfinished_with_payload(conn)?;
            let mut works = Vec::new();
            let mut loaded_by_workspace: HashMap<String, Loaded> = HashMap::new();
            for handoff in in_flight {
                if !loaded_by_workspace.contains_key(&handoff.workspace_id) {
                    loaded_by_workspace.insert(handoff.workspace_id.clone(), load(conn, &handoff.workspace_id)?);
                }
                let loaded = &loaded_by_workspace[&handoff.workspace_id];
                let Some(from_run) = loaded.runs.iter().find(|r| r.id == handoff.from_run_id).cloned() else { continue };
                let payload = unfinished.iter().find(|p| p.run.id == from_run.id).map(|p| p.payload.clone());
                let checkpoint = handoff::latest_checkpoint(&loaded.reports, &loaded.runs, &handoff.task_id);
                works.push(HandoffWork { handoff, from_run, payload, checkpoint });
            }
            Ok(works)
        })
        .await
    }

    /// 步驟 2：到了安全邊界（要求之後保存了 checkpoint，或等滿上限）而且承接帳號仍有名額，就記下 checkpoint 並進入 `stopping`。
    /// 舊 Run 已由別的路徑結束時放棄交接。回傳有改變的交接；條件還不成立時回傳 `None`（下一次再判定）。
    pub async fn begin_handoff_stop(
        &self,
        id: &str,
        quota_used: HashMap<String, f64>,
        now: Timestamp,
    ) -> Result<Option<Handoff>, HandoffApplyError> {
        let id = id.to_string();
        let outcome = self
            .write(move |tx| {
                let Some(current) = load_handoff(tx, &id)? else { return Ok(Err(HandoffError::NotPending.into())) };
                if current.phase != HandoffPhase::Requested {
                    return Ok(Ok(None));
                }
                let loaded = load(tx, &current.workspace_id)?;
                let Some(from_run) = loaded.runs.iter().find(|r| r.id == current.from_run_id) else {
                    return Ok(Err(RunError::RunNotFound(current.from_run_id.clone()).into()));
                };
                if from_run.status.is_terminal() {
                    let reason = ended_reason(from_run);
                    return Ok(abandon_in(tx, &current, &reason, now).map(Some));
                }
                let checkpoint = handoff::latest_checkpoint(&loaded.reports, &loaded.runs, &current.task_id);
                // 突然限流（交接時的原因，或之後帳號才撞到上限）：舊 Worker 已無法工作，不等 checkpoint、也不需要承接帳號
                let sudden = handoff::is_sudden_limit(&current.reason) || gate_is_sudden(tx, &current.from_profile_id)?;
                if !sudden && !handoff::at_safe_boundary(&current, checkpoint.as_ref(), now) {
                    return Ok(Ok(None));
                }
                // 沒有承接帳號就先不要求舊程序停止：寧可留在原帳號，也不讓 Task 停在沒有 writer 的狀態
                let placement =
                    successor_placement(tx, &current.workspace_id, &quota_used, Some(&current.to_profile_id), None)?;
                if !sudden && matches!(placement, Placement::Wait(_)) {
                    return Ok(Ok(None));
                }
                match handoff::begin_stop(&current, checkpoint, now) {
                    Ok(Some((next, event))) => {
                        save_handoff(tx, &next)?;
                        append_event(tx, &event)?;
                        Ok(Ok(Some(next)))
                    }
                    Ok(None) => Ok(Ok(None)),
                    Err(rejected) => Ok(Err(rejected.into())),
                }
            })
            .await?;
        outcome
    }

    /// 步驟 3、4：舊程序已確認停止（`stop_outcome`）。單一交易：舊 Run 以 `handed_off` 結束並釋放寫入所有權，
    /// 以條件更新（舊 Run 仍持有 `from_generation` 的 lease）把世代轉給新建立的 successor Run，交接進入 `transferred`。
    /// 舊 Run 已由別的路徑結束、或沒有承接帳號時放棄交接（後者把 Task 排回派工佇列）。已轉移過時原樣回傳（冪等）。
    pub async fn transfer_handoff(
        &self,
        id: &str,
        stop_outcome: &str,
        quota_used: HashMap<String, f64>,
        now: Timestamp,
        mut new_run_id: impl FnMut() -> Option<String> + Send + 'static,
    ) -> Result<Handoff, HandoffApplyError> {
        let (id, stop_outcome) = (id.to_string(), stop_outcome.to_string());
        let outcome = self
            .write(move |tx| {
                let Some(current) = load_handoff(tx, &id)? else { return Ok(Err(HandoffError::NotPending.into())) };
                if !current.phase.is_in_flight() {
                    return Ok(Ok(current));
                }
                let workspace_id = current.workspace_id.clone();
                let loaded = load(tx, &workspace_id)?;
                let Some(from_run) = loaded.runs.iter().find(|r| r.id == current.from_run_id) else {
                    return Ok(Err(RunError::RunNotFound(current.from_run_id.clone()).into()));
                };
                if from_run.status.is_terminal() {
                    let reason = ended_reason(from_run);
                    return Ok(abandon_in(tx, &current, &reason, now));
                }
                // 條件更新：舊 Run 必須仍持有交接當時那個世代的寫入所有權
                let lease = loaded.leases.iter().find(|l| l.run_id == from_run.id);
                if !run::is_current_writer(from_run, lease, current.from_generation) {
                    return Ok(abandon_in(tx, &current, "generation_changed", now));
                }
                // 突然限流而所有合適帳號都受限（名額以舊 Run 結束後計）：舊 Run 結束、Task 進入 waiting_resource，
                // 保存要延續的 checkpoint 並排定重新評估（票 29）
                let sudden = handoff::is_sudden_limit(&current.reason) || gate_is_sudden(tx, &current.from_profile_id)?;
                let placement = successor_placement(
                    tx,
                    &workspace_id,
                    &quota_used,
                    Some(&current.to_profile_id),
                    Some(&from_run.id),
                )?;
                if let (true, Placement::Wait(reasons)) = (sudden, placement) {
                    let command =
                        RunCommand::ParkForResource { run_id: from_run.id.clone(), handoff_id: current.id.clone() };
                    match decide_in(&loaded, &workspace_id, &loaded.tasks, command, now) {
                        Ok(change) => save(tx, &change, now)?,
                        Err(rejected) => return Ok(Err(rejected.into())),
                    }
                    let checkpoint = handoff::latest_checkpoint(&loaded.reports, &loaded.runs, &current.task_id);
                    let resets = blocking_resets(tx)?;
                    return match handoff::wait_for_resource(&current, checkpoint, &stop_outcome, &resets, reasons, now) {
                        Ok((next, event)) => {
                            save_handoff(tx, &next)?;
                            append_event(tx, &event)?;
                            Ok(Ok(next))
                        }
                        Err(rejected) => Ok(Err(rejected.into())),
                    };
                }
                let command = RunCommand::HandOff { run_id: from_run.id.clone(), handoff_id: current.id.clone() };
                match decide_in(&loaded, &workspace_id, &loaded.tasks, command, now) {
                    Ok(change) => save(tx, &change, now)?,
                    Err(rejected) => return Ok(Err(rejected.into())),
                }

                // 舊 Run 已結束、Task 回到 ready：同一個交易中以交接槽派出 successor
                let mut loaded = load(tx, &workspace_id)?;
                loaded.guards = load_active_guards(tx, &workspace_id, now)?;
                let checkpoint = handoff::latest_checkpoint(&loaded.reports, &loaded.runs, &current.task_id);
                let placement = successor_placement(tx, &workspace_id, &quota_used, Some(&current.to_profile_id), None)?;
                let dispatched = match (placement, new_run_id()) {
                    (Placement::Place { profile_id }, Some(run_id)) => {
                        let command = RunCommand::Dispatch { run_id, task_id: current.task_id.clone(), profile_id };
                        decide_in(&loaded, &workspace_id, &loaded.tasks, command, now).map_err(|e| failed_check(&e).message)
                    }
                    (Placement::Wait(reasons), _) => {
                        Err(reasons.iter().map(|r| r.message.as_str()).collect::<Vec<_>>().join("；"))
                    }
                    (_, None) => Err("無法取得系統亂數，暫時不能建立 Run".to_string()),
                };
                let mut change = match dispatched {
                    Ok(change) => change,
                    Err(why) => {
                        // 沒有承接帳號：Task 留在 ready 並排回派工佇列，由一般排程接手
                        tx.execute(
                            "INSERT INTO dispatch_request (task_id, workspace_id, priority, requested_at)
                             VALUES (?1, ?2, 0, ?3) ON CONFLICT(task_id) DO NOTHING",
                            rusqlite::params![current.task_id, workspace_id, now.as_millis()],
                        )?;
                        return Ok(abandon_in(tx, &current, &format!("no_successor：{why}"), now));
                    }
                };
                // 交接說明（含要延續的 checkpoint）由 Context Builder 放進 Prompt
                super::context::attach_context(tx, &loaded, &mut change, Some(&current), now)?;
                save(tx, &change, now)?;
                let Some(successor) = change.run.as_ref() else {
                    return Ok(Err(RunError::RunNotFound(current.task_id.clone()).into()));
                };
                match handoff::transfer(&current, successor, checkpoint, &stop_outcome, now) {
                    Ok((next, event)) => {
                        save_handoff(tx, &next)?;
                        append_event(tx, &event)?;
                        Ok(Ok(next))
                    }
                    Err(rejected) => Ok(Err(rejected.into())),
                }
            })
            .await?;
        outcome
    }

    /// 放棄一個進行中的交接（例如舊 Run 已由別的路徑結束）；已不在進行中時原樣回傳。
    pub async fn abandon_handoff(&self, id: &str, reason: &str, now: Timestamp) -> Result<Handoff, HandoffApplyError> {
        let (id, reason) = (id.to_string(), reason.to_string());
        let outcome = self
            .write(move |tx| {
                let Some(current) = load_handoff(tx, &id)? else { return Ok(Err(HandoffError::NotPending.into())) };
                if !current.phase.is_in_flight() {
                    return Ok(Ok(current));
                }
                Ok(abandon_in(tx, &current, &reason, now))
            })
            .await?;
        outcome
    }

    /// 這個 Run 正在進行中（requested、stopping）的交接；沒有就是 `None`。
    pub async fn in_flight_handoff_of_run(&self, run_id: &str) -> StoreResult<Option<Handoff>> {
        let run_id = run_id.to_string();
        self.read(move |conn| {
            Ok(load_handoffs_where(conn, "from_run_id = ?1 AND phase IN ('requested', 'stopping')", [run_id])?.pop())
        })
        .await
    }

    /// 步驟 5：successor Worker 確認接手（範圍取自憑證）。只有仍是寫入所有者的 successor 可以確認。
    pub async fn confirm_handoff(
        &self,
        scope: &AgentScope,
        confirmation: HandoffConfirmation,
        now: Timestamp,
    ) -> Result<Handoff, HandoffApplyError> {
        let scope = scope.clone();
        let outcome = self
            .write(move |tx| {
                let loaded = load(tx, &scope.workspace_id)?;
                let Some(run) = loaded.runs.iter().find(|r| r.id == scope.run_id) else {
                    return Ok(Err(RunError::RunNotFound(scope.run_id.clone()).into()));
                };
                let lease = loaded.leases.iter().find(|l| l.run_id == run.id);
                if !run::is_current_writer(run, lease, scope.generation) {
                    return Ok(Err(RunError::StaleReport.into()));
                }
                let found = load_handoffs_where(
                    tx,
                    "workspace_id = ?1 AND to_run_id = ?2",
                    rusqlite::params![scope.workspace_id, scope.run_id],
                )?;
                let Some(current) = found.into_iter().last() else { return Ok(Err(HandoffError::NotPending.into())) };
                match handoff::confirm(&current, &scope.run_id, confirmation, now) {
                    Ok(Some((next, event))) => {
                        save_handoff(tx, &next)?;
                        append_event(tx, &event)?;
                        Ok(Ok(next))
                    }
                    Ok(None) => Ok(Ok(current)),
                    Err(rejected) => Ok(Err(rejected.into())),
                }
            })
            .await?;
        outcome
    }

    /// Workspace 內的全部交接，依建立順序。
    pub async fn handoffs(&self, workspace_id: &str) -> StoreResult<Vec<Handoff>> {
        let workspace_id = workspace_id.to_string();
        self.read(move |conn| Ok(load_handoffs_where(conn, "workspace_id = ?1", [workspace_id])?)).await
    }
}

/// 舊 Run 已由別的路徑結束時的放棄原因。
fn ended_reason(run: &Run) -> String {
    format!("from_run_ended：{}", run.end_reason.as_deref().unwrap_or(run.status.as_str()))
}

fn abandon_in(
    tx: &Transaction<'_>,
    current: &Handoff,
    reason: &str,
    now: Timestamp,
) -> Result<Handoff, HandoffApplyError> {
    let (next, event) = handoff::abandon(current, reason, now)?;
    save_handoff(tx, &next)?;
    append_event(tx, &event)?;
    Ok(next)
}

/// 資源恢復：Task 回到 ready，以承接帳號派出 successor（Prompt 附上最近的有效 checkpoint），交接從等待中轉移給它。
/// 派工檢查沒過（Guard、工作目錄等）時什麼都不寫，回傳原因（繼續等待）。
fn resume_in(
    tx: &Transaction<'_>,
    current: &Handoff,
    task: &Task,
    profile_id: String,
    now: Timestamp,
    new_run_id: &mut impl FnMut() -> Option<String>,
) -> StoreResult<Result<Handoff, WaitReason>> {
    let workspace_id = current.workspace_id.clone();
    let mut loaded = load(tx, &workspace_id)?;
    loaded.guards = load_active_guards(tx, &workspace_id, now)?;
    let Some((ready, resumed)) = handoff::resume_task(task, now) else {
        return Ok(Err(failed_check(&RunError::TaskNotReady(task.status))));
    };
    let tasks: Vec<Task> = loaded.tasks.iter().map(|t| if t.id == ready.id { ready.clone() } else { t.clone() }).collect();
    let Some(run_id) = new_run_id() else {
        return Ok(Err(WaitReason {
            category: WaitCategory::StateUnknown,
            code: "entropy_unavailable".to_string(),
            message: "無法取得系統亂數，暫時不能建立 Run".to_string(),
            profile_id: None,
            guard: None,
        }));
    };
    let command = RunCommand::Dispatch { run_id, task_id: current.task_id.clone(), profile_id };
    let mut change = match decide_in(&loaded, &workspace_id, &tasks, command, now) {
        Ok(change) => change,
        Err(rejected) => return Ok(Err(failed_check(&rejected))),
    };
    let checkpoint = handoff::latest_checkpoint(&loaded.reports, &loaded.runs, &current.task_id);
    // Task 回到 ready 的事件在派工事件之前（版本依序）
    for event in &resumed {
        append_event(tx, event)?;
    }
    super::context::attach_context(tx, &loaded, &mut change, Some(current), now)?;
    save(tx, &change, now)?;
    let Some(successor) = change.run.as_ref() else {
        return Ok(Err(failed_check(&RunError::RunNotFound(current.task_id.clone()))));
    };
    let stop_outcome = current.stop_outcome.clone().unwrap_or_default();
    let (next, event) = match handoff::transfer(current, successor, checkpoint, &stop_outcome, now) {
        Ok(moved) => moved,
        Err(rejected) => {
            let reason = WaitReason {
                category: WaitCategory::StateUnknown,
                code: rejected.code().to_string(),
                message: rejected.to_string(),
                profile_id: None,
                guard: None,
            };
            return Ok(Err(reason));
        }
    };
    save_handoff(tx, &next)?;
    append_event(tx, &event)?;
    Ok(Ok(next))
}

/// 等待原因中有生命週期的阻擋（完全退出的停止意圖、Workspace 等待「繼續工作」）。
fn halted(reasons: &[WaitReason]) -> bool {
    reasons.iter().any(|r| r.category == WaitCategory::Lifecycle)
}

/// 帳號目前的門檻是突然限流（draining、`limit_reached`）嗎。
fn gate_is_sudden(conn: &Connection, profile_id: &str) -> rusqlite::Result<bool> {
    let gates = load_quota_gates(conn)?;
    Ok(gates.get(profile_id).is_some_and(|g| g.state == GateState::Draining && handoff::is_sudden_limit(&g.reason)))
}

/// 每個合適（可派工）帳號不能接手時的重置時間：門檻關閉的帳號是門檻記下的重置時間，其餘（例如名額已滿）是未知。
fn blocking_resets(conn: &Connection) -> rusqlite::Result<Vec<Option<Timestamp>>> {
    let profiles = load_profiles(conn)?;
    let gates = load_quota_gates(conn)?;
    Ok(scheduler::usable_profiles(&profiles)
        .into_iter()
        .map(|p| gates.get(&p.id).filter(|g| g.state.blocks_dispatch()).and_then(|g| g.reset_at))
        .collect())
}

/// 承接帳號的選擇：可派工、額度門檻 open、名額夠（可用交接保留槽）；`preferred` 可派時優先。
/// 生命週期的停止意圖或 Workspace 暫停時不交接（交接也是派工）。`ending` 是即將在同一個交易中結束的舊 Run，
/// 不佔名額。
fn successor_placement(
    conn: &Connection,
    workspace_id: &str,
    quota_used: &HashMap<String, f64>,
    preferred: Option<&str>,
    ending: Option<&str>,
) -> rusqlite::Result<Placement> {
    let profiles = load_profiles(conn)?;
    let settings = load_settings(conn)?;
    let gates = load_gate_states(conn)?;
    let mut active = load_active_runs(conn)?;
    active.retain(|r| Some(r.id.as_str()) != ending);
    let workspace = load_workspaces(conn)?.into_iter().find(|w| w.id == workspace_id);
    let halt = halt_for(conn, workspace.as_ref(), workspace_id)?;
    let capacity = Capacity {
        profiles: &profiles,
        settings: &settings,
        active_runs: &active,
        quota_used,
        quota_gates: &gates,
        halt,
    };
    Ok(scheduler::place_preferring(&capacity, SlotKind::Handoff, preferred))
}

// ---------------------------------------------------------------- 資料列

pub(crate) fn load_quota_gates(conn: &Connection) -> rusqlite::Result<HashMap<String, QuotaGate>> {
    conn.prepare("SELECT profile_id, state, reason, used, reset_at, since, version FROM quota_gate")?
        .query_map([], |r| {
            let state: String = r.get(1)?;
            Ok(QuotaGate {
                profile_id: r.get(0)?,
                state: GateState::parse(&state).ok_or_else(|| {
                    rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, "未知的門檻狀態".into())
                })?,
                reason: r.get(2)?,
                used: r.get(3)?,
                reset_at: r.get::<_, Option<i64>>(4)?.map(Timestamp::from_millis),
                since: Timestamp::from_millis(r.get(5)?),
                version: r.get(6)?,
            })
        })?
        .map(|gate| gate.map(|g| (g.profile_id.clone(), g)))
        .collect()
}

/// 排程用：每個帳號的門檻狀態（沒有項目的是 open）。
pub(crate) fn load_gate_states(conn: &Connection) -> rusqlite::Result<HashMap<String, GateState>> {
    Ok(load_quota_gates(conn)?.into_iter().map(|(id, gate)| (id, gate.state)).collect())
}

fn save_gate(tx: &Transaction<'_>, gate: &QuotaGate) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO quota_gate (profile_id, state, reason, used, reset_at, since, version)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(profile_id) DO UPDATE SET state = excluded.state, reason = excluded.reason, used = excluded.used,
           reset_at = excluded.reset_at, since = excluded.since, version = excluded.version",
        rusqlite::params![
            gate.profile_id,
            gate.state.as_str(),
            gate.reason,
            gate.used,
            gate.reset_at.map(Timestamp::as_millis),
            gate.since.as_millis(),
            gate.version,
        ],
    )?;
    Ok(())
}

const HANDOFF_COLUMNS: &str = "id, workspace_id, task_id, from_run_id, from_profile_id, from_generation, to_profile_id, \
     to_run_id, to_generation, phase, reason, checkpoint, stop_outcome, confirmation, abandoned_reason, requested_at, \
     stopping_at, transferred_at, confirmed_at, updated_at, version, waiting_at, next_check_at, wait_attempts, \
     wait_reset_at, wait_reasons";

fn handoff_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Handoff> {
    let phase: String = r.get(9)?;
    let timestamp = |index: usize| r.get::<_, Option<i64>>(index).map(|v| v.map(Timestamp::from_millis));
    let checkpoint: Option<String> = r.get(11)?;
    let confirmation: Option<String> = r.get(13)?;
    let wait_reasons: String = r.get(25)?;
    Ok(Handoff {
        id: r.get(0)?,
        workspace_id: r.get(1)?,
        task_id: r.get(2)?,
        from_run_id: r.get(3)?,
        from_profile_id: r.get(4)?,
        from_generation: r.get(5)?,
        to_profile_id: r.get(6)?,
        to_run_id: r.get(7)?,
        to_generation: r.get(8)?,
        phase: HandoffPhase::parse(&phase).ok_or_else(|| {
            rusqlite::Error::FromSqlConversionFailure(9, rusqlite::types::Type::Text, "未知的交接階段".into())
        })?,
        reason: r.get(10)?,
        checkpoint: checkpoint.map(|raw| json_column(11, &raw)).transpose()?,
        stop_outcome: r.get(12)?,
        confirmation: confirmation.map(|raw| json_column(13, &raw)).transpose()?,
        abandoned_reason: r.get(14)?,
        requested_at: Timestamp::from_millis(r.get(15)?),
        stopping_at: timestamp(16)?,
        transferred_at: timestamp(17)?,
        confirmed_at: timestamp(18)?,
        waiting_at: timestamp(21)?,
        next_check_at: timestamp(22)?,
        wait_attempts: r.get(23)?,
        wait_reset_at: timestamp(24)?,
        wait_reasons: json_column(25, &wait_reasons)?,
        updated_at: Timestamp::from_millis(r.get(19)?),
        version: r.get(20)?,
    })
}

fn load_handoffs_where<P: rusqlite::Params>(conn: &Connection, filter: &str, params: P) -> rusqlite::Result<Vec<Handoff>> {
    conn.prepare(&format!("SELECT {HANDOFF_COLUMNS} FROM handoff WHERE {filter} ORDER BY rowid"))?
        .query_map(params, handoff_row)?
        .collect()
}

fn load_handoff(conn: &Connection, id: &str) -> rusqlite::Result<Option<Handoff>> {
    Ok(load_handoffs_where(conn, "id = ?1", [id])?.pop())
}

fn save_handoff(tx: &Transaction<'_>, h: &Handoff) -> rusqlite::Result<()> {
    // Domain 型別只含字串與數字，序列化不會失敗
    let checkpoint = h.checkpoint.as_ref().map(|c| serde_json::to_string(c).expect("checkpoint 可序列化"));
    let confirmation = h.confirmation.as_ref().map(|c| serde_json::to_string(c).expect("確認內容可序列化"));
    let wait_reasons = serde_json::to_string(&h.wait_reasons).expect("等待原因可序列化");
    let millis = |t: Option<Timestamp>| t.map(Timestamp::as_millis);
    tx.execute(
        &format!(
            "INSERT INTO handoff ({HANDOFF_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26)
             ON CONFLICT(id) DO UPDATE SET to_profile_id = excluded.to_profile_id, to_run_id = excluded.to_run_id,
               to_generation = excluded.to_generation, phase = excluded.phase, checkpoint = excluded.checkpoint,
               stop_outcome = excluded.stop_outcome, confirmation = excluded.confirmation,
               abandoned_reason = excluded.abandoned_reason, stopping_at = excluded.stopping_at,
               transferred_at = excluded.transferred_at, confirmed_at = excluded.confirmed_at,
               waiting_at = excluded.waiting_at, next_check_at = excluded.next_check_at,
               wait_attempts = excluded.wait_attempts, wait_reset_at = excluded.wait_reset_at,
               wait_reasons = excluded.wait_reasons, updated_at = excluded.updated_at, version = excluded.version"
        ),
        rusqlite::params![
            h.id,
            h.workspace_id,
            h.task_id,
            h.from_run_id,
            h.from_profile_id,
            h.from_generation,
            h.to_profile_id,
            h.to_run_id,
            h.to_generation,
            h.phase.as_str(),
            h.reason,
            checkpoint,
            h.stop_outcome,
            confirmation,
            h.abandoned_reason,
            h.requested_at.as_millis(),
            millis(h.stopping_at),
            millis(h.transferred_at),
            millis(h.confirmed_at),
            h.updated_at.as_millis(),
            h.version,
            millis(h.waiting_at),
            millis(h.next_check_at),
            h.wait_attempts,
            millis(h.wait_reset_at),
            wait_reasons,
        ],
    )?;
    Ok(())
}
