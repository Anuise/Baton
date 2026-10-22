//! account_profile 與首次啟動精靈狀態的持久化。
//!
//! 規則都在 Domain（`baton_domain::profile`、`baton_domain::setup`）；這裡只做「在同一個交易中載入目前狀態 →
//! 交給 Domain 判定 → 寫入新狀態與事件」。只保存核對結果，不保存任何登入資料。

use baton_domain::profile::{self, AccountProfile, ProfileCommand, ProfileError, ProfileStatus, Verification};
use baton_domain::setup;

use super::*;

/// 套用 account_profile 命令的失敗：Domain 拒絕（使用者可修正）或儲存層錯誤。
#[derive(Debug, thiserror::Error)]
pub enum ProfileApplyError {
    #[error(transparent)]
    Rejected(#[from] ProfileError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl From<rusqlite::Error> for ProfileApplyError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error.into())
    }
}

impl Store {
    /// 套用一個 account_profile 命令：在同一個交易中載入目前狀態、交給 Domain 判定，
    /// 再寫入新狀態並追加事件。Domain 拒絕（例如第 9 個 Profile）時不寫入任何東西。
    pub async fn apply_profile_command(
        &self,
        command: ProfileCommand,
        now: Timestamp,
    ) -> Result<AccountProfile, ProfileApplyError> {
        let outcome = self
            .write(move |tx| {
                let existing = load_profiles(tx)?;
                match profile::decide(&existing, command, now) {
                    Err(rejected) => Ok(Err(rejected)),
                    Ok(decision) => {
                        save_profile(tx, &decision.profile)?;
                        for event in &decision.events {
                            append_event(tx, event)?;
                        }
                        Ok(Ok(decision.profile))
                    }
                }
            })
            .await?;
        Ok(outcome?)
    }

    /// 全部 account_profile（含已封存），依建立順序排列。
    pub async fn list_profiles(&self) -> StoreResult<Vec<AccountProfile>> {
        self.read(|conn| Ok(load_profiles(conn)?)).await
    }

    /// 背景執行行為告知被確認的時間；還沒確認為 `None`。
    pub async fn background_notice_acknowledged_at(&self) -> StoreResult<Option<Timestamp>> {
        self.read(|conn| Ok(load_notice_acknowledgement(conn)?)).await
    }

    /// 確認背景執行行為告知；重複確認不改變時間，也不追加事件。
    pub async fn acknowledge_background_notice(&self, now: Timestamp) -> StoreResult<Timestamp> {
        self.write(move |tx| {
            let acknowledgement = setup::acknowledge_background_notice(load_notice_acknowledgement(tx)?, now);
            if let Some(event) = &acknowledgement.event {
                tx.execute(
                    "INSERT INTO setup_state (key, acknowledged_at) VALUES (?1, ?2)",
                    rusqlite::params![setup::BACKGROUND_NOTICE_ID, acknowledgement.acknowledged_at.as_millis()],
                )?;
                append_event(tx, event)?;
            }
            Ok(acknowledgement.acknowledged_at)
        })
        .await
    }
}

fn load_notice_acknowledgement(conn: &Connection) -> rusqlite::Result<Option<Timestamp>> {
    let millis: Option<i64> = conn
        .query_row("SELECT acknowledged_at FROM setup_state WHERE key = ?1", [setup::BACKGROUND_NOTICE_ID], |r| r.get(0))
        .map(Some)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(other),
        })?;
    Ok(millis.map(Timestamp::from_millis))
}

fn load_profiles(conn: &Connection) -> rusqlite::Result<Vec<AccountProfile>> {
    let mut statement = conn.prepare(
        "SELECT id, name, config_dir, expected_account, status, verification, version, created_at, archived_at
         FROM account_profile ORDER BY rowid",
    )?;
    let rows = statement.query_map([], |r| {
        let status: String = r.get(4)?;
        let verification: Option<String> = r.get(5)?;
        Ok(AccountProfile {
            id: r.get(0)?,
            name: r.get(1)?,
            config_dir: r.get(2)?,
            expected_account: r.get(3)?,
            status: ProfileStatus::parse(&status).ok_or_else(|| {
                rusqlite::Error::FromSqlConversionFailure(4, rusqlite::types::Type::Text, "未知的 Profile 狀態".into())
            })?,
            verification: verification.map(|json| json_column::<Verification>(5, &json)).transpose()?,
            version: r.get(6)?,
            created_at: Timestamp::from_millis(r.get(7)?),
            archived_at: r.get::<_, Option<i64>>(8)?.map(Timestamp::from_millis),
        })
    })?;
    rows.collect()
}

fn save_profile(tx: &Transaction<'_>, profile: &AccountProfile) -> rusqlite::Result<()> {
    let verification = profile.verification.as_ref().map(|v| serde_json::to_string(v).expect("Verification 可序列化"));
    tx.execute(
        "INSERT INTO account_profile
         (id, name, config_dir, expected_account, status, verification, version, created_at, archived_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(id) DO UPDATE SET
           expected_account = excluded.expected_account, status = excluded.status,
           verification = excluded.verification, version = excluded.version, archived_at = excluded.archived_at",
        rusqlite::params![
            profile.id,
            profile.name,
            profile.config_dir,
            profile.expected_account,
            profile.status.as_str(),
            verification,
            profile.version,
            profile.created_at.as_millis(),
            profile.archived_at.map(|t| t.as_millis()),
        ],
    )?;
    Ok(())
}
