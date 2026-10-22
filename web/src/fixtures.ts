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
  DeviationView,
  DrillReport,
  DecisionRequestView,
  ExitRunResult,
  Handoff,
  LifecycleStatus,
  ImportPreview,
  Maintenance,
  MaintenanceOutcome,
  Metrics,
  MetricsResponse,
  IntegrationEntry,
  PlatformResponse,
  Projection,
  QuotaAssessment,
  RecoveryAttempt,
  RecoveryEvent,
  RecoveryStatusView,
  ReviewAxisView,
  ReviewFinding,
  ReviewList,
  ReviewView,
  QuotaObservation,
  RunContext,
  RunView,
  SchedulingOverview,
  SettingsVersionSummary,
  SetupStatus,
  StageAttempt,
  StageView,
  TimelineEvent,
  UnderstandingItemView,
  UnderstandingView,
  ToolCheck,
  UpdateStatusView,
  EvidenceBundle,
  VerificationAttempt,
  VerificationOverview,
  VerificationProfileDefinition,
  VerificationProfileVersion,
  VerificationRequest,
  VerificationStepRecord,
  WorkflowList,
  WorkflowView,
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

/** 測試用的更新狀態：0.1.0 已檢查過、是最新版本、沒有進行中或最近一次的維護。 */
export function updateStatus(overrides: Partial<UpdateStatusView> = {}): UpdateStatusView {
  return {
    current_version: "0.1.0",
    status: "up_to_date",
    available: null,
    checked_at: CHECKED_AT,
    next_check_at: CHECKED_AT + 86_400_000,
    error: null,
    maintenance: null,
    last_maintenance: null,
    ...overrides,
  };
}

/** 測試用的可更新狀態：有 0.2.0 可以更新（附更新說明）。 */
export function availableUpdateStatus(overrides: Partial<UpdateStatusView> = {}): UpdateStatusView {
  return updateStatus({
    status: "available",
    available: { version: "0.2.0", notes: "修正完全退出的逾時", published_at: "2023-11-15T00:00:00Z" },
    ...overrides,
  });
}

/** 測試用的進行中維護：從 0.1.0 更新到 0.2.0，安裝程式已下載並驗證，停止中。 */
export function maintenance(overrides: Partial<Maintenance> = {}): Maintenance {
  return {
    kind: "update",
    phase: "stopping",
    requested_at: CHECKED_AT,
    from_version: "0.1.0",
    target_version: "0.2.0",
    package_path: "C:\\Baton\\updates\\Baton_0.2.0_x64-setup.exe",
    backup_id: null,
    message: null,
    completed_at: null,
    ...overrides,
  };
}

/** 測試用的最近一次維護：已從 0.1.0 更新到 0.2.0，在 CHECKED_AT 一分鐘後結束。 */
export function maintenanceOutcome(overrides: Partial<MaintenanceOutcome> = {}): MaintenanceOutcome {
  return {
    kind: "update",
    from_version: "0.1.0",
    running_version: "0.2.0",
    target_version: "0.2.0",
    applied: true,
    requested_at: CHECKED_AT,
    finished_at: CHECKED_AT + 60_000,
    backup_id: "bk_pre_update",
    message: null,
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

/** 測試用的專案理解項目：Project Brief 的已確認描述，附一個未變更的檔案來源。 */
export function understandingItem(overrides: Partial<UnderstandingItemView> = {}): UnderstandingItemView {
  return {
    id: "und_1",
    workspace_id: "ws_1",
    section: "brief",
    key: "purpose",
    text: "整理個人筆記的工具",
    status: "confirmed",
    sources: [
      {
        kind: "file",
        path: "README.md",
        line: 3,
        version: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        checked_at: CHECKED_AT,
        changed: false,
      },
    ],
    revision: 1,
    recorded_by: "controller",
    created_at: CHECKED_AT,
    updated_at: CHECKED_AT,
    stale: false,
    ...overrides,
  };
}

/** 測試用的投影：與 Baton 版本同步的 GLOSSARY.md 第 1 版。 */
export function projection(overrides: Partial<Projection> = {}): Projection {
  return {
    workspace_id: "ws_1",
    path: "GLOSSARY.md",
    kind: "glossary",
    version: 1,
    body_hash: "hash_body",
    baseline_hash: "hash_body",
    status: "in_sync",
    external_hash: null,
    detected_at: null,
    revision: 1,
    updated_at: CHECKED_AT,
    ...overrides,
  };
}

/** 測試用的專案理解：Git repo、一個項目、一個同步中的投影。 */
export function understandingView(overrides: Partial<UnderstandingView> = {}): UnderstandingView {
  return {
    workspace_id: "ws_1",
    projection_enabled: true,
    items: [understandingItem()],
    projections: [projection()],
    ...overrides,
  };
}

/** 測試用的 Stage 嘗試：Controller 以 prof_1 記錄的第 1 次「建立重現」，通過且附一個有效的產出物證據。 */
export function stageAttempt(overrides: Partial<StageAttempt> = {}): StageAttempt {
  return {
    id: "att_1",
    workflow_id: "wf_1",
    stage_key: "reproduction",
    attempt: 1,
    outcome: "passed",
    evidence: [{ check: "reproduction", kind: "artifact", artifact_id: "art_1", status: "valid" }],
    missing: [],
    note: "以單元測試重現匯出錯誤",
    recorded_by: "ctl_1",
    profile_id: "prof_1",
    created_at: CHECKED_AT,
    ...overrides,
  };
}

/** 測試用的 Stage：沒有 Gate、尚未進行的「建立重現」，失敗時重試。 */
export function stageView(overrides: Partial<StageView> = {}): StageView {
  return {
    key: "reproduction",
    title: "建立重現",
    method: "reproduction",
    entry_conditions: ["已有症狀描述"],
    expected_outputs: ["能捕捉原症狀的重現"],
    allowed_actions: ["讀取程式碼", "撰寫測試"],
    budget: { max_attempts: 3 },
    exit_checks: [{ key: "reproduction", description: "能捕捉原症狀的重現", requires: "artifact" }],
    failure_route: { kind: "retry" },
    gate: "none",
    state: "pending",
    attempts: [],
    ...overrides,
  };
}

/**
 * 測試用的 Workflow：缺陷處理第 1 版（已有第 2 版），綁定 task_1，進行中。
 * 「建立重現」已通過、目前在「縮小範圍」、最後是需要裁定的「修正確認」。
 */
export function workflowView(overrides: Partial<WorkflowView> = {}): WorkflowView {
  return {
    id: "wf_1",
    workspace_id: "ws_1",
    goal_id: "goal_1",
    task_id: "task_1",
    task_title: "修正匯出錯誤",
    playbook_version_id: "pbv_1",
    playbook_key: "defect",
    playbook_version: 1,
    playbook_title: "缺陷處理",
    latest_playbook_version: 2,
    situation: "defect",
    route_reason: "情境「缺陷處理」→ 缺陷處理（defect）第 1 版。Controller 的判斷：使用者回報匯出失敗",
    status: "active",
    current_stage: "minimization",
    blocked_reason: null,
    resume_stage: null,
    revision: 3,
    created_by: "ctl_1",
    created_at: CHECKED_AT,
    updated_at: CHECKED_AT,
    stages: [
      stageView({
        state: "passed",
        attempts: [
          stageAttempt({
            outcome: "incomplete",
            evidence: [],
            missing: [{ check: "reproduction", reason: "缺少證據：能捕捉原症狀的重現" }],
            note: "還沒有重現",
          }),
          stageAttempt({ id: "att_2", attempt: 2, created_at: CHECKED_AT + 60_000 }),
        ],
      }),
      stageView({
        key: "minimization",
        title: "縮小範圍",
        method: undefined,
        entry_conditions: ["已有重現"],
        expected_outputs: ["最小重現"],
        allowed_actions: ["修改測試"],
        budget: { max_attempts: 2 },
        exit_checks: [{ key: "minimal", description: "最小重現", requires: "artifact" }],
        failure_route: { kind: "return_to", stage: "reproduction" },
        state: "current",
      }),
      stageView({
        key: "fix_review",
        title: "修正確認",
        method: "review",
        entry_conditions: ["已有修正"],
        expected_outputs: ["使用者確認"],
        allowed_actions: [],
        budget: { max_attempts: 1 },
        exit_checks: [{ key: "approved", description: "使用者同意修正", requires: "decision" }],
        failure_route: { kind: "block" },
        gate: "decision",
      }),
    ],
    ...overrides,
  };
}

export function workflowList(overrides: Partial<WorkflowList> = {}): WorkflowList {
  return { workspace_id: "ws_1", workflows: [workflowView()], ...overrides };
}

/** 測試用的 finding：rev_1 規範軸上尚未分類的一項。 */
export function reviewFinding(overrides: Partial<ReviewFinding> = {}): ReviewFinding {
  return {
    id: "fnd_1",
    review_id: "rev_1",
    axis: "standards",
    title: "匯出函式吞掉錯誤",
    detail: "寫檔失敗時沒有回報",
    location: "src/export.rs:42",
    state: "open",
    classification: null,
    reason: null,
    decision_request_id: null,
    decided_by: null,
    classified_by: null,
    version: 1,
    created_at: CHECKED_AT,
    updated_at: CHECKED_AT,
    ...overrides,
  };
}

/** 測試用的一軸結果：沒有 finding、通過。 */
export function reviewAxisView(overrides: Partial<ReviewAxisView> = {}): ReviewAxisView {
  return {
    summary: "符合專案慣例",
    paths: ["src/export.rs"],
    reused_from: null,
    reviewer_task_id: "task_2",
    passed: true,
    findings: [],
    ...overrides,
  };
}

/** 測試用的審查：task_2 審查 task_1 的 candidate e4f5a6b，兩軸都通過；需求軸沿用 rev_1。 */
export function reviewView(overrides: Partial<ReviewView> = {}): ReviewView {
  return {
    id: "rev_2",
    workspace_id: "ws_1",
    subject_task_id: "task_1",
    subject_task_title: "修正匯出錯誤",
    reviewer_run_id: "run_2",
    reviewer_task_id: "task_2",
    reviewer_task_title: "審查匯出修正",
    base_revision: "a1b2c3d",
    candidate_revision: "e4f5a6b",
    spec_version: 2,
    standards_source: "AGENTS.md",
    standards: reviewAxisView(),
    spec: reviewAxisView({ summary: "滿足契約的驗收條件", paths: [], reused_from: "rev_1" }),
    passed: true,
    stale_reason: null,
    created_at: CHECKED_AT + 60_000,
    ...overrides,
  };
}

/**
 * 測試用的未通過審查：對較早的 candidate 9c8d7e6，規範軸有一項尚未分類的 finding；
 * 需求軸的 finding 已經使用者同意駁回而通過。已有對較新 candidate 的審查，因此已過期。
 */
export function failingReviewView(overrides: Partial<ReviewView> = {}): ReviewView {
  return reviewView({
    id: "rev_1",
    candidate_revision: "9c8d7e6",
    standards: reviewAxisView({ summary: "錯誤處理不符合慣例", passed: false, findings: [reviewFinding()] }),
    spec: reviewAxisView({
      summary: "匯出格式與契約一致",
      paths: ["src/export.rs", "tests/export.rs"],
      findings: [
        reviewFinding({
          id: "fnd_2",
          axis: "spec",
          title: "CSV 標頭順序不同",
          detail: "契約沒有規定標頭順序",
          location: null,
          state: "dismissed",
          classification: "dismiss",
          reason: "契約沒有規定標頭順序",
          decision_request_id: "dec_1",
          decided_by: "desktop",
          classified_by: "ctl_1",
          version: 2,
        }),
      ],
    }),
    passed: false,
    stale_reason: "已有對較新 candidate 的審查",
    created_at: CHECKED_AT,
    ...overrides,
  });
}

/** 測試用的設計偏離：task_1 回報、使用者已接受，「匯出設計」從第 1 版更新為第 2 版。 */
export function deviationView(overrides: Partial<DeviationView> = {}): DeviationView {
  return {
    id: "dev_1",
    workspace_id: "ws_1",
    task_id: "task_1",
    reported_by_run_id: "run_1",
    reported_generation: 1,
    design_artifact_id: "art_design",
    design_version: 1,
    deviation: "改用串流匯出",
    reason: "大檔案會耗盡記憶體",
    affected_contracts: ["ExportService::export 的回傳型別"],
    proposed_design: "匯出改為逐列寫入",
    status: "accepted",
    resolution_reason: "串流匯出較穩定",
    decision_request_id: "dec_2",
    accepted_by: "desktop",
    resolved_by: "ctl_1",
    accepted_design_version: 2,
    version: 3,
    created_at: CHECKED_AT,
    updated_at: CHECKED_AT,
    design_name: "匯出設計",
    task_title: "修正匯出錯誤",
    ...overrides,
  };
}

/** 測試用的審查清單：通過與未通過（已過期）的審查各一份；已接受與等待裁定的設計偏離各一項。 */
export function reviewList(overrides: Partial<ReviewList> = {}): ReviewList {
  return {
    workspace_id: "ws_1",
    reviews: [reviewView(), failingReviewView()],
    deviations: [
      deviationView({
        id: "dev_2",
        deviation: "匯出檔名加上時間戳",
        reason: "避免覆寫既有檔案",
        affected_contracts: [],
        status: "awaiting_decision",
        resolution_reason: "檔名規則屬於設計範圍",
        decision_request_id: "dec_3",
        accepted_by: null,
        resolved_by: "ctl_1",
        accepted_design_version: null,
        version: 2,
        created_at: CHECKED_AT + 60_000,
        updated_at: CHECKED_AT + 60_000,
      }),
      deviationView(),
    ],
    ...overrides,
  };
}

/**
 * 測試用的 Profile 內容：代表旅程「冒煙測試」與一般旅程「匯出報表」；
 * Feature Map 的「匯入」沒有旅程，另宣告「圖形介面」不在自動驗證範圍內。
 */
export function verificationProfileDefinition(
  overrides: Partial<VerificationProfileDefinition> = {},
): VerificationProfileDefinition {
  return {
    interface: "cli_api",
    doctor: [],
    setup: [],
    journeys: [
      {
        key: "smoke",
        title: "冒煙測試",
        representative: true,
        steps: [
          {
            key: "version",
            title: "顯示版本",
            operation: { kind: "command", program: "tool", args: ["--version"] },
            assertions: [{ kind: "exit_code", equals: 0 }],
          },
        ],
      },
      {
        key: "export_flow",
        title: "匯出報表",
        representative: false,
        steps: [
          {
            key: "export",
            title: "匯出",
            operation: { kind: "http", method: "POST", url: "http://127.0.0.1:8080/export", headers: [] },
            assertions: [
              { kind: "status", equals: 200 },
              { kind: "json_equals", pointer: "/ok", value: true },
            ],
          },
        ],
      },
    ],
    teardown: [],
    evidence: { output_max_chars: 4000, redact: [] },
    budget: { max_operations: 200 },
    feature_map: [
      { key: "cli", title: "命令列", criterion: "可以顯示版本", journeys: ["smoke"] },
      { key: "export", title: "匯出", criterion: "可以匯出報表", journeys: ["export_flow"] },
      { key: "import", title: "匯入", criterion: "可以匯入資料", journeys: [] },
    ],
    uncovered: [{ area: "圖形介面", reason: "CLI／API Adapter 不涵蓋" }],
    ...overrides,
  };
}

/** 測試用的 Profile 版本：第 2 版。 */
export function verificationProfileVersion(overrides: Partial<VerificationProfileVersion> = {}): VerificationProfileVersion {
  return {
    id: "vprof_2",
    workspace_id: "ws_1",
    version: 2,
    definition: verificationProfileDefinition(),
    content_hash: "hash_2",
    published_by: "a1b2c3d4e5f6a7b8",
    created_at: CHECKED_AT,
    ...overrides,
  };
}

/** 測試用的步驟紀錄：冒煙測試中通過的「顯示版本」。 */
export function verificationStepRecord(overrides: Partial<VerificationStepRecord> = {}): VerificationStepRecord {
  return {
    phase: "journey",
    journey: "smoke",
    key: "version",
    title: "顯示版本",
    operation: "執行「tool --version」",
    verdict: "passed",
    reason_code: "assertions_passed",
    engine_code: "exit:0",
    assertions: [{ assertion: { kind: "exit_code", equals: 0 }, passed: true, actual: "0" }],
    output: "tool 1.2.3",
    attempts: 1,
    ...overrides,
  };
}

/** 測試用的 Evidence Bundle：Task task_1 契約第 3 版、Profile 第 2 版，只執行代表旅程並通過。 */
export function evidenceBundle(overrides: Partial<EvidenceBundle> = {}): EvidenceBundle {
  return {
    id: "vbun_1",
    workspace_id: "ws_1",
    request_id: "vreq_1",
    task_id: "task_1",
    run_id: "run_1",
    generation: 1,
    requirement_version: 3,
    profile_version_id: "vprof_2",
    profile_version: 2,
    candidate_revision: "abc1234",
    build: "tool 1.2.3",
    subject_dir: "C:\\work\\notes",
    journeys: [{ key: "smoke", title: "冒煙測試", representative: true, verdict: "passed", reason_code: "all_steps_passed" }],
    steps: [verificationStepRecord()],
    verdict: "passed",
    reason_code: "all_journeys_passed",
    covered_features: ["cli"],
    uncovered: [
      { feature: "import", area: "匯入", reason: "Feature Map 中沒有驗證這個功能的旅程" },
      { area: "圖形介面", reason: "CLI／API Adapter 不涵蓋" },
      { feature: "export", area: "匯出", reason: "這次驗證沒有執行對應的旅程" },
    ],
    cleanup: { state_dir: "C:\\data\\verification\\vreq_1", state_dir_removed: true, problems: [] },
    artifacts: [],
    started_at: CHECKED_AT,
    finished_at: CHECKED_AT + 5_000,
    ...overrides,
  };
}

/** 測試用的驗證要求：使用者剛排入、尚未執行。 */
export function verificationRequest(overrides: Partial<VerificationRequest> = {}): VerificationRequest {
  return {
    id: "vreq_2",
    workspace_id: "ws_1",
    task_id: null,
    profile_version_id: "vprof_2",
    profile_version: 2,
    journeys: ["smoke", "export_flow"],
    requested_by: "desktop",
    status: "queued",
    bundle_id: null,
    created_at: CHECKED_AT,
    ...overrides,
  };
}

/** 測試用的驗證概況：第 2 版已由 vbun_1 證明可用。 */
export function verificationOverview(overrides: Partial<VerificationOverview> = {}): VerificationOverview {
  return {
    workspace_id: "ws_1",
    profile: verificationProfileVersion(),
    proven_by: "vbun_1",
    uncovered: [
      { feature: "import", area: "匯入", reason: "Feature Map 中沒有驗證這個功能的旅程" },
      { area: "圖形介面", reason: "CLI／API Adapter 不涵蓋" },
    ],
    requests: [],
    bundles: [evidenceBundle()],
    ...overrides,
  };
}

/**
 * 測試用的 web Profile 內容：代表旅程「管理訂單」扮演店家管理員（admin）、以櫃台人員（clerk）登入；
 * 「瀏覽商品」扮演訪客（shopper）、不登入。密碼只以秘密參照出現。
 */
export function webVerificationProfileDefinition(
  overrides: Partial<VerificationProfileDefinition> = {},
): VerificationProfileDefinition {
  return {
    interface: "web",
    doctor: [],
    setup: [],
    journeys: [
      {
        key: "orders",
        title: "管理訂單",
        representative: true,
        persona: "admin",
        identity: "clerk",
        steps: [
          {
            key: "open_orders",
            title: "開啟訂單頁",
            operation: { kind: "browser", action: { kind: "goto", url: "http://127.0.0.1:3000/orders" } },
            assertions: [
              { kind: "url_contains", text: "/orders" },
              { kind: "judge", expectation: "畫面列出今天的訂單" },
            ],
          },
        ],
      },
      {
        key: "browse",
        title: "瀏覽商品",
        representative: false,
        persona: "shopper",
        steps: [
          {
            key: "list",
            title: "商品列表",
            operation: { kind: "browser", action: { kind: "goto", url: "http://127.0.0.1:3000/" } },
            assertions: [{ kind: "visible", selector: "#products" }],
          },
        ],
      },
    ],
    teardown: [],
    evidence: { output_max_chars: 4000, redact: [], screenshots: "every_step", trace: true, video: true, mask: ["#card"] },
    budget: { max_operations: 200, max_model_calls: 20 },
    feature_map: [
      { key: "orders", title: "訂單管理", criterion: "可以看到今天的訂單", journeys: ["orders"] },
      { key: "catalog", title: "商品瀏覽", criterion: "訪客可以看到商品", journeys: ["browse"] },
    ],
    uncovered: [],
    secrets: [{ key: "clerk_password", reference: "env:SHOP_CLERK_PASSWORD" }],
    personas: [
      { key: "admin", title: "店家管理員", description: "管理商品與訂單" },
      { key: "shopper", title: "訪客", description: "沒有帳號的顧客" },
    ],
    identities: [
      {
        key: "clerk",
        title: "櫃台人員",
        login: [
          {
            key: "goto_login",
            title: "開啟登入頁",
            operation: { kind: "browser", action: { kind: "goto", url: "http://127.0.0.1:3000/login" } },
            assertions: [],
          },
          {
            key: "fill_password",
            title: "輸入密碼",
            operation: { kind: "browser", action: { kind: "fill", selector: "#password", text: "{{secret:clerk_password}}" } },
            assertions: [],
          },
        ],
        check: {
          key: "whoami",
          title: "確認登入身分",
          operation: { kind: "browser", action: { kind: "goto", url: "http://127.0.0.1:3000/me" } },
          assertions: [{ kind: "contains", text: "clerk" }],
        },
      },
    ],
    ...overrides,
  };
}

/**
 * 測試用的 web Evidence Bundle：「管理訂單」的語意判斷遇到模型限流（證據不足）；「瀏覽商品」通過。
 * Artifacts：已保存的截圖、因輸入過秘密而沒有保存的 trace、已保存的錄影。
 */
export function webEvidenceBundle(overrides: Partial<EvidenceBundle> = {}): EvidenceBundle {
  return evidenceBundle({
    id: "vbun_web",
    build: null,
    journeys: [
      {
        key: "orders",
        title: "管理訂單",
        representative: true,
        persona: "admin",
        identity: "clerk",
        verdict: "inconclusive",
        reason_code: "judge_rate_limited",
      },
      {
        key: "browse",
        title: "瀏覽商品",
        representative: false,
        persona: "shopper",
        verdict: "passed",
        reason_code: "all_steps_passed",
      },
    ],
    steps: [
      verificationStepRecord({
        phase: "login",
        journey: "orders",
        key: "fill_password",
        title: "輸入密碼",
        operation: "瀏覽器在「#password」輸入「[已遮蔽]」",
        engine_code: undefined,
        assertions: [],
        output: "",
      }),
      verificationStepRecord({
        phase: "identity",
        journey: "orders",
        key: "whoami",
        title: "確認登入身分",
        operation: "瀏覽器前往 http://127.0.0.1:3000/me",
        engine_code: undefined,
        assertions: [{ assertion: { kind: "contains", text: "clerk" }, passed: true, actual: "包含" }],
        output: "",
      }),
      verificationStepRecord({
        journey: "orders",
        key: "open_orders",
        title: "開啟訂單頁",
        operation: "瀏覽器前往 http://127.0.0.1:3000/orders",
        verdict: "inconclusive",
        reason_code: "judge_rate_limited",
        engine_code: "model:rate_limited",
        assertions: [
          { assertion: { kind: "url_contains", text: "/orders" }, passed: true, actual: "http://127.0.0.1:3000/orders" },
          { assertion: { kind: "judge", expectation: "畫面列出今天的訂單" }, passed: false, actual: "模型限流，無法判定" },
        ],
        output: "",
      }),
      verificationStepRecord({
        journey: "browse",
        key: "list",
        title: "商品列表",
        operation: "瀏覽器前往 http://127.0.0.1:3000/",
        engine_code: undefined,
        assertions: [
          { assertion: { kind: "visible", selector: "#products" }, passed: true, actual: "可見" },
          { assertion: { kind: "judge", expectation: "畫面列出商品" }, passed: true, actual: "畫面上有三項商品與價格" },
        ],
        output: "",
      }),
    ],
    verdict: "inconclusive",
    reason_code: "judge_rate_limited",
    covered_features: ["catalog"],
    uncovered: [],
    artifacts: [
      {
        kind: "screenshot",
        journey: "orders",
        step: "open_orders",
        name: "open_orders.png",
        storage_path: "ws_1/blob_shot",
        sha256: "aa11",
        size_bytes: 2048,
      },
      { kind: "trace", journey: "orders", name: "orders-trace.zip", withheld: "secret_input_recorded" },
      {
        kind: "video",
        journey: "browse",
        name: "browse.webm",
        storage_path: "ws_1/blob_video",
        sha256: "bb22",
        size_bytes: 4096,
      },
    ],
    ...overrides,
  });
}

/** 測試用的一次嘗試：第 1 次、以 prof_a 執行並完成，沒有承接。 */
export function verificationAttempt(overrides: Partial<VerificationAttempt> = {}): VerificationAttempt {
  return {
    attempt: 1,
    status: "completed",
    judge_profile_id: "prof_a",
    end_reason: null,
    journeys: ["smoke"],
    steps: [{ phase: "journey", journey: "smoke", key: "version", verdict: "passed", reason_code: "assertions_passed" }],
    takeover: null,
    started_at: CHECKED_AT,
    ended_at: CHECKED_AT + 5_000,
    ...overrides,
  };
}

/**
 * 測試用的交接 web Evidence Bundle：第 1 次嘗試（prof_a）完成「瀏覽商品」後，在「管理訂單」送出外部操作「退款」時
 * 語意判斷的帳號限流而交接；第 2 次嘗試（prof_b）停止前一次的服務與 Session、沿用「瀏覽商品」，
 * 結果未知的「退款」不重送，「管理訂單」因此證據不足。
 */
export function handoffEvidenceBundle(overrides: Partial<EvidenceBundle> = {}): EvidenceBundle {
  return webEvidenceBundle({
    id: "vbun_handoff",
    journeys: [
      {
        key: "orders",
        title: "管理訂單",
        representative: true,
        persona: "admin",
        identity: "clerk",
        verdict: "inconclusive",
        reason_code: "external_outcome_unknown",
      },
      {
        key: "browse",
        title: "瀏覽商品",
        representative: false,
        persona: "shopper",
        verdict: "passed",
        reason_code: "all_steps_passed",
      },
    ],
    steps: [
      verificationStepRecord({ journey: "browse", key: "list", title: "商品列表", engine_code: undefined, output: "" }),
      verificationStepRecord({
        journey: "orders",
        key: "refund",
        title: "退款",
        operation: "瀏覽器點擊「#refund」",
        verdict: "inconclusive",
        reason_code: "external_outcome_unknown",
        engine_code: undefined,
        assertions: [],
        output: "",
      }),
    ],
    verdict: "inconclusive",
    reason_code: "external_outcome_unknown",
    artifacts: [],
    attempts: [
      verificationAttempt({
        status: "handed_off",
        end_reason: "judge_rate_limited",
        journeys: ["browse"],
        steps: [
          { phase: "journey", journey: "browse", key: "list", verdict: "passed", reason_code: "assertions_passed" },
          { phase: "journey", journey: "orders", key: "open_orders", verdict: "passed", reason_code: "assertions_passed" },
        ],
      }),
      verificationAttempt({
        attempt: 2,
        judge_profile_id: "prof_b",
        journeys: ["orders"],
        steps: [
          { phase: "journey", journey: "orders", key: "refund", verdict: "inconclusive", reason_code: "external_outcome_unknown" },
        ],
        takeover: {
          from_attempt: 1,
          stopped_services: ["svc-1"],
          closed_sessions: ["b2"],
          problems: ["瀏覽器 Session b3 無法確認已關閉"],
          reused_journeys: ["browse"],
          not_resent: [{ journey: "orders", step: "refund", reason: "external_outcome_unknown", attempt: 1 }],
        },
        started_at: CHECKED_AT + 6_000,
        ended_at: CHECKED_AT + 9_000,
      }),
    ],
    ...overrides,
  });
}
