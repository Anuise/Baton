//! 監護程序（Guardian）的規則（票 32；DR-20261021-06、ADR-0006、spec D-24）。
//!
//! 純邏輯、無 I/O。幾條貫穿全檔的規則：
//! - **監護不寫控制資料庫**：監護只 append 自己的監護日誌（每筆一行、寫完 fsync），Runtime 啟動後第一件事是把
//!   尚未匯入的紀錄匯入控制資料庫（以嘗試識別去重）。監護讀 Runtime 寫的恢復狀態鏡像與生命週期檔，不讀 SQLite。
//! - **共用三次額度**：監護在啟動 Runtime 之前就寫下「事件、輪次、嘗試序號、嘗試識別」，那就是這一次嘗試；
//!   Runtime 接續同一次，不另扣也不另取。Runtime 啟動早期就失敗（沒來得及匯入）也已經計次。
//! - **抑制優先**：完全退出（停止意圖）、更新與解除安裝（維護抑制）、或生命週期檔讀不懂（停止意圖不明）時一律不啟動。
//! - **降級**：監護不在時 Runtime 照常工作，只顯示「工作繼續，自動重啟保護不可用」。

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::lifecycle::LifecycleFile;
use crate::recovery::{self, RecoveryEvent, RecoveryStatus, MAX_ATTEMPTS};
use crate::Timestamp;

/// 監護相關檔案所在的子目錄（位於資料目錄）。
pub const GUARDIAN_DIR: &str = "guardian";
/// 監護日誌（append-only，每筆一行 JSON）。
pub const JOURNAL_FILE_NAME: &str = "journal.jsonl";
/// Runtime 寫給監護看的恢復狀態鏡像。
pub const RECOVERY_MIRROR_FILE_NAME: &str = "recovery.json";
/// Runtime 程序的身分（程序識別碼與記錄時間），監護用來確認是哪一個程序結束。
pub const RUNTIME_INSTANCE_FILE_NAME: &str = "runtime.json";
/// 監護程序的身分，Runtime 用來判斷監護是否還在。
pub const GUARDIAN_INSTANCE_FILE_NAME: &str = "guardian.json";
/// 監護啟動 Runtime 時，以這個環境變數交付這一次的嘗試識別。
pub const GUARDIAN_ATTEMPT_ENV: &str = "BATON_GUARDIAN_ATTEMPT";
/// 監護不在時固定顯示的文字（ADR-0006 M1）。
pub const DEGRADED_MESSAGE: &str = "工作繼續，自動重啟保護不可用";
/// 恢復狀態鏡像的格式版本。
pub const RECOVERY_MIRROR_VERSION: u32 = 1;

/// 監護確認 Runtime 意外終止、即將重新啟動時寫下的嘗試。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuardianAttempt {
    pub event_id: String,
    pub round: u32,
    /// 監護算出的嘗試序號（1～3）；Runtime 匯入時以控制資料庫的計數為準。
    pub seq: u32,
    pub attempt_id: String,
    /// 監護確認 Runtime 意外終止的時間。
    pub detected_at: Timestamp,
    /// 寫下這筆紀錄（即將啟動 Runtime）的時間。
    pub at: Timestamp,
    pub reason: String,
}

/// 監護日誌的一筆紀錄。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JournalRecord {
    /// 監護在啟動 Runtime 之前寫下的嘗試（寫下即計次）。
    Attempt(GuardianAttempt),
    /// 寫下嘗試之後、啟動之前發現受抑制或已有 Runtime：這次沒有啟動 Runtime，不計次。
    Withdrawn { attempt_id: String, at: Timestamp, reason: String },
}

impl JournalRecord {
    /// 單行 JSON（不含換行）。
    pub fn to_line(&self) -> String {
        // Domain 型別只含字串與數字，序列化不會失敗
        serde_json::to_string(self).expect("監護日誌紀錄可序列化")
    }

    pub fn attempt_id(&self) -> &str {
        match self {
            JournalRecord::Attempt(attempt) => &attempt.attempt_id,
            JournalRecord::Withdrawn { attempt_id, .. } => attempt_id,
        }
    }
}

/// 解析監護日誌。解析不了的行（寫到一半就中斷、空行）略過。
pub fn parse_journal(text: &str) -> Vec<JournalRecord> {
    text.lines().filter_map(|line| serde_json::from_str(line.trim()).ok()).collect()
}

/// 被撤回的嘗試識別。
pub fn withdrawn_ids(journal: &[JournalRecord]) -> HashSet<&str> {
    journal
        .iter()
        .filter_map(|r| match r {
            JournalRecord::Withdrawn { attempt_id, .. } => Some(attempt_id.as_str()),
            JournalRecord::Attempt(_) => None,
        })
        .collect()
}

// ---------------------------------------------------------------- 恢復狀態鏡像

/// 鏡像中的恢復事件摘要。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MirroredEvent {
    pub id: String,
    pub round: u32,
    pub status: RecoveryStatus,
    pub attempts_used: u32,
    pub next_attempt_at: Option<Timestamp>,
}

/// Runtime 寫給監護看的恢復狀態（原子寫入）。監護以它與監護日誌算出已用次數與退避期限，不讀 SQLite。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryMirror {
    pub version: u32,
    /// 最新的恢復事件；從未崩潰為 `None`。
    pub event: Option<MirroredEvent>,
    /// Runtime 已處理到的最後一筆監護日誌（嘗試識別）；在它之後的紀錄 Runtime 還沒看過。
    pub journal_cursor: Option<String>,
    pub updated_at: Timestamp,
}

impl RecoveryMirror {
    pub fn of(event: Option<&RecoveryEvent>, journal_cursor: Option<String>, now: Timestamp) -> Self {
        Self {
            version: RECOVERY_MIRROR_VERSION,
            event: event.map(|e| MirroredEvent {
                id: e.id.clone(),
                round: e.round,
                status: e.status,
                attempts_used: e.attempts_used,
                next_attempt_at: e.next_attempt_at,
            }),
            journal_cursor,
            updated_at: now,
        }
    }
}

// ---------------------------------------------------------------- 抑制

/// 讀取生命週期檔的結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LifecycleFileRead {
    Missing,
    Parsed(LifecycleFile),
    /// 讀不到或解析不了：停止意圖不明。
    Unreadable,
}

/// 不自動啟動 Runtime 的原因。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub enum Suppression {
    /// 完全退出的停止意圖。
    FullExit,
    /// 更新、解除安裝等維護。
    Maintenance(String),
    /// 生命週期檔讀不懂：停止意圖不明時不推定允許啟動。
    Unknown,
}

pub fn suppression_of(read: &LifecycleFileRead) -> Option<Suppression> {
    match read {
        LifecycleFileRead::Missing => None,
        LifecycleFileRead::Unreadable => Some(Suppression::Unknown),
        LifecycleFileRead::Parsed(file) => {
            if file.stop.is_some() {
                Some(Suppression::FullExit)
            } else {
                file.maintenance.as_ref().map(|m| Suppression::Maintenance(m.kind.clone()))
            }
        }
    }
}

// ---------------------------------------------------------------- 重啟規劃

/// 監護確認 Runtime 意外終止後的判斷依據。
#[derive(Debug, Clone)]
pub struct RestartInput<'a> {
    pub mirror: Option<&'a RecoveryMirror>,
    pub journal: &'a [JournalRecord],
    pub suppression: Option<Suppression>,
    /// 監護確認 Runtime 結束的時間（嘗試失敗被記下的時刻，退避從這裡起算）。
    pub exit_observed_at: Timestamp,
    pub now: Timestamp,
}

/// 監護的決定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestartPlan {
    Suppressed(Suppression),
    /// 這一輪三次都用掉了：不再自動啟動，等使用者開 App。
    Exhausted,
    /// 退避中。
    Wait { until: Timestamp },
    /// 先把這筆嘗試寫進監護日誌（fsync），再啟動 Runtime。
    Start(GuardianAttempt),
}

/// 監護眼中的目前事件。
enum Known {
    /// Runtime 在等下一次嘗試（退避期限照舊）。
    Waiting { event_id: String, round: u32, used: u32, next: Option<Timestamp> },
    /// 嘗試進行中或觀察中就結束了：那次嘗試失敗。
    InFlight { event_id: String, round: u32, used: u32 },
    Exhausted,
}

fn known_state(mirror: Option<&RecoveryMirror>, journal: &[JournalRecord]) -> Option<Known> {
    let mut state = mirror.and_then(|m| m.event.as_ref()).and_then(|e| {
        let (event_id, round, used) = (e.id.clone(), e.round, e.attempts_used);
        match e.status {
            RecoveryStatus::Waiting => Some(Known::Waiting { event_id, round, used, next: e.next_attempt_at }),
            RecoveryStatus::Attempting | RecoveryStatus::Observing => Some(Known::InFlight { event_id, round, used }),
            RecoveryStatus::Exhausted => Some(Known::Exhausted),
            RecoveryStatus::Succeeded => None,
        }
    });
    // Runtime 還沒看過的紀錄：游標之後的部分（沒有游標就是全部）
    let cursor = mirror.and_then(|m| m.journal_cursor.as_deref());
    let start = cursor.and_then(|c| journal.iter().rposition(|r| r.attempt_id() == c)).map_or(0, |i| i + 1);
    let pending = &journal[start..];
    let withdrawn = withdrawn_ids(pending);
    for record in pending {
        let JournalRecord::Attempt(attempt) = record else { continue };
        if withdrawn.contains(attempt.attempt_id.as_str()) {
            continue;
        }
        state = Some(match state {
            Some(Known::Waiting { event_id, round, used, .. } | Known::InFlight { event_id, round, used })
                if event_id == attempt.event_id && round == attempt.round =>
            {
                Known::InFlight { event_id, round, used: used.max(attempt.seq) }
            }
            Some(Known::Exhausted) => Known::Exhausted,
            _ => Known::InFlight { event_id: attempt.event_id.clone(), round: attempt.round, used: attempt.seq },
        });
    }
    state
}

/// 監護確認 Runtime 意外終止後，決定要不要、什麼時候重新啟動。新事件與新嘗試的識別由呼叫端產生。
pub fn plan_restart(input: &RestartInput<'_>, new_event_id: &str, new_attempt_id: &str) -> RestartPlan {
    if let Some(suppression) = &input.suppression {
        return RestartPlan::Suppressed(suppression.clone());
    }
    let start = |event_id: &str, round: u32, seq: u32| {
        RestartPlan::Start(GuardianAttempt {
            event_id: event_id.to_string(),
            round,
            seq,
            attempt_id: new_attempt_id.to_string(),
            detected_at: input.exit_observed_at,
            at: input.now,
            reason: "監護確認 Runtime 意外終止".to_string(),
        })
    };
    let (event_id, round, seq, not_before) = match known_state(input.mirror, input.journal) {
        None => return start(new_event_id, 1, 1),
        Some(Known::Exhausted) => return RestartPlan::Exhausted,
        Some(Known::Waiting { event_id, round, used, next }) => (event_id, round, used + 1, next.unwrap_or(input.now)),
        Some(Known::InFlight { event_id, round, used }) => {
            let seq = used + 1;
            (event_id, round, seq, input.exit_observed_at.plus_millis(recovery::backoff_before(seq)))
        }
    };
    if seq > MAX_ATTEMPTS {
        return RestartPlan::Exhausted;
    }
    if input.now < not_before {
        return RestartPlan::Wait { until: not_before };
    }
    start(&event_id, round, seq)
}

// ---------------------------------------------------------------- 監護狀態（顯示用）

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuardianState {
    /// 監護在執行：Runtime 意外終止時會有限次自動重啟。
    Active,
    /// 監護不在：工作繼續，但沒有自動重啟保護。
    Unavailable,
    /// 舊版 Runtime 沒有回報。
    #[default]
    Unknown,
}

/// 監護狀態（頂部狀態列顯示，spec D-21）。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct GuardianStatus {
    pub state: GuardianState,
    /// 降級時的說明；正常時為 `None`。
    pub message: Option<String>,
}

impl GuardianStatus {
    pub fn of(running: bool) -> Self {
        if running {
            Self { state: GuardianState::Active, message: None }
        } else {
            Self { state: GuardianState::Unavailable, message: Some(DEGRADED_MESSAGE.to_string()) }
        }
    }
}

#[cfg(test)]
mod tests;
