//! 桌面殼經由 RuntimeClient 管理 Guard 與重犯事件（票 38）：與 App 實際使用的路徑相同（讀憑證檔、帶標頭）。

use std::sync::Arc;

use baton_api::{
    CreateGuardRequest, CreateKnowledgeRequest, CreateWorkspaceRequest, GuardRevisionRequest, ReportRecurrenceRequest,
    ScopeInput,
};
use baton_client::{ClientError, RuntimeClient};
use baton_domain::guard::{GuardRule, GuardStatus, RecurrenceKind};
use baton_domain::knowledge::{KnowledgeKind, LessonDetail, ScopeKind};
use baton_domain::task::ContractFieldName;
use baton_harness::fakes::FakeSecretProtector;
use baton_harness::Harness;

fn app_client(harness: &Harness) -> RuntimeClient {
    RuntimeClient::new(harness.addr(), harness.data_dir().to_path_buf(), Arc::new(FakeSecretProtector))
}

#[tokio::test]
async fn app_turns_a_lesson_into_a_guard_toggles_it_and_reports_a_recurrence() {
    let harness = Harness::start().await;
    let client = app_client(&harness);
    let dir = harness.data_dir().join("app");
    std::fs::create_dir_all(&dir).unwrap();
    let workspace_id = client
        .create_workspace(&CreateWorkspaceRequest { path: dir.to_string_lossy().into_owned(), name: None })
        .await
        .unwrap()
        .id;
    let lesson = client
        .create_knowledge(&CreateKnowledgeRequest {
            kind: KnowledgeKind::Lesson,
            scope: ScopeInput { kind: ScopeKind::Workspace, workspace_id: Some(workspace_id.clone()), goal_id: None, task_id: None },
            title: "部署前要寫回滾計畫".into(),
            body: String::new(),
            tags: vec![],
            evidence: vec![],
            lesson: Some(LessonDetail::default()),
            valid_from: None,
            valid_until: None,
            supersedes: None,
        })
        .await
        .unwrap();

    let rule = GuardRule::Preflight { field: ContractFieldName::DoneCriteria, must_mention: "回滾".into() };
    let created = client.create_guard(&CreateGuardRequest { lesson_id: lesson.entry.id.clone(), rule }).await.unwrap();
    assert_eq!((created.guard.status, created.active), (GuardStatus::Enabled, true));
    assert_eq!(created.lesson.title, "部署前要寫回滾計畫");

    let listed = client.list_guards(Some(&workspace_id)).await.unwrap();
    assert_eq!(listed.guards.len(), 1);
    assert!(client.list_guards(None).await.unwrap().guards.is_empty(), "沒有指定 Workspace 時只列全域教訓的 Guard");

    let disabled = client.disable_guard(&created.guard.id, &GuardRevisionRequest { expected_revision: 1 }).await.unwrap();
    assert!(!disabled.active);
    let stale = client.enable_guard(&created.guard.id, &GuardRevisionRequest { expected_revision: 1 }).await;
    assert!(matches!(stale, Err(ClientError::Rejected { ref code, .. }) if code == "guard_revision_conflict"), "{stale:?}");
    client.enable_guard(&created.guard.id, &GuardRevisionRequest { expected_revision: 2 }).await.unwrap();

    let request = ReportRecurrenceRequest { note: "又忘了回滾".into(), workspace_id: None, task_id: None };
    let reported = client.report_recurrence(&lesson.entry.id, &request).await.unwrap();
    assert_eq!(reported.kind, RecurrenceKind::Reported);
    let recurrences = client.lesson_recurrences(&lesson.entry.id).await.unwrap().recurrences;
    assert_eq!(recurrences.len(), 1);
    assert_eq!(recurrences[0].lesson_id, lesson.entry.id);
}
