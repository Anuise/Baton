//! 排程與容量的單元測試（票 19）。
//!
//! 全部是純 Domain 測試（不經 HTTP）：帳號 Profile 一律經由 `profile::decide` 建立與核對，
//! 進行中的 Run 只組出 `place` 需要的欄位。

use proptest::prelude::*;

use super::*;
use crate::authorization::Capability;
use crate::profile::{AccountProfile, AuthReport, AuthSource, ProfileCommand};
use crate::run::{Run, RunError, RunStatus};
use crate::task::{MissingItem, TaskStatus};

const T0: Timestamp = Timestamp::from_millis(1_000);
const T1: Timestamp = Timestamp::from_millis(2_000);

// --- 共用輔助 ---

fn config_dir(id: &str) -> String {
    format!("C:\\Baton\\profiles\\{id}")
}

fn created(id: &str, expected: Option<&str>) -> AccountProfile {
    profile::decide(
        &[],
        ProfileCommand::Create {
            id: id.into(),
            name: format!("帳號 {id}"),
            config_dir: config_dir(id),
            expected_account: expected.map(Into::into),
        },
        T0,
    )
    .unwrap()
    .profile
}

fn verified(profile: AccountProfile, report: AuthReport) -> AccountProfile {
    let id = profile.id.clone();
    profile::decide(std::slice::from_ref(&profile), ProfileCommand::RecordVerification { id, report }, T1)
        .unwrap()
        .profile
}

fn native(id: &str, account: Option<&str>) -> AuthReport {
    AuthReport {
        logged_in: true,
        source: AuthSource::NativeLogin,
        source_detail: None,
        account: account.map(Into::into),
        config_dir: Some(config_dir(id)),
    }
}

/// 核對無誤、可派工的帳號（每個帳號用自己的電子郵件，不會被當成重複身分）。
fn usable(id: &str) -> AccountProfile {
    verified(created(id, None), native(id, Some(&format!("{id}@x.com"))))
}

/// 建立後還沒核對的帳號。
fn unverified(id: &str) -> AccountProfile {
    created(id, None)
}

fn logged_out(id: &str) -> AccountProfile {
    verified(
        created(id, None),
        AuthReport { logged_in: false, source: AuthSource::None, source_detail: None, account: None, config_dir: None },
    )
}

fn overridden(id: &str) -> AccountProfile {
    verified(
        created(id, None),
        AuthReport {
            logged_in: true,
            source: AuthSource::ApiKey,
            source_detail: Some("ANTHROPIC_API_KEY".into()),
            account: None,
            config_dir: Some(config_dir(id)),
        },
    )
}

fn identity_unconfirmed(id: &str) -> AccountProfile {
    verified(created(id, None), native(id, None))
}

fn archived(profile: AccountProfile) -> AccountProfile {
    let id = profile.id.clone();
    profile::decide(std::slice::from_ref(&profile), ProfileCommand::Archive { id }, T1).unwrap().profile
}

/// 進行中的 Run（只有排程判定會讀到的欄位有意義）。
fn run(id: &str, profile_id: &str, status: RunStatus) -> Run {
    Run {
        id: id.into(),
        workspace_id: "ws".into(),
        task_id: format!("task_{id}"),
        profile_id: profile_id.into(),
        status,
        version: 1,
        generation: 1,
        contract_version: 1,
        authorization_revision: None,
        command_id: None,
        process: None,
        cancel_requested_at: None,
        end_reason: None,
        created_at: T0,
        updated_at: T0,
        started_at: None,
        ended_at: None,
    }
}

/// 每個帳號各有指定數量的進行中 Run。
fn runs_per_account(counts: &[(&str, usize)]) -> Vec<Run> {
    let mut runs = Vec::new();
    for (profile_id, count) in counts {
        for n in 0..*count {
            runs.push(run(&format!("{profile_id}_{n}"), profile_id, RunStatus::Running));
        }
    }
    runs
}

/// 一次排程判定需要的全部事實。
struct World {
    profiles: Vec<AccountProfile>,
    settings: SchedulerSettings,
    runs: Vec<Run>,
    quota: HashMap<String, f64>,
    halt: Option<WaitReason>,
}

impl World {
    fn new(profiles: Vec<AccountProfile>, mode: CapacityMode) -> Self {
        World {
            profiles,
            settings: SchedulerSettings { mode, version: 0, updated_at: None },
            runs: Vec::new(),
            quota: HashMap::new(),
            halt: None,
        }
    }

    fn with_runs(mut self, counts: &[(&str, usize)]) -> Self {
        self.runs = runs_per_account(counts);
        self
    }

    fn with_quota(mut self, profile_id: &str, used: f64) -> Self {
        self.quota.insert(profile_id.into(), used);
        self
    }

    fn place(&self, kind: SlotKind) -> Placement {
        place(
            &Capacity {
                profiles: &self.profiles,
                settings: &self.settings,
                active_runs: &self.runs,
                quota_used: &self.quota,
                halt: self.halt.clone(),
            },
            kind,
        )
    }
}

fn placed(placement: Placement) -> String {
    match placement {
        Placement::Place { profile_id } => profile_id,
        Placement::Wait(reasons) => panic!("預期可以派出，卻在等待：{reasons:?}"),
    }
}

fn waiting(placement: Placement) -> Vec<WaitReason> {
    match placement {
        Placement::Wait(reasons) => reasons,
        Placement::Place { profile_id } => panic!("預期要等待，卻派給了 {profile_id}"),
    }
}

/// 把等待原因整理成 (類別, 代碼, 帳號) 方便整批斷言。
fn summary(reasons: &[WaitReason]) -> Vec<(WaitCategory, &str, Option<&str>)> {
    reasons.iter().map(|r| (r.category, r.code.as_str(), r.profile_id.as_deref())).collect()
}

fn codes(reasons: &[WaitReason]) -> Vec<&str> {
    reasons.iter().map(|r| r.code.as_str()).collect()
}

// ============================================================ 生命週期（票 25）

#[test]
fn stop_intent_halts_every_placement_including_handoff_and_quota_recovery() {
    let halt = WaitReason {
        category: WaitCategory::Lifecycle,
        code: "runtime_stopping".into(),
        message: "停止中".into(),
        profile_id: None,
    };
    // 帳號都有空、額度也已恢復（低於門檻）：沒有停止意圖時派得出去
    let mut world = World::new(vec![usable("p1"), usable("p2")], CapacityMode::ReserveHandoff).with_quota("p1", 0.1);
    assert_eq!(placed(world.place(SlotKind::Normal)), "p1");
    world.halt = Some(halt.clone());
    for kind in [SlotKind::Normal, SlotKind::Handoff] {
        assert_eq!(waiting(world.place(kind)), vec![halt.clone()], "{kind:?} 也不能在停止意圖生效時派出");
    }
}

// ============================================================ 容量預設

#[test]
fn capacity_defaults_to_reserving_a_handoff_slot() {
    assert_eq!(CapacityMode::default(), CapacityMode::ReserveHandoff);
    assert_eq!(SchedulerSettings::default().mode, CapacityMode::ReserveHandoff);
    assert_eq!(SchedulerSettings::default().version, 0, "從未改過時版本是 0");
}

#[test]
fn the_two_capacity_presets_have_the_documented_numbers() {
    assert_eq!(
        CapacityMode::ReserveHandoff.limits(),
        CapacityLimits { per_account: 2, global: 6, handoff_reserve: 1 }
    );
    assert_eq!(
        CapacityMode::HigherConcurrency.limits(),
        CapacityLimits { per_account: 3, global: 8, handoff_reserve: 0 }
    );
}

#[test]
fn capacity_modes_round_trip_through_their_stable_names() {
    for mode in CapacityMode::ALL {
        assert_eq!(CapacityMode::parse(mode.as_str()), Some(mode));
    }
    assert_eq!(CapacityMode::ReserveHandoff.as_str(), "reserve_handoff");
    assert_eq!(CapacityMode::HigherConcurrency.as_str(), "higher_concurrency");
    assert_eq!(CapacityMode::parse("unlimited"), None);
    assert_eq!(serde_json::to_value(CapacityMode::HigherConcurrency).unwrap(), serde_json::json!("higher_concurrency"));
}

#[test]
fn the_handoff_slot_is_reserved_only_with_three_or_more_usable_accounts() {
    let limits = CapacityMode::ReserveHandoff.limits();

    for accounts in [0, 1, 2] {
        assert_eq!(limits.effective_reserve(accounts), 0, "{accounts} 個帳號不保留");
        assert_eq!(limits.normal_limit(accounts), 6);
    }
    for accounts in [3, 4, 8] {
        assert_eq!(limits.effective_reserve(accounts), 1, "{accounts} 個帳號保留 1 個");
        assert_eq!(limits.normal_limit(accounts), 5);
    }
}

#[test]
fn higher_concurrency_never_reserves_a_handoff_slot() {
    let limits = CapacityMode::HigherConcurrency.limits();

    for accounts in [0, 2, 3, 8] {
        assert_eq!(limits.effective_reserve(accounts), 0);
        assert_eq!(limits.normal_limit(accounts), 8);
    }
}

#[test]
fn every_capacity_mode_describes_its_numbers() {
    let reserve = CapacityMode::ReserveHandoff.description();
    let higher = CapacityMode::HigherConcurrency.description();

    assert!(reserve.contains('2') && reserve.contains('6') && reserve.contains("交接槽"), "{reserve}");
    assert!(higher.contains('3') && higher.contains('8') && higher.contains("不保留"), "{higher}");
}

// ============================================================ 進行中的 Run

#[test]
fn queued_starting_and_running_runs_are_active_and_finished_ones_are_not() {
    for status in RunStatus::ALL {
        let active = is_active(&run("r", "p1", status));
        assert_eq!(active, !status.is_terminal(), "{status:?}");
    }
    for status in [RunStatus::Queued, RunStatus::Starting, RunStatus::Running] {
        assert!(is_active(&run("r", "p1", status)));
    }
}

#[test]
fn a_run_that_was_asked_to_cancel_still_occupies_its_slot_until_it_stops() {
    let mut cancelling = run("r", "p1", RunStatus::Running);
    cancelling.cancel_requested_at = Some(T1);

    assert!(is_active(&cancelling), "程序還沒確認停止，名額不能釋出");
}

// ============================================================ 選帳號

#[test]
fn the_first_task_goes_to_the_earliest_account_and_the_second_to_the_other_account() {
    let mut world = World::new(vec![usable("p1"), usable("p2")], CapacityMode::ReserveHandoff);

    assert_eq!(placed(world.place(SlotKind::Normal)), "p1", "同樣空閒時取較早建立的帳號");
    world.runs.push(run("r1", "p1", RunStatus::Queued));
    assert_eq!(placed(world.place(SlotKind::Normal)), "p2", "第一個帳號已有 Run，換第二個帳號");
}

#[test]
fn the_account_with_the_fewest_active_runs_is_chosen() {
    let world = World::new(vec![usable("p1"), usable("p2"), usable("p3")], CapacityMode::HigherConcurrency)
        .with_runs(&[("p1", 2), ("p2", 1), ("p3", 2)]);

    assert_eq!(placed(world.place(SlotKind::Normal)), "p2");
}

#[test]
fn ties_go_to_the_earlier_created_account_even_when_it_is_not_the_first_in_the_busy_list() {
    let world = World::new(vec![usable("p1"), usable("p2"), usable("p3")], CapacityMode::HigherConcurrency)
        .with_runs(&[("p1", 2), ("p2", 1), ("p3", 1)]);

    assert_eq!(placed(world.place(SlotKind::Normal)), "p2", "p2 與 p3 同為最少，取較早建立的 p2");
}

#[test]
fn archived_and_blocked_accounts_are_never_chosen() {
    let world = World::new(
        vec![archived(usable("p1")), unverified("p2"), logged_out("p3"), overridden("p4"), usable("p5")],
        CapacityMode::ReserveHandoff,
    );

    assert_eq!(placed(world.place(SlotKind::Normal)), "p5");
}

#[test]
fn only_dispatchable_accounts_are_listed_as_usable() {
    let profiles = vec![usable("p1"), unverified("p2"), archived(usable("p3")), usable("p4")];

    let ids: Vec<&str> = usable_profiles(&profiles).iter().map(|p| p.id.as_str()).collect();

    assert_eq!(ids, ["p1", "p4"]);
}

// ============================================================ 每帳號上限

#[test]
fn an_account_at_its_per_account_limit_is_skipped_for_another_account() {
    let world =
        World::new(vec![usable("p1"), usable("p2")], CapacityMode::ReserveHandoff).with_runs(&[("p1", 2), ("p2", 1)]);

    assert_eq!(placed(world.place(SlotKind::Normal)), "p2");
}

#[test]
fn when_every_account_is_at_its_limit_the_task_waits_with_one_capacity_reason_per_account() {
    let world =
        World::new(vec![usable("p1"), usable("p2")], CapacityMode::ReserveHandoff).with_runs(&[("p1", 2), ("p2", 2)]);

    let reasons = waiting(world.place(SlotKind::Normal));

    assert_eq!(
        summary(&reasons),
        [
            (WaitCategory::Capacity, "account_capacity_full", Some("p1")),
            (WaitCategory::Capacity, "account_capacity_full", Some("p2")),
        ]
    );
    assert!(reasons[0].message.contains("2/2"), "{}", reasons[0].message);
}

#[test]
fn the_per_account_limit_follows_the_capacity_mode() {
    let reserve = World::new(vec![usable("p1")], CapacityMode::ReserveHandoff).with_runs(&[("p1", 2)]);
    let higher = World::new(vec![usable("p1")], CapacityMode::HigherConcurrency).with_runs(&[("p1", 2)]);
    let higher_full = World::new(vec![usable("p1")], CapacityMode::HigherConcurrency).with_runs(&[("p1", 3)]);

    assert_eq!(codes(&waiting(reserve.place(SlotKind::Normal))), ["account_capacity_full"], "預設每帳號 2 個");
    assert_eq!(placed(higher.place(SlotKind::Normal)), "p1", "較高並行每帳號 3 個");
    assert_eq!(codes(&waiting(higher_full.place(SlotKind::Normal))), ["account_capacity_full"]);
}

// ============================================================ 全域上限

#[test]
fn the_global_limit_stops_dispatch_even_when_an_account_still_has_room() {
    let world = World::new(vec![usable("p1"), usable("p2"), usable("p3")], CapacityMode::HigherConcurrency)
        .with_runs(&[("p1", 3), ("p2", 3), ("p3", 2)]);

    let reasons = waiting(world.place(SlotKind::Normal));

    assert_eq!(reasons[0].category, WaitCategory::Capacity);
    assert_eq!(reasons[0].code, "global_capacity_full");
    assert_eq!(reasons[0].profile_id, None, "全域名額不屬於任何帳號");
    assert!(reasons[0].message.contains("8/8"), "{}", reasons[0].message);
}

#[test]
fn the_global_limit_is_six_in_the_default_mode_and_a_handoff_may_use_all_of_them() {
    let world = World::new(vec![usable("p1"), usable("p2"), usable("p3")], CapacityMode::ReserveHandoff)
        .with_runs(&[("p1", 2), ("p2", 2), ("p3", 2)]);

    let normal = waiting(world.place(SlotKind::Normal));
    let handoff = waiting(world.place(SlotKind::Handoff));

    assert_eq!(normal[0].code, "global_capacity_full", "已滿 6 個，不是「保留」造成的");
    assert_eq!(handoff[0].code, "global_capacity_full");
}

// ============================================================ 交接保留槽

#[test]
fn normal_dispatch_is_stopped_at_five_with_three_accounts_but_a_handoff_can_use_the_sixth() {
    let world = World::new(vec![usable("p1"), usable("p2"), usable("p3")], CapacityMode::ReserveHandoff)
        .with_runs(&[("p1", 2), ("p2", 2), ("p3", 1)]);

    let normal = waiting(world.place(SlotKind::Normal));

    assert_eq!(
        summary(&normal),
        [
            (WaitCategory::Capacity, "handoff_slot_reserved", None),
            (WaitCategory::Capacity, "account_capacity_full", Some("p1")),
            (WaitCategory::Capacity, "account_capacity_full", Some("p2")),
        ],
        "p3 還有名額，擋住一般派工的是保留槽；已滿的 p1、p2 如實列出"
    );
    assert!(normal[0].message.contains('5') && normal[0].message.contains('1'), "{}", normal[0].message);
    assert_eq!(placed(world.place(SlotKind::Handoff)), "p3", "交接可以用到第 6 個名額");
}

#[test]
fn when_no_account_is_full_the_only_reason_is_the_reserved_handoff_slot() {
    let world = World::new(
        vec![usable("p1"), usable("p2"), usable("p3"), usable("p4"), usable("p5")],
        CapacityMode::ReserveHandoff,
    )
    .with_runs(&[("p1", 1), ("p2", 1), ("p3", 1), ("p4", 1), ("p5", 1)]);

    let reasons = waiting(world.place(SlotKind::Normal));

    assert_eq!(summary(&reasons), [(WaitCategory::Capacity, "handoff_slot_reserved", None)]);
    assert_eq!(placed(world.place(SlotKind::Handoff)), "p1", "交接可以用保留的名額");
}

#[test]
fn normal_dispatch_may_still_take_the_fifth_slot_with_three_accounts() {
    let world = World::new(vec![usable("p1"), usable("p2"), usable("p3")], CapacityMode::ReserveHandoff)
        .with_runs(&[("p1", 2), ("p2", 1), ("p3", 1)]);

    assert_eq!(placed(world.place(SlotKind::Normal)), "p2", "前 5 個名額一般派工可用");
}

#[test]
fn no_slot_is_reserved_with_only_two_usable_accounts_but_the_per_account_limit_still_applies() {
    let three_runs =
        World::new(vec![usable("p1"), usable("p2")], CapacityMode::ReserveHandoff).with_runs(&[("p1", 2), ("p2", 1)]);
    let four_runs =
        World::new(vec![usable("p1"), usable("p2")], CapacityMode::ReserveHandoff).with_runs(&[("p1", 2), ("p2", 2)]);

    assert_eq!(placed(three_runs.place(SlotKind::Normal)), "p2");
    let reasons = waiting(four_runs.place(SlotKind::Normal));
    assert!(reasons.iter().all(|r| r.code == "account_capacity_full"), "{reasons:?}");
    assert!(!codes(&reasons).contains(&"handoff_slot_reserved"));
}

#[test]
fn a_blocked_third_account_does_not_count_toward_the_reserve_threshold() {
    // 三個 Profile 但只有兩個可用：不保留；p3 的殘留 Run 讓總數到 5
    let world = World::new(vec![usable("p1"), usable("p2"), unverified("p3")], CapacityMode::ReserveHandoff)
        .with_runs(&[("p1", 2), ("p2", 1), ("p3", 2)]);

    assert_eq!(placed(world.place(SlotKind::Normal)), "p2", "5 個進行中仍低於不保留時的 6 個上限");
}

#[test]
fn higher_concurrency_lets_normal_dispatch_use_all_eight_slots() {
    let world = World::new(vec![usable("p1"), usable("p2"), usable("p3")], CapacityMode::HigherConcurrency)
        .with_runs(&[("p1", 3), ("p2", 3), ("p3", 1)]);

    assert_eq!(placed(world.place(SlotKind::Normal)), "p3", "7 個進行中時一般派工仍有第 8 個名額");
}

// ============================================================ 等待原因：帳號與狀態不明

#[test]
fn without_any_profile_the_wait_reason_is_no_account() {
    let world = World::new(vec![], CapacityMode::ReserveHandoff);

    let reasons = waiting(world.place(SlotKind::Normal));

    assert_eq!(summary(&reasons), [(WaitCategory::Account, "no_account", None)]);
}

#[test]
fn archived_profiles_alone_also_mean_no_account() {
    let world = World::new(vec![archived(usable("p1"))], CapacityMode::ReserveHandoff);

    assert_eq!(codes(&waiting(world.place(SlotKind::Normal))), ["no_account"]);
}

#[test]
fn a_profile_that_was_never_verified_is_state_unknown_not_an_account_problem() {
    let world = World::new(vec![unverified("p1")], CapacityMode::ReserveHandoff);

    let reasons = waiting(world.place(SlotKind::Normal));

    assert_eq!(summary(&reasons), [(WaitCategory::StateUnknown, "profile_unverified", Some("p1"))]);
    assert!(reasons[0].message.contains("帳號 p1"), "訊息指出是哪個帳號：{}", reasons[0].message);
}

#[test]
fn a_logged_in_profile_without_an_identity_is_state_unknown() {
    let world = World::new(vec![identity_unconfirmed("p1")], CapacityMode::ReserveHandoff);

    assert_eq!(
        summary(&waiting(world.place(SlotKind::Normal))),
        [(WaitCategory::StateUnknown, "identity_unconfirmed", Some("p1"))]
    );
}

#[test]
fn not_logged_in_overridden_and_mismatched_profiles_are_account_problems() {
    let mismatched = verified(created("p4", Some("someone@x.com")), native("p4", Some("other@x.com")));
    let first = usable("p5");
    let duplicate = verified(created("p6", None), native("p6", Some("p5@x.com")));
    let cases = [
        (vec![logged_out("p1")], "profile_not_logged_in", "p1"),
        (vec![overridden("p2")], "auth_source_overridden", "p2"),
        (vec![mismatched], "identity_mismatch", "p4"),
        (vec![first, duplicate], "duplicate_identity", "p6"),
    ];

    for (profiles, code, profile_id) in cases {
        // 第一個可用帳號的名額已滿，才會輪到等待原因
        let world = World::new(profiles, CapacityMode::ReserveHandoff).with_runs(&[("p5", 2)]);
        let reasons = waiting(world.place(SlotKind::Normal));
        let blocked: Vec<_> = reasons.iter().filter(|r| r.profile_id.as_deref() == Some(profile_id)).collect();
        assert_eq!(blocked.len(), 1, "{code}：{reasons:?}");
        assert_eq!((blocked[0].category, blocked[0].code.as_str()), (WaitCategory::Account, code));
    }
}

#[test]
fn state_unknown_and_account_problems_from_different_profiles_stay_separate() {
    let world = World::new(vec![unverified("p1"), logged_out("p2")], CapacityMode::ReserveHandoff);

    let reasons = waiting(world.place(SlotKind::Normal));

    assert_eq!(
        summary(&reasons),
        [
            (WaitCategory::StateUnknown, "profile_unverified", Some("p1")),
            (WaitCategory::Account, "profile_not_logged_in", Some("p2")),
        ]
    );
}

#[test]
fn a_blocked_profile_does_not_stop_a_usable_one_from_taking_the_task() {
    let world = World::new(vec![unverified("p1"), usable("p2")], CapacityMode::ReserveHandoff);

    assert_eq!(placed(world.place(SlotKind::Normal)), "p2");
}

#[test]
fn blocked_profiles_and_a_full_global_limit_are_reported_in_their_own_categories() {
    let world = World::new(vec![usable("p1"), usable("p2"), usable("p3"), unverified("p4")], CapacityMode::ReserveHandoff)
        .with_runs(&[("p1", 2), ("p2", 2), ("p3", 1)]);

    let reasons = waiting(world.place(SlotKind::Normal));

    assert_eq!(
        summary(&reasons),
        [
            (WaitCategory::Capacity, "handoff_slot_reserved", None),
            (WaitCategory::StateUnknown, "profile_unverified", Some("p4")),
            (WaitCategory::Capacity, "account_capacity_full", Some("p1")),
            (WaitCategory::Capacity, "account_capacity_full", Some("p2")),
        ]
    );
}

// ============================================================ 等待原因：額度

#[test]
fn quota_at_or_above_the_seventy_five_percent_threshold_stops_dispatch_to_that_account() {
    assert_eq!(QUOTA_STOP_DISPATCH_RATIO, 0.75);

    for used in [0.75, 0.8, 1.0] {
        let world = World::new(vec![usable("p1")], CapacityMode::ReserveHandoff).with_quota("p1", used);
        let reasons = waiting(world.place(SlotKind::Normal));
        assert_eq!(summary(&reasons), [(WaitCategory::Quota, "quota_near_limit", Some("p1"))], "用量 {used}");
    }
}

#[test]
fn quota_below_the_threshold_does_not_block() {
    for used in [0.0, 0.5, 0.7499] {
        let world = World::new(vec![usable("p1")], CapacityMode::ReserveHandoff).with_quota("p1", used);
        assert_eq!(placed(world.place(SlotKind::Normal)), "p1", "用量 {used}");
    }
}

#[test]
fn an_unobserved_account_is_not_blocked_and_is_not_treated_as_zero_usage() {
    let world = World::new(vec![usable("p1")], CapacityMode::ReserveHandoff);

    assert!(world.quota.is_empty());
    assert_eq!(placed(world.place(SlotKind::Normal)), "p1", "沒有樣本（未觀測）不阻擋");
}

#[test]
fn an_account_near_its_quota_is_skipped_for_one_with_room() {
    let world = World::new(vec![usable("p1"), usable("p2")], CapacityMode::ReserveHandoff).with_quota("p1", 0.9);

    assert_eq!(placed(world.place(SlotKind::Normal)), "p2");
}

#[test]
fn the_quota_message_names_the_account_and_the_threshold() {
    let world = World::new(vec![usable("p1")], CapacityMode::ReserveHandoff).with_quota("p1", 0.8);

    let reasons = waiting(world.place(SlotKind::Normal));

    assert!(reasons[0].message.contains("帳號 p1"), "{}", reasons[0].message);
    assert!(reasons[0].message.contains("80%") && reasons[0].message.contains("75%"), "{}", reasons[0].message);
}

#[test]
fn quota_and_capacity_reasons_for_different_accounts_are_both_listed_under_their_own_category() {
    let world = World::new(vec![usable("p1"), usable("p2")], CapacityMode::ReserveHandoff)
        .with_runs(&[("p1", 2)])
        .with_quota("p2", 0.8);

    let reasons = waiting(world.place(SlotKind::Normal));

    assert_eq!(
        summary(&reasons),
        [
            (WaitCategory::Capacity, "account_capacity_full", Some("p1")),
            (WaitCategory::Quota, "quota_near_limit", Some("p2")),
        ],
        "每個帳號只有一個原因，額度與容量不混在一起"
    );
}

#[test]
fn an_account_near_its_quota_is_not_also_reported_as_full() {
    let world = World::new(vec![usable("p1")], CapacityMode::ReserveHandoff)
        .with_runs(&[("p1", 2)])
        .with_quota("p1", 0.9);

    assert_eq!(codes(&waiting(world.place(SlotKind::Normal))), ["quota_near_limit"]);
}

// ============================================================ 切換容量預設

#[test]
fn changing_to_the_current_mode_changes_nothing() {
    let current = SchedulerSettings::default();

    assert_eq!(change_mode(&current, CapacityMode::ReserveHandoff, T1), None);
    let higher = SchedulerSettings { mode: CapacityMode::HigherConcurrency, version: 3, updated_at: Some(T0) };
    assert_eq!(change_mode(&higher, CapacityMode::HigherConcurrency, T1), None, "不論版本為何，相同模式都冪等");
}

#[test]
fn changing_the_mode_bumps_the_version_and_appends_one_event() {
    let (next, event) = change_mode(&SchedulerSettings::default(), CapacityMode::HigherConcurrency, T1).unwrap();

    assert_eq!(next, SchedulerSettings { mode: CapacityMode::HigherConcurrency, version: 1, updated_at: Some(T1) });
    assert_eq!(event.aggregate_type, SETTINGS_AGGREGATE);
    assert_eq!(event.aggregate_id, SETTINGS_ID);
    assert_eq!(event.aggregate_version, 1, "事件版本與設定版本對應");
    assert_eq!(event.event_type, EVENT_SETTINGS_CHANGED);
    assert_eq!(event.occurred_at, T1);
    assert_eq!(event.payload, serde_json::json!({ "from": "reserve_handoff", "to": "higher_concurrency" }));
}

#[test]
fn switching_back_and_forth_keeps_versions_consecutive() {
    let (first, _) = change_mode(&SchedulerSettings::default(), CapacityMode::HigherConcurrency, T0).unwrap();
    let (second, event) = change_mode(&first, CapacityMode::ReserveHandoff, T1).unwrap();

    assert_eq!((second.mode, second.version, event.aggregate_version), (CapacityMode::ReserveHandoff, 2, 2));
    assert_eq!(event.payload, serde_json::json!({ "from": "higher_concurrency", "to": "reserve_handoff" }));
}

// ============================================================ Task 層級的等待原因

fn missing(code: MissingCode) -> MissingItem {
    MissingItem { code, field: None, message: format!("{code:?}") }
}

#[test]
fn unfinished_dependencies_wait_under_the_dependency_category() {
    let reasons = task_wait_reasons(&RunError::DependenciesUnfinished(vec!["先做這個".into()])).unwrap();

    assert_eq!(summary(&reasons), [(WaitCategory::Dependency, "dependencies_unfinished", None)]);
    assert!(reasons[0].message.contains("先做這個"), "{}", reasons[0].message);
}

#[test]
fn an_unconfirmed_or_insufficient_authorization_waits_under_the_authorization_category() {
    let unconfirmed =
        task_wait_reasons(&RunError::AdmissionRejected(AdmissionDenied::AuthorizationUnconfirmed("專案".into()))).unwrap();
    let not_granted = task_wait_reasons(&RunError::AdmissionRejected(AdmissionDenied::CapabilityNotGranted {
        workspace: "專案".into(),
        revision: 2,
        missing: vec![Capability::RunBuildTest],
    }))
    .unwrap();

    assert_eq!(summary(&unconfirmed), [(WaitCategory::Authorization, "authorization_unconfirmed", None)]);
    assert_eq!(summary(&not_granted), [(WaitCategory::Authorization, "capability_not_granted", None)]);
}

#[test]
fn authorization_gaps_found_by_the_admission_report_wait_with_one_reason_each() {
    let reasons = task_wait_reasons(&RunError::AdmissionDenied(vec![
        missing(MissingCode::AuthorizationUnconfirmed),
        missing(MissingCode::CapabilityNotGranted),
    ]))
    .unwrap();

    assert_eq!(
        summary(&reasons),
        [
            (WaitCategory::Authorization, "authorization_unconfirmed", None),
            (WaitCategory::Authorization, "capability_not_granted", None),
        ]
    );
}

#[test]
fn a_task_that_cannot_be_dispatched_for_other_reasons_is_rejected_instead_of_waiting() {
    let not_waitable = [
        RunError::TaskNotReady(TaskStatus::Running),
        RunError::TaskNotFound("t".into()),
        RunError::WorkspaceNotFound("ws".into()),
        RunError::ExecutionConflict { task_id: "t".into(), run_id: "r".into() },
        RunError::AdmissionRejected(AdmissionDenied::WorkspaceArchived("專案".into())),
        // 契約本身有缺項不是等一等就好，必須回去補契約
        RunError::AdmissionDenied(vec![missing(MissingCode::FieldMissing)]),
        RunError::AdmissionDenied(vec![
            missing(MissingCode::CapabilityNotGranted),
            missing(MissingCode::ReferenceInvalid),
        ]),
        RunError::AdmissionDenied(vec![]),
    ];

    for error in not_waitable {
        assert_eq!(task_wait_reasons(&error), None, "{error}");
    }
}

// ============================================================ 性質

#[derive(Debug, Clone, Copy)]
enum ProfileState {
    Usable,
    Unverified,
    LoggedOut,
    Overridden,
}

fn profile_state() -> impl Strategy<Value = ProfileState> {
    prop_oneof![
        4 => Just(ProfileState::Usable),
        1 => Just(ProfileState::Unverified),
        1 => Just(ProfileState::LoggedOut),
        1 => Just(ProfileState::Overridden),
    ]
}

fn run_status() -> impl Strategy<Value = RunStatus> {
    prop_oneof![Just(RunStatus::Queued), Just(RunStatus::Starting), Just(RunStatus::Running)]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// 任意進行中 Run 配置下：派出的帳號必定可用、額度未達門檻、每帳號進行中 Run 小於上限，
    /// 且是候選中最少者；一般派工後全域進行中不超過 `normal_limit`，交接不超過全域上限。
    #[test]
    fn a_placement_always_respects_account_usability_and_every_capacity_limit(
        states in proptest::collection::vec(profile_state(), 1..=5),
        run_counts in proptest::collection::vec((0usize..=4, run_status()), 5),
        quotas in proptest::collection::vec(proptest::option::of(0.0f64..=1.0), 5),
        higher in any::<bool>(),
        handoff in any::<bool>(),
    ) {
        let profiles: Vec<AccountProfile> = states
            .iter()
            .enumerate()
            .map(|(i, state)| {
                let id = format!("p{i}");
                match state {
                    ProfileState::Usable => usable(&id),
                    ProfileState::Unverified => unverified(&id),
                    ProfileState::LoggedOut => logged_out(&id),
                    ProfileState::Overridden => overridden(&id),
                }
            })
            .collect();
        let mut world = World::new(profiles, if higher { CapacityMode::HigherConcurrency } else { CapacityMode::ReserveHandoff });
        for (i, (count, status)) in run_counts.iter().take(states.len()).enumerate() {
            for n in 0..*count {
                world.runs.push(run(&format!("r{i}_{n}"), &format!("p{i}"), *status));
            }
        }
        for (i, used) in quotas.iter().take(states.len()).enumerate() {
            if let Some(used) = used {
                world.quota.insert(format!("p{i}"), *used);
            }
        }
        let kind = if handoff { SlotKind::Handoff } else { SlotKind::Normal };
        let limits = world.settings.mode.limits();
        let usable_ids: Vec<String> = usable_profiles(&world.profiles).iter().map(|p| p.id.clone()).collect();
        let running_of = |id: &str| world.runs.iter().filter(|r| r.profile_id == id).count();

        match world.place(kind) {
            Placement::Place { profile_id } => {
                prop_assert!(usable_ids.contains(&profile_id), "{profile_id} 不是可用帳號");
                prop_assert!(running_of(&profile_id) < limits.per_account, "{profile_id} 已達每帳號上限");
                prop_assert!(world.quota.get(&profile_id).is_none_or(|u| *u < QUOTA_STOP_DISPATCH_RATIO));
                let allowed = match kind {
                    SlotKind::Normal => limits.normal_limit(usable_ids.len()),
                    SlotKind::Handoff => limits.global,
                };
                prop_assert!(world.runs.len() + 1 <= allowed, "派出後 {} 個超過 {allowed}", world.runs.len() + 1);
                let least = usable_ids
                    .iter()
                    .filter(|id| running_of(id) < limits.per_account)
                    .filter(|id| world.quota.get(*id).is_none_or(|u| *u < QUOTA_STOP_DISPATCH_RATIO))
                    .map(|id| running_of(id))
                    .min()
                    .unwrap();
                prop_assert_eq!(running_of(&profile_id), least, "應選進行中 Run 最少的帳號");
            }
            Placement::Wait(reasons) => {
                prop_assert!(!reasons.is_empty(), "等待一定要有原因");
                prop_assert!(reasons.iter().all(|r| !r.code.is_empty() && !r.message.is_empty()));
            }
        }
    }
}
