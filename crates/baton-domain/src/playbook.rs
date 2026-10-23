//! 隨 App 發佈的固定指示（desktop-controller-cli 票 04）：
//! - Controller 操作手冊：Controller CLI 以 `--append-system-prompt-file` 載入，有版本號，Controller 頁簽顯示版本。
//! - Worker 通則：Claude Code Adapter 附加在每個 Worker Run 的系統提示。
//! - AI 決策紀錄的標頭範本：開啟 Controller CLI 前，Workspace 沒有 `.baton/ai-decisions.md` 時以它建立。
//!
//! 內容都是唯讀文件，不含帳號名稱、額度門檻、並行上限或 worktree 路徑（這些由 Runtime 的設定與排程負責）。
//! 純資料、無 I/O。

use std::path::{Path, PathBuf};

/// Controller 操作手冊的版本；內容有任何變動就要調整，並同步手冊標題。
pub const PLAYBOOK_VERSION: &str = "1";
/// Controller 操作手冊（`execution.md` 的平台化版本）；標題行帶版本號。
pub const CONTROLLER_PLAYBOOK: &str = include_str!("playbook/controller.md");
/// Worker 通則：與 Task 的六項最小契約一起構成 Worker 的指示。
pub const WORKER_RULES: &str = include_str!("playbook/worker.md");
/// AI 決策紀錄的標頭範本（格式與 `.scratch` 的 `ai-decisions.md` 檔頭相同）。
pub const AI_DECISIONS_TEMPLATE: &str = include_str!("playbook/ai-decisions.md");
/// AI 決策紀錄相對 Workspace 目錄的位置（寫給 Agent 看的形式）。
pub const AI_DECISIONS_RELATIVE_PATH: &str = ".baton/ai-decisions.md";
/// 附加系統提示檔的檔名：Controller CLI 放在 Run 專屬的唯讀目錄，Worker 放在 Run 目錄。
pub const SYSTEM_PROMPT_FILE: &str = "system-prompt.md";

/// Workspace 的 AI 決策紀錄檔。
pub fn ai_decisions_path(workspace_dir: &Path) -> PathBuf {
    workspace_dir.join(".baton").join("ai-decisions.md")
}

/// 附加系統提示的內容：固定指示（操作手冊或 Worker 通則）在前，共同設定的指示（`CLAUDE.md`）在後。
/// Claude Code 只接受一個附加系統提示檔，所以兩者合併成一個檔案；共同指示是空白時不附加。
pub fn system_prompt(base: &str, instructions: Option<&str>) -> String {
    match instructions.filter(|text| !text.trim().is_empty()) {
        Some(text) => format!("{}\n\n---\n\n{text}", base.trim_end()),
        None => base.to_string(),
    }
}

#[cfg(test)]
mod tests;
