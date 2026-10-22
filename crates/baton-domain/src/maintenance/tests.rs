//! 更新檢查與維護（更新、解除安裝）的單元測試（票 34）。純 Domain，不經 HTTP。

use super::*;
use crate::lifecycle::{LifecyclePhase, LifecycleState};

const T0: Timestamp = Timestamp::from_millis(1_000);
const T1: Timestamp = Timestamp::from_millis(2_000);

fn release(version: &str) -> ReleaseInfo {
    ReleaseInfo {
        version: version.into(),
        notes: Some("修正".into()),
        published_at: None,
        url: format!("https://example.invalid/Baton_{version}_x64-setup.exe"),
        signature: "sig".into(),
    }
}

fn running() -> LifecycleState {
    LifecycleState::default()
}

fn stopping() -> LifecycleState {
    LifecycleState { phase: LifecyclePhase::Stopping, version: 1, ..LifecycleState::default() }
}

// ---------------------------------------------------------------- 版本比較

#[test]
fn newer_versions_compare_numerically_and_a_release_beats_its_prerelease() {
    assert!(is_newer("0.2.0", "0.1.0"));
    assert!(is_newer("0.10.0", "0.9.9"), "以數字比較，不是字串");
    assert!(is_newer("1.0.0", "1.0.0-beta.2"), "正式版比同號的預覽版新");
    assert!(is_newer("1.0.0-beta.10", "1.0.0-beta.2"));
    assert!(is_newer("v1.2.3", "1.2.2"), "允許前綴 v");
    assert!(!is_newer("0.1.0", "0.1.0"));
    assert!(!is_newer("0.1.0", "0.2.0"));
    assert!(!is_newer("1.0.0-beta.1", "1.0.0"));
    assert!(!is_newer("0.1.0+build.7", "0.1.0"), "建置中繼資料不影響先後");
    assert!(!is_newer("不是版本", "0.1.0"), "看不懂的版本不視為較新");
}

// ---------------------------------------------------------------- 更新檢查：啟動時與每日一次，只通知

#[test]
fn a_new_check_is_due_at_startup_and_then_once_a_day() {
    let check = UpdateCheck::new("0.1.0");
    assert_eq!(check.status, UpdateCheckStatus::NotChecked);
    assert!(check.due(T0), "啟動後第一次一定要檢查");

    let checked = record_check(&check, CheckResult::Latest(release("0.1.0")), T0);
    assert_eq!(checked.status, UpdateCheckStatus::UpToDate);
    assert_eq!(checked.checked_at, Some(T0));
    assert_eq!(checked.next_check_at, Some(T0.plus_millis(UPDATE_CHECK_INTERVAL_MS)));
    assert!(!checked.due(T0.plus_millis(UPDATE_CHECK_INTERVAL_MS - 1)));
    assert!(checked.due(T0.plus_millis(UPDATE_CHECK_INTERVAL_MS)));
}

#[test]
fn a_newer_release_is_only_reported_as_available() {
    let checked = record_check(&UpdateCheck::new("0.1.0"), CheckResult::Latest(release("0.2.0")), T0);
    assert_eq!(checked.status, UpdateCheckStatus::Available);
    assert_eq!(checked.available.as_ref().map(|r| r.version.as_str()), Some("0.2.0"));
    assert_eq!(checked.error, None);
}

#[test]
fn a_failed_check_keeps_the_known_update_and_retries_the_next_day() {
    let available = record_check(&UpdateCheck::new("0.1.0"), CheckResult::Latest(release("0.2.0")), T0);
    let failed = record_check(&available, CheckResult::Failed("連線逾時".into()), T1);
    assert_eq!(failed.status, UpdateCheckStatus::Failed);
    assert_eq!(failed.error.as_deref(), Some("連線逾時"));
    assert_eq!(failed.available, available.available, "之前看到的新版本仍可更新");
    assert_eq!(failed.checked_at, Some(T1));
    assert_eq!(failed.next_check_at, Some(T1.plus_millis(UPDATE_CHECK_INTERVAL_MS)));

    let unconfigured = record_check(&UpdateCheck::new("0.1.0"), CheckResult::Unconfigured, T0);
    assert_eq!(unconfigured.status, UpdateCheckStatus::Unconfigured);
    assert_eq!(unconfigured.available, None);
}

#[test]
fn an_older_or_equal_release_clears_a_previously_seen_update() {
    let available = record_check(&UpdateCheck::new("0.1.0"), CheckResult::Latest(release("0.2.0")), T0);
    let withdrawn = record_check(&available, CheckResult::Latest(release("0.1.0")), T1);
    assert_eq!(withdrawn.status, UpdateCheckStatus::UpToDate);
    assert_eq!(withdrawn.available, None);
}

// ---------------------------------------------------------------- 維護：開始

#[test]
fn update_maintenance_starts_in_stopping_and_mirrors_a_suppression() {
    let (record, event) = begin(
        &MaintenanceRecord::default(),
        &running(),
        MaintenanceRequest::update("0.1.0", Some("0.2.0".into()), Some("C:\\data\\updates\\setup.exe".into())),
        T0,
    )
    .unwrap()
    .expect("應開始維護");
    let current = record.current.as_ref().unwrap();
    assert_eq!(current.kind, MaintenanceKind::Update);
    assert_eq!(current.phase, MaintenancePhase::Stopping);
    assert_eq!(current.from_version, "0.1.0");
    assert_eq!(current.target_version.as_deref(), Some("0.2.0"));
    assert_eq!(record.version, 1);
    assert_eq!(event.event_type, EVENT_MAINTENANCE_REQUESTED);
    assert_eq!(event.aggregate_type, MAINTENANCE_AGGREGATE);

    let suppression = record.suppression().expect("維護期間有抑制");
    assert_eq!(suppression.kind, "update");
    assert_eq!(suppression.requested_at, T0);
}

#[test]
fn maintenance_is_idempotent_for_the_same_kind_and_rejects_a_different_one() {
    let request = MaintenanceRequest::uninstall("0.1.0");
    let (record, _) = begin(&MaintenanceRecord::default(), &running(), request.clone(), T0).unwrap().unwrap();
    // 開始之後停止意圖就生效了：同一種維護的重複要求沒有任何變動
    assert_eq!(begin(&record, &stopping(), request, T1).unwrap(), None);
    assert_eq!(
        begin(&record, &stopping(), MaintenanceRequest::update("0.1.0", None, None), T1),
        Err(MaintenanceError::InProgress(MaintenanceKind::Uninstall))
    );
}

#[test]
fn maintenance_cannot_start_while_a_full_exit_is_in_effect() {
    let rejected = begin(&MaintenanceRecord::default(), &stopping(), MaintenanceRequest::uninstall("0.1.0"), T0);
    assert_eq!(rejected, Err(MaintenanceError::StopIntentActive));
    assert_eq!(MaintenanceError::StopIntentActive.code(), "stop_intent_active");
    assert_eq!(MaintenanceError::InProgress(MaintenanceKind::Update).code(), "maintenance_in_progress");
}

// ---------------------------------------------------------------- 維護：停止流程之後

#[test]
fn after_the_stop_flow_maintenance_becomes_ready_or_failed_once() {
    let (record, _) =
        begin(&MaintenanceRecord::default(), &running(), MaintenanceRequest::update("0.1.0", Some("0.2.0".into()), None), T0)
            .unwrap()
            .unwrap();

    let (ready, event) =
        complete(&record, AfterStop::Ready { backup_id: Some("b1".into()), note: None }, T1).expect("停止中才能完成");
    let current = ready.current.as_ref().unwrap();
    assert_eq!(current.phase, MaintenancePhase::Ready);
    assert_eq!(current.backup_id.as_deref(), Some("b1"));
    assert_eq!(current.completed_at, Some(T1));
    assert_eq!(event.event_type, EVENT_MAINTENANCE_READY);
    assert!(ready.suppression().is_some(), "準備套用時仍維持抑制");
    assert_eq!(complete(&ready, AfterStop::Failed("x".into()), T1), None, "只完成一次");

    let (failed, event) = complete(&record, AfterStop::Failed("備份失敗".into()), T1).unwrap();
    let current = failed.current.as_ref().unwrap();
    assert_eq!(current.phase, MaintenancePhase::Failed);
    assert_eq!(current.message.as_deref(), Some("備份失敗"));
    assert_eq!(event.event_type, EVENT_MAINTENANCE_FAILED);
}

// ---------------------------------------------------------------- 維護：下次手動開啟時結束

#[test]
fn reopening_finishes_maintenance_and_reports_whether_the_update_was_applied() {
    let (record, _) =
        begin(&MaintenanceRecord::default(), &running(), MaintenanceRequest::update("0.1.0", Some("0.2.0".into()), None), T0)
            .unwrap()
            .unwrap();
    let (ready, _) = complete(&record, AfterStop::Ready { backup_id: Some("b1".into()), note: None }, T0).unwrap();

    let (finished, outcome, event) = finish(&ready, "0.2.0", T1).expect("有維護才結束");
    assert_eq!(finished.current, None);
    assert_eq!(finished.suppression(), None, "結束後不再抑制");
    assert_eq!(outcome.applied, Some(true));
    assert_eq!(outcome.running_version, "0.2.0");
    assert_eq!(outcome.backup_id.as_deref(), Some("b1"));
    assert_eq!(finished.last, Some(outcome));
    assert_eq!(event.event_type, EVENT_MAINTENANCE_FINISHED);
    assert_eq!(finish(&finished, "0.2.0", T1), None);

    let (_, not_applied, _) = finish(&ready, "0.1.0", T1).unwrap();
    assert_eq!(not_applied.applied, Some(false), "版本沒變：更新沒有套用");

    // 安裝程式直接執行的更新（不知道目標版本）：版本變了就算套用
    let (manual, _) =
        begin(&MaintenanceRecord::default(), &running(), MaintenanceRequest::update("0.1.0", None, None), T0).unwrap().unwrap();
    assert_eq!(finish(&manual, "0.3.0", T1).unwrap().1.applied, Some(true));
    assert_eq!(finish(&manual, "0.1.0", T1).unwrap().1.applied, Some(false));

    let (uninstall, _) =
        begin(&MaintenanceRecord::default(), &running(), MaintenanceRequest::uninstall("0.1.0"), T0).unwrap().unwrap();
    assert_eq!(finish(&uninstall, "0.1.0", T1).unwrap().1.applied, None, "解除安裝後又重新安裝：沒有套用與否");
}

