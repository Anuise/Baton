//! Verification Profile、Feature Map 與 CLI／API、web 驗證的判定（票 45、46，spec D-29）。
//!
//! 純邏輯、無 I/O。驗證檢查的是真實介面的行為，不是 Worker 的說法：
//! - Profile 是 Workspace 的版本化內容（只追加）：doctor、啟動命令、就緒條件、資料準備、旅程（操作 + 程式斷言）、
//!   teardown、證據保存政策、預算與 Feature Map。每個步驟至少一個程式斷言；需要人工判斷的步驟以 `manual` 宣告。
//! - 操作與判定分離：Runtime 的 Adapter 只執行操作、取得觀察（`Observation`），判定由這裡的 `judge_step` 依斷言做出。
//! - 每個步驟保存 verdict（七種）與 reason_code（更細的機器可讀原因），來源引擎的原始代碼另存為 `engine_code`。
//!   證據不足（逾時、未執行）、環境阻礙、授權阻礙、預算用盡、無法自動化都不算通過，也不算產品缺陷。
//! - Evidence Bundle 綁定 build、candidate revision、需求（Task 契約版本）與 Profile 版本；只追加、不可修改。
//! - 新 Profile 版本要實際跑過至少一條代表旅程並通過才算「已驗證可用」；未覆蓋的範圍（Feature Map 中沒有旅程的
//!   功能與明確宣告的未覆蓋範圍）隨 Profile 與每份 Evidence Bundle 一起標示。
//! - web（票 46）：瀏覽器操作之後觀察當前畫面；需要語意判斷的斷言交給受限的 judge，輸入只有當前畫面與固定預期。
//!   模型限流或無法使用是證據不足（`inconclusive`），不是產品缺陷。秘密只以參照（`{{secret:KEY}}`）出現在 Profile 中，
//!   值只在 Runner 執行操作時代入，證據與 judge 的輸入都以值遮蔽。測試 persona（扮演的角色）與登入身分（實際的帳號）
//!   分開宣告、分開綁定到旅程；登入身分要以 `check` 步驟證明實際登入的是誰，不能因 persona 的名稱推定權限。
//! - 已驗證動作重播（票 48，見 `replay`）：目標導向動作（`act`）由模型依當前畫面決定一個具體動作；只有後續有效斷言
//!   證明結果的動作才錄製，重播時仍執行真實動作並重新檢查後置條件。`no_live_model` 的驗證要求不呼叫任何模型。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::artifact::sha256_hex;
use crate::event::DomainEvent;
use crate::Timestamp;

pub const TOOL_RUN_VERIFICATION: &str = "run_verification";
pub const TOOL_GET_VERIFICATION: &str = "get_verification";

pub const PROFILE_AGGREGATE: &str = "verification_profile";
pub const REQUEST_AGGREGATE: &str = "verification_request";
pub const EVENT_PROFILE_PUBLISHED: &str = "verification_profile_published";
pub const EVENT_VERIFICATION_REQUESTED: &str = "verification_requested";
pub const EVENT_VERIFICATION_COMPLETED: &str = "verification_completed";

/// 識別（旅程、步驟、功能）的長度上限；只能用小寫英數、`_` 與 `-`。
pub const KEY_MAX_CHARS: usize = 64;
/// 標題、說明等文字的長度上限。
pub const TEXT_MAX_CHARS: usize = 2_000;
/// 旅程、功能、每個階段的步驟數上限。
pub const LIST_MAX: usize = 50;
/// 就緒條件最多重試幾次。
pub const READINESS_ATTEMPTS_MAX: u32 = 60;
/// 就緒條件兩次嘗試之間最長的間隔（毫秒）。
pub const READINESS_INTERVAL_MAX_MS: u64 = 60_000;
/// 單一操作的逾時上限與預設值（毫秒）。
pub const OPERATION_TIMEOUT_MAX_MS: u64 = 10 * 60 * 1000;
pub const OPERATION_TIMEOUT_DEFAULT_MS: u64 = 60_000;
/// 一次驗證預設最多執行幾次操作（含就緒重試，不含 teardown）。
pub const DEFAULT_MAX_OPERATIONS: u32 = 200;
/// 證據中保存的輸出預設上限（字元數，保留結尾）。
pub const DEFAULT_OUTPUT_MAX_CHARS: usize = 4_000;
/// 證據輸出上限的最大值。
pub const OUTPUT_MAX_CHARS_LIMIT: usize = 20_000;
/// 遮蔽後顯示的文字。
pub const REDACTED: &str = "[已遮蔽]";

/// 操作中可使用的佔位字：本次驗證的識別與它專屬的狀態目錄（清理只移除這個目錄）。
pub const PLACEHOLDER_RUN_ID: &str = "{{run_id}}";
pub const PLACEHOLDER_STATE_DIR: &str = "{{state_dir}}";
/// 秘密參照的佔位字：`{{secret:KEY}}`，KEY 是 Profile `secrets` 中宣告的識別。
pub const PLACEHOLDER_SECRET_PREFIX: &str = "{{secret:";
/// 一次驗證預設最多呼叫幾次語意 judge（模型呼叫）。
pub const DEFAULT_MAX_MODEL_CALLS: u32 = 20;
/// 交給語意 judge 的畫面文字上限（字元數，保留開頭）。
pub const JUDGE_PAGE_TEXT_MAX_CHARS: usize = 20_000;
/// 單一證據 Artifact（截圖、trace、錄影）的大小上限（位元組）。
pub const EVIDENCE_ARTIFACT_MAX_BYTES: usize = 50 * 1024 * 1024;
/// 證據 Artifact 在成果儲存中的子目錄。
pub const EVIDENCE_ARTIFACTS_DIR: &str = "verification";

// ---------------------------------------------------------------- Profile 內容

/// 受驗的介面：CLI／API（票 45）與 web（票 46）；mobile 延後。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Interface {
    CliApi,
    /// 以 Playwright 為基礎的瀏覽器操作；也可以混用命令與 HTTP（例如以 API 準備資料）。
    Web,
}

/// 一個操作：Adapter 只負責執行並回報觀察，不做判定。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Operation {
    /// 執行命令（不經過 shell），工作目錄是受驗的目錄。
    Command {
        program: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_ms: Option<u64>,
    },
    /// 送出 HTTP 請求。
    Http {
        method: String,
        url: String,
        #[serde(default)]
        headers: Vec<Header>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_ms: Option<u64>,
    },
    /// 瀏覽器操作（只用於 `web` 介面）；之後觀察當前畫面。
    Browser {
        action: BrowserAction,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_ms: Option<u64>,
    },
    /// 需要人工判斷的步驟：Adapter 無法自動化，判定為 `automation_unsupported`。
    Manual { instruction: String },
}

/// 瀏覽器操作；`fill` 的文字可以用 `{{secret:KEY}}` 引用秘密。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BrowserAction {
    Goto { url: String },
    Click { selector: String },
    Fill { selector: String, text: String },
    Press { selector: String, key: String },
    WaitFor { selector: String },
    /// 目標導向動作（票 48）：模型依當前畫面、目標與參數決定一個具體動作（點擊、輸入或按鍵）。只能用在旅程步驟，
    /// 至少要有一個不靠模型的斷言（後置條件）；通過的動作會錄製，之後可以不呼叫模型重播。
    Act {
        goal: String,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        params: BTreeMap<String, String>,
        /// Profile 宣告為正式環境時，仍允許重播這個步驟的錄製（預設不允許：正式環境的副作用不重播）。
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        replay_in_production: bool,
    },
}

impl BrowserAction {
    fn describe(&self) -> String {
        match self {
            BrowserAction::Goto { url } => format!("瀏覽器前往 {url}"),
            BrowserAction::Click { selector } => format!("瀏覽器點擊「{selector}」"),
            BrowserAction::Fill { selector, text } => format!("瀏覽器在「{selector}」輸入「{text}」"),
            BrowserAction::Press { selector, key } => format!("瀏覽器在「{selector}」按下 {key}"),
            BrowserAction::WaitFor { selector } => format!("瀏覽器等待「{selector}」出現"),
            BrowserAction::Act { goal, .. } => format!("瀏覽器依目標動作：{goal}"),
        }
    }

    fn selector(&self) -> Option<&str> {
        match self {
            BrowserAction::Goto { .. } | BrowserAction::Act { .. } => None,
            BrowserAction::Click { selector }
            | BrowserAction::Fill { selector, .. }
            | BrowserAction::Press { selector, .. }
            | BrowserAction::WaitFor { selector } => Some(selector),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Header {
    pub name: String,
    pub value: String,
}

impl Operation {
    /// 給人看的一行描述（證據中保存的是遮蔽後的版本）。
    pub fn describe(&self) -> String {
        match self {
            Operation::Command { program, args, .. } => {
                let line: Vec<&str> = std::iter::once(program.as_str()).chain(args.iter().map(String::as_str)).collect();
                format!("執行「{}」", line.join(" "))
            }
            Operation::Http { method, url, .. } => format!("{method} {url}"),
            Operation::Browser { action, .. } => action.describe(),
            Operation::Manual { instruction } => format!("人工步驟：{instruction}"),
        }
    }

    fn timeout_ms(&self) -> Option<u64> {
        match self {
            Operation::Command { timeout_ms, .. } | Operation::Http { timeout_ms, .. } | Operation::Browser { timeout_ms, .. } => {
                *timeout_ms
            }
            Operation::Manual { .. } => None,
        }
    }

    /// 這個操作的全部文字欄位（用於找出秘密參照）。
    fn texts(&self) -> Vec<&str> {
        match self {
            Operation::Command { program, args, .. } => std::iter::once(program.as_str()).chain(args.iter().map(String::as_str)).collect(),
            Operation::Http { url, headers, body, .. } => std::iter::once(url.as_str())
                .chain(headers.iter().map(|h| h.value.as_str()))
                .chain(body.as_deref())
                .collect(),
            Operation::Browser { action, .. } => match action {
                BrowserAction::Goto { url } => vec![url],
                BrowserAction::Fill { selector, text } => vec![selector, text],
                BrowserAction::Press { selector, key } => vec![selector, key],
                BrowserAction::Click { selector } | BrowserAction::WaitFor { selector } => vec![selector],
                BrowserAction::Act { goal, params, .. } => {
                    std::iter::once(goal.as_str()).chain(params.values().map(String::as_str)).collect()
                }
            },
            Operation::Manual { instruction } => vec![instruction],
        }
    }

    /// 這個操作引用的秘密識別（`{{secret:KEY}}`），依出現順序、不重複。
    pub fn secret_keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = Vec::new();
        for text in self.texts() {
            for key in secret_keys_in(text) {
                if !keys.contains(&key) {
                    keys.push(key);
                }
            }
        }
        keys
    }

    /// 瀏覽器 `fill` 的 selector（輸入了秘密時，截圖要遮住這個欄位）。
    pub fn filled_selector(&self) -> Option<&str> {
        match self {
            Operation::Browser { action: BrowserAction::Fill { selector, .. }, .. } => Some(selector),
            _ => None,
        }
    }

    /// 目標導向動作（票 48）。
    pub fn is_act(&self) -> bool {
        matches!(self, Operation::Browser { action: BrowserAction::Act { .. }, .. })
    }

    /// 實際使用的逾時（毫秒）。
    pub fn effective_timeout_ms(&self) -> u64 {
        self.timeout_ms().unwrap_or(OPERATION_TIMEOUT_DEFAULT_MS)
    }
}

/// 程式斷言：判定只依觀察到的事實，不採信任何敘述。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Assertion {
    /// 命令的結束碼。
    ExitCode { equals: i32 },
    /// HTTP 狀態碼。
    Status { equals: u16 },
    /// 命令輸出或 HTTP 回應本文包含這段文字。
    Contains { text: String },
    /// 命令輸出或 HTTP 回應本文不包含這段文字。
    NotContains { text: String },
    /// 輸出或本文是 JSON，`pointer`（RFC 6901）指到的值等於 `value`。
    JsonEquals { pointer: String, value: serde_json::Value },
    /// 瀏覽器：這個 selector 對應的元素在當前畫面上可見。
    Visible { selector: String },
    /// 瀏覽器：當前網址包含這段文字。
    UrlContains { text: String },
    /// 瀏覽器：需要理解畫面語意時的受限 judge。輸入只有當前畫面與這段固定預期，不採信任何行動者的敘述。
    Judge { expectation: String },
}

impl Assertion {
    pub fn describe(&self) -> String {
        match self {
            Assertion::ExitCode { equals } => format!("結束碼為 {equals}"),
            Assertion::Status { equals } => format!("HTTP 狀態碼為 {equals}"),
            Assertion::Contains { text } => format!("包含「{text}」"),
            Assertion::NotContains { text } => format!("不包含「{text}」"),
            Assertion::JsonEquals { pointer, value } => format!("JSON {pointer} 等於 {value}"),
            Assertion::Visible { selector } => format!("「{selector}」可見"),
            Assertion::UrlContains { text } => format!("網址包含「{text}」"),
            Assertion::Judge { expectation } => format!("語意判斷：{expectation}"),
        }
    }
}

/// 一個步驟：一個操作與它的斷言。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    pub key: String,
    #[serde(default)]
    pub title: String,
    pub operation: Operation,
    #[serde(default)]
    pub assertions: Vec<Assertion>,
    /// 操作的副作用在本次驗證的隔離狀態之外（例如寄信、呼叫第三方付款）；只能用在旅程步驟（票 49）。
    /// 送出前先記入驗證日誌，結果未知或已送出而旅程沒有完成時，承接的嘗試不重送。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub external: bool,
}

/// 長駐的受驗服務（例如 API 伺服器）：驗證開始時啟動，teardown 時停止。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceSpec {
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
}

/// 就緒條件：一個有斷言的步驟，最多嘗試 `attempts` 次。「就緒」必須代表可使用，不能只是程序存在。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Readiness {
    pub step: Step,
    #[serde(default = "default_attempts")]
    pub attempts: u32,
    #[serde(default = "default_interval_ms")]
    pub interval_ms: u64,
}

fn default_attempts() -> u32 {
    10
}

fn default_interval_ms() -> u64 {
    1_000
}

/// 使用者旅程：依序執行的步驟；代表旅程用來證明新 Profile 實際可用。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Journey {
    pub key: String,
    pub title: String,
    #[serde(default)]
    pub representative: bool,
    /// 測試 persona（扮演的角色）；只是背景說明，不帶任何權限。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub persona: Option<String>,
    /// 登入身分（實際登入的帳號）；與 persona 分開綁定。沒有時以未登入狀態執行。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<String>,
    pub steps: Vec<Step>,
}

/// 秘密參照：Profile 與資料庫只保存參照（例如 `env:SHOP_PASSWORD`），值只在 Runner 執行操作時解析。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretRef {
    pub key: String,
    pub reference: String,
}

/// 測試 persona：旅程扮演的角色與目的（給 judge 的領域背景）。名稱不代表任何權限。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Persona {
    pub key: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
}

/// 登入身分：實際登入應用程式的帳號。`login` 執行登入（以秘密參照輸入帳密），`check` 證明實際登入的是誰。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    pub key: String,
    pub title: String,
    pub login: Vec<Step>,
    pub check: Step,
}

/// Feature Map 的一項：功能、對應的驗收準則，以及驗證它的旅程（空的就是未覆蓋）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Feature {
    pub key: String,
    pub title: String,
    /// 驗收準則（需求的文字或引用）。
    pub criterion: String,
    #[serde(default)]
    pub journeys: Vec<String>,
}

/// 明確宣告不在自動驗證範圍內的部分與原因。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UncoveredArea {
    pub area: String,
    pub reason: String,
}

/// 證據保存政策。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidencePolicy {
    /// 每個步驟保存的輸出上限（字元數，保留結尾）。
    #[serde(default = "default_output_max_chars")]
    pub output_max_chars: usize,
    /// 在證據（輸出、操作描述）中遮蔽的文字，例如測試用的權杖。
    #[serde(default)]
    pub redact: Vec<String>,
    /// 瀏覽器截圖的保存時機。
    #[serde(default, skip_serializing_if = "ScreenshotPolicy::is_default")]
    pub screenshots: ScreenshotPolicy,
    /// 保存瀏覽器 trace（Session 中輸入過秘密時不保存）。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub trace: bool,
    /// 保存瀏覽器錄影（Session 中輸入過秘密時不保存）。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub video: bool,
    /// 截圖時遮住的 selector（輸入過秘密的欄位會自動加入）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mask: Vec<String>,
}

/// 瀏覽器截圖的保存時機。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScreenshotPolicy {
    /// 只保存沒有通過的步驟的截圖。
    #[default]
    OnFailure,
    EveryStep,
    Off,
}

impl ScreenshotPolicy {
    fn is_default(&self) -> bool {
        *self == ScreenshotPolicy::default()
    }
}

fn default_output_max_chars() -> usize {
    DEFAULT_OUTPUT_MAX_CHARS
}

impl Default for EvidencePolicy {
    fn default() -> Self {
        Self {
            output_max_chars: DEFAULT_OUTPUT_MAX_CHARS,
            redact: Vec::new(),
            screenshots: ScreenshotPolicy::default(),
            trace: false,
            video: false,
            mask: Vec::new(),
        }
    }
}

/// 一次驗證的預算。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerificationBudget {
    /// 最多執行幾次操作（含就緒重試，不含 teardown）；用盡後其餘步驟判定為 `budget_exhausted`。
    #[serde(default = "default_max_operations")]
    pub max_operations: u32,
    /// 最多呼叫幾次語意 judge（模型呼叫）；用盡後需要 judge 的斷言判定為 `budget_exhausted`。
    #[serde(default = "default_max_model_calls", skip_serializing_if = "is_default_max_model_calls")]
    pub max_model_calls: u32,
}

fn default_max_operations() -> u32 {
    DEFAULT_MAX_OPERATIONS
}

fn default_max_model_calls() -> u32 {
    DEFAULT_MAX_MODEL_CALLS
}

fn is_default_max_model_calls(value: &u32) -> bool {
    *value == DEFAULT_MAX_MODEL_CALLS
}

impl Default for VerificationBudget {
    fn default() -> Self {
        Self { max_operations: DEFAULT_MAX_OPERATIONS, max_model_calls: DEFAULT_MAX_MODEL_CALLS }
    }
}

/// build 識別：執行這個命令，輸出的最後一行就是 build 識別。沒有設定時 build 以 candidate revision 表示。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildIdentity {
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
}

/// 錄製與重播的政策（票 48）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayPolicy {
    /// 受驗環境是正式環境：錄製的動作預設不重播（目標導向動作可以個別以 `replay_in_production` 允許）。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub production: bool,
    /// 資料集版本：資料相容性的一部分，改變時之前的錄製都是 stale。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dataset: Option<String>,
}

impl ReplayPolicy {
    fn is_default(&self) -> bool {
        *self == ReplayPolicy::default()
    }
}

/// Verification Profile 的內容（一個版本）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileDefinition {
    pub interface: Interface,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<BuildIdentity>,
    /// 環境前提檢查；未通過是 `environment_blocked`。
    #[serde(default)]
    pub doctor: Vec<Step>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<ServiceSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readiness: Option<Readiness>,
    /// 資料準備；未通過是 `environment_blocked`。
    #[serde(default)]
    pub setup: Vec<Step>,
    pub journeys: Vec<Journey>,
    /// 清理本次驗證的狀態；結果記在證據中，不改變判定。
    #[serde(default)]
    pub teardown: Vec<Operation>,
    #[serde(default)]
    pub evidence: EvidencePolicy,
    #[serde(default)]
    pub budget: VerificationBudget,
    pub feature_map: Vec<Feature>,
    #[serde(default)]
    pub uncovered: Vec<UncoveredArea>,
    /// 秘密參照（票 46）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secrets: Vec<SecretRef>,
    /// 測試 persona（票 46）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub personas: Vec<Persona>,
    /// 登入身分（票 46）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub identities: Vec<Identity>,
    /// 錄製與重播的政策（票 48）。
    #[serde(default, skip_serializing_if = "ReplayPolicy::is_default")]
    pub replay: ReplayPolicy,
}

impl ProfileDefinition {
    pub fn journey(&self, key: &str) -> Option<&Journey> {
        self.journeys.iter().find(|j| j.key == key)
    }

    pub fn persona(&self, key: &str) -> Option<&Persona> {
        self.personas.iter().find(|p| p.key == key)
    }

    pub fn identity(&self, key: &str) -> Option<&Identity> {
        self.identities.iter().find(|i| i.key == key)
    }

    pub fn representative_journeys(&self) -> Vec<&Journey> {
        self.journeys.iter().filter(|j| j.representative).collect()
    }
}

/// Profile 的一個不可變版本。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileVersion {
    pub id: String,
    pub workspace_id: String,
    pub version: i64,
    pub definition: ProfileDefinition,
    pub content_hash: String,
    /// 發布者：桌面憑證的指紋。
    pub published_by: String,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProfileError {
    #[error("Verification Profile 不正確：{0}")]
    Invalid(String),
    #[error("這個 Workspace 還沒有 Verification Profile")]
    NotFound,
    #[error("驗證要求不正確：{0}")]
    RequestInvalid(String),
    #[error("找不到 Evidence Bundle {0}")]
    BundleNotFound(String),
    #[error("找不到證據檔案：{0}")]
    ArtifactNotFound(String),
}

impl ProfileError {
    pub fn code(&self) -> &'static str {
        match self {
            ProfileError::Invalid(_) => "verification_profile_invalid",
            ProfileError::NotFound => "verification_profile_not_found",
            ProfileError::RequestInvalid(_) => "verification_request_invalid",
            ProfileError::BundleNotFound(_) => "evidence_bundle_not_found",
            ProfileError::ArtifactNotFound(_) => "evidence_artifact_not_found",
        }
    }
}

fn invalid(message: impl Into<String>) -> ProfileError {
    ProfileError::Invalid(message.into())
}

fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.chars().count() <= KEY_MAX_CHARS
        && key.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

fn check_text(text: &str, label: &str, required: bool) -> Result<(), ProfileError> {
    if required && text.trim().is_empty() {
        return Err(invalid(format!("{label}不能空白")));
    }
    if text.chars().count() > TEXT_MAX_CHARS {
        return Err(invalid(format!("{label}最多 {TEXT_MAX_CHARS} 個字元")));
    }
    Ok(())
}

fn check_count<T>(list: &[T], label: &str) -> Result<(), ProfileError> {
    if list.len() > LIST_MAX {
        return Err(invalid(format!("{label}最多 {LIST_MAX} 項")));
    }
    Ok(())
}

const HTTP_METHODS: &[&str] = &["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD"];

fn check_operation(operation: &Operation, label: &str) -> Result<(), ProfileError> {
    if let Some(timeout) = operation.timeout_ms() {
        if timeout == 0 || timeout > OPERATION_TIMEOUT_MAX_MS {
            return Err(invalid(format!("{label}的逾時必須在 1～{OPERATION_TIMEOUT_MAX_MS} 毫秒之間")));
        }
    }
    match operation {
        Operation::Command { program, .. } => check_text(program, &format!("{label}的程式"), true),
        Operation::Http { method, url, headers, .. } => {
            if !HTTP_METHODS.contains(&method.as_str()) {
                return Err(invalid(format!("{label}的 HTTP 方法必須是 {} 之一", HTTP_METHODS.join("、"))));
            }
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                return Err(invalid(format!("{label}的網址必須以 http:// 或 https:// 開頭")));
            }
            check_count(headers, &format!("{label}的標頭"))?;
            if headers.iter().any(|h| h.name.trim().is_empty()) {
                return Err(invalid(format!("{label}的標頭名稱不能空白")));
            }
            Ok(())
        }
        Operation::Browser { action, .. } => {
            if let BrowserAction::Goto { url } = action {
                if !(url.starts_with("http://") || url.starts_with("https://")) {
                    return Err(invalid(format!("{label}的網址必須以 http:// 或 https:// 開頭")));
                }
            }
            if let Some(selector) = action.selector() {
                check_text(selector, &format!("{label}的 selector"), true)?;
            }
            match action {
                BrowserAction::Fill { text, .. } => check_text(text, &format!("{label}的輸入文字"), false),
                BrowserAction::Press { key, .. } => check_text(key, &format!("{label}的按鍵"), true),
                BrowserAction::Act { goal, params, .. } => {
                    check_text(goal, &format!("{label}的目標"), true)?;
                    if params.len() > LIST_MAX {
                        return Err(invalid(format!("{label}的參數最多 {LIST_MAX} 項")));
                    }
                    for (key, value) in params {
                        if !valid_key(key) {
                            return Err(invalid(format!(
                                "{label}的參數識別「{key}」只能用小寫英數、_ 與 -（最多 {KEY_MAX_CHARS} 字）"
                            )));
                        }
                        check_text(value, &format!("{label}的參數「{key}」"), false)?;
                    }
                    if !operation.secret_keys().is_empty() {
                        return Err(invalid(format!("{label}的目標與參數會交給模型，不能引用秘密")));
                    }
                    Ok(())
                }
                _ => Ok(()),
            }
        }
        Operation::Manual { instruction } => check_text(instruction, &format!("{label}的說明"), true),
    }
}

/// 檢查一個步驟：識別、操作，以及斷言（自動化的步驟至少一個程式斷言，而且斷言適用於這種操作）。
fn check_step(step: &Step, label: &str) -> Result<(), ProfileError> {
    if !valid_key(&step.key) {
        return Err(invalid(format!("{label}的識別「{}」只能用小寫英數、_ 與 -（最多 {KEY_MAX_CHARS} 字）", step.key)));
    }
    let label = format!("{label}「{}」", step.key);
    check_text(&step.title, &format!("{label}的標題"), false)?;
    check_operation(&step.operation, &label)?;
    check_count(&step.assertions, &format!("{label}的斷言"))?;
    match &step.operation {
        Operation::Manual { .. } => {
            if !step.assertions.is_empty() {
                return Err(invalid(format!("{label}是人工步驟，不能有程式斷言")));
            }
        }
        operation => {
            if step.assertions.is_empty() {
                return Err(invalid(format!("{label}至少要有一個程式斷言；驗證不能只看操作有沒有執行")));
            }
            let is_http = matches!(operation, Operation::Http { .. });
            let is_browser = matches!(operation, Operation::Browser { .. });
            for assertion in &step.assertions {
                match assertion {
                    Assertion::ExitCode { .. } if is_http || is_browser => {
                        return Err(invalid(format!("{label}不是命令，不能斷言結束碼")));
                    }
                    Assertion::Status { .. } if !(is_http || is_browser) => {
                        return Err(invalid(format!("{label}是命令，不能斷言 HTTP 狀態碼")));
                    }
                    Assertion::JsonEquals { .. } if is_browser => {
                        return Err(invalid(format!("{label}是瀏覽器操作，不能斷言 JSON；請改用畫面文字或語意判斷")));
                    }
                    Assertion::Visible { .. } | Assertion::UrlContains { .. } | Assertion::Judge { .. } if !is_browser => {
                        return Err(invalid(format!("{label}不是瀏覽器操作，不能斷言畫面")));
                    }
                    Assertion::Contains { text } | Assertion::NotContains { text } | Assertion::UrlContains { text }
                        if text.is_empty() =>
                    {
                        return Err(invalid(format!("{label}的文字斷言不能空白")));
                    }
                    Assertion::Visible { selector } if selector.trim().is_empty() => {
                        return Err(invalid(format!("{label}的 selector 不能空白")));
                    }
                    Assertion::Judge { expectation } => check_text(expectation, &format!("{label}的語意判斷預期"), true)?,
                    Assertion::JsonEquals { pointer, .. } if !(pointer.is_empty() || pointer.starts_with('/')) => {
                        return Err(invalid(format!("{label}的 JSON pointer「{pointer}」必須是空字串或以 / 開頭")));
                    }
                    _ => {}
                }
            }
            if operation.is_act() && step.assertions.iter().all(|a| matches!(a, Assertion::Judge { .. })) {
                return Err(invalid(format!(
                    "{label}是目標導向動作，至少要有一個不靠模型的斷言（後置條件）；重播時要能不呼叫模型重新檢查"
                )));
            }
        }
    }
    Ok(())
}

fn check_unique<'a>(keys: impl Iterator<Item = &'a str>, label: &str) -> Result<(), ProfileError> {
    let mut seen = std::collections::HashSet::new();
    for key in keys {
        if !seen.insert(key) {
            return Err(invalid(format!("{label}「{key}」重複")));
        }
    }
    Ok(())
}

/// 發布前的檢查：結構完整、識別不重複、每個自動化步驟有程式斷言、至少一條代表旅程、Feature Map 引用的旅程存在。
pub fn validate(definition: &ProfileDefinition) -> Result<(), ProfileError> {
    if let Some(build) = &definition.build {
        check_text(&build.program, "build 識別的程式", true)?;
    }
    check_count(&definition.doctor, "doctor")?;
    for step in &definition.doctor {
        check_step(step, "doctor 步驟")?;
    }
    check_unique(definition.doctor.iter().map(|s| s.key.as_str()), "doctor 步驟")?;
    if let Some(start) = &definition.start {
        check_text(&start.program, "啟動命令的程式", true)?;
    }
    if let Some(readiness) = &definition.readiness {
        check_step(&readiness.step, "就緒條件")?;
        if matches!(readiness.step.operation, Operation::Manual { .. }) {
            return Err(invalid("就緒條件必須是可自動檢查的步驟"));
        }
        if readiness.attempts == 0 || readiness.attempts > READINESS_ATTEMPTS_MAX {
            return Err(invalid(format!("就緒條件的嘗試次數必須在 1～{READINESS_ATTEMPTS_MAX} 之間")));
        }
        if readiness.interval_ms > READINESS_INTERVAL_MAX_MS {
            return Err(invalid(format!("就緒條件的間隔最多 {READINESS_INTERVAL_MAX_MS} 毫秒")));
        }
    }
    check_count(&definition.setup, "資料準備")?;
    for step in &definition.setup {
        check_step(step, "資料準備步驟")?;
    }
    check_unique(definition.setup.iter().map(|s| s.key.as_str()), "資料準備步驟")?;
    if definition.journeys.is_empty() {
        return Err(invalid("至少要有一條旅程"));
    }
    check_count(&definition.journeys, "旅程")?;
    for journey in &definition.journeys {
        if !valid_key(&journey.key) {
            return Err(invalid(format!("旅程識別「{}」只能用小寫英數、_ 與 -（最多 {KEY_MAX_CHARS} 字）", journey.key)));
        }
        check_text(&journey.title, &format!("旅程「{}」的標題", journey.key), true)?;
        if journey.steps.is_empty() {
            return Err(invalid(format!("旅程「{}」至少要有一個步驟", journey.key)));
        }
        check_count(&journey.steps, &format!("旅程「{}」的步驟", journey.key))?;
        for step in &journey.steps {
            check_step(step, &format!("旅程「{}」的步驟", journey.key))?;
        }
        check_unique(journey.steps.iter().map(|s| s.key.as_str()), &format!("旅程「{}」的步驟", journey.key))?;
    }
    check_unique(definition.journeys.iter().map(|j| j.key.as_str()), "旅程")?;
    let representative: Vec<&Journey> = definition.representative_journeys();
    if representative.is_empty() {
        return Err(invalid("至少要有一條代表旅程（representative）；新 Profile 要實際跑過它才算可用"));
    }
    if representative.iter().any(|j| j.steps.iter().all(|s| matches!(s.operation, Operation::Manual { .. }))) {
        return Err(invalid("代表旅程必須至少有一個可自動化的步驟"));
    }
    check_count(&definition.teardown, "teardown")?;
    for (index, operation) in definition.teardown.iter().enumerate() {
        check_operation(operation, &format!("teardown 第 {} 項", index + 1))?;
        if matches!(operation, Operation::Manual { .. }) {
            return Err(invalid("teardown 必須是可自動執行的操作"));
        }
    }
    if definition.evidence.output_max_chars == 0 || definition.evidence.output_max_chars > OUTPUT_MAX_CHARS_LIMIT {
        return Err(invalid(format!("證據輸出上限必須在 1～{OUTPUT_MAX_CHARS_LIMIT} 個字元之間")));
    }
    check_count(&definition.evidence.redact, "遮蔽清單")?;
    if definition.evidence.redact.iter().any(|r| r.is_empty()) {
        return Err(invalid("遮蔽清單的每一項都不能是空字串"));
    }
    if definition.budget.max_operations == 0 {
        return Err(invalid("預算至少要允許一次操作"));
    }
    if definition.feature_map.is_empty() {
        return Err(invalid("Feature Map 至少要有一項功能"));
    }
    check_count(&definition.feature_map, "Feature Map")?;
    for feature in &definition.feature_map {
        if !valid_key(&feature.key) {
            return Err(invalid(format!("功能識別「{}」只能用小寫英數、_ 與 -（最多 {KEY_MAX_CHARS} 字）", feature.key)));
        }
        check_text(&feature.title, &format!("功能「{}」的標題", feature.key), true)?;
        check_text(&feature.criterion, &format!("功能「{}」的驗收準則", feature.key), true)?;
        if let Some(missing) = feature.journeys.iter().find(|j| definition.journey(j).is_none()) {
            return Err(invalid(format!("功能「{}」引用的旅程「{missing}」不存在", feature.key)));
        }
    }
    check_unique(definition.feature_map.iter().map(|f| f.key.as_str()), "功能")?;
    check_count(&definition.uncovered, "未覆蓋範圍")?;
    for area in &definition.uncovered {
        check_text(&area.area, "未覆蓋範圍", true)?;
        check_text(&area.reason, &format!("未覆蓋範圍「{}」的原因", area.area), true)?;
    }
    check_external_only_in_journeys(definition)?;
    check_secrets_personas_identities(definition)
}

/// `external` 只能用在旅程步驟（票 49）：前置步驟與登入在每次嘗試都重新執行，外部副作用會被重送。
fn check_external_only_in_journeys(definition: &ProfileDefinition) -> Result<(), ProfileError> {
    let prerequisite = definition.doctor.iter().chain(definition.readiness.iter().map(|r| &r.step)).chain(&definition.setup);
    let login = definition.identities.iter().flat_map(|i| i.login.iter().chain(std::iter::once(&i.check)));
    match prerequisite.chain(login).find(|s| s.external) {
        Some(step) => Err(invalid(format!(
            "步驟「{}」不是旅程步驟，不能標示 external；前置步驟與登入在承接時會重新執行",
            step.key
        ))),
        None => Ok(()),
    }
}

/// 秘密參照的格式：`env:NAME`（Runtime 的環境變數）。
fn valid_secret_reference(reference: &str) -> bool {
    reference.strip_prefix("env:").is_some_and(|name| {
        !name.is_empty()
            && name.len() <= 128
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            && !name.starts_with(|c: char| c.is_ascii_digit())
    })
}

/// Profile 中全部的操作（含登入身分的登入與核對、teardown）。
fn all_operations(definition: &ProfileDefinition) -> Vec<&Operation> {
    let steps = definition
        .doctor
        .iter()
        .chain(definition.readiness.iter().map(|r| &r.step))
        .chain(&definition.setup)
        .chain(definition.journeys.iter().flat_map(|j| &j.steps))
        .chain(definition.identities.iter().flat_map(|i| i.login.iter().chain(std::iter::once(&i.check))));
    steps.map(|s| &s.operation).chain(&definition.teardown).collect()
}

/// 票 46 的檢查：瀏覽器操作只用於 web 介面；秘密參照都有宣告；persona 與登入身分分開宣告，旅程引用的都存在；
/// 登入身分一定有核對實際身分的 `check` 步驟。
fn check_secrets_personas_identities(definition: &ProfileDefinition) -> Result<(), ProfileError> {
    let operations = all_operations(definition);
    if definition.interface != Interface::Web && operations.iter().any(|o| matches!(o, Operation::Browser { .. })) {
        return Err(invalid("只有 web 介面的 Profile 可以使用瀏覽器操作"));
    }
    // 瀏覽器 Session 只在旅程（含登入與身分核對）中存在
    let mut outside_journeys = definition
        .doctor
        .iter()
        .chain(definition.readiness.iter().map(|r| &r.step))
        .chain(&definition.setup)
        .map(|s| &s.operation)
        .chain(&definition.teardown);
    if outside_journeys.any(|o| matches!(o, Operation::Browser { .. })) {
        return Err(invalid("瀏覽器操作只能用在旅程與登入身分中；doctor、就緒、資料準備與 teardown 請改用命令或 HTTP"));
    }
    // 目標導向動作（票 48）只用在旅程步驟：登入與身分核對要輸入秘密、證明身分，必須是固定的操作
    let mut identity_steps = definition.identities.iter().flat_map(|i| i.login.iter().chain(std::iter::once(&i.check)));
    if identity_steps.any(|s| s.operation.is_act()) {
        return Err(invalid("目標導向動作只能用在旅程步驟；登入與身分核對必須是固定的操作"));
    }
    if let Some(dataset) = &definition.replay.dataset {
        check_text(dataset, "資料集版本", true)?;
    }
    check_count(&definition.secrets, "秘密參照")?;
    for secret in &definition.secrets {
        if !valid_key(&secret.key) {
            return Err(invalid(format!("秘密識別「{}」只能用小寫英數、_ 與 -（最多 {KEY_MAX_CHARS} 字）", secret.key)));
        }
        if !valid_secret_reference(&secret.reference) {
            return Err(invalid(format!(
                "秘密「{}」的參照必須是 env:<環境變數名稱>；Profile 只保存參照，不保存秘密的值",
                secret.key
            )));
        }
    }
    check_unique(definition.secrets.iter().map(|s| s.key.as_str()), "秘密")?;
    for operation in &operations {
        if let Some(missing) = operation.secret_keys().into_iter().find(|k| !definition.secrets.iter().any(|s| &s.key == k)) {
            return Err(invalid(format!("操作引用的秘密「{missing}」沒有在 secrets 中宣告")));
        }
    }
    check_count(&definition.personas, "persona")?;
    for persona in &definition.personas {
        if !valid_key(&persona.key) {
            return Err(invalid(format!("persona 識別「{}」只能用小寫英數、_ 與 -（最多 {KEY_MAX_CHARS} 字）", persona.key)));
        }
        check_text(&persona.title, &format!("persona「{}」的標題", persona.key), true)?;
        check_text(&persona.description, &format!("persona「{}」的說明", persona.key), false)?;
    }
    check_unique(definition.personas.iter().map(|p| p.key.as_str()), "persona")?;
    check_count(&definition.identities, "登入身分")?;
    for identity in &definition.identities {
        if !valid_key(&identity.key) {
            return Err(invalid(format!("登入身分識別「{}」只能用小寫英數、_ 與 -（最多 {KEY_MAX_CHARS} 字）", identity.key)));
        }
        let label = format!("登入身分「{}」", identity.key);
        check_text(&identity.title, &format!("{label}的標題"), true)?;
        if identity.login.is_empty() {
            return Err(invalid(format!("{label}至少要有一個登入步驟")));
        }
        check_count(&identity.login, &format!("{label}的登入步驟"))?;
        for step in &identity.login {
            check_step(step, &format!("{label}的登入步驟"))?;
            if matches!(step.operation, Operation::Manual { .. }) {
                return Err(invalid(format!("{label}的登入步驟必須可以自動執行")));
            }
        }
        check_unique(identity.login.iter().map(|s| s.key.as_str()), &format!("{label}的登入步驟"))?;
        check_step(&identity.check, &format!("{label}的身分核對"))?;
        if matches!(identity.check.operation, Operation::Manual { .. }) {
            return Err(invalid(format!("{label}的身分核對必須可以自動執行；不能只憑名稱推定登入的是誰")));
        }
    }
    check_unique(definition.identities.iter().map(|i| i.key.as_str()), "登入身分")?;
    for journey in &definition.journeys {
        if let Some(persona) = journey.persona.as_deref().filter(|p| definition.persona(p).is_none()) {
            return Err(invalid(format!("旅程「{}」引用的 persona「{persona}」不存在", journey.key)));
        }
        if let Some(identity) = journey.identity.as_deref().filter(|i| definition.identity(i).is_none()) {
            return Err(invalid(format!("旅程「{}」引用的登入身分「{identity}」不存在", journey.key)));
        }
    }
    Ok(())
}

fn hash_of<T: Serialize>(value: &T) -> String {
    sha256_hex(&serde_json::to_vec(value).expect("內容一定可以序列化"))
}

/// 發布 Profile 的新版本：與最新版本內容相同時不產生新版本（回 `None`）。
pub fn publish(
    latest: Option<&ProfileVersion>,
    workspace_id: &str,
    definition: ProfileDefinition,
    published_by: String,
    id: String,
    now: Timestamp,
) -> Result<Option<(ProfileVersion, DomainEvent)>, ProfileError> {
    validate(&definition)?;
    let content_hash = hash_of(&definition);
    if latest.is_some_and(|l| l.content_hash == content_hash) {
        return Ok(None);
    }
    let version = latest.map_or(1, |l| l.version + 1);
    let profile = ProfileVersion {
        id,
        workspace_id: workspace_id.to_string(),
        version,
        definition,
        content_hash,
        published_by,
        created_at: now,
    };
    let event = DomainEvent {
        aggregate_type: PROFILE_AGGREGATE,
        aggregate_id: workspace_id.to_string(),
        aggregate_version: version,
        event_type: EVENT_PROFILE_PUBLISHED,
        occurred_at: now,
        payload: serde_json::json!({
            "profile_version_id": profile.id,
            "version": version,
            "content_hash": profile.content_hash,
            "journeys": profile.definition.journeys.iter().map(|j| &j.key).collect::<Vec<_>>(),
            "uncovered": uncovered(&profile.definition),
        }),
    };
    Ok(Some((profile, event)))
}

// ---------------------------------------------------------------- 覆蓋範圍

/// 一項未覆蓋的範圍：Feature Map 中沒有旅程的功能、這次驗證沒有執行的功能，或明確宣告的未覆蓋範圍。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UncoveredItem {
    /// 功能識別（明確宣告的範圍沒有）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feature: Option<String>,
    pub area: String,
    pub reason: String,
}

/// Profile 本身未覆蓋的範圍：沒有旅程的功能，加上明確宣告的範圍。
pub fn uncovered(definition: &ProfileDefinition) -> Vec<UncoveredItem> {
    uncovered_by(definition, |feature| feature.journeys.is_empty(), "Feature Map 中沒有驗證這個功能的旅程")
}

/// 一次只執行 `journeys` 時未覆蓋的範圍：沒有任何一條對應旅程被執行的功能，加上 Profile 本身的未覆蓋範圍。
pub fn uncovered_by_run(definition: &ProfileDefinition, journeys: &[String]) -> Vec<UncoveredItem> {
    let mut items = uncovered(definition);
    items.extend(
        definition
            .feature_map
            .iter()
            .filter(|f| !f.journeys.is_empty() && !f.journeys.iter().any(|j| journeys.contains(j)))
            .map(|f| UncoveredItem {
                feature: Some(f.key.clone()),
                area: f.title.clone(),
                reason: "這次驗證沒有執行對應的旅程".into(),
            }),
    );
    items
}

fn uncovered_by(definition: &ProfileDefinition, missing: impl Fn(&Feature) -> bool, reason: &str) -> Vec<UncoveredItem> {
    let mut items: Vec<UncoveredItem> = definition
        .feature_map
        .iter()
        .filter(|f| missing(f))
        .map(|f| UncoveredItem { feature: Some(f.key.clone()), area: f.title.clone(), reason: reason.into() })
        .collect();
    items.extend(
        definition.uncovered.iter().map(|u| UncoveredItem { feature: None, area: u.area.clone(), reason: u.reason.clone() }),
    );
    items
}

/// 這次驗證覆蓋到的功能（至少一條對應旅程被執行）。
pub fn covered_features(definition: &ProfileDefinition, journeys: &[String]) -> Vec<String> {
    definition
        .feature_map
        .iter()
        .filter(|f| f.journeys.iter().any(|j| journeys.contains(j)))
        .map(|f| f.key.clone())
        .collect()
}

// ---------------------------------------------------------------- 驗證要求

/// 這次要執行的旅程：指定的旅程（空的是全部）；Profile 版本還沒被證明可用時一定加入代表旅程。
pub fn plan_journeys(definition: &ProfileDefinition, requested: &[String], proven: bool) -> Result<Vec<String>, ProfileError> {
    if let Some(missing) = requested.iter().find(|j| definition.journey(j).is_none()) {
        return Err(ProfileError::RequestInvalid(format!("Profile 沒有旅程「{missing}」")));
    }
    let wanted = |journey: &Journey| {
        requested.is_empty() || requested.contains(&journey.key) || (!proven && journey.representative)
    };
    Ok(definition.journeys.iter().filter(|j| wanted(j)).map(|j| j.key.clone()).collect())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestStatus {
    /// 等待 Runtime 執行。
    Queued,
    /// 已執行並保存 Evidence Bundle。
    Completed,
}

impl RequestStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            RequestStatus::Queued => "queued",
            RequestStatus::Completed => "completed",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        [RequestStatus::Queued, RequestStatus::Completed].into_iter().find(|s| s.as_str() == raw)
    }
}

/// 一次驗證的要求：固定要求當時的 Profile 版本與要執行的旅程。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerificationRequest {
    pub id: String,
    pub workspace_id: String,
    /// 受驗的 Task（在它的工作目錄中驗證，並綁定它的契約版本）；沒有時在 Workspace 目錄中驗證。
    pub task_id: Option<String>,
    pub profile_version_id: String,
    pub profile_version: i64,
    pub journeys: Vec<String>,
    /// 要求者：`desktop` 或 Controller 那一輪的 Run 識別。
    pub requested_by: String,
    pub status: RequestStatus,
    pub bundle_id: Option<String>,
    pub created_at: Timestamp,
    /// 不呼叫任何即時模型（票 48）：語意判斷與目標導向動作的規劃都被 Adapter 阻擋，只執行不需模型的斷言與重播。
    #[serde(default)]
    pub no_live_model: bool,
}

pub fn requested_event(request: &VerificationRequest) -> DomainEvent {
    DomainEvent {
        aggregate_type: REQUEST_AGGREGATE,
        aggregate_id: request.id.clone(),
        aggregate_version: 1,
        event_type: EVENT_VERIFICATION_REQUESTED,
        occurred_at: request.created_at,
        payload: serde_json::json!({
            "workspace_id": request.workspace_id,
            "task_id": request.task_id,
            "profile_version": request.profile_version,
            "journeys": request.journeys,
            "requested_by": request.requested_by,
            "no_live_model": request.no_live_model,
        }),
    }
}

pub fn completed_event(bundle: &EvidenceBundle) -> DomainEvent {
    DomainEvent {
        aggregate_type: REQUEST_AGGREGATE,
        aggregate_id: bundle.request_id.clone(),
        aggregate_version: 2,
        event_type: EVENT_VERIFICATION_COMPLETED,
        occurred_at: bundle.finished_at,
        payload: serde_json::json!({
            "workspace_id": bundle.workspace_id,
            "task_id": bundle.task_id,
            "evidence_bundle_id": bundle.id,
            "verdict": bundle.verdict,
            "reason_code": bundle.reason_code,
            "profile_version": bundle.profile_version,
            "candidate_revision": bundle.candidate_revision,
            "build": bundle.build,
        }),
    }
}

// ---------------------------------------------------------------- 判定

/// 步驟的判定（七種）；只有 `passed` 是通過。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Passed,
    /// 產品行為與斷言不符（產品缺陷）。
    ProductFailure,
    /// 證據不足（逾時、被終止、未執行）。
    Inconclusive,
    /// 環境阻礙（找不到程式、連不上、doctor／就緒／資料準備未通過）。
    EnvironmentBlocked,
    /// 授權阻礙（401／403）。
    AuthBlocked,
    /// 預算用盡。
    BudgetExhausted,
    /// 這個 Adapter 無法自動化的步驟。
    AutomationUnsupported,
}

impl Verdict {
    pub const ALL: [Verdict; 7] = [
        Verdict::Passed,
        Verdict::ProductFailure,
        Verdict::Inconclusive,
        Verdict::EnvironmentBlocked,
        Verdict::AuthBlocked,
        Verdict::BudgetExhausted,
        Verdict::AutomationUnsupported,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Passed => "passed",
            Verdict::ProductFailure => "product_failure",
            Verdict::Inconclusive => "inconclusive",
            Verdict::EnvironmentBlocked => "environment_blocked",
            Verdict::AuthBlocked => "auth_blocked",
            Verdict::BudgetExhausted => "budget_exhausted",
            Verdict::AutomationUnsupported => "automation_unsupported",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Verdict::ALL.into_iter().find(|v| v.as_str() == raw)
    }
}

/// 步驟屬於驗證的哪個階段；doctor、就緒與資料準備的斷言未通過是環境問題，只有旅程中的斷言未通過才是產品缺陷。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Build,
    Doctor,
    Start,
    Readiness,
    Setup,
    /// 旅程開始前，以旅程綁定的登入身分登入（票 46）；未通過是 `auth_blocked`。
    Login,
    /// 核對實際登入的身分（票 46）；未通過是 `auth_blocked`。
    Identity,
    Journey,
    Teardown,
}

/// 一個語意 judge 斷言的結果（由 Runtime 呼叫 judge 後轉成這個型別）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Judgement {
    Decided { satisfied: bool, rationale: String },
    /// 模型限流：無法判定，不是產品缺陷。
    RateLimited,
    /// 模型無法使用（沒有可用帳號、回應無法解讀等）。
    Unavailable { reason: String },
    /// 模型呼叫預算已用盡，沒有呼叫。
    BudgetExhausted,
    /// `no_live_model`：Adapter 禁止即時模型呼叫，沒有呼叫（票 48）。
    LiveModelDisabled,
}

/// Adapter 執行操作後觀察到的事實（Domain 不依賴 Port，由 Runtime 轉成這個型別）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Observation {
    /// 瀏覽器操作之後的當前畫面（票 46）。`visible` 是 (selector, 是否可見)；`judgements` 依步驟中語意判斷斷言的順序。
    Page { url: String, status: Option<u16>, text: String, visible: Vec<(String, bool)>, judgements: Vec<Judgement> },
    /// 瀏覽器操作本身沒有完成（例如找不到元素）；`error` 是引擎的錯誤名稱。
    ActionFailed { error: String, message: String },
    Exited { code: i32, output: String },
    /// 程序被強制結束而沒有結束碼。
    Terminated { output: String },
    TimedOut { output: String },
    /// 命令無法啟動（例如找不到程式）。
    NotStarted { reason: String },
    Responded { status: u16, body: String },
    /// 連不上（拒絕連線、DNS 失敗等）。
    Unreachable { reason: String },
    /// Adapter 不支援這種操作。
    Unsupported { reason: String },
}

/// 一個斷言的結果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssertionResult {
    pub assertion: Assertion,
    pub passed: bool,
    /// 實際觀察到的值（已遮蔽）。
    pub actual: String,
}

/// 一個步驟的紀錄（Evidence Bundle 的一部分）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepRecord {
    pub phase: Phase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub journey: Option<String>,
    pub key: String,
    pub title: String,
    /// 操作描述（已遮蔽，佔位字已代換）。
    pub operation: String,
    pub verdict: Verdict,
    /// 更細的機器可讀原因，例如 `assertion_failed`、`endpoint_unreachable`。
    pub reason_code: String,
    /// 來源引擎的原始代碼，例如 `exit:1`、`http:404`、`timeout`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine_code: Option<String>,
    pub assertions: Vec<AssertionResult>,
    /// 輸出或回應本文的結尾（已遮蔽）。
    pub output: String,
    /// 實際執行的次數（就緒條件可能重試）。
    pub attempts: u32,
}

/// 遮蔽與截斷：先遮蔽政策列出的文字，再保留結尾的 `max` 個字元。
pub fn sanitize(text: &str, policy: &EvidencePolicy) -> String {
    let mut text = text.to_string();
    for secret in policy.redact.iter().filter(|s| !s.is_empty()) {
        text = text.replace(secret.as_str(), REDACTED);
    }
    let count = text.chars().count();
    if count <= policy.output_max_chars {
        return text;
    }
    let tail: String = text.chars().skip(count - policy.output_max_chars).collect();
    format!("…{tail}")
}

/// 代換操作中的佔位字。
pub fn substitute(raw: &str, run_id: &str, state_dir: &str) -> String {
    raw.replace(PLACEHOLDER_RUN_ID, run_id).replace(PLACEHOLDER_STATE_DIR, state_dir)
}

/// 代換一個操作中所有欄位的佔位字。
pub fn bind_operation(operation: &Operation, run_id: &str, state_dir: &str) -> Operation {
    map_operation(operation, |raw| substitute(raw, run_id, state_dir))
}

/// 以 `f` 轉換操作中所有可以含佔位字的欄位（HTTP 方法與標頭名稱除外）。
fn map_operation(operation: &Operation, f: impl Fn(&str) -> String) -> Operation {
    let s = |raw: &str| f(raw);
    match operation {
        Operation::Command { program, args, timeout_ms } => Operation::Command {
            program: s(program),
            args: args.iter().map(|a| s(a)).collect(),
            timeout_ms: *timeout_ms,
        },
        Operation::Http { method, url, headers, body, timeout_ms } => Operation::Http {
            method: method.clone(),
            url: s(url),
            headers: headers.iter().map(|h| Header { name: h.name.clone(), value: s(&h.value) }).collect(),
            body: body.as_deref().map(s),
            timeout_ms: *timeout_ms,
        },
        Operation::Browser { action, timeout_ms } => Operation::Browser {
            action: match action {
                BrowserAction::Goto { url } => BrowserAction::Goto { url: s(url) },
                BrowserAction::Click { selector } => BrowserAction::Click { selector: s(selector) },
                BrowserAction::Fill { selector, text } => BrowserAction::Fill { selector: s(selector), text: s(text) },
                BrowserAction::Press { selector, key } => BrowserAction::Press { selector: s(selector), key: s(key) },
                BrowserAction::WaitFor { selector } => BrowserAction::WaitFor { selector: s(selector) },
                BrowserAction::Act { goal, params, replay_in_production } => BrowserAction::Act {
                    goal: s(goal),
                    params: params.iter().map(|(k, v)| (k.clone(), s(v))).collect(),
                    replay_in_production: *replay_in_production,
                },
            },
            timeout_ms: *timeout_ms,
        },
        Operation::Manual { instruction } => Operation::Manual { instruction: instruction.clone() },
    }
}

/// 文字中引用的秘密識別（`{{secret:KEY}}`），依出現順序。
pub fn secret_keys_in(text: &str) -> Vec<String> {
    let mut keys = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(PLACEHOLDER_SECRET_PREFIX) {
        let after = &rest[start + PLACEHOLDER_SECRET_PREFIX.len()..];
        let Some(end) = after.find("}}") else { break };
        keys.push(after[..end].to_string());
        rest = &after[end + 2..];
    }
    keys
}

/// 把秘密的值代入操作（只在 Runner 執行操作的當下使用；結果不得寫入任何紀錄）。`values` 是 (識別, 值)。
pub fn reveal_secrets(operation: &Operation, values: &[(String, String)]) -> Operation {
    map_operation(operation, |raw| {
        let mut text = raw.to_string();
        for (key, value) in values {
            text = text.replace(&format!("{PLACEHOLDER_SECRET_PREFIX}{key}}}}}"), value);
        }
        text
    })
}

fn check_assertion(assertion: &Assertion, observation: &Observation, policy: &EvidencePolicy) -> AssertionResult {
    let text = match observation {
        Observation::Exited { output, .. } => Some(output.as_str()),
        Observation::Responded { body, .. } => Some(body.as_str()),
        Observation::Page { text, .. } => Some(text.as_str()),
        _ => None,
    };
    let (passed, actual) = match (assertion, observation) {
        (Assertion::ExitCode { equals }, Observation::Exited { code, .. }) => (code == equals, format!("結束碼 {code}")),
        (Assertion::Status { equals }, Observation::Responded { status, .. }) => {
            (status == equals, format!("HTTP {status}"))
        }
        (Assertion::Status { equals }, Observation::Page { status, .. }) => match status {
            Some(status) => (status == equals, format!("HTTP {status}")),
            None => (false, "這個 Session 還沒有導覽回應".into()),
        },
        (Assertion::Visible { selector }, Observation::Page { visible, .. }) => {
            match visible.iter().find(|(s, _)| s == selector) {
                Some((_, true)) => (true, "可見".into()),
                Some((_, false)) => (false, "不可見或不存在".into()),
                None => (false, "沒有檢查這個 selector".into()),
            }
        }
        (Assertion::UrlContains { text: wanted }, Observation::Page { url, .. }) => (url.contains(wanted.as_str()), url.clone()),
        (Assertion::Contains { text: wanted }, _) if text.is_some() => {
            let found = text.unwrap_or_default().contains(wanted.as_str());
            (found, if found { "包含".into() } else { "不包含".into() })
        }
        (Assertion::NotContains { text: unwanted }, _) if text.is_some() => {
            let found = text.unwrap_or_default().contains(unwanted.as_str());
            (!found, if found { "包含".into() } else { "不包含".into() })
        }
        (Assertion::JsonEquals { pointer, value }, _) if text.is_some() => {
            match serde_json::from_str::<serde_json::Value>(text.unwrap_or_default()) {
                Err(error) => (false, format!("不是 JSON：{error}")),
                Ok(document) => match document.pointer(pointer) {
                    None => (false, format!("{pointer} 不存在")),
                    Some(actual) => (actual == value, actual.to_string()),
                },
            }
        }
        _ => (false, "沒有可供判定的觀察".into()),
    };
    AssertionResult { assertion: assertion.clone(), passed, actual: sanitize(&actual, policy) }
}

/// 語意判斷斷言的結果；沒有做出判定時另外回傳 (判定, reason_code, engine_code)。
fn judgement_result(
    assertion: &Assertion,
    judgement: Option<&Judgement>,
    policy: &EvidencePolicy,
) -> (AssertionResult, Option<(Verdict, &'static str, &'static str)>) {
    let (passed, actual, problem) = match judgement {
        Some(Judgement::Decided { satisfied, rationale }) => (*satisfied, rationale.clone(), None),
        Some(Judgement::RateLimited) => {
            (false, "模型限流，無法判定".to_string(), Some((Verdict::Inconclusive, "judge_rate_limited", "model:rate_limited")))
        }
        Some(Judgement::Unavailable { reason }) => (
            false,
            format!("語意判斷無法使用：{reason}"),
            Some((Verdict::Inconclusive, "judge_unavailable", "model:unavailable")),
        ),
        Some(Judgement::BudgetExhausted) => (
            false,
            "模型呼叫預算已用盡，沒有判定".to_string(),
            Some((Verdict::BudgetExhausted, "model_call_budget_exhausted", "model:budget_exhausted")),
        ),
        Some(Judgement::LiveModelDisabled) => (
            false,
            "no_live_model：不呼叫模型，沒有判定".to_string(),
            Some((Verdict::Inconclusive, "live_model_disabled", "model:disabled")),
        ),
        None => (
            false,
            "沒有語意判斷的結果".to_string(),
            Some((Verdict::Inconclusive, "judge_unavailable", "model:unavailable")),
        ),
    };
    (AssertionResult { assertion: assertion.clone(), passed, actual: sanitize(&actual, policy) }, problem)
}

/// 斷言明確預期這個授權相關的狀態碼（例如測「未登入會被拒絕」）時，401／403 不是授權阻礙。
fn expects_status(step: &Step, status: u16) -> bool {
    step.assertions.iter().any(|a| matches!(a, Assertion::Status { equals } if *equals == status))
}

/// 判定一個步驟：依觀察到的事實與斷言，不採信任何敘述。
pub fn judge_step(
    phase: Phase,
    journey: Option<&str>,
    step: &Step,
    operation: &Operation,
    observation: Observation,
    attempts: u32,
    policy: &EvidencePolicy,
) -> StepRecord {
    // 每個斷言的結果；語意判斷沒有做出判定（限流、無法使用、預算用盡）時另外記下原因，那不是斷言不符
    let mut judgements = match &observation {
        Observation::Page { judgements, .. } => judgements.iter(),
        _ => [].iter(),
    };
    let mut undecided: Option<(Verdict, &'static str, &'static str)> = None;
    let mut decided_failure = false;
    let assertions: Vec<AssertionResult> = match &observation {
        Observation::Exited { .. } | Observation::Responded { .. } | Observation::Page { .. } => step
            .assertions
            .iter()
            .map(|assertion| {
                let (result, problem) = match assertion {
                    Assertion::Judge { .. } => judgement_result(assertion, judgements.next(), policy),
                    _ => (check_assertion(assertion, &observation, policy), None),
                };
                match problem {
                    Some(problem) => {
                        undecided.get_or_insert(problem);
                    }
                    None if !result.passed => decided_failure = true,
                    None => {}
                }
                result
            })
            .collect(),
        _ => Vec::new(),
    };
    let all_passed = !assertions.is_empty() && assertions.iter().all(|a| a.passed);
    let failed_reason = match phase {
        Phase::Journey => (Verdict::ProductFailure, "assertion_failed"),
        Phase::Build => (Verdict::EnvironmentBlocked, "build_unidentified"),
        Phase::Doctor => (Verdict::EnvironmentBlocked, "doctor_failed"),
        Phase::Start | Phase::Readiness => (Verdict::EnvironmentBlocked, "not_ready"),
        Phase::Setup => (Verdict::EnvironmentBlocked, "setup_failed"),
        Phase::Login => (Verdict::AuthBlocked, "login_failed"),
        Phase::Identity => (Verdict::AuthBlocked, "identity_not_confirmed"),
        Phase::Teardown => (Verdict::EnvironmentBlocked, "teardown_failed"),
    };
    let (verdict, reason_code, engine_code, output) = match &observation {
        Observation::Exited { code, output } => {
            let (verdict, reason) = if all_passed { (Verdict::Passed, "assertions_passed") } else { failed_reason };
            (verdict, reason, Some(format!("exit:{code}")), output.clone())
        }
        Observation::Responded { status, body } => {
            let (verdict, reason) = if all_passed {
                (Verdict::Passed, "assertions_passed")
            } else if matches!(status, 401 | 403) && !expects_status(step, *status) {
                (Verdict::AuthBlocked, if *status == 401 { "http_unauthorized" } else { "http_forbidden" })
            } else {
                failed_reason
            };
            (verdict, reason, Some(format!("http:{status}")), body.clone())
        }
        // 畫面：程式斷言或語意判斷明確不符才是失敗；只有語意判斷沒有做出判定時，是證據不足或預算用盡
        Observation::Page { status, text, .. } => {
            let http = status.map(|s| format!("http:{s}"));
            let (verdict, reason, engine) = if all_passed {
                (Verdict::Passed, "assertions_passed", http)
            } else if let Some(status) = status.filter(|s| matches!(s, 401 | 403) && !expects_status(step, *s)) {
                (Verdict::AuthBlocked, if status == 401 { "http_unauthorized" } else { "http_forbidden" }, http)
            } else if let (false, Some((verdict, reason, engine))) = (decided_failure, undecided) {
                (verdict, reason, Some(engine.to_string()))
            } else {
                (failed_reason.0, failed_reason.1, http)
            };
            (verdict, reason, engine, text.clone())
        }
        Observation::ActionFailed { error, message } => {
            (Verdict::Inconclusive, "action_failed", Some(format!("playwright:{error}")), message.clone())
        }
        Observation::Terminated { output } => {
            (Verdict::Inconclusive, "operation_terminated", Some("terminated".into()), output.clone())
        }
        Observation::TimedOut { output } => (Verdict::Inconclusive, "operation_timed_out", Some("timeout".into()), output.clone()),
        Observation::NotStarted { reason } => {
            (Verdict::EnvironmentBlocked, "operation_not_started", Some("not_started".into()), reason.clone())
        }
        Observation::Unreachable { reason } => {
            (Verdict::EnvironmentBlocked, "endpoint_unreachable", Some("unreachable".into()), reason.clone())
        }
        Observation::Unsupported { reason } => (Verdict::AutomationUnsupported, "manual_step", None, reason.clone()),
    };
    StepRecord {
        phase,
        journey: journey.map(str::to_string),
        key: step.key.clone(),
        title: step.title.clone(),
        operation: sanitize(&operation.describe(), policy),
        verdict,
        reason_code: reason_code.into(),
        engine_code,
        assertions,
        output: sanitize(&output, policy),
        attempts,
    }
}

/// 沒有執行的步驟（預算用盡、前一步未通過、前置階段受阻）的紀錄。
pub fn not_run(
    phase: Phase,
    journey: Option<&str>,
    step: &Step,
    verdict: Verdict,
    reason_code: &str,
    policy: &EvidencePolicy,
) -> StepRecord {
    StepRecord {
        phase,
        journey: journey.map(str::to_string),
        key: step.key.clone(),
        title: step.title.clone(),
        operation: sanitize(&step.operation.describe(), policy),
        verdict,
        reason_code: reason_code.into(),
        engine_code: None,
        assertions: Vec::new(),
        output: String::new(),
        attempts: 0,
    }
}

/// 一條旅程的結果：全部步驟都執行且通過才是 `passed`；單一旅程未完成不能拼接為通過。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JourneyResult {
    pub key: String,
    pub title: String,
    pub representative: bool,
    /// 旅程扮演的 persona（票 46）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub persona: Option<String>,
    /// 旅程實際登入的身分（票 46）；與 persona 分開記錄。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<String>,
    pub verdict: Verdict,
    pub reason_code: String,
}

/// 依步驟紀錄判定一條旅程：屬於這條旅程的紀錄（瀏覽器啟動、登入、身分核對、旅程步驟）中，
/// 第一個未通過的決定結果（它之後的步驟沒有執行）；旅程步驟不齊全時是 `inconclusive`。
pub fn judge_journey(journey: &Journey, steps: &[StepRecord]) -> JourneyResult {
    let mine: Vec<&StepRecord> =
        steps.iter().filter(|s| s.phase != Phase::Teardown && s.journey.as_deref() == Some(&journey.key)).collect();
    let journey_steps = mine.iter().filter(|s| s.phase == Phase::Journey).count();
    let (verdict, reason_code) = if let Some(first) = mine.iter().find(|s| s.verdict != Verdict::Passed) {
        (first.verdict, first.reason_code.clone())
    } else if journey_steps != journey.steps.len() {
        (Verdict::Inconclusive, "journey_incomplete".to_string())
    } else {
        (Verdict::Passed, "all_steps_passed".to_string())
    };
    JourneyResult {
        key: journey.key.clone(),
        title: journey.title.clone(),
        representative: journey.representative,
        persona: journey.persona.clone(),
        identity: journey.identity.clone(),
        verdict,
        reason_code,
    }
}

/// 整份驗證的判定（teardown 不影響）：
/// - 任何旅程是產品缺陷 → `product_failure`（即使另有其他問題）；
/// - 否則前置階段（build、doctor、啟動、就緒、資料準備；不屬於任何旅程）第一個未通過的步驟決定結果；
/// - 否則第一條未通過的旅程決定結果（登入與身分核對屬於旅程）；
/// - 沒有執行任何旅程 → `inconclusive`（沒有證據不算通過）；全部通過才是 `passed`。
pub fn aggregate(steps: &[StepRecord], journeys: &[JourneyResult]) -> (Verdict, String) {
    if let Some(failed) = journeys.iter().find(|j| j.verdict == Verdict::ProductFailure) {
        return (Verdict::ProductFailure, failed.reason_code.clone());
    }
    if let Some(blocked) =
        steps.iter().find(|s| s.journey.is_none() && s.phase != Phase::Teardown && s.verdict != Verdict::Passed)
    {
        return (blocked.verdict, blocked.reason_code.clone());
    }
    if let Some(first) = journeys.iter().find(|j| j.verdict != Verdict::Passed) {
        return (first.verdict, first.reason_code.clone());
    }
    if journeys.is_empty() {
        return (Verdict::Inconclusive, "no_journeys".into());
    }
    (Verdict::Passed, "all_journeys_passed".into())
}

// ---------------------------------------------------------------- Evidence Bundle

/// 清理的結果：只移除本次驗證的狀態，證據保留在 Evidence Bundle。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CleanupRecord {
    /// 本次驗證專屬的狀態目錄（`{{state_dir}}`）。
    pub state_dir: String,
    pub state_dir_removed: bool,
    /// 本次驗證啟動的服務是否已停止（沒有啟動服務時為 `None`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_stopped: Option<bool>,
    /// 清理失敗的原因。
    #[serde(default)]
    pub problems: Vec<String>,
}

/// 驗證的證據集合：綁定 build、candidate revision、需求與 Profile 版本；只追加、不可修改。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceBundle {
    pub id: String,
    pub workspace_id: String,
    pub request_id: String,
    pub task_id: Option<String>,
    /// 受驗 Task 最新的 Run 與世代（驗證當時）。
    pub run_id: Option<String>,
    pub generation: Option<i64>,
    /// 需求：受驗 Task 驗證當時的契約版本。
    pub requirement_version: Option<i64>,
    pub profile_version_id: String,
    pub profile_version: i64,
    /// 受驗目錄的 Git HEAD（不是 Git repo 時為 `None`）。
    pub candidate_revision: Option<String>,
    /// build 識別（Profile 的 build 命令輸出；沒有設定時是 `revision:<commit>`；都取不到時為 `None`）。
    pub build: Option<String>,
    /// 受驗的目錄。
    pub subject_dir: String,
    pub journeys: Vec<JourneyResult>,
    pub steps: Vec<StepRecord>,
    pub verdict: Verdict,
    pub reason_code: String,
    pub covered_features: Vec<String>,
    pub uncovered: Vec<UncoveredItem>,
    pub cleanup: CleanupRecord,
    /// 截圖、trace、錄影（票 46）；依政策遮蔽，無法確認不含秘密的不保存。
    #[serde(default)]
    pub artifacts: Vec<EvidenceArtifact>,
    /// 這次驗證禁止即時模型呼叫（票 48）。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub no_live_model: bool,
    /// 目標導向動作的來源（live／replay）與錄製的處理（票 48）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<replay::ActionRecord>,
    /// 即時模型與重播分開計算的用量（票 48）。
    #[serde(default)]
    pub usage: replay::VerificationUsage,
    /// 每次嘗試的摘要（票 49）：帳號交接或 Runtime 中斷後由新的嘗試接手；每條旅程的結果只來自一次嘗試。
    #[serde(default)]
    pub attempts: Vec<crate::verification_attempt::AttemptSummary>,
    pub started_at: Timestamp,
    pub finished_at: Timestamp,
}

/// 證據 Artifact 的種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceArtifactKind {
    Screenshot,
    Trace,
    Video,
}

impl EvidenceArtifactKind {
    pub fn content_type(self) -> &'static str {
        match self {
            EvidenceArtifactKind::Screenshot => "image/png",
            EvidenceArtifactKind::Trace => "application/zip",
            EvidenceArtifactKind::Video => "video/webm",
        }
    }
}

/// 一個證據 Artifact：保存在成果儲存（`storage_path` 相對於成果根目錄，內容定址）；沒有保存時記下原因。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceArtifact {
    pub kind: EvidenceArtifactKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub journey: Option<String>,
    /// 截圖所屬的步驟。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<String>,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    /// 沒有保存的原因：`secret_detected`（內容含秘密的值，或畫面顯示過秘密）、`secret_input_recorded`（輸入過秘密的 Session 的
    /// trace 或錄影，無法確認已遮蔽）、`too_large`、`storage_failed`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub withheld: Option<String>,
}

/// 證據 Artifact 依政策不能保存的原因（`None` 是可以保存）。不能假設引擎的 trace 與錄影已經遮蔽：
/// Session 中輸入過秘密時一律不保存 trace 與錄影。影像與壓縮檔中的文字無法從位元組找到，所以
/// `secret_shown`（截圖：這個畫面顯示秘密；trace 與錄影：這個 Session 的畫面顯示過秘密）時也不保存；
/// 任何內容含有秘密的值同樣不保存。
pub fn withhold_reason(
    kind: EvidenceArtifactKind,
    bytes: &[u8],
    secrets: &[String],
    secret_entered: bool,
    secret_shown: bool,
) -> Option<&'static str> {
    if bytes.len() > EVIDENCE_ARTIFACT_MAX_BYTES {
        return Some("too_large");
    }
    if secret_entered && matches!(kind, EvidenceArtifactKind::Trace | EvidenceArtifactKind::Video) {
        return Some("secret_input_recorded");
    }
    let contains = |needle: &[u8]| !needle.is_empty() && bytes.windows(needle.len()).any(|w| w == needle);
    if secret_shown || secrets.iter().any(|s| contains(s.as_bytes())) {
        return Some("secret_detected");
    }
    None
}

/// Profile 版本是否已被證明可用：有一份這個版本的 Evidence Bundle，其中有代表旅程通過。回傳證明它的 Bundle 識別。
pub fn proven_by<'a>(profile: &ProfileVersion, bundles: &'a [EvidenceBundle]) -> Option<&'a EvidenceBundle> {
    bundles
        .iter()
        .filter(|b| b.profile_version_id == profile.id)
        .find(|b| b.journeys.iter().any(|j| j.representative && j.verdict == Verdict::Passed))
}

pub mod replay;

#[cfg(test)]
mod tests;
