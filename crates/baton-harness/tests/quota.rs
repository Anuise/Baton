//! 情境測試：額度觀測與帳號頁（票 27），經由 Runtime harness 的真實 HTTP API 驗證。
//!
//! 「已觀測」與「推估」兩個來源都由 scripted fake AgentAdapter 的事件產生（CLI 的限流訊號、Run 結果的用量），
//! 外部觀測器是 fake QuotaObserver；時間由 fake Clock 決定，測試決定何時讓 Supervisor 處理一次。
//! 只斷言外部行為：排程總覽（帳號頁的資料）、派工回應、Port 被呼叫的次數；`harness.db()` 只用來確認原始資料
//! 確實保存（必須看落地內容的事實）。
//!
//! 每個測試名稱對應票 27 的驗收條件：
//! - 觀測不到時顯示「未知」而非 0：`an_account_without_any_observation_is_unknown_not_zero`、
//!   `usage_without_a_plan_limit_is_still_unknown`、`an_exhausted_signal_counts_until_its_reset_then_returns_to_unknown`、
//!   `a_failing_quota_observer_leaves_an_unknown_observation_not_zero`
//! - 已觀測與推估並存時以較保守者判斷：`observed_and_estimated_coexist_and_the_more_conservative_one_decides_dispatch`、
//!   `a_higher_observation_overrides_a_lower_estimate`
//! - 其他裝置用量不可見時明確標示觀測不完整：`estimates_alone_are_marked_incomplete_because_other_devices_are_invisible`
//! - 每筆觀測保存來源、時間、可信度與原始資料參照：`each_observation_keeps_source_time_confidence_and_a_raw_data_reference`
//! - 每 5 分鐘輪詢（spec D-31）：`the_quota_observer_is_polled_at_most_every_five_minutes`
//! - 方案上限：`plan_limits_are_validated_and_survive_a_restart`

use baton_domain::quota::{LimitStatus, QuotaWindow};
use baton_domain::Timestamp;
use baton_harness::Harness;
use baton_ports::{AdapterEvent, Clock, Confidence, QuotaSample, QuotaSource};
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

const WORKSPACES: &str = "/api/v1/workspaces";
const SCHEDULING: &str = "/api/v1/scheduling";
const CLOCK_START: Timestamp = Timestamp::from_millis(1_700_000_000_000);
const MINUTE: i64 = 60 * 1000;
const HOUR: i64 = 60 * MINUTE;

// --- 共用輔助 ---

async fn start_with_account() -> (Harness, String) {
    let harness = Harness::builder().clock_at(CLOCK_START).start().await;
    let id = harness.add_account("主要帳號", "main@example.com").await;
    (harness, id)
}

fn filled(text: &str) -> Value {
    json!({ "status": "filled", "text": text })
}

fn contract() -> Value {
    json!({
        "objective": filled("整理資料"),
        "inputs": filled("既有的資料"),
        "scope": filled("公開資料"),
        "deliverables": filled("比較表"),
        "done_criteria": filled("五個產品"),
        "blocker_handling": filled("卡住時回報"),
        "required_capabilities": [],
    })
}

/// 一個 Workspace 與 Goal，回傳 (workspace_id, goal_id)。
async fn setup(harness: &Harness) -> (String, String) {
    let dir = harness.data_dir().join("projects").join("main");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.to_string_lossy().into_owned();
    let (status, body) = harness.send_json(Method::POST, WORKSPACES, Some(json!({ "path": path }))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let ws = body["id"].as_str().unwrap().to_string();
    let (status, goal) =
        harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/goals"), Some(json!({ "title": "目標" }))).await;
    assert_eq!(status, StatusCode::CREATED, "{goal}");
    (ws, goal["id"].as_str().unwrap().to_string())
}

async fn ready_task(harness: &Harness, ws: &str, goal: &str, title: &str) -> String {
    let body = json!({ "goal_id": goal, "title": title, "contract": contract() });
    let (status, task) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks"), Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    let tid = task["id"].as_str().unwrap().to_string();
    let (status, ready) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks/{tid}/ready"), None).await;
    assert_eq!(status, StatusCode::OK, "{ready}");
    tid
}

async fn dispatch(harness: &Harness, ws: &str, tid: &str) -> (StatusCode, Value) {
    harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks/{tid}/dispatch"), None).await
}

/// 派出一個 Run 並讓 Supervisor 啟動它，回傳 Run 識別。
async fn running_run(harness: &Harness, ws: &str, goal: &str, title: &str) -> String {
    let tid = ready_task(harness, ws, goal, title).await;
    let (status, run) = dispatch(harness, ws, &tid).await;
    assert_eq!(status, StatusCode::CREATED, "{run}");
    let rid = run["id"].as_str().unwrap().to_string();
    harness.supervise().await;
    rid
}

/// Worker 送出事件，Supervisor 處理一次。
async fn emit(harness: &Harness, rid: &str, event: AdapterEvent) {
    harness.agent_adapter.emit(rid, event);
    harness.supervise().await;
}

fn usage(tokens: u64) -> AdapterEvent {
    AdapterEvent::UsageReported { tokens, raw: format!("{{\"type\":\"result\",\"usage\":{{\"tokens\":{tokens}}}}}") }
}

fn signal(window: Option<QuotaWindow>, status: LimitStatus, used: Option<f64>, reset_at: Option<Timestamp>) -> AdapterEvent {
    AdapterEvent::RateLimited {
        window,
        status,
        used,
        reset_at_millis: reset_at.map(Timestamp::as_millis),
        raw: "{\"type\":\"rate_limit_event\"}".into(),
    }
}

async fn set_limits(harness: &Harness, profile_id: &str, body: Value) -> (StatusCode, Value) {
    harness.send_json(Method::PUT, &format!("/api/v1/profiles/{profile_id}/quota-limits"), Some(body)).await
}

async fn overview(harness: &Harness) -> Value {
    let (status, body) = harness.send_json(Method::GET, SCHEDULING, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

/// 排程總覽（帳號頁）中某個帳號的額度判斷。
async fn quota_of(harness: &Harness, profile_id: &str) -> Value {
    let body = overview(harness).await;
    let account = body["accounts"].as_array().unwrap().iter().find(|a| a["profile_id"] == profile_id).unwrap().clone();
    assert_eq!(account["quota_used"], account["quota"]["used"], "quota_used 與判斷一致：{account}");
    account["quota"].clone()
}

fn window<'a>(quota: &'a Value, name: &str) -> &'a Value {
    quota["windows"].as_array().unwrap().iter().find(|w| w["window"] == name).unwrap()
}

// --- 觀測不到時顯示「未知」而非 0 ---

#[tokio::test]
async fn an_account_without_any_observation_is_unknown_not_zero() {
    let (harness, profile) = start_with_account().await;

    let quota = quota_of(&harness, &profile).await;

    assert!(quota["used"].is_null(), "{quota}");
    assert_eq!(quota["confidence"], "unknown");
    assert!(quota["source"].is_null());
    assert!(quota["reset_at"].is_null());
    assert!(quota["observed_at"].is_null());
    assert_eq!(quota["complete"], false);
    for name in ["five_hour", "seven_day"] {
        let w = window(&quota, name);
        assert!(w["effective"].is_null(), "{w}");
        assert!(w["observed"].is_null(), "{w}");
        assert!(w["estimated"]["used"].is_null(), "推估存在但不知道用量：{w}");
        assert_eq!(w["estimated"]["confidence"], "unknown");
    }
}

#[tokio::test]
async fn usage_without_a_plan_limit_is_still_unknown() {
    let (harness, profile) = start_with_account().await;
    let (ws, goal) = setup(&harness).await;
    let rid = running_run(&harness, &ws, &goal, "用量").await;

    emit(&harness, &rid, usage(12_000)).await;

    let quota = quota_of(&harness, &profile).await;
    assert!(quota["used"].is_null(), "沒有方案上限算不出比例：{quota}");
    let estimated = &window(&quota, "five_hour")["estimated"];
    assert_eq!(estimated["tokens"], 12_000, "本機用量仍然記下：{estimated}");
    assert!(estimated["limit_tokens"].is_null());
    assert_eq!(estimated["confidence"], "unknown");
}

#[tokio::test]
async fn an_exhausted_signal_counts_until_its_reset_then_returns_to_unknown() {
    let (harness, profile) = start_with_account().await;
    let (ws, goal) = setup(&harness).await;
    let rid = running_run(&harness, &ws, &goal, "撞到上限").await;
    let reset = CLOCK_START.plus_millis(2 * HOUR);

    emit(&harness, &rid, signal(Some(QuotaWindow::FiveHour), LimitStatus::Exhausted, None, Some(reset))).await;

    let quota = quota_of(&harness, &profile).await;
    assert_eq!(quota["used"], 1.0, "已達上限視為 100%：{quota}");
    assert_eq!(quota["source"], "observed");
    assert_eq!(quota["confidence"], "observed");
    assert_eq!(quota["reset_at"], reset.as_millis());
    assert_eq!(window(&quota, "five_hour")["observed"]["status"], "exhausted");

    // 已達上限的帳號不再派新的 Run
    let tid = ready_task(&harness, &ws, &goal, "等重置").await;
    let (status, body) = dispatch(&harness, &ws, &tid).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert_eq!(body["reasons"][0]["code"], "quota_near_limit", "{body}");

    // 重置之後：訊號失效，回到未知而不是 0
    harness.clock.set(reset);
    let after = quota_of(&harness, &profile).await;
    assert!(after["used"].is_null(), "{after}");
    assert!(window(&after, "five_hour")["observed"].is_null());
}

#[tokio::test]
async fn a_failing_quota_observer_leaves_an_unknown_observation_not_zero() {
    // fake QuotaObserver 沒有腳本的帳號一律回傳錯誤
    let (harness, profile) = start_with_account().await;

    let quota = quota_of(&harness, &profile).await;

    assert!(quota["used"].is_null(), "{quota}");
    assert_eq!(quota["external"]["confidence"], "unknown", "{quota}");
    assert!(quota["external"]["used"].is_null());
    assert_eq!(quota["external"]["raw_ref"], "quota_observer");
    let raw: String = harness
        .db()
        .query_row("SELECT raw FROM quota_observation WHERE origin = 'observer'", [], |r| r.get(0))
        .unwrap();
    assert!(raw.contains("沒有"), "失敗原因保存為原始資料：{raw}");
}

// --- 已觀測與推估並存時以較保守者判斷 ---

#[tokio::test]
async fn observed_and_estimated_coexist_and_the_more_conservative_one_decides_dispatch() {
    let (harness, profile) = start_with_account().await;
    let (ws, goal) = setup(&harness).await;
    let (status, body) = set_limits(&harness, &profile, json!({ "five_hour_tokens": 10_000, "seven_day_tokens": null })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let rid = running_run(&harness, &ws, &goal, "累積用量").await;

    // CLI 說只用了 30%，但本機推估已經 80%：取較保守的 80%
    emit(&harness, &rid, usage(8_000)).await;
    emit(&harness, &rid, signal(Some(QuotaWindow::FiveHour), LimitStatus::Allowed, Some(0.3), None)).await;

    let quota = quota_of(&harness, &profile).await;
    assert_eq!(quota["used"], 0.8, "{quota}");
    assert_eq!(quota["source"], "estimated");
    assert_eq!(quota["confidence"], "estimated");
    let five_hour = window(&quota, "five_hour");
    assert_eq!(five_hour["observed"]["used"], 0.3, "兩個來源並存：{five_hour}");
    assert_eq!(five_hour["estimated"]["used"], 0.8);
    assert_eq!(five_hour["effective"]["source"], "estimated");

    // 排程依同一個判斷：80% 已達停止派工門檻
    let tid = ready_task(&harness, &ws, &goal, "第二個").await;
    let (status, body) = dispatch(&harness, &ws, &tid).await;
    assert_eq!(status, StatusCode::ACCEPTED, "較保守的推估擋下派工：{body}");
    assert_eq!(body["reasons"][0]["category"], "quota");
}

#[tokio::test]
async fn a_higher_observation_overrides_a_lower_estimate() {
    let (harness, profile) = start_with_account().await;
    let (ws, goal) = setup(&harness).await;
    set_limits(&harness, &profile, json!({ "five_hour_tokens": null, "seven_day_tokens": 100_000 })).await;
    let rid = running_run(&harness, &ws, &goal, "累積用量").await;

    emit(&harness, &rid, usage(20_000)).await;
    emit(&harness, &rid, signal(Some(QuotaWindow::SevenDay), LimitStatus::Warning, Some(0.9), None)).await;

    let quota = quota_of(&harness, &profile).await;
    assert_eq!(quota["used"], 0.9, "{quota}");
    assert_eq!(quota["source"], "observed");
    let seven_day = window(&quota, "seven_day");
    assert_eq!(seven_day["estimated"]["used"], 0.2);
    assert_eq!(seven_day["observed"]["status"], "warning");
}

// --- 其他裝置用量不可見時明確標示觀測不完整 ---

#[tokio::test]
async fn estimates_alone_are_marked_incomplete_because_other_devices_are_invisible() {
    let (harness, profile) = start_with_account().await;
    let (ws, goal) = setup(&harness).await;
    set_limits(&harness, &profile, json!({ "five_hour_tokens": 10_000, "seven_day_tokens": 100_000 })).await;
    let rid = running_run(&harness, &ws, &goal, "只有推估").await;
    emit(&harness, &rid, usage(1_000)).await;

    let quota = quota_of(&harness, &profile).await;
    assert_eq!(quota["used"], 0.1, "{quota}");
    assert_eq!(quota["complete"], false);
    let reason = quota["incomplete_reason"].as_str().unwrap();
    assert!(reason.contains("其他裝置"), "{reason}");

    // CLI 回報兩個視窗的帳號層級用量之後才算完整
    emit(&harness, &rid, signal(Some(QuotaWindow::FiveHour), LimitStatus::Allowed, Some(0.2), None)).await;
    let half = quota_of(&harness, &profile).await;
    assert_eq!(half["complete"], false, "只有五小時視窗有已觀測：{half}");
    assert!(half["incomplete_reason"].as_str().unwrap().contains("七日"));

    emit(&harness, &rid, signal(Some(QuotaWindow::SevenDay), LimitStatus::Allowed, Some(0.05), None)).await;
    let full = quota_of(&harness, &profile).await;
    assert_eq!(full["complete"], true, "{full}");
    assert!(full["incomplete_reason"].is_null());
}

// --- 每筆觀測保存來源、時間、可信度與原始資料參照 ---

#[tokio::test]
async fn each_observation_keeps_source_time_confidence_and_a_raw_data_reference() {
    let (harness, profile) = start_with_account().await;
    let (ws, goal) = setup(&harness).await;
    set_limits(&harness, &profile, json!({ "five_hour_tokens": 10_000, "seven_day_tokens": null })).await;
    let rid = running_run(&harness, &ws, &goal, "原始資料").await;
    harness.clock.advance_millis(MINUTE);
    let signal_at = harness.clock.now();
    emit(&harness, &rid, signal(Some(QuotaWindow::FiveHour), LimitStatus::Allowed, Some(0.4), None)).await;
    harness.clock.advance_millis(MINUTE);
    let usage_at = harness.clock.now();
    emit(&harness, &rid, usage(500)).await;

    let quota = quota_of(&harness, &profile).await;
    let five_hour = window(&quota, "five_hour");
    let observed = &five_hour["observed"];
    assert_eq!(observed["source"], "observed");
    assert_eq!(observed["origin"], "adapter");
    assert_eq!(observed["observed_at"], signal_at.as_millis());
    assert_eq!(observed["confidence"], "observed");
    assert_eq!(observed["raw_ref"], format!("run:{rid}"));
    assert!(observed.get("raw").is_none(), "原始內容不經 API 回傳");
    let estimated = &five_hour["estimated"];
    assert_eq!(estimated["source"], "estimated");
    assert_eq!(estimated["observed_at"], usage_at.as_millis());
    assert_eq!(estimated["confidence"], "estimated");
    assert!(estimated["raw_ref"].as_str().unwrap().starts_with("quota_usage:seq="), "{estimated}");

    // 原始資料落地：CLI 的訊號原文與 Run 回報的用量原文
    let db = harness.db();
    let signal_raw: String = db
        .query_row("SELECT raw FROM quota_observation WHERE origin = 'adapter'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(signal_raw, "{\"type\":\"rate_limit_event\"}");
    let (usage_run, usage_raw): (String, String) =
        db.query_row("SELECT run_id, raw FROM quota_usage", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!(usage_run, rid);
    assert!(usage_raw.contains("\"result\""), "{usage_raw}");
}

// --- 每 5 分鐘輪詢（spec D-31） ---

#[tokio::test]
async fn the_quota_observer_is_polled_at_most_every_five_minutes() {
    let (harness, profile) = start_with_account().await;
    let sample = |used: f64| QuotaSample {
        source: QuotaSource::Observed,
        observed_at: harness.clock.now(),
        used: Some(used),
        reset_at: None,
        confidence: Confidence::Observed,
    };
    harness.quota_observer.set_sample(&profile, sample(0.2));

    harness.supervise().await;
    let first = harness.quota_observer.observe_count(&profile);
    assert!(first >= 1, "第一次處理就輪詢");
    assert_eq!(quota_of(&harness, &profile).await["used"], 0.2);

    // 5 分鐘內不論處理幾次、看幾次帳號頁，都不再輪詢
    harness.quota_observer.set_sample(&profile, sample(0.6));
    harness.clock.advance_millis(5 * MINUTE - 1);
    harness.supervise().await;
    let quota = quota_of(&harness, &profile).await;
    assert_eq!(harness.quota_observer.observe_count(&profile), first);
    assert_eq!(quota["used"], 0.2, "還沒到下一次輪詢：{quota}");

    harness.clock.advance_millis(1);
    harness.supervise().await;
    assert_eq!(harness.quota_observer.observe_count(&profile), first + 1);
    let quota = quota_of(&harness, &profile).await;
    assert_eq!(quota["used"], 0.6, "{quota}");
    assert_eq!(quota["external"]["source"], "observed");
    assert_eq!(quota["complete"], true, "外部觀測器的已觀測樣本看得到帳號層級用量");
}

// --- 方案上限 ---

#[tokio::test]
async fn plan_limits_are_validated_and_survive_a_restart() {
    let (mut harness, profile) = start_with_account().await;

    let (status, body) = set_limits(&harness, &profile, json!({ "five_hour_tokens": 0, "seven_day_tokens": null })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"], "plan_limit_invalid");
    let (status, body) = set_limits(&harness, &profile, json!({ "five_hour_tokens": -1 })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let (status, body) = set_limits(&harness, "prof_missing", json!({ "five_hour_tokens": 1 })).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["error"], "profile_not_found");

    let (status, body) =
        set_limits(&harness, &profile, json!({ "five_hour_tokens": 50_000, "seven_day_tokens": 400_000 })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["accounts"][0]["plan_limits"], json!({ "five_hour_tokens": 50_000, "seven_day_tokens": 400_000 }));

    harness.restart().await;
    let after = overview(&harness).await;
    assert_eq!(after["accounts"][0]["plan_limits"], json!({ "five_hour_tokens": 50_000, "seven_day_tokens": 400_000 }));
    assert_eq!(window(&after["accounts"][0]["quota"], "seven_day")["estimated"]["limit_tokens"], 400_000);

    let (status, _) = harness.send_json(Method::POST, &format!("/api/v1/profiles/{profile}/archive"), None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = set_limits(&harness, &profile, json!({ "five_hour_tokens": 1 })).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"], "profile_archived");
}
