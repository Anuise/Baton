//! Workspace 的狀態機：命令 + 目前狀態 + 時間 → 新狀態 + 事件。
//!
//! 純邏輯、無 I/O。所有 Workspace 狀態變更都只能經過 `decide`；
//! Store 在一個交易中套用它的結果。

use serde::{Deserialize, Serialize};

use crate::event::DomainEvent;
use crate::Timestamp;

pub const AGGREGATE_TYPE: &str = "workspace";
pub const EVENT_CREATED: &str = "workspace_created";
pub const EVENT_ARCHIVED: &str = "workspace_archived";
pub const EVENT_MODEL_OVERRIDES_CHANGED: &str = "workspace_model_overrides_changed";

/// 顯示名稱的長度上限（字元數）。
pub const MAX_NAME_CHARS: usize = 100;
/// 模型名稱的長度上限（字元數）。
pub const MAX_MODEL_CHARS: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceStatus {
    Active,
    Archived,
}

impl WorkspaceStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            WorkspaceStatus::Active => "active",
            WorkspaceStatus::Archived => "archived",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "active" => Some(WorkspaceStatus::Active),
            "archived" => Some(WorkspaceStatus::Archived),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workspace {
    pub id: String,
    pub name: String,
    /// 使用者指定的目錄（已正規化）；可以不是 Git repo。
    pub path: String,
    pub status: WorkspaceStatus,
    /// 每次狀態變更加一；事件的 `aggregate_version` 與它對應。
    pub version: i64,
    pub created_at: Timestamp,
    pub archived_at: Option<Timestamp>,
    /// 依角色覆寫的模型（spec D-30）；沒有覆寫的角色沿用帳號的 Claude Code 預設。
    #[serde(default)]
    pub model_overrides: ModelOverrides,
}

/// Agent 的角色（spec D-30 的 Controller、Worker、Reviewer）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRole {
    Controller,
    Worker,
    Reviewer,
}

/// 每個角色的模型覆寫；`None` 表示不指定模型（平台預設，spec D-30）。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ModelOverrides {
    #[serde(default)]
    pub controller: Option<String>,
    #[serde(default)]
    pub worker: Option<String>,
    #[serde(default)]
    pub reviewer: Option<String>,
}

impl ModelOverrides {
    /// 某個角色要使用的模型；沒有覆寫時為 `None`。
    pub fn model_for(&self, role: AgentRole) -> Option<&str> {
        match role {
            AgentRole::Controller => self.controller.as_deref(),
            AgentRole::Worker => self.worker.as_deref(),
            AgentRole::Reviewer => self.reviewer.as_deref(),
        }
    }

    /// 去掉前後空白，空字串視為不覆寫；名稱只能由 Claude Code `--model` 的別名或完整名稱會用到的字元組成。
    pub fn normalized(self) -> Result<Self, WorkspaceError> {
        let normalize = |value: Option<String>| -> Result<Option<String>, WorkspaceError> {
            let Some(trimmed) = value.as_deref().map(str::trim).filter(|v| !v.is_empty()) else { return Ok(None) };
            let allowed = |c: char| c.is_ascii_alphanumeric() || "._-:[]@/".contains(c);
            if trimmed.chars().count() > MAX_MODEL_CHARS || !trimmed.chars().all(allowed) {
                return Err(WorkspaceError::ModelNameInvalid(trimmed.to_string()));
            }
            Ok(Some(trimmed.to_string()))
        };
        Ok(Self {
            controller: normalize(self.controller)?,
            worker: normalize(self.worker)?,
            reviewer: normalize(self.reviewer)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceCommand {
    /// `id` 由呼叫端產生（Domain 不產生亂數）；`name` 省略時取目錄名稱。
    Create { id: String, name: Option<String>, path: String },
    Archive { id: String },
    /// 設定依角色的模型覆寫（整組取代）；與目前相同時沒有任何變動。
    SetModelOverrides { id: String, overrides: ModelOverrides },
}

/// 命令成功後的結果：新的 Workspace 狀態與要追加的事件。
#[derive(Debug, Clone, PartialEq)]
pub struct Decision {
    pub workspace: Workspace,
    pub events: Vec<DomainEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WorkspaceError {
    #[error("目錄路徑是空的")]
    PathEmpty,
    #[error("目錄路徑必須是絕對路徑（例如 C:\\work\\notes 或 \\\\server\\share\\dir），且不含 . 或 .. 區段：{0}")]
    PathNotAbsolute(String),
    #[error("名稱不能超過 {MAX_NAME_CHARS} 個字元")]
    NameTooLong,
    #[error("這個目錄已經是 Workspace「{name}」（{id}）")]
    AlreadyRegistered { id: String, name: String },
    #[error("找不到 Workspace {0}")]
    NotFound(String),
    #[error("Workspace {0} 已經封存")]
    AlreadyArchived(String),
    #[error("模型名稱只能包含英數字與 . _ - : [ ] @ /，且不超過 {MAX_MODEL_CHARS} 個字元：{0}")]
    ModelNameInvalid(String),
}

impl WorkspaceError {
    /// 穩定的機器可讀代碼，API 與 UI 依它判斷錯誤種類。
    pub fn code(&self) -> &'static str {
        match self {
            WorkspaceError::PathEmpty | WorkspaceError::PathNotAbsolute(_) => "workspace_path_invalid",
            WorkspaceError::NameTooLong => "workspace_name_invalid",
            WorkspaceError::AlreadyRegistered { .. } => "workspace_already_exists",
            WorkspaceError::NotFound(_) => "workspace_not_found",
            WorkspaceError::AlreadyArchived(_) => "workspace_already_archived",
            WorkspaceError::ModelNameInvalid(_) => "model_name_invalid",
        }
    }
}

/// 把使用者輸入的目錄路徑正規化：去掉前後空白與引號、統一為反斜線、去掉結尾分隔線。
/// 必須是絕對路徑；API 在檢查目錄是否存在之前先呼叫它。
pub fn normalize_path(raw: &str) -> Result<String, WorkspaceError> {
    let trimmed = raw.trim().trim_matches('"').trim();
    if trimmed.is_empty() {
        return Err(WorkspaceError::PathEmpty);
    }
    let unified = trimmed.replace('/', "\\");
    let bytes = unified.as_bytes();
    let drive = bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\';
    let unc = unified.starts_with("\\\\") && unified.len() > 2 && !unified[2..].starts_with('\\');
    if !(drive || unc) || unified.split('\\').skip(1).any(|part| part == "." || part == "..") {
        return Err(WorkspaceError::PathNotAbsolute(trimmed.to_string()));
    }
    // 磁碟根目錄保留結尾的反斜線（C:\），其餘去掉
    let stripped = unified.trim_end_matches('\\');
    Ok(if drive && stripped.len() == 2 { format!("{stripped}\\") } else { stripped.to_string() })
}

/// 套用一個命令。`existing` 是目前所有的 Workspace（含已封存）。
pub fn decide(existing: &[Workspace], command: WorkspaceCommand, now: Timestamp) -> Result<Decision, WorkspaceError> {
    match command {
        WorkspaceCommand::Create { id, name, path } => {
            let path = normalize_path(&path)?;
            let key = path_key(&path);
            if let Some(found) =
                existing.iter().find(|w| w.status == WorkspaceStatus::Active && path_key(&w.path) == key)
            {
                return Err(WorkspaceError::AlreadyRegistered { id: found.id.clone(), name: found.name.clone() });
            }
            let name = match name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
                Some(name) => name.to_string(),
                None => default_name(&path),
            };
            if name.chars().count() > MAX_NAME_CHARS {
                return Err(WorkspaceError::NameTooLong);
            }
            let workspace = Workspace {
                id,
                name,
                path,
                status: WorkspaceStatus::Active,
                version: 1,
                created_at: now,
                archived_at: None,
                model_overrides: ModelOverrides::default(),
            };
            let created = event(
                &workspace,
                EVENT_CREATED,
                now,
                serde_json::json!({ "name": workspace.name, "path": workspace.path }),
            );
            Ok(Decision { workspace, events: vec![created] })
        }
        WorkspaceCommand::Archive { id } => {
            let current = existing.iter().find(|w| w.id == id).ok_or_else(|| WorkspaceError::NotFound(id.clone()))?;
            if current.status == WorkspaceStatus::Archived {
                return Err(WorkspaceError::AlreadyArchived(id));
            }
            let workspace = Workspace {
                status: WorkspaceStatus::Archived,
                version: current.version + 1,
                archived_at: Some(now),
                ..current.clone()
            };
            let archived = event(&workspace, EVENT_ARCHIVED, now, serde_json::json!({}));
            Ok(Decision { workspace, events: vec![archived] })
        }
        WorkspaceCommand::SetModelOverrides { id, overrides } => {
            let current = existing.iter().find(|w| w.id == id).ok_or_else(|| WorkspaceError::NotFound(id.clone()))?;
            if current.status == WorkspaceStatus::Archived {
                return Err(WorkspaceError::AlreadyArchived(id));
            }
            let overrides = overrides.normalized()?;
            if overrides == current.model_overrides {
                return Ok(Decision { workspace: current.clone(), events: Vec::new() });
            }
            let workspace =
                Workspace { version: current.version + 1, model_overrides: overrides.clone(), ..current.clone() };
            let changed = event(
                &workspace,
                EVENT_MODEL_OVERRIDES_CHANGED,
                now,
                serde_json::json!({ "from": current.model_overrides, "to": overrides }),
            );
            Ok(Decision { workspace, events: vec![changed] })
        }
    }
}

fn event(workspace: &Workspace, event_type: &'static str, now: Timestamp, payload: serde_json::Value) -> DomainEvent {
    DomainEvent {
        aggregate_type: AGGREGATE_TYPE,
        aggregate_id: workspace.id.clone(),
        aggregate_version: workspace.version,
        event_type,
        occurred_at: now,
        payload,
    }
}

/// Windows 路徑不分大小寫；以小寫作為「是不是同一個目錄」的比較鍵。
fn path_key(path: &str) -> String {
    path.to_lowercase()
}

/// 預設名稱取最後一段目錄名；磁碟根目錄沒有目錄名稱，用磁碟代號。
fn default_name(path: &str) -> String {
    path.trim_end_matches('\\').rsplit('\\').next().unwrap_or(path).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const T1: Timestamp = Timestamp::from_millis(1_000);
    const T2: Timestamp = Timestamp::from_millis(2_000);

    fn create(id: &str, name: Option<&str>, path: &str) -> WorkspaceCommand {
        WorkspaceCommand::Create { id: id.into(), name: name.map(Into::into), path: path.into() }
    }

    fn created(id: &str, path: &str) -> Workspace {
        decide(&[], create(id, None, path), T1).unwrap().workspace
    }

    #[test]
    fn create_produces_active_workspace_and_one_created_event() {
        let decision = decide(&[], create("w1", Some("筆記"), "C:\\work\\notes"), T1).unwrap();

        let ws = &decision.workspace;
        assert_eq!((ws.id.as_str(), ws.name.as_str(), ws.path.as_str()), ("w1", "筆記", "C:\\work\\notes"));
        assert_eq!((ws.status, ws.version, ws.created_at, ws.archived_at), (WorkspaceStatus::Active, 1, T1, None));
        assert_eq!(decision.events.len(), 1);
        let event = &decision.events[0];
        assert_eq!((event.aggregate_type, event.event_type, event.aggregate_version), ("workspace", EVENT_CREATED, 1));
        assert_eq!((event.aggregate_id.as_str(), event.occurred_at), ("w1", T1));
        assert_eq!(event.payload["path"], "C:\\work\\notes");
    }

    #[test]
    fn name_defaults_to_the_directory_name() {
        assert_eq!(created("w1", "C:\\work\\notes").name, "notes");
        assert_eq!(created("w2", "\\\\srv\\share\\docs").name, "docs");
        assert_eq!(decide(&[], create("w3", Some("   "), "C:\\work\\notes"), T1).unwrap().workspace.name, "notes");
    }

    #[test]
    fn path_is_normalized_but_original_case_is_kept() {
        assert_eq!(created("w1", "  \"C:/Work/Notes/\"  ").path, "C:\\Work\\Notes");
        assert_eq!(created("w2", "d:\\").path, "d:\\");
    }

    #[test]
    fn relative_or_malformed_paths_are_rejected() {
        for bad in ["notes", ".\\notes", "C:notes", "C:", "\\rooted", "\\\\", "C:\\a\\..\\b", "C:\\a\\.\\b", "/usr/bin"] {
            let result = decide(&[], create("w1", None, bad), T1);
            assert!(matches!(result, Err(WorkspaceError::PathNotAbsolute(_))), "{bad}: {result:?}");
        }
        assert_eq!(decide(&[], create("w1", None, "  "), T1), Err(WorkspaceError::PathEmpty));
    }

    #[test]
    fn same_directory_cannot_be_registered_twice_ignoring_case_and_separators() {
        let existing = [created("w1", "C:\\Work\\Notes")];

        let result = decide(&existing, create("w2", None, "c:/work/notes/"), T1);

        assert_eq!(result, Err(WorkspaceError::AlreadyRegistered { id: "w1".into(), name: "Notes".into() }));
    }

    #[test]
    fn archived_workspace_does_not_block_registering_the_same_directory_again() {
        let mut archived = created("w1", "C:\\work\\notes");
        archived.status = WorkspaceStatus::Archived;

        assert!(decide(&[archived], create("w2", None, "C:\\work\\notes"), T2).is_ok());
    }

    #[test]
    fn overly_long_name_is_rejected() {
        let long = "x".repeat(MAX_NAME_CHARS + 1);
        assert_eq!(decide(&[], create("w1", Some(&long), "C:\\a"), T1), Err(WorkspaceError::NameTooLong));
        assert!(decide(&[], create("w1", Some(&"x".repeat(MAX_NAME_CHARS)), "C:\\a"), T1).is_ok());
    }

    #[test]
    fn archive_marks_archived_bumps_version_and_appends_an_event() {
        let existing = [created("w1", "C:\\work\\notes")];

        let decision = decide(&existing, WorkspaceCommand::Archive { id: "w1".into() }, T2).unwrap();

        let ws = &decision.workspace;
        assert_eq!((ws.status, ws.version, ws.archived_at, ws.created_at), (WorkspaceStatus::Archived, 2, Some(T2), T1));
        assert_eq!((ws.name.as_str(), ws.path.as_str()), ("notes", "C:\\work\\notes"));
        assert_eq!(decision.events.len(), 1);
        assert_eq!((decision.events[0].event_type, decision.events[0].aggregate_version), (EVENT_ARCHIVED, 2));
    }

    #[test]
    fn archive_of_unknown_or_already_archived_workspace_fails() {
        let mut archived = created("w1", "C:\\work\\notes");
        archived.status = WorkspaceStatus::Archived;

        assert_eq!(
            decide(&[], WorkspaceCommand::Archive { id: "nope".into() }, T2),
            Err(WorkspaceError::NotFound("nope".into()))
        );
        assert_eq!(
            decide(&[archived], WorkspaceCommand::Archive { id: "w1".into() }, T2),
            Err(WorkspaceError::AlreadyArchived("w1".into()))
        );
    }

    fn set_models(id: &str, controller: Option<&str>, worker: Option<&str>, reviewer: Option<&str>) -> WorkspaceCommand {
        let overrides = ModelOverrides {
            controller: controller.map(Into::into),
            worker: worker.map(Into::into),
            reviewer: reviewer.map(Into::into),
        };
        WorkspaceCommand::SetModelOverrides { id: id.into(), overrides }
    }

    #[test]
    fn new_workspaces_override_no_model_so_every_role_uses_the_accounts_default() {
        let ws = created("w1", "C:\\work\\notes");

        for role in [AgentRole::Controller, AgentRole::Worker, AgentRole::Reviewer] {
            assert_eq!(ws.model_overrides.model_for(role), None);
        }
    }

    #[test]
    fn model_overrides_are_set_per_role_trimmed_and_recorded_as_an_event() {
        let existing = [created("w1", "C:\\work\\notes")];

        let decision = decide(&existing, set_models("w1", None, Some("  claude-opus-5-5 "), Some("")), T2).unwrap();

        let ws = &decision.workspace;
        assert_eq!(ws.model_overrides.model_for(AgentRole::Worker), Some("claude-opus-5-5"));
        assert_eq!(ws.model_overrides.model_for(AgentRole::Reviewer), None, "空字串視為不覆寫");
        assert_eq!(ws.model_overrides.model_for(AgentRole::Controller), None);
        assert_eq!(ws.version, 2);
        assert_eq!(decision.events.len(), 1);
        assert_eq!(decision.events[0].event_type, EVENT_MODEL_OVERRIDES_CHANGED);
        assert_eq!(decision.events[0].payload["to"]["worker"], "claude-opus-5-5");
    }

    #[test]
    fn setting_the_same_model_overrides_again_changes_nothing() {
        let first = decide(&[created("w1", "C:\\a")], set_models("w1", Some("sonnet"), None, None), T2).unwrap().workspace;

        let again = decide(std::slice::from_ref(&first), set_models("w1", Some(" sonnet "), None, None), T2).unwrap();

        assert_eq!((again.workspace, again.events.len()), (first, 0));
    }

    #[test]
    fn model_names_outside_the_allowed_characters_are_rejected() {
        let existing = [created("w1", "C:\\a")];
        let too_long = "x".repeat(MAX_MODEL_CHARS + 1);
        for bad in ["opus --dangerously-skip-permissions", "a;b", "模型", too_long.as_str()] {
            let result = decide(&existing, set_models("w1", None, Some(bad), None), T2);
            assert!(matches!(result, Err(WorkspaceError::ModelNameInvalid(_))), "{bad}: {result:?}");
        }
        let accepted = set_models("w1", Some("claude-opus-5-5[1m]"), Some("us.anthropic.claude:1"), None);
        assert!(decide(&existing, accepted, T2).is_ok());
    }

    #[test]
    fn model_overrides_of_unknown_or_archived_workspaces_are_rejected() {
        let mut archived = created("w1", "C:\\a");
        archived.status = WorkspaceStatus::Archived;

        assert_eq!(decide(&[], set_models("nope", None, None, None), T2), Err(WorkspaceError::NotFound("nope".into())));
        assert_eq!(
            decide(&[archived], set_models("w1", Some("opus"), None, None), T2),
            Err(WorkspaceError::AlreadyArchived("w1".into()))
        );
    }

    #[test]
    fn workspaces_serialized_before_model_overrides_existed_have_no_overrides() {
        let old = r#"{"id":"w1","name":"a","path":"C:\\a","status":"active","version":1,"created_at":1,"archived_at":null}"#;

        let ws: Workspace = serde_json::from_str(old).unwrap();

        assert_eq!(ws.model_overrides, ModelOverrides::default());
    }

    #[test]
    fn error_codes_are_stable() {
        assert_eq!(WorkspaceError::PathEmpty.code(), "workspace_path_invalid");
        assert_eq!(WorkspaceError::NotFound("x".into()).code(), "workspace_not_found");
        assert_eq!(WorkspaceError::AlreadyArchived("x".into()).code(), "workspace_already_archived");
        assert_eq!(
            WorkspaceError::AlreadyRegistered { id: "a".into(), name: "b".into() }.code(),
            "workspace_already_exists"
        );
    }
}
