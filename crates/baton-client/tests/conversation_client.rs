//! 桌面殼經由 RuntimeClient 讀取對話與送出訊息（票 16）：與 App 實際使用的路徑相同（讀憑證檔、帶標頭）。

use std::sync::Arc;

use baton_api::CreateWorkspaceRequest;
use baton_client::{ClientError, RuntimeClient};
use baton_domain::conversation::{TurnKind, TurnStatus};
use baton_harness::fakes::FakeSecretProtector;
use baton_harness::Harness;

fn app_client(harness: &Harness) -> RuntimeClient {
    RuntimeClient::new(harness.addr(), harness.data_dir().to_path_buf(), Arc::new(FakeSecretProtector))
}

async fn new_workspace(harness: &Harness, client: &RuntimeClient) -> String {
    let dir = harness.data_dir().join("app");
    std::fs::create_dir_all(&dir).unwrap();
    client
        .create_workspace(&CreateWorkspaceRequest { path: dir.to_string_lossy().into_owned(), name: None })
        .await
        .unwrap()
        .id
}

#[tokio::test]
async fn app_sends_a_message_and_sees_it_pending_in_the_conversation() {
    let harness = Harness::start().await;
    let client = app_client(&harness);
    let workspace_id = new_workspace(&harness, &client).await;

    let empty = client.conversation(&workspace_id).await.unwrap();
    assert_eq!(empty.conversation_id, None);
    assert!(empty.turns.is_empty());

    let posted = client.send_conversation_message(&workspace_id, "請先整理需求").await.unwrap();
    assert!(!posted.busy);
    assert_eq!(posted.turn.kind, TurnKind::UserMessage);
    assert_eq!(posted.turn.status, TurnStatus::Pending);

    let view = client.conversation(&workspace_id).await.unwrap();
    assert!(view.conversation_id.is_some());
    assert_eq!(view.turns, vec![posted.turn]);
}

#[tokio::test]
async fn a_blank_message_is_rejected_with_its_code() {
    let harness = Harness::start().await;
    let client = app_client(&harness);
    let workspace_id = new_workspace(&harness, &client).await;
    match client.send_conversation_message(&workspace_id, "  ").await {
        Err(ClientError::Rejected { code, .. }) => assert_eq!(code, "message_empty"),
        other => panic!("空白訊息應該被拒絕：{other:?}"),
    }
}
