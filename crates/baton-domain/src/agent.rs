//! Agent 工具介面的純邏輯：交給 MCP shim 的環境變數名稱、MCP 設定內容，以及 Worker 回報的內容與驗證（票 11，spec D-05）。
//!
//! Agent 憑證只經環境變數交給 shim；MCP 設定檔只放「環境變數的參照」，絕不含憑證本身。
//! Worker 的回報（進度、checkpoint、受阻）都帶著憑證所屬的世代，由 Run 的 Domain 命令判定是否過期。

use serde::{Deserialize, Serialize};

use crate::Timestamp;

/// 交給 MCP shim 的環境變數：Agent 範圍憑證。
pub const AGENT_TOKEN_ENV: &str = "BATON_AGENT_TOKEN";
/// 交給 MCP shim 的環境變數：Runtime 的 loopback 位址（例如 `http://127.0.0.1:47821`）。
pub const RUNTIME_URL_ENV: &str = "BATON_RUNTIME_URL";
/// MCP 設定檔中 shim 的伺服器名稱。
pub const MCP_SERVER_NAME: &str = "baton";

/// Agent API 的工具路徑前綴；每個工具一個路徑：`{AGENT_TOOLS_PREFIX}/{工具名稱}`。
pub const AGENT_TOOLS_PREFIX: &str = "/agent/v1/tools";

/// 工具名稱。
pub const TOOL_GET_TASK_CONTRACT: &str = "get_task_contract";
pub const TOOL_REPORT_PROGRESS: &str = "report_progress";
pub const TOOL_SAVE_CHECKPOINT: &str = "save_checkpoint";
pub const TOOL_REPORT_BLOCKED: &str = "report_blocked";
/// 票 22：裁定請求與權限提示轉接。
pub const TOOL_REQUEST_DECISION: &str = "request_decision";
pub const TOOL_PERMISSION_PROMPT: &str = "permission_prompt";
/// 票 35：Controller 的動作先經決策權限表（spec D-15）判定；必須送裁定的動作被拒絕並轉成裁定請求。
pub const TOOL_CONTROLLER_ACTION: &str = "controller_action";
/// Worker 間訊息（票 18）。
pub const TOOL_SEND_MESSAGE: &str = "send_message";
pub const TOOL_READ_INBOX: &str = "read_inbox";
/// 交接（票 28）：successor Worker 確認接手。
pub const TOOL_CONFIRM_HANDOFF: &str = "confirm_handoff";

/// 進度與 checkpoint 摘要、受阻原因的長度上限（字元數）。
pub const SUMMARY_MAX_CHARS: usize = 500;
pub const REASON_MAX_CHARS: usize = 1_000;
/// checkpoint 細節的大小上限（位元組）。
pub const DETAILS_MAX_BYTES: usize = 16 * 1024;

/// 給 `claude --mcp-config` 的設定內容：只含 shim 指令與「環境變數參照」（`${NAME}` 由 Claude Code 展開），
/// 所以設定檔、Prompt 與日誌都不會出現憑證本身。
pub fn mcp_config_json(shim_program: &str) -> String {
    serde_json::json!({
        "mcpServers": {
            MCP_SERVER_NAME: {
                "command": shim_program,
                "args": [],
                "env": {
                    AGENT_TOKEN_ENV: format!("${{{AGENT_TOKEN_ENV}}}"),
                    RUNTIME_URL_ENV: format!("${{{RUNTIME_URL_ENV}}}"),
                },
            }
        }
    })
    .to_string()
}

/// Worker 回報的種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportKind {
    Progress,
    Checkpoint,
    Blocked,
}

impl ReportKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ReportKind::Progress => "progress",
            ReportKind::Checkpoint => "checkpoint",
            ReportKind::Blocked => "blocked",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        [ReportKind::Progress, ReportKind::Checkpoint, ReportKind::Blocked].into_iter().find(|k| k.as_str() == raw)
    }
}

/// Worker 回報的內容（已驗證、已去除前後空白）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReportContent {
    Progress { summary: String },
    Checkpoint { summary: String, details: String },
    Blocked { reason: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ReportError {
    #[error("摘要不能是空的")]
    SummaryEmpty,
    #[error("摘要最多 {SUMMARY_MAX_CHARS} 個字元")]
    SummaryTooLong,
    #[error("checkpoint 細節最多 {DETAILS_MAX_BYTES} 位元組")]
    DetailsTooLarge,
    #[error("受阻原因不能是空的")]
    ReasonEmpty,
    #[error("受阻原因最多 {REASON_MAX_CHARS} 個字元")]
    ReasonTooLong,
}

impl ReportError {
    pub fn code(self) -> &'static str {
        "report_invalid"
    }
}

fn trimmed(raw: &str, max_chars: usize, empty: ReportError, too_long: ReportError) -> Result<String, ReportError> {
    let text = raw.trim();
    if text.is_empty() {
        return Err(empty);
    }
    if text.chars().count() > max_chars {
        return Err(too_long);
    }
    Ok(text.to_string())
}

impl ReportContent {
    pub fn progress(summary: &str) -> Result<Self, ReportError> {
        let summary = trimmed(summary, SUMMARY_MAX_CHARS, ReportError::SummaryEmpty, ReportError::SummaryTooLong)?;
        Ok(ReportContent::Progress { summary })
    }

    pub fn checkpoint(summary: &str, details: &str) -> Result<Self, ReportError> {
        let summary = trimmed(summary, SUMMARY_MAX_CHARS, ReportError::SummaryEmpty, ReportError::SummaryTooLong)?;
        if details.len() > DETAILS_MAX_BYTES {
            return Err(ReportError::DetailsTooLarge);
        }
        Ok(ReportContent::Checkpoint { summary, details: details.to_string() })
    }

    pub fn blocked(reason: &str) -> Result<Self, ReportError> {
        let reason = trimmed(reason, REASON_MAX_CHARS, ReportError::ReasonEmpty, ReportError::ReasonTooLong)?;
        Ok(ReportContent::Blocked { reason })
    }

    pub fn kind(&self) -> ReportKind {
        match self {
            ReportContent::Progress { .. } => ReportKind::Progress,
            ReportContent::Checkpoint { .. } => ReportKind::Checkpoint,
            ReportContent::Blocked { .. } => ReportKind::Blocked,
        }
    }
}

/// 一筆已收到的 Worker 回報（追加式稽核）。`accepted = false` 表示來自已不是寫入所有者的世代：
/// 內容保留下來供稽核，但沒有改變任何目前狀態。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportRecord {
    pub workspace_id: String,
    pub run_id: String,
    /// 回報者憑證所屬的世代。
    pub generation: i64,
    pub accepted: bool,
    pub content: ReportContent,
    pub received_at: Timestamp,
}

/// 面板看到的最新進度。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgressNote {
    pub summary: String,
    pub reported_at: Timestamp,
}

/// 面板看到的最新 checkpoint。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointNote {
    pub summary: String,
    pub details: String,
    pub saved_at: Timestamp,
}

/// 面板看到的受阻回報。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockedNote {
    pub reason: String,
    pub reported_at: Timestamp,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_config_references_environment_variables_but_never_holds_a_secret() {
        let config = mcp_config_json("C:\\Baton\\baton-mcp-shim.exe");
        let parsed: serde_json::Value = serde_json::from_str(&config).unwrap();

        let server = &parsed["mcpServers"][MCP_SERVER_NAME];
        assert_eq!(server["command"], "C:\\Baton\\baton-mcp-shim.exe");
        assert_eq!(server["env"][AGENT_TOKEN_ENV], "${BATON_AGENT_TOKEN}");
        assert_eq!(server["env"][RUNTIME_URL_ENV], "${BATON_RUNTIME_URL}");
    }

    #[test]
    fn report_content_is_trimmed_and_bounded() {
        assert_eq!(ReportContent::progress("  完成一半  ").unwrap(), ReportContent::Progress { summary: "完成一半".into() });
        assert_eq!(ReportContent::progress("   "), Err(ReportError::SummaryEmpty));
        assert_eq!(ReportContent::progress(&"字".repeat(SUMMARY_MAX_CHARS + 1)), Err(ReportError::SummaryTooLong));
        assert!(ReportContent::progress(&"字".repeat(SUMMARY_MAX_CHARS)).is_ok());

        assert_eq!(ReportContent::checkpoint("摘要", &"x".repeat(DETAILS_MAX_BYTES + 1)), Err(ReportError::DetailsTooLarge));
        assert_eq!(
            ReportContent::checkpoint("摘要", "細節 ").unwrap(),
            ReportContent::Checkpoint { summary: "摘要".into(), details: "細節 ".into() }
        );

        assert_eq!(ReportContent::blocked(""), Err(ReportError::ReasonEmpty));
        assert_eq!(ReportContent::blocked(&"x".repeat(REASON_MAX_CHARS + 1)), Err(ReportError::ReasonTooLong));
    }

    #[test]
    fn report_kind_round_trips_through_its_string_form() {
        for kind in [ReportKind::Progress, ReportKind::Checkpoint, ReportKind::Blocked] {
            assert_eq!(ReportKind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(ReportKind::parse("heartbeat"), None);
    }
}
