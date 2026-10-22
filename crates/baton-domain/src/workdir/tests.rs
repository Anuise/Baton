use super::*;
use crate::task::TaskStatus;
use crate::workspace::{self, WorkspaceCommand};

const T0: Timestamp = Timestamp::from_millis(1_000_000);

fn workspace(path: &str) -> Workspace {
    let create = WorkspaceCommand::Create { id: "w1".into(), name: None, path: path.into() };
    workspace::decide(&[], create, T0).unwrap().workspace
}

fn task(id: &str, kind: TaskKind) -> Task {
    Task {
        id: id.into(),
        workspace_id: "w1".into(),
        goal_id: "g1".into(),
        title: id.into(),
        kind,
        status: TaskStatus::Ready,
        version: 1,
        contract_version: 1,
        created_at: T0,
        updated_at: T0,
    }
}

#[test]
fn development_tasks_get_a_worktree_on_their_own_branch_and_others_get_an_output_directory() {
    let ws = workspace(r"C:\work\app");

    let dev = plan(&ws, &task("t_1", TaskKind::Development));
    assert_eq!(dev.kind, WorkdirKind::Worktree);
    assert_eq!(dev.path, r"C:\work\app\.baton\worktrees\t_1");
    assert_eq!(dev.branch.as_deref(), Some("baton/t_1"));
    assert_eq!(dev.repo, r"C:\work\app");
    assert_eq!(dev.gitignore, r"C:\work\app\.baton\.gitignore");

    for kind in [TaskKind::Research, TaskKind::Document] {
        let out = plan(&ws, &task("t_2", kind));
        assert_eq!(out.kind, WorkdirKind::OutputDir);
        assert_eq!(out.path, r"C:\work\app\.baton\outputs\t_2");
        assert_eq!(out.branch, None, "輸出目錄沒有分支");
    }
}

#[test]
fn different_tasks_never_share_a_directory_even_at_a_drive_root() {
    let ws = workspace(r"D:\");
    let a = plan(&ws, &task("a", TaskKind::Development));
    let b = plan(&ws, &task("b", TaskKind::Development));
    assert_eq!(a.path, r"D:\.baton\worktrees\a");
    assert_ne!(a.path, b.path);
    assert_ne!(a.branch, b.branch);
}

#[test]
fn the_default_task_kind_is_research_and_kinds_round_trip() {
    assert_eq!(TaskKind::default(), TaskKind::Research);
    for kind in TaskKind::ALL {
        assert_eq!(TaskKind::parse(kind.as_str()), Some(kind));
    }
    let parsed: Task = serde_json::from_value(serde_json::json!({
        "id": "t", "workspace_id": "w", "goal_id": "g", "title": "x", "status": "ready",
        "version": 1, "contract_version": 1, "created_at": 0, "updated_at": 0
    }))
    .unwrap();
    assert_eq!(parsed.kind, TaskKind::Research, "舊資料沒有 kind 時視為研究");
}

#[test]
fn preparation_is_retried_after_a_failure_but_never_redone_once_ready() {
    let ws = workspace(r"C:\work\notes");
    let p = plan(&ws, &task("t", TaskKind::Development));
    assert!(needs_preparation(None, T0, false));

    let (failed, event) = record_failed(None, &p, &PrepareFailure::NotGitRepository, T0);
    assert_eq!(failed.status, WorkdirStatus::Failed);
    assert!(failed.failure.as_deref().unwrap().contains("不是 Git repo"), "{failed:?}");
    assert_eq!((event.event_type, event.aggregate_version), (EVENT_FAILED, 1));
    assert!(!needs_preparation(Some(&failed), T0.plus_millis(PREPARE_RETRY_MS - 1), false), "失敗後不會每秒重試");
    assert!(needs_preparation(Some(&failed), T0.plus_millis(PREPARE_RETRY_MS), false));
    assert!(needs_preparation(Some(&failed), T0, true), "使用者再次要求派工時立即重試");

    let later = T0.plus_millis(5);
    let (ready, event) = record_prepared(Some(&failed), &p, Some("main".into()), later);
    assert_eq!((ready.status, ready.version, ready.ready_at), (WorkdirStatus::Ready, 2, Some(later)));
    assert_eq!(ready.base_branch.as_deref(), Some("main"));
    assert_eq!(ready.failure, None);
    assert_eq!((event.event_type, event.aggregate_version), (EVENT_PREPARED, 2));
    assert!(!needs_preparation(Some(&ready), later.plus_millis(PREPARE_RETRY_MS * 10), true));
}

#[test]
fn failure_messages_explain_what_to_do() {
    assert!(PrepareFailure::DetachedHead.message(WorkdirKind::Worktree).contains("detached HEAD"));
    assert_eq!(PrepareFailure::Io("拒絕存取".into()).message(WorkdirKind::OutputDir), "無法建立輸出目錄：拒絕存取");
    assert_eq!(PrepareFailure::Io("拒絕存取".into()).message(WorkdirKind::Worktree), "無法建立 worktree：拒絕存取");
}
