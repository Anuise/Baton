//! 情境測試：Project Brief、Glossary 與 Project Map（票 42），經由 Runtime harness 的真實 HTTP API 驗證。
//!
//! Controller 是腳本化的（同票 17）：測試在 Controller 的一輪進行中，以那一輪啟動時收到的 Controller 憑證呼叫工具。
//! Workspace 目錄是暫存目錄中的真實檔案；Git 狀態由 fake WorkspaceFs 腳本化（`init_repo`）。
//! 只斷言外部行為：工具與桌面 API 的回應、Workspace 內的檔案內容、Controller 對話收到的工作事件與 Prompt。
//!
//! 每個測試名稱對應票 42 的驗收條件：
//! - 未知資訊不被寫成已確認事實：`unknown_information_is_never_recorded_or_projected_as_a_confirmed_fact`、
//!   `a_confirmed_item_whose_source_file_changes_is_flagged_for_recheck`
//! - 投影檔被外部修改時產生協調事件而非靜默覆寫：
//!   `an_externally_edited_glossary_raises_a_reconciliation_event_instead_of_being_overwritten`、
//!   `a_kept_external_glossary_edited_again_can_still_be_overwritten_with_the_hash_from_the_view`、
//!   `a_glossary_that_already_exists_in_the_repo_is_not_overwritten_on_first_projection`、
//!   `workspace_decisions_are_projected_as_adrs_and_an_external_deletion_is_reconciled`
//! - Controller 透過工具建立並維護附來源的 Brief／Glossary／Map：
//!   `the_controller_builds_a_sourced_brief_glossary_and_map_when_the_workspace_is_first_connected`、
//!   `only_the_controller_can_record_project_understanding`
//! - 非 Git repo 不產生 repo 投影：`a_workspace_that_is_not_a_git_repo_gets_no_projection_files`

use std::path::{Path, PathBuf};

use baton_domain::artifact::sha256_hex;
use baton_domain::Timestamp;
use baton_harness::Harness;
use baton_ports::AdapterEvent;
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

const WORKSPACES: &str = "/api/v1/workspaces";
const CLOCK_START: Timestamp = Timestamp::from_millis(1_700_000_000_123);
/// 合併視窗（spec D-31）。
const WINDOW_MS: i64 = 5_000;
const README: &str = "# 範例專案\n\n這是給小型團隊用的記帳工具。\n";

// --- 共用輔助 ---

struct Project {
    harness: Harness,
    ws: String,
    dir: PathBuf,
}

/// 啟動 Harness、建立一個帳號與一個 Workspace（`git` 為 true 時目錄是 Git repo），目錄內有 README.md 與 src/store.rs。
async fn project(git: bool) -> Project {
    let harness = Harness::builder().clock_at(CLOCK_START).start().await;
    harness.add_account("帳號 1", "account1@example.com").await;
    let dir = harness.data_dir().join("projects").join("ledger");
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("README.md"), README).unwrap();
    std::fs::write(dir.join("src").join("store.rs"), "// 帳目的持久化\n").unwrap();
    if git {
        harness.workspace_fs.init_repo(&dir, "main");
    }
    let (status, body) = harness.send_json(Method::POST, WORKSPACES, Some(json!({ "path": dir.to_string_lossy() }))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let ws = body["id"].as_str().unwrap().to_string();
    Project { harness, ws, dir }
}

fn read(dir: &Path, relative: &str) -> Option<String> {
    std::fs::read_to_string(dir.join(relative)).ok()
}

async fn conversation(harness: &Harness, ws: &str) -> Value {
    let (status, body) = harness.send_json(Method::GET, &format!("{WORKSPACES}/{ws}/conversation"), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

/// 使用者送一則訊息、合併視窗過後喚起 Controller，回傳這一輪的 (Controller Run 識別, Controller 憑證)。
async fn controller_turn(harness: &Harness, ws: &str, text: &str) -> (String, String) {
    let path = format!("{WORKSPACES}/{ws}/conversation/messages");
    let (status, body) = harness.send_json(Method::POST, &path, Some(json!({ "text": text }))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    harness.clock.advance_millis(WINDOW_MS);
    harness.supervise().await;
    let view = conversation(harness, ws).await;
    let run_id = view["active_run"]["id"].as_str().unwrap_or_else(|| panic!("Controller 沒有被喚起：{view}")).to_string();
    let token = harness.agent_credential(&run_id).expect("Controller 程序啟動時應該收到 Controller 憑證");
    (run_id, token)
}

async fn finish_turn(harness: &Harness, run_id: &str) {
    harness.agent_adapter.emit(run_id, AdapterEvent::Started { native_session_id: Some(format!("native-{run_id}")) });
    harness.agent_adapter.emit(run_id, AdapterEvent::Message { text: "好的".into() });
    harness.agent_adapter.emit(run_id, AdapterEvent::Exited { success: true });
    harness.supervise().await;
}

async fn record(harness: &Harness, token: &str, body: Value) -> (StatusCode, Value) {
    harness.call_tool(token, "record_understanding", body).await
}

/// 預期成功的記錄，回傳回應。
async fn recorded(harness: &Harness, token: &str, body: Value) -> Value {
    let (status, response) = record(harness, token, body).await;
    assert_eq!(status, StatusCode::OK, "{response}");
    response
}

fn term(key: &str, text: &str, status: &str, sources: Value) -> Value {
    json!({ "section": "glossary", "key": key, "text": text, "status": status, "sources": sources })
}

async fn view(harness: &Harness, ws: &str) -> Value {
    let (status, body) = harness.send_json(Method::GET, &format!("{WORKSPACES}/{ws}/understanding"), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

async fn sync(harness: &Harness, ws: &str) -> Value {
    let (status, body) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/understanding/sync"), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

async fn reconcile(harness: &Harness, ws: &str, path: &str, action: &str, expected: Option<&str>) -> (StatusCode, Value) {
    let body = json!({ "path": path, "action": action, "expected_external_hash": expected });
    harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/understanding/projections/reconcile"), Some(body)).await
}

fn item<'a>(view: &'a Value, section: &str, key: &str) -> &'a Value {
    view["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["section"] == section && i["key"] == key)
        .unwrap_or_else(|| panic!("沒有 {section}／{key}：{view}"))
}

fn projection<'a>(view: &'a Value, path: &str) -> &'a Value {
    view["projections"].as_array().unwrap().iter().find(|p| p["path"] == path).unwrap_or_else(|| panic!("沒有投影 {path}：{view}"))
}

/// Controller 對話中尚待處理的工作事件內容。
async fn pending_work_events(harness: &Harness, ws: &str) -> Vec<String> {
    conversation(harness, ws).await["turns"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["kind"] == "work_event" && t["status"] == "pending")
        .map(|t| t["content"].as_str().unwrap().to_string())
        .collect()
}

fn error_code(body: &Value) -> &str {
    body["error"].as_str().unwrap_or_default()
}

// --- Controller 建立並維護專案理解 ---

#[tokio::test]
async fn the_controller_builds_a_sourced_brief_glossary_and_map_when_the_workspace_is_first_connected() {
    let Project { harness, ws, dir } = project(true).await;
    let (run_id, token) = controller_turn(&harness, &ws, "幫我看看這個專案").await;

    // 首次接入：Prompt 要求建立專案理解，讀到的專案理解是空的
    let prompt = conversation(&harness, &ws).await["runs"][0]["prompt"].as_str().unwrap().to_string();
    assert!(prompt.contains("record_understanding") && prompt.contains("get_project_understanding"), "{prompt}");
    assert!(prompt.contains("hypothesis"), "{prompt}");
    let (status, empty) = harness.call_tool(&token, "get_project_understanding", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{empty}");
    assert_eq!(empty["items"], json!([]));

    let brief = json!({
        "section": "brief", "key": "產品目標", "text": "給小型團隊用的記帳工具",
        "status": "confirmed", "sources": [{ "path": "README.md", "line": 3 }],
    });
    let response = recorded(&harness, &token, brief.clone()).await;
    assert_eq!(response["changed"], true);
    assert_eq!(response["item"]["revision"], 1);
    recorded(&harness, &token, term("帳目", "一筆收入或支出", "confirmed", json!([{ "path": "README.md" }]))).await;
    let component = json!({
        "section": "map", "key": "Store", "text": "帳目的持久化", "status": "confirmed",
        "sources": [{ "path": "src/store.rs" }, { "reference": "https://example.com/design", "version": "2023-11-01" }],
    });
    recorded(&harness, &token, component).await;
    // 未確認的關係：沒有指定狀態就是假說
    recorded(&harness, &token, json!({ "section": "map", "key": "API → Store", "text": "API 大概直接呼叫 Store" })).await;
    // 重複送出相同內容不產生新版本
    let again = recorded(&harness, &token, brief).await;
    assert_eq!(again["changed"], false);
    assert_eq!(again["item"]["revision"], 1);

    let view = view(&harness, &ws).await;
    assert_eq!(view["projection_enabled"], true);
    assert_eq!(view["items"].as_array().unwrap().len(), 4);
    let goal = item(&view, "brief", "產品目標");
    assert_eq!(goal["status"], "confirmed");
    assert_eq!(goal["recorded_by"], run_id.as_str());
    // 來源位置、版本（Runtime 自行計算的內容雜湊）與查驗時間
    let source = &goal["sources"][0];
    assert_eq!(source["kind"], "file");
    assert_eq!(source["path"], "README.md");
    assert_eq!(source["line"], 3);
    assert_eq!(source["version"], sha256_hex(README.as_bytes()));
    assert_eq!(source["checked_at"], CLOCK_START.as_millis() + WINDOW_MS);
    assert_eq!(source["changed"], false);
    let store = item(&view, "map", "Store");
    assert_eq!(store["sources"][1]["kind"], "external");
    assert_eq!(store["sources"][1]["version"], "2023-11-01");
    assert_eq!(item(&view, "map", "API → Store")["status"], "hypothesis");

    // Glossary 投影到 repo 的 GLOSSARY.md，帶版本
    let glossary = read(&dir, "GLOSSARY.md").expect("應該產生 GLOSSARY.md");
    assert!(glossary.contains("Glossary 第 1 版"), "{glossary}");
    assert!(glossary.contains("## 帳目\n\n一筆收入或支出\n\n- 狀態：已確認\n- 來源：`README.md`"), "{glossary}");
    assert_eq!(projection(&view, "GLOSSARY.md")["status"], "in_sync");

    // 維護：更新描述會增加版本；不再適用的移除後不出現在投影
    let updated = recorded(&harness, &token, term("帳目", "一筆收入、支出或轉帳", "confirmed", json!([{ "path": "README.md" }]))).await;
    assert_eq!(updated["item"]["revision"], 2);
    recorded(&harness, &token, json!({ "section": "glossary", "key": "帳目", "status": "retired" })).await;
    let glossary = read(&dir, "GLOSSARY.md").unwrap();
    assert!(glossary.contains("Glossary 第 3 版") && !glossary.contains("## 帳目"), "{glossary}");

    // 這一輪結束後，它的憑證不能再記錄
    finish_turn(&harness, &run_id).await;
    let (status, body) = record(&harness, &token, term("新詞", "說明", "hypothesis", json!([]))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
}

#[tokio::test]
async fn only_the_controller_can_record_project_understanding() {
    let Project { harness, ws, .. } = project(true).await;
    // 一個 Worker Run 的 Agent 憑證
    let (_, goal) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/goals"), Some(json!({ "title": "目標" }))).await;
    let filled = |text: &str| json!({ "status": "filled", "text": text });
    let contract = json!({
        "objective": filled("整理"), "inputs": filled("資料"), "scope": filled("公開"), "deliverables": filled("表"),
        "done_criteria": filled("完成"), "blocker_handling": filled("回報"), "required_capabilities": [],
    });
    let body = json!({ "goal_id": goal["id"], "title": "整理", "contract": contract });
    let (_, task) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks"), Some(body)).await;
    let tid = task["id"].as_str().unwrap();
    harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks/{tid}/ready"), None).await;
    let (status, run) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks/{tid}/dispatch"), None).await;
    assert_eq!(status, StatusCode::CREATED, "{run}");
    harness.supervise().await;
    let worker = harness.agent_credential(run["id"].as_str().unwrap()).unwrap();

    for (tool, body) in [
        ("record_understanding", term("帳目", "一筆收入", "hypothesis", json!([]))),
        ("get_project_understanding", json!({})),
    ] {
        let (status, response) = harness.call_tool(&worker, tool, body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{tool}：{response}");
        assert_eq!(error_code(&response), "controller_only");
    }
    assert_eq!(view(&harness, &ws).await["items"], json!([]));
}

// --- 未知資訊不被寫成已確認事實 ---

#[tokio::test]
async fn unknown_information_is_never_recorded_or_projected_as_a_confirmed_fact() {
    let Project { harness, ws, dir } = project(true).await;
    let (_, token) = controller_turn(&harness, &ws, "建立專案理解").await;

    // 沒有來源的「已確認」被拒絕
    let (status, body) = record(&harness, &token, term("帳本", "所有帳目的集合", "confirmed", json!([]))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(error_code(&body), "understanding_unverified_claim");
    // 找不到的來源被拒絕（不論宣稱什麼狀態）
    for claimed in ["confirmed", "hypothesis"] {
        let (status, body) = record(&harness, &token, term("帳本", "所有帳目的集合", claimed, json!([{ "path": "docs/ledger.md" }]))).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert_eq!(error_code(&body), "understanding_source_not_found");
    }
    // 外部來源沒有版本、或指向 Workspace 外，都不能當作來源
    for source in [json!({ "reference": "https://example.com" }), json!({ "path": "../secret.txt" })] {
        let (status, body) = record(&harness, &token, term("帳本", "所有帳目的集合", "confirmed", json!([source]))).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert_eq!(error_code(&body), "understanding_invalid");
    }
    assert_eq!(view(&harness, &ws).await["items"], json!([]), "被拒絕的記錄什麼都不寫");
    assert_eq!(read(&dir, "GLOSSARY.md"), None);

    // 沒有來源的描述只能是假說或未知，投影中也照實標示
    recorded(&harness, &token, json!({ "section": "glossary", "key": "帳本", "text": "應該是所有帳目的集合" })).await;
    recorded(&harness, &token, term("結算", "不知道是每月還是每年", "unknown", json!([]))).await;
    recorded(&harness, &token, term("帳目", "一筆收入或支出", "confirmed", json!([{ "path": "README.md" }]))).await;

    let view = view(&harness, &ws).await;
    assert_eq!(item(&view, "glossary", "帳本")["status"], "hypothesis");
    assert_eq!(item(&view, "glossary", "結算")["status"], "unknown");
    let glossary = read(&dir, "GLOSSARY.md").unwrap();
    assert!(glossary.contains("## 帳本\n\n應該是所有帳目的集合\n\n- 狀態：假說（未確認）"), "{glossary}");
    assert!(glossary.contains("## 待確認（未知）\n\n- **結算**：不知道是每月還是每年"), "{glossary}");
    assert!(!glossary.contains("## 結算"), "未知的名詞不能寫成定義：{glossary}");
    assert_eq!(glossary.matches("- 狀態：已確認").count(), 1, "{glossary}");
}

#[tokio::test]
async fn a_confirmed_item_whose_source_file_changes_is_flagged_for_recheck() {
    let Project { harness, ws, dir } = project(true).await;
    let (_, token) = controller_turn(&harness, &ws, "建立專案理解").await;
    recorded(&harness, &token, term("帳目", "一筆收入或支出", "confirmed", json!([{ "path": "README.md" }]))).await;
    let before = view(&harness, &ws).await;
    assert_eq!(item(&before, "glossary", "帳目")["stale"], false);

    std::fs::write(dir.join("README.md"), "# 範例專案\n\n改成給個人用的記帳工具。\n").unwrap();
    let after = view(&harness, &ws).await;
    let entry = item(&after, "glossary", "帳目");
    assert_eq!(entry["stale"], true, "來源已變更：需要重新查驗");
    assert_eq!(entry["sources"][0]["changed"], true);

    // Controller 重新查驗後，版本與查驗時間更新
    harness.clock.advance_millis(60_000);
    let rechecked = recorded(&harness, &token, term("帳目", "一筆收入或支出", "confirmed", json!([{ "path": "README.md" }]))).await;
    assert_eq!(rechecked["item"]["stale"], false);
    assert_eq!(rechecked["item"]["sources"][0]["version"], sha256_hex(read(&dir, "README.md").unwrap().as_bytes()));
    assert_eq!(rechecked["item"]["sources"][0]["checked_at"], CLOCK_START.as_millis() + WINDOW_MS + 60_000);
}

// --- 投影檔被外部修改時產生協調事件而非靜默覆寫 ---

#[tokio::test]
async fn an_externally_edited_glossary_raises_a_reconciliation_event_instead_of_being_overwritten() {
    let Project { mut harness, ws, dir } = project(true).await;
    let (run_id, token) = controller_turn(&harness, &ws, "建立專案理解").await;
    recorded(&harness, &token, term("帳目", "一筆收入或支出", "confirmed", json!([{ "path": "README.md" }]))).await;
    assert!(read(&dir, "GLOSSARY.md").unwrap().contains("第 1 版"));

    // 使用者直接改了 GLOSSARY.md
    let edited = "# Glossary\n\n## 帳目\n\n使用者自己改的定義\n";
    std::fs::write(dir.join("GLOSSARY.md"), edited).unwrap();
    let response = recorded(&harness, &token, term("帳本", "所有帳目的集合", "hypothesis", json!([]))).await;
    let glossary = response["projections"].as_array().unwrap().iter().find(|p| p["path"] == "GLOSSARY.md").unwrap().clone();
    assert_eq!(glossary["status"], "conflict", "{response}");
    assert_eq!(read(&dir, "GLOSSARY.md").as_deref(), Some(edited), "外部修改沒有被覆寫");

    let view = view(&harness, &ws).await;
    let conflict = projection(&view, "GLOSSARY.md");
    assert_eq!(conflict["status"], "conflict");
    assert_eq!(conflict["external_hash"], sha256_hex(edited.as_bytes()));
    // 協調事件：Controller 在對話中收到工作事件（下一輪處理）
    let events = pending_work_events(&harness, &ws).await;
    assert_eq!(events.len(), 1, "{events:?}");
    assert!(events[0].contains("GLOSSARY.md") && events[0].contains("被外部修改"), "{events:?}");
    finish_turn(&harness, &run_id).await;

    // 協調中：之後的變更與重新比對都不寫入，也不重複通知
    let (_, token) = controller_turn(&harness, &ws, "繼續").await;
    recorded(&harness, &token, term("結算", "每月結算", "hypothesis", json!([]))).await;
    sync(&harness, &ws).await;
    assert_eq!(read(&dir, "GLOSSARY.md").as_deref(), Some(edited));
    let notices = conversation(&harness, &ws).await["turns"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["kind"] == "work_event" && t["content"].as_str().unwrap().contains("GLOSSARY.md"))
        .count();
    assert_eq!(notices, 1, "同一個協調只通知一次");

    // 重啟後協調仍在
    harness.restart().await;
    let view = sync(&harness, &ws).await;
    assert_eq!(projection(&view, "GLOSSARY.md")["status"], "conflict");

    // 使用者檢視後檔案又被改：拒絕，不覆寫沒看過的內容
    let seen = sha256_hex(edited.as_bytes());
    let edited_again = format!("{edited}\n再補一行\n");
    std::fs::write(dir.join("GLOSSARY.md"), &edited_again).unwrap();
    let (status, body) = reconcile(&harness, &ws, "GLOSSARY.md", "overwrite", Some(&seen)).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(error_code(&body), "projection_changed");
    assert_eq!(read(&dir, "GLOSSARY.md").as_deref(), Some(edited_again.as_str()));

    // 保留外部版本：Baton 停止寫入這個檔
    let seen = sha256_hex(edited_again.as_bytes());
    let (status, kept) = reconcile(&harness, &ws, "GLOSSARY.md", "keep_external", Some(&seen)).await;
    assert_eq!(status, StatusCode::OK, "{kept}");
    assert_eq!(projection(&kept, "GLOSSARY.md")["status"], "detached");
    sync(&harness, &ws).await;
    assert_eq!(read(&dir, "GLOSSARY.md").as_deref(), Some(edited_again.as_str()));

    // 之後使用者改為以 Baton 版本覆寫：寫入目前的專案理解，版本遞增
    let (status, overwritten) = reconcile(&harness, &ws, "GLOSSARY.md", "overwrite", Some(&seen)).await;
    assert_eq!(status, StatusCode::OK, "{overwritten}");
    assert_eq!(projection(&overwritten, "GLOSSARY.md")["status"], "in_sync");
    let glossary = read(&dir, "GLOSSARY.md").unwrap();
    assert!(glossary.contains("Glossary 第 2 版"), "{glossary}");
    assert!(glossary.contains("## 帳本") && glossary.contains("## 結算") && glossary.contains("## 帳目"), "{glossary}");
}

#[tokio::test]
async fn a_kept_external_glossary_edited_again_can_still_be_overwritten_with_the_hash_from_the_view() {
    let Project { harness, ws, dir } = project(true).await;
    let (_, token) = controller_turn(&harness, &ws, "建立專案理解").await;
    recorded(&harness, &token, term("帳目", "一筆收入或支出", "confirmed", json!([{ "path": "README.md" }]))).await;
    let edited = "# Glossary

使用者自己的版本
";
    std::fs::write(dir.join("GLOSSARY.md"), edited).unwrap();
    let view = sync(&harness, &ws).await;
    let seen = projection(&view, "GLOSSARY.md")["external_hash"].as_str().unwrap().to_string();
    let (status, kept) = reconcile(&harness, &ws, "GLOSSARY.md", "keep_external", Some(&seen)).await;
    assert_eq!(status, StatusCode::OK, "{kept}");
    assert_eq!(projection(&kept, "GLOSSARY.md")["status"], "detached");
    assert_eq!(projection(&kept, "GLOSSARY.md")["external_hash"], seen.as_str());

    // 保留外部版本之後使用者繼續編輯：重新比對不寫入、不通知，但視圖帶著檔案目前的雜湊
    let edited_again = format!("{edited}
又補了一段
");
    std::fs::write(dir.join("GLOSSARY.md"), &edited_again).unwrap();
    let view = sync(&harness, &ws).await;
    let detached = projection(&view, "GLOSSARY.md");
    assert_eq!(detached["status"], "detached");
    assert_eq!(detached["external_hash"], sha256_hex(edited_again.as_bytes()));
    assert_eq!(read(&dir, "GLOSSARY.md").as_deref(), Some(edited_again.as_str()));
    let notices = pending_work_events(&harness, &ws).await;
    assert_eq!(notices.iter().filter(|e| e.contains("GLOSSARY.md")).count(), 1, "只有最初的協調通知過 Controller：{notices:?}");

    // 用過時的雜湊（保留時看到的版本）覆寫：拒絕
    let (status, body) = reconcile(&harness, &ws, "GLOSSARY.md", "overwrite", Some(&seen)).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(error_code(&body), "projection_changed");
    assert_eq!(read(&dir, "GLOSSARY.md").as_deref(), Some(edited_again.as_str()));

    // 用視圖提供的雜湊覆寫：寫入 Baton 版本
    let current = detached["external_hash"].as_str().unwrap();
    let (status, overwritten) = reconcile(&harness, &ws, "GLOSSARY.md", "overwrite", Some(current)).await;
    assert_eq!(status, StatusCode::OK, "{overwritten}");
    assert_eq!(projection(&overwritten, "GLOSSARY.md")["status"], "in_sync");
    let glossary = read(&dir, "GLOSSARY.md").unwrap();
    assert!(glossary.contains("Baton 投影：Glossary") && glossary.contains("## 帳目"), "{glossary}");
}

#[tokio::test]
async fn a_glossary_that_already_exists_in_the_repo_is_not_overwritten_on_first_projection() {
    let Project { harness, ws, dir } = project(true).await;
    let ours = "# 我們自己的 Glossary\n";
    std::fs::write(dir.join("GLOSSARY.md"), ours).unwrap();
    let (_, token) = controller_turn(&harness, &ws, "建立專案理解").await;
    recorded(&harness, &token, term("帳目", "一筆收入或支出", "confirmed", json!([{ "path": "README.md" }]))).await;

    assert_eq!(read(&dir, "GLOSSARY.md").as_deref(), Some(ours));
    let view = view(&harness, &ws).await;
    assert_eq!(projection(&view, "GLOSSARY.md")["status"], "conflict");
    let events = pending_work_events(&harness, &ws).await;
    assert!(events.iter().any(|e| e.contains("GLOSSARY.md") && e.contains("之前就已存在")), "{events:?}");
}

#[tokio::test]
async fn workspace_decisions_are_projected_as_adrs_and_an_external_deletion_is_reconciled() {
    let Project { harness, ws, dir } = project(true).await;
    let decision = json!({
        "kind": "decision", "scope": { "kind": "workspace", "workspace_id": ws },
        "title": "帳目一律以分為單位存整數", "body": "避免浮點誤差", "evidence": ["README.md"],
    });
    let (status, entry) = harness.send_json(Method::POST, "/api/v1/knowledge", Some(decision)).await;
    assert_eq!(status, StatusCode::CREATED, "{entry}");
    let id = entry["id"].as_str().unwrap();
    let path = format!("docs/adr/baton-{id}.md");
    let adr = read(&dir, &path).expect("Workspace 範圍的決策應該投影成 ADR");
    assert!(adr.contains("ADR 第 1 版") && adr.contains("# 帳目一律以分為單位存整數") && adr.contains("- 狀態：已採用"), "{adr}");

    // 決策被撤銷：檔案沒被動過，直接更新
    let body = json!({ "expected_revision": entry["revision"], "reason": "改用十進位型別" });
    let (status, revoked) = harness.send_json(Method::POST, &format!("/api/v1/knowledge/{id}/revoke"), Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{revoked}");
    let adr = read(&dir, &path).unwrap();
    assert!(adr.contains("ADR 第 2 版") && adr.contains("- 狀態：已撤銷") && adr.contains("改用十進位型別"), "{adr}");

    // 外部刪除：重新比對時開啟協調，不擅自重建
    std::fs::remove_file(dir.join(&path)).unwrap();
    let view = sync(&harness, &ws).await;
    let conflict = projection(&view, &path);
    assert_eq!(conflict["status"], "conflict");
    assert_eq!(conflict["external_hash"], Value::Null);
    assert_eq!(read(&dir, &path), None);

    // 使用者選擇以 Baton 版本覆寫（檢視時檔案不存在）
    let (status, reconciled) = reconcile(&harness, &ws, &path, "overwrite", None).await;
    assert_eq!(status, StatusCode::OK, "{reconciled}");
    assert_eq!(projection(&reconciled, &path)["status"], "in_sync");
    assert!(read(&dir, &path).unwrap().contains("- 狀態：已撤銷"));
    // 沒有在協調中的檔案不能保留外部版本
    let (status, body) = reconcile(&harness, &ws, &path, "keep_external", None).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(error_code(&body), "projection_transition_invalid");
}

#[tokio::test]
async fn a_workspace_that_is_not_a_git_repo_gets_no_projection_files() {
    let Project { harness, ws, dir } = project(false).await;
    let (_, token) = controller_turn(&harness, &ws, "建立專案理解").await;
    recorded(&harness, &token, term("帳目", "一筆收入或支出", "confirmed", json!([{ "path": "README.md" }]))).await;

    let view = sync(&harness, &ws).await;
    assert_eq!(view["projection_enabled"], false);
    assert_eq!(view["projections"], json!([]));
    assert_eq!(item(&view, "glossary", "帳目")["status"], "confirmed");
    assert_eq!(read(&dir, "GLOSSARY.md"), None);
}
