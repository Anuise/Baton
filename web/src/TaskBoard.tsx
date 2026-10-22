import { useCallback, useEffect, useState, type FormEvent } from "react";

import { buildContract, ContractEditor, draftFromContract, emptyDraft, type ContractDraft } from "./ContractEditor";
import { formatTimestamp } from "./time";
import type {
  AcceptanceBasis,
  Goal,
  IntegrationEntry,
  IntegrationStatus,
  MissingItem,
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
  const [panel, setPanel] = useState<{ kind: "create" } | { kind: "edit"; task: TaskView } | null>(null);
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
      <h2>工作面板：{workspace.name}</h2>
      <div>
        <button type="button" onClick={() => setPanel({ kind: "create" })}>
          建立 Task
        </button>
      </div>

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
        />
      )}

      <div className="task-columns">
        {COLUMNS.map(([status, label]) => {
          const tasks = board.tasks.filter((t) => t.status === status);
          return (
            <section key={status} className="task-column" aria-label={`${label}（${tasks.length}）`}>
              <h3>
                {label}（{tasks.length}）
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
                  accountName={accountNameOf(overview, board.runs, task.id)}
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
                  onEdit={() => setPanel({ kind: "edit", task })}
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
  /** 最新 Run 使用的帳號名稱。 */
  accountName: string | null;
  /** 在佇列中等待名額時的順位（未知為 null）與原因；不在佇列為 null。 */
  queued: { position: number | null; reasons: WaitReason[] } | null;
  timeZone?: string;
  error: string | null;
  onReady: () => void;
  onCancel: () => void;
  onDispatch: () => void;
  onCancelRun: (runId: string) => void;
  onEdit: () => void;
  onAddDependency: (dependsOn: string) => Promise<boolean>;
  /** 手動接受成功：看板需要重新載入。 */
  onAccepted: () => void;
}

type HistoryState = { state: "loading" } | { state: "ready"; contracts: TaskContract[] } | { state: "failed"; reason: string };

function TaskCard({ client, workspaceId, task, goalTitle, titleOf, others, run, verification, workdir, runLabelOf, integration, accountName, queued, timeZone, error, onReady, onCancel, onDispatch, onCancelRun, onEdit, onAddDependency, onAccepted }: CardProps) {
  const [addingDependency, setAddingDependency] = useState(false);
  const [dependsOn, setDependsOn] = useState("");
  const [historyOpen, setHistoryOpen] = useState(false);
  const [history, setHistory] = useState<HistoryState>({ state: "loading" });

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

  const addDependency = async () => {
    if (await onAddDependency(dependsOn)) {
      setAddingDependency(false);
      setDependsOn("");
    }
  };

  return (
    <article className="task-card" aria-label={task.title}>
      <h4>{task.title}</h4>
      <p className="task-goal">Goal：{goalTitle}</p>
      <p className="task-kind">種類：{kindLabel(task.kind)}</p>
      <p className="task-contract-version">契約版本 v{task.contract_version}</p>
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

      {workdir && <WorkdirStatus workdir={workdir} runLabelOf={runLabelOf} />}

      {run && <RunStatusLine run={run} accountName={accountName} timeZone={timeZone} />}

      {verification && <VerificationSummary record={verification} timeZone={timeZone} />}

      {integration && <IntegrationSummary entry={integration} awaiting={awaitingIntegration} />}

      {task.status === "verifying" && !awaitingIntegration && (
        <VerificationPanel client={client} workspaceId={workspaceId} taskId={task.id} onAccepted={onAccepted} />
      )}

      <div className="task-actions">
        {task.status === "ready" && (
          <button type="button" onClick={onDispatch}>
            派工
          </button>
        )}
        {run && isRunActive(run.status) && run.cancel_requested_at === null && (
          <button type="button" onClick={() => onCancelRun(run.id)}>
            取消 Run
          </button>
        )}
        {task.status === "pending" && (
          <button type="button" onClick={onReady}>
            標記為可派工
          </button>
        )}
        {!closed && (
          <>
            <button type="button" onClick={onEdit}>
              編輯契約
            </button>
            <button type="button" onClick={() => setAddingDependency(true)}>
              新增相依
            </button>
            <button type="button" onClick={onCancel}>
              取消
            </button>
          </>
        )}
        <button type="button" aria-expanded={historyOpen} onClick={() => setHistoryOpen(!historyOpen)}>
          契約版本歷史
        </button>
      </div>

      {addingDependency && (
        <div className="task-dependency-form">
          <label>
            相依的 Task
            <select value={dependsOn} onChange={(e) => setDependsOn(e.target.value)}>
              <option value="">選擇 Task…</option>
              {others.map((other) => (
                <option key={other.id} value={other.id}>
                  {other.title}
                </option>
              ))}
            </select>
          </label>
          <button type="button" disabled={dependsOn === ""} onClick={() => void addDependency()}>
            確定新增
          </button>
          <button type="button" onClick={() => setAddingDependency(false)}>
            關閉
          </button>
        </div>
      )}

      {error && <p role="alert">{error}</p>}

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
              <li key={entry.id} aria-current={processing ? "step" : undefined}>
                {titleOf(entry.task_id)}（{entry.branch} → {entry.target_branch}）：{INTEGRATION_STATUS_LABELS[entry.status]}
                {processing && <strong>（處理中）</strong>}
              </li>
            );
          })}
        </ol>
      )}
    </section>
  );
}

/** Task 的工作目錄：種類、路徑、worktree 的分支與整合目標、狀態；準備失敗時醒目顯示原因。 */
function WorkdirStatus({ workdir, runLabelOf }: { workdir: WorkdirView; runLabelOf: (runId: string) => string }) {
  const failed = workdir.status === "failed";
  return (
    <div className={failed ? "task-workdir task-workdir-failed" : "task-workdir"} role="group" aria-label="工作目錄">
      <p>
        {workdir.kind === "worktree" ? "Worktree" : "輸出目錄"}：<code>{workdir.path}</code>
      </p>
      {workdir.branch && (
        <p>
          分支 {workdir.branch}
          {workdir.base_branch && ` → 整合到 ${workdir.base_branch}`}
        </p>
      )}
      <p>狀態：{failed ? "準備失敗" : "已就緒"}</p>
      {failed && <p role="alert">工作目錄無法使用，不會派工：{workdir.failure}</p>}
      <p>
        {workdir.run_ids.length === 0
          ? "尚未有 Run 在此目錄執行"
          : `在此目錄執行過的 Run：${workdir.run_ids.map(runLabelOf).join("、")}`}
      </p>
    </div>
  );
}

/** 開發 Task 最新的整合項目：狀態、未通過的原因與整合檢查、已整合的合併 commit。 */
function IntegrationSummary({ entry, awaiting }: { entry: IntegrationEntry; awaiting: boolean }) {
  const failedChecks = entry.checks.filter((result) => result.status === "failed");
  return (
    <div
      className={entry.status === "failed" ? "task-integration task-integration-failed" : "task-integration"}
      role="group"
      aria-label="整合狀態"
    >
      {awaiting && (
        <p>
          <strong>成果已通過驗證，等待整合</strong>：整合驗證通過、目標分支更新後才算完成
        </p>
      )}
      <p>
        整合（{entry.branch} → {entry.target_branch}）：<strong>{INTEGRATION_STATUS_LABELS[entry.status]}</strong>
      </p>
      {entry.status === "integrated" && entry.merge_commit && <p>合併 commit {entry.merge_commit.slice(0, 8)}</p>}
      {entry.failure && <p>{entry.failure}</p>}
      {failedChecks.length > 0 && (
        <details>
          <summary>未通過的整合檢查（{failedChecks.length}）</summary>
          <ul>
            {failedChecks.map((result, index) => (
              <li key={index}>
                {describeCheck(result.check)}：{result.detail}
              </li>
            ))}
          </ul>
        </details>
      )}
    </div>
  );
}

/** 排隊中：順位與依類別分組的原因（名額已滿、額度、授權、相依、狀態不明、帳號、完全退出、工作目錄、Guard 擋下）。 */
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

/** 最新 Run 的狀態列：狀態、使用的帳號、程序 pid、結束原因、lease 最後心跳；已要求取消時說明正在等程序停止。 */
function RunStatusLine({ run, accountName, timeZone }: { run: RunView; accountName: string | null; timeZone?: string }) {
  return (
    <div className="task-run" role="group" aria-label="Run 狀態">
      <p>
        Run（第 {run.generation} 次）：<strong>{RUN_STATUS_LABELS[run.status]}</strong>
        {accountName && `，帳號 ${accountName}`}
        {run.process && `，程序 pid ${run.process.pid}`}
      </p>
      {run.end_reason && <p>結束原因：{run.end_reason}</p>}
      {run.lease && <p>最後心跳：{formatTimestamp(run.lease.last_heartbeat_at, timeZone)}</p>}
      {isRunActive(run.status) && run.cancel_requested_at !== null && <p>取消中，等待程序停止…</p>}
      {run.latest_progress && (
        <p>
          最新進度：{run.latest_progress.summary}（{formatTimestamp(run.latest_progress.reported_at, timeZone)}）
        </p>
      )}
      {run.latest_checkpoint && (
        <div>
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
    </div>
  );
}

/** 最新驗證紀錄：未通過時列出未通過的檢查與證據（預設收合）。 */
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
  const failed = record.checks.filter((result) => result.status === "failed");
  return (
    <div className="task-verification-failed" role="group" aria-label="驗證紀錄">
      <p>
        <strong>上次驗證未通過</strong>（第 {record.generation} 次 Run，{decidedAt}）
      </p>
      {record.note && <p>{record.note}</p>}
      {failed.length > 0 && (
        <details>
          <summary>未通過的檢查（{failed.length}）</summary>
          <ul>
            {failed.map((result, index) => (
              <li key={index}>
                {describeCheck(result.check)}：{result.detail}
              </li>
            ))}
          </ul>
        </details>
      )}
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
      <p>
        <strong>驗證中</strong>：Worker 已結束，成果尚待確定性檢查或手動接受
      </p>
      <div className="task-actions">
        <button type="button" aria-expanded={open} onClick={() => setOpen(!open)}>
          檢視驗證
        </button>
      </div>
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
          ? `Runtime 正在執行 ${view.checks.length} 項確定性檢查，全部通過後自動完成`
          : "Runtime 不會自動判定，需要手動接受成果"}
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
}

const NEW_GOAL = "__new__";

/** 建立 Task 與編輯契約共用的表單。 */
function TaskForm({ client, workspaceId, board, task, onDone, onClose, onGoalCreated }: FormProps) {
  const [draft, setDraft] = useState<ContractDraft>(() => (task ? draftFromContract(task.contract) : emptyDraft()));
  const [goalChoice, setGoalChoice] = useState(board.goals[0]?.id ?? NEW_GOAL);
  const [goalTitle, setGoalTitle] = useState("");
  const [title, setTitle] = useState("");
  const [kind, setKind] = useState<TaskKind>("research");
  const [note, setNote] = useState("");
  const [messages, setMessages] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);

  const submit = async (event: FormEvent) => {
    event.preventDefault();
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

  return (
    <section className="task-form" aria-label={task ? `編輯契約：${task.title}` : "建立 Task"}>
      <h3>{task ? `編輯契約：${task.title}` : "建立 Task"}</h3>
      <form onSubmit={(event) => void submit(event)}>
        {!task && (
          <>
            <label>
              Goal
              <select value={goalChoice} onChange={(e) => setGoalChoice(e.target.value)}>
                {board.goals.map((goal) => (
                  <option key={goal.id} value={goal.id}>
                    {goal.title}
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
              <select value={kind} onChange={(e) => setKind(e.target.value as TaskKind)}>
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
        <div className="task-actions">
          <button type="submit" disabled={busy}>
            {task ? "儲存契約" : "建立"}
          </button>
          <button type="button" onClick={onClose}>
            關閉
          </button>
        </div>
        {messages.length > 0 && (
          <div role="alert">
            {messages.map((message) => (
              <p key={message}>{message}</p>
            ))}
          </div>
        )}
      </form>
    </section>
  );
}
