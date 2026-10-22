//! 更新與解除安裝的維護的持久化（票 34；spec D-19、D-25）。
//!
//! 規則都在 Domain（`baton_domain::maintenance`）；這裡只做「在同一個交易中載入目前狀態 → 交給 Domain 判定 →
//! 寫入新狀態與事件」。開始維護與完全退出的停止意圖在同一個交易中持久化，所以兩者不會只有一個寫進資料庫。
//! 生命週期檔的內容一律由 `lifecycle_mirror` 產生，維護進行中時每次鏡像都帶著維護抑制。

use baton_domain::lifecycle::{self, ExitTiming, LifecycleFile};
use baton_domain::maintenance::{
    self, AfterStop, MaintenanceError, MaintenanceOutcome, MaintenanceRecord, MaintenanceRequest,
};

use super::lifecycle::{apply_change, load_lifecycle, load_workspace_lifecycles};
use super::*;

/// 開始維護的失敗。
#[derive(Debug, thiserror::Error)]
pub enum MaintenanceApplyError {
    #[error(transparent)]
    Rejected(#[from] MaintenanceError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl Store {
    pub async fn maintenance(&self) -> StoreResult<MaintenanceRecord> {
        self.read(|conn| Ok(load_maintenance(conn)?)).await
    }

    /// 開始維護：同一個交易中持久化維護與完全退出的停止意圖（暫停每個使用中的 Workspace），之後由停止流程停下全部 Run。
    /// 同一種維護的重複要求不寫入任何東西。
    pub async fn begin_maintenance(
        &self,
        request: MaintenanceRequest,
        timing: ExitTiming,
        now: Timestamp,
    ) -> Result<MaintenanceRecord, MaintenanceApplyError> {
        let outcome = self
            .write(move |tx| {
                let record = load_maintenance(tx)?;
                let state = load_lifecycle(tx)?;
                match maintenance::begin(&record, &state, request, now) {
                    Err(rejected) => Ok(Err(rejected)),
                    Ok(None) => Ok(Ok(record)),
                    Ok(Some((next, event))) => {
                        save_maintenance(tx, &next)?;
                        append_event(tx, &event)?;
                        let change = lifecycle::request_exit(
                            &state,
                            &load_workspaces(tx)?,
                            &load_workspace_lifecycles(tx)?,
                            timing,
                            now,
                        );
                        apply_change(tx, &change)?;
                        Ok(Ok(next))
                    }
                }
            })
            .await?;
        Ok(outcome?)
    }

    /// 停止流程之後：記下「準備套用」或「失敗」；不是停止中的維護時不寫入。
    pub async fn complete_maintenance(&self, result: AfterStop, now: Timestamp) -> StoreResult<MaintenanceRecord> {
        self.write(move |tx| {
            let record = load_maintenance(tx)?;
            match maintenance::complete(&record, result, now) {
                Some((next, event)) => {
                    save_maintenance(tx, &next)?;
                    append_event(tx, &event)?;
                    Ok(next)
                }
                None => Ok(record),
            }
        })
        .await
    }

    /// 使用者手動開啟後結束維護並記下結果；沒有進行中的維護時為 `None`。停止意圖不在這裡解除（之後依 DR-01 重新開啟）。
    pub async fn finish_maintenance(&self, running_version: &str, now: Timestamp) -> StoreResult<Option<MaintenanceOutcome>> {
        let running_version = running_version.to_string();
        self.write(move |tx| {
            let record = load_maintenance(tx)?;
            match maintenance::finish(&record, &running_version, now) {
                Some((next, outcome, event)) => {
                    save_maintenance(tx, &next)?;
                    append_event(tx, &event)?;
                    Ok(Some(outcome))
                }
                None => Ok(None),
            }
        })
        .await
    }

    /// 生命週期檔應有的內容：資料庫中的停止意圖，加上進行中維護的維護抑制。
    pub async fn lifecycle_mirror(&self, now: Timestamp) -> StoreResult<LifecycleFile> {
        self.read(move |conn| {
            let state = load_lifecycle(conn)?;
            let record = load_maintenance(conn)?;
            Ok(LifecycleFile::mirror(&state, now).with_maintenance(record.suppression()))
        })
        .await
    }
}

fn load_maintenance(conn: &Connection) -> rusqlite::Result<MaintenanceRecord> {
    let row = conn.query_row("SELECT current, last, version, updated_at FROM maintenance WHERE id = 1", [], |r| {
        Ok(MaintenanceRecord {
            current: json_column(0, r.get(0)?)?,
            last: json_column(1, r.get(1)?)?,
            version: r.get(2)?,
            updated_at: Some(Timestamp::from_millis(r.get(3)?)),
        })
    });
    match row {
        Ok(record) => Ok(record),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(MaintenanceRecord::default()),
        Err(other) => Err(other),
    }
}

fn json_column<T: serde::de::DeserializeOwned>(index: usize, raw: Option<String>) -> rusqlite::Result<Option<T>> {
    raw.map(|raw| serde_json::from_str(&raw))
        .transpose()
        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Text, Box::new(e)))
}

fn save_maintenance(tx: &Transaction<'_>, record: &MaintenanceRecord) -> rusqlite::Result<()> {
    // Domain 型別只含字串與數字，序列化不會失敗
    let current = record.current.as_ref().map(|m| serde_json::to_string(m).expect("維護可序列化"));
    let last = record.last.as_ref().map(|m| serde_json::to_string(m).expect("維護結果可序列化"));
    tx.execute(
        "INSERT INTO maintenance (id, current, last, version, updated_at) VALUES (1, ?1, ?2, ?3, ?4)
         ON CONFLICT(id) DO UPDATE SET current = excluded.current, last = excluded.last, version = excluded.version,
           updated_at = excluded.updated_at",
        rusqlite::params![current, last, record.version, record.updated_at.map(|t| t.as_millis()).unwrap_or_default()],
    )?;
    Ok(())
}
