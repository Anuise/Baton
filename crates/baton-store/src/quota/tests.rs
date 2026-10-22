use baton_domain::profile::ProfileCommand;
use baton_domain::quota::RateLimitSignal;

use super::*;

const T1: Timestamp = Timestamp::from_millis(10 * 24 * 60 * 60 * 1000);
const MINUTE: i64 = 60 * 1000;

async fn store_with_profile(dir: &Path, id: &str) -> Store {
    let store = Store::open(dir, T1).await.unwrap();
    let create = ProfileCommand::Create {
        id: id.into(),
        name: id.into(),
        config_dir: format!("C:\\baton\\profiles\\{id}"),
        expected_account: None,
    };
    store.apply_profile_command(create, T1).await.unwrap();
    store
}

fn limits(five_hour: Option<u64>, seven_day: Option<u64>) -> PlanLimits {
    PlanLimits { five_hour_tokens: five_hour, seven_day_tokens: seven_day }
}

#[tokio::test]
async fn a_profile_without_observations_is_unknown() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_with_profile(dir.path(), "p1").await;
    let status = store.quota_status(T1).await.unwrap();
    assert_eq!(status["p1"].assessment.used, None);
    assert_eq!(status["p1"].plan_limits, PlanLimits::default());
}

#[tokio::test]
async fn usage_and_plan_limits_produce_a_persisted_estimate_that_survives_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_with_profile(dir.path(), "p1").await;
    store.record_quota_usage("p1".into(), "run_1".into(), 3_000, "{\"usage\":{}}".into(), T1).await.unwrap();
    assert_eq!(store.quota_status(T1).await.unwrap()["p1"].assessment.used, None, "沒有方案上限就是未知");

    store.set_quota_plan_limits("p1".into(), limits(Some(10_000), None), T1).await.unwrap();
    let status = store.quota_status(T1).await.unwrap();
    assert_eq!(status["p1"].assessment.used, Some(0.3));
    assert_eq!(status["p1"].plan_limits, limits(Some(10_000), None));
    let five_hour = status["p1"].assessment.windows[0].estimated.clone().unwrap();
    assert_eq!(five_hour.raw_ref, "quota_usage:seq=1..1");
    assert_eq!(five_hour.observed_at, T1);
    drop(store);

    let reopened = Store::open(dir.path(), T1).await.unwrap();
    assert_eq!(reopened.quota_status(T1).await.unwrap()["p1"].assessment.used, Some(0.3));
}

#[tokio::test]
async fn plan_limits_are_validated_versioned_and_unchanged_limits_append_no_event() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_with_profile(dir.path(), "p1").await;
    assert!(matches!(
        store.set_quota_plan_limits("p1".into(), limits(Some(0), None), T1).await,
        Err(QuotaApplyError::Invalid(_))
    ));
    assert!(matches!(
        store.set_quota_plan_limits("nope".into(), limits(Some(1), None), T1).await,
        Err(QuotaApplyError::ProfileNotFound(_))
    ));
    store.set_quota_plan_limits("p1".into(), limits(Some(1), None), T1).await.unwrap();
    store.set_quota_plan_limits("p1".into(), limits(Some(1), None), T1).await.unwrap();
    store.set_quota_plan_limits("p1".into(), limits(Some(1), Some(2)), T1).await.unwrap();

    let conn = Connection::open(dir.path().join(DB_FILE_NAME)).unwrap();
    let versions: Vec<i64> = conn
        .prepare("SELECT aggregate_version FROM event WHERE event_type = 'quota_plan_limits_changed' ORDER BY seq")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(versions, [1, 2]);
}

#[tokio::test]
async fn archived_profiles_cannot_get_plan_limits() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_with_profile(dir.path(), "p1").await;
    store.apply_profile_command(ProfileCommand::Archive { id: "p1".into() }, T1).await.unwrap();
    assert!(matches!(
        store.set_quota_plan_limits("p1".into(), limits(Some(1), None), T1).await,
        Err(QuotaApplyError::ProfileArchived(_))
    ));
}

#[tokio::test]
async fn polls_are_due_every_five_minutes_per_active_profile() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_with_profile(dir.path(), "p1").await;
    assert_eq!(store.quota_polls_due(T1).await.unwrap(), ["p1"]);
    store.record_quota_poll(vec![("p1".into(), None)], T1).await.unwrap();
    assert!(store.quota_polls_due(T1.plus_millis(5 * MINUTE - 1)).await.unwrap().is_empty());
    assert_eq!(store.quota_polls_due(T1.plus_millis(5 * MINUTE)).await.unwrap(), ["p1"]);
}

#[tokio::test]
async fn the_latest_signal_and_external_sample_are_judged_together() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_with_profile(dir.path(), "p1").await;
    let signal = RateLimitSignal {
        window: Some(QuotaWindow::SevenDay),
        status: LimitStatus::Allowed,
        used: Some(0.4),
        reset_at: None,
        raw: "raw".into(),
    };
    store.record_quota_signal(quota::observe_signal("p1", "run_1", &signal, T1)).await.unwrap();
    let mut external = quota::observe_signal("p1", "x", &signal, T1);
    external.origin = ObservationOrigin::Observer;
    external.window = None;
    external.used = Some(0.6);
    external.raw_ref = "quota_observer".into();
    store.record_quota_poll(vec![("p1".into(), Some(external))], T1).await.unwrap();

    let assessment = store.quota_status(T1).await.unwrap().remove("p1").unwrap().assessment;
    assert_eq!(assessment.used, Some(0.6));
    assert_eq!(assessment.windows[1].observed.as_ref().unwrap().raw.as_deref(), Some("raw"));
}

#[tokio::test]
async fn polling_deletes_observations_and_usage_older_than_the_retention_except_the_latest_of_each_kind() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_with_profile(dir.path(), "p1").await;
    store.record_quota_usage("p1".into(), "run_1".into(), 10, "raw".into(), T1).await.unwrap();
    let signal = RateLimitSignal { window: None, status: LimitStatus::Allowed, used: None, reset_at: None, raw: "r".into() };
    store.record_quota_signal(quota::observe_signal("p1", "run_1", &signal, T1)).await.unwrap();
    let later = T1.plus_millis(QUOTA_RETENTION_MILLIS + 1);
    store.record_quota_poll(vec![("p1".into(), None)], later).await.unwrap();

    let conn = Connection::open(dir.path().join(DB_FILE_NAME)).unwrap();
    let count = |sql: &str| conn.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap();
    assert_eq!(count("SELECT COUNT(*) FROM quota_usage"), 0);
    assert_eq!(
        count(&format!("SELECT COUNT(*) FROM quota_observation WHERE origin = 'estimate' AND observed_at < {}", later.as_millis())),
        0,
        "舊的推估已被這次輪詢取代"
    );
    assert_eq!(count("SELECT COUNT(*) FROM quota_observation WHERE origin = 'adapter'"), 1, "最新的訊號保留");
    assert_eq!(count("SELECT COUNT(*) FROM quota_observation"), 3);
}
