import type {
  AccountProfile,
  AccountRun,
  AccountScheduling,
  PlatformResponse,
  RunView,
  SchedulingOverview,
  SetupStatus,
  ToolCheck,
  VerificationRecord,
  VerificationView,
} from "./runtimeClient";

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

/** 測試用的 Run（使用 prof_1）；預設是 running。 */
export function runView(overrides: Partial<RunView> = {}): RunView {
  return {
    id: "run_1",
    workspace_id: "ws_1",
    task_id: "task_1",
    status: "running",
    profile_id: "prof_1",
    version: 1,
    generation: 1,
    contract_version: 1,
    authorization_revision: null,
    command_id: null,
    process: null,
    cancel_requested_at: null,
    end_reason: null,
    created_at: CHECKED_AT,
    updated_at: CHECKED_AT,
    started_at: null,
    ended_at: null,
    lease: null,
    latest_progress: null,
    latest_checkpoint: null,
    blocked: null,
    ...overrides,
  };
}

/** 排程總覽裡帳號 prof_1 的進行中 Run。 */
export function accountRun(overrides: Partial<AccountRun> = {}): AccountRun {
  return {
    run_id: "run_1",
    workspace_id: "ws_1",
    task_id: "task_1",
    task_title: "整理資料",
    status: "running",
    generation: 1,
    cancel_requested: false,
    ...overrides,
  };
}

/** 排程總覽裡的帳號：預設沒有進行中的 Run、用量未知、每帳號上限 2。 */
export function accountScheduling(overrides: Partial<AccountScheduling> = {}): AccountScheduling {
  return {
    profile_id: "prof_1",
    name: "主要帳號",
    dispatchable: true,
    blockers: [],
    quota_used: null,
    running: 0,
    limit: 2,
    runs: [],
    ...overrides,
  };
}

/** 測試用的排程總覽：一個可用帳號、保留交接容量（帳號不到 3 個，實際不保留）。 */
export function schedulingOverview(overrides: Partial<SchedulingOverview> = {}): SchedulingOverview {
  return {
    settings: {
      mode: "reserve_handoff",
      mode_label: "保留交接容量",
      modes: [
        {
          mode: "reserve_handoff",
          label: "保留交接容量",
          description: "每帳號 2 個、全域 6 個並行 Run；帳號數達 3 個以上時保留 1 個交接槽",
          per_account_limit: 2,
          global_limit: 6,
          handoff_reserve: 1,
        },
        {
          mode: "higher_concurrency",
          label: "較高並行",
          description: "每帳號 3 個、全域 8 個並行 Run，不保留交接槽",
          per_account_limit: 3,
          global_limit: 8,
          handoff_reserve: 0,
        },
      ],
    },
    capacity: {
      per_account_limit: 2,
      global_limit: 6,
      handoff_reserve: 0,
      normal_limit: 6,
      running: 0,
      usable_accounts: 1,
    },
    accounts: [accountScheduling()],
    queue: [],
    ...overrides,
  };
}

/** 測試用的成果雜湊（64 位小寫十六進位）。 */
export const REPORT_SHA256 = "0123456789abcdef".repeat(4);

/** 測試用的驗證資料：task_1 在 verifying，第 1 次 Run 產生 report.md v1，Runtime 會自動執行兩項檢查。 */
export function verificationView(overrides: Partial<VerificationView> = {}): VerificationView {
  return {
    task_id: "task_1",
    status: "verifying",
    basis: {
      run_id: "run_1",
      generation: 1,
      contract_version: 1,
      artifacts: [{ artifact_id: "art_1", version: 1 }],
    },
    checks: [
      { kind: "command", program: "cargo", args: ["test"], expected_exit_code: 0 },
      { kind: "artifact", name: "report.md", sha256: null },
    ],
    artifacts: [{ artifact_id: "art_1", name: "report.md", version: 1, sha256: REPORT_SHA256 }],
    automatic: true,
    records: [],
    ...overrides,
  };
}

/** 測試用的驗證紀錄：task_1 第 1 次 Run 的自動檢查通過。 */
export function verificationRecord(overrides: Partial<VerificationRecord> = {}): VerificationRecord {
  return {
    id: "ver_1",
    workspace_id: "ws_1",
    task_id: "task_1",
    run_id: "run_1",
    generation: 1,
    contract_version: 1,
    method: "checks",
    outcome: "passed",
    checks: [],
    artifacts: [{ artifact_id: "art_1", name: "report.md", version: 1, sha256: REPORT_SHA256 }],
    accepted_by: null,
    note: null,
    decided_at: CHECKED_AT,
    ...overrides,
  };
}
