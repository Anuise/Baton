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
