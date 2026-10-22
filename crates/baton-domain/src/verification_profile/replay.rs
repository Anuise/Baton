//! 已驗證動作的錄製與重播（票 48）。
//!
//! 純邏輯、無 I/O：
//! - 目標導向動作（`act`）由模型依當前畫面決定一個具體動作。只有步驟通過、而且至少一個不靠模型的斷言證明了結果時，
//!   具體動作才錄製（`proven`）；後置條件沒有成立、或只靠模型判斷的結果都不錄製。
//! - 錄製以案例（旅程與步驟）為鍵，附上錄製身分：案例路徑、目標、參數、persona 與登入身分、瀏覽器引擎、資料相容性。
//!   任何一個維度與這次不同，錄製就是 stale、不沿用（`decide`）。
//! - 重播仍執行真實動作並重新檢查後置條件；動作無法完成或後置條件明確不成立時，錄製改為 stale（`disproven`）。
//!   快取命中不是通過：步驟的判定只看這次重播的觀察。
//! - Profile 宣告為正式環境時，錄製預設不重播（`replay_in_production` 個別允許）。
//! - 即時模型與重播的用量分開計算（`VerificationUsage`）。

use serde::{Deserialize, Serialize};

use super::{hash_of, Assertion, BrowserAction, EvidenceBundle, Journey, ProfileDefinition, Step, StepRecord, Verdict};
use crate::Timestamp;

/// 引擎取不到時的值。
pub const UNKNOWN_ENGINE: &str = "unknown";

/// 錄製身分：錄製當時的案例、目標、參數、persona、引擎與資料相容性。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayIdentity {
    /// 案例：旅程、步驟，以及這條旅程中它之前的全部操作（含登入身分的登入與核對）的雜湊。
    pub case: String,
    /// 目標（Profile 中的原文）。
    pub goal: String,
    /// 參數的雜湊。
    pub params: String,
    /// persona（識別與內容）與登入身分的雜湊。
    pub persona: String,
    /// 瀏覽器引擎（Runner 回報，例如 Playwright 與瀏覽器的版本）。
    pub engine: String,
    /// 資料相容性：Profile 宣告的資料集版本與資料準備步驟的雜湊。
    pub data: String,
    /// 目標或參數含本次驗證的佔位字（`{{run_id}}`、`{{state_dir}}`）：錄下的動作帶著代換後的值，換一次驗證就不再正確，
    /// 所以這類步驟不重播也不保存錄製（review-notes #26）。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub run_scoped: bool,
}

impl ReplayIdentity {
    /// 與另一個身分不同的維度，依固定順序：`case`、`goal`、`params`、`persona`、`engine`、`data`。
    pub fn differences(&self, other: &ReplayIdentity) -> Vec<&'static str> {
        [
            ("case", self.case == other.case),
            ("goal", self.goal == other.goal),
            ("params", self.params == other.params),
            ("persona", self.persona == other.persona),
            ("engine", self.engine == other.engine),
            ("data", self.data == other.data),
        ]
        .into_iter()
        .filter(|(_, same)| !same)
        .map(|(name, _)| name)
        .collect()
    }
}

/// 錄製的鍵：Workspace 內的旅程與步驟。
pub fn case_key(journey: &str, step: &str) -> String {
    format!("{journey}/{step}")
}

/// 旅程中一個目標導向步驟的錄製身分；不是目標導向動作或不屬於這條旅程時為 `None`。
/// 使用 Profile 中的原文（佔位字尚未代換）；目標或參數含本次驗證的佔位字時標為 `run_scoped`，不重播也不錄製。
pub fn identity(definition: &ProfileDefinition, journey: &Journey, step: &Step, engine: &str) -> Option<ReplayIdentity> {
    let super::Operation::Browser { action: BrowserAction::Act { goal, params, .. }, .. } = &step.operation else {
        return None;
    };
    let index = journey.steps.iter().position(|s| s.key == step.key)?;
    let login = journey.identity.as_deref().and_then(|i| definition.identity(i));
    let path: Vec<&Step> =
        login.into_iter().flat_map(|i| i.login.iter().chain(std::iter::once(&i.check))).chain(&journey.steps[..index]).collect();
    let persona = journey.persona.as_deref().map(|key| (key, definition.persona(key)));
    Some(ReplayIdentity {
        case: hash_of(&serde_json::json!({ "journey": journey.key, "step": step.key, "path": path })),
        goal: goal.clone(),
        params: hash_of(params),
        persona: hash_of(&serde_json::json!({ "persona": persona, "identity": journey.identity })),
        engine: engine.to_string(),
        data: hash_of(&serde_json::json!({ "dataset": definition.replay.dataset, "setup": definition.setup })),
        run_scoped: std::iter::once(goal)
            .chain(params.values())
            .any(|text| text.contains(super::PLACEHOLDER_RUN_ID) || text.contains(super::PLACEHOLDER_STATE_DIR)),
    })
}

/// 錄製的狀態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayStatus {
    Valid,
    /// 身分不一致或重播被推翻：不再沿用，直到同一案例有新的錄製。
    Stale,
}

impl ReplayStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            ReplayStatus::Valid => "valid",
            ReplayStatus::Stale => "stale",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        [ReplayStatus::Valid, ReplayStatus::Stale].into_iter().find(|s| s.as_str() == raw)
    }
}

/// 一個案例的錄製（控制資料庫中每個 Workspace、每個案例一筆）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayEntry {
    pub case_key: String,
    pub identity: ReplayIdentity,
    /// 錄製的具體動作（點擊、輸入或按鍵）。
    pub action: BrowserAction,
    pub status: ReplayStatus,
    /// 標記 stale 的原因（`valid` 時是空的）。
    pub stale_reasons: Vec<String>,
    /// 錄製它的 Evidence Bundle。
    pub bundle_id: String,
    pub recorded_at: Timestamp,
}

/// 執行前錄製的狀態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheState {
    /// 這個案例沒有錄製。
    None,
    /// 有可用的錄製，這次重播它。
    Valid,
    /// 錄製先前已經是 stale。
    Stale,
    /// 錄製的身分與這次不同（這次標記為 stale）。
    Mismatch,
    /// 正式環境：錄製預設不重播。
    ProductionNotReplayable,
}

/// 執行前對錄製的判斷。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplayDecision {
    /// 重播這個錄製的動作（仍執行真實動作並重新檢查後置條件）。
    Replay(BrowserAction),
    /// 不重播（需要即時模型）；`stale` 是這次發現、要標記的不一致（例如 `engine_changed`）。
    Live { cache: CacheState, stale: Vec<String> },
}

/// 依錄製與這次的身分決定是否重播。`production` 是 Profile 宣告的正式環境，`replay_in_production` 是步驟的個別允許。
pub fn decide(
    entry: Option<&ReplayEntry>,
    identity: &ReplayIdentity,
    production: bool,
    replay_in_production: bool,
) -> ReplayDecision {
    let Some(entry) = entry.filter(|_| !identity.run_scoped) else {
        return ReplayDecision::Live { cache: CacheState::None, stale: Vec::new() };
    };
    if entry.status == ReplayStatus::Stale {
        return ReplayDecision::Live { cache: CacheState::Stale, stale: Vec::new() };
    }
    let differences = entry.identity.differences(identity);
    if !differences.is_empty() {
        let stale = differences.into_iter().map(|d| format!("{d}_changed")).collect();
        return ReplayDecision::Live { cache: CacheState::Mismatch, stale };
    }
    if production && !replay_in_production {
        return ReplayDecision::Live { cache: CacheState::ProductionNotReplayable, stale: Vec::new() };
    }
    ReplayDecision::Replay(entry.action.clone())
}

/// 模型可以為目標導向動作選擇的具體動作：點擊、輸入或按鍵（不能任意導覽，也不能再是目標導向動作）。
pub fn plannable(action: &BrowserAction) -> bool {
    matches!(action, BrowserAction::Click { .. } | BrowserAction::Fill { .. } | BrowserAction::Press { .. })
}

/// 動作的結果是否被後續有效斷言證明：步驟通過，而且至少一個不靠模型的斷言成立。
pub fn proven(record: &StepRecord) -> bool {
    record.verdict == Verdict::Passed
        && record.assertions.iter().any(|a| a.passed && !matches!(a.assertion, Assertion::Judge { .. }))
}

/// 重播的結果是否推翻了錄製：動作無法完成，或不靠模型的斷言明確不成立。
/// 環境阻礙、授權阻礙、預算用盡與模型無法判定都不是動作的問題，不推翻錄製。
pub fn disproven(record: &StepRecord) -> bool {
    if matches!(record.verdict, Verdict::EnvironmentBlocked | Verdict::AuthBlocked | Verdict::BudgetExhausted) {
        return false;
    }
    record.reason_code == "action_failed"
        || record.assertions.iter().any(|a| !a.passed && !matches!(a.assertion, Assertion::Judge { .. }))
}

/// 動作的來源。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionSource {
    /// 這次由模型即時決定。
    Live,
    /// 重播已驗證的錄製（沒有呼叫模型）。
    Replay,
}

/// 目標導向步驟的判定：動作是模型或錄製選的，不是 Profile 寫定的操作，所以後置條件不成立是證據不足（`inconclusive`），
/// 不是產品缺陷；要判定產品缺陷請寫成固定的操作。
pub fn settle(mut record: StepRecord, source: ActionSource) -> StepRecord {
    if record.verdict == Verdict::ProductFailure {
        record.verdict = Verdict::Inconclusive;
        record.reason_code = match source {
            ActionSource::Live => "act_postcondition_failed",
            ActionSource::Replay => "replay_postcondition_failed",
        }
        .into();
    }
    record
}

/// 這次錄製的動作。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordedAction {
    pub identity: ReplayIdentity,
    pub action: BrowserAction,
}

/// 一個目標導向動作的紀錄（Evidence Bundle 的一部分）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionRecord {
    pub journey: String,
    pub step: String,
    pub goal: String,
    /// 執行前錄製的狀態。
    pub cache: CacheState,
    /// 實際執行的動作來源；沒有執行任何動作時為 `None`（例如 no_live_model 又沒有可用的錄製）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<ActionSource>,
    /// 實際執行的具體動作描述（已遮蔽）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    /// 這次把錄製標為 stale 的原因：不一致的維度（例如 `engine_changed`）或 `postcondition_failed`。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stale_reasons: Vec<String>,
    /// 這次錄製的動作（取代同一案例原有的錄製）；沒有被後續有效斷言證明時為 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recorded: Option<RecordedAction>,
}

/// 一次驗證的用量：即時模型與重播分開計算。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerificationUsage {
    /// 實際呼叫模型的次數（語意判斷與動作規劃）。
    #[serde(default)]
    pub live_model_calls: u32,
    /// 由模型即時決定並執行的動作。
    #[serde(default)]
    pub live_actions: u32,
    /// 重播錄製而執行的動作（沒有呼叫模型）。
    #[serde(default)]
    pub replayed_actions: u32,
    /// no_live_model 擋下、沒有呼叫的模型呼叫。
    #[serde(default)]
    pub blocked_model_calls: u32,
}

/// 保存 Bundle 時對錄製的變更。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplayUpdate {
    /// 新的錄製（取代同一案例原有的錄製，狀態為 `valid`）。
    Record { case_key: String, recorded: RecordedAction },
    /// 把同一案例的錄製標為 stale。
    MarkStale { case_key: String, reasons: Vec<String> },
}

/// Bundle 對錄製的變更：有新錄製時取代，否則有 stale 原因時標記。
pub fn updates(bundle: &EvidenceBundle) -> Vec<ReplayUpdate> {
    bundle
        .actions
        .iter()
        .filter_map(|record| {
            let case_key = case_key(&record.journey, &record.step);
            // 含本次驗證佔位字的步驟不保存錄製
            let recorded = record.recorded.as_ref().filter(|r| !r.identity.run_scoped);
            match (recorded, record.stale_reasons.is_empty()) {
                (Some(recorded), _) => Some(ReplayUpdate::Record { case_key, recorded: recorded.clone() }),
                (None, false) => Some(ReplayUpdate::MarkStale { case_key, reasons: record.stale_reasons.clone() }),
                (None, true) => None,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests;
