//! 搜尋實作的細節：全文檢索（FTS5 + trigram）與退回 LIKE 必須給出相同結果，索引可以從條目重建。
//! 情境層級的行為（候選、撤銷、Agent 工具）在 `baton-harness/tests/knowledge.rs`。

use super::*;

const T1: Timestamp = Timestamp::from_millis(1_000);

async fn open(dir: &Path, full_text_search: bool) -> Store {
    Store::open_with(dir, T1, StoreOptions { full_text_search }).await.unwrap()
}

fn global() -> ScopeRequest {
    ScopeRequest { kind: ScopeKind::Global, workspace_id: None, goal_id: None, task_id: None }
}

async fn add(store: &Store, id: &str, kind: KnowledgeKind, title: &str, body: &str) {
    let content = KnowledgeContent::new(kind, title, body, &[], &[], None).unwrap();
    let request = NewKnowledgeRequest {
        id: id.into(),
        kind,
        scope: global(),
        content,
        valid_from: None,
        valid_until: None,
        supersedes: None,
    };
    store.create_knowledge(request, T1).await.unwrap();
}

async fn seed(store: &Store) {
    add(store, "d1", KnowledgeKind::Decision, "資料庫採用 SQLite", "控制資料庫使用單一寫入佇列").await;
    add(store, "l1", KnowledgeKind::Lesson, "測試前先清除快取", "快取殘留會讓整合測試誤判").await;
    add(store, "f1", KnowledgeKind::Fact, "100% 覆蓋_率 \"引號\"", "特殊字元").await;
}

async fn ids(store: &Store, query: &str) -> Vec<String> {
    let terms = knowledge::parse_query(query).unwrap();
    let audience = KnowledgeAudience::Desktop { workspace_id: None };
    let mut found: Vec<String> =
        store.knowledge_search(audience, terms, false, T1).await.unwrap().into_iter().map(|e| e.id).collect();
    found.sort();
    found
}

const QUERIES: [(&str, &[&str]); 9] = [
    ("快取", &["l1"]),
    ("資料庫", &["d1"]),
    ("寫入佇列", &["d1"]),
    ("sqlite", &["d1"]),
    ("測試 快取", &["l1"]),
    ("測試 資料庫", &[]),
    ("100%", &["f1"]),
    ("覆蓋_率", &["f1"]),
    ("\"引號\"", &["f1"]),
];

#[tokio::test]
async fn full_text_and_like_fallback_return_the_same_results() {
    let fts_dir = tempfile::tempdir().unwrap();
    let like_dir = tempfile::tempdir().unwrap();
    let fts = open(fts_dir.path(), true).await;
    let like = open(like_dir.path(), false).await;
    assert_eq!(fts.search_mode(), SearchMode::FullText);
    assert_eq!(like.search_mode(), SearchMode::LikeFallback);
    seed(&fts).await;
    seed(&like).await;

    for (query, expected) in QUERIES {
        assert_eq!(ids(&fts, query).await, expected, "全文檢索：{query}");
        assert_eq!(ids(&like, query).await, expected, "LIKE：{query}");
    }
    // `%`、`_` 不是萬用字元
    assert_eq!(ids(&fts, "0%覆").await, Vec::<String>::new());
    assert_eq!(ids(&like, "率_").await, Vec::<String>::new());
}

#[tokio::test]
async fn index_is_rebuilt_on_open_so_entries_written_while_degraded_become_searchable() {
    let dir = tempfile::tempdir().unwrap();
    let degraded = open(dir.path(), false).await;
    seed(&degraded).await;
    drop(degraded);

    let restored = open(dir.path(), true).await;
    assert_eq!(restored.search_mode(), SearchMode::FullText);
    for (query, expected) in QUERIES {
        assert_eq!(ids(&restored, query).await, expected, "{query}");
    }
}

#[tokio::test]
async fn corrections_replace_the_indexed_text() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path(), true).await;
    seed(&store).await;
    let content = KnowledgeContent::new(KnowledgeKind::Lesson, "部署前檢查憑證", "", &[], &[], None).unwrap();
    let command = KnowledgeCommand::Correct { content, valid_until: None, note: None };
    store.apply_knowledge_command("l1", 1, command, T1).await.unwrap();

    assert_eq!(ids(&store, "清除快取").await, Vec::<String>::new(), "舊版本的文字不再被索引");
    assert_eq!(ids(&store, "檢查憑證").await, ["l1"]);
}

#[tokio::test]
async fn versions_are_append_only_and_entries_cannot_be_deleted() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path(), true).await;
    seed(&store).await;
    drop(store);
    let conn = Connection::open(dir.path().join(DB_FILE_NAME)).unwrap();
    assert!(conn.execute("UPDATE knowledge_entry_version SET note = 'x'", []).is_err());
    assert!(conn.execute("DELETE FROM knowledge_entry_version", []).is_err());
    assert!(conn.execute("DELETE FROM knowledge_entry", []).is_err());
}

// --- Controller 提交候選教訓（票 54，spec D-05） ---

fn complete_contract() -> baton_domain::task::ContractContent {
    use baton_domain::task::{ContractContent, ContractField};
    let filled = |text: &str| Some(ContractField::Filled { text: text.into(), references: vec![] });
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

/// 兩個 Workspace（w1、w2），各有一個 Goal（g1、g2）與一個 Task（t1、t2）。
async fn two_workspaces(dir: &Path) -> Store {
    use baton_domain::task::TaskCommand;
    let store = open(dir, true).await;
    for (ws, goal, task) in [("w1", "g1", "t1"), ("w2", "g2", "t2")] {
        let path = format!("C:\\work\\{ws}");
        store.apply_workspace_command(WorkspaceCommand::Create { id: ws.into(), name: None, path }, T1).await.unwrap();
        let create_goal = TaskCommand::CreateGoal { id: goal.into(), title: "目標".into() };
        store.apply_task_command(ws.into(), create_goal, T1).await.unwrap();
        let create_task = TaskCommand::CreateTask {
            id: task.into(),
            goal_id: goal.into(),
            title: task.into(),
            kind: Default::default(),
            contract: complete_contract(),
        };
        store.apply_task_command(ws.into(), create_task, T1).await.unwrap();
    }
    store
}

fn lesson(title: &str) -> KnowledgeContent {
    KnowledgeContent::new(KnowledgeKind::Lesson, title, "內文", &[], &[], None).unwrap()
}

fn controller_of(ws: &str) -> ControllerScope {
    ControllerScope { workspace_id: ws.into(), controller_run_id: "ctl_1".into() }
}

#[tokio::test]
async fn a_controller_lesson_is_a_candidate_scoped_by_the_named_task_of_its_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let store = two_workspaces(dir.path()).await;
    let controller = controller_of("w1");

    let expected = [
        (ScopeKind::Task, Some("g1"), Some("t1")),
        (ScopeKind::Goal, Some("g1"), None),
        (ScopeKind::Workspace, None, None),
    ];
    for (i, (kind, goal, task)) in expected.into_iter().enumerate() {
        let id = format!("c{i}");
        let entry =
            store.record_controller_lesson_candidate(&controller, id.clone(), "t1", kind, lesson("教訓"), T1).await.unwrap();
        assert_eq!(entry.id, id);
        assert_eq!(entry.status, KnowledgeStatus::Candidate);
        assert_eq!(
            entry.scope,
            KnowledgeScope {
                kind,
                workspace_id: Some("w1".into()),
                goal_id: goal.map(str::to_string),
                task_id: task.map(str::to_string),
            }
        );
        assert_eq!(entry.source, KnowledgeSource::Controller { controller_run_id: "ctl_1".into(), task_id: "t1".into() });
        // 寫入後讀回的內容一致
        assert_eq!(store.knowledge_detail(&id).await.unwrap().0, entry);
    }
}

#[tokio::test]
async fn a_controller_cannot_attach_a_lesson_to_another_workspace_or_to_the_global_scope() {
    let dir = tempfile::tempdir().unwrap();
    let store = two_workspaces(dir.path()).await;
    let controller = controller_of("w1");

    // 別的 Workspace 的 Task、不存在的 Task：找不到，什麼都不寫
    for task in ["t2", "t_missing"] {
        let error = store
            .record_controller_lesson_candidate(&controller, "x".into(), task, ScopeKind::Task, lesson("教訓"), T1)
            .await
            .unwrap_err();
        assert!(matches!(&error, KnowledgeApplyError::Run(RunError::TaskNotFound(id)) if id == task), "{error:?}");
    }
    // 全域範圍一律拒絕
    let error = store
        .record_controller_lesson_candidate(&controller, "x".into(), "t1", ScopeKind::Global, lesson("教訓"), T1)
        .await
        .unwrap_err();
    assert!(matches!(error, KnowledgeApplyError::Rejected(KnowledgeError::ScopeInvalid(_))), "{error:?}");

    for ws in ["w1", "w2"] {
        let audience = KnowledgeAudience::Desktop { workspace_id: Some(ws.into()) };
        assert!(store.knowledge_search(audience, vec![], true, T1).await.unwrap().is_empty(), "{ws} 不應該有條目");
    }
}
