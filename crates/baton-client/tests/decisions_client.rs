//! 桌面殼經由 RuntimeClient 使用裁定中心（票 22）：與 App 實際使用的路徑相同（讀憑證檔、帶標頭）。
//! Agent 端的請求用真實 Agent 憑證打 Agent API 製造。

use std::sync::Arc;

use baton_api::{CreateTaskRequest, CreateWorkspaceRequest};
use baton_client::{ClientError, DispatchOutcome, RuntimeClient};
use baton_domain::decision::{DecisionKind, DecisionStatus};
use baton_domain::task::{ContractContent, ContractField};
use baton_domain::agent::TOOL_REQUEST_DECISION;
use baton_harness::fakes::FakeSecretProtector;
use baton_harness::Harness;
use reqwest::StatusCode;
use serde_json::json;

fn app_client(harness: &Harness) -> RuntimeClient {
    RuntimeClient::new(harness.addr(), harness.data_dir().to_path_buf(), Arc::new(FakeSecretProtector))
}

fn filled(text: &str) -> Option<ContractField> {
    Some(ContractField::Filled { text: text.into(), references: vec![] })
}

fn complete() -> ContractContent {
    ContractContent {
        objective: filled("整理資料"),
        inputs: Some(ContractField::NotApplicable { reason: "沒有輸入".into() }),
        scope: filled("公開資料"),
        deliverables: filled("比較表"),
        done_criteria: filled("五個產品"),
        blocker_handling: filled("回報"),
        required_capabilities: vec![],
        verification_checks: vec![],
    }
}

/// 一個 Workspace 與一個已啟動的 Run；回傳 (workspace_id, task_id, run_id, Agent 憑證)。
async fn started_run(harness: &Harness, client: &RuntimeClient) -> (String, String, String, String) {
    let dir = harness.data_dir().join("app");
    std::fs::create_dir_all(&dir).unwrap();
    let workspace_id = client
        .create_workspace(&CreateWorkspaceRequest { path: dir.to_string_lossy().into_owned(), name: None })
        .await
        .unwrap()
        .id;
    // 派工需要一個可派工的帳號（票 19）
    harness.add_account("主要帳號", "main@example.com").await;
    let goal = client.create_goal(&workspace_id, "研究競品").await.unwrap();
    let created = client
        .create_task(
            &workspace_id,
            &CreateTaskRequest { goal_id: goal.id.clone(), title: "整理".into(), contract: complete() },
        )
        .await
        .unwrap();
    client.mark_task_ready(&workspace_id, &created.task.id).await.unwrap();
    let run = match client.dispatch_task(&workspace_id, &created.task.id).await.unwrap() {
        DispatchOutcome::Started(view) => *view,
        DispatchOutcome::Queued(queued) => panic!("應該直接派出，卻排隊中：{queued:?}"),
    };
    harness.supervise().await;
    let token = harness.agent_credential(&run.run.id).expect("Run 啟動時應該把 Agent 憑證放進程序的環境變數");
    (workspace_id, created.task.id, run.run.id, token)
}

/// Agent 提出一筆請求；回傳裁定請求 id。
async fn agent_asks(harness: &Harness, token: &str, question: &str) -> String {
    let body = json!({
        "question": question,
        "reason": "契約沒有指定",
        "options": [{"id":"a","label":"SQLite"},{"id":"b","label":"Postgres","tradeoff":"要另外架設"}],
        "recommendation": "a",
    });
    let (status, reply) = harness.call_tool(token, TOOL_REQUEST_DECISION, body).await;
    assert_eq!(status, StatusCode::OK, "{reply}");
    reply["decision_request_id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn app_lists_a_pending_decision_with_its_detail() {
    let harness = Harness::start().await;
    let client = app_client(&harness);
    let (workspace_id, task_id, run_id, token) = started_run(&harness, &client).await;
    let request_id = agent_asks(&harness, &token, "用哪個資料庫？").await;

    let list = client.decision_list(&workspace_id).await.unwrap();
    let detail = client.decision_detail(&workspace_id, &request_id).await.unwrap();

    assert_eq!(list.pending_count, 1);
    assert_eq!(list.decisions.len(), 1);
    assert_eq!(list.decisions[0], detail);
    assert_eq!(detail.id, request_id);
    assert_eq!(detail.kind, DecisionKind::Agent);
    assert_eq!(detail.status, DecisionStatus::Pending);
    assert_eq!(detail.question, "用哪個資料庫？");
    assert_eq!(detail.run_id, run_id);
    assert_eq!(detail.task_id, task_id);
    assert_eq!(detail.options.len(), 2);
    assert_eq!(detail.recommendation.as_deref(), Some("a"));
    assert!(detail.decision.is_none());
}

#[tokio::test]
async fn app_answers_a_decision_and_the_pending_count_drops() {
    let harness = Harness::start().await;
    let client = app_client(&harness);
    let (workspace_id, _task_id, _run_id, token) = started_run(&harness, &client).await;
    let request_id = agent_asks(&harness, &token, "用哪個資料庫？").await;

    let answered =
        client.decision_answer(&workspace_id, &request_id, "b", Some("團隊已有 Postgres".into())).await.unwrap();

    assert_eq!(answered.decision.status, DecisionStatus::Answered);
    let result = answered.decision.decision.as_ref().expect("回答後應該有裁定");
    assert_eq!(result.option_id, "b");
    assert_eq!(result.note.as_deref(), Some("團隊已有 Postgres"));
    assert_eq!(result.decided_by, "desktop");
    let list = client.decision_list(&workspace_id).await.unwrap();
    assert_eq!(list.pending_count, 0);
    assert_eq!(list.decisions[0].status, DecisionStatus::Answered);
    assert_eq!(client.pending_decisions().await.unwrap().pending_count, 0);
}

#[tokio::test]
async fn pending_decisions_counts_requests_across_the_runtime() {
    let harness = Harness::start().await;
    let client = app_client(&harness);
    let (workspace_id, _task_id, _run_id, token) = started_run(&harness, &client).await;
    assert_eq!(client.pending_decisions().await.unwrap().pending_count, 0);

    let first = agent_asks(&harness, &token, "用哪個資料庫？").await;
    agent_asks(&harness, &token, "要不要加快取？").await;
    let pending = client.pending_decisions().await.unwrap();

    assert_eq!(pending.pending_count, 2);
    assert_eq!(pending.by_workspace.len(), 1);
    assert_eq!(pending.by_workspace[0].workspace_id, workspace_id);
    assert_eq!(pending.by_workspace[0].pending_count, 2);

    client.decision_answer(&workspace_id, &first, "a", None).await.unwrap();
    assert_eq!(client.pending_decisions().await.unwrap().pending_count, 1);
}

#[tokio::test]
async fn app_is_told_why_a_decision_cannot_be_answered() {
    let harness = Harness::start().await;
    let client = app_client(&harness);
    let (workspace_id, _task_id, _run_id, token) = started_run(&harness, &client).await;
    let request_id = agent_asks(&harness, &token, "用哪個資料庫？").await;

    let unknown_option = client.decision_answer(&workspace_id, &request_id, "zzz", None).await.unwrap_err();
    let missing = client.decision_detail(&workspace_id, "decision-nope").await.unwrap_err();
    client.decision_answer(&workspace_id, &request_id, "a", None).await.unwrap();
    let conflicting = client.decision_answer(&workspace_id, &request_id, "b", None).await.unwrap_err();

    let code_of = |error: &ClientError| match error {
        ClientError::Rejected { code, .. } => code.clone(),
        other => panic!("應該回報 Runtime 的拒絕：{other:?}"),
    };
    assert_eq!(code_of(&unknown_option), "decision_option_unknown");
    assert_eq!(code_of(&missing), "decision_not_found");
    assert_eq!(code_of(&conflicting), "decision_already_answered");
}
