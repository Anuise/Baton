use super::*;
use crate::verification_profile::{Assertion, Operation, Step};

fn step(key: &str, external: bool) -> Step {
    Step {
        key: key.into(),
        title: format!("步驟 {key}"),
        operation: Operation::Command { program: "shop".into(), args: Vec::new(), timeout_ms: None },
        assertions: vec![Assertion::ExitCode { equals: 0 }],
        external,
    }
}

fn checkout() -> Journey {
    Journey {
        key: "checkout".into(),
        title: "結帳".into(),
        representative: true,
        persona: None,
        identity: None,
        steps: vec![step("cart", false), step("pay", true), step("receipt", false)],
    }
}

fn record(journey: &str, key: &str, verdict: Verdict) -> StepRecord {
    StepRecord {
        phase: Phase::Journey,
        journey: Some(journey.into()),
        key: key.into(),
        title: String::new(),
        operation: String::new(),
        verdict,
        reason_code: if verdict == Verdict::Passed { "assertions_passed".into() } else { "assertion_failed".into() },
        engine_code: None,
        assertions: Vec::new(),
        output: String::new(),
        attempts: 1,
    }
}

fn attempt(n: u32, status: AttemptStatus, journal: Vec<JournalEntry>) -> Attempt {
    Attempt {
        request_id: "vr_1".into(),
        attempt: n,
        status,
        judge_profile_id: Some(format!("prof_{n}")),
        end_reason: None,
        started_at: Timestamp::from_millis(n as i64 * 10),
        ended_at: None,
        journal,
    }
}

fn build(revision: &str) -> JournalEntry {
    JournalEntry::Build { candidate_revision: Some(revision.into()), build: Some(format!("revision:{revision}")) }
}

fn done(journey: &str, key: &str) -> JournalEntry {
    JournalEntry::StepCompleted { record: record(journey, key, Verdict::Passed) }
}

#[test]
fn leftovers_are_the_services_and_sessions_without_a_stop_record_in_any_attempt() {
    let first = attempt(
        1,
        AttemptStatus::Interrupted,
        vec![
            JournalEntry::ServiceStarted { handle: "svc-1".into() },
            JournalEntry::BrowserOpened { journey: "a".into(), session: "b1".into() },
            JournalEntry::BrowserClosed { session: "b1".into() },
            JournalEntry::BrowserOpened { journey: "b".into(), session: "b2".into() },
        ],
    );
    assert_eq!(leftovers(std::slice::from_ref(&first)), Leftovers { services: vec!["svc-1".into()], sessions: vec!["b2".into()] });
    // 承接者停止它們的紀錄寫在自己的日誌中
    let second = attempt(
        2,
        AttemptStatus::Running,
        vec![JournalEntry::ServiceStopped { handle: "svc-1".into() }, JournalEntry::BrowserClosed { session: "b2".into() }],
    );
    assert_eq!(leftovers(&[first, second]), Leftovers::default());
}

#[test]
fn only_accounts_that_were_rate_limited_are_excluded() {
    let mut limited = attempt(1, AttemptStatus::HandedOff, Vec::new());
    limited.end_reason = Some(REASON_JUDGE_RATE_LIMITED.into());
    let interrupted = Attempt { end_reason: Some(REASON_RUNTIME_INTERRUPTED.into()), ..attempt(2, AttemptStatus::Interrupted, Vec::new()) };
    assert_eq!(rate_limited_accounts(&[limited, interrupted]), vec!["prof_1".to_string()]);
}

#[test]
fn a_completed_journey_is_reused_only_when_the_build_is_the_same() {
    let first = attempt(
        1,
        AttemptStatus::HandedOff,
        vec![
            build("c1"),
            done("checkout", "cart"),
            done("checkout", "pay"),
            done("checkout", "receipt"),
            JournalEntry::JourneyCompleted { journey: "checkout".into() },
        ],
    );
    let previous = [first];
    match plan_journey(&checkout(), &previous, Some("c1"), Some("revision:c1")) {
        JourneyPlan::Reuse { attempt: from, steps, .. } => assert_eq!((from, steps.len()), (1, 3)),
        other => panic!("同一個 build 應該沿用：{other:?}"),
    }
    // build 改變：舊的結果不能沿用；旅程中沒有開始過外部操作，以新的 build 整條重測
    assert_eq!(plan_journey(&checkout(), &previous, Some("c2"), Some("revision:c2")), JourneyPlan::Run);
}

/// R151 第 23 項：build 與 revision 都觀察不到時，無法證明是同一個版本，不沿用（AI-427）。
#[test]
fn a_completed_journey_is_not_reused_when_neither_build_nor_revision_was_observed() {
    let unobserved = JournalEntry::Build { candidate_revision: None, build: None };
    let first = attempt(
        1,
        AttemptStatus::HandedOff,
        vec![
            unobserved,
            done("checkout", "cart"),
            done("checkout", "pay"),
            done("checkout", "receipt"),
            JournalEntry::JourneyCompleted { journey: "checkout".into() },
        ],
    );
    assert_eq!(plan_journey(&checkout(), &[first], None, None), JourneyPlan::Run);
}

/// review-notes #25：完整結束、但 build 不同而不能沿用的旅程，其中已送出的外部操作也不重送（AI-428）。
#[test]
fn a_completed_journey_that_cannot_be_reused_does_not_resend_its_external_operation() {
    let first = attempt(
        1,
        AttemptStatus::HandedOff,
        vec![
            build("c1"),
            done("checkout", "cart"),
            JournalEntry::ExternalStarted { journey: "checkout".into(), step: "pay".into() },
            done("checkout", "pay"),
            done("checkout", "receipt"),
            JournalEntry::JourneyCompleted { journey: "checkout".into() },
        ],
    );
    let plan = plan_journey(&checkout(), &[first], Some("c2"), Some("revision:c2"));
    let JourneyPlan::NotResent { attempt: from, step, reason, steps, .. } = plan else {
        panic!("已送出的外部操作不能因 build 改變而重送：{plan:?}");
    };
    assert_eq!((from, step.as_str(), reason, steps.len()), (1, "pay", REASON_EXTERNAL_ALREADY_SENT, 3));
}

#[test]
fn an_unfinished_journey_without_external_operations_is_rerun_from_the_start() {
    let first = attempt(1, AttemptStatus::HandedOff, vec![build("c1"), done("checkout", "cart")]);
    assert_eq!(plan_journey(&checkout(), &[first], Some("c1"), Some("revision:c1")), JourneyPlan::Run);
}

#[test]
fn an_external_operation_that_was_started_is_never_resent() {
    // 結果未知：送出前的標記有，完成紀錄沒有
    let unknown = attempt(
        1,
        AttemptStatus::Interrupted,
        vec![build("c1"), done("checkout", "cart"), JournalEntry::ExternalStarted { journey: "checkout".into(), step: "pay".into() }],
    );
    let JourneyPlan::NotResent { attempt: from, step, reason, steps, .. } = plan_journey(&checkout(), &[unknown.clone()], Some("c1"), Some("revision:c1"))
    else {
        panic!("結果未知的外部操作不能重送");
    };
    assert_eq!((from, step.as_str(), reason, steps.len()), (1, "pay", REASON_EXTERNAL_OUTCOME_UNKNOWN, 1));

    // 已送出、旅程沒有完成：重跑會重送
    let sent = attempt(
        1,
        AttemptStatus::HandedOff,
        vec![
            build("c1"),
            done("checkout", "cart"),
            JournalEntry::ExternalStarted { journey: "checkout".into(), step: "pay".into() },
            done("checkout", "pay"),
        ],
    );
    let plan = plan_journey(&checkout(), &[sent], Some("c2"), Some("revision:c2"));
    assert!(matches!(plan, JourneyPlan::NotResent { reason: REASON_EXTERNAL_ALREADY_SENT, .. }), "{plan:?}");

    // 之後的嘗試沒有重跑它，再之後的嘗試仍然不重送
    let skipped = attempt(2, AttemptStatus::Interrupted, vec![build("c1")]);
    let plan = plan_journey(&checkout(), &[unknown, skipped], Some("c1"), Some("revision:c1"));
    assert!(matches!(plan, JourneyPlan::NotResent { attempt: 1, reason: REASON_EXTERNAL_OUTCOME_UNKNOWN, .. }), "{plan:?}");
}

#[test]
fn a_journey_that_is_not_resent_is_never_a_pass() {
    let journey = checkout();
    let policy = EvidencePolicy::default();
    let partial = vec![record("checkout", "cart", Verdict::Passed)];
    let records = not_resent_records(&journey, &partial, "pay", REASON_EXTERNAL_OUTCOME_UNKNOWN, &policy);
    let codes: Vec<(&str, &str)> = records.iter().map(|r| (r.key.as_str(), r.reason_code.as_str())).collect();
    assert_eq!(codes, [("cart", "assertions_passed"), ("pay", "external_outcome_unknown"), ("receipt", "not_executed")]);
    let result = not_resent_result(&journey, &records, REASON_EXTERNAL_OUTCOME_UNKNOWN);
    assert_eq!((result.verdict, result.reason_code.as_str()), (Verdict::Inconclusive, "external_outcome_unknown"));

    // 全部步驟都有通過的紀錄但旅程沒有確認結束：仍不算通過
    let all = vec![
        record("checkout", "cart", Verdict::Passed),
        record("checkout", "pay", Verdict::Passed),
        record("checkout", "receipt", Verdict::Passed),
    ];
    let records = not_resent_records(&journey, &all, "pay", REASON_EXTERNAL_ALREADY_SENT, &policy);
    assert_eq!(records.len(), 3);
    assert_eq!(not_resent_result(&journey, &records, REASON_EXTERNAL_ALREADY_SENT).verdict, Verdict::Inconclusive);

    // 程式斷言已經證明的產品缺陷照舊
    let failed = vec![record("checkout", "cart", Verdict::ProductFailure)];
    let records = not_resent_records(&journey, &failed, "pay", REASON_EXTERNAL_ALREADY_SENT, &policy);
    assert_eq!(not_resent_result(&journey, &records, REASON_EXTERNAL_ALREADY_SENT).verdict, Verdict::ProductFailure);
}

#[test]
fn the_summary_lists_each_attempts_steps_and_the_journeys_whose_result_came_from_it() {
    let takeover = Takeover { from_attempt: 1, reused_journeys: vec!["browse".into()], ..Takeover::default() };
    let attempts = [
        Attempt {
            end_reason: Some(REASON_JUDGE_RATE_LIMITED.into()),
            ..attempt(1, AttemptStatus::HandedOff, vec![done("browse", "open"), done("checkout", "cart")])
        },
        attempt(2, AttemptStatus::Completed, vec![JournalEntry::TakenOver { takeover: takeover.clone() }, done("checkout", "cart")]),
    ];
    let summary = summarize(&attempts, &[("browse".into(), 1), ("checkout".into(), 2)]);
    assert_eq!(summary[0].journeys, ["browse"]);
    assert_eq!(summary[0].steps.len(), 2);
    assert_eq!(summary[0].end_reason.as_deref(), Some(REASON_JUDGE_RATE_LIMITED));
    assert_eq!((summary[1].journeys.as_slice(), summary[1].takeover.as_ref()), (&["checkout".to_string()][..], Some(&takeover)));
    let json = serde_json::to_value(&summary[0]).unwrap();
    assert_eq!(json["status"], "handed_off");
}

#[test]
fn journal_entries_round_trip_with_their_kind() {
    let entry = JournalEntry::ExternalStarted { journey: "checkout".into(), step: "pay".into() };
    let json = serde_json::to_value(&entry).unwrap();
    assert_eq!(json["kind"], entry.kind());
    assert_eq!(serde_json::from_value::<JournalEntry>(json).unwrap(), entry);
    for status in AttemptStatus::ALL {
        assert_eq!(AttemptStatus::parse(status.as_str()), Some(status));
    }
}
