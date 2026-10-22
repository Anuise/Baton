use std::collections::BTreeMap;

use super::*;
use crate::verification_profile::{
    judge_step, validate, AssertionResult, EvidencePolicy, Feature, Identity, Interface, Judgement, Observation, Operation,
    Persona, Phase, ReplayPolicy, SecretRef, VerificationBudget,
};

const ENGINE: &str = "playwright 1.62.1 chromium 140";

fn browser(action: BrowserAction) -> Operation {
    Operation::Browser { action, timeout_ms: None }
}

fn act(goal: &str) -> Operation {
    let params = BTreeMap::from([("item".to_string(), "紅色雨傘".to_string())]);
    browser(BrowserAction::Act { goal: goal.into(), params, replay_in_production: false })
}

fn step(key: &str, operation: Operation, assertions: Vec<Assertion>) -> Step {
    Step { key: key.into(), title: format!("步驟 {key}"), operation, assertions, external: false }
}

fn goto(url: &str) -> Operation {
    browser(BrowserAction::Goto { url: url.into() })
}

fn definition() -> ProfileDefinition {
    ProfileDefinition {
        interface: Interface::Web,
        build: None,
        doctor: Vec::new(),
        start: None,
        readiness: None,
        setup: vec![step(
            "seed",
            Operation::Http { method: "POST".into(), url: "http://127.0.0.1:3000/api/seed".into(), headers: Vec::new(), body: None, timeout_ms: None },
            vec![Assertion::Status { equals: 200 }],
        )],
        journeys: vec![Journey {
            key: "checkout".into(),
            title: "結帳".into(),
            representative: true,
            persona: Some("shopper".into()),
            identity: Some("member".into()),
            steps: vec![
                step("open", goto("http://127.0.0.1:3000/products"), vec![Assertion::Contains { text: "商品".into() }]),
                step("add", act("把指定商品加入購物車"), vec![Assertion::Contains { text: "購物車（1）".into() }]),
            ],
        }],
        teardown: Vec::new(),
        evidence: EvidencePolicy::default(),
        budget: VerificationBudget::default(),
        feature_map: vec![Feature { key: "cart".into(), title: "購物車".into(), criterion: "可加入商品".into(), journeys: vec!["checkout".into()] }],
        uncovered: Vec::new(),
        secrets: vec![SecretRef { key: "member-password".into(), reference: "env:SHOP_MEMBER_PASSWORD".into() }],
        personas: vec![Persona { key: "shopper".into(), title: "顧客".into(), description: "想買雨傘".into() }],
        identities: vec![Identity {
            key: "member".into(),
            title: "會員帳號".into(),
            login: vec![step(
                "password",
                browser(BrowserAction::Fill { selector: "#password".into(), text: "{{secret:member-password}}".into() }),
                vec![Assertion::Visible { selector: "#submit".into() }],
            )],
            check: step("whoami", goto("http://127.0.0.1:3000/me"), vec![Assertion::Contains { text: "會員".into() }]),
        }],
        replay: ReplayPolicy::default(),
    }
}

fn identity_of(definition: &ProfileDefinition, engine: &str) -> ReplayIdentity {
    let journey = &definition.journeys[0];
    identity(definition, journey, &journey.steps[1], engine).expect("是目標導向步驟")
}

fn entry(identity: ReplayIdentity, status: ReplayStatus) -> ReplayEntry {
    ReplayEntry {
        case_key: case_key("checkout", "add"),
        identity,
        action: BrowserAction::Click { selector: "role=button[name=\"加入購物車\"]".into() },
        stale_reasons: if status == ReplayStatus::Stale { vec!["postcondition_failed".into()] } else { Vec::new() },
        status,
        bundle_id: "evb_1".into(),
        recorded_at: Timestamp::from_millis(1),
    }
}

fn record(step: &Step, observation: Observation) -> StepRecord {
    judge_step(Phase::Journey, Some("checkout"), step, &step.operation, observation, 1, &EvidencePolicy::default())
}

fn page(text: &str, judgements: Vec<Judgement>) -> Observation {
    Observation::Page { url: "http://127.0.0.1:3000/cart".into(), status: Some(200), text: text.into(), visible: Vec::new(), judgements }
}

#[test]
fn act_steps_are_validated_as_journey_steps_with_a_postcondition_that_needs_no_model() {
    assert_eq!(validate(&definition()), Ok(()));

    // 只有語意判斷：重播時無法不呼叫模型重新檢查
    let mut judge_only = definition();
    judge_only.journeys[0].steps[1].assertions = vec![Assertion::Judge { expectation: "購物車有雨傘".into() }];
    assert!(validate(&judge_only).unwrap_err().to_string().contains("不靠模型的斷言"));
    // 目標與參數會交給模型，不能引用秘密
    let mut secret = definition();
    secret.journeys[0].steps[1].operation = act("以 {{secret:member-password}} 結帳");
    assert!(validate(&secret).unwrap_err().to_string().contains("不能引用秘密"));
    // 登入與身分核對必須是固定的操作
    let mut in_login = definition();
    in_login.identities[0].login.push(step("auto", act("登入"), vec![Assertion::Visible { selector: "#me".into() }]));
    assert!(validate(&in_login).unwrap_err().to_string().contains("只能用在旅程步驟"));
    // 目標不能空白、參數識別有格式
    let mut blank = definition();
    blank.journeys[0].steps[1].operation = act(" ");
    assert!(validate(&blank).is_err());
    let mut bad_param = definition();
    bad_param.journeys[0].steps[1].operation = browser(BrowserAction::Act {
        goal: "加入購物車".into(),
        params: BTreeMap::from([("Item Name".to_string(), "x".to_string())]),
        replay_in_production: false,
    });
    assert!(validate(&bad_param).is_err());
    // CLI／API 的 Profile 不能用
    let mut cli = definition();
    cli.interface = Interface::CliApi;
    cli.identities.clear();
    cli.journeys[0].identity = None;
    assert!(validate(&cli).is_err());
}

#[test]
fn the_identity_covers_case_goal_params_persona_engine_and_data() {
    let base = definition();
    let original = identity_of(&base, ENGINE);
    assert_eq!(identity_of(&base, ENGINE), original, "相同的內容得到相同的身分");
    assert_eq!(original.goal, "把指定商品加入購物車");

    let changed = |change: &dyn Fn(&mut ProfileDefinition), engine: &str| -> Vec<&'static str> {
        let mut definition = definition();
        change(&mut definition);
        original.differences(&identity_of(&definition, engine))
    };
    // 案例：之前的步驟或登入路徑改變
    assert_eq!(changed(&|d| d.journeys[0].steps[0].operation = goto("http://127.0.0.1:3000/sale"), ENGINE), ["case"]);
    assert_eq!(changed(&|d| d.identities[0].check.assertions = vec![Assertion::Contains { text: "VIP".into() }], ENGINE), ["case"]);
    // 目標與參數
    assert_eq!(changed(&|d| d.journeys[0].steps[1].operation = act("把商品加入願望清單"), ENGINE), ["goal"]);
    let other_params = browser(BrowserAction::Act {
        goal: "把指定商品加入購物車".into(),
        params: BTreeMap::from([("item".to_string(), "藍色雨傘".to_string())]),
        replay_in_production: false,
    });
    assert_eq!(changed(&|d| d.journeys[0].steps[1].operation = other_params.clone(), ENGINE), ["params"]);
    // persona（內容或綁定）與登入身分
    assert_eq!(changed(&|d| d.personas[0].description = "只看特價品".into(), ENGINE), ["persona"]);
    assert_eq!(changed(&|d| d.journeys[0].persona = None, ENGINE), ["persona"]);
    // 引擎
    assert_eq!(changed(&|_| {}, "playwright 1.63.0 chromium 141"), ["engine"]);
    // 資料相容性：資料集版本與資料準備
    assert_eq!(changed(&|d| d.replay.dataset = Some("2026-10".into()), ENGINE), ["data"]);
    assert_eq!(changed(&|d| d.setup[0].key = "seed-v2".into(), ENGINE), ["data"]);
    // 之後的步驟與後置條件不影響身分（重播時會重新檢查後置條件）
    assert_eq!(changed(&|d| d.journeys[0].steps[1].assertions.push(Assertion::Visible { selector: "#cart".into() }), ENGINE), Vec::<&str>::new());

    let journey = &base.journeys[0];
    assert_eq!(identity(&base, journey, &journey.steps[0], ENGINE), None, "固定的操作沒有錄製身分");
}

#[test]
fn only_a_valid_matching_recording_is_replayed_and_stale_ones_are_never_reused() {
    let identity = identity_of(&definition(), ENGINE);
    let valid = entry(identity.clone(), ReplayStatus::Valid);

    assert_eq!(decide(None, &identity, false, false), ReplayDecision::Live { cache: CacheState::None, stale: Vec::new() });
    assert_eq!(decide(Some(&valid), &identity, false, false), ReplayDecision::Replay(valid.action.clone()));
    // 先前已經 stale：不沿用舊的成功
    let stale = entry(identity.clone(), ReplayStatus::Stale);
    assert_eq!(decide(Some(&stale), &identity, false, false), ReplayDecision::Live { cache: CacheState::Stale, stale: Vec::new() });
    // 身分不一致：這次標記 stale，原因是不一致的維度
    let upgraded = identity_of(&definition(), "playwright 1.63.0");
    assert_eq!(
        decide(Some(&valid), &upgraded, false, false),
        ReplayDecision::Live { cache: CacheState::Mismatch, stale: vec!["engine_changed".into()] }
    );
    // 正式環境：預設不重播，個別允許才重播
    assert_eq!(
        decide(Some(&valid), &identity, true, false),
        ReplayDecision::Live { cache: CacheState::ProductionNotReplayable, stale: Vec::new() }
    );
    assert_eq!(decide(Some(&valid), &identity, true, true), ReplayDecision::Replay(valid.action.clone()));
}

#[test]
fn only_actions_proven_by_a_later_deterministic_assertion_are_recorded() {
    let base = definition();
    let add = &base.journeys[0].steps[1];
    assert!(proven(&record(add, page("購物車（1）", Vec::new()))));
    // 後置條件不成立
    assert!(!proven(&record(add, page("購物車（0）", Vec::new()))));
    // 動作無法完成
    assert!(!proven(&record(add, Observation::ActionFailed { error: "TimeoutError".into(), message: "等不到".into() })));

    // 只有語意判斷成立、程式斷言沒有成立的不算；語意判斷無法判定時步驟沒有通過，也不錄製
    let mut with_judge = add.clone();
    with_judge.assertions.push(Assertion::Judge { expectation: "購物車有紅色雨傘".into() });
    let decided = |satisfied| vec![Judgement::Decided { satisfied, rationale: "r".into() }];
    assert!(proven(&record(&with_judge, page("購物車（1）", decided(true)))));
    assert!(!proven(&record(&with_judge, page("購物車（1）", vec![Judgement::LiveModelDisabled]))));
    let judged_only = StepRecord {
        assertions: vec![AssertionResult { assertion: Assertion::Judge { expectation: "e".into() }, passed: true, actual: "r".into() }],
        ..record(&with_judge, page("購物車（1）", decided(true)))
    };
    assert!(!proven(&judged_only), "只有模型判斷不是有效的後續斷言");
}

#[test]
fn a_replay_is_disproven_only_by_its_own_action_or_deterministic_postconditions() {
    let base = definition();
    let add = &base.journeys[0].steps[1];
    assert!(!disproven(&record(add, page("購物車（1）", Vec::new()))));
    assert!(disproven(&record(add, page("購物車（0）", Vec::new()))));
    assert!(disproven(&record(add, Observation::ActionFailed { error: "TimeoutError".into(), message: "等不到".into() })));
    // 環境與授權問題不是錄製的錯
    assert!(!disproven(&record(add, Observation::Unreachable { reason: "refused".into() })));
    let denied = Observation::Page { url: "u".into(), status: Some(403), text: "禁止".into(), visible: Vec::new(), judgements: Vec::new() };
    assert!(!disproven(&record(add, denied)));
    // 模型無法判定也不推翻
    let mut with_judge = add.clone();
    with_judge.assertions.push(Assertion::Judge { expectation: "購物車有紅色雨傘".into() });
    assert!(!disproven(&record(&with_judge, page("購物車（1）", vec![Judgement::LiveModelDisabled]))));
}

#[test]
fn an_unmet_postcondition_of_a_chosen_action_is_inconclusive_not_a_product_failure() {
    let base = definition();
    let add = &base.journeys[0].steps[1];
    let failed = record(add, page("購物車（0）", Vec::new()));
    assert_eq!(failed.verdict, Verdict::ProductFailure);

    let live = settle(failed.clone(), ActionSource::Live);
    assert_eq!((live.verdict, live.reason_code.as_str()), (Verdict::Inconclusive, "act_postcondition_failed"));
    let replayed = settle(failed, ActionSource::Replay);
    assert_eq!((replayed.verdict, replayed.reason_code.as_str()), (Verdict::Inconclusive, "replay_postcondition_failed"));
    let passed = settle(record(add, page("購物車（1）", Vec::new())), ActionSource::Replay);
    assert_eq!(passed.verdict, Verdict::Passed);
}

#[test]
fn only_clicks_fills_and_key_presses_can_be_planned() {
    assert!(plannable(&BrowserAction::Click { selector: "#a".into() }));
    assert!(plannable(&BrowserAction::Fill { selector: "#q".into(), text: "傘".into() }));
    assert!(plannable(&BrowserAction::Press { selector: "#q".into(), key: "Enter".into() }));
    assert!(!plannable(&BrowserAction::Goto { url: "https://evil.example".into() }));
    assert!(!plannable(&BrowserAction::WaitFor { selector: "#a".into() }));
}

#[test]
fn a_new_recording_replaces_the_case_and_otherwise_stale_reasons_mark_it() {
    let identity = identity_of(&definition(), ENGINE);
    let base = ActionRecord {
        journey: "checkout".into(),
        step: "add".into(),
        goal: "g".into(),
        cache: CacheState::None,
        source: Some(ActionSource::Live),
        action: None,
        stale_reasons: Vec::new(),
        recorded: None,
    };
    let recorded = RecordedAction { identity, action: BrowserAction::Click { selector: "#add".into() } };
    let actions = vec![
        ActionRecord { recorded: Some(recorded.clone()), stale_reasons: vec!["engine_changed".into()], ..base.clone() },
        ActionRecord { step: "pay".into(), stale_reasons: vec!["postcondition_failed".into()], ..base.clone() },
        ActionRecord { step: "noop".into(), ..base },
    ];
    let bundle: EvidenceBundle = serde_json::from_value(serde_json::json!({
        "id": "evb_2", "workspace_id": "ws", "request_id": "r", "task_id": null, "run_id": null, "generation": null,
        "requirement_version": null, "profile_version_id": "vp", "profile_version": 1, "candidate_revision": null,
        "build": null, "subject_dir": "d", "journeys": [], "steps": [], "verdict": "passed", "reason_code": "x",
        "covered_features": [], "uncovered": [],
        "cleanup": { "state_dir": "s", "state_dir_removed": true },
        "actions": actions, "started_at": 1, "finished_at": 2,
    }))
    .unwrap();

    assert_eq!(
        updates(&bundle),
        vec![
            ReplayUpdate::Record { case_key: "checkout/add".into(), recorded },
            ReplayUpdate::MarkStale { case_key: "checkout/pay".into(), reasons: vec!["postcondition_failed".into()] },
        ]
    );
    assert_eq!(bundle.usage, VerificationUsage::default(), "舊的 Bundle 沒有用量時是 0");
}
