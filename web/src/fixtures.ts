import type {
  AccountProfile,
  AccountRun,
  AccountScheduling,
  AuthorityTable,
  BackupStatus,
  BackupSummary,
  BudgetView,
  ControllerRun,
  ConversationTurn,
  ConversationView,
  DrillReport,
  DecisionRequestView,
  ExitRunResult,
  Handoff,
  LifecycleStatus,
  ImportPreview,
  Metrics,
  MetricsResponse,
  IntegrationEntry,
  PlatformResponse,
  QuotaAssessment,
  RecoveryAttempt,
  RecoveryEvent,
  RecoveryStatusView,
  QuotaObservation,
  RunContext,
  RunView,
  SchedulingOverview,
  SettingsVersionSummary,
  SetupStatus,
  TimelineEvent,
  ToolCheck,
  WorkspaceLifecycleView,
  VerificationRecord,
  VerificationView,
  WorkdirView,
  WorktreeCleanup,
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
      cli_version: "2.1.307 (Claude Code)",
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

/** 測試用的裁定請求：pending 的 Agent 請求，有兩個選項與一個被阻擋的 Task。 */
export function decisionView(overrides: Partial<DecisionRequestView> = {}): DecisionRequestView {
  return {
    id: "dec_1",
    workspace_id: "ws_1",
    run_id: "run_1",
    task_id: "task_1",
    task_title: "整理文件",
    kind: "agent",
    status: "pending",
    question: "要保留舊的目錄結構嗎？",
    reason: "兩種做法都符合契約，需要使用者決定",
    options: [
      { id: "keep", label: "保留", tradeoff: "不影響既有連結，但結構較亂" },
      { id: "restructure", label: "重整", tradeoff: null },
    ],
    recommendation: "keep",
    evidence: ["docs/ 底下有 12 個檔案", "README 引用了舊路徑"],
    urgency: "normal",
    deadline_at: null,
    overdue: false,
    revision: 1,
    deferred_until: null,
    deferred: false,
    merged_into: null,
    status_reason: null,
    links: [],
    merged_from: [],
    blocking: [{ task_id: "task_1", task_title: "整理文件", task_status: "waiting_decision" }],
    permission: null,
    authority: null,
    target: { task_id: "task_1", contract_version: 1, authorization_revision: 1 },
    allowed_actions: ["accept", "modify", "answer", "investigate", "defer", "reject"],
    created_at: CHECKED_AT,
    decision: null,
    ...overrides,
  };
}

/** 測試用的決策權限表：一項內建可自主、一項已收窄、一項內建必須送裁定、一項只能人工發起。 */
export function authorityTable(overrides: Partial<AuthorityTable> = {}): AuthorityTable {
  return {
    revision: 1,
    entries: [
      { matter: "split_task", label: "拆分、合併、排序 Task", builtin: "autonomous", effective: "autonomous", narrowed: false },
      { matter: "retry_task", label: "重試 Task", builtin: "autonomous", effective: "requires_decision", narrowed: true },
      { matter: "change_acceptance", label: "修改完成條件或驗收標準", builtin: "requires_decision", effective: "requires_decision", narrowed: false },
      { matter: "retry_after_recovery_failures", label: "三次恢復失敗後的重試", builtin: "human_only", effective: "human_only", narrowed: false },
    ],
    ...overrides,
  };
}

/** 測試用的已採用共同設定版本摘要。 */
export function settingsVersionSummary(overrides: Partial<SettingsVersionSummary> = {}): SettingsVersionSummary {
  return {
    version: 1,
    content_hash: "a1b2c3d4e5f60718293a4b5c6d7e8f90",
    adopted_at: CHECKED_AT,
    counts: { preferences: 3, instructions: 1, hooks: 2, mcp_servers: 1, plugins: 4 },
    exclusion_count: 2,
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
    admitted_at: null,
    settings_version: null,
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

/** 測試用的匯入預覽：找到設定檔與指示檔、排除兩項、有一筆新增與一筆變更。 */
export function importPreview(overrides: Partial<ImportPreview> = {}): ImportPreview {
  return {
    candidate_hash: "f0e1d2c3b4a5968778695a4b3c2d1e0f",
    sources: [
      { kind: "settings", path: "C:\\Users\\me\\.claude\\settings.json", found: true, sha256: "aa11" },
      { kind: "instructions", path: "C:\\Users\\me\\.claude\\CLAUDE.md", found: false, sha256: null },
      { kind: "user_state", path: "C:\\Users\\me\\.claude.json", found: true, sha256: "bb22" },
    ],
    exclusions: [
      { source: "user_state", item: "oauthAccount", reason: "login_related" },
      { source: "settings", item: "env.MY_API_KEY", reason: "secret_key" },
    ],
    bundle: {
      preferences: { model: "opus", theme: "dark" },
      instructions: null,
      hooks: {},
      mcp_servers: {},
      plugins: {},
      marketplaces: {},
    },
    base_version: 1,
    diff: [
      { category: "preferences", key: "theme", change: "added", before: null, after: "dark" },
      { category: "preferences", key: "model", change: "changed", before: "sonnet", after: "opus" },
      { category: "mcp", key: "docs", change: "removed", before: { command: "docs-server" }, after: null },
    ],
    changed: true,
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

/** 測試用的額度觀測：預設是 prof_1 五小時視窗、CLI 回報的已觀測用量 30%。 */
export function quotaObservation(overrides: Partial<QuotaObservation> = {}): QuotaObservation {
  return {
    profile_id: "prof_1",
    origin: "adapter",
    source: "observed",
    window: "five_hour",
    status: "allowed",
    used: 0.3,
    tokens: null,
    limit_tokens: null,
    reset_at: null,
    observed_at: CHECKED_AT,
    confidence: "observed",
    raw_ref: "run:run_1",
    ...overrides,
  };
}

/** 測試用的額度判斷：預設完全未知（兩個視窗都沒有觀測、觀測不完整）。 */
export function quotaAssessment(overrides: Partial<QuotaAssessment> = {}): QuotaAssessment {
  return {
    used: null,
    source: null,
    confidence: "unknown",
    reset_at: null,
    observed_at: null,
    complete: false,
    incomplete_reason:
      "觀測不完整：五小時、七日視窗沒有來自 Claude Code 的已觀測用量；推估只計入這台電腦上 Baton 派出的 Run，其他裝置與互動式使用的用量看不到",
    windows: [
      { window: "five_hour", label: "五小時", effective: null, observed: null, estimated: null },
      { window: "seven_day", label: "七日", effective: null, observed: null, estimated: null },
    ],
    external: null,
    ...overrides,
  };
}

/** 排程總覽裡的帳號：預設沒有進行中的 Run、用量未知、沒有設定方案上限、每帳號上限 2、額度門檻從未離開 open。 */
export function accountScheduling(overrides: Partial<AccountScheduling> = {}): AccountScheduling {
  return {
    profile_id: "prof_1",
    name: "主要帳號",
    dispatchable: true,
    blockers: [],
    quota_used: null,
    quota: quotaAssessment(),
    plan_limits: { five_hour_tokens: null, seven_day_tokens: null },
    running: 0,
    limit: 2,
    runs: [],
    quota_gate: null,
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

/** 測試用的備份摘要：CHECKED_AT 的每日備份，含 2 個成果檔案，同時是每日與每週保留。 */
export function backupSummary(overrides: Partial<BackupSummary> = {}): BackupSummary {
  return {
    id: `${CHECKED_AT}-daily`,
    trigger: "daily",
    created_at: CHECKED_AT,
    artifact_count: 2,
    retained_as: ["daily", "weekly"],
    ...overrides,
  };
}

/** 生命週期裡的 Workspace：預設不自動續跑、沒有暫停。 */
export function workspaceLifecycle(overrides: Partial<WorkspaceLifecycleView> = {}): WorkspaceLifecycleView {
  return {
    workspace_id: "ws_1",
    name: "筆記",
    auto_resume_after_exit: false,
    auto_resume_across_login: false,
    paused_since: null,
    paused_by: null,
    awaiting_resume: false,
    ...overrides,
  };
}

/** 測試用的還原演練結果：演練 backupSummary() 那份備份，五項核對全部通過、沒有中斷的 Run。 */
export function drillReport(overrides: Partial<DrillReport> = {}): DrillReport {
  return {
    backup_id: `${CHECKED_AT}-daily`,
    backup_created_at: CHECKED_AT,
    performed_at: CHECKED_AT + 3_600_000,
    passed: true,
    checks: [
      { kind: "restore", passed: true, detail: "已還原到暫存目錄" },
      { kind: "database_integrity", passed: true, detail: "integrity_check 回報 ok" },
      { kind: "foreign_keys", passed: true, detail: "沒有違反外鍵的資料" },
      { kind: "artifact_hashes", passed: true, detail: "2 個成果檔案的 SHA-256 都相符" },
      { kind: "artifact_relations", passed: true, detail: "成果版本都對得上檔案" },
    ],
    interrupted_runs: 0,
    ...overrides,
  };
}

/** 完全退出時 run_1 的停止結果；預設還在等待停止。 */
export function exitRun(overrides: Partial<ExitRunResult> = {}): ExitRunResult {
  return { run_id: "run_1", workspace_id: "ws_1", task_id: "task_1", outcome: "pending", ...overrides };
}

/** 測試用的生命週期：預設 Runtime 運作中、沒有停止意圖、登入時不自動啟動、一個 Workspace。 */
export function lifecycleStatus(overrides: Partial<LifecycleStatus> = {}): LifecycleStatus {
  return {
    phase: "running",
    stop_intent_active: false,
    timing: { stop_grace_ms: 30_000, kill_confirm_ms: 10_000 },
    exit: null,
    active_runs: 0,
    login_startup: { enabled: false, registered: false },
    workspaces: [workspaceLifecycle()],
    ...overrides,
  };
}

/** 測試用的恢復嘗試：第 1 輪第 1 次，處置中，run_1 已重新排隊。 */
export function recoveryAttempt(overrides: Partial<RecoveryAttempt> = {}): RecoveryAttempt {
  return {
    id: "rcv_1",
    round: 1,
    seq: 1,
    started_at: CHECKED_AT,
    finished_at: null,
    outcome: "in_progress",
    failure: null,
    affected: [
      { run_id: "run_1", workspace_id: "ws_1", task_id: "task_1", disposition: "requeued", message: null },
    ],
    ...overrides,
  };
}

/** 測試用的恢復事件：第 1 輪、嘗試中、已用 1 次。 */
export function recoveryEvent(overrides: Partial<RecoveryEvent> = {}): RecoveryEvent {
  return {
    id: "rec_1",
    round: 1,
    status: "attempting",
    attempts_used: 1,
    detected_at: CHECKED_AT,
    next_attempt_at: null,
    observe_until: null,
    last_failure: null,
    closed_at: null,
    version: 2,
    updated_at: CHECKED_AT,
    attempts: [recoveryAttempt()],
    ...overrides,
  };
}

/** 測試用的崩潰恢復總覽：預設沒有恢復事件。 */
export function recoveryStatus(overrides: Partial<RecoveryStatusView> = {}): RecoveryStatusView {
  return { event: null, max_attempts: 3, can_retry: false, blocking: null, ...overrides };
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
    integration: null,
    ...overrides,
  };
}

/** 測試用的 context receipt：run_1 載入 Task 契約 v3 與一則決策，一則教訓因超出預算而省略。 */
export function runContextView(overrides: Partial<RunContext> = {}): RunContext {
  return {
    receipt: {
      run_id: "run_1",
      task_id: "task_1",
      built_at: CHECKED_AT,
      budget_tokens: 8000,
      used_tokens: 1234,
      prompt_sha256: REPORT_SHA256,
      items: [
        { kind: "contract", ref_id: "task_1", version: 3, title: "部署", tokens: 120, status: "loaded", omitted_reason: null },
        { kind: "decision", ref_id: "know_1", version: 1, title: "一律用 pnpm", tokens: 40, status: "loaded", omitted_reason: null },
        { kind: "messages", ref_id: "run_1", version: null, title: "未讀訊息 2 則", tokens: 30, status: "loaded", omitted_reason: null },
        {
          kind: "lesson",
          ref_id: "know_2",
          version: 2,
          title: "部署前先備份",
          tokens: 4100,
          status: "omitted",
          omitted_reason: "budget_exceeded",
        },
      ],
    },
    prompt: "# Task 契約\n部署到正式環境",
    ...overrides,
  };
}

/** 測試用的備份狀態：一份備份與一次通過的還原演練。 */
export function backupStatus(overrides: Partial<BackupStatus> = {}): BackupStatus {
  return {
    backups: [backupSummary()],
    latest_drill: drillReport(),
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

/** 測試用的工作目錄：task_1 已就緒的 worktree（平台分支 baton/task_1，整合到 main），run_1 在其中執行過。 */
export function workdirView(overrides: Partial<WorkdirView> = {}): WorkdirView {
  return {
    task_id: "task_1",
    workspace_id: "ws_1",
    kind: "worktree",
    path: "C:\\work\\app\\.baton\\worktrees\\task_1",
    branch: "baton/task_1",
    base_branch: "main",
    status: "ready",
    failure: null,
    version: 1,
    attempted_at: CHECKED_AT,
    ready_at: CHECKED_AT,
    run_ids: ["run_1"],
    cleanup: null,
    ...overrides,
  };
}

/** 測試用的清理登記：task_1 已整合的 worktree 等待清理，尚未嘗試過。 */
export function worktreeCleanup(overrides: Partial<WorktreeCleanup> = {}): WorktreeCleanup {
  return {
    task_id: "task_1",
    workspace_id: "ws_1",
    path: "C:\\work\\app\\.baton\\worktrees\\task_1",
    branch: "baton/task_1",
    status: "pending",
    version: 1,
    attempts: 0,
    failure: null,
    requested_at: CHECKED_AT,
    next_attempt_at: null,
    finished_at: null,
    ...overrides,
  };
}

/** 測試用的整合項目：task_1 第 1 次 Run 的平台分支排隊等待整合到 main。 */
export function integrationEntry(overrides: Partial<IntegrationEntry> = {}): IntegrationEntry {
  return {
    id: "int_1",
    seq: 1,
    workspace_id: "ws_1",
    task_id: "task_1",
    run_id: "run_1",
    generation: 1,
    branch: "baton/task_1",
    target_branch: "main",
    status: "queued",
    version: 1,
    merge_commit: null,
    target_tip: null,
    checks: [],
    failure: null,
    queued_at: CHECKED_AT,
    started_at: null,
    finished_at: null,
    ...overrides,
  };
}

/** 測試用的對話輸入：conv_1 的第 1 筆，已處理的使用者訊息。 */
export function conversationTurn(overrides: Partial<ConversationTurn> = {}): ConversationTurn {
  return {
    id: "turn_1",
    conversation_id: "conv_1",
    seq: 1,
    kind: "user_message",
    content: "先整理文件目錄",
    status: "processed",
    controller_run_id: "crun_1",
    attempts: 1,
    source: null,
    created_at: CHECKED_AT,
    processed_at: CHECKED_AT + 60_000,
    ...overrides,
  };
}

/** 測試用的 Controller Run：conv_1 的第一輪，使用 prof_1，預設 running。 */
export function controllerRun(overrides: Partial<ControllerRun> = {}): ControllerRun {
  return {
    id: "crun_1",
    conversation_id: "conv_1",
    workspace_id: "ws_1",
    profile_id: "prof_1",
    status: "running",
    version: 1,
    command_id: "cmd_1",
    prompt: "先整理文件目錄",
    working_dir: "C:\\work\\notes",
    model: null,
    input_turn_ids: ["turn_1"],
    agent_session_id: "ses_1",
    resume_native_session_id: null,
    native_session_id: null,
    process: null,
    end_reason: null,
    handoff: null,
    created_at: CHECKED_AT,
    updated_at: CHECKED_AT,
    started_at: CHECKED_AT,
    ended_at: null,
    ...overrides,
  };
}

/** 測試用的對話：預設還沒有任何訊息、不忙碌。 */
export function conversationView(overrides: Partial<ConversationView> = {}): ConversationView {
  return {
    conversation_id: null,
    busy: false,
    active_run: null,
    next_wake_at: null,
    waiting: null,
    turns: [],
    runs: [],
    ...overrides,
  };
}

/** 測試用的修正與重試預算：task_1 三種計數都還沒用。 */
export function budgetView(overrides: Partial<BudgetView> = {}): BudgetView {
  return {
    task_id: "task_1",
    counters: [
      { kind: "implementation", label: "實作失敗修正", used: 0, limit: 3 },
      { kind: "verification", label: "驗證不完整重驗", used: 0, limit: 2 },
      { kind: "infrastructure", label: "基礎設施重試", used: 0, limit: 5 },
    ],
    retry_not_before: null,
    exhausted: null,
    decision_request_id: null,
    ...overrides,
  };
}

/** 測試用的交接：task_1 第 1 次 Run 從 prof_1 交接到 prof_2（達 85% 交接門檻），剛要求、尚未保存檢查點。 */
export function handoff(overrides: Partial<Handoff> = {}): Handoff {
  return {
    id: "ho_1",
    workspace_id: "ws_1",
    task_id: "task_1",
    from_run_id: "run_1",
    from_profile_id: "prof_1",
    from_generation: 1,
    to_profile_id: "prof_2",
    to_run_id: null,
    to_generation: null,
    phase: "requested",
    reason: "drain_threshold",
    checkpoint: null,
    stop_outcome: null,
    confirmation: null,
    abandoned_reason: null,
    requested_at: CHECKED_AT,
    stopping_at: null,
    transferred_at: null,
    confirmed_at: null,
    waiting_at: null,
    next_check_at: null,
    wait_attempts: 0,
    wait_reset_at: null,
    wait_reasons: [],
    updated_at: CHECKED_AT,
    version: 1,
    ...overrides,
  };
}

/** 測試用的時間線事件：task_1 第 1 次 Run 建立（派工）。 */
export function timelineEvent(overrides: Partial<TimelineEvent> = {}): TimelineEvent {
  return {
    seq: 1,
    occurred_at: CHECKED_AT,
    category: "dispatch",
    aggregate_type: "run",
    aggregate_id: "run_1",
    event_type: "run_created",
    task_ids: ["task_1"],
    run_id: "run_1",
    payload: {},
    ...overrides,
  };
}

/** 測試用的觀測指標：每項都有資料（沒有 null）。 */
export function metrics(overrides: Partial<Metrics> = {}): Metrics {
  return {
    events: 42,
    task_completion: { completion: { numerator: 1, denominator: 3, rate: 1 / 3 }, cancelled: 1 },
    handoff: { success: { numerator: 1, denominator: 2, rate: 0.5 }, abandoned: 1, confirmed: 1, in_flight: 0 },
    recovery: { count: 1, pending: 0, average_ms: 40_000, max_ms: 40_000 },
    duplicate_operations: { blocked: 3, already_succeeded: 2, result_unknown: 1, in_progress_elsewhere: 0, unknown_outcomes: 1 },
    decision_wait: { count: 2, pending: 1, average_ms: 40_000, max_ms: 150_000 },
    lesson_recurrence: { rate: { numerator: 1, denominator: 4, rate: 0.25 }, recurrences: 2, prevented: 1, reported: 1 },
    usage: { total_tokens: 2000, runs: 3, by_stage: [{ stage: "implement", tokens: 2000, runs: 3 }] },
    context: { runs_known: 2, runs_unknown: 1, average_tokens: 45_000, max_tokens: 60_000 },
    ...overrides,
  };
}

/** 測試用的觀測指標回應：一個帳號的服務端額度兩個視窗都看得到。 */
export function metricsResponse(overrides: Partial<MetricsResponse> = {}): MetricsResponse {
  return {
    workspace_id: "ws_1",
    metrics: metrics(),
    server_quota: [{ profile_id: "prof_1", name: "Work-1", five_hour_used: 0.42, seven_day_used: 0.1 }],
    ...overrides,
  };
}
