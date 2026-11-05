/**
 * long 資料集：刻意製造長文字與大量資料的壓力，用來檢查排版在文字很多、很長時的樣子
 * （只在 web/preview.html 使用，不進正式建置）。
 */
import {
  CHECKED_AT,
  accountRun,
  accountScheduling,
  authorityTable,
  availableUpdateStatus,
  backupSummary,
  budgetView,
  controllerCliRun,
  decisionView,
  drillReport,
  explorationFinding,
  explorationOverview,
  failingReviewView,
  handoff,
  integrationEntry,
  lifecycleStatus,
  metricsResponse,
  missingClaude,
  okTool,
  profile,
  quotaAssessment,
  recoveryEvent,
  recoveryStatus,
  reviewList,
  runView,
  schedulingOverview,
  settingsVersionSummary,
  setupStatus,
  timelineEvent,
  understandingItem,
  understandingView,
  verificationOverview,
  verificationRecord,
  workdirView,
  workflowList,
  workflowView,
  workspaceLifecycle,
} from "../fixtures";
import type { AccountProfile, DecisionRequestView, TaskStatus, TaskView, TimelineCategory } from "../runtimeClient";
import {
  asSetup,
  authorizationOf,
  contractOf,
  decisionListOf,
  guardOf,
  knowledgeOf,
  taskOf,
  workspaceOf,
  type PreviewData,
} from "./data";

/** long 資料集獨有的字串；建置產物以它確認沒有夾帶預覽資料。 */
export const LONG_MARKER = "長文字壓力測試專用標記";

/** 100 字以上的中文 Task 標題。 */
export const LONG_TASK_TITLE =
  `${LONG_MARKER}：重新整理跨 Workspace 的帳號額度觀測與交接流程，確保在五小時與七日視窗同時接近上限、` +
  "而且使用者正離線處理多項高風險權限裁定時，Controller 仍能依決策權限表把尚未完成的 Task 安全交接給其他可派工的帳號，並保留完整的檢查點與證據鏈供事後稽核";

/** 100 字以上的中文裁定標題。 */
export const LONG_DECISION_QUESTION =
  "整合佇列中第 17 個項目的合併提交與目標分支的最新提交衝突，而且衝突範圍同時涵蓋資料庫遷移腳本、前端狀態列元件與 Runtime 的派工排程邏輯，" +
  "要直接以平台分支的版本覆寫、改為人工逐檔合併，還是先撤回這次整合並重新派工給原本的 Worker 處理？";

export const LONG_ID = "task_7b5deb026d9841b0cced7314aa0f9e8d7c6b5a4f3e2d1c0b9a8f7e6d5c4b3a29f8e7d6c5b4a3";
export const LONG_PATH =
  "C:\\Users\\Administrator.CORPORATE-DOMAIN\\AppData\\Local\\Baton\\workspaces\\ws_75b131a47faed9ffc3cd741a\\worktrees\\task_7b5deb026d9841b0cced7314\\crates\\baton-harness\\tests\\ui_e2e\\fixtures\\deeply_nested_directory_structure\\report-final-revision-2026-10-28.md";
export const LONG_SHA256 = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
export const LONG_URL =
  "https://login.example-enterprise-identity-provider.com/oauth2/v2.0/authorize?client_id=8f14e45f-ceea-467a-9a36-dedd4bea2543&redirect_uri=http%3A%2F%2F127.0.0.1%3A47821%2Fcallback&scope=openid%20profile%20offline_access&state=Zm9vYmFyYmF6cXV4cXV1eGNvcmdlZ3JhdWx0";
export const LONG_ERROR =
  "Runtime 拒絕連線（storage_unwritable）：無法寫入控制資料庫 C:\\Users\\Administrator.CORPORATE-DOMAIN\\AppData\\Local\\Baton\\data\\baton.sqlite3-wal，" +
  "SQLite 回報 SQLITE_IOERR_WRITE（disk I/O error, extended code 778），可能是防毒軟體鎖定了檔案、磁碟已滿或使用者帳號沒有寫入權限；" +
  "Runtime 已停止新派工，已在執行的 Run 會繼續到結束但無法保存檢查點，請釋放磁碟空間或排除鎖定後按「重新檢查」。";
export const LONG_ACCOUNT = "very.long.account.name.for.layout.stress.testing+baton-worker-07@subdomain.example-enterprise-corporation.com";
export const MIXED_SENTENCE =
  "在 Windows 11 26H2 Pro x64（build 26300.1742）上以 Claude Code 2.1.307 執行 cargo test -p baton-store --release 時，" +
  "第 3 次 Run 於 14:32:05 回報 exit code 101（LNK1104: cannot open file 'msvcrt.lib'），已用 87.5% 的五小時額度與 1,234,567 tokens。";

/** 多段落的說明文字。 */
export const LONG_PARAGRAPHS = [
  `${MIXED_SENTENCE}這個 Task 的目標是讓整個派工流程在極端情況下仍然可預期：每一次交接都必須先保存檢查點，再停止舊程序，最後由接手的 Worker 確認已完成、未完成與副作用不明的項目。`,
  `輸入包含 ${LONG_PATH} 底下的全部測試資料、SHA-256 為 ${LONG_SHA256} 的基準報告，以及登入網址 ${LONG_URL}。`,
  "範圍限制：不修改 Rust 的資料庫遷移，不改 CI；遇到需要修改公開 API 的情況，先回報設計偏離並等待裁定。所有變更都要附上測試，並在 AI 決策紀錄追加條目。",
  `完成條件：1. 所有帳號的額度觀測都有來源標記；2. 交接紀錄可以從時間線回溯；3. 錯誤訊息（例如「${LONG_ERROR.slice(0, 60)}…」）都能完整顯示，不被截斷也不撐破版面。`,
].join("\n\n");

const LONG_NAME = (n: number) => `${LONG_MARKER}的 Workspace 第 ${n} 號：跨部門共用的企業內部知識庫與自動化測試平台（含舊版遷移資料）`;

const STATUSES: TaskStatus[] = ["pending", "ready", "running", "waiting_decision", "waiting_resource", "blocked", "verifying", "completed", "cancelled"];

function longTasks(): TaskView[] {
  return Array.from({ length: 45 }, (_, i) => {
    const id = i === 0 ? LONG_ID : `task_${String(i).padStart(3, "0")}_${LONG_SHA256.slice(0, 24)}`;
    const status = STATUSES[i % STATUSES.length];
    const title = i % 3 === 0 ? LONG_TASK_TITLE : i % 3 === 1 ? `${MIXED_SENTENCE.slice(0, 70)}（第 ${i} 項）` : `修正 ${LONG_PATH.slice(-60)} 的格式`;
    return taskOf(id, title, status, {
      kind: i % 4 === 0 ? "research" : i % 4 === 1 ? "document" : "development",
      goal_id: i % 2 === 0 ? "goal_long" : "goal_1",
      contract_version: 12 + i,
      contract: contractOf(id, {
        objective: LONG_PARAGRAPHS,
        inputs: `${LONG_PATH}\n${LONG_URL}\n${LONG_SHA256}`,
        scope: LONG_PARAGRAPHS,
        deliverables: `${MIXED_SENTENCE}\n\n${LONG_PATH}`,
        done_criteria: LONG_PARAGRAPHS,
        blocker_handling: LONG_ERROR,
      }),
      dependencies: i > 2 ? [LONG_ID] : [],
      unfinished_dependencies: i > 2 && status === "pending" ? [LONG_ID] : [],
      missing:
        status === "pending"
          ? [
              { code: "field_missing", field: "done_criteria", message: `還沒有填寫完成條件。${MIXED_SENTENCE}` },
              { code: "capability_not_granted", field: null, message: `授權沒有包含「部署」能力，目標 ${LONG_URL}` },
              { code: "reference_invalid", field: "inputs", message: `引用的 Task ${LONG_ID} 契約第 3 版已失效` },
            ]
          : [],
    });
  });
}

function longDecisions(): DecisionRequestView[] {
  return Array.from({ length: 30 }, (_, i) =>
    decisionView({
      id: `dec_${i}_${LONG_SHA256.slice(0, 32)}`,
      task_id: LONG_ID,
      task_title: LONG_TASK_TITLE,
      kind: i % 3 === 0 ? "permission" : i % 3 === 1 ? "authority" : "agent",
      question: i % 2 === 0 ? LONG_DECISION_QUESTION : `${MIXED_SENTENCE}要繼續嗎？`,
      reason: LONG_PARAGRAPHS,
      urgency: i % 4 === 0 ? "high" : i % 4 === 1 ? "low" : "normal",
      overdue: i % 5 === 0,
      deadline_at: CHECKED_AT + 3_600_000,
      options: [
        { id: "overwrite", label: "以平台分支的版本覆寫目標分支上全部衝突的檔案", tradeoff: LONG_PARAGRAPHS.slice(0, 200) },
        { id: "manual", label: "人工逐檔合併", tradeoff: `需要人工處理 ${LONG_PATH}` },
        { id: "revert", label: "撤回整合並重新派工", tradeoff: null },
        { id: "very_long_option_identifier_without_any_spaces_for_layout_testing", label: LONG_SHA256, tradeoff: LONG_URL },
      ],
      recommendation: "manual",
      evidence: [LONG_PATH, LONG_URL, `SHA-256 ${LONG_SHA256}`, LONG_ERROR, MIXED_SENTENCE],
      blocking: Array.from({ length: 6 }, (_, j) => ({ task_id: `${LONG_ID}_${j}`, task_title: LONG_TASK_TITLE, task_status: "waiting_decision" as const })),
      permission:
        i % 3 === 0
          ? {
              tool_name: "mcp__very_long_server_name_for_testing__execute_powershell_command_with_elevated_privileges",
              input: { command: `Remove-Item -Recurse -Force "${LONG_PATH}"`, url: LONG_URL },
              capability: "production_data",
              capability_label: "正式環境資料",
              summary: `要刪除 ${LONG_PATH}`,
              binding: null,
              prior_use: null,
              rejected_approval: null,
              approval_use: null,
            }
          : null,
      authority:
        i % 3 === 1 ? { matter: "change_acceptance", matter_label: "修改完成條件或驗收標準", summary: LONG_PARAGRAPHS.slice(0, 160) } : null,
    }),
  );
}

function longProfiles(dispatchable: boolean): AccountProfile[] {
  return Array.from({ length: 8 }, (_, i) =>
    profile({
      id: `prof_${i}_${LONG_SHA256.slice(0, 20)}`,
      name: `${LONG_MARKER}的帳號 ${i}：研發部門共用的高額度訂閱（請勿用於個人專案）`,
      config_dir: `${LONG_PATH}\\profiles\\prof_${i}`,
      config_dir_origin: i % 2 === 0 ? "managed" : "external",
      expected_account: LONG_ACCOUNT,
      verification:
        dispatchable || i % 2 === 0
          ? {
              checked_at: CHECKED_AT,
              logged_in: dispatchable,
              source: dispatchable ? "native_login" : "api_key",
              source_detail: dispatchable ? null : `ANTHROPIC_API_KEY 由 ${LONG_PATH} 設定`,
              account: LONG_ACCOUNT,
              config_dir_matches: dispatchable,
              cli_version: "2.1.307 (Claude Code)",
            }
          : null,
      dispatchable: dispatchable && i % 3 !== 2,
      blockers:
        dispatchable && i % 3 !== 2
          ? []
          : [
              { code: "auth_source_not_native", message: `認證來源不是原生登入：${LONG_ERROR.slice(0, 120)}` },
              { code: "account_mismatch", message: `登入的帳號不是預期的 ${LONG_ACCOUNT}` },
            ],
    }),
  );
}

const CATEGORIES: TimelineCategory[] = ["dispatch", "handoff", "decision", "recovery", "verification", "cleanup", "task", "external_operation", "lesson", "usage", "controller"];

/** long 資料集。 */
export function longData(): PreviewData {
  const workspace = workspaceOf({ name: LONG_NAME(1), path: LONG_PATH });
  // 其餘 14 個路徑只有最後幾字不同（…-10-28-2.md），檢查側欄縮短後仍看得出差異
  const workspaces = [
    workspace,
    ...Array.from({ length: 14 }, (_, i) =>
      workspaceOf({ id: `ws_${i + 2}_${LONG_SHA256.slice(0, 16)}`, name: LONG_NAME(i + 2), path: LONG_PATH.replace(/\.md$/, `-${i + 2}.md`) }),
    ),
  ];
  const tasks = longTasks();
  const decisions = longDecisions();
  const profiles = longProfiles(true);
  const lessons = Array.from({ length: 25 }, (_, i) =>
    knowledgeOf(`know_${i}_${LONG_SHA256.slice(0, 24)}`, i % 2 === 0 ? LONG_TASK_TITLE : MIXED_SENTENCE, {
      kind: i % 3 === 0 ? "lesson" : i % 3 === 1 ? "fact" : "decision",
      status: i % 4 === 0 ? "candidate" : "verified",
      content: {
        title: i % 2 === 0 ? LONG_TASK_TITLE : MIXED_SENTENCE,
        body: LONG_PARAGRAPHS,
        tags: ["額度", "交接", "very-long-tag-without-spaces-for-layout-stress-testing", LONG_SHA256],
        evidence: [LONG_PATH, LONG_URL],
        lesson: i % 3 === 0 ? { trigger: MIXED_SENTENCE, root_cause: LONG_ERROR, resolution: LONG_PARAGRAPHS } : null,
      },
    }),
  );
  return {
    health: {
      status: "ok",
      product: "Baton",
      version: "0.1.0-preview.20261028+build.7b5deb026d9841b0",
      boot_count: 98_765,
      started_at: CHECKED_AT,
      pid: 4242,
      storage: {
        dispatch_halted: true,
        fault: { kind: "unwritable", code: "storage_unwritable", summary: LONG_ERROR, action: LONG_PARAGRAPHS.slice(0, 180), detail: LONG_ERROR },
      },
      guardian: { state: "unavailable", message: `監護程序沒有在執行：${LONG_ERROR.slice(0, 100)}` },
    },
    platform: {
      platform: { os_family: "windows11", os_build: "26300.1742", feature_version: "26H2", edition: "Enterprise multi-session", native_arch: "arm64", process_arch: "x64" },
      app_version: "0.1.0-preview.20261028+build.7b5deb026d9841b0",
      assessment: {
        status: "unsupported",
        issues: [
          { code: "unsupported_architecture", message: `原生架構為 arm64，目前是 x64 模擬執行，不在正式支援範圍。${MIXED_SENTENCE}` },
          { code: "support_ended", message: LONG_ERROR },
        ],
        support_ends_on: "2028-10-10",
        blocks_dispatch: false,
      },
      support_table_source: LONG_URL,
      support_table_checked_on: "2026-10-22",
    },
    workspaces,
    models: {
      models: Array.from({ length: 12 }, (_, i) => ({ id: `claude-very-long-model-identifier-${i}-20261028-preview-experimental`, label: `${MIXED_SENTENCE.slice(0, 40)} ${i}` })),
      updated_at: CHECKED_AT,
    },
    authorization: authorizationOf(workspace, {
      risk_disclosure: LONG_PARAGRAPHS,
      effective: {
        workspace_id: workspace.id,
        revision: 128,
        grants: [
          { capability: "read_files", targets: [LONG_PATH, LONG_URL], restrictions: [MIXED_SENTENCE] },
          { capability: "write_files", targets: [LONG_PATH], restrictions: [LONG_PARAGRAPHS.slice(0, 120)] },
          { capability: "deploy", targets: [LONG_URL], restrictions: ["只在週末", LONG_ERROR] },
        ],
        source: { kind: "template_edited", template_id: "dev", base_revision: 127, confirmed_by: LONG_ACCOUNT },
        confirmed_at: CHECKED_AT,
      },
      legacy_runs: Array.from({ length: 4 }, (_, i) => ({
        run_id: `run_${i}_${LONG_SHA256}`,
        task_id: LONG_ID,
        task_title: LONG_TASK_TITLE,
        status: "running" as const,
        authorization_revision: 120 + i,
        admitted_at: CHECKED_AT,
        cancel_requested: i % 2 === 0,
        revoked_capabilities: ["deploy" as const, "production_data" as const],
      })),
    }),
    board: {
      workspace_id: workspace.id,
      goals: [
        { id: "goal_1", workspace_id: workspace.id, title: LONG_TASK_TITLE, version: 7, created_at: CHECKED_AT },
        { id: "goal_long", workspace_id: workspace.id, title: MIXED_SENTENCE, version: 3, created_at: CHECKED_AT },
      ],
      tasks,
      runs: tasks
        .filter((task) => ["running", "verifying", "blocked", "waiting_resource", "completed"].includes(task.status))
        .map((task, i) =>
          runView({
            id: `run_${i}_${LONG_SHA256.slice(0, 32)}`,
            task_id: task.id,
            profile_id: profiles[i % profiles.length].id,
            generation: 10 + i,
            status: task.status === "completed" ? "succeeded" : task.status === "blocked" ? "failed" : "running",
            end_reason: task.status === "blocked" ? LONG_ERROR : null,
            latest_progress: { summary: LONG_PARAGRAPHS.slice(0, 260), reported_at: CHECKED_AT },
            latest_checkpoint: { summary: MIXED_SENTENCE, details: LONG_PARAGRAPHS, saved_at: CHECKED_AT },
            blocked: task.status === "blocked" ? { reason: LONG_ERROR, reported_at: CHECKED_AT } : null,
          }),
        ),
      capabilities: authorizationOf(workspace).capabilities,
      verifications: tasks
        .filter((task) => task.status === "verifying" || task.status === "completed")
        .map((task, i) =>
          verificationRecord({
            id: `ver_${i}`,
            task_id: task.id,
            outcome: i % 3 === 0 ? "failed" : i % 3 === 1 ? "inconclusive" : "passed",
            note: LONG_ERROR,
            checks: [
              { check: { kind: "command", program: "cargo", args: ["test", "-p", "baton-store", "--release", "--", "--nocapture"], expected_exit_code: 0 }, status: "failed", detail: LONG_ERROR },
              { check: { kind: "file", path: LONG_PATH, sha256: LONG_SHA256 }, status: "passed", detail: MIXED_SENTENCE },
            ],
            artifacts: [{ artifact_id: "art_1", name: LONG_PATH.split("\\").at(-1)!, version: 42, sha256: LONG_SHA256 }],
          }),
        ),
      authorization_revision: 128,
      workdirs: tasks.map((task, i) =>
        workdirView({
          task_id: task.id,
          path: LONG_PATH,
          branch: `baton/${task.id}`,
          status: i % 7 === 0 ? "failed" : "ready",
          failure: i % 7 === 0 ? LONG_ERROR : null,
          run_ids: [`run_${i}_${LONG_SHA256.slice(0, 32)}`],
        }),
      ),
      integrations: tasks
        .filter((task) => task.status === "verifying")
        .map((task, i) =>
          integrationEntry({
            id: `int_${i}`,
            seq: i + 1,
            task_id: task.id,
            branch: `baton/${task.id}`,
            target_branch: "release/2026-10-very-long-release-branch-name-for-testing",
            status: i === 0 ? "failed" : "queued",
            failure: i === 0 ? LONG_ERROR : null,
            merge_commit: LONG_SHA256.slice(0, 40),
          }),
        ),
      budgets: tasks.map((task, i) =>
        budgetView({
          task_id: task.id,
          counters: [
            { kind: "implementation", label: "實作失敗修正", used: i % 4, limit: 3 },
            { kind: "verification", label: "驗證不完整重驗", used: i % 3, limit: 2 },
            { kind: "infrastructure", label: "基礎設施重試", used: i % 6, limit: 5 },
          ],
          exhausted: i % 5 === 0 ? "implementation" : null,
          retry_not_before: i % 4 === 0 ? CHECKED_AT + 600_000 : null,
        }),
      ),
      handoffs: tasks
        .filter((task) => task.status === "waiting_resource")
        .map((task, i) =>
          handoff({
            id: `ho_${i}`,
            task_id: task.id,
            from_profile_id: profiles[0].id,
            to_profile_id: profiles[1].id,
            phase: "waiting_resource",
            next_check_at: CHECKED_AT + 600_000,
            wait_attempts: 7,
            wait_reasons: [{ category: "quota", code: "quota_exhausted", message: LONG_ERROR }],
          }),
        ),
    },
    decisions: decisionListOf(decisions),
    authority: authorityTable({
      entries: [...authorityTable().entries, ...Array.from({ length: 8 }, (_, i) => ({ matter: `matter_${i}`, label: `${MIXED_SENTENCE.slice(0, 50)}（${i}）`, builtin: "requires_decision" as const, effective: "autonomous" as const, overridden: true }))],
    }),
    pending: { pending_count: 12_345, by_workspace: workspaces.map((w, i) => ({ workspace_id: w.id, pending_count: i === 0 ? 1_284 : 99_999 - i })) },
    artifacts: {
      artifacts: Array.from({ length: 20 }, (_, i) => ({
        id: `art_${i}_${LONG_SHA256.slice(0, 16)}`,
        workspace_id: workspace.id,
        task_id: LONG_ID,
        name: i % 2 === 0 ? LONG_PATH.split("\\").slice(-3).join("/") : `${LONG_MARKER}_very_long_artifact_file_name_without_spaces_${LONG_SHA256}.json`,
        created_at: CHECKED_AT,
        versions: Array.from({ length: 3 }, (_, v) => ({
          version: v + 1,
          description: v === 0 ? LONG_PARAGRAPHS.slice(0, 220) : MIXED_SENTENCE,
          size_bytes: 1_234_567_890 + v,
          sha256: LONG_SHA256,
          storage_path: LONG_PATH,
          produced_by_run_id: `run_${i}_${LONG_SHA256}`,
          produced_by_generation: 10 + v,
          created_at: CHECKED_AT,
          file_status: v === 2 && i % 4 === 0 ? ("size_mismatch" as const) : ("ok" as const),
        })),
      })),
      unregistered_files: [
        { storage_path: LONG_PATH, size_bytes: 9_876_543_210, kind: "staging" },
        { storage_path: `${LONG_PATH}.partial`, size_bytes: 42, kind: "unregistered" },
      ],
    },
    knowledge: { search_mode: "like_fallback", entries: lessons },
    guards: {
      guards: lessons.slice(0, 8).map((lesson, i) =>
        guardOf(`grd_${i}`, lesson, {
          description: `${MIXED_SENTENCE}契約範圍必須提到「${LONG_PATH.slice(0, 40)}」`,
          active: i % 2 === 0,
          inactive_reason: i % 2 === 0 ? null : LONG_ERROR,
          prevented_count: 1_234,
          reported_count: 567,
        }),
      ),
    },
    scheduling: schedulingOverview({
      capacity: { per_account_limit: 3, global_limit: 24, handoff_reserve: 1, normal_limit: 23, running: 22, usable_accounts: 5 },
      accounts: profiles.map((p, i) =>
        accountScheduling({
          profile_id: p.id,
          name: p.name,
          dispatchable: p.dispatchable,
          blockers: p.blockers,
          running: i % 4,
          limit: 3,
          quota_used: 0.875,
          quota: quotaAssessment({ incomplete_reason: `${quotaAssessment().incomplete_reason}。${MIXED_SENTENCE}` }),
          runs: Array.from({ length: i % 4 }, (_, j) => accountRun({ run_id: `run_${i}_${j}_${LONG_SHA256}`, task_id: LONG_ID, task_title: LONG_TASK_TITLE })),
        }),
      ),
      queue: tasks
        .filter((task) => task.status === "ready")
        .map((task, i) => ({
          task_id: task.id,
          workspace_id: workspace.id,
          title: task.title,
          priority: i,
          requested_at: CHECKED_AT,
          position: i + 1,
          reasons: [
            { category: "capacity" as const, code: "global_limit", message: MIXED_SENTENCE },
            { category: "quota" as const, code: "quota_exhausted", message: LONG_ERROR },
          ],
        })),
    }),
    profiles: { profiles, limit: 8 },
    setup: setupStatus({
      claude_code: { ...okTool, version: "2.1.307 (Claude Code) build 7b5deb026d9841b0cced7314", message: MIXED_SENTENCE },
      profiles,
      background_notice: { text: LONG_PARAGRAPHS, acknowledged_at: CHECKED_AT },
    }),
    lifecycle: lifecycleStatus({
      login_startup: { enabled: true, registered: null },
      workspaces: workspaces.map((w, i) => workspaceLifecycle({ workspace_id: w.id, name: w.name, auto_resume_after_exit: i % 2 === 0, paused_since: i % 3 === 0 ? CHECKED_AT : null, paused_by: i % 3 === 0 ? ("full_exit" as const) : null, awaiting_resume: i % 3 === 0 })),
    }),
    sharedSettings: {
      current: settingsVersionSummary({ version: 1_024, content_hash: LONG_SHA256, counts: { preferences: 1_234, instructions: 56, hooks: 789, mcp_servers: 101, plugins: 2_048 }, exclusion_count: 999 }),
      versions: Array.from({ length: 12 }, (_, i) => settingsVersionSummary({ version: 1_024 - i, content_hash: LONG_SHA256 })),
    },
    backups: {
      backups: Array.from({ length: 15 }, (_, i) => backupSummary({ id: `${CHECKED_AT - i * 86_400_000}-daily-${LONG_SHA256}`, created_at: CHECKED_AT - i * 86_400_000, artifact_count: 12_345 + i })),
      latest_drill: drillReport({
        passed: false,
        interrupted_runs: 17,
        checks: drillReport().checks.map((check, i) => ({ ...check, passed: i % 2 === 0, detail: i % 2 === 0 ? MIXED_SENTENCE : LONG_ERROR })),
      }),
    },
    updates: availableUpdateStatus({
      current_version: "0.1.0-preview.20261028+build.7b5deb026d9841b0",
      available: { version: "0.2.0-rc.1+build.9f8e7d6c5b4a3f2e1d0c9b8a7f6e5d4c", notes: LONG_PARAGRAPHS, published_at: "2026-10-28T00:00:00Z" },
      error: LONG_ERROR,
    }),
    controller: {
      workspace_id: workspace.id,
      run: controllerCliRun({ id: `ctl_${LONG_SHA256}`, profile_id: profiles[0].id, working_dir: LONG_PATH, command_id: LONG_ID, end_reason: LONG_ERROR }),
      playbook_version: "2026.10.28-preview.7b5deb026d9841b0cced7314",
    },
    recovery: recoveryStatus({
      event: recoveryEvent({ status: "exhausted", attempts_used: 3, last_failure: LONG_ERROR }),
      can_retry: true,
      blocking: { category: "state_unknown", code: "recovery_exhausted", message: LONG_ERROR },
    }),
    timeline: {
      workspace_id: workspace.id,
      task_id: null,
      events: Array.from({ length: 150 }, (_, i) =>
        timelineEvent({
          seq: i + 1,
          occurred_at: CHECKED_AT + i * 60_000,
          category: CATEGORIES[i % CATEGORIES.length],
          aggregate_type: "run",
          aggregate_id: `run_${i}_${LONG_SHA256}`,
          event_type: i % 2 === 0 ? "run_created" : "very_long_event_type_name_without_spaces_for_layout_stress_testing",
          task_ids: [LONG_ID],
          run_id: `run_${i}_${LONG_SHA256}`,
          payload: { profile_id: profiles[i % profiles.length].id, path: LONG_PATH, message: MIXED_SENTENCE, url: LONG_URL },
        }),
      ),
    },
    metrics: metricsResponse({
      server_quota: profiles.map((p, i) => ({ profile_id: p.id, name: p.name, five_hour_used: 0.875 + i / 100, seven_day_used: 0.999 })),
    }),
    understanding: understandingView({
      items: Array.from({ length: 12 }, (_, i) =>
        understandingItem({
          id: `und_${i}`,
          key: `very_long_understanding_key_without_spaces_${i}_${LONG_SHA256.slice(0, 16)}`,
          text: i % 2 === 0 ? LONG_PARAGRAPHS : MIXED_SENTENCE,
          stale: i % 3 === 0,
          sources: [{ kind: "file", path: LONG_PATH, line: 12_345, version: LONG_SHA256, checked_at: CHECKED_AT, changed: i % 3 === 0 }],
        }),
      ),
    }),
    workflows: workflowList({
      workflows: Array.from({ length: 6 }, (_, i) =>
        workflowView({ id: `wf_${i}`, task_id: LONG_ID, task_title: LONG_TASK_TITLE, route_reason: LONG_PARAGRAPHS, blocked_reason: i % 2 === 0 ? LONG_ERROR : null }),
      ),
    }),
    reviews: reviewList({
      reviews: Array.from({ length: 6 }, (_, i) =>
        failingReviewView({ id: `rev_${i}`, subject_task_title: LONG_TASK_TITLE, reviewer_task_title: MIXED_SENTENCE, candidate_revision: LONG_SHA256.slice(0, 40), base_revision: LONG_SHA256.slice(24), standards_source: LONG_PATH, stale_reason: LONG_ERROR }),
      ),
    }),
    verification: verificationOverview(),
    exploration: explorationOverview({
      findings: [
        ...explorationOverview().findings,
        ...Array.from({ length: 8 }, (_, i) =>
          explorationFinding({ id: `xf_long_${i}`, title: LONG_TASK_TITLE, task_title: LONG_DECISION_QUESTION, steps: [MIXED_SENTENCE, LONG_URL, LONG_PATH], expected: LONG_PARAGRAPHS, actual: LONG_ERROR, environment: MIXED_SENTENCE }),
        ),
      ],
    }),
  };
}

/** long 首次啟動精靈：找不到 Claude Code、Git 太舊、帳號都不能派工。 */
export function longSetup(data: PreviewData): PreviewData {
  const setup = asSetup(data, longProfiles(false));
  return {
    ...setup,
    setup: {
      ...setup.setup,
      claude_code: { ...missingClaude, message: `${missingClaude.message}。${MIXED_SENTENCE}`, docs_url: LONG_URL },
      git: {
        ...okTool,
        status: "too_old",
        version: "2.20.1.windows.1",
        minimum_version: "2.40.0",
        message: LONG_ERROR,
        guidance: [
          {
            title: "以 WinGet 更新 Git for Windows 到最新版本（需要系統管理員權限）",
            command: `winget upgrade --id Git.Git --exact --source winget --accept-package-agreements --location "${LONG_PATH}"`,
          },
        ],
      },
    },
  };
}
