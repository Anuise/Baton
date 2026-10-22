//! 派工請求佇列、容量設定與排程流程的持久化（票 19）。
//!
//! 規則都在 Domain（`baton_domain::scheduler`、`baton_domain::run`）；這裡只做「在同一個交易中載入目前狀態
//! （全部 Profile、全域進行中的 Run、容量設定）→ 交給 Domain 判定 → 派出或留在佇列」。
//! 判定與寫入在同一個交易裡，所以兩個同時到達的派工請求不會一起搶到最後一個名額。
//! 佇列只記錄「哪個 Task 在排隊」；等待原因每次由目前狀態重新判定，不落地，才不會過期。

use std::collections::HashMap;

use baton_domain::profile::AccountProfile;
use baton_domain::run::{self, Run, RunCommand, RunContext, RunError};
use baton_domain::scheduler::{
    self, Capacity, CapacityMode, Evaluation, SchedulerSettings, SlotKind, WaitCategory, WaitReason,
};

use super::profiles::load_profiles;
use super::runs::{load_active_runs, save, RunApplyError};
use super::tasks::load;
use super::*;

/// 使用者（或 Controller）要求派工一個 ready 的 Task。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleRequest {
    pub workspace_id: String,
    pub task_id: String,
    /// 數字越大越優先；同優先序依要求順序。
    pub priority: i32,
}

/// 排程時被派出的 Run。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartedRun {
    pub workspace_id: String,
    pub run_id: String,
    pub task_id: String,
    pub profile_id: String,
}

/// 這次排程後仍在排隊的 Task 與原因。
#[derive(Debug, Clone, PartialEq)]
pub struct WaitingTask {
    pub workspace_id: String,
    pub task_id: String,
    pub reasons: Vec<WaitReason>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ScheduleOutcome {
    pub started: Vec<StartedRun>,
    pub waiting: Vec<WaitingTask>,
}

/// 佇列中的一筆要求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedRequest {
    pub seq: i64,
    pub workspace_id: String,
    pub task_id: String,
    pub title: String,
    pub priority: i32,
    pub requested_at: Timestamp,
}

/// 排程總覽的資料：容量設定、全部 Profile、進行中的 Run（附 Task 標題）與佇列（附目前的等待原因）。
#[derive(Debug, Clone, PartialEq)]
pub struct SchedulingSnapshot {
    pub settings: SchedulerSettings,
    pub profiles: Vec<AccountProfile>,
    pub active_runs: Vec<(Run, String)>,
    pub queue: Vec<(QueuedRequest, Vec<WaitReason>)>,
}

impl Store {
    /// 一次排程：先把新的要求（若有）放進佇列，再依「優先序高者先、同優先序先要求者先」逐一嘗試派出，
    /// 全部在同一個交易中完成。新要求遇到「不是等一等就好」的錯誤（Task 不是 ready、找不到、契約失效）時
    /// 原樣拒絕且什麼都不寫；授權、相依、帳號與容量造成的等待則留在佇列。
    pub async fn schedule(
        &self,
        request: Option<ScheduleRequest>,
        quota_used: HashMap<String, f64>,
        now: Timestamp,
        mut new_run_id: impl FnMut() -> Option<String> + Send + 'static,
    ) -> Result<ScheduleOutcome, RunApplyError> {
        let outcome = self
            .write(move |tx| {
                let profiles = load_profiles(tx)?;
                let settings = load_settings(tx)?;
                if let Some(request) = &request {
                    // 先用目前狀態判定一次：不可能派的要求不進佇列
                    let loaded = load(tx, &request.workspace_id)?;
                    if loaded.workspace.is_none() {
                        return Ok(Err(RunError::WorkspaceNotFound(request.workspace_id.clone())));
                    }
                    let active = load_active_runs(tx)?;
                    let capacity = Capacity { profiles: &profiles, settings: &settings, active_runs: &active, quota_used: &quota_used };
                    let task_ctx = loaded.ctx(&request.workspace_id);
                    let ctx = RunContext { task: &task_ctx, runs: &loaded.runs, leases: &loaded.leases, workdirs: &loaded.workdirs };
                    if let Err(rejected) = scheduler::evaluate(&ctx, &capacity, &request.task_id, SlotKind::Normal) {
                        return Ok(Err(rejected));
                    }
                    tx.execute(
                        "INSERT INTO dispatch_request (task_id, workspace_id, priority, requested_at)
                         VALUES (?1, ?2, ?3, ?4)
                         ON CONFLICT(task_id) DO UPDATE SET priority = excluded.priority",
                        rusqlite::params![request.task_id, request.workspace_id, request.priority, now.as_millis()],
                    )?;
                }

                let mut outcome = ScheduleOutcome::default();
                for entry in load_queue(tx)? {
                    let loaded = load(tx, &entry.workspace_id)?;
                    let active = load_active_runs(tx)?;
                    let capacity = Capacity { profiles: &profiles, settings: &settings, active_runs: &active, quota_used: &quota_used };
                    let task_ctx = loaded.ctx(&entry.workspace_id);
                    let ctx = RunContext { task: &task_ctx, runs: &loaded.runs, leases: &loaded.leases, workdirs: &loaded.workdirs };
                    let waiting = |reasons| WaitingTask {
                        workspace_id: entry.workspace_id.clone(),
                        task_id: entry.task_id.clone(),
                        reasons,
                    };
                    match scheduler::evaluate(&ctx, &capacity, &entry.task_id, SlotKind::Normal) {
                        Ok(Evaluation::Dispatch { profile_id }) => {
                            let Some(run_id) = new_run_id() else {
                                outcome.waiting.push(waiting(vec![WaitReason {
                                    category: WaitCategory::StateUnknown,
                                    code: "entropy_unavailable".to_string(),
                                    message: "無法取得系統亂數，暫時不能建立 Run".to_string(),
                                    profile_id: None,
                                }]));
                                continue;
                            };
                            let command = RunCommand::Dispatch {
                                run_id,
                                task_id: entry.task_id.clone(),
                                profile_id: profile_id.clone(),
                            };
                            match run::decide(&ctx, command, now) {
                                Ok(mut change) => {
                                    // 與 apply_run_command 的派工相同：Prompt 附上收件 Task 的未讀訊息摘要（票 18）
                                    super::messages::attach_unread_summary(tx, &mut change, now)?;
                                    let run_id = change.run.as_ref().map(|r| r.id.clone()).unwrap_or_default();
                                    save(tx, &change, now)?;
                                    outcome.started.push(StartedRun {
                                        workspace_id: entry.workspace_id.clone(),
                                        run_id,
                                        task_id: entry.task_id.clone(),
                                        profile_id,
                                    });
                                }
                                Err(rejected) => outcome.waiting.push(waiting(vec![failed_check(&rejected)])),
                            }
                        }
                        Ok(Evaluation::Wait(reasons)) => outcome.waiting.push(waiting(reasons)),
                        // Task 已經不是 ready 或已不存在：不再排隊
                        Err(RunError::TaskNotReady(_) | RunError::TaskNotFound(_)) => {
                            tx.execute("DELETE FROM dispatch_request WHERE task_id = ?1", [&entry.task_id])?;
                        }
                        Err(rejected) => outcome.waiting.push(waiting(vec![failed_check(&rejected)])),
                    }
                }
                Ok(Ok(outcome))
            })
            .await?;
        Ok(outcome?)
    }

    /// 排程總覽：容量設定、全部 Profile、進行中的 Run 與佇列（附各自目前的等待原因）。
    pub async fn scheduling_snapshot(&self, quota_used: HashMap<String, f64>) -> StoreResult<SchedulingSnapshot> {
        self.read(move |conn| {
            let profiles = load_profiles(conn)?;
            let settings = load_settings(conn)?;
            let active = load_active_runs(conn)?;
            let mut active_runs = Vec::with_capacity(active.len());
            for run in &active {
                let title: String = conn
                    .query_row("SELECT title FROM task WHERE id = ?1", [&run.task_id], |r| r.get(0))
                    .unwrap_or_else(|_| run.task_id.clone());
                active_runs.push((run.clone(), title));
            }

            let mut queue = Vec::new();
            for entry in load_queue(conn)? {
                let loaded = load(conn, &entry.workspace_id)?;
                let capacity = Capacity { profiles: &profiles, settings: &settings, active_runs: &active, quota_used: &quota_used };
                let task_ctx = loaded.ctx(&entry.workspace_id);
                let ctx = RunContext { task: &task_ctx, runs: &loaded.runs, leases: &loaded.leases, workdirs: &loaded.workdirs };
                let reasons = match scheduler::evaluate(&ctx, &capacity, &entry.task_id, SlotKind::Normal) {
                    // 現在就能派：只是還沒輪到排程處理，沒有需要使用者處理的原因
                    Ok(Evaluation::Dispatch { .. }) => Vec::new(),
                    Ok(Evaluation::Wait(reasons)) => reasons,
                    Err(RunError::TaskNotReady(_) | RunError::TaskNotFound(_)) => continue,
                    Err(rejected) => vec![failed_check(&rejected)],
                };
                queue.push((entry, reasons));
            }
            Ok(SchedulingSnapshot { settings, profiles, active_runs, queue })
        })
        .await
    }

    /// 佇列裡有沒有人在排隊（Supervisor 每次處理都先問，沒人排隊就不開寫入交易）。
    pub async fn has_dispatch_requests(&self) -> StoreResult<bool> {
        self.read(|conn| Ok(conn.query_row("SELECT EXISTS (SELECT 1 FROM dispatch_request)", [], |r| r.get(0))?)).await
    }

    pub async fn scheduler_settings(&self) -> StoreResult<SchedulerSettings> {
        self.read(|conn| Ok(load_settings(conn)?)).await
    }

    /// 切換容量預設；與目前相同時不寫入也不追加事件。
    pub async fn set_capacity_mode(&self, mode: CapacityMode, now: Timestamp) -> StoreResult<SchedulerSettings> {
        self.write(move |tx| {
            let current = load_settings(tx)?;
            let Some((next, event)) = scheduler::change_mode(&current, mode, now) else {
                return Ok(current);
            };
            tx.execute(
                "INSERT INTO scheduler_setting (id, mode, version, updated_at) VALUES (1, ?1, ?2, ?3)
                 ON CONFLICT(id) DO UPDATE SET mode = excluded.mode, version = excluded.version,
                   updated_at = excluded.updated_at",
                rusqlite::params![next.mode.as_str(), next.version, now.as_millis()],
            )?;
            append_event(tx, &event)?;
            Ok(next)
        })
        .await
    }
}

/// 派工前的檢查在排隊期間不再通過、又不是授權或相依這類可以等的原因（例如前一個 Run 還沒確認停止）：
/// 保留在佇列並如實顯示，歸為「狀態不明」。
fn failed_check(error: &RunError) -> WaitReason {
    WaitReason {
        category: WaitCategory::StateUnknown,
        code: error.code().to_string(),
        message: error.to_string(),
        profile_id: None,
    }
}

fn load_settings(conn: &Connection) -> rusqlite::Result<SchedulerSettings> {
    let row = conn.query_row("SELECT mode, version, updated_at FROM scheduler_setting WHERE id = 1", [], |r| {
        let mode: String = r.get(0)?;
        Ok(SchedulerSettings {
            mode: CapacityMode::parse(&mode).ok_or_else(|| {
                rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, "未知的容量預設".into())
            })?,
            version: r.get(1)?,
            updated_at: Some(Timestamp::from_millis(r.get(2)?)),
        })
    });
    match row {
        Ok(settings) => Ok(settings),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(SchedulerSettings::default()),
        Err(other) => Err(other),
    }
}

/// 佇列：優先序高者在前，同優先序依要求順序。
fn load_queue(conn: &Connection) -> rusqlite::Result<Vec<QueuedRequest>> {
    conn.prepare(
        "SELECT q.seq, q.workspace_id, q.task_id, t.title, q.priority, q.requested_at
         FROM dispatch_request q JOIN task t ON t.id = q.task_id
         ORDER BY q.priority DESC, q.seq ASC",
    )?
    .query_map([], |r| {
        Ok(QueuedRequest {
            seq: r.get(0)?,
            workspace_id: r.get(1)?,
            task_id: r.get(2)?,
            title: r.get(3)?,
            priority: r.get(4)?,
            requested_at: Timestamp::from_millis(r.get(5)?),
        })
    })?
    .collect()
}

#[cfg(test)]
mod tests;
