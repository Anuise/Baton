use super::*;
use crate::verification_profile::{
    Assertion, EvidenceBundle, EvidencePolicy, Feature, Interface, Journey, JourneyResult, Operation, ProfileDefinition,
    Step, VerificationBudget,
};

fn now() -> Timestamp {
    Timestamp::from_millis(1_000)
}

fn http_step(key: &str, assertions: Vec<Assertion>) -> Step {
    Step {
        key: key.into(),
        title: String::new(),
        operation: Operation::Http {
            method: "GET".into(),
            url: "http://127.0.0.1:8080/items".into(),
            headers: Vec::new(),
            body: None,
            timeout_ms: None,
        },
        assertions,
        external: false,
    }
}

fn journey(key: &str, representative: bool) -> Journey {
    Journey {
        key: key.into(),
        title: format!("旅程 {key}"),
        representative,
        persona: None,
        identity: None,
        steps: vec![http_step("list", vec![Assertion::Status { equals: 200 }])],
    }
}

fn profile() -> ProfileDefinition {
    ProfileDefinition {
        interface: Interface::CliApi,
        build: None,
        doctor: Vec::new(),
        start: None,
        readiness: None,
        setup: Vec::new(),
        journeys: vec![journey("browse", true), journey("export", false)],
        teardown: Vec::new(),
        evidence: EvidencePolicy::default(),
        budget: VerificationBudget::default(),
        feature_map: vec![Feature {
            key: "items".into(),
            title: "項目".into(),
            criterion: "可以列出項目".into(),
            journeys: vec!["browse".into()],
        }],
        uncovered: Vec::new(),
        secrets: Vec::new(),
        personas: Vec::new(),
        identities: Vec::new(),
    }
}

fn charter_input() -> CharterInput {
    CharterInput {
        task_id: "task_x".into(),
        area: "結帳流程".into(),
        role: "一般顧客".into(),
        goal: "找出金額計算錯誤".into(),
        forbidden_side_effects: vec!["不得送出真實付款".into()],
        max_steps: 40,
        max_model_calls: 5,
    }
}

fn charter() -> Charter {
    define_charter(None, "ws", charter_input(), "crun_1".into(), "xch_1".into(), now()).unwrap().unwrap().0
}

fn report_input() -> FindingReport {
    FindingReport {
        title: "折扣後總額少算".into(),
        feature: Some("items".into()),
        steps: vec!["加入兩件商品".into(), "套用折扣碼".into()],
        expected: "總額為 180".into(),
        actual: "總額顯示 160".into(),
        environment: "本機 dev server，seed 資料 v3".into(),
        evidence: vec!["art_1".into()],
    }
}

fn finding(status: FindingStatus) -> ExplorationFinding {
    let (mut finding, _) = report(Some(&charter()), "ws", "task_x", report_input(), "run_1".into(), "xf_1".into(), now()).unwrap();
    finding.status = status;
    if status.is_defect() {
        finding.case = Some(ReproductionCase {
            journey: case_journey("xf_1", &journey("repro", false)),
            request_id: "vreq_0".into(),
            bundle_id: "bundle_0".into(),
            failed_step: Some("list".into()),
            confirmed_at: now(),
        });
    }
    if status == FindingStatus::Excluded {
        finding.exclusion =
            Some(Exclusion { reason: ExclusionReason::Environment, rationale: "測試機斷線".into(), decision_request_id: None });
    }
    finding
}

fn result(key: &str, representative: bool, verdict: Verdict) -> JourneyResult {
    JourneyResult {
        key: key.into(),
        title: key.into(),
        representative,
        persona: None,
        identity: None,
        verdict,
        reason_code: if verdict == Verdict::Passed { "all_steps_passed".into() } else { "assertion_failed".into() },
    }
}

fn bundle(journeys: Vec<JourneyResult>) -> EvidenceBundle {
    EvidenceBundle {
        id: "bundle_1".into(),
        workspace_id: "ws".into(),
        request_id: "vreq_1".into(),
        task_id: Some("task_x".into()),
        run_id: None,
        generation: None,
        requirement_version: Some(1),
        profile_version_id: "vp_ws_v1".into(),
        profile_version: 1,
        candidate_revision: None,
        build: None,
        subject_dir: "C:/ws".into(),
        journeys,
        steps: Vec::new(),
        verdict: Verdict::ProductFailure,
        reason_code: "assertion_failed".into(),
        covered_features: Vec::new(),
        uncovered: Vec::new(),
        cleanup: crate::verification_profile::CleanupRecord {
            state_dir: String::new(),
            state_dir_removed: true,
            service_stopped: None,
            problems: Vec::new(),
        },
        artifacts: Vec::new(),
        attempts: Vec::new(),
        started_at: now(),
        finished_at: now(),
    }
}

// ---------------------------------------------------------------- 章程

#[test]
fn a_charter_needs_area_role_goal_forbidden_side_effects_and_limits() {
    let mut input = charter_input();
    input.forbidden_side_effects.clear();
    assert!(matches!(define_charter(None, "ws", input, "c".into(), "x".into(), now()), Err(ExplorationError::Invalid(_))));
    for blank in ["area", "role", "goal"] {
        let mut input = charter_input();
        match blank {
            "area" => input.area = " ".into(),
            "role" => input.role = String::new(),
            _ => input.goal = "\n".into(),
        }
        assert!(matches!(define_charter(None, "ws", input, "c".into(), "x".into(), now()), Err(ExplorationError::Invalid(_))));
    }
    let mut input = charter_input();
    input.max_steps = 0;
    assert!(matches!(define_charter(None, "ws", input, "c".into(), "x".into(), now()), Err(ExplorationError::Invalid(_))));
    let mut input = charter_input();
    input.max_steps = MAX_STEPS_LIMIT + 1;
    assert!(matches!(define_charter(None, "ws", input, "c".into(), "x".into(), now()), Err(ExplorationError::Invalid(_))));
    let mut input = charter_input();
    input.max_model_calls = MAX_MODEL_CALLS_LIMIT + 1;
    assert!(matches!(define_charter(None, "ws", input, "c".into(), "x".into(), now()), Err(ExplorationError::Invalid(_))));
}

#[test]
fn redefining_a_charter_adds_a_version_unless_the_content_is_unchanged() {
    let first = charter();
    assert_eq!(first.version, 1);
    assert!(define_charter(Some(&first), "ws", charter_input(), "c".into(), "xch_2".into(), now()).unwrap().is_none());
    let mut input = charter_input();
    input.max_steps = 80;
    let (second, event) = define_charter(Some(&first), "ws", input, "c".into(), "xch_2".into(), now()).unwrap().unwrap();
    assert_eq!((second.version, second.max_steps), (2, 80));
    assert_eq!(event.event_type, EVENT_CHARTER_DEFINED);
}

#[test]
fn the_worker_prompt_spells_out_the_charter_boundaries() {
    let text = charter_prompt(&charter());
    for part in ["結帳流程", "一般顧客", "找出金額計算錯誤", "不得送出真實付款", "40", "5", "report_exploration_finding"] {
        assert!(text.contains(part), "{part}：{text}");
    }
}

// ---------------------------------------------------------------- 疑點

#[test]
fn findings_can_only_be_reported_under_a_charter_and_start_as_suspected() {
    let rejected = report(None, "ws", "task_x", report_input(), "run_1".into(), "xf_1".into(), now());
    assert_eq!(rejected.unwrap_err(), ExplorationError::CharterRequired("task_x".into()));
    let (finding, event) =
        report(Some(&charter()), "ws", "task_x", report_input(), "run_1".into(), "xf_1".into(), now()).unwrap();
    assert_eq!(finding.status, FindingStatus::Suspected);
    assert_eq!((finding.charter_id.as_str(), finding.charter_version), ("xch_1", 1));
    assert!(finding.case.is_none());
    assert_eq!(event.event_type, EVENT_FINDING_REPORTED);
}

#[test]
fn a_finding_keeps_reproduction_steps_expected_and_actual_observation() {
    for broken in ["steps", "expected", "actual", "title"] {
        let mut input = report_input();
        match broken {
            "steps" => input.steps.clear(),
            "expected" => input.expected = " ".into(),
            "actual" => input.actual = String::new(),
            _ => input.title = String::new(),
        }
        let rejected = report(Some(&charter()), "ws", "task_x", input, "run_1".into(), "xf_1".into(), now());
        assert!(matches!(rejected, Err(ExplorationError::Invalid(_))), "{broken}");
    }
}

// ---------------------------------------------------------------- 分流

#[test]
fn suspected_findings_can_be_excluded_with_a_reason_and_are_kept() {
    let change = triage(
        &finding(FindingStatus::Suspected),
        TriageAction::Exclude { reason: ExclusionReason::SeedData, rationale: "seed 資料的價格本來就是 160".into() },
        None,
    )
    .unwrap();
    assert_eq!(change.status, FindingStatus::Excluded);
    assert_eq!(change.exclusion.as_ref().map(|e| e.reason), Some(ExclusionReason::SeedData));
}

#[test]
fn excluding_as_expected_design_requires_the_users_decision() {
    let action = TriageAction::Exclude { reason: ExclusionReason::ExpectedDesign, rationale: "規格就是這樣".into() };
    assert_eq!(
        triage(&finding(FindingStatus::Suspected), action.clone(), None).unwrap_err(),
        ExplorationError::DecisionRequired
    );
    let change = triage(&finding(FindingStatus::Suspected), action, Some("dreq_1".into())).unwrap();
    assert_eq!(change.exclusion.unwrap().decision_request_id.as_deref(), Some("dreq_1"));
}

#[test]
fn confirmed_defects_cannot_be_excluded_or_downgraded() {
    for status in [FindingStatus::Confirmed, FindingStatus::Fixed, FindingStatus::Regressed] {
        for action in [
            TriageAction::Exclude { reason: ExclusionReason::Environment, rationale: "環境".into() },
            TriageAction::Exclude { reason: ExclusionReason::ExpectedDesign, rationale: "設計".into() },
            TriageAction::MarkUnverified { rationale: "重現不了".into() },
            TriageAction::Reopen { rationale: "重開".into() },
        ] {
            let rejected = triage(&finding(status), action, Some("dreq_1".into())).unwrap_err();
            assert_eq!(rejected.code(), "exploration_defect_locked", "{status:?}");
        }
    }
}

#[test]
fn a_suspected_finding_can_be_kept_as_an_unverified_risk_and_excluded_ones_reopened() {
    let change =
        triage(&finding(FindingStatus::Suspected), TriageAction::MarkUnverified { rationale: "環境一直無法啟動".into() }, None)
            .unwrap();
    assert_eq!((change.status, change.unverified_reason.as_deref()), (FindingStatus::UnverifiedRisk, Some("環境一直無法啟動")));
    let reopened = triage(&finding(FindingStatus::Excluded), TriageAction::Reopen { rationale: "誤判".into() }, None).unwrap();
    assert_eq!((reopened.status, reopened.exclusion), (FindingStatus::Suspected, None));
    assert!(triage(&finding(FindingStatus::Suspected), TriageAction::Reopen { rationale: "x".into() }, None).is_err());
    assert!(matches!(
        triage(&finding(FindingStatus::Suspected), TriageAction::MarkUnverified { rationale: " ".into() }, None),
        Err(ExplorationError::Invalid(_))
    ));
}

// ---------------------------------------------------------------- 重現

#[test]
fn a_reproduction_case_must_run_in_the_profile_environment_with_an_automated_assertion() {
    let definition = profile();
    assert!(check_case(&definition, &journey("repro", false)).is_ok());
    let mut manual = journey("repro", false);
    manual.steps = vec![Step {
        key: "look".into(),
        title: String::new(),
        operation: Operation::Manual { instruction: "看看".into() },
        assertions: Vec::new(),
        external: false,
    }];
    assert!(matches!(check_case(&definition, &manual), Err(ExplorationError::Invalid(_))));
    let mut unknown_identity = journey("repro", false);
    unknown_identity.identity = Some("admin".into());
    assert!(matches!(check_case(&definition, &unknown_identity), Err(ExplorationError::Invalid(_))));
    // 與 Profile 旅程同名也可以：案例的識別一律由 Runtime 指定
    assert!(check_case(&definition, &journey("browse", false)).is_ok());
}

#[test]
fn planning_a_reproduction_picks_the_purpose_from_the_status_and_locks_the_confirmed_case() {
    let definition = profile();
    let planned = plan_reproduction(&finding(FindingStatus::Suspected), Some(&journey("repro", false)), &definition).unwrap();
    assert_eq!(planned.purpose, AttemptPurpose::Reproduce);
    assert_eq!(planned.journey.key, case_key("xf_1"));
    assert!(!planned.journey.representative);
    assert!(matches!(
        plan_reproduction(&finding(FindingStatus::Suspected), None, &definition),
        Err(ExplorationError::Invalid(_))
    ));

    // 已確認的缺陷：只能以確認時的案例驗證修復，不能換成別的案例
    let confirmed = finding(FindingStatus::Confirmed);
    let planned = plan_reproduction(&confirmed, None, &definition).unwrap();
    assert_eq!((planned.purpose, planned.journey.clone()), (AttemptPurpose::VerifyFix, confirmed.case.clone().unwrap().journey));
    let mut weaker = journey("repro", false);
    weaker.steps[0].assertions = vec![Assertion::NotContains { text: "錯誤".into() }];
    assert_eq!(plan_reproduction(&confirmed, Some(&weaker), &definition).unwrap_err().code(), "exploration_defect_locked");
    assert!(plan_reproduction(&confirmed, Some(&journey("repro", false)), &definition).is_ok());

    for status in [FindingStatus::Fixed, FindingStatus::Excluded] {
        assert_eq!(
            plan_reproduction(&finding(status), Some(&journey("repro", false)), &definition).unwrap_err().code(),
            "exploration_transition_refused",
            "{status:?}"
        );
    }
}

#[test]
fn a_reproduction_runs_the_representative_journeys_as_a_control() {
    assert_eq!(reproduction_journeys(&profile(), "xf_1"), vec!["browse".to_string(), case_key("xf_1")]);
}

#[test]
fn only_a_product_assertion_failure_with_a_healthy_control_confirms_a_finding() {
    let key = case_key("xf_1");
    let suspected = finding(FindingStatus::Suspected);
    let judged = judge_attempt(
        &suspected,
        AttemptPurpose::Reproduce,
        &bundle(vec![result("browse", true, Verdict::Passed), result(&key, false, Verdict::ProductFailure)]),
    );
    assert_eq!((judged.outcome, judged.status), (AttemptOutcome::Reproduced, Some(FindingStatus::Confirmed)));

    // 環境、授權、證據不足、預算：都不是產品缺陷，疑點維持原狀
    for verdict in [
        Verdict::EnvironmentBlocked,
        Verdict::AuthBlocked,
        Verdict::Inconclusive,
        Verdict::BudgetExhausted,
        Verdict::AutomationUnsupported,
    ] {
        let judged = judge_attempt(
            &suspected,
            AttemptPurpose::Reproduce,
            &bundle(vec![result("browse", true, Verdict::Passed), result(&key, false, verdict)]),
        );
        assert_eq!((judged.outcome, judged.status), (AttemptOutcome::Inconclusive, None), "{verdict:?}");
    }
    // 對照旅程沒有通過：環境沒有證明可用，案例的產品斷言失敗也不算確認
    let judged = judge_attempt(
        &suspected,
        AttemptPurpose::Reproduce,
        &bundle(vec![result("browse", true, Verdict::EnvironmentBlocked), result(&key, false, Verdict::ProductFailure)]),
    );
    assert_eq!((judged.outcome, judged.status), (AttemptOutcome::Inconclusive, None));
    assert_eq!(judged.control_verdict, Some(Verdict::EnvironmentBlocked));
    // 案例沒有出現在 Bundle 中：不能當作通過
    let judged = judge_attempt(&suspected, AttemptPurpose::Reproduce, &bundle(vec![result("browse", true, Verdict::Passed)]));
    assert_eq!((judged.outcome, judged.status, judged.verdict), (AttemptOutcome::Inconclusive, None, None));
}

#[test]
fn a_reproduction_that_passes_keeps_the_finding_as_an_unverified_risk() {
    let key = case_key("xf_1");
    let passing = bundle(vec![result("browse", true, Verdict::Passed), result(&key, false, Verdict::Passed)]);
    let judged = judge_attempt(&finding(FindingStatus::Suspected), AttemptPurpose::Reproduce, &passing);
    assert_eq!((judged.outcome, judged.status), (AttemptOutcome::NotReproduced, Some(FindingStatus::UnverifiedRisk)));
    let judged = judge_attempt(&finding(FindingStatus::UnverifiedRisk), AttemptPurpose::Reproduce, &passing);
    assert_eq!((judged.outcome, judged.status), (AttemptOutcome::NotReproduced, None));
}

#[test]
fn a_fix_is_verified_only_when_the_confirmed_case_passes() {
    let key = case_key("xf_1");
    let confirmed = finding(FindingStatus::Confirmed);
    let judged = judge_attempt(
        &confirmed,
        AttemptPurpose::VerifyFix,
        &bundle(vec![result("browse", true, Verdict::Passed), result(&key, false, Verdict::Passed)]),
    );
    assert_eq!((judged.outcome, judged.status), (AttemptOutcome::FixVerified, Some(FindingStatus::Fixed)));
    let judged = judge_attempt(
        &confirmed,
        AttemptPurpose::VerifyFix,
        &bundle(vec![result("browse", true, Verdict::Passed), result(&key, false, Verdict::ProductFailure)]),
    );
    assert_eq!((judged.outcome, judged.status), (AttemptOutcome::StillFailing, None));
    let judged = judge_attempt(
        &confirmed,
        AttemptPurpose::VerifyFix,
        &bundle(vec![result("browse", true, Verdict::Inconclusive), result(&key, false, Verdict::Passed)]),
    );
    assert_eq!((judged.outcome, judged.status), (AttemptOutcome::Inconclusive, None));
}

#[test]
fn a_fixed_defect_that_fails_in_regression_is_regressed_and_recovers_when_it_passes_again() {
    let key = case_key("xf_1");
    let failing = bundle(vec![result(&key, false, Verdict::ProductFailure)]);
    let judged = judge_attempt(&finding(FindingStatus::Fixed), AttemptPurpose::Regression, &failing);
    assert_eq!((judged.outcome, judged.status), (AttemptOutcome::Regressed, Some(FindingStatus::Regressed)));
    let passing = bundle(vec![result(&key, false, Verdict::Passed)]);
    let judged = judge_attempt(&finding(FindingStatus::Regressed), AttemptPurpose::Regression, &passing);
    assert_eq!((judged.outcome, judged.status), (AttemptOutcome::RegressionPassed, Some(FindingStatus::Fixed)));
    let judged = judge_attempt(&finding(FindingStatus::Fixed), AttemptPurpose::Regression, &passing);
    assert_eq!((judged.outcome, judged.status), (AttemptOutcome::RegressionPassed, None));
    let blocked = bundle(vec![result(&key, false, Verdict::EnvironmentBlocked)]);
    let judged = judge_attempt(&finding(FindingStatus::Fixed), AttemptPurpose::Regression, &blocked);
    assert_eq!((judged.outcome, judged.status), (AttemptOutcome::Inconclusive, None));
}

#[test]
fn only_fixed_and_regressed_defects_are_part_of_normal_regression() {
    let in_regression: Vec<FindingStatus> = FindingStatus::ALL.into_iter().filter(|s| s.in_regression()).collect();
    assert_eq!(in_regression, vec![FindingStatus::Fixed, FindingStatus::Regressed]);
}

#[test]
fn case_keys_cannot_collide_with_profile_journeys() {
    let key = case_key("xf_1");
    assert_eq!(finding_of_case(&key), Some("xf_1"));
    assert_eq!(finding_of_case("browse"), None);
    // Profile 的旅程識別只能用小寫英數、_ 與 -，案例識別含 `.`，不會撞名
    let mut definition = profile();
    definition.journeys[1].key = key;
    assert!(crate::verification_profile::validate(&definition).is_err());
}

#[test]
fn applying_a_status_change_bumps_the_version_and_emits_an_event() {
    let suspected = finding(FindingStatus::Suspected);
    let change = triage(&suspected, TriageAction::MarkUnverified { rationale: "無法重現".into() }, None).unwrap();
    let (updated, event) = apply(&suspected, change, "triaged", now());
    assert_eq!((updated.version, updated.status), (2, FindingStatus::UnverifiedRisk));
    assert_eq!((event.event_type, event.aggregate_version), (EVENT_FINDING_STATUS_CHANGED, 2));
    assert_eq!(event.payload["from"], "suspected");
    assert_eq!(event.payload["to"], "unverified_risk");
}
