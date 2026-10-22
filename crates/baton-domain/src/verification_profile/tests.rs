use super::*;

fn http(method: &str, url: &str) -> Operation {
    Operation::Http { method: method.into(), url: url.into(), headers: Vec::new(), body: None, timeout_ms: None }
}

fn command(program: &str) -> Operation {
    Operation::Command { program: program.into(), args: Vec::new(), timeout_ms: None }
}

fn step(key: &str, operation: Operation, assertions: Vec<Assertion>) -> Step {
    Step { key: key.into(), title: format!("步驟 {key}"), operation, assertions }
}

fn journey(key: &str, representative: bool, steps: Vec<Step>) -> Journey {
    Journey { key: key.into(), title: format!("旅程 {key}"), representative, persona: None, identity: None, steps }
}

fn definition() -> ProfileDefinition {
    ProfileDefinition {
        interface: Interface::CliApi,
        build: None,
        doctor: vec![step("tool", command("ledger"), vec![Assertion::ExitCode { equals: 0 }])],
        start: Some(ServiceSpec { program: "ledger".into(), args: vec!["serve".into()] }),
        readiness: Some(Readiness {
            step: step("health", http("GET", "http://127.0.0.1:8080/health"), vec![Assertion::Status { equals: 200 }]),
            attempts: 3,
            interval_ms: 500,
        }),
        setup: Vec::new(),
        journeys: vec![
            journey(
                "create-item",
                true,
                vec![
                    step("create", http("POST", "http://127.0.0.1:8080/items"), vec![Assertion::Status { equals: 201 }]),
                    step(
                        "read-back",
                        http("GET", "http://127.0.0.1:8080/items/1"),
                        vec![Assertion::JsonEquals { pointer: "/name".into(), value: serde_json::json!("午餐") }],
                    ),
                ],
            ),
            journey("visual", false, vec![step("look", Operation::Manual { instruction: "確認版面".into() }, Vec::new())]),
        ],
        teardown: Vec::new(),
        evidence: EvidencePolicy::default(),
        budget: VerificationBudget::default(),
        feature_map: vec![
            Feature { key: "items".into(), title: "記帳項目".into(), criterion: "新增後可讀回".into(), journeys: vec!["create-item".into()] },
            Feature { key: "export".into(), title: "匯出".into(), criterion: "可匯出 CSV".into(), journeys: Vec::new() },
        ],
        uncovered: vec![UncoveredArea { area: "效能".into(), reason: "沒有基準資料".into() }],
        secrets: Vec::new(),
        personas: Vec::new(),
        identities: Vec::new(),
    }
}

fn record(phase: Phase, journey: Option<&str>, key: &str, verdict: Verdict) -> StepRecord {
    StepRecord {
        phase,
        journey: journey.map(str::to_string),
        key: key.into(),
        title: String::new(),
        operation: String::new(),
        verdict,
        reason_code: verdict.as_str().into(),
        engine_code: None,
        assertions: Vec::new(),
        output: String::new(),
        attempts: 1,
    }
}

#[test]
fn a_profile_needs_a_representative_journey_and_assertions_on_every_automated_step() {
    assert_eq!(validate(&definition()), Ok(()));

    let mut none_representative = definition();
    none_representative.journeys[0].representative = false;
    assert!(matches!(validate(&none_representative), Err(ProfileError::Invalid(m)) if m.contains("代表旅程")));

    let mut no_assertion = definition();
    no_assertion.journeys[0].steps[0].assertions.clear();
    assert!(matches!(validate(&no_assertion), Err(ProfileError::Invalid(m)) if m.contains("程式斷言")));

    let mut wrong_assertion = definition();
    wrong_assertion.journeys[0].steps[0].assertions = vec![Assertion::ExitCode { equals: 0 }];
    assert!(matches!(validate(&wrong_assertion), Err(ProfileError::Invalid(m)) if m.contains("結束碼")));

    let mut unknown_journey = definition();
    unknown_journey.feature_map[1].journeys = vec!["nope".into()];
    assert!(matches!(validate(&unknown_journey), Err(ProfileError::Invalid(m)) if m.contains("nope")));

    let mut manual_representative = definition();
    manual_representative.journeys[1].representative = true;
    manual_representative.journeys[0].representative = false;
    assert!(matches!(validate(&manual_representative), Err(ProfileError::Invalid(m)) if m.contains("可自動化")));

    let mut empty_map = definition();
    empty_map.feature_map.clear();
    assert!(matches!(validate(&empty_map), Err(ProfileError::Invalid(m)) if m.contains("Feature Map")));
}

#[test]
fn publishing_identical_content_adds_no_version_and_changes_add_one() {
    let now = Timestamp::from_millis(1);
    let (first, event) = publish(None, "ws", definition(), "desk".into(), "vp_1".into(), now).unwrap().unwrap();
    assert_eq!(first.version, 1);
    assert_eq!(event.event_type, EVENT_PROFILE_PUBLISHED);
    assert_eq!(publish(Some(&first), "ws", definition(), "desk".into(), "vp_2".into(), now), Ok(None));
    let mut changed = definition();
    changed.journeys[0].title = "新增並讀回".into();
    let (second, event) = publish(Some(&first), "ws", changed, "desk".into(), "vp_2".into(), now).unwrap().unwrap();
    assert_eq!((second.version, event.aggregate_version), (2, 2));
}

#[test]
fn uncovered_scope_lists_features_without_journeys_declared_areas_and_journeys_not_run() {
    let def = definition();
    let profile_level: Vec<String> = uncovered(&def).into_iter().map(|u| u.area).collect();
    assert_eq!(profile_level, vec!["匯出".to_string(), "效能".to_string()]);
    let run_level: Vec<String> = uncovered_by_run(&def, &["visual".into()]).into_iter().map(|u| u.area).collect();
    assert_eq!(run_level, vec!["匯出".to_string(), "效能".to_string(), "記帳項目".to_string()]);
    assert_eq!(covered_features(&def, &["create-item".into()]), vec!["items".to_string()]);
}

#[test]
fn an_unproven_profile_always_runs_its_representative_journey() {
    let def = definition();
    assert_eq!(plan_journeys(&def, &["visual".into()], false).unwrap(), vec!["create-item".to_string(), "visual".to_string()]);
    assert_eq!(plan_journeys(&def, &["visual".into()], true).unwrap(), vec!["visual".to_string()]);
    assert_eq!(plan_journeys(&def, &[], true).unwrap().len(), 2);
    assert!(matches!(plan_journeys(&def, &["nope".into()], true), Err(ProfileError::RequestInvalid(_))));
}

fn judge_http(phase: Phase, s: &Step, status: u16, body: &str) -> StepRecord {
    judge_step(phase, Some("j"), s, &s.operation, Observation::Responded { status, body: body.into() }, 1, &EvidencePolicy::default())
}

#[test]
fn an_assertion_mismatch_in_a_journey_is_a_product_failure_but_elsewhere_an_environment_problem() {
    let def = definition();
    let read_back = &def.journeys[0].steps[1];
    let failed = judge_http(Phase::Journey, read_back, 200, r#"{"name":"晚餐"}"#);
    assert_eq!((failed.verdict, failed.reason_code.as_str()), (Verdict::ProductFailure, "assertion_failed"));
    assert_eq!(failed.engine_code.as_deref(), Some("http:200"));
    assert_eq!(failed.assertions[0].actual, "\"晚餐\"");
    let passed = judge_http(Phase::Journey, read_back, 200, r#"{"name":"午餐"}"#);
    assert_eq!(passed.verdict, Verdict::Passed);
    let health = &def.readiness.as_ref().unwrap().step;
    let not_ready = judge_http(Phase::Readiness, health, 503, "");
    assert_eq!((not_ready.verdict, not_ready.reason_code.as_str()), (Verdict::EnvironmentBlocked, "not_ready"));
    let doctor = &def.doctor[0];
    let missing = judge_step(
        Phase::Doctor,
        None,
        doctor,
        &doctor.operation,
        Observation::Exited { code: 1, output: "缺少設定".into() },
        1,
        &EvidencePolicy::default(),
    );
    assert_eq!((missing.verdict, missing.reason_code.as_str()), (Verdict::EnvironmentBlocked, "doctor_failed"));
}

#[test]
fn unauthorized_responses_are_auth_blocked_unless_the_step_expects_them() {
    let def = definition();
    let create = &def.journeys[0].steps[0];
    let blocked = judge_http(Phase::Journey, create, 401, "");
    assert_eq!((blocked.verdict, blocked.reason_code.as_str()), (Verdict::AuthBlocked, "http_unauthorized"));
    let forbidden = judge_http(Phase::Journey, create, 403, "");
    assert_eq!(forbidden.verdict, Verdict::AuthBlocked);
    let expects = step("anonymous", http("GET", "http://x/admin"), vec![Assertion::Status { equals: 401 }]);
    assert_eq!(judge_http(Phase::Journey, &expects, 401, "").verdict, Verdict::Passed);
    assert_eq!(judge_http(Phase::Journey, &expects, 200, "").verdict, Verdict::ProductFailure);
}

#[test]
fn missing_evidence_and_broken_environments_are_never_passed_nor_product_failures() {
    let def = definition();
    let create = &def.journeys[0].steps[0];
    let policy = EvidencePolicy::default();
    let cases = [
        (Observation::TimedOut { output: String::new() }, Verdict::Inconclusive, "operation_timed_out"),
        (Observation::Terminated { output: String::new() }, Verdict::Inconclusive, "operation_terminated"),
        (Observation::Unreachable { reason: "連線被拒".into() }, Verdict::EnvironmentBlocked, "endpoint_unreachable"),
        (Observation::NotStarted { reason: "找不到程式".into() }, Verdict::EnvironmentBlocked, "operation_not_started"),
        (Observation::Unsupported { reason: "人工".into() }, Verdict::AutomationUnsupported, "manual_step"),
    ];
    for (observation, verdict, reason) in cases {
        let judged = judge_step(Phase::Journey, Some("j"), create, &create.operation, observation, 1, &policy);
        assert_eq!((judged.verdict, judged.reason_code.as_str()), (verdict, reason));
        assert!(judged.assertions.is_empty(), "沒有觀察就沒有斷言結果");
    }
}

#[test]
fn a_journey_passes_only_when_every_step_ran_and_passed() {
    let j = journey("j", true, vec![step("a", command("x"), vec![]), step("b", command("x"), vec![])]);
    let all = [record(Phase::Journey, Some("j"), "a", Verdict::Passed), record(Phase::Journey, Some("j"), "b", Verdict::Passed)];
    assert_eq!(judge_journey(&j, &all).verdict, Verdict::Passed);
    // 只有一半的步驟：不能拼接為通過
    assert_eq!(judge_journey(&j, &all[..1]).verdict, Verdict::Inconclusive);
    let second_failed =
        [record(Phase::Journey, Some("j"), "a", Verdict::Passed), record(Phase::Journey, Some("j"), "b", Verdict::BudgetExhausted)];
    assert_eq!(judge_journey(&j, &second_failed).verdict, Verdict::BudgetExhausted);
}

#[test]
fn the_overall_verdict_puts_product_failures_first_then_blocked_prerequisites() {
    let passed = JourneyResult { key: "a".into(), title: String::new(), representative: true, persona: None, identity: None, verdict: Verdict::Passed, reason_code: String::new() };
    let failed = JourneyResult { verdict: Verdict::ProductFailure, reason_code: "assertion_failed".into(), ..passed.clone() };
    let teardown_failed = record(Phase::Teardown, None, "t", Verdict::EnvironmentBlocked);
    assert_eq!(aggregate(&[teardown_failed.clone()], &[passed.clone()]).0, Verdict::Passed);
    assert_eq!(aggregate(&[], &[]).0, Verdict::Inconclusive);
    let doctor = record(Phase::Doctor, None, "d", Verdict::EnvironmentBlocked);
    assert_eq!(aggregate(&[doctor.clone()], &[]).0, Verdict::EnvironmentBlocked);
    assert_eq!(aggregate(&[doctor], &[passed.clone(), failed]).0, Verdict::ProductFailure);
    let auth = JourneyResult { verdict: Verdict::AuthBlocked, ..passed.clone() };
    assert_eq!(aggregate(&[], &[passed, auth]).0, Verdict::AuthBlocked);
}

#[test]
fn evidence_is_redacted_truncated_and_placeholders_are_bound_to_this_run() {
    let policy = EvidencePolicy { output_max_chars: 8, redact: vec!["s3cret".into()], ..EvidencePolicy::default() };
    assert_eq!(sanitize("token=s3cret", &policy), "…en=[已遮蔽]");
    let bound = bind_operation(
        &Operation::Command { program: "ledger".into(), args: vec!["--db".into(), "{{state_dir}}/db".into()], timeout_ms: None },
        "vreq_1",
        "D:/state/vreq_1",
    );
    assert_eq!(bound.describe(), "執行「ledger --db D:/state/vreq_1/db」");
    let bound = bind_operation(&http("GET", "http://x/runs/{{run_id}}"), "vreq_1", "s");
    assert_eq!(bound.describe(), "GET http://x/runs/vreq_1");
}

#[test]
fn a_profile_version_is_proven_only_by_a_passed_representative_journey_of_that_version() {
    let (profile, _) = publish(None, "ws", definition(), "desk".into(), "vp_1".into(), Timestamp::from_millis(1)).unwrap().unwrap();
    let journey_result = |verdict| JourneyResult {
        key: "create-item".into(),
        title: String::new(),
        representative: true,
        persona: None,
        identity: None,
        verdict,
        reason_code: String::new(),
    };
    let bundle = |id: &str, version_id: &str, verdict| EvidenceBundle {
        id: id.into(),
        workspace_id: "ws".into(),
        request_id: "r".into(),
        task_id: None,
        run_id: None,
        generation: None,
        requirement_version: None,
        profile_version_id: version_id.into(),
        profile_version: 1,
        candidate_revision: None,
        build: None,
        subject_dir: String::new(),
        journeys: vec![journey_result(verdict)],
        steps: Vec::new(),
        verdict,
        reason_code: String::new(),
        covered_features: Vec::new(),
        uncovered: Vec::new(),
        cleanup: CleanupRecord { state_dir: String::new(), state_dir_removed: true, service_stopped: None, problems: Vec::new() },
        artifacts: Vec::new(),
        started_at: Timestamp::from_millis(1),
        finished_at: Timestamp::from_millis(2),
    };
    let blocked = bundle("b1", "vp_1", Verdict::EnvironmentBlocked);
    let other_version = bundle("b2", "vp_0", Verdict::Passed);
    assert!(proven_by(&profile, &[blocked.clone(), other_version.clone()]).is_none());
    let passed = bundle("b3", "vp_1", Verdict::Passed);
    assert_eq!(proven_by(&profile, &[blocked, other_version, passed]).map(|b| b.id.as_str()), Some("b3"));
}

// ---------------------------------------------------------------- 票 46：web

fn browser(action: BrowserAction) -> Operation {
    Operation::Browser { action, timeout_ms: None }
}

fn goto(url: &str) -> Operation {
    browser(BrowserAction::Goto { url: url.into() })
}

fn web_definition() -> ProfileDefinition {
    let mut definition = definition();
    definition.interface = Interface::Web;
    definition.secrets = vec![SecretRef { key: "clerk-password".into(), reference: "env:SHOP_CLERK_PASSWORD".into() }];
    definition.personas = vec![Persona { key: "admin".into(), title: "店家管理員".into(), description: "管理商品".into() }];
    definition.identities = vec![Identity {
        key: "clerk".into(),
        title: "店員帳號".into(),
        login: vec![step(
            "password",
            browser(BrowserAction::Fill { selector: "#password".into(), text: "{{secret:clerk-password}}".into() }),
            vec![Assertion::Visible { selector: "#password".into() }],
        )],
        check: step("whoami", goto("http://127.0.0.1:3000/me"), vec![Assertion::Contains { text: "店員".into() }]),
    }];
    definition.journeys[0].persona = Some("admin".into());
    definition.journeys[0].identity = Some("clerk".into());
    definition
}

fn page(text: &str, status: Option<u16>, judgements: Vec<Judgement>) -> Observation {
    Observation::Page { url: "http://127.0.0.1:3000/items".into(), status, text: text.into(), visible: Vec::new(), judgements }
}

#[test]
fn web_profiles_declare_secrets_by_reference_and_bind_personas_and_identities_separately() {
    assert_eq!(validate(&web_definition()), Ok(()));

    // 瀏覽器操作只用於 web 介面
    let mut cli = web_definition();
    cli.interface = Interface::CliApi;
    assert!(validate(&cli).is_err());
    // 秘密只保存參照
    let mut literal = web_definition();
    literal.secrets[0].reference = "hunter2".into();
    assert!(validate(&literal).is_err());
    // 引用沒有宣告的秘密
    let mut undeclared = web_definition();
    undeclared.secrets.clear();
    assert!(validate(&undeclared).unwrap_err().to_string().contains("clerk-password"));
    // 旅程引用的 persona 與登入身分要分別存在：名為 admin 的 persona 不會變成登入身分
    let mut persona_as_identity = web_definition();
    persona_as_identity.journeys[0].identity = Some("admin".into());
    assert!(validate(&persona_as_identity).unwrap_err().to_string().contains("登入身分「admin」不存在"));
    // 身分核對必須可以自動執行
    let mut manual_check = web_definition();
    manual_check.identities[0].check = step("whoami", Operation::Manual { instruction: "看右上角".into() }, Vec::new());
    assert!(validate(&manual_check).is_err());
    // 畫面斷言只用於瀏覽器步驟；瀏覽器步驟不能斷言結束碼
    let mut screen_on_http = web_definition();
    screen_on_http.journeys[0].steps[0].assertions.push(Assertion::Visible { selector: "#x".into() });
    assert!(validate(&screen_on_http).is_err());
    let mut exit_on_browser = web_definition();
    exit_on_browser.identities[0].check.assertions.push(Assertion::ExitCode { equals: 0 });
    assert!(validate(&exit_on_browser).is_err());
}

#[test]
fn the_serialized_cli_profile_does_not_change_when_web_fields_are_empty() {
    let json = serde_json::to_value(definition()).unwrap();
    for field in ["secrets", "personas", "identities"] {
        assert!(json.get(field).is_none(), "{field} 不應出現在 CLI Profile 的序列化中");
    }
    assert!(json["evidence"].get("screenshots").is_none() && json["budget"].get("max_model_calls").is_none());
}

#[test]
fn secrets_are_revealed_only_for_the_runner_and_never_in_the_description() {
    let fill = browser(BrowserAction::Fill { selector: "#password".into(), text: "{{secret:clerk-password}}".into() });
    assert_eq!(fill.secret_keys(), ["clerk-password"]);
    assert_eq!(fill.filled_selector(), Some("#password"));
    let revealed = reveal_secrets(&fill, &[("clerk-password".into(), "hunter2".into())]);
    assert_eq!(revealed, browser(BrowserAction::Fill { selector: "#password".into(), text: "hunter2".into() }));
    assert!(fill.describe().contains("{{secret:clerk-password}}"));
    assert_eq!(secret_keys_in("a {{secret:x}} b {{secret:y}}"), ["x", "y"]);
    // 空的秘密值不會把整段文字遮掉
    let policy = EvidencePolicy { redact: vec![String::new(), "hunter2".into()], ..EvidencePolicy::default() };
    assert_eq!(sanitize("pw=hunter2", &policy), "pw=[已遮蔽]");
}

#[test]
fn a_rate_limited_or_unavailable_judge_is_inconclusive_not_a_product_failure() {
    let step = step(
        "list",
        goto("http://127.0.0.1:3000/items"),
        vec![Assertion::Contains { text: "商品".into() }, Assertion::Judge { expectation: "清單依價格排序".into() }],
    );
    let policy = EvidencePolicy::default();
    let judge = |observation| judge_step(Phase::Journey, Some("j"), &step, &step.operation, observation, 1, &policy);

    let limited = judge(page("商品 A 100 元", Some(200), vec![Judgement::RateLimited]));
    assert_eq!((limited.verdict, limited.reason_code.as_str()), (Verdict::Inconclusive, "judge_rate_limited"));
    assert_eq!(limited.engine_code.as_deref(), Some("model:rate_limited"));
    let unavailable = judge(page("商品", Some(200), vec![Judgement::Unavailable { reason: "沒有可用帳號".into() }]));
    assert_eq!((unavailable.verdict, unavailable.reason_code.as_str()), (Verdict::Inconclusive, "judge_unavailable"));
    let exhausted = judge(page("商品", Some(200), vec![Judgement::BudgetExhausted]));
    assert_eq!(exhausted.verdict, Verdict::BudgetExhausted);

    // 程式斷言明確不符時，即使 judge 限流，仍是產品缺陷
    let missing = judge(page("空的", Some(200), vec![Judgement::RateLimited]));
    assert_eq!((missing.verdict, missing.reason_code.as_str()), (Verdict::ProductFailure, "assertion_failed"));
    // judge 明確判定不符是產品缺陷；符合才通過
    let wrong = judge(page("商品", Some(200), vec![Judgement::Decided { satisfied: false, rationale: "沒有排序".into() }]));
    assert_eq!(wrong.verdict, Verdict::ProductFailure);
    assert_eq!(wrong.assertions[1].actual, "沒有排序");
    let right = judge(page("商品", Some(200), vec![Judgement::Decided { satisfied: true, rationale: "已排序".into() }]));
    assert_eq!(right.verdict, Verdict::Passed);
    // 被拒絕的導覽是授權阻礙
    let denied = judge(page("請先登入", Some(401), vec![Judgement::RateLimited]));
    assert_eq!(denied.verdict, Verdict::AuthBlocked);
    // 操作本身沒有完成：證據不足
    let failed = judge(Observation::ActionFailed { error: "TimeoutError".into(), message: "找不到 #buy".into() });
    assert_eq!((failed.verdict, failed.engine_code.as_deref()), (Verdict::Inconclusive, Some("playwright:TimeoutError")));
}

#[test]
fn login_and_identity_problems_are_auth_blocked_and_belong_to_the_journey() {
    let check = step("whoami", goto("http://127.0.0.1:3000/me"), vec![Assertion::Contains { text: "管理員".into() }]);
    let policy = EvidencePolicy::default();
    let record = judge_step(Phase::Identity, Some("j"), &check, &check.operation, page("一般使用者", Some(200), Vec::new()), 1, &policy);
    assert_eq!((record.verdict, record.reason_code.as_str()), (Verdict::AuthBlocked, "identity_not_confirmed"));

    let j = Journey { persona: Some("admin".into()), identity: Some("clerk".into()), ..journey("j", true, vec![check.clone()]) };
    let skipped = not_run(Phase::Journey, Some("j"), &check, Verdict::Inconclusive, "blocked_by_identity", &policy);
    let result = judge_journey(&j, &[record.clone(), skipped.clone()]);
    assert_eq!((result.verdict, result.reason_code.as_str()), (Verdict::AuthBlocked, "identity_not_confirmed"));
    assert_eq!((result.persona.as_deref(), result.identity.as_deref()), (Some("admin"), Some("clerk")));
    // 旅程內的登入問題不算前置階段，由旅程決定整體判定
    assert_eq!(aggregate(&[record, skipped], &[result]).1, "identity_not_confirmed");
}

#[test]
fn artifacts_that_may_contain_secrets_are_withheld() {
    let secrets = vec!["hunter2".to_string()];
    assert_eq!(withhold_reason(EvidenceArtifactKind::Screenshot, b"PNG...", &secrets, true), None);
    assert_eq!(withhold_reason(EvidenceArtifactKind::Screenshot, b"pw=hunter2", &secrets, false), Some("secret_detected"));
    assert_eq!(withhold_reason(EvidenceArtifactKind::Trace, b"zip", &secrets, true), Some("secret_input_recorded"));
    assert_eq!(withhold_reason(EvidenceArtifactKind::Video, b"webm", &secrets, false), None);
}
