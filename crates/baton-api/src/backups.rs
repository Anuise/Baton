//! 備份狀態端點（票 40，spec D-18）：設定頁顯示現有的備份與最新一次還原演練的結果。
//!
//! 備份與演練紀錄都存在資料目錄的 `backups/` 之下（不在控制資料庫裡），所以這裡直接讀那個目錄。

use std::collections::BTreeSet;
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use baton_domain::backup::{self, BackupTrigger, DrillReport, RetentionReason};
use baton_domain::Timestamp;
use serde::{Deserialize, Serialize};

use crate::workspaces::ApiError;
use crate::ApiState;

pub const BACKUPS_PATH: &str = "/api/v1/backups";

/// 一份備份的摘要。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupSummary {
    pub id: String,
    pub trigger: BackupTrigger,
    pub created_at: Timestamp,
    /// 備份中的成果檔案數。
    pub artifact_count: usize,
    /// 依保留規則，這份備份為什麼被保留。
    pub retained_as: BTreeSet<RetentionReason>,
}

/// 設定頁的備份狀態：備份由新到舊，以及最新一次還原演練（還沒做過時為 `None`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupStatus {
    pub backups: Vec<BackupSummary>,
    pub latest_drill: Option<DrillReport>,
}

pub(crate) async fn status(State(state): State<Arc<ApiState>>) -> Result<Json<BackupStatus>, ApiError> {
    let data_dir = state.data_dir.clone();
    let read = tokio::task::spawn_blocking(move || {
        Ok::<_, baton_store::backup::BackupError>((
            baton_store::backup::list_backups(&data_dir)?,
            baton_store::backup::latest_drill(&data_dir)?,
        ))
    })
    .await;
    let (manifests, latest_drill) = match read {
        Ok(Ok(found)) => found,
        Ok(Err(error)) => return Err(unavailable(error)),
        Err(error) => return Err(unavailable(error)),
    };
    let mut retained = backup::retention(&manifests);
    let backups = manifests
        .into_iter()
        .rev()
        .map(|manifest| BackupSummary {
            retained_as: retained.remove(&manifest.id).unwrap_or_default(),
            id: manifest.id,
            trigger: manifest.trigger,
            created_at: manifest.created_at,
            artifact_count: manifest.artifacts.len(),
        })
        .collect();
    Ok(Json(BackupStatus { backups, latest_drill }))
}

fn unavailable(error: impl std::fmt::Display) -> ApiError {
    ApiError {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        code: "backup_status_unavailable",
        message: format!("無法讀取備份狀態：{error}"),
    }
}
