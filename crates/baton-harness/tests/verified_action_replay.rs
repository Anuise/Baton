//! 情境測試：已驗證動作重播與 no_live_model（票 48），經由 Runtime harness 的真實 HTTP API 驗證。
//!
//! 受驗的網站由 fake BrowserDriver 扮演；目標導向動作（`act`）的規劃與語意判斷由腳本化的 fake 模型回應，
//! `calls()` 是模型實際收到的呼叫總數。只斷言外部行為：桌面 API 的回應、Evidence Bundle 的內容（步驟判定、
//! 動作紀錄與用量）、Runner 實際收到的操作，以及模型實際收到的呼叫。
//!
//! 每個測試名稱對應票 48 的驗收條件：
//! - 未經斷言確認的動作不會被錄製：`only_actions_proven_by_a_later_assertion_are_recorded_and_replayed_for_real`
//! - stale 快取不沿用舊成功：`a_stale_recording_never_carries_an_old_success_forward`、
//!   `a_replay_whose_postcondition_fails_marks_the_recording_stale`
//! - no_live_model 下任何模型呼叫都被阻擋：`no_live_model_blocks_every_model_call_at_the_adapter`
//! - 正式環境副作用預設不可重播：`production_side_effects_are_not_replayed_by_default`
//! - 重播與 live 用量分開呈現：`replay_and_live_usage_are_reported_separately`

use baton_domain::Timestamp;
use baton_harness::fakes::FakePage;
use baton_harness::Harness;
use baton_ports::{ActionPlan, BrowserCommand, JudgeOutcome};
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

const WORKSPACES: &str = "/api/v1/workspaces";
const CLOCK_START: Timestamp = Timestamp::from_millis(1_700_000_000_123);
const SITE: &str = "http://127.0.0.1:3000";

const ADD_GOAL: &str = "把指定的商品加入購物車";
const CHECKOUT_GOAL: &str = "完成結帳";
const ADD_BUTTON: &str = "role=button[name=\"加入購物車\"]";
const WISHLIST_BUTTON: &str = "role=button[name=\"加入願望清單\"]";
const CHECKOUT_BUTTON: &str = "role=button[name=\"結帳\"]";
const CART_OK: &str = "購物車裡是顧客要買的紅色雨傘";
const ORDERED: &str = "畫面確認訂單已經成立";

// --- 共用輔助 ---

struct World {
    harness: Harness,
    ws: String,
}

fn url(path: &str) -> String {
    format!("{SITE}{path}")
}

fn step(key: &str, operation: Value, assertions: Value) -> Value {
    json!({ "key": key, "title": format!("步驟 {key}"), "operation": operation, "assertions": assertions })
}

fn goto(path: &str) -> Value {
    json!({ "kind": "browser", "action": { "kind": "goto", "url": url(path) } })
}

fn act(goal: &str) -> Value {
    json!({ "kind": "browser", "action": { "kind": "act", "goal": goal, "params": { "item": "紅色雨傘" } } })
}

/// 網路商店：顧客打開商品頁，以目標導向動作把商品加入購物車，後置條件是購物車顯示一件商品。
fn shop_profile() -> Value {
    json!({
        "interface": "web",
        "personas": [{ "key": "shopper", "title": "顧客", "description": "想買雨傘的顧客" }],
        "journeys": [{
            "key": "buy",
            "title": "購買商品",
            "representative": true,
            "persona": "shopper",
            "steps": [
                step("open", goto("/products"), json!([{ "kind": "contains", "text": "商品清單" }])),
                step("add", act(ADD_GOAL), json!([{ "kind": "contains", "text": "購物車（1）" }])),
            ],
        }],
        "feature_map": [{ "key": "cart", "title": "購物車", "criterion": "可以把商品加入購物車", "journeys": ["buy"] }],
    })
}

fn click(selector: &str) -> ActionPlan {
    ActionPlan::Planned { command: BrowserCommand::Click { selector: selector.into() }, rationale: "畫面上有這個按鈕".into() }
}

fn step_in<'a>(bundle: &'a Value, key: &str) -> &'a Value {
    bundle["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["journey"] == "buy" && s["key"] == key)
        .unwrap_or_else(|| panic!("沒有步驟 {key}：{bundle}"))
}

fn action_in<'a>(bundle: &'a Value, key: &str) -> &'a Value {
    bundle["actions"]
        .as_array()
        .unwrap_or_else(|| panic!("沒有動作紀錄：{bundle}"))
        .iter()
        .find(|a| a["step"] == key)
        .unwrap_or_else(|| panic!("沒有步驟 {key} 的動作紀錄：{bundle}"))
}

fn verdict_of(record: &Value) -> (&str, &str) {
    (record["verdict"].as_str().unwrap_or_default(), record["reason_code"].as_str().unwrap_or_default())
}

async fn world() -> World {
    let harness = Harness::builder().clock_at(CLOCK_START).start().await;
    harness.add_account("帳號 A", "account0@example.com").await;
    let dir = harness.data_dir().join("projects").join("shop");
    std::fs::create_dir_all(&dir).unwrap();
    let (status, body) = harness.send_json(Method::POST, WORKSPACES, Some(json!({ "path": dir.to_string_lossy() }))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let ws = body["id"].as_str().unwrap().to_string();
    World { harness, ws }
}

impl World {
    fn h(&self) -> &Harness {
        &self.harness
    }

    fn path(&self, rest: &str) -> String {
        format!("{WORKSPACES}/{}/verification{rest}", self.ws)
    }

    /// 網站正常：商品頁有加入購物車與願望清單兩個按鈕，購物車頁可以結帳。模型會選對按鈕，畫面判斷也成立。
    fn healthy(&self) {
        let site = &self.h().browser_driver;
        site.script_page(&url("/products"), FakePage::new("商品", "商品清單：紅色雨傘 300 元", &[ADD_BUTTON, WISHLIST_BUTTON]));
        site.script_link(ADD_BUTTON, &url("/cart"));
        site.script_link(WISHLIST_BUTTON, &url("/wishlist"));
        site.script_page(&url("/cart"), FakePage::new("購物車", "購物車（1）：紅色雨傘", &[CHECKOUT_BUTTON]));
        site.script_page(&url("/wishlist"), FakePage::new("願望清單", "願望清單（1）：紅色雨傘；購物車（0）", &[]));
        site.script_link(CHECKOUT_BUTTON, &url("/done"));
        site.script_page(&url("/done"), FakePage::new("完成", "訂單已成立", &[]));
        let model = &self.h().semantic_judge;
        model.script_plan(ADD_GOAL, vec![click(ADD_BUTTON)]);
        model.script_plan(CHECKOUT_GOAL, vec![click(CHECKOUT_BUTTON)]);
        model.script(CART_OK, vec![JudgeOutcome::Decided { satisfied: true, rationale: "購物車有紅色雨傘".into() }]);
        model.script(ORDERED, vec![JudgeOutcome::Decided { satisfied: true, rationale: "顯示訂單已成立".into() }]);
    }

    async fn publish(&self, definition: Value) -> Value {
        let (status, body) = self.h().send_json(Method::POST, &self.path("/profile"), Some(definition)).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        body["profile"].clone()
    }

    async fn overview(&self) -> Value {
        let (status, body) = self.h().send_json(Method::GET, &self.path(""), None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body
    }

    /// 從桌面要求並執行一次驗證，回傳 (驗證要求, Evidence Bundle)。
    async fn verify_with(&self, body: Value) -> (Value, Value) {
        let (status, request) = self.h().send_json(Method::POST, &self.path("/runs"), Some(body)).await;
        assert_eq!(status, StatusCode::CREATED, "{request}");
        self.h().supervise().await;
        let overview = self.overview().await;
        let done = overview["requests"].as_array().unwrap().iter().find(|r| r["id"] == request["id"]).unwrap().clone();
        assert_eq!(done["status"], "completed", "驗證要求應該已執行：{overview}");
        let id = done["bundle_id"].as_str().unwrap();
        let (status, bundle) = self.h().send_json(Method::GET, &self.path(&format!("/bundles/{id}")), None).await;
        assert_eq!(status, StatusCode::OK, "{bundle}");
        (done, bundle)
    }

    /// 允許即時模型的一次驗證。
    async fn live(&self) -> Value {
        self.verify_with(json!({})).await.1
    }

    /// 禁止即時模型（no_live_model）的一次驗證。
    async fn offline(&self) -> Value {
        self.verify_with(json!({ "no_live_model": true })).await.1
    }

    /// Runner 收到的操作數（之後以 `clicks_since` 只看這次驗證的操作）。
    fn mark(&self) -> usize {
        self.h().browser_driver.commands().len()
    }

    /// 自 `mark` 之後 Runner 收到的、點擊這個 selector 的操作數。
    fn clicks_since(&self, mark: usize, selector: &str) -> usize {
        self.h().browser_driver.commands()[mark..].iter().filter(|c| c.starts_with("click ") && c.ends_with(selector)).count()
    }

    fn model_calls(&self) -> usize {
        self.h().semantic_judge.calls()
    }

    /// 先以即時模型錄製加入購物車的動作。
    async fn record(&self) -> Value {
        let bundle = self.live().await;
        assert_eq!(bundle["verdict"], "passed", "{bundle}");
        assert!(action_in(&bundle, "add")["recorded"].is_object(), "被後置條件證明的動作要錄製：{bundle}");
        bundle
    }
}

// --- 驗收條件：未經斷言確認的動作不會被錄製 ---

#[tokio::test]
async fn only_actions_proven_by_a_later_assertion_are_recorded_and_replayed_for_real() {
    let w = world().await;
    w.publish(shop_profile()).await;
    w.healthy();

    // 模型選錯按鈕：後置條件不成立 → 不錄製；選的動作不是 Profile 寫定的操作，所以是證據不足，不是產品缺陷
    w.h().semantic_judge.script_plan(ADD_GOAL, vec![click(WISHLIST_BUTTON)]);
    let mark = w.mark();
    let bundle = w.live().await;
    assert_eq!(w.clicks_since(mark, WISHLIST_BUTTON), 1, "模型選的動作確實交給了 Runner");
    assert_eq!(verdict_of(step_in(&bundle, "add")), ("inconclusive", "act_postcondition_failed"), "{bundle}");
    assert_ne!(bundle["verdict"], "product_failure");
    let add = action_in(&bundle, "add");
    assert_eq!((add["cache"].as_str(), add["source"].as_str()), (Some("none"), Some("live")), "{add}");
    assert!(add["action"].as_str().unwrap().contains("加入願望清單"), "{add}");
    assert!(add.get("recorded").is_none(), "沒有被斷言證明的動作不錄製：{add}");
    // 模型請求只含目標、參數、persona 與當前畫面
    let request = w.h().semantic_judge.plan_requests().pop().unwrap();
    assert_eq!(request.goal, ADD_GOAL);
    assert_eq!(request.params, vec![("item".to_string(), "紅色雨傘".to_string())]);
    assert_eq!((request.page_url.as_str(), request.page_text.as_str()), (url("/products").as_str(), "商品清單：紅色雨傘 300 元"));
    assert!(request.persona.as_deref().is_some_and(|p| p.contains("顧客")), "{request:?}");

    // 沒有錄製：no_live_model 下不重播也不規劃，那個沒被證明的動作不會再送出
    let (calls, mark) = (w.model_calls(), w.mark());
    let bundle = w.offline().await;
    assert_eq!(w.model_calls(), calls, "no_live_model 不呼叫模型");
    assert_eq!(w.clicks_since(mark, WISHLIST_BUTTON) + w.clicks_since(mark, ADD_BUTTON), 0, "沒有錄製可重播");
    assert_eq!(verdict_of(step_in(&bundle, "add")), ("inconclusive", "live_model_disabled"));
    let add = action_in(&bundle, "add");
    assert_eq!(add["cache"], "none");
    assert!(add.get("source").is_none(), "沒有執行任何動作：{add}");

    // 模型選對按鈕：後置條件成立 → 錄製
    w.h().semantic_judge.script_plan(ADD_GOAL, vec![click(ADD_BUTTON)]);
    let bundle = w.record().await;
    let recorded = &action_in(&bundle, "add")["recorded"];
    assert_eq!(recorded["action"], json!({ "kind": "click", "selector": ADD_BUTTON }));
    assert_eq!(recorded["identity"]["goal"], ADD_GOAL);
    assert_eq!(recorded["identity"]["engine"], "fake-browser 1");

    // 重播：不呼叫模型，但 Runner 實際再執行同一個動作並重新檢查後置條件
    let (calls, mark) = (w.model_calls(), w.mark());
    let bundle = w.offline().await;
    assert_eq!(w.model_calls(), calls, "重播不呼叫模型");
    assert_eq!(w.clicks_since(mark, ADD_BUTTON), 1, "重播執行真實動作");
    assert_eq!(bundle["verdict"], "passed", "{bundle}");
    let add_step = step_in(&bundle, "add");
    assert_eq!(add_step["assertions"][0]["passed"], true, "重新檢查後置條件：{add_step}");
    let add = action_in(&bundle, "add");
    assert_eq!((add["cache"].as_str(), add["source"].as_str()), (Some("valid"), Some("replay")), "{add}");
    assert!(add.get("recorded").is_none() && add.get("stale_reasons").is_none(), "重播不改變錄製：{add}");

    // 只靠語意判斷證明的目標導向動作無法發布（重播時要能不呼叫模型重新檢查）
    let mut judge_only = shop_profile();
    judge_only["journeys"][0]["steps"][1]["assertions"] = json!([{ "kind": "judge", "expectation": "購物車有紅色雨傘" }]);
    let (status, body) = w.h().send_json(Method::POST, &w.path("/profile"), Some(judge_only)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"], "verification_profile_invalid", "{body}");
}

// --- 驗收條件：stale 快取不沿用舊成功 ---

#[tokio::test]
async fn a_stale_recording_never_carries_an_old_success_forward() {
    let w = world().await;
    w.publish(shop_profile()).await;
    w.healthy();
    w.record().await;

    // 引擎改變：錄製身分不一致 → 標為 stale，不重播，判定不是 passed
    w.h().browser_driver.set_engine(Some("fake-browser 2"));
    let mark = w.mark();
    let bundle = w.offline().await;
    assert_eq!(w.clicks_since(mark, ADD_BUTTON), 0, "不沿用 stale 的錄製");
    assert_ne!(bundle["verdict"], "passed", "{bundle}");
    let add = action_in(&bundle, "add");
    assert_eq!(add["cache"], "mismatch", "{add}");
    assert_eq!(add["stale_reasons"], json!(["engine_changed"]));
    // 之後仍是 stale：即使引擎改回來也不沿用舊的成功
    w.h().browser_driver.set_engine(Some("fake-browser 1"));
    let mark = w.mark();
    let bundle = w.offline().await;
    assert_eq!(w.clicks_since(mark, ADD_BUTTON), 0);
    assert_eq!(action_in(&bundle, "add")["cache"], "stale");
    assert_eq!(verdict_of(step_in(&bundle, "add")), ("inconclusive", "live_model_disabled"));

    // 允許即時模型時重新規劃並錄製，錄製恢復可用
    let bundle = w.record().await;
    assert_eq!(action_in(&bundle, "add")["cache"], "stale");
    let bundle = w.offline().await;
    assert_eq!((bundle["verdict"].as_str(), action_in(&bundle, "add")["source"].as_str()), (Some("passed"), Some("replay")));

    // 目標改變（新的 Profile 版本）
    let mut changed_goal = shop_profile();
    changed_goal["journeys"][0]["steps"][1]["operation"] = act("把指定的商品放進購物車");
    w.publish(changed_goal).await;
    w.h().semantic_judge.script_plan("把指定的商品放進購物車", vec![click(ADD_BUTTON)]);
    let bundle = w.offline().await;
    assert_ne!(bundle["verdict"], "passed");
    assert_eq!(action_in(&bundle, "add")["stale_reasons"], json!(["goal_changed"]));

    // 資料集版本改變（資料相容性）
    let bundle = w.record().await;
    assert_eq!(action_in(&bundle, "add")["cache"], "stale");
    let mut new_dataset = shop_profile();
    new_dataset["journeys"][0]["steps"][1]["operation"] = act("把指定的商品放進購物車");
    new_dataset["replay"] = json!({ "dataset": "2026-11" });
    w.publish(new_dataset).await;
    let mark = w.mark();
    let bundle = w.offline().await;
    assert_eq!(w.clicks_since(mark, ADD_BUTTON), 0);
    assert_ne!(bundle["verdict"], "passed");
    let add = action_in(&bundle, "add");
    assert_eq!((add["cache"].as_str(), &add["stale_reasons"]), (Some("mismatch"), &json!(["data_changed"])));
}

#[tokio::test]
async fn a_replay_whose_postcondition_fails_marks_the_recording_stale() {
    let w = world().await;
    w.publish(shop_profile()).await;
    w.healthy();
    w.record().await;

    // 網站行為改變：重播的動作實際執行了，但後置條件不成立 → 證據不足（不是產品缺陷），錄製改為 stale
    w.h().browser_driver.script_link(ADD_BUTTON, &url("/cart-empty"));
    w.h().browser_driver.script_page(&url("/cart-empty"), FakePage::new("購物車", "購物車（0）", &[]));
    let mark = w.mark();
    let bundle = w.offline().await;
    assert_eq!(w.clicks_since(mark, ADD_BUTTON), 1, "重播仍執行真實動作");
    assert_eq!(verdict_of(step_in(&bundle, "add")), ("inconclusive", "replay_postcondition_failed"), "{bundle}");
    assert_eq!(bundle["verdict"], "inconclusive");
    let add = action_in(&bundle, "add");
    assert_eq!((add["cache"].as_str(), add["source"].as_str()), (Some("valid"), Some("replay")));
    assert_eq!(add["stale_reasons"], json!(["postcondition_failed"]));

    // 下一次不再重播：網站恢復也不沿用舊的成功
    w.healthy();
    let mark = w.mark();
    let bundle = w.offline().await;
    assert_eq!(w.clicks_since(mark, ADD_BUTTON), 0);
    assert_eq!(action_in(&bundle, "add")["cache"], "stale");
    assert_ne!(bundle["verdict"], "passed");
}

// --- 驗收條件：no_live_model 下任何模型呼叫都被阻擋 ---

#[tokio::test]
async fn no_live_model_blocks_every_model_call_at_the_adapter() {
    let w = world().await;
    let mut profile = shop_profile();
    let confirm = json!([{ "kind": "contains", "text": "購物車（1）" }, { "kind": "judge", "expectation": CART_OK }]);
    profile["journeys"][0]["steps"].as_array_mut().unwrap().push(step("confirm", goto("/cart"), confirm));
    w.publish(profile).await;
    w.healthy();

    // 沒有錄製、有可用帳號、模型也有腳本：仍然一次都不呼叫
    let (request, bundle) = w.verify_with(json!({ "no_live_model": true })).await;
    assert_eq!(w.model_calls(), 0, "no_live_model 下模型沒有收到任何呼叫");
    assert_eq!(request["no_live_model"], true, "驗證要求記著 no_live_model：{request}");
    assert_eq!(bundle["no_live_model"], true);
    let add = step_in(&bundle, "add");
    assert_eq!((add["verdict"].as_str(), add["engine_code"].as_str()), (Some("inconclusive"), Some("model:disabled")), "{add}");
    assert_eq!(add["reason_code"], "live_model_disabled");
    assert_eq!(bundle["usage"], json!({ "live_model_calls": 0, "live_actions": 0, "replayed_actions": 0, "blocked_model_calls": 1 }));

    // 允許即時模型：規劃與語意判斷各呼叫一次，動作被錄製
    let (request, bundle) = w.verify_with(json!({})).await;
    assert_eq!(request["no_live_model"], false);
    assert_eq!(bundle["verdict"], "passed", "{bundle}");
    assert_eq!(w.model_calls(), 2);
    assert!(bundle.get("no_live_model").is_none());

    // 有錄製時 no_live_model 重播動作，語意判斷仍被擋下；模型呼叫數不變
    let bundle = w.offline().await;
    assert_eq!(w.model_calls(), 2, "no_live_model 下模型沒有收到任何呼叫");
    assert_eq!(action_in(&bundle, "add")["source"], "replay");
    assert_eq!(verdict_of(step_in(&bundle, "add")), ("passed", "assertions_passed"));
    let confirm = step_in(&bundle, "confirm");
    assert_eq!(verdict_of(confirm), ("inconclusive", "live_model_disabled"), "{confirm}");
    assert_eq!(confirm["engine_code"], "model:disabled");
    assert_eq!(confirm["assertions"][0]["passed"], true, "不需模型的斷言照常判定");
    assert!(confirm["assertions"][1]["actual"].as_str().unwrap().contains("no_live_model"), "{confirm}");
    assert_eq!(bundle["verdict"], "inconclusive", "需要模型的斷言沒有判定，整體不算通過");
    assert_eq!(bundle["usage"], json!({ "live_model_calls": 0, "live_actions": 0, "replayed_actions": 1, "blocked_model_calls": 1 }));
    // 概況中的驗證要求也記著 no_live_model
    let overview = w.overview().await;
    let flags: Vec<bool> = overview["requests"].as_array().unwrap().iter().map(|r| r["no_live_model"].as_bool().unwrap()).collect();
    assert_eq!(flags, vec![true, false, true], "新的在前：{overview}");
}

// --- 驗收條件：正式環境副作用預設不可重播 ---

#[tokio::test]
async fn production_side_effects_are_not_replayed_by_default() {
    let w = world().await;
    let mut production = shop_profile();
    production["replay"] = json!({ "production": true });
    w.publish(production.clone()).await;
    w.healthy();
    w.record().await;

    // 有效的錄製也不重播：no_live_model 下沒有可執行的動作 → 證據不足，Runner 沒收到動作
    let mark = w.mark();
    let bundle = w.offline().await;
    assert_eq!(w.clicks_since(mark, ADD_BUTTON), 0, "正式環境的副作用不重播");
    let add = action_in(&bundle, "add");
    assert_eq!(add["cache"], "production_not_replayable", "{add}");
    assert!(add.get("stale_reasons").is_none(), "不重播不代表錄製失效：{add}");
    assert_eq!(verdict_of(step_in(&bundle, "add")), ("inconclusive", "live_model_disabled"));

    // 步驟個別允許才重播（錄製身分不變，同一個錄製可以沿用）
    production["journeys"][0]["steps"][1]["operation"]["action"]["replay_in_production"] = json!(true);
    w.publish(production).await;
    let mark = w.mark();
    let bundle = w.offline().await;
    assert_eq!(w.clicks_since(mark, ADD_BUTTON), 1);
    assert_eq!((bundle["verdict"].as_str(), action_in(&bundle, "add")["source"].as_str()), (Some("passed"), Some("replay")));
}

// --- 驗收條件：重播與 live 用量分開呈現 ---

#[tokio::test]
async fn replay_and_live_usage_are_reported_separately() {
    let w = world().await;
    let mut profile = shop_profile();
    let checkout = json!([{ "kind": "contains", "text": "訂單已成立" }, { "kind": "judge", "expectation": ORDERED }]);
    profile["journeys"][0]["steps"].as_array_mut().unwrap().push(step("checkout", act(CHECKOUT_GOAL), checkout));
    w.publish(profile).await;
    w.healthy();
    // 第一次結帳的規劃模型無法使用：只錄製加入購物車
    w.h().semantic_judge.script_plan(
        CHECKOUT_GOAL,
        vec![ActionPlan::Unavailable { reason: "Not logged in".into() }, click(CHECKOUT_BUTTON)],
    );

    let bundle = w.live().await;
    assert_eq!(verdict_of(step_in(&bundle, "checkout")), ("inconclusive", "planner_unavailable"));
    assert!(action_in(&bundle, "add")["recorded"].is_object());
    assert_eq!(bundle["usage"], json!({ "live_model_calls": 2, "live_actions": 1, "replayed_actions": 0, "blocked_model_calls": 0 }));

    // 加入購物車重播；結帳由模型規劃、畫面判斷呼叫模型
    let bundle = w.live().await;
    assert_eq!(bundle["verdict"], "passed", "{bundle}");
    assert_eq!(action_in(&bundle, "add")["source"], "replay");
    assert_eq!(action_in(&bundle, "checkout")["source"], "live");
    assert_eq!(bundle["usage"], json!({ "live_model_calls": 2, "live_actions": 1, "replayed_actions": 1, "blocked_model_calls": 0 }));

    // 兩個動作都重播；語意判斷被擋下
    let calls = w.model_calls();
    let bundle = w.offline().await;
    assert_eq!(w.model_calls(), calls);
    assert_eq!(bundle["usage"], json!({ "live_model_calls": 0, "live_actions": 0, "replayed_actions": 2, "blocked_model_calls": 1 }));
    assert_eq!(verdict_of(step_in(&bundle, "checkout")), ("inconclusive", "live_model_disabled"));
}
