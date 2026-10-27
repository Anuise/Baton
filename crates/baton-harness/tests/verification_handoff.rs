//! 情境測試：驗證中的額度交接（票 49），經由 Runtime harness 的真實 HTTP API 驗證。
//!
//! 受驗的網站由 fake BrowserDriver 扮演、受驗服務與 API 由 fake VerificationDriver 扮演；語意 judge 是腳本化的 fake，
//! 記錄每一次的輸入（含使用的帳號）。帳號額度由 fake QuotaObserver 的樣本決定。「Runtime 中途崩潰」= 讓某個 HTTP 請求
//! 送出後永遠不回來、等 fake 回報那個請求已送出就以 `supervise_until` 丟棄那次處理，再 `crash_restart`（資料目錄與
//! 全部 fake 都保留，所以前一個 Runtime 啟動的服務與瀏覽器 Session 仍然存在）。只斷言外部行為：桌面 API 回傳的
//! Evidence Bundle、受驗服務與瀏覽器實際收到的操作、judge 實際收到的要求。
//!
//! 每個測試名稱對應票 49 的驗收條件：
//! - 驗證中途交接後結果正確，未完成案例不會被拼成 passed：
//!   `a_rate_limited_judge_hands_off_and_the_successor_reruns_the_unfinished_case_from_a_fresh_fixture`、
//!   `an_unfinished_case_is_not_stitched_into_a_pass_when_the_successor_cannot_finish_it`、
//!   `after_a_crash_the_successor_stops_the_old_test_processes_and_reruns_the_unfinished_case`、
//!   `a_successor_runs_nothing_while_an_old_test_process_cannot_be_confirmed_stopped`
//! - 結果未知的外部操作不因重測而重送：
//!   `an_external_operation_with_an_unknown_result_is_not_resent_after_a_crash`、
//!   `a_case_that_already_sent_an_external_operation_is_not_rerun_after_a_handoff`
//! - 驗證納入帳號額度；全帳號受限時只執行不需模型的斷言：
//!   `the_judge_only_uses_accounts_below_the_quota_gate_and_without_any_only_model_free_assertions_run`

use std::path::Path;

use baton_domain::Timestamp;
use baton_harness::fakes::FakePage;
use baton_harness::Harness;
use baton_ports::{Clock, Confidence, JudgeOutcome, QuotaSample, QuotaSource};
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

const WORKSPACES: &str = "/api/v1/workspaces";
const CLOCK_START: Timestamp = Timestamp::from_millis(1_700_000_000_123);
const SITE: &str = "http://127.0.0.1:3000";
const API: &str = "http://127.0.0.1:3000/api";

const SORTED: &str = "商品依價格由低到高排列";
const PAID: &str = "訂單顯示為已付款";

// --- 共用輔助 ---

struct World {
    harness: Harness,
    ws: String,
    accounts: Vec<String>,
}

fn url(path: &str) -> String {
    format!("{SITE}{path}")
}

fn step(key: &str, operation: Value, assertions: Value) -> Value {
    json!({ "key": key, "title": format!("步驟 {key}"), "operation": operation, "assertions": assertions })
}

fn external(mut step: Value) -> Value {
    step["external"] = json!(true);
    step
}

fn browser(action: Value) -> Value {
    json!({ "kind": "browser", "action": action })
}

fn goto(path: &str) -> Value {
    browser(json!({ "kind": "goto", "url": url(path) }))
}

fn http(method: &str, path: &str) -> Value {
    json!({ "kind": "http", "method": method, "url": format!("{API}{path}") })
}

/// 網路商店：啟動服務、就緒、準備資料；`journeys` 依序執行。
fn shop_profile(interface: &str, journeys: Value) -> Value {
    let keys: Vec<Value> = journeys.as_array().unwrap().iter().map(|j| j["key"].clone()).collect();
    json!({
        "interface": interface,
        "start": { "program": "shop", "args": ["serve", "--data", "{{state_dir}}"] },
        "readiness": { "step": step("health", http("GET", "/health"), json!([{ "kind": "status", "equals": 200 }])), "attempts": 1 },
        "setup": [step("seed", http("POST", "/seed"), json!([{ "kind": "status", "equals": 200 }]))],
        "journeys": journeys,
        "feature_map": [{ "key": "shop", "title": "商店", "criterion": "顧客可以購物", "journeys": keys }],
    })
}

/// 不需要模型的旅程：商品目錄。
fn catalog() -> Value {
    json!({
        "key": "catalog",
        "title": "商品目錄",
        "representative": true,
        "steps": [step("open", goto("/catalog"), json!([{ "kind": "contains", "text": "商品目錄" }]))],
    })
}

/// 需要語意判斷的旅程：依價格排序；中間查一次庫存 API。
fn sorting() -> Value {
    json!({
        "key": "sorting",
        "title": "依價格排序",
        "steps": [
            step("open", goto("/products"), json!([{ "kind": "visible", "selector": "#sort" }, { "kind": "contains", "text": "商品清單" }])),
            step("stock", http("GET", "/stock"), json!([{ "kind": "status", "equals": 200 }])),
            step("sort", browser(json!({ "kind": "click", "selector": "#sort" })), json!([
                { "kind": "url_contains", "text": "sort=price" },
                { "kind": "judge", "expectation": SORTED },
            ])),
        ],
    })
}

/// 下單：付款是外部操作（例如呼叫第三方金流），之後看訂單頁。
fn checkout() -> Value {
    json!({
        "key": "checkout",
        "title": "結帳",
        "steps": [
            external(step("pay", http("POST", "/orders"), json!([{ "kind": "status", "equals": 201 }]))),
            step("confirm", goto("/orders/1"), json!([{ "kind": "judge", "expectation": PAID }])),
        ],
    })
}

fn journey_of<'a>(bundle: &'a Value, key: &str) -> &'a Value {
    bundle["journeys"].as_array().unwrap().iter().find(|j| j["key"] == key).unwrap_or_else(|| panic!("沒有旅程 {key}：{bundle}"))
}

/// Bundle 中某條旅程的旅程步驟 (步驟, 判定, 原因)。
fn journey_steps(bundle: &Value, journey: &str) -> Vec<(String, String, String)> {
    bundle["steps"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["journey"] == journey && s["phase"] == "journey")
        .map(|s| {
            let text = |k: &str| s[k].as_str().unwrap().to_string();
            (text("key"), text("verdict"), text("reason_code"))
        })
        .collect()
}

fn strings(value: &Value) -> Vec<String> {
    value.as_array().unwrap_or_else(|| panic!("不是陣列：{value}")).iter().map(|v| v.as_str().unwrap().to_string()).collect()
}

async fn world(accounts: usize) -> World {
    let harness = Harness::builder().clock_at(CLOCK_START).start().await;
    let mut ids = Vec::new();
    for index in 0..accounts {
        let name = ["帳號 A", "帳號 B", "帳號 C"][index];
        ids.push(harness.add_account(name, &format!("account{index}@example.com")).await);
    }
    let dir = harness.data_dir().join("projects").join("shop");
    std::fs::create_dir_all(&dir).unwrap();
    let (status, body) = harness.send_json(Method::POST, WORKSPACES, Some(json!({ "path": dir.to_string_lossy() }))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    // 受驗目錄是 Git repo（有 revision）：沿用已完成的旅程需要觀察得到的 build 與 revision（AI-427、R151 第 23 項）
    harness.verification_driver.set_revision(Path::new(body["path"].as_str().unwrap()), "c1");
    let ws = body["id"].as_str().unwrap().to_string();
    World { harness, ws, accounts: ids }
}

impl World {
    fn h(&self) -> &Harness {
        &self.harness
    }

    fn path(&self, rest: &str) -> String {
        format!("{WORKSPACES}/{}/verification{rest}", self.ws)
    }

    /// 服務與網站都正常；語意判斷都會通過。
    fn healthy(&self) {
        let driver = &self.h().verification_driver;
        driver.script_response("GET", &format!("{API}/health"), 200, "ok");
        driver.script_response("POST", &format!("{API}/seed"), 200, "seeded");
        driver.script_response("GET", &format!("{API}/stock"), 200, "{\"A\":3,\"B\":5}");
        driver.script_response("POST", &format!("{API}/orders"), 201, "{\"id\":1}");
        let site = &self.h().browser_driver;
        site.script_page(&url("/catalog"), FakePage::new("目錄", "商品目錄：A、B", &[]));
        site.script_page(&url("/products"), FakePage::new("商品", "商品清單：B 200 元、A 100 元", &["#sort"]));
        site.script_link("#sort", &url("/products?sort=price"));
        site.script_page(&url("/products?sort=price"), FakePage::new("商品", "商品清單：A 100 元、B 200 元", &["#sort"]));
        site.script_page(&url("/orders/1"), FakePage::new("訂單", "訂單 1：已付款", &[]));
        let judge = &self.h().semantic_judge;
        judge.script(SORTED, vec![JudgeOutcome::Decided { satisfied: true, rationale: "A 100 元在 B 200 元之前".into() }]);
        judge.script(PAID, vec![JudgeOutcome::Decided { satisfied: true, rationale: "畫面寫著已付款".into() }]);
    }

    async fn publish(&self, definition: Value) {
        let (status, body) = self.h().send_json(Method::POST, &self.path("/profile"), Some(definition)).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }

    async fn request(&self) -> String {
        let (status, request) = self.h().send_json(Method::POST, &self.path("/runs"), Some(json!({}))).await;
        assert_eq!(status, StatusCode::CREATED, "{request}");
        request["id"].as_str().unwrap().to_string()
    }

    async fn request_of(&self, id: &str) -> Value {
        let (status, overview) = self.h().send_json(Method::GET, &self.path(""), None).await;
        assert_eq!(status, StatusCode::OK, "{overview}");
        overview["requests"].as_array().unwrap().iter().find(|r| r["id"] == id).unwrap().clone()
    }

    async fn bundle_of(&self, request: &str) -> Value {
        let done = self.request_of(request).await;
        assert_eq!(done["status"], "completed", "驗證要求應該已執行：{done}");
        let path = self.path(&format!("/bundles/{}", done["bundle_id"].as_str().unwrap()));
        let (status, bundle) = self.h().send_json(Method::GET, &path, None).await;
        assert_eq!(status, StatusCode::OK, "{bundle}");
        bundle
    }

    /// 要求並執行一次驗證，回傳 Evidence Bundle。
    async fn verify(&self) -> Value {
        let request = self.request().await;
        self.h().supervise().await;
        self.bundle_of(&request).await
    }

    /// 要求一次驗證，讓它在送出 `method path` 之後當掉，再重啟 Runtime。回傳驗證要求識別。
    async fn crash_during(&mut self, method: &str, path: &str) -> String {
        let request = self.request().await;
        self.h().verification_driver.hang_next_http(method, &format!("{API}{path}"));
        self.h().supervise_until(self.h().verification_driver.wait_for_http_hang()).await;
        assert_eq!(self.request_of(&request).await["status"], "queued", "當掉時沒有 Bundle");
        self.harness.crash_restart().await;
        request
    }

    fn judge_accounts(&self) -> Vec<String> {
        self.h().semantic_judge.requests().iter().map(|r| r.account.profile_id.clone()).collect()
    }

    fn browser_count(&self, needle: &str) -> usize {
        self.h().browser_driver.commands().iter().filter(|c| c.contains(needle)).count()
    }

    fn http_count(&self, method: &str, path: &str) -> usize {
        let target = format!("{API}{path}");
        self.h().verification_driver.http_requests().iter().filter(|r| r.method == method && r.url == target).count()
    }

    /// 受驗服務收到的操作中，`first` 第一次出現在 `then` 第 `nth` 次出現之前。
    fn happened_before(&self, first: &str, then: &str, nth: usize) -> bool {
        let operations = self.h().verification_driver.operations();
        let first = operations.iter().position(|o| o == first);
        let then = operations.iter().enumerate().filter(|(_, o)| o.starts_with(then)).nth(nth).map(|(i, _)| i);
        matches!((first, then), (Some(a), Some(b)) if a < b)
    }

    /// 帳號的用量樣本；先讓時間前進，下一次處理時才會重新觀測。
    fn set_usage(&self, profile: &str, used: f64) {
        self.h().clock.advance_millis(5 * 60 * 1000);
        let sample = QuotaSample {
            source: QuotaSource::Observed,
            observed_at: self.h().clock.now(),
            used: Some(used),
            reset_at: None,
            confidence: Confidence::Observed,
        };
        self.h().quota_observer.set_sample(profile, sample);
    }
}


// --- 驗收條件：驗證中途交接後結果正確，未完成案例不會被拼成 passed ---

#[tokio::test]
async fn a_rate_limited_judge_hands_off_and_the_successor_reruns_the_unfinished_case_from_a_fresh_fixture() {
    let w = world(2).await;
    let (a, b) = (w.accounts[0].clone(), w.accounts[1].clone());
    w.publish(shop_profile("web", json!([catalog(), sorting()]))).await;
    w.healthy();
    // 帳號 A 判斷到一半限流；另一個帳號可以接手
    w.h().semantic_judge.script(
        SORTED,
        vec![
            JudgeOutcome::RateLimited { reset_at_millis: None },
            JudgeOutcome::Decided { satisfied: true, rationale: "A 100 元在 B 200 元之前".into() },
        ],
    );
    let bundle = w.verify().await;

    assert_eq!((bundle["verdict"].as_str(), bundle["reason_code"].as_str()), (Some("passed"), Some("all_journeys_passed")), "{bundle}");
    assert_eq!(w.judge_accounts(), [a.clone(), b.clone()], "限流之後由另一個帳號判斷");
    let attempts = bundle["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 2, "{bundle}");
    let (first, second) = (&attempts[0], &attempts[1]);
    assert_eq!((first["status"].as_str(), first["end_reason"].as_str()), (Some("handed_off"), Some("judge_rate_limited")));
    assert_eq!(first["judge_profile_id"], a.as_str());
    assert_eq!(strings(&first["journeys"]), ["catalog"], "目錄的結果採用第一次嘗試");
    // 第一次嘗試記下了它完成的步驟與判定，包含交接前沒有完成的排序旅程
    let recorded: Vec<(String, String)> = first["steps"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["journey"] == "sorting")
        .map(|s| (s["key"].as_str().unwrap().into(), s["reason_code"].as_str().unwrap().into()))
        .collect();
    assert_eq!(
        recorded,
        [("open".into(), "assertions_passed".into()), ("stock".into(), "assertions_passed".into()), ("sort".into(), "judge_rate_limited".into())]
    );
    assert_eq!((second["status"].as_str(), second["judge_profile_id"].as_str()), (Some("completed"), Some(b.as_str())));
    assert_eq!(strings(&second["journeys"]), ["sorting"]);
    let takeover = &second["takeover"];
    assert_eq!(takeover["from_attempt"], 1);
    assert_eq!(strings(&takeover["reused_journeys"]), ["catalog"], "已完成的旅程是確認過的邊界，不重跑");
    assert!(strings(&takeover["problems"]).is_empty(), "{takeover}");

    // 排序旅程在新的嘗試中從頭重跑（新的 Session、新的狀態目錄、重新準備資料）；目錄沒有重跑
    assert_eq!(w.browser_count(&url("/catalog")), 1);
    assert_eq!(w.browser_count(&url("/products")), 2, "兩次嘗試各自從頭開啟");
    assert_eq!(w.http_count("GET", "/stock"), 2, "不是外部操作的步驟照常重跑");
    assert_eq!(w.http_count("POST", "/seed"), 2, "承接者重新準備隔離的資料");
    assert_eq!(journey_steps(&bundle, "sorting").iter().map(|(_, v, _)| v.as_str()).collect::<Vec<_>>(), ["passed"; 3]);
    // 交接先停止舊的測試程序：第一次的服務在第二次啟動之前停止，瀏覽器 Session 都已關閉
    assert!(w.happened_before("stop svc-1", "start shop serve", 1), "{:?}", w.h().verification_driver.operations());
    assert!(w.h().verification_driver.running_services().is_empty());
    assert_eq!(w.h().browser_driver.open_sessions(), 0);
}

#[tokio::test]
async fn an_unfinished_case_is_not_stitched_into_a_pass_when_the_successor_cannot_finish_it() {
    let w = world(2).await;
    let (a, b) = (w.accounts[0].clone(), w.accounts[1].clone());
    w.publish(shop_profile("web", json!([catalog(), sorting()]))).await;
    w.healthy();
    // 兩個帳號都限流：第一次的「開啟、查庫存」與第二次的「開啟、查庫存」都通過，但排序始終沒有被判斷
    w.h().semantic_judge.script(SORTED, vec![JudgeOutcome::RateLimited { reset_at_millis: None }]);
    let bundle = w.verify().await;

    assert_eq!(w.judge_accounts(), [a, b], "沒有第三個帳號：第二次限流後不再呼叫模型");
    assert_eq!((bundle["verdict"].as_str(), bundle["reason_code"].as_str()), (Some("inconclusive"), Some("judge_rate_limited")), "{bundle}");
    let sorting = journey_of(&bundle, "sorting");
    assert_eq!((sorting["verdict"].as_str(), sorting["reason_code"].as_str()), (Some("inconclusive"), Some("judge_rate_limited")));
    assert_eq!(journey_of(&bundle, "catalog")["verdict"], "passed");
    // 旅程的步驟紀錄只來自一次嘗試（最後一次），沒有把前一次的步驟拼進來
    let steps = journey_steps(&bundle, "sorting");
    assert_eq!(steps.len(), 3, "{steps:?}");
    assert_eq!(steps[2].2, "judge_rate_limited");
    let attempts = bundle["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 2);
    assert_eq!(strings(&attempts[1]["journeys"]), ["sorting"]);
}

#[tokio::test]
async fn after_a_crash_the_successor_stops_the_old_test_processes_and_reruns_the_unfinished_case() {
    let mut w = world(1).await;
    w.publish(shop_profile("web", json!([catalog(), sorting()]))).await;
    w.healthy();
    // 目錄已完成；排序旅程「開啟」通過之後，查庫存時 Runtime 當掉
    let request = w.crash_during("GET", "/stock").await;
    assert_eq!(w.h().verification_driver.running_services(), ["svc-1"], "前一個 Runtime 啟動的服務仍在執行");
    assert_eq!(w.h().browser_driver.open_sessions(), 1, "排序旅程的瀏覽器 Session 仍開著");

    // 重啟後網站改變：排序頁已經壞掉。第一次嘗試中「開啟」通過的紀錄不能拼進結果
    w.h().browser_driver.script_page(&url("/products"), FakePage::new("商品", "系統錯誤", &[]));
    w.h().supervise().await;
    let bundle = w.bundle_of(&request).await;

    assert_eq!((bundle["verdict"].as_str(), bundle["reason_code"].as_str()), (Some("product_failure"), Some("assertion_failed")), "{bundle}");
    let steps = journey_steps(&bundle, "sorting");
    let verdicts: Vec<&str> = steps.iter().map(|(_, v, _)| v.as_str()).collect();
    assert_eq!(verdicts, ["product_failure", "inconclusive", "inconclusive"], "{steps:?}");
    assert_eq!(journey_of(&bundle, "catalog")["verdict"], "passed", "已完成的目錄沿用");
    let attempts = bundle["attempts"].as_array().unwrap();
    assert_eq!((attempts[0]["status"].as_str(), attempts[0]["end_reason"].as_str()), (Some("interrupted"), Some("runtime_interrupted")));
    let takeover = &attempts[1]["takeover"];
    assert_eq!(strings(&takeover["stopped_services"]), ["svc-1"], "承接者以安全的參照停止舊服務：{takeover}");
    assert_eq!(strings(&takeover["closed_sessions"]).len(), 1, "{takeover}");
    assert_eq!(strings(&takeover["reused_journeys"]), ["catalog"]);
    assert!(w.happened_before("stop svc-1", "start shop serve", 1), "{:?}", w.h().verification_driver.operations());
    assert!(w.h().verification_driver.running_services().is_empty());
    assert_eq!(w.h().browser_driver.open_sessions(), 0);
    assert_eq!(w.browser_count(&url("/catalog")), 1, "目錄沒有重跑");
}

#[tokio::test]
async fn a_successor_runs_nothing_while_an_old_test_process_cannot_be_confirmed_stopped() {
    let mut w = world(1).await;
    w.publish(shop_profile("web", json!([catalog(), sorting()]))).await;
    w.healthy();
    let request = w.crash_during("GET", "/stock").await;
    // 前一個 Runtime 啟動的服務停不下來：它可能仍在回應，承接者不能對它驗證
    w.h().verification_driver.fail_stop("svc-1", "存取被拒");
    w.h().supervise().await;
    let bundle = w.bundle_of(&request).await;

    assert_eq!((bundle["verdict"].as_str(), bundle["reason_code"].as_str()), (Some("environment_blocked"), Some("previous_session_unconfirmed")), "{bundle}");
    let operations = w.h().verification_driver.operations();
    assert_eq!(operations.iter().filter(|o| o.starts_with("start ")).count(), 1, "沒有啟動新的服務：{operations:?}");
    assert_eq!(w.http_count("GET", "/stock"), 1, "沒有重跑");
    let takeover = &bundle["attempts"][1]["takeover"];
    let problems = strings(&takeover["problems"]);
    assert!(problems.len() == 1 && problems[0].contains("svc-1") && problems[0].contains("存取被拒"), "{takeover}");
    assert_ne!(journey_of(&bundle, "sorting")["verdict"], "passed");
}

// --- 驗收條件：結果未知的外部操作不因重測而重送 ---

#[tokio::test]
async fn an_external_operation_with_an_unknown_result_is_not_resent_after_a_crash() {
    let mut w = world(1).await;
    w.publish(shop_profile("web", json!([catalog(), checkout()]))).await;
    w.healthy();
    // 付款請求送出後 Runtime 當掉：付款到底有沒有成立不知道
    let request = w.crash_during("POST", "/orders").await;
    w.h().supervise().await;
    let bundle = w.bundle_of(&request).await;

    assert_eq!(w.http_count("POST", "/orders"), 1, "結果未知的付款不重送");
    assert_eq!(w.browser_count(&url("/orders/1")), 0, "那條旅程也不重跑");
    assert_eq!((bundle["verdict"].as_str(), bundle["reason_code"].as_str()), (Some("inconclusive"), Some("external_outcome_unknown")), "{bundle}");
    let journey = journey_of(&bundle, "checkout");
    assert_eq!((journey["verdict"].as_str(), journey["reason_code"].as_str()), (Some("inconclusive"), Some("external_outcome_unknown")));
    assert_eq!(
        journey_steps(&bundle, "checkout"),
        [
            ("pay".to_string(), "inconclusive".to_string(), "external_outcome_unknown".to_string()),
            ("confirm".to_string(), "inconclusive".to_string(), "not_executed".to_string()),
        ]
    );
    let takeover = &bundle["attempts"][1]["takeover"];
    assert_eq!(takeover["not_resent"], json!([{ "journey": "checkout", "step": "pay", "reason": "external_outcome_unknown", "attempt": 1 }]));
    assert_eq!(journey_of(&bundle, "catalog")["verdict"], "passed");

    // 再處理一次也不會重送（要求已經完成）
    w.h().supervise().await;
    assert_eq!(w.http_count("POST", "/orders"), 1);
}

#[tokio::test]
async fn a_case_that_already_sent_an_external_operation_is_not_rerun_after_a_handoff() {
    let w = world(2).await;
    let a = w.accounts[0].clone();
    w.publish(shop_profile("web", json!([catalog(), checkout()]))).await;
    w.healthy();
    // 付款已成立，確認畫面時帳號 A 限流：交接給 B，但重跑結帳會再付一次款
    w.h().semantic_judge.script(
        PAID,
        vec![JudgeOutcome::RateLimited { reset_at_millis: None }, JudgeOutcome::Decided { satisfied: true, rationale: "已付款".into() }],
    );
    let bundle = w.verify().await;

    assert_eq!(w.http_count("POST", "/orders"), 1, "已送出的付款不重送");
    assert_eq!(w.judge_accounts(), [a], "結帳沒有重跑，所以不再判斷");
    let journey = journey_of(&bundle, "checkout");
    assert_eq!((journey["verdict"].as_str(), journey["reason_code"].as_str()), (Some("inconclusive"), Some("external_already_sent")));
    assert_ne!(bundle["verdict"], "passed", "{bundle}");
    let attempts = bundle["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 2);
    assert_eq!(strings(&attempts[0]["journeys"]), ["catalog", "checkout"], "兩條旅程的結果都來自第一次嘗試");
    assert_eq!(attempts[1]["takeover"]["not_resent"][0]["reason"], "external_already_sent");
}

// --- 驗證納入帳號額度；全帳號受限時只執行不需模型的斷言 ---

#[tokio::test]
async fn the_judge_only_uses_accounts_below_the_quota_gate_and_without_any_only_model_free_assertions_run() {
    let w = world(2).await;
    let (a, b) = (w.accounts[0].clone(), w.accounts[1].clone());
    w.publish(shop_profile("web", json!([catalog(), sorting()]))).await;
    w.healthy();

    // A 已達停止派工門檻：語意判斷改用 B
    w.set_usage(&a, 0.8);
    w.h().supervise().await;
    let bundle = w.verify().await;
    assert_eq!(bundle["verdict"], "passed", "{bundle}");
    assert_eq!(w.judge_accounts(), std::slice::from_ref(&b));
    assert_eq!(bundle["attempts"][0]["judge_profile_id"], b.as_str());

    // 兩個帳號都受限：不呼叫模型；不需要模型的旅程照常驗證，需要的保持未完成（不是通過也不是缺陷）
    w.set_usage(&b, 0.9);
    w.h().supervise().await;
    let bundle = w.verify().await;
    assert_eq!(w.judge_accounts().len(), 1, "全部受限時不呼叫模型");
    assert_eq!(bundle["attempts"][0]["judge_profile_id"], Value::Null);
    assert_eq!(journey_of(&bundle, "catalog")["verdict"], "passed");
    let sorting = journey_of(&bundle, "sorting");
    assert_eq!((sorting["verdict"].as_str(), sorting["reason_code"].as_str()), (Some("inconclusive"), Some("judge_unavailable")));
    assert_eq!(bundle["verdict"], "inconclusive");
}
