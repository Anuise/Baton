//! 情境測試：知識、教訓與全文檢索（票 37，spec D-26、D-28），經由 Runtime harness 的真實 HTTP API 驗證。
//!
//! Agent（Worker 的憑證；Controller 憑證尚未存在，見 ai-decisions）以 `record_lesson_candidate` 提交候選教訓、
//! 以 `search_knowledge` 取得「有效」的知識；使用者以桌面憑證在知識分頁的 API 確認、修正、撤銷、淘汰與搜尋。
//! 「被當作已驗證載入」以 Agent 的 `search_knowledge` 觀察：它只回傳有效條目，是 Agent 取得知識的唯一入口。

use baton_domain::knowledge::{TOOL_RECORD_LESSON_CANDIDATE, TOOL_SEARCH_KNOWLEDGE};
use baton_domain::Timestamp;
use baton_harness::Harness;
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

const WORKSPACES: &str = "/api/v1/workspaces";
const KNOWLEDGE: &str = "/api/v1/knowledge";
const CLOCK_START: Timestamp = Timestamp::from_millis(1_700_000_000_000);

// --- 共用輔助 ---

async fn started(builder: baton_harness::HarnessBuilder) -> Harness {
    let harness = builder.clock_at(CLOCK_START).start().await;
    // 派工需要一個可派工的帳號（票 19）
    harness.add_account("主要帳號", "main@example.com").await;
    harness
}

async fn harness() -> Harness {
    started(Harness::builder()).await
}

async fn new_workspace(harness: &Harness, name: &str) -> String {
    let dir = harness.data_dir().join("projects").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let (status, body) =
        harness.send_json(Method::POST, WORKSPACES, Some(json!({ "path": dir.to_string_lossy() }))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["id"].as_str().unwrap().to_string()
}

fn filled(text: &str) -> Value {
    json!({ "status": "filled", "text": text })
}

/// 在 Workspace 內建立 Goal 與 ready 的 Task，回傳 (goal_id, task_id)。
async fn ready_task(harness: &Harness, ws: &str, title: &str) -> (String, String) {
    let (status, goal) =
        harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/goals"), Some(json!({ "title": "目標" }))).await;
    assert_eq!(status, StatusCode::CREATED, "{goal}");
    let contract = json!({
        "objective": filled("整理資料"),
        "inputs": filled("既有的資料"),
        "scope": filled("公開資料"),
        "deliverables": filled("比較表"),
        "done_criteria": filled("五個產品"),
        "blocker_handling": filled("卡住時回報"),
        "required_capabilities": [],
    });
    let body = json!({ "goal_id": goal["id"], "title": title, "contract": contract });
    let (status, task) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks"), Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    let tid = task["id"].as_str().unwrap().to_string();
    let (status, ready) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks/{tid}/ready"), None).await;
    assert_eq!(status, StatusCode::OK, "{ready}");
    (goal["id"].as_str().unwrap().to_string(), tid)
}

/// 派工並讓 Supervisor 啟動它，回傳 (run_id, Worker 的 Agent 憑證)。
async fn start_run(harness: &Harness, ws: &str, tid: &str) -> (String, String) {
    let (status, run) = harness.send_json(Method::POST, &format!("{WORKSPACES}/{ws}/tasks/{tid}/dispatch"), None).await;
    assert_eq!(status, StatusCode::CREATED, "{run}");
    let rid = run["id"].as_str().unwrap().to_string();
    harness.supervise().await;
    let token = harness.agent_credential(&rid).expect("Run 啟動時應該把 Agent 憑證放進程序的環境變數");
    (rid, token)
}

/// 一個 Workspace、一個進行中的 Run：(workspace_id, goal_id, task_id, run_id, Agent 憑證)。
async fn running(harness: &Harness, name: &str) -> (String, String, String, String, String) {
    let ws = new_workspace(harness, name).await;
    let (goal, tid) = ready_task(harness, &ws, "整理").await;
    let (rid, token) = start_run(harness, &ws, &tid).await;
    (ws, goal, tid, rid, token)
}

fn error_code(body: &Value) -> &str {
    body["error"].as_str().unwrap_or_default()
}

/// 使用者建立條目，預期成功，回傳條目。
async fn create(harness: &Harness, body: Value) -> Value {
    let (status, entry) = harness.send_json(Method::POST, KNOWLEDGE, Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{entry}");
    entry
}

fn in_workspace(ws: &str, kind: &str, title: &str, body: &str) -> Value {
    json!({ "kind": kind, "scope": { "kind": "workspace", "workspace_id": ws }, "title": title, "body": body })
}

/// 桌面列表：`query` 是查詢字串（不含 `?`）。
async fn list(harness: &Harness, query: &str) -> Value {
    let (status, body) = harness.send_json(Method::GET, &format!("{KNOWLEDGE}?{query}"), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

/// 桌面搜尋 Workspace 內的有效條目，回傳標題（依更新時間由新到舊）。
async fn search_titles(harness: &Harness, ws: &str, q: &str) -> Vec<String> {
    let query = format!("workspace_id={ws}&q={}", urlencode(q));
    titles(&list(harness, &query).await)
}

fn titles(list: &Value) -> Vec<String> {
    list["entries"].as_array().unwrap().iter().map(|e| e["content"]["title"].as_str().unwrap().to_string()).collect()
}

fn urlencode(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

async fn action(harness: &Harness, id: &str, name: &str, body: Value) -> (StatusCode, Value) {
    harness.send_json(Method::POST, &format!("{KNOWLEDGE}/{id}/{name}"), Some(body)).await
}

async fn detail(harness: &Harness, id: &str) -> Value {
    let (status, body) = harness.send_json(Method::GET, &format!("{KNOWLEDGE}/{id}"), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

/// Agent 搜尋，預期成功，回傳標題。
async fn agent_search(harness: &Harness, token: &str, query: &str) -> Vec<String> {
    let (status, body) = harness.call_tool(token, TOOL_SEARCH_KNOWLEDGE, json!({ "query": query })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    titles(&body)
}

fn knowledge_rows(harness: &Harness) -> i64 {
    harness.db().query_row("SELECT COUNT(*) FROM knowledge_entry", [], |r| r.get(0)).unwrap()
}

// ============================================================ 候選教訓與使用者確認（D-28）

#[tokio::test]
async fn a_candidate_lesson_is_not_loaded_as_verified_until_the_user_confirms_it() {
    let mut harness = harness().await;
    let (ws, _goal, tid, rid, token) = running(&harness, "main").await;

    // Agent 試圖直接提交「已驗證」：狀態欄位不被採信，結果一律是候選
    let candidate = json!({
        "title": "測試前先清除快取",
        "body": "快取殘留會讓整合測試誤判為通過",
        "trigger": "整合測試在第二次執行時才通過",
        "root_cause": "建置快取沒有失效",
        "resolution": "測試前清除快取目錄",
        "evidence": ["run 紀錄：第二次才通過"],
        "status": "verified",
        "kind": "decision",
    });
    let (status, recorded) = harness.call_tool(&token, TOOL_RECORD_LESSON_CANDIDATE, candidate).await;
    assert_eq!(status, StatusCode::CREATED, "{recorded}");
    assert_eq!(recorded["status"], "candidate");
    let id = recorded["entry_id"].as_str().unwrap().to_string();

    // 未確認：Agent 搜不到，知識分頁的有效清單也沒有它
    assert!(agent_search(&harness, &token, "快取").await.is_empty(), "候選教訓不能被 Agent 當作有效知識載入");
    assert!(agent_search(&harness, &token, "").await.is_empty());
    assert!(search_titles(&harness, &ws, "快取").await.is_empty());

    // 知識分頁可以看到候選本身（待確認），附來源 Run 與教訓內容
    let all = list(&harness, &format!("workspace_id={ws}&include_inactive=true")).await;
    let entry = &all["entries"][0];
    assert_eq!(entry["id"], id.as_str());
    assert_eq!((entry["kind"].as_str(), entry["status"].as_str(), entry["effective"].as_bool()), (Some("lesson"), Some("candidate"), Some(false)));
    assert_eq!(entry["source"], json!({ "kind": "agent", "run_id": rid, "generation": 1, "task_id": tid }));
    assert_eq!(entry["scope"]["kind"], "task");
    assert_eq!(entry["scope"]["task_id"], tid.as_str());
    assert_eq!(entry["content"]["lesson"]["resolution"], "測試前清除快取目錄");
    assert_eq!(entry["content"]["evidence"], json!(["run 紀錄：第二次才通過"]));

    // 重啟後仍是候選
    harness.restart().await;
    assert!(agent_search(&harness, &token, "快取").await.is_empty());
    assert_eq!(detail(&harness, &id).await["entry"]["status"], "candidate");

    // 使用者確認之後才升為已驗證，Agent 才搜得到
    let (status, confirmed) = action(&harness, &id, "confirm", json!({ "expected_revision": 1 })).await;
    assert_eq!(status, StatusCode::OK, "{confirmed}");
    assert_eq!((confirmed["status"].as_str(), confirmed["effective"].as_bool()), (Some("verified"), Some(true)));
    assert_eq!(agent_search(&harness, &token, "快取").await, ["測試前先清除快取"]);
    assert_eq!(search_titles(&harness, &ws, "快取").await, ["測試前先清除快取"]);

    let history: Vec<_> = detail(&harness, &id).await["history"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["event_type"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(history, ["knowledge_created", "knowledge_confirmed"]);
}

#[tokio::test]
async fn agents_can_only_submit_candidates_within_their_own_run_and_never_through_the_management_api() {
    let harness = harness().await;
    let (ws, goal, tid, _rid, token) = running(&harness, "main").await;

    // 範圍由憑證所屬 Run 的 Task 解析：不能是全域，也不能指定別的 Workspace
    let (status, body) =
        harness.call_tool(&token, TOOL_RECORD_LESSON_CANDIDATE, json!({ "title": "全域教訓", "scope": "global" })).await;
    assert_eq!((status, error_code(&body)), (StatusCode::BAD_REQUEST, "knowledge_scope_invalid"), "{body}");
    let foreign = json!({ "title": "別人的", "workspace_id": "ws_other" });
    let (status, body) = harness.call_tool(&token, TOOL_RECORD_LESSON_CANDIDATE, foreign).await;
    assert_eq!((status, error_code(&body)), (StatusCode::FORBIDDEN, "agent_scope_mismatch"), "{body}");
    let (status, body) = harness.call_tool(&token, TOOL_RECORD_LESSON_CANDIDATE, json!({ "title": "  " })).await;
    assert_eq!((status, error_code(&body)), (StatusCode::BAD_REQUEST, "knowledge_invalid"), "{body}");

    let (status, body) =
        harness.call_tool(&token, TOOL_RECORD_LESSON_CANDIDATE, json!({ "title": "目標層級的教訓", "scope": "goal" })).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let entry = detail(&harness, body["entry_id"].as_str().unwrap()).await["entry"].clone();
    assert_eq!(entry["scope"], json!({ "kind": "goal", "workspace_id": ws, "goal_id": goal, "task_id": null }));
    let _ = tid;

    // Agent 憑證不能使用知識分頁的管理 API（建立、確認、列表）
    let id = body["entry_id"].as_str().unwrap();
    let attempts = [
        (Method::GET, KNOWLEDGE.to_string(), None),
        (Method::POST, KNOWLEDGE.to_string(), Some(in_workspace(&ws, "lesson", "自己確認", ""))),
        (Method::POST, format!("{KNOWLEDGE}/{id}/confirm"), Some(json!({ "expected_revision": 1 }))),
    ];
    for (method, path, body) in attempts {
        let mut request = harness
            .request(method.clone(), &path)
            .bearer_auth(&token)
            .header(baton_api::CLIENT_HEADER, baton_api::CLIENT_HEADER_VALUE);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.unwrap();
        let status = response.status();
        let body: Value = response.json().await.unwrap();
        assert_eq!((status, error_code(&body)), (StatusCode::FORBIDDEN, "role_not_allowed"), "{method} {path}");
    }
    assert_eq!(detail(&harness, id).await["entry"]["status"], "candidate");

    // 桌面憑證也不能冒充 Agent 提交候選
    let (status, body) = harness
        .call_tool(&harness.desktop_credential(), TOOL_RECORD_LESSON_CANDIDATE, json!({ "title": "桌面送的" }))
        .await;
    assert_eq!((status, error_code(&body)), (StatusCode::FORBIDDEN, "role_not_allowed"), "{body}");
    assert_eq!(knowledge_rows(&harness), 1, "被拒絕的請求不寫入任何東西");
}

#[tokio::test]
async fn a_run_that_is_no_longer_the_writer_cannot_submit_or_search() {
    let harness = harness().await;
    let ws = new_workspace(&harness, "main").await;
    let (_goal, tid) = ready_task(&harness, &ws, "整理").await;
    harness.agent_adapter.script_next_run(vec![baton_ports::AdapterEvent::Exited { success: true }]);
    let (_rid, token) = start_run(&harness, &ws, &tid).await;
    harness.supervise().await; // Run 結束

    let (status, body) = harness.call_tool(&token, TOOL_RECORD_LESSON_CANDIDATE, json!({ "title": "結束後才提交" })).await;
    assert_eq!((status, error_code(&body)), (StatusCode::CONFLICT, "run_report_stale"), "{body}");
    let (status, body) = harness.call_tool(&token, TOOL_SEARCH_KNOWLEDGE, json!({ "query": "" })).await;
    assert_eq!((status, error_code(&body)), (StatusCode::UNAUTHORIZED, "credential_revoked"), "{body}");
    assert_eq!(knowledge_rows(&harness), 0);
}

// ============================================================ 修正、撤銷、淘汰、取代與歷史

#[tokio::test]
async fn a_revoked_lesson_keeps_its_history_and_is_no_longer_found_as_effective() {
    let mut harness = harness().await;
    let (ws, _goal, _tid, _rid, token) = running(&harness, "main").await;
    let mut lesson = in_workspace(&ws, "lesson", "重試前等待十秒", "連線逾時後立即重試會被限流");
    lesson["lesson"] = json!({ "trigger": "連線逾時", "root_cause": "", "resolution": "等待十秒再重試" });
    let created = create(&harness, lesson).await;
    let id = created["id"].as_str().unwrap().to_string();
    assert_eq!((created["status"].as_str(), created["revision"].as_i64()), (Some("verified"), Some(1)));
    assert_eq!(agent_search(&harness, &token, "逾時").await, ["重試前等待十秒"]);

    // 修正：追加第 2 版，舊內容保留
    let correction = json!({
        "expected_revision": 1,
        "title": "重試前等待三十秒",
        "body": "連線逾時後立即重試會被限流",
        "lesson": { "trigger": "連線逾時", "root_cause": "服務端限流", "resolution": "等待三十秒再重試" },
        "note": "十秒仍會被限流",
    });
    let (status, corrected) = action(&harness, &id, "correct", correction).await;
    assert_eq!(status, StatusCode::OK, "{corrected}");
    assert_eq!((corrected["version"].as_i64(), corrected["revision"].as_i64()), (Some(2), Some(2)));
    assert_eq!(agent_search(&harness, &token, "三十秒").await, ["重試前等待三十秒"]);
    assert!(agent_search(&harness, &token, "等待十秒").await.is_empty(), "舊版本的文字不再被搜尋到");

    // 撤銷：需要原因；之後不再被當作有效知識搜尋到
    let (status, body) = action(&harness, &id, "revoke", json!({ "expected_revision": 2, "reason": " " })).await;
    assert_eq!((status, error_code(&body)), (StatusCode::BAD_REQUEST, "knowledge_invalid"), "{body}");
    let (status, revoked) =
        action(&harness, &id, "revoke", json!({ "expected_revision": 2, "reason": "限流與等待時間無關" })).await;
    assert_eq!(status, StatusCode::OK, "{revoked}");
    assert_eq!((revoked["status"].as_str(), revoked["effective"].as_bool()), (Some("revoked"), Some(false)));

    harness.restart().await;
    assert!(agent_search(&harness, &token, "逾時").await.is_empty(), "撤銷的教訓不再被 Agent 載入");
    assert!(search_titles(&harness, &ws, "逾時").await.is_empty(), "撤銷的教訓不再是有效的搜尋結果");

    // 歷史完整保留：顯示歷史時仍找得到，附撤銷原因、兩個版本與全部變更
    let all = list(&harness, &format!("workspace_id={ws}&q={}&include_inactive=true", urlencode("逾時"))).await;
    assert_eq!(titles(&all), ["重試前等待三十秒"]);
    assert_eq!(all["entries"][0]["status_reason"], "限流與等待時間無關");
    let detail = detail(&harness, &id).await;
    let versions: Vec<_> = detail["versions"].as_array().unwrap().iter().map(|v| v["content"]["title"].clone()).collect();
    assert_eq!(versions, [json!("重試前等待十秒"), json!("重試前等待三十秒")]);
    assert_eq!(detail["versions"][1]["note"], "十秒仍會被限流");
    let history: Vec<_> =
        detail["history"].as_array().unwrap().iter().map(|e| e["event_type"].as_str().unwrap().to_string()).collect();
    assert_eq!(history, ["knowledge_created", "knowledge_corrected", "knowledge_revoked"]);
    assert_eq!(detail["history"][2]["payload"]["reason"], "限流與等待時間無關");

    // 撤銷是終態
    let (status, body) = action(&harness, &id, "confirm", json!({ "expected_revision": 3 })).await;
    assert_eq!((status, error_code(&body)), (StatusCode::CONFLICT, "knowledge_transition_invalid"), "{body}");
    let rows: i64 = harness
        .db()
        .query_row("SELECT COUNT(*) FROM knowledge_entry_version WHERE entry_id = ?1", [&id], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 2);
}

#[tokio::test]
async fn a_rejected_candidate_is_revoked_and_never_becomes_effective() {
    let harness = harness().await;
    let (ws, _goal, _tid, _rid, token) = running(&harness, "main").await;
    let (_, recorded) =
        harness.call_tool(&token, TOOL_RECORD_LESSON_CANDIDATE, json!({ "title": "每次都刪除工作目錄" })).await;
    let id = recorded["entry_id"].as_str().unwrap();

    let (status, revoked) = action(&harness, id, "revoke", json!({ "expected_revision": 1, "reason": "會刪掉使用者的資料" })).await;
    assert_eq!(status, StatusCode::OK, "{revoked}");
    let (status, body) = action(&harness, id, "confirm", json!({ "expected_revision": 2 })).await;
    assert_eq!((status, error_code(&body)), (StatusCode::CONFLICT, "knowledge_transition_invalid"), "{body}");
    assert!(agent_search(&harness, &token, "工作目錄").await.is_empty());
    assert!(search_titles(&harness, &ws, "工作目錄").await.is_empty());
}

#[tokio::test]
async fn stale_revisions_are_rejected_so_concurrent_edits_do_not_overwrite_each_other() {
    let harness = harness().await;
    let ws = new_workspace(&harness, "main").await;
    let id = create(&harness, in_workspace(&ws, "fact", "部署在週五凍結", "")).await["id"].as_str().unwrap().to_string();
    let (status, _) = action(&harness, &id, "retire", json!({ "expected_revision": 1, "reason": "已取消凍結" })).await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = action(&harness, &id, "revoke", json!({ "expected_revision": 1, "reason": "舊畫面送出" })).await;
    assert_eq!((status, error_code(&body)), (StatusCode::CONFLICT, "knowledge_revision_conflict"), "{body}");
    assert_eq!(detail(&harness, &id).await["entry"]["status"], "retired");
    assert!(search_titles(&harness, &ws, "凍結").await.is_empty(), "淘汰的條目不是有效知識");

    let (status, body) = action(&harness, "know_missing", "confirm", json!({ "expected_revision": 1 })).await;
    assert_eq!((status, error_code(&body)), (StatusCode::NOT_FOUND, "knowledge_not_found"), "{body}");
}

#[tokio::test]
async fn a_new_decision_can_supersede_an_old_one_and_both_stay_linked() {
    let harness = harness().await;
    let ws = new_workspace(&harness, "main").await;
    let old = create(&harness, in_workspace(&ws, "decision", "資料庫採用 PostgreSQL", "")).await;
    let old_id = old["id"].as_str().unwrap();
    let mut replacement = in_workspace(&ws, "decision", "資料庫改用 SQLite", "單機桌面應用不需要伺服器");
    replacement["supersedes"] = json!(old_id);
    let new = create(&harness, replacement).await;

    assert_eq!(new["supersedes"], old_id);
    let old = detail(&harness, old_id).await["entry"].clone();
    assert_eq!((old["status"].as_str(), old["superseded_by"].as_str()), (Some("superseded"), new["id"].as_str()));
    assert_eq!(search_titles(&harness, &ws, "資料庫").await, ["資料庫改用 SQLite"]);

    // 已被取代的不能再被取代
    let mut again = in_workspace(&ws, "decision", "資料庫改用 MySQL", "");
    again["supersedes"] = json!(old_id);
    let (status, body) = harness.send_json(Method::POST, KNOWLEDGE, Some(again)).await;
    assert_eq!((status, error_code(&body)), (StatusCode::CONFLICT, "knowledge_supersede_invalid"), "{body}");
}

// ============================================================ 範圍與有效期間

#[tokio::test]
async fn agents_only_get_knowledge_that_applies_to_their_run() {
    let harness = harness().await;
    let (ws, goal, tid, _rid, token) = running(&harness, "main").await;
    let (other_goal, other_task) = ready_task(&harness, &ws, "另一個工作").await;
    let other_ws = new_workspace(&harness, "other").await;

    let scoped = |scope: Value, title: &str| json!({ "kind": "fact", "scope": scope, "title": title });
    for body in [
        scoped(json!({ "kind": "global" }), "全域：提交訊息用繁體中文"),
        scoped(json!({ "kind": "workspace", "workspace_id": ws }), "本工作區：提交訊息附票號"),
        scoped(json!({ "kind": "goal", "workspace_id": ws, "goal_id": goal }), "本目標：提交訊息附目標"),
        scoped(json!({ "kind": "task", "workspace_id": ws, "task_id": tid }), "本工作：提交訊息附作者"),
        scoped(json!({ "kind": "goal", "workspace_id": ws, "goal_id": other_goal }), "別的目標：提交訊息不限"),
        scoped(json!({ "kind": "task", "workspace_id": ws, "task_id": other_task }), "別的工作：提交訊息不限"),
        scoped(json!({ "kind": "workspace", "workspace_id": other_ws }), "別的工作區：提交訊息不限"),
    ] {
        create(&harness, body).await;
    }

    let mut seen = agent_search(&harness, &token, "提交訊息").await;
    seen.sort();
    let mut expected = ["全域：提交訊息用繁體中文", "本工作區：提交訊息附票號", "本目標：提交訊息附目標", "本工作：提交訊息附作者"];
    expected.sort();
    assert_eq!(seen, expected);

    // 知識分頁：Workspace 內全部範圍加上全域；不指定 Workspace 時只有全域
    assert_eq!(search_titles(&harness, &ws, "提交訊息").await.len(), 6);
    assert_eq!(titles(&list(&harness, &format!("q={}", urlencode("提交訊息"))).await), ["全域：提交訊息用繁體中文"]);

    // Task 範圍的 Goal 由 Runtime 依 Task 補上；不存在或不屬於這個 Workspace 的範圍被拒絕
    let entry = &list(&harness, &format!("workspace_id={ws}&q={}", urlencode("附作者"))).await["entries"][0];
    assert_eq!(entry["scope"]["goal_id"], goal.as_str());
    for scope in [
        json!({ "kind": "workspace", "workspace_id": "ws_missing" }),
        json!({ "kind": "task", "workspace_id": other_ws, "task_id": tid }),
        json!({ "kind": "global", "workspace_id": ws }),
        json!({ "kind": "goal", "workspace_id": ws }),
    ] {
        let (status, body) = harness.send_json(Method::POST, KNOWLEDGE, Some(scoped(scope.clone(), "錯誤範圍"))).await;
        assert_eq!((status, error_code(&body)), (StatusCode::BAD_REQUEST, "knowledge_scope_invalid"), "{scope}");
    }
    let (status, body) = harness.send_json(Method::GET, &format!("{KNOWLEDGE}?workspace_id=ws_missing"), None).await;
    assert_eq!((status, error_code(&body)), (StatusCode::NOT_FOUND, "workspace_not_found"), "{body}");
}

#[tokio::test]
async fn knowledge_outside_its_validity_period_is_not_effective() {
    let harness = harness().await;
    let ws = new_workspace(&harness, "main").await;
    let mut body = in_workspace(&ws, "decision", "暫時停用夜間建置", "");
    body["valid_until"] = json!(CLOCK_START.as_millis() + 60_000);
    let entry = create(&harness, body).await;
    assert_eq!(entry["effective"], true);
    assert_eq!(search_titles(&harness, &ws, "夜間建置").await, ["暫時停用夜間建置"]);

    harness.clock.advance_millis(60_000);
    assert!(search_titles(&harness, &ws, "夜間建置").await.is_empty(), "過期之後不是有效知識");
    let all = list(&harness, &format!("workspace_id={ws}&include_inactive=true")).await;
    assert_eq!((all["entries"][0]["status"].as_str(), all["entries"][0]["effective"].as_bool()), (Some("verified"), Some(false)));
}

// ============================================================ 繁體中文全文檢索與降級（D-26）

async fn seed_for_search(harness: &Harness, ws: &str) {
    create(harness, in_workspace(ws, "decision", "快取一律放在資料目錄", "避免使用者的專案目錄被寫入暫存檔")).await;
    let mut lesson = in_workspace(ws, "lesson", "測試前先清除快取", "");
    lesson["lesson"] = json!({ "trigger": "整合測試時好時壞", "root_cause": "快取殘留", "resolution": "清除後重跑" });
    create(harness, lesson).await;
    create(harness, in_workspace(ws, "fact", "部署視窗在週二", "需要提前一天通知")).await;
}

async fn assert_traditional_chinese_search(harness: &Harness, ws: &str) {
    let mut found = search_titles(harness, ws, "快取").await;
    found.sort();
    assert_eq!(found, ["快取一律放在資料目錄", "測試前先清除快取"], "兩個字的關鍵字找到決策與教訓");
    assert_eq!(search_titles(harness, ws, "時好時壞").await, ["測試前先清除快取"], "教訓的觸發條件也被檢索");
    assert_eq!(search_titles(harness, ws, "專案目錄").await, ["快取一律放在資料目錄"]);
    assert_eq!(search_titles(harness, ws, "快取 殘留").await, ["測試前先清除快取"], "多個關鍵字全部都要出現");
    assert_eq!(search_titles(harness, ws, "部署").await, ["部署視窗在週二"]);
    assert!(search_titles(harness, ws, "資料庫").await.is_empty());

    let (status, body) = harness.send_json(Method::GET, &format!("{KNOWLEDGE}?workspace_id={ws}&q={}", urlencode("快")), None).await;
    assert_eq!((status, error_code(&body)), (StatusCode::BAD_REQUEST, "knowledge_query_invalid"), "{body}");
}

#[tokio::test]
async fn traditional_chinese_keywords_of_two_or_more_characters_find_decisions_and_lessons() {
    let harness = harness().await;
    let (ws, _goal, _tid, _rid, token) = running(&harness, "main").await;
    seed_for_search(&harness, &ws).await;

    assert_eq!(list(&harness, &format!("workspace_id={ws}")).await["search_mode"], "full_text");
    assert_traditional_chinese_search(&harness, &ws).await;
    let mut seen = agent_search(&harness, &token, "快取").await;
    seen.sort();
    assert_eq!(seen, ["快取一律放在資料目錄", "測試前先清除快取"]);
    let (status, body) = harness.call_tool(&token, TOOL_SEARCH_KNOWLEDGE, json!({ "query": "快" })).await;
    assert_eq!((status, error_code(&body)), (StatusCode::BAD_REQUEST, "knowledge_query_invalid"), "{body}");
}

#[tokio::test]
async fn without_fts5_search_falls_back_to_like_shows_the_degradation_and_recovers_after_rebuilding() {
    let mut harness = started(Harness::builder().without_full_text_search()).await;
    let ws = new_workspace(&harness, "main").await;
    seed_for_search(&harness, &ws).await;

    // 降級：結果相同，回應與日誌都標示降級
    assert_eq!(list(&harness, &format!("workspace_id={ws}")).await["search_mode"], "like_fallback");
    assert!(harness.log_text().contains("降級"), "日誌應記錄全文檢索降級：{}", harness.log_text());
    assert_traditional_chinese_search(&harness, &ws).await;

    // FTS5 恢復可用：啟動時從條目重建索引，降級期間寫入的條目一樣搜得到
    harness.set_full_text_search(true);
    harness.restart().await;
    assert_eq!(list(&harness, &format!("workspace_id={ws}")).await["search_mode"], "full_text");
    assert_traditional_chinese_search(&harness, &ws).await;
}
