//! 備份與還原演練的純邏輯（票 40，spec D-18）。
//!
//! - **觸發**：每日一次、migration 前，以及重要裁定或 Stage 完成後；重要裁定與 Stage 完成的要求在同一小時內合併為一次
//!   （[`due_backup`]）。migration 前的備份由 Store 開啟資料庫時直接建立，不經過這裡的判斷。
//! - **保留**：最近 7 個有備份的日子各留最新的一份、最近 4 個有備份的週各留最新的一份，另外永遠留下最新一份 migration 前的備份
//!   （[`retention`]）；其餘刪除。日與週都以 UTC 計（spec D-33），週從星期一開始。
//! - **演練**：每週一次（[`drill_due`]）在隔離目錄還原最新的備份，核對資料庫完整性、成果雜湊與關聯（[`check_artifacts`]）。
//!
//! 檔案與資料庫的操作在 Store；這裡只有判斷與型別。

use std::collections::{BTreeMap, BTreeSet, HashSet};

use serde::{Deserialize, Serialize};

use crate::Timestamp;

const DAY_MILLIS: i64 = 24 * 60 * 60 * 1000;

/// 每日備份的間隔：距離上一份備份（不論觸發原因）滿 24 小時就再備份一次。
pub const DAILY_INTERVAL_MILLIS: i64 = DAY_MILLIS;
/// 重要裁定與 Stage 完成的備份要求在這段時間內合併為一次（spec D-18 的「同一小時內」）。
pub const MILESTONE_COALESCE_MILLIS: i64 = 60 * 60 * 1000;
/// 還原演練的間隔。
pub const DRILL_INTERVAL_MILLIS: i64 = 7 * DAY_MILLIS;
/// 保留幾個日子的每日備份。
pub const KEEP_DAILY: usize = 7;
/// 保留幾週的每週備份。
pub const KEEP_WEEKLY: usize = 4;

/// 這份備份為什麼被建立。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackupTrigger {
    Daily,
    PreMigration,
    /// 重要裁定或 Stage 完成。
    Milestone,
    /// 套用更新之前（票 34，spec D-19）：在停止流程之後建立，依一般規則保留。
    PreUpdate,
}

impl BackupTrigger {
    pub fn as_str(self) -> &'static str {
        match self {
            BackupTrigger::Daily => "daily",
            BackupTrigger::PreMigration => "pre_migration",
            BackupTrigger::Milestone => "milestone",
            BackupTrigger::PreUpdate => "pre_update",
        }
    }
}

/// 備份 manifest 中的一個成果檔案：相對於成果儲存根目錄的路徑、大小與 SHA-256（取自同一份資料庫快照）。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ArtifactFileEntry {
    pub storage_path: String,
    pub size_bytes: i64,
    pub sha256: String,
}

/// 一份備份的描述，與資料庫快照一起存放在備份目錄中。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupManifest {
    pub id: String,
    pub trigger: BackupTrigger,
    pub created_at: Timestamp,
    /// 快照的 schema 版本（`schema_migrations` 的最大版本）。
    pub schema_version: usize,
    pub database_size_bytes: i64,
    pub database_sha256: String,
    /// 快照中登記的全部成果版本（依路徑排序）。
    pub artifacts: Vec<ArtifactFileEntry>,
}

/// 備份的識別：建立時間（固定 13 位數的毫秒，依名稱排序就是依時間排序）加上觸發原因。
pub fn backup_id(trigger: BackupTrigger, created_at: Timestamp) -> String {
    format!("{:013}-{}", created_at.as_millis().max(0), trigger.as_str())
}

/// 這份備份為什麼被保留。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetentionReason {
    Daily,
    Weekly,
    LatestPreMigration,
}

fn utc_day(at: Timestamp) -> i64 {
    at.as_millis().div_euclid(DAY_MILLIS)
}

/// 1970-01-01 是星期四；加 3 天讓每週從星期一開始。
fn utc_week(at: Timestamp) -> i64 {
    (utc_day(at) + 3).div_euclid(7)
}

/// 保留哪些備份與原因；不在結果中的備份應該刪除。
pub fn retention(backups: &[BackupManifest]) -> BTreeMap<String, BTreeSet<RetentionReason>> {
    let mut newest_first: Vec<&BackupManifest> = backups.iter().collect();
    newest_first.sort_by(|a, b| (b.created_at, &b.id).cmp(&(a.created_at, &a.id)));

    let mut kept: BTreeMap<String, BTreeSet<RetentionReason>> = BTreeMap::new();
    let (mut days, mut weeks) = (HashSet::new(), HashSet::new());
    let mut pre_migration_kept = false;
    for backup in newest_first {
        let mut reasons = BTreeSet::new();
        let day = utc_day(backup.created_at);
        if !days.contains(&day) && days.len() < KEEP_DAILY {
            days.insert(day);
            reasons.insert(RetentionReason::Daily);
        }
        let week = utc_week(backup.created_at);
        if !weeks.contains(&week) && weeks.len() < KEEP_WEEKLY {
            weeks.insert(week);
            reasons.insert(RetentionReason::Weekly);
        }
        if backup.trigger == BackupTrigger::PreMigration && !pre_migration_kept {
            pre_migration_kept = true;
            reasons.insert(RetentionReason::LatestPreMigration);
        }
        if !reasons.is_empty() {
            kept.insert(backup.id.clone(), reasons);
        }
    }
    kept
}

/// 現在該不該建立備份，以及原因。
///
/// - `latest`：最新一份備份（不論原因）的建立時間。
/// - `milestone_requested`：尚未被備份涵蓋的重要裁定或 Stage 完成要求（最早的一次）。
///
/// 重要裁定的要求在距離上一份備份滿一小時後才執行，所以同一小時內的多次要求只產生一份備份，而且這份備份涵蓋全部要求。
pub fn due_backup(
    latest: Option<Timestamp>,
    milestone_requested: Option<Timestamp>,
    now: Timestamp,
) -> Option<BackupTrigger> {
    let elapsed = |interval: i64| latest.is_none_or(|at| now.as_millis() - at.as_millis() >= interval);
    if milestone_requested.is_some() && elapsed(MILESTONE_COALESCE_MILLIS) {
        return Some(BackupTrigger::Milestone);
    }
    elapsed(DAILY_INTERVAL_MILLIS).then_some(BackupTrigger::Daily)
}

/// 現在該不該做還原演練：從沒做過，或上次演練已滿一週。
pub fn drill_due(last_drill: Option<Timestamp>, now: Timestamp) -> bool {
    last_drill.is_none_or(|at| now.as_millis() - at.as_millis() >= DRILL_INTERVAL_MILLIS)
}

/// 還原演練的一項核對。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DrillCheckKind {
    /// 檔案能完整還原到乾淨目錄。
    Restore,
    /// SQLite `integrity_check`。
    DatabaseIntegrity,
    /// SQLite `foreign_key_check`：資料表之間的關聯完整。
    ForeignKeys,
    /// 還原出來的每個成果檔案，大小與 SHA-256 都與 manifest 相同。
    ArtifactHashes,
    /// 資料庫登記的成果版本與 manifest 一一對應。
    ArtifactRelations,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DrillCheck {
    pub kind: DrillCheckKind,
    pub passed: bool,
    pub detail: String,
}

/// 一次還原演練的結果；設定頁顯示最新一次。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DrillReport {
    /// 被演練的備份；連一份備份都沒有時為 `None`。
    pub backup_id: Option<String>,
    pub backup_created_at: Option<Timestamp>,
    pub performed_at: Timestamp,
    /// 全部核對都通過。
    pub passed: bool,
    pub checks: Vec<DrillCheck>,
    /// 還原時被標為中斷的 Run（備份當下尚未結束、還原後不會自動重做）。
    pub interrupted_runs: usize,
}

impl DrillReport {
    pub fn new(
        backup: Option<&BackupManifest>,
        performed_at: Timestamp,
        checks: Vec<DrillCheck>,
        interrupted_runs: usize,
    ) -> Self {
        Self {
            backup_id: backup.map(|b| b.id.clone()),
            backup_created_at: backup.map(|b| b.created_at),
            performed_at,
            passed: !checks.is_empty() && checks.iter().all(|c| c.passed),
            checks,
            interrupted_runs,
        }
    }
}

/// 核對成果：`restored` 是還原目錄中實際讀到的檔案（路徑 → 大小與 SHA-256），`recorded` 是還原後資料庫登記的成果版本。
pub fn check_artifacts(
    manifest: &[ArtifactFileEntry],
    recorded: &[ArtifactFileEntry],
    restored: &BTreeMap<String, (i64, String)>,
) -> Vec<DrillCheck> {
    let mismatched: Vec<&str> = manifest
        .iter()
        .filter(|entry| restored.get(&entry.storage_path) != Some(&(entry.size_bytes, entry.sha256.clone())))
        .map(|entry| entry.storage_path.as_str())
        .collect();
    let hashes = DrillCheck {
        kind: DrillCheckKind::ArtifactHashes,
        passed: mismatched.is_empty(),
        detail: if mismatched.is_empty() {
            format!("{} 個成果檔案的大小與雜湊都一致", manifest.len())
        } else {
            format!("{} 個成果檔案缺少或與 manifest 不一致：{}", mismatched.len(), mismatched.join("、"))
        },
    };

    let in_manifest: BTreeSet<&ArtifactFileEntry> = manifest.iter().collect();
    let in_database: BTreeSet<&ArtifactFileEntry> = recorded.iter().collect();
    let differing = in_manifest.symmetric_difference(&in_database).count();
    let relations = DrillCheck {
        kind: DrillCheckKind::ArtifactRelations,
        passed: differing == 0,
        detail: if differing == 0 {
            format!("資料庫登記的 {} 個成果版本都在 manifest 中", recorded.len())
        } else {
            format!("資料庫登記的成果版本與 manifest 有 {differing} 筆不一致")
        },
    };
    vec![hashes, relations]
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: i64 = 60 * 60 * 1000;
    /// 2026-10-19（星期一）00:00 UTC。
    const MONDAY: i64 = 1_792_368_000_000;

    fn at(millis: i64) -> Timestamp {
        Timestamp::from_millis(millis)
    }

    fn backup(trigger: BackupTrigger, millis: i64) -> BackupManifest {
        BackupManifest {
            id: backup_id(trigger, at(millis)),
            trigger,
            created_at: at(millis),
            schema_version: 1,
            database_size_bytes: 0,
            database_sha256: String::new(),
            artifacts: Vec::new(),
        }
    }

    #[test]
    fn monday_constant_is_a_monday_in_utc() {
        assert_eq!(utc_week(at(MONDAY)), utc_week(at(MONDAY + 6 * DAY_MILLIS + 23 * HOUR)));
        assert_ne!(utc_week(at(MONDAY)), utc_week(at(MONDAY - 1)));
    }

    #[test]
    fn backup_ids_sort_in_creation_order() {
        let early = backup_id(BackupTrigger::Milestone, at(999));
        let late = backup_id(BackupTrigger::Daily, at(1_000_000));
        assert!(early < late);
        assert_eq!(late, "0000001000000-daily");
    }

    #[test]
    fn first_backup_is_due_immediately_as_daily() {
        assert_eq!(due_backup(None, None, at(MONDAY)), Some(BackupTrigger::Daily));
    }

    #[test]
    fn daily_backup_is_due_after_twenty_four_hours() {
        let latest = Some(at(MONDAY));
        assert_eq!(due_backup(latest, None, at(MONDAY + 24 * HOUR - 1)), None);
        assert_eq!(due_backup(latest, None, at(MONDAY + 24 * HOUR)), Some(BackupTrigger::Daily));
    }

    #[test]
    fn milestone_runs_at_once_when_no_backup_in_the_last_hour() {
        let latest = Some(at(MONDAY));
        let now = at(MONDAY + 2 * HOUR);
        assert_eq!(due_backup(latest, Some(now), now), Some(BackupTrigger::Milestone));
    }

    #[test]
    fn milestones_within_the_same_hour_wait_and_coalesce() {
        let latest = Some(at(MONDAY));
        let requested = Some(at(MONDAY + 10 * 60 * 1000));
        assert_eq!(due_backup(latest, requested, at(MONDAY + 59 * 60 * 1000)), None);
        assert_eq!(due_backup(latest, requested, at(MONDAY + HOUR)), Some(BackupTrigger::Milestone));
    }

    #[test]
    fn drill_is_due_weekly() {
        assert!(drill_due(None, at(MONDAY)));
        assert!(!drill_due(Some(at(MONDAY)), at(MONDAY + 7 * DAY_MILLIS - 1)));
        assert!(drill_due(Some(at(MONDAY)), at(MONDAY + 7 * DAY_MILLIS)));
    }

    #[test]
    fn retention_keeps_newest_per_day_for_seven_days_and_per_week_for_four_weeks() {
        // 連續 40 天，每天 01:00 與 13:00 各一份
        let backups: Vec<_> = (0..40)
            .flat_map(|d| [1, 13].map(|h| backup(BackupTrigger::Daily, MONDAY + d * DAY_MILLIS + h * HOUR)))
            .collect();
        let kept = retention(&backups);

        let daily: Vec<_> = kept.iter().filter(|(_, r)| r.contains(&RetentionReason::Daily)).collect();
        assert_eq!(daily.len(), KEEP_DAILY);
        let weekly: Vec<_> = kept.iter().filter(|(_, r)| r.contains(&RetentionReason::Weekly)).collect();
        assert_eq!(weekly.len(), KEEP_WEEKLY);
        // 每日保留的都是當天最新（13:00）的那份
        for (id, _) in daily {
            assert!(backups.iter().any(|b| &b.id == id && (b.created_at.as_millis() - MONDAY) % DAY_MILLIS == 13 * HOUR));
        }
        // 最新的一份同時是每日與每週
        let newest = &backups.last().unwrap().id;
        assert_eq!(kept[newest], BTreeSet::from([RetentionReason::Daily, RetentionReason::Weekly]));
        // 每週保留的是各週最新的一份：第 39 天（最後一週的星期五）與第 34、27、20 天（各週的星期日）
        let weekly_days: BTreeSet<i64> = kept
            .iter()
            .filter(|(_, r)| r.contains(&RetentionReason::Weekly))
            .map(|(id, _)| (backups.iter().find(|b| &b.id == id).unwrap().created_at.as_millis() - MONDAY) / DAY_MILLIS)
            .collect();
        assert_eq!(weekly_days, BTreeSet::from([20, 27, 34, 39]));
        assert!(kept.len() < backups.len());
    }

    #[test]
    fn retention_always_keeps_the_latest_pre_migration_backup() {
        let backups = vec![
            backup(BackupTrigger::PreMigration, MONDAY + HOUR),
            backup(BackupTrigger::PreMigration, MONDAY + 2 * HOUR),
            backup(BackupTrigger::Daily, MONDAY + 3 * HOUR),
        ];
        let kept = retention(&backups);
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[&backups[1].id], BTreeSet::from([RetentionReason::LatestPreMigration]));
        assert!(!kept.contains_key(&backups[0].id));
    }

    fn entry(path: &str, size: i64, sha: &str) -> ArtifactFileEntry {
        ArtifactFileEntry { storage_path: path.into(), size_bytes: size, sha256: sha.into() }
    }

    #[test]
    fn artifact_checks_pass_when_files_and_records_match_the_manifest() {
        let manifest = vec![entry("ws/a", 3, "x"), entry("ws/b", 1, "y")];
        let restored = BTreeMap::from([("ws/a".to_string(), (3, "x".to_string())), ("ws/b".to_string(), (1, "y".to_string()))]);
        let checks = check_artifacts(&manifest, &manifest, &restored);
        assert!(checks.iter().all(|c| c.passed), "{checks:?}");
    }

    #[test]
    fn artifact_checks_report_missing_tampered_and_unrelated_entries() {
        let manifest = vec![entry("ws/a", 3, "x"), entry("ws/b", 1, "y")];
        let restored = BTreeMap::from([("ws/a".to_string(), (3, "tampered".to_string()))]);
        let recorded = vec![entry("ws/a", 3, "x")];
        let checks = check_artifacts(&manifest, &recorded, &restored);
        assert_eq!(checks[0].kind, DrillCheckKind::ArtifactHashes);
        assert!(!checks[0].passed);
        assert!(checks[0].detail.contains("ws/a") && checks[0].detail.contains("ws/b"), "{}", checks[0].detail);
        assert_eq!(checks[1].kind, DrillCheckKind::ArtifactRelations);
        assert!(!checks[1].passed);
    }

    #[test]
    fn drill_report_without_checks_is_not_a_pass() {
        assert!(!DrillReport::new(None, at(1), Vec::new(), 0).passed);
        let ok = DrillCheck { kind: DrillCheckKind::Restore, passed: true, detail: String::new() };
        assert!(DrillReport::new(None, at(1), vec![ok], 0).passed);
    }
}
