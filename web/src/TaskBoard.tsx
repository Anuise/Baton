import { useCallback, useEffect, useId, useRef, useState, type FormEvent, type KeyboardEvent } from "react";

import { buildContract, ContractEditor, draftFromContract, emptyDraft, type ContractDraft } from "./ContractEditor";
import { HelpTip } from "./HelpTip";
import { optionLabel } from "./textFormat";
import { formatTimestamp } from "./time";
import { errorCode } from "./runtimeClient";
import type {
  AcceptanceBasis,
  BudgetView,
  CheckResult,
  CleanupStatus,
  ContextItem,
  ContextItemKind,
  Goal,
  Handoff,
  HandoffPhase,
  IntegrationEntry,
  IntegrationStatus,
  MissingItem,
  RunContext,
  RunStatus,
  RunView,
  SchedulingClient,
  SchedulingOverview,
  TaskBoard as TaskBoardData,
  TaskClient,
  TaskContract,
  TaskKind,
  TaskStatus,
  TaskView,
  VerificationCheck,
  VerificationRecord,
  VerificationView,
  WaitCategory,
  WaitReason,
  WorkdirView,
  Workspace,
  WorktreeCleanup,
} from "./runtimeClient";

/** 排程總覽讀不到時是 null：看板照常顯示，只是沒有排隊順位與帳號名稱。 */
type ViewState =
  | { state: "loading" }
  | { state: "ready"; board: TaskBoardData; overview: SchedulingOverview | null }
  | { state: "failed"; reason: string };

/** 派工後回傳排隊中：還沒出現在排程總覽的佇列之前，先顯示 Runtime 回報的原因。 */
interface QueuedNotice {
  taskId: string;
  reasons: WaitReason[];
}

export type TaskBoardClient = TaskClient & Pick<SchedulingClient, "schedulingOverview">;

interface Props {
  client: TaskBoardClient;
  workspace: Workspace;
  /** 顯示用的時區；省略時使用系統時區（spec D-33），只有測試會指定。 */
  timeZone?: string;
}

export const COLUMNS: [TaskStatus, string][] = [
  ["pending", "待處理"],
  ["ready", "可派工"],
  ["running", "執行中"],
  ["waiting_decision", "等待裁定"],
  ["waiting_resource", "等待資源"],
  ["blocked", "受阻"],
  ["verifying", "驗證中"],
  ["completed", "已完成"],
  ["cancelled", "已取消"],
];

export const RUN_STATUS_LABELS: Record<RunStatus, string> = {
  queued: "排隊中",
  starting: "啟動中",
  running: "執行中",
  succeeded: "成功",
  failed: "失敗",
  interrupted: "中斷",
  cancelled: "已取消",
};

const WAIT_CATEGORY_LABELS: [WaitCategory, string][] = [
  ["capacity", "名額已滿"],
  ["quota", "額度"],
  ["authorization", "授權"],
  ["dependency", "相依"],
  ["state_unknown", "狀態不明"],
  ["account", "帳號"],
  ["workdir", "工作目錄"],
  ["lifecycle", "完全退出"],
  ["guard", "Guard 擋下"],
  ["backoff", "重試退避"],
];

/** 建立表單的選項順序：預設的「研究」排第一。 */
const TASK_KIND_LABELS: [TaskKind, string][] = [
  ["research", "研究"],
  ["development", "程式開發"],
  ["document", "文件"],
];

const kindLabel = (kind: TaskKind) => TASK_KIND_LABELS.find(([k]) => k === kind)?.[1] ?? kind;

const INTEGRATION_STATUS_LABELS: Record<IntegrationStatus, string> = {
  queued: "排隊等待整合",
  merging: "合併中",
  verifying: "整合驗證中",
  advancing: "更新目標分支中",
  integrated: "已整合",
  failed: "未通過整合",
};

const CLEANUP_STATUS_LABELS: Record<CleanupStatus, string> = {
  pending: "待清理",
  removing: "清理中",
  cleaned: "已清理",
  failed: "清理失敗",
};

const HANDOFF_PHASE_LABELS: Record<HandoffPhase, string> = {
  requested: "交接中：等待保存檢查點",
  stopping: "交接中：停止舊程序",
  transferred: "已交接，等待接手確認",
  confirmed: "已交接並確認",
  abandoned: "交接已放棄",
  waiting_resource: "等待資源：所有合適帳號都受限",
};

/** 額度門檻原因代碼的顯示文字（帳號頁的門檻與交接原因共用）；未知代碼原樣顯示。 */
export const GATE_REASON_LABELS: Record<string, string> = {
  stop_dispatch_threshold: "達 75% 停止派工門檻",
  drain_threshold: "達 85% 交接門檻",
  limit_warning: "Claude Code 回報限流警告",
  limit_reached: "Claude Code 回報已達上限（突然限流）",
  recovered: "用量回到 60% 以下",
  reset_observed: "已過重置時間",
  usage_unknown: "用量未知（訊號已失效）",
};

/** 交接相關的結束原因與狀態文字：這些 Run 不是一般的中斷，也不另列結束原因。 */
const HANDOFF_END_REASON_LABELS: Record<string, string> = {
  handed_off: "已交接",
  handed_off_waiting_resource: "已停止（等待資源）",
};

/** Run 的狀態文字：因交接（或突然限流而等待資源）結束的 Run 用交接的說法，不是一般的中斷。 */
const runStatusLabel = (run: RunView) =>
  (run.end_reason !== null ? HANDOFF_END_REASON_LABELS[run.end_reason] : undefined) ?? RUN_STATUS_LABELS[run.status];

/** 整合項目尚未結束（排隊中或處理中）。 */
const isIntegrationActive = (status: IntegrationStatus) => status !== "integrated" && status !== "failed";

/** 整合佇列「目前這一個」：正在合併、驗證或前移目標分支。 */
const isIntegrationProcessing = (status: IntegrationStatus) =>
  status === "merging" || status === "verifying" || status === "advancing";

/** Run 仍在進行（尚未結束）。 */
const isRunActive = (status: RunStatus) => status === "queued" || status === "starting" || status === "running";

/** 同一個 Task 最新的 Run：generation 最大者。 */
function latestRun(runs: RunView[], taskId: string): RunView | null {
  return runs
    .filter((r) => r.task_id === taskId)
    .reduce<RunView | null>((latest, r) => (latest === null || r.generation > latest.generation ? r : latest), null);
}

/** 同一個 Task 最新的整合項目：seq 最大者。 */
function latestIntegration(entries: IntegrationEntry[], taskId: string): IntegrationEntry | null {
  return entries
    .filter((e) => e.task_id === taskId)
    .reduce<IntegrationEntry | null>((latest, e) => (latest === null || e.seq > latest.seq ? e : latest), null);
}

/** 同一個 Task 最新的驗證紀錄（紀錄由舊到新）。 */
function latestVerification(records: VerificationRecord[], taskId: string): VerificationRecord | null {
  return records.filter((r) => r.task_id === taskId).at(-1) ?? null;
}

const statusLabel = (status: TaskStatus) => COLUMNS.find(([s]) => s === status)?.[1] ?? status;

/** SHA-256 只顯示前幾碼。 */
const shortHash = (sha256: string) => `${sha256.slice(0, 12)}…`;

/** 宣告的確定性檢查的說明文字。 */
function describeCheck(check: VerificationCheck): string {
  const hashNote = (sha256: string | null) => (sha256 === null ? "（存在即可）" : `（SHA-256 ${shortHash(sha256)}）`);
  switch (check.kind) {
    case "command":
      return `指令 ${[check.program, ...check.args].join(" ")}（預期結束碼 ${check.expected_exit_code}）`;
    case "file":
      return `檔案 ${check.path}${hashNote(check.sha256)}`;
    case "artifact":
      return `成果 ${check.name}${hashNote(check.sha256)}`;
  }
}

const isClosed = (status: TaskStatus) => status === "completed" || status === "cancelled";

function reasonOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** 「未知」不等於「未提供」：兩者分開標示。 */
function missingKind(item: MissingItem): string | null {
  if (item.code === "field_unknown") return "未知";
  if (item.code === "field_missing") return "未提供";
  return null;
}

/** 最新 Run 使用的帳號名稱；總覽讀不到或帳號已封存時退回 Profile 識別。 */
function accountNameOf(overview: SchedulingOverview | null, runs: RunView[], taskId: string): string | null {
  const run = latestRun(runs, taskId);
  if (run === null) return null;
  return overview?.accounts.find((account) => account.profile_id === run.profile_id)?.name ?? run.profile_id;
}

/** ready 的 Task 在佇列中時的排隊資訊：優先用排程總覽的順位與原因，其次是剛派工時回報的原因。 */
function queuedInfo(
  overview: SchedulingOverview | null,
  notice: QueuedNotice | null,
  task: TaskView,
): { position: number | null; reasons: WaitReason[] } | null {
  if (task.status !== "ready") return null;
  const entry = overview?.queue.find((e) => e.task_id === task.id);
  if (entry) return { position: entry.position, reasons: entry.reasons };
  if (notice?.taskId === task.id) return { position: null, reasons: notice.reasons };
  return null;
}

/**
 * 工作面板：依狀態分欄顯示 Workspace 內的 Task，建立 Task、編輯六項最小派工契約、
 * 標記為可派工。缺少的項目直接列在 pending 的卡片上。
 */
export function TaskBoard({ client, workspace, timeZone }: Props) {
  const [loaded, setLoaded] = useState<ViewState>({ state: "loading" });
  // returnFocus：對話框關閉後焦點回到開啟它的按鈕
  const [panel, setPanel] = useState<
    ({ kind: "create" } | { kind: "edit"; task: TaskView }) & { returnFocus: HTMLElement | null } | null
  >(null);
  const [cardError, setCardError] = useState<{ taskId: string; message: string } | null>(null);
  const [queuedNotice, setQueuedNotice] = useState<QueuedNotice | null>(null);

  const load = useCallback(async () => {
    try {
      const [board, overview] = await Promise.all([
        client.getTaskBoard(workspace.id),
        // 排程總覽只補充排隊與帳號資訊，讀不到不影響看板
        Promise.resolve()
          .then(() => client.schedulingOverview())
          .catch(() => null),
      ]);
      setLoaded({ state: "ready", board, overview });
    } catch (e) {
      setLoaded({ state: "failed", reason: reasonOf(e) });
    }
  }, [client, workspace.id]);

  useEffect(() => {
    setLoaded({ state: "loading" });
    setPanel(null);
    setCardError(null);
    setQueuedNotice(null);
    void load();
  }, [load]);

  if (loaded.state === "loading") return <section aria-label="工作面板">載入中…</section>;
  if (loaded.state === "failed") {
    return (
      <section aria-label="工作面板">
        <p role="alert">無法取得工作面板：{loaded.reason}</p>
      </section>
    );
  }
  const { board, overview } = loaded;
  const titleOf = (taskId: string) => board.tasks.find((t) => t.id === taskId)?.title ?? taskId;
  const runLabelOf = (runId: string) => {
    const found = board.runs.find((r) => r.id === runId);
    return found ? `第 ${found.generation} 次` : runId;
  };
  /** 帳號名稱；總覽讀不到或帳號已封存時退回 Profile 識別。 */
  const profileNameOf = (profileId: string) =>
    overview?.accounts.find((account) => account.profile_id === profileId)?.name ?? profileId;

  /** 執行卡片上的操作：失敗時把 Runtime 的說明顯示在該卡片上；無論成敗都重新載入看板。 */
  const act = async (task: TaskView, action: () => Promise<unknown>): Promise<boolean> => {
    setCardError(null);
    setQueuedNotice(null);
    let ok = true;
    try {
      await action();
    } catch (e) {
      ok = false;
      setCardError({ taskId: task.id, message: reasonOf(e) });
    }
    await load();
    return ok;
  };

  const done = async () => {
    setPanel(null);
    await load();
  };

  return (
    <section className="task-board" aria-label="工作面板">
      <header className="task-board-header">
        <h2>工作</h2>
        <button
          type="button"
          className="primary-action"
          onClick={(e) => setPanel({ kind: "create", returnFocus: e.currentTarget })}
        >
          建立 Task
        </button>
      </header>

      <IntegrationQueue entries={board.integrations} titleOf={titleOf} />

      {panel && (
        <TaskForm
          key={panel.kind === "edit" ? panel.task.id : "create"}
          client={client}
          workspaceId={workspace.id}
          board={board}
          task={panel.kind === "edit" ? panel.task : null}
          onDone={() => void done()}
          onClose={() => setPanel(null)}
          onGoalCreated={() => void load()}
          returnFocus={panel.returnFocus}
        />
      )}

      <div className="task-columns">
        {COLUMNS.map(([status, label]) => {
          const tasks = board.tasks.filter((t) => t.status === status);
          return (
            <section key={status} className="task-column" aria-label={`${label}（${tasks.length}）`}>
              <h3>
                {label}
                <span className="badge task-count">{tasks.length}</span>
              </h3>
              {tasks.map((task) => (
                <TaskCard
                  key={task.id}
                  client={client}
                  workspaceId={workspace.id}
                  task={task}
                  goalTitle={board.goals.find((g) => g.id === task.goal_id)?.title ?? task.goal_id}
                  titleOf={titleOf}
                  others={board.tasks.filter((t) => t.id !== task.id)}
                  run={latestRun(board.runs, task.id)}
                  verification={latestVerification(board.verifications, task.id)}
                  workdir={board.workdirs.filter((w) => w.task_id === task.id).at(-1) ?? null}
                  runLabelOf={runLabelOf}
                  integration={latestIntegration(board.integrations, task.id)}
                  budget={board.budgets.find((b) => b.task_id === task.id) ?? null}
                  handoff={(board.handoffs ?? []).filter((h) => h.task_id === task.id).at(-1) ?? null}
                  profileNameOf={profileNameOf}
                  accountName={accountNameOf(overview, board.runs, task.id)}
                  authorizationRevision={board.authorization_revision}
                  queued={queuedInfo(overview, queuedNotice, task)}
                  timeZone={timeZone}
                  error={cardError?.taskId === task.id ? cardError.message : null}
                  onReady={() => void act(task, () => client.markTaskReady(workspace.id, task.id))}
                  onCancel={() => void act(task, () => client.cancelTask(workspace.id, task.id))}
                  onDispatch={() =>
                    void act(task, async () => {
                      const result = await client.dispatchTask(workspace.id, task.id);
                      if (result.kind === "queued") setQueuedNotice({ taskId: task.id, reasons: result.reasons });
                    })
                  }
                  onCancelRun={(runId) => void act(task, () => client.cancelRun(workspace.id, runId))}
                  onEdit={(returnFocus) => setPanel({ kind: "edit", task, returnFocus })}
                  onAddDependency={(dependsOn) => act(task, () => client.addTaskDependency(workspace.id, task.id, dependsOn))}
                  onAccepted={() => void load()}
                />
              ))}
            </section>
          );
        })}
      </div>
    </section>
  );
}

interface CardProps {
  client: TaskClient;
  workspaceId: string;
  task: TaskView;
  goalTitle: string;
  titleOf: (taskId: string) => string;
  /** 同一個 Workspace 內的其他 Task，供選擇相依對象。 */
  others: TaskView[];
  /** 這個 Task 最新的 Run；從未派工過為 null。 */
  run: RunView | null;
  /** 這個 Task 最新的驗證紀錄；沒有驗證過為 null。 */
  verification: VerificationRecord | null;
  /** 這個 Task 登記的工作目錄；尚未準備過為 null。 */
  workdir: WorkdirView | null;
  /** Run 的顯示文字（第 N 次）。 */
  runLabelOf: (runId: string) => string;
  /** 這個 Task 最新的整合項目；沒有排入過整合為 null。 */
  integration: IntegrationEntry | null;
  /** 這個 Task 的修正與重試預算；沒有資料為 null。 */
  budget: BudgetView | null;
  /** 這個 Task 最新的交接；從未交接為 null。 */
  handoff: Handoff | null;
  /** 帳號 Profile 識別 → 顯示名稱。 */
  profileNameOf: (profileId: string) => string;
  /** 最新 Run 使用的帳號名稱。 */
  accountName: string | null;
  /** Workspace 目前生效的授權 revision；尚未確認為 null。 */
  authorizationRevision: number | null;
  /** 在佇列中等待名額時的順位（未知為 null）與原因；不在佇列為 null。 */
  queued: { position: number | null; reasons: WaitReason[] } | null;
  timeZone?: string;
  error: string | null;
  onReady: () => void;
  onCancel: () => void;
  onDispatch: () => void;
  onCancelRun: (runId: string) => void;
  /** 開啟契約對話框；returnFocus 是關閉後焦點要回到的按鈕。 */
  onEdit: (returnFocus: HTMLElement | null) => void;
  onAddDependency: (dependsOn: string) => Promise<boolean>;
  /** 手動接受成功：看板需要重新載入。 */
  onAccepted: () => void;
}

type HistoryState = { state: "loading" } | { state: "ready"; contracts: TaskContract[] } | { state: "failed"; reason: string };

type StatusTone = "success" | "warning" | "danger" | "accent";

/** 卡片上的一行狀態：徽章（最多 6 個字）＋ 一行說明；細節在「詳細資訊」裡。 */
interface CardStatus {
  tone: StatusTone;
  label: string;
  text: string | null;
}

/** 依需要使用者注意的程度列出卡片的全部狀態（最重要的在前）。 */
function cardStatuses({
  task,
  run,
  queued,
  budget,
  workdir,
  handoff,
  integration,
  awaitingIntegration,
  verification,
  accountName,
  titleOf,
}: {
  task: TaskView;
  run: RunView | null;
  queued: CardProps["queued"];
  budget: BudgetView | null;
  workdir: WorkdirView | null;
  handoff: Handoff | null;
  integration: IntegrationEntry | null;
  awaitingIntegration: boolean;
  verification: VerificationRecord | null;
  accountName: string | null;
  titleOf: (taskId: string) => string;
}): CardStatus[] {
  const statuses: CardStatus[] = [];
  if (budget !== null && budget.exhausted !== null) {
    const label = budget.counters.find((counter) => counter.kind === budget.exhausted)?.label ?? budget.exhausted;
    statuses.push({ tone: "danger", label: "預算用盡", text: `${label}預算已用完` });
  }
  if (workdir?.status === "failed") statuses.push({ tone: "danger", label: "目錄無法使用", text: workdir.failure });
  if (run?.blocked) statuses.push({ tone: "danger", label: "受阻", text: run.blocked.reason });
  if (queued) {
    const categories = WAIT_CATEGORY_LABELS.filter(([category]) => queued.reasons.some((r) => r.category === category));
    statuses.push({
      tone: "warning",
      label: queued.position === null ? "排隊中" : `排隊第 ${queued.position} 位`,
      text: categories.length === 0 ? "等待排程處理" : categories.map(([, label]) => label).join("、"),
    });
  }
  if (task.status === "pending" && task.missing.length > 0) {
    statuses.push({ tone: "warning", label: `缺少 ${task.missing.length} 項`, text: task.missing.map((item) => item.message).join("；") });
  }
  if (handoff && handoff.phase !== "confirmed" && handoff.phase !== "abandoned") {
    statuses.push({ tone: "accent", label: "交接中", text: HANDOFF_PHASE_LABELS[handoff.phase] });
  }
  if (integration?.status === "failed" && task.status !== "completed") {
    statuses.push({ tone: "danger", label: "整合未通過", text: integration.failure });
  }
  if (awaitingIntegration && integration) {
    statuses.push({ tone: "accent", label: "等待整合", text: INTEGRATION_STATUS_LABELS[integration.status] });
  }
  if (verification && verification.outcome !== "passed" && !isClosed(task.status)) {
    statuses.push(
      verification.outcome === "inconclusive"
        ? { tone: "warning", label: "驗證不完整", text: verification.note }
        : { tone: "danger", label: "驗證未通過", text: verification.note },
    );
  }
  if (run && isRunActive(run.status)) {
    statuses.push({ tone: "success", label: RUN_STATUS_LABELS[run.status], text: accountName && `帳號 ${accountName}` });
  }
  if (task.unfinished_dependencies.length > 0) {
    statuses.push({ tone: "warning", label: "等待相依", text: task.unfinished_dependencies.map(titleOf).join("、") });
  }
  if (workdir?.cleanup?.status === "failed") statuses.push({ tone: "danger", label: "清理失敗", text: workdir.cleanup.failure });
  return statuses;
}

/**
 * 狀態列：最重要的一項，沒有值得一提的狀態時為 null。第一項是 danger 時，其餘的 danger 也帶出來（`others`），
 * 讓例如工作目錄失敗不被預算用盡蓋住。
 */
function cardStatus(args: Parameters<typeof cardStatuses>[0]): (CardStatus & { others: CardStatus[] }) | null {
  const [first, ...rest] = cardStatuses(args);
  if (first === undefined) return null;
  return { ...first, others: first.tone === "danger" ? rest.filter((status) => status.tone === "danger") : [] };
}

function TaskCard({ client, workspaceId, task, goalTitle, titleOf, others, run, verification, workdir, runLabelOf, integration, budget, handoff, profileNameOf, accountName, authorizationRevision, queued, timeZone, error, onReady, onCancel, onDispatch, onCancelRun, onEdit, onAddDependency, onAccepted }: CardProps) {
  const [addingDependency, setAddingDependency] = useState(false);
  const [dependsOn, setDependsOn] = useState("");
  const [historyOpen, setHistoryOpen] = useState(false);
  const [history, setHistory] = useState<HistoryState>({ state: "loading" });
  const [moreOpen, setMoreOpen] = useState(false);
  const moreId = useId();
  const moreToggle = useRef<HTMLButtonElement>(null);

  // 展開時載入歷史；契約有新版本時重新載入
  useEffect(() => {
    if (!historyOpen) return;
    let current = true;
    setHistory({ state: "loading" });
    client.getTask(workspaceId, task.id).then(
      (detail) => current && setHistory({ state: "ready", contracts: detail.contract_history }),
      (e) => current && setHistory({ state: "failed", reason: reasonOf(e) }),
    );
    return () => {
      current = false;
    };
  }, [client, workspaceId, task.id, task.contract_version, historyOpen]);

  const closed = isClosed(task.status);
  // 開發 Task 的成果已通過驗證、排入整合：整合完成前仍停在 verifying，也不能再手動接受
  const awaitingIntegration = task.status === "verifying" && integration !== null && isIntegrationActive(integration.status);
  // 驗證中的卡片以「成果驗證」區塊當狀態列與主要動作
  const verifying = task.status === "verifying" && !awaitingIntegration;
  const status = verifying
    ? null
    : cardStatus({ task, run, queued, budget, workdir, handoff, integration, awaitingIntegration, verification, accountName, titleOf });
  const stripe = verifying ? "accent" : status?.tone;
  const canCancelRun = run !== null && isRunActive(run.status) && run.cancel_requested_at === null;
  const hasMore = canCancelRun || !closed;

  const addDependency = async () => {
    if (await onAddDependency(dependsOn)) {
      setAddingDependency(false);
      setDependsOn("");
    }
  };

  /** 「更多」裡的動作：先收起選單再執行。 */
  const pick = (action: () => void) => () => {
    setMoreOpen(false);
    action();
  };

  /** Esc 收起「更多」，焦點回到「更多」按鈕。 */
  const closeMoreOnEscape = (event: KeyboardEvent) => {
    if (event.key !== "Escape" || !moreOpen) return;
    event.stopPropagation();
    setMoreOpen(false);
    moreToggle.current?.focus();
  };

  return (
    <article className={stripe ? `card task-card stripe-${stripe}` : "card task-card"} aria-label={task.title}>
      {/* 標題最多 2 行、Goal 1 行；展開「詳細資訊」時顯示完整文字 */}
      <h4 className="task-title clamp-2" title={task.title}>
        {task.title}
      </h4>
      <p className="task-meta">
        <span className="badge task-kind">{kindLabel(task.kind)}</span>
        <span className="task-goal truncate" title={`Goal：${goalTitle}`}>
          Goal：{goalTitle}
        </span>
      </p>
      {status && (
        <p className="task-status">
          <span className={`badge badge-${status.tone}`}>{status.label}</span>
          {status.others[0] && (
            <span className="badge badge-danger" title={status.others[0].text ?? undefined}>
              {status.others[0].label}
            </span>
          )}
          {status.others.length > 1 && (
            <span
              className="badge badge-danger"
              aria-label={`另有 ${status.others.length - 1} 項：${status.others.slice(1).map((other) => other.label).join("、")}`}
              title={status.others
                .slice(1)
                .map((other) => (other.text ? `${other.label}：${other.text}` : other.label))
                .join("\n")}
            >
              +{status.others.length - 1}
            </span>
          )}
          {status.text && (
            <span className="task-status-text truncate" title={status.text}>
              {status.text}
            </span>
          )}
        </p>
      )}

      {verifying && (
        <VerificationPanel client={client} workspaceId={workspaceId} taskId={task.id} onAccepted={onAccepted} />
      )}

      {/* 細節一律收合，卡片高度可預期；工作目錄或清理失敗的原因已在上方的狀態列 */}
      <details className="task-details">
        <summary>詳細資訊</summary>
        <div className="task-details-body">
          {task.unfinished_dependencies.length > 0 && (
            <p className="task-dependencies">等待相依：{task.unfinished_dependencies.map(titleOf).join("、")}</p>
          )}
          {task.status === "pending" && task.missing.length > 0 && (
            <div className="task-missing" role="group" aria-label="缺少項目">
              <strong>缺少項目</strong>
              <ul>
                {task.missing.map((item, index) => {
                  const kind = missingKind(item);
                  return (
                    <li key={index} className={item.code === "field_unknown" ? "task-missing-unknown" : undefined}>
                      {kind && <strong>{kind}</strong>} <span>{item.message}</span>
                    </li>
                  );
                })}
              </ul>
            </div>
          )}

          {queued && <QueuedStatus position={queued.position} reasons={queued.reasons} />}

          {run && (
            <RunStatusLine
              client={client}
              workspaceId={workspaceId}
              run={run}
              accountName={accountName}
              authorizationRevision={authorizationRevision}
              timeZone={timeZone}
            />
          )}

          {handoff && <HandoffSummary handoff={handoff} profileNameOf={profileNameOf} timeZone={timeZone} />}

          {verification && <VerificationSummary record={verification} timeZone={timeZone} />}

          {integration && <IntegrationSummary entry={integration} awaiting={awaitingIntegration} />}

          {budget && isBudgetUsed(budget) && <BudgetStatus budget={budget} timeZone={timeZone} />}

          {workdir && <WorkdirStatus workdir={workdir} runLabelOf={runLabelOf} timeZone={timeZone} />}

          <p className="task-contract-version">
            <span>契約版本 v{task.contract_version}</span>
            <button
              type="button"
              className="btn-quiet btn-sm"
              aria-expanded={historyOpen}
              onClick={() => setHistoryOpen(!historyOpen)}
            >
              契約版本歷史
            </button>
          </p>
          {historyOpen && (
            <div className="task-history" role="group" aria-label="契約版本歷史">
              {history.state === "loading" && <p>載入中…</p>}
              {history.state === "failed" && <p role="alert">無法取得契約版本歷史：{history.reason}</p>}
              {history.state === "ready" && (
                <ol>
                  {[...history.contracts].reverse().map((contract) => (
                    <li key={contract.version}>
                      v{contract.version}：{formatTimestamp(contract.created_at, timeZone)}
                      {contract.note && `，${contract.note}`}
                    </li>
                  ))}
                </ol>
              )}
            </div>
          )}
        </div>
      </details>

      {error && <p role="alert">{error}</p>}

      {/* 每張卡片只放一個主要動作，其餘收進「更多」（MASTER 5.1） */}
      {(task.status === "ready" || task.status === "pending" || hasMore) && (
        <div className="task-actions">
          {task.status === "ready" && (
            <button type="button" className="primary-action" onClick={onDispatch}>
              派工
            </button>
          )}
          {task.status === "pending" && (
            <button type="button" className="primary-action" onClick={onReady}>
              標記為可派工
            </button>
          )}
          {hasMore && (
            <button
              ref={moreToggle}
              type="button"
              className="btn-quiet task-more-toggle"
              aria-expanded={moreOpen}
              aria-controls={moreId}
              onClick={() => setMoreOpen(!moreOpen)}
              onKeyDown={closeMoreOnEscape}
            >
              更多
            </button>
          )}
        </div>
      )}
      {moreOpen && (
        <div id={moreId} className="task-more" role="group" aria-label="更多動作" onKeyDown={closeMoreOnEscape}>
          {!closed && (
            <>
              <button type="button" className="btn-quiet" onClick={pick(() => onEdit(moreToggle.current))}>
                編輯契約
              </button>
              <button type="button" className="btn-quiet" onClick={pick(() => setAddingDependency(true))}>
                新增相依
              </button>
            </>
          )}
          {canCancelRun && (
            <button type="button" className="btn-danger" onClick={pick(() => onCancelRun(run.id))}>
              取消 Run
            </button>
          )}
          {!closed && (
            <button type="button" className="btn-danger" onClick={pick(onCancel)}>
              取消
            </button>
          )}
        </div>
      )}

      {addingDependency && (
        <div className="task-dependency-form">
          <label>
            相依的 Task
            {/* 從「更多」開啟時「更多」選單已收起，焦點直接移到這裡，不落到 body */}
            <select autoFocus value={dependsOn} onChange={(e) => setDependsOn(e.target.value)}>
              <option value="">選擇 Task…</option>
              {others.map((other) => (
                <option key={other.id} value={other.id}>
                  {optionLabel(other.title)}
                </option>
              ))}
            </select>
          </label>
          <div className="form-actions">
            <button type="button" disabled={dependsOn === ""} onClick={() => void addDependency()}>
              確定新增
            </button>
            <button type="button" className="btn-quiet" onClick={() => setAddingDependency(false)}>
              關閉
            </button>
          </div>
        </div>
      )}
    </article>
  );
}

/** 整合佇列：這個 Workspace 尚未結束的整合項目，依排入順序，標出處理中的一個。 */
function IntegrationQueue({ entries, titleOf }: { entries: IntegrationEntry[]; titleOf: (taskId: string) => string }) {
  const pending = entries.filter((e) => isIntegrationActive(e.status)).sort((a, b) => a.seq - b.seq);
  return (
    <section className="integration-queue" aria-label="整合佇列">
      <h3>整合佇列</h3>
      {pending.length === 0 ? (
        <p>沒有等待整合的項目</p>
      ) : (
        <ol>
          {pending.map((entry) => {
            const processing = isIntegrationProcessing(entry.status);
            return (
              // 一項一列：標題 1 行、分支等寬 1 行（完整文字在 title），狀態徽章靠右
              <li key={entry.id} aria-current={processing ? "step" : undefined}>
                <span className="integration-title truncate" title={titleOf(entry.task_id)}>
                  {titleOf(entry.task_id)}
                </span>
                <code className="integration-branch truncate" title={`${entry.branch} → ${entry.target_branch}`}>
                  {entry.branch} → {entry.target_branch}
                </code>
                <span className={processing ? "badge badge-accent" : "badge"}>
                  {INTEGRATION_STATUS_LABELS[entry.status]}
                  {processing && "（處理中）"}
                </span>
              </li>
            );
          })}
        </ol>
      )}
    </section>
  );
}

/** Task 的工作目錄：種類、路徑、worktree 的分支與整合目標、狀態；準備失敗時醒目顯示原因；有清理登記時顯示清理狀態。 */
function WorkdirStatus({
  workdir,
  runLabelOf,
  timeZone,
}: {
  workdir: WorkdirView;
  runLabelOf: (runId: string) => string;
  timeZone?: string;
}) {
  const failed = workdir.status === "failed";
  const removed = workdir.cleanup?.status === "cleaned";
  return (
    <div className={failed ? "task-workdir task-workdir-failed" : "task-workdir"} role="group" aria-label="工作目錄">
      <p>
        {workdir.kind === "worktree" ? "Worktree" : "輸出目錄"}：
        <code className={removed ? "task-workdir-removed" : undefined}>{workdir.path}</code>
        {removed && "（已移除）"}
      </p>
      {workdir.branch && (
        <p>
          分支 {workdir.branch}
          {workdir.base_branch && ` → 整合到 ${workdir.base_branch}`}
        </p>
      )}
      {!removed && <p>狀態：{failed ? "準備失敗" : "已就緒"}</p>}
      {failed && <p role="alert">工作目錄無法使用，不會派工：{workdir.failure}</p>}
      <p>
        {workdir.run_ids.length === 0
          ? "尚未有 Run 在此目錄執行"
          : `在此目錄執行過的 Run：${workdir.run_ids.map(runLabelOf).join("、")}`}
      </p>
      {workdir.cleanup && <CleanupStatusLine cleanup={workdir.cleanup} timeZone={timeZone} />}
    </div>
  );
}

/**
 * worktree 的清理狀態；與 Task 是否完成分開顯示，清理失敗不代表 Task 失敗。
 * 待清理且上次失敗時顯示原因與自動重試時間；清理失敗（不再自動重試）時醒目顯示原因。
 */
function CleanupStatusLine({ cleanup, timeZone }: { cleanup: WorktreeCleanup; timeZone?: string }) {
  return (
    <div
      className={cleanup.status === "failed" ? "task-workdir-cleanup task-workdir-cleanup-failed" : "task-workdir-cleanup"}
      role="status"
      aria-label="清理狀態"
    >
      <p>
        清理：<strong>{CLEANUP_STATUS_LABELS[cleanup.status]}</strong>
      </p>
      {cleanup.status === "pending" && cleanup.failure && <p>上次清理失敗：{cleanup.failure}</p>}
      {cleanup.status === "pending" && cleanup.failure && cleanup.next_attempt_at !== null && (
        <p>將於 {formatTimestamp(cleanup.next_attempt_at, timeZone)} 自動重試</p>
      )}
      {cleanup.status === "failed" && <p role="alert">清理失敗，worktree 已保留：{cleanup.failure}</p>}
      {cleanup.status === "cleaned" && cleanup.finished_at !== null && (
        <p>已清理（{formatTimestamp(cleanup.finished_at, timeZone)}）</p>
      )}
    </div>
  );
}

/** 開發 Task 最新的整合項目：狀態、未通過的原因與整合檢查（未通過與不完整分開）、已整合的合併 commit。 */
function IntegrationSummary({ entry, awaiting }: { entry: IntegrationEntry; awaiting: boolean }) {
  return (
    <div
      className={entry.status === "failed" ? "task-integration task-integration-failed" : "task-integration"}
      role="group"
      aria-label="整合狀態"
    >
      {awaiting && (
        <p>
          <strong>成果已通過驗證，等待整合</strong>{" "}
          <HelpTip label="等待整合">整合驗證通過、目標分支更新後才算完成</HelpTip>
        </p>
      )}
      <p>
        整合（{entry.branch} → {entry.target_branch}）：<strong>{INTEGRATION_STATUS_LABELS[entry.status]}</strong>
      </p>
      {entry.status === "integrated" && entry.merge_commit && <p>合併 commit {entry.merge_commit.slice(0, 8)}</p>}
      {entry.failure && <p>{entry.failure}</p>}
      <CheckResultList title="未通過的整合檢查" results={entry.checks.filter((result) => result.status === "failed")} />
      <CheckResultList title="不完整的整合檢查" results={entry.checks.filter((result) => result.status === "inconclusive")} />
    </div>
  );
}

/** 排隊中：順位與依類別分組的原因（名額已滿、額度、授權、相依、狀態不明、帳號、完全退出、工作目錄、Guard 擋下、重試退避）。 */
function QueuedStatus({ position, reasons }: { position: number | null; reasons: WaitReason[] }) {
  return (
    <div className="task-queued" role="group" aria-label="排隊原因">
      <p>
        <strong>{position === null ? "排隊中" : `排隊中（第 ${position} 位）`}</strong>
      </p>
      {reasons.length === 0 && <p>等待排程處理</p>}
      <ul>
        {WAIT_CATEGORY_LABELS.map(([category, label]) => {
          const inCategory = reasons.filter((reason) => reason.category === category);
          if (inCategory.length === 0) return null;
          return (
            <li key={category}>
              <strong>{label}</strong>
              <ul>
                {inCategory.map((reason, index) => (
                  <li key={`${reason.code}-${reason.profile_id ?? index}`}>
                    {reason.message}
                    {/* Guard 擋下時標明來源教訓，讓使用者知道是哪個教訓擋下的 */}
                    {reason.guard && <p className="task-guard-source">來源教訓：{reason.guard.lesson_title}</p>}
                  </li>
                ))}
              </ul>
            </li>
          );
        })}
      </ul>
    </div>
  );
}

/**
 * 最新 Run 的狀態列：狀態、使用的帳號、程序 pid、結束原因、lease 最後心跳；已要求取消時說明正在等程序停止。
 * 撤銷授權前已准入的 Run 沿用准入時的 revision（ADR-0003 R1-a），比目前舊時另外標示。
 */
function RunStatusLine({
  client,
  workspaceId,
  run,
  accountName,
  authorizationRevision,
  timeZone,
}: {
  client: TaskClient;
  workspaceId: string;
  run: RunView;
  accountName: string | null;
  authorizationRevision: number | null;
  timeZone?: string;
}) {
  const usesOlderAuthorization =
    isRunActive(run.status) &&
    run.admitted_at !== null &&
    run.authorization_revision !== null &&
    authorizationRevision !== null &&
    run.authorization_revision < authorizationRevision;
  return (
    <div className="task-run" role="group" aria-label="Run 狀態">
      <p>
        Run（第 {run.generation} 次）：<strong>{runStatusLabel(run)}</strong>
        {accountName && `，帳號 ${accountName}`}
      </p>
      {usesOlderAuthorization && (
        <p>
          沿用舊授權（第 {run.authorization_revision} 版，目前第 {authorizationRevision} 版）
        </p>
      )}
      {run.end_reason && !(run.end_reason in HANDOFF_END_REASON_LABELS) && <p>結束原因：{run.end_reason}</p>}
      {isRunActive(run.status) && run.cancel_requested_at !== null && <p>取消中，等待程序停止…</p>}
      {run.latest_progress && (
        <p>
          最新進度：{run.latest_progress.summary}（{formatTimestamp(run.latest_progress.reported_at, timeZone)}）
        </p>
      )}
      {run.latest_checkpoint && (
        <div className="task-checkpoint">
          <p>
            Checkpoint：{run.latest_checkpoint.summary}（{formatTimestamp(run.latest_checkpoint.saved_at, timeZone)}）
          </p>
          {run.latest_checkpoint.details !== "" && (
            <details>
              <summary>細節</summary>
              {run.latest_checkpoint.details}
            </details>
          )}
        </div>
      )}
      {run.blocked && (
        <p role="alert">
          受阻：{run.blocked.reason}（{formatTimestamp(run.blocked.reported_at, timeZone)}）
        </p>
      )}
      {/* 程序、心跳與 context receipt 是除錯用的技術資訊，預設收合 */}
      <details className="task-run-more">
        <summary>技術細節</summary>
        {run.process && <p>程序 pid {run.process.pid}</p>}
        {run.lease && <p>最後心跳：{formatTimestamp(run.lease.last_heartbeat_at, timeZone)}</p>}
        <ContextReceiptPanel client={client} workspaceId={workspaceId} runId={run.id} />
      </details>
    </div>
  );
}

const CONTEXT_KIND_LABELS: Record<ContextItemKind, string> = {
  contract: "Task 契約",
  handoff: "交接說明",
  checkpoint: "最近檢查點",
  decision: "決策",
  lesson: "教訓",
  messages: "未讀訊息摘要",
  evidence: "證據",
};

const OMITTED_REASON_LABELS: Record<string, string> = { budget_exceeded: "超出預算" };

/** 「類型：標題 v版本（tokens）」；沒有版本時省略版本。 */
const describeContextItem = (item: ContextItem) =>
  `${CONTEXT_KIND_LABELS[item.kind] ?? item.kind}：${item.title}${item.version === null ? "" : ` v${item.version}`}（${item.tokens} tokens）`;

type ContextState =
  | { state: "loading" }
  | { state: "ready"; context: RunContext }
  | { state: "missing" }
  | { state: "failed"; reason: string };

/** Run 的 context receipt（展開時才載入）：token 預算、依優先序載入與因預算省略的項目，以及實際交給 Worker 的 Prompt。 */
function ContextReceiptPanel({ client, workspaceId, runId }: { client: TaskClient; workspaceId: string; runId: string }) {
  const [open, setOpen] = useState(false);
  const [loaded, setLoaded] = useState<ContextState>({ state: "loading" });

  useEffect(() => {
    if (!open) return;
    let current = true;
    setLoaded({ state: "loading" });
    client.runContext(workspaceId, runId).then(
      (context) => current && setLoaded({ state: "ready", context }),
      (e) =>
        current &&
        // 本票之前建立的 Run 沒有 receipt，不算錯誤
        setLoaded(errorCode(e) === "context_receipt_not_found" ? { state: "missing" } : { state: "failed", reason: reasonOf(e) }),
    );
    return () => {
      current = false;
    };
  }, [client, workspaceId, runId, open]);

  const items = loaded.state === "ready" ? loaded.context.receipt.items : [];
  const loadedItems = items.filter((item) => item.status === "loaded");
  const omittedItems = items.filter((item) => item.status === "omitted");
  return (
    <div className="task-run-context" role="group" aria-label="Context receipt">
      <button type="button" aria-expanded={open} onClick={() => setOpen(!open)}>
        Context receipt
      </button>
      {open && loaded.state === "loading" && <p>載入中…</p>}
      {open && loaded.state === "missing" && <p>這個 Run 建立得較早，沒有 context receipt</p>}
      {open && loaded.state === "failed" && <p role="alert">無法取得 context receipt：{loaded.reason}</p>}
      {open && loaded.state === "ready" && (
        <>
          <p>
            已用 {loaded.context.receipt.used_tokens} / {loaded.context.receipt.budget_tokens} tokens
          </p>
          <div role="group" aria-label="已載入項目">
            <strong>已載入（{loadedItems.length}）</strong>
            <ul>
              {loadedItems.map((item, index) => (
                <li key={index}>{describeContextItem(item)}</li>
              ))}
            </ul>
          </div>
          {omittedItems.length > 0 && (
            <div role="group" aria-label="省略項目">
              <strong>省略（{omittedItems.length}）</strong>
              <ul>
                {omittedItems.map((item, index) => (
                  <li key={index}>
                    {describeContextItem(item)}
                    {item.omitted_reason !== null && `，${OMITTED_REASON_LABELS[item.omitted_reason] ?? item.omitted_reason}`}
                  </li>
                ))}
              </ul>
            </div>
          )}
          <details>
            <summary>實際 Prompt</summary>
            <pre>{loaded.context.prompt}</pre>
          </details>
        </>
      )}
    </div>
  );
}

/**
 * Task 最新的跨帳號交接（spec D-21）：階段、從哪個帳號到哪個帳號與原因、延續的 checkpoint；
 * 確認後列出接手者確認的三欄，放棄時顯示原因；等待資源時說明預計重新評估的時間、依據與等待原因。
 */
function HandoffSummary({
  handoff,
  profileNameOf,
  timeZone,
}: {
  handoff: Handoff;
  profileNameOf: (profileId: string) => string;
  timeZone?: string;
}) {
  const { checkpoint, confirmation } = handoff;
  return (
    <div className={`task-handoff task-handoff-${handoff.phase}`} role="group" aria-label="交接狀態">
      <p>
        交接：<strong>{HANDOFF_PHASE_LABELS[handoff.phase]}</strong>
      </p>
      <p>
        從 {profileNameOf(handoff.from_profile_id)}{" "}
        {/* 突然限流而所有帳號都受限時尚未選定承接帳號（空字串），不顯示空的帳號名 */}
        {handoff.to_profile_id === "" ? "交接，尚未選定承接帳號" : `交接到 ${profileNameOf(handoff.to_profile_id)}`}（
        {GATE_REASON_LABELS[handoff.reason] ?? handoff.reason}）
      </p>
      {checkpoint && (
        <p>
          交接 checkpoint：{checkpoint.summary}（{formatTimestamp(checkpoint.saved_at, timeZone)}）
        </p>
      )}
      {handoff.phase === "confirmed" && confirmation && (
        <ul>
          <li>已完成：{confirmation.completed}</li>
          <li>未完成：{confirmation.incomplete}</li>
          <li>未知副作用：{confirmation.unknown_side_effects}</li>
        </ul>
      )}
      {handoff.phase === "abandoned" && handoff.abandoned_reason && <p>放棄原因：{handoff.abandoned_reason}</p>}
      {handoff.phase === "waiting_resource" && <WaitingResourceSummary handoff={handoff} timeZone={timeZone} />}
    </div>
  );
}

/** 等待資源：預計重新評估的時間、依據（帳號重置時間後 1 分鐘，或未知時退避輪詢）與最近一次評估的等待原因。 */
function WaitingResourceSummary({ handoff, timeZone }: { handoff: Handoff; timeZone?: string }) {
  // 舊版 Runtime 沒有等待欄位，視為 null／0／空陣列
  const nextCheckAt = handoff.next_check_at ?? null;
  const resetAt = handoff.wait_reset_at ?? null;
  const reasons = handoff.wait_reasons ?? [];
  return (
    <div className="task-handoff-waiting" role="group" aria-label="等待資源">
      {nextCheckAt !== null && <p>預計 {formatTimestamp(nextCheckAt, timeZone)} 重新評估</p>}
      <p>
        {resetAt !== null
          ? `依帳號重置時間（${formatTimestamp(resetAt, timeZone)}）後 1 分鐘`
          : `重置時間未知，退避輪詢（第 ${(handoff.wait_attempts ?? 0) + 1} 次評估）`}
      </p>
      {reasons.length > 0 && (
        <ul>
          {reasons.map((reason, index) => (
            <li key={`${reason.code}-${reason.profile_id ?? index}`}>{reason.message}</li>
          ))}
        </ul>
      )}
    </div>
  );
}

/** 最新驗證紀錄：未通過或不完整時列出未通過、不完整的檢查與證據（預設收合）。 */
function VerificationSummary({ record, timeZone }: { record: VerificationRecord; timeZone?: string }) {
  const decidedAt = formatTimestamp(record.decided_at, timeZone);
  if (record.outcome === "passed") {
    return (
      <div role="group" aria-label="驗證紀錄">
        <p>
          <strong>已通過驗證（{record.method === "checks" ? "自動檢查" : "手動接受"}）</strong>，{decidedAt}
        </p>
      </div>
    );
  }
  // 不完整是證據不足或環境問題，不是產品缺陷：與未通過分開標示
  const heading = record.outcome === "inconclusive" ? "上次驗證不完整" : "上次驗證未通過";
  return (
    <div className="task-verification-failed" role="group" aria-label="驗證紀錄">
      <p>
        <strong>{heading}</strong>（第 {record.generation} 次 Run，{decidedAt}）
      </p>
      {record.note && <p>{record.note}</p>}
      <CheckResultList title="未通過的檢查" results={record.checks.filter((result) => result.status === "failed")} />
      <CheckResultList title="不完整的檢查" results={record.checks.filter((result) => result.status === "inconclusive")} />
    </div>
  );
}

/** 檢查結果與證據的清單（預設收合）；沒有項目時不顯示。 */
function CheckResultList({ title, results }: { title: string; results: CheckResult[] }) {
  if (results.length === 0) return null;
  return (
    <details>
      <summary>
        {title}（{results.length}）
      </summary>
      <ul>
        {results.map((result, index) => (
          <li key={index}>
            {describeCheck(result.check)}：{result.detail}
          </li>
        ))}
      </ul>
    </details>
  );
}

/** 有任何計數用過或已用盡才顯示預算，避免卡片雜訊。 */
const isBudgetUsed = (budget: BudgetView) => budget.exhausted !== null || budget.counters.some((counter) => counter.used > 0);

/** 修正與重試預算（spec D-16）：三種計數、基礎設施重試的退避期限；用盡時 Task 已阻擋，提示到裁定中心。 */
function BudgetStatus({ budget, timeZone }: { budget: BudgetView; timeZone?: string }) {
  const exhaustedLabel = budget.counters.find((counter) => counter.kind === budget.exhausted)?.label ?? budget.exhausted;
  return (
    <div
      className={budget.exhausted === null ? "task-budget" : "task-budget task-budget-exhausted"}
      role="group"
      aria-label="修正與重試預算"
    >
      <ul>
        {budget.counters.map((counter) => (
          <li key={counter.kind}>
            {counter.label} {counter.used}/{counter.limit}
          </li>
        ))}
      </ul>
      {budget.retry_not_before !== null && <p>下次重試不早於 {formatTimestamp(budget.retry_not_before, timeZone)}</p>}
      {budget.exhausted !== null && <p role="alert">{exhaustedLabel}預算已用完，請到「裁定」決定下一步</p>}
    </div>
  );
}

type VerificationState = { state: "loading" } | { state: "ready"; view: VerificationView } | { state: "failed"; reason: string };

interface VerificationPanelProps {
  client: TaskClient;
  workspaceId: string;
  taskId: string;
  onAccepted: () => void;
}

/**
 * verifying 的 Task：Worker 成功結束不代表完成，要通過確定性檢查或由使用者手動接受。
 * 展開後顯示宣告的檢查與成果 manifest；接受時把 Runtime 給的依據原樣送回。
 */
function VerificationPanel({ client, workspaceId, taskId, onAccepted }: VerificationPanelProps) {
  const [open, setOpen] = useState(false);
  const [loaded, setLoaded] = useState<VerificationState>({ state: "loading" });
  const [note, setNote] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const reload = useCallback(async () => {
    try {
      setLoaded({ state: "ready", view: await client.taskVerification(workspaceId, taskId) });
    } catch (e) {
      setLoaded({ state: "failed", reason: reasonOf(e) });
    }
  }, [client, workspaceId, taskId]);

  useEffect(() => {
    if (!open) return;
    setLoaded({ state: "loading" });
    void reload();
  }, [open, reload]);

  const accept = async (basis: AcceptanceBasis) => {
    setBusy(true);
    setError(null);
    try {
      await client.acceptTask(workspaceId, taskId, { basis, note: note.trim() === "" ? null : note.trim() });
      onAccepted();
    } catch (e) {
      // 依據過期（409）等失敗：顯示 Runtime 的說明，並重新讀取最新的驗證資料
      setError(reasonOf(e));
      await reload();
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="task-verification" role="group" aria-label="成果驗證">
      {/* 收合時是卡片的狀態列＋主要動作；「?」說明在展開後才出現，卡片上只留兩個按鈕 */}
      <p className="task-status">
        <span className="badge badge-accent">驗證中</span>
        <button type="button" className="primary-action btn-sm" aria-expanded={open} onClick={() => setOpen(!open)}>
          檢視驗證
        </button>
        {open && <HelpTip label="驗證中">Worker 已結束，成果要通過自動檢查或由你手動接受才算完成</HelpTip>}
      </p>
      {open && loaded.state === "loading" && <p>載入中…</p>}
      {open && loaded.state === "failed" && <p role="alert">無法取得驗證資料：{loaded.reason}</p>}
      {open && loaded.state === "ready" && (
        <VerificationDetail
          view={loaded.view}
          note={note}
          busy={busy}
          onNoteChange={setNote}
          onAccept={(basis) => void accept(basis)}
        />
      )}
      {error && <p role="alert">{error}</p>}
    </div>
  );
}

interface VerificationDetailProps {
  view: VerificationView;
  note: string;
  busy: boolean;
  onNoteChange: (note: string) => void;
  onAccept: (basis: AcceptanceBasis) => void;
}

/** 驗證資料：自動檢查的說明、宣告的檢查、成果 manifest 與手動接受。 */
function VerificationDetail({ view, note, busy, onNoteChange, onAccept }: VerificationDetailProps) {
  const { basis } = view;
  return (
    <>
      <p>
        {view.automatic
          ? `正在執行 ${view.checks.length} 項確定性檢查，全部通過後自動完成`
          : "沒有自動檢查，需要你手動接受成果"}
      </p>
      {view.checks.length > 0 && (
        <div>
          <strong>宣告的檢查</strong>
          <ul>
            {view.checks.map((check, index) => (
              <li key={index}>{describeCheck(check)}</li>
            ))}
          </ul>
        </div>
      )}
      <div>
        <strong>成果 manifest</strong>
        {view.artifacts.length === 0 ? (
          <p>沒有登記的成果</p>
        ) : (
          <ul>
            {view.artifacts.map((entry) => (
              <li key={entry.artifact_id}>
                {entry.name} v{entry.version}，SHA-256 {shortHash(entry.sha256)}
              </li>
            ))}
          </ul>
        )}
      </div>
      {view.integration && (
        <p>{`最新整合項目：${INTEGRATION_STATUS_LABELS[view.integration.status]}（第 ${view.integration.generation} 次 Run）`}</p>
      )}
      {view.status !== "verifying" && <p>Task 目前是「{statusLabel(view.status)}」，不能接受成果</p>}
      <label>
        附註（選填）
        <input value={note} onChange={(e) => onNoteChange(e.target.value)} />
      </label>
      <div className="task-actions">
        <button
          type="button"
          disabled={busy || basis === null || view.status !== "verifying"}
          onClick={() => basis !== null && onAccept(basis)}
        >
          接受成果
        </button>
      </div>
    </>
  );
}

interface FormProps {
  client: TaskClient;
  workspaceId: string;
  board: TaskBoardData;
  /** 有值表示編輯該 Task 的契約；沒有則是建立新的 Task。 */
  task: TaskView | null;
  onDone: () => void;
  onClose: () => void;
  /** 建立 Task 前先建立了新的 Goal：看板需要重新載入才看得到它。 */
  onGoalCreated: () => void;
  /** 對話框關閉後焦點回到的按鈕；已不在畫面上時退回開啟前的焦點。 */
  returnFocus: HTMLElement | null;
}

const NEW_GOAL = "__new__";

/**
 * 建立 Task 與編輯契約共用的表單，以 modal 對話框呈現（MASTER 5.6），不把看板推下去：
 * 開啟時焦點移到第一個欄位，Esc、「關閉」與標題列的關閉鈕都直接關閉（與改版前相同，不另外確認），
 * 關閉後焦點回到開啟它的按鈕。
 */
function TaskForm({ client, workspaceId, board, task, onDone, onClose, onGoalCreated, returnFocus }: FormProps) {
  const dialog = useRef<HTMLDialogElement>(null);
  const [draft, setDraft] = useState<ContractDraft>(() => (task ? draftFromContract(task.contract) : emptyDraft()));
  const [goalChoice, setGoalChoice] = useState(board.goals[0]?.id ?? NEW_GOAL);
  const [goalTitle, setGoalTitle] = useState("");
  const [title, setTitle] = useState("");
  const [kind, setKind] = useState<TaskKind>("research");
  const [note, setNote] = useState("");
  const [messages, setMessages] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    const element = dialog.current;
    if (element === null) return;
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    // jsdom 沒有 showModal：退回以 open 屬性顯示（焦點與 Esc 由下方自行處理，行為相同）
    if (typeof element.showModal === "function") element.showModal();
    else element.setAttribute("open", "");
    element.querySelector<HTMLElement>("input, select, textarea")?.focus();
    return () => {
      if (typeof element.close === "function" && element.open) element.close();
      const target = returnFocus?.isConnected ? returnFocus : previous;
      if (target?.isConnected) target.focus();
    };
    // 只在開啟時執行一次；returnFocus 在同一次開啟中不會改變
  }, []);

  const closeOnEscape = (event: KeyboardEvent) => {
    if (event.key !== "Escape") return;
    event.preventDefault();
    onClose();
  };

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    // 儲存中按鈕只標 aria-disabled（保留焦點），重按在這裡擋下
    if (busy) return;
    const problems: string[] = [];
    if (!task && title.trim() === "") problems.push("請輸入 Task 標題");
    if (!task && goalChoice === NEW_GOAL && goalTitle.trim() === "") problems.push("請輸入 Goal 名稱");
    const { contract, errors } = buildContract(draft);
    problems.push(...errors);
    if (problems.length > 0) {
      setMessages(problems);
      return;
    }
    setBusy(true);
    setMessages([]);
    try {
      if (task) {
        await client.updateTaskContract(workspaceId, task.id, contract, note.trim() === "" ? undefined : note.trim());
      } else {
        let goalId = goalChoice;
        if (goalChoice === NEW_GOAL) {
          const goal: Goal = await client.createGoal(workspaceId, goalTitle.trim());
          // Goal 已建立：之後失敗重送時改用它，避免重複建立
          goalId = goal.id;
          setGoalChoice(goal.id);
          onGoalCreated();
        }
        await client.createTask(workspaceId, { goalId, title: title.trim(), kind, contract });
      }
      onDone();
    } catch (e) {
      setMessages([reasonOf(e)]);
    } finally {
      setBusy(false);
    }
  };

  const heading = task ? `編輯契約：${task.title}` : "建立 Task";
  return (
    <dialog
      ref={dialog}
      className="task-form task-dialog"
      aria-label={heading}
      // 瀏覽器原生的 Esc（cancel 事件）也交給 React 狀態關閉，不讓 dialog 自己關掉
      onCancel={(event) => {
        event.preventDefault();
        onClose();
      }}
      onKeyDown={closeOnEscape}
    >
      {/* 標題列：動作名稱 + Task 標題（完整換行，不截斷） */}
      <header className="task-dialog-header">
        <div className="task-dialog-title">
          <h3>{task ? "編輯契約" : "建立 Task"}</h3>
          {task && <p>{task.title}</p>}
        </div>
        <button type="button" className="btn-quiet btn-icon" aria-label="關閉對話框" onClick={onClose}>
          ×
        </button>
      </header>
      <form className="task-dialog-form" onSubmit={(event) => void submit(event)}>
        <div className="task-dialog-body">
          {!task && (
            <>
              <label>
                Goal
                <select className="task-form-select" value={goalChoice} onChange={(e) => setGoalChoice(e.target.value)}>
                  {board.goals.map((goal) => (
                    <option key={goal.id} value={goal.id}>
                      {optionLabel(goal.title)}
                    </option>
                  ))}
                  <option value={NEW_GOAL}>新增 Goal…</option>
                </select>
              </label>
              {goalChoice === NEW_GOAL && (
                <label>
                  新 Goal 名稱
                  <input value={goalTitle} onChange={(e) => setGoalTitle(e.target.value)} />
                </label>
              )}
              <label>
                Task 標題
                <input value={title} onChange={(e) => setTitle(e.target.value)} />
              </label>
              <label>
                種類
                <select className="contract-mode" value={kind} onChange={(e) => setKind(e.target.value as TaskKind)}>
                  {TASK_KIND_LABELS.map(([value, label]) => (
                    <option key={value} value={value}>
                      {label}
                    </option>
                  ))}
                </select>
              </label>
            </>
          )}
          <ContractEditor draft={draft} onChange={setDraft} capabilities={board.capabilities} />
          {task && (
            <label>
              變更說明（選填）
              <input value={note} onChange={(e) => setNote(e.target.value)} />
            </label>
          )}
        </div>
        <footer className="task-dialog-footer">
          {messages.length > 0 && (
            <div role="alert" className="task-dialog-errors">
              {messages.map((message) => (
                <p key={message}>{message}</p>
              ))}
            </div>
          )}
          <div className="form-actions">
            <button type="button" onClick={onClose}>
              關閉
            </button>
            {/* 不用 disabled：按鈕被停用時瀏覽器會把焦點移到 body，離開對話框 */}
            <button type="submit" aria-disabled={busy}>
              {task ? "儲存契約" : "建立"}
            </button>
          </div>
        </footer>
      </form>
    </dialog>
  );
}
