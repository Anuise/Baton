use super::*;
use crate::handoff::HandoffPhase;
use crate::knowledge::{KnowledgeContent, KnowledgeEntry, KnowledgeKind, KnowledgeScope, KnowledgeSource, KnowledgeStatus, LessonDetail, ScopeKind};
use crate::message::MessageKind;
use crate::task::{Task, TaskStatus};
use crate::workdir::TaskKind;

const T0: Timestamp = Timestamp::from_millis(1_000_000);
const CONTRACT: &str = "Task：部署\n\n目標：部署新版";

fn task() -> Task {
    Task {
        id: "task_1".into(),
        workspace_id: "ws_1".into(),
        goal_id: "goal_1".into(),
        title: "部署".into(),
        kind: TaskKind::default(),
        status: TaskStatus::Running,
        version: 3,
        contract_version: 2,
        created_at: T0,
        updated_at: T0,
    }
}

fn scope(kind: ScopeKind) -> KnowledgeScope {
    let ws = (kind != ScopeKind::Global).then(|| "ws_1".to_string());
    let goal = matches!(kind, ScopeKind::Goal | ScopeKind::Task).then(|| "goal_1".to_string());
    let task = (kind == ScopeKind::Task).then(|| "task_1".to_string());
    KnowledgeScope { kind, workspace_id: ws, goal_id: goal, task_id: task }
}

fn entry(id: &str, kind: KnowledgeKind, scope_kind: ScopeKind, title: &str, text: &str) -> KnowledgeEntry {
    let lesson = (kind == KnowledgeKind::Lesson).then(|| LessonDetail {
        trigger: format!("{title}的觸發"),
        root_cause: "根因不放進摘要".into(),
        resolution: text.into(),
    });
    let body = if kind == KnowledgeKind::Lesson { String::new() } else { text.to_string() };
    KnowledgeEntry {
        id: id.into(),
        kind,
        scope: scope(scope_kind),
        status: KnowledgeStatus::Verified,
        source: KnowledgeSource::User,
        valid_from: T0,
        valid_until: None,
        version: 1,
        revision: 1,
        supersedes: None,
        superseded_by: None,
        status_reason: None,
        content: KnowledgeContent { title: title.into(), body, tags: vec![], evidence: vec![], lesson },
        created_at: T0,
        updated_at: T0,
    }
}

fn lesson(id: &str, scope_kind: ScopeKind, title: &str, resolution: &str) -> KnowledgeEntry {
    entry(id, KnowledgeKind::Lesson, scope_kind, title, resolution)
}

fn inputs<'a>(task: &'a Task, knowledge: &'a [KnowledgeEntry]) -> ContextInputs<'a> {
    ContextInputs {
        run_id: "run_1",
        task,
        contract: CONTRACT,
        handoff: None,
        checkpoint: None,
        knowledge,
        unread: &[],
        unread_total: 0,
        evidence: &[],
        budget_tokens: DEFAULT_BUDGET_TOKENS,
        now: T0.plus_millis(10),
    }
}

fn loaded(receipt: &ContextReceipt) -> Vec<&str> {
    receipt.items.iter().filter(|i| i.status == ItemStatus::Loaded).map(|i| i.ref_id.as_str()).collect()
}

fn omitted(receipt: &ContextReceipt) -> Vec<&str> {
    receipt.items.iter().filter(|i| i.status == ItemStatus::Omitted).map(|i| i.ref_id.as_str()).collect()
}

#[test]
fn tokens_are_estimated_as_one_per_cjk_character_and_a_quarter_per_ascii_character() {
    assert_eq!(estimate_tokens(""), 0);
    assert_eq!(estimate_tokens("教訓"), 2);
    assert_eq!(estimate_tokens("abcd"), 1);
    assert_eq!(estimate_tokens("abcde"), 2);
    assert_eq!(estimate_tokens("ab教訓"), 3);
}

#[test]
fn the_contract_is_always_loaded_first_with_its_version() {
    let task = task();
    let built = build(&inputs(&task, &[]));
    assert!(built.prompt.starts_with(CONTRACT));
    let first = &built.receipt.items[0];
    assert_eq!((first.kind, first.ref_id.as_str(), first.version), (ItemKind::Contract, "task_1", Some(2)));
    assert_eq!(first.status, ItemStatus::Loaded);
    assert_eq!(built.receipt.run_id, "run_1");
    assert_eq!(built.receipt.budget_tokens, DEFAULT_BUDGET_TOKENS);
    assert_eq!(built.receipt.used_tokens, estimate_tokens(&built.prompt));
}

#[test]
fn verified_lessons_that_apply_are_loaded_with_trigger_and_resolution_but_not_root_cause() {
    let task = task();
    let knowledge = [lesson("les_1", ScopeKind::Workspace, "部署前要寫回滾計畫", "先寫回滾步驟")];
    let built = build(&inputs(&task, &knowledge));
    assert!(built.prompt.contains("〔les_1 v1〕部署前要寫回滾計畫"), "{}", built.prompt);
    assert!(built.prompt.contains("先寫回滾步驟"));
    assert!(built.prompt.contains("部署前要寫回滾計畫的觸發"));
    assert!(!built.prompt.contains("根因不放進摘要"));
    assert_eq!(loaded(&built.receipt), ["task_1", "les_1"]);
}

#[test]
fn candidate_expired_and_out_of_scope_entries_are_never_loaded() {
    let task = task();
    let mut candidate = lesson("les_candidate", ScopeKind::Workspace, "候選", "候選做法");
    candidate.status = KnowledgeStatus::Candidate;
    let mut revoked = lesson("les_revoked", ScopeKind::Workspace, "撤銷", "撤銷做法");
    revoked.status = KnowledgeStatus::Revoked;
    let mut expired = lesson("les_expired", ScopeKind::Workspace, "過期", "過期做法");
    expired.valid_until = Some(T0);
    let mut other_task = lesson("les_other", ScopeKind::Task, "別的 Task", "別的做法");
    other_task.scope.task_id = Some("task_2".into());
    let mut fact = entry("fact_1", KnowledgeKind::Fact, ScopeKind::Workspace, "事實", "一般知識");
    fact.status = KnowledgeStatus::Verified;
    let knowledge = [candidate, revoked, expired, other_task, fact];
    let built = build(&inputs(&task, &knowledge));
    assert_eq!(loaded(&built.receipt), ["task_1"]);
    assert!(omitted(&built.receipt).is_empty(), "不適用的條目不是被省略，而是根本不是候選");
    for text in ["候選做法", "撤銷做法", "過期做法", "別的做法", "一般知識"] {
        assert!(!built.prompt.contains(text), "{text}");
    }
}

#[test]
fn decisions_come_before_lessons_and_more_specific_scopes_come_first() {
    let task = task();
    let knowledge = [
        lesson("les_global", ScopeKind::Global, "全域教訓", "全域"),
        lesson("les_task", ScopeKind::Task, "Task 教訓", "Task"),
        entry("dec_ws", KnowledgeKind::Decision, ScopeKind::Workspace, "Workspace 決策", "用 SQLite"),
        lesson("les_goal", ScopeKind::Goal, "Goal 教訓", "Goal"),
    ];
    let built = build(&inputs(&task, &knowledge));
    assert_eq!(loaded(&built.receipt), ["task_1", "dec_ws", "les_task", "les_goal", "les_global"]);
    let at = |text: &str| built.prompt.find(text).unwrap();
    assert!(at("適用決策") < at("相關教訓"));
    assert!(at("Task 教訓") < at("Goal 教訓") && at("Goal 教訓") < at("全域教訓"));
}

#[test]
fn entries_whose_tags_appear_in_the_task_rank_above_others_in_the_same_scope() {
    let task = task();
    let plain = lesson("les_plain", ScopeKind::Workspace, "一般", "一般");
    let mut tagged = lesson("les_tagged", ScopeKind::Workspace, "標籤相符", "相符");
    tagged.content.tags = vec!["部署".into()];
    tagged.updated_at = T0.plus_millis(-1);
    let knowledge = [plain, tagged];
    let built = build(&inputs(&task, &knowledge));
    assert_eq!(loaded(&built.receipt), ["task_1", "les_tagged", "les_plain"]);
}

#[test]
fn items_that_do_not_fit_the_budget_are_omitted_and_recorded_while_smaller_ones_still_load() {
    let task = task();
    let big = "很".repeat(3_000);
    let knowledge = [
        lesson("les_task", ScopeKind::Task, "Task 範圍的大教訓", &big),
        lesson("les_ws_big", ScopeKind::Workspace, "Workspace 的大教訓", &big),
        lesson("les_ws_small", ScopeKind::Workspace, "小教訓", "短"),
    ];
    let mut input = inputs(&task, &knowledge);
    input.budget_tokens = 5_000;
    let built = build(&input);
    assert_eq!(loaded(&built.receipt), ["task_1", "les_task", "les_ws_small"]);
    assert_eq!(omitted(&built.receipt), ["les_ws_big"]);
    let skipped = built.receipt.items.iter().find(|i| i.ref_id == "les_ws_big").unwrap();
    assert_eq!(skipped.omitted_reason, Some(OmitReason::BudgetExceeded));
    assert!(skipped.tokens > 3_000);
    assert!(!built.prompt.contains("Workspace 的大教訓"));
    assert!(built.receipt.used_tokens <= 5_000);
    assert_eq!(built.receipt.used_tokens, estimate_tokens(&built.prompt));
}

#[test]
fn the_contract_and_handoff_are_kept_even_when_they_exceed_the_budget() {
    let task = task();
    let handoff = handoff();
    let checkpoint = checkpoint();
    let knowledge = [lesson("les_1", ScopeKind::Workspace, "教訓", "做法")];
    let mut input = inputs(&task, &knowledge);
    input.budget_tokens = 10;
    input.handoff = Some(&handoff);
    input.checkpoint = Some(&checkpoint);
    let built = build(&input);
    assert!(built.prompt.contains(CONTRACT) && built.prompt.contains("交接："));
    assert_eq!(loaded(&built.receipt), ["task_1", "handoff_1", "run_0"]);
    assert_eq!(omitted(&built.receipt), ["les_1"]);
    assert!(built.receipt.used_tokens > 10);
}

fn handoff() -> Handoff {
    Handoff {
        id: "handoff_1".into(),
        workspace_id: "ws_1".into(),
        task_id: "task_1".into(),
        from_run_id: "run_0".into(),
        from_profile_id: "p1".into(),
        from_generation: 1,
        to_profile_id: "p2".into(),
        to_run_id: None,
        to_generation: None,
        phase: HandoffPhase::Stopping,
        reason: "five_hour".into(),
        checkpoint: None,
        stop_outcome: None,
        confirmation: None,
        abandoned_reason: None,
        requested_at: T0,
        stopping_at: None,
        transferred_at: None,
        confirmed_at: None,
        waiting_at: None,
        next_check_at: None,
        wait_attempts: 0,
        wait_reset_at: None,
        wait_reasons: vec![],
        updated_at: T0,
        version: 2,
    }
}

fn checkpoint() -> CheckpointRef {
    CheckpointRef { run_id: "run_0".into(), generation: 1, summary: "完成步驟二".into(), details: "細節".into(), saved_at: T0 }
}

#[test]
fn a_handoff_carries_its_checkpoint_inside_the_handoff_note() {
    let task = task();
    let handoff = handoff();
    let checkpoint = checkpoint();
    let mut input = inputs(&task, &[]);
    input.handoff = Some(&handoff);
    input.checkpoint = Some(&checkpoint);
    let built = build(&input);
    assert_eq!(built.prompt.matches("完成步驟二").count(), 1, "檢查點只出現一次：{}", built.prompt);
    let item = built.receipt.items.iter().find(|i| i.kind == ItemKind::Checkpoint).unwrap();
    assert_eq!((item.ref_id.as_str(), item.version, item.status), ("run_0", Some(1), ItemStatus::Loaded));
}

#[test]
fn without_a_handoff_the_latest_checkpoint_is_loaded_on_its_own() {
    let task = task();
    let checkpoint = checkpoint();
    let mut input = inputs(&task, &[]);
    input.checkpoint = Some(&checkpoint);
    let built = build(&input);
    assert!(built.prompt.contains("最近的檢查點") && built.prompt.contains("完成步驟二") && built.prompt.contains("細節"));
    assert!(!built.prompt.contains("交接："));
    assert_eq!(loaded(&built.receipt), ["task_1", "run_0"]);
}

#[test]
fn unread_messages_and_evidence_are_listed_after_knowledge() {
    let task = task();
    let unread = [UnreadSummaryItem {
        message_id: "msg_1".into(),
        kind: MessageKind::Note,
        subject: "介面改了".into(),
        from_task_title: "後端".into(),
    }];
    let evidence = [EvidenceItem {
        artifact_id: "art_1".into(),
        name: "report.md".into(),
        task_title: "調查".into(),
        version: 3,
        sha256: "a".repeat(64),
        description: "調查報告".into(),
    }];
    let knowledge = [lesson("les_1", ScopeKind::Workspace, "教訓", "做法")];
    let mut input = inputs(&task, &knowledge);
    input.unread = &unread;
    input.unread_total = 1;
    input.evidence = &evidence;
    let built = build(&input);
    assert!(built.messages_loaded);
    assert_eq!(loaded(&built.receipt), ["task_1", "les_1", "inbox", "art_1"]);
    let at = |text: &str| built.prompt.find(text).unwrap();
    assert!(at("相關教訓") < at("未讀訊息：1 則") && at("未讀訊息：1 則") < at("證據索引"));
    assert!(built.prompt.contains("〔art_1 v3〕report.md"));
    let art = built.receipt.items.iter().find(|i| i.ref_id == "art_1").unwrap();
    assert_eq!((art.kind, art.version), (ItemKind::Evidence, Some(3)));
}

#[test]
fn an_omitted_message_summary_is_not_marked_as_delivered() {
    let task = task();
    let unread = [UnreadSummaryItem {
        message_id: "msg_1".into(),
        kind: MessageKind::Note,
        subject: "介面改了".into(),
        from_task_title: "後端".into(),
    }];
    let mut input = inputs(&task, &[]);
    input.unread = &unread;
    input.unread_total = 1;
    input.budget_tokens = estimate_tokens(CONTRACT);
    let built = build(&input);
    assert!(!built.messages_loaded);
    assert_eq!(omitted(&built.receipt), ["inbox"]);
}

#[test]
fn the_receipt_records_the_version_actually_rendered_and_the_prompt_hash() {
    let task = task();
    let mut corrected = lesson("les_1", ScopeKind::Workspace, "修正後的標題", "修正後的做法");
    corrected.version = 2;
    let knowledge = [corrected];
    let built = build(&inputs(&task, &knowledge));
    let item = built.receipt.items.iter().find(|i| i.ref_id == "les_1").unwrap();
    assert_eq!((item.version, item.title.as_str()), (Some(2), "修正後的標題"));
    assert!(built.prompt.contains("〔les_1 v2〕修正後的標題"));
    assert_eq!(built.receipt.prompt_sha256, crate::artifact::sha256_hex(built.prompt.as_bytes()));
}
