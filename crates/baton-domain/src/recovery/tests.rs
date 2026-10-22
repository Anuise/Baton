//! 崩潰恢復狀態機的單元測試與性質測試（票 31）。純 Domain，不經 HTTP。

use proptest::prelude::*;

use super::*;

const T0: Timestamp = Timestamp::from_millis(1_000_000);

fn crashed_at(now: Timestamp) -> RecoveryEvent {
    on_start(None, true, "rec_1", now).expect("崩潰後建立事件").event
}

fn apply(event: &RecoveryEvent, command: RecoveryCommand, now: Timestamp) -> RecoveryEvent {
    decide(Some(event), command, now).expect("命令應被接受").expect("應有變動").event
}

fn begin(event: &RecoveryEvent, id: &str, now: Timestamp) -> RecoveryEvent {
    apply(event, RecoveryCommand::Begin { attempt_id: id.into() }, now)
}

fn fail(event: &RecoveryEvent, now: Timestamp) -> RecoveryEvent {
    apply(event, RecoveryCommand::Fail { reason: "無法確認舊程序停止".into(), affected: vec![] }, now)
}

#[test]
fn clean_start_without_event_creates_nothing() {
    assert!(on_start(None, false, "rec_1", T0).is_none());
}

#[test]
fn crash_creates_event_with_immediate_first_attempt() {
    let change = on_start(None, true, "rec_1", T0).unwrap();
    let event = change.event;
    assert_eq!((event.round, event.attempts_used, event.status), (1, 0, RecoveryStatus::Waiting));
    assert_eq!(event.next_attempt_at, Some(T0));
    assert_eq!(change.events[0].event_type, EVENT_RUNTIME_CRASH_DETECTED);
    assert!(dispatch_halt(Some(&event)).is_some(), "第一次嘗試完成前不派工");
}

#[test]
fn attempt_is_counted_when_it_begins_and_crash_during_it_fails_with_thirty_second_backoff() {
    let event = begin(&crashed_at(T0), "a1", T0);
    assert_eq!((event.attempts_used, event.status), (1, RecoveryStatus::Attempting));
    assert_eq!(event.attempts[0].seq, 1);

    let later = T0.plus_millis(5_000);
    let after = on_start(Some(&event), true, "rec_new", later).unwrap().event;
    assert_eq!(after.id, "rec_1", "沿用原事件");
    assert_eq!((after.attempts_used, after.status), (1, RecoveryStatus::Waiting));
    assert_eq!(after.next_attempt_at, Some(later.plus_millis(30_000)));
    assert_eq!(after.attempts[0].outcome, AttemptOutcome::Failed);
}

#[test]
fn backoff_cannot_be_skipped_and_restart_during_backoff_changes_nothing() {
    let waiting = fail(&begin(&crashed_at(T0), "a1", T0), T0);
    let deadline = waiting.next_attempt_at.unwrap();
    assert_eq!(deadline, T0.plus_millis(30_000));

    let early = decide(Some(&waiting), RecoveryCommand::Begin { attempt_id: "a2".into() }, deadline.plus_millis(-1));
    assert_eq!(early, Err(RecoveryError::BackoffActive(deadline)));
    assert!(on_start(Some(&waiting), true, "rec_new", T0.plus_millis(10_000)).is_none(), "退避中重建不計次也不改期限");

    let second = begin(&waiting, "a2", deadline);
    assert_eq!(second.attempts_used, 2);
    let waiting_third = fail(&second, deadline);
    assert_eq!(waiting_third.next_attempt_at, Some(deadline.plus_millis(120_000)), "第三次前等 2 分鐘");
}

#[test]
fn completed_attempt_observes_ten_minutes_before_success() {
    let observing = apply(&begin(&crashed_at(T0), "a1", T0), RecoveryCommand::Complete { affected: vec![] }, T0);
    assert_eq!(observing.status, RecoveryStatus::Observing);
    assert_eq!(observing.observe_until, Some(T0.plus_millis(600_000)));
    assert!(dispatch_halt(Some(&observing)).is_none(), "觀察期間可派工");

    let early = decide(Some(&observing), RecoveryCommand::Observe, T0.plus_millis(599_999)).unwrap();
    assert!(early.is_none());
    let done = apply(&observing, RecoveryCommand::Observe, T0.plus_millis(600_000));
    assert_eq!(done.status, RecoveryStatus::Succeeded);
    assert_eq!(done.closed_at, Some(T0.plus_millis(600_000)));
    assert_eq!(done.attempts[0].outcome, AttemptOutcome::Succeeded);
}

#[test]
fn crash_during_observation_keeps_event_and_count() {
    let observing = apply(&begin(&crashed_at(T0), "a1", T0), RecoveryCommand::Complete { affected: vec![] }, T0);
    let crashed = on_start(Some(&observing), true, "rec_new", T0.plus_millis(60_000)).unwrap().event;
    assert_eq!(crashed.id, "rec_1");
    assert_eq!((crashed.attempts_used, crashed.status), (1, RecoveryStatus::Waiting));
    assert_eq!(crashed.attempts[0].outcome, AttemptOutcome::Failed);

    assert!(on_start(Some(&observing), false, "rec_new", T0.plus_millis(60_000)).is_none(), "正常重啟不打斷觀察");
}

#[test]
fn crash_after_success_starts_a_new_event() {
    let observing = apply(&begin(&crashed_at(T0), "a1", T0), RecoveryCommand::Complete { affected: vec![] }, T0);
    let done = apply(&observing, RecoveryCommand::Observe, T0.plus_millis(600_000));
    let next = on_start(Some(&done), true, "rec_2", T0.plus_millis(700_000)).unwrap().event;
    assert_eq!((next.id.as_str(), next.round, next.attempts_used), ("rec_2", 1, 0));
}

fn exhausted() -> RecoveryEvent {
    let mut event = crashed_at(T0);
    let mut now = T0;
    for seq in 1..=3 {
        now = event.next_attempt_at.unwrap();
        event = fail(&begin(&event, &format!("a{seq}"), now), now);
    }
    assert_eq!(now, T0.plus_millis(150_000));
    event
}

#[test]
fn third_failure_exhausts_and_nothing_automatic_produces_a_fourth() {
    let event = exhausted();
    assert_eq!((event.status, event.attempts_used), (RecoveryStatus::Exhausted, 3));
    assert_eq!(event.next_attempt_at, None);
    assert!(dispatch_halt(Some(&event)).is_some_and(|r| r.code == "crash_recovery_exhausted"));

    let much_later = T0.plus_millis(10 * 3_600_000);
    assert!(on_start(Some(&event), true, "rec_new", much_later).is_none(), "重建不清除用盡");
    assert!(on_start(Some(&event), false, "rec_new", much_later).is_none());
    let fourth = decide(Some(&event), RecoveryCommand::Begin { attempt_id: "a4".into() }, much_later);
    assert_eq!(fourth, Err(RecoveryError::NotWaiting("exhausted")));
}

#[test]
fn manual_retry_starts_new_round_and_keeps_history() {
    let event = exhausted();
    let retried = apply(&event, RecoveryCommand::Retry, T0.plus_millis(200_000));
    assert_eq!((retried.round, retried.attempts_used, retried.status), (2, 0, RecoveryStatus::Waiting));
    assert_eq!(retried.next_attempt_at, Some(T0.plus_millis(200_000)));
    assert_eq!(retried.attempts.len(), 3, "舊輪次的嘗試保留");

    let first_of_round_two = begin(&retried, "b1", T0.plus_millis(200_000));
    let attempt = first_of_round_two.attempts.last().unwrap();
    assert_eq!((attempt.round, attempt.seq), (2, 1));
}

#[test]
fn retry_is_only_allowed_when_exhausted() {
    let waiting = crashed_at(T0);
    assert_eq!(decide(Some(&waiting), RecoveryCommand::Retry, T0), Err(RecoveryError::NotExhausted("waiting")));
    assert_eq!(decide(None, RecoveryCommand::Retry, T0), Err(RecoveryError::NoEvent));
}

#[test]
fn start_while_attempting_fails_the_attempt_even_after_clean_end() {
    let attempting = begin(&crashed_at(T0), "a1", T0);
    let after = on_start(Some(&attempting), false, "rec_new", T0).unwrap().event;
    assert_eq!(after.status, RecoveryStatus::Waiting);
    assert_eq!(after.attempts[0].outcome, AttemptOutcome::Failed);
}

// ---------------------------------------------------------------- 性質測試

#[derive(Debug, Clone)]
enum Op {
    Advance(u32),
    Start { crashed: bool },
    Begin,
    Complete,
    Fail,
    Observe,
    Retry,
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        (0u32..700_000).prop_map(Op::Advance),
        any::<bool>().prop_map(|crashed| Op::Start { crashed }),
        Just(Op::Begin),
        Just(Op::Complete),
        Just(Op::Fail),
        Just(Op::Observe),
        Just(Op::Retry),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// 恢復嘗試數永不超過 3（每一輪），序號連續；退避期限內不會開始嘗試；只有人工重試會開新輪次。
    #[test]
    fn attempts_never_exceed_three_per_round(ops in proptest::collection::vec(op(), 1..120)) {
        let mut now = T0;
        let mut current: Option<RecoveryEvent> = None;
        let mut counter = 0;
        for op in ops {
            counter += 1;
            let before = current.clone();
            let change = match op {
                Op::Advance(ms) => { now = now.plus_millis(i64::from(ms)); continue; }
                Op::Start { crashed } => on_start(current.as_ref(), crashed, &format!("rec_{counter}"), now),
                Op::Begin => decide(current.as_ref(), RecoveryCommand::Begin { attempt_id: format!("a{counter}") }, now).ok().flatten(),
                Op::Complete => decide(current.as_ref(), RecoveryCommand::Complete { affected: vec![] }, now).ok().flatten(),
                Op::Fail => decide(current.as_ref(), RecoveryCommand::Fail { reason: "x".into(), affected: vec![] }, now).ok().flatten(),
                Op::Observe => decide(current.as_ref(), RecoveryCommand::Observe, now).ok().flatten(),
                Op::Retry => decide(current.as_ref(), RecoveryCommand::Retry, now).ok().flatten(),
            };
            let Some(change) = change else { continue };
            let event = change.event;
            if let Some(before) = before.as_ref().filter(|b| b.id == event.id) {
                // 新嘗試只能在退避期滿之後開始
                if event.attempts.len() > before.attempts.len() {
                    prop_assert!(before.next_attempt_at.is_none_or(|t| now >= t));
                }
                // 輪次只會因人工重試而增加
                if event.round != before.round {
                    prop_assert!(matches!(op, Op::Retry));
                    prop_assert_eq!(before.status, RecoveryStatus::Exhausted);
                }
            }
            prop_assert!(event.attempts_used <= MAX_ATTEMPTS);
            for round in 1..=event.round {
                let seqs: Vec<u32> = event.attempts.iter().filter(|a| a.round == round).map(|a| a.seq).collect();
                prop_assert!(seqs.len() <= MAX_ATTEMPTS as usize);
                prop_assert_eq!(seqs.clone(), (1..=seqs.len() as u32).collect::<Vec<_>>());
            }
            prop_assert!(event.attempts.iter().filter(|a| matches!(a.outcome, AttemptOutcome::InProgress | AttemptOutcome::Observing)).count() <= 1);
            current = Some(event);
        }
    }
}
