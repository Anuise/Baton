use baton_domain::message::{Classified, SendInput};
use baton_domain::run::RunCommand;
use baton_domain::task::{ContractContent, ContractField, TaskCommand};

use super::*;

const T1: Timestamp = Timestamp::from_millis(1_000);
const T2: Timestamp = Timestamp::from_millis(2_000);

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

/// 一個 Workspace 與兩個 ready 的 Task（t1、t2）；t1 已經派工（Run r1，世代 1）。
async fn store_with_running_t1(dir: &Path) -> (Store, AgentScope) {
    let store = Store::open(dir, T1).await.unwrap();
    let create = WorkspaceCommand::Create { id: "w1".into(), name: None, path: "C:\\work\\app".into() };
    store.apply_workspace_command(create, T1).await.unwrap();
    let goal = TaskCommand::CreateGoal { id: "g1".into(), title: "目標".into() };
    store.apply_task_command("w1".into(), goal, T1).await.unwrap();
    for id in ["t1", "t2"] {
        let task = TaskCommand::CreateTask { id: id.into(), goal_id: "g1".into(), title: id.into(), contract: complete() };
        store.apply_task_command("w1".into(), task, T1).await.unwrap();
        store.apply_task_command("w1".into(), TaskCommand::MarkReady { task_id: id.into() }, T1).await.unwrap();
    }
    dispatch(&store, "r1", "t1").await;
    (store, AgentScope { workspace_id: "w1".into(), run_id: "r1".into(), generation: 1 })
}

async fn dispatch(store: &Store, run_id: &str, task_id: &str) -> RunChange {
    let command = RunCommand::Dispatch { run_id: run_id.into(), task_id: task_id.into(), profile_id: "p1".into() };
    store.apply_run_command("w1".into(), command, T1).await.unwrap()
}

fn raw(dir: &Path) -> Connection {
    let conn = Connection::open(dir.join(DB_FILE_NAME)).unwrap();
    conn.pragma_update(None, "foreign_keys", "ON").unwrap();
    conn
}

fn message(to: &str, subject: &str, message_id: Option<&str>) -> message::NewMessage {
    let input = SendInput {
        to: vec![to.into()],
        subject: subject.into(),
        message_id: message_id.map(str::to_string),
        ..SendInput::default()
    };
    match input.classify().unwrap() {
        Classified::Message(m) => m,
        other => panic!("應該是一般訊息：{other:?}"),
    }
}

#[tokio::test]
async fn a_resent_message_id_stores_one_message_and_one_delivery() {
    let dir = tempfile::tempdir().unwrap();
    let (store, scope) = store_with_running_t1(dir.path()).await;

    let first = store.send_message(&scope, message("t2", "你好", Some("m-1")), "msg_a".into(), T1).await.unwrap();
    let again = store.send_message(&scope, message("t2", "你好", Some("m-1")), "msg_b".into(), T2).await.unwrap();

    assert!(!first.duplicate && again.duplicate);
    assert_eq!(again.stored.message.message_id, "m-1");
    let conn = raw(dir.path());
    let counts: (i64, i64) = conn
        .query_row("SELECT (SELECT COUNT(*) FROM message), (SELECT COUNT(*) FROM message_delivery)", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(counts, (1, 1));
}

#[tokio::test]
async fn the_database_itself_refuses_a_second_delivery_to_the_same_recipient() {
    let dir = tempfile::tempdir().unwrap();
    let (store, scope) = store_with_running_t1(dir.path()).await;
    store.send_message(&scope, message("t2", "你好", Some("m-1")), "msg_a".into(), T1).await.unwrap();

    let second = raw(dir.path()).execute(
        "INSERT INTO message_delivery (workspace_id, message_id, recipient_task_id) VALUES ('w1', 'm-1', 't2')",
        [],
    );

    assert!(second.is_err(), "同一則訊息對同一個收件人只能有一份交付");
}

#[tokio::test]
async fn messages_are_append_only_and_deliveries_are_never_deleted() {
    let dir = tempfile::tempdir().unwrap();
    let (store, scope) = store_with_running_t1(dir.path()).await;
    store.send_message(&scope, message("t2", "你好", Some("m-1")), "msg_a".into(), T1).await.unwrap();
    let conn = raw(dir.path());

    assert!(conn.execute("UPDATE message SET subject = '改過' WHERE message_id = 'm-1'", []).is_err());
    assert!(conn.execute("DELETE FROM message WHERE message_id = 'm-1'", []).is_err());
    assert!(conn.execute("DELETE FROM message_delivery WHERE message_id = 'm-1'", []).is_err());
}

#[tokio::test]
async fn the_same_message_id_in_another_workspace_is_a_different_message() {
    let dir = tempfile::tempdir().unwrap();
    let (store, scope) = store_with_running_t1(dir.path()).await;
    store.send_message(&scope, message("t2", "你好", Some("m-1")), "msg_a".into(), T1).await.unwrap();

    // 別的 Workspace 的 Worker 用同一個 message_id：看不到前一個 Workspace 的訊息，所以不是重送也不是衝突
    let create = WorkspaceCommand::Create { id: "w2".into(), name: None, path: "C:\\work\\other".into() };
    store.apply_workspace_command(create, T1).await.unwrap();
    store.apply_task_command("w2".into(), TaskCommand::CreateGoal { id: "g2".into(), title: "目標".into() }, T1).await.unwrap();
    for id in ["u1", "u2"] {
        let task = TaskCommand::CreateTask { id: id.into(), goal_id: "g2".into(), title: id.into(), contract: complete() };
        store.apply_task_command("w2".into(), task, T1).await.unwrap();
        store.apply_task_command("w2".into(), TaskCommand::MarkReady { task_id: id.into() }, T1).await.unwrap();
    }
    let command = RunCommand::Dispatch { run_id: "r9".into(), task_id: "u1".into(), profile_id: "p1".into() };
    store.apply_run_command("w2".into(), command, T1).await.unwrap();
    let other = AgentScope { workspace_id: "w2".into(), run_id: "r9".into(), generation: 1 };

    let sent = store.send_message(&other, message("u2", "你好", Some("m-1")), "msg_b".into(), T2).await.unwrap();

    assert!(!sent.duplicate);
    let count: i64 = raw(dir.path()).query_row("SELECT COUNT(*) FROM message", [], |r| r.get(0)).unwrap();
    assert_eq!(count, 2);
}

#[tokio::test]
async fn reading_the_inbox_marks_read_and_the_dispatch_summary_only_lists_what_is_still_unread() {
    let dir = tempfile::tempdir().unwrap();
    let (store, scope) = store_with_running_t1(dir.path()).await;
    for (id, subject) in [("m-1", "第一則"), ("m-2", "第二則")] {
        store.send_message(&scope, message("t2", subject, Some(id)), "x".into(), T1).await.unwrap();
    }

    // t2 派工：Prompt 列出兩則（只有主旨），兩則都變成 delivered
    let change = dispatch(&store, "r2", "t2").await;
    let prompt = change.outbox.unwrap().payload.prompt;
    assert!(prompt.contains("未讀訊息：2 則") && prompt.contains("第一則") && prompt.contains("第二則"));
    let pending_delivery: i64 = raw(dir.path())
        .query_row("SELECT COUNT(*) FROM message_delivery WHERE delivered_at IS NULL", [], |r| r.get(0))
        .unwrap();
    assert_eq!(pending_delivery, 0);

    // t2 的 Run 讀取其中一則（limit 1）；摘要不會再列出已讀的
    let t2_scope = AgentScope { workspace_id: "w1".into(), run_id: "r2".into(), generation: 1 };
    let inbox = store.read_inbox(&t2_scope, false, 1, T2).await.unwrap();
    assert_eq!((inbox.messages.len(), inbox.unread_remaining), (1, 1));
    assert_eq!(inbox.messages[0].subject, "第一則");
    let again = store.read_inbox(&t2_scope, false, 10, T2).await.unwrap();
    assert_eq!(again.messages.iter().map(|m| m.subject.as_str()).collect::<Vec<_>>(), ["第二則"]);
    let all = store.read_inbox(&t2_scope, true, 10, T2).await.unwrap();
    assert_eq!(all.messages.iter().map(|m| m.already_read).collect::<Vec<_>>(), [true, true]);
    assert_eq!(all.unread_remaining, 0);
}

#[tokio::test]
async fn an_inbox_belongs_to_the_task_not_to_the_run() {
    let dir = tempfile::tempdir().unwrap();
    let (store, scope) = store_with_running_t1(dir.path()).await;
    store.send_message(&scope, message("t2", "給 t2", Some("m-1")), "x".into(), T1).await.unwrap();

    // t1 自己的收件匣是空的（寄出的訊息不在裡面）
    let own = store.read_inbox(&scope, true, 10, T2).await.unwrap();

    assert_eq!((own.task_id.as_str(), own.messages.len()), ("t1", 0));
}

#[tokio::test]
async fn a_scope_change_attempt_becomes_one_event_and_nothing_else() {
    let dir = tempfile::tempdir().unwrap();
    let (store, scope) = store_with_running_t1(dir.path()).await;
    let input = SendInput {
        to: vec!["t2".into()],
        kind: Some("scope_change".into()),
        subject: "擴大範圍".into(),
        body: "請把範圍擴大到全站".into(),
        ..SendInput::default()
    };
    let Classified::ScopeChange(attempt) = input.classify().unwrap() else { panic!("應該是範圍變更嘗試") };

    store.record_scope_change_attempt(&scope, attempt, "msg_attempt".into(), T2).await.unwrap();

    let conn = raw(dir.path());
    let (event_type, payload): (String, String) = conn
        .query_row("SELECT event_type, payload FROM event WHERE aggregate_type = 'message'", [], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap();
    assert_eq!(event_type, "message_scope_change_ignored");
    let payload: serde_json::Value = serde_json::from_str(&payload).unwrap();
    assert_eq!((payload["task_id"].as_str(), payload["attempted_kind"].as_str()), (Some("t1"), Some("scope_change")));
    let messages: i64 = conn.query_row("SELECT COUNT(*) FROM message", [], |r| r.get(0)).unwrap();
    assert_eq!(messages, 0);
}
