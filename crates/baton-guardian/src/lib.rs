//! 監護程序（Guardian，票 32；DR-20261021-06、ADR-0006、spec D-24）。
//!
//! 獨立的輕量程序，同一次 Windows 登入期間監看 Runtime，只對**已確認意外終止**的 Runtime 做有限次自動重啟：
//! - 確認 Runtime 程序已經結束才算終止（不以 heartbeat 或 API 逾時推定）。
//! - 重啟前讀生命週期檔：完全退出、更新、解除安裝或停止意圖不明時不啟動，監護結束。
//! - 以監護日誌與 Runtime 寫的恢復狀態鏡像算出已用次數與退避期限（不讀 SQLite）；用盡就不再啟動。
//! - 先把這次嘗試 append 到監護日誌並 fsync，再做最後核對（已有 Runtime 或剛出現抑制就撤回），最後才啟動 Runtime；
//!   寫不進日誌就不啟動。Runtime 啟動後會匯入這筆紀錄並接續同一次嘗試。
//!
//! 監護不派工、不裁定、不寫控制資料庫。何時動作由呼叫端決定：正式執行檔每秒 `step` 一次，測試 harness 由測試逐步驅動。

pub mod files;
pub mod host;
pub mod process;

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use baton_domain::guardian::{self, GuardianAttempt, JournalRecord, RestartInput, RestartPlan, Suppression};
use baton_domain::Timestamp;
use baton_ports::Clock;

/// 監護看到的 Runtime。正式版以程序控制代碼與程序身分判斷；測試 harness 以程序內的 Runtime 實作。
#[async_trait]
pub trait RuntimeHost: Send {
    /// Runtime 是否在執行。只有確認程序已經結束才回傳 `false`。
    async fn runtime_running(&mut self) -> bool;

    /// 啟動 Runtime，並交付這一次的嘗試識別（環境變數 `BATON_GUARDIAN_ATTEMPT`）。
    /// 只回報「程序有沒有啟動起來」；之後 Runtime 自己結束，由下一次 `runtime_running` 發現。
    async fn start_runtime(&mut self, attempt_id: &str) -> Result<(), String>;
}

/// 監護結束（不再重啟 Runtime）的原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuardianEnd {
    /// 完全退出、維護或停止意圖不明。
    Suppressed(Suppression),
    /// 這一輪三次都用掉了，等使用者開 App。
    Exhausted,
    /// 無法讀寫監護日誌（或取得識別所需的亂數）：無法保存或核對時不自動啟動。
    JournalUnavailable(String),
}

/// `step` 一次做了什麼。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuardianStep {
    /// Runtime 在執行，繼續監看。
    Watching,
    /// 確認 Runtime 已經結束；下一次 `step` 決定是否重啟。
    RuntimeExited,
    /// 退避中，最早在 `until` 才啟動。
    Waiting { until: Timestamp },
    /// 已寫下嘗試並啟動 Runtime。
    Restarted(GuardianAttempt),
    /// 寫下嘗試之後發現已經有 Runtime 在執行（例如使用者剛開 App）：撤回這次嘗試，改為監看它。
    Withdrawn { attempt_id: String },
    /// 寫下嘗試之後無法啟動程序：這次嘗試照樣計次（啟動早期失敗也計次），下一次 `step` 依退避再決定。
    StartFailed { attempt: GuardianAttempt, error: String },
    /// 監護結束。
    Finished(GuardianEnd),
}

#[derive(Debug, Clone)]
enum State {
    Watching,
    AwaitingRestart { exit_observed_at: Timestamp },
    Finished(GuardianEnd),
}

pub struct Guardian {
    data_dir: PathBuf,
    clock: Arc<dyn Clock>,
    state: State,
}

impl Guardian {
    /// 從「監看中」開始：監護只重啟它親眼看到結束的 Runtime。
    pub fn new(data_dir: PathBuf, clock: Arc<dyn Clock>) -> Self {
        Self { data_dir, clock, state: State::Watching }
    }

    /// 監護已經結束時的原因。
    pub fn finished(&self) -> Option<&GuardianEnd> {
        match &self.state {
            State::Finished(end) => Some(end),
            _ => None,
        }
    }

    pub async fn step(&mut self, host: &mut dyn RuntimeHost) -> GuardianStep {
        match self.state.clone() {
            State::Finished(end) => GuardianStep::Finished(end),
            State::Watching => {
                if host.runtime_running().await {
                    return GuardianStep::Watching;
                }
                let now = self.clock.now();
                self.note(now, "確認 Runtime 已經結束");
                self.state = State::AwaitingRestart { exit_observed_at: now };
                GuardianStep::RuntimeExited
            }
            State::AwaitingRestart { exit_observed_at } => self.consider_restart(host, exit_observed_at).await,
        }
    }

    fn note(&self, now: Timestamp, message: &str) {
        files::append_log(&self.data_dir, now, message);
    }

    fn finish(&mut self, end: GuardianEnd) -> GuardianStep {
        self.note(self.clock.now(), &format!("監護結束，不再重啟 Runtime：{end:?}"));
        self.state = State::Finished(end.clone());
        GuardianStep::Finished(end)
    }

    async fn consider_restart(&mut self, host: &mut dyn RuntimeHost, exit_observed_at: Timestamp) -> GuardianStep {
        let now = self.clock.now();
        let journal = match files::read_journal(&self.data_dir) {
            Ok(journal) => journal,
            Err(error) => return self.finish(GuardianEnd::JournalUnavailable(format!("無法讀取監護日誌：{error}"))),
        };
        let (Some(event_id), Some(attempt_id)) = (new_id("rec"), new_id("rcv")) else {
            return self.finish(GuardianEnd::JournalUnavailable("無法取得系統亂數".to_string()));
        };
        let mirror = files::read_recovery_mirror(&self.data_dir);
        let input = RestartInput {
            mirror: mirror.as_ref(),
            journal: &journal,
            suppression: guardian::suppression_of(&files::read_lifecycle(&self.data_dir)),
            exit_observed_at,
            now,
        };
        match guardian::plan_restart(&input, &event_id, &attempt_id) {
            RestartPlan::Suppressed(suppression) => self.finish(GuardianEnd::Suppressed(suppression)),
            RestartPlan::Exhausted => self.finish(GuardianEnd::Exhausted),
            RestartPlan::Wait { until } => {
                // 退避期間使用者可能已經開 App 啟動了 Runtime：改為監看它
                if host.runtime_running().await {
                    self.state = State::Watching;
                    return GuardianStep::Watching;
                }
                GuardianStep::Waiting { until }
            }
            RestartPlan::Start(attempt) => self.start(host, attempt).await,
        }
    }

    /// 先寫下嘗試（fsync），再做最後核對，最後才啟動。
    async fn start(&mut self, host: &mut dyn RuntimeHost, attempt: GuardianAttempt) -> GuardianStep {
        if let Err(error) = files::append_journal(&self.data_dir, &JournalRecord::Attempt(attempt.clone())) {
            return self.finish(GuardianEnd::JournalUnavailable(format!("無法寫入監護日誌：{error}")));
        }
        // 最後核對：寫下之後才出現的 Runtime 或抑制都會讓這次嘗試撤回，不啟動
        let running = host.runtime_running().await;
        let suppression = guardian::suppression_of(&files::read_lifecycle(&self.data_dir));
        if running || suppression.is_some() {
            let reason = if running { "已有 Runtime 在執行".to_string() } else { format!("受抑制：{suppression:?}") };
            let withdrawn =
                JournalRecord::Withdrawn { attempt_id: attempt.attempt_id.clone(), at: self.clock.now(), reason };
            if let Err(error) = files::append_journal(&self.data_dir, &withdrawn) {
                // 撤回寫不進去：這次嘗試會被計次（偏保守），照樣不啟動
                self.note(self.clock.now(), &format!("無法寫入撤回紀錄：{error}"));
            }
            return match suppression {
                Some(suppression) => self.finish(GuardianEnd::Suppressed(suppression)),
                None => {
                    self.state = State::Watching;
                    GuardianStep::Withdrawn { attempt_id: attempt.attempt_id }
                }
            };
        }
        self.note(
            self.clock.now(),
            &format!("啟動 Runtime：事件 {} 第 {} 輪第 {} 次（{}）", attempt.event_id, attempt.round, attempt.seq, attempt.attempt_id),
        );
        match host.start_runtime(&attempt.attempt_id).await {
            Ok(()) => {
                self.state = State::Watching;
                GuardianStep::Restarted(attempt)
            }
            Err(error) => {
                let now = self.clock.now();
                self.note(now, &format!("無法啟動 Runtime：{error}"));
                self.state = State::AwaitingRestart { exit_observed_at: now };
                GuardianStep::StartFailed { attempt, error }
            }
        }
    }
}

fn new_id(prefix: &str) -> Option<String> {
    let mut entropy = [0u8; 12];
    getrandom::fill(&mut entropy).ok()?;
    let hex: String = entropy.iter().map(|b| format!("{b:02x}")).collect();
    Some(format!("{prefix}_{hex}"))
}

#[cfg(test)]
mod tests;
