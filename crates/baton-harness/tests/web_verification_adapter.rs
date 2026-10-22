//! 情境測試：web 驗證 Adapter（票 46，spec D-29），經由 Runtime harness 的真實 HTTP API 驗證。
//!
//! 受驗的網站由 fake BrowserDriver 扮演（畫面依這個瀏覽器 Session 輸入過的帳密決定，所以「以誰登入」是網站自己的事實）；
//! 受驗服務的啟動、就緒與資料準備沿用票 45 的 fake VerificationDriver；語意 judge 是腳本化的 fake，記錄每一次的輸入；
//! 秘密參照由 fake SecretResolver 解析。只斷言外部行為：桌面 API 的回應、Evidence Bundle 與證據檔案的內容、
//! Runner 實際收到的操作、judge 實際收到的輸入，以及資料目錄中的檔案。
//!
//! 每個測試名稱對應票 46 的驗收條件：
//! - 模型限流不被記為產品缺陷：`a_rate_limited_model_is_never_recorded_as_a_product_defect`
//!   （另見 `the_judge_sees_only_the_current_screen_and_the_fixed_expectation_not_the_workers_claim`）
//! - 秘密不出現在 Prompt 或證據中：`secrets_reach_only_the_runner_and_never_the_prompt_or_the_evidence`
//! - 測試 persona 與實際登入身分分開綁定：`test_personas_and_login_identities_are_bound_separately`
//! - 瀏覽器與環境問題不算產品缺陷：`browser_and_environment_problems_are_not_product_defects`

use std::path::Path;

use baton_domain::Timestamp;
use baton_harness::fakes::FakePage;
use baton_harness::Harness;
use baton_ports::{AdapterEvent, JudgeOutcome};
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

const WORKSPACES: &str = "/api/v1/workspaces";
const CLOCK_START: Timestamp = Timestamp::from_millis(1_700_000_000_123);
const SITE: &str = "http://127.0.0.1:3000";
const API: &str = "http://127.0.0.1:3000/api";

const ADMIN_PASSWORD: &str = "boss-密碼-9f2c";
const CLERK_PASSWORD: &str = "clerk-密碼-71ab";
const API_TOKEN: &str = "api-token-5d8e0c";

const SORTED: &str = "商品依價格由低到高排列";
const EDITABLE: &str = "畫面讓店長可以修改每個商品的價格";

// --- 共用輔助 ---

struct World {
    harness: Harness,
    ws: String,
    goal: String,
}

fn url(path: &str) -> String {
    format!("{SITE}{path}")
}

fn step(key: &str, operation: Value, assertions: Value) -> Value {
    json!({ "key": key, "title": format!("步驟 {key}"), "operation": operation, "assertions": assertions })
}

fn browser(action: Value) -> Value {
    json!({ "kind": "browser", "action": action })
}

fn goto(path: &str) -> Value {
    browser(json!({ "kind": "goto", "url": url(path) }))
}

fn click(selector: &str) -> Value {
    browser(json!({ "kind": "click", "selector": selector }))
}

fn fill(selector: &str, text: &str) -> Value {
    browser(json!({ "kind": "fill", "selector": selector, "text": text }))
}

/// 以帳號與秘密參照登入的步驟。
fn login(user: &str, secret: &str) -> Value {
    json!([
        step("open-login", goto("/login"), json!([{ "kind": "visible", "selector": "#username" }])),
        step("username", fill("#username", user), json!([{ "kind": "visible", "selector": "#password" }])),
        step("password", fill("#password", &format!("{{{{secret:{secret}}}}}")), json!([{ "kind": "visible", "selector": "#submit" }])),
        step("submit", click("#submit"), json!([{ "kind": "url_contains", "text": "/me" }])),
    ])
}

/// 網路商店的 web Profile：啟動服務、就緒、以 API 權杖準備資料；兩個 persona 與兩個登入身分分開宣告；
/// 兩條旅程（顧客依價格瀏覽、店長調整價格），各有語意判斷；每一步都截圖，並要求 trace 與錄影。
fn shop_profile() -> Value {
    let mut seed = json!({ "kind": "http", "method": "POST", "url": format!("{API}/seed") });
    seed["headers"] = json!([{ "name": "Authorization", "value": "Bearer {{secret:api-token}}" }]);
    json!({
        "interface": "web",
        "start": { "program": "shop", "args": ["serve", "--data", "{{state_dir}}"] },
        "readiness": {
            "step": step("health", json!({ "kind": "http", "method": "GET", "url": format!("{API}/health") }), json!([{ "kind": "status", "equals": 200 }])),
            "attempts": 2,
            "interval_ms": 100,
        },
        "setup": [step("seed", seed, json!([{ "kind": "status", "equals": 200 }]))],
        "secrets": [
            { "key": "admin-password", "reference": "env:SHOP_ADMIN_PASSWORD" },
            { "key": "clerk-password", "reference": "env:SHOP_CLERK_PASSWORD" },
            { "key": "api-token", "reference": "env:SHOP_API_TOKEN" },
        ],
        "personas": [
            { "key": "admin", "title": "店家管理員", "description": "負責調整商品價格的店長" },
            { "key": "shopper", "title": "顧客", "description": "想找便宜商品的顧客" },
        ],
        "identities": [
            {
                "key": "store-admin",
                "title": "店長帳號",
                "login": login("boss", "admin-password"),
                "check": step("whoami", goto("/me"), json!([{ "kind": "contains", "text": "角色：管理員" }])),
            },
            {
                "key": "clerk",
                "title": "店員帳號",
                "login": login("clerk01", "clerk-password"),
                "check": step("whoami", goto("/me"), json!([{ "kind": "contains", "text": "角色：店員" }])),
            },
        ],
        "journeys": [
            {
                "key": "browse",
                "title": "依價格瀏覽商品",
                "representative": true,
                "persona": "shopper",
                "steps": [
                    step("open", goto("/products"), json!([{ "kind": "visible", "selector": "#sort" }, { "kind": "contains", "text": "商品清單" }])),
                    step("sort", click("#sort"), json!([{ "kind": "url_contains", "text": "sort=price" }, { "kind": "judge", "expectation": SORTED }])),
                ],
            },
            {
                "key": "manage-prices",
                "title": "調整價格",
                "persona": "admin",
                "identity": "store-admin",
                "steps": [
                    step("open-admin", goto("/admin/prices"), json!([{ "kind": "contains", "text": "價格管理" }, { "kind": "judge", "expectation": EDITABLE }])),
                ],
            },
        ],
        "evidence": { "screenshots": "every_step", "trace": true, "video": true },
        "feature_map": [
            { "key": "catalog", "title": "商品目錄", "criterion": "可依價格排序", "journeys": ["browse"] },
            { "key": "pricing", "title": "價格管理", "criterion": "店長可以調整價格", "journeys": ["manage-prices"] },
        ],
    })
}

/// 這個 Session 最後輸入的帳號與密碼對應的角色（網站自己的事實，與 Profile 怎麼稱呼這個帳號無關）。
fn role_of(fills: &[(String, String)]) -> (&'static str, String) {
    let last = |selector: &str| fills.iter().rev().find(|(s, _)| s == selector).map(|(_, t)| t.clone()).unwrap_or_default();
    let (user, password) = (last("#username"), last("#password"));
    match (user.as_str(), password.as_str()) {
        ("boss", ADMIN_PASSWORD) => ("管理員", user),
        ("clerk01", CLERK_PASSWORD) => ("店員", user),
        _ => ("訪客", String::new()),
    }
}

fn error_code(body: &Value) -> &str {
    body["error"].as_str().unwrap_or_default()
}

fn step_in<'a>(bundle: &'a Value, journey: &str, key: &str) -> &'a Value {
    bundle["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["journey"] == journey && s["key"] == key)
        .unwrap_or_else(|| panic!("沒有旅程 {journey} 的步驟 {key}：{bundle}"))
}

fn journey_of<'a>(bundle: &'a Value, key: &str) -> &'a Value {
    bundle["journeys"].as_array().unwrap().iter().find(|j| j["key"] == key).unwrap_or_else(|| panic!("沒有旅程 {key}：{bundle}"))
}

/// 目錄下（遞迴）內容含有這段文字的檔案。
fn files_containing(dir: &Path, needle: &str) -> Vec<String> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else { return found };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(files_containing(&path, needle));
        } else if let Ok(bytes) = std::fs::read(&path) {
            if bytes.windows(needle.len()).any(|w| w == needle.as_bytes()) {
                found.push(path.display().to_string());
            }
        }
    }
    found
}

async fn world() -> World {
    world_with_account(true).await
}

async fn world_with_account(account: bool) -> World {
    let harness = Harness::builder().clock_at(CLOCK_START).start().await;
    if account {
        harness.add_account("帳號 A", "account0@example.com").await;
    }
    let dir = harness.data_dir().join("projects").join("shop");
    std::fs::create_dir_all(&dir).unwrap();
    let (status, body) = harness.send_json(Method::POST, WORKSPACES, Some(json!({ "path": dir.to_string_lossy() }))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let ws = body["id"].as_str().unwrap().to_string();
    let (status, goal) =
        harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/goals"), Some(json!({ "title": "網路商店" }))).await;
    assert_eq!(status, StatusCode::CREATED, "{goal}");
    let goal = goal["id"].as_str().unwrap().to_string();
    World { harness, ws, goal }
}

impl World {
    fn h(&self) -> &Harness {
        &self.harness
    }

    fn path(&self, rest: &str) -> String {
        format!("{WORKSPACES}/{}/verification{rest}", self.ws)
    }

    /// 服務、秘密與網站都正常：顧客看得到依價格排序的清單，店長登入後看得到價格管理，店員與訪客被拒絕。
    fn healthy(&self) {
        let driver = &self.h().verification_driver;
        driver.script_response("GET", &format!("{API}/health"), 200, "ok");
        driver.script_response("POST", &format!("{API}/seed"), 200, "seeded");
        let secrets = &self.h().secret_resolver;
        secrets.set("env:SHOP_ADMIN_PASSWORD", ADMIN_PASSWORD);
        secrets.set("env:SHOP_CLERK_PASSWORD", CLERK_PASSWORD);
        secrets.set("env:SHOP_API_TOKEN", API_TOKEN);
        let site = &self.h().browser_driver;
        site.script_page(&url("/login"), FakePage::new("登入", "請輸入帳號與密碼", &["#username", "#password", "#submit"]));
        site.script_link("#submit", &url("/me"));
        site.script_page_with(&url("/me"), |fills| {
            let (role, user) = role_of(fills);
            FakePage::new("我的帳號", &format!("登入者：{user}　角色：{role}"), &[])
        });
        site.script_page_with(&url("/admin/prices"), |fills| match role_of(fills).0 {
            "管理員" => FakePage::new("價格管理", "價格管理：A 100 元［修改］、B 200 元［修改］", &[]),
            _ => FakePage::new("權限不足", "權限不足", &[]).with_status(403),
        });
        site.script_page(&url("/products"), FakePage::new("商品", "商品清單：B 200 元、A 100 元", &["#sort"]));
        site.script_link("#sort", &url("/products?sort=price"));
        site.script_page(&url("/products?sort=price"), FakePage::new("商品", "商品清單：A 100 元、B 200 元", &["#sort"]));
        let judge = &self.h().semantic_judge;
        judge.script(SORTED, vec![JudgeOutcome::Decided { satisfied: true, rationale: "A 100 元在 B 200 元之前".into() }]);
        judge.script(EDITABLE, vec![JudgeOutcome::Decided { satisfied: true, rationale: "每個商品旁有［修改］".into() }]);
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

    /// 從桌面要求並執行一次驗證，回傳 Evidence Bundle。
    async fn verify(&self, body: Value) -> Value {
        let (status, request) = self.h().send_json(Method::POST, &self.path("/runs"), Some(body)).await;
        assert_eq!(status, StatusCode::CREATED, "{request}");
        self.h().supervise().await;
        let overview = self.overview().await;
        let done = overview["requests"].as_array().unwrap().iter().find(|r| r["id"] == request["id"]).unwrap().clone();
        assert_eq!(done["status"], "completed", "驗證要求應該已執行：{overview}");
        let id = done["bundle_id"].as_str().unwrap();
        let (status, bundle) = self.h().send_json(Method::GET, &self.path(&format!("/bundles/{id}")), None).await;
        assert_eq!(status, StatusCode::OK, "{bundle}");
        bundle
    }

    /// 讀取 Bundle 中第 `index` 個證據檔案。
    async fn artifact(&self, bundle: &Value, index: usize) -> (StatusCode, Value) {
        let path = self.path(&format!("/bundles/{}/artifacts/{index}", bundle["id"].as_str().unwrap()));
        self.h().send_json(Method::GET, &path, None).await
    }

    fn judge_calls(&self) -> usize {
        self.h().semantic_judge.requests().len()
    }
}

fn artifacts_of<'a>(bundle: &'a Value, kind: &str) -> Vec<&'a Value> {
    bundle["artifacts"].as_array().unwrap().iter().filter(|a| a["kind"] == kind).collect()
}

// --- 驗收條件：模型限流不被記為產品缺陷 ---

#[tokio::test]
async fn a_rate_limited_model_is_never_recorded_as_a_product_defect() {
    let w = world().await;
    w.publish(shop_profile()).await;
    w.healthy();

    // 模型限流：程式斷言都通過，只有語意判斷沒有結果 → 證據不足，不是產品缺陷
    w.h().semantic_judge.script(SORTED, vec![JudgeOutcome::RateLimited { reset_at_millis: Some(1_700_003_600_000) }]);
    let bundle = w.verify(json!({})).await;
    assert_eq!((bundle["verdict"].as_str(), bundle["reason_code"].as_str()), (Some("inconclusive"), Some("judge_rate_limited")), "{bundle}");
    let sort = step_in(&bundle, "browse", "sort");
    assert_eq!((sort["verdict"].as_str(), sort["engine_code"].as_str()), (Some("inconclusive"), Some("model:rate_limited")));
    assert_eq!(sort["assertions"][0]["passed"], true, "程式斷言照常判定：{sort}");
    assert!(sort["assertions"][1]["actual"].as_str().unwrap().contains("限流"), "{sort}");
    assert_eq!(journey_of(&bundle, "browse")["verdict"], "inconclusive");
    // 同一次驗證中已經限流，之後的語意判斷不再呼叫模型，也一樣是證據不足
    assert_eq!(w.judge_calls(), 1, "限流之後不再呼叫模型");
    let admin = step_in(&bundle, "manage-prices", "open-admin");
    assert_eq!((admin["verdict"].as_str(), admin["reason_code"].as_str()), (Some("inconclusive"), Some("judge_rate_limited")));

    // 限流的同時程式斷言明確不符：缺陷由程式斷言證明，仍是產品缺陷
    w.healthy();
    w.h().semantic_judge.script(SORTED, vec![JudgeOutcome::RateLimited { reset_at_millis: None }]);
    w.h().browser_driver.script_link("#sort", &url("/products?sort=name"));
    w.h().browser_driver.script_page(&url("/products?sort=name"), FakePage::new("商品", "商品清單：A 100 元、B 200 元", &["#sort"]));
    let bundle = w.verify(json!({ "journeys": ["browse"] })).await;
    assert_eq!((bundle["verdict"].as_str(), bundle["reason_code"].as_str()), (Some("product_failure"), Some("assertion_failed")));

    // 模型明確判定不符才是產品缺陷
    w.healthy();
    w.h().semantic_judge.script(SORTED, vec![JudgeOutcome::Decided { satisfied: false, rationale: "B 200 元排在 A 100 元前面".into() }]);
    w.h().browser_driver.script_page(&url("/products?sort=price"), FakePage::new("商品", "商品清單：B 200 元、A 100 元", &["#sort"]));
    let bundle = w.verify(json!({ "journeys": ["browse"] })).await;
    assert_eq!(bundle["verdict"], "product_failure", "{bundle}");
    assert_eq!(step_in(&bundle, "browse", "sort")["assertions"][1]["actual"], "B 200 元排在 A 100 元前面");

    // 模型無法使用、模型呼叫預算用盡：也都不是產品缺陷
    w.healthy();
    w.h().semantic_judge.script(SORTED, vec![JudgeOutcome::Unavailable { reason: "Not logged in".into() }]);
    let bundle = w.verify(json!({ "journeys": ["browse"] })).await;
    assert_eq!((bundle["verdict"].as_str(), bundle["reason_code"].as_str()), (Some("inconclusive"), Some("judge_unavailable")));
    let mut one_call = shop_profile();
    one_call["budget"] = json!({ "max_model_calls": 1 });
    w.publish(one_call).await;
    w.healthy();
    let calls = w.judge_calls();
    let bundle = w.verify(json!({})).await;
    assert_eq!(w.judge_calls(), calls + 1, "預算只允許一次模型呼叫");
    assert_eq!((bundle["verdict"].as_str(), bundle["reason_code"].as_str()), (Some("budget_exhausted"), Some("model_call_budget_exhausted")));

    // 沒有限流也沒有程式斷言不符的那幾份，沒有一份被記為產品缺陷
    let verdicts: Vec<String> =
        w.overview().await["bundles"].as_array().unwrap().iter().map(|b| b["verdict"].as_str().unwrap().to_string()).collect();
    assert_eq!(verdicts, ["budget_exhausted", "inconclusive", "product_failure", "product_failure", "inconclusive"]);
}

#[tokio::test]
async fn without_a_usable_account_the_judge_is_unavailable_not_a_failure() {
    let w = world_with_account(false).await;
    w.publish(shop_profile()).await;
    w.healthy();
    let bundle = w.verify(json!({ "journeys": ["browse"] })).await;
    assert_eq!((bundle["verdict"].as_str(), bundle["reason_code"].as_str()), (Some("inconclusive"), Some("judge_unavailable")));
    assert_eq!(w.judge_calls(), 0, "沒有可用帳號時不呼叫模型");
}

#[tokio::test]
async fn the_judge_sees_only_the_current_screen_and_the_fixed_expectation_not_the_workers_claim() {
    let w = world().await;
    w.publish(shop_profile()).await;
    w.healthy();
    // Worker 宣稱已完成排序（敘述不是證據）
    let body = json!({ "goal_id": w.goal, "title": "商品依價格排序", "kind": "research", "contract": {
        "objective": { "status": "filled", "text": "商品清單可以依價格排序" },
        "inputs": { "status": "filled", "text": "既有的商品頁" },
        "scope": { "status": "filled", "text": "商品頁" },
        "deliverables": { "status": "filled", "text": "排序按鈕" },
        "done_criteria": { "status": "filled", "text": "依價格由低到高排列" },
        "blocker_handling": { "status": "filled", "text": "卡住時回報" },
        "required_capabilities": [],
    }});
    let (status, task) = w.h().send_json(Method::POST, &format!("{WORKSPACES}/{}/tasks", w.ws), Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    let tid = task["id"].as_str().unwrap().to_string();
    let (status, _) = w.h().send_json(Method::POST, &format!("{WORKSPACES}/{}/tasks/{tid}/ready", w.ws), None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, run) = w.h().send_json(Method::POST, &format!("{WORKSPACES}/{}/tasks/{tid}/dispatch", w.ws), None).await;
    assert_eq!(status, StatusCode::CREATED, "{run}");
    let rid = run["id"].as_str().unwrap().to_string();
    w.h().supervise().await;
    let worker = w.h().agent_credential(&rid).unwrap();
    let claim = "已完成：商品已依價格由低到高排列，我親自確認過";
    let (status, body) = w.h().call_tool(&worker, "report_progress", json!({ "summary": claim })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    w.h().agent_adapter.emit(&rid, AdapterEvent::Exited { success: true });
    w.h().supervise().await;

    // 真實畫面並沒有排序
    w.h().browser_driver.script_page(&url("/products?sort=price"), FakePage::new("商品", "商品清單：B 200 元、A 100 元", &["#sort"]));
    w.h().semantic_judge.script(SORTED, vec![JudgeOutcome::Decided { satisfied: false, rationale: "沒有依價格排列".into() }]);
    let bundle = w.verify(json!({ "task_id": tid, "journeys": ["browse"] })).await;
    assert_eq!(bundle["verdict"], "product_failure", "{bundle}");

    let requests = w.h().semantic_judge.requests();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.expectation, SORTED, "預期取自 Profile");
    assert_eq!(request.page_url, url("/products?sort=price"));
    assert_eq!(request.page_text, "商品清單：B 200 元、A 100 元", "輸入取自當前畫面");
    assert!(request.persona.as_deref().unwrap().contains("想找便宜商品的顧客"), "persona 只是領域背景：{request:?}");
    for field in [&request.expectation, &request.page_text, &request.page_title, request.persona.as_ref().unwrap()] {
        assert!(!field.contains("親自確認"), "行動者的敘述不能進入判斷：{request:?}");
    }
    assert!(request.account.config_dir.to_string_lossy().contains("profiles"), "{request:?}");
}

// --- 驗收條件：秘密不出現在 Prompt 或證據中 ---

#[tokio::test]
async fn secrets_reach_only_the_runner_and_never_the_prompt_or_the_evidence() {
    let w = world().await;
    let profile = w.publish(shop_profile()).await;
    w.healthy();
    // 有缺陷的網站：登入後把密碼印在畫面上
    w.h().browser_driver.script_page_with(&url("/me"), |fills| {
        let (role, user) = role_of(fills);
        let password = fills.iter().rev().find(|(s, _)| s == "#password").map(|(_, t)| t.clone()).unwrap_or_default();
        FakePage::new("我的帳號", &format!("登入者：{user}　角色：{role}（除錯：密碼 {password}）"), &[])
    });
    w.h().browser_driver.script_page_with(&url("/admin/prices"), |fills| {
        let password = fills.iter().rev().find(|(s, _)| s == "#password").map(|(_, t)| t.clone()).unwrap_or_default();
        FakePage::new("價格管理", &format!("價格管理：A 100 元［修改］ 目前密碼 {password}"), &[])
    });
    // 顧客旅程沒有輸入任何秘密，但排序後的頁面顯示了 API 權杖
    w.h().browser_driver.script_page(
        &url("/products?sort=price"),
        FakePage::new("商品", &format!("商品清單：A 100 元、B 200 元（除錯：權杖 {API_TOKEN}）"), &["#sort"]),
    );

    let bundle = w.verify(json!({})).await;
    assert_eq!(bundle["verdict"], "passed", "{bundle}");

    // 秘密的值確實交給了 Runner（登入表單與 API 標頭）
    let commands = w.h().browser_driver.commands();
    assert!(commands.iter().any(|c| c.starts_with("fill ") && c.ends_with(&format!("#password {ADMIN_PASSWORD}"))), "{commands:?}");
    let sent = w.h().verification_driver.http_requests();
    assert!(sent.iter().any(|r| r.headers.contains(&("Authorization".into(), format!("Bearer {API_TOKEN}")))));

    // 但不在 Prompt 中：judge 收到的畫面已遮蔽
    for request in w.h().semantic_judge.requests() {
        let prompt = format!("{request:?}");
        assert!(!prompt.contains(ADMIN_PASSWORD) && !prompt.contains(API_TOKEN), "{prompt}");
    }
    let admin_request = w.h().semantic_judge.requests().into_iter().find(|r| r.expectation == EDITABLE).unwrap();
    assert!(admin_request.page_text.contains("[已遮蔽]"), "{admin_request:?}");

    // 也不在證據中：Bundle 只有參照與遮蔽後的文字
    let text = bundle.to_string();
    for secret in [ADMIN_PASSWORD, CLERK_PASSWORD, API_TOKEN] {
        assert!(!text.contains(secret), "Bundle 含有秘密：{text}");
    }
    let password = step_in(&bundle, "manage-prices", "password");
    assert_eq!((password["phase"].as_str(), password["verdict"].as_str()), (Some("login"), Some("passed")));
    assert!(password["operation"].as_str().unwrap().contains("{{secret:admin-password}}"), "{password}");
    assert!(step_in(&bundle, "manage-prices", "whoami")["output"].as_str().unwrap().contains("[已遮蔽]"));

    // 證據檔案（截圖、trace、錄影的內容都是不透明的位元組，畫面文字在其中找不到）：
    // 畫面顯示秘密的截圖不保存；輸入過秘密的 Session 的 trace 與錄影不保存；其他截圖保存，並遮住密碼欄位
    let screenshot = |journey: &str, step: &str| {
        artifacts_of(&bundle, "screenshot").into_iter().find(|a| a["journey"] == journey && a["step"] == step).unwrap().clone()
    };
    let recording = |kind: &str, journey: &str| artifacts_of(&bundle, kind).into_iter().find(|a| a["journey"] == journey).unwrap().clone();
    let whoami = screenshot("manage-prices", "whoami");
    assert_eq!(whoami["withheld"], "secret_detected", "{whoami}");
    assert!(whoami["storage_path"].is_null());
    assert_eq!(recording("trace", "manage-prices")["withheld"], "secret_input_recorded");
    assert_eq!(recording("video", "manage-prices")["withheld"], "secret_input_recorded");
    // 沒有輸入秘密、但畫面顯示過秘密的 Session（顧客瀏覽）：那一步的截圖與整個 Session 的 trace、錄影都不保存
    let sorted = screenshot("browse", "sort");
    assert_eq!(sorted["withheld"], "secret_detected", "{sorted}");
    assert!(screenshot("browse", "open")["storage_path"].is_string(), "顯示秘密之前的截圖照常保存：{bundle}");
    for kind in ["trace", "video"] {
        let file = recording(kind, "browse");
        assert_eq!(file["withheld"], "secret_detected", "{file}");
        assert!(file["storage_path"].is_null());
    }
    let masks = w.h().browser_driver.screenshot_masks();
    assert!(masks.last().unwrap().contains(&"#password".to_string()), "輸入過秘密的欄位在之後的截圖都遮住：{masks:?}");
    assert!(!masks[0].contains(&"#password".to_string()));

    // 證據檔案可以讀取；不保存的讀不到
    let artifacts = bundle["artifacts"].as_array().unwrap();
    let stored = artifacts.iter().position(|a| a["kind"] == "screenshot" && a["storage_path"].is_string()).unwrap();
    let (status, content) = w.artifact(&bundle, stored).await;
    assert_eq!(status, StatusCode::OK, "{content}");
    assert_eq!((content["kind"].as_str(), content["content_type"].as_str()), (Some("screenshot"), Some("image/png")));
    assert!(!content["content_base64"].as_str().unwrap().is_empty());
    let withheld = artifacts.iter().position(|a| a["withheld"].is_string()).unwrap();
    let (status, body) = w.artifact(&bundle, withheld).await;
    assert_eq!((status, error_code(&body)), (StatusCode::NOT_FOUND, "evidence_artifact_not_found"), "{body}");
    let (status, body) = w.artifact(&bundle, artifacts.len()).await;
    assert_eq!((status, error_code(&body)), (StatusCode::NOT_FOUND, "evidence_artifact_not_found"), "{body}");

    // Profile 只保存參照；資料目錄中任何檔案（資料庫、證據檔案）都沒有秘密的值
    assert_eq!(profile["definition"]["secrets"][0]["reference"], "env:SHOP_ADMIN_PASSWORD");
    for secret in [ADMIN_PASSWORD, CLERK_PASSWORD, API_TOKEN] {
        assert_eq!(files_containing(w.h().data_dir(), secret), Vec::<String>::new(), "資料目錄含有秘密 {secret}");
    }

    // 沒有輸入也沒有顯示秘密的 Session：截圖、trace 與錄影照常保存
    w.healthy();
    let clean = w.verify(json!({ "journeys": ["browse"] })).await;
    for file in clean["artifacts"].as_array().unwrap() {
        assert!(file["withheld"].is_null() && file["storage_path"].is_string(), "{file}");
    }
    assert_eq!(artifacts_of(&clean, "trace").len() + artifacts_of(&clean, "video").len(), 2, "{clean}");

    // 解析不到的秘密：不執行那個操作，判定為環境阻礙
    let mut missing = shop_profile();
    missing["secrets"][1]["reference"] = json!("env:SHOP_MISSING_PASSWORD");
    missing["journeys"][1]["identity"] = json!("clerk");
    w.publish(missing).await;
    w.healthy();
    let before = w.h().browser_driver.commands().len();
    let bundle = w.verify(json!({ "journeys": ["manage-prices"] })).await;
    let password = step_in(&bundle, "manage-prices", "password");
    assert_eq!((password["verdict"].as_str(), password["reason_code"].as_str()), (Some("environment_blocked"), Some("secret_unavailable")));
    assert_eq!(bundle["verdict"], "environment_blocked");
    let after = w.h().browser_driver.commands();
    assert!(!after[before..].iter().any(|c| c.contains("#password")), "沒有秘密就不輸入：{after:?}");
}

// --- 驗收條件：測試 persona 與實際登入身分分開綁定 ---

#[tokio::test]
async fn test_personas_and_login_identities_are_bound_separately() {
    let w = world().await;
    w.publish(shop_profile()).await;
    w.healthy();

    // persona 與登入身分是兩個名稱空間：旅程不能把 persona 當成登入身分
    let mut persona_as_identity = shop_profile();
    persona_as_identity["journeys"][1]["identity"] = json!("admin");
    let (status, body) = w.h().send_json(Method::POST, &w.path("/profile"), Some(persona_as_identity)).await;
    assert_eq!((status, error_code(&body)), (StatusCode::UNPROCESSABLE_ENTITY, "verification_profile_invalid"), "{body}");
    // 登入身分必須有自動的身分核對
    let mut no_check = shop_profile();
    no_check["identities"][0]["check"] = json!({ "key": "whoami", "operation": { "kind": "manual", "instruction": "看右上角" } });
    let (status, body) = w.h().send_json(Method::POST, &w.path("/profile"), Some(no_check)).await;
    assert_eq!((status, error_code(&body)), (StatusCode::UNPROCESSABLE_ENTITY, "verification_profile_invalid"), "{body}");

    // 正常：店長 persona 以店長帳號登入，身分核對通過才執行旅程
    let bundle = w.verify(json!({})).await;
    assert_eq!(bundle["verdict"], "passed", "{bundle}");
    let manage = journey_of(&bundle, "manage-prices");
    assert_eq!((manage["persona"].as_str(), manage["identity"].as_str()), (Some("admin"), Some("store-admin")));
    let browse = journey_of(&bundle, "browse");
    assert_eq!((browse["persona"].as_str(), browse["identity"].as_str()), (Some("shopper"), None));
    let phases: Vec<&str> = bundle["steps"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["journey"] == "manage-prices")
        .map(|s| s["phase"].as_str().unwrap())
        .collect();
    assert_eq!(phases, ["login", "login", "login", "login", "identity", "journey"]);
    // 每條旅程一個隔離的瀏覽器 Session；顧客旅程沒有登入
    let commands = w.h().browser_driver.commands();
    let opened: Vec<&String> = commands.iter().filter(|c| c.starts_with("open ")).collect();
    assert_eq!(opened.len(), 2, "{commands:?}");
    assert_eq!(commands.iter().filter(|c| c.starts_with("fill ")).count(), 2, "只有店長旅程輸入帳密：{commands:?}");
    assert_eq!(w.h().browser_driver.open_sessions(), 0, "Session 都已關閉");

    // persona 叫 admin 不代表有管理員身分：同一個 persona 綁到店員帳號，旅程以店員身分執行並被網站拒絕 → 授權阻礙
    let mut clerk = shop_profile();
    clerk["journeys"][1]["identity"] = json!("clerk");
    clerk["identities"][1]["check"]["assertions"] = json!([{ "kind": "contains", "text": "角色：店員" }]);
    w.publish(clerk).await;
    w.healthy();
    let bundle = w.verify(json!({ "journeys": ["manage-prices"] })).await;
    let manage = journey_of(&bundle, "manage-prices");
    assert_eq!((manage["persona"].as_str(), manage["identity"].as_str()), (Some("admin"), Some("clerk")));
    assert_eq!((bundle["verdict"].as_str(), bundle["reason_code"].as_str()), (Some("auth_blocked"), Some("http_forbidden")), "{bundle}");
    assert!(w.h().browser_driver.commands().iter().any(|c| c.ends_with(&format!("#password {CLERK_PASSWORD}"))));

    // 登入身分名為店長，但這組帳密實際登入的是店員：身分核對沒有通過 → 授權阻礙，旅程不執行
    let mut mislabeled = shop_profile();
    mislabeled["secrets"][0]["reference"] = json!("env:SHOP_CLERK_PASSWORD");
    mislabeled["identities"][0]["login"] = login("clerk01", "admin-password");
    w.publish(mislabeled).await;
    w.healthy();
    let editable_calls = || w.h().semantic_judge.requests().iter().filter(|r| r.expectation == EDITABLE).count();
    let judge_calls = editable_calls();
    let bundle = w.verify(json!({ "journeys": ["manage-prices"] })).await;
    assert_eq!((bundle["verdict"].as_str(), bundle["reason_code"].as_str()), (Some("auth_blocked"), Some("identity_not_confirmed")), "{bundle}");
    let whoami = step_in(&bundle, "manage-prices", "whoami");
    assert_eq!(whoami["phase"], "identity");
    assert!(whoami["output"].as_str().unwrap().contains("角色：店員"), "證據保存實際登入的身分：{whoami}");
    let open_admin = step_in(&bundle, "manage-prices", "open-admin");
    assert_eq!((open_admin["attempts"].as_i64(), open_admin["reason_code"].as_str()), (Some(0), Some("blocked_by_identity")));
    assert_eq!(editable_calls(), judge_calls, "身分沒有確認就不做旅程中的判斷");

    // 登入失敗（帳密錯誤，登入後仍在登入頁）→ 授權阻礙
    let mut wrong = shop_profile();
    wrong["identities"][0]["login"][3]["assertions"] = json!([{ "kind": "url_contains", "text": "/dashboard" }]);
    w.publish(wrong).await;
    w.healthy();
    let bundle = w.verify(json!({ "journeys": ["manage-prices"] })).await;
    assert_eq!((bundle["verdict"].as_str(), bundle["reason_code"].as_str()), (Some("auth_blocked"), Some("login_failed")), "{bundle}");
    assert_eq!(step_in(&bundle, "manage-prices", "whoami")["reason_code"], "blocked_by_login");
}

// --- 瀏覽器與環境問題不算產品缺陷 ---

#[tokio::test]
async fn browser_and_environment_problems_are_not_product_defects() {
    let w = world().await;
    w.publish(shop_profile()).await;

    // 瀏覽器無法啟動 → 環境阻礙；旅程不執行
    w.healthy();
    w.h().browser_driver.fail_open(Some("找不到受驗專案的 Playwright"));
    let bundle = w.verify(json!({ "journeys": ["browse"] })).await;
    assert_eq!((bundle["verdict"].as_str(), bundle["reason_code"].as_str()), (Some("environment_blocked"), Some("browser_unavailable")), "{bundle}");
    assert_eq!(step_in(&bundle, "browse", "open")["attempts"], 0);
    w.h().browser_driver.fail_open(None);

    // 網站連不上 → 環境阻礙
    w.healthy();
    w.h().browser_driver.script_link("#sort", "http://127.0.0.1:3999/down");
    let bundle = w.verify(json!({ "journeys": ["browse"] })).await;
    assert_eq!((bundle["verdict"].as_str(), bundle["reason_code"].as_str()), (Some("environment_blocked"), Some("endpoint_unreachable")));

    // 找不到要操作的元素 → 證據不足（不是產品缺陷）；失敗步驟的截圖保存
    w.healthy();
    w.h().browser_driver.script_page(&url("/products"), FakePage::new("商品", "商品清單：B 200 元、A 100 元", &[]));
    let mut on_failure = shop_profile();
    on_failure["evidence"] = json!({});
    on_failure["journeys"][0]["steps"][0]["assertions"] = json!([{ "kind": "contains", "text": "商品清單" }]);
    w.publish(on_failure).await;
    let bundle = w.verify(json!({ "journeys": ["browse"] })).await;
    assert_eq!((bundle["verdict"].as_str(), bundle["reason_code"].as_str()), (Some("inconclusive"), Some("action_failed")), "{bundle}");
    let sort = step_in(&bundle, "browse", "sort");
    assert_eq!((sort["verdict"].as_str(), sort["engine_code"].as_str()), (Some("inconclusive"), Some("playwright:TimeoutError")), "{sort}");
    let screenshots = artifacts_of(&bundle, "screenshot");
    assert_eq!(screenshots.len(), 1, "預設只保存未通過步驟的截圖：{screenshots:?}");
    assert_eq!(screenshots[0]["step"], "sort");
    assert!(artifacts_of(&bundle, "trace").is_empty() && artifacts_of(&bundle, "video").is_empty(), "預設不錄 trace 與錄影");
    assert_eq!(w.h().browser_driver.open_sessions(), 0);

    // CLI／API Profile 不能使用瀏覽器操作
    let mut cli = shop_profile();
    cli["interface"] = json!("cli_api");
    let (status, body) = w.h().send_json(Method::POST, &w.path("/profile"), Some(cli)).await;
    assert_eq!((status, error_code(&body)), (StatusCode::UNPROCESSABLE_ENTITY, "verification_profile_invalid"), "{body}");
}
