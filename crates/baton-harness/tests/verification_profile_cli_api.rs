//! 情境測試：Verification Profile 與 CLI／API 驗證 Adapter（票 45，spec D-29），經由 Runtime harness 的真實 HTTP API 驗證。
//!
//! 受驗的「真實介面」由 fake VerificationDriver 扮演：命令與 HTTP 端點的回應由測試腳本化，狀態目錄是暫存目錄下的
//! 真實目錄，就緒重試的等待推進 fake 時鐘。Controller 與 Worker 是腳本化的（同票 17、43）。只斷言外部行為：
//! 桌面 API 與工具的回應、Evidence Bundle 的內容、驗證 Adapter 實際執行的操作、狀態目錄是否還在。
//!
//! 每個測試名稱對應票 45 的驗收條件：
//! - Worker 宣稱成功但實際副作用不符時驗證失敗：
//!   `a_worker_claiming_success_fails_verification_when_the_real_side_effect_is_missing`
//! - 證據不足與環境阻礙不算通過：`missing_evidence_and_environment_or_auth_problems_never_count_as_passed`
//! - 新 Profile 至少實際跑過一條代表旅程，並標示未覆蓋範圍：
//!   `a_new_profile_must_actually_pass_a_representative_journey_and_marks_what_it_does_not_cover`
//! - Evidence Bundle 綁定 build、revision、需求與 Profile 版本：`an_evidence_bundle_binds_build_revision_requirement_and_profile_version`
//! - 清理只移除本 Run 的狀態並保留證據：`cleanup_removes_only_this_verifications_state_and_keeps_the_evidence`
//! - 驗證要求在 Runtime 重啟後仍會執行：`a_queued_verification_still_runs_after_a_runtime_restart`

use std::path::{Path, PathBuf};

use baton_domain::Timestamp;
use baton_harness::Harness;
use baton_ports::{AdapterEvent, Clock, CommandOutput, HttpOutcome};
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

const WORKSPACES: &str = "/api/v1/workspaces";
const PLAYBOOKS: &str = "/api/v1/playbooks";
const CLOCK_START: Timestamp = Timestamp::from_millis(1_700_000_000_123);
/// 合併視窗（spec D-31）。
const WINDOW_MS: i64 = 5_000;
const API: &str = "http://127.0.0.1:8080";
const TOKEN: &str = "test-token-123";

// --- 共用輔助 ---

struct World {
    harness: Harness,
    ws: String,
    goal: String,
    dir: PathBuf,
}

fn filled(text: &str) -> Value {
    json!({ "status": "filled", "text": text })
}

fn contract() -> Value {
    json!({
        "objective": filled("讓記帳 API 可以新增支出"),
        "inputs": filled("既有的記帳 API"),
        "scope": filled("記帳 API"),
        "deliverables": filled("新增支出的端點"),
        "done_criteria": filled("新增的支出可以讀回"),
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

fn command(args: &[&str]) -> Value {
    json!({ "kind": "command", "program": "ledger", "args": args })
}

/// 記帳工具的 Profile：doctor、啟動 API、就緒、資料準備、三條旅程（代表旅程、命令旅程、人工旅程）、teardown 與 Feature Map。
fn profile() -> Value {
    let mut create = http("POST", "/expenses");
    create["headers"] = json!([{ "name": "Authorization", "value": format!("Bearer {TOKEN}") }]);
    create["body"] = json!(r#"{"memo":"午餐","amount":120}"#);
    json!({
        "interface": "cli_api",
        "doctor": [step("cli", command(&["--version"]), json!([{ "kind": "exit_code", "equals": 0 }]))],
        "start": { "program": "ledger", "args": ["serve", "--data", "{{state_dir}}"] },
        "readiness": {
            "step": step("health", http("GET", "/health"), json!([{ "kind": "status", "equals": 200 }, { "kind": "contains", "text": "ready" }])),
            "attempts": 3,
            "interval_ms": 500,
        },
        "setup": [step("seed", command(&["seed", "--run", "{{run_id}}"]), json!([{ "kind": "exit_code", "equals": 0 }]))],
        "journeys": [
            {
                "key": "record-expense",
                "title": "記一筆支出並讀回",
                "representative": true,
                "steps": [
                    step("create", create, json!([{ "kind": "status", "equals": 201 }])),
                    step("read-back", http("GET", "/expenses"), json!([
                        { "kind": "status", "equals": 200 },
                        { "kind": "json_equals", "pointer": "/0/memo", "value": "午餐" },
                    ])),
                ],
            },
            {
                "key": "report",
                "title": "月報",
                "steps": [step("monthly", command(&["report"]), json!([{ "kind": "exit_code", "equals": 0 }, { "kind": "contains", "text": "合計" }]))],
            },
            {
                "key": "layout",
                "title": "報表版面",
                "steps": [{ "key": "look", "title": "看版面", "operation": { "kind": "manual", "instruction": "確認報表版面對齊" } }],
            },
        ],
        "teardown": [command(&["purge", "--run", "{{run_id}}"])],
        "evidence": { "redact": [TOKEN] },
        "feature_map": [
            { "key": "expenses", "title": "記帳", "criterion": "新增的支出可以讀回", "journeys": ["record-expense"] },
            { "key": "monthly-report", "title": "月報", "criterion": "月報顯示合計", "journeys": ["report"] },
            { "key": "export", "title": "匯出 CSV", "criterion": "可以匯出 CSV", "journeys": [] },
        ],
        "uncovered": [{ "area": "大量資料的效能", "reason": "沒有基準資料" }],
    })
}

fn error_code(body: &Value) -> &str {
    body["error"].as_str().unwrap_or_default()
}

fn step_of<'a>(bundle: &'a Value, key: &str) -> &'a Value {
    bundle["steps"].as_array().unwrap().iter().find(|s| s["key"] == key).unwrap_or_else(|| panic!("沒有步驟 {key}：{bundle}"))
}

fn journey_of<'a>(bundle: &'a Value, key: &str) -> &'a Value {
    bundle["journeys"].as_array().unwrap().iter().find(|j| j["key"] == key).unwrap_or_else(|| panic!("沒有旅程 {key}：{bundle}"))
}

async fn world() -> World {
    let harness = Harness::builder().clock_at(CLOCK_START).start().await;
    harness.add_account("帳號 A", "account0@example.com").await;
    let dir = harness.data_dir().join("projects").join("ledger");
    std::fs::create_dir_all(&dir).unwrap();
    let (status, body) = harness.send_json(Method::POST, WORKSPACES, Some(json!({ "path": dir.to_string_lossy() }))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let ws = body["id"].as_str().unwrap().to_string();
    let (status, goal) =
        harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/goals"), Some(json!({ "title": "記帳工具" }))).await;
    assert_eq!(status, StatusCode::CREATED, "{goal}");
    let goal = goal["id"].as_str().unwrap().to_string();
    World { harness, ws, goal, dir }
}

impl World {
    fn h(&self) -> &Harness {
        &self.harness
    }

    fn path(&self, rest: &str) -> String {
        format!("{WORKSPACES}/{}/verification{rest}", self.ws)
    }

    /// 受驗的 API 與 CLI 一切正常：新增的支出讀得回來。
    fn healthy(&self) {
        let driver = &self.h().verification_driver;
        driver.script_exit("ledger", &["--version"], 0, "ledger 1.4.2\n");
        driver.script_exit("ledger", &["report"], 0, "十月合計 120 元\n");
        // 其餘 ledger 命令（seed、purge）都成功
        driver.script_exit("ledger", &[], 0, "ok\n");
        driver.script_response("GET", &format!("{API}/health"), 200, "ready");
        driver.script_response("POST", &format!("{API}/expenses"), 201, &format!(r#"{{"id":1,"echo":"{TOKEN}"}}"#));
        driver.script_response("GET", &format!("{API}/expenses"), 200, r#"[{"memo":"午餐","amount":120}]"#);
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

    /// 從桌面要求執行驗證，回傳要求。
    async fn request(&self, body: Value) -> Value {
        let (status, request) = self.h().send_json(Method::POST, &self.path("/runs"), Some(body)).await;
        assert_eq!(status, StatusCode::CREATED, "{request}");
        assert_eq!(request["status"], "queued");
        request
    }

    /// 讓 Supervisor 執行驗證，回傳這個要求的 Evidence Bundle。
    async fn bundle_of(&self, request: &Value) -> Value {
        self.h().supervise().await;
        let overview = self.overview().await;
        let done = overview["requests"].as_array().unwrap().iter().find(|r| r["id"] == request["id"]).unwrap().clone();
        assert_eq!(done["status"], "completed", "驗證要求應該已執行：{overview}");
        let id = done["bundle_id"].as_str().unwrap();
        let (status, bundle) = self.h().send_json(Method::GET, &self.path(&format!("/bundles/{id}")), None).await;
        assert_eq!(status, StatusCode::OK, "{bundle}");
        bundle
    }

    /// 從桌面要求並執行一次驗證，回傳 Evidence Bundle。
    async fn verify(&self, body: Value) -> Value {
        let request = self.request(body).await;
        self.bundle_of(&request).await
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

    /// 發布「調查與研究」Playbook 的新版本：只有一個安全 Gate，離開條件是通過的 Evidence Bundle。
    async fn publish_verification_playbook(&self) {
        let (status, catalog) = self.h().send_json(Method::GET, PLAYBOOKS, None).await;
        assert_eq!(status, StatusCode::OK, "{catalog}");
        let mut playbook = catalog["playbooks"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|p| p["key"] == "investigation")
            .last()
            .unwrap()["definition"]
            .clone();
        let mut only = playbook["stages"][0].clone();
        only["key"] = json!("real_interface");
        only["title"] = json!("真實介面驗證");
        only["gate"] = json!("safety");
        only["failure_route"] = json!({ "kind": "retry" });
        only["exit_checks"] = json!([{ "key": "journey_passed", "description": "代表旅程在真實介面通過", "requires": "verification" }]);
        playbook["stages"] = json!([only]);
        let (status, body) = self.h().send_json(Method::POST, PLAYBOOKS, Some(playbook)).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }

    /// 以 Evidence Bundle 回報 Workflow 目前的 Stage，回傳 attempt 與更新後的 Workflow。
    async fn record(&self, token: &str, workflow: &Value, bundle_id: &Value) -> Value {
        let body = json!({
            "workflow_id": workflow["id"],
            "stage": workflow["current_stage"],
            "expected_revision": workflow["revision"],
            "outcome": "passed",
            "evidence": [{ "check": "journey_passed", "evidence_bundle_id": bundle_id }],
        });
        let (status, body) = self.h().call_tool(token, "record_stage_result", body).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body
    }

    fn state_dir(&self, request: &Value) -> PathBuf {
        self.h().data_dir().join("verification").join(request["id"].as_str().unwrap())
    }
}

fn operations_with(h: &Harness, prefix: &str) -> Vec<String> {
    h.verification_driver.operations().into_iter().filter(|o| o.starts_with(prefix)).collect()
}

fn exists(path: &Path) -> bool {
    path.exists()
}

// --- 驗收條件：Worker 宣稱成功但實際副作用不符時驗證失敗 ---

#[tokio::test]
async fn a_worker_claiming_success_fails_verification_when_the_real_side_effect_is_missing() {
    let w = world().await;
    let profile_v1 = w.publish(profile()).await;
    w.publish_verification_playbook().await;
    let tid = w.task("新增支出的 API").await;
    let (rid, worker) = w.dispatch(&tid).await;

    // Worker 說它做完了，程序也正常結束
    let (status, body) = w.h().call_tool(&worker, "report_progress", json!({ "summary": "已完成：POST /expenses 會保存支出" })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    w.h().agent_adapter.emit(&rid, AdapterEvent::Exited { success: true });
    w.h().supervise().await;

    // 但真實介面上，新增之後讀不回來
    w.healthy();
    w.h().verification_driver.script_response("GET", &format!("{API}/expenses"), 200, "[]");

    let controller = w.controller_turn("驗證新增支出").await;
    // 只有 Controller 能要求驗證；Worker 不能替自己的工作做驗證
    let (status, body) = w.h().call_tool(&worker, "run_verification", json!({ "task_id": tid })).await;
    assert_eq!((status, error_code(&body)), (StatusCode::FORBIDDEN, "controller_only"), "{body}");
    let (status, workflow) =
        w.h().call_tool(&controller, "start_workflow", json!({ "situation": "investigation", "task_id": tid, "rationale": "確認行為" })).await;
    assert_eq!(status, StatusCode::OK, "{workflow}");
    assert_eq!(workflow["current_stage"], "real_interface");

    let (status, request) =
        w.h().call_tool(&controller, "run_verification", json!({ "task_id": tid, "journeys": ["record-expense"] })).await;
    assert_eq!(status, StatusCode::CREATED, "{request}");
    assert_eq!(request["requested_by"].as_str(), workflow["created_by"].as_str(), "要求者是這一輪 Controller");
    let failed = w.bundle_of(&request).await;
    assert_eq!((failed["verdict"].as_str(), failed["reason_code"].as_str()), (Some("product_failure"), Some("assertion_failed")));
    let create = step_of(&failed, "create");
    assert_eq!((create["verdict"].as_str(), create["engine_code"].as_str()), (Some("passed"), Some("http:201")));
    let read_back = step_of(&failed, "read-back");
    assert_eq!(read_back["verdict"], "product_failure", "{read_back}");
    assert_eq!(read_back["assertions"][1]["passed"], false);
    assert!(read_back["assertions"][1]["actual"].as_str().unwrap().contains("不存在"), "{read_back}");
    // 秘密被遮蔽
    assert!(!failed.to_string().contains(TOKEN), "{failed}");
    assert!(create["output"].as_str().unwrap().contains("[已遮蔽]"), "{create}");
    // 送出的請求確實帶了測試權杖（只是不出現在證據中）
    let sent = w.h().verification_driver.http_requests();
    assert!(sent.iter().any(|r| r.method == "POST" && r.headers.contains(&("Authorization".into(), format!("Bearer {TOKEN}")))));

    // 未通過的驗證不能讓 Stage 通過
    let attempt = w.record(&controller, &workflow, &failed["id"]).await;
    assert_eq!(attempt["attempt"]["outcome"], "incomplete", "{attempt}");
    assert_eq!(attempt["attempt"]["evidence"][0]["status"], "not_passed");
    assert!(attempt["attempt"]["missing"][0]["reason"].as_str().unwrap().contains("驗證沒有通過"), "{attempt}");
    let workflow = attempt["workflow"].clone();

    // 修好之後：通過的驗證，但使用者先改了 Profile → 舊 Profile 版本的通過不再算數
    w.healthy();
    let passed_v1 = w.verify(json!({ "task_id": tid, "journeys": ["record-expense"] })).await;
    assert_eq!(passed_v1["verdict"], "passed", "{passed_v1}");
    let mut changed = profile();
    changed["journeys"][0]["title"] = json!("新增一筆支出並從清單讀回");
    let profile_v2 = w.publish(changed).await;
    assert_eq!((profile_v1["version"].as_i64(), profile_v2["version"].as_i64()), (Some(1), Some(2)));
    let attempt = w.record(&controller, &workflow, &passed_v1["id"]).await;
    assert_eq!(attempt["attempt"]["outcome"], "incomplete", "{attempt}");
    assert_eq!(attempt["attempt"]["evidence"][0]["status"], "not_passed");
    let workflow = attempt["workflow"].clone();

    // 依新版本重新驗證並通過，Stage 才通過
    let (status, request) =
        w.h().call_tool(&controller, "run_verification", json!({ "task_id": tid, "journeys": ["record-expense"] })).await;
    assert_eq!(status, StatusCode::CREATED, "{request}");
    let passed_v2 = w.bundle_of(&request).await;
    assert_eq!((passed_v2["verdict"].as_str(), passed_v2["profile_version"].as_i64()), (Some("passed"), Some(2)));
    let attempt = w.record(&controller, &workflow, &passed_v2["id"]).await;
    assert_eq!(attempt["attempt"]["outcome"], "passed", "{attempt}");
    assert_eq!(attempt["workflow"]["status"], "completed");

    // Controller 也讀得到同樣的概況
    let (status, overview) = w.h().call_tool(&controller, "get_verification", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{overview}");
    assert_eq!(overview["proven_by"], passed_v2["id"]);
    assert_eq!(overview["bundles"].as_array().unwrap().len(), 3);
}

// --- 驗收條件：證據不足與環境阻礙不算通過 ---

#[tokio::test]
async fn missing_evidence_and_environment_or_auth_problems_never_count_as_passed() {
    let w = world().await;
    w.publish(profile()).await;
    let driver = &w.h().verification_driver;

    // doctor：找不到 CLI → 環境阻礙；服務沒有啟動、旅程沒有執行，但 teardown 仍執行
    w.healthy();
    driver.script_start_failure("ledger", &["--version"], "找不到程式「ledger」");
    let before_start = operations_with(w.h(), "start").len();
    let bundle = w.verify(json!({ "journeys": ["record-expense"] })).await;
    assert_eq!((bundle["verdict"].as_str(), bundle["reason_code"].as_str()), (Some("environment_blocked"), Some("operation_not_started")));
    assert_eq!(operations_with(w.h(), "start").len(), before_start, "doctor 沒有通過就不啟動服務");
    assert!(operations_with(w.h(), "http POST").is_empty(), "旅程沒有執行");
    assert_eq!(journey_of(&bundle, "record-expense")["verdict"], "inconclusive");
    assert!(!step_of(&bundle, "create")["reason_code"].as_str().unwrap().is_empty());
    assert_eq!(step_of(&bundle, "create")["attempts"], 0);

    // 就緒：服務程序在，但 /health 一直 503 → 不是「可使用」，環境阻礙；重試間隔推進時鐘
    w.healthy();
    driver.script_response("GET", &format!("{API}/health"), 503, "starting");
    let started = w.h().clock.now();
    let bundle = w.verify(json!({ "journeys": ["record-expense"] })).await;
    assert_eq!((bundle["verdict"].as_str(), bundle["reason_code"].as_str()), (Some("environment_blocked"), Some("not_ready")));
    assert_eq!(step_of(&bundle, "health")["attempts"], 3);
    assert_eq!(w.h().clock.now().as_millis() - started.as_millis(), 1_000, "三次嘗試之間等兩次 500 毫秒");
    assert!(w.h().verification_driver.running_services().is_empty(), "沒有就緒的服務也會停止");

    // 就緒條件先失敗再成功：可以繼續
    w.healthy();
    driver.script_http(
        "GET",
        &format!("{API}/health"),
        vec![HttpOutcome::Unreachable { reason: "連線被拒".into() }, HttpOutcome::Responded { status: 200, body: "ready".into() }],
    );
    let bundle = w.verify(json!({ "journeys": ["record-expense"] })).await;
    assert_eq!(bundle["verdict"], "passed", "{bundle}");
    assert_eq!(step_of(&bundle, "health")["attempts"], 2);

    // 授權：401 → 授權阻礙，不是產品缺陷
    w.healthy();
    driver.script_response("POST", &format!("{API}/expenses"), 401, "unauthorized");
    let bundle = w.verify(json!({ "journeys": ["record-expense"] })).await;
    assert_eq!((bundle["verdict"].as_str(), bundle["reason_code"].as_str()), (Some("auth_blocked"), Some("http_unauthorized")));
    // 前一步沒有通過，後面的步驟沒有執行，也不拼接成通過
    assert_eq!(step_of(&bundle, "read-back")["attempts"], 0);
    assert_eq!(step_of(&bundle, "read-back")["reason_code"], "not_executed");

    // 證據不足：命令逾時 → inconclusive
    w.healthy();
    driver.script_command("ledger", &["report"], vec![CommandOutput::TimedOut { output: String::new() }]);
    let bundle = w.verify(json!({ "journeys": ["report"] })).await;
    assert_eq!(journey_of(&bundle, "report")["verdict"], "inconclusive");
    assert_eq!((bundle["verdict"].as_str(), bundle["reason_code"].as_str()), (Some("inconclusive"), Some("operation_timed_out")));

    // 無法自動化的人工步驟
    w.healthy();
    let bundle = w.verify(json!({ "journeys": ["layout"] })).await;
    assert_eq!(journey_of(&bundle, "layout")["verdict"], "automation_unsupported");
    assert_eq!(bundle["verdict"], "automation_unsupported");

    // 預算用盡：doctor、就緒、資料準備用完三次操作，旅程判定為 budget_exhausted
    let mut tight = profile();
    tight["budget"] = json!({ "max_operations": 3 });
    w.publish(tight).await;
    w.healthy();
    let bundle = w.verify(json!({ "journeys": ["record-expense"] })).await;
    assert_eq!((bundle["verdict"].as_str(), bundle["reason_code"].as_str()), (Some("budget_exhausted"), Some("operation_budget_exhausted")));
    assert_eq!(step_of(&bundle, "create")["verdict"], "budget_exhausted");

    // 以上沒有任何一份被記為產品缺陷
    let overview = w.overview().await;
    let verdicts: Vec<&str> = overview["bundles"].as_array().unwrap().iter().map(|b| b["verdict"].as_str().unwrap()).collect();
    assert_eq!(
        verdicts,
        ["budget_exhausted", "automation_unsupported", "inconclusive", "auth_blocked", "passed", "environment_blocked", "environment_blocked"]
    );
}

// --- 驗收條件：新 Profile 至少實際跑過一條代表旅程，並標示未覆蓋範圍 ---

#[tokio::test]
async fn a_new_profile_must_actually_pass_a_representative_journey_and_marks_what_it_does_not_cover() {
    let w = world().await;
    // 沒有 Profile 時不能要求驗證
    let (status, body) = w.h().send_json(Method::POST, &w.path("/runs"), Some(json!({}))).await;
    assert_eq!((status, error_code(&body)), (StatusCode::NOT_FOUND, "verification_profile_not_found"), "{body}");
    // 沒有代表旅程、或步驟沒有程式斷言的 Profile 不能發布
    let mut no_representative = profile();
    no_representative["journeys"][0]["representative"] = json!(false);
    let (status, body) = w.h().send_json(Method::POST, &w.path("/profile"), Some(no_representative)).await;
    assert_eq!((status, error_code(&body)), (StatusCode::UNPROCESSABLE_ENTITY, "verification_profile_invalid"), "{body}");
    let mut no_assertion = profile();
    no_assertion["journeys"][0]["steps"][0]["assertions"] = json!([]);
    let (status, body) = w.h().send_json(Method::POST, &w.path("/profile"), Some(no_assertion)).await;
    assert_eq!((status, error_code(&body)), (StatusCode::UNPROCESSABLE_ENTITY, "verification_profile_invalid"), "{body}");

    w.publish(profile()).await;
    let overview = w.overview().await;
    assert!(overview["proven_by"].is_null(), "還沒跑過的 Profile 不算可用：{overview}");
    let uncovered: Vec<&str> = overview["uncovered"].as_array().unwrap().iter().map(|u| u["area"].as_str().unwrap()).collect();
    assert_eq!(uncovered, ["匯出 CSV", "大量資料的效能"]);
    // 內容相同的再次發布不產生新版本
    let (status, body) = w.h().send_json(Method::POST, &w.path("/profile"), Some(profile())).await;
    assert_eq!((status, body["created"].as_bool()), (StatusCode::OK, Some(false)), "{body}");

    // 只要求月報旅程：版本還沒被證明可用，所以一定加入代表旅程
    let request = w.request(json!({ "journeys": ["report"] })).await;
    assert_eq!(request["journeys"], json!(["record-expense", "report"]));
    // 環境不通（沒有服務在跑）：代表旅程沒有通過，Profile 仍然未證明
    let bundle = w.bundle_of(&request).await;
    assert_eq!(bundle["verdict"], "environment_blocked");
    assert!(w.overview().await["proven_by"].is_null());

    // 實際跑過且代表旅程通過 → 已證明可用
    w.healthy();
    let bundle = w.verify(json!({ "journeys": ["report"] })).await;
    assert_eq!(bundle["verdict"], "passed", "{bundle}");
    assert_eq!(journey_of(&bundle, "record-expense")["representative"], true);
    assert_eq!(w.overview().await["proven_by"], bundle["id"]);
    assert_eq!(bundle["covered_features"], json!(["expenses", "monthly-report"]));
    let areas: Vec<&str> = bundle["uncovered"].as_array().unwrap().iter().map(|u| u["area"].as_str().unwrap()).collect();
    assert_eq!(areas, ["匯出 CSV", "大量資料的效能"]);

    // 已證明之後可以只跑指定的旅程；沒跑到的功能標示為這次未覆蓋
    let request = w.request(json!({ "journeys": ["report"] })).await;
    assert_eq!(request["journeys"], json!(["report"]));
    let bundle = w.bundle_of(&request).await;
    let uncovered = bundle["uncovered"].as_array().unwrap();
    let expenses = uncovered.iter().find(|u| u["feature"] == "expenses").unwrap_or_else(|| panic!("{bundle}"));
    assert!(expenses["reason"].as_str().unwrap().contains("沒有執行"), "{expenses}");

    // 未知的旅程被拒絕
    let (status, body) = w.h().send_json(Method::POST, &w.path("/runs"), Some(json!({ "journeys": ["nope"] }))).await;
    assert_eq!((status, error_code(&body)), (StatusCode::UNPROCESSABLE_ENTITY, "verification_request_invalid"), "{body}");

    // 新版本要重新證明
    let mut changed = profile();
    changed["feature_map"][2]["journeys"] = json!(["report"]);
    w.publish(changed).await;
    let overview = w.overview().await;
    assert!(overview["proven_by"].is_null(), "{overview}");
    let uncovered: Vec<&str> = overview["uncovered"].as_array().unwrap().iter().map(|u| u["area"].as_str().unwrap()).collect();
    assert_eq!(uncovered, ["大量資料的效能"]);
}

// --- Evidence Bundle 綁定 build、revision、需求與 Profile 版本 ---

#[tokio::test]
async fn an_evidence_bundle_binds_build_revision_requirement_and_profile_version() {
    let w = world().await;
    let mut with_build = profile();
    with_build["build"] = json!({ "program": "ledger", "args": ["--version"] });
    w.publish(with_build).await;
    w.healthy();
    w.h().verification_driver.set_revision(&w.dir, "4f2a9c1");

    let bundle = w.verify(json!({ "journeys": ["record-expense"] })).await;
    assert_eq!(bundle["verdict"], "passed", "{bundle}");
    assert_eq!(bundle["build"], "ledger 1.4.2");
    assert_eq!(bundle["candidate_revision"], "4f2a9c1");
    assert_eq!((bundle["profile_version"].as_i64(), bundle["profile_version_id"].is_string()), (Some(1), true));
    assert_eq!(bundle["subject_dir"].as_str(), Some(w.dir.to_string_lossy().as_ref()));
    assert!(bundle["task_id"].is_null() && bundle["requirement_version"].is_null());

    // 沒有 build 命令時以 revision 表示 build
    w.publish(profile()).await;
    let bundle = w.verify(json!({ "journeys": ["record-expense"] })).await;
    assert_eq!((bundle["build"].as_str(), bundle["profile_version"].as_i64()), (Some("revision:4f2a9c1"), Some(2)));

    // 綁定 Task：需求版本是 Task 的契約版本，受驗 Run 是它最新的 Run
    let tid = w.task("新增支出的 API").await;
    let (rid, _) = w.dispatch(&tid).await;
    let bundle = w.verify(json!({ "task_id": tid, "journeys": ["record-expense"] })).await;
    assert_eq!((bundle["task_id"].as_str(), bundle["requirement_version"].as_i64()), (Some(tid.as_str()), Some(1)));
    assert_eq!((bundle["run_id"].as_str(), bundle["generation"].as_i64()), (Some(rid.as_str()), Some(1)));
    assert_ne!(bundle["subject_dir"].as_str(), Some(w.dir.to_string_lossy().as_ref()), "在 Task 的工作目錄中驗證");
    // 其他 Workspace 的 Task 不能指定
    let (status, body) = w.h().send_json(Method::POST, &w.path("/runs"), Some(json!({ "task_id": "task_missing" }))).await;
    assert_eq!((status, error_code(&body)), (StatusCode::NOT_FOUND, "task_not_found"), "{body}");
}

// --- 驗收條件：清理只移除本 Run 的狀態並保留證據 ---

#[tokio::test]
async fn cleanup_removes_only_this_verifications_state_and_keeps_the_evidence() {
    let w = world().await;
    w.publish(profile()).await;
    w.healthy();
    // 另一次驗證留下的狀態與專案自己的檔案
    let other = w.h().data_dir().join("verification").join("vreq_other");
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(other.join("ledger.db"), "其他驗證的資料").unwrap();
    std::fs::write(w.dir.join("ledger.db"), "專案的資料").unwrap();
    // 這次的旅程會失敗：清理不因失敗而略過
    w.h().verification_driver.script_response("GET", &format!("{API}/expenses"), 200, "[]");

    let request = w.request(json!({ "journeys": ["record-expense"] })).await;
    let state = w.state_dir(&request);
    let id = request["id"].as_str().unwrap();
    let bundle = w.bundle_of(&request).await;
    assert_eq!(bundle["verdict"], "product_failure");

    // 操作都綁定這次驗證的識別與狀態目錄
    let state_text = state.to_string_lossy().to_string();
    let ops = w.h().verification_driver.operations();
    assert!(ops.contains(&format!("start ledger serve --data {state_text}")), "{ops:?}");
    assert!(ops.contains(&format!("command ledger seed --run {id}")), "{ops:?}");
    assert!(ops.contains(&format!("command ledger purge --run {id}")), "teardown 執行了：{ops:?}");
    let purge = ops.iter().position(|o| o.starts_with("command ledger purge")).unwrap();
    let read_back = ops.iter().position(|o| o.starts_with("http GET http://127.0.0.1:8080/expenses")).unwrap();
    assert!(read_back < purge, "teardown 在旅程之後");

    // 只移除這次的狀態，服務已停止
    assert!(!exists(&state), "這次驗證的狀態目錄已移除");
    assert!(exists(&other.join("ledger.db")), "其他驗證的狀態不受影響");
    assert_eq!(std::fs::read_to_string(w.dir.join("ledger.db")).unwrap(), "專案的資料");
    assert!(w.h().verification_driver.running_services().is_empty());
    assert_eq!(bundle["cleanup"]["state_dir"].as_str(), Some(state_text.as_str()));
    assert_eq!((bundle["cleanup"]["state_dir_removed"].as_bool(), bundle["cleanup"]["service_stopped"].as_bool()), (Some(true), Some(true)));
    assert!(bundle["cleanup"]["problems"].as_array().unwrap().is_empty());
    let teardown = bundle["steps"].as_array().unwrap().iter().find(|s| s["phase"] == "teardown").unwrap();
    assert_eq!(teardown["verdict"], "passed");

    // 證據保留：清理之後仍讀得到每一步的判定與觀察
    let (status, again) = w.h().send_json(Method::GET, &w.path(&format!("/bundles/{}", bundle["id"].as_str().unwrap())), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(again, bundle);
    assert_eq!(step_of(&again, "read-back")["output"], "[]");

    // teardown 失敗時記錄問題，但不改變判定
    w.healthy();
    // 其餘 ledger 命令（teardown 的 purge）都失敗；資料準備另外指定為成功
    w.h().verification_driver.script_exit("ledger", &[], 1, "purge failed");
    let request = w.request(json!({ "journeys": ["report"] })).await;
    let id = request["id"].as_str().unwrap().to_string();
    w.h().verification_driver.script_exit("ledger", &["seed", "--run", &id], 0, "seeded");
    let bundle = w.bundle_of(&request).await;
    assert_eq!(bundle["verdict"], "passed", "{bundle}");
    assert!(!bundle["cleanup"]["problems"].as_array().unwrap().is_empty(), "{bundle}");
    assert!(!exists(&w.state_dir(&request)), "teardown 命令失敗時仍移除這次的狀態目錄");
}

// --- 驗證要求在 Runtime 重啟後仍會執行 ---

#[tokio::test]
async fn a_queued_verification_still_runs_after_a_runtime_restart() {
    let mut w = world().await;
    w.publish(profile()).await;
    w.healthy();
    let request = w.request(json!({ "journeys": ["record-expense"] })).await;
    let second = w.request(json!({ "journeys": ["report"] })).await;
    w.harness.restart().await;
    let bundle = w.bundle_of(&request).await;
    assert_eq!(bundle["verdict"], "passed", "{bundle}");
    // 同一刻排入的要求依排入順序執行
    assert_eq!(w.bundle_of(&second).await["verdict"], "passed");
    let ops = w.h().verification_driver.operations();
    let first_post = ops.iter().position(|o| o.starts_with("http POST")).unwrap();
    let report = ops.iter().position(|o| o == "command ledger report").unwrap();
    assert!(first_post < report, "{ops:?}");
    // 已完成的要求不會再執行一次
    let posts = operations_with(w.h(), "http POST").len();
    w.h().supervise().await;
    assert_eq!(operations_with(w.h(), "http POST").len(), posts);
}
