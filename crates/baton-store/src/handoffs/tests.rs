//! 交接各 phase 交易的持久化測試：名額計算與取消意圖（R151 第 5、6 項）。

use baton_domain::profile::{AuthReport, AuthSource, ProfileCommand};
use baton_domain::run::ProcessRecord;
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

/// 一個 Workspace、一批 ready 的 Task，以及一批核對通過的帳號。
async fn store_with(dir: &Path, task_ids: &[&str], profile_ids: &[&str]) -> Store {
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
    for id in profile_ids {
        let create = ProfileCommand::Create {
            id: (*id).into(),
            name: (*id).into(),
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
        let verify = ProfileCommand::RecordVerification { id: (*id).into(), report, cli_version: Some("2.1.307 (Claude Code)".into()) };
        store.apply_profile_command(verify, T1).await.unwrap();
    }
    store
}

/// 在指定帳號上把 Task 的 Run 推進到 running。
async fn running_on(store: &Store, run_id: &str, task_id: &str, profile_id: &str) {
    let dispatch = RunCommand::Dispatch { run_id: run_id.into(), task_id: task_id.into(), profile_id: profile_id.into() };
    store.apply_run_command("w1".into(), dispatch, T1).await.unwrap();
    let begin = RunCommand::BeginLaunch {
        run_id: run_id.into(),
        command_id: format!("cmd-{run_id}"),
        working_dir: "C:\\work\\app".into(),
        resume_check: None,
    };
    store.apply_run_command("w1".into(), begin, T1).await.unwrap();
    let process = ProcessRecord { pid: 4242, created_at_millis: 99, command_hash: "h".into() };
    store.apply_run_command("w1".into(), RunCommand::RecordReceipt { run_id: run_id.into(), process }, T1).await.unwrap();
}

fn raw(dir: &Path) -> Connection {
    Connection::open(dir.join(DB_FILE_NAME)).unwrap()
}

/// 讓帳號進入 draining（用量 90%）。
fn drain(dir: &Path, profile_id: &str) {
    raw(dir)
        .execute(
            "INSERT INTO quota_gate (profile_id, state, reason, used, reset_at, since, version)
             VALUES (?1, 'draining', 'drain_threshold', 0.9, NULL, ?2, 1)",
            rusqlite::params![profile_id, T1.as_millis()],
        )
        .unwrap();
}

fn ids(prefix: &'static str) -> impl FnMut() -> Option<String> + Send + 'static {
    let mut next = 0;
    move || {
        next += 1;
        Some(format!("{prefix}_{next}"))
    }
}

fn queued_task_ids(dir: &Path) -> Vec<String> {
    raw(dir)
        .prepare("SELECT task_id FROM dispatch_request ORDER BY seq")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// 安全邊界的等待上限之後。
fn after_checkpoint_wait() -> Timestamp {
    T2.plus_millis(handoff::CHECKPOINT_WAIT_MS + 1)
}

// ---------------------------------------------------------------- 名額（R151 第 6 項）

#[tokio::test]
async fn a_handoff_is_requested_even_when_the_global_capacity_is_full_because_the_old_run_will_end() {
    let dir = tempfile::tempdir().unwrap();
    let tasks = ["t1", "t2", "t3", "t4", "t5", "t6"];
    let store = store_with(dir.path(), &tasks, &["p1", "p2", "p3", "p4"]).await;
    // 全域 6 個名額全滿：p1、p2、p3 各 2 個，p4 空著
    for (i, task) in tasks.iter().enumerate() {
        running_on(&store, &format!("r{}", i + 1), task, ["p1", "p2", "p3"][i / 2]).await;
    }
    drain(dir.path(), "p1");

    let created = store.request_handoffs(HashMap::new(), T2, ids("hof")).await.unwrap();

    assert_eq!(created.len(), 2, "舊 Run 會在轉移時結束，不佔名額：{created:?}");
    assert!(created.iter().all(|h| h.to_profile_id == "p4"));
}

#[tokio::test]
async fn a_requested_handoff_moves_to_stopping_even_when_the_global_capacity_filled_up_meanwhile() {
    let dir = tempfile::tempdir().unwrap();
    let tasks = ["t1", "t2", "t3", "t4", "t5", "t6"];
    let store = store_with(dir.path(), &tasks, &["p1", "p2", "p3", "p4"]).await;
    for (i, task) in tasks.iter().take(5).enumerate() {
        running_on(&store, &format!("r{}", i + 1), task, ["p1", "p2", "p3"][i / 2]).await;
    }
    drain(dir.path(), "p1");
    let created = store.request_handoffs(HashMap::new(), T2, ids("hof")).await.unwrap();
    let first = created.iter().find(|h| h.from_run_id == "r1").expect("r1 的交接").clone();
    // 交接要求之後，第 6 個名額被別的 Run 用掉
    running_on(&store, "r6", "t6", "p3").await;

    let next = store.begin_handoff_stop(&first.id, HashMap::new(), after_checkpoint_wait()).await.unwrap();

    assert_eq!(next.map(|h| h.phase), Some(HandoffPhase::Stopping));
}

// ---------------------------------------------------------------- 取消意圖（R151 第 5 項）

/// p1 上 t1 的 Run（r1）在 draining 中已建立交接（承接帳號 p2），之後使用者要求取消 r1。
async fn handoff_then_cancel(dir: &Path) -> (Store, Handoff) {
    let store = store_with(dir, &["t1"], &["p1", "p2"]).await;
    running_on(&store, "r1", "t1", "p1").await;
    drain(dir, "p1");
    let created = store.request_handoffs(HashMap::new(), T2, ids("hof")).await.unwrap();
    assert_eq!(created.len(), 1);
    store.apply_run_command("w1".into(), RunCommand::RequestCancel { run_id: "r1".into() }, T2).await.unwrap();
    (store, created[0].clone())
}

#[tokio::test]
async fn a_cancel_request_abandons_a_requested_handoff_instead_of_stopping_for_it() {
    let dir = tempfile::tempdir().unwrap();
    let (store, requested) = handoff_then_cancel(dir.path()).await;

    let next = store.begin_handoff_stop(&requested.id, HashMap::new(), after_checkpoint_wait()).await.unwrap().unwrap();

    assert_eq!(next.phase, HandoffPhase::Abandoned);
    assert!(next.abandoned_reason.unwrap().contains("cancel"), "放棄原因指出取消");
}

#[tokio::test]
async fn a_cancel_request_turns_the_transfer_into_a_cancel_confirmation_without_a_successor() {
    let dir = tempfile::tempdir().unwrap();
    let (store, requested) = handoff_then_cancel(dir.path()).await;

    let next = store
        .transfer_handoff(&requested.id, "stopped_gracefully", HashMap::new(), after_checkpoint_wait(), ids("run"))
        .await
        .unwrap();

    assert_eq!(next.phase, HandoffPhase::Abandoned);
    assert_eq!(next.to_run_id, None);
    let board = store.task_board("w1").await.unwrap();
    assert_eq!(board.runs.len(), 1, "不派出接手的 Run");
    assert_eq!(board.runs[0].run.status, RunStatus::Cancelled);
    assert!(board.runs[0].lease.as_ref().is_some_and(|l| !l.is_held()));
    assert_eq!(board.tasks[0].task.status, TaskStatus::Ready);
    assert!(queued_task_ids(dir.path()).is_empty(), "取消 Run＝暫停，不自動重派");
}
