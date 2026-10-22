//! 票 50：Gate 失效與重驗。

use super::*;

/// 五個 Stage：設計（成果）→ 驗證（Evidence Bundle）→ 審查（雙軸審查）→ 裁定（使用者裁定）→ 整合（成果接受）。
fn gated_playbook() -> PlaybookVersion {
    let stage = |key: &str, requires: EvidenceKind, gate: GateKind, route: FailureRoute| StageDefinition {
        key: key.into(),
        title: format!("Stage {key}"),
        method: None,
        entry_conditions: vec![],
        expected_outputs: vec![],
        allowed_actions: vec![],
        budget: StageBudget { max_attempts: 3 },
        exit_checks: vec![ExitCheck { key: format!("{key}_check"), description: format!("{key} 的證據"), requires, reviewers: None }],
        failure_route: route,
        gate,
    };
    let definition = PlaybookDefinition {
        key: "feature".into(),
        title: "功能開發".into(),
        situation: Situation::Feature,
        applicability: "測試".into(),
        stages: vec![
            stage("design", EvidenceKind::Artifact, GateKind::None, FailureRoute::Retry),
            stage("verify", EvidenceKind::Verification, GateKind::Safety, FailureRoute::ReturnTo { stage: "design".into() }),
            stage("review", EvidenceKind::Review, GateKind::Safety, FailureRoute::Retry),
            stage("decide", EvidenceKind::Decision, GateKind::Decision, FailureRoute::Block),
            stage("integrate", EvidenceKind::TaskAccepted, GateKind::Safety, FailureRoute::Retry),
        ],
    };
    publish(None, definition, "pbv_gated")
}

fn gated_started() -> (WorkflowInstance, PlaybookVersion) {
    let playbook = gated_playbook();
    let route = route(std::slice::from_ref(&playbook), Situation::Feature, Some(TaskKind::Development), "測試").unwrap();
    let input = StartInput {
        workspace_id: "ws".into(),
        goal_id: "goal".into(),
        task_id: Some("task".into()),
        route,
        created_by: "ctl_1".into(),
    };
    (start("wf".into(), input, &playbook, None, T0).unwrap().workflow, playbook)
}

/// 以有效證據、在 `build` 上通過目前的 Stage。
fn pass_at(workflow: &WorkflowInstance, playbook: &PlaybookVersion, prior: &mut Vec<StageAttempt>, build: &str) -> WorkflowInstance {
    let stage = playbook.definition.stage(workflow.current_stage.as_deref().unwrap()).unwrap();
    let check = &stage.exit_checks[0];
    let reference = match check.requires {
        EvidenceKind::Artifact => EvidenceRef::Artifact { artifact_id: "art".into() },
        EvidenceKind::Decision => EvidenceRef::Decision { decision_request_id: "dr".into() },
        EvidenceKind::TaskAccepted => EvidenceRef::TaskAccepted { task_id: "task".into() },
        EvidenceKind::Review => EvidenceRef::Review { review_id: "rev".into() },
        EvidenceKind::Verification => EvidenceRef::Verification { evidence_bundle_id: "evb".into() },
    };
    let reviewers = (check.requires == EvidenceKind::Review)
        .then(|| AxisReviewers { standards: "t_reviewer".into(), spec: "t_reviewer".into() });
    let evidence = vec![CheckedEvidence { evidence: Evidence { check: check.key.clone(), reference }, status: EvidenceStatus::Valid, reviewers }];
    let input = StageResultInput {
        stage: stage.key.clone(),
        expected_revision: workflow.revision,
        outcome: ClaimedOutcome::Passed,
        evidence,
        note: String::new(),
        build: Some(build.into()),
        basis_overturned: None,
        budget_decision: BudgetDecision::default(),
    };
    let change = record_result(workflow, playbook, prior, input, format!("att_{}", prior.len()), recorder(), T0).unwrap();
    prior.push(change.attempt.unwrap());
    change.workflow
}

/// 走到「整合」：前四個 Stage 都在 build c1 上通過。
fn at_integrate() -> (WorkflowInstance, PlaybookVersion, Vec<StageAttempt>) {
    let (mut workflow, playbook) = gated_started();
    let mut prior = Vec::new();
    for _ in 0..4 {
        workflow = pass_at(&workflow, &playbook, &mut prior, "c1");
    }
    assert_eq!(workflow.current_stage.as_deref(), Some("integrate"));
    (workflow, playbook, prior)
}

/// 重新查驗的事實：目前有效通過的 Stage 的證據照原樣有效（`statuses` 覆寫某個 Stage 的查驗結果）。
fn facts(
    workflow: &WorkflowInstance,
    playbook: &PlaybookVersion,
    prior: &[StageAttempt],
    build: Option<&str>,
    statuses: &[(&str, EvidenceStatus)],
) -> GateFacts {
    let rechecked = passed_attempts(workflow, playbook, prior)
        .into_iter()
        .map(|a| {
            let status = statuses.iter().find(|(s, _)| *s == a.stage_key).map(|(_, st)| *st);
            let evidence = a
                .evidence
                .iter()
                .cloned()
                .map(|mut e| {
                    if let Some(status) = status {
                        e.status = status;
                    }
                    e
                })
                .collect();
            (a.stage_key.clone(), evidence)
        })
        .collect();
    GateFacts { rechecked, current_build: build.map(str::to_string), basis_overturned: None }
}

const T1: Timestamp = Timestamp::from_millis(1_700_000_060_000);

#[test]
fn a_build_change_invalidates_only_the_gates_whose_evidence_depends_on_the_build() {
    let (workflow, playbook, mut prior) = at_integrate();
    let invalidation =
        revalidate(&workflow, &playbook, &prior, &facts(&workflow, &playbook, &prior, Some("c2"), &[]), T1).expect("build 改變了");

    let invalidated: Vec<&str> = invalidation.attempts.iter().map(|a| a.stage_key.as_str()).collect();
    assert_eq!(invalidated, vec!["verify", "review"], "設計、裁定與 build 無關，不受影響");
    for attempt in &invalidation.attempts {
        assert_eq!(attempt.outcome, AttemptOutcome::Invalidated);
        assert_eq!(attempt.recorded_by, RECORDED_BY_RUNTIME);
        assert!(attempt.missing.iter().all(|m| m.reason.contains("c1") && m.reason.contains("c2")), "{:?}", attempt.missing);
    }
    let next = &invalidation.workflow;
    assert_eq!((next.current_stage.as_deref(), next.status), (Some("verify"), WorkflowStatus::Active), "回到最早失效的 Gate 重驗");
    assert_eq!(next.resume_stage.as_deref(), Some("integrate"));
    assert_eq!(next.revision, workflow.revision + 1);
    assert_eq!(invalidation.event.event_type, EVENT_GATE_INVALIDATED);
    prior.extend(invalidation.attempts.clone());
    assert_eq!(
        stage_states(next, &playbook, &prior),
        vec![StageState::Passed, StageState::Current, StageState::Invalidated, StageState::Passed, StageState::Pending]
    );
    // 已失效的 Gate 不會再失效一次
    assert!(revalidate(next, &playbook, &prior, &facts(next, &playbook, &prior, Some("c3"), &[]), T1).is_none());

    // 重驗：驗證與審查在新的 build 上通過後，仍有效的裁定 Gate 被沿用，直接回到原本的「整合」
    let next = pass_at(next, &playbook, &mut prior, "c2");
    assert_eq!(next.current_stage.as_deref(), Some("review"));
    let next = pass_at(&next, &playbook, &mut prior, "c2");
    assert_eq!(next.current_stage.as_deref(), Some("integrate"));
    assert_eq!(next.resume_stage, None);
    assert!(stage_states(&next, &playbook, &prior)[..4].iter().all(|s| *s == StageState::Passed));
}

#[test]
fn the_same_or_an_unknown_build_keeps_every_gate() {
    let (workflow, playbook, prior) = at_integrate();
    assert!(revalidate(&workflow, &playbook, &prior, &facts(&workflow, &playbook, &prior, Some("c1"), &[]), T1).is_none());
    assert!(revalidate(&workflow, &playbook, &prior, &facts(&workflow, &playbook, &prior, None, &[]), T1).is_none());
}

#[test]
fn evidence_that_no_longer_checks_out_invalidates_its_gate() {
    let (workflow, playbook, prior) = at_integrate();
    // 例如 Verification Profile 或契約改版：Bundle 不再有效，審查已過期
    let facts = facts(
        &workflow,
        &playbook,
        &prior,
        Some("c1"),
        &[("verify", EvidenceStatus::NotPassed), ("review", EvidenceStatus::Stale)],
    );
    let invalidation = revalidate(&workflow, &playbook, &prior, &facts, T1).unwrap();
    let reasons: Vec<(&str, &str)> =
        invalidation.attempts.iter().map(|a| (a.stage_key.as_str(), a.missing[0].reason.as_str())).collect();
    assert_eq!(reasons.len(), 2);
    assert!(reasons[0].1.contains("Profile"), "{reasons:?}");
    assert!(reasons[1].1.contains("過期"), "{reasons:?}");
    assert_eq!(invalidation.workflow.current_stage.as_deref(), Some("verify"));
}

#[test]
fn a_bundle_for_another_build_does_not_open_the_gate() {
    let (workflow, playbook) = gated_started();
    let mut prior = Vec::new();
    let workflow = pass_at(&workflow, &playbook, &mut prior, "c1");
    let bundle = CheckedEvidence {
        evidence: Evidence { check: "verify_check".into(), reference: EvidenceRef::Verification { evidence_bundle_id: "evb".into() } },
        status: EvidenceStatus::BuildChanged,
        reviewers: None,
    };
    let after = report(&workflow, &playbook, &mut prior, ClaimedOutcome::Passed, vec![bundle], "").unwrap();
    assert_eq!(after.current_stage.as_deref(), Some("verify"));
    let attempt = prior.last().unwrap();
    assert_eq!(attempt.outcome, AttemptOutcome::Incomplete);
    assert!(attempt.missing[0].reason.contains("build"), "{:?}", attempt.missing);
}

#[test]
fn an_overturned_assumption_invalidates_all_but_user_decisions_and_blocks_passing_until_the_contract_changes() {
    let (workflow, playbook, prior) = at_integrate();
    let mut facts = facts(&workflow, &playbook, &prior, Some("c1"), &[]);
    facts.basis_overturned = Some("使用者推翻了假設「用 REST」".into());
    let invalidation = revalidate(&workflow, &playbook, &prior, &facts, T1).unwrap();
    let invalidated: Vec<&str> = invalidation.attempts.iter().map(|a| a.stage_key.as_str()).collect();
    assert_eq!(invalidated, vec!["design", "verify", "review"], "使用者的裁定本身不依賴假設");
    let workflow = invalidation.workflow;
    assert_eq!(workflow.current_stage.as_deref(), Some("design"));

    let mut prior = prior;
    prior.extend(invalidation.attempts);
    let input = StageResultInput {
        stage: "design".into(),
        expected_revision: workflow.revision,
        outcome: ClaimedOutcome::Passed,
        evidence: vec![evidence("design_check", EvidenceStatus::Valid)],
        note: String::new(),
        build: Some("c1".into()),
        basis_overturned: Some("使用者推翻了假設「用 REST」".into()),
        budget_decision: BudgetDecision::default(),
    };
    let change = record_result(&workflow, &playbook, &prior, input, "att_x".into(), recorder(), T1).unwrap();
    assert_eq!(change.attempt.unwrap().outcome, AttemptOutcome::Blocked, "舊成果不能在被推翻的前提下通過");
    assert_eq!(change.workflow.status, WorkflowStatus::Blocked);
    assert!(change.workflow.blocked_reason.unwrap().contains("用 REST"));
}

#[test]
fn a_failure_route_back_redoes_later_stages_instead_of_reusing_them() {
    let (workflow, playbook, mut prior) = at_integrate();
    let invalidation =
        revalidate(&workflow, &playbook, &prior, &facts(&workflow, &playbook, &prior, Some("c2"), &[]), T1).unwrap();
    prior.extend(invalidation.attempts);
    let workflow = invalidation.workflow;
    let failed = report(&workflow, &playbook, &mut prior, ClaimedOutcome::Failed, vec![], "驗證失敗").unwrap();
    assert_eq!((failed.current_stage.as_deref(), failed.resume_stage.as_deref()), (Some("design"), None));
    let again = pass_at(&failed, &playbook, &mut prior, "c3");
    assert_eq!(again.current_stage.as_deref(), Some("verify"), "從失敗去向往後照常重做");
    assert_eq!(stage_states(&again, &playbook, &prior)[3], StageState::Pending);
}

#[test]
fn a_pass_records_the_build_it_was_made_on_and_the_controller_note_names_gates_to_reverify() {
    let (workflow, playbook, mut prior) = at_integrate();
    assert!(prior.iter().all(|a| a.build.as_deref() == Some("c1")));
    let invalidation =
        revalidate(&workflow, &playbook, &prior, &facts(&workflow, &playbook, &prior, Some("c2"), &[]), T1).unwrap();
    prior.extend(invalidation.attempts);
    let note = WorkflowNote::from_parts(&invalidation.workflow, &playbook, &prior, None).unwrap().render();
    assert!(note.contains("需要重驗") && note.contains("Stage review"), "{note}");
}
