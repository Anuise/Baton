//! 驗證的嘗試、日誌與交接（票 49；spec「驗證 Run 與一般 Run 共用帳號額度與交接機制」）。
//!
//! 純邏輯、無 I/O。一個驗證要求可以有多次嘗試（attempt）：
//! - 每次嘗試在執行中依序把可恢復的事實記入日誌（`JournalEntry`）：build、啟動的服務與瀏覽器 Session（安全的參照：
//!   只有識別，不含 cookie、權杖或秘密）、外部操作送出前的標記、每個完成的步驟與判定、證據檔案、完成的旅程。
//! - 語意 judge 的帳號限流而另有帳號可用時，這次嘗試在步驟邊界交接：先停止這次的測試程序（關閉瀏覽器、teardown、
//!   停止服務、移除狀態目錄），再由使用另一個帳號的新嘗試接手。Runtime 在執行途中中斷時，下一次處理把仍在執行的嘗試
//!   記為中斷，同樣由新的嘗試接手。
//! - 承接者先以日誌中的參照停止前一次留下的測試程序（`leftovers`），無法確認停止就不執行任何操作。瀏覽器 Session 無法移轉，
//!   所以未完成的旅程從隔離的 fixture（新的狀態目錄、重新準備資料、新的 Session）整條重跑；已完成的旅程是最近確認的邊界，
//!   build 相同時沿用（`plan_journey`）。一條旅程的步驟紀錄只來自一次嘗試，不拼接。
//! - 外部操作（`Step::external`）送出前先記入日誌。未完成的旅程中已經開始的外部操作不重送，那條旅程也不重跑：
//!   結果未知是 `external_outcome_unknown`，已送出是 `external_already_sent`，旅程的結果不會是通過。

use serde::{Deserialize, Serialize};

use crate::event::DomainEvent;
use crate::verification_profile::{self as vp, EvidenceArtifact, EvidencePolicy, Journey, JourneyResult, Phase, StepRecord, Verdict};
use crate::Timestamp;

pub const ATTEMPT_AGGREGATE: &str = "verification_attempt";
pub const EVENT_ATTEMPT_STARTED: &str = "verification_attempt_started";
pub const EVENT_ATTEMPT_HANDED_OFF: &str = "verification_attempt_handed_off";
pub const EVENT_ATTEMPT_INTERRUPTED: &str = "verification_attempt_interrupted";

/// 交接原因：語意 judge 的帳號限流，另有帳號可以接手。
pub const REASON_JUDGE_RATE_LIMITED: &str = "judge_rate_limited";
/// 中斷原因：Runtime 在執行途中結束（崩潰或重啟），這次嘗試沒有結束紀錄。
pub const REASON_RUNTIME_INTERRUPTED: &str = "runtime_interrupted";
/// 外部操作已開始送出但沒有結果紀錄：結果未知，不重送。
pub const REASON_EXTERNAL_OUTCOME_UNKNOWN: &str = "external_outcome_unknown";
/// 外部操作已送出、旅程沒有完成：重跑會重送，所以不重跑。
pub const REASON_EXTERNAL_ALREADY_SENT: &str = "external_already_sent";
/// 外部操作無法先記入日誌：不送出（之後無法判斷有沒有送過）。
pub const REASON_JOURNAL_UNAVAILABLE: &str = "journal_unavailable";
/// 前一次嘗試的測試程序無法確認已停止：這次不執行任何操作。
pub const REASON_PREVIOUS_SESSION_UNCONFIRMED: &str = "previous_session_unconfirmed";

/// 一次嘗試的狀態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptStatus {
    Running,
    /// 在步驟邊界交接給新的嘗試（測試程序已停止）。
    HandedOff,
    /// Runtime 在執行途中中斷；由新的嘗試查驗並停止留下的測試程序。
    Interrupted,
    /// 產生了 Evidence Bundle。
    Completed,
}

impl AttemptStatus {
    pub const ALL: [AttemptStatus; 4] =
        [AttemptStatus::Running, AttemptStatus::HandedOff, AttemptStatus::Interrupted, AttemptStatus::Completed];

    pub fn as_str(self) -> &'static str {
        match self {
            AttemptStatus::Running => "running",
            AttemptStatus::HandedOff => "handed_off",
            AttemptStatus::Interrupted => "interrupted",
            AttemptStatus::Completed => "completed",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        AttemptStatus::ALL.into_iter().find(|s| s.as_str() == raw)
    }
}

/// 驗證日誌的一筆（只追加）。參照只有識別，不含任何秘密或登入狀態。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JournalEntry {
    /// 這次嘗試驗證的 build 與 candidate revision（決定能不能沿用已完成的旅程）。
    Build { candidate_revision: Option<String>, build: Option<String> },
    ServiceStarted { handle: String },
    ServiceStopped { handle: String },
    BrowserOpened { journey: String, session: String },
    BrowserClosed { session: String },
    /// 外部操作即將送出（送出前寫入）。
    ExternalStarted { journey: String, step: String },
    /// 一個步驟完成：操作與判定。
    StepCompleted { record: StepRecord },
    ArtifactStored { artifact: EvidenceArtifact },
    /// 旅程完整結束（最近確認的邊界）。
    JourneyCompleted { journey: String },
    /// 承接者查驗前一次嘗試的現況並停止留下的測試程序。
    TakenOver { takeover: Takeover },
}

impl JournalEntry {
    pub fn kind(&self) -> &'static str {
        match self {
            JournalEntry::Build { .. } => "build",
            JournalEntry::ServiceStarted { .. } => "service_started",
            JournalEntry::ServiceStopped { .. } => "service_stopped",
            JournalEntry::BrowserOpened { .. } => "browser_opened",
            JournalEntry::BrowserClosed { .. } => "browser_closed",
            JournalEntry::ExternalStarted { .. } => "external_started",
            JournalEntry::StepCompleted { .. } => "step_completed",
            JournalEntry::ArtifactStored { .. } => "artifact_stored",
            JournalEntry::JourneyCompleted { .. } => "journey_completed",
            JournalEntry::TakenOver { .. } => "taken_over",
        }
    }
}

/// 一次嘗試與它的日誌（依寫入順序）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attempt {
    pub request_id: String,
    pub attempt: u32,
    pub status: AttemptStatus,
    /// 這次嘗試語意 judge 使用的帳號；沒有可用帳號時為 `None`（只執行不需模型的斷言）。
    pub judge_profile_id: Option<String>,
    pub end_reason: Option<String>,
    pub started_at: Timestamp,
    pub ended_at: Option<Timestamp>,
    pub journal: Vec<JournalEntry>,
}

impl Attempt {
    /// 日誌中記下的 (candidate revision, build)；還沒識別 build 就中斷時為 `None`。
    fn build(&self) -> Option<(Option<&str>, Option<&str>)> {
        self.journal.iter().find_map(|e| match e {
            JournalEntry::Build { candidate_revision, build } => Some((candidate_revision.as_deref(), build.as_deref())),
            _ => None,
        })
    }

    fn completed(&self, journey: &str) -> bool {
        self.journal.iter().any(|e| matches!(e, JournalEntry::JourneyCompleted { journey: j } if j == journey))
    }

    /// 這條旅程在這次嘗試中完成的步驟紀錄（依執行順序）。
    fn records_of(&self, journey: &str) -> Vec<StepRecord> {
        self.journal
            .iter()
            .filter_map(|e| match e {
                JournalEntry::StepCompleted { record } if record.journey.as_deref() == Some(journey) => Some(record.clone()),
                _ => None,
            })
            .collect()
    }

    fn artifacts_of(&self, journey: &str) -> Vec<EvidenceArtifact> {
        self.journal
            .iter()
            .filter_map(|e| match e {
                JournalEntry::ArtifactStored { artifact } if artifact.journey.as_deref() == Some(journey) => Some(artifact.clone()),
                _ => None,
            })
            .collect()
    }

    /// 這條旅程中已經開始的外部操作：(步驟, 有沒有完成紀錄)。
    fn externals_of(&self, journey: &str) -> Vec<(String, bool)> {
        let records = self.records_of(journey);
        self.journal
            .iter()
            .filter_map(|e| match e {
                JournalEntry::ExternalStarted { journey: j, step } if j == journey => {
                    let done = records.iter().any(|r| r.phase == Phase::Journey && r.key == *step);
                    Some((step.clone(), done))
                }
                _ => None,
            })
            .collect()
    }

    fn takeover(&self) -> Option<Takeover> {
        self.journal.iter().find_map(|e| match e {
            JournalEntry::TakenOver { takeover } => Some(takeover.clone()),
            _ => None,
        })
    }
}

/// 不重送的外部操作。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotResent {
    pub journey: String,
    pub step: String,
    /// `external_outcome_unknown` 或 `external_already_sent`。
    pub reason: String,
    /// 送出那個操作的嘗試。
    pub attempt: u32,
}

/// 承接者查驗前一次嘗試的現況：停止了哪些留下的測試程序、哪些無法確認、沿用哪些旅程、哪些外部操作不重送。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Takeover {
    pub from_attempt: u32,
    pub stopped_services: Vec<String>,
    pub closed_sessions: Vec<String>,
    /// 無法確認已停止的原因；有任何一項時這次嘗試不執行操作。
    pub problems: Vec<String>,
    pub reused_journeys: Vec<String>,
    pub not_resent: Vec<NotResent>,
}

/// 先前的嘗試留下、還沒有停止紀錄的測試程序（安全的參照）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Leftovers {
    pub services: Vec<String>,
    pub sessions: Vec<String>,
}

/// 依全部嘗試的日誌計算還沒停止的服務與瀏覽器 Session（承接者停止它們時記在自己的日誌）。
pub fn leftovers(attempts: &[Attempt]) -> Leftovers {
    let mut left = Leftovers::default();
    for entry in attempts.iter().flat_map(|a| &a.journal) {
        match entry {
            JournalEntry::ServiceStarted { handle } => left.services.push(handle.clone()),
            JournalEntry::ServiceStopped { handle } => left.services.retain(|h| h != handle),
            JournalEntry::BrowserOpened { session, .. } => left.sessions.push(session.clone()),
            JournalEntry::BrowserClosed { session } => left.sessions.retain(|s| s != session),
            _ => {}
        }
    }
    left
}

/// 先前因模型限流而交接的嘗試所用的帳號：同一個驗證要求不再用它們做語意判斷。
pub fn rate_limited_accounts(attempts: &[Attempt]) -> Vec<String> {
    attempts
        .iter()
        .filter(|a| a.status == AttemptStatus::HandedOff && a.end_reason.as_deref() == Some(REASON_JUDGE_RATE_LIMITED))
        .filter_map(|a| a.judge_profile_id.clone())
        .collect()
}

/// 承接時一條旅程怎麼處理。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JourneyPlan {
    /// 從隔離的 fixture 整條執行。
    Run,
    /// 沿用先前嘗試完整結束的結果（build 與 candidate revision 相同）。
    Reuse { attempt: u32, steps: Vec<StepRecord>, artifacts: Vec<EvidenceArtifact> },
    /// 先前的嘗試已經開始其中的外部操作而旅程沒有完成：不重送、不重跑，保留那次的紀錄。
    NotResent { attempt: u32, step: String, reason: &'static str, steps: Vec<StepRecord>, artifacts: Vec<EvidenceArtifact> },
}

/// 決定一條旅程在這次嘗試中怎麼處理（`previous` 依嘗試順序）：
/// 1. 最近一次完整結束、而且驗證的是同一個 build 與 revision 的結果直接沿用；
/// 2. 否則，最近一次開始了其中外部操作而沒有完成的嘗試：不重送（有結果未知的操作時以它為準）；
/// 3. 其餘整條重跑（未完成的步驟不拼接到新的嘗試）。
pub fn plan_journey(journey: &Journey, previous: &[Attempt], candidate_revision: Option<&str>, build: Option<&str>) -> JourneyPlan {
    let key = journey.key.as_str();
    if let Some(done) = previous.iter().rev().find(|a| a.completed(key) && a.build() == Some((candidate_revision, build))) {
        return JourneyPlan::Reuse { attempt: done.attempt, steps: done.records_of(key), artifacts: done.artifacts_of(key) };
    }
    for attempt in previous.iter().rev().filter(|a| !a.completed(key)) {
        let externals = attempt.externals_of(key);
        let chosen = externals.iter().find(|(_, done)| !done).or(externals.last());
        if let Some((step, done)) = chosen {
            return JourneyPlan::NotResent {
                attempt: attempt.attempt,
                step: step.clone(),
                reason: if *done { REASON_EXTERNAL_ALREADY_SENT } else { REASON_EXTERNAL_OUTCOME_UNKNOWN },
                steps: attempt.records_of(key),
                artifacts: attempt.artifacts_of(key),
            };
        }
    }
    JourneyPlan::Run
}

/// 不重跑的旅程在 Bundle 中的步驟紀錄：那次嘗試已完成的紀錄，加上沒有紀錄的旅程步驟
/// （結果未知的外部操作記為 `external_outcome_unknown`，其餘 `not_executed`），都是證據不足。
pub fn not_resent_records(
    journey: &Journey,
    partial: &[StepRecord],
    external_step: &str,
    reason: &str,
    policy: &EvidencePolicy,
) -> Vec<StepRecord> {
    let mut records = partial.to_vec();
    for step in &journey.steps {
        if partial.iter().any(|r| r.phase == Phase::Journey && r.key == step.key) {
            continue;
        }
        let code = if step.key == external_step && reason == REASON_EXTERNAL_OUTCOME_UNKNOWN {
            REASON_EXTERNAL_OUTCOME_UNKNOWN
        } else {
            "not_executed"
        };
        records.push(vp::not_run(Phase::Journey, Some(&journey.key), step, Verdict::Inconclusive, code, policy));
    }
    records
}

/// 不重跑的旅程的結果：已由程式斷言證明的產品缺陷照舊，其餘一律是證據不足（原因是不重送），不會是通過。
pub fn not_resent_result(journey: &Journey, records: &[StepRecord], reason: &str) -> JourneyResult {
    let mut result = vp::judge_journey(journey, records);
    if result.verdict != Verdict::ProductFailure {
        result.verdict = Verdict::Inconclusive;
        result.reason_code = reason.to_string();
    }
    result
}

/// 一個已記錄步驟的摘要。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordedStep {
    pub phase: Phase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub journey: Option<String>,
    pub key: String,
    pub verdict: Verdict,
    pub reason_code: String,
}

/// Evidence Bundle 中一次嘗試的摘要。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttemptSummary {
    pub attempt: u32,
    pub status: AttemptStatus,
    pub judge_profile_id: Option<String>,
    pub end_reason: Option<String>,
    /// Bundle 中結果採用自這次嘗試的旅程。
    pub journeys: Vec<String>,
    /// 這次嘗試完成的步驟與判定（含交接前沒有完成的旅程；那些紀錄不會被拼進其他嘗試的結果）。
    pub steps: Vec<RecordedStep>,
    pub takeover: Option<Takeover>,
    pub started_at: Timestamp,
    pub ended_at: Option<Timestamp>,
}

/// 依嘗試順序產生摘要；`sources` 是 (旅程, 結果採用自哪一次嘗試)。
pub fn summarize(attempts: &[Attempt], sources: &[(String, u32)]) -> Vec<AttemptSummary> {
    attempts
        .iter()
        .map(|a| AttemptSummary {
            attempt: a.attempt,
            status: a.status,
            judge_profile_id: a.judge_profile_id.clone(),
            end_reason: a.end_reason.clone(),
            journeys: sources.iter().filter(|(_, n)| *n == a.attempt).map(|(j, _)| j.clone()).collect(),
            steps: a
                .journal
                .iter()
                .filter_map(|e| match e {
                    JournalEntry::StepCompleted { record } => Some(RecordedStep {
                        phase: record.phase,
                        journey: record.journey.clone(),
                        key: record.key.clone(),
                        verdict: record.verdict,
                        reason_code: record.reason_code.clone(),
                    }),
                    _ => None,
                })
                .collect(),
            takeover: a.takeover(),
            started_at: a.started_at,
            ended_at: a.ended_at,
        })
        .collect()
}

fn attempt_id(request_id: &str, attempt: u32) -> String {
    format!("{request_id}#{attempt}")
}

pub fn started_event(attempt: &Attempt, interrupted: &[u32]) -> DomainEvent {
    DomainEvent {
        aggregate_type: ATTEMPT_AGGREGATE,
        aggregate_id: attempt_id(&attempt.request_id, attempt.attempt),
        aggregate_version: 1,
        event_type: EVENT_ATTEMPT_STARTED,
        occurred_at: attempt.started_at,
        payload: serde_json::json!({
            "request_id": attempt.request_id,
            "attempt": attempt.attempt,
            "judge_profile_id": attempt.judge_profile_id,
            "interrupted_attempts": interrupted,
        }),
    }
}

/// 嘗試結束（交接或中斷）的事件；產生 Bundle 的嘗試以驗證要求的完成事件為準。
pub fn ended_event(request_id: &str, attempt: u32, status: AttemptStatus, reason: &str, now: Timestamp) -> DomainEvent {
    DomainEvent {
        aggregate_type: ATTEMPT_AGGREGATE,
        aggregate_id: attempt_id(request_id, attempt),
        aggregate_version: 2,
        event_type: if status == AttemptStatus::HandedOff { EVENT_ATTEMPT_HANDED_OFF } else { EVENT_ATTEMPT_INTERRUPTED },
        occurred_at: now,
        payload: serde_json::json!({ "request_id": request_id, "attempt": attempt, "reason": reason }),
    }
}

#[cfg(test)]
mod tests;
