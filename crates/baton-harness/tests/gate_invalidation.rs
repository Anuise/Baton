//! 情境測試：Gate 失效與重驗（票 50，spec 使用者故事 23、73），經由 Runtime harness 的真實 HTTP API 驗證。
//!
//! 受驗的「真實介面」由 fake VerificationDriver 扮演（同票 45）：build 是受驗目錄的 revision，由測試以 `set_revision`
//! 模擬「程式碼改了」。Controller 與 Worker 是腳本化的。只斷言外部行為：工具與桌面 API 的回應、流程視圖、
//! 驗證要求與 Evidence Bundle、裁定中心、Controller 收到的工作事件。
//!
//! 每個測試名稱對應票 50 的驗收條件：
//! - build 改變後受影響 Gate 失效、獨立 Gate 不受影響（含票 45 驗證 R132 的觀察：驗證通過後再改程式碼，舊 Bundle 不能再用）：
//!   `a_build_change_invalidates_the_affected_gate_queues_reverification_and_leaves_independent_gates_open`
//! - 決策變更後過期的裁定請求被撤銷並重新整理（使用者答案改變既有假設時，受影響 Task 與成果被阻擋或重驗，獨立工作繼續）：
//!   `an_answer_that_overturns_an_assumption_revokes_stale_requests_and_blocks_or_reverifies_only_dependent_work`
//! - 完成條件不能被 Agent 改寫以讓舊成果通過：`completion_criteria_cannot_be_rewritten_by_an_agent_to_let_old_results_pass`
//! - Verification Profile 改變時受影響 Gate 失效：`a_new_verification_profile_invalidates_gates_that_relied_on_the_old_one`

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
    dir: PathBuf,
}

fn filled(text: &str) -> Value {
    json!({ "status": "filled", "text": text })
}

fn contract(done: &str) -> Value {
    json!({
        "objective": filled("讓記帳 API 可以新增支出"),
        "inputs": filled("既有的記帳 API"),
        "scope": filled("記帳 API"),
        "deliverables": filled("新增支出的端點"),
        "done_criteria": filled(done),
        "blocker_handling": filled("卡住時回報"),
        "required_capabilities": [],
    })
}

fn error_code(body: &Value) -> &str {
    body["error"].as_str().unwrap_or_default()
}

/// 只有一條代表旅程的記帳 API Profile；`memo` 是讀回時預期的內容（換一個值就是新的 Profile 版本）。
fn profile(memo: &str) -> Value {
    json!({
        "interface": "cli_api",
        "journeys": [{
            "key": "record-expense",
            "title": "記一筆支出並讀回",
            "representative": true,
            "steps": [{
                "key": "read-back",
                "title": "讀回",
                "operation": { "kind": "http", "method": "GET", "url": format!("{API}/expenses") },
                "assertions": [{ "kind": "status", "equals": 200 }, { "kind": "contains", "text": memo }],
            }],
        }],
        "feature_map": [{ "key": "expenses", "title": "記帳", "criterion": "新增的支出可以讀回", "journeys": ["record-expense"] }],
    })
}

fn stage_of<'a>(workflow: &'a Value, key: &str) -> &'a Value {
    workflow["stages"].as_array().unwrap().iter().find(|s| s["key"] == key).unwrap_or_else(|| panic!("沒有 Stage {key}：{workflow}"))
}

fn last_attempt(stage: &Value) -> &Value {
    stage["attempts"].as_array().unwrap().last().unwrap_or_else(|| panic!("Stage 沒有 attempt：{stage}"))
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
    let w = World { harness, ws, goal, dir };
    // 受驗的 API 讀得回「午餐」
    w.h().verification_driver.script_response("GET", &format!("{API}/expenses"), 200, r#"[{"memo":"午餐"}]"#);
    w.publish_profile("午餐").await;
    w.publish_gated_playbook().await;
    w
}

impl World {
    fn h(&self) -> &Harness {
        &self.harness
    }

    fn ws_path(&self, rest: &str) -> String {
        format!("{WORKSPACES}/{}{rest}", self.ws)
    }

    async fn publish_profile(&self, memo: &str) {
        let (status, body) = self.h().send_json(Method::POST, &self.ws_path("/verification/profile"), Some(profile(memo))).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }

    /// 發布「調查與研究」Playbook 的新版本，三個 Stage：使用者核准（裁定 Gate）→ 真實介面驗證（安全 Gate，Evidence Bundle）
    /// → 使用者驗收（裁定 Gate）。
    async fn publish_gated_playbook(&self) {
        let (status, catalog) = self.h().send_json(Method::GET, PLAYBOOKS, None).await;
        assert_eq!(status, StatusCode::OK, "{catalog}");
        let mut playbook =
            catalog["playbooks"].as_array().unwrap().iter().filter(|p| p["key"] == "investigation").last().unwrap()["definition"].clone();
        let base = playbook["stages"][0].clone();
        let stage = |key: &str, title: &str, gate: &str, requires: &str| {
            let mut stage = base.clone();
            stage["key"] = json!(key);
            stage["title"] = json!(title);
            stage["gate"] = json!(gate);
            stage["failure_route"] = json!({ "kind": "retry" });
            stage["exit_checks"] = json!([{ "key": format!("{key}_check"), "description": title, "requires": requires }]);
            stage
        };
        playbook["stages"] = json!([
            stage("approval", "使用者核准", "decision", "decision"),
            stage("real_interface", "真實介面驗證", "safety", "verification"),
            stage("sign_off", "使用者驗收", "decision", "decision"),
        ]);
        let (status, body) = self.h().send_json(Method::POST, PLAYBOOKS, Some(playbook)).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }

    /// 建立研究 Task（標為 ready）；`inputs` 可以換成引用裁定請求的「輸入與依據」。
    async fn task_with(&self, title: &str, inputs: Option<Value>) -> String {
        let mut contract = contract("新增的支出可以讀回");
        if let Some(inputs) = inputs {
            contract["inputs"] = inputs;
        }
        let body = json!({ "goal_id": self.goal, "title": title, "kind": "research", "contract": contract });
        let (status, task) = self.h().send_json(Method::POST, &self.ws_path("/tasks"), Some(body)).await;
        assert_eq!(status, StatusCode::CREATED, "{task}");
        let tid = task["id"].as_str().unwrap().to_string();
        let (status, ready) = self.h().send_json(Method::POST, &self.ws_path(&format!("/tasks/{tid}/ready")), None).await;
        assert_eq!(status, StatusCode::OK, "{ready}");
        tid
    }

    async fn task(&self, title: &str) -> String {
        self.task_with(title, None).await
    }

    /// 派工並讓 Supervisor 啟動它；回傳 Task 的輸出目錄（受驗目錄）。
    async fn dispatch(&self, tid: &str) -> PathBuf {
        let (status, run) = self.h().send_json(Method::POST, &self.ws_path(&format!("/tasks/{tid}/dispatch")), None).await;
        assert_eq!(status, StatusCode::CREATED, "{run}");
        self.h().supervise().await;
        self.dir.join(".baton").join("outputs").join(tid)
    }

    async fn controller_turn(&self, text: &str) -> String {
        let (status, body) = self.h().send_json(Method::POST, &self.ws_path("/conversation/messages"), Some(json!({ "text": text }))).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        self.h().clock.advance_millis(WINDOW_MS);
        self.h().supervise().await;
        let (status, view) = self.h().send_json(Method::GET, &self.ws_path("/conversation"), None).await;
        assert_eq!(status, StatusCode::OK, "{view}");
        let run = view["runs"].as_array().unwrap().last().cloned().unwrap_or_else(|| panic!("Controller 沒有被喚起：{view}"));
        self.h().agent_credential(run["id"].as_str().unwrap()).expect("Controller 程序啟動時應該收到 Controller 憑證")
    }

    /// Controller 向使用者提問（阻擋 `blocking`），回傳裁定請求識別。
    async fn ask(&self, controller: &str, question: &str, recommendation: &str, blocking: &[&str]) -> String {
        let body = json!({
            "question": question,
            "reason": "屬於產品方向",
            "options": [{ "id": "rest", "label": "REST" }, { "id": "grpc", "label": "gRPC" }],
            "recommendation": recommendation,
            "blocking": blocking,
        });
        let (status, asked) = self.h().call_tool(controller, "propose_decision_request", body).await;
        assert_eq!(status, StatusCode::CREATED, "{asked}");
        asked["decision_request_id"].as_str().unwrap().to_string()
    }

    async fn decision(&self, rid: &str) -> Value {
        let (status, view) = self.h().send_json(Method::GET, &self.ws_path(&format!("/decisions/{rid}")), None).await;
        assert_eq!(status, StatusCode::OK, "{view}");
        view
    }

    /// 使用者在桌面以「接受某個選項」回答。
    async fn answer(&self, rid: &str, option: &str) -> Value {
        let view = self.decision(rid).await;
        let body = json!({ "action": "accept", "option_id": option, "expected_revision": view["revision"] });
        let (status, answered) = self.h().send_json(Method::POST, &self.ws_path(&format!("/decisions/{rid}/answer")), Some(body)).await;
        assert_eq!(status, StatusCode::OK, "{answered}");
        answered
    }

    /// 一個已經由使用者回答（接受建議）的裁定請求，作為裁定 Gate 的證據。
    async fn answered_decision(&self, controller: &str, question: &str) -> String {
        let holder = self.task(&format!("等待「{question}」")).await;
        let rid = self.ask(controller, question, "rest", &[&holder]).await;
        self.answer(&rid, "rest").await;
        rid
    }

    async fn start_workflow(&self, controller: &str, tid: &str) -> Value {
        let body = json!({ "situation": "investigation", "task_id": tid, "rationale": "確認行為" });
        let (status, workflow) = self.h().call_tool(controller, "start_workflow", body).await;
        assert_eq!(status, StatusCode::OK, "{workflow}");
        workflow
    }

    /// 以一項證據回報 Workflow 目前的 Stage，回傳 (HTTP 狀態, 回應)。
    async fn record(&self, controller: &str, workflow: &Value, evidence: Value) -> (StatusCode, Value) {
        let check = format!("{}_check", workflow["current_stage"].as_str().unwrap());
        let mut item = evidence;
        item["check"] = json!(check);
        let body = json!({
            "workflow_id": workflow["id"],
            "stage": workflow["current_stage"],
            "expected_revision": workflow["revision"],
            "outcome": "passed",
            "evidence": [item],
        });
        self.h().call_tool(controller, "record_stage_result", body).await
    }

    async fn pass(&self, controller: &str, workflow: &Value, evidence: Value) -> Value {
        let (status, body) = self.record(controller, workflow, evidence).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["attempt"]["outcome"], "passed", "{body}");
        body["workflow"].clone()
    }

    /// Controller 要求對 Task 驗證，讓 Supervisor 執行，回傳 Evidence Bundle 識別。
    async fn verify(&self, controller: &str, tid: &str) -> String {
        let (status, request) = self.h().call_tool(controller, "run_verification", json!({ "task_id": tid })).await;
        assert_eq!(status, StatusCode::CREATED, "{request}");
        self.h().supervise().await;
        let request = self.request(request["id"].as_str().unwrap()).await;
        assert_eq!(request["status"], "completed", "{request}");
        let bundle = request["bundle_id"].as_str().unwrap().to_string();
        assert_eq!(self.bundle(&bundle).await["verdict"], "passed");
        bundle
    }

    async fn overview(&self) -> Value {
        let (status, body) = self.h().send_json(Method::GET, &self.ws_path("/verification"), None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body
    }

    async fn request(&self, id: &str) -> Value {
        let overview = self.overview().await;
        overview["requests"].as_array().unwrap().iter().find(|r| r["id"] == id).cloned().unwrap_or_else(|| panic!("沒有驗證要求 {id}"))
    }

    async fn bundle(&self, id: &str) -> Value {
        let (status, bundle) = self.h().send_json(Method::GET, &self.ws_path(&format!("/verification/bundles/{id}")), None).await;
        assert_eq!(status, StatusCode::OK, "{bundle}");
        bundle
    }

    /// 流程視圖中的 Workflow。
    async fn workflow(&self, id: &Value) -> Value {
        let (status, list) = self.h().send_json(Method::GET, &self.ws_path("/workflows"), None).await;
        assert_eq!(status, StatusCode::OK, "{list}");
        list["workflows"].as_array().unwrap().iter().find(|w| w["id"] == *id).cloned().unwrap_or_else(|| panic!("沒有 Workflow {id}"))
    }

    /// 走完「使用者核准」與「真實介面驗證」，停在「使用者驗收」。
    async fn to_sign_off(&self, controller: &str, tid: &str, approval: &str) -> Value {
        let workflow = self.start_workflow(controller, tid).await;
        let workflow = self.pass(controller, &workflow, json!({ "decision_request_id": approval })).await;
        let bundle = self.verify(controller, tid).await;
        let workflow = self.pass(controller, &workflow, json!({ "evidence_bundle_id": bundle })).await;
        assert_eq!(workflow["current_stage"], "sign_off");
        workflow
    }

    /// Controller 收到、尚未處理的工作事件內容。
    async fn work_events(&self) -> Vec<String> {
        let (status, view) = self.h().send_json(Method::GET, &self.ws_path("/conversation"), None).await;
        assert_eq!(status, StatusCode::OK, "{view}");
        view["turns"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|t| t["kind"] == "work_event")
            .map(|t| t["content"].as_str().unwrap_or_default().to_string())
            .collect()
    }

    async fn missing_codes(&self, controller: &str, tid: &str) -> (StatusCode, Vec<String>, Value) {
        let (status, body) = self.h().call_tool(controller, "dispatch_task", json!({ "task_id": tid })).await;
        let codes = body["missing"].as_array().map(|m| m.iter().map(|i| i["code"].as_str().unwrap().to_string()).collect()).unwrap_or_default();
        (status, codes, body)
    }
}

// --- 驗收條件：build 改變後受影響 Gate 失效、獨立 Gate 不受影響 ---

#[tokio::test]
async fn a_build_change_invalidates_the_affected_gate_queues_reverification_and_leaves_independent_gates_open() {
    let w = world().await;
    let a = w.task("新增支出的 API").await;
    let b = w.task("月報的 API").await;
    let dir_a = w.dispatch(&a).await;
    let dir_b = w.dispatch(&b).await;
    w.h().verification_driver.set_revision(&dir_a, "a1");
    w.h().verification_driver.set_revision(&dir_b, "b1");
    let controller = w.controller_turn("驗證兩個 API").await;
    let approval = w.answered_decision(&controller, "第一版用 REST 嗎？").await;
    let flow_a = w.to_sign_off(&controller, &a, &approval).await;
    let flow_b = w.to_sign_off(&controller, &b, &approval).await;
    let old_bundle = last_attempt(stage_of(&w.workflow(&flow_a["id"]).await, "real_interface"))["evidence"][0]["evidence_bundle_id"].clone();
    assert_eq!(last_attempt(stage_of(&w.workflow(&flow_a["id"]).await, "real_interface"))["build"], "a1", "通過時記下 build");

    // 驗證通過之後，A 的程式碼又改了
    w.h().verification_driver.set_revision(&dir_a, "a2");
    w.h().supervise().await;

    let now_a = w.workflow(&flow_a["id"]).await;
    assert_eq!((now_a["current_stage"].as_str(), now_a["status"].as_str()), (Some("real_interface"), Some("active")), "{now_a}");
    assert_eq!(now_a["resume_stage"], "sign_off");
    assert_eq!(stage_of(&now_a, "approval")["state"], "passed", "使用者的核准與 build 無關，不受影響");
    let invalidated = last_attempt(stage_of(&now_a, "real_interface"));
    assert_eq!((invalidated["outcome"].as_str(), invalidated["recorded_by"].as_str()), (Some("invalidated"), Some("runtime")));
    let reason = invalidated["missing"][0]["reason"].as_str().unwrap();
    assert!(reason.contains("a1") && reason.contains("a2"), "{reason}");
    // 另一個 Task 的 Gate 是獨立的：它的 build 沒變
    let now_b = w.workflow(&flow_b["id"]).await;
    assert_eq!((now_b["current_stage"].as_str(), now_b["revision"].as_i64()), (Some("sign_off"), flow_b["revision"].as_i64()));

    // 重驗工作：Runtime 自動排入同一個 Task 的驗證要求，在新的 build 上執行；Controller 收到工作事件
    let overview = w.overview().await;
    let reverify = overview["requests"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["requested_by"].as_str().unwrap_or_default().contains("gate_invalidated"))
        .cloned()
        .unwrap_or_else(|| panic!("應該自動排入重驗：{overview}"));
    assert_eq!((reverify["task_id"].as_str(), reverify["status"].as_str()), (Some(a.as_str()), Some("completed")));
    let new_bundle = reverify["bundle_id"].clone();
    assert_eq!(w.bundle(new_bundle.as_str().unwrap()).await["candidate_revision"], "a2");
    let events = w.work_events().await;
    assert!(events.iter().any(|e| e.contains("Gate 失效") && e.contains("真實介面驗證") && e.contains(&flow_a["id"].as_str().unwrap().to_string())), "{events:?}");
    assert!(!events.iter().any(|e| e.contains("Gate 失效") && e.contains(flow_b["id"].as_str().unwrap())), "{events:?}");

    // 舊的 Bundle 驗的是舊 build，不能再開啟 Gate（票 45 驗證 R132 的觀察）
    let (status, body) = w.record(&controller, &now_a, json!({ "evidence_bundle_id": old_bundle })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["attempt"]["outcome"], "incomplete", "{body}");
    assert_eq!(body["attempt"]["evidence"][0]["status"], "build_changed");
    // 新的 Bundle 通過後，仍有效的核准被沿用，直接回到原本的「使用者驗收」
    let again = w.pass(&controller, &body["workflow"], json!({ "evidence_bundle_id": new_bundle })).await;
    assert_eq!((again["current_stage"].as_str(), again["resume_stage"].as_str()), (Some("sign_off"), None));
    assert_eq!(stage_of(&w.workflow(&flow_a["id"]).await, "real_interface")["state"], "passed");

    // 之後 build 沒再變：不會重複失效
    w.h().supervise().await;
    assert_eq!(w.workflow(&flow_a["id"]).await["revision"], again["revision"]);
}

// --- 驗收條件：決策變更後過期的裁定請求被撤銷並重新整理 ---

#[tokio::test]
async fn an_answer_that_overturns_an_assumption_revokes_stale_requests_and_blocks_or_reverifies_only_dependent_work() {
    let w = world().await;
    let controller = w.controller_turn("規劃 API").await;
    let approval = w.answered_decision(&controller, "要開始做嗎？").await;
    // 協定還沒定：Controller 建議 REST，最後的整合等答案；依賴它的 API 實作先照建議（假設）進行
    let integration = w.task("最終整合").await;
    let protocol = w.ask(&controller, "API 用哪種協定？", "rest", &[&integration]).await;
    let built_on = json!({
        "status": "filled",
        "text": "照建議先用 REST",
        "references": [{ "kind": "decision", "target": protocol, "version": 1 }],
    });
    let api = w.task_with("記帳 API（假設用 REST）", Some(built_on)).await;
    let report = w.task("月報（與協定無關）").await;
    // 兩個 Task 各自有一個等待中的提問
    let about_api = w.ask(&controller, "API 的路徑要加 /v1 嗎？", "rest", &[&api]).await;
    let about_report = w.ask(&controller, "月報要含稅嗎？", "rest", &[&report]).await;
    // API 實作的成果已經通過驗證 Gate
    let flow = w.to_sign_off(&controller, &api, &approval).await;
    let events_before = w.work_events().await.len();

    // 使用者回答 gRPC：推翻了 REST 的假設
    w.answer(&protocol, "grpc").await;

    // 依據被推翻的假設的請求過期（撤銷），獨立的請求仍在等待
    let stale = w.decision(&about_api).await;
    assert_eq!(stale["status"], "superseded", "{stale}");
    assert!(stale["status_reason"].as_str().unwrap().contains("假設"), "{stale}");
    assert_eq!(w.decision(&about_report).await["status"], "pending");
    // 重新整理裁定中心時也看得到它已過期；答案本身照常保存
    let (status, list) = w.h().send_json(Method::GET, &w.ws_path("/decisions"), None).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(w.decision(&protocol).await["status"], "answered");
    // Controller 收到「假設被推翻」的工作事件，點名受影響的 Task、不提獨立的 Task
    let events = w.work_events().await;
    let notice = events.iter().skip(events_before).find(|e| e.contains("推翻")).unwrap_or_else(|| panic!("{events:?}"));
    assert!(notice.contains(&api) && !notice.contains(&report), "{notice}");

    // 受影響的 Task 被阻擋：契約依據失效，不能派工；獨立的 Task 只等它自己的提問
    let (status, codes, body) = w.missing_codes(&controller, &api).await;
    assert_eq!((status, codes), (StatusCode::CONFLICT, vec!["reference_invalid".to_string()]), "{body}");
    assert!(body["missing"][0]["message"].as_str().unwrap().contains("gRPC"), "{body}");
    let (_, codes, _) = w.missing_codes(&controller, &report).await;
    assert_eq!(codes, vec!["decision_pending".to_string()]);

    // 受影響的成果被重驗：在 REST 前提下通過的驗證 Gate 失效（使用者的核准不受影響），而且在更新契約前不能再通過
    w.h().supervise().await;
    let now = w.workflow(&flow["id"]).await;
    assert_eq!(now["current_stage"], "real_interface", "{now}");
    assert_eq!(stage_of(&now, "approval")["state"], "passed");
    assert!(last_attempt(stage_of(&now, "real_interface"))["missing"][0]["reason"].as_str().unwrap().contains("假設"));
    let bundle = w.verify(&controller, &api).await;
    let (_, body) = w.record(&controller, &now, json!({ "evidence_bundle_id": bundle })).await;
    assert_eq!(body["attempt"]["outcome"], "blocked", "{body}");
    assert!(body["workflow"]["blocked_reason"].as_str().unwrap().contains("更新契約"), "{body}");

    // Controller 依答案更新契約（依據改成使用者的答案）之後，在新契約下重驗即可通過
    let mut updated = contract("新增的支出可以讀回");
    updated["inputs"] = json!({ "status": "filled", "text": "使用者選了 gRPC", "references": [] });
    let (status, body) = w.h().call_tool(&controller, "update_task", json!({ "task_id": api, "contract": updated })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let bundle = w.verify(&controller, &api).await;
    let current = w.workflow(&flow["id"]).await;
    let passed = w.pass(&controller, &current, json!({ "evidence_bundle_id": bundle })).await;
    assert_eq!(passed["current_stage"], "sign_off");
}

// --- 驗收條件：完成條件不能被 Agent 改寫以讓舊成果通過 ---

#[tokio::test]
async fn completion_criteria_cannot_be_rewritten_by_an_agent_to_let_old_results_pass() {
    let w = world().await;
    let a = w.task("新增支出的 API").await;
    let dir = w.dispatch(&a).await;
    w.h().verification_driver.set_revision(&dir, "a1");
    let controller = w.controller_turn("驗證新增支出").await;
    let approval = w.answered_decision(&controller, "要開始做嗎？").await;
    let flow = w.to_sign_off(&controller, &a, &approval).await;
    let old_bundle = last_attempt(stage_of(&w.workflow(&flow["id"]).await, "real_interface"))["evidence"][0]["evidence_bundle_id"].clone();

    // Controller 想放寬完成條件：被拒絕並轉成裁定請求，契約與 Gate 都不變
    let looser = contract("API 回 200 就算完成");
    let (status, refused) = w.h().call_tool(&controller, "update_task", json!({ "task_id": a, "contract": looser })).await;
    assert_eq!((status, refused["status"].as_str()), (StatusCode::CONFLICT, Some("requires_decision")), "{refused}");
    w.h().supervise().await;
    let unchanged = w.workflow(&flow["id"]).await;
    assert_eq!((unchanged["current_stage"].as_str(), unchanged["revision"].as_i64()), (Some("sign_off"), flow["revision"].as_i64()));
    // Agent 也不能改驗收的 Profile（只有使用者能從桌面發布）
    let (status, body) =
        w.h().call_tool(&controller, "publish_verification_profile", json!({ "definition": profile("任何東西") })).await;
    assert!(status.is_client_error(), "{body}");

    // 使用者核准之後完成條件才改變；舊條件下通過的 Gate 隨即失效，舊的 Bundle 不能讓新條件通過
    let request = refused["decision_request_id"].as_str().unwrap();
    w.answer(request, "approve").await;
    let (status, body) = w.h().call_tool(&controller, "update_task", json!({ "task_id": a, "contract": looser })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    w.h().supervise().await;
    let now = w.workflow(&flow["id"]).await;
    assert_eq!(now["current_stage"], "real_interface", "{now}");
    assert_eq!(last_attempt(stage_of(&now, "real_interface"))["outcome"], "invalidated");
    let (_, body) = w.record(&controller, &now, json!({ "evidence_bundle_id": old_bundle })).await;
    assert_eq!((body["attempt"]["outcome"].as_str(), body["attempt"]["evidence"][0]["status"].as_str()), (Some("incomplete"), Some("not_passed")), "{body}");
    // 新條件下的重驗（Runtime 已自動排入並執行）才能通過
    let overview = w.overview().await;
    let reverify = overview["requests"].as_array().unwrap().iter().find(|r| r["requested_by"].as_str().unwrap_or_default().contains("gate_invalidated")).cloned().unwrap();
    assert_eq!(w.bundle(reverify["bundle_id"].as_str().unwrap()).await["requirement_version"], 2);
    let passed = w.pass(&controller, &body["workflow"], json!({ "evidence_bundle_id": reverify["bundle_id"] })).await;
    assert_eq!(passed["current_stage"], "sign_off");
}

// --- Verification Profile 改變時受影響 Gate 失效 ---

#[tokio::test]
async fn a_new_verification_profile_invalidates_gates_that_relied_on_the_old_one() {
    let w = world().await;
    let a = w.task("新增支出的 API").await;
    let controller = w.controller_turn("驗證新增支出").await;
    let approval = w.answered_decision(&controller, "要開始做嗎？").await;
    let flow = w.to_sign_off(&controller, &a, &approval).await;

    // 使用者發布新的 Profile：預期讀回「晚餐」，而真實介面只有「午餐」
    w.publish_profile("晚餐").await;
    w.h().supervise().await;

    let now = w.workflow(&flow["id"]).await;
    assert_eq!(now["current_stage"], "real_interface", "{now}");
    assert_eq!(stage_of(&now, "approval")["state"], "passed");
    assert!(last_attempt(stage_of(&now, "real_interface"))["missing"][0]["reason"].as_str().unwrap().contains("Profile"), "{now}");
    // 在新 Profile 下的重驗沒有通過：Gate 保持關閉
    let overview = w.overview().await;
    let reverify = overview["requests"].as_array().unwrap().iter().find(|r| r["requested_by"].as_str().unwrap_or_default().contains("gate_invalidated")).cloned().unwrap();
    assert_eq!((reverify["profile_version"].as_i64(), reverify["status"].as_str()), (Some(2), Some("completed")));
    let bundle = w.bundle(reverify["bundle_id"].as_str().unwrap()).await;
    assert_eq!(bundle["verdict"], "product_failure");
    let (_, body) = w.record(&controller, &now, json!({ "evidence_bundle_id": reverify["bundle_id"] })).await;
    assert_eq!(body["attempt"]["outcome"], "incomplete", "{body}");
}
