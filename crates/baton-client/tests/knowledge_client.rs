//! 桌面殼經由 RuntimeClient 管理知識條目（票 37）：與 App 實際使用的路徑相同（讀憑證檔、帶標頭）。

use std::sync::Arc;

use baton_api::{
    CorrectKnowledgeRequest, CreateKnowledgeRequest, CreateWorkspaceRequest, KnowledgeList, KnowledgeListQuery,
    KnowledgeReasonRequest, ScopeInput,
};
use baton_client::{ClientError, RuntimeClient};
use baton_domain::knowledge::{KnowledgeKind, KnowledgeStatus, LessonDetail, ScopeKind};
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

fn query(workspace_id: &str, q: &str, include_inactive: bool) -> KnowledgeListQuery {
    KnowledgeListQuery { workspace_id: Some(workspace_id.to_string()), q: Some(q.to_string()), include_inactive }
}

fn titles(list: &KnowledgeList) -> Vec<&str> {
    list.entries.iter().map(|e| e.entry.content.title.as_str()).collect()
}

#[tokio::test]
async fn app_creates_searches_corrects_and_revokes_a_lesson() {
    let harness = Harness::start().await;
    let client = app_client(&harness);
    let workspace_id = new_workspace(&harness, &client).await;

    let created = client
        .create_knowledge(&CreateKnowledgeRequest {
            kind: KnowledgeKind::Lesson,
            scope: ScopeInput {
                kind: ScopeKind::Workspace,
                workspace_id: Some(workspace_id.clone()),
                goal_id: None,
                task_id: None,
            },
            title: "測試前先清除快取".into(),
            body: "快取殘留會讓整合測試誤判為通過".into(),
            tags: vec!["測試".into()],
            evidence: vec![],
            lesson: Some(LessonDetail {
                trigger: "第二次執行才通過".into(),
                root_cause: String::new(),
                resolution: "清除快取目錄".into(),
            }),
            valid_from: None,
            valid_until: None,
            supersedes: None,
        })
        .await
        .unwrap();
    assert_eq!((created.entry.status, created.effective), (KnowledgeStatus::Verified, true));
    let id = created.entry.id.clone();

    // 中文兩字搜尋：查詢字串必須正確 URL 編碼
    let found = client.list_knowledge(&query(&workspace_id, "快取", false)).await.unwrap();
    assert_eq!(titles(&found), ["測試前先清除快取"]);

    let corrected = client
        .correct_knowledge(
            &id,
            &CorrectKnowledgeRequest {
                expected_revision: created.entry.revision,
                title: "測試前先清除建置快取".into(),
                body: "快取殘留會讓整合測試誤判為通過".into(),
                tags: vec!["測試".into()],
                evidence: vec![],
                lesson: Some(LessonDetail {
                    trigger: "第二次執行才通過".into(),
                    root_cause: "建置快取沒有失效".into(),
                    resolution: "清除快取目錄".into(),
                }),
                valid_until: None,
                note: Some("補上根因".into()),
            },
        )
        .await
        .unwrap();
    assert_eq!(corrected.entry.version, 2);

    let revoked = client
        .revoke_knowledge(
            &id,
            &KnowledgeReasonRequest { expected_revision: corrected.entry.revision, reason: "已改用乾淨的建置環境".into() },
        )
        .await
        .unwrap();
    assert_eq!((revoked.entry.status, revoked.effective), (KnowledgeStatus::Revoked, false));

    // 撤銷後預設列表搜不到；include_inactive 才看得到
    assert!(client.list_knowledge(&query(&workspace_id, "快取", false)).await.unwrap().entries.is_empty());
    let all = client.list_knowledge(&query(&workspace_id, "快取", true)).await.unwrap();
    assert_eq!(titles(&all), ["測試前先清除建置快取"]);

    let detail = client.knowledge_detail(&id).await.unwrap();
    assert_eq!(detail.entry.entry.status, KnowledgeStatus::Revoked);
    let versions: Vec<_> = detail.versions.iter().map(|v| (v.version, v.content.title.as_str())).collect();
    assert_eq!(versions, [(1, "測試前先清除快取"), (2, "測試前先清除建置快取")]);
}

#[tokio::test]
async fn app_gets_the_runtime_code_when_the_query_is_invalid() {
    let harness = Harness::start().await;
    let client = app_client(&harness);
    let workspace_id = new_workspace(&harness, &client).await;

    let error = client.list_knowledge(&query(&workspace_id, "快", false)).await.unwrap_err();
    assert!(matches!(&error, ClientError::Rejected { code, .. } if code == "knowledge_query_invalid"), "{error:?}");
}
