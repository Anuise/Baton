//! 首次啟動精靈的判定（spec D-20）：Claude Code 與 Git for Windows 的檢查結果、官方指引，
//! 以及「背景執行行為」告知的確認。
//!
//! 純邏輯、無 I/O。缺項只提供官方指令讓使用者自行執行，Runtime 不下載也不執行任何安裝腳本。

use serde::{Deserialize, Serialize};

use crate::event::DomainEvent;
use crate::{claude_code, product, Timestamp};

/// 最低支援的 Claude Code 版本（主、次、修訂）：與 Profile 派工阻擋共用 Adapter 驗證時固定的版本
/// （`claude_code::MIN_SUPPORTED_VERSION`，票 14）。
pub const MIN_CLAUDE_CODE_VERSION: (u64, u64, u64) = (
    claude_code::MIN_SUPPORTED_VERSION.major as u64,
    claude_code::MIN_SUPPORTED_VERSION.minor as u64,
    claude_code::MIN_SUPPORTED_VERSION.patch as u64,
);

pub const CLAUDE_CODE_DOCS_URL: &str = "https://code.claude.com/docs/en/setup";
pub const GIT_DOCS_URL: &str = "https://git-scm.com/download/win";

pub const AGGREGATE_TYPE: &str = "setup";
pub const BACKGROUND_NOTICE_ID: &str = "background_notice";
pub const EVENT_BACKGROUND_NOTICE_ACKNOWLEDGED: &str = "background_notice_acknowledged";

/// Port 探測一個命令列工具的結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolProbe {
    /// 不在 PATH 上。
    NotFound,
    /// 找得到；`version_output` 是 `--version` 輸出的第一行（取不到時為 `None`）。
    Found { version_output: Option<String> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    Ok,
    Missing,
    TooOld,
    /// 找得到，但版本無法判斷，或探測本身失敗。
    Unknown,
}

/// 只顯示、不會被執行的官方指令。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Guidance {
    pub title: String,
    pub command: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCheck {
    pub status: ToolStatus,
    pub version: Option<String>,
    pub minimum_version: Option<String>,
    pub message: String,
    /// 缺項或版本過舊時，使用者可以自行執行的官方指令；`Ok` 時為空。
    pub guidance: Vec<Guidance>,
    pub docs_url: String,
}

/// 從 `--version` 輸出取出版本字串（第一個以數字開頭的字詞），例如
/// `2.1.307 (Claude Code)` → `2.1.307`、`git version 2.54.0.windows.1` → `2.54.0.windows.1`。
pub fn extract_version(output: &str) -> Option<String> {
    output
        .split_whitespace()
        .find(|word| word.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .map(str::to_string)
}

/// 版本字串的前三段數字；缺少的段視為 0。
pub fn parse_version(version: &str) -> Option<(u64, u64, u64)> {
    let mut parts = version.split('.').map(|part| part.parse::<u64>().ok());
    let major = parts.next()??;
    let minor = parts.next().flatten().unwrap_or(0);
    let patch = parts.next().flatten().unwrap_or(0);
    Some((major, minor, patch))
}

fn guidance(title: &str, command: &str) -> Guidance {
    Guidance { title: title.to_string(), command: command.to_string() }
}

fn claude_install_guidance() -> Vec<Guidance> {
    vec![
        guidance("PowerShell 安裝（官方原生安裝程式）", "irm https://claude.ai/install.ps1 | iex"),
        guidance("或使用 WinGet", "winget install Anthropic.ClaudeCode"),
    ]
}

fn git_install_guidance() -> Vec<Guidance> {
    vec![guidance("使用 WinGet 安裝 Git for Windows", "winget install --id Git.Git -e --source winget")]
}

fn failed(probe_failure: &str, tool: &str, docs_url: &str) -> ToolCheck {
    ToolCheck {
        status: ToolStatus::Unknown,
        version: None,
        minimum_version: None,
        message: format!("無法檢查 {tool}：{probe_failure}"),
        guidance: Vec::new(),
        docs_url: docs_url.to_string(),
    }
}

/// 判定 Claude Code：在 PATH 上、版本不低於最低版本。`probe` 的 `Err` 是探測本身失敗的說明。
pub fn check_claude(probe: Result<ToolProbe, String>) -> ToolCheck {
    let (major, minor, patch) = MIN_CLAUDE_CODE_VERSION;
    let minimum = format!("{major}.{minor}.{patch}");
    let base = |status, version, message: String, guidance| ToolCheck {
        status,
        version,
        minimum_version: Some(minimum.clone()),
        message,
        guidance,
        docs_url: CLAUDE_CODE_DOCS_URL.to_string(),
    };
    match probe {
        Err(reason) => ToolCheck { minimum_version: Some(minimum.clone()), ..failed(&reason, "Claude Code", CLAUDE_CODE_DOCS_URL) },
        Ok(ToolProbe::NotFound) => base(
            ToolStatus::Missing,
            None,
            format!(
                "在 PATH 上找不到 Claude Code。請自行執行下列官方指令安裝，再按「重新檢查」。{} 不會代為下載或安裝",
                product::PRODUCT_NAME
            ),
            claude_install_guidance(),
        ),
        Ok(ToolProbe::Found { version_output }) => {
            let version = version_output.as_deref().and_then(extract_version);
            match version.as_deref().and_then(parse_version) {
                None => base(
                    ToolStatus::Unknown,
                    version,
                    "找到 Claude Code，但無法判斷版本。請在終端機執行 claude --version 確認，再按「重新檢查」".into(),
                    vec![guidance("確認版本", "claude --version")],
                ),
                Some(found) if found < MIN_CLAUDE_CODE_VERSION => base(
                    ToolStatus::TooOld,
                    version.clone(),
                    format!(
                        "Claude Code 版本 {} 低於最低支援的 {minimum}。請自行執行官方更新指令，再按「重新檢查」",
                        version.as_deref().unwrap_or_default()
                    ),
                    vec![guidance("更新 Claude Code", "claude update")],
                ),
                Some(_) => base(
                    ToolStatus::Ok,
                    version.clone(),
                    format!("Claude Code {} 可用", version.as_deref().unwrap_or_default()),
                    Vec::new(),
                ),
            }
        }
    }
}

/// 判定 Git for Windows：在 PATH 上即可（版本不設下限）。
pub fn check_git(probe: Result<ToolProbe, String>) -> ToolCheck {
    let base = |status, version, message: String, guidance| ToolCheck {
        status,
        version,
        minimum_version: None,
        message,
        guidance,
        docs_url: GIT_DOCS_URL.to_string(),
    };
    match probe {
        Err(reason) => failed(&reason, "Git for Windows", GIT_DOCS_URL),
        Ok(ToolProbe::NotFound) => base(
            ToolStatus::Missing,
            None,
            format!(
                "在 PATH 上找不到 Git for Windows（Claude Code 的 Bash 工具與 {name} 的 worktree 管理需要它）。\
                 請自行執行下列官方指令或到下載頁安裝，再按「重新檢查」。{name} 不會代為下載或安裝",
                name = product::PRODUCT_NAME
            ),
            git_install_guidance(),
        ),
        Ok(ToolProbe::Found { version_output }) => {
            let version = version_output.as_deref().and_then(extract_version);
            let message = match &version {
                Some(v) => format!("Git {v} 可用"),
                None => "找到 Git".to_string(),
            };
            base(ToolStatus::Ok, version, message, Vec::new())
        }
    }
}

/// 首次使用時告知的背景執行行為（使用者故事 7）。
pub fn background_notice_text() -> String {
    format!(
        "關閉視窗後，{name} 會在背景繼續執行已派出的工作；關閉視窗不等於停止。\
         要停止所有工作，請從系統匣圖示選擇「完全退出」。",
        name = product::PRODUCT_NAME
    )
}

/// 確認背景行為告知的結果。
#[derive(Debug, Clone, PartialEq)]
pub struct Acknowledgement {
    pub acknowledged_at: Timestamp,
    /// 只有第一次確認會產生事件；重複確認不改變任何東西。
    pub event: Option<DomainEvent>,
}

/// `existing` 是先前已確認的時間（沒有為 `None`）。
pub fn acknowledge_background_notice(existing: Option<Timestamp>, now: Timestamp) -> Acknowledgement {
    match existing {
        Some(acknowledged_at) => Acknowledgement { acknowledged_at, event: None },
        None => Acknowledgement {
            acknowledged_at: now,
            event: Some(DomainEvent {
                aggregate_type: AGGREGATE_TYPE,
                aggregate_id: BACKGROUND_NOTICE_ID.to_string(),
                aggregate_version: 1,
                event_type: EVENT_BACKGROUND_NOTICE_ACKNOWLEDGED,
                occurred_at: now,
                payload: serde_json::json!({}),
            }),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(output: &str) -> Result<ToolProbe, String> {
        Ok(ToolProbe::Found { version_output: Some(output.to_string()) })
    }

    #[test]
    fn version_is_extracted_from_the_first_numeric_word() {
        assert_eq!(extract_version("2.1.307 (Claude Code)").as_deref(), Some("2.1.307"));
        assert_eq!(extract_version("git version 2.54.0.windows.1").as_deref(), Some("2.54.0.windows.1"));
        assert_eq!(extract_version("claude: command not found"), None);
        assert_eq!(extract_version(""), None);
    }

    #[test]
    fn versions_are_parsed_and_compared_numerically() {
        assert_eq!(parse_version("2.1.307"), Some((2, 1, 307)));
        assert_eq!(parse_version("2.54.0.windows.1"), Some((2, 54, 0)));
        assert_eq!(parse_version("3"), Some((3, 0, 0)));
        assert_eq!(parse_version("x.1"), None);
        assert!((2, 10, 0) > (2, 9, 99), "按數值而不是字串比較");
    }

    #[test]
    fn missing_claude_shows_official_commands_and_a_recheck_hint_without_installing() {
        let check = check_claude(Ok(ToolProbe::NotFound));

        assert_eq!(check.status, ToolStatus::Missing);
        assert!(check.guidance.iter().any(|g| g.command == "irm https://claude.ai/install.ps1 | iex"));
        assert!(check.guidance.iter().any(|g| g.command == "winget install Anthropic.ClaudeCode"));
        assert!(check.message.contains("重新檢查") && check.message.contains("不會代為"));
        assert_eq!(check.docs_url, CLAUDE_CODE_DOCS_URL);
    }

    #[test]
    fn claude_below_the_minimum_version_is_too_old_and_points_to_the_update_command() {
        let check = check_claude(found("2.0.9 (Claude Code)"));

        assert_eq!(check.status, ToolStatus::TooOld);
        assert_eq!(check.version.as_deref(), Some("2.0.9"));
        assert_eq!(check.minimum_version.as_deref(), Some("2.1.307"));
        assert_eq!(check.guidance[0].command, "claude update");
    }

    #[test]
    fn claude_at_or_above_the_minimum_version_is_ok_with_no_guidance() {
        for output in ["2.1.307 (Claude Code)", "2.2.0 (Claude Code)", "10.0.0"] {
            let check = check_claude(found(output));
            assert_eq!(check.status, ToolStatus::Ok, "{output}");
            assert!(check.guidance.is_empty());
        }
    }

    #[test]
    fn unparsable_version_and_probe_failure_are_unknown_not_ok() {
        assert_eq!(check_claude(found("weird output")).status, ToolStatus::Unknown);
        assert_eq!(check_claude(Ok(ToolProbe::Found { version_output: None })).status, ToolStatus::Unknown);
        let failed = check_claude(Err("逾時".into()));
        assert_eq!(failed.status, ToolStatus::Unknown);
        assert!(failed.message.contains("逾時"));
    }

    #[test]
    fn git_missing_gives_the_winget_command_and_download_page_and_found_is_ok() {
        let missing = check_git(Ok(ToolProbe::NotFound));
        assert_eq!(missing.status, ToolStatus::Missing);
        assert_eq!(missing.guidance[0].command, "winget install --id Git.Git -e --source winget");
        assert_eq!(missing.docs_url, GIT_DOCS_URL);

        let ok = check_git(found("git version 2.54.0.windows.1"));
        assert_eq!((ok.status, ok.version.as_deref()), (ToolStatus::Ok, Some("2.54.0.windows.1")));
        assert!(ok.guidance.is_empty());
    }

    #[test]
    fn background_notice_mentions_closing_the_window_and_the_tray_exit() {
        let text = background_notice_text();

        assert!(text.contains("背景") && text.contains("完全退出") && text.contains(product::PRODUCT_NAME));
    }

    #[test]
    fn only_the_first_acknowledgement_emits_an_event_and_later_ones_keep_the_original_time() {
        let first = acknowledge_background_notice(None, Timestamp::from_millis(5));
        let again = acknowledge_background_notice(Some(first.acknowledged_at), Timestamp::from_millis(9));

        assert_eq!(first.acknowledged_at, Timestamp::from_millis(5));
        assert_eq!(first.event.as_ref().map(|e| e.event_type), Some(EVENT_BACKGROUND_NOTICE_ACKNOWLEDGED));
        assert_eq!((again.acknowledged_at, again.event), (Timestamp::from_millis(5), None));
    }
}
