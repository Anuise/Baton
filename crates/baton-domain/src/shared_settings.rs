//! 共同設定（S3，spec D-09）：從使用者既有的 Claude Code 設定匯入共同基準、顯示來源與差異、
//! 採用後建立不可變的版本，並把某個版本渲染成 Run 專屬的設定快照檔。
//!
//! 純邏輯、無 I/O：讀檔由 `UserSettingsSource` Port 負責，這裡只處理「來源檔位元組 → 候選內容」。
//!
//! - 只讀白名單內的來源：`settings.json`（偏好、hooks、plugins）、`CLAUDE.md`（指示）、`.claude.json` 中的 `mcpServers`。
//!   `.credentials.json` 與 `.claude.json` 的其餘欄位（登入帳號、使用者識別）永遠不會被讀進來。
//! - 任何登入相關或看起來像秘密的內容不進入候選內容；排除紀錄只寫「哪個項目、為什麼」，不含值。
//! - 內容雜湊以排序後的標準 JSON 計算，與來源檔的排版、鍵順序無關。

mod secrets;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::event::DomainEvent;
use crate::Timestamp;

pub const AGGREGATE_TYPE: &str = "shared_settings";
/// 共同設定是全域唯一的聚合（版本號就是聚合版本）。
pub const AGGREGATE_ID: &str = "global";
pub const EVENT_ADOPTED: &str = "shared_settings_adopted";

/// Run 設定快照目錄中的檔名。
pub const SETTINGS_FILE: &str = "settings.json";
pub const MCP_FILE: &str = "mcp.json";
pub const INSTRUCTIONS_FILE: &str = "CLAUDE.md";
pub const MANIFEST_FILE: &str = "manifest.json";

/// 在 `settings.json` 中歸入偏好以外類別的欄位。
const HOOKS_KEY: &str = "hooks";
const PLUGINS_KEY: &str = "enabledPlugins";
const MARKETPLACES_KEY: &str = "extraKnownMarketplaces";
const MCP_KEY: &str = "mcpServers";
const ENV_KEY: &str = "env";

/// 與登入、認證來源有關的 `settings.json` 欄位：不是共同基準的一部分，一律排除。
const LOGIN_RELATED_KEYS: [&str; 9] = [
    "apiKeyHelper",
    "awsAuthRefresh",
    "awsCredentialExport",
    "forceLoginMethod",
    "forceLoginOrgUUID",
    "otelHeadersHelper",
    "oauthAccount",
    "primaryApiKey",
    "customApiKeyResponses",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    /// `settings.json`：偏好、hooks、plugins。
    Settings,
    /// `CLAUDE.md`：全域指示。
    Instructions,
    /// `.claude.json`：只取 `mcpServers`。
    UserState,
}

/// Port 讀到的一個來源檔；`bytes` 為 `None` 表示檔案不存在。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceFile {
    pub path: String,
    pub bytes: Option<Vec<u8>>,
}

/// Port 提供的全部來源（白名單：只有這三個檔案）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceFiles {
    pub settings: SourceFile,
    pub instructions: SourceFile,
    pub user_state: SourceFile,
}

/// 共同設定的內容，依類別分開存放；欄位內容是原樣的 JSON（已通過秘密遮蔽）。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SettingsBundle {
    /// `settings.json` 中除 hooks、plugins 以外的偏好（含 `permissions`、`env` 中非登入的項目）。
    pub preferences: BTreeMap<String, Value>,
    /// 全域指示（`CLAUDE.md` 全文）。
    pub instructions: Option<String>,
    /// 依事件名稱分項的 hooks。
    pub hooks: BTreeMap<String, Value>,
    /// 依伺服器名稱分項的 MCP 伺服器設定。
    pub mcp_servers: BTreeMap<String, Value>,
    /// 啟用的 plugins（`enabledPlugins`）。
    pub plugins: BTreeMap<String, Value>,
    /// plugin 市集來源（`extraKnownMarketplaces`）。
    pub marketplaces: BTreeMap<String, Value>,
}

impl SettingsBundle {
    /// 內容雜湊：排序後標準 JSON 的 SHA-256（十六進位小寫）。
    pub fn content_hash(&self) -> String {
        let value = serde_json::to_value(self).expect("SettingsBundle 可序列化");
        sha256_hex(canonical_json(&value).as_bytes())
    }

    /// 各類別的項目數（顯示用）。
    pub fn counts(&self) -> BundleCounts {
        BundleCounts {
            preferences: self.preferences.len(),
            instructions: usize::from(self.instructions.is_some()),
            hooks: self.hooks.len(),
            mcp_servers: self.mcp_servers.len(),
            plugins: self.plugins.len() + self.marketplaces.len(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleCounts {
    pub preferences: usize,
    pub instructions: usize,
    pub hooks: usize,
    pub mcp_servers: usize,
    pub plugins: usize,
}

/// 一個來源檔的讀取結果（不含內容，只有指紋）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceReport {
    pub kind: SourceKind,
    pub path: String,
    pub found: bool,
    /// 來源檔位元組的 SHA-256；檔案不存在為 `None`。
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExclusionReason {
    /// 與登入、認證來源有關（例如 `apiKeyHelper`、`ANTHROPIC_API_KEY`）。
    LoginRelated,
    /// 欄位名稱像秘密（例如 `GITHUB_TOKEN`）：只丟掉這個欄位。
    SecretKey,
    /// 內容像秘密（例如 `sk-ant-…`）：整個項目（hook 事件、MCP 伺服器、指示檔等）都不匯入。
    SecretValue,
}

/// 沒有被匯入的項目與原因；**絕不含值**。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Exclusion {
    pub source: SourceKind,
    /// 項目路徑，例如 `env.ANTHROPIC_API_KEY`、`mcpServers.gh.env.GITHUB_TOKEN`、`CLAUDE.md`。
    pub item: String,
    pub reason: ExclusionReason,
}

/// 匯入的結果：候選內容、來源指紋與排除清單。尚未採用，不會寫入任何地方。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportCandidate {
    pub bundle: SettingsBundle,
    pub content_hash: String,
    pub sources: Vec<SourceReport>,
    pub exclusions: Vec<Exclusion>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ImportError {
    #[error("{} 不是有效的 JSON：{message}", source_label(*file))]
    InvalidJson { file: SourceKind, message: String },
    #[error("{} 的最外層不是 JSON 物件", source_label(*file))]
    NotAnObject { file: SourceKind },
    #[error("{} 的 {key} 欄位格式不是預期的物件", source_label(*file))]
    UnexpectedShape { file: SourceKind, key: String },
    #[error("{} 不是有效的 UTF-8 文字", source_label(*file))]
    NotUtf8 { file: SourceKind },
}

impl ImportError {
    pub fn code(&self) -> &'static str {
        "settings_source_invalid"
    }
}

fn source_label(kind: SourceKind) -> &'static str {
    match kind {
        SourceKind::Settings => "settings.json",
        SourceKind::Instructions => "CLAUDE.md",
        SourceKind::UserState => ".claude.json",
    }
}

/// 從來源檔建立候選內容。來源檔格式錯誤時回報錯誤，不猜測、不部分匯入。
pub fn import(files: &SourceFiles) -> Result<ImportCandidate, ImportError> {
    let mut bundle = SettingsBundle::default();
    let mut exclusions = Vec::new();

    if let Some(settings) = parse_object(SourceKind::Settings, &files.settings)? {
        import_settings(settings, &mut bundle, &mut exclusions)?;
    }
    if let Some(state) = parse_object(SourceKind::UserState, &files.user_state)? {
        // `.claude.json` 同時放著登入帳號等資料：只取 mcpServers，其餘整份不讀
        if let Some(servers) = state.get(MCP_KEY) {
            import_items(SourceKind::UserState, MCP_KEY, servers, &mut bundle.mcp_servers, &mut exclusions)?;
        }
    }
    if let Some(bytes) = &files.instructions.bytes {
        let text = String::from_utf8(bytes.clone()).map_err(|_| ImportError::NotUtf8 { file: SourceKind::Instructions })?;
        if secrets::contains_secret_value(&text) {
            exclusions.push(Exclusion {
                source: SourceKind::Instructions,
                item: INSTRUCTIONS_FILE.to_string(),
                reason: ExclusionReason::SecretValue,
            });
        } else if !text.trim().is_empty() {
            bundle.instructions = Some(text);
        }
    }

    let sources = [
        (SourceKind::Settings, &files.settings),
        (SourceKind::Instructions, &files.instructions),
        (SourceKind::UserState, &files.user_state),
    ]
    .into_iter()
    .map(|(kind, file)| SourceReport {
        kind,
        path: file.path.clone(),
        found: file.bytes.is_some(),
        sha256: file.bytes.as_deref().map(sha256_hex),
    })
    .collect();
    Ok(ImportCandidate { content_hash: bundle.content_hash(), bundle, sources, exclusions })
}

/// 解析來源 JSON 物件；檔案不存在或只有空白回傳 `None`。
fn parse_object(source: SourceKind, file: &SourceFile) -> Result<Option<Map<String, Value>>, ImportError> {
    let Some(bytes) = &file.bytes else { return Ok(None) };
    let text = std::str::from_utf8(bytes).map_err(|_| ImportError::NotUtf8 { file: source })?;
    let text = text.trim_start_matches('\u{feff}');
    if text.trim().is_empty() {
        return Ok(None);
    }
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(object)) => Ok(Some(object)),
        Ok(_) => Err(ImportError::NotAnObject { file: source }),
        Err(error) => Err(ImportError::InvalidJson { file: source, message: error.to_string() }),
    }
}

fn import_settings(
    settings: Map<String, Value>,
    bundle: &mut SettingsBundle,
    exclusions: &mut Vec<Exclusion>,
) -> Result<(), ImportError> {
    let source = SourceKind::Settings;
    for (key, value) in settings {
        match key.as_str() {
            HOOKS_KEY => import_items(source, HOOKS_KEY, &value, &mut bundle.hooks, exclusions)?,
            PLUGINS_KEY => import_items(source, PLUGINS_KEY, &value, &mut bundle.plugins, exclusions)?,
            MARKETPLACES_KEY => import_items(source, MARKETPLACES_KEY, &value, &mut bundle.marketplaces, exclusions)?,
            _ if LOGIN_RELATED_KEYS.contains(&key.as_str()) => {
                exclusions.push(Exclusion { source, item: key, reason: ExclusionReason::LoginRelated });
            }
            _ => {
                let value = if key == ENV_KEY { without_login_env(value, exclusions) } else { value };
                import_item(source, key.clone(), &key, &value, &mut bundle.preferences, exclusions);
            }
        }
    }
    Ok(())
}

/// 移除 `env` 中會改變認證來源的變數，並記錄。
fn without_login_env(env: Value, exclusions: &mut Vec<Exclusion>) -> Value {
    let Value::Object(entries) = env else { return env };
    let mut kept = Map::new();
    for (name, value) in entries {
        if secrets::is_login_env_key(&name) {
            exclusions.push(Exclusion {
                source: SourceKind::Settings,
                item: format!("{ENV_KEY}.{name}"),
                reason: ExclusionReason::LoginRelated,
            });
        } else {
            kept.insert(name, value);
        }
    }
    Value::Object(kept)
}

/// 匯入「名稱 → 內容」的一組項目（hooks 事件、MCP 伺服器、plugins…），每一項各自遮蔽。
fn import_items(
    source: SourceKind,
    root: &str,
    value: &Value,
    target: &mut BTreeMap<String, Value>,
    exclusions: &mut Vec<Exclusion>,
) -> Result<(), ImportError> {
    let Value::Object(items) = value else {
        return Err(ImportError::UnexpectedShape { file: source, key: root.to_string() });
    };
    for (name, item) in items {
        import_item(source, name.clone(), &format!("{root}.{name}"), item, target, exclusions);
    }
    Ok(())
}

/// 匯入單一項目：欄位名稱像秘密者只丟掉該欄位；內容像秘密則整個項目不匯入。
fn import_item(
    source: SourceKind,
    name: String,
    path: &str,
    value: &Value,
    target: &mut BTreeMap<String, Value>,
    exclusions: &mut Vec<Exclusion>,
) {
    let mut dropped = Vec::new();
    let top_level_secret = matches!(value, Value::String(text) if is_secret_entry(&name, text));
    if secrets::contains_secret_value(&name) || top_level_secret {
        let reason = if top_level_secret { ExclusionReason::SecretKey } else { ExclusionReason::SecretValue };
        exclusions.push(Exclusion { source, item: path.to_string(), reason });
        return;
    }
    match clean(value, path, &mut dropped) {
        Ok(cleaned) => {
            exclusions.extend(dropped.into_iter().map(|(item, reason)| Exclusion { source, item, reason }));
            target.insert(name, cleaned);
        }
        Err(Tainted) => {
            exclusions.push(Exclusion { source, item: path.to_string(), reason: ExclusionReason::SecretValue });
        }
    }
}

/// 項目內容含有像秘密的字串，整個項目都要排除。
struct Tainted;

/// 欄位名稱像秘密、值是非空且不是環境變數參照的字串。
fn is_secret_entry(key: &str, value: &str) -> bool {
    !value.is_empty() && secrets::is_secret_key(key) && !secrets::is_reference(value)
}

/// 遞迴遮蔽：丟掉「名稱像秘密」的字串欄位（記入 `dropped`）；遇到「內容像秘密」的字串就回報 `Tainted`。
fn clean(value: &Value, path: &str, dropped: &mut Vec<(String, ExclusionReason)>) -> Result<Value, Tainted> {
    match value {
        Value::String(text) if secrets::contains_secret_value(text) => Err(Tainted),
        Value::Array(items) => {
            items.iter().enumerate().map(|(i, item)| clean(item, &format!("{path}[{i}]"), dropped)).collect::<Result<_, _>>().map(Value::Array)
        }
        Value::Object(entries) => {
            let mut kept = Map::new();
            for (key, item) in entries {
                let child = format!("{path}.{key}");
                if secrets::contains_secret_value(key) {
                    return Err(Tainted);
                }
                if matches!(item, Value::String(text) if is_secret_entry(key, text)) {
                    dropped.push((child, ExclusionReason::SecretKey));
                    continue;
                }
                kept.insert(key.clone(), clean(item, &child, dropped)?);
            }
            Ok(Value::Object(kept))
        }
        other => Ok(other.clone()),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Preferences,
    Instructions,
    Hooks,
    Mcp,
    Plugins,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Change {
    Added,
    Removed,
    Changed,
}

/// 候選內容與目前版本的一項差異；`before`／`after` 是已遮蔽的內容，可以直接顯示。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffEntry {
    pub category: Category,
    pub key: String,
    pub change: Change,
    pub before: Option<Value>,
    pub after: Option<Value>,
}

/// 比較候選內容與目前版本（沒有目前版本時全部視為新增）；沒變的項目不列出。
pub fn diff(current: Option<&SettingsBundle>, candidate: &SettingsBundle) -> Vec<DiffEntry> {
    let empty = SettingsBundle::default();
    let current = current.unwrap_or(&empty);
    let mut entries = Vec::new();
    let mut compare = |category: Category, before: &BTreeMap<String, Value>, after: &BTreeMap<String, Value>| {
        let keys: std::collections::BTreeSet<&String> = before.keys().chain(after.keys()).collect();
        for key in keys {
            let change = match (before.get(key), after.get(key)) {
                (None, Some(_)) => Change::Added,
                (Some(_), None) => Change::Removed,
                (Some(a), Some(b)) if a != b => Change::Changed,
                _ => continue,
            };
            entries.push(DiffEntry {
                category,
                key: key.clone(),
                change,
                before: before.get(key).cloned(),
                after: after.get(key).cloned(),
            });
        }
    };
    compare(Category::Preferences, &current.preferences, &candidate.preferences);
    compare(Category::Hooks, &current.hooks, &candidate.hooks);
    compare(Category::Mcp, &current.mcp_servers, &candidate.mcp_servers);
    compare(Category::Plugins, &current.plugins, &candidate.plugins);
    let marketplaces = |bundle_marketplaces: &BTreeMap<String, Value>| -> BTreeMap<String, Value> {
        bundle_marketplaces.iter().map(|(name, value)| (format!("marketplace/{name}"), value.clone())).collect()
    };
    compare(Category::Plugins, &marketplaces(&current.marketplaces), &marketplaces(&candidate.marketplaces));
    let instruction = |text: &Option<String>| -> BTreeMap<String, Value> {
        text.iter().map(|t| (INSTRUCTIONS_FILE.to_string(), Value::String(t.clone()))).collect()
    };
    compare(Category::Instructions, &instruction(&current.instructions), &instruction(&candidate.instructions));
    entries
}

/// 一個已採用的共同設定版本；建立後不可變。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingsVersion {
    /// 從 1 起連續遞增。
    pub version: i64,
    pub content_hash: String,
    pub bundle: SettingsBundle,
    pub sources: Vec<SourceReport>,
    pub exclusions: Vec<Exclusion>,
    pub adopted_at: Timestamp,
}

/// 版本清單用的摘要（不含內容）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingsVersionSummary {
    pub version: i64,
    pub content_hash: String,
    pub adopted_at: Timestamp,
    pub counts: BundleCounts,
    pub exclusion_count: usize,
}

impl SettingsVersion {
    pub fn summary(&self) -> SettingsVersionSummary {
        SettingsVersionSummary {
            version: self.version,
            content_hash: self.content_hash.clone(),
            adopted_at: self.adopted_at,
            counts: self.bundle.counts(),
            exclusion_count: self.exclusions.len(),
        }
    }
}

/// 採用的結果：新版本與要追加的事件。
#[derive(Debug, Clone, PartialEq)]
pub struct Adoption {
    pub version: SettingsVersion,
    pub event: DomainEvent,
}

/// 採用候選內容：每次採用都建立新版本（編號接在目前最新版本之後），既有版本不受影響。
pub fn adopt(latest_version: Option<i64>, candidate: ImportCandidate, now: Timestamp) -> Adoption {
    let version = latest_version.unwrap_or(0) + 1;
    let event = DomainEvent {
        aggregate_type: AGGREGATE_TYPE,
        aggregate_id: AGGREGATE_ID.to_string(),
        aggregate_version: version,
        event_type: EVENT_ADOPTED,
        occurred_at: now,
        payload: serde_json::json!({
            "version": version,
            "content_hash": candidate.content_hash,
            "counts": candidate.bundle.counts(),
            "excluded": candidate.exclusions.len(),
        }),
    };
    let version = SettingsVersion {
        version,
        content_hash: candidate.content_hash,
        bundle: candidate.bundle,
        sources: candidate.sources,
        exclusions: candidate.exclusions,
        adopted_at: now,
    };
    Adoption { version, event }
}

/// 某個 Run 固定使用的設定版本與快照目錄（D-09）：第一次產生後不再改變，之後採用新版本也不影響已准入的 Run。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunSettingsPin {
    pub run_id: String,
    pub version: i64,
    pub content_hash: String,
    /// 唯讀快照目錄的路徑。
    pub directory: String,
    pub created_at: Timestamp,
}

/// 快照目錄中的一個檔案。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotFile {
    pub name: String,
    pub bytes: Vec<u8>,
}

/// 把版本渲染成 Run 專屬的設定快照檔（只含有內容的類別，外加 `manifest.json`）。
///
/// 輸出完全由版本內容決定（鍵排序、固定縮排），同一版本永遠渲染出相同的位元組。
/// `manifest.json` 記錄版本、內容雜湊與每個檔案的 SHA-256，供核對。
pub fn snapshot_files(version: &SettingsVersion) -> Vec<SnapshotFile> {
    let bundle = &version.bundle;
    let mut files = Vec::new();

    let mut settings = Map::new();
    settings.extend(bundle.preferences.iter().map(|(k, v)| (k.clone(), v.clone())));
    for (key, items) in [(HOOKS_KEY, &bundle.hooks), (PLUGINS_KEY, &bundle.plugins), (MARKETPLACES_KEY, &bundle.marketplaces)] {
        if !items.is_empty() {
            settings.insert(key.to_string(), Value::Object(items.iter().map(|(k, v)| (k.clone(), v.clone())).collect()));
        }
    }
    if !settings.is_empty() {
        files.push(json_file(SETTINGS_FILE, &Value::Object(settings)));
    }
    if !bundle.mcp_servers.is_empty() {
        let servers = Value::Object(bundle.mcp_servers.iter().map(|(k, v)| (k.clone(), v.clone())).collect());
        files.push(json_file(MCP_FILE, &serde_json::json!({ MCP_KEY: servers })));
    }
    if let Some(text) = &bundle.instructions {
        files.push(SnapshotFile { name: INSTRUCTIONS_FILE.to_string(), bytes: text.clone().into_bytes() });
    }

    let hashes: Map<String, Value> =
        files.iter().map(|f| (f.name.clone(), Value::String(sha256_hex(&f.bytes)))).collect();
    let manifest = serde_json::json!({
        "version": version.version,
        "content_hash": version.content_hash,
        "files": hashes,
    });
    files.push(json_file(MANIFEST_FILE, &manifest));
    files
}

fn json_file(name: &str, value: &Value) -> SnapshotFile {
    let mut text = serde_json::to_string_pretty(&sorted(value)).expect("JSON 可序列化");
    text.push('\n');
    SnapshotFile { name: name.to_string(), bytes: text.into_bytes() }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SnapshotError {
    #[error("快照檔 {file} 不是有效的內容：{message}")]
    Invalid { file: String, message: String },
}

/// 從快照檔還原內容（核對快照目錄與版本雜湊是否一致用）；缺少的檔案視為該類別為空。
pub fn bundle_from_snapshot_files(files: &[(String, Vec<u8>)]) -> Result<SettingsBundle, SnapshotError> {
    let find = |name: &str| files.iter().find(|(n, _)| n == name).map(|(_, bytes)| bytes.as_slice());
    let invalid = |file: &str, message: String| SnapshotError::Invalid { file: file.to_string(), message };
    let parse = |name: &str| -> Result<Option<Map<String, Value>>, SnapshotError> {
        let Some(bytes) = find(name) else { return Ok(None) };
        match serde_json::from_slice::<Value>(bytes) {
            Ok(Value::Object(object)) => Ok(Some(object)),
            Ok(_) => Err(invalid(name, "最外層不是 JSON 物件".into())),
            Err(error) => Err(invalid(name, error.to_string())),
        }
    };
    let as_map = |file: &str, value: Value| -> Result<BTreeMap<String, Value>, SnapshotError> {
        match value {
            Value::Object(items) => Ok(items.into_iter().collect()),
            _ => Err(invalid(file, "預期是物件".into())),
        }
    };

    let mut bundle = SettingsBundle::default();
    if let Some(settings) = parse(SETTINGS_FILE)? {
        for (key, value) in settings {
            match key.as_str() {
                HOOKS_KEY => bundle.hooks = as_map(SETTINGS_FILE, value)?,
                PLUGINS_KEY => bundle.plugins = as_map(SETTINGS_FILE, value)?,
                MARKETPLACES_KEY => bundle.marketplaces = as_map(SETTINGS_FILE, value)?,
                _ => {
                    bundle.preferences.insert(key, value);
                }
            }
        }
    }
    if let Some(mut mcp) = parse(MCP_FILE)? {
        if let Some(servers) = mcp.remove(MCP_KEY) {
            bundle.mcp_servers = as_map(MCP_FILE, servers)?;
        }
    }
    if let Some(bytes) = find(INSTRUCTIONS_FILE) {
        let text = String::from_utf8(bytes.to_vec()).map_err(|_| invalid(INSTRUCTIONS_FILE, "不是 UTF-8".into()))?;
        bundle.instructions = Some(text);
    }
    Ok(bundle)
}

/// SHA-256 的十六進位小寫字串。
pub fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// 標準 JSON：物件鍵遞迴排序、無多餘空白。
fn canonical_json(value: &Value) -> String {
    serde_json::to_string(&sorted(value)).expect("JSON 可序列化")
}

fn sorted(value: &Value) -> Value {
    match value {
        Value::Object(entries) => {
            let ordered: BTreeMap<&String, Value> = entries.iter().map(|(k, v)| (k, sorted(v))).collect();
            Value::Object(ordered.into_iter().map(|(k, v)| (k.clone(), v)).collect())
        }
        Value::Array(items) => Value::Array(items.iter().map(sorted).collect()),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests;
