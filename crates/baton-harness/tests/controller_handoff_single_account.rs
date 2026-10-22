//! 情境測試：Controller 交接（票 30）的 M7 邊界（最終 code-review 修正，review-notes #18）。
//!
//! 唯一的帳號進入 draining、Controller 有一輪在執行：沒有其他帳號能承接，等滿 2 分鐘也不要求停止，
//! 這一輪留在原帳號做完，回覆照常保存。

use baton_domain::quota::{LimitStatus, QuotaWindow};
use baton_domain::Timestamp;
use baton_harness::Harness;
use baton_ports::AdapterEvent;
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

const WORKSPACES: &str = "/api/v1/workspaces";
const CLOCK_START: Timestamp = Timestamp::from_millis(1_700_000_000_123);
const SECOND: i64 = 1000;
const MINUTE: i64 = 60 * SECOND;
/// 合併視窗（spec D-31）。
const WINDOW_MS: i64 = 5 * SECOND;
/// 帳號進入 draining 後，進行中的一輪最多再等多久才被要求停止（與 Worker 的交接相同）。
const HANDOFF_WAIT_MS: i64 = 2 * MINUTE;

async fn conversation(harness: &Harness, ws: &str) -> Value {
    let (status, body) = harness.send_json(Method::GET, &format!("{WORKSPACES}/{ws}/conversation"), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

#[tokio::test]
async fn a_turn_on_the_only_draining_account_is_not_stopped_and_finishes_where_it_is() {
    let harness = Harness::builder().clock_at(CLOCK_START).start().await;
    let only = harness.add_account("帳號 A", "account0@example.com").await;
    let dir = harness.data_dir().join("projects").join("main");
    std::fs::create_dir_all(&dir).unwrap();
    let (status, body) = harness.send_json(Method::POST, WORKSPACES, Some(json!({ "path": dir.to_string_lossy() }))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let ws = body["id"].as_str().unwrap().to_string();

    let (status, sent) =
        harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/conversation/messages"), Some(json!({ "text": "盤點進度" }))).await;
    assert_eq!(status, StatusCode::CREATED, "{sent}");
    let turn = sent["turn"]["id"].as_str().unwrap().to_string();
    harness.clock.advance_millis(WINDOW_MS);
    harness.supervise().await;
    let runs = conversation(&harness, &ws).await["runs"].as_array().unwrap().clone();
    assert_eq!(runs.len(), 1);
    let run_id = runs[0]["id"].as_str().unwrap().to_string();
    assert_eq!(runs[0]["profile_id"], only.as_str());

    // 這一輪的 CLI 回報接近上限：唯一的帳號進入 draining
    harness.agent_adapter.emit(
        &run_id,
        AdapterEvent::RateLimited {
            window: Some(QuotaWindow::FiveHour),
            status: LimitStatus::Warning,
            used: Some(0.9),
            reset_at_millis: None,
            raw: "{}".into(),
        },
    );
    harness.supervise().await;

    // 等滿 2 分鐘：沒有帳號能承接，不要求停止，這一輪仍在原帳號執行
    harness.clock.advance_millis(HANDOFF_WAIT_MS + SECOND);
    harness.supervise().await;
    harness.supervise().await;
    assert!(harness.process_control.stop_calls().is_empty(), "沒有承接帳號時不停止進行中的一輪");
    let runs = conversation(&harness, &ws).await["runs"].as_array().unwrap().clone();
    assert_eq!(runs.len(), 1, "{runs:?}");
    assert_eq!(runs[0]["status"], "running");
    assert_eq!(runs[0]["profile_id"], only.as_str());

    // 這一輪自己做完：回覆照常保存，輸入處理完成
    harness.agent_adapter.emit(&run_id, AdapterEvent::Message { text: "進度如下".into() });
    harness.agent_adapter.emit(&run_id, AdapterEvent::Exited { success: true });
    harness.supervise().await;
    let view = conversation(&harness, &ws).await;
    let turns = view["turns"].as_array().unwrap();
    let user = turns.iter().find(|t| t["id"] == turn.as_str()).unwrap();
    assert_eq!(user["status"], "processed", "{user}");
    assert!(turns.iter().any(|t| t["kind"] == "controller_reply" && t["content"] == "進度如下"), "{view}");
}
