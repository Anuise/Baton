use baton_domain::profile::{AuthReport, AuthSource, ProfileCommand};
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

/// 一個 Workspace，以及一批 ready 的 Task。
async fn store_with_ready_tasks(dir: &Path, task_ids: &[&str]) -> Store {
    let store = Store::open(dir, T1).await.unwrap();
    let create = WorkspaceCommand::Create { id: "w1".into(), name: None, path: "C:\\work\\app".into() };
    store.apply_workspace_command(create, T1).await.unwrap();
    let goal = TaskCommand::CreateGoal { id: "g1".into(), title: "目標".into() };
    store.apply_task_command("w1".into(), goal, T1).await.unwrap();
    for id in task_ids {
        let task = TaskCommand::CreateTask { id: (*id).into(), goal_id: "g1".into(), title: (*id).into(), kind: Default::default(), contract: complete() };
        store.apply_task_command("w1".into(), task, T1).await.unwrap();
        store.apply_task_command("w1".into(), TaskCommand::MarkReady { task_id: (*id).into() }, T1).await.unwrap();
        crate::workdirs::register_ready(&store, "w1", id, T1).await;
    }
    store
}

/// 建立一個核對通過、可派工的帳號 Profile。
async fn add_profile(store: &Store, id: &str) {
    let create = ProfileCommand::Create {
        id: id.into(),
        name: id.into(),
        config_dir: format!("C:\\baton\\profiles\\{id}"),
        expected_account: None,
    };
    store.apply_profile_command(create, T1).await.unwrap();
    let report = AuthReport {
        logged_in: true,
        source: AuthSource::NativeLogin,
        source_detail: None,
        account: Some(format!("{id}@example.com")),
        config_dir: Some(format!("C:\\baton\\profiles\\{id}")),
    };
    store.apply_profile_command(ProfileCommand::RecordVerification { id: id.into(), report }, T1).await.unwrap();
}

fn request(task_id: &str, priority: i32) -> Option<ScheduleRequest> {
    Some(ScheduleRequest { workspace_id: "w1".into(), task_id: task_id.into(), priority })
}

/// 依序產生 run_1、run_2…
fn run_ids() -> impl FnMut() -> Option<String> + Send + 'static {
    let mut next = 0;
    move || {
        next += 1;
        Some(format!("run_{next}"))
    }
}

fn raw(dir: &Path) -> Connection {
    Connection::open(dir.join(DB_FILE_NAME)).unwrap()
}

fn queued_task_ids(dir: &Path) -> Vec<String> {
    raw(dir)
        .prepare("SELECT task_id FROM dispatch_request ORDER BY priority DESC, seq")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

#[tokio::test]
async fn each_dispatch_goes_to_the_least_loaded_account_and_the_association_is_persisted() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_with_ready_tasks(dir.path(), &["t1", "t2", "t3"]).await;
    add_profile(&store, "p1").await;
    add_profile(&store, "p2").await;
    // 三次排程共用同一個 id 產生器，Run 識別才不會重複
    let mut next = 0;
    let mut started = Vec::new();
    for task in ["t1", "t2", "t3"] {
        let first = next;
        let outcome = store
            .schedule(request(task, 0), HashMap::new(), T2, move || Some(format!("run_{}", first + 1)))
            .await
            .unwrap();
        next += 1;
        assert!(outcome.waiting.is_empty(), "{outcome:?}");
        started.extend(outcome.started.into_iter().map(|s| (s.task_id, s.profile_id)));
    }
    drop(store);

    let reopened = Store::open(dir.path(), T2).await.unwrap();
    let board = reopened.task_board("w1").await.unwrap();
    let by_task: Vec<(String, String)> =
        board.runs.iter().map(|v| (v.run.task_id.clone(), v.run.profile_id.clone())).collect();
    assert_eq!(
        started,
        vec![("t1".into(), "p1".into()), ("t2".into(), "p2".into()), ("t3".into(), "p1".into())]
    );
    assert_eq!(by_task, started, "重新開啟後 Run 與帳號的關聯還在");
    assert!(queued_task_ids(dir.path()).is_empty(), "派出的 Task 不留在佇列");
}

#[tokio::test]
async fn a_request_that_cannot_ever_be_dispatched_is_rejected_and_nothing_is_written() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_with_ready_tasks(dir.path(), &["t1"]).await;
    add_profile(&store, "p1").await;
    let task = TaskCommand::CreateTask {
        id: "draft".into(),
        goal_id: "g1".into(),
        title: "草稿".into(),
        kind: Default::default(),
        contract: ContractContent::default(),
    };
    store.apply_task_command("w1".into(), task, T1).await.unwrap();
    let events_before: i64 = raw(dir.path()).query_row("SELECT COUNT(*) FROM event", [], |r| r.get(0)).unwrap();

    let not_ready = store.schedule(request("draft", 0), HashMap::new(), T2, run_ids()).await;
    let missing = store.schedule(request("nope", 0), HashMap::new(), T2, run_ids()).await;

    assert!(matches!(not_ready, Err(RunApplyError::Rejected(RunError::TaskNotReady(_)))), "{not_ready:?}");
    assert!(matches!(missing, Err(RunApplyError::Rejected(RunError::TaskNotFound(_)))), "{missing:?}");
    assert!(queued_task_ids(dir.path()).is_empty());
    let events_after: i64 = raw(dir.path()).query_row("SELECT COUNT(*) FROM event", [], |r| r.get(0)).unwrap();
    assert_eq!(events_after, events_before);
}

#[tokio::test]
async fn waiting_requests_queue_by_priority_and_leave_the_queue_when_the_task_is_cancelled() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_with_ready_tasks(dir.path(), &["t1", "t2"]).await;

    // 還沒有任何帳號：兩個要求都在等
    let first = store.schedule(request("t1", 0), HashMap::new(), T2, run_ids()).await.unwrap();
    let second = store.schedule(request("t2", 5), HashMap::new(), T2, run_ids()).await.unwrap();
    // 重複要求同一個 Task 不會多一筆
    store.schedule(request("t1", 0), HashMap::new(), T2, run_ids()).await.unwrap();

    assert!(first.started.is_empty() && second.started.is_empty());
    assert_eq!(second.waiting.len(), 2);
    assert!(second.waiting.iter().all(|w| w.reasons.iter().all(|r| r.code == "no_account")));
    assert_eq!(queued_task_ids(dir.path()), vec!["t2".to_string(), "t1".to_string()], "優先序高者在前");

    store.apply_task_command("w1".into(), TaskCommand::Cancel { task_id: "t2".into() }, T2).await.unwrap();

    assert_eq!(queued_task_ids(dir.path()), vec!["t1".to_string()]);
    let snapshot = store.scheduling_snapshot(HashMap::new()).await.unwrap();
    assert_eq!(snapshot.queue.len(), 1);
    assert_eq!(snapshot.queue[0].0.task_id, "t1");
}

#[tokio::test]
async fn waiting_reasons_are_recomputed_from_the_current_state_instead_of_stored() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_with_ready_tasks(dir.path(), &["t1"]).await;
    store.schedule(request("t1", 0), HashMap::new(), T2, run_ids()).await.unwrap();
    let before = store.scheduling_snapshot(HashMap::new()).await.unwrap();

    add_profile(&store, "p1").await;
    let after = store.scheduling_snapshot(HashMap::new()).await.unwrap();

    assert_eq!(before.queue[0].1[0].code, "no_account");
    assert!(after.queue[0].1.is_empty(), "有了可用帳號，原因就消失（等排程處理）");
    // 之後的一次排程把它派出
    let outcome = store.schedule(None, HashMap::new(), T2, run_ids()).await.unwrap();
    assert_eq!(outcome.started.len(), 1);
    assert_eq!(outcome.started[0].profile_id, "p1");
}

#[tokio::test]
async fn capacity_mode_is_persisted_and_switching_to_the_same_mode_adds_no_event() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path(), T1).await.unwrap();
    let default = store.scheduler_settings().await.unwrap();

    let changed = store.set_capacity_mode(CapacityMode::HigherConcurrency, T2).await.unwrap();
    let again = store.set_capacity_mode(CapacityMode::HigherConcurrency, T2).await.unwrap();
    drop(store);
    let reopened = Store::open(dir.path(), T2).await.unwrap();
    let persisted = reopened.scheduler_settings().await.unwrap();

    assert_eq!((default.mode, default.version), (CapacityMode::ReserveHandoff, 0));
    assert_eq!((changed.mode, changed.version), (CapacityMode::HigherConcurrency, 1));
    assert_eq!(again, changed);
    assert_eq!(persisted.mode, CapacityMode::HigherConcurrency);
    let events: i64 = raw(dir.path())
        .query_row("SELECT COUNT(*) FROM event WHERE aggregate_type = 'scheduler_settings'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(events, 1);
}
