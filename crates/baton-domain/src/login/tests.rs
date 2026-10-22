//! 跨登入啟動與續跑的單元測試（票 33）。純 Domain，不經 HTTP。

use super::*;
use crate::lifecycle::{self, complete_exit, dispatch_halt, reopen, request_exit, ExitTiming, LifecyclePhase};

const T0: Timestamp = Timestamp::from_millis(1_000);
const T1: Timestamp = Timestamp::from_millis(2_000);

fn workspace(id: &str, status: WorkspaceStatus) -> Workspace {
    Workspace {
        id: id.into(),
        name: format!("名稱 {id}"),
        path: format!("C:\\work\\{id}"),
        status,
        version: 1,
        created_at: T0,
        archived_at: None,
        model_overrides: Default::default(),
    }
}

fn across_login(id: &str, enabled: bool) -> WorkspaceLifecycle {
    let mut lifecycle = WorkspaceLifecycle::new(id);
    lifecycle.auto_resume_across_login = enabled;
    lifecycle
}

#[test]
fn both_new_settings_default_to_off_and_are_separate_from_the_exit_setting() {
    assert!(!LoginStartup::default().enabled);
    let lifecycle = WorkspaceLifecycle::new("ws_a");
    assert!(!lifecycle.auto_resume_across_login);

    let (changed, event) = set_login_resume(&lifecycle, true, T1).unwrap();
    assert!(changed.auto_resume_across_login);
    assert!(!changed.auto_resume_after_exit, "跨登入設定不連帶改變完全退出後的設定");
    assert_eq!(event.event_type, EVENT_WORKSPACE_LOGIN_RESUME_CHANGED);
    assert!(set_login_resume(&changed, true, T1).is_none(), "相同值沒有變動");

    let (exit_only, _) = lifecycle::set_auto_resume(&lifecycle, true, T1).unwrap();
    assert!(!exit_only.auto_resume_across_login, "完全退出後的設定不連帶改變跨登入設定");
}

#[test]
fn login_startup_changes_are_versioned_and_idempotent() {
    let (on, event) = set_login_startup(&LoginStartup::default(), true, T1).unwrap();
    assert!(on.enabled);
    assert_eq!(on.version, 1);
    assert_eq!(event.event_type, EVENT_LOGIN_STARTUP_CHANGED);
    assert_eq!(event.aggregate_version, 1);
    assert!(set_login_startup(&on, true, T1).is_none());
    let (off, _) = set_login_startup(&on, false, T1).unwrap();
    assert_eq!(off.version, 2);
}

#[test]
fn login_boundary_compares_login_identities() {
    assert_eq!(login_boundary(None, Some("L1")), LoginBoundary::FirstStart);
    assert_eq!(login_boundary(None, None), LoginBoundary::FirstStart);
    assert_eq!(login_boundary(Some(Some("L1")), Some("L1")), LoginBoundary::SameLogin);
    assert_eq!(login_boundary(Some(Some("L1")), Some("L2")), LoginBoundary::NewLogin);
    assert_eq!(login_boundary(Some(None), Some("L2")), LoginBoundary::Unverified);
    assert_eq!(login_boundary(Some(Some("L1")), None), LoginBoundary::Unverified);
}

#[test]
fn a_new_login_pauses_only_workspaces_without_cross_login_resume() {
    let workspaces = [
        workspace("ws_on", WorkspaceStatus::Active),
        workspace("ws_off", WorkspaceStatus::Active),
        workspace("ws_archived", WorkspaceStatus::Archived),
    ];
    // ws_off 開了「完全退出後自動續跑」，但那個設定與跨登入無關
    let mut off = across_login("ws_off", false);
    off.auto_resume_after_exit = true;
    let existing = [across_login("ws_on", true), off];

    let change = cross_login(&LifecycleState::default(), LoginBoundary::NewLogin, &workspaces, &existing, T1);
    assert!(change.state.is_none(), "跨登入不改變 Runtime 的生命週期狀態");
    assert_eq!(change.workspaces.len(), 1);
    let paused = &change.workspaces[0];
    assert_eq!(paused.workspace_id, "ws_off");
    assert_eq!(paused.paused_since, Some(T1));
    assert_eq!(paused.paused_by, Some(PauseCause::NewLogin));
    assert_eq!(change.events[0].event_type, EVENT_WORKSPACE_PAUSED_BY_LOGIN);
    assert_eq!(change.events[0].payload["cause"], "new_login");

    let halt = dispatch_halt(&LifecycleState::default(), paused, "OFF").unwrap();
    assert_eq!(halt.code, "awaiting_resume");
    assert!(halt.message.contains("跨登入"), "{}", halt.message);
}

#[test]
fn the_same_login_or_a_first_start_changes_nothing() {
    let workspaces = [workspace("ws_off", WorkspaceStatus::Active)];
    for boundary in [LoginBoundary::SameLogin, LoginBoundary::FirstStart] {
        let change = cross_login(&LifecycleState::default(), boundary, &workspaces, &[], T1);
        assert_eq!(change, LifecycleChange::default(), "{boundary:?}");
    }
}

#[test]
fn an_unverified_login_pauses_every_workspace_regardless_of_settings() {
    let workspaces = [workspace("ws_on", WorkspaceStatus::Active), workspace("ws_off", WorkspaceStatus::Active)];
    let existing = [across_login("ws_on", true)];
    let change = cross_login(&LifecycleState::default(), LoginBoundary::Unverified, &workspaces, &existing, T1);
    let causes: Vec<_> = change.workspaces.iter().map(|w| (w.workspace_id.as_str(), w.paused_by)).collect();
    assert_eq!(
        causes,
        vec![("ws_on", Some(PauseCause::LoginUnverified)), ("ws_off", Some(PauseCause::LoginUnverified))]
    );
}

#[test]
fn a_stop_intent_takes_priority_over_cross_login_handling() {
    let workspaces = [workspace("ws_off", WorkspaceStatus::Active)];
    let stopping = request_exit(&LifecycleState::default(), &workspaces, &[], ExitTiming::default(), T0);
    let state = stopping.state.unwrap();
    let change = cross_login(&state, LoginBoundary::NewLogin, &workspaces, &stopping.workspaces, T1);
    assert_eq!(change, LifecycleChange::default());
}

#[test]
fn existing_pauses_keep_their_cause_and_are_never_released_by_a_login() {
    let workspaces = [workspace("ws_on", WorkspaceStatus::Active)];
    let mut paused = across_login("ws_on", true);
    paused.paused_since = Some(T0);
    paused.paused_by = Some(PauseCause::FullExit);
    for boundary in [LoginBoundary::NewLogin, LoginBoundary::Unverified] {
        let change = cross_login(&LifecycleState::default(), boundary, &workspaces, &[paused.clone()], T1);
        assert!(change.workspaces.is_empty(), "{boundary:?}：跨登入自動續跑不解除既有的暫停");
    }
}

#[test]
fn reopening_after_exit_does_not_release_a_pause_caused_by_a_login() {
    // ws_a 先因跨登入被暫停，之後完全退出再重新開啟：即使開了「完全退出後自動續跑」，仍等「繼續工作」
    let workspaces = [workspace("ws_a", WorkspaceStatus::Active), workspace("ws_b", WorkspaceStatus::Active)];
    let mut a = WorkspaceLifecycle::new("ws_a");
    a.auto_resume_after_exit = true;
    a.paused_since = Some(T0);
    a.paused_by = Some(PauseCause::NewLogin);
    let mut b = WorkspaceLifecycle::new("ws_b");
    b.auto_resume_after_exit = true;

    let stopping = request_exit(&LifecycleState::default(), &workspaces, &[a.clone(), b], ExitTiming::default(), T0);
    let state = stopping.state.unwrap();
    assert_eq!(stopping.workspaces.len(), 1, "已暫停的 ws_a 維持原本的暫停");
    assert_eq!(stopping.workspaces[0].paused_by, Some(PauseCause::FullExit));
    let (stopped, _) = complete_exit(&state, state.last_exit.clone().unwrap(), T1).unwrap();

    let all = [a, stopping.workspaces[0].clone()];
    let change = reopen(&stopped, stopped.version, &all, T1).unwrap();
    assert_eq!(change.state.unwrap().phase, LifecyclePhase::Running);
    let released: Vec<_> = change.workspaces.iter().map(|w| w.workspace_id.as_str()).collect();
    assert_eq!(released, vec!["ws_b"]);
    assert_eq!(change.workspaces[0].paused_by, None);
}

#[test]
fn pause_causes_round_trip_through_their_codes() {
    for cause in PauseCause::ALL {
        assert_eq!(PauseCause::parse(cause.as_str()), Some(cause));
    }
    assert_eq!(PauseCause::parse("other"), None);
}
