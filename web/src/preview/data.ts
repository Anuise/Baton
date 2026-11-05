/**
 * 開發用預覽的資料：畫面初次載入時各個讀取方法回傳的內容（只在 web/preview.html 使用，不進正式建置）。
 * normal 是日常份量；long 在 longData.ts，刻意製造長文字與大量資料的壓力。
 */
import {
  CHECKED_AT,
  authorityTable,
  availableUpdateStatus,
  backupStatus,
  budgetView,
  controllerCliRun,
  decisionView,
  explorationOverview,
  integrationEntry,
  lifecycleStatus,
  metricsResponse,
  profile,
  recoveryStatus,
  reviewList,
  runView,
  schedulingOverview,
  accountScheduling,
  settingsVersionSummary,
  setupStatus,
  supportedPlatform,
  timelineEvent,
  understandingView,
  verificationOverview,
  verificationRecord,
  workdirView,
  workflowList,
  workspaceLifecycle,
} from "../fixtures";
import type {
  AccountProfile,
  ArtifactList,
  AuthorityTable,
  AuthorizationView,
  BackupStatus,
  ControllerCliView,
  DecisionList,
  DecisionRequestView,
  ExplorationOverview,
  GuardList,
  GuardView,
  HealthResponse,
  KnowledgeEntryView,
  KnowledgeList,
  LifecycleStatus,
  MetricsResponse,
  ModelCatalog,
  PendingDecisions,
  PlatformResponse,
  ProfileList,
  RecoveryStatusView,
  ReviewList,
  SchedulingOverview,
  SetupStatus,
  SharedSettingsStatus,
  TaskBoard,
  TaskContract,
  TaskStatus,
  TaskView,
  TimelineResponse,
  UnderstandingView,
  UpdateStatusView,
  VerificationOverview,
  WorkflowList,
  Workspace,
} from "../runtimeClient";

/** 各個讀取方法的回傳內容；Workspace 相關的內容不分 Workspace，一律回傳同一份。 */
export interface PreviewData {
  health: HealthResponse;
  platform: PlatformResponse;
  workspaces: Workspace[];
  models: ModelCatalog;
  authorization: AuthorizationView;
  board: TaskBoard;
  decisions: DecisionList;
  authority: AuthorityTable;
  pending: PendingDecisions;
  artifacts: ArtifactList;
  knowledge: KnowledgeList;
  guards: GuardList;
  scheduling: SchedulingOverview;
  profiles: ProfileList;
  setup: SetupStatus;
  lifecycle: LifecycleStatus;
  sharedSettings: SharedSettingsStatus;
  backups: BackupStatus;
  updates: UpdateStatusView;
  controller: ControllerCliView;
  recovery: RecoveryStatusView;
  timeline: TimelineResponse;
  metrics: MetricsResponse;
  understanding: UnderstandingView;
  workflows: WorkflowList;
  reviews: ReviewList;
  verification: VerificationOverview;
  exploration: ExplorationOverview;
}

export const PREVIEW_WORKSPACE_ID = "ws_1";

export function workspaceOf(overrides: Partial<Workspace> = {}): Workspace {
  return {
    id: PREVIEW_WORKSPACE_ID,
    name: "筆記",
    path: "C:\\work\\notes",
    status: "active",
    version: 1,
    created_at: CHECKED_AT,
    archived_at: null,
    model_overrides: { controller: null, worker: null, reviewer: null },
    ...overrides,
  };
}

/** 六項契約都已填寫的 Task 契約；`texts` 依欄位覆寫內容。 */
export function contractOf(taskId: string, texts: Partial<Record<"objective" | "inputs" | "scope" | "deliverables" | "done_criteria" | "blocker_handling", string>> = {}): TaskContract {
  const filled = (text: string) => ({ status: "filled" as const, text });
  return {
    task_id: taskId,
    version: 1,
    objective: filled(texts.objective ?? "整理文件目錄，讓新成員能找到各模組的說明"),
    inputs: filled(texts.inputs ?? "docs/ 底下現有的文件"),
    scope: filled(texts.scope ?? "只動 docs/，不改程式碼"),
    deliverables: filled(texts.deliverables ?? "整理後的 docs/ 與索引頁"),
    done_criteria: filled(texts.done_criteria ?? "每個模組都有對應的說明頁，索引頁的連結都有效"),
    blocker_handling: filled(texts.blocker_handling ?? "遇到無法判斷歸屬的文件時送裁定"),
    required_capabilities: ["read_files", "write_files"],
    verification_checks: [],
    note: null,
    created_at: CHECKED_AT,
  };
}

export function taskOf(id: string, title: string, status: TaskStatus, overrides: Partial<TaskView> = {}): TaskView {
  return {
    id,
    workspace_id: PREVIEW_WORKSPACE_ID,
    goal_id: "goal_1",
    title,
    kind: "development",
    status,
    version: 1,
    contract_version: 1,
    created_at: CHECKED_AT,
    updated_at: CHECKED_AT,
    contract: contractOf(id),
    dependencies: [],
    unfinished_dependencies: [],
    missing: [],
    ...overrides,
  };
}

export function authorizationOf(workspace: Workspace, overrides: Partial<AuthorizationView> = {}): AuthorizationView {
  const revision = {
    workspace_id: workspace.id,
    revision: 1,
    grants: [
      { capability: "read_files" as const, targets: [workspace.path], restrictions: [] },
      { capability: "write_files" as const, targets: [workspace.path], restrictions: ["不得修改 .git"] },
    ],
    source: { kind: "template" as const, template_id: "dev", base_revision: null, confirmed_by: "desktop" },
    confirmed_at: CHECKED_AT,
  };
  return {
    workspace_id: workspace.id,
    status: "confirmed",
    effective: revision,
    revisions: [revision],
    templates: [
      { id: "dev", name: "一般開發", description: "讀寫專案檔案、執行建置與測試", grants: revision.grants },
    ],
    capabilities: [
      { capability: "read_files", label: "讀取檔案", high_risk: false },
      { capability: "write_files", label: "寫入檔案", high_risk: false },
      { capability: "run_build_test", label: "執行建置與測試", high_risk: false },
      { capability: "deploy", label: "部署", high_risk: true },
    ],
    risk_disclosure: "高風險能力可能影響專案以外的系統，確認前請先閱讀說明。",
    legacy_runs: [],
    ...overrides,
  };
}

export function knowledgeOf(id: string, title: string, overrides: Partial<KnowledgeEntryView> = {}): KnowledgeEntryView {
  return {
    id,
    kind: "decision",
    scope: { kind: "workspace", workspace_id: PREVIEW_WORKSPACE_ID, goal_id: null, task_id: null },
    status: "verified",
    source: { kind: "user" },
    valid_from: CHECKED_AT,
    valid_until: null,
    version: 1,
    revision: 1,
    supersedes: null,
    superseded_by: null,
    status_reason: null,
    content: { title, body: "所有前端套件一律以 pnpm 安裝。", tags: ["前端"], evidence: [], lesson: null },
    created_at: CHECKED_AT,
    updated_at: CHECKED_AT,
    effective: true,
    ...overrides,
  };
}

export function guardOf(id: string, lesson: KnowledgeEntryView, overrides: Partial<GuardView> = {}): GuardView {
  return {
    id,
    lesson_id: lesson.id,
    rule: { kind: "preflight", field: "scope", must_mention: "備份" },
    status: "enabled",
    revision: 1,
    created_at: CHECKED_AT,
    updated_at: CHECKED_AT,
    lesson: { id: lesson.id, title: lesson.content.title, status: lesson.status, effective: lesson.effective, scope: lesson.scope },
    active: true,
    inactive_reason: null,
    description: "契約的範圍必須提到「備份」",
    prevented_count: 1,
    last_prevented_at: CHECKED_AT,
    reported_count: 0,
    ...overrides,
  };
}

export function decisionListOf(decisions: DecisionRequestView[]): DecisionList {
  const pending = decisions.filter((d) => d.status === "pending");
  return {
    pending_count: pending.length,
    overview: {
      pending: pending.length,
      actionable: pending.filter((d) => !d.deferred).length,
      deferred: pending.filter((d) => d.deferred).length,
      overdue: pending.filter((d) => d.overdue).length,
      high_urgency: pending.filter((d) => d.urgency === "high").length,
      answered: decisions.filter((d) => d.status === "answered").length,
      superseded: decisions.filter((d) => d.status === "superseded").length,
      merged: decisions.filter((d) => d.status === "merged").length,
    },
    decisions,
  };
}

/** 首次啟動精靈全畫面：沒有任何可派工的帳號。 */
export function asSetup(data: PreviewData, profiles: AccountProfile[]): PreviewData {
  return {
    ...data,
    setup: { ...data.setup, profiles, ready: false, background_notice: { ...data.setup.background_notice, acknowledged_at: null } },
    profiles: { ...data.profiles, profiles },
  };
}

/** 日常份量的資料。 */
export function normalData(): PreviewData {
  const workspace = workspaceOf();
  const tasks = [
    taskOf("task_1", "整理文件", "running"),
    taskOf("task_2", "修正匯出錯誤", "verifying"),
    taskOf("task_3", "新增匯入功能", "pending", {
      kind: "research",
      missing: [{ code: "field_missing", field: "done_criteria", message: "還沒有填寫完成條件" }],
    }),
    taskOf("task_4", "更新相依套件", "ready"),
    taskOf("task_5", "撰寫使用手冊", "waiting_decision", { kind: "document" }),
    taskOf("task_6", "清理舊分支", "completed"),
  ];
  const decisions = [decisionView(), decisionView({ id: "dec_2", question: "匯出檔名要加上日期嗎？", urgency: "high", task_id: "task_5", task_title: "撰寫使用手冊" })];
  return {
    health: { status: "ok", product: "Baton", version: "0.1.0", boot_count: 3, started_at: CHECKED_AT, pid: 4242, guardian: { state: "active", message: null } },
    platform: supportedPlatform,
    workspaces: [workspace, workspaceOf({ id: "ws_2", name: "文件", path: "C:\\work\\docs" })],
    models: {
      models: [
        { id: "claude-opus-5-5", label: "Opus 5.5" },
        { id: "claude-sonnet-5-5", label: "Sonnet 5.5" },
      ],
      updated_at: null,
    },
    authorization: authorizationOf(workspace),
    board: {
      workspace_id: workspace.id,
      goals: [{ id: "goal_1", workspace_id: workspace.id, title: "第一版發布", version: 1, created_at: CHECKED_AT }],
      tasks,
      runs: [
        runView({ latest_progress: { summary: "已整理 8 個檔案中的 5 個", reported_at: CHECKED_AT } }),
        runView({ id: "run_2", task_id: "task_2", status: "succeeded" }),
      ],
      capabilities: authorizationOf(workspace).capabilities,
      verifications: [verificationRecord({ task_id: "task_2", run_id: "run_2" })],
      authorization_revision: 1,
      workdirs: [workdirView(), workdirView({ task_id: "task_2", path: "C:\\work\\notes\\.baton\\worktrees\\task_2", branch: "baton/task_2", run_ids: ["run_2"] })],
      integrations: [integrationEntry({ task_id: "task_2", run_id: "run_2", branch: "baton/task_2" })],
      budgets: tasks.map((task) => budgetView({ task_id: task.id })),
      handoffs: [],
    },
    decisions: decisionListOf(decisions),
    authority: authorityTable(),
    pending: { pending_count: 2, by_workspace: [{ workspace_id: workspace.id, pending_count: 2 }] },
    artifacts: {
      artifacts: [
        {
          id: "art_1",
          workspace_id: workspace.id,
          task_id: "task_2",
          name: "report.md",
          created_at: CHECKED_AT,
          versions: [
            {
              version: 1,
              description: "匯出錯誤的分析報告",
              size_bytes: 2048,
              sha256: "0123456789abcdef".repeat(4),
              storage_path: "ws_1/art_1/v1",
              produced_by_run_id: "run_2",
              produced_by_generation: 1,
              created_at: CHECKED_AT,
              file_status: "ok",
            },
          ],
        },
      ],
      unregistered_files: [],
    },
    knowledge: { search_mode: "full_text", entries: [knowledgeOf("know_1", "一律用 pnpm")] },
    guards: { guards: [guardOf("grd_1", knowledgeOf("know_2", "部署前先備份", { kind: "lesson" }))] },
    scheduling: schedulingOverview({ accounts: [accountScheduling({ running: 1 })] }),
    profiles: { profiles: [profile(), profile({ id: "prof_2", name: "備用帳號", expected_account: "backup@example.com" })], limit: 8 },
    setup: setupStatus({ background_notice: { text: "關閉視窗後，Baton 會在背景繼續執行已派出的工作。", acknowledged_at: CHECKED_AT } }),
    lifecycle: lifecycleStatus({ workspaces: [workspaceLifecycle(), workspaceLifecycle({ workspace_id: "ws_2", name: "文件" })] }),
    sharedSettings: { current: settingsVersionSummary(), versions: [settingsVersionSummary()] },
    backups: backupStatus(),
    updates: availableUpdateStatus(),
    controller: { workspace_id: workspace.id, run: controllerCliRun(), playbook_version: "3" },
    recovery: recoveryStatus(),
    timeline: {
      workspace_id: workspace.id,
      task_id: null,
      events: [
        timelineEvent(),
        timelineEvent({ seq: 2, occurred_at: CHECKED_AT + 60_000, category: "decision", aggregate_type: "decision", aggregate_id: "dec_1", event_type: "decision_requested", run_id: null }),
        timelineEvent({ seq: 3, occurred_at: CHECKED_AT + 120_000, category: "verification", aggregate_type: "task", aggregate_id: "task_2", event_type: "verification_passed", task_ids: ["task_2"], run_id: "run_2" }),
      ],
    },
    metrics: metricsResponse(),
    understanding: understandingView(),
    workflows: workflowList(),
    reviews: reviewList(),
    verification: verificationOverview(),
    exploration: explorationOverview(),
  };
}

/** normal 首次啟動精靈：已建立一個帳號但還沒登入。 */
export function normalSetupProfiles(): AccountProfile[] {
  return [
    profile({
      verification: null,
      dispatchable: false,
      blockers: [{ code: "not_verified", message: "尚未核對登入狀態" }],
    }),
  ];
}
