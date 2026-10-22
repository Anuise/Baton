//! Artifact（成果）的純邏輯：登記請求的驗證、版本與 manifest、未登記檔案的辨識與預覽判定（票 12，spec D-17）。
//!
//! 成果的寫入順序是「寫暫存 → fsync → 原子改名 → 提交 manifest」：manifest（`artifact_version`）只在檔案已經完整落地之後才存在，
//! 所以「已登記」永遠表示內容完整。中途中斷只會留下**未登記**的檔案（暫存檔，或已改名但還沒提交 manifest 的檔案），
//! 它們由 [`classify_stored_files`] 辨識，不會被當成成果，也不會被自動刪除。
//!
//! 同一個 Task 內同名的成果是同一個 Artifact；再次登記只會追加新版本，舊版本的檔案與 manifest 都不會被覆寫。

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::event::DomainEvent;
use crate::Timestamp;

/// Worker 登記成果的工具名稱。
pub const TOOL_REGISTER_ARTIFACT: &str = "register_artifact";

/// 資料目錄下成果儲存的子目錄（spec D-17）：`<資料目錄>/artifacts/<workspace_id>/<blob 檔名>`。
pub const ARTIFACTS_DIR: &str = "artifacts";
/// 暫存檔的檔名後綴；寫到一半中斷的檔案靠它辨識。
pub const STAGING_SUFFIX: &str = ".partial";

/// 成果名稱、描述的長度上限（字元數）。
pub const NAME_MAX_CHARS: usize = 200;
pub const DESCRIPTION_MAX_CHARS: usize = 1_000;
/// 單一成果的大小上限（位元組）。
pub const ARTIFACT_MAX_BYTES: usize = 10 * 1024 * 1024;
/// 預覽最多回傳的位元組數。
pub const PREVIEW_MAX_BYTES: usize = 64 * 1024;

pub const EVENT_ARTIFACT_REGISTERED: &str = "artifact_registered";

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ArtifactError {
    #[error("成果名稱不能是空的")]
    NameEmpty,
    #[error("成果名稱最多 {NAME_MAX_CHARS} 個字元")]
    NameTooLong,
    #[error("成果名稱必須是檔名：不可含路徑分隔字元、控制字元或 Windows 不允許的符號，也不可以是「.」「..」")]
    NameInvalid,
    #[error("成果描述最多 {DESCRIPTION_MAX_CHARS} 個字元")]
    DescriptionTooLong,
    #[error("成果內容最多 {ARTIFACT_MAX_BYTES} 位元組")]
    TooLarge,
    #[error("必須擇一提供 content（文字）或 content_base64，不可兩者都給或都沒有")]
    ContentSource,
    #[error("content_base64 不是有效的 Base64")]
    ContentNotBase64,
}

impl ArtifactError {
    pub fn code(self) -> &'static str {
        match self {
            ArtifactError::TooLarge => "artifact_too_large",
            _ => "artifact_invalid",
        }
    }
}

/// 登記請求中已驗證的名稱與描述（前後空白已去除）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactDraft {
    pub name: String,
    pub description: String,
}

impl ArtifactDraft {
    pub fn new(name: &str, description: &str) -> Result<Self, ArtifactError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(ArtifactError::NameEmpty);
        }
        if name.chars().count() > NAME_MAX_CHARS {
            return Err(ArtifactError::NameTooLong);
        }
        let forbidden = |c: char| c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|');
        if name == "." || name == ".." || name.chars().any(forbidden) {
            return Err(ArtifactError::NameInvalid);
        }
        let description = description.trim();
        if description.chars().count() > DESCRIPTION_MAX_CHARS {
            return Err(ArtifactError::DescriptionTooLong);
        }
        Ok(Self { name: name.to_string(), description: description.to_string() })
    }
}

/// 內容大小檢查。
pub fn check_size(len: usize) -> Result<(), ArtifactError> {
    if len > ARTIFACT_MAX_BYTES {
        return Err(ArtifactError::TooLarge);
    }
    Ok(())
}

/// 內容的 SHA-256（小寫十六進位）。
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// 某個 Workspace 在成果儲存內的相對目錄名稱（也是 manifest `storage_path` 的第一段）。
pub fn blob_storage_path(workspace_id: &str, blob_id: &str) -> String {
    format!("{workspace_id}/{blob_id}")
}

/// 成果的某一個版本：不可變，登記後不再更改。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactVersion {
    pub version: i64,
    pub description: String,
    pub size_bytes: i64,
    /// 內容的 SHA-256（小寫十六進位）。
    pub sha256: String,
    /// 相對於成果儲存根目錄的位置（一律用 `/` 分隔）。
    pub storage_path: String,
    /// 產生者：登記這個版本的 Run 與它的世代。
    pub produced_by_run_id: String,
    pub produced_by_generation: i64,
    pub created_at: Timestamp,
}

/// 一個成果（同一個 Task 內同名者）與它的全部版本，版本由舊到新。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifact {
    pub id: String,
    pub workspace_id: String,
    pub task_id: String,
    pub name: String,
    pub created_at: Timestamp,
    pub versions: Vec<ArtifactVersion>,
}

impl Artifact {
    pub fn latest_version(&self) -> Option<&ArtifactVersion> {
        self.versions.last()
    }
}

/// 一次登記的全部輸入（內容已經完整落地、雜湊與大小已算好）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registration {
    /// 這個成果尚不存在時使用的識別。
    pub new_artifact_id: String,
    pub workspace_id: String,
    pub task_id: String,
    pub run_id: String,
    pub generation: i64,
    pub draft: ArtifactDraft,
    pub size_bytes: i64,
    pub sha256: String,
    pub storage_path: String,
}

/// 登記的結果：要寫入的新版本與事件。
#[derive(Debug, Clone, PartialEq)]
pub struct Registered {
    pub artifact_id: String,
    /// 這次登記是否建立了新的成果（false 表示為既有成果追加新版本）。
    pub created: bool,
    pub version: ArtifactVersion,
    pub events: Vec<DomainEvent>,
}

/// 決定登記結果：既有成果（同 Workspace、同 Task、同名）則版本遞增，否則建立版本 1。
/// 既有版本不會被修改。
pub fn register(existing: Option<&Artifact>, registration: Registration, now: Timestamp) -> Registered {
    let next = existing.and_then(Artifact::latest_version).map_or(1, |v| v.version + 1);
    let artifact_id = existing.map_or(registration.new_artifact_id.clone(), |a| a.id.clone());
    let version = ArtifactVersion {
        version: next,
        description: registration.draft.description.clone(),
        size_bytes: registration.size_bytes,
        sha256: registration.sha256.clone(),
        storage_path: registration.storage_path.clone(),
        produced_by_run_id: registration.run_id.clone(),
        produced_by_generation: registration.generation,
        created_at: now,
    };
    let event = DomainEvent {
        aggregate_type: "artifact",
        aggregate_id: artifact_id.clone(),
        aggregate_version: next,
        event_type: EVENT_ARTIFACT_REGISTERED,
        occurred_at: now,
        payload: serde_json::json!({
            "workspace_id": registration.workspace_id,
            "task_id": registration.task_id,
            "name": registration.draft.name,
            "version": next,
            "size_bytes": registration.size_bytes,
            "sha256": registration.sha256,
            "run_id": registration.run_id,
            "generation": registration.generation,
        }),
    };
    Registered { artifact_id, created: existing.is_none(), version, events: vec![event] }
}

/// 成果儲存內的一個檔案（路徑相對於成果儲存根目錄，以 `/` 分隔）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredFile {
    pub storage_path: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnregisteredKind {
    /// 暫存檔：寫入尚未完成就中斷。內容不完整，不可使用。
    Staging,
    /// 已經原子改名、內容完整，但 manifest 還沒提交就中斷（或登記被拒）。仍不算成果。
    Unregistered,
}

/// 沒有被任何 manifest 引用的檔案；恢復時顯示給使用者，不自動刪除。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnregisteredFile {
    pub storage_path: String,
    pub size_bytes: u64,
    pub kind: UnregisteredKind,
}

/// 找出沒有被 manifest 引用的檔案：暫存檔一律是 `Staging`，其餘沒被引用的是 `Unregistered`。
pub fn classify_stored_files(files: &[StoredFile], registered_paths: &HashSet<String>) -> Vec<UnregisteredFile> {
    let mut found: Vec<UnregisteredFile> = files
        .iter()
        .filter_map(|file| {
            let kind = if file.storage_path.ends_with(STAGING_SUFFIX) {
                UnregisteredKind::Staging
            } else if registered_paths.contains(&file.storage_path) {
                return None;
            } else {
                UnregisteredKind::Unregistered
            };
            Some(UnregisteredFile { storage_path: file.storage_path.clone(), size_bytes: file.size_bytes, kind })
        })
        .collect();
    found.sort_by(|a, b| a.storage_path.cmp(&b.storage_path));
    found
}

/// 已登記版本的檔案現況（只比對存在與大小；完整雜湊在下載與預覽時核對）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileStatus {
    Ok,
    Missing,
    SizeMismatch,
}

pub fn file_status(version: &ArtifactVersion, files: &HashMap<String, u64>) -> FileStatus {
    match files.get(&version.storage_path) {
        None => FileStatus::Missing,
        Some(size) if i64::try_from(*size).is_ok_and(|s| s == version.size_bytes) => FileStatus::Ok,
        Some(_) => FileStatus::SizeMismatch,
    }
}

/// 預覽內容。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Preview {
    /// UTF-8 文字（最多 `PREVIEW_MAX_BYTES`，在字元邊界截斷）。
    Text { text: String, truncated: bool },
    /// 不是文字（含 NUL 或不是有效 UTF-8），只能下載。
    Binary,
}

pub fn preview(bytes: &[u8]) -> Preview {
    let truncated = bytes.len() > PREVIEW_MAX_BYTES;
    let head = &bytes[..bytes.len().min(PREVIEW_MAX_BYTES)];
    if head.contains(&0) {
        return Preview::Binary;
    }
    match std::str::from_utf8(head) {
        Ok(text) => Preview::Text { text: text.to_string(), truncated },
        // 只是被截斷在多位元組字元中間：丟掉那個殘缺字元
        Err(e) if truncated && e.error_len().is_none() => {
            Preview::Text { text: String::from_utf8_lossy(&head[..e.valid_up_to()]).into_owned(), truncated }
        }
        Err(_) => Preview::Binary,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registration(name: &str, run: &str, sha: &str, path: &str) -> Registration {
        Registration {
            new_artifact_id: "art_new".into(),
            workspace_id: "ws".into(),
            task_id: "task".into(),
            run_id: run.into(),
            generation: 1,
            draft: ArtifactDraft::new(name, "").unwrap(),
            size_bytes: 3,
            sha256: sha.into(),
            storage_path: path.into(),
        }
    }

    #[test]
    fn names_must_be_plain_file_names() {
        assert_eq!(ArtifactDraft::new("  report.md  ", " 說明 ").unwrap().name, "report.md");
        assert_eq!(ArtifactDraft::new("  ", ""), Err(ArtifactError::NameEmpty));
        assert_eq!(ArtifactDraft::new(&"字".repeat(NAME_MAX_CHARS + 1), ""), Err(ArtifactError::NameTooLong));
        for bad in ["..", ".", "a/b", "a\\b", "C:x", "a*b", "a\nb", "a|b", "<a>"] {
            assert_eq!(ArtifactDraft::new(bad, ""), Err(ArtifactError::NameInvalid), "{bad}");
        }
        assert_eq!(
            ArtifactDraft::new("a", &"x".repeat(DESCRIPTION_MAX_CHARS + 1)),
            Err(ArtifactError::DescriptionTooLong)
        );
    }

    #[test]
    fn size_limit_has_its_own_error_code() {
        assert!(check_size(ARTIFACT_MAX_BYTES).is_ok());
        assert_eq!(check_size(ARTIFACT_MAX_BYTES + 1), Err(ArtifactError::TooLarge));
        assert_eq!(ArtifactError::TooLarge.code(), "artifact_too_large");
        assert_eq!(ArtifactError::NameEmpty.code(), "artifact_invalid");
    }

    #[test]
    fn sha256_matches_the_known_digest() {
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    #[test]
    fn first_registration_is_version_one_and_later_ones_append_without_touching_old_versions() {
        let now = Timestamp::from_millis(10);
        let first = register(None, registration("r.md", "run1", &"a".repeat(64), "ws/blob_1"), now);
        assert!(first.created);
        assert_eq!(first.version.version, 1);
        assert_eq!(first.artifact_id, "art_new");
        assert_eq!(first.events[0].event_type, EVENT_ARTIFACT_REGISTERED);
        assert_eq!(first.events[0].aggregate_version, 1);

        let artifact = Artifact {
            id: first.artifact_id.clone(),
            workspace_id: "ws".into(),
            task_id: "task".into(),
            name: "r.md".into(),
            created_at: now,
            versions: vec![first.version.clone()],
        };
        let second = register(Some(&artifact), registration("r.md", "run2", &"b".repeat(64), "ws/blob_2"), now.plus_millis(5));
        assert!(!second.created);
        assert_eq!(second.artifact_id, "art_new");
        assert_eq!(second.version.version, 2);
        assert_eq!(second.version.storage_path, "ws/blob_2");
        assert_ne!(second.version.storage_path, first.version.storage_path);
        assert_eq!(second.events[0].aggregate_version, 2);
    }

    #[test]
    fn staging_and_unreferenced_files_are_identified_and_registered_ones_are_not() {
        let files = vec![
            StoredFile { storage_path: "ws/blob_ok".into(), size_bytes: 3 },
            StoredFile { storage_path: "ws/blob_half.partial".into(), size_bytes: 1 },
            StoredFile { storage_path: "ws/blob_renamed".into(), size_bytes: 9 },
        ];
        let registered: HashSet<String> = ["ws/blob_ok".to_string()].into();
        let found = classify_stored_files(&files, &registered);
        assert_eq!(
            found,
            vec![
                UnregisteredFile { storage_path: "ws/blob_half.partial".into(), size_bytes: 1, kind: UnregisteredKind::Staging },
                UnregisteredFile { storage_path: "ws/blob_renamed".into(), size_bytes: 9, kind: UnregisteredKind::Unregistered },
            ]
        );
    }

    #[test]
    fn file_status_compares_presence_and_size() {
        let version = ArtifactVersion {
            version: 1,
            description: String::new(),
            size_bytes: 3,
            sha256: "a".repeat(64),
            storage_path: "ws/blob".into(),
            produced_by_run_id: "run".into(),
            produced_by_generation: 1,
            created_at: Timestamp::from_millis(0),
        };
        let mut files = HashMap::new();
        assert_eq!(file_status(&version, &files), FileStatus::Missing);
        files.insert("ws/blob".to_string(), 2);
        assert_eq!(file_status(&version, &files), FileStatus::SizeMismatch);
        files.insert("ws/blob".to_string(), 3);
        assert_eq!(file_status(&version, &files), FileStatus::Ok);
    }

    #[test]
    fn preview_shows_text_cuts_on_a_character_boundary_and_refuses_binary() {
        assert_eq!(preview("你好".as_bytes()), Preview::Text { text: "你好".into(), truncated: false });

        // 3 位元組的字元在上限處被切開：丟掉殘缺字元，標示已截斷
        let mut long = vec![b'a'; PREVIEW_MAX_BYTES - 1];
        long.extend("字".as_bytes());
        match preview(&long) {
            Preview::Text { text, truncated } => {
                assert!(truncated);
                assert_eq!(text.len(), PREVIEW_MAX_BYTES - 1);
            }
            other => panic!("預期文字預覽：{other:?}"),
        }

        assert_eq!(preview(&[0x89, b'P', b'N', b'G', 0, 1]), Preview::Binary);
        assert_eq!(preview(&[0xff, 0xfe, 0x41]), Preview::Binary);
    }
}
