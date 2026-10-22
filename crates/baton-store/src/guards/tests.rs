//! 儲存層的細節：Guard 不可刪除、重犯事件是追加式。
//! 情境層級的行為（准入時擋下、UI 看到的原因、成效）在 `baton-harness/tests/guards.rs`。

use baton_domain::knowledge::{KnowledgeContent, KnowledgeKind, ScopeKind};
use baton_domain::task::ContractFieldName;

use super::*;
use crate::{NewKnowledgeRequest, ScopeRequest};

const T1: Timestamp = Timestamp::from_millis(1_000);

async fn store_with_guard(dir: &Path) -> Store {
    let store = Store::open(dir, T1).await.unwrap();
    let content = KnowledgeContent::new(KnowledgeKind::Lesson, "部署要先備份", "", &[], &[], None).unwrap();
    let request = NewKnowledgeRequest {
        id: "k1".into(),
        kind: KnowledgeKind::Lesson,
        scope: ScopeRequest { kind: ScopeKind::Global, workspace_id: None, goal_id: None, task_id: None },
        content,
        valid_from: None,
        valid_until: None,
        supersedes: None,
    };
    store.create_knowledge(request, T1).await.unwrap();
    let rule = GuardRule::Preflight { field: ContractFieldName::Scope, must_mention: "備份".into() };
    store.create_guard("g1".into(), "k1", rule, T1).await.unwrap();
    store
}

#[tokio::test]
async fn guards_cannot_be_deleted_and_recurrences_are_append_only() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_with_guard(dir.path()).await;
    drop(store);
    let conn = Connection::open(dir.path().join(DB_FILE_NAME)).unwrap();
    conn.execute_batch("PRAGMA foreign_keys = OFF;").unwrap();
    assert!(conn.execute("DELETE FROM guard", []).is_err());
    conn.execute(
        "INSERT INTO lesson_recurrence (id, lesson_id, kind, message, occurred_at) VALUES ('r1', 'k1', 'reported', 'x', 1)",
        [],
    )
    .unwrap();
    assert!(conn.execute("UPDATE lesson_recurrence SET message = 'y'", []).is_err());
    assert!(conn.execute("DELETE FROM lesson_recurrence", []).is_err());
}
