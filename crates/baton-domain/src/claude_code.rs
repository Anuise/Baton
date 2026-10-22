//! Claude Code CLI 的版本判定（票 14）：Adapter 依賴的命令列參數與 stream-json 事件格式
//! 只在已驗證過的版本上宣稱可用，版本不足或無法判定時阻擋該 Profile 的派工並說明原因。
//!
//! 純邏輯、無 I/O：版本字串由 `ClaudeEnvironment::probe_claude` 取得（`claude --version` 的第一行）。

use std::fmt;

/// 真實 Adapter 契約測試通過的最低版本（見 `docs/adapter-contract.md`）。
/// Adapter 用到的 `--setting-sources`、`--strict-mcp-config`、`--append-system-prompt-file`、
/// `--mcp-config`、`--allowedTools` 與 stream-json 的 `system/init`、`rate_limit_event`、`result` 都以這個版本為準。
pub const MIN_SUPPORTED_VERSION: CliVersion = CliVersion { major: 2, minor: 1, patch: 307 };

/// `major.minor.patch`；比較時依序比較三個數字。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CliVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl CliVersion {
    /// 從 `claude --version` 的輸出（例如 `2.1.307 (Claude Code)`）取出版本；格式不符回傳 `None`。
    pub fn parse(output: &str) -> Option<Self> {
        let token = output.split_whitespace().next()?;
        let token = token.strip_prefix('v').unwrap_or(token);
        // 預發行或建置標記（2.1.307-beta.1、2.1.307+abc）不影響三個數字的比較
        let core = token.split(['-', '+']).next()?;
        let mut parts = core.split('.');
        let number = |part: Option<&str>| part.and_then(|p| p.parse::<u32>().ok());
        let version = Self { major: number(parts.next())?, minor: number(parts.next())?, patch: number(parts.next())? };
        parts.next().is_none().then_some(version)
    }
}

impl fmt::Display for CliVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// 版本判定結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionCheck {
    Supported(CliVersion),
    /// 版本低於 `MIN_SUPPORTED_VERSION`。
    Unsupported { found: CliVersion },
    /// 找不到 Claude Code、取不到版本，或版本字串看不懂（`raw` 是原始輸出，可能沒有）。
    Unknown { raw: Option<String> },
}

/// 判定 `claude --version` 的輸出；`None` 表示沒有取得任何輸出。
pub fn check(version_output: Option<&str>) -> VersionCheck {
    let raw = version_output.map(str::trim).filter(|v| !v.is_empty());
    match raw.and_then(CliVersion::parse) {
        Some(found) if found >= MIN_SUPPORTED_VERSION => VersionCheck::Supported(found),
        Some(found) => VersionCheck::Unsupported { found },
        None => VersionCheck::Unknown { raw: raw.map(str::to_string) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(major: u32, minor: u32, patch: u32) -> CliVersion {
        CliVersion { major, minor, patch }
    }

    #[test]
    fn the_version_line_printed_by_claude_is_parsed() {
        assert_eq!(CliVersion::parse("2.1.307 (Claude Code)"), Some(v(2, 1, 307)));
        assert_eq!(CliVersion::parse("v2.3.0"), Some(v(2, 3, 0)));
        assert_eq!(CliVersion::parse("2.1.308-beta.1 (Claude Code)"), Some(v(2, 1, 308)));
    }

    #[test]
    fn text_that_is_not_a_three_part_version_is_not_a_version() {
        for bad in ["", "Claude Code", "2.1", "2.1.x", "2.1.3.4", "unknown option --version"] {
            assert_eq!(CliVersion::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn versions_compare_numerically_not_as_text() {
        assert!(v(2, 1, 307) > v(2, 1, 99));
        assert!(v(2, 10, 0) > v(2, 9, 999));
        assert!(v(3, 0, 0) > v(2, 99, 99));
    }

    #[test]
    fn the_minimum_and_newer_versions_are_supported_and_older_ones_are_not() {
        assert_eq!(check(Some("2.1.307 (Claude Code)")), VersionCheck::Supported(MIN_SUPPORTED_VERSION));
        assert_eq!(check(Some("2.2.0 (Claude Code)")), VersionCheck::Supported(v(2, 2, 0)));
        assert_eq!(check(Some("2.1.306 (Claude Code)")), VersionCheck::Unsupported { found: v(2, 1, 306) });
        assert_eq!(check(Some("1.0.128 (Claude Code)")), VersionCheck::Unsupported { found: v(1, 0, 128) });
    }

    #[test]
    fn a_missing_or_unreadable_version_is_unknown_not_supported() {
        assert_eq!(check(None), VersionCheck::Unknown { raw: None });
        assert_eq!(check(Some("  ")), VersionCheck::Unknown { raw: None });
        assert_eq!(check(Some("garbage")), VersionCheck::Unknown { raw: Some("garbage".into()) });
    }
}
