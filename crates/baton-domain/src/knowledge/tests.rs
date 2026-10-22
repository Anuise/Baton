use super::*;

const T0: Timestamp = Timestamp::from_millis(1_000);
const T1: Timestamp = Timestamp::from_millis(2_000);
const T2: Timestamp = Timestamp::from_millis(3_000);

fn content(kind: KnowledgeKind, title: &str) -> KnowledgeContent {
    KnowledgeContent::new(kind, title, "內文", &[], &[], None).unwrap()
}

fn workspace_scope(ws: &str) -> KnowledgeScope {
    KnowledgeScope { kind: ScopeKind::Workspace, workspace_id: Some(ws.into()), goal_id: None, task_id: None }
}

fn agent() -> KnowledgeSource {
    KnowledgeSource::Agent { run_id: "run_1".into(), generation: 1, task_id: "task_1".into() }
}

fn new(id: &str, kind: KnowledgeKind, source: KnowledgeSource) -> NewKnowledge {
    NewKnowledge {
        id: id.into(),
        kind,
        scope: workspace_scope("ws_1"),
        content: content(kind, "標題"),
        source,
        valid_from: None,
        valid_until: None,
    }
}

fn created(id: &str, kind: KnowledgeKind, source: KnowledgeSource) -> KnowledgeEntry {
    create(new(id, kind, source), None, T0).unwrap().entries.remove(0)
}

#[test]
fn user_entries_are_verified_and_agent_lessons_are_always_candidates() {
    let user = created("k1", KnowledgeKind::Decision, KnowledgeSource::User);
    assert_eq!(user.status, KnowledgeStatus::Verified);
    assert!(user.is_effective_at(T0));

    let candidate = created("k2", KnowledgeKind::Lesson, agent());
    assert_eq!(candidate.status, KnowledgeStatus::Candidate);
    assert!(!candidate.is_effective_at(T0), "候選教訓不是有效知識");
}

#[test]
fn agents_cannot_submit_anything_but_lessons() {
    let error = create(new("k1", KnowledgeKind::Decision, agent()), None, T0).unwrap_err();
    assert_eq!(error.code(), "knowledge_invalid");
}

#[test]
fn creation_records_first_version_and_created_event() {
    let change = create(new("k1", KnowledgeKind::Fact, KnowledgeSource::User), None, T0).unwrap();
    let (id, version) = change.new_version.unwrap();
    assert_eq!((id.as_str(), version.version), ("k1", 1));
    assert_eq!(change.events.len(), 1);
    assert_eq!(change.events[0].event_type, EVENT_CREATED);
    assert_eq!(change.events[0].aggregate_version, 1);
}

#[test]
fn confirming_a_candidate_makes_it_verified_and_only_candidates_can_be_confirmed() {
    let candidate = created("k1", KnowledgeKind::Lesson, agent());
    let change = decide(&candidate, 1, KnowledgeCommand::Confirm, T1).unwrap();
    let verified = &change.entries[0];
    assert_eq!(verified.status, KnowledgeStatus::Verified);
    assert_eq!(verified.revision, 2);
    assert!(verified.is_effective_at(T1));

    let error = decide(verified, 2, KnowledgeCommand::Confirm, T2).unwrap_err();
    assert_eq!(error.code(), "knowledge_transition_invalid");
}

#[test]
fn stale_revision_is_rejected_before_anything_else() {
    let candidate = created("k1", KnowledgeKind::Lesson, agent());
    let error = decide(&candidate, 7, KnowledgeCommand::Confirm, T1).unwrap_err();
    assert_eq!(error, KnowledgeError::RevisionConflict { current: 1 });
}

#[test]
fn revoking_keeps_the_entry_with_reason_but_it_is_no_longer_effective() {
    let entry = created("k1", KnowledgeKind::Lesson, KnowledgeSource::User);
    let change = decide(&entry, 1, KnowledgeCommand::Revoke { reason: "  結論錯誤 ".into() }, T1).unwrap();
    let revoked = &change.entries[0];
    assert_eq!(revoked.status, KnowledgeStatus::Revoked);
    assert_eq!(revoked.status_reason.as_deref(), Some("結論錯誤"));
    assert_eq!(revoked.content, entry.content, "撤銷不改變內容");
    assert!(!revoked.is_effective_at(T1));
    assert_eq!(change.events[0].event_type, EVENT_REVOKED);

    // 撤銷之後不能再確認、修正或淘汰
    for command in [
        KnowledgeCommand::Confirm,
        KnowledgeCommand::Retire { reason: "x".into() },
        KnowledgeCommand::Revoke { reason: "x".into() },
    ] {
        assert_eq!(decide(revoked, 2, command, T2).unwrap_err().code(), "knowledge_transition_invalid");
    }
}

#[test]
fn revoke_and_retire_require_a_reason() {
    let entry = created("k1", KnowledgeKind::Fact, KnowledgeSource::User);
    let error = decide(&entry, 1, KnowledgeCommand::Revoke { reason: "  ".into() }, T1).unwrap_err();
    assert_eq!(error.code(), "knowledge_invalid");
    let error = decide(&entry, 1, KnowledgeCommand::Retire { reason: "".into() }, T1).unwrap_err();
    assert_eq!(error.code(), "knowledge_invalid");
}

#[test]
fn only_verified_entries_can_be_retired() {
    let candidate = created("k1", KnowledgeKind::Lesson, agent());
    let error = decide(&candidate, 1, KnowledgeCommand::Retire { reason: "過時".into() }, T1).unwrap_err();
    assert_eq!(error.code(), "knowledge_transition_invalid");

    let verified = created("k2", KnowledgeKind::Fact, KnowledgeSource::User);
    let retired = decide(&verified, 1, KnowledgeCommand::Retire { reason: "過時".into() }, T1).unwrap().entries.remove(0);
    assert_eq!(retired.status, KnowledgeStatus::Retired);
    assert!(!retired.is_effective_at(T1));
}

#[test]
fn correction_appends_a_new_version_and_keeps_status() {
    let candidate = created("k1", KnowledgeKind::Lesson, agent());
    let corrected_content = content(KnowledgeKind::Lesson, "修正後的標題");
    let change = decide(
        &candidate,
        1,
        KnowledgeCommand::Correct { content: corrected_content.clone(), valid_until: None, note: Some("補充".into()) },
        T1,
    )
    .unwrap();
    let entry = &change.entries[0];
    assert_eq!(entry.status, KnowledgeStatus::Candidate, "修正候選不會讓它變成已驗證");
    assert_eq!(entry.version, 2);
    assert_eq!(entry.content, corrected_content);
    let (_, version) = change.new_version.unwrap();
    assert_eq!((version.version, version.note.as_deref()), (2, Some("補充")));
}

#[test]
fn correction_without_any_change_is_rejected() {
    let entry = created("k1", KnowledgeKind::Fact, KnowledgeSource::User);
    let command = KnowledgeCommand::Correct { content: entry.content.clone(), valid_until: None, note: None };
    assert_eq!(decide(&entry, 1, command, T1).unwrap_err().code(), "knowledge_invalid");
}

#[test]
fn changing_only_the_validity_does_not_add_a_content_version() {
    let entry = created("k1", KnowledgeKind::Fact, KnowledgeSource::User);
    let command = KnowledgeCommand::Correct { content: entry.content.clone(), valid_until: Some(T2), note: None };
    let change = decide(&entry, 1, command, T1).unwrap();
    assert_eq!(change.entries[0].version, 1);
    assert_eq!(change.entries[0].valid_until, Some(T2));
    assert!(change.new_version.is_none());
    assert!(change.entries[0].is_effective_at(T1));
    assert!(!change.entries[0].is_effective_at(T2), "有效期限的結束時間不含在內");
}

#[test]
fn validity_window_controls_effectiveness() {
    let mut input = new("k1", KnowledgeKind::Decision, KnowledgeSource::User);
    input.valid_from = Some(T1);
    input.valid_until = Some(T2);
    let entry = create(input, None, T0).unwrap().entries.remove(0);
    assert!(!entry.is_effective_at(T0), "尚未生效");
    assert!(entry.is_effective_at(T1));
    assert!(!entry.is_effective_at(T2), "已過期");

    let mut backwards = new("k2", KnowledgeKind::Decision, KnowledgeSource::User);
    backwards.valid_from = Some(T2);
    backwards.valid_until = Some(T1);
    assert_eq!(create(backwards, None, T0).unwrap_err().code(), "knowledge_invalid");
}

#[test]
fn superseding_marks_the_old_entry_and_links_both_ways() {
    let old = created("old", KnowledgeKind::Decision, KnowledgeSource::User);
    let change = create(new("new", KnowledgeKind::Decision, KnowledgeSource::User), Some(&old), T1).unwrap();
    assert_eq!(change.entries.len(), 2);
    let (fresh, replaced) = (&change.entries[0], &change.entries[1]);
    assert_eq!(fresh.supersedes.as_deref(), Some("old"));
    assert_eq!(replaced.status, KnowledgeStatus::Superseded);
    assert_eq!(replaced.superseded_by.as_deref(), Some("new"));
    assert_eq!(replaced.revision, 2);
    assert!(!replaced.is_effective_at(T1));
    let types: Vec<_> = change.events.iter().map(|e| e.event_type).collect();
    assert_eq!(types, [EVENT_CREATED, EVENT_SUPERSEDED]);
}

#[test]
fn a_replacement_cannot_have_a_narrower_scope_than_the_entry_it_supersedes() {
    // R151 第 25 項：在 Workspace B 建一個 B 範圍的條目，不能讓全域教訓對所有 Workspace 失效
    let mut global = new("g", KnowledgeKind::Lesson, KnowledgeSource::User);
    global.scope = KnowledgeScope::global();
    let global = create(global, None, T0).unwrap().entries.remove(0);
    let error = create(new("narrow", KnowledgeKind::Lesson, KnowledgeSource::User), Some(&global), T1).unwrap_err();
    assert_eq!(error.code(), "knowledge_supersede_invalid");

    let other_workspace = created("w1", KnowledgeKind::Lesson, KnowledgeSource::User);
    let mut elsewhere = new("w2", KnowledgeKind::Lesson, KnowledgeSource::User);
    elsewhere.scope = workspace_scope("ws_2");
    assert_eq!(create(elsewhere, Some(&other_workspace), T1).unwrap_err().code(), "knowledge_supersede_invalid");

    let task = KnowledgeScope {
        kind: ScopeKind::Task,
        workspace_id: Some("ws_1".into()),
        goal_id: Some("goal_1".into()),
        task_id: Some("task_1".into()),
    };
    let mut task_entry = new("t", KnowledgeKind::Lesson, KnowledgeSource::User);
    task_entry.scope = task.clone();
    let task_entry = create(task_entry, None, T0).unwrap().entries.remove(0);
    let workspace = workspace_scope("ws_1");
    assert!(workspace.covers(&task) && !task.covers(&workspace));
    // 同範圍或更寬的範圍可以取代
    assert!(create(new("wide", KnowledgeKind::Lesson, KnowledgeSource::User), Some(&task_entry), T1).is_ok());
    let mut wider = new("wider", KnowledgeKind::Lesson, KnowledgeSource::User);
    wider.scope = KnowledgeScope::global();
    assert!(create(wider, Some(&global), T1).is_ok());
}

#[test]
fn only_verified_entries_of_the_same_kind_can_be_superseded_and_only_by_users() {
    let candidate = created("c", KnowledgeKind::Lesson, agent());
    let error = create(new("n1", KnowledgeKind::Lesson, KnowledgeSource::User), Some(&candidate), T1).unwrap_err();
    assert_eq!(error.code(), "knowledge_supersede_invalid");

    let decision = created("d", KnowledgeKind::Decision, KnowledgeSource::User);
    let error = create(new("n2", KnowledgeKind::Fact, KnowledgeSource::User), Some(&decision), T1).unwrap_err();
    assert_eq!(error.code(), "knowledge_supersede_invalid");

    let lesson = created("l", KnowledgeKind::Lesson, KnowledgeSource::User);
    let error = create(new("n3", KnowledgeKind::Lesson, agent()), Some(&lesson), T1).unwrap_err();
    assert_eq!(error.code(), "knowledge_supersede_invalid");
}

#[test]
fn content_is_trimmed_and_validated() {
    let tags = vec![" 測試 ".to_string(), "測試".to_string(), "".to_string()];
    let content = KnowledgeContent::new(KnowledgeKind::Fact, "  標題 ", " 內文 ", &tags, &[], None).unwrap();
    assert_eq!((content.title.as_str(), content.body.as_str()), ("標題", "內文"));
    assert_eq!(content.tags, ["測試"], "去除空白、空字串與重複");

    let empty = KnowledgeContent::new(KnowledgeKind::Fact, "  ", "", &[], &[], None).unwrap_err();
    assert_eq!(empty.code(), "knowledge_invalid");
    let long = "字".repeat(TITLE_MAX_CHARS + 1);
    assert!(KnowledgeContent::new(KnowledgeKind::Fact, &long, "", &[], &[], None).is_err());
    let lesson_on_fact = KnowledgeContent::new(KnowledgeKind::Fact, "t", "", &[], &[], Some(LessonDetail::default()));
    assert!(lesson_on_fact.is_err(), "只有教訓可以有教訓欄位");
    let lesson = KnowledgeContent::new(KnowledgeKind::Lesson, "t", "", &[], &[], None).unwrap();
    assert_eq!(lesson.lesson, Some(LessonDetail::default()), "教訓一律帶教訓欄位");
}

#[test]
fn search_text_covers_every_text_field() {
    let detail = LessonDetail { trigger: "觸發".into(), root_cause: "根因".into(), resolution: "解法".into() };
    let content = KnowledgeContent::new(
        KnowledgeKind::Lesson,
        "標題",
        "內文",
        &["標籤".to_string()],
        &["證據".to_string()],
        Some(detail),
    )
    .unwrap();
    let text = content.search_text();
    for part in ["標題", "內文", "觸發", "根因", "解法", "標籤", "證據"] {
        assert!(text.contains(part), "{part} 應該在檢索文字中：{text}");
    }
}

#[test]
fn query_terms_need_at_least_two_characters() {
    assert_eq!(parse_query("  ").unwrap(), Vec::<String>::new());
    assert_eq!(parse_query("快取 逾時 快取").unwrap(), ["快取", "逾時"]);
    assert_eq!(parse_query("快").unwrap_err().code(), "knowledge_query_invalid");
    assert_eq!(parse_query("快取 a").unwrap_err().code(), "knowledge_query_invalid");
    assert!(parse_query(&"字".repeat(QUERY_MAX_CHARS + 1)).is_err());
    let many = (0..=QUERY_MAX_TERMS).map(|i| format!("詞{i}")).collect::<Vec<_>>().join(" ");
    assert!(parse_query(&many).is_err());
}

#[test]
fn runs_see_global_their_workspace_goal_and_task_only() {
    let run = Audience::Run { workspace_id: "ws_1", goal_id: "goal_1", task_id: "task_1" };
    let scope = |kind, ws: Option<&str>, goal: Option<&str>, task: Option<&str>| KnowledgeScope {
        kind,
        workspace_id: ws.map(Into::into),
        goal_id: goal.map(Into::into),
        task_id: task.map(Into::into),
    };
    assert!(run.can_see(&KnowledgeScope::global()));
    assert!(run.can_see(&scope(ScopeKind::Workspace, Some("ws_1"), None, None)));
    assert!(run.can_see(&scope(ScopeKind::Goal, Some("ws_1"), Some("goal_1"), None)));
    assert!(run.can_see(&scope(ScopeKind::Task, Some("ws_1"), Some("goal_1"), Some("task_1"))));
    assert!(!run.can_see(&scope(ScopeKind::Workspace, Some("ws_2"), None, None)));
    assert!(!run.can_see(&scope(ScopeKind::Goal, Some("ws_1"), Some("goal_2"), None)));
    assert!(!run.can_see(&scope(ScopeKind::Task, Some("ws_1"), Some("goal_1"), Some("task_2"))));

    let desktop = Audience::Desktop { workspace_id: Some("ws_1") };
    assert!(desktop.can_see(&scope(ScopeKind::Task, Some("ws_1"), Some("goal_9"), Some("task_9"))));
    assert!(!desktop.can_see(&scope(ScopeKind::Workspace, Some("ws_2"), None, None)));
    let global_only = Audience::Desktop { workspace_id: None };
    assert!(global_only.can_see(&KnowledgeScope::global()));
    assert!(!global_only.can_see(&scope(ScopeKind::Workspace, Some("ws_1"), None, None)));
}
