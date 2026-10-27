//! 額度觀測的單元測試（票 27）：純 Domain，輸入觀測與時間，斷言判斷結果。

use super::*;

const NOW: Timestamp = Timestamp::from_millis(100 * 24 * HOUR_MILLIS);
const MINUTE: i64 = 60 * 1000;

fn usage(seq: i64, tokens: u64, minutes_ago: i64) -> UsageRecord {
    UsageRecord { seq, tokens, reported_at: NOW.plus_millis(-minutes_ago * MINUTE) }
}

fn limits(five_hour: Option<u64>, seven_day: Option<u64>) -> PlanLimits {
    PlanLimits { five_hour_tokens: five_hour, seven_day_tokens: seven_day }
}

fn signal(window: Option<QuotaWindow>, status: LimitStatus, used: Option<f64>, reset_at: Option<Timestamp>) -> RateLimitSignal {
    RateLimitSignal { window, status, used, reset_at, raw: "{\"type\":\"rate_limit_event\"}".into() }
}

fn observed(window: Option<QuotaWindow>, status: LimitStatus, used: Option<f64>, reset_at: Option<Timestamp>) -> QuotaObservation {
    observe_signal("p1", "run_1", &signal(window, status, used, reset_at), NOW)
}

fn estimated(window: QuotaWindow, tokens: u64, limit: Option<u64>) -> QuotaObservation {
    let limits = match window {
        QuotaWindow::FiveHour => limits(limit, None),
        QuotaWindow::SevenDay => limits(None, limit),
    };
    estimate("p1", window, &[usage(1, tokens, 10)], &limits, NOW)
}

fn external(source: QuotaSource, used: Option<f64>) -> QuotaObservation {
    QuotaObservation {
        profile_id: "p1".into(),
        origin: ObservationOrigin::Observer,
        source,
        window: None,
        status: None,
        used,
        tokens: None,
        limit_tokens: None,
        reset_at: None,
        observed_at: NOW,
        confidence: if used.is_some() { Confidence::Observed } else { Confidence::Unknown },
        raw_ref: "quota_observer".into(),
        raw: None,
    }
}

// --- 推估 ---

#[test]
fn estimate_without_a_plan_limit_is_unknown_not_zero() {
    let estimate = estimate("p1", QuotaWindow::FiveHour, &[usage(1, 5_000, 10)], &PlanLimits::default(), NOW);
    assert_eq!(estimate.used, None);
    assert_eq!(estimate.tokens, Some(5_000), "token 數仍然記下");
    assert_eq!(estimate.confidence, Confidence::Unknown);
    assert_eq!(estimate.source, QuotaSource::Estimated);
}

#[test]
fn estimate_divides_usage_inside_the_rolling_window_by_the_plan_limit() {
    let records = [usage(1, 4_000, 6 * 60), usage(2, 3_000, 60), usage(3, 1_000, 10)];
    let limits = limits(Some(10_000), Some(100_000));

    let five_hour = estimate("p1", QuotaWindow::FiveHour, &records, &limits, NOW);
    assert_eq!(five_hour.tokens, Some(4_000), "六小時前的用量不在五小時視窗內");
    assert_eq!(five_hour.used, Some(0.4));
    assert_eq!(five_hour.limit_tokens, Some(10_000));
    assert_eq!(five_hour.confidence, Confidence::Estimated);
    assert_eq!(five_hour.raw_ref, "quota_usage:seq=2..3");
    assert_eq!(five_hour.reset_at, Some(NOW.plus_millis(-60 * MINUTE + 5 * HOUR_MILLIS)), "最早一筆滑出視窗的時間");

    let seven_day = estimate("p1", QuotaWindow::SevenDay, &records, &limits, NOW);
    assert_eq!(seven_day.tokens, Some(8_000));
    assert_eq!(seven_day.used, Some(0.08));
}

#[test]
fn estimate_with_a_limit_but_no_local_usage_is_unknown_and_has_no_reset_time() {
    let estimate = estimate("p1", QuotaWindow::FiveHour, &[], &limits(Some(10_000), None), NOW);
    assert_eq!(estimate.used, None, "沒有本機用量就沒有推估的依據，不是 0");
    assert_eq!(estimate.confidence, Confidence::Unknown);
    assert_eq!(estimate.tokens, Some(0));
    assert_eq!(estimate.limit_tokens, Some(10_000));
    assert_eq!(estimate.reset_at, None);
    assert_eq!(estimate.raw_ref, "quota_usage:none");
}

#[test]
fn plan_limits_must_be_positive() {
    assert_eq!(limits(Some(0), None).validate(), Err(PlanLimitError::NotPositive));
    assert_eq!(limits(None, Some(0)).validate(), Err(PlanLimitError::NotPositive));
    assert_eq!(limits(None, None).validate(), Ok(()));
    assert_eq!(limits(Some(1), Some(2)).validate(), Ok(()));
}

#[test]
fn changing_plan_limits_produces_a_versioned_event_and_unchanged_limits_write_nothing() {
    let event = change_plan_limits("p1", &PlanLimits::default(), 0, limits(Some(10), None), NOW).unwrap().unwrap();
    assert_eq!(event.event_type, "quota_plan_limits_changed");
    assert_eq!(event.aggregate_id, "p1");
    assert_eq!(event.aggregate_version, 1);
    assert_eq!(event.payload, serde_json::json!({ "five_hour_tokens": 10, "seven_day_tokens": null }));

    assert_eq!(change_plan_limits("p1", &limits(Some(10), None), 1, limits(Some(10), None), NOW), Ok(None));
    assert_eq!(
        change_plan_limits("p1", &PlanLimits::default(), 0, limits(Some(0), None), NOW),
        Err(PlanLimitError::NotPositive)
    );
}

// --- 已觀測 ---

#[test]
fn a_signal_keeps_source_time_confidence_and_a_reference_to_the_raw_data() {
    let reset = NOW.plus_millis(HOUR_MILLIS);
    let obs = observed(Some(QuotaWindow::SevenDay), LimitStatus::Allowed, Some(0.3), Some(reset));
    assert_eq!(obs.source, QuotaSource::Observed);
    assert_eq!(obs.origin, ObservationOrigin::Adapter);
    assert_eq!(obs.observed_at, NOW);
    assert_eq!(obs.confidence, Confidence::Observed);
    assert_eq!(obs.raw_ref, "run:run_1");
    assert_eq!(obs.raw.as_deref(), Some("{\"type\":\"rate_limit_event\"}"));
    assert_eq!(obs.reset_at, Some(reset));
}

#[test]
fn a_signal_without_a_number_is_unknown_and_exhausted_counts_as_full() {
    let allowed = observed(None, LimitStatus::Allowed, None, None);
    assert_eq!(allowed.confidence, Confidence::Unknown);
    assert_eq!(allowed.judged_used(), None);

    let exhausted = observed(None, LimitStatus::Exhausted, None, None);
    assert_eq!(exhausted.confidence, Confidence::Observed);
    assert_eq!(exhausted.judged_used(), Some(1.0));

    let warning = observed(None, LimitStatus::Warning, None, None);
    assert_eq!(warning.judged_used(), Some(QUOTA_STOP_DISPATCH_RATIO), "警告沒有數字時至少視為停止派工門檻");
    assert_eq!(observed(None, LimitStatus::Warning, Some(0.9), None).judged_used(), Some(0.9));
}

#[test]
fn raw_data_is_truncated() {
    let long = "x".repeat(QUOTA_RAW_MAX_CHARS + 50);
    let mut s = signal(None, LimitStatus::Allowed, None, None);
    s.raw = long;
    assert_eq!(observe_signal("p1", "r", &s, NOW).raw.unwrap().chars().count(), QUOTA_RAW_MAX_CHARS);
}

#[test]
fn a_signal_expires_at_its_reset_time_or_after_one_window_without_one() {
    let reset = NOW.plus_millis(HOUR_MILLIS);
    let with_reset = observed(Some(QuotaWindow::FiveHour), LimitStatus::Exhausted, None, Some(reset));
    assert!(with_reset.is_current(reset.plus_millis(-1)));
    assert!(!with_reset.is_current(reset));

    let without = observed(None, LimitStatus::Exhausted, None, None);
    assert!(without.is_current(NOW.plus_millis(5 * HOUR_MILLIS - 1)));
    assert!(!without.is_current(NOW.plus_millis(5 * HOUR_MILLIS)));
}

// --- 判斷 ---

#[test]
fn nothing_observed_is_unknown_and_incomplete() {
    let assessment = assess(&[], NOW);
    assert_eq!(assessment.used, None);
    assert_eq!(assessment.confidence, Confidence::Unknown);
    assert_eq!(assessment.source, None);
    assert!(!assessment.complete);
    assert_eq!(assessment.windows.len(), 2);
    assert!(assessment.windows.iter().all(|w| w.effective.is_none()));
}

#[test]
fn the_more_conservative_of_observed_and_estimated_wins_in_either_direction() {
    let observed_low = observed(Some(QuotaWindow::FiveHour), LimitStatus::Allowed, Some(0.3), None);
    let estimated_high = estimated(QuotaWindow::FiveHour, 8_000, Some(10_000));
    let a = assess(&[observed_low.clone(), estimated_high.clone()], NOW);
    assert_eq!(a.used, Some(0.8));
    assert_eq!(a.source, Some(QuotaSource::Estimated));
    assert_eq!(a.confidence, Confidence::Estimated);
    let window = &a.windows[0];
    assert_eq!(window.window, QuotaWindow::FiveHour);
    assert_eq!(window.observed.as_ref(), Some(&observed_low), "兩個來源都保留");
    assert_eq!(window.estimated.as_ref(), Some(&estimated_high));

    let observed_high = observed(Some(QuotaWindow::FiveHour), LimitStatus::Allowed, Some(0.9), None);
    let b = assess(&[observed_high, estimated_high], NOW);
    assert_eq!(b.used, Some(0.9));
    assert_eq!(b.source, Some(QuotaSource::Observed));
}

#[test]
fn the_tighter_window_decides_the_account() {
    let a = assess(
        &[estimated(QuotaWindow::FiveHour, 2_000, Some(10_000)), estimated(QuotaWindow::SevenDay, 70_000, Some(100_000))],
        NOW,
    );
    assert_eq!(a.used, Some(0.7));
    assert_eq!(a.windows[1].effective.as_ref().unwrap().used, Some(0.7));
}

#[test]
fn an_unknown_estimate_does_not_hide_a_known_observation_and_vice_versa() {
    let a = assess(&[estimated(QuotaWindow::FiveHour, 2_000, None)], NOW);
    assert_eq!(a.used, None, "沒有方案上限的推估不是 0");

    let b = assess(
        &[estimated(QuotaWindow::FiveHour, 2_000, None), observed(Some(QuotaWindow::FiveHour), LimitStatus::Allowed, Some(0.4), None)],
        NOW,
    );
    assert_eq!(b.used, Some(0.4));
}

#[test]
fn an_expired_signal_no_longer_counts() {
    let reset = NOW.plus_millis(HOUR_MILLIS);
    let exhausted = observed(Some(QuotaWindow::FiveHour), LimitStatus::Exhausted, None, Some(reset));
    assert_eq!(assess(std::slice::from_ref(&exhausted), NOW).used, Some(1.0));
    assert_eq!(assess(std::slice::from_ref(&exhausted), NOW).reset_at, Some(reset));
    let after = assess(&[exhausted], reset);
    assert_eq!(after.used, None, "重置後回到未知，不是 0");
    assert!(after.windows[0].observed.is_none());
}

#[test]
fn a_signal_without_a_window_counts_toward_the_five_hour_window() {
    let a = assess(&[observed(None, LimitStatus::Exhausted, None, None)], NOW);
    assert!(a.windows[0].observed.is_some());
    assert!(a.windows[1].observed.is_none());
}

#[test]
fn estimates_alone_are_marked_incomplete_because_other_devices_are_invisible() {
    let a = assess(
        &[estimated(QuotaWindow::FiveHour, 1_000, Some(10_000)), estimated(QuotaWindow::SevenDay, 1_000, Some(100_000))],
        NOW,
    );
    assert!(!a.complete);
    let reason = a.incomplete_reason.unwrap();
    assert!(reason.contains("其他裝置"), "{reason}");
    assert!(reason.contains("五小時、七日"), "{reason}");
}

#[test]
fn observed_numbers_for_both_windows_are_complete() {
    let a = assess(
        &[
            observed(Some(QuotaWindow::FiveHour), LimitStatus::Allowed, Some(0.2), None),
            observed(Some(QuotaWindow::SevenDay), LimitStatus::Exhausted, None, None),
        ],
        NOW,
    );
    assert!(a.complete);
    assert_eq!(a.incomplete_reason, None);

    let only_one = assess(&[observed(Some(QuotaWindow::FiveHour), LimitStatus::Allowed, Some(0.2), None)], NOW);
    assert!(!only_one.complete);
    assert!(only_one.incomplete_reason.unwrap().contains("七日"));
}

#[test]
fn a_warning_without_a_number_does_not_make_the_window_complete() {
    // 沒有數字的警告判斷時至少視為 75%（保守），但那不是觀測到的用量，視窗仍缺少已觀測用量（AI-124）
    let a = assess(
        &[
            observed(Some(QuotaWindow::FiveHour), LimitStatus::Warning, None, None),
            observed(Some(QuotaWindow::SevenDay), LimitStatus::Allowed, Some(0.2), None),
        ],
        NOW,
    );
    assert_eq!(a.used, Some(0.75), "判斷仍以停止派工門檻計");
    assert!(!a.complete);
    assert!(a.incomplete_reason.unwrap().contains("五小時"));
}

#[test]
fn an_external_observed_sample_counts_and_is_complete_while_a_failed_one_is_unknown() {
    let a = assess(&[external(QuotaSource::Observed, Some(0.6))], NOW);
    assert_eq!(a.used, Some(0.6));
    assert!(a.complete);

    let failed = assess(&[external(QuotaSource::Observed, None)], NOW);
    assert_eq!(failed.used, None);
    assert!(!failed.complete);
    assert!(failed.external.is_some(), "失敗也保留為一筆未知的觀測");

    let estimated_sample = assess(&[external(QuotaSource::Estimated, Some(0.6))], NOW);
    assert_eq!(estimated_sample.used, Some(0.6));
    assert!(!estimated_sample.complete, "推估的樣本看不到其他裝置");
}

#[test]
fn the_newest_signal_for_a_window_replaces_older_ones() {
    let mut older = observed(Some(QuotaWindow::FiveHour), LimitStatus::Exhausted, None, None);
    older.observed_at = NOW.plus_millis(-10 * MINUTE);
    let newer = observed(Some(QuotaWindow::FiveHour), LimitStatus::Allowed, Some(0.1), None);
    assert_eq!(assess(&[older, newer], NOW).used, Some(0.1));
}

#[test]
fn api_shape_uses_snake_case_and_hides_raw_data() {
    let json = serde_json::to_value(observed(Some(QuotaWindow::SevenDay), LimitStatus::Warning, None, None)).unwrap();
    assert_eq!(json["window"], "seven_day");
    assert_eq!(json["status"], "warning");
    assert_eq!(json["source"], "observed");
    assert_eq!(json["confidence"], "unknown");
    assert!(json["used"].is_null());
    assert!(json.get("raw").is_none());
}
