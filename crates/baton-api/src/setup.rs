//! 首次啟動精靈端點（spec D-20）：檢查 Claude Code 與 Git for Windows、各 Profile 的登入狀態，
//! 並記錄使用者已閱讀背景執行行為的告知。
//!
//! 缺項只回傳官方指令供顯示；這裡不下載、不安裝、不執行任何腳本。「重新檢查」就是再呼叫一次。

use std::sync::Arc;

use axum::extract::State;
use axum::Json;
use baton_domain::profile::{self, ProfileStatus, ProfileView};
use baton_domain::setup::{self, ToolCheck, ToolStatus};
use baton_domain::Timestamp;
use serde::{Deserialize, Serialize};

use crate::workspaces::ApiError;
use crate::ApiState;

/// 精靈狀態：GET 只重新探測工具並讀取已記錄的 Profile 結果。
pub const SETUP_PATH: &str = "/api/v1/setup";
/// 「重新檢查」：在 GET 的基礎上，另外重新核對每個使用中 Profile 的登入狀態。
pub const SETUP_RECHECK_PATH: &str = "/api/v1/setup/recheck";
/// 確認已閱讀背景執行行為的告知。
pub const SETUP_ACK_PATH: &str = "/api/v1/setup/background-notice/acknowledge";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackgroundNotice {
    pub text: String,
    /// 使用者確認的時間；還沒確認為 `None`。
    pub acknowledged_at: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupStatus {
    pub claude_code: ToolCheck,
    pub git: ToolCheck,
    /// 使用中的 Profile 與各自的登入、身分與可派工狀態。
    pub profiles: Vec<ProfileView>,
    pub profile_limit: usize,
    pub background_notice: BackgroundNotice,
    /// Claude Code 與 Git 都可用，且至少有一個可派工的 Profile。
    pub ready: bool,
}

pub(crate) async fn status(State(state): State<Arc<ApiState>>) -> Result<Json<SetupStatus>, ApiError> {
    Ok(Json(build_status(&state).await?))
}

pub(crate) async fn recheck(State(state): State<Arc<ApiState>>) -> Result<Json<SetupStatus>, ApiError> {
    let active: Vec<_> =
        state.store.list_profiles().await?.into_iter().filter(|p| p.status == ProfileStatus::Active).collect();
    // 某個 Profile 核對失敗（例如 Claude Code 沒有回應）時保留它先前的結果，其餘照常更新；
    // 失敗的原因會在工具檢查或該 Profile 自己的「重新核對」顯示
    let checks = active.iter().map(|profile| crate::profiles::verify_profile(&state, profile));
    let _ = futures::future::join_all(checks).await;
    Ok(Json(build_status(&state).await?))
}

pub(crate) async fn acknowledge_background_notice(
    State(state): State<Arc<ApiState>>,
) -> Result<Json<SetupStatus>, ApiError> {
    state.store.acknowledge_background_notice(state.clock.now()).await?;
    Ok(Json(build_status(&state).await?))
}

async fn build_status(state: &ApiState) -> Result<SetupStatus, ApiError> {
    let (claude, git) = futures::join!(state.claude_env.probe_claude(), state.claude_env.probe_git());
    let claude_code = setup::check_claude(claude.map_err(|e| e.to_string()));
    let git = setup::check_git(git.map_err(|e| e.to_string()));

    let profiles: Vec<ProfileView> =
        profile::views(&state.store.list_profiles().await?).into_iter().filter(|v| v.profile.status == ProfileStatus::Active).collect();
    let ready = claude_code.status == ToolStatus::Ok && git.status == ToolStatus::Ok && profiles.iter().any(|v| v.dispatchable);
    Ok(SetupStatus {
        claude_code,
        git,
        profiles,
        profile_limit: profile::MAX_PROFILES,
        background_notice: BackgroundNotice {
            text: setup::background_notice_text(),
            acknowledged_at: state.store.background_notice_acknowledged_at().await?,
        },
        ready,
    })
}
