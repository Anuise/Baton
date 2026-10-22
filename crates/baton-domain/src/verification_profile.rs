//! Verification Profile、Feature Map 與 CLI／API 驗證的判定（票 45，spec D-29）。
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

// ---------------------------------------------------------------- Profile 內容

/// 受驗的介面；本票只有 CLI／API（web 是票 46，mobile 延後）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Interface {
    CliApi,
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
    /// 需要人工或語意判斷的步驟：CLI／API Adapter 無法自動化，判定為 `automation_unsupported`。
    Manual { instruction: String },
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
            Operation::Manual { instruction } => format!("人工步驟：{instruction}"),
        }
    }

    fn timeout_ms(&self) -> Option<u64> {
        match self {
            Operation::Command { timeout_ms, .. } | Operation::Http { timeout_ms, .. } => *timeout_ms,
            Operation::Manual { .. } => None,
        }
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
}

impl Assertion {
    pub fn describe(&self) -> String {
        match self {
            Assertion::ExitCode { equals } => format!("結束碼為 {equals}"),
            Assertion::Status { equals } => format!("HTTP 狀態碼為 {equals}"),
            Assertion::Contains { text } => format!("包含「{text}」"),
            Assertion::NotContains { text } => format!("不包含「{text}」"),
            Assertion::JsonEquals { pointer, value } => format!("JSON {pointer} 等於 {value}"),
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
    pub steps: Vec<Step>,
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
}

fn default_output_max_chars() -> usize {
    DEFAULT_OUTPUT_MAX_CHARS
}

impl Default for EvidencePolicy {
    fn default() -> Self {
        Self { output_max_chars: DEFAULT_OUTPUT_MAX_CHARS, redact: Vec::new() }
    }
}

/// 一次驗證的預算。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerificationBudget {
    /// 最多執行幾次操作（含就緒重試，不含 teardown）；用盡後其餘步驟判定為 `budget_exhausted`。
    #[serde(default = "default_max_operations")]
    pub max_operations: u32,
}

fn default_max_operations() -> u32 {
    DEFAULT_MAX_OPERATIONS
}

impl Default for VerificationBudget {
    fn default() -> Self {
        Self { max_operations: DEFAULT_MAX_OPERATIONS }
    }
}

/// build 識別：執行這個命令，輸出的最後一行就是 build 識別。沒有設定時 build 以 candidate revision 表示。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildIdentity {
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
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
}

impl ProfileDefinition {
    pub fn journey(&self, key: &str) -> Option<&Journey> {
        self.journeys.iter().find(|j| j.key == key)
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
}

impl ProfileError {
    pub fn code(&self) -> &'static str {
        match self {
            ProfileError::Invalid(_) => "verification_profile_invalid",
            ProfileError::NotFound => "verification_profile_not_found",
            ProfileError::RequestInvalid(_) => "verification_request_invalid",
            ProfileError::BundleNotFound(_) => "evidence_bundle_not_found",
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
            for assertion in &step.assertions {
                match assertion {
                    Assertion::ExitCode { .. } if is_http => {
                        return Err(invalid(format!("{label}是 HTTP 請求，不能斷言結束碼")));
                    }
                    Assertion::Status { .. } if !is_http => {
                        return Err(invalid(format!("{label}是命令，不能斷言 HTTP 狀態碼")));
                    }
                    Assertion::Contains { text } | Assertion::NotContains { text } if text.is_empty() => {
                        return Err(invalid(format!("{label}的文字斷言不能空白")));
                    }
                    Assertion::JsonEquals { pointer, .. } if !(pointer.is_empty() || pointer.starts_with('/')) => {
                        return Err(invalid(format!("{label}的 JSON pointer「{pointer}」必須是空字串或以 / 開頭")));
                    }
                    _ => {}
                }
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
    Journey,
    Teardown,
}

/// Adapter 執行操作後觀察到的事實（Domain 不依賴 Port，由 Runtime 轉成這個型別）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Observation {
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
    for secret in &policy.redact {
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
    let s = |raw: &str| substitute(raw, run_id, state_dir);
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
        Operation::Manual { instruction } => Operation::Manual { instruction: instruction.clone() },
    }
}

fn check_assertion(assertion: &Assertion, observation: &Observation, policy: &EvidencePolicy) -> AssertionResult {
    let text = match observation {
        Observation::Exited { output, .. } => Some(output.as_str()),
        Observation::Responded { body, .. } => Some(body.as_str()),
        _ => None,
    };
    let (passed, actual) = match (assertion, observation) {
        (Assertion::ExitCode { equals }, Observation::Exited { code, .. }) => (code == equals, format!("結束碼 {code}")),
        (Assertion::Status { equals }, Observation::Responded { status, .. }) => {
            (status == equals, format!("HTTP {status}"))
        }
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
    let assertions: Vec<AssertionResult> = match &observation {
        Observation::Exited { .. } | Observation::Responded { .. } => {
            step.assertions.iter().map(|a| check_assertion(a, &observation, policy)).collect()
        }
        _ => Vec::new(),
    };
    let all_passed = !assertions.is_empty() && assertions.iter().all(|a| a.passed);
    let failed_reason = match phase {
        Phase::Journey => (Verdict::ProductFailure, "assertion_failed"),
        Phase::Build => (Verdict::EnvironmentBlocked, "build_unidentified"),
        Phase::Doctor => (Verdict::EnvironmentBlocked, "doctor_failed"),
        Phase::Start | Phase::Readiness => (Verdict::EnvironmentBlocked, "not_ready"),
        Phase::Setup => (Verdict::EnvironmentBlocked, "setup_failed"),
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
    pub verdict: Verdict,
    pub reason_code: String,
}

/// 依步驟紀錄判定一條旅程：第一個未通過的步驟決定結果（它之後的步驟沒有執行）。
pub fn judge_journey(journey: &Journey, steps: &[StepRecord]) -> JourneyResult {
    let mine: Vec<&StepRecord> = steps.iter().filter(|s| s.phase == Phase::Journey && s.journey.as_deref() == Some(&journey.key)).collect();
    let (verdict, reason_code) = if mine.len() != journey.steps.len() {
        (Verdict::Inconclusive, "journey_incomplete".to_string())
    } else if let Some(first) = mine.iter().find(|s| s.verdict != Verdict::Passed) {
        (first.verdict, first.reason_code.clone())
    } else {
        (Verdict::Passed, "all_steps_passed".to_string())
    };
    JourneyResult {
        key: journey.key.clone(),
        title: journey.title.clone(),
        representative: journey.representative,
        verdict,
        reason_code,
    }
}

/// 整份驗證的判定（teardown 不影響）：
/// - 任何旅程是產品缺陷 → `product_failure`（即使另有其他問題）；
/// - 否則前置階段（build、doctor、啟動、就緒、資料準備）第一個未通過的步驟決定結果；
/// - 否則第一條未通過的旅程決定結果；
/// - 沒有執行任何旅程 → `inconclusive`（沒有證據不算通過）；全部通過才是 `passed`。
pub fn aggregate(steps: &[StepRecord], journeys: &[JourneyResult]) -> (Verdict, String) {
    if let Some(failed) = journeys.iter().find(|j| j.verdict == Verdict::ProductFailure) {
        return (Verdict::ProductFailure, failed.reason_code.clone());
    }
    if let Some(blocked) = steps.iter().find(|s| !matches!(s.phase, Phase::Journey | Phase::Teardown) && s.verdict != Verdict::Passed) {
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
    pub started_at: Timestamp,
    pub finished_at: Timestamp,
}

/// Profile 版本是否已被證明可用：有一份這個版本的 Evidence Bundle，其中有代表旅程通過。回傳證明它的 Bundle 識別。
pub fn proven_by<'a>(profile: &ProfileVersion, bundles: &'a [EvidenceBundle]) -> Option<&'a EvidenceBundle> {
    bundles
        .iter()
        .filter(|b| b.profile_version_id == profile.id)
        .find(|b| b.journeys.iter().any(|j| j.representative && j.verdict == Verdict::Passed))
}

#[cfg(test)]
mod tests;
