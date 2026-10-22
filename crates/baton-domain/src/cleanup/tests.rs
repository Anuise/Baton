use super::*;
use crate::integration::{self, IntegrationEntry};
use crate::run::{Run, RunStatus};
use crate::workdir::{self, TaskKind, WorkdirStatus};
use crate::workspace::{self, WorkspaceCommand};

const T0: Timestamp = Timestamp::from_millis(1_000_000);

fn later(ms: i64) -> Timestamp {
    T0.plus_millis(ms)
}

fn workspace() -> Workspace {
    let create = WorkspaceCommand::Create { id: "w1".into(), name: None, path: r"C:\work\app".into() };
    workspace::decide(&[], create, T0).unwrap().workspace
}

fn task(status: TaskStatus, kind: TaskKind) -> Task {
    Task {
        id: "t1".into(),
        workspace_id: "w1".into(),
        goal_id: "g1".into(),
        title: "t1".into(),
        kind,
        status,
        version: 3,
        contract_version: 1,
        created_at: T0,
        updated_at: T0,
    }
}

fn registered(kind: TaskKind) -> Workdir {
    let plan = workdir::plan(&workspace(), &task(TaskStatus::Ready, kind));
    let base = plan.branch.as_ref().map(|_| "main".to_string());
    workdir::record_prepared(None, &plan, base, T0).0
}

fn run(id: &str, status: RunStatus) -> Run {
    Run {
        id: id.into(),
        workspace_id: "w1".into(),
        task_id: "t1".into(),
        profile_id: "p1".into(),
        status,
        version: 1,
        generation: 1,
        contract_version: 1,
        authorization_revision: None,
        command_id: None,
        process: None,
        cancel_requested_at: None,
        end_reason: None,
        created_at: T0,
        updated_at: T0,
        started_at: None,
        ended_at: None,
        native_session_id: None,
        admitted_at: None,
        settings_version: None,
        agent_session_id: None,
        resume_native_session_id: None,
    }
}

fn entry(seq: i64, status: IntegrationStatus) -> IntegrationEntry {
    let (mut entry, _) =
        integration::enqueue(format!("int_{seq}"), &task(TaskStatus::Verifying, TaskKind::Development), "r1", 1, "baton/t1", "main", T0);
    entry.seq = seq;
    entry.status = status;
    entry
}

fn requested() -> WorktreeCleanup {
    request(&registered(TaskKind::Development), T0).0
}

fn begin(cleanup: &WorktreeCleanup, now: Timestamp) -> WorktreeCleanup {
    decide(cleanup, CleanupCommand::Begin { task_id: "t1".into() }, now).unwrap().cleanup
}

fn finish(cleanup: &WorktreeCleanup, result: Result<(), String>, now: Timestamp) -> CleanupChange {
    decide(cleanup, CleanupCommand::Finish { task_id: "t1".into(), result }, now).unwrap()
}

#[test]
fn cleanup_is_requested_only_for_a_completed_integrated_task_whose_runs_have_all_ended() {
    let workdir = registered(TaskKind::Development);
    let completed = task(TaskStatus::Completed, TaskKind::Development);
    let ended = [run("r1", RunStatus::Failed), run("r2", RunStatus::Succeeded)];
    let integrated = [entry(1, IntegrationStatus::Failed), entry(2, IntegrationStatus::Integrated)];
    assert!(should_request(&workdir, &completed, &ended, &integrated, None));

    // 暫停、交接、失敗、取消都不會讓 Task 完成，所以都不觸發
    for status in [TaskStatus::Ready, TaskStatus::Running, TaskStatus::Blocked, TaskStatus::Verifying, TaskStatus::Cancelled] {
        let other = task(status, TaskKind::Development);
        assert!(!should_request(&workdir, &other, &ended, &integrated, None), "{status:?} 不應觸發清理");
    }
    // 最新的整合項目不是已整合
    let not_integrated = [entry(1, IntegrationStatus::Integrated), entry(2, IntegrationStatus::Queued)];
    assert!(!should_request(&workdir, &completed, &ended, &not_integrated, None));
    assert!(!should_request(&workdir, &completed, &ended, &[], None));
    // 還有 Run 沒結束（程序可能還在寫）
    let still_running = [run("r1", RunStatus::Succeeded), run("r2", RunStatus::Running)];
    assert!(!should_request(&workdir, &completed, &still_running, &integrated, None));
    // 已經登記過
    assert!(!should_request(&workdir, &completed, &ended, &integrated, Some(&requested())));
}

#[test]
fn output_directories_and_unavailable_workdirs_are_never_cleaned() {
    let completed = task(TaskStatus::Completed, TaskKind::Research);
    let integrated = [entry(1, IntegrationStatus::Integrated)];
    assert!(!should_request(&registered(TaskKind::Research), &completed, &[], &integrated, None));

    let mut failed = registered(TaskKind::Development);
    failed.status = WorkdirStatus::Failed;
    let completed = task(TaskStatus::Completed, TaskKind::Development);
    assert!(!should_request(&failed, &completed, &[], &integrated, None));
}

#[test]
fn a_request_persists_the_target_and_is_due_immediately() {
    let (cleanup, event) = request(&registered(TaskKind::Development), T0);
    assert_eq!(cleanup.status, CleanupStatus::Pending);
    assert_eq!(cleanup.path, r"C:\work\app\.baton\worktrees\t1");
    assert_eq!(cleanup.branch, "baton/t1");
    assert_eq!((cleanup.attempts, cleanup.version), (0, 1));
    assert_eq!((event.event_type, event.aggregate_type, event.aggregate_version), (EVENT_REQUESTED, AGGREGATE_TYPE, 1));
    assert!(is_due(&cleanup, T0));
    assert_eq!(trash_dir(&workspace(), "t1"), r"C:\work\app\.baton\trash\t1");
}

#[test]
fn a_successful_attempt_cleans_the_worktree() {
    let removing = begin(&requested(), later(1));
    assert_eq!((removing.status, removing.attempts, removing.next_attempt_at), (CleanupStatus::Removing, 1, None));

    let change = finish(&removing, Ok(()), later(2));
    assert_eq!(change.cleanup.status, CleanupStatus::Cleaned);
    assert_eq!(change.cleanup.finished_at, Some(later(2)));
    assert_eq!(change.events[0].event_type, EVENT_CLEANED);
    assert!(!is_due(&change.cleanup, later(10_000_000)));
}

#[test]
fn a_failed_attempt_keeps_the_cleanup_pending_with_backoff_until_retries_run_out() {
    let mut cleanup = requested();
    let mut now = T0;
    let mut delays = Vec::new();
    for attempt in 1..=MAX_RETRIES {
        cleanup = begin(&cleanup, now);
        assert_eq!(cleanup.attempts, attempt);
        let change = finish(&cleanup, Err("檔案被占用".into()), now);
        cleanup = change.cleanup;
        assert_eq!(cleanup.status, CleanupStatus::Pending, "第 {attempt} 次失敗後仍待清理");
        assert_eq!(cleanup.failure.as_deref(), Some("檔案被占用"));
        assert_eq!(change.events[0].event_type, EVENT_RETRY_SCHEDULED);
        let at = cleanup.next_attempt_at.unwrap();
        delays.push(at.as_millis() - now.as_millis());
        // 還沒到重試時間不能開始
        assert!(!is_due(&cleanup, at.plus_millis(-1)));
        assert_eq!(
            decide(&cleanup, CleanupCommand::Begin { task_id: "t1".into() }, at.plus_millis(-1)),
            Err(CleanupError::NotDue)
        );
        now = at;
    }
    assert_eq!(delays, vec![60_000, 120_000, 240_000, 480_000, 600_000], "1 分鐘起加倍、上限 10 分鐘");

    // 第一次加上 5 次重試都失敗：清理失敗，不再自動重試
    cleanup = begin(&cleanup, now);
    let change = finish(&cleanup, Err("仍被占用".into()), now);
    assert_eq!(change.cleanup.status, CleanupStatus::Failed);
    assert_eq!(change.cleanup.attempts, MAX_RETRIES + 1);
    assert_eq!(change.cleanup.failure.as_deref(), Some("仍被占用"));
    assert_eq!(change.events[0].event_type, EVENT_FAILED);
    assert!(!is_due(&change.cleanup, now.plus_millis(100_000_000)));
    assert!(decide(&change.cleanup, CleanupCommand::Begin { task_id: "t1".into() }, now).is_err());
}

#[test]
fn resuming_an_interrupted_attempt_does_not_count_as_another_attempt() {
    let removing = begin(&requested(), later(1));
    assert!(is_due(&removing, later(1)), "停在清理中的登記重建後立即接續");
    let resumed = decide(&removing, CleanupCommand::Begin { task_id: "t1".into() }, later(2)).unwrap();
    assert_eq!((resumed.cleanup.status, resumed.cleanup.attempts), (CleanupStatus::Removing, 1));
    assert_eq!(resumed.events[0].payload["detail"]["resumed"], true);
}

#[test]
fn results_are_only_recorded_while_removing() {
    let pending = requested();
    let result = decide(&pending, CleanupCommand::Finish { task_id: "t1".into(), result: Ok(()) }, later(1));
    assert!(matches!(result, Err(CleanupError::TransitionInvalid { from: CleanupStatus::Pending, .. })));
    let other = decide(&pending, CleanupCommand::Begin { task_id: "t2".into() }, later(1));
    assert_eq!(other, Err(CleanupError::NotFound("t2".into())));
}

#[test]
fn only_a_clean_registered_worktree_on_the_platform_branch_or_a_missing_one_may_be_removed() {
    let cleanup = requested();
    let platform = Some("baton/t1".to_string());
    assert_eq!(check_safety(&cleanup, &Inspection::Missing), Ok(()));
    assert_eq!(check_safety(&cleanup, &Inspection::Registered { branch: platform.clone(), unsaved: vec![] }), Ok(()));

    let foreign = check_safety(&cleanup, &Inspection::Foreign).unwrap_err();
    assert!(foreign.contains("歸屬不明") && foreign.contains(&cleanup.path), "{foreign}");
    let other_branch =
        check_safety(&cleanup, &Inspection::Registered { branch: Some("main".into()), unsaved: vec![] }).unwrap_err();
    assert!(other_branch.contains("main") && other_branch.contains("baton/t1"), "{other_branch}");
    let detached = check_safety(&cleanup, &Inspection::Registered { branch: None, unsaved: vec![] }).unwrap_err();
    assert!(detached.contains("detached HEAD"), "{detached}");

    let unsaved: Vec<String> = (1..=7).map(|i| format!("f{i}.txt")).collect();
    let dirty = check_safety(&cleanup, &Inspection::Registered { branch: platform, unsaved }).unwrap_err();
    assert!(dirty.contains("7 個未保存的變更") && dirty.contains("f1.txt") && dirty.contains("…"), "{dirty}");
    assert!(!dirty.contains("f6.txt"), "最多列出 5 個：{dirty}");
}

#[test]
fn statuses_round_trip() {
    for status in CleanupStatus::ALL {
        assert_eq!(CleanupStatus::parse(status.as_str()), Some(status));
    }
    assert_eq!(CleanupStatus::parse("other"), None);
}
