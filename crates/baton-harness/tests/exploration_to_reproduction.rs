//! 情境測試：探索測試到可重現缺陷（票 47，issue #3 §10.3），經由 Runtime harness 的真實 HTTP API 驗證。
//!
//! 受驗的「真實介面」由 fake VerificationDriver 扮演（HTTP 端點的回應由測試腳本化；沒有腳本的端點連不上）。
//! Controller 與 Worker 是腳本化的。只斷言外部行為：工具與桌面 API 的回應、探索概況、Evidence Bundle、
//! Worker 拿到的 Prompt，以及控制資料庫拒絕的直接竄改。
//!
//! 每個測試名稱對應票 47 的驗收條件：
//! - 環境故障不被記為產品缺陷：`an_environment_failure_is_never_recorded_as_a_product_defect`
//! - 未經重現的疑點不會顯示為 confirmed：`a_suspicion_is_only_shown_as_confirmed_after_a_reproduction_fails_on_a_product_assertion`
//! - 不能以排除標籤或刪除案例掩蓋缺陷：`a_defect_cannot_be_hidden_by_exclusion_labels_or_by_dropping_its_case`
//! - 探索先定章程、無法重現保留為未驗風險、預期設計由使用者裁定：
//!   `exploration_starts_from_a_charter_and_unreproducible_or_expected_findings_are_kept_with_their_reason`

use std::path::PathBuf;

use baton_domain::Timestamp;
use baton_harness::Harness;
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

const WORKSPACES: &str = "/api/v1/workspaces";
const PLAYBOOKS: &str = "/api/v1/playbooks";
const CLOCK_START: Timestamp = Timestamp::from_millis(1_700_000_000_123);
/// 合併視窗（spec D-31）。
const WINDOW_MS: i64 = 5_000;
const API: &str = "http://127.0.0.1:8080";

// --- 共用輔助 ---

struct World {
    harness: Harness,
    ws: String,
    goal: String,
    #[allow(dead_code)]
    dir: PathBuf,
}

fn filled(text: &str) -> Value {
    json!({ "status": "filled", "text": text })
}

fn contract() -> Value {
    json!({
        "objective": filled("探索結帳流程，找出金額計算的問題"),
        "inputs": filled("商店的測試環境"),
        "scope": filled("結帳流程"),
        "deliverables": filled("疑點清單"),
        "done_criteria": filled("章程範圍都探索過"),
        "blocker_handling": filled("卡住時回報"),
        "required_capabilities": [],
    })
}

fn step(key: &str, operation: Value, assertions: Value) -> Value {
    json!({ "key": key, "title": format!("步驟 {key}"), "operation": operation, "assertions": assertions })
}

fn http(method: &str, path: &str) -> Value {
    json!({ "kind": "http", "method": method, "url": format!("{API}{path}") })
}

/// 商店的 Profile：就緒條件、一條代表旅程（瀏覽商品）與 Feature Map。
fn profile() -> Value {
    json!({
        "interface": "cli_api",
        "readiness": {
            "step": step("health", http("GET", "/health"), json!([{ "kind": "status", "equals": 200 }])),
            "attempts": 2,
            "interval_ms": 100,
        },
        "journeys": [{
            "key": "browse",
            "title": "瀏覽商品",
            "representative": true,
            "steps": [step("items", http("GET", "/items"), json!([{ "kind": "status", "equals": 200 }, { "kind": "contains", "text": "商品" }]))],
        }],
        "feature_map": [
            { "key": "catalog", "title": "商品目錄", "criterion": "可以瀏覽商品", "journeys": ["browse"] },
            { "key": "checkout", "title": "結帳", "criterion": "折扣後總額正確", "journeys": [] },
        ],
    })
}

/// 對應原症狀的重現案例：套用折扣後，總額應該是 180。
fn case() -> Value {
    json!({
        "key": "discount-total",
        "title": "折扣後總額",
        "steps": [step("total", http("GET", "/cart/total"), json!([
            { "kind": "status", "equals": 200 },
            { "kind": "json_equals", "pointer": "/total", "value": 180 },
        ]))],
    })
}

fn charter(task: &str) -> Value {
    json!({
        "task_id": task,
        "area": "結帳流程",
        "role": "一般顧客",
        "goal": "找出金額計算錯誤",
        "forbidden_side_effects": ["不得送出真實付款"],
        "max_steps": 40,
        "max_model_calls": 3,
    })
}

fn finding_report() -> Value {
    json!({
        "title": "折扣後總額少算",
        "feature": "checkout",
        "steps": ["加入兩件商品", "套用折扣碼 SAVE10", "查看總額"],
        "expected": "總額為 180",
        "actual": "總額顯示 160",
        "environment": "測試環境，seed 資料 v3",
    })
}

fn error_code(body: &Value) -> &str {
    body["error"].as_str().unwrap_or_default()
}

fn journey_of<'a>(bundle: &'a Value, key: &str) -> Option<&'a Value> {
    bundle["journeys"].as_array().unwrap().iter().find(|j| j["key"] == key)
}

async fn world() -> World {
    let harness = Harness::builder().clock_at(CLOCK_START).start().await;
    harness.add_account("帳號 A", "account0@example.com").await;
    let dir = harness.data_dir().join("projects").join("shop");
    std::fs::create_dir_all(&dir).unwrap();
    let (status, body) = harness.send_json(Method::POST, WORKSPACES, Some(json!({ "path": dir.to_string_lossy() }))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let ws = body["id"].as_str().unwrap().to_string();
    let (status, goal) =
        harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/goals"), Some(json!({ "title": "商店" }))).await;
    assert_eq!(status, StatusCode::CREATED, "{goal}");
    let goal = goal["id"].as_str().unwrap().to_string();
    let w = World { harness, ws, goal, dir };
    let (status, body) = w.h().send_json(Method::POST, &w.verification("/profile"), Some(profile())).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    w
}

/// 已經回報了一個疑點的探索：(World, Controller 憑證, Worker 憑證, 探索 Task, 疑點)。
async fn explored() -> (World, String, String, String, Value) {
    let w = world().await;
    let task = w.task("探索結帳").await;
    let controller = w.controller_turn("探索結帳流程").await;
    let (status, body) = w.h().call_tool(&controller, "define_exploration_charter", charter(&task)).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let (_, worker) = w.dispatch(&task).await;
    let (status, finding) = w.h().call_tool(&worker, "report_exploration_finding", finding_report()).await;
    assert_eq!(status, StatusCode::CREATED, "{finding}");
    (w, controller, worker, task, finding)
}

impl World {
    fn h(&self) -> &Harness {
        &self.harness
    }

    fn verification(&self, rest: &str) -> String {
        format!("{WORKSPACES}/{}/verification{rest}", self.ws)
    }

    /// 商店正常運作；`total` 是購物車 API 回報的總額。
    fn shop(&self, total: i64) {
        let driver = &self.h().verification_driver;
        driver.script_response("GET", &format!("{API}/health"), 200, "ok");
        driver.script_response("GET", &format!("{API}/items"), 200, r#"[{"name":"商品 A"}]"#);
        driver.script_response("GET", &format!("{API}/cart/total"), 200, &format!(r#"{{"total":{total}}}"#));
    }

    async fn task(&self, title: &str) -> String {
        let body = json!({ "goal_id": self.goal, "title": title, "kind": "research", "contract": contract() });
        let (status, task) = self.h().send_json(Method::POST, &format!("{WORKSPACES}/{}/tasks", self.ws), Some(body)).await;
        assert_eq!(status, StatusCode::CREATED, "{task}");
        let tid = task["id"].as_str().unwrap().to_string();
        let (status, ready) =
            self.h().send_json(Method::POST, &format!("{WORKSPACES}/{}/tasks/{tid}/ready", self.ws), None).await;
        assert_eq!(status, StatusCode::OK, "{ready}");
        tid
    }

    /// 派工並讓 Supervisor 啟動它，回傳 (run_id, Worker 的 Agent 憑證)。
    async fn dispatch(&self, tid: &str) -> (String, String) {
        let path = format!("{WORKSPACES}/{}/tasks/{tid}/dispatch", self.ws);
        let (status, run) = self.h().send_json(Method::POST, &path, None).await;
        assert_eq!(status, StatusCode::CREATED, "{run}");
        let rid = run["id"].as_str().unwrap().to_string();
        self.h().supervise().await;
        let token = self.h().agent_credential(&rid).expect("Run 啟動時應該拿到 Agent 憑證");
        (rid, token)
    }

    async fn controller_turn(&self, text: &str) -> String {
        let path = format!("{WORKSPACES}/{}/conversation/messages", self.ws);
        let (status, body) = self.h().send_json(Method::POST, &path, Some(json!({ "text": text }))).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        self.h().clock.advance_millis(WINDOW_MS);
        self.h().supervise().await;
        let (status, view) = self.h().send_json(Method::GET, &format!("{WORKSPACES}/{}/conversation", self.ws), None).await;
        assert_eq!(status, StatusCode::OK, "{view}");
        let run = view["runs"].as_array().unwrap().last().cloned().unwrap_or_else(|| panic!("Controller 沒有被喚起：{view}"));
        self.h().agent_credential(run["id"].as_str().unwrap()).expect("Controller 程序啟動時應該收到 Controller 憑證")
    }

    /// 桌面的探索概況。
    async fn exploration(&self) -> Value {
        let (status, body) = self.h().send_json(Method::GET, &format!("{WORKSPACES}/{}/exploration", self.ws), None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body
    }

    async fn finding(&self, id: &Value) -> Value {
        let overview = self.exploration().await;
        overview["findings"].as_array().unwrap().iter().find(|f| f["id"] == *id).cloned().unwrap_or_else(|| panic!("疑點 {id} 不見了：{overview}"))
    }

    /// Controller 要求重現（或修復驗證），讓 Supervisor 執行，回傳 (重現紀錄, Evidence Bundle, 疑點)。
    async fn reproduce(&self, controller: &str, finding: &Value, journey: Option<Value>) -> (Value, Value, Value) {
        let mut body = json!({ "finding_id": finding["id"] });
        if let Some(journey) = journey {
            body["journey"] = journey;
        }
        let (status, queued) = self.h().call_tool(controller, "reproduce_exploration_finding", body).await;
        assert_eq!(status, StatusCode::CREATED, "{queued}");
        assert_eq!(queued["attempt"]["outcome"], Value::Null, "結果出來之前是排隊中");
        self.h().supervise().await;
        let current = self.finding(&finding["id"]).await;
        let attempt = current["attempts"][0].clone();
        assert_eq!(attempt["request_id"], queued["request"]["id"], "{current}");
        let bundle = self.bundle(attempt["bundle_id"].as_str().expect("重現應該已執行")).await;
        (attempt, bundle, current)
    }

    async fn bundle(&self, id: &str) -> Value {
        let (status, bundle) = self.h().send_json(Method::GET, &self.verification(&format!("/bundles/{id}")), None).await;
        assert_eq!(status, StatusCode::OK, "{bundle}");
        bundle
    }

    /// 從桌面要求一般驗證並執行，回傳 Evidence Bundle。
    async fn verify(&self, journeys: Value) -> Value {
        let (status, request) = self.h().send_json(Method::POST, &self.verification("/runs"), Some(json!({ "journeys": journeys }))).await;
        assert_eq!(status, StatusCode::CREATED, "{request}");
        self.h().supervise().await;
        let (status, overview) = self.h().send_json(Method::GET, &self.verification(""), None).await;
        assert_eq!(status, StatusCode::OK, "{overview}");
        let done = overview["requests"].as_array().unwrap().iter().find(|r| r["id"] == request["id"]).unwrap().clone();
        self.bundle(done["bundle_id"].as_str().expect("驗證應該已執行")).await
    }

    async fn triage(&self, controller: &str, finding: &Value, body: Value) -> (StatusCode, Value) {
        let mut body = body;
        body["finding_id"] = finding["id"].clone();
        self.h().call_tool(controller, "triage_exploration_finding", body).await
    }

    /// 使用者在桌面回答裁定請求。
    async fn answer(&self, request: &str, option: &str) {
        let path = format!("{WORKSPACES}/{}/decisions/{request}", self.ws);
        let (_, view) = self.h().send_json(Method::GET, &path, None).await;
        let (status, answered) = self
            .h()
            .send_json(Method::POST, &format!("{path}/answer"), Some(json!({ "option_id": option, "expected_revision": view["revision"] })))
            .await;
        assert_eq!(status, StatusCode::OK, "{answered}");
    }

    /// 發布「調查與研究」Playbook 的新版本：只有一個安全 Gate，離開條件是通過的 Evidence Bundle。
    async fn publish_verification_playbook(&self) {
        let (status, catalog) = self.h().send_json(Method::GET, PLAYBOOKS, None).await;
        assert_eq!(status, StatusCode::OK, "{catalog}");
        let mut playbook =
            catalog["playbooks"].as_array().unwrap().iter().filter(|p| p["key"] == "investigation").last().unwrap()["definition"].clone();
        let mut only = playbook["stages"][0].clone();
        only["key"] = json!("real_interface");
        only["title"] = json!("真實介面驗證");
        only["gate"] = json!("safety");
        only["failure_route"] = json!({ "kind": "retry" });
        only["exit_checks"] = json!([{ "key": "journey_passed", "description": "在真實介面通過", "requires": "verification" }]);
        playbook["stages"] = json!([only]);
        let (status, body) = self.h().send_json(Method::POST, PLAYBOOKS, Some(playbook)).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }
}

/// 可寫的控制資料庫連線：模擬繞過 Runtime 直接竄改資料（harness 的 `db()` 是唯讀的）。
fn tamper(h: &Harness) -> rusqlite::Connection {
    let db = rusqlite::Connection::open(h.data_dir().join(baton_store::DB_FILE_NAME)).unwrap();
    db.busy_timeout(std::time::Duration::from_secs(5)).unwrap();
    db
}

fn section<'a>(overview: &'a Value, status: &str) -> Vec<&'a Value> {
    overview["findings"].as_array().unwrap().iter().filter(|f| f["status"] == status).collect()
}

// --- 驗收條件：環境故障不被記為產品缺陷 ---

#[tokio::test]
async fn an_environment_failure_is_never_recorded_as_a_product_defect() {
    let (w, controller, _, _, finding) = explored().await;
    let driver = &w.h().verification_driver;

    // 1. 受驗服務沒有起來（就緒條件連不上）：案例沒有執行，不能算缺陷
    let (attempt, bundle, current) = w.reproduce(&controller, &finding, Some(case())).await;
    assert_eq!(bundle["verdict"], "environment_blocked", "{bundle}");
    assert_eq!((attempt["purpose"].as_str(), attempt["outcome"].as_str()), (Some("reproduce"), Some("inconclusive")), "{attempt}");
    assert_eq!((current["status"].as_str(), current.get("case")), (Some("suspected"), None), "{current}");

    // 2. 服務正常、對照通過，但案例要打的端點連不上：環境阻礙，不是缺陷
    driver.script_response("GET", &format!("{API}/health"), 200, "ok");
    driver.script_response("GET", &format!("{API}/items"), 200, r#"[{"name":"商品 A"}]"#);
    let (attempt, _, current) = w.reproduce(&controller, &finding, Some(case())).await;
    assert_eq!((attempt["verdict"].as_str(), attempt["control_verdict"].as_str()), (Some("environment_blocked"), Some("passed")));
    assert_eq!((attempt["outcome"].as_str(), current["status"].as_str()), (Some("inconclusive"), Some("suspected")));

    // 3. 授權阻礙（401）也不是缺陷
    driver.script_response("GET", &format!("{API}/cart/total"), 401, "unauthorized");
    let (attempt, _, current) = w.reproduce(&controller, &finding, Some(case())).await;
    assert_eq!((attempt["verdict"].as_str(), attempt["outcome"].as_str()), (Some("auth_blocked"), Some("inconclusive")));
    assert_eq!(current["status"], "suspected");

    // 4. 對照（代表旅程）本身失敗：環境沒有證明可用，即使案例的斷言失敗也不確認
    driver.script_response("GET", &format!("{API}/items"), 500, "boom");
    driver.script_response("GET", &format!("{API}/cart/total"), 200, r#"{"total":160}"#);
    let (attempt, bundle, current) = w.reproduce(&controller, &finding, Some(case())).await;
    assert_eq!(journey_of(&bundle, &format!("finding.{}", finding["id"].as_str().unwrap())).unwrap()["verdict"], "product_failure");
    assert_eq!((attempt["control_verdict"].as_str(), attempt["outcome"].as_str()), (Some("product_failure"), Some("inconclusive")));
    assert_eq!(current["status"], "suspected");

    // 每次嘗試都留下紀錄（新的在前），沒有任何一次把疑點記成缺陷
    let overview = w.exploration().await;
    assert_eq!(overview["findings"][0]["attempts"].as_array().unwrap().len(), 4);
    assert!(section(&overview, "confirmed").is_empty(), "{overview}");

    // 確認是環境問題後由 Controller 排除：保留紀錄與原因，不是刪除
    let (status, body) =
        w.triage(&controller, &finding, json!({ "action": "exclude", "reason": "environment", "rationale": "測試環境的購物車服務沒有部署" })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let excluded = w.finding(&finding["id"]).await;
    assert_eq!((excluded["status"].as_str(), excluded["exclusion"]["reason"].as_str()), (Some("excluded"), Some("environment")));
    assert_eq!(excluded["attempts"].as_array().unwrap().len(), 4, "排除不會刪掉重現紀錄");
    // 已排除的疑點要先重新開啟才能再重現
    let (status, body) =
        w.h().call_tool(&controller, "reproduce_exploration_finding", json!({ "finding_id": finding["id"], "journey": case() })).await;
    assert_eq!((status, error_code(&body)), (StatusCode::CONFLICT, "exploration_transition_refused"), "{body}");
}

// --- 驗收條件：未經重現的疑點不會顯示為 confirmed ---

#[tokio::test]
async fn a_suspicion_is_only_shown_as_confirmed_after_a_reproduction_fails_on_a_product_assertion() {
    let (w, controller, worker, task, finding) = explored().await;
    assert_eq!((finding["status"].as_str(), finding["task_id"].as_str()), (Some("suspected"), Some(task.as_str())), "{finding}");
    assert_eq!(finding.get("case"), None);
    let overview = w.exploration().await;
    assert_eq!(section(&overview, "suspected").len(), 1);
    assert!(section(&overview, "confirmed").is_empty());

    // 沒有任何工具或狀態可以直接宣稱已確認
    let (status, body) = w.triage(&controller, &finding, json!({ "action": "confirm", "rationale": "我看到了" })).await;
    assert!(status.is_client_error(), "{body}");
    let (status, body) =
        w.h().call_tool(&worker, "reproduce_exploration_finding", json!({ "finding_id": finding["id"], "journey": case() })).await;
    assert_eq!((status, error_code(&body)), (StatusCode::FORBIDDEN, "controller_only"), "{body}");
    let (status, body) = w.h().call_tool(&controller, "report_exploration_finding", finding_report()).await;
    assert_eq!((status, error_code(&body)), (StatusCode::FORBIDDEN, "worker_only"), "{body}");
    // 重現一定要附案例；只有人工步驟的案例無法證明產品行為
    let (status, body) = w.h().call_tool(&controller, "reproduce_exploration_finding", json!({ "finding_id": finding["id"] })).await;
    assert_eq!((status, error_code(&body)), (StatusCode::UNPROCESSABLE_ENTITY, "exploration_invalid"), "{body}");
    let manual = json!({ "key": "look", "title": "看一下", "steps": [{ "key": "look", "operation": { "kind": "manual", "instruction": "看總額" } }] });
    let (status, body) =
        w.h().call_tool(&controller, "reproduce_exploration_finding", json!({ "finding_id": finding["id"], "journey": manual })).await;
    assert_eq!((status, error_code(&body)), (StatusCode::UNPROCESSABLE_ENTITY, "exploration_invalid"), "{body}");
    // 直接改資料庫也不行：沒有重現案例的疑點不能是 confirmed
    let forged = tamper(w.h()).execute("UPDATE exploration_finding SET status = 'confirmed', version = version + 1 WHERE id = ?1", [finding["id"].as_str().unwrap()]);
    assert!(forged.is_err(), "沒有重現案例的疑點被改成 confirmed");
    assert_eq!(w.finding(&finding["id"]).await["status"], "suspected");

    // 產品以斷言失敗（總額 160 而不是 180），對照通過：升為已確認缺陷，記下案例、證據與失敗的步驟
    w.shop(160);
    let (attempt, bundle, confirmed) = w.reproduce(&controller, &finding, Some(case())).await;
    assert_eq!((attempt["outcome"].as_str(), attempt["verdict"].as_str()), (Some("reproduced"), Some("product_failure")), "{attempt}");
    assert_eq!(attempt["control_verdict"], "passed");
    assert_eq!(confirmed["status"], "confirmed", "{confirmed}");
    assert_eq!(confirmed["case"]["bundle_id"], bundle["id"]);
    assert_eq!(confirmed["case"]["failed_step"], "total");
    let key = format!("finding.{}", finding["id"].as_str().unwrap());
    assert_eq!(confirmed["case"]["journey"]["key"], key.as_str());
    assert_eq!(journey_of(&bundle, &key).unwrap()["verdict"], "product_failure");
    assert_eq!(journey_of(&bundle, "browse").unwrap()["verdict"], "passed", "代表旅程當對照一起執行");
    let overview = w.exploration().await;
    assert_eq!(section(&overview, "confirmed").len(), 1);
    assert!(section(&overview, "suspected").is_empty());
}

// --- 驗收條件：不能以排除標籤或刪除案例掩蓋缺陷 ---

#[tokio::test]
async fn a_defect_cannot_be_hidden_by_exclusion_labels_or_by_dropping_its_case() {
    let (w, controller, _, task, finding) = explored().await;
    w.shop(160);
    let (_, failing, _) = w.reproduce(&controller, &finding, Some(case())).await;
    let key = format!("finding.{}", finding["id"].as_str().unwrap());

    // 已確認的缺陷：任何排除標籤、改為未驗風險都被拒絕，也不會因此建立裁定請求
    for body in [
        json!({ "action": "exclude", "reason": "environment", "rationale": "應該是環境問題" }),
        json!({ "action": "exclude", "reason": "seed_data", "rationale": "資料問題" }),
        json!({ "action": "exclude", "reason": "expected_design", "rationale": "規格本來就這樣" }),
        json!({ "action": "mark_unverified", "rationale": "先不管" }),
    ] {
        let (status, rejected) = w.triage(&controller, &finding, body).await;
        assert_eq!((status, error_code(&rejected)), (StatusCode::CONFLICT, "exploration_defect_locked"), "{rejected}");
    }
    let (status, decisions) = w.h().send_json(Method::GET, &format!("{WORKSPACES}/{}/decisions", w.ws), None).await;
    assert_eq!(status, StatusCode::OK, "{decisions}");
    assert!(!decisions.to_string().contains("預期設計"), "{decisions}");
    // 不能換成比較寬鬆的案例
    let mut weaker = case();
    weaker["steps"][0]["assertions"] = json!([{ "kind": "status", "equals": 200 }]);
    let (status, body) =
        w.h().call_tool(&controller, "reproduce_exploration_finding", json!({ "finding_id": finding["id"], "journey": weaker })).await;
    assert_eq!((status, error_code(&body)), (StatusCode::CONFLICT, "exploration_defect_locked"), "{body}");
    // 直接刪除或改標籤也被控制資料庫拒絕
    let db = tamper(w.h());
    let id = finding["id"].as_str().unwrap();
    // 確認這個連線真的可以寫（否則下面的拒絕沒有意義）
    db.execute("UPDATE exploration_finding SET updated_at = updated_at + 1, version = version + 1 WHERE id = ?1", [id]).unwrap();
    assert!(db.execute("DELETE FROM exploration_finding WHERE id = ?1", [id]).is_err());
    assert!(db.execute("DELETE FROM exploration_attempt WHERE finding_id = ?1", [id]).is_err());
    assert!(db
        .execute("UPDATE exploration_finding SET status = 'excluded', exclusion = '{}', reproduction_case = NULL, version = version + 1 WHERE id = ?1", [id])
        .is_err());
    assert_eq!(w.finding(&finding["id"]).await["status"], "confirmed");

    // 失敗中的重現只用於診斷：還沒修好的案例不加入一般驗證，不混淆既有的 release gate
    let regular = w.verify(json!([])).await;
    assert_eq!(regular["verdict"], "passed", "{regular}");
    assert!(journey_of(&regular, &key).is_none(), "{regular}");

    // 修復後以同一個案例驗證；通過後成為已修復
    w.shop(180);
    let (attempt, fixed_bundle, fixed) = w.reproduce(&controller, &finding, None).await;
    assert_eq!((attempt["purpose"].as_str(), attempt["outcome"].as_str()), (Some("verify_fix"), Some("fix_verified")), "{attempt}");
    assert_eq!(fixed["status"], "fixed");
    assert_eq!(fixed["case"]["bundle_id"], failing["id"], "確認時的案例不變");
    // 修復驗證的 Bundle 只用於診斷，不能當作 Stage 的驗證證據
    w.publish_verification_playbook().await;
    let (status, workflow) =
        w.h().call_tool(&controller, "start_workflow", json!({ "situation": "investigation", "task_id": task, "rationale": "確認修復" })).await;
    assert_eq!(status, StatusCode::OK, "{workflow}");
    let record = json!({
        "workflow_id": workflow["id"],
        "stage": workflow["current_stage"],
        "expected_revision": workflow["revision"],
        "outcome": "passed",
        "evidence": [{ "check": "journey_passed", "evidence_bundle_id": fixed_bundle["id"] }],
    });
    let (status, recorded) = w.h().call_tool(&controller, "record_stage_result", record).await;
    assert_eq!(status, StatusCode::OK, "{recorded}");
    assert_ne!(recorded["attempt"]["outcome"], "passed", "{recorded}");

    // 已修復的案例納入正常回歸：每一次一般驗證都會執行，指定旅程也排除不掉，並對應到需求
    let regular = w.verify(json!(["browse"])).await;
    let regression = journey_of(&regular, &key).unwrap_or_else(|| panic!("回歸案例沒有執行：{regular}"));
    assert_eq!(regression["verdict"], "passed");
    assert!(regular["covered_features"].as_array().unwrap().contains(&json!("checkout")), "{regular}");
    let (status, body) = w.h().send_json(Method::POST, &w.verification("/runs"), Some(json!({ "journeys": [key] }))).await;
    assert_eq!((status, error_code(&body)), (StatusCode::UNPROCESSABLE_ENTITY, "verification_request_invalid"), "{body}");
    assert_eq!(w.finding(&finding["id"]).await["attempts"][0]["outcome"], "regression_passed");

    // 產品又壞了：一般驗證失敗、缺陷標為回歸失敗；它仍留在回歸範圍內，也不能排除
    w.shop(160);
    let regular = w.verify(json!(["browse"])).await;
    assert_eq!(regular["verdict"], "product_failure", "{regular}");
    let regressed = w.finding(&finding["id"]).await;
    assert_eq!((regressed["status"].as_str(), regressed["attempts"][0]["outcome"].as_str()), (Some("regressed"), Some("regressed")));
    let (status, body) =
        w.triage(&controller, &finding, json!({ "action": "exclude", "reason": "tool_limitation", "rationale": "測試工具不穩" })).await;
    assert_eq!((status, error_code(&body)), (StatusCode::CONFLICT, "exploration_defect_locked"), "{body}");
    let (status, republished) = w.h().send_json(Method::POST, &w.verification("/profile"), Some({
        let mut p = profile();
        p["uncovered"] = json!([{ "area": "結帳", "reason": "暫時不驗" }]);
        p
    })).await;
    assert_eq!(status, StatusCode::CREATED, "{republished}");
    let regular = w.verify(json!([])).await;
    assert_eq!(journey_of(&regular, &key).unwrap()["verdict"], "product_failure", "發布新的 Profile 也拿不掉回歸案例");
    assert_eq!(regular["verdict"], "product_failure");
}

// --- 章程、未驗風險與預期設計 ---

#[tokio::test]
async fn exploration_starts_from_a_charter_and_unreproducible_or_expected_findings_are_kept_with_their_reason() {
    let w = world().await;
    let task = w.task("探索結帳").await;
    let controller = w.controller_turn("探索結帳流程").await;

    // 章程要有區域、角色、目標、禁止的副作用與上限；Worker 不能自己定章程
    let mut incomplete = charter(&task);
    incomplete["forbidden_side_effects"] = json!([]);
    let (status, body) = w.h().call_tool(&controller, "define_exploration_charter", incomplete).await;
    assert_eq!((status, error_code(&body)), (StatusCode::UNPROCESSABLE_ENTITY, "exploration_invalid"), "{body}");

    // 沒有章程時 Worker 不能回報疑點
    let other = w.task("沒有章程的探索").await;
    let (_, unchartered) = w.dispatch(&other).await;
    let (status, body) = w.h().call_tool(&unchartered, "report_exploration_finding", finding_report()).await;
    assert_eq!((status, error_code(&body)), (StatusCode::CONFLICT, "exploration_charter_required"), "{body}");
    let (status, body) = w.h().call_tool(&unchartered, "define_exploration_charter", charter(&other)).await;
    assert_eq!((status, error_code(&body)), (StatusCode::FORBIDDEN, "controller_only"), "{body}");

    let (status, defined) = w.h().call_tool(&controller, "define_exploration_charter", charter(&task)).await;
    assert_eq!(status, StatusCode::CREATED, "{defined}");
    let (status, same) = w.h().call_tool(&controller, "define_exploration_charter", charter(&task)).await;
    assert_eq!((status, same["created"].as_bool()), (StatusCode::OK, Some(false)), "內容相同不新增版本");

    // Worker 的 Prompt 帶著章程的邊界
    let (rid, worker) = w.dispatch(&task).await;
    let launched = w.h().agent_adapter.launched();
    let prompt = &launched.iter().find(|s| s.run_id == rid).unwrap().prompt;
    for part in ["探索章程", "結帳流程", "一般顧客", "不得送出真實付款", "40 步"] {
        assert!(prompt.contains(part), "Prompt 缺少「{part}」：{prompt}");
    }

    // 回報的證據必須是已登記的成果或 Evidence Bundle；功能必須在 Feature Map 中
    let mut bad = finding_report();
    bad["evidence"] = json!(["art_does_not_exist"]);
    let (status, body) = w.h().call_tool(&worker, "report_exploration_finding", bad).await;
    assert_eq!((status, error_code(&body)), (StatusCode::UNPROCESSABLE_ENTITY, "exploration_invalid"), "{body}");
    let mut bad = finding_report();
    bad["feature"] = json!("refunds");
    let (status, body) = w.h().call_tool(&worker, "report_exploration_finding", bad).await;
    assert_eq!((status, error_code(&body)), (StatusCode::UNPROCESSABLE_ENTITY, "exploration_invalid"), "{body}");

    let (status, finding) = w.h().call_tool(&worker, "report_exploration_finding", finding_report()).await;
    assert_eq!(status, StatusCode::CREATED, "{finding}");
    let (status, rounding) = w.h().call_tool(&worker, "report_exploration_finding", json!({
        "title": "價格顯示四捨五入",
        "steps": ["打開商品頁"],
        "expected": "顯示 99.5",
        "actual": "顯示 100",
    })).await;
    assert_eq!(status, StatusCode::CREATED, "{rounding}");

    // 無法重現：案例通過，保留為未驗風險（不駁回，也不宣稱已確認）
    w.shop(180);
    let (attempt, _, current) = w.reproduce(&controller, &finding, Some(case())).await;
    assert_eq!(attempt["outcome"], "not_reproduced");
    assert_eq!(current["status"], "unverified_risk", "{current}");
    assert!(current["unverified_reason"].as_str().unwrap().contains("無法重現"), "{current}");

    // 排除為預期設計是產品取捨：先送裁定，疑點維持原狀；使用者同意後再呼叫一次才排除
    let expected = json!({ "action": "exclude", "reason": "expected_design", "rationale": "價格依規格顯示到整數" });
    let (status, body) = w.triage(&controller, &rounding, expected.clone()).await;
    assert_eq!((status, error_code(&body), body["matter"].as_str()), (StatusCode::CONFLICT, "requires_decision", Some("product_direction")), "{body}");
    let request = body["decision_request_id"].as_str().unwrap().to_string();
    assert_eq!(w.finding(&rounding["id"]).await["status"], "suspected");
    w.answer(&request, "approve").await;
    let (status, body) = w.triage(&controller, &rounding, expected).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["finding"]["exclusion"]["decision_request_id"], request.as_str());

    // 概況把疑點、未驗風險與已排除分開，全部都還在
    let overview = w.exploration().await;
    assert_eq!(overview["charters"][0]["area"], "結帳流程");
    assert_eq!(section(&overview, "unverified_risk").len(), 1);
    assert_eq!(section(&overview, "excluded").len(), 1);
    assert_eq!(overview["findings"].as_array().unwrap().len(), 2);
    let (status, from_tool) = w.h().call_tool(&controller, "get_exploration", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{from_tool}");
    assert_eq!(from_tool["findings"], overview["findings"]);
}
