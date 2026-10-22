import type { AccountProfile, PlatformResponse, SetupStatus, ToolCheck } from "./runtimeClient";

/** 測試用的平台狀態：Windows 11 26H2 Pro x64，正式支援。 */
export const supportedPlatform: PlatformResponse = {
  platform: {
    os_family: "windows11",
    os_build: "26300",
    feature_version: "26H2",
    edition: "Pro",
    native_arch: "x64",
    process_arch: "x64",
  },
  app_version: "0.1.0",
  assessment: { status: "supported", issues: [], support_ends_on: "2028-10-10", blocks_dispatch: false },
  support_table_source: "https://example.test/windows11-release-information",
  support_table_checked_on: "2026-10-22",
};

export const unofficialPlatform: PlatformResponse = {
  ...supportedPlatform,
  platform: { ...supportedPlatform.platform, native_arch: "arm64", process_arch: "x64" },
  assessment: {
    status: "unsupported",
    issues: [{ code: "unsupported_architecture", message: "原生架構為 arm64，目前是 x64 模擬執行，不在正式支援範圍" }],
    support_ends_on: null,
    blocks_dispatch: false,
  },
};

export const endedPlatform: PlatformResponse = {
  ...supportedPlatform,
  assessment: {
    status: "unsupported",
    issues: [{ code: "support_ended", message: "Windows 11 26H2 已結束 Microsoft 服務（L1 警告，不阻擋派工）" }],
    support_ends_on: "2028-10-10",
    blocks_dispatch: false,
  },
};

/** 2023-11-14 22:13:20 UTC。 */
export const CHECKED_AT = 1_700_000_000_000;

/** 測試用的 Profile：已用原生登入核對過、可派工。 */
export function profile(overrides: Partial<AccountProfile> = {}): AccountProfile {
  return {
    id: "prof_1",
    name: "主要帳號",
    config_dir: "C:\\Baton\\profiles\\prof_1",
    expected_account: "me@example.com",
    status: "active",
    verification: {
      checked_at: CHECKED_AT,
      logged_in: true,
      source: "native_login",
      source_detail: null,
      account: "me@example.com",
      config_dir_matches: true,
    },
    version: 3,
    created_at: CHECKED_AT,
    archived_at: null,
    dispatchable: true,
    blockers: [],
    ...overrides,
  };
}

export const okTool: ToolCheck = {
  status: "ok",
  version: "2.1.307",
  minimum_version: null,
  message: "Claude Code 2.1.307 可用",
  guidance: [],
  docs_url: "https://example.test/docs",
};

export const missingClaude: ToolCheck = {
  status: "missing",
  version: null,
  minimum_version: "2.1.0",
  message: "在 PATH 上找不到 Claude Code。請自行執行下列官方指令安裝，再按「重新檢查」。Baton 不會代為下載或安裝",
  guidance: [
    { title: "PowerShell 安裝（官方原生安裝程式）", command: "irm https://claude.ai/install.ps1 | iex" },
    { title: "或使用 WinGet", command: "winget install Anthropic.ClaudeCode" },
  ],
  docs_url: "https://code.claude.com/docs/en/setup",
};

export function setupStatus(overrides: Partial<SetupStatus> = {}): SetupStatus {
  return {
    claude_code: okTool,
    git: { ...okTool, version: "2.54.0.windows.1", message: "Git 2.54.0.windows.1 可用" },
    profiles: [profile()],
    profile_limit: 8,
    background_notice: { text: "關閉視窗後，Baton 會在背景繼續執行已派出的工作。", acknowledged_at: null },
    ready: true,
    ...overrides,
  };
}
