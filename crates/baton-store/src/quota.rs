//! 額度觀測的持久化（票 27）。
//!
//! 規則都在 Domain（`baton_domain::quota`）；這裡只做「把觀測、用量與方案上限寫下來」與「載入每個（來源、視窗）
//! 最新的一筆交給 Domain 判斷」。每次記下用量或變更方案上限，都在同一個交易中重新推估並寫入新的推估觀測，
//! 所以帳號頁看到的推估一定是一筆有時間、有原始資料參照的觀測。

use std::collections::HashMap;

use baton_domain::profile::ProfileStatus;
use baton_domain::quota::{
    self, Confidence, LimitStatus, ObservationOrigin, PlanLimitError, PlanLimits, QuotaAssessment, QuotaObservation,
    QuotaSource, QuotaWindow, UsageRecord, QUOTA_POLL_INTERVAL_MILLIS, QUOTA_RETENTION_MILLIS,
};

use super::profiles::load_profiles;
use super::*;

/// 一個帳號的額度：Domain 的判斷與使用者設定的方案上限。
#[derive(Debug, Clone, PartialEq)]
pub struct AccountQuota {
    pub assessment: QuotaAssessment,
    pub plan_limits: PlanLimits,
}

/// 設定方案上限的失敗。
#[derive(Debug, thiserror::Error)]
pub enum QuotaApplyError {
    #[error(transparent)]
    Invalid(#[from] PlanLimitError),
    #[error("找不到帳號 Profile {0}")]
    ProfileNotFound(String),
    #[error("帳號 Profile {0} 已封存，不能設定方案上限")]
    ProfileArchived(String),
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl From<rusqlite::Error> for QuotaApplyError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error.into())
    }
}

impl Store {
    /// 使用中的 Profile 裡，距離上次輪詢已滿 5 分鐘（或從未輪詢）者，依建立順序。
    pub async fn quota_polls_due(&self, now: Timestamp) -> StoreResult<Vec<String>> {
        self.read(move |conn| {
            let polled: HashMap<String, i64> = conn
                .prepare("SELECT profile_id, polled_at FROM quota_poll")?
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?;
            Ok(load_profiles(conn)?
                .into_iter()
                .filter(|p| p.status == ProfileStatus::Active)
                .filter(|p| polled.get(&p.id).is_none_or(|at| now.as_millis() - at >= QUOTA_POLL_INTERVAL_MILLIS))
                .map(|p| p.id)
                .collect())
        })
        .await
    }

    /// 記下一次輪詢：外部觀測器的樣本（沒有外部觀測器時為 `None`）、輪詢時間、重新推估，並刪除超過保留期限的資料。
    pub async fn record_quota_poll(
        &self,
        polled: Vec<(String, Option<QuotaObservation>)>,
        now: Timestamp,
    ) -> StoreResult<()> {
        self.write(move |tx| {
            for (profile_id, sample) in &polled {
                if let Some(sample) = sample {
                    insert_observation(tx, sample)?;
                }
                tx.execute(
                    "INSERT INTO quota_poll (profile_id, polled_at) VALUES (?1, ?2)
                     ON CONFLICT(profile_id) DO UPDATE SET polled_at = excluded.polled_at",
                    rusqlite::params![profile_id, now.as_millis()],
                )?;
                record_estimates(tx, profile_id, now)?;
            }
            let before = now.as_millis() - QUOTA_RETENTION_MILLIS;
            // 每個（帳號、來源、視窗）最新的一筆不刪：它仍是目前的判斷依據
            tx.execute(
                "DELETE FROM quota_observation WHERE observed_at < ?1 AND seq NOT IN
                 (SELECT MAX(seq) FROM quota_observation GROUP BY profile_id, origin, quota_window)",
                [before],
            )?;
            tx.execute("DELETE FROM quota_usage WHERE reported_at < ?1", [before])?;
            Ok(())
        })
        .await
    }

    /// 記下 Adapter 解析到的限流訊號（已觀測）。
    pub async fn record_quota_signal(&self, observation: QuotaObservation) -> StoreResult<()> {
        self.write(move |tx| Ok(insert_observation(tx, &observation)?)).await
    }

    /// 記下一個 Run 回報的用量，並在同一個交易中重新推估。
    pub async fn record_quota_usage(
        &self,
        profile_id: String,
        run_id: String,
        tokens: u64,
        raw: String,
        now: Timestamp,
    ) -> StoreResult<()> {
        self.write(move |tx| {
            tx.execute(
                "INSERT INTO quota_usage (profile_id, run_id, tokens, reported_at, raw) VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![profile_id, run_id, tokens as i64, now.as_millis(), quota::truncate_raw(&raw)],
            )?;
            record_estimates(tx, &profile_id, now)?;
            Ok(())
        })
        .await
    }

    /// 設定一個使用中 Profile 的方案上限；有變更時追加事件並立即重新推估，相同時不寫入。
    pub async fn set_quota_plan_limits(
        &self,
        profile_id: String,
        limits: PlanLimits,
        now: Timestamp,
    ) -> Result<PlanLimits, QuotaApplyError> {
        self.write(move |tx| {
                let profile = load_profiles(tx)?.into_iter().find(|p| p.id == profile_id);
                match profile {
                    None => return Ok(Err(QuotaApplyError::ProfileNotFound(profile_id))),
                    Some(p) if p.status != ProfileStatus::Active => {
                        return Ok(Err(QuotaApplyError::ProfileArchived(profile_id)))
                    }
                    Some(_) => {}
                }
                let (current, version) = load_plan_limits(tx, &profile_id)?;
                let event = match quota::change_plan_limits(&profile_id, &current, version, limits, now) {
                    Err(invalid) => return Ok(Err(invalid.into())),
                    Ok(None) => return Ok(Ok(current)),
                    Ok(Some(event)) => event,
                };
                tx.execute(
                    "INSERT INTO quota_plan_limit (profile_id, five_hour_tokens, seven_day_tokens, version, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)
                     ON CONFLICT(profile_id) DO UPDATE SET five_hour_tokens = excluded.five_hour_tokens,
                       seven_day_tokens = excluded.seven_day_tokens, version = excluded.version,
                       updated_at = excluded.updated_at",
                    rusqlite::params![
                        profile_id,
                        limits.five_hour_tokens.map(|t| t as i64),
                        limits.seven_day_tokens.map(|t| t as i64),
                        event.aggregate_version,
                        now.as_millis(),
                    ],
                )?;
                append_event(tx, &event)?;
                record_estimates(tx, &profile_id, now)?;
                Ok(Ok(limits))
            })
            .await?
    }

    /// 全部 Profile 目前的額度判斷（沒有任何觀測的 Profile 也有一筆「未知」）。
    pub async fn quota_status(&self, now: Timestamp) -> StoreResult<HashMap<String, AccountQuota>> {
        self.read(move |conn| Ok(load_quota_status(conn, now)?)).await
    }
}

/// 全部 Profile 目前的額度判斷（在呼叫端的交易或連線中）。
pub(crate) fn load_quota_status(conn: &Connection, now: Timestamp) -> rusqlite::Result<HashMap<String, AccountQuota>> {
    let mut latest: HashMap<String, Vec<QuotaObservation>> = HashMap::new();
    let mut statement = conn.prepare(&format!(
        "SELECT {OBSERVATION_COLUMNS} FROM quota_observation WHERE seq IN
         (SELECT MAX(seq) FROM quota_observation GROUP BY profile_id, origin, quota_window)"
    ))?;
    for observation in statement.query_map([], observation_row)? {
        let observation = observation?;
        latest.entry(observation.profile_id.clone()).or_default().push(observation);
    }
    let mut status = HashMap::new();
    for profile in load_profiles(conn)? {
        let observations = latest.remove(&profile.id).unwrap_or_default();
        let (plan_limits, _) = load_plan_limits(conn, &profile.id)?;
        status.insert(profile.id, AccountQuota { assessment: quota::assess(&observations, now), plan_limits });
    }
    Ok(status)
}

const OBSERVATION_COLUMNS: &str = "profile_id, origin, source, quota_window, status, used, tokens, limit_tokens, \
     reset_at, observed_at, confidence, raw_ref, raw";

fn insert_observation(tx: &Transaction<'_>, o: &QuotaObservation) -> rusqlite::Result<()> {
    tx.execute(
        &format!("INSERT INTO quota_observation ({OBSERVATION_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)"),
        rusqlite::params![
            o.profile_id,
            o.origin.as_str(),
            o.source.as_str(),
            o.window.map(QuotaWindow::as_str),
            o.status.map(LimitStatus::as_str),
            o.used,
            o.tokens.map(|t| t as i64),
            o.limit_tokens.map(|t| t as i64),
            o.reset_at.map(Timestamp::as_millis),
            o.observed_at.as_millis(),
            o.confidence.as_str(),
            o.raw_ref,
            o.raw,
        ],
    )?;
    Ok(())
}

fn parse_column<T>(index: usize, raw: &str, parse: impl Fn(&str) -> Option<T>) -> rusqlite::Result<T> {
    parse(raw).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Text, format!("未知的值 {raw}").into())
    })
}

fn observation_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<QuotaObservation> {
    let window: Option<String> = r.get(3)?;
    let status: Option<String> = r.get(4)?;
    Ok(QuotaObservation {
        profile_id: r.get(0)?,
        origin: parse_column(1, &r.get::<_, String>(1)?, ObservationOrigin::parse)?,
        source: parse_column(2, &r.get::<_, String>(2)?, QuotaSource::parse)?,
        window: window.map(|w| parse_column(3, &w, QuotaWindow::parse)).transpose()?,
        status: status.map(|s| parse_column(4, &s, LimitStatus::parse)).transpose()?,
        used: r.get(5)?,
        tokens: r.get::<_, Option<i64>>(6)?.map(|t| t as u64),
        limit_tokens: r.get::<_, Option<i64>>(7)?.map(|t| t as u64),
        reset_at: r.get::<_, Option<i64>>(8)?.map(Timestamp::from_millis),
        observed_at: Timestamp::from_millis(r.get(9)?),
        confidence: parse_column(10, &r.get::<_, String>(10)?, Confidence::parse)?,
        raw_ref: r.get(11)?,
        raw: r.get(12)?,
    })
}

/// 方案上限與其版本；沒有設定過為（全部未設定, 0）。
fn load_plan_limits(conn: &Connection, profile_id: &str) -> rusqlite::Result<(PlanLimits, i64)> {
    let row = conn.query_row(
        "SELECT five_hour_tokens, seven_day_tokens, version FROM quota_plan_limit WHERE profile_id = ?1",
        [profile_id],
        |r| {
            Ok((
                PlanLimits {
                    five_hour_tokens: r.get::<_, Option<i64>>(0)?.map(|t| t as u64),
                    seven_day_tokens: r.get::<_, Option<i64>>(1)?.map(|t| t as u64),
                },
                r.get(2)?,
            ))
        },
    );
    match row {
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok((PlanLimits::default(), 0)),
        other => other,
    }
}

/// 依七日內的本機用量與方案上限，為兩個視窗各寫入一筆推估觀測。
fn record_estimates(tx: &Transaction<'_>, profile_id: &str, now: Timestamp) -> rusqlite::Result<()> {
    let since = now.as_millis() - QuotaWindow::SevenDay.duration_millis();
    let usage: Vec<UsageRecord> = tx
        .prepare("SELECT seq, tokens, reported_at FROM quota_usage WHERE profile_id = ?1 AND reported_at > ?2")?
        .query_map(rusqlite::params![profile_id, since], |r| {
            Ok(UsageRecord {
                seq: r.get(0)?,
                tokens: r.get::<_, i64>(1)? as u64,
                reported_at: Timestamp::from_millis(r.get(2)?),
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    let (limits, _) = load_plan_limits(tx, profile_id)?;
    for window in QuotaWindow::ALL {
        insert_observation(tx, &quota::estimate(profile_id, window, &usage, &limits, now))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
