use super::*;
use crate::profile::{AuthSource, ConfigDirOrigin, ProfileStatus, Verification};
use crate::workspace::ModelOverrides;

const NOW: Timestamp = Timestamp::from_millis(1_000);

fn workspace(id: &str, status: WorkspaceStatus) -> Workspace {
    Workspace {
        id: id.into(),
        name: format!("名稱-{id}"),
        path: format!("C:\\work\\{id}"),
        status,
        version: 1,
        created_at: Timestamp::from_millis(0),
        archived_at: None,
        model_overrides: ModelOverrides::default(),
    }
}

fn profile(id: &str, account: &str, logged_in: bool) -> AccountProfile {
    AccountProfile {
        id: id.into(),
        name: format!("帳號 {id}"),
        config_dir: format!("C:\\profiles\\{id}"),
        config_dir_origin: ConfigDirOrigin::Managed,
        expected_account: Some(account.into()),
        status: ProfileStatus::Active,
        verification: Some(Verification {
            checked_at: Timestamp::from_millis(0),
            logged_in,
            source: if logged_in { AuthSource::NativeLogin } else { AuthSource::None },
            source_detail: None,
            account: logged_in.then(|| account.to_string()),
            config_dir_matches: true,
            cli_version: Some("2.1.307 (Claude Code)".into()),
        }),
        version: 1,
        created_at: Timestamp::from_millis(0),
        archived_at: None,
    }
}

fn ids(n: u32) -> OpenIds {
    OpenIds { run_id: format!("ctl_{n}"), command_id: format!("cmd_{n}") }
}

fn opened(ws: &Workspace, profiles: &[AccountProfile]) -> ControllerCliRun {
    open(ws, "p1", profiles, &[], Some(3), ids(1), NOW).unwrap().run
}

fn host() -> ProcessRecord {
    ProcessRecord { pid: 42, created_at_millis: 7, command_hash: "h".into() }
}

#[test]
fn opening_creates_a_starting_run_in_the_workspace_directory_and_records_the_account() {
    let ws = workspace("w1", WorkspaceStatus::Active);
    let profiles = [profile("p1", "a@example.com", true)];

    let change = open(&ws, "p1", &profiles, &[], Some(3), ids(1), NOW).unwrap();

    assert_eq!(change.run.status, ControllerCliStatus::Starting);
    assert_eq!(change.run.working_dir, "C:\\work\\w1");
    assert_eq!(change.run.profile_id, "p1");
    assert_eq!(change.run.settings_version, Some(3));
    assert_eq!(change.run.launched_at, None);
    assert_eq!(change.events.len(), 1);
    assert_eq!(change.events[0].event_type, EVENT_CONTROLLER_CLI_OPENED);
    assert_eq!(change.events[0].aggregate_version, 1);
    assert_eq!(change.events[0].payload["profile_id"], "p1");
}

#[test]
fn a_workspace_has_at_most_one_unfinished_controller_run_but_a_profile_may_serve_several_workspaces() {
    let profiles = [profile("p1", "a@example.com", true)];
    let w1 = workspace("w1", WorkspaceStatus::Active);
    let w2 = workspace("w2", WorkspaceStatus::Active);
    let first = opened(&w1, &profiles);

    let again = open(&w1, "p1", &profiles, std::slice::from_ref(&first), None, ids(2), NOW);
    assert_eq!(again.unwrap_err(), ControllerCliError::AlreadyOpen("ctl_1".into()));

    // 另一個 Workspace 的 Run 不影響
    assert!(open(&w2, "p1", &profiles, std::slice::from_ref(&first), None, ids(3), NOW).is_ok());

    // 結束之後可以重新開啟
    let ended = end(&first, END_USER_ENDED, "使用者結束", None, NOW).unwrap().run;
    assert!(open(&w1, "p1", &profiles, &[ended], None, ids(4), NOW).is_ok());
}

#[test]
fn unusable_profiles_and_archived_workspaces_cannot_open() {
    let ws = workspace("w1", WorkspaceStatus::Active);
    let profiles = [profile("p1", "a@example.com", false)];
    let error = open(&ws, "p1", &profiles, &[], None, ids(1), NOW).unwrap_err();
    assert_eq!(error.code(), "profile_unavailable");

    let profiles = [profile("p1", "a@example.com", true)];
    assert_eq!(open(&ws, "nope", &profiles, &[], None, ids(1), NOW).unwrap_err().code(), "profile_not_found");

    let archived = workspace("w1", WorkspaceStatus::Archived);
    assert_eq!(open(&archived, "p1", &profiles, &[], None, ids(1), NOW).unwrap_err(), ControllerCliError::WorkspaceArchived);
}

#[test]
fn launch_intent_is_written_once_and_a_profile_that_became_unusable_ends_the_run_without_opening() {
    let ws = workspace("w1", WorkspaceStatus::Active);
    let profiles = [profile("p1", "a@example.com", true)];
    let run = opened(&ws, &profiles);

    let launched = begin_launch(&run, &profiles, true, NOW).unwrap();
    assert_eq!(launched.run.launched_at, Some(NOW));
    assert_eq!(launched.events[0].event_type, EVENT_CONTROLLER_CLI_LAUNCH_STARTED);
    assert!(begin_launch(&launched.run, &profiles, true, NOW).is_err(), "launch intent 不能寫兩次");

    let logged_out = [profile("p1", "a@example.com", false)];
    let ended = begin_launch(&run, &logged_out, true, NOW).unwrap().run;
    assert_eq!(ended.status, ControllerCliStatus::Ended);
    assert_eq!(ended.end_code.as_deref(), Some(END_PROFILE_UNAVAILABLE));
    assert_eq!(ended.launched_at, None);
}

#[test]
fn the_receipt_and_the_host_report_both_lead_to_running_in_either_order() {
    let ws = workspace("w1", WorkspaceStatus::Active);
    let profiles = [profile("p1", "a@example.com", true)];
    let run = begin_launch(&opened(&ws, &profiles), &profiles, true, NOW).unwrap().run;

    let receipt_first = record_receipt(&run, host(), NOW).unwrap();
    assert_eq!(receipt_first.run.status, ControllerCliStatus::Running);
    assert_eq!(receipt_first.events[0].event_type, EVENT_CONTROLLER_CLI_RUNNING);
    let then_host = host_started(&receipt_first.run, 99, NOW).unwrap();
    assert!(then_host.events.is_empty(), "已在執行中，不重複記錄");
    assert_eq!(then_host.run.claude_pid, Some(99));

    let host_first = host_started(&run, 99, NOW).unwrap();
    assert_eq!(host_first.run.status, ControllerCliStatus::Running);
    let then_receipt = record_receipt(&host_first.run, host(), NOW).unwrap();
    assert!(then_receipt.events.is_empty());
    assert_eq!(then_receipt.run.host, Some(host()));
}

#[test]
fn ending_is_terminal_and_records_the_reason_and_exit_code() {
    let ws = workspace("w1", WorkspaceStatus::Active);
    let profiles = [profile("p1", "a@example.com", true)];
    let run = record_receipt(&begin_launch(&opened(&ws, &profiles), &profiles, true, NOW).unwrap().run, host(), NOW).unwrap().run;

    let requested = request_end(&run, NOW).unwrap().unwrap();
    assert_eq!(requested.run.end_requested_at, Some(NOW));
    assert_eq!(request_end(&requested.run, NOW).unwrap(), None, "重複要求不產生事件");

    let ended = end(&requested.run, END_HOST_EXITED, "claude 已結束", Some(3), NOW).unwrap();
    assert_eq!(ended.run.status, ControllerCliStatus::Ended);
    assert_eq!(ended.run.exit_code, Some(3));
    assert_eq!(ended.events[0].event_type, EVENT_CONTROLLER_CLI_ENDED);
    assert_eq!(ended.events[0].payload["end_code"], END_HOST_EXITED);
    assert_eq!(ended.events[0].payload["profile_id"], "p1");

    assert_eq!(end(&ended.run, END_HOST_GONE, "x", None, NOW).unwrap_err(), ControllerCliError::AlreadyEnded);
    assert!(host_started(&ended.run, 1, NOW).is_err());
    assert!(record_receipt(&ended.run, host(), NOW).is_err());
}

// ---------------------------------------------------------------- 票 05：同帳號接續、換帳號重開

/// 開啟、開窗並進入執行中，再以指定原因結束。
fn ran_and_ended(ws: &Workspace, profile_id: &str, n: u32, profiles: &[AccountProfile]) -> ControllerCliRun {
    let run = open(ws, profile_id, profiles, &[], None, ids(n), NOW).unwrap().run;
    let run = record_receipt(&begin_launch(&run, profiles, true, NOW).unwrap().run, host(), NOW).unwrap().run;
    end(&run, END_HOST_EXITED, "claude 已結束", Some(0), NOW).unwrap().run
}

#[test]
fn reopening_with_the_same_profile_continues_the_native_session_and_another_profile_does_not() {
    let ws = workspace("w1", WorkspaceStatus::Active);
    let profiles = [profile("p1", "a@example.com", true), profile("p2", "b@example.com", true)];

    let first = open(&ws, "p1", &profiles, &[], None, ids(1), NOW).unwrap();
    assert!(!first.run.continue_session, "第一次開啟沒有可接續的 Session");
    assert_eq!(first.events[0].payload["continue_session"], false);

    let previous = [ran_and_ended(&ws, "p1", 1, &profiles)];
    let same = open(&ws, "p1", &profiles, &previous, None, ids(2), NOW).unwrap();
    assert!(same.run.continue_session);
    assert_eq!(same.events[0].payload["continue_session"], true);
    assert!(!open(&ws, "p2", &profiles, &previous, None, ids(3), NOW).unwrap().run.continue_session);
}

#[test]
fn only_the_latest_run_that_actually_ran_decides_whether_to_continue() {
    let ws = workspace("w1", WorkspaceStatus::Active);
    let profiles = [profile("p1", "a@example.com", true), profile("p2", "b@example.com", true)];
    let ran_p1 = ran_and_ended(&ws, "p1", 1, &profiles);
    // p2 開啟失敗（沒開出視窗）：沒有原生 Session，不算換手
    let failed_p2 = end(&open(&ws, "p2", &profiles, &[], None, ids(2), NOW).unwrap().run, END_CLAUDE_NOT_FOUND, "x", None, NOW)
        .unwrap()
        .run;
    assert!(open(&ws, "p1", &profiles, &[ran_p1.clone(), failed_p2.clone()], None, ids(3), NOW).unwrap().run.continue_session);

    // p1 之後 p2 真的跑過：再以 p1 開啟是換帳號，不接續
    let ran_p2 = ran_and_ended(&ws, "p2", 4, &profiles);
    assert!(!open(&ws, "p1", &profiles, &[ran_p1, ran_p2], None, ids(5), NOW).unwrap().run.continue_session);

    // 其他 Workspace 的 Run 不影響
    let other = ran_and_ended(&workspace("w2", WorkspaceStatus::Active), "p1", 6, &profiles);
    assert!(!open(&ws, "p1", &profiles, &[other], None, ids(7), NOW).unwrap().run.continue_session);
}

#[test]
fn switching_requests_the_end_and_remembers_the_new_profile() {
    let ws = workspace("w1", WorkspaceStatus::Active);
    let profiles = [profile("p1", "a@example.com", true), profile("p2", "b@example.com", true)];
    let run = record_receipt(&begin_launch(&opened(&ws, &profiles), &profiles, true, NOW).unwrap().run, host(), NOW).unwrap().run;

    let switched = request_switch(&run, "p2", &profiles, NOW).unwrap();
    assert_eq!(switched.run.end_requested_at, Some(NOW));
    assert_eq!(switched.run.switch_to_profile_id.as_deref(), Some("p2"));
    assert_eq!(switched.run.status, ControllerCliStatus::Running, "由 Supervisor 結束宿主之後才結束");
    assert_eq!(switched.events[0].event_type, EVENT_CONTROLLER_CLI_SWITCH_REQUESTED);
    assert_eq!(switched.events[0].payload["from_profile_id"], "p1");
    assert_eq!(switched.events[0].payload["to_profile_id"], "p2");

    // 已在結束中（含已要求換帳號）不能再換
    assert_eq!(request_switch(&switched.run, "p2", &profiles, NOW).unwrap_err(), ControllerCliError::Ending);
    let ending = request_end(&run, NOW).unwrap().unwrap().run;
    assert_eq!(request_switch(&ending, "p2", &profiles, NOW).unwrap_err(), ControllerCliError::Ending);
}

#[test]
fn switching_to_the_same_or_an_unusable_profile_is_refused() {
    let ws = workspace("w1", WorkspaceStatus::Active);
    let profiles = [profile("p1", "a@example.com", true), profile("p2", "b@example.com", false)];
    let run = opened(&ws, &profiles);

    assert_eq!(request_switch(&run, "p1", &profiles, NOW).unwrap_err(), ControllerCliError::SameProfile);
    assert_eq!(request_switch(&run, "p2", &profiles, NOW).unwrap_err().code(), "profile_unavailable");
    assert_eq!(request_switch(&run, "nope", &profiles, NOW).unwrap_err().code(), "profile_not_found");
    let ended = end(&run, END_HOST_EXITED, "x", None, NOW).unwrap().run;
    assert_eq!(request_switch(&ended, "p2", &profiles, NOW).unwrap_err(), ControllerCliError::AlreadyEnded);
}

#[test]
fn after_the_switched_run_ends_a_new_run_opens_with_the_new_profile_without_continuing() {
    let ws = workspace("w1", WorkspaceStatus::Active);
    let profiles = [profile("p1", "a@example.com", true), profile("p2", "b@example.com", true)];
    let run = record_receipt(&begin_launch(&opened(&ws, &profiles), &profiles, true, NOW).unwrap().run, host(), NOW).unwrap().run;
    let switching = request_switch(&run, "p2", &profiles, NOW).unwrap().run;
    assert_eq!(open_switched(&ws, &switching, &profiles, Some(4), ids(2), NOW).unwrap_err(), ControllerCliError::AlreadyOpen("ctl_1".into()));

    let ended = end(&switching, END_SWITCHED, "換帳號重開", None, NOW).unwrap().run;
    let next = open_switched(&ws, &ended, &profiles, Some(4), ids(2), NOW).unwrap().unwrap();

    assert_eq!(next.run.id, "ctl_2");
    assert_eq!(next.run.profile_id, "p2");
    assert_eq!(next.run.status, ControllerCliStatus::Starting);
    assert_eq!(next.run.settings_version, Some(4));
    assert_eq!(next.run.switched_from_run_id.as_deref(), Some("ctl_1"));
    assert!(!next.run.continue_session, "換帳號不接續原生 Session");
    assert_eq!(next.events[0].event_type, EVENT_CONTROLLER_CLI_OPENED);
    assert_eq!(next.events[0].payload["switched_from_run_id"], "ctl_1");
    assert_eq!(next.events[0].payload["profile_id"], "p2");

    // 沒有要求換帳號的 Run 結束時不開新的
    let plain = end(&opened(&ws, &profiles), END_HOST_EXITED, "x", None, NOW).unwrap().run;
    assert_eq!(open_switched(&ws, &plain, &profiles, None, ids(3), NOW).unwrap(), None);
}

#[test]
fn a_continue_whose_native_session_is_gone_opens_a_new_session_instead() {
    let ws = workspace("w1", WorkspaceStatus::Active);
    let profiles = [profile("p1", "a@example.com", true)];
    let previous = [ran_and_ended(&ws, "p1", 1, &profiles)];
    let reopened = open(&ws, "p1", &profiles, &previous, None, ids(2), NOW).unwrap().run;
    assert!(reopened.continue_session);

    let kept = begin_launch(&reopened, &profiles, true, NOW).unwrap();
    assert!(kept.run.continue_session);
    assert_eq!(kept.events[0].payload["continue_session"], true);

    let fresh = begin_launch(&reopened, &profiles, false, NOW).unwrap();
    assert!(!fresh.run.continue_session, "原生 Session 不存在時改開新的");
    assert_eq!(fresh.events[0].payload["continue_session"], false);

    // 沒有要接續時不受影響
    let first = opened(&ws, &profiles);
    assert!(!begin_launch(&first, &profiles, true, NOW).unwrap().run.continue_session);
}

/// 開啟後開窗並進入 running。
fn running(run: ControllerCliRun) -> ControllerCliRun {
    record_receipt(&run, host(), NOW).unwrap().run
}

#[test]
fn reopening_with_the_account_that_last_ran_continues_its_native_session_and_another_account_does_not() {
    let ws = workspace("w1", WorkspaceStatus::Active);
    let profiles = [profile("p1", "a@example.com", true), profile("p2", "b@example.com", true)];
    let first = opened(&ws, &profiles);
    assert!(!first.continue_session, "第一次開啟沒有可接續的原生 Session");
    let first = end(&running(first), END_USER_ENDED, "結束", None, NOW).unwrap().run;

    let same = open(&ws, "p1", &profiles, std::slice::from_ref(&first), None, ids(2), NOW).unwrap();
    assert!(same.run.continue_session);
    assert_eq!(same.events[0].payload["continue_session"], true);
    let other = open(&ws, "p2", &profiles, std::slice::from_ref(&first), None, ids(2), NOW).unwrap();
    assert!(!other.run.continue_session, "換帳號不接續");

    // 開啟失敗（沒有執行過）的 Run 不算：比對的是最近一次真的執行過的 Run
    let failed = end(&opened(&ws, &profiles), END_CLAUDE_NOT_FOUND, "找不到", None, NOW).unwrap().run;
    let failed = ControllerCliRun { id: "ctl_2".into(), profile_id: "p2".into(), ..failed };
    let after_failure = open(&ws, "p1", &profiles, &[first, failed], None, ids(3), NOW).unwrap();
    assert!(after_failure.run.continue_session);
}

#[test]
fn interrupting_is_terminal_and_records_the_restart_reason() {
    let ws = workspace("w1", WorkspaceStatus::Active);
    let profiles = [profile("p1", "a@example.com", true)];
    let run = running(opened(&ws, &profiles));

    let change = interrupt(&run, "Runtime 重新啟動時終端機已關閉", NOW).unwrap();

    assert_eq!(change.run.status, ControllerCliStatus::Interrupted);
    assert_eq!(change.run.end_code.as_deref(), Some(END_RUNTIME_RESTARTED));
    assert_eq!(change.run.ended_at, Some(NOW));
    assert!(!credential_valid(&change.run));
    assert_eq!(change.events[0].event_type, EVENT_CONTROLLER_CLI_INTERRUPTED);
    assert_eq!(interrupt(&change.run, "再一次", NOW), Err(ControllerCliError::AlreadyEnded));
}

#[test]
fn rebinding_keeps_the_run_open_but_invalidates_its_credential_once() {
    let ws = workspace("w1", WorkspaceStatus::Active);
    let profiles = [profile("p1", "a@example.com", true)];
    let run = running(opened(&ws, &profiles));
    assert!(credential_valid(&run));

    let change = rebind(&run, NOW).unwrap().unwrap();

    assert_eq!(change.run.status, ControllerCliStatus::Running);
    assert_eq!(change.run.rebound_at, Some(NOW));
    assert!(!credential_valid(&change.run));
    assert_eq!(change.events[0].event_type, EVENT_CONTROLLER_CLI_REBOUND);
    assert_eq!(rebind(&change.run, NOW), Ok(None), "已重新綁定過就不再改變");
    // 之後仍可以結束（使用者按結束或關掉終端機）
    assert_eq!(end(&change.run, END_HOST_GONE, "關閉", None, NOW).unwrap().run.status, ControllerCliStatus::Ended);
}
