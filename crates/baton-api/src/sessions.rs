//! Session 端點（桌面角色專用，票 15）：列出 Workspace 的 Session、讀取一個 Session 的事件與對話紀錄段落。
//!
//! Session 只隨 Run 改變（Supervisor 寫入 launch intent、Adapter 回報原生識別、Run 結束），沒有寫入端點。

use std::sync::Arc;

use axum::extract::{Path as UrlPath, State};
use axum::Json;
use baton_domain::session::{SessionDetail, SessionView};
use serde::{Deserialize, Serialize};

use crate::tasks::TaskApiError;
use crate::ApiState;

pub(crate) const SESSIONS_ROUTE: &str = "/api/v1/workspaces/{id}/sessions";
pub(crate) const SESSION_ROUTE: &str = "/api/v1/workspaces/{id}/sessions/{session_id}";

pub fn sessions_path(workspace_id: &str) -> String {
    format!("{}/{workspace_id}/sessions", crate::WORKSPACES_PATH)
}

pub fn session_path(workspace_id: &str, session_id: &str) -> String {
    format!("{}/{session_id}", sessions_path(workspace_id))
}

/// Workspace 的全部 Session，依建立順序。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionList {
    pub sessions: Vec<SessionView>,
}

pub(crate) async fn list(
    State(state): State<Arc<ApiState>>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<SessionList>, TaskApiError> {
    Ok(Json(SessionList { sessions: state.store.session_list(&id).await? }))
}

pub(crate) async fn detail(
    State(state): State<Arc<ApiState>>,
    UrlPath((id, session_id)): UrlPath<(String, String)>,
) -> Result<Json<SessionDetail>, TaskApiError> {
    Ok(Json(state.store.session_detail(&id, &session_id).await?))
}
