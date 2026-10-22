import { useCallback, useEffect, useState, type FormEvent } from "react";

import { buildContract, ContractEditor, draftFromContract, emptyDraft, type ContractDraft } from "./ContractEditor";
import { formatTimestamp } from "./time";
import type {
  Goal,
  MissingItem,
  RunStatus,
  RunView,
  TaskBoard as TaskBoardData,
  TaskClient,
  TaskContract,
  TaskStatus,
  TaskView,
  Workspace,
} from "./runtimeClient";

type ViewState = { state: "loading" } | { state: "ready"; board: TaskBoardData } | { state: "failed"; reason: string };

interface Props {
  client: TaskClient;
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

const RUN_STATUS_LABELS: Record<RunStatus, string> = {
  queued: "排隊中",
  starting: "啟動中",
  running: "執行中",
  succeeded: "成功",
  failed: "失敗",
  interrupted: "中斷",
  cancelled: "已取消",
};

/** Run 仍在進行（尚未結束）。 */
const isRunActive = (status: RunStatus) => status === "queued" || status === "starting" || status === "running";

/** 同一個 Task 最新的 Run：generation 最大者。 */
function latestRun(runs: RunView[], taskId: string): RunView | null {
  return runs
    .filter((r) => r.task_id === taskId)
    .reduce<RunView | null>((latest, r) => (latest === null || r.generation > latest.generation ? r : latest), null);
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

/**
 * 工作面板：依狀態分欄顯示 Workspace 內的 Task，建立 Task、編輯六項最小派工契約、
 * 標記為可派工。缺少的項目直接列在 pending 的卡片上。
 */
export function TaskBoard({ client, workspace, timeZone }: Props) {
  const [loaded, setLoaded] = useState<ViewState>({ state: "loading" });
  const [panel, setPanel] = useState<{ kind: "create" } | { kind: "edit"; task: TaskView } | null>(null);
  const [cardError, setCardError] = useState<{ taskId: string; message: string } | null>(null);

  const load = useCallback(async () => {
    try {
      setLoaded({ state: "ready", board: await client.getTaskBoard(workspace.id) });
    } catch (e) {
      setLoaded({ state: "failed", reason: reasonOf(e) });
    }
  }, [client, workspace.id]);

  useEffect(() => {
    setLoaded({ state: "loading" });
    setPanel(null);
    setCardError(null);
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
  const { board } = loaded;
  const titleOf = (taskId: string) => board.tasks.find((t) => t.id === taskId)?.title ?? taskId;

  /** 執行卡片上的操作：失敗時把 Runtime 的說明顯示在該卡片上；無論成敗都重新載入看板。 */
  const act = async (task: TaskView, action: () => Promise<unknown>): Promise<boolean> => {
    setCardError(null);
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
                  timeZone={timeZone}
                  error={cardError?.taskId === task.id ? cardError.message : null}
                  onReady={() => void act(task, () => client.markTaskReady(workspace.id, task.id))}
                  onCancel={() => void act(task, () => client.cancelTask(workspace.id, task.id))}
                  onDispatch={() => void act(task, () => client.dispatchTask(workspace.id, task.id))}
                  onCancelRun={(runId) => void act(task, () => client.cancelRun(workspace.id, runId))}
                  onEdit={() => setPanel({ kind: "edit", task })}
                  onAddDependency={(dependsOn) => act(task, () => client.addTaskDependency(workspace.id, task.id, dependsOn))}
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
  timeZone?: string;
  error: string | null;
  onReady: () => void;
  onCancel: () => void;
  onDispatch: () => void;
  onCancelRun: (runId: string) => void;
  onEdit: () => void;
  onAddDependency: (dependsOn: string) => Promise<boolean>;
}

type HistoryState = { state: "loading" } | { state: "ready"; contracts: TaskContract[] } | { state: "failed"; reason: string };

function TaskCard({ client, workspaceId, task, goalTitle, titleOf, others, run, timeZone, error, onReady, onCancel, onDispatch, onCancelRun, onEdit, onAddDependency }: CardProps) {
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

      {run && <RunStatusLine run={run} timeZone={timeZone} />}

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

/** 最新 Run 的狀態列：狀態、程序 pid、結束原因、lease 最後心跳；已要求取消時說明正在等程序停止。 */
function RunStatusLine({ run, timeZone }: { run: RunView; timeZone?: string }) {
  return (
    <div className="task-run" role="group" aria-label="Run 狀態">
      <p>
        Run（第 {run.generation} 次）：<strong>{RUN_STATUS_LABELS[run.status]}</strong>
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
        await client.createTask(workspaceId, { goalId, title: title.trim(), contract });
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
