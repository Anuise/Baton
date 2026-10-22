use super::*;

const T0: Timestamp = Timestamp::from_millis(1_700_000_000_000);

fn axis(summary: &str, paths: &[&str], findings: &[&str]) -> AxisInput {
    AxisInput {
        summary: summary.into(),
        paths: paths.iter().map(|p| p.to_string()).collect(),
        findings: findings.iter().map(|t| FindingInput { title: t.to_string(), detail: format!("{t} 的說明"), location: None }).collect(),
        reuse_from: None,
    }
}

fn reuse(from: &str) -> AxisInput {
    AxisInput { reuse_from: Some(from.into()), ..AxisInput::default() }
}

fn submission(candidate: &str, standards: AxisInput, spec: AxisInput) -> ReviewSubmission {
    ReviewSubmission {
        subject_task_id: "t_author".into(),
        base_revision: "base".into(),
        candidate_revision: candidate.into(),
        spec_version: 2,
        standards_source: "AGENTS.md@base".into(),
        standards,
        spec,
    }
}

fn reviewer(task: &str) -> Reviewer {
    Reviewer { run_id: format!("run_{task}"), task_id: task.into(), sessions: vec![format!("ses_{task}")] }
}

fn subject() -> SubjectTask {
    SubjectTask { workspace_id: "ws".into(), id: "t_author".into(), contract_version: 2, author_sessions: vec!["ses_t_author".into()] }
}

fn submit_as(id: &str, task: &str, input: ReviewSubmission, priors: &[PriorReview]) -> Result<Submitted, ReviewError> {
    let mut n = 0;
    submit(id.into(), &mut || { n += 1; format!("{id}_f{n}") }, input, &reviewer(task), &subject(), priors, T0)
}

fn submitted(id: &str, input: ReviewSubmission) -> Submitted {
    submit_as(id, "t_reviewer", input, &[]).unwrap()
}

fn prior(s: &Submitted, changed: Result<Vec<&str>, &str>) -> PriorReview {
    PriorReview {
        review: s.review.clone(),
        findings: s.findings.clone(),
        changed_paths: changed.map(|c| c.iter().map(|p| p.to_string()).collect()).map_err(str::to_string),
    }
}

fn dismissed(mut findings: Vec<Finding>) -> Vec<Finding> {
    for f in &mut findings {
        f.state = FindingState::Dismissed;
    }
    findings
}

// --- 雙軸各自保存；任一軸不通過整份就不通過 ---

#[test]
fn both_axes_are_kept_separately_with_fixed_revisions_and_spec_version() {
    let s = submitted("rev1", submission("c1", axis("依 AGENTS.md 檢查", &["src/export.rs"], &["命名不一致"]), axis("對照契約第 2 版", &["src/export.rs"], &[])));
    assert_eq!((s.review.base_revision.as_str(), s.review.candidate_revision.as_str(), s.review.spec_version), ("base", "c1", 2));
    assert_eq!(s.review.standards_source, "AGENTS.md@base");
    assert_eq!((s.review.standards.reviewer_task_id.as_str(), s.review.spec.reviewer_task_id.as_str()), ("t_reviewer", "t_reviewer"));
    assert_eq!(s.findings.len(), 1);
    assert_eq!((s.findings[0].axis, s.findings[0].state), (Axis::Standards, FindingState::Open));
    assert_eq!(s.event.event_type, EVENT_REVIEW_SUBMITTED);
}

#[test]
fn spec_met_but_standards_violated_and_standards_met_but_spec_missing_both_fail() {
    let standards_violated = submitted("r1", submission("c1", axis("規範", &["src"], &["違反錯誤處理規範"]), axis("需求", &["src"], &[])));
    assert!(!axis_passed(&standards_violated.review, Axis::Standards, &standards_violated.findings));
    assert!(axis_passed(&standards_violated.review, Axis::Spec, &standards_violated.findings));
    assert!(!review_passed(&standards_violated.review, &standards_violated.findings));

    let spec_missing = submitted("r2", submission("c1", axis("規範", &["src"], &[]), axis("需求", &["src"], &["缺少匯出 CSV 標頭"])));
    assert!(axis_passed(&spec_missing.review, Axis::Standards, &spec_missing.findings));
    assert!(!review_passed(&spec_missing.review, &spec_missing.findings));

    // 分類為修正、等待裁定都不算解決；只有駁回（經使用者裁定）才算
    let mut findings = spec_missing.findings.clone();
    for state in [FindingState::Fix, FindingState::AwaitingDecision] {
        findings[0].state = state;
        assert!(!review_passed(&spec_missing.review, &findings), "{state:?}");
    }
    assert!(review_passed(&spec_missing.review, &dismissed(findings)));

    let clean = submitted("r3", submission("c1", axis("規範", &["src"], &[]), axis("需求", &["src"], &[])));
    assert!(review_passed(&clean.review, &clean.findings));
}

#[test]
fn each_axis_must_say_what_was_reviewed() {
    let err = submit_as("r", "t_reviewer", submission("c1", axis(" ", &["src"], &[]), axis("需求", &["src"], &[])), &[]).unwrap_err();
    assert_eq!(err.code(), "review_invalid");
    assert!(err.to_string().contains("規範"), "{err}");
    let mut bad = submission("c1", axis("規範", &[], &[]), axis("需求", &[], &[]));
    bad.candidate_revision = "-x".into();
    assert_eq!(submit_as("r", "t_reviewer", bad, &[]).unwrap_err().code(), "review_invalid");
}

// --- 作者不能擔任自己工作的 reviewer ---

#[test]
fn the_author_cannot_review_their_own_work_even_from_a_rebuilt_session() {
    let input = || submission("c1", axis("規範", &[], &[]), axis("需求", &[], &[]));
    let err = submit_as("r", "t_author", input(), &[]).unwrap_err();
    assert_eq!(err, ReviewError::ReviewerIsAuthor);

    // 另一個 Task 的 Run 卻沿用作者的 Session（例如重建或接續）：仍是作者
    let mut n = 0;
    let borrowed = Reviewer { run_id: "run_x".into(), task_id: "t_other".into(), sessions: vec!["ses_t_author".into()] };
    let err = submit("r".into(), &mut || { n += 1; n.to_string() }, input(), &borrowed, &subject(), &[], T0).unwrap_err();
    assert_eq!(err.code(), "reviewer_is_author");
}

#[test]
fn a_review_against_an_old_spec_version_is_refused() {
    let mut input = submission("c1", axis("規範", &[], &[]), axis("需求", &[], &[]));
    input.spec_version = 1;
    assert_eq!(submit_as("r", "t_reviewer", input, &[]).unwrap_err(), ReviewError::SpecOutdated { submitted: 1, current: 2 });
}

// --- 修正後只沿用可證明不受影響的審查結果 ---

#[test]
fn an_axis_is_reused_only_when_the_change_provably_does_not_touch_what_it_reviewed() {
    // 第一次：規範軸有 finding（要修正），需求軸乾淨、只審過 src/export.rs
    let first = submitted("r1", submission("c1", axis("規範", &["src"], &["錯誤處理"]), axis("需求", &["src/export.rs"], &[])));

    // 修正只動到 src/util.rs：需求軸可以沿用
    let ok = submit_as("r2", "t_reviewer2", submission("c2", axis("規範重審", &["src"], &[]), reuse("r1")), &[prior(&first, Ok(vec!["src/util.rs"]))]).unwrap();
    assert_eq!(ok.review.spec.reused_from.as_deref(), Some("r1"));
    assert_eq!(ok.review.spec.paths, ["src/export.rs"]);
    assert_eq!(ok.review.spec.reviewer_task_id, "t_reviewer", "沿用的軸記原本的 reviewer");
    assert_eq!(ok.review.standards.reviewer_task_id, "t_reviewer2");
    assert!(review_passed(&ok.review, &ok.findings));

    // 修正動到需求軸審過的檔案：不能沿用
    let err = submit_as("r3", "t_reviewer2", submission("c2", axis("規範", &["src"], &[]), reuse("r1")), &[prior(&first, Ok(vec!["SRC\\Export.rs"]))])
        .unwrap_err();
    assert!(matches!(&err, ReviewError::ReuseRejected { axis: Axis::Spec, reason } if reason.contains("src/export.rs")), "{err}");

    // 審過整個目錄：目錄下任何變更都算受影響
    let dir = submitted("r4", submission("c1", axis("規範", &["src"], &[]), axis("需求", &["src/"], &[])));
    let err = submit_as("r5", "t_reviewer2", submission("c2", axis("規範", &["src"], &[]), reuse("r4")), &[prior(&dir, Ok(vec!["src/a/b.rs"]))]).unwrap_err();
    assert_eq!(err.code(), "review_reuse_rejected");

    // 無法取得變更（例如不是 Git repo）：無法證明，不能沿用
    let err = submit_as("r6", "t_reviewer2", submission("c2", axis("規範", &["src"], &[]), reuse("r1")), &[prior(&first, Err("不是 Git repo"))]).unwrap_err();
    assert!(err.to_string().contains("不是 Git repo"), "{err}");

    // 這一軸先前還有未解決的 finding：不能沿用
    let err = submit_as("r7", "t_reviewer2", submission("c2", reuse("r1"), axis("需求", &["src"], &[])), &[prior(&first, Ok(vec![]))]).unwrap_err();
    assert!(matches!(&err, ReviewError::ReuseRejected { axis: Axis::Standards, .. }), "{err}");
    // 駁回之後就可以
    let mut resolved = prior(&first, Ok(vec!["docs/readme.md"]));
    resolved.findings = dismissed(resolved.findings);
    assert!(submit_as("r8", "t_reviewer2", submission("c2", reuse("r1"), axis("需求", &["src"], &[])), &[resolved]).is_ok());
}

#[test]
fn reuse_also_requires_the_same_spec_version_or_standards_source_and_recorded_paths() {
    let first = submitted("r1", submission("c1", axis("規範", &[], &[]), axis("需求", &["src"], &[])));
    // 規範軸沒有記錄範圍：無法證明
    let err = submit_as("r2", "t_x", submission("c2", reuse("r1"), axis("需求", &["src"], &[])), &[prior(&first, Ok(vec![]))]).unwrap_err();
    assert!(err.to_string().contains("paths"), "{err}");
    // 規範來源不同
    let mut other_source = submission("c1", reuse("r1"), axis("需求", &["src"], &[]));
    other_source.standards_source = "AGENTS.md@v2".into();
    let with_paths = submitted("r1", submission("c1", axis("規範", &["src"], &[]), axis("需求", &["src"], &[])));
    let err = submit_as("r3", "t_x", other_source, &[prior(&with_paths, Ok(vec![]))]).unwrap_err();
    assert!(err.to_string().contains("規範來源"), "{err}");
    // 規格版本不同（先前的審查對照舊契約）
    let mut old = prior(&with_paths, Ok(vec![]));
    old.review.spec_version = 1;
    let err = submit_as("r4", "t_x", submission("c1", axis("規範", &["src"], &[]), reuse("r1")), &[old]).unwrap_err();
    assert!(err.to_string().contains("規格版本"), "{err}");
    // 沿用時不能另外附 finding；找不到要沿用的審查
    let mut mixed = reuse("r1");
    mixed.findings = axis("", &[], &["x"]).findings;
    let err = submit_as("r5", "t_x", submission("c1", axis("規範", &["src"], &[]), mixed), &[prior(&with_paths, Ok(vec![]))]).unwrap_err();
    assert_eq!(err.code(), "review_invalid");
    let err = submit_as("r6", "t_x", submission("c1", axis("規範", &["src"], &[]), reuse("nope")), &[]).unwrap_err();
    assert_eq!(err, ReviewError::ReviewNotFound("nope".into()));
}

// --- Gate 用的審查狀態 ---

#[test]
fn a_review_stands_only_for_its_task_the_current_spec_and_the_latest_candidate() {
    let s = submitted("r1", submission("c1", axis("規範", &["src"], &[]), axis("需求", &["src"], &["缺需求"])));
    let current = |findings: &[Finding], version, latest: &str, task: Option<&str>| standing(&s.review, findings, version, latest, task);
    assert!(matches!(current(&s.findings, 2, "c1", Some("t_author")), Standing::NotPassed(axes) if axes == vec![Axis::Spec]));
    let ok = dismissed(s.findings.clone());
    assert_eq!(current(&ok, 2, "c1", Some("t_author")), Standing::Passed);
    assert!(matches!(current(&ok, 3, "c1", Some("t_author")), Standing::Stale(reason) if reason.contains("第 3 版")));
    assert!(matches!(current(&ok, 2, "c2", Some("t_author")), Standing::Stale(reason) if reason.contains("c2")));
    assert_eq!(current(&ok, 2, "c1", Some("t_other")), Standing::OtherSubject);
    assert_eq!(current(&ok, 2, "c1", None), Standing::OtherSubject);
}

// --- finding 的分類 ---

#[test]
fn a_dismissed_finding_is_final_and_every_classification_keeps_who_and_why() {
    let s = submitted("r1", submission("c1", axis("規範", &["src"], &["命名"]), axis("需求", &["src"], &[])));
    let finding = s.findings[0].clone();
    let change = FindingChange { state: FindingState::AwaitingDecision, classification: Classification::Dismiss, reason: "命名沿用既有慣例".into() };
    let (waiting, event) = classify(&finding, change, Some("dr1".into()), None, "ctl_1", T0).unwrap();
    assert_eq!((waiting.state, waiting.decision_request_id.as_deref(), waiting.version), (FindingState::AwaitingDecision, Some("dr1"), 2));
    assert_eq!((event.event_type, event.aggregate_version), (EVENT_FINDING_CLASSIFIED, 2));
    let change = FindingChange { state: FindingState::Dismissed, classification: Classification::Dismiss, reason: "命名沿用既有慣例".into() };
    let (done, _) = classify(&waiting, change.clone(), Some("dr1".into()), Some("desktop".into()), "ctl_2", T0).unwrap();
    assert_eq!((done.decided_by.as_deref(), done.classified_by.as_deref()), (Some("desktop"), Some("ctl_2")));
    assert_eq!(classify(&done, change, None, None, "ctl_3", T0).unwrap_err(), ReviewError::FindingResolved("r1_f1".into()));

    let empty = FindingChange { state: FindingState::Fix, classification: Classification::Fix, reason: "  ".into() };
    assert_eq!(classify(&finding, empty, None, None, "ctl_1", T0).unwrap_err().code(), "review_invalid");
}

// --- 設計偏離 ---

fn deviation_input(version: i64) -> DeviationInput {
    DeviationInput {
        design_artifact_id: "art_design".into(),
        design_version: version,
        deviation: "改用串流寫檔".into(),
        reason: "大檔案記憶體不足".into(),
        affected_contracts: vec!["匯出 API 回應時間".into()],
        proposed_design: "# 設計 v2\n改用串流".into(),
    }
}

fn reporter() -> Reporter {
    Reporter { workspace_id: "ws".into(), run_id: "run_impl".into(), task_id: "t_author".into(), generation: 1 }
}

#[test]
fn a_deviation_is_reported_against_the_current_design_version_and_resolved_once() {
    assert_eq!(report_deviation("dev1".into(), deviation_input(1), &reporter(), Some(2), T0).unwrap_err(), ReviewError::DesignOutdated { reported: 1, current: 2 });
    assert_eq!(report_deviation("dev1".into(), deviation_input(1), &reporter(), None, T0).unwrap_err().code(), "design_not_found");
    let mut empty = deviation_input(2);
    empty.reason = " ".into();
    assert_eq!(report_deviation("dev1".into(), empty, &reporter(), Some(2), T0).unwrap_err().code(), "review_invalid");

    let (deviation, event) = report_deviation("dev1".into(), deviation_input(2), &reporter(), Some(2), T0).unwrap();
    assert_eq!((deviation.status, deviation.design_version, deviation.version), (DeviationStatus::Proposed, 2, 1));
    assert_eq!(event.event_type, EVENT_DEVIATION_REPORTED);

    let awaiting = DeviationResolution { status: DeviationStatus::AwaitingDecision, reason: "合理".into(), decision_request_id: Some("dr".into()), accepted_by: None, accepted_design_version: None };
    let (waiting, _) = resolve_deviation(&deviation, awaiting, "ctl_1", T0).unwrap();
    let accepted = DeviationResolution {
        status: DeviationStatus::Accepted,
        reason: "合理".into(),
        decision_request_id: Some("dr".into()),
        accepted_by: Some("desktop".into()),
        accepted_design_version: Some(3),
    };
    let (done, event) = resolve_deviation(&waiting, accepted.clone(), "ctl_2", T0).unwrap();
    assert_eq!((done.status, done.accepted_by.as_deref(), done.accepted_design_version), (DeviationStatus::Accepted, Some("desktop"), Some(3)));
    assert_eq!((event.event_type, event.aggregate_version), (EVENT_DEVIATION_RESOLVED, 3));
    assert_eq!(resolve_deviation(&done, accepted, "ctl_3", T0).unwrap_err(), ReviewError::DeviationResolved("dev1".into()));
}
