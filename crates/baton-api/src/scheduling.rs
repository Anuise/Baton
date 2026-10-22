//! 排程端點（桌面角色專用）：排程總覽（容量、各帳號進行中的 Run、佇列與等待原因）與容量預設切換（票 19）。
//!
//! 規則都在 Domain（`baton_domain::scheduler`）；端點只做「經 Port 取得額度事實 → 交給 Store → 對應成
//! 穩定的欄位」。派工請求本身走 `runs::dispatch`，兩者共用同一個排程流程（`schedule_now`）。

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use baton_domain::profile::{self, BlockerInfo, ProfileStatus};
use baton_domain::quota::{PlanLimits, QuotaAssessment};
use baton_domain::run::RunStatus;
use baton_domain::scheduler::{self, CapacityMode, SchedulerSettings, WaitReason};
use baton_domain::Timestamp;
use baton_store::{AccountQuota, ScheduleOutcome, ScheduleRequest};
use serde::{Deserialize, Serialize};

use crate::tasks::TaskApiError;
use crate::workspaces::{new_id, ApiError};
use crate::ApiState;

/// 排程總覽路徑。
pub const SCHEDULING_PATH: &str = "/api/v1/scheduling";
/// 容量預設切換路徑（PUT）。
pub const SCHEDULING_SETTINGS_PATH: &str = "/api/v1/scheduling/settings";

/// 一組容量預設的說明（設定頁的選項）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapacityModeInfo {
    pub mode: CapacityMode,
    pub label: String,
    pub description: String,
    pub per_account_limit: usize,
    pub global_limit: usize,
    pub handoff_reserve: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchedulingSettingsView {
    pub mode: CapacityMode,
    pub mode_label: String,
    pub modes: Vec<CapacityModeInfo>,
}

/// 目前生效的容量數字與使用情形。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapacityView {
    pub per_account_limit: usize,
    pub global_limit: usize,
    /// 實際保留給交接的名額（可用帳號數不到 3 時為 0）。
    pub handoff_reserve: usize,
    /// 一般派工最多能佔用的全域名額（全域上限扣掉保留的交接槽）。
    pub normal_limit: usize,
    /// 目前進行中的 Run 總數。
    pub running: usize,
    /// 可派工的帳號數。
    pub usable_accounts: usize,
}

/// 帳號上一個進行中的 Run。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountRun {
    pub run_id: String,
    pub workspace_id: String,
    pub task_id: String,
    pub task_title: String,
    pub status: RunStatus,
    pub generation: i64,
    /// 已要求取消但程序還沒確認停止。
    pub cancel_requested: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccountScheduling {
    pub profile_id: String,
    pub name: String,
    pub dispatchable: bool,
    pub blockers: Vec<BlockerInfo>,
    /// 以較保守者判斷的用量比例（同 `quota.used`）；未觀測為 `null`，不當作 0。
    pub quota_used: Option<f64>,
    /// 額度判斷：兩個視窗各自的已觀測與推估、可信度、重置與觀測時間、是否觀測不完整（票 27）。
    pub quota: QuotaAssessment,
    /// 使用者設定的方案上限（推估的分母）。
    pub plan_limits: PlanLimits,
    pub running: usize,
    pub limit: usize,
    pub runs: Vec<AccountRun>,
}

/// 佇列中等待名額的 Task。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueEntryView {
    pub workspace_id: String,
    pub task_id: String,
    pub title: String,
    pub priority: i32,
    pub requested_at: Timestamp,
    /// 佇列中的順位（從 1 起，優先序高者在前）。
    pub position: usize,
    /// 為什麼還沒開始；空的代表現在就能派，只是還沒輪到排程處理。
    pub reasons: Vec<WaitReason>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SchedulingOverview {
    pub settings: SchedulingSettingsView,
    pub capacity: CapacityView,
    /// 使用中的帳號，依建立順序。
    pub accounts: Vec<AccountScheduling>,
    pub queue: Vec<QueueEntryView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateSchedulingSettingsRequest {
    pub mode: CapacityMode,
}

fn settings_view(settings: &SchedulerSettings) -> SchedulingSettingsView {
    SchedulingSettingsView {
        mode: settings.mode,
        mode_label: settings.mode.label().to_string(),
        modes: CapacityMode::ALL
            .into_iter()
            .map(|mode| {
                let limits = mode.limits();
                CapacityModeInfo {
                    mode,
                    label: mode.label().to_string(),
                    description: mode.description(),
                    per_account_limit: limits.per_account,
                    global_limit: limits.global,
                    handoff_reserve: limits.handoff_reserve,
                }
            })
            .collect(),
    }
}

/// 輪詢到期的帳號後，取得使用中帳號目前已知的用量（排程用）。
pub(crate) async fn known_quota(state: &ApiState) -> Result<HashMap<String, f64>, TaskApiError> {
    let status = crate::quota::current(&state.store, &*state.quota_observer, state.clock.now()).await.map_err(ApiError::from)?;
    Ok(crate::quota::known_used(&status))
}

/// 現在排程一次（可帶一個新的派工要求）。
pub(crate) async fn schedule_now(
    state: &ApiState,
    request: Option<ScheduleRequest>,
) -> Result<ScheduleOutcome, TaskApiError> {
    let quota = known_quota(state).await?;
    let outcome = state
        .store
        .schedule(request, quota, state.clock.now(), || new_id("run").ok())
        .await?;
    if !outcome.started.is_empty() {
        state.supervisor_wake.notify_one();
    }
    Ok(outcome)
}

pub(crate) async fn overview(state: &ApiState) -> Result<SchedulingOverview, TaskApiError> {
    let mut status =
        crate::quota::current(&state.store, &*state.quota_observer, state.clock.now()).await.map_err(ApiError::from)?;
    let quota = crate::quota::known_used(&status);
    let snapshot = state.store.scheduling_snapshot(quota.clone(), state.clock.now()).await.map_err(ApiError::from)?;
    let limits = snapshot.settings.mode.limits();
    let usable = scheduler::usable_profiles(&snapshot.profiles).len();

    let accounts = snapshot
        .profiles
        .iter()
        .filter(|p| p.status == ProfileStatus::Active)
        .map(|p| {
            let blockers = profile::blockers(p, &snapshot.profiles);
            let account_quota = status.remove(&p.id).unwrap_or_else(|| AccountQuota {
                assessment: baton_domain::quota::assess(&[], state.clock.now()),
                plan_limits: PlanLimits::default(),
            });
            let runs: Vec<AccountRun> = snapshot
                .active_runs
                .iter()
                .filter(|(run, _)| run.profile_id == p.id)
                .map(|(run, title)| AccountRun {
                    run_id: run.id.clone(),
                    workspace_id: run.workspace_id.clone(),
                    task_id: run.task_id.clone(),
                    task_title: title.clone(),
                    status: run.status,
                    generation: run.generation,
                    cancel_requested: run.cancel_requested_at.is_some(),
                })
                .collect();
            AccountScheduling {
                profile_id: p.id.clone(),
                name: p.name.clone(),
                dispatchable: blockers.is_empty(),
                blockers: blockers
                    .iter()
                    .map(|b| BlockerInfo { code: b.code().to_string(), message: b.message() })
                    .collect(),
                quota_used: quota.get(&p.id).copied(),
                quota: account_quota.assessment,
                plan_limits: account_quota.plan_limits,
                running: runs.len(),
                limit: limits.per_account,
                runs,
            }
        })
        .collect();

    let queue = snapshot
        .queue
        .into_iter()
        .enumerate()
        .map(|(index, (entry, reasons))| QueueEntryView {
            workspace_id: entry.workspace_id,
            task_id: entry.task_id,
            title: entry.title,
            priority: entry.priority,
            requested_at: entry.requested_at,
            position: index + 1,
            reasons,
        })
        .collect();

    Ok(SchedulingOverview {
        settings: settings_view(&snapshot.settings),
        capacity: CapacityView {
            per_account_limit: limits.per_account,
            global_limit: limits.global,
            handoff_reserve: limits.effective_reserve(usable),
            normal_limit: limits.normal_limit(usable),
            running: snapshot.active_runs.len(),
            usable_accounts: usable,
        },
        accounts,
        queue,
    })
}

pub(crate) async fn view(State(state): State<Arc<ApiState>>) -> Result<Json<SchedulingOverview>, TaskApiError> {
    Ok(Json(overview(&state).await?))
}

/// 切換容量預設。容量變大可能讓排隊中的 Task 有名額，所以切換後立刻排程一次；已在執行的 Run 不受影響。
pub(crate) async fn update_settings(
    State(state): State<Arc<ApiState>>,
    body: Result<Json<UpdateSchedulingSettingsRequest>, JsonRejection>,
) -> Result<Json<SchedulingOverview>, TaskApiError> {
    let Json(request) = body.map_err(|rejection| ApiError {
        status: StatusCode::BAD_REQUEST,
        code: "bad_request",
        message: format!("請求內容不正確（mode 必須是 reserve_handoff 或 higher_concurrency）：{rejection}"),
    })?;
    state.store.set_capacity_mode(request.mode, state.clock.now()).await.map_err(ApiError::from)?;
    schedule_now(&state, None).await?;
    Ok(Json(overview(&state).await?))
}
