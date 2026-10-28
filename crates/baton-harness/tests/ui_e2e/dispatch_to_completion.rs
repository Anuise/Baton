//! UI E2E「派工到完成」（票 53）：使用者在 Web UI 建立 Workspace、確認授權、建立 Task 並派工；
//! scripted fake Worker 經 Agent API 回報進度、登記成果後正常結束；Runtime 執行契約宣告的確定性檢查，
//! 通過後 Task 自動完成，UI 的工作面板顯示在「已完成」欄。
//!
//! 使用者的動作一律經畫面完成；只有 Worker 的行為（fake AgentAdapter 與 Agent 憑證呼叫工具）與
//! Supervisor 的推進（`supervise`）在 Rust 端進行。API 只用來讀取（查 Run 識別與事後佐證），不改任何狀態。

use baton_domain::agent::TOOL_REPORT_PROGRESS;
use baton_domain::artifact::TOOL_REGISTER_ARTIFACT;
use baton_ports::AdapterEvent;
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

use crate::browser::{exact_text, has_text, label, role, text};
use crate::UiE2e;

const WORKSPACE_NAME: &str = "報告專案";
const TASK_TITLE: &str = "比較報告";

/// 六項最小派工契約的欄位標籤與填寫內容（標籤與 `ContractEditor` 的 `FIELD_LABELS` 相同）。
const CONTRACT: [(&str, &str); 6] = [
    ("目標", "產出比較報告"),
    ("輸入與依據", "既有的資料"),
    ("範圍與限制", "只用公開資料"),
    ("預期成果", "report.md"),
    ("完成與驗證條件", "登記 report.md 成果"),
    ("阻礙處理", "卡住時回報"),
];

#[tokio::test(flavor = "multi_thread")]
#[ignore = "UI E2E：需要 Playwright，以 node scripts/test-e2e.mjs 執行"]
async fn a_task_dispatched_from_the_ui_completes_after_the_worker_reports_and_checks_pass() {
    let e2e = UiE2e::start().await;
    let (harness, browser) = (&e2e.harness, &e2e.browser);
    let sidebar = role("complementary", "Workspace");
    let tab = |name: &str| [role("tablist", "Workspace 功能"), role("tab", name)];
    // 收在「更多」入口內的頁面：先切到「更多」，再點第二層頁簽
    let more_page = |name: &str| [role("tablist", "更多"), role("tab", name)];

    // 1. 建立 Workspace：在側欄填入目錄與名稱
    let dir = harness.data_dir().join("projects").join("report");
    std::fs::create_dir_all(&dir).unwrap();
    browser.fill(&[sidebar.clone(), label("目錄")], &dir.to_string_lossy()).await;
    browser.fill(&[sidebar.clone(), label("名稱（選填）")], WORKSPACE_NAME).await;
    browser.click(&[sidebar.clone(), role("button", "建立 Workspace")]).await;

    // 2. 確認授權：建立後自動選中該 Workspace，切到「更多」的「授權」頁簽；套用「研究／文件」範本、檢視摘要後確認
    browser.click(&tab("更多")).await;
    browser.click(&more_page("授權")).await;
    let authorization = role("region", "授權設定");
    browser.wait_visible(&[authorization.clone(), role("heading", "授權設定")]).await;
    // 頁面標題只寫頁名，Workspace 名稱由頁簽列上方的情境列顯示
    browser.wait_visible(&[label("目前的 Workspace"), exact_text(WORKSPACE_NAME)]).await;
    browser.wait_visible(&[authorization.clone(), text("尚未確認授權")]).await;
    browser.click(&[authorization.clone(), role("button", "套用「研究／文件」")]).await;
    browser.click(&[authorization.clone(), role("button", "檢視確認摘要")]).await;
    browser.click(&[authorization.clone(), role("region", "確認摘要"), role("button", "確認並保存授權")]).await;
    browser.wait_visible(&[authorization.clone(), text("已保存為 revision 1")]).await;

    // 3. 建立 Task：新 Goal、六項契約全部填寫、宣告確定性檢查（成果 report.md）、所需能力在授權範圍內
    browser.click(&tab("工作")).await;
    let board = role("region", "工作面板");
    browser.click(&[board.clone(), role("button", "建立 Task")]).await;
    let form = role("dialog", "建立 Task");
    browser.fill(&[form.clone(), label("新 Goal 名稱")], "季度報告").await;
    browser.fill(&[form.clone(), label("Task 標題")], TASK_TITLE).await;
    browser.select(&[form.clone(), role("combobox", "種類")], "研究").await;
    for (field, content) in CONTRACT {
        browser.select(&[form.clone(), role("combobox", &format!("{field}：模式"))], "填寫").await;
        browser.fill(&[form.clone(), label(&format!("{field}：內容"))], content).await;
    }
    browser.fill(&[form.clone(), label("確定性檢查（每行一項）")], "artifact: report.md").await;
    browser.check(&[form.clone(), label("讀取檔案")]).await;
    browser.check(&[form.clone(), label("編輯檔案")]).await;
    browser.click(&[form.clone(), role("button", "建立")]).await;
    browser.wait_hidden(&[form]).await;

    // 4. 標記為可派工並派工
    let card = |column: &str| [board.clone(), role("region", column), role("article", TASK_TITLE)];
    browser.click(&[&card("待處理（1）")[..], &[role("button", "標記為可派工")]].concat()).await;
    browser.click(&[&card("可派工（1）")[..], &[role("button", "派工")]].concat()).await;
    browser.wait_visible(&card("執行中（1）")).await;

    // 5. scripted fake Worker：Supervisor 啟動 Run；Worker 以 Agent 憑證回報進度、登記成果，然後正常結束
    let (ws, tid, rid) = dispatched_run(harness).await;
    harness.supervise().await;
    let token = harness.agent_credential(&rid).expect("Run 啟動時應該拿到 Agent 憑證");
    let (status, body) =
        harness.call_tool(&token, TOOL_REPORT_PROGRESS, json!({ "summary": "報告已寫好，登記成果中" })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) =
        harness.call_tool(&token, TOOL_REGISTER_ARTIFACT, json!({ "name": "report.md", "content": "abc" })).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    harness.agent_adapter.emit(&rid, AdapterEvent::Exited { success: true });
    harness.supervise().await;

    // 6. 工作面板沒有輪詢：切到別的頁簽（「更多」的「時間線」）再切回「工作」讓它重新載入；
    //    Task 在「已完成」欄，展開卡片的「詳細資訊」後顯示經自動檢查通過驗證
    browser.click(&tab("更多")).await;
    browser.click(&more_page("時間線")).await;
    browser.wait_hidden(std::slice::from_ref(&board)).await;
    browser.click(&tab("工作")).await;
    let completed = card("已完成（1）");
    browser.wait_visible(&completed).await;
    browser.click(&[&completed[..], &[exact_text("詳細資訊")]].concat()).await;
    browser.wait_visible(&[&completed[..], &[role("group", "驗證紀錄"), has_text("已通過驗證（自動檢查）")]].concat()).await;
    // Worker 的回報與 Run 的結果也顯示在同一張卡片上
    let run_status = browser.text(&[&completed[..], &[role("group", "Run 狀態")]].concat()).await;
    assert!(run_status.contains("Run（第 1 次）：成功"), "{run_status}");
    assert!(run_status.contains("最新進度：報告已寫好，登記成果中"), "{run_status}");
    assert_eq!(browser.count(&[board.clone(), role("region", "執行中（0）")]).await, 1, "執行中欄已沒有這個 Task");

    // 7. API 佐證：Task 為 completed，驗證紀錄是這次 Run 的自動檢查、成果檢查通過
    let (status, detail) = harness.send_json(Method::GET, &format!("/api/v1/workspaces/{ws}/tasks/{tid}"), None).await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    assert_eq!(detail["task"]["status"], "completed", "{detail}");
    let (status, view) =
        harness.send_json(Method::GET, &format!("/api/v1/workspaces/{ws}/tasks/{tid}/verification"), None).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    let records = view["records"].as_array().expect("驗證紀錄是陣列");
    assert_eq!(records.len(), 1, "{view}");
    let record = &records[0];
    assert_eq!((record["method"].as_str(), record["outcome"].as_str()), (Some("checks"), Some("passed")), "{record}");
    assert_eq!(record["run_id"].as_str(), Some(rid.as_str()));
    let check = &record["checks"][0];
    assert_eq!((check["check"]["kind"].as_str(), check["check"]["name"].as_str()), (Some("artifact"), Some("report.md")));
    assert_eq!(check["status"], "passed");
    assert_eq!(record["artifacts"][0]["name"], "report.md");
    // Run 依 UI 中確認的授權（revision 1）准入，並以成功結束
    let (status, run) = harness.send_json(Method::GET, &format!("/api/v1/workspaces/{ws}/runs/{rid}"), None).await;
    assert_eq!(status, StatusCode::OK, "{run}");
    assert_eq!((run["authorization_revision"].as_i64(), run["status"].as_str()), (Some(1), Some("succeeded")), "{run}");

    e2e.finish().await;
}

/// 經 UI 派工後，查出唯一的 Workspace、Task 與 Run 識別（只讀）。
async fn dispatched_run(harness: &baton_harness::Harness) -> (String, String, String) {
    let (status, list) = harness.send_json(Method::GET, "/api/v1/workspaces", None).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let workspaces = list["workspaces"].as_array().expect("Workspace 清單是陣列");
    assert_eq!(workspaces.len(), 1, "{list}");
    let ws = id_of(&workspaces[0]);
    let (status, runs) = harness.send_json(Method::GET, &format!("/api/v1/workspaces/{ws}/runs"), None).await;
    assert_eq!(status, StatusCode::OK, "{runs}");
    let runs = runs["runs"].as_array().expect("Run 清單是陣列");
    assert_eq!(runs.len(), 1, "UI 派工建立了一個 Run：{runs:?}");
    (ws, runs[0]["task_id"].as_str().expect("Run 有 task_id").to_string(), id_of(&runs[0]))
}

fn id_of(value: &Value) -> String {
    value["id"].as_str().expect("有 id").to_string()
}
