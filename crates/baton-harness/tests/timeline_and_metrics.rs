//! 情境測試：事件時間線與觀測指標（票 41），經由 Runtime harness 的真實 HTTP API 驗證。
//!
//! 一個 Workspace 依序經歷：開發 Task 派工、驗證、整合與 worktree 清理；研究 Task 派工、外部操作、送裁定與回答、
//! 主動交接（帳號 A draining → 帳號 B）、承接的 Worker 重複登記同一個外部操作被擋下、Runtime 意外崩潰後恢復；
//! 另一個 Task 被取消、使用者回報一次教訓重犯。用量由 scripted fake AgentAdapter 回報，時間由 fake Clock 決定。
//!
//! 每個測試名稱對應票 41 的驗收條件：
//! - 時間線依 Task 過濾後事件完整且有序：`filtering_the_timeline_by_task_keeps_every_event_of_that_task_in_commit_order`、
//!   `a_task_outside_the_workspace_cannot_be_used_as_a_filter`
//! - 每項指標可由事件重新計算並與顯示一致：`every_metric_is_recomputed_from_the_timeline_events_and_matches_what_is_shown`、
//!   `controller_usage_counts_for_its_workspace_but_belongs_to_no_task`
//! - 不可見的服務端額度顯示未知：`server_side_quota_that_cannot_be_seen_is_unknown_even_when_local_usage_is_estimated`

use std::collections::HashSet;
use std::path::PathBuf;

use baton_domain::agent::{TOOL_CONFIRM_HANDOFF, TOOL_REQUEST_DECISION};
use baton_domain::conversation::MERGE_WINDOW_MS;
use baton_domain::external_operation::{TOOL_BEGIN_EXTERNAL_OPERATION, TOOL_RECORD_EXTERNAL_OPERATION_RESULT};
use baton_domain::metrics::{self, Metrics, Ratio, StageUsage};
use baton_domain::quota::{LimitStatus, QuotaWindow};
use baton_domain::timeline::{TimelineCategory, TimelineEntry};
use baton_domain::Timestamp;
use baton_harness::Harness;
use baton_ports::AdapterEvent;
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

const WORKSPACES: &str = "/api/v1/workspaces";
const KNOWLEDGE: &str = "/api/v1/knowledge";
const CLOCK_START: Timestamp = Timestamp::from_millis(1_700_000_000_000);
const SECOND: i64 = 1000;
const MINUTE: i64 = 60 * SECOND;
/// 從 Worker 送出裁定請求到使用者回答的時間。
const DECISION_WAIT_MS: i64 = 90 * SECOND;
/// 從 Runtime 偵測到崩潰到恢復嘗試完成處置的時間。
const RECOVERY_MS: i64 = 5 * SECOND;

// --- 共用輔助 ---

struct World {
    harness: Harness,
    ws: String,
    goal: String,
    account_b: String,
}

fn filled(text: &str) -> Value {
    json!({ "status": "filled", "text": text })
}

fn contract(checks: Value) -> Value {
    json!({
        "objective": filled("完成工作"),
        "inputs": filled("既有資料"),
        "scope": filled("本專案"),
        "deliverables": filled("成果"),
        "done_criteria": filled("檢查通過"),
        "blocker_handling": filled("卡住時回報"),
        "required_capabilities": [],
        "verification_checks": checks,
    })
}

async fn world() -> World {
    let harness = Harness::builder().clock_at(CLOCK_START).start().await;
    harness.add_account("帳號 A", "a@example.com").await;
    let account_b = harness.add_account("帳號 B", "b@example.com").await;
    harness.command_runner.script_exit("cargo", 0, "test result: ok");
    let dir = harness.data_dir().join("projects").join("app");
    std::fs::create_dir_all(&dir).unwrap();
    let (status, ws) = harness.send_json(Method::POST, WORKSPACES, Some(json!({ "path": dir.to_string_lossy() }))).await;
    assert_eq!(status, StatusCode::CREATED, "{ws}");
    harness.workspace_fs.init_repo(&PathBuf::from(ws["path"].as_str().unwrap()), "main");
    let ws = ws["id"].as_str().unwrap().to_string();
    let (status, goal) =
        harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/goals"), Some(json!({ "title": "目標" }))).await;
    assert_eq!(status, StatusCode::CREATED, "{goal}");
    World { harness, ws, goal: goal["id"].as_str().unwrap().to_string(), account_b }
}

impl World {
    fn h(&self) -> &Harness {
        &self.harness
    }

    fn task_path(&self, tid: &str) -> String {
        format!("{WORKSPACES}/{}/tasks/{tid}", self.ws)
    }

    async fn ready_task(&self, title: &str, kind: &str, checks: Value) -> String {
        let body = json!({ "goal_id": self.goal, "title": title, "kind": kind, "contract": contract(checks) });
        let (status, task) = self.h().send_json(Method::POST, &format!("{WORKSPACES}/{}/tasks", self.ws), Some(body)).await;
        assert_eq!(status, StatusCode::CREATED, "{task}");
        let tid = task["id"].as_str().unwrap().to_string();
        let (status, ready) = self.h().send_json(Method::POST, &format!("{}/ready", self.task_path(&tid)), None).await;
        assert_eq!(status, StatusCode::OK, "{ready}");
        tid
    }

    /// 派工並讓 Supervisor 啟動它，回傳 Run 識別。
    async fn start_run(&self, tid: &str) -> String {
        let (status, run) = self.h().send_json(Method::POST, &format!("{}/dispatch", self.task_path(tid)), None).await;
        assert_eq!(status, StatusCode::CREATED, "{run}");
        let rid = run["id"].as_str().unwrap().to_string();
        self.h().supervise().await;
        assert_eq!(self.run(&rid).await["status"], "running");
        rid
    }

    async fn runs_of(&self, tid: &str) -> Vec<Value> {
        let (status, body) = self.h().send_json(Method::GET, &format!("{WORKSPACES}/{}/runs", self.ws), None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body["runs"].as_array().unwrap().iter().filter(|r| r["task_id"] == tid).cloned().collect()
    }

    async fn run(&self, rid: &str) -> Value {
        let (_, body) = self.h().send_json(Method::GET, &format!("{WORKSPACES}/{}/runs", self.ws), None).await;
        body["runs"].as_array().unwrap().iter().find(|r| r["id"] == rid).unwrap().clone()
    }

    async fn running_run_of(&self, tid: &str) -> String {
        let runs = self.runs_of(tid).await;
        let running: Vec<&Value> = runs.iter().filter(|r| r["status"] == "running").collect();
        assert_eq!(running.len(), 1, "{runs:?}");
        running[0]["id"].as_str().unwrap().to_string()
    }

    async fn task_status(&self, tid: &str) -> String {
        let (status, detail) = self.h().send_json(Method::GET, &self.task_path(tid), None).await;
        assert_eq!(status, StatusCode::OK, "{detail}");
        detail["task"]["status"].as_str().unwrap().to_string()
    }

    async fn board(&self) -> Value {
        let (status, body) = self.h().send_json(Method::GET, &format!("{WORKSPACES}/{}/tasks", self.ws), None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body
    }

    async fn tool(&self, rid: &str, tool: &str, body: Value) -> Value {
        let token = self.h().agent_credential(rid).expect("Run 已啟動，Worker 有 Agent 憑證");
        let (status, response) = self.h().call_tool(&token, tool, body).await;
        assert_eq!(status, StatusCode::OK, "{tool}：{response}");
        response
    }

    async fn report_usage(&self, rid: &str, tokens: u64, context_tokens: Option<u64>) {
        self.h().agent_adapter.emit(rid, AdapterEvent::UsageReported { tokens, context_tokens, raw: "{}".into() });
        self.h().supervise().await;
    }

    async fn worker_exits(&self, rid: &str, success: bool) {
        self.h().agent_adapter.emit(rid, AdapterEvent::Exited { success });
        self.h().supervise().await;
    }

    async fn timeline(&self, task_id: Option<&str>) -> Vec<TimelineEntry> {
        let path = match task_id {
            Some(tid) => format!("{WORKSPACES}/{}/timeline?task_id={tid}", self.ws),
            None => format!("{WORKSPACES}/{}/timeline", self.ws),
        };
        let (status, body) = self.h().send_json(Method::GET, &path, None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["task_id"].as_str(), task_id);
        serde_json::from_value(body["events"].clone()).unwrap()
    }

    async fn metrics(&self) -> Value {
        let (status, body) = self.h().send_json(Method::GET, &format!("{WORKSPACES}/{}/metrics", self.ws), None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body
    }
}

/// 一個 Workspace 的完整經歷（見檔頭）；回傳（開發 Task、研究 Task、被取消的 Task）。
async fn busy_workspace(w: &mut World) -> (String, String, String) {
    // 開發 Task：派工 → 回報用量 → 成功結束 → 確定性檢查通過、整合、清理 worktree
    let checks = json!([{ "kind": "command", "program": "cargo", "args": ["test"], "expected_exit_code": 0 }]);
    let dev = w.ready_task("修正登入", "development", checks).await;
    let dev_run = w.start_run(&dev).await;
    w.report_usage(&dev_run, 1_000, Some(40_000)).await;
    w.h().clock.advance_millis(MINUTE);
    w.worker_exits(&dev_run, true).await;
    assert_eq!(w.task_status(&dev).await, "completed");

    // 研究 Task：外部操作成功 → 回報用量（看不到 Context）→ 送裁定，Worker 結束；90 秒後使用者回答
    let research = w.ready_task("整理資料", "research", json!([])).await;
    let first = w.start_run(&research).await;
    let notify = json!({ "idempotency_key": "notify-1", "operation": "notify", "target": "team", "request": {} });
    assert_eq!(w.tool(&first, TOOL_BEGIN_EXTERNAL_OPERATION, notify.clone()).await["status"], "proceed");
    let result = json!({ "idempotency_key": "notify-1", "result": "succeeded", "receipt": "msg #7" });
    w.tool(&first, TOOL_RECORD_EXTERNAL_OPERATION_RESULT, result).await;
    w.report_usage(&first, 500, None).await;
    let question = json!({
        "question": "要先整理哪一份？",
        "reason": "取決於使用者價值",
        "options": [{ "id": "a", "label": "甲", "tradeoff": "快" }, { "id": "b", "label": "乙", "tradeoff": "完整" }],
        "recommendation": "b",
        "evidence": ["兩份都缺"],
        "urgency": "normal",
    });
    let request_id = w.tool(&first, TOOL_REQUEST_DECISION, question).await["decision_request_id"].as_str().unwrap().to_string();
    w.worker_exits(&first, true).await;
    w.h().clock.advance_millis(DECISION_WAIT_MS);
    let decision_path = format!("{WORKSPACES}/{}/decisions/{request_id}", w.ws);
    let (_, view) = w.h().send_json(Method::GET, &decision_path, None).await;
    let body = json!({ "option_id": "b", "expected_revision": view["revision"] });
    let (status, answered) = w.h().send_json(Method::POST, &format!("{decision_path}/answer"), Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{answered}");

    // 回答後重新派出；帳號 A 接近五小時上限 → 交接給帳號 B，Worker 依要求自行結束，承接的 Worker 確認接手
    w.h().supervise().await;
    let second = w.running_run_of(&research).await;
    w.h().clock.advance_millis(MINUTE);
    let signal = AdapterEvent::RateLimited {
        window: Some(QuotaWindow::FiveHour),
        status: LimitStatus::Allowed,
        used: Some(0.86),
        reset_at_millis: None,
        raw: "{}".into(),
    };
    w.h().agent_adapter.emit(&second, signal);
    w.h().supervise().await;
    w.worker_exits(&second, false).await;
    let successor = w.running_run_of(&research).await;
    assert_eq!(w.run(&successor).await["profile_id"], w.account_b.as_str());
    let confirmation = json!({ "completed": "甲", "incomplete": "乙", "unknown_side_effects": "無" });
    w.tool(&successor, TOOL_CONFIRM_HANDOFF, confirmation).await;

    // 承接的 Worker 以同一個冪等鍵再登記一次：已成功，被擋下（重複操作事件）
    assert_eq!(w.tool(&successor, TOOL_BEGIN_EXTERNAL_OPERATION, notify).await["status"], "already_succeeded");

    // Runtime 意外終止：5 秒後恢復嘗試停下承接的 Worker、以新 Run 恢復；穩定 10 分鐘後成功
    w.harness.crash_restart().await;
    w.h().clock.advance_millis(RECOVERY_MS);
    w.h().supervise().await;
    w.h().clock.advance_millis(10 * MINUTE);
    w.h().supervise().await;
    let (_, recovery) = w.h().send_json(Method::GET, "/api/v1/recovery", None).await;
    assert_eq!(recovery["event"]["status"], "succeeded", "{recovery}");

    // 第三個 Task 被取消；使用者回報研究 Task 重犯了一個教訓
    let cancelled = w.ready_task("不做了", "research", json!([])).await;
    let (status, body) = w.h().send_json(Method::POST, &format!("{}/cancel", w.task_path(&cancelled)), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let lesson = json!({
        "kind": "lesson",
        "scope": { "kind": "workspace", "workspace_id": w.ws },
        "title": "通知前先確認收件人",
        "lesson": { "trigger": "通知", "root_cause": "收件人錯誤", "resolution": "先核對名單" },
    });
    let (status, entry) = w.h().send_json(Method::POST, KNOWLEDGE, Some(lesson)).await;
    assert_eq!(status, StatusCode::CREATED, "{entry}");
    let path = format!("{KNOWLEDGE}/{}/recurrences", entry["id"].as_str().unwrap());
    let report = json!({ "note": "又寄錯人", "workspace_id": w.ws, "task_id": research });
    let (status, body) = w.h().send_json(Method::POST, &path, Some(report)).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    (dev, research, cancelled)
}

fn types(entries: &[TimelineEntry]) -> Vec<&str> {
    entries.iter().map(|e| e.event_type.as_str()).collect()
}

fn categories(entries: &[TimelineEntry]) -> HashSet<TimelineCategory> {
    entries.iter().map(|e| e.category).collect()
}

/// `event_type` 第一次出現的位置。
fn position(entries: &[TimelineEntry], event_type: &str) -> usize {
    entries.iter().position(|e| e.event_type == event_type).unwrap_or_else(|| panic!("時間線沒有 {event_type}：{:?}", types(entries)))
}

// --- 時間線依 Task 過濾後事件完整且有序 ---

#[tokio::test]
async fn filtering_the_timeline_by_task_keeps_every_event_of_that_task_in_commit_order() {
    let mut w = world().await;
    let (dev, research, cancelled) = busy_workspace(&mut w).await;
    let all = w.timeline(None).await;
    let seqs: Vec<i64> = all.iter().map(|e| e.seq).collect();
    assert!(seqs.windows(2).all(|p| p[0] < p[1]), "依提交順序：{seqs:?}");

    let of_research = w.timeline(Some(&research)).await;
    // 有序且完整：正好是全部事件中與這個 Task 有關的那些，順序不變
    let expected: Vec<&TimelineEntry> = all.iter().filter(|e| e.concerns(&research)).collect();
    assert_eq!(of_research.iter().collect::<Vec<_>>(), expected);
    assert!(of_research.iter().all(|e| e.concerns(&research)));
    use TimelineCategory as C;
    for category in [C::Dispatch, C::Decision, C::Handoff, C::Recovery, C::Task, C::ExternalOperation, C::Lesson, C::Usage] {
        assert!(categories(&of_research).contains(&category), "研究 Task 的時間線缺少 {category:?}：{:?}", types(&of_research));
    }
    // 每個 Run 的派工與結束、裁定、交接都在，依發生的先後
    for run in w.runs_of(&research).await {
        let rid = run["id"].as_str().unwrap();
        let events: Vec<&str> =
            of_research.iter().filter(|e| e.run_id.as_deref() == Some(rid) && e.category == C::Dispatch).map(|e| e.event_type.as_str()).collect();
        assert_eq!(events.first(), Some(&"run_created"), "Run {rid}：{events:?}");
        assert!(events.contains(&"run_started"), "Run {rid}：{events:?}");
    }
    let handoff_id = w.board().await["handoffs"][0]["id"].as_str().unwrap().to_string();
    assert!(of_research.iter().any(|e| e.aggregate_id == handoff_id));
    let story = [
        "decision_requested",
        "decision_answered",
        "handoff_requested",
        "run_handed_off",
        "handoff_transferred",
        "handoff_confirmed",
        "external_operation_duplicate_blocked",
        "runtime_crash_detected",
        "recovery_attempt_completed",
        "recovery_succeeded",
        "lesson_recurrence_recorded",
    ];
    let order: Vec<usize> = story.iter().map(|t| position(&of_research, t)).collect();
    assert!(order.windows(2).all(|p| p[0] < p[1]), "{story:?} 的先後：{order:?}");

    // 開發 Task：派工、驗證、清理；沒有別的 Task 的裁定或交接，也沒有被恢復影響
    let of_dev = w.timeline(Some(&dev)).await;
    assert!(of_dev.iter().all(|e| e.task_ids == [dev.clone()]));
    for category in [C::Dispatch, C::Verification, C::Cleanup, C::Usage] {
        assert!(categories(&of_dev).contains(&category), "開發 Task 的時間線缺少 {category:?}：{:?}", types(&of_dev));
    }
    for category in [C::Decision, C::Handoff, C::Recovery] {
        assert!(!categories(&of_dev).contains(&category), "{:?}", types(&of_dev));
    }
    assert!(position(&of_dev, "run_succeeded") < position(&of_dev, "task_verification_recorded"));
    assert!(position(&of_dev, "task_verification_recorded") < position(&of_dev, "worktree_cleaned"));

    // 只有建立與取消的 Task
    let of_cancelled = w.timeline(Some(&cancelled)).await;
    assert_eq!(types(&of_cancelled), ["task_created", "task_status_changed", "task_status_changed"]);
    assert_eq!(of_cancelled[2].payload["to"], "cancelled");

    // 恢復事件屬於整個 Runtime：未過濾時同樣在，關聯到它處置過的研究 Task
    let crash = &all[position(&all, "runtime_crash_detected")];
    assert_eq!(crash.task_ids, [research.clone()]);
    assert!(all.iter().all(|e| e.occurred_at >= CLOCK_START), "時間以 UTC 毫秒存放");
}

#[tokio::test]
async fn a_task_outside_the_workspace_cannot_be_used_as_a_filter() {
    let w = world().await;
    let tid = w.ready_task("整理資料", "research", json!([])).await;
    let other = w.h().data_dir().join("projects").join("other");
    std::fs::create_dir_all(&other).unwrap();
    let (_, ws2) = w.h().send_json(Method::POST, WORKSPACES, Some(json!({ "path": other.to_string_lossy() }))).await;
    let ws2 = ws2["id"].as_str().unwrap();

    let (status, body) = w.h().send_json(Method::GET, &format!("{WORKSPACES}/{ws2}/timeline?task_id={tid}"), None).await;
    assert_eq!((status, body["error"].as_str()), (StatusCode::NOT_FOUND, Some("task_not_found")), "{body}");
    let (status, body) = w.h().send_json(Method::GET, &format!("{WORKSPACES}/{ws2}/timeline"), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["events"], json!([]), "別的 Workspace 的事件不會出現");
    let (status, body) = w.h().send_json(Method::GET, &format!("{WORKSPACES}/nope/timeline"), None).await;
    assert_eq!((status, body["error"].as_str()), (StatusCode::NOT_FOUND, Some("workspace_not_found")), "{body}");
    let (status, body) = w.h().send_json(Method::GET, &format!("{WORKSPACES}/nope/metrics"), None).await;
    assert_eq!((status, body["error"].as_str()), (StatusCode::NOT_FOUND, Some("workspace_not_found")), "{body}");
}

// --- 每項指標可由事件重新計算並與顯示一致 ---

#[tokio::test]
async fn every_metric_is_recomputed_from_the_timeline_events_and_matches_what_is_shown() {
    let mut w = world().await;
    busy_workspace(&mut w).await;

    let shown = w.metrics().await;
    let shown_metrics: Metrics = serde_json::from_value(shown["metrics"].clone()).unwrap();
    let events = w.timeline(None).await;
    assert_eq!(metrics::compute(&events), shown_metrics, "拿時間線事件重新計算，得到顯示的數字");
    assert_eq!(shown_metrics.events, events.len() as u64);

    // 每一項都對得上這個 Workspace 實際發生的事
    let m = shown_metrics;
    assert_eq!(m.task_completion.completion, Ratio { numerator: 1, denominator: 2, rate: Some(0.5) }, "取消的不計入");
    assert_eq!(m.task_completion.cancelled, 1);
    assert_eq!(m.handoff.success, Ratio { numerator: 1, denominator: 1, rate: Some(1.0) });
    assert_eq!((m.handoff.confirmed, m.handoff.in_flight, m.handoff.abandoned), (1, 0, 0));
    assert_eq!((m.recovery.count, m.recovery.pending, m.recovery.average_ms), (1, 0, Some(RECOVERY_MS)));
    assert_eq!((m.duplicate_operations.blocked, m.duplicate_operations.already_succeeded), (1, 1));
    assert_eq!(m.duplicate_operations.unknown_outcomes, 0);
    assert_eq!((m.decision_wait.count, m.decision_wait.pending, m.decision_wait.max_ms), (1, 0, Some(DECISION_WAIT_MS)));
    assert_eq!(m.lesson_recurrence.rate, Ratio { numerator: 1, denominator: 3, rate: Some(1.0 / 3.0) });
    assert_eq!((m.lesson_recurrence.reported, m.lesson_recurrence.prevented), (1, 0));
    assert_eq!(m.usage.total_tokens, 1_500);
    assert_eq!(m.usage.by_stage, [StageUsage { stage: None, tokens: 1_500, runs: 2 }], "第一版沒有 Stage：未分階段");
    assert_eq!((m.context.runs_known, m.context.runs_unknown, m.context.average_tokens), (1, 1, Some(40_000)));

    // 用量紀錄過了保留期被刪除之後，指標仍由不可刪除的事件算出（不會跟著消失）
    w.h().clock.advance_millis(9 * 24 * 60 * MINUTE);
    let later: Metrics = serde_json::from_value(w.metrics().await["metrics"].clone()).unwrap();
    assert_eq!(later.usage, m.usage);
    assert_eq!(later.context, m.context);
}

#[tokio::test]
async fn controller_usage_counts_for_its_workspace_but_belongs_to_no_task() {
    let w = world().await;
    let tid = w.ready_task("整理資料", "research", json!([])).await;
    let rid = w.start_run(&tid).await;
    w.report_usage(&rid, 300, Some(9_000)).await;

    // 使用者找 Controller 說話：Controller Run 回報用量後回覆
    let path = format!("{WORKSPACES}/{}/conversation/messages", w.ws);
    let (status, body) = w.h().send_json(Method::POST, &path, Some(json!({ "text": "目前進度？" }))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    w.h().clock.advance_millis(MERGE_WINDOW_MS);
    w.h().supervise().await;
    let (_, conversation) = w.h().send_json(Method::GET, &format!("{WORKSPACES}/{}/conversation", w.ws), None).await;
    let controller = conversation["runs"][0]["id"].as_str().expect("已啟動 Controller Run").to_string();
    w.h().agent_adapter.emit(&controller, AdapterEvent::UsageReported { tokens: 700, context_tokens: Some(21_000), raw: "{}".into() });
    w.h().agent_adapter.emit(&controller, AdapterEvent::Message { text: "還在整理".into() });
    w.h().agent_adapter.emit(&controller, AdapterEvent::Exited { success: true });
    w.h().supervise().await;

    let all = w.timeline(None).await;
    let usage: Vec<&TimelineEntry> = all.iter().filter(|e| e.category == TimelineCategory::Usage).collect();
    assert_eq!(usage.len(), 2, "{:?}", types(&all));
    let of_controller = usage.iter().find(|e| e.aggregate_id == controller).expect("Controller 的用量在 Workspace 的時間線上");
    assert!(of_controller.task_ids.is_empty(), "Controller 不屬於任何 Task");
    assert!(w.timeline(Some(&tid)).await.iter().all(|e| e.aggregate_id != controller));

    let shown: Metrics = serde_json::from_value(w.metrics().await["metrics"].clone()).unwrap();
    assert_eq!(shown, metrics::compute(&all));
    assert_eq!((shown.usage.total_tokens, shown.usage.runs), (1_000, 2));
    assert_eq!((shown.context.runs_known, shown.context.max_tokens), (2, Some(21_000)));
}

// --- 不可見的服務端額度顯示未知 ---

#[tokio::test]
async fn server_side_quota_that_cannot_be_seen_is_unknown_even_when_local_usage_is_estimated() {
    let w = world().await;
    let tid = w.ready_task("整理資料", "research", json!([])).await;
    let rid = w.start_run(&tid).await;
    // 本機推估有數字（設了方案上限、有用量），但那不是服務端額度
    let profile = w.run(&rid).await["profile_id"].as_str().unwrap().to_string();
    let limits = json!({ "five_hour_tokens": 10_000, "seven_day_tokens": 100_000 });
    let (status, body) = w.h().send_json(Method::PUT, &format!("/api/v1/profiles/{profile}/quota-limits"), Some(limits)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    w.report_usage(&rid, 2_000, Some(10_000)).await;

    let quota = w.metrics().await["server_quota"].clone();
    let accounts = quota.as_array().unwrap();
    assert_eq!(accounts.len(), 2);
    for account in accounts {
        assert_eq!((&account["five_hour_used"], &account["seven_day_used"]), (&Value::Null, &Value::Null), "{account}");
    }

    // Claude Code 回報了五小時視窗的帳號用量：只有那個視窗看得到，週視窗仍是未知
    let signal = AdapterEvent::RateLimited {
        window: Some(QuotaWindow::FiveHour),
        status: LimitStatus::Allowed,
        used: Some(0.42),
        reset_at_millis: None,
        raw: "{}".into(),
    };
    w.h().agent_adapter.emit(&rid, signal);
    w.h().supervise().await;
    let quota = w.metrics().await["server_quota"].clone();
    let account = quota.as_array().unwrap().iter().find(|a| a["profile_id"] == profile.as_str()).unwrap().clone();
    assert_eq!(account["five_hour_used"].as_f64(), Some(0.42));
    assert_eq!(account["seven_day_used"], Value::Null);
    assert_eq!(account["name"], "帳號 A");
}
