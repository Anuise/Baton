//! 情境測試：雙軸 Review、獨立 reviewer 與設計偏離（票 44），經由 Runtime harness 的真實 HTTP API 驗證。
//!
//! Controller 是腳本化的（同票 17、42、43）：測試在 Controller CLI 開著時，以開窗時交給它的 Controller 憑證呼叫工具。
//! reviewer 是另一個 Task 的 Worker，以 `submit_review` 提交；Gate 由 Controller 以 `record_stage_result` 附審查證據回報，
//! Runtime 查驗。裁定由使用者以桌面 API 回答。兩個 candidate 之間的變更檔案由 fake Git 腳本化（正式版以 git diff 計算）。
//! 只斷言外部行為：工具與桌面 API 的回應、交給 Adapter 的 Prompt、context receipt、成果版本。
//!
//! 每個測試名稱對應票 44 的驗收條件：
//! - 需求符合但規範違反、規範符合但需求缺漏，兩者都不能通過 Gate：
//!   `requirement_met_but_standards_violated_or_standards_met_but_requirement_missing_never_opens_the_gate`
//! - 作者不能擔任自己工作的必要 reviewer：
//!   `the_author_cannot_be_the_required_reviewer_and_the_gate_stays_closed_until_enough_independent_reviewers`
//! - 修正後只有可證明不受影響的審查結果被沿用：`after_a_fix_only_review_results_provably_unaffected_are_reused`
//! - 設計偏離保存原因與接受者、接受後更新設計版本：
//!   `an_accepted_design_deviation_keeps_who_accepted_it_and_the_next_worker_gets_the_new_design_version`

use baton_domain::artifact::TOOL_REGISTER_ARTIFACT;
use baton_domain::Timestamp;
use baton_harness::Harness;
use baton_ports::AdapterEvent;
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

const WORKSPACES: &str = "/api/v1/workspaces";
const PLAYBOOKS: &str = "/api/v1/playbooks";
const CLOCK_START: Timestamp = Timestamp::from_millis(1_700_000_000_123);

// --- 共用輔助 ---

struct World {
    harness: Harness,
    ws: String,
    goal: String,
    /// Controller CLI 專用帳號的 Profile 識別（不拿來派 Worker）。
    controller_profile: String,
    dir: std::path::PathBuf,
}

fn filled(text: &str) -> Value {
    json!({ "status": "filled", "text": text })
}

fn contract(objective: &str) -> Value {
    json!({
        "objective": filled(objective),
        "inputs": filled("既有的匯出程式"),
        "scope": filled("這個 Workspace"),
        "deliverables": filled("匯出功能"),
        "done_criteria": filled("匯出的 CSV 有標頭"),
        "blocker_handling": filled("卡住時回報"),
        "required_capabilities": [],
    })
}

/// 四個 Worker 帳號（作者、證據與 reviewer 的 Worker 同時執行也有容量）加上 Controller CLI 專用的帳號，Workspace 是 Git repo。
async fn world() -> World {
    let harness = Harness::builder().clock_at(CLOCK_START).start().await;
    for (index, name) in ["A", "B", "C", "D"].into_iter().enumerate() {
        harness.add_account(&format!("帳號 {name}"), &format!("account{index}@example.com")).await;
    }
    let controller_profile = harness.add_account("Controller 帳號", "controller@example.com").await;
    let dir = harness.data_dir().join("projects").join("ledger");
    std::fs::create_dir_all(&dir).unwrap();
    let (status, body) = harness.send_json(Method::POST, WORKSPACES, Some(json!({ "path": dir.to_string_lossy() }))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let ws = body["id"].as_str().unwrap().to_string();
    let dir = std::path::PathBuf::from(body["path"].as_str().unwrap());
    harness.workspace_fs.init_repo(&dir, "main");
    let (status, goal) =
        harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/goals"), Some(json!({ "title": "記帳工具" }))).await;
    assert_eq!(status, StatusCode::CREATED, "{goal}");
    let goal = goal["id"].as_str().unwrap().to_string();
    World { harness, ws, goal, controller_profile, dir }
}

fn error_code(body: &Value) -> &str {
    body["error"].as_str().unwrap_or_default()
}

fn axis(summary: &str, paths: &[&str], findings: &[&str]) -> Value {
    let findings: Vec<Value> = findings.iter().map(|t| json!({ "title": t, "detail": format!("{t}：細節"), "location": "src/export.rs:10" })).collect();
    json!({ "summary": summary, "paths": paths, "findings": findings })
}

fn reuse(review_id: &Value) -> Value {
    json!({ "reuse_from": review_id })
}

fn review(subject: &str, candidate: &str, spec_version: i64, standards: Value, spec: Value) -> Value {
    json!({
        "subject_task_id": subject,
        "base_revision": "base",
        "candidate_revision": candidate,
        "spec_version": spec_version,
        "standards_source": "AGENTS.md@base",
        "standards": standards,
        "spec": spec,
    })
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
        let body = json!({ "goal_id": self.goal, "title": title, "kind": kind, "contract": contract("完成工作") });
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

    /// 另一個 Task 的 Worker：產出證據或擔任 reviewer。
    async fn worker(&self, title: &str) -> String {
        let tid = self.task(title, "research").await;
        self.dispatch(&tid).await.1
    }

    async fn artifact(&self, worker: &str, name: &str, content: &str) -> String {
        let (status, body) = self.h().call_tool(worker, TOOL_REGISTER_ARTIFACT, json!({ "name": name, "content": content })).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        body["artifact_id"].as_str().unwrap().to_string()
    }

    /// 以 Controller 帳號開啟這個 Workspace 的 Controller CLI，回傳 Controller 憑證。
    async fn open_controller(&self) -> String {
        self.h().open_controller_cli(&self.ws, &self.controller_profile).await.1
    }

    async fn start_feature(&self, token: &str, task: &str) -> Value {
        let (status, workflow) =
            self.h().call_tool(token, "start_workflow", json!({ "situation": "feature", "task_id": task, "rationale": "新功能" })).await;
        assert_eq!(status, StatusCode::OK, "{workflow}");
        workflow
    }

    async fn record(&self, token: &str, workflow: &Value, evidence: Value) -> Value {
        let body = json!({
            "workflow_id": workflow["id"],
            "stage": workflow["current_stage"],
            "expected_revision": workflow["revision"],
            "outcome": "passed",
            "evidence": evidence,
        });
        let (status, result) = self.h().call_tool(token, "record_stage_result", body).await;
        assert_eq!(status, StatusCode::OK, "{result}");
        result
    }

    /// 以登記的成果通過一般 Stage，直到「獨立審查」。
    async fn to_review_stage(&self, token: &str, worker: &str, mut workflow: Value) -> Value {
        while workflow["current_stage"] != "independent_review" {
            let current = workflow["current_stage"].as_str().unwrap().to_string();
            let stage = workflow["stages"].as_array().unwrap().iter().find(|s| s["key"] == current.as_str()).unwrap().clone();
            let mut evidence = Vec::new();
            for check in stage["exit_checks"].as_array().unwrap() {
                let name = format!("{}-{current}.md", workflow["id"].as_str().unwrap());
                evidence.push(json!({ "check": check["key"], "artifact_id": self.artifact(worker, &name, "內容").await }));
            }
            let result = self.record(token, &workflow, json!(evidence)).await;
            assert_eq!(result["attempt"]["outcome"], "passed", "{result}");
            workflow = result["workflow"].clone();
        }
        workflow
    }

    async fn submit(&self, reviewer: &str, body: Value) -> (StatusCode, Value) {
        self.h().call_tool(reviewer, "submit_review", body).await
    }

    async fn submitted(&self, reviewer: &str, body: Value) -> Value {
        let (status, review) = self.submit(reviewer, body).await;
        assert_eq!(status, StatusCode::CREATED, "{review}");
        review
    }

    async fn classify(&self, token: &str, finding: &Value, classification: &str, reason: &str) -> (StatusCode, Value) {
        let body = json!({ "finding_id": finding["id"], "classification": classification, "reason": reason, "evidence": ["規範文件第 3 節"] });
        self.h().call_tool(token, "classify_review_finding", body).await
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

    /// 使用者在桌面以整張表取代決策權限表（沒列出的事項回到內建值）。
    async fn authority(&self, expected_revision: i64, entries: Value) {
        let body = json!({ "expected_revision": expected_revision, "entries": entries });
        let (status, table) =
            self.h().send_json(Method::PUT, &format!("{WORKSPACES}/{}/decision-authority", self.ws), Some(body)).await;
        assert_eq!(status, StatusCode::OK, "{table}");
    }

    fn decision_rows(&self) -> i64 {
        self.h().db().query_row("SELECT COUNT(*) FROM decision", [], |r| r.get(0)).unwrap()
    }

    /// 桌面的審查與設計偏離。
    async fn reviews(&self) -> Value {
        let (status, body) = self.h().send_json(Method::GET, &format!("{WORKSPACES}/{}/reviews", self.ws), None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body
    }

    async fn review_view(&self, id: &Value) -> Value {
        let list = self.reviews().await;
        list["reviews"].as_array().unwrap().iter().find(|r| &r["id"] == id).cloned().unwrap_or_else(|| panic!("找不到審查 {id}：{list}"))
    }
}

// --- 驗收條件：需求符合但規範違反、規範符合但需求缺漏，兩者都不能通過 Gate ---

#[tokio::test]
async fn requirement_met_but_standards_violated_or_standards_met_but_requirement_missing_never_opens_the_gate() {
    let w = world().await;
    let evidence = w.worker("整理證據").await;
    let reviewer = w.worker("審查匯出功能").await;
    let token = w.open_controller().await;
    let author = w.task("新增 CSV 匯出", "development").await;
    let workflow = w.start_feature(&token, &author).await;
    let workflow = w.to_review_stage(&token, &evidence, workflow).await;

    // 需求符合、規範違反：兩軸分開保存，固定 revision 與規格版本
    let standards_violated = w
        .submitted(&reviewer, review(&author, "c1", 1, axis("依 AGENTS.md 檢查錯誤處理", &["src"], &["吞掉 IO 錯誤"]), axis("對照契約第 1 版", &["src/export.rs"], &[])))
        .await;
    assert_eq!((standards_violated["base_revision"].as_str(), standards_violated["candidate_revision"].as_str()), (Some("base"), Some("c1")));
    assert_eq!((standards_violated["spec_version"].as_i64(), standards_violated["standards_source"].as_str()), (Some(1), Some("AGENTS.md@base")));
    assert_eq!((standards_violated["standards"]["passed"].as_bool(), standards_violated["spec"]["passed"].as_bool()), (Some(false), Some(true)));
    assert_eq!(standards_violated["passed"], false);
    assert_eq!(standards_violated["standards"]["findings"][0]["state"], "open");
    let result = w.record(&token, &workflow, json!([{ "check": "review", "review_id": standards_violated["id"] }])).await;
    assert_eq!(result["attempt"]["outcome"], "incomplete", "{result}");
    assert_eq!(result["attempt"]["evidence"][0]["status"], "not_passed");
    assert!(result["attempt"]["missing"][0]["reason"].as_str().unwrap().contains("兩軸都要通過"), "{result}");
    assert_eq!(result["workflow"]["current_stage"], "independent_review");
    let workflow = result["workflow"].clone();

    // 規範符合、需求缺漏（對較新的 candidate）：同樣不通過；舊的審查因為已有較新的 candidate 而過期
    let spec_missing = w
        .submitted(&reviewer, review(&author, "c2", 1, axis("規範沒有問題", &["src"], &[]), axis("對照契約第 1 版", &["src/export.rs"], &["匯出的 CSV 沒有標頭"])))
        .await;
    assert_eq!((spec_missing["standards"]["passed"].as_bool(), spec_missing["spec"]["passed"].as_bool()), (Some(true), Some(false)));
    let evidence_of = |review: &Value| json!([{ "check": "review", "review_id": review["id"] }]);
    let result = w.record(&token, &workflow, evidence_of(&spec_missing)).await;
    assert_eq!((result["attempt"]["outcome"].as_str(), result["attempt"]["evidence"][0]["status"].as_str()), (Some("incomplete"), Some("not_passed")));
    let workflow = result["workflow"].clone();
    let result = w.record(&token, &workflow, evidence_of(&standards_violated)).await;
    assert_eq!(result["attempt"]["evidence"][0]["status"], "stale", "{result}");
    assert!(w.review_view(&standards_violated["id"]).await["stale_reason"].as_str().unwrap().contains("c2"));
    let workflow = result["workflow"].clone();

    // Controller 依證據分類：駁回必須送裁定；Worker 不能分類；沒有理由不能分類
    let finding = spec_missing["spec"]["findings"][0].clone();
    let (status, body) = w.classify(&reviewer, &finding, "dismiss", "我覺得不用").await;
    assert_eq!((status, error_code(&body)), (StatusCode::FORBIDDEN, "controller_only"), "{body}");
    let (status, body) = w.classify(&token, &finding, "dismiss", " ").await;
    assert_eq!((status, error_code(&body)), (StatusCode::UNPROCESSABLE_ENTITY, "review_invalid"), "{body}");
    let (status, body) = w.classify(&token, &finding, "dismiss", "標頭由下游的匯入工具補上，契約沒有要求").await;
    assert_eq!((status, error_code(&body), body["matter"].as_str()), (StatusCode::CONFLICT, "requires_decision", Some("dismiss_review_finding")), "{body}");
    let request = body["decision_request_id"].as_str().unwrap().to_string();
    let waiting = w.review_view(&spec_missing["id"]).await;
    assert_eq!(waiting["spec"]["findings"][0]["state"], "awaiting_decision");
    assert_eq!(waiting["spec"]["findings"][0]["decision_request_id"], request.as_str());
    // 等待裁定時這一軸仍不通過
    let result = w.record(&token, &workflow, evidence_of(&spec_missing)).await;
    assert_eq!(result["attempt"]["outcome"], "incomplete");
    let workflow = result["workflow"].clone();

    // 使用者同意駁回後，Controller 再呼叫一次才套用：記下裁定與裁定者
    w.answer(&request, "approve").await;
    let (status, body) = w.classify(&token, &finding, "dismiss", "標頭由下游的匯入工具補上，契約沒有要求").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!((body["finding"]["state"].as_str(), body["finding"]["decided_by"].as_str()), (Some("dismissed"), Some("desktop")));
    assert_eq!(body["finding"]["decision_request_id"], request.as_str());
    assert_eq!(body["review"]["passed"], true);
    // 駁回之後不能再改
    let (status, body) = w.classify(&token, &finding, "fix", "改主意").await;
    assert_eq!((status, error_code(&body)), (StatusCode::CONFLICT, "finding_resolved"), "{body}");

    let result = w.record(&token, &workflow, evidence_of(&spec_missing)).await;
    assert_eq!(result["attempt"]["outcome"], "passed", "{result}");
    assert_eq!(result["workflow"]["current_stage"], "integration");
}

#[tokio::test]
async fn a_review_of_an_older_candidate_cannot_open_the_gate_once_more_changes_are_committed() {
    let w = world().await;
    let evidence = w.worker("整理證據").await;
    let reviewer = w.worker("審查匯出功能").await;
    let token = w.open_controller().await;
    let author = w.task("新增 CSV 匯出", "development").await;
    let workflow = w.start_feature(&token, &author).await;
    let workflow = w.to_review_stage(&token, &evidence, workflow).await;
    let passing = |candidate: &str| review(&author, candidate, 1, axis("規範沒有問題", &["src"], &[]), axis("需求都符合", &["src"], &[]));
    w.h().verification_driver.set_revision(&w.dir, "c1");
    let reviewed = w.submitted(&reviewer, passing("c1")).await;

    // 審查之後又提交了改動：審查的 candidate 已不是受驗目錄目前的 revision，不能開啟 Gate（R151 第 13 項）
    w.h().verification_driver.set_revision(&w.dir, "c2");
    let result = w.record(&token, &workflow, json!([{ "check": "review", "review_id": reviewed["id"] }])).await;
    assert_eq!(
        (result["attempt"]["outcome"].as_str(), result["attempt"]["evidence"][0]["status"].as_str()),
        (Some("incomplete"), Some("build_changed")),
        "{result}"
    );
    assert!(result["attempt"]["missing"][0]["reason"].as_str().unwrap().contains("重新審查"), "{result}");

    // 對目前的 candidate 重新審查才能通過
    let fresh = w.submitted(&reviewer, passing("c2")).await;
    let result = w.record(&token, &result["workflow"], json!([{ "check": "review", "review_id": fresh["id"] }])).await;
    assert_eq!(result["attempt"]["outcome"], "passed", "{result}");
}

// --- 驗收條件：作者不能擔任自己工作的必要 reviewer ---

#[tokio::test]
async fn the_author_cannot_be_the_required_reviewer_and_the_gate_stays_closed_until_enough_independent_reviewers() {
    let w = world().await;
    let evidence = w.worker("整理證據").await;
    let token = w.open_controller().await;

    // 使用者把獨立審查改為每一軸需要兩位 reviewer
    let (_, catalog) = w.h().send_json(Method::GET, PLAYBOOKS, None).await;
    let mut definition = catalog["playbooks"].as_array().unwrap().iter().find(|p| p["key"] == "feature").unwrap()["definition"].clone();
    let stage = definition["stages"].as_array_mut().unwrap().iter_mut().find(|s| s["key"] == "independent_review").unwrap();
    assert_eq!(stage["exit_checks"][0]["requires"], "review");
    stage["exit_checks"][0]["reviewers"] = json!(2);
    let (status, published) = w.h().send_json(Method::POST, PLAYBOOKS, Some(definition)).await;
    assert_eq!(status, StatusCode::CREATED, "{published}");

    let author = w.task("新增 CSV 匯出", "development").await;
    let (_, author_worker) = w.dispatch(&author).await;
    let workflow = w.start_feature(&token, &author).await;
    assert_eq!(workflow["playbook_version"], 2);
    let workflow = w.to_review_stage(&token, &evidence, workflow).await;
    let body = || review(&author, "c1", 1, axis("規範", &["src"], &[]), axis("需求", &["src"], &[]));

    // 作者（被審 Task 的 Run）不能審查自己的工作；Controller 也不能代替 reviewer
    let (status, rejected) = w.submit(&author_worker, body()).await;
    assert_eq!((status, error_code(&rejected)), (StatusCode::FORBIDDEN, "reviewer_is_author"), "{rejected}");
    let (status, rejected) = w.submit(&token, body()).await;
    assert_eq!((status, error_code(&rejected)), (StatusCode::FORBIDDEN, "worker_only"), "{rejected}");
    assert!(w.reviews().await["reviews"].as_array().unwrap().is_empty(), "被拒絕的審查不會留下");

    // 沒有 reviewer：Gate 保持關閉並阻擋
    let result = w.record(&token, &workflow, json!([])).await;
    assert_eq!((result["attempt"]["outcome"].as_str(), result["workflow"]["status"].as_str()), (Some("blocked"), Some("blocked")));
    let reason = result["workflow"]["blocked_reason"].as_str().unwrap();
    assert!(reason.contains("缺少非作者 reviewer") && reason.contains("需要 2 位，目前 0 位"), "{reason}");
    let workflow = result["workflow"].clone();

    // 一位 reviewer 通過：仍阻擋
    let first_reviewer = w.worker("審查一").await;
    let first = w.submitted(&first_reviewer, body()).await;
    assert_eq!(first["passed"], true);
    let result = w.record(&token, &workflow, json!([{ "check": "review", "review_id": first["id"] }])).await;
    assert_eq!(result["attempt"]["outcome"], "blocked", "{result}");
    assert!(result["attempt"]["missing"][0]["reason"].as_str().unwrap().contains("目前 1 位"));
    let workflow = result["workflow"].clone();

    // 同一位 reviewer 再交一份不算第二位
    let again = w.submitted(&first_reviewer, body()).await;
    let both = json!([{ "check": "review", "review_id": first["id"] }, { "check": "review", "review_id": again["id"] }]);
    let result = w.record(&token, &workflow, both).await;
    assert_eq!(result["attempt"]["outcome"], "blocked", "{result}");
    let workflow = result["workflow"].clone();

    // 審查別的 Task 的審查不算
    let other = w.task("別的工作", "development").await;
    let second_reviewer = w.worker("審查二").await;
    let unrelated = w.submitted(&second_reviewer, review(&other, "c1", 1, axis("規範", &["src"], &[]), axis("需求", &["src"], &[]))).await;
    let mixed = json!([{ "check": "review", "review_id": first["id"] }, { "check": "review", "review_id": unrelated["id"] }]);
    let result = w.record(&token, &workflow, mixed).await;
    assert_eq!((result["attempt"]["outcome"].as_str(), result["attempt"]["evidence"][1]["status"].as_str()), (Some("incomplete"), Some("other_subject")));
    let workflow = result["workflow"].clone();

    // 第二位非作者 reviewer 通過：Gate 才開啟
    let second = w.submitted(&second_reviewer, body()).await;
    let two = json!([{ "check": "review", "review_id": first["id"] }, { "check": "review", "review_id": second["id"] }]);
    let result = w.record(&token, &workflow, two).await;
    assert_eq!(result["attempt"]["outcome"], "passed", "{result}");
    assert_eq!((result["workflow"]["status"].as_str(), result["workflow"]["current_stage"].as_str()), (Some("active"), Some("integration")));
}

// --- 驗收條件：修正後只有可證明不受影響的審查結果被沿用 ---

#[tokio::test]
async fn after_a_fix_only_review_results_provably_unaffected_are_reused() {
    let w = world().await;
    let evidence = w.worker("整理證據").await;
    let first_reviewer = w.worker("審查一").await;
    let second_reviewer = w.worker("審查二").await;
    let token = w.open_controller().await;
    let author = w.task("新增 CSV 匯出", "development").await;
    let workflow = w.start_feature(&token, &author).await;
    let workflow = w.to_review_stage(&token, &evidence, workflow).await;

    // 第一次審查：規範軸有問題，需求軸通過（只審過 src/export.rs）
    let first = w
        .submitted(&first_reviewer, review(&author, "c1", 1, axis("錯誤處理規範", &["src"], &["吞掉 IO 錯誤"]), axis("需求", &["src/export.rs"], &[])))
        .await;
    let (status, body) = w.classify(&token, &first["standards"]["findings"][0], "fix", "錯誤確實被吞掉，見 src/util.rs:20").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!((body["finding"]["state"].as_str(), body["finding"]["classification"].as_str()), (Some("fix"), Some("fix")));
    assert_eq!(body["review"]["passed"], false, "修正之前仍不通過");

    // 修正只改了 src/util.rs（Runtime 以 Git 算出）
    w.h().workspace_fs.set_changed_paths(&w.dir, "c1", "c2", &["src/util.rs"]);
    w.h().workspace_fs.set_changed_paths(&w.dir, "c1", "c3", &["src/util.rs", "SRC\\Export.rs"]);

    // 規範軸還有要修正的 finding：不能沿用
    let (status, body) = w.submit(&second_reviewer, review(&author, "c2", 1, reuse(&first["id"]), axis("需求", &["src"], &[]))).await;
    assert_eq!((status, error_code(&body)), (StatusCode::CONFLICT, "review_reuse_rejected"), "{body}");
    assert!(body["message"].as_str().unwrap().contains("規範"), "{body}");
    // 變更動到需求軸審過的檔案：不能沿用
    let (status, body) = w.submit(&second_reviewer, review(&author, "c3", 1, axis("規範重審", &["src"], &[]), reuse(&first["id"]))).await;
    assert_eq!((status, error_code(&body)), (StatusCode::CONFLICT, "review_reuse_rejected"), "{body}");
    assert!(body["message"].as_str().unwrap().contains("src/export.rs"), "{body}");
    // 算不出變更（revision 不存在）：無法證明，不能沿用
    let (status, body) = w.submit(&second_reviewer, review(&author, "c9", 1, axis("規範重審", &["src"], &[]), reuse(&first["id"]))).await;
    assert_eq!((status, error_code(&body)), (StatusCode::CONFLICT, "review_reuse_rejected"), "{body}");
    assert!(body["message"].as_str().unwrap().contains("無法證明"), "{body}");

    // 可證明不受影響：需求軸沿用，規範軸重新審查
    let second = w.submitted(&second_reviewer, review(&author, "c2", 1, axis("規範重審", &["src"], &[]), reuse(&first["id"]))).await;
    assert_eq!(second["spec"]["reused_from"], first["id"]);
    assert_eq!(second["spec"]["paths"], json!(["src/export.rs"]));
    assert_eq!(second["spec"]["reviewer_task_id"], first["reviewer_task_id"], "沿用的軸記原本的 reviewer");
    assert_eq!(second["standards"]["reused_from"], Value::Null);
    assert_eq!(second["passed"], true);
    assert!(w.review_view(&first["id"]).await["stale_reason"].is_string(), "修正前的審查已過期");

    // 需求改了（契約第 2 版）：對照舊規格的審查過期，需求軸不能沿用；規範軸輸入沒變，可以沿用
    let mut changed = contract("完成工作");
    changed["done_criteria"] = filled("匯出的 CSV 有標頭，日期用 ISO 8601");
    let (status, body) = w.h().call_tool(&token, "update_task", json!({ "task_id": author, "contract": changed })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let stale = w.review_view(&second["id"]).await;
    assert!(stale["stale_reason"].as_str().unwrap().contains("第 2 版"), "{stale}");
    let result = w.record(&token, &workflow, json!([{ "check": "review", "review_id": second["id"] }])).await;
    assert_eq!((result["attempt"]["outcome"].as_str(), result["attempt"]["evidence"][0]["status"].as_str()), (Some("incomplete"), Some("stale")));
    let workflow = result["workflow"].clone();
    let (status, body) = w.submit(&first_reviewer, review(&author, "c2", 1, reuse(&second["id"]), reuse(&second["id"]))).await;
    assert_eq!((status, error_code(&body)), (StatusCode::CONFLICT, "review_spec_outdated"), "{body}");
    let (status, body) = w.submit(&first_reviewer, review(&author, "c2", 2, axis("規範", &["src"], &[]), reuse(&second["id"]))).await;
    assert_eq!((status, error_code(&body)), (StatusCode::CONFLICT, "review_reuse_rejected"), "{body}");
    assert!(body["message"].as_str().unwrap().contains("規格版本"), "{body}");
    let third = w.submitted(&first_reviewer, review(&author, "c2", 2, reuse(&second["id"]), axis("對照契約第 2 版", &["src/export.rs"], &[]))).await;
    assert_eq!(third["standards"]["reused_from"], second["id"]);
    assert_eq!(third["standards"]["reviewer_task_id"], second["reviewer_task_id"]);
    let result = w.record(&token, &workflow, json!([{ "check": "review", "review_id": third["id"] }])).await;
    assert_eq!(result["attempt"]["outcome"], "passed", "{result}");
}

// --- 設計偏離：保存偏離、原因與接受者，接受後更新設計版本 ---

#[tokio::test]
async fn an_accepted_design_deviation_keeps_who_accepted_it_and_the_next_worker_gets_the_new_design_version() {
    let w = world().await;
    let author = w.task("新增 CSV 匯出", "development").await;
    let (first_run, implementer) = w.dispatch(&author).await;
    let design = w.artifact(&implementer, "design.md", "# 設計 v1\n一次載入全部資料").await;

    let deviation = |version: i64| {
        json!({
            "design_artifact_id": design,
            "design_version": version,
            "deviation": "改用串流寫檔",
            "reason": "大檔案一次載入會耗盡記憶體",
            "affected_contracts": ["匯出 API 的回應時間"],
            "proposed_design": "# 設計 v2\n改用串流寫檔",
        })
    };
    // 對照的不是目前的設計版本、或不是已登記的成果：拒絕
    let (status, body) = w.h().call_tool(&implementer, "report_design_deviation", deviation(2)).await;
    assert_eq!((status, error_code(&body)), (StatusCode::CONFLICT, "design_outdated"), "{body}");
    let mut unknown = deviation(1);
    unknown["design_artifact_id"] = json!("art_missing");
    let (status, body) = w.h().call_tool(&implementer, "report_design_deviation", unknown).await;
    assert_eq!((status, error_code(&body)), (StatusCode::NOT_FOUND, "design_not_found"), "{body}");

    let (status, reported) = w.h().call_tool(&implementer, "report_design_deviation", deviation(1)).await;
    assert_eq!(status, StatusCode::CREATED, "{reported}");
    assert_eq!((reported["status"].as_str(), reported["design_version"].as_i64()), (Some("proposed"), Some(1)));
    assert_eq!((reported["reason"].as_str(), reported["design_name"].as_str()), (Some("大檔案一次載入會耗盡記憶體"), Some("design.md")));
    assert_eq!(reported["affected_contracts"], json!(["匯出 API 的回應時間"]));
    w.h().agent_adapter.emit(&first_run, AdapterEvent::Exited { success: false });
    w.h().supervise().await;

    // Controller 接受偏離：必須送裁定；Worker 不能處理偏離
    let token = w.open_controller().await;
    let resolve = json!({ "deviation_id": reported["id"], "resolution": "accept", "reason": "串流是唯一可行的做法" });
    let (status, body) = w.h().call_tool(&implementer, "resolve_design_deviation", resolve.clone()).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let (status, body) = w.h().call_tool(&token, "resolve_design_deviation", resolve.clone()).await;
    assert_eq!((status, error_code(&body), body["matter"].as_str()), (StatusCode::CONFLICT, "requires_decision", Some("accept_design_deviation")), "{body}");
    let request = body["decision_request_id"].as_str().unwrap().to_string();
    let (status, listed) = w.h().call_tool(&token, "get_reviews", json!({ "task_id": author })).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert_eq!(listed["deviations"][0]["status"], "awaiting_decision");

    // 使用者核准後，Controller 再呼叫一次：記下接受者，設計追加新版本
    w.answer(&request, "approve").await;
    let (status, body) = w.h().call_tool(&token, "resolve_design_deviation", resolve.clone()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let accepted = &body["deviation"];
    assert_eq!((accepted["status"].as_str(), accepted["accepted_by"].as_str()), (Some("accepted"), Some("desktop")));
    assert_eq!((accepted["decision_request_id"].as_str(), accepted["accepted_design_version"].as_i64()), (Some(request.as_str()), Some(2)));
    assert_eq!(accepted["resolution_reason"], "串流是唯一可行的做法");
    let (status, body) = w.h().call_tool(&token, "resolve_design_deviation", resolve).await;
    assert_eq!((status, error_code(&body)), (StatusCode::CONFLICT, "deviation_resolved"), "{body}");

    let preview = format!("{WORKSPACES}/{}/artifacts/{design}/versions/2/preview", w.ws);
    let (status, body) = w.h().send_json(Method::GET, &preview, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["preview"]["text"], "# 設計 v2\n改用串流寫檔");
    let (status, body) = w.h().send_json(Method::GET, &format!("{WORKSPACES}/{}/artifacts/{design}/versions/1/preview", w.ws), None).await;
    assert_eq!((status, body["preview"]["text"].as_str()), (StatusCode::OK, Some("# 設計 v1\n一次載入全部資料")), "舊版本保留");

    // 下一位 Worker 拿到的是新的設計版本
    let (second_run, _) = w.dispatch(&author).await;
    let (status, context) = w.h().send_json(Method::GET, &format!("{WORKSPACES}/{}/runs/{second_run}/context", w.ws), None).await;
    assert_eq!(status, StatusCode::OK, "{context}");
    let item = context["receipt"]["items"].as_array().unwrap().iter().find(|i| i["ref_id"] == design.as_str()).cloned().unwrap_or_else(|| panic!("{context}"));
    assert_eq!((item["kind"].as_str(), item["version"].as_i64()), (Some("evidence"), Some(2)));

    // 被拒絕的偏離：保存理由，設計不變
    let implementer = w.h().agent_credential(&second_run).unwrap();
    let (status, rejected) = w.h().call_tool(&implementer, "report_design_deviation", {
        let mut body = deviation(2);
        body["deviation"] = json!("改用 XML");
        body
    }).await;
    assert_eq!(status, StatusCode::CREATED, "{rejected}");
    let (status, body) = w
        .h()
        .call_tool(&token, "resolve_design_deviation", json!({ "deviation_id": rejected["id"], "resolution": "reject", "reason": "契約要求 CSV" }))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!((body["deviation"]["status"].as_str(), body["deviation"]["accepted_by"].as_str()), (Some("rejected"), None));
    let desktop = w.reviews().await;
    assert_eq!(desktop["deviations"].as_array().unwrap().len(), 2);
    assert_eq!(desktop["deviations"][1]["accepted_design_version"], 2);
}

// --- 決策權限表放寬或改為只能人工發起：分類 finding 與接受偏離依生效等級處理 ---

#[tokio::test]
async fn classifying_a_finding_follows_the_effective_authority_when_the_table_is_relaxed_or_reserved_for_the_user() {
    let w = world().await;
    let reviewer = w.worker("審查匯出功能").await;
    let (controller_run, token) = w.h().open_controller_cli(&w.ws, &w.controller_profile).await;
    let author = w.task("新增 CSV 匯出", "development").await;
    let reviewed = w
        .submitted(
            &reviewer,
            review(&author, "c1", 1, axis("規範沒有問題", &["src"], &[]), axis("對照契約第 1 版", &["src/export.rs"], &["匯出的 CSV 沒有標頭", "欄位順序與契約不同"])),
        )
        .await;
    let (header, order) = (reviewed["spec"]["findings"][0].clone(), reviewed["spec"]["findings"][1].clone());
    let decisions = w.decision_rows();

    // 只能人工發起：分類為修正、駁回、交給使用者都被拒絕，finding 不變，也不建立裁定請求
    w.authority(
        0,
        json!([
            { "matter": "classify_finding_as_fix", "effective": "human_only" },
            { "matter": "dismiss_review_finding", "effective": "human_only" },
        ]),
    )
    .await;
    for classification in ["fix", "dismiss", "needs_decision"] {
        let (status, body) = w.classify(&token, &header, classification, "標頭由下游的匯入工具補上").await;
        assert_eq!((status, error_code(&body)), (StatusCode::FORBIDDEN, "human_only"), "{classification}：{body}");
    }
    let current = w.review_view(&reviewed["id"]).await;
    assert_eq!(current["spec"]["findings"][0]["state"], "open", "{current}");
    assert_eq!(w.decision_rows(), decisions, "只能人工發起的事項不建立裁定請求");

    // 放寬為可自主處理：駁回直接套用（等同使用者已核准），不送裁定；記下由哪個 Controller Run 決定
    w.authority(1, json!([{ "matter": "dismiss_review_finding", "effective": "autonomous" }])).await;
    let (status, body) = w.classify(&token, &header, "dismiss", "標頭由下游的匯入工具補上，契約沒有要求").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["finding"]["state"], "dismissed", "{body}");
    let marker = format!("controller:{controller_run}");
    let decided = (body["finding"]["decision_request_id"].as_str(), body["finding"]["decided_by"].as_str());
    assert_eq!(decided, (Some(marker.as_str()), Some(marker.as_str())), "{body}");
    let (status, body) = w.classify(&token, &order, "needs_decision", "欄位順序契約沒有規定").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["finding"]["state"], "dismissed", "{body}");
    assert_eq!(body["review"]["passed"], true, "{body}");
    assert_eq!(w.decision_rows(), decisions, "可自主處理的事項不建立裁定請求");
}

#[tokio::test]
async fn accepting_a_design_deviation_follows_the_effective_authority_when_the_table_is_relaxed_or_reserved_for_the_user() {
    let w = world().await;
    let author = w.task("新增 CSV 匯出", "development").await;
    let (_, implementer) = w.dispatch(&author).await;
    let design = w.artifact(&implementer, "design.md", "# 設計 v1\n一次載入全部資料").await;
    let report = json!({
        "design_artifact_id": design,
        "design_version": 1,
        "deviation": "改用串流寫檔",
        "reason": "大檔案一次載入會耗盡記憶體",
        "affected_contracts": ["匯出 API 的回應時間"],
        "proposed_design": "# 設計 v2\n改用串流寫檔",
    });
    let (status, reported) = w.h().call_tool(&implementer, "report_design_deviation", report).await;
    assert_eq!(status, StatusCode::CREATED, "{reported}");
    let (controller_run, token) = w.h().open_controller_cli(&w.ws, &w.controller_profile).await;
    let resolve = json!({ "deviation_id": reported["id"], "resolution": "accept", "reason": "串流是唯一可行的做法" });
    let decisions = w.decision_rows();

    // 只能人工發起：Controller 接受偏離被拒絕，偏離維持原狀，也不建立裁定請求
    w.authority(0, json!([{ "matter": "accept_design_deviation", "effective": "human_only" }])).await;
    let (status, body) = w.h().call_tool(&token, "resolve_design_deviation", resolve.clone()).await;
    assert_eq!((status, error_code(&body)), (StatusCode::FORBIDDEN, "human_only"), "{body}");
    let (status, listed) = w.h().call_tool(&token, "get_reviews", json!({ "task_id": author })).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert_eq!(listed["deviations"][0]["status"], "proposed", "{listed}");
    assert_eq!(w.decision_rows(), decisions);

    // 放寬為可自主處理：直接接受（等同使用者已核准），記下由哪個 Controller Run 接受，設計追加新版本
    w.authority(1, json!([{ "matter": "accept_design_deviation", "effective": "autonomous" }])).await;
    let (status, body) = w.h().call_tool(&token, "resolve_design_deviation", resolve).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let accepted = &body["deviation"];
    let marker = format!("controller:{controller_run}");
    assert_eq!((accepted["status"].as_str(), accepted["accepted_by"].as_str()), (Some("accepted"), Some(marker.as_str())), "{body}");
    assert_eq!((accepted["decision_request_id"].as_str(), accepted["accepted_design_version"].as_i64()), (Some(marker.as_str()), Some(2)));
    assert_eq!(w.decision_rows(), decisions, "可自主處理的事項不建立裁定請求");
    let preview = format!("{WORKSPACES}/{}/artifacts/{design}/versions/2/preview", w.ws);
    let (status, body) = w.h().send_json(Method::GET, &preview, None).await;
    assert_eq!((status, body["preview"]["text"].as_str()), (StatusCode::OK, Some("# 設計 v2\n改用串流寫檔")), "{body}");
}
