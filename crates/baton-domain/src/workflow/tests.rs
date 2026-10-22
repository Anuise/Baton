use super::*;

const T0: Timestamp = Timestamp::from_millis(1_700_000_000_000);

fn method_keys() -> Vec<String> {
    builtin_methods().into_iter().map(|m| m.key).collect()
}

fn publish(latest: Option<&PlaybookVersion>, definition: PlaybookDefinition, id: &str) -> PlaybookVersion {
    let keys = method_keys();
    let keys: Vec<&str> = keys.iter().map(String::as_str).collect();
    publish_playbook(latest, definition, &keys, Origin::Builtin, id.into(), T0).unwrap().unwrap().0
}

fn catalog() -> Vec<PlaybookVersion> {
    builtin_playbooks().into_iter().map(|d| publish(None, d.clone(), &format!("pbv_{}", d.key))).collect()
}

fn playbook(key: &str) -> PlaybookVersion {
    catalog().into_iter().find(|p| p.key == key).unwrap()
}

fn started(key: &str) -> (WorkflowInstance, PlaybookVersion) {
    let playbook = playbook(key);
    let route = route(&catalog(), playbook.definition.situation, Some(TaskKind::Development), "測試").unwrap();
    let input = StartInput {
        workspace_id: "ws".into(),
        goal_id: "goal".into(),
        task_id: Some("task".into()),
        route,
        created_by: "ctl_1".into(),
    };
    (start("wf".into(), input, &playbook, None, T0).unwrap().workflow, playbook)
}

fn evidence(check: &str, status: EvidenceStatus) -> CheckedEvidence {
    CheckedEvidence {
        evidence: Evidence { check: check.into(), reference: EvidenceRef::Artifact { artifact_id: format!("art_{check}") } },
        status,
        reviewers: None,
    }
}

fn recorder() -> Recorder {
    Recorder { recorded_by: "ctl_1".into(), profile_id: Some("prof_a".into()) }
}

fn report(
    workflow: &WorkflowInstance,
    playbook: &PlaybookVersion,
    prior: &mut Vec<StageAttempt>,
    outcome: ClaimedOutcome,
    evidence: Vec<CheckedEvidence>,
    note: &str,
) -> Result<WorkflowInstance, WorkflowError> {
    report_with(workflow, playbook, prior, outcome, evidence, note, BudgetDecision::default())
}

fn report_with(
    workflow: &WorkflowInstance,
    playbook: &PlaybookVersion,
    prior: &mut Vec<StageAttempt>,
    outcome: ClaimedOutcome,
    evidence: Vec<CheckedEvidence>,
    note: &str,
    budget_decision: BudgetDecision,
) -> Result<WorkflowInstance, WorkflowError> {
    let input = StageResultInput {
        stage: workflow.current_stage.clone().unwrap(),
        expected_revision: workflow.revision,
        outcome,
        evidence,
        note: note.into(),
        build: None,
        basis_overturned: None,
        budget_decision,
    };
    let change = record_result(workflow, playbook, prior, input, format!("att_{}", prior.len()), recorder(), T0)?;
    prior.push(change.attempt.unwrap());
    Ok(change.workflow)
}

/// 以每項 exit check 都有效的證據通過目前的 Stage。
fn pass(workflow: &WorkflowInstance, playbook: &PlaybookVersion, prior: &mut Vec<StageAttempt>) -> WorkflowInstance {
    pass_with(workflow, playbook, prior, BudgetDecision::default())
}

/// 同 [`pass`]，附上「預算用盡後的下一步」裁定的狀態。
fn pass_with(
    workflow: &WorkflowInstance,
    playbook: &PlaybookVersion,
    prior: &mut Vec<StageAttempt>,
    budget_decision: BudgetDecision,
) -> WorkflowInstance {
    let stage = playbook.definition.stage(workflow.current_stage.as_deref().unwrap()).unwrap();
    let evidence = stage
        .exit_checks
        .iter()
        .map(|c| {
            let reference = match c.requires {
                EvidenceKind::Artifact => EvidenceRef::Artifact { artifact_id: "art".into() },
                EvidenceKind::Decision => EvidenceRef::Decision { decision_request_id: "dr".into() },
                EvidenceKind::TaskAccepted => EvidenceRef::TaskAccepted { task_id: "task".into() },
                EvidenceKind::Review => EvidenceRef::Review { review_id: "rev".into() },
                EvidenceKind::Verification => EvidenceRef::Verification { evidence_bundle_id: "evb".into() },
            };
            let reviewers = (c.requires == EvidenceKind::Review)
                .then(|| AxisReviewers { standards: "t_reviewer".into(), spec: "t_reviewer".into() });
            CheckedEvidence { evidence: Evidence { check: c.key.clone(), reference }, status: EvidenceStatus::Valid, reviewers }
        })
        .collect();
    report_with(workflow, playbook, prior, ClaimedOutcome::Passed, evidence, "", budget_decision).unwrap()
}

/// 每項 exit check 都有效的證據（給 [`needs_budget_decision`] 判斷用）。
fn full_input(workflow: &WorkflowInstance, playbook: &PlaybookVersion) -> StageResultInput {
    let stage = playbook.definition.stage(workflow.current_stage.as_deref().unwrap()).unwrap();
    StageResultInput {
        stage: stage.key.clone(),
        expected_revision: workflow.revision,
        outcome: ClaimedOutcome::Passed,
        evidence: stage.exit_checks.iter().map(|c| evidence(&c.key, EvidenceStatus::Valid)).collect(),
        note: String::new(),
        build: None,
        basis_overturned: None,
        budget_decision: BudgetDecision::default(),
    }
}

#[test]
fn the_seven_builtin_playbooks_are_valid_and_cover_every_situation() {
    let keys = method_keys();
    let keys: Vec<&str> = keys.iter().map(String::as_str).collect();
    let playbooks = builtin_playbooks();
    assert_eq!(playbooks.len(), 7);
    for situation in Situation::ALL {
        assert_eq!(playbooks.iter().filter(|p| p.situation == situation).count(), 1, "{situation:?}");
    }
    for playbook in &playbooks {
        validate_playbook(playbook, &keys).unwrap_or_else(|e| panic!("{}：{e}", playbook.key));
    }
}

#[test]
fn development_playbooks_have_safety_gates_and_non_development_ones_never_edit_code() {
    for playbook in builtin_playbooks() {
        let edits = playbook.stages.iter().any(|s| s.allowed_actions.iter().any(|a| a == "修改程式"));
        assert_eq!(edits, playbook.situation.is_development() && playbook.situation != Situation::Ambiguous, "{}", playbook.key);
    }
    assert!(playbook("feature").definition.stages.iter().any(|s| s.gate == GateKind::Safety));
    assert!(playbook("ambiguous").definition.stages.iter().any(|s| s.gate == GateKind::Decision));
}

#[test]
fn the_router_picks_the_playbook_for_the_situation_and_records_reason_and_version() {
    let catalog = catalog();
    for (situation, key) in [(Situation::Feature, "feature"), (Situation::Defect, "defect"), (Situation::Investigation, "investigation")]
    {
        let route = route(&catalog, situation, Some(TaskKind::Development), "使用者回報的問題").unwrap();
        assert_eq!(route.playbook_key, key);
        assert_eq!(route.playbook_version, 1);
        assert!(route.reason.contains(situation.label()) && route.reason.contains("第 1 版"), "{}", route.reason);
        assert!(route.reason.contains("使用者回報的問題"));
    }
}

#[test]
fn research_and_document_tasks_never_get_a_development_playbook() {
    let catalog = catalog();
    for kind in [TaskKind::Research, TaskKind::Document] {
        for situation in [Situation::Feature, Situation::Defect, Situation::Migration, Situation::Performance] {
            let error = route(&catalog, situation, Some(kind), "理由").unwrap_err();
            assert_eq!(error.code(), "playbook_not_applicable");
        }
        assert_eq!(route(&catalog, Situation::Investigation, Some(kind), "理由").unwrap().playbook_key, "investigation");
    }
    assert!(route(&catalog, Situation::Feature, Some(TaskKind::Development), " ").is_err(), "需要理由");
}

#[test]
fn a_new_version_is_appended_only_when_the_content_changes() {
    let keys = method_keys();
    let keys: Vec<&str> = keys.iter().map(String::as_str).collect();
    let v1 = playbook("feature");
    let same = publish_playbook(Some(&v1), v1.definition.clone(), &keys, Origin::Builtin, "x".into(), T0).unwrap();
    assert!(same.is_none());
    let mut changed = v1.definition.clone();
    changed.stages[0].title = "需求契約（新）".into();
    let (v2, event) = publish_playbook(Some(&v1), changed.clone(), &keys, Origin::User, "pbv2".into(), T0).unwrap().unwrap();
    assert_eq!((v2.version, v2.origin), (2, Origin::User));
    assert_ne!(v2.content_hash, v1.content_hash);
    assert_eq!(event.event_type, EVENT_PLAYBOOK_VERSION_PUBLISHED);
    changed.situation = Situation::Defect;
    assert_eq!(
        publish_playbook(Some(&v1), changed, &keys, Origin::User, "y".into(), T0).unwrap_err().code(),
        "playbook_invalid",
        "不能改情境"
    );
}

#[test]
fn invalid_playbooks_are_rejected_before_they_can_be_selected() {
    let keys = method_keys();
    let keys: Vec<&str> = keys.iter().map(String::as_str).collect();
    let base = playbook("defect").definition;
    let mut no_checks = base.clone();
    no_checks.stages[0].exit_checks.clear();
    let mut bad_route = base.clone();
    bad_route.stages[1].failure_route = FailureRoute::ReturnTo { stage: "nowhere".into() };
    let mut bad_method = base.clone();
    bad_method.stages[0].method = Some("unknown".into());
    let mut no_budget = base.clone();
    no_budget.stages[0].budget.max_attempts = 0;
    let mut decision_without_decision = base.clone();
    decision_without_decision.stages[0].gate = GateKind::Decision;
    let mut duplicate = base;
    duplicate.stages[1].key = duplicate.stages[0].key.clone();
    for definition in [no_checks, bad_route, bad_method, no_budget, decision_without_decision, duplicate] {
        assert_eq!(validate_playbook(&definition, &keys).unwrap_err().code(), "playbook_invalid");
    }
}

#[test]
fn a_stage_advances_only_with_verified_evidence_for_every_exit_check() {
    let (workflow, playbook) = started("defect");
    let mut prior = Vec::new();
    // 只說「做完了」：incomplete，留在同一個 Stage
    let after = report(&workflow, &playbook, &mut prior, ClaimedOutcome::Passed, vec![], "做完了").unwrap();
    assert_eq!(after.current_stage.as_deref(), Some("reproduction"));
    assert_eq!(after.status, WorkflowStatus::Active);
    assert_eq!(prior[0].outcome, AttemptOutcome::Incomplete);
    assert_eq!(prior[0].missing[0].check, "reproduction");
    // 引用不存在的成果：仍是 incomplete
    let after = report(&after, &playbook, &mut prior, ClaimedOutcome::Passed, vec![evidence("reproduction", EvidenceStatus::NotFound)], "")
        .unwrap();
    assert_eq!(prior[1].outcome, AttemptOutcome::Incomplete);
    assert_eq!(prior[1].attempt, 2);
    // 有效的證據才前進
    let after = report(&after, &playbook, &mut prior, ClaimedOutcome::Passed, vec![evidence("reproduction", EvidenceStatus::Valid)], "")
        .unwrap();
    assert_eq!(prior[2].outcome, AttemptOutcome::Passed);
    assert_eq!(after.current_stage.as_deref(), Some("minimization"));
    assert_eq!(after.revision, 4);
}

#[test]
fn a_pending_decision_blocks_the_stage_until_it_is_answered() {
    let (mut workflow, playbook) = started("ambiguous");
    let mut prior = Vec::new();
    for _ in 0..3 {
        workflow = pass(&workflow, &playbook, &mut prior);
    }
    assert_eq!(workflow.current_stage.as_deref(), Some("decision"));
    let pending = CheckedEvidence {
        evidence: Evidence { check: "decision".into(), reference: EvidenceRef::Decision { decision_request_id: "dr".into() } },
        status: EvidenceStatus::Pending,
        reviewers: None,
    };
    let blocked = report(&workflow, &playbook, &mut prior, ClaimedOutcome::Passed, vec![pending.clone()], "").unwrap();
    assert_eq!(blocked.status, WorkflowStatus::Blocked);
    assert_eq!(prior.last().unwrap().outcome, AttemptOutcome::Blocked);
    let answered = CheckedEvidence { status: EvidenceStatus::Valid, ..pending };
    let resumed = report(&blocked, &playbook, &mut prior, ClaimedOutcome::Passed, vec![answered], "").unwrap();
    assert_eq!((resumed.status, resumed.current_stage.as_deref()), (WorkflowStatus::Active, Some("update_brief")));
}

#[test]
fn evidence_must_match_an_exit_check_of_the_current_stage_and_its_kind() {
    let (workflow, playbook) = started("defect");
    let wrong_check = report(&workflow, &playbook, &mut Vec::new(), ClaimedOutcome::Passed, vec![evidence("fix", EvidenceStatus::Valid)], "");
    assert_eq!(wrong_check.unwrap_err().code(), "workflow_invalid");
    let wrong_kind = CheckedEvidence {
        evidence: Evidence { check: "reproduction".into(), reference: EvidenceRef::TaskAccepted { task_id: "task".into() } },
        status: EvidenceStatus::Valid,
        reviewers: None,
    };
    let wrong_kind = report(&workflow, &playbook, &mut Vec::new(), ClaimedOutcome::Passed, vec![wrong_kind], "");
    assert_eq!(wrong_kind.unwrap_err().code(), "workflow_invalid");
}

#[test]
fn only_the_current_stage_at_the_current_revision_can_be_reported() {
    let (workflow, playbook) = started("feature");
    let mut input = StageResultInput {
        stage: "design".into(),
        expected_revision: 1,
        outcome: ClaimedOutcome::Passed,
        evidence: vec![],
        note: String::new(),
        build: None,
        basis_overturned: None,
        budget_decision: BudgetDecision::default(),
    };
    let error = record_result(&workflow, &playbook, &[], input.clone(), "a".into(), recorder(), T0).unwrap_err();
    assert_eq!(error.code(), "stage_not_current");
    input.stage = "requirements_contract".into();
    input.expected_revision = 7;
    let error = record_result(&workflow, &playbook, &[], input, "a".into(), recorder(), T0).unwrap_err();
    assert_eq!(error.code(), "workflow_revision_mismatch");
}

#[test]
fn failures_follow_the_failure_route_until_the_budget_is_used_up() {
    let (mut workflow, playbook) = started("defect");
    let mut prior = Vec::new();
    for _ in 0..3 {
        workflow = pass(&workflow, &playbook, &mut prior);
    }
    assert_eq!(workflow.current_stage.as_deref(), Some("probe"));
    // probe 失敗 → 回到 hypothesis
    workflow = report(&workflow, &playbook, &mut prior, ClaimedOutcome::Failed, vec![], "假說被推翻").unwrap();
    assert_eq!(workflow.current_stage.as_deref(), Some("hypothesis"));
    assert!(report(&workflow, &playbook, &mut prior, ClaimedOutcome::Failed, vec![], "").is_err(), "失敗需要說明");
    // hypothesis 是 retry：失敗三次用盡預算 → 阻擋
    for n in 1..=3 {
        workflow = report(&workflow, &playbook, &mut prior, ClaimedOutcome::Failed, vec![], "仍然無法證實").unwrap();
        assert_eq!(workflow.status == WorkflowStatus::Blocked, n == 3, "第 {n} 次");
    }
    assert!(workflow.blocked_reason.as_deref().unwrap().contains("用盡預算"));
    // 缺證據的回報不解除預算用盡的阻擋，也不需要送裁定
    assert!(!needs_budget_decision(&workflow, &playbook, &prior, &StageResultInput { evidence: vec![], ..full_input(&workflow, &playbook) }).unwrap());
    workflow = report(&workflow, &playbook, &mut prior, ClaimedOutcome::Passed, vec![], "").unwrap();
    assert_eq!(workflow.status, WorkflowStatus::Blocked);
    // 證據齊全也不能由 Controller 自行解除：預算用盡後的下一步必須經使用者裁定（review-notes #20，spec D-15）
    assert!(needs_budget_decision(&workflow, &playbook, &prior, &full_input(&workflow, &playbook)).unwrap());
    for decision in [
        BudgetDecision::NotRequested,
        BudgetDecision::Pending { request_id: "dr_budget".into() },
        BudgetDecision::Denied { request_id: "dr_budget".into() },
        BudgetDecision::Unavailable { reason: "Workflow 沒有綁定 Task".into() },
    ] {
        let still = pass_with(&workflow, &playbook, &mut prior, decision.clone());
        assert_eq!((still.status, still.current_stage.as_deref()), (WorkflowStatus::Blocked, Some("hypothesis")), "{decision:?}");
        assert!(still.blocked_reason.as_deref().unwrap().contains("預算用盡後的下一步"), "{decision:?}");
        assert_eq!(prior.last().unwrap().outcome, AttemptOutcome::Blocked);
        workflow = still;
    }
    assert!(workflow.blocked_reason.as_deref().unwrap().contains("Workflow 沒有綁定 Task"));
    // 使用者核准之後，有證據的通過才繼續
    workflow = pass_with(&workflow, &playbook, &mut prior, BudgetDecision::Approved);
    assert_eq!((workflow.status, workflow.current_stage.as_deref()), (WorkflowStatus::Active, Some("probe")));
    // 預算沒有用盡的 Stage 不需要裁定
    assert!(!needs_budget_decision(&workflow, &playbook, &prior, &full_input(&workflow, &playbook)).unwrap());
}

#[test]
fn a_reported_block_needs_a_reason_and_can_be_cleared_by_evidence() {
    let (workflow, playbook) = started("feature");
    let mut prior = Vec::new();
    let blocked = report(&workflow, &playbook, &mut prior, ClaimedOutcome::Blocked, vec![], "沒有獨立 reviewer").unwrap();
    assert_eq!(blocked.status, WorkflowStatus::Blocked);
    assert!(blocked.blocked_reason.as_deref().unwrap().contains("沒有獨立 reviewer"));
    assert!(report(&blocked, &playbook, &mut prior, ClaimedOutcome::Blocked, vec![], " ").is_err(), "阻擋需要說明");
    let resumed = pass(&blocked, &playbook, &mut prior);
    assert_eq!((resumed.status, resumed.blocked_reason, resumed.current_stage.as_deref()), (WorkflowStatus::Active, None, Some("design")));
}

#[test]
fn skipping_needs_a_reason_and_never_bypasses_decision_or_safety_gates() {
    let (workflow, playbook) = started("feature");
    assert_eq!(
        skip_stage(&workflow, &playbook, &[], "requirements_contract", 1, "  ", "s".into(), T0).unwrap_err().code(),
        "workflow_invalid"
    );
    let change = skip_stage(&workflow, &playbook, &[], "requirements_contract", 1, "需求已在 issue 中寫明", "s".into(), T0).unwrap();
    let attempt = change.attempt.unwrap();
    assert_eq!((attempt.outcome, attempt.note.as_str(), attempt.recorded_by.as_str()), (AttemptOutcome::Skipped, "需求已在 issue 中寫明", "desktop"));
    assert_eq!(change.workflow.current_stage.as_deref(), Some("design"));

    let mut prior = Vec::new();
    let mut workflow = workflow;
    while workflow.current_stage.as_deref() != Some("real_interface_verification") {
        workflow = pass(&workflow, &playbook, &mut prior);
    }
    let error = skip_stage(&workflow, &playbook, &prior, "real_interface_verification", workflow.revision, "趕時間", "s".into(), T0);
    assert_eq!(error.unwrap_err().code(), "stage_not_skippable");

    let (mut ambiguous, playbook) = started("ambiguous");
    let mut prior = Vec::new();
    for _ in 0..3 {
        ambiguous = pass(&ambiguous, &playbook, &mut prior);
    }
    let error = skip_stage(&ambiguous, &playbook, &prior, "decision", ambiguous.revision, "我已經口頭同意", "s".into(), T0);
    assert_eq!(error.unwrap_err().code(), "stage_not_skippable");
}

#[test]
fn the_last_stage_completes_the_workflow_and_stage_states_follow_the_attempts() {
    let (mut workflow, playbook) = started("investigation");
    let mut prior = Vec::new();
    workflow = pass(&workflow, &playbook, &mut prior);
    let states = stage_states(&workflow, &playbook, &prior);
    assert_eq!(states, vec![StageState::Passed, StageState::Current, StageState::Pending, StageState::Pending]);
    while workflow.current_stage.is_some() {
        workflow = pass(&workflow, &playbook, &mut prior);
    }
    assert_eq!(workflow.status, WorkflowStatus::Completed);
    assert!(stage_states(&workflow, &playbook, &prior).iter().all(|s| *s == StageState::Passed));
    let input = StageResultInput { stage: "conclusion".into(), expected_revision: workflow.revision, outcome: ClaimedOutcome::Passed, evidence: vec![], note: String::new(), build: None, basis_overturned: None, budget_decision: BudgetDecision::default() };
    assert_eq!(record_result(&workflow, &playbook, &prior, input, "a".into(), recorder(), T0).unwrap_err().code(), "workflow_completed");
}

#[test]
fn a_task_runs_at_most_one_open_workflow() {
    let (open, playbook) = started("feature");
    let route = route(&catalog(), Situation::Feature, None, "理由").unwrap();
    let input = StartInput { workspace_id: "ws".into(), goal_id: "goal".into(), task_id: Some("task".into()), route, created_by: "c".into() };
    assert_eq!(start("wf2".into(), input, &playbook, Some(&open), T0).unwrap_err().code(), "workflow_exists");
}

#[test]
fn notes_for_the_controller_and_the_worker_name_the_current_stage() {
    let (mut workflow, playbook) = started("defect");
    let mut prior = Vec::new();
    workflow = pass(&workflow, &playbook, &mut prior);
    let note = WorkflowNote::from_parts(&workflow, &playbook, &prior, Some("修正匯出錯誤".into())).unwrap().render();
    assert!(note.contains("目前 Stage「最小化」") && note.contains("已完成：建立重現") && note.contains("修正匯出錯誤"), "{note}");
    let worker = worker_stage_note(&workflow, &playbook).unwrap();
    assert!(worker.contains("Stage「最小化」（minimization）") && worker.contains("不要重做"), "{worker}");
}

// --- 票 44：雙軸審查作為 Gate 的證據 ---

fn review(id: &str, status: EvidenceStatus, standards: &str, spec: &str) -> CheckedEvidence {
    CheckedEvidence {
        evidence: Evidence { check: "review".into(), reference: EvidenceRef::Review { review_id: id.into() } },
        status,
        reviewers: Some(AxisReviewers { standards: standards.into(), spec: spec.into() }),
    }
}

/// 前進到功能開發的「獨立審查」。
fn at_review(playbook: &PlaybookVersion) -> (WorkflowInstance, Vec<StageAttempt>) {
    let route = route(&catalog(), Situation::Feature, Some(TaskKind::Development), "測試").unwrap();
    let input = StartInput { workspace_id: "ws".into(), goal_id: "goal".into(), task_id: Some("task".into()), route, created_by: "c".into() };
    let mut workflow = start("wf".into(), input, playbook, None, T0).unwrap().workflow;
    let mut prior = Vec::new();
    while workflow.current_stage.as_deref() != Some("independent_review") {
        workflow = pass(&workflow, playbook, &mut prior);
    }
    (workflow, prior)
}

#[test]
fn the_independent_review_gate_needs_a_passed_two_axis_review_not_an_artifact() {
    let playbook = playbook("feature");
    let stage = playbook.definition.stage("independent_review").unwrap();
    assert_eq!((stage.gate, stage.exit_checks[0].requires), (GateKind::Safety, EvidenceKind::Review));
    let (workflow, mut prior) = at_review(&playbook);
    let artifact = CheckedEvidence {
        evidence: Evidence { check: "review".into(), reference: EvidenceRef::Artifact { artifact_id: "art".into() } },
        status: EvidenceStatus::Valid,
        reviewers: None,
    };
    let error = report(&workflow, &playbook, &mut prior, ClaimedOutcome::Passed, vec![artifact], "作者說已審過").unwrap_err();
    assert_eq!(error.code(), "workflow_invalid", "作者的工作摘要不是審查證據");

    // 沒有任何非作者的審查：Gate 保持關閉並記為 blocked
    let blocked = report(&workflow, &playbook, &mut prior, ClaimedOutcome::Passed, vec![], "").unwrap();
    assert_eq!((blocked.status, prior.last().unwrap().outcome), (WorkflowStatus::Blocked, AttemptOutcome::Blocked));
    assert!(blocked.blocked_reason.as_deref().unwrap().contains("非作者"), "{:?}", blocked.blocked_reason);

    // 審查有未解決的 finding 或已過期：incomplete，不前進
    for status in [EvidenceStatus::NotPassed, EvidenceStatus::Stale, EvidenceStatus::OtherSubject] {
        let after = report(&blocked, &playbook, &mut prior, ClaimedOutcome::Passed, vec![review("rev", status, "r1", "r1")], "").unwrap();
        assert_eq!((after.current_stage.as_deref(), prior.last().unwrap().outcome), (Some("independent_review"), AttemptOutcome::Incomplete), "{status:?}");
    }
    let passed = report(&blocked, &playbook, &mut prior, ClaimedOutcome::Passed, vec![review("rev", EvidenceStatus::Valid, "r1", "r1")], "").unwrap();
    assert_eq!(passed.current_stage.as_deref(), Some("integration"));
}

#[test]
fn the_gate_stays_closed_until_each_axis_has_the_required_number_of_distinct_reviewers() {
    let keys = method_keys();
    let keys: Vec<&str> = keys.iter().map(String::as_str).collect();
    let v1 = playbook("feature");
    let mut definition = v1.definition.clone();
    let stage = definition.stages.iter_mut().find(|s| s.key == "independent_review").unwrap();
    stage.exit_checks[0].reviewers = Some(2);
    let (v2, _) = publish_playbook(Some(&v1), definition, &keys, Origin::User, "pbv2".into(), T0).unwrap().unwrap();
    let (workflow, mut prior) = at_review(&v2);

    let one = report(&workflow, &v2, &mut prior, ClaimedOutcome::Passed, vec![review("a", EvidenceStatus::Valid, "r1", "r1")], "").unwrap();
    assert_eq!(one.status, WorkflowStatus::Blocked);
    assert!(prior.last().unwrap().missing[0].reason.contains("2 位"), "{:?}", prior.last().unwrap().missing);
    // 同一位 reviewer 交兩份不算兩位
    let same = vec![review("a", EvidenceStatus::Valid, "r1", "r1"), review("b", EvidenceStatus::Valid, "r1", "r1")];
    assert_eq!(report(&one, &v2, &mut prior, ClaimedOutcome::Passed, same, "").unwrap().status, WorkflowStatus::Blocked);
    // 第二位只沿用了第一位的需求軸：需求軸仍只有一位
    let reused = vec![review("a", EvidenceStatus::Valid, "r1", "r1"), review("b", EvidenceStatus::Valid, "r2", "r1")];
    assert_eq!(report(&one, &v2, &mut prior, ClaimedOutcome::Passed, reused, "").unwrap().status, WorkflowStatus::Blocked);
    let two = vec![review("a", EvidenceStatus::Valid, "r1", "r1"), review("b", EvidenceStatus::Valid, "r2", "r2")];
    assert_eq!(report(&one, &v2, &mut prior, ClaimedOutcome::Passed, two, "").unwrap().current_stage.as_deref(), Some("integration"));
}

#[test]
fn reviewer_counts_are_only_for_review_checks_and_within_range() {
    let keys = method_keys();
    let keys: Vec<&str> = keys.iter().map(String::as_str).collect();
    let base = playbook("feature").definition;
    let mut on_artifact = base.clone();
    on_artifact.stages[0].exit_checks[0].reviewers = Some(1);
    let mut zero = base.clone();
    zero.stages.iter_mut().find(|s| s.key == "independent_review").unwrap().exit_checks[0].reviewers = Some(0);
    for definition in [on_artifact, zero] {
        assert_eq!(validate_playbook(&definition, &keys).unwrap_err().code(), "playbook_invalid");
    }
    let evidence = normalize_evidence(&[EvidenceInput { check: "review".into(), review_id: Some("rev_1".into()), ..Default::default() }]).unwrap();
    assert_eq!(evidence[0].reference, EvidenceRef::Review { review_id: "rev_1".into() });
    let both = EvidenceInput { check: "review".into(), review_id: Some("rev_1".into()), artifact_id: Some("art".into()), ..Default::default() };
    assert_eq!(normalize_evidence(&[both]).unwrap_err().code(), "workflow_invalid");
}

#[path = "gate_tests.rs"]
mod gate;
