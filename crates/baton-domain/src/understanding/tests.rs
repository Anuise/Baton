use super::*;

const T0: Timestamp = Timestamp::from_millis(1_700_000_000_000);
const T1: Timestamp = Timestamp::from_millis(1_700_000_060_000);

fn file(path: &str) -> SourceCheck {
    SourceCheck::Found(Source::File { path: path.into(), line: None, version: "a".repeat(64), checked_at: T0 })
}

fn input(status: ItemStatus, sources: Vec<SourceCheck>) -> RecordInput {
    RecordInput { section: Section::Glossary, key: "Run".into(), text: "一次程序執行".into(), status, sources }
}

fn recorded(status: ItemStatus, sources: Vec<SourceCheck>) -> UnderstandingItem {
    record(None, "u_1".into(), "ws_1", input(status, sources), "ctl_1", T0).unwrap().unwrap().item
}

// --- 未知資訊不被寫成已確認事實 ---

#[test]
fn a_confirmed_item_without_any_source_is_rejected() {
    let error = record(None, "u_1".into(), "ws_1", input(ItemStatus::Confirmed, vec![]), "ctl_1", T0).unwrap_err();
    assert_eq!(error, UnderstandingError::UnverifiedClaim);
}

#[test]
fn a_source_that_cannot_be_found_is_rejected_whatever_the_status() {
    for status in [ItemStatus::Confirmed, ItemStatus::Hypothesis, ItemStatus::Unknown] {
        let missing = SourceCheck::Missing { path: "docs/none.md".into() };
        let error = record(None, "u_1".into(), "ws_1", input(status, vec![file("a.md"), missing]), "ctl_1", T0).unwrap_err();
        assert_eq!(error, UnderstandingError::SourceNotFound("docs/none.md".into()));
    }
}

#[test]
fn hypotheses_and_unknowns_need_no_source_and_confirmed_items_keep_their_checked_sources() {
    assert_eq!(recorded(ItemStatus::Hypothesis, vec![]).status, ItemStatus::Hypothesis);
    assert_eq!(recorded(ItemStatus::Unknown, vec![]).status, ItemStatus::Unknown);
    let confirmed = recorded(ItemStatus::Confirmed, vec![file("README.md")]);
    assert_eq!(confirmed.status, ItemStatus::Confirmed);
    assert_eq!(confirmed.sources.len(), 1);
    assert_eq!(confirmed.revision, 1);
}

#[test]
fn external_sources_must_name_a_version_and_paths_must_stay_inside_the_workspace() {
    let external = SourceInput { reference: Some("https://example.com/spec".into()), ..Default::default() };
    assert!(matches!(normalize_sources(&[external]), Err(UnderstandingError::Invalid(_))));
    for path in ["../secret.txt", "C:/Windows/win.ini", "/etc/passwd"] {
        let input = SourceInput { path: Some(path.into()), ..Default::default() };
        assert!(matches!(normalize_sources(&[input]), Err(UnderstandingError::Invalid(_))), "{path}");
    }
    let both = SourceInput { path: Some("a.md".into()), reference: Some("x".into()), ..Default::default() };
    assert!(normalize_sources(&[both]).is_err());
    let ok = SourceInput { path: Some("docs\\a.md".into()), line: Some(3), ..Default::default() };
    assert_eq!(normalize_sources(&[ok]).unwrap(), vec![SourceRequest::File { path: "docs/a.md".into(), line: Some(3) }]);
}

#[test]
fn recording_the_same_content_again_changes_nothing_and_a_change_bumps_the_revision() {
    let item = recorded(ItemStatus::Hypothesis, vec![]);
    assert_eq!(record(Some(&item), "u_2".into(), "ws_1", input(ItemStatus::Hypothesis, vec![]), "ctl_2", T1).unwrap(), None);
    let change = record(Some(&item), "u_2".into(), "ws_1", input(ItemStatus::Confirmed, vec![file("a.md")]), "ctl_2", T1)
        .unwrap()
        .unwrap();
    assert_eq!(change.item.id, "u_1");
    assert_eq!(change.item.revision, 2);
    assert_eq!(change.item.recorded_by, "ctl_2");
    assert_eq!(change.event.aggregate_version, 2);
    assert_eq!(change.event.payload["status"], "confirmed");
}

#[test]
fn retiring_needs_an_existing_item_and_keeps_its_description() {
    let error = record(None, "u_1".into(), "ws_1", input(ItemStatus::Retired, vec![]), "ctl_1", T0).unwrap_err();
    assert!(matches!(error, UnderstandingError::NotFound(_)));
    let item = recorded(ItemStatus::Confirmed, vec![file("a.md")]);
    let mut retire = input(ItemStatus::Retired, vec![]);
    retire.text = String::new();
    let retired = record(Some(&item), "x".into(), "ws_1", retire, "ctl_2", T1).unwrap().unwrap().item;
    assert_eq!(retired.status, ItemStatus::Retired);
    assert_eq!(retired.text, item.text);
    assert_eq!(retired.sources, item.sources);
}

// --- 產生內容 ---

#[test]
fn the_glossary_marks_hypotheses_and_lists_unknown_terms_separately() {
    let confirmed = recorded(ItemStatus::Confirmed, vec![file("docs/run.md")]);
    let mut hypothesis = recorded(ItemStatus::Hypothesis, vec![]);
    hypothesis.key = "Lease".into();
    hypothesis.text = "大概是租約".into();
    let mut unknown = recorded(ItemStatus::Unknown, vec![]);
    unknown.key = "Baton".into();
    unknown.text = "不知道指什麼".into();
    let mut retired = recorded(ItemStatus::Hypothesis, vec![]);
    retired.key = "Old".into();
    retired.status = ItemStatus::Retired;
    let body = render_glossary(&[confirmed, hypothesis, unknown, retired]);
    assert!(body.contains("## Run\n\n一次程序執行\n\n- 狀態：已確認\n- 來源：`docs/run.md`（版本 aaaaaaaaaaaa，查驗於 2023-11-14 22:13 UTC）"), "{body}");
    assert!(body.contains("## Lease\n\n大概是租約\n\n- 狀態：假說（未確認）"), "{body}");
    assert!(body.contains("## 待確認（未知）\n\n- **Baton**：不知道指什麼"), "{body}");
    assert!(!body.contains("## Baton"), "未知的名詞不能寫成定義：{body}");
    assert!(!body.contains("Old"), "{body}");
}

#[test]
fn format_utc_handles_dates() {
    assert_eq!(format_utc(Timestamp::from_millis(0)), "1970-01-01 00:00 UTC");
    assert_eq!(format_utc(Timestamp::from_millis(951_782_400_000)), "2000-02-29 00:00 UTC");
}

// --- 投影 ---

fn target(body: &str) -> ProjectionTarget {
    ProjectionTarget { path: GLOSSARY_PATH.into(), kind: ProjectionKind::Glossary, body: body.into() }
}

fn written(record: Option<&Projection>, body: &str, current: Option<&[u8]>) -> (Projection, String) {
    let plan = plan_projection("ws_1", record, &target(body), current, T0);
    (plan.projection.unwrap(), plan.write.unwrap())
}

#[test]
fn a_missing_file_is_written_with_a_versioned_header() {
    let (projection, content) = written(None, "# Glossary\n", None);
    assert!(content.starts_with("<!-- Baton 投影：Glossary 第 1 版。"), "{content}");
    assert!(content.ends_with("# Glossary\n"));
    assert_eq!(projection.status, ProjectionStatus::InSync);
    assert_eq!(projection.baseline_hash.as_deref(), Some(sha256_hex(content.as_bytes()).as_str()));
}

#[test]
fn an_untouched_file_is_updated_and_the_version_follows_the_content() {
    let (first, content) = written(None, "v1", None);
    let (second, next) = written(Some(&first), "v2", Some(content.as_bytes()));
    assert_eq!(second.version, 2);
    assert!(next.contains("第 2 版"));
    // 內容沒變：什麼都不做
    let plan = plan_projection("ws_1", Some(&second), &target("v2"), Some(next.as_bytes()), T1);
    assert_eq!(plan, ProjectionPlan::unchanged());
}

#[test]
fn an_edited_deleted_or_pre_existing_file_opens_a_conflict_instead_of_being_overwritten() {
    let (first, _) = written(None, "v1", None);
    for current in [Some(&b"user edit"[..]), None] {
        let plan = plan_projection("ws_1", Some(&first), &target("v2"), current, T1);
        assert_eq!(plan.write, None);
        assert!(plan.conflict_opened);
        let projection = plan.projection.unwrap();
        assert_eq!(projection.status, ProjectionStatus::Conflict);
        assert_eq!(projection.external_hash, current.map(sha256_hex));
        assert_eq!(plan.events[0].event_type, EVENT_PROJECTION_CONFLICT_DETECTED);
    }
    let plan = plan_projection("ws_1", None, &target("v1"), Some(b"# Our own glossary"), T0);
    assert_eq!(plan.write, None);
    assert!(plan.conflict_opened);
}

#[test]
fn while_in_conflict_nothing_is_written_and_only_a_new_external_edit_is_reported() {
    let (first, _) = written(None, "v1", None);
    let conflict = plan_projection("ws_1", Some(&first), &target("v2"), Some(b"edit 1"), T1).projection.unwrap();
    assert_eq!(plan_projection("ws_1", Some(&conflict), &target("v3"), Some(b"edit 1"), T1), ProjectionPlan::unchanged());
    let again = plan_projection("ws_1", Some(&conflict), &target("v3"), Some(b"edit 2"), T1);
    assert_eq!(again.write, None);
    assert!(!again.conflict_opened, "同一個協調不重複通知");
    assert_eq!(again.events.len(), 1);
}

#[test]
fn reconciling_checks_what_the_user_saw_then_overwrites_or_detaches() {
    let (first, _) = written(None, "v1", None);
    let conflict = plan_projection("ws_1", Some(&first), &target("v2"), Some(b"edit"), T1).projection.unwrap();
    let seen = sha256_hex(b"edit");
    let error = reconcile(&conflict, ReconcileAction::Overwrite, Some(&seen), &target("v2"), Some(b"edit again"), T1);
    assert_eq!(error, Err(UnderstandingError::ProjectionChanged(GLOSSARY_PATH.into())));

    let kept = reconcile(&conflict, ReconcileAction::KeepExternal, Some(&seen), &target("v2"), Some(b"edit"), T1).unwrap();
    assert_eq!(kept.write, None);
    let detached = kept.projection.unwrap();
    assert_eq!(detached.status, ProjectionStatus::Detached);
    // 保留外部版本之後不再寫入，也不再偵測
    assert_eq!(plan_projection("ws_1", Some(&detached), &target("v3"), Some(b"other"), T1), ProjectionPlan::unchanged());
    assert!(reconcile(&detached, ReconcileAction::KeepExternal, Some(&seen), &target("v3"), Some(b"edit"), T1).is_err());

    let overwritten = reconcile(&detached, ReconcileAction::Overwrite, Some(&seen), &target("v3"), Some(b"edit"), T1).unwrap();
    let content = overwritten.write.unwrap();
    assert!(content.contains("第 2 版") && content.ends_with("v3"), "{content}");
    assert_eq!(overwritten.projection.unwrap().status, ProjectionStatus::InSync);
}
