//! Context Builder 與 context receipt（票 39，spec D-07、D-27、使用者故事 61）。
//!
//! - 派工當下為 Worker Run 組裝 Prompt：Task 契約、目前的 Workflow Stage（票 43）、交接說明或最近檢查點、適用決策、相關教訓、未讀訊息摘要與證據索引，
//!   依這個順序排定優先序；Task 契約與交接說明是必要項目，一定載入（即使超出預算）。
//! - 決策與教訓只取「有效且適用於這個 Run」的條目（已驗證、在有效期間內、範圍涵蓋這個 Task）；
//!   依相關性排序：範圍越具體越前面（Task > Goal > Workspace > 全域），同範圍內標籤出現在 Task 標題或契約中的較前面，再依最後更新時間由新到舊。
//! - 依優先序逐項放入，放不進 token 預算的項目省略並記錄原因，後面較小的項目仍可放入。
//! - context receipt 記下每一項的來源、版本、估計 token 數與載入或省略，以及預算、實際用量與 Prompt 的雜湊；
//!   Prompt 與 receipt 由同一次組裝產生，所以 receipt 列的版本就是 Prompt 實際載入的內容。

use std::cmp::Reverse;

use serde::{Deserialize, Serialize};

use crate::handoff::{self, CheckpointRef, Handoff};
use crate::knowledge::{Audience, KnowledgeEntry, KnowledgeKind, ScopeKind};
use crate::message::{self, UnreadSummaryItem};
use crate::task::Task;
use crate::Timestamp;

/// 每個 Run 的 context 預算（估計的 token 數）。
pub const DEFAULT_BUDGET_TOKENS: usize = 8_000;

const DECISIONS_HEADER: &str = "\n\n適用決策（已確認，必須遵守）：";
const LESSONS_HEADER: &str = "\n\n相關教訓（已驗證，避免重犯）：";
const EVIDENCE_HEADER: &str = "\n\n證據索引（已登記的成果，需要時讀取）：";
/// 未讀訊息摘要在 receipt 中的識別（摘要是一個整體）。
pub const INBOX_REF: &str = "inbox";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    Contract,
    /// Task 所在 Workflow 的目前 Stage（票 43）。
    Stage,
    Handoff,
    Checkpoint,
    Decision,
    Lesson,
    Messages,
    Evidence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemStatus {
    Loaded,
    Omitted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OmitReason {
    /// 放入後會超出 token 預算。
    BudgetExceeded,
}

/// receipt 中的一項。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextItem {
    pub kind: ItemKind,
    /// 來源識別：Task、交接、保存檢查點的 Run、知識條目、成果；未讀訊息摘要是 [`INBOX_REF`]。
    pub ref_id: String,
    /// 載入的版本：契約版本、知識條目的內容版本、成果版本、檢查點的世代；沒有版本的是 `None`。
    pub version: Option<i64>,
    pub title: String,
    /// 這一項文字的估計 token 數；交接時的檢查點包含在交接說明中，記為 0。
    pub tokens: usize,
    pub status: ItemStatus,
    pub omitted_reason: Option<OmitReason>,
}

/// 一個 Run 的 context receipt：依優先序列出全部候選項目。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextReceipt {
    pub run_id: String,
    pub task_id: String,
    pub built_at: Timestamp,
    pub budget_tokens: usize,
    /// 整個 Prompt 的估計 token 數。
    pub used_tokens: usize,
    /// Prompt 的 SHA-256（小寫十六進位），用來核對 receipt 對應的就是實際交給 Worker 的 Prompt。
    pub prompt_sha256: String,
    pub items: Vec<ContextItem>,
}

/// 證據索引的一項：已登記成果的最新版本。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceItem {
    pub artifact_id: String,
    pub name: String,
    /// 產生這個成果的 Task（這個 Task 或它依賴的 Task）。
    pub task_title: String,
    pub version: i64,
    pub sha256: String,
    pub description: String,
}

/// Task 所在 Workflow 的目前 Stage（票 43）：必要項目，緊接在契約之後。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageItem {
    pub workflow_id: String,
    /// Workflow 的 revision。
    pub revision: i64,
    pub title: String,
    pub text: String,
}

/// 組裝需要的全部輸入（由 Store 在派工的交易中取得）。
#[derive(Debug, Clone)]
pub struct ContextInputs<'a> {
    pub run_id: &'a str,
    pub task: &'a Task,
    /// 派工時固定的 Task 契約文字（`run` 模組產生）。
    pub contract: &'a str,
    /// Task 未完成的 Workflow 的目前 Stage；換帳號交接後 successor 收到的是同一個 Stage。
    pub stage: Option<&'a StageItem>,
    /// successor Run 承接的交接；有交接時檢查點放在交接說明中。
    pub handoff: Option<&'a Handoff>,
    /// Task 最近一筆被接受的檢查點。
    pub checkpoint: Option<&'a CheckpointRef>,
    /// Workspace 與全域的知識條目；不適用、無效的條目在這裡過濾掉。
    pub knowledge: &'a [KnowledgeEntry],
    pub unread: &'a [UnreadSummaryItem],
    pub unread_total: usize,
    pub evidence: &'a [EvidenceItem],
    pub budget_tokens: usize,
    pub now: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuiltContext {
    pub prompt: String,
    pub receipt: ContextReceipt,
    /// 未讀訊息摘要是否放進了 Prompt（放進了才把列出的訊息標為已交付）。
    pub messages_loaded: bool,
}

/// 估計 token 數：非 ASCII 字元（含中文）每字 1，ASCII 每 4 個字元 1（無條件進位）。
pub fn estimate_tokens(text: &str) -> usize {
    let ascii = text.chars().filter(char::is_ascii).count();
    ascii.div_ceil(4) + (text.chars().count() - ascii)
}

/// 一個候選項目與它在 Prompt 中的文字。
struct Candidate {
    item: ContextItem,
    text: String,
}

fn candidate(kind: ItemKind, ref_id: &str, version: Option<i64>, title: &str, text: String) -> Candidate {
    let item = ContextItem {
        kind,
        ref_id: ref_id.to_string(),
        version,
        title: title.to_string(),
        tokens: estimate_tokens(&text),
        status: ItemStatus::Loaded,
        omitted_reason: None,
    };
    Candidate { item, text }
}

pub fn build(inputs: &ContextInputs<'_>) -> BuiltContext {
    let task = inputs.task;
    let mut prompt = inputs.contract.to_string();
    let mut items = vec![candidate(ItemKind::Contract, &task.id, Some(task.contract_version), &task.title, prompt.clone()).item];
    if let Some(stage) = inputs.stage {
        items.push(candidate(ItemKind::Stage, &stage.workflow_id, Some(stage.revision), &stage.title, stage.text.clone()).item);
        prompt.push_str(&stage.text);
    }

    // 交接說明是必要項目，包含要延續的檢查點；沒有交接時檢查點是第一個依預算放入的項目
    let mut sections: Vec<(&str, Vec<Candidate>)> = Vec::new();
    match (inputs.handoff, inputs.checkpoint) {
        (Some(h), checkpoint) => {
            let note = handoff::successor_prompt(h, checkpoint);
            items.push(candidate(ItemKind::Handoff, &h.id, Some(h.version), &format!("自 Run {} 交接", h.from_run_id), note.clone()).item);
            prompt.push_str(&note);
            if let Some(c) = checkpoint {
                // 文字已在交接說明中，這一項只記錄延續了哪個檢查點（0 token）
                items.push(candidate(ItemKind::Checkpoint, &c.run_id, Some(c.generation), &c.summary, String::new()).item);
            }
        }
        (None, Some(c)) => {
            let mut text = format!("\n\n最近的檢查點（Run {}，第 {} 世代保存）：{}", c.run_id, c.generation, c.summary);
            if !c.details.trim().is_empty() {
                text.push_str(&format!("\n檢查點細節：\n{}", c.details));
            }
            sections.push(("", vec![candidate(ItemKind::Checkpoint, &c.run_id, Some(c.generation), &c.summary, text)]));
        }
        (None, None) => {}
    }

    let relevant = relevant_knowledge(inputs);
    let of_kind = |kind: KnowledgeKind| relevant.iter().copied().filter(move |e| e.kind == kind);
    sections.push((DECISIONS_HEADER, of_kind(KnowledgeKind::Decision).map(decision_candidate).collect()));
    sections.push((LESSONS_HEADER, of_kind(KnowledgeKind::Lesson).map(lesson_candidate).collect()));
    if inputs.unread_total > 0 {
        let text = message::render_unread_summary(inputs.unread, inputs.unread_total);
        let title = format!("未讀訊息 {} 則", inputs.unread_total);
        sections.push(("", vec![candidate(ItemKind::Messages, INBOX_REF, None, &title, text)]));
    }
    sections.push((EVIDENCE_HEADER, inputs.evidence.iter().map(evidence_candidate).collect()));

    let mut messages_loaded = false;
    for (header, candidates) in sections {
        let mut header_written = false;
        for Candidate { mut item, text } in candidates {
            let header_text = if header_written { "" } else { header };
            if estimate_tokens(&format!("{prompt}{header_text}{text}")) <= inputs.budget_tokens {
                prompt.push_str(header_text);
                prompt.push_str(&text);
                header_written = true;
                messages_loaded |= item.kind == ItemKind::Messages;
            } else {
                item.status = ItemStatus::Omitted;
                item.omitted_reason = Some(OmitReason::BudgetExceeded);
            }
            items.push(item);
        }
    }

    let receipt = ContextReceipt {
        run_id: inputs.run_id.to_string(),
        task_id: task.id.clone(),
        built_at: inputs.now,
        budget_tokens: inputs.budget_tokens,
        used_tokens: estimate_tokens(&prompt),
        prompt_sha256: crate::artifact::sha256_hex(prompt.as_bytes()),
        items,
    };
    BuiltContext { prompt, receipt, messages_loaded }
}

/// 有效、適用於這個 Run 的決策與教訓，依相關性排序。
fn relevant_knowledge<'a>(inputs: &ContextInputs<'a>) -> Vec<&'a KnowledgeEntry> {
    let task = inputs.task;
    let audience = Audience::Run { workspace_id: &task.workspace_id, goal_id: &task.goal_id, task_id: &task.id };
    let haystack = format!("{}\n{}", task.title, inputs.contract);
    let mut entries: Vec<&KnowledgeEntry> = inputs
        .knowledge
        .iter()
        .filter(|e| matches!(e.kind, KnowledgeKind::Decision | KnowledgeKind::Lesson))
        .filter(|e| e.is_effective_at(inputs.now) && audience.can_see(&e.scope))
        .collect();
    entries.sort_by_key(|e| {
        let specificity = match e.scope.kind {
            ScopeKind::Task => 3,
            ScopeKind::Goal => 2,
            ScopeKind::Workspace => 1,
            ScopeKind::Global => 0,
        };
        let tag_hits = e.content.tags.iter().filter(|t| haystack.contains(t.as_str())).count();
        (Reverse(specificity), Reverse(tag_hits), Reverse(e.updated_at), e.id.clone())
    });
    entries
}

fn decision_candidate(entry: &KnowledgeEntry) -> Candidate {
    let content = &entry.content;
    let mut text = format!("\n- 〔{} v{}〕{}", entry.id, entry.version, content.title);
    if !content.body.is_empty() {
        text.push_str(&format!("：{}", content.body));
    }
    candidate(ItemKind::Decision, &entry.id, Some(entry.version), &content.title, text)
}

/// 教訓摘要：標題、觸發條件與做法；兩者都空白時改用內文。根因與證據不放進 Prompt（需要時以 search_knowledge 查詢）。
fn lesson_candidate(entry: &KnowledgeEntry) -> Candidate {
    let content = &entry.content;
    let mut text = format!("\n- 〔{} v{}〕{}", entry.id, entry.version, content.title);
    let detail = content.lesson.clone().unwrap_or_default();
    if !detail.trigger.is_empty() {
        text.push_str(&format!("\n  觸發條件：{}", detail.trigger));
    }
    if !detail.resolution.is_empty() {
        text.push_str(&format!("\n  做法：{}", detail.resolution));
    }
    if detail.trigger.is_empty() && detail.resolution.is_empty() && !content.body.is_empty() {
        text.push_str(&format!("\n  {}", content.body));
    }
    candidate(ItemKind::Lesson, &entry.id, Some(entry.version), &content.title, text)
}

fn evidence_candidate(evidence: &EvidenceItem) -> Candidate {
    let short: String = evidence.sha256.chars().take(12).collect();
    let mut text = format!(
        "\n- 〔{} v{}〕{}（Task「{}」，sha256 {short}）",
        evidence.artifact_id, evidence.version, evidence.name, evidence.task_title
    );
    if !evidence.description.is_empty() {
        text.push_str(&format!("：{}", evidence.description));
    }
    candidate(ItemKind::Evidence, &evidence.artifact_id, Some(evidence.version), &evidence.name, text)
}

#[cfg(test)]
mod tests;
