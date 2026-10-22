//! 情境測試：方法登錄、Playbook 路由與 Stage／Gate（票 43），經由 Runtime harness 的真實 HTTP API 驗證。
//!
//! Controller 是腳本化的（同票 17、42）：測試在 Controller 的一輪進行中，以那一輪啟動時收到的 Controller 憑證呼叫工具。
//! 證據是真的：成果由 Worker 以 `register_artifact` 登記，裁定由 Worker 以 `request_decision` 提出、使用者以桌面 API 回答。
//! 帳號額度由 fake QuotaObserver 的樣本產生（同票 30）。只斷言外部行為：工具與桌面 API 的回應、交給 Adapter 的 Prompt、
//! context receipt。
//!
//! 每個測試名稱對應票 43 的驗收條件：
//! - 功能、缺陷、調查分別走對應 Playbook；非開發工作不套用開發流程：
//!   `features_defects_and_investigations_each_follow_their_own_playbook_and_non_development_work_gets_no_development_flow`
//! - 缺少證據時 Stage 記錄 incomplete 或 blocked：
//!   `a_stage_without_evidence_is_recorded_as_incomplete_or_blocked_and_words_alone_never_advance_it`、
//!   `skipping_a_stage_keeps_the_reason_and_decision_or_safety_gates_cannot_be_skipped`
//! - Playbook 升級不改變進行中的 Workflow：`a_playbook_upgrade_does_not_change_a_workflow_already_in_progress`
//! - 換帳號不會讓 Workflow 從頭跑：`switching_accounts_continues_the_workflow_from_its_current_stage_instead_of_starting_over`

use baton_domain::agent::TOOL_REQUEST_DECISION;
use baton_domain::artifact::TOOL_REGISTER_ARTIFACT;
use baton_domain::Timestamp;
use baton_harness::Harness;
use baton_ports::{AdapterEvent, Clock, Confidence, QuotaSample, QuotaSource};
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

const WORKSPACES: &str = "/api/v1/workspaces";
const PLAYBOOKS: &str = "/api/v1/playbooks";
const CLOCK_START: Timestamp = Timestamp::from_millis(1_700_000_000_123);
/// 合併視窗（spec D-31）。
const WINDOW_MS: i64 = 5_000;
/// 額度觀測器的輪詢間隔（spec D-31）。
const QUOTA_POLL_MS: i64 = 5 * 60 * 1000;

// --- 共用輔助 ---

struct World {
    harness: Harness,
    /// 依建立順序：帳號 A、B
    accounts: Vec<String>,
    ws: String,
    goal: String,
}

fn filled(text: &str) -> Value {
    json!({ "status": "filled", "text": text })
}

fn contract() -> Value {
    json!({
        "objective": filled("完成工作"),
        "inputs": filled("既有的資料"),
        "scope": filled("這個 Workspace"),
        "deliverables": filled("報告"),
        "done_criteria": filled("報告已登記"),
        "blocker_handling": filled("卡住時回報"),
        "required_capabilities": [],
    })
}

async fn world(accounts: usize) -> World {
    let harness = Harness::builder().clock_at(CLOCK_START).start().await;
    let mut ids = Vec::new();
    for (index, name) in ["A", "B"].into_iter().take(accounts).enumerate() {
        ids.push(harness.add_account(&format!("帳號 {name}"), &format!("account{index}@example.com")).await);
    }
    let dir = harness.data_dir().join("projects").join("ledger");
    std::fs::create_dir_all(&dir).unwrap();
    let (status, body) = harness.send_json(Method::POST, WORKSPACES, Some(json!({ "path": dir.to_string_lossy() }))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let ws = body["id"].as_str().unwrap().to_string();
    let (status, goal) =
        harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/goals"), Some(json!({ "title": "記帳工具" }))).await;
    assert_eq!(status, StatusCode::CREATED, "{goal}");
    let goal = goal["id"].as_str().unwrap().to_string();
    World { harness, accounts: ids, ws, goal }
}

fn error_code(body: &Value) -> &str {
    body["error"].as_str().unwrap_or_default()
}

fn stage<'a>(workflow: &'a Value, key: &str) -> &'a Value {
    workflow["stages"].as_array().unwrap().iter().find(|s| s["key"] == key).unwrap_or_else(|| panic!("沒有 Stage {key}：{workflow}"))
}

fn stage_keys(workflow: &Value) -> Vec<String> {
    workflow["stages"].as_array().unwrap().iter().map(|s| s["key"].as_str().unwrap().to_string()).collect()
}

impl World {
    fn h(&self) -> &Harness {
        &self.harness
    }

    fn task_path(&self, tid: &str) -> String {
        format!("{WORKSPACES}/{}/tasks/{tid}", self.ws)
    }

    /// 建立契約完整、已 ready 的 Task。
    async fn task(&self, title: &str, kind: &str) -> String {
        let body = json!({ "goal_id": self.goal, "title": title, "kind": kind, "contract": contract() });
        let (status, task) = self.h().send_json(Method::POST, &format!("{WORKSPACES}/{}/tasks", self.ws), Some(body)).await;
        assert_eq!(status, StatusCode::CREATED, "{task}");
        let tid = task["id"].as_str().unwrap().to_string();
        let (status, ready) = self.h().send_json(Method::POST, &format!("{}/ready", self.task_path(&tid)), None).await;
        assert_eq!(status, StatusCode::OK, "{ready}");
        tid
    }

    /// 派工並讓 Supervisor 啟動它，回傳 (run_id, Worker 的 Agent 憑證)。
    async fn dispatch(&self, tid: &str) -> (String, String) {
        let (status, run) = self.h().send_json(Method::POST, &format!("{}/dispatch", self.task_path(tid)), None).await;
        assert_eq!(status, StatusCode::CREATED, "{run}");
        let rid = run["id"].as_str().unwrap().to_string();
        self.h().supervise().await;
        let token = self.h().agent_credential(&rid).expect("Run 啟動時應該拿到 Agent 憑證");
        (rid, token)
    }

    /// 一個執行中、負責產出證據的研究 Worker，回傳它的 Agent 憑證。
    async fn evidence_worker(&self) -> String {
        let tid = self.task("整理證據", "research").await;
        self.dispatch(&tid).await.1
    }

    /// Worker 登記一個成果，回傳成果識別。
    async fn artifact(&self, worker: &str, name: &str) -> String {
        let (status, body) =
            self.h().call_tool(worker, TOOL_REGISTER_ARTIFACT, json!({ "name": name, "content": format!("{name} 的內容") })).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        body["artifact_id"].as_str().unwrap().to_string()
    }

    /// 使用者送一則訊息、合併視窗過後喚起 Controller，回傳這一輪的 (Controller Run, Controller 憑證)。
    async fn controller_turn(&self, text: &str) -> (Value, String) {
        let path = format!("{WORKSPACES}/{}/conversation/messages", self.ws);
        let (status, body) = self.h().send_json(Method::POST, &path, Some(json!({ "text": text }))).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        self.h().clock.advance_millis(WINDOW_MS);
        self.h().supervise().await;
        let (status, view) = self.h().send_json(Method::GET, &format!("{WORKSPACES}/{}/conversation", self.ws), None).await;
        assert_eq!(status, StatusCode::OK, "{view}");
        let run = view["runs"].as_array().unwrap().last().cloned().unwrap_or_else(|| panic!("Controller 沒有被喚起：{view}"));
        let token = self.h().agent_credential(run["id"].as_str().unwrap()).expect("Controller 程序啟動時應該收到 Controller 憑證");
        (run, token)
    }

    async fn finish_turn(&self, run: &Value) {
        let run_id = run["id"].as_str().unwrap();
        let adapter = &self.h().agent_adapter;
        adapter.emit(run_id, AdapterEvent::Started { native_session_id: Some(format!("native-{run_id}")) });
        adapter.emit(run_id, AdapterEvent::Message { text: "好的".into() });
        adapter.emit(run_id, AdapterEvent::Exited { success: true });
        self.h().supervise().await;
    }

    /// fake QuotaObserver 回報某個帳號的用量；等輪詢到期後 Supervisor 處理一次。
    async fn observe(&self, profile: &str, used: f64) {
        let sample = QuotaSample {
            source: QuotaSource::Observed,
            observed_at: self.h().clock.now(),
            used: Some(used),
            reset_at: None,
            confidence: Confidence::Observed,
        };
        self.h().quota_observer.set_sample(profile, sample);
        self.h().clock.advance_millis(QUOTA_POLL_MS);
        self.h().supervise().await;
    }

    async fn start(&self, token: &str, body: Value) -> (StatusCode, Value) {
        self.h().call_tool(token, "start_workflow", body).await
    }

    /// 預期成功的開始，回傳 Workflow 視圖。
    async fn started(&self, token: &str, body: Value) -> Value {
        let (status, workflow) = self.start(token, body).await;
        assert_eq!(status, StatusCode::OK, "{workflow}");
        workflow
    }

    /// 對 Workflow 目前的 Stage 回報結果。
    async fn record(&self, token: &str, workflow: &Value, outcome: &str, evidence: Value, note: &str) -> (StatusCode, Value) {
        let body = json!({
            "workflow_id": workflow["id"],
            "stage": workflow["current_stage"],
            "expected_revision": workflow["revision"],
            "outcome": outcome,
            "evidence": evidence,
            "note": note,
        });
        self.h().call_tool(token, "record_stage_result", body).await
    }

    /// 以一個新登記的成果通過目前 Stage 的單一 artifact exit check，回傳更新後的 Workflow。
    async fn pass_with_artifact(&self, token: &str, worker: &str, workflow: &Value) -> Value {
        let current = stage(workflow, workflow["current_stage"].as_str().unwrap());
        let mut evidence = Vec::new();
        for check in current["exit_checks"].as_array().unwrap() {
            assert_eq!(check["requires"], "artifact", "{check}");
            let name = format!("{}-{}-{}.md", workflow["id"].as_str().unwrap(), current["key"].as_str().unwrap(), check["key"].as_str().unwrap());
            evidence.push(json!({ "check": check["key"], "artifact_id": self.artifact(worker, &name).await }));
        }
        let (status, body) = self.record(token, workflow, "passed", json!(evidence), "").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["attempt"]["outcome"], "passed", "{body}");
        body["workflow"].clone()
    }

    async fn workflows(&self) -> Vec<Value> {
        let (status, body) = self.h().send_json(Method::GET, &format!("{WORKSPACES}/{}/workflows", self.ws), None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body["workflows"].as_array().unwrap().clone()
    }

    async fn workflow(&self, id: &Value) -> Value {
        self.workflows().await.into_iter().find(|w| &w["id"] == id).unwrap_or_else(|| panic!("找不到 Workflow {id}"))
    }

    async fn skip(&self, workflow: &Value, stage: &str, reason: &str) -> (StatusCode, Value) {
        let path = format!("{WORKSPACES}/{}/workflows/{}/skip", self.ws, workflow["id"].as_str().unwrap());
        let body = json!({ "stage": stage, "expected_revision": workflow["revision"], "reason": reason });
        self.h().send_json(Method::POST, &path, Some(body)).await
    }
}

// --- 驗收條件：功能、缺陷、調查分別走對應 Playbook；非開發工作不套用開發流程 ---

#[tokio::test]
async fn features_defects_and_investigations_each_follow_their_own_playbook_and_non_development_work_gets_no_development_flow() {
    let w = world(1).await;
    let worker = w.evidence_worker().await;
    let (_, token) = w.controller_turn("加上匯出功能、修正匯出錯誤，並調查競品").await;
    let feature = w.task("新增 CSV 匯出", "development").await;
    let defect = w.task("修正匯出的金額錯誤", "development").await;
    let research = w.task("調查競品的匯出格式", "research").await;
    let document = w.task("撰寫匯出說明", "document").await;

    let cases = [
        (&feature, "feature", "功能開發", "requirements_contract", "使用者要求新功能"),
        (&defect, "defect", "缺陷處理", "reproduction", "使用者回報金額錯誤"),
        (&research, "investigation", "調查與研究", "question", "要先了解競品"),
    ];
    for (task, situation, title, first, rationale) in cases {
        let workflow = w.started(&token, json!({ "situation": situation, "task_id": task, "rationale": rationale })).await;
        assert_eq!(workflow["playbook_key"], situation);
        assert_eq!((workflow["playbook_title"].as_str(), workflow["playbook_version"].as_i64()), (Some(title), Some(1)));
        assert_eq!((workflow["status"].as_str(), workflow["current_stage"].as_str()), (Some("active"), Some(first)));
        assert_eq!(workflow["task_id"], task.as_str());
        let reason = workflow["route_reason"].as_str().unwrap();
        assert!(reason.contains(title) && reason.contains("第 1 版") && reason.contains(rationale), "Router 記下理由與版本：{reason}");
        assert_eq!(workflow["stages"][0]["state"], "current");
        // 每個 Stage 宣告契約
        for s in workflow["stages"].as_array().unwrap() {
            for field in ["entry_conditions", "expected_outputs", "allowed_actions", "exit_checks"] {
                assert!(!s[field].as_array().unwrap().is_empty(), "{field}：{s}");
            }
            assert!(s["budget"]["max_attempts"].as_i64().unwrap() >= 1 && s["failure_route"]["kind"].is_string(), "{s}");
        }
    }
    let all = w.workflows().await;
    let of = |key: &str| all.iter().find(|wf| wf["playbook_key"] == key).unwrap().clone();
    assert!(stage_keys(&of("feature")).contains(&"real_interface_verification".to_string()));
    assert!(stage_keys(&of("defect")).contains(&"regression_verification".to_string()));
    // 調查流程不含實作：沒有任何 Stage 允許修改程式
    let investigation = of("investigation");
    for s in investigation["stages"].as_array().unwrap() {
        assert!(!s["allowed_actions"].as_array().unwrap().iter().any(|a| a == "修改程式"), "{s}");
    }

    // 研究與文件 Task 不套用開發流程
    for (task, situation) in [(&research, "feature"), (&document, "defect"), (&document, "performance")] {
        let (status, body) = w.start(&token, json!({ "situation": situation, "task_id": task, "rationale": "試試看" })).await;
        assert_eq!((status, error_code(&body)), (StatusCode::UNPROCESSABLE_ENTITY, "playbook_not_applicable"), "{body}");
    }
    let docs = w.started(&token, json!({ "situation": "investigation", "task_id": document, "rationale": "整理既有行為" })).await;
    assert_eq!(docs["playbook_key"], "investigation");
    // 同一個 Task 同時只走一個流程
    let (status, body) = w.start(&token, json!({ "situation": "feature", "task_id": feature, "rationale": "再開一次" })).await;
    assert_eq!((status, error_code(&body)), (StatusCode::CONFLICT, "workflow_exists"), "{body}");
    // 需求仍模糊的目標可以只綁 Goal
    let ambiguous = w.started(&token, json!({ "situation": "ambiguous", "goal_id": w.goal, "rationale": "成功條件不清楚" })).await;
    assert!(ambiguous["task_id"].is_null());

    // Controller 讀得到七個 Playbook 與它們是不是開發流程
    let (status, listed) = w.h().call_tool(&token, "get_workflows", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    let playbooks = listed["playbooks"].as_array().unwrap();
    assert_eq!(playbooks.len(), 7);
    let development = |key: &str| playbooks.iter().find(|p| p["key"] == key).unwrap()["development"].as_bool().unwrap();
    assert!(development("feature") && development("defect") && !development("investigation") && !development("ambiguous"));
    assert_eq!(listed["workflows"].as_array().unwrap().len(), 5);

    // Worker 不能選流程
    let (status, body) = w.start(&worker, json!({ "situation": "feature", "task_id": feature, "rationale": "我來" })).await;
    assert_eq!((status, error_code(&body)), (StatusCode::FORBIDDEN, "controller_only"), "{body}");
}

// --- 驗收條件：缺少證據時 Stage 記錄 incomplete 或 blocked ---

#[tokio::test]
async fn a_stage_without_evidence_is_recorded_as_incomplete_or_blocked_and_words_alone_never_advance_it() {
    let w = world(1).await;
    let worker = w.evidence_worker().await;
    let (_, token) = w.controller_turn("需求還很模糊，先釐清").await;
    let mut workflow = w.started(&token, json!({ "situation": "ambiguous", "goal_id": w.goal, "rationale": "成功條件不清楚" })).await;

    // 只有文字宣告：incomplete，不推進
    let (status, body) = w.record(&token, &workflow, "passed", json!([]), "現況已經讀完了").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["attempt"]["outcome"], "incomplete");
    assert_eq!(body["attempt"]["missing"][0]["check"], "current_state");
    assert_eq!(body["workflow"]["current_stage"], "read_current_state");
    workflow = body["workflow"].clone();
    // 引用不存在的成果：仍是 incomplete，並保留查驗結果
    let (_, body) = w.record(&token, &workflow, "passed", json!([{ "check": "current_state", "artifact_id": "art_missing" }]), "").await;
    assert_eq!(body["attempt"]["outcome"], "incomplete", "{body}");
    assert_eq!(body["attempt"]["evidence"][0]["status"], "not_found");
    assert_eq!(body["workflow"]["current_stage"], "read_current_state");
    workflow = body["workflow"].clone();
    // 證據種類不對：拒絕
    let (status, body) = w.record(&token, &workflow, "passed", json!([{ "check": "current_state", "decision_request_id": "dr_1" }]), "").await;
    assert_eq!((status, error_code(&body)), (StatusCode::UNPROCESSABLE_ENTITY, "workflow_invalid"), "{body}");
    // 已登記的成果才推進
    workflow = w.pass_with_artifact(&token, &worker, &workflow).await;
    assert_eq!(workflow["current_stage"], "sharp_questions");
    assert_eq!(stage(&workflow, "read_current_state")["state"], "passed");
    let attempts = stage(&workflow, "read_current_state")["attempts"].as_array().unwrap().clone();
    let outcomes: Vec<&str> = attempts.iter().map(|a| a["outcome"].as_str().unwrap()).collect();
    assert_eq!(outcomes, ["incomplete", "incomplete", "passed"], "同一個 Stage 的多次 attempt 都保留");
    workflow = w.pass_with_artifact(&token, &worker, &workflow).await;
    workflow = w.pass_with_artifact(&token, &worker, &workflow).await;
    assert_eq!(workflow["current_stage"], "decision");
    assert_eq!(stage(&workflow, "decision")["gate"], "decision");

    // 裁定 Gate：裁定尚未回答 → blocked
    let question = json!({
        "question": "第一版只做網頁嗎？",
        "reason": "影響範圍",
        "options": [{ "id": "web", "label": "只做網頁" }, { "id": "all", "label": "全平台" }],
        "recommendation": "web",
        "evidence": ["研究結果"],
        "urgency": "normal",
    });
    let (status, asked) = w.h().call_tool(&worker, TOOL_REQUEST_DECISION, question).await;
    assert!(status.is_success(), "{asked}");
    let request = asked["decision_request_id"].as_str().unwrap().to_string();
    let decision = json!([{ "check": "decision", "decision_request_id": request }]);
    let (_, body) = w.record(&token, &workflow, "passed", decision.clone(), "").await;
    assert_eq!(body["attempt"]["outcome"], "blocked", "{body}");
    assert_eq!(body["attempt"]["evidence"][0]["status"], "pending");
    assert_eq!((body["workflow"]["status"].as_str(), body["workflow"]["current_stage"].as_str()), (Some("blocked"), Some("decision")));
    assert!(body["workflow"]["blocked_reason"].as_str().unwrap().contains("等待裁定"));
    workflow = body["workflow"].clone();
    assert_eq!(stage(&w.workflow(&workflow["id"]).await, "decision")["state"], "blocked");

    // 使用者回答之後，同一份證據才通過
    let path = format!("{WORKSPACES}/{}/decisions/{request}", w.ws);
    let (_, view) = w.h().send_json(Method::GET, &path, None).await;
    let (status, answered) =
        w.h().send_json(Method::POST, &format!("{path}/answer"), Some(json!({ "option_id": "web", "expected_revision": view["revision"] }))).await;
    assert_eq!(status, StatusCode::OK, "{answered}");
    let (_, body) = w.record(&token, &workflow, "passed", decision, "").await;
    assert_eq!(body["attempt"]["outcome"], "passed", "{body}");
    assert_eq!((body["workflow"]["status"].as_str(), body["workflow"]["current_stage"].as_str()), (Some("active"), Some("update_brief")));
    workflow = body["workflow"].clone();

    // Controller 回報無法繼續（例如沒有獨立 reviewer）：需要原因，記為 blocked
    let (status, body) = w.record(&token, &workflow, "blocked", json!([]), "").await;
    assert_eq!((status, error_code(&body)), (StatusCode::UNPROCESSABLE_ENTITY, "workflow_invalid"), "{body}");
    let (_, body) = w.record(&token, &workflow, "blocked", json!([]), "詞彙需要使用者確認").await;
    assert_eq!((body["attempt"]["outcome"].as_str(), body["workflow"]["status"].as_str()), (Some("blocked"), Some("blocked")));
    workflow = body["workflow"].clone();
    // 已通過的 Stage 不重跑；舊的 revision 不能回報
    let mut stale = workflow.clone();
    stale["current_stage"] = json!("read_current_state");
    let (status, body) = w.record(&token, &stale, "passed", json!([]), "").await;
    assert_eq!((status, error_code(&body)), (StatusCode::CONFLICT, "stage_not_current"), "{body}");
    let mut stale = workflow.clone();
    stale["revision"] = json!(1);
    let (status, body) = w.record(&token, &stale, "passed", json!([]), "").await;
    assert_eq!((status, error_code(&body)), (StatusCode::CONFLICT, "workflow_revision_mismatch"), "{body}");

    workflow = w.pass_with_artifact(&token, &worker, &workflow).await;
    assert_eq!((workflow["status"].as_str(), workflow["current_stage"].as_str()), (Some("completed"), None));
    let states: Vec<&str> = workflow["stages"].as_array().unwrap().iter().map(|s| s["state"].as_str().unwrap()).collect();
    assert_eq!(states, ["passed"; 5]);
}

#[tokio::test]
async fn skipping_a_stage_keeps_the_reason_and_decision_or_safety_gates_cannot_be_skipped() {
    let w = world(1).await;
    let worker = w.evidence_worker().await;
    let (_, token) = w.controller_turn("新增匯出功能").await;
    let task = w.task("新增 CSV 匯出", "development").await;
    let mut workflow = w.started(&token, json!({ "situation": "feature", "task_id": task, "rationale": "新功能" })).await;

    // 沒有原因不能跳過
    let (status, body) = w.skip(&workflow, "requirements_contract", "  ").await;
    assert_eq!((status, error_code(&body)), (StatusCode::UNPROCESSABLE_ENTITY, "workflow_invalid"), "{body}");
    // 使用者附原因跳過一般 Stage：原因保存在 attempt 上
    let (status, body) = w.skip(&workflow, "requirements_contract", "需求契約已寫在 issue 中").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    workflow = body;
    let skipped = stage(&workflow, "requirements_contract");
    assert_eq!(skipped["state"], "skipped");
    assert_eq!(skipped["attempts"][0]["outcome"], "skipped");
    assert_eq!(skipped["attempts"][0]["note"], "需求契約已寫在 issue 中");
    assert_eq!(skipped["attempts"][0]["recorded_by"], "desktop");
    assert_eq!(workflow["current_stage"], "design");

    // 前進到安全 Gate（真實介面驗證）
    while workflow["current_stage"] != "real_interface_verification" {
        workflow = w.pass_with_artifact(&token, &worker, &workflow).await;
    }
    assert_eq!(stage(&workflow, "real_interface_verification")["gate"], "safety");
    let (status, body) = w.skip(&workflow, "real_interface_verification", "測試已經很完整").await;
    assert_eq!((status, error_code(&body)), (StatusCode::CONFLICT, "stage_not_skippable"), "{body}");
    // 失敗依 failure_route 回到實作
    let (_, body) = w.record(&token, &workflow, "failed", json!([]), "匯出的檔案在 Excel 中亂碼").await;
    assert_eq!(body["attempt"]["outcome"], "failed", "{body}");
    assert_eq!(body["workflow"]["current_stage"], "tests_and_implementation");
    workflow = body["workflow"].clone();
    workflow = w.pass_with_artifact(&token, &worker, &workflow).await;
    workflow = w.pass_with_artifact(&token, &worker, &workflow).await;
    // 獨立審查需要非作者 reviewer 的雙軸審查（票 44）：證據 Worker 是另一個 Task，可以擔任 reviewer
    assert_eq!(workflow["current_stage"], "independent_review");
    let axis = json!({ "summary": "已審查", "paths": ["src"], "findings": [] });
    let review = json!({
        "subject_task_id": task,
        "base_revision": "base",
        "candidate_revision": "c1",
        "spec_version": 1,
        "standards_source": "AGENTS.md",
        "standards": axis,
        "spec": axis,
    });
    let (status, reviewed) = w.h().call_tool(&worker, "submit_review", review).await;
    assert_eq!(status, StatusCode::CREATED, "{reviewed}");
    let (_, body) = w.record(&token, &workflow, "passed", json!([{ "check": "review", "review_id": reviewed["id"] }]), "").await;
    assert_eq!(body["attempt"]["outcome"], "passed", "{body}");
    workflow = body["workflow"].clone();
    // 整合是安全 Gate，需要 Task 通過成果接受
    assert_eq!(workflow["current_stage"], "integration");
    let (status, body) = w.skip(&workflow, "integration", "直接合併").await;
    assert_eq!((status, error_code(&body)), (StatusCode::CONFLICT, "stage_not_skippable"), "{body}");
    let (_, body) = w.record(&token, &workflow, "passed", json!([{ "check": "accepted", "task_id": task }]), "").await;
    assert_eq!(body["attempt"]["outcome"], "incomplete", "{body}");
    assert_eq!(body["attempt"]["evidence"][0]["status"], "not_accepted");
}

// --- 驗收條件：Playbook 升級不改變進行中的 Workflow ---

#[tokio::test]
async fn a_playbook_upgrade_does_not_change_a_workflow_already_in_progress() {
    let mut w = world(1).await;
    let worker = w.evidence_worker().await;
    let (_, token) = w.controller_turn("修正匯出錯誤").await;
    let first = w.task("修正金額錯誤", "development").await;
    let mut running = w.started(&token, json!({ "situation": "defect", "task_id": first, "rationale": "使用者回報" })).await;
    running = w.pass_with_artifact(&token, &worker, &running).await;
    assert_eq!(running["current_stage"], "minimization");
    let v1_stages = stage_keys(&running);

    // 使用者發布缺陷處理的第 2 版：拿掉「最小化」、新增「影響評估」
    let (status, catalog) = w.h().send_json(Method::GET, PLAYBOOKS, None).await;
    assert_eq!(status, StatusCode::OK, "{catalog}");
    let v1 = catalog["playbooks"].as_array().unwrap().iter().find(|p| p["key"] == "defect").unwrap().clone();
    assert!(catalog["methods"].as_array().unwrap().iter().any(|m| m["key"] == "reproduction" && m["version"] == 1));
    let mut definition = v1["definition"].clone();
    let stages = definition["stages"].as_array_mut().unwrap();
    stages.retain(|s| s["key"] != "minimization");
    let mut impact = stages[0].clone();
    impact["key"] = json!("impact_assessment");
    impact["title"] = json!("影響評估");
    impact["exit_checks"] = json!([{ "key": "impact", "description": "影響範圍", "requires": "artifact" }]);
    stages.insert(1, impact);
    let (status, published) = w.h().send_json(Method::POST, PLAYBOOKS, Some(definition.clone())).await;
    assert_eq!(status, StatusCode::CREATED, "{published}");
    assert_eq!((published["created"].as_bool(), published["playbook"]["version"].as_i64()), (Some(true), Some(2)));
    // 相同內容再發布不產生新版本；不合法的版本被拒絕
    let (status, again) = w.h().send_json(Method::POST, PLAYBOOKS, Some(definition.clone())).await;
    assert_eq!((status, again["created"].as_bool(), again["playbook"]["version"].as_i64()), (StatusCode::OK, Some(false), Some(2)));
    let mut broken = definition.clone();
    broken["stages"][0]["exit_checks"] = json!([]);
    let (status, body) = w.h().send_json(Method::POST, PLAYBOOKS, Some(broken)).await;
    assert_eq!((status, error_code(&body)), (StatusCode::UNPROCESSABLE_ENTITY, "playbook_invalid"), "{body}");

    // 舊版本沒有被覆寫
    let (_, catalog) = w.h().send_json(Method::GET, PLAYBOOKS, None).await;
    let defects: Vec<&Value> = catalog["playbooks"].as_array().unwrap().iter().filter(|p| p["key"] == "defect").collect();
    assert_eq!(defects.len(), 2);
    assert_eq!(defects[0], &v1, "第 1 版內容與雜湊不變");

    // 進行中的 Workflow 維持第 1 版：Stage 不變，仍在「最小化」，並看得到已有新版
    let current = w.workflow(&running["id"]).await;
    assert_eq!((current["playbook_version"].as_i64(), current["latest_playbook_version"].as_i64()), (Some(1), Some(2)));
    assert_eq!(stage_keys(&current), v1_stages);
    assert_eq!(current["current_stage"], "minimization");
    let next = w.pass_with_artifact(&token, &worker, &current).await;
    assert_eq!(next["current_stage"], "hypothesis", "依第 1 版的順序推進");

    // 之後開始的 Workflow 才選用第 2 版
    let second = w.task("修正日期錯誤", "development").await;
    let upgraded = w.started(&token, json!({ "situation": "defect", "task_id": second, "rationale": "又一個缺陷" })).await;
    assert_eq!(upgraded["playbook_version"], 2);
    assert!(upgraded["route_reason"].as_str().unwrap().contains("第 2 版"));
    assert_eq!(stage_keys(&upgraded)[1], "impact_assessment");

    // 重啟 Runtime：內建內容沒變不新增版本，使用者的第 2 版仍是最新；進行中的 Workflow 仍是第 1 版
    w.harness.restart().await;
    let (_, catalog) = w.h().send_json(Method::GET, PLAYBOOKS, None).await;
    assert_eq!(catalog["playbooks"].as_array().unwrap().iter().filter(|p| p["key"] == "defect").count(), 2);
    assert_eq!(w.workflow(&running["id"]).await["playbook_version"], 1);
}

// --- 驗收條件：換帳號不會讓 Workflow 從頭跑 ---

#[tokio::test]
async fn switching_accounts_continues_the_workflow_from_its_current_stage_instead_of_starting_over() {
    let mut w = world(2).await;
    let (a, b) = (w.accounts[0].clone(), w.accounts[1].clone());
    let worker = w.evidence_worker().await;
    let research = w.task("調查競品的匯出格式", "research").await;

    // 第一輪在帳號 A：開始調查流程並通過前兩個 Stage
    let (first, token) = w.controller_turn("調查競品的匯出格式").await;
    assert_eq!(first["profile_id"], a.as_str());
    let mut workflow = w.started(&token, json!({ "situation": "investigation", "task_id": research, "rationale": "要先了解" })).await;
    workflow = w.pass_with_artifact(&token, &worker, &workflow).await;
    workflow = w.pass_with_artifact(&token, &worker, &workflow).await;
    assert_eq!(workflow["current_stage"], "evidence_uncertainty");
    w.finish_turn(&first).await;

    // A 接近上限：下一輪換到 B，新的 Session 從持久化的目前 Stage 接續
    w.observe(&a, 0.9).await;
    let (second, token) = w.controller_turn("繼續").await;
    assert_eq!(second["profile_id"], b.as_str());
    assert!(second["handoff"]["from_profile_id"] == a.as_str(), "{second}");
    let prompt = second["prompt"].as_str().unwrap();
    assert!(prompt.contains("## 進行中的 Workflow"), "{prompt}");
    assert!(prompt.contains("目前 Stage「證據與不確定性」（evidence_uncertainty）"), "{prompt}");
    assert!(prompt.contains("已完成：明確問題、一手來源／實驗"), "{prompt}");

    let (status, listed) = w.h().call_tool(&token, "get_workflows", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    let resumed = listed["workflows"][0].clone();
    assert_eq!((resumed["current_stage"].as_str(), resumed["revision"].clone()), (Some("evidence_uncertainty"), workflow["revision"].clone()));
    // 已通過的 Stage 不從頭再跑
    let mut restart = resumed.clone();
    restart["current_stage"] = json!("question");
    let (status, body) = w.record(&token, &restart, "passed", json!([]), "").await;
    assert_eq!((status, error_code(&body)), (StatusCode::CONFLICT, "stage_not_current"), "{body}");
    workflow = w.pass_with_artifact(&token, &worker, &resumed).await;
    assert_eq!(workflow["current_stage"], "conclusion");
    let profile = |key: &str| stage(&workflow, key)["attempts"][0]["profile_id"].clone();
    assert_eq!((profile("question"), profile("evidence_uncertainty")), (json!(a), json!(b)), "attempt 記下各自的帳號");
    w.finish_turn(&second).await;

    // Worker 的 Prompt 也從目前 Stage 開始（不是第一個 Stage），並記在 context receipt
    let (run, _) = w.dispatch(&research).await;
    let launched = w.h().agent_adapter.launched().into_iter().find(|s| s.run_id == run).unwrap();
    assert!(launched.prompt.contains("Stage「結論或裁定」（conclusion）"), "{}", launched.prompt);
    assert!(!launched.prompt.contains("Stage「明確問題」"), "{}", launched.prompt);
    let (status, context) = w.h().send_json(Method::GET, &format!("{WORKSPACES}/{}/runs/{run}/context", w.ws), None).await;
    assert_eq!(status, StatusCode::OK, "{context}");
    let item = context["receipt"]["items"].as_array().unwrap().iter().find(|i| i["kind"] == "stage").cloned().unwrap_or_else(|| panic!("{context}"));
    assert_eq!((item["ref_id"].clone(), item["status"].as_str()), (workflow["id"].clone(), Some("loaded")));

    // 重啟 Runtime 後仍在同一個 Stage
    w.harness.restart().await;
    assert_eq!(w.workflow(&workflow["id"]).await["current_stage"], "conclusion");
}

#[tokio::test]
async fn a_stage_whose_budget_is_used_up_only_continues_after_the_user_decides() {
    let w = world(1).await;
    let worker = w.evidence_worker().await;
    let (_, token) = w.controller_turn("新增匯出功能").await;
    let task = w.task("新增 CSV 匯出", "development").await;
    let mut workflow = w.started(&token, json!({ "situation": "feature", "task_id": task, "rationale": "新功能" })).await;
    let max = stage(&workflow, "requirements_contract")["budget"]["max_attempts"].as_i64().unwrap();
    for _ in 0..max {
        let (status, body) = w.record(&token, &workflow, "failed", json!([]), "需求還不清楚").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        workflow = body["workflow"].clone();
    }
    assert_eq!(workflow["status"], "blocked", "{workflow}");

    // 附齊證據回報 passed 也不能自行解除：預算用盡後的下一步必須由使用者裁定（review-notes #20）
    let name = format!("{}-requirements.md", workflow["id"].as_str().unwrap());
    let evidence = json!([{ "check": stage(&workflow, "requirements_contract")["exit_checks"][0]["key"], "artifact_id": w.artifact(&worker, &name).await }]);
    let (status, body) = w.record(&token, &workflow, "passed", evidence.clone(), "").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!((body["attempt"]["outcome"].as_str(), body["workflow"]["status"].as_str()), (Some("blocked"), Some("blocked")), "{body}");
    assert!(body["workflow"]["blocked_reason"].as_str().unwrap().contains("預算用盡後的下一步"), "{body}");
    workflow = body["workflow"].clone();
    let (status, list) = w.h().send_json(Method::GET, &format!("{WORKSPACES}/{}/decisions", w.ws), None).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let request = list["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["status"] == "pending" && d["authority"]["matter"] == "next_step_after_budget")
        .cloned()
        .unwrap_or_else(|| panic!("應該建立「預算用盡後的下一步」裁定請求：{list}"));
    assert!(workflow["blocked_reason"].as_str().unwrap().contains(request["id"].as_str().unwrap()), "{workflow}");
    // 等待裁定時再回報一次：回到同一個請求，不重複建立
    let (_, body) = w.record(&token, &workflow, "passed", evidence.clone(), "").await;
    assert_eq!(body["attempt"]["outcome"], "blocked", "{body}");
    workflow = body["workflow"].clone();

    // 使用者核准後，同樣的證據才能讓 Workflow 繼續
    let path = format!("{WORKSPACES}/{}/decisions/{}", w.ws, request["id"].as_str().unwrap());
    let (_, view) = w.h().send_json(Method::GET, &path, None).await;
    let body = json!({ "action": "accept", "option_id": "approve", "expected_revision": view["revision"] });
    let (status, answered) = w.h().send_json(Method::POST, &format!("{path}/answer"), Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{answered}");
    let (_, body) = w.record(&token, &workflow, "passed", evidence, "").await;
    assert_eq!((body["attempt"]["outcome"].as_str(), body["workflow"]["status"].as_str()), (Some("passed"), Some("active")), "{body}");
    assert_eq!(body["workflow"]["current_stage"], "design");
}
