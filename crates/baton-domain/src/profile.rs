//! account_profile 的狀態機與「可不可以派工」的判定（spec D-32；使用者故事 4、5）。
//!
//! 純邏輯、無 I/O。一個 Profile 對應一個獨立的 `CLAUDE_CONFIG_DIR`，只承載原生登入狀態；
//! 這裡只保存「核對結果」（登入與否、認證來源、帳號身分），不保存任何登入資料或 token。
//! 所有狀態變更都只能經過 `decide`；Store 在一個交易中套用它的結果。

use serde::{Deserialize, Serialize};

use crate::claude_code::{self, VersionCheck};
use crate::event::DomainEvent;
use crate::{workspace, Timestamp};

pub const AGGREGATE_TYPE: &str = "account_profile";
pub const EVENT_CREATED: &str = "account_profile_created";
pub const EVENT_VERIFIED: &str = "account_profile_verified";
pub const EVENT_IDENTITY_PINNED: &str = "account_profile_identity_pinned";
pub const EVENT_ARCHIVED: &str = "account_profile_archived";

/// 第一版支援的 account_profile 數量上限（spec D-32）。
pub const MAX_PROFILES: usize = 8;

/// Profile 名稱的長度上限（字元數）。
pub const MAX_NAME_CHARS: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileStatus {
    Active,
    Archived,
}

impl ProfileStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            ProfileStatus::Active => "active",
            ProfileStatus::Archived => "archived",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "active" => Some(ProfileStatus::Active),
            "archived" => Some(ProfileStatus::Archived),
            _ => None,
        }
    }
}

/// 設定目錄的來源：平台在資料目錄下建立，或使用者登記的既有（外部）目錄。
/// 平台不刪除任何設定目錄；外部目錄尤其不可修改，因為使用者在平台外也在用它。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigDirOrigin {
    Managed,
    External,
}

impl ConfigDirOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            ConfigDirOrigin::Managed => "managed",
            ConfigDirOrigin::External => "external",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "managed" => Some(ConfigDirOrigin::Managed),
            "external" => Some(ConfigDirOrigin::External),
            _ => None,
        }
    }
}

/// Claude Code 實際使用的認證來源。只有 `NativeLogin`（此 Profile 設定目錄中的原生登入）可以派工；
/// 環境變數或 credential helper 提供的認證會讓實際帳號與 Profile 脫鉤。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthSource {
    NativeLogin,
    /// `ANTHROPIC_API_KEY` 或 `apiKeyHelper`。
    ApiKey,
    /// `CLAUDE_CODE_OAUTH_TOKEN` 或 `ANTHROPIC_AUTH_TOKEN`。
    OauthToken,
    /// Bedrock、Vertex 等第三方供應商。
    ThirdPartyProvider,
    /// CLI 回報了這個版本不認識的認證方式。
    Other,
    /// 沒有任何認證。
    None,
}

/// Port 回報的一次認證核對結果（`claude auth status` 的整理）；不含任何 token。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthReport {
    pub logged_in: bool,
    pub source: AuthSource,
    /// 認證來源的細節，例如環境變數名稱或供應商名稱，只用於顯示。
    pub source_detail: Option<String>,
    /// 實際認證的帳號（電子郵件）；環境變數提供的 API key 或 token 沒有帳號。
    pub account: Option<String>,
    /// CLI 回報它實際使用的設定目錄；沒有回報時為 `None`。
    pub config_dir: Option<String>,
}

/// 最近一次核對的結果（保存在 Profile 上）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Verification {
    pub checked_at: Timestamp,
    pub logged_in: bool,
    pub source: AuthSource,
    pub source_detail: Option<String>,
    pub account: Option<String>,
    /// CLI 實際使用的設定目錄是否就是這個 Profile 的目錄；CLI 沒有回報時視為相符。
    pub config_dir_matches: bool,
    /// 核對當下 `claude --version` 的輸出；取不到（或票 14 之前的核對）為 `None`。
    #[serde(default)]
    pub cli_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountProfile {
    pub id: String,
    pub name: String,
    /// 這個 Profile 專屬的 `CLAUDE_CONFIG_DIR`；任兩個 Profile 不共用、也不互相包含。
    pub config_dir: String,
    pub config_dir_origin: ConfigDirOrigin,
    /// 預期的帳號身分：建立時指定，或第一次核對到原生登入時固定下來。
    pub expected_account: Option<String>,
    pub status: ProfileStatus,
    /// 尚未核對過為 `None`。
    pub verification: Option<Verification>,
    /// 每個事件使版本加一；事件的 `aggregate_version` 與它對應。
    pub version: i64,
    pub created_at: Timestamp,
    pub archived_at: Option<Timestamp>,
}

/// 阻擋 Profile 被派工的原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Blocker {
    Archived,
    Unverified,
    NotLoggedIn,
    AuthSourceOverridden { source: AuthSource, detail: Option<String> },
    ConfigDirMismatch,
    IdentityUnconfirmed,
    IdentityMismatch { expected: String, actual: String },
    DuplicateIdentity { account: String, other_name: String },
    /// Claude Code 版本低於 Adapter 驗證過的最低版本。
    CliVersionUnsupported { found: String },
    /// 無法取得或看不懂 Claude Code 的版本。
    CliVersionUnknown { raw: Option<String> },
}

impl Blocker {
    /// 穩定的機器可讀代碼。
    pub fn code(&self) -> &'static str {
        match self {
            Blocker::Archived => "profile_archived",
            Blocker::Unverified => "profile_unverified",
            Blocker::NotLoggedIn => "profile_not_logged_in",
            Blocker::AuthSourceOverridden { .. } => "auth_source_overridden",
            Blocker::ConfigDirMismatch => "config_dir_mismatch",
            Blocker::IdentityUnconfirmed => "identity_unconfirmed",
            Blocker::IdentityMismatch { .. } => "identity_mismatch",
            Blocker::DuplicateIdentity { .. } => "duplicate_identity",
            Blocker::CliVersionUnsupported { .. } => "cli_version_unsupported",
            Blocker::CliVersionUnknown { .. } => "cli_version_unknown",
        }
    }

    pub fn message(&self) -> String {
        match self {
            Blocker::Archived => "這個 Profile 已封存".to_string(),
            Blocker::Unverified => "尚未核對登入狀態；請完成原生登入後按「重新核對」".to_string(),
            Blocker::NotLoggedIn => "尚未登入；請在終端機完成原生登入後按「重新核對」".to_string(),
            Blocker::AuthSourceOverridden { source, detail } => {
                let origin = match (source, detail) {
                    (AuthSource::ApiKey, Some(d)) => format!("API key（{d}）"),
                    (AuthSource::ApiKey, None) => "API key".to_string(),
                    (AuthSource::OauthToken, Some(d)) => format!("OAuth token（{d}）"),
                    (AuthSource::OauthToken, None) => "OAuth token（環境變數）".to_string(),
                    (AuthSource::ThirdPartyProvider, Some(d)) => format!("第三方供應商（{d}）"),
                    (AuthSource::ThirdPartyProvider, None) => "第三方供應商".to_string(),
                    (_, Some(d)) => format!("未知的認證方式（{d}）"),
                    (_, None) => "未知的認證方式".to_string(),
                };
                format!(
                    "實際使用的認證來源是 {origin}，不是這個 Profile 的原生登入；\
                     環境變數或 credential helper 覆蓋了登入，已阻擋派工。請移除覆蓋來源後重新核對"
                )
            }
            Blocker::ConfigDirMismatch => {
                "Claude Code 實際使用的設定目錄不是這個 Profile 的目錄，已阻擋派工".to_string()
            }
            Blocker::IdentityUnconfirmed => "已登入，但無法取得帳號身分，無法確認是不是預期的帳號".to_string(),
            Blocker::IdentityMismatch { expected, actual } => {
                format!("實際認證的帳號是 {actual}，與這個 Profile 預期的 {expected} 不符，已阻擋派工")
            }
            Blocker::DuplicateIdentity { account, other_name } => {
                format!("帳號 {account} 已經由 Profile「{other_name}」使用；同一個帳號不能同時綁定兩個 Profile")
            }
            Blocker::CliVersionUnsupported { found } => format!(
                "Claude Code 版本 {found} 低於支援的最低版本 {}，已阻擋派工；請更新 Claude Code 後重新核對",
                claude_code::MIN_SUPPORTED_VERSION
            ),
            Blocker::CliVersionUnknown { raw: Some(raw) } => format!(
                "無法判讀 Claude Code 的版本（{raw}），無法確認支援 Adapter 需要的功能，已阻擋派工；請確認安裝後重新核對"
            ),
            Blocker::CliVersionUnknown { raw: None } => {
                "核對時沒有取得 Claude Code 的版本，無法確認支援 Adapter 需要的功能，已阻擋派工；請確認安裝後重新核對"
                    .to_string()
            }
        }
    }
}

/// 對外（API、UI）呈現的阻擋原因。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockerInfo {
    pub code: String,
    pub message: String,
}

/// Profile 加上目前的可派工判定；Scheduler 只應該派給 `dispatchable` 的 Profile。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileView {
    #[serde(flatten)]
    pub profile: AccountProfile,
    pub dispatchable: bool,
    pub blockers: Vec<BlockerInfo>,
}

/// 帳號是否相同：電子郵件不分大小寫。
fn same_account(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

/// 判定一個 Profile 目前有哪些阻擋原因；空的就是可以派工。`all` 是全部 Profile（依建立順序）。
pub fn blockers(profile: &AccountProfile, all: &[AccountProfile]) -> Vec<Blocker> {
    if profile.status == ProfileStatus::Archived {
        return vec![Blocker::Archived];
    }
    let Some(verification) = &profile.verification else {
        return vec![Blocker::Unverified];
    };
    if !verification.logged_in {
        return vec![Blocker::NotLoggedIn];
    }

    let mut found = Vec::new();
    // 版本由核對當下的 `claude --version` 決定：Adapter 只在驗證過的版本上啟動 Worker
    match claude_code::check(verification.cli_version.as_deref()) {
        VersionCheck::Supported(_) => {}
        VersionCheck::Unsupported { found: version } => {
            found.push(Blocker::CliVersionUnsupported { found: version.to_string() })
        }
        VersionCheck::Unknown { raw } => found.push(Blocker::CliVersionUnknown { raw }),
    }
    if !verification.config_dir_matches {
        found.push(Blocker::ConfigDirMismatch);
    }
    if verification.source != AuthSource::NativeLogin {
        // 認證來源不是這個 Profile 的登入時，它回報的帳號（若有）不能當成 Profile 的身分
        found.push(Blocker::AuthSourceOverridden {
            source: verification.source,
            detail: verification.source_detail.clone(),
        });
        return found;
    }
    let Some(account) = &verification.account else {
        found.push(Blocker::IdentityUnconfirmed);
        return found;
    };
    if let Some(expected) = &profile.expected_account {
        if !same_account(expected, account) {
            found.push(Blocker::IdentityMismatch { expected: expected.clone(), actual: account.clone() });
        }
    }
    // 同一個帳號只算最早建立的那個 Profile
    let earlier = all.iter().take_while(|other| other.id != profile.id).find(|other| {
        other.status == ProfileStatus::Active
            && other.verification.as_ref().is_some_and(|v| {
                v.logged_in
                    && v.source == AuthSource::NativeLogin
                    && v.account.as_deref().is_some_and(|a| same_account(a, account))
            })
    });
    if let Some(other) = earlier {
        found.push(Blocker::DuplicateIdentity { account: account.clone(), other_name: other.name.clone() });
    }
    found
}

/// 全部 Profile 的呈現（保留傳入順序）。
pub fn views(profiles: &[AccountProfile]) -> Vec<ProfileView> {
    profiles
        .iter()
        .map(|profile| {
            let blockers = blockers(profile, profiles);
            ProfileView {
                profile: profile.clone(),
                dispatchable: blockers.is_empty(),
                blockers: blockers
                    .iter()
                    .map(|b| BlockerInfo { code: b.code().to_string(), message: b.message() })
                    .collect(),
            }
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileCommand {
    /// `id` 與 `config_dir` 由呼叫端產生（Domain 不產生亂數，也不決定資料目錄位置）。
    Create { id: String, name: String, config_dir: String, expected_account: Option<String> },
    /// 把使用者既有的設定目錄（例如 `~/.claude-Work-3`）登記成 Profile；`managed_root` 是平台建立設定目錄的位置，
    /// 外部目錄不能在它之內或包含它。
    CreateFromExistingDir { id: String, name: String, config_dir: String, managed_root: String },
    /// `cli_version` 是核對當下 `claude --version` 的輸出（取不到為 `None`）。
    RecordVerification { id: String, report: AuthReport, cli_version: Option<String> },
    Archive { id: String },
}

/// 命令成功後的結果：新的 Profile 狀態與要追加的事件。
#[derive(Debug, Clone, PartialEq)]
pub struct Decision {
    pub profile: AccountProfile,
    pub events: Vec<DomainEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProfileError {
    #[error("Profile 名稱不能是空的")]
    NameEmpty,
    #[error("Profile 名稱不能超過 {MAX_NAME_CHARS} 個字元")]
    NameTooLong,
    #[error("已經有名為「{0}」的 Profile")]
    NameTaken(String),
    #[error("預期帳號必須是電子郵件（例如 name@example.com）：{0}")]
    ExpectedAccountInvalid(String),
    #[error("最多只能有 {MAX_PROFILES} 個帳號 Profile（目前已有 {MAX_PROFILES} 個）。請先封存用不到的 Profile，再新增")]
    LimitReached,
    #[error("設定目錄 {0} 與其他 Profile 的設定目錄相同或互相包含；每個 Profile 必須有獨立的設定目錄")]
    ConfigDirShared(String),
    #[error(r"設定目錄必須是完整的絕對路徑（例如 C:\Users\me\.claude-Work-3）：{0}")]
    ConfigDirInvalid(String),
    #[error("設定目錄 {0} 與平台資料目錄的 profiles 重疊；平台建立的設定目錄請用「新增 Profile」")]
    ConfigDirManaged(String),
    #[error("找不到 Profile {0}")]
    NotFound(String),
    #[error("Profile {0} 已經封存")]
    AlreadyArchived(String),
    #[error("帳號清單已經變動（有新增或封存），無法套用這個順序；請重新整理後再調整")]
    OrderMismatch,
}

impl ProfileError {
    /// 穩定的機器可讀代碼，API 與 UI 依它判斷錯誤種類。
    pub fn code(&self) -> &'static str {
        match self {
            ProfileError::NameEmpty | ProfileError::NameTooLong => "profile_name_invalid",
            ProfileError::NameTaken(_) => "profile_name_taken",
            ProfileError::ExpectedAccountInvalid(_) => "profile_account_invalid",
            ProfileError::LimitReached => "profile_limit_reached",
            ProfileError::ConfigDirShared(_) => "profile_config_dir_shared",
            ProfileError::ConfigDirInvalid(_) => "profile_config_dir_invalid",
            ProfileError::ConfigDirManaged(_) => "profile_config_dir_managed",
            ProfileError::NotFound(_) => "profile_not_found",
            ProfileError::AlreadyArchived(_) => "profile_already_archived",
            ProfileError::OrderMismatch => "profile_order_mismatch",
        }
    }
}

/// 設定目錄的比較鍵：Windows 路徑不分大小寫，斜線與結尾分隔線視為相同，並去掉 `\\?\` 前綴。
fn dir_key(path: &str) -> String {
    let unified = path.trim().replace('/', "\\").to_lowercase();
    let without_prefix = unified.strip_prefix("\\\\?\\").unwrap_or(&unified);
    without_prefix.trim_end_matches('\\').to_string()
}

/// 兩個目錄相同，或其中一個在另一個之內。
fn dirs_overlap(a: &str, b: &str) -> bool {
    let (a, b) = (dir_key(a), dir_key(b));
    a == b || a.starts_with(&format!("{b}\\")) || b.starts_with(&format!("{a}\\"))
}

fn normalize_expected_account(raw: Option<String>) -> Result<Option<String>, ProfileError> {
    let Some(raw) = raw else { return Ok(None) };
    let account = raw.trim();
    if account.is_empty() {
        return Ok(None);
    }
    let valid = !account.contains(char::is_whitespace)
        && account.split('@').count() == 2
        && account.split('@').all(|part| !part.is_empty());
    if valid {
        Ok(Some(account.to_string()))
    } else {
        Err(ProfileError::ExpectedAccountInvalid(account.to_string()))
    }
}

/// 套用一個命令。`existing` 是目前所有的 Profile（含已封存，依建立順序）。
pub fn decide(existing: &[AccountProfile], command: ProfileCommand, now: Timestamp) -> Result<Decision, ProfileError> {
    match command {
        ProfileCommand::Create { id, name, config_dir, expected_account } => {
            create(existing, id, name, config_dir, ConfigDirOrigin::Managed, expected_account, now)
        }
        ProfileCommand::CreateFromExistingDir { id, name, config_dir, managed_root } => {
            let config_dir = workspace::normalize_path(&config_dir).map_err(|_| ProfileError::ConfigDirInvalid(config_dir))?;
            if dirs_overlap(&config_dir, &managed_root) {
                return Err(ProfileError::ConfigDirManaged(config_dir));
            }
            create(existing, id, name, config_dir, ConfigDirOrigin::External, None, now)
        }
        ProfileCommand::RecordVerification { id, report, cli_version } => {
            let current = find_active(existing, &id)?;
            let verification = Verification {
                checked_at: now,
                logged_in: report.logged_in,
                source: report.source,
                source_detail: report.source_detail.clone(),
                account: report.account.clone(),
                config_dir_matches: report.config_dir.as_deref().is_none_or(|dir| dir_key(dir) == dir_key(&current.config_dir)),
                cli_version,
            };
            let mut profile = AccountProfile {
                verification: Some(verification.clone()),
                version: current.version + 1,
                ..current.clone()
            };
            let mut events = vec![event(
                &profile,
                profile.version,
                EVENT_VERIFIED,
                now,
                serde_json::json!({
                    "logged_in": verification.logged_in,
                    "auth_source": verification.source,
                    "auth_source_detail": verification.source_detail,
                    "account": verification.account,
                    "config_dir_matches": verification.config_dir_matches,
                    "cli_version": verification.cli_version,
                }),
            )];
            // 沒有預期帳號時，第一次在這個 Profile 自己的原生登入核對到的帳號就是它的身分
            let pinnable = verification.logged_in
                && verification.source == AuthSource::NativeLogin
                && verification.config_dir_matches;
            if let (None, true, Some(account)) = (&profile.expected_account, pinnable, &verification.account) {
                profile.expected_account = Some(account.clone());
                profile.version += 1;
                events.push(event(
                    &profile,
                    profile.version,
                    EVENT_IDENTITY_PINNED,
                    now,
                    serde_json::json!({ "account": account }),
                ));
            }
            Ok(Decision { profile, events })
        }
        ProfileCommand::Archive { id } => {
            let current = find_active(existing, &id)?;
            let profile = AccountProfile {
                status: ProfileStatus::Archived,
                version: current.version + 1,
                archived_at: Some(now),
                ..current.clone()
            };
            let archived = event(&profile, profile.version, EVENT_ARCHIVED, now, serde_json::json!({}));
            Ok(Decision { profile, events: vec![archived] })
        }
    }
}

/// 建立 Profile 的共同規則：名稱、預期帳號、數量上限、設定目錄不共用。
fn create(
    existing: &[AccountProfile],
    id: String,
    name: String,
    config_dir: String,
    config_dir_origin: ConfigDirOrigin,
    expected_account: Option<String>,
    now: Timestamp,
) -> Result<Decision, ProfileError> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err(ProfileError::NameEmpty);
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(ProfileError::NameTooLong);
    }
    let expected_account = normalize_expected_account(expected_account)?;
    let active = existing.iter().filter(|p| p.status == ProfileStatus::Active);
    if active.clone().any(|p| p.name.to_lowercase() == name.to_lowercase()) {
        return Err(ProfileError::NameTaken(name));
    }
    if active.count() >= MAX_PROFILES {
        return Err(ProfileError::LimitReached);
    }
    // 已封存的 Profile 目錄也不能重用：它的登入資料仍在那裡
    if existing.iter().any(|p| dirs_overlap(&p.config_dir, &config_dir)) {
        return Err(ProfileError::ConfigDirShared(config_dir));
    }
    let profile = AccountProfile {
        id,
        name,
        config_dir,
        config_dir_origin,
        expected_account,
        status: ProfileStatus::Active,
        verification: None,
        version: 1,
        created_at: now,
        archived_at: None,
    };
    let created = event(
        &profile,
        profile.version,
        EVENT_CREATED,
        now,
        serde_json::json!({
            "name": profile.name,
            "config_dir": profile.config_dir,
            "config_dir_origin": profile.config_dir_origin,
            "expected_account": profile.expected_account,
        }),
    );
    Ok(Decision { profile, events: vec![created] })
}

fn find_active<'a>(existing: &'a [AccountProfile], id: &str) -> Result<&'a AccountProfile, ProfileError> {
    let current = existing.iter().find(|p| p.id == id).ok_or_else(|| ProfileError::NotFound(id.to_string()))?;
    if current.status == ProfileStatus::Archived {
        return Err(ProfileError::AlreadyArchived(id.to_string()));
    }
    Ok(current)
}

/// 調整帳號的顯示順序（只影響帳號頁的排列，不影響派工；派工仍依建立順序）。
/// `current` 是目前所有的 Profile（含已封存，依顯示順序）；`requested` 必須恰好列出每個使用中的 Profile 一次。
/// 回傳全部 Profile 的新順序：使用中的依要求排列，已封存的維持原本的相對順序排在後面。
pub fn reorder(current: &[AccountProfile], requested: &[String]) -> Result<Vec<String>, ProfileError> {
    // 排序後比較：同時抓到缺少、多出、重複與已封存的項目
    let mut active: Vec<&str> =
        current.iter().filter(|p| p.status == ProfileStatus::Active).map(|p| p.id.as_str()).collect();
    let mut listed: Vec<&str> = requested.iter().map(String::as_str).collect();
    active.sort_unstable();
    listed.sort_unstable();
    if listed != active {
        return Err(ProfileError::OrderMismatch);
    }
    let archived = current.iter().filter(|p| p.status == ProfileStatus::Archived).map(|p| p.id.clone());
    Ok(requested.iter().cloned().chain(archived).collect())
}

fn event(
    profile: &AccountProfile,
    version: i64,
    event_type: &'static str,
    now: Timestamp,
    payload: serde_json::Value,
) -> DomainEvent {
    DomainEvent {
        aggregate_type: AGGREGATE_TYPE,
        aggregate_id: profile.id.clone(),
        aggregate_version: version,
        event_type,
        occurred_at: now,
        payload,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T1: Timestamp = Timestamp::from_millis(1_000);
    const T2: Timestamp = Timestamp::from_millis(2_000);

    fn create_cmd(id: &str, name: &str, expected: Option<&str>) -> ProfileCommand {
        ProfileCommand::Create {
            id: id.into(),
            name: name.into(),
            config_dir: format!("C:\\Baton\\profiles\\{id}"),
            expected_account: expected.map(Into::into),
        }
    }

    fn created(id: &str, name: &str) -> AccountProfile {
        decide(&[], create_cmd(id, name, None), T1).unwrap().profile
    }

    fn native(account: Option<&str>, dir: &str) -> AuthReport {
        AuthReport {
            logged_in: true,
            source: AuthSource::NativeLogin,
            source_detail: None,
            account: account.map(Into::into),
            config_dir: Some(dir.into()),
        }
    }

    fn verified(mut profile: AccountProfile, report: AuthReport) -> AccountProfile {
        let decision =
            decide(std::slice::from_ref(&profile), ProfileCommand::RecordVerification { id: profile.id.clone(), report, cli_version: supported_cli() }, T2)
                .unwrap();
        profile = decision.profile;
        profile
    }

    fn supported_cli() -> Option<String> {
        Some(format!("{} (Claude Code)", claude_code::MIN_SUPPORTED_VERSION))
    }

    fn codes(profile: &AccountProfile, all: &[AccountProfile]) -> Vec<&'static str> {
        blockers(profile, all).iter().map(Blocker::code).collect()
    }

    #[test]
    fn create_starts_unverified_and_blocked_with_one_created_event() {
        let decision = decide(&[], create_cmd("p1", "  工作帳號  ", Some("a@x.com")), T1).unwrap();

        let profile = &decision.profile;
        assert_eq!((profile.name.as_str(), profile.status, profile.version), ("工作帳號", ProfileStatus::Active, 1));
        assert_eq!(profile.expected_account.as_deref(), Some("a@x.com"));
        assert!(profile.verification.is_none());
        assert_eq!(decision.events.len(), 1);
        assert_eq!((decision.events[0].event_type, decision.events[0].aggregate_version), (EVENT_CREATED, 1));
        assert_eq!(codes(profile, std::slice::from_ref(profile)), ["profile_unverified"]);
    }

    #[test]
    fn name_and_expected_account_are_validated() {
        assert_eq!(decide(&[], create_cmd("p1", "   ", None), T1), Err(ProfileError::NameEmpty));
        assert_eq!(decide(&[], create_cmd("p1", &"x".repeat(MAX_NAME_CHARS + 1), None), T1), Err(ProfileError::NameTooLong));
        assert!(decide(&[], create_cmd("p1", &"x".repeat(MAX_NAME_CHARS), None), T1).is_ok());
        for bad in ["nobody", "a@", "@x.com", "a b@x.com", "a@b@c"] {
            let result = decide(&[], create_cmd("p1", "n", Some(bad)), T1);
            assert!(matches!(result, Err(ProfileError::ExpectedAccountInvalid(_))), "{bad}: {result:?}");
        }
        // 空白視為沒有指定
        assert_eq!(decide(&[], create_cmd("p1", "n", Some("  ")), T1).unwrap().profile.expected_account, None);
    }

    #[test]
    fn names_are_unique_among_active_profiles_ignoring_case() {
        let existing = [created("p1", "Work")];

        assert_eq!(decide(&existing, create_cmd("p2", "work", None), T2), Err(ProfileError::NameTaken("work".into())));
    }

    #[test]
    fn the_ninth_active_profile_is_rejected_with_the_limit_in_the_message() {
        let mut all: Vec<AccountProfile> = Vec::new();
        for n in 1..=MAX_PROFILES {
            let decision = decide(&all, create_cmd(&format!("p{n}"), &format!("帳號 {n}"), None), T1).unwrap();
            all.push(decision.profile);
        }

        let error = decide(&all, create_cmd("p9", "帳號 9", None), T2).unwrap_err();

        assert_eq!(error, ProfileError::LimitReached);
        assert_eq!(error.code(), "profile_limit_reached");
        assert!(error.to_string().contains('8'), "{error}");
    }

    #[test]
    fn archiving_frees_a_slot_and_a_name() {
        let mut all: Vec<AccountProfile> = Vec::new();
        for n in 1..=MAX_PROFILES {
            all.push(decide(&all, create_cmd(&format!("p{n}"), &format!("帳號 {n}"), None), T1).unwrap().profile);
        }
        let archived = decide(&all, ProfileCommand::Archive { id: "p1".into() }, T2).unwrap().profile;
        all[0] = archived;

        assert!(decide(&all, create_cmd("p9", "帳號 1", None), T2).is_ok());
    }

    #[test]
    fn config_dirs_are_never_shared_or_nested_even_with_archived_profiles() {
        let mut archived = created("p1", "a");
        archived.status = ProfileStatus::Archived;
        let existing = [archived];
        let shared = |dir: &str| {
            decide(
                &existing,
                ProfileCommand::Create { id: "p2".into(), name: "b".into(), config_dir: dir.into(), expected_account: None },
                T2,
            )
        };

        for same_or_nested in [
            "C:\\Baton\\profiles\\p1",
            "c:/baton/PROFILES/p1/",
            "\\\\?\\C:\\Baton\\profiles\\p1",
            "C:\\Baton\\profiles\\p1\\sub",
            "C:\\Baton\\profiles",
        ] {
            assert!(matches!(shared(same_or_nested), Err(ProfileError::ConfigDirShared(_))), "{same_or_nested}");
        }
        // 前綴相同但不是同一層目錄名稱，不算共用
        assert!(shared("C:\\Baton\\profiles\\p10").is_ok());
    }

    #[test]
    fn native_login_with_matching_identity_is_dispatchable_and_pins_the_account() {
        let profile = created("p1", "a");

        let after = verified(profile, native(Some("A@x.com"), "C:\\Baton\\profiles\\p1"));

        assert_eq!(after.expected_account.as_deref(), Some("A@x.com"), "第一次原生登入核對到的帳號成為預期身分");
        assert!(codes(&after, std::slice::from_ref(&after)).is_empty());
        assert_eq!(after.version, 3, "核對與固定身分各一個事件");
    }

    #[test]
    fn verification_appends_a_verified_event_and_a_pinned_event_only_the_first_time() {
        let profile = created("p1", "a");
        let first = decide(
            std::slice::from_ref(&profile),
            ProfileCommand::RecordVerification { id: "p1".into(), report: native(Some("a@x.com"), "C:\\Baton\\profiles\\p1"), cli_version: supported_cli() },
            T2,
        )
        .unwrap();
        let types: Vec<_> = first.events.iter().map(|e| (e.event_type, e.aggregate_version)).collect();
        assert_eq!(types, [(EVENT_VERIFIED, 2), (EVENT_IDENTITY_PINNED, 3)]);

        let second = decide(
            std::slice::from_ref(&first.profile),
            ProfileCommand::RecordVerification { id: "p1".into(), report: native(Some("a@x.com"), "C:\\Baton\\profiles\\p1"), cli_version: supported_cli() },
            T2,
        )
        .unwrap();
        let types: Vec<_> = second.events.iter().map(|e| (e.event_type, e.aggregate_version)).collect();
        assert_eq!(types, [(EVENT_VERIFIED, 4)]);
    }

    #[test]
    fn not_logged_in_blocks_dispatch() {
        let profile = verified(
            created("p1", "a"),
            AuthReport { logged_in: false, source: AuthSource::None, source_detail: None, account: None, config_dir: None },
        );

        assert_eq!(codes(&profile, std::slice::from_ref(&profile)), ["profile_not_logged_in"]);
        assert_eq!(profile.expected_account, None, "沒登入不會固定身分");
    }

    #[test]
    fn identity_that_differs_from_the_expected_account_blocks_dispatch_ignoring_case() {
        let profile = decide(&[], create_cmd("p1", "a", Some("a@x.com")), T1).unwrap().profile;

        let same = verified(profile.clone(), native(Some("A@X.COM"), "C:\\Baton\\profiles\\p1"));
        let other = verified(profile, native(Some("b@x.com"), "C:\\Baton\\profiles\\p1"));

        assert!(codes(&same, std::slice::from_ref(&same)).is_empty());
        assert_eq!(codes(&other, std::slice::from_ref(&other)), ["identity_mismatch"]);
        assert_eq!(other.expected_account.as_deref(), Some("a@x.com"), "不符時不會改寫預期身分");
    }

    #[test]
    fn environment_or_helper_overrides_block_dispatch_and_never_pin_an_identity() {
        for (source, detail) in [
            (AuthSource::ApiKey, Some("ANTHROPIC_API_KEY")),
            (AuthSource::ApiKey, Some("apiKeyHelper")),
            (AuthSource::OauthToken, None),
            (AuthSource::ThirdPartyProvider, Some("bedrock")),
            (AuthSource::Other, Some("sso")),
        ] {
            let report = AuthReport {
                logged_in: true,
                source,
                source_detail: detail.map(Into::into),
                // 就算覆蓋來源帶有帳號，也不能當成這個 Profile 的身分
                account: Some("b@x.com".into()),
                config_dir: Some("C:\\Baton\\profiles\\p1".into()),
            };
            let profile = verified(created("p1", "a"), report);

            assert_eq!(codes(&profile, std::slice::from_ref(&profile)), ["auth_source_overridden"], "{source:?}");
            assert_eq!(profile.expected_account, None);
        }
    }

    #[test]
    fn override_message_names_the_actual_source() {
        let blocker = Blocker::AuthSourceOverridden { source: AuthSource::ApiKey, detail: Some("ANTHROPIC_API_KEY".into()) };

        assert!(blocker.message().contains("ANTHROPIC_API_KEY"));
        assert!(blocker.message().contains("阻擋派工"));
    }

    fn verified_with_cli(cli_version: Option<&str>) -> AccountProfile {
        let profile = created("p1", "a");
        let report = native(Some("a@x.com"), "C:\\Baton\\profiles\\p1");
        let command = ProfileCommand::RecordVerification {
            id: profile.id.clone(),
            report,
            cli_version: cli_version.map(Into::into),
        };
        decide(std::slice::from_ref(&profile), command, T2).unwrap().profile
    }

    #[test]
    fn a_claude_code_older_than_the_supported_minimum_blocks_dispatch_and_says_why() {
        let profile = verified_with_cli(Some("2.0.5 (Claude Code)"));

        let found = blockers(&profile, std::slice::from_ref(&profile));

        assert_eq!(found, [Blocker::CliVersionUnsupported { found: "2.0.5".into() }]);
        let message = found[0].message();
        assert!(message.contains("2.0.5") && message.contains(&claude_code::MIN_SUPPORTED_VERSION.to_string()), "{message}");
        assert_eq!(profile.verification.unwrap().cli_version.as_deref(), Some("2.0.5 (Claude Code)"));
    }

    #[test]
    fn an_unknown_claude_code_version_blocks_dispatch_instead_of_assuming_support() {
        for cli in [None, Some("not a version")] {
            let profile = verified_with_cli(cli);

            assert_eq!(codes(&profile, std::slice::from_ref(&profile)), ["cli_version_unknown"], "{cli:?}");
        }
    }

    #[test]
    fn the_supported_claude_code_version_does_not_block() {
        let profile = verified_with_cli(Some("2.1.307 (Claude Code)"));

        assert!(blockers(&profile, std::slice::from_ref(&profile)).is_empty());
    }

    #[test]
    fn verifications_recorded_before_versions_were_checked_must_be_rechecked() {
        let old = r#"{"checked_at":1,"logged_in":true,"source":"native_login","source_detail":null,"account":"a@x.com","config_dir_matches":true}"#;

        let verification: Verification = serde_json::from_str(old).unwrap();

        assert_eq!(verification.cli_version, None);
        let mut profile = verified_with_cli(Some("2.1.307"));
        profile.verification = Some(verification);
        assert_eq!(codes(&profile, std::slice::from_ref(&profile)), ["cli_version_unknown"]);
    }

    #[test]
    fn a_config_dir_other_than_the_profiles_blocks_dispatch() {
        let profile = verified(created("p1", "a"), native(Some("a@x.com"), "C:\\Users\\me\\.claude"));

        assert_eq!(codes(&profile, std::slice::from_ref(&profile)), ["config_dir_mismatch"]);
        assert_eq!(profile.expected_account, None, "設定目錄不符時不固定身分");
        // 路徑大小寫、斜線與 \\?\ 前綴的差異不算不符
        let same = verified(created("p2", "b"), native(Some("b@x.com"), "\\\\?\\c:/baton/Profiles/p2/"));
        assert!(codes(&same, std::slice::from_ref(&same)).is_empty());
    }

    #[test]
    fn logged_in_without_an_account_cannot_be_confirmed() {
        let profile = verified(created("p1", "a"), native(None, "C:\\Baton\\profiles\\p1"));

        assert_eq!(codes(&profile, std::slice::from_ref(&profile)), ["identity_unconfirmed"]);
    }

    #[test]
    fn the_same_account_cannot_serve_two_profiles_only_the_earlier_one_keeps_it() {
        let first = verified(created("p1", "a"), native(Some("a@x.com"), "C:\\Baton\\profiles\\p1"));
        let second = verified(created("p2", "b"), native(Some("A@x.com"), "C:\\Baton\\profiles\\p2"));
        let all = [first.clone(), second.clone()];

        assert!(codes(&first, &all).is_empty());
        assert_eq!(codes(&second, &all), ["duplicate_identity"]);
        let views = views(&all);
        assert_eq!(views.iter().map(|v| v.dispatchable).collect::<Vec<_>>(), [true, false]);
        assert_eq!(views[1].blockers[0].code, "duplicate_identity");
        assert!(views[1].blockers[0].message.contains("a"), "訊息指出占用的 Profile");
    }

    #[test]
    fn verification_of_unknown_or_archived_profile_fails() {
        let mut archived = created("p1", "a");
        archived.status = ProfileStatus::Archived;
        let record = |id: &str| ProfileCommand::RecordVerification { id: id.into(), report: native(None, "x"), cli_version: supported_cli() };

        assert_eq!(decide(&[], record("nope"), T2), Err(ProfileError::NotFound("nope".into())));
        assert_eq!(decide(&[archived], record("p1"), T2), Err(ProfileError::AlreadyArchived("p1".into())));
    }

    #[test]
    fn archive_marks_archived_bumps_version_and_blocks_dispatch() {
        let profile = created("p1", "a");

        let decision = decide(std::slice::from_ref(&profile), ProfileCommand::Archive { id: "p1".into() }, T2).unwrap();

        assert_eq!((decision.profile.status, decision.profile.version, decision.profile.archived_at), (ProfileStatus::Archived, 2, Some(T2)));
        assert_eq!(decision.events[0].event_type, EVENT_ARCHIVED);
        assert_eq!(codes(&decision.profile, std::slice::from_ref(&decision.profile)), ["profile_archived"]);
        assert_eq!(
            decide(&[decision.profile], ProfileCommand::Archive { id: "p1".into() }, T2),
            Err(ProfileError::AlreadyArchived("p1".into()))
        );
    }

    const MANAGED_ROOT: &str = "C:\\Baton\\profiles";

    fn from_dir(id: &str, name: &str, dir: &str) -> ProfileCommand {
        ProfileCommand::CreateFromExistingDir {
            id: id.into(),
            name: name.into(),
            config_dir: dir.into(),
            managed_root: MANAGED_ROOT.into(),
        }
    }

    #[test]
    fn platform_created_profiles_are_marked_managed() {
        assert_eq!(created("p1", "a").config_dir_origin, ConfigDirOrigin::Managed);
    }

    #[test]
    fn an_existing_dir_becomes_an_external_unverified_profile_with_a_normalized_path() {
        let decision = decide(&[], from_dir("p1", " Work 3 ", "  \"c:/Users/me/.claude-Work-3/\" "), T1).unwrap();

        let profile = &decision.profile;
        assert_eq!(profile.name, "Work 3");
        assert_eq!(profile.config_dir, "c:\\Users\\me\\.claude-Work-3");
        assert_eq!(profile.config_dir_origin, ConfigDirOrigin::External);
        assert_eq!((profile.status, profile.version, profile.expected_account.as_deref()), (ProfileStatus::Active, 1, None));
        assert_eq!(codes(profile, std::slice::from_ref(profile)), ["profile_unverified"]);
        assert_eq!(decision.events.len(), 1);
        assert_eq!(decision.events[0].event_type, EVENT_CREATED);
        assert_eq!(decision.events[0].payload["config_dir_origin"], "external");
    }

    #[test]
    fn an_existing_dir_must_be_an_absolute_path() {
        for bad in ["", "   ", ".claude-Work-3", "Users\\me\\.claude", "C:\\Users\\..\\me"] {
            let error = decide(&[], from_dir("p1", "a", bad), T1).unwrap_err();
            assert!(matches!(error, ProfileError::ConfigDirInvalid(_)), "{bad:?}: {error:?}");
            assert_eq!(error.code(), "profile_config_dir_invalid");
        }
    }

    #[test]
    fn an_existing_dir_under_or_containing_the_platform_profiles_dir_is_rejected() {
        for inside in ["C:\\Baton\\profiles\\mine", "c:/baton/PROFILES", "C:\\Baton", "\\\\?\\C:\\Baton\\profiles\\x"] {
            let error = decide(&[], from_dir("p1", "a", inside), T1).unwrap_err();
            assert!(matches!(error, ProfileError::ConfigDirManaged(_)), "{inside}: {error:?}");
            assert_eq!(error.code(), "profile_config_dir_managed");
            assert!(error.to_string().contains("profiles"), "{error}");
        }
        // 只是名稱前綴相同的同層目錄不受影響
        assert!(decide(&[], from_dir("p1", "a", "C:\\Baton\\profiles-old"), T1).is_ok());
    }

    #[test]
    fn an_existing_dir_shared_with_or_nested_in_another_profile_is_rejected_with_the_reason() {
        let mut archived = decide(&[], from_dir("p1", "a", "C:\\Users\\me\\.claude-Work-3"), T1).unwrap().profile;
        archived.status = ProfileStatus::Archived;
        let existing = [created("p0", "managed"), archived];

        for overlapping in [
            "C:\\Users\\me\\.claude-Work-3",
            "c:/users/ME/.claude-Work-3/",
            "C:\\Users\\me\\.claude-Work-3\\nested",
            "C:\\Users\\me",
            "C:\\Baton\\profiles\\p0\\inner",
        ] {
            let error = decide(&existing, from_dir("p2", "b", overlapping), T2).unwrap_err();
            // 位於平台 profiles 底下的目錄由另一個原因擋下；其餘都是與其他 Profile 重疊
            if overlapping.starts_with("C:\\Baton") {
                assert!(matches!(error, ProfileError::ConfigDirManaged(_)), "{overlapping}: {error:?}");
            } else {
                assert!(matches!(error, ProfileError::ConfigDirShared(_)), "{overlapping}: {error:?}");
                assert!(error.to_string().contains("相同或互相包含"), "{error}");
            }
        }
        assert!(decide(&existing, from_dir("p2", "b", "C:\\Users\\me\\.claude-Work-4"), T2).is_ok());
    }

    #[test]
    fn an_existing_dir_profile_obeys_the_name_and_limit_rules() {
        let mut all: Vec<AccountProfile> = Vec::new();
        for n in 1..=MAX_PROFILES {
            all.push(decide(&all, create_cmd(&format!("p{n}"), &format!("帳號 {n}"), None), T1).unwrap().profile);
        }

        assert_eq!(decide(&all, from_dir("x", "帳號 1", "D:\\a"), T2), Err(ProfileError::NameTaken("帳號 1".into())));
        assert_eq!(decide(&all, from_dir("x", "新的", "D:\\a"), T2), Err(ProfileError::LimitReached));
        assert_eq!(decide(&[], from_dir("x", " ", "D:\\a"), T2), Err(ProfileError::NameEmpty));
    }

    #[test]
    fn archiving_an_external_profile_keeps_its_origin() {
        let profile = decide(&[], from_dir("p1", "a", "D:\\claude"), T1).unwrap().profile;

        let archived = decide(std::slice::from_ref(&profile), ProfileCommand::Archive { id: "p1".into() }, T2).unwrap().profile;

        assert_eq!((archived.status, archived.config_dir_origin), (ProfileStatus::Archived, ConfigDirOrigin::External));
    }

    #[test]
    fn error_codes_are_stable() {
        assert_eq!(ProfileError::LimitReached.code(), "profile_limit_reached");
        assert_eq!(ProfileError::NameTaken("x".into()).code(), "profile_name_taken");
        assert_eq!(ProfileError::NotFound("x".into()).code(), "profile_not_found");
        assert_eq!(ConfigDirOrigin::parse(ConfigDirOrigin::External.as_str()), Some(ConfigDirOrigin::External));
        assert_eq!(ConfigDirOrigin::parse(ConfigDirOrigin::Managed.as_str()), Some(ConfigDirOrigin::Managed));
        assert_eq!(ProfileError::OrderMismatch.code(), "profile_order_mismatch");
    }

    fn archive(profile: AccountProfile) -> AccountProfile {
        let id = profile.id.clone();
        decide(&[profile], ProfileCommand::Archive { id }, T2).unwrap().profile
    }

    fn ids(order: &[String]) -> Vec<&str> {
        order.iter().map(String::as_str).collect()
    }

    #[test]
    fn reorder_puts_active_profiles_in_the_requested_order_and_archived_ones_after_them() {
        let current = [created("p1", "一"), archive(created("p2", "二")), created("p3", "三"), created("p4", "四")];
        let requested = ["p4".to_string(), "p1".to_string(), "p3".to_string()];

        assert_eq!(ids(&reorder(&current, &requested).unwrap()), ["p4", "p1", "p3", "p2"]);
    }

    #[test]
    fn reorder_must_list_every_active_profile_exactly_once() {
        let current = [created("p1", "一"), archive(created("p2", "二")), created("p3", "三")];
        let order = |raw: &[&str]| raw.iter().map(|s| s.to_string()).collect::<Vec<_>>();

        // 少一個、多一個不存在的、重複、包含已封存的：都代表畫面上的清單已經不是目前的狀態
        for bad in [&["p1"][..], &["p1", "p3", "p9"], &["p1", "p1", "p3"], &["p1", "p2", "p3"]] {
            assert_eq!(reorder(&current, &order(bad)), Err(ProfileError::OrderMismatch), "{bad:?}");
        }
    }
}
