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

    let launched = begin_launch(&run, &profiles, NOW).unwrap();
    assert_eq!(launched.run.launched_at, Some(NOW));
    assert_eq!(launched.events[0].event_type, EVENT_CONTROLLER_CLI_LAUNCH_STARTED);
    assert!(begin_launch(&launched.run, &profiles, NOW).is_err(), "launch intent 不能寫兩次");

    let logged_out = [profile("p1", "a@example.com", false)];
    let ended = begin_launch(&run, &logged_out, NOW).unwrap().run;
    assert_eq!(ended.status, ControllerCliStatus::Ended);
    assert_eq!(ended.end_code.as_deref(), Some(END_PROFILE_UNAVAILABLE));
    assert_eq!(ended.launched_at, None);
}

#[test]
fn the_receipt_and_the_host_report_both_lead_to_running_in_either_order() {
    let ws = workspace("w1", WorkspaceStatus::Active);
    let profiles = [profile("p1", "a@example.com", true)];
    let run = begin_launch(&opened(&ws, &profiles), &profiles, NOW).unwrap().run;

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
    let run = record_receipt(&begin_launch(&opened(&ws, &profiles), &profiles, NOW).unwrap().run, host(), NOW).unwrap().run;

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
