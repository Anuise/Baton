import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import {
  accountScheduling,
  budgetView,
  handoff,
  integrationEntry,
  REPORT_SHA256,
  runContextView,
  schedulingOverview,
  verificationRecord,
  verificationView,
  workdirView,
  worktreeCleanup,
} from "./fixtures";
import { TaskBoard, type TaskBoardClient } from "./TaskBoard";
import {
  CONTRACT_FIELDS,
  type AccountScheduling,
  type BudgetView,
  type Capability,
  type ContractInput,
  type Goal,
  type Handoff,
  type IntegrationEntry,
  type MissingItem,
  type QueueEntry,
  type RunContext,
  type RunStatus,
  type RunView,
  type TaskBoard as TaskBoardData,
  type TaskContract,
  type TaskView,
  type VerificationRecord,
  type VerificationView,
  type WaitReason,
  type WorkdirView,
  type Workspace,
} from "./runtimeClient";

const workspace: Workspace = {
  id: "ws_1",
  name: "筆記",
  path: "C:\\work\\notes",
  status: "active",
  version: 1,
  created_at: 1_700_000_000_000,
  archived_at: null,
  model_overrides: { controller: null, worker: null, reviewer: null },
};

/** 2023-11-14 22:13:20 UTC。 */
const T0 = 1_700_000_000_000;

const CAPABILITIES: TaskBoardData["capabilities"] = [
  { capability: "read_files", label: "讀取檔案", high_risk: false },
  { capability: "run_build_test", label: "建置與測試", high_risk: false },
  { capability: "deploy", label: "部署", high_risk: true },
];

const FIELD_NAMES = {
  objective: "目標",
  inputs: "輸入與依據",
  scope: "範圍與限制",
  deliverables: "預期成果",
  done_criteria: "完成與驗證條件",
  blocker_handling: "阻礙處理",
} as const;

const goal = (id: string, title: string): Goal => ({ id, workspace_id: workspace.id, title, version: 1, created_at: T0 });

function contract(taskId: string, overrides: Partial<TaskContract> = {}): TaskContract {
  const empty = Object.fromEntries(CONTRACT_FIELDS.map((name) => [name, null]));
  return {
    ...empty,
    task_id: taskId,
    version: 1,
    required_capabilities: [],
    verification_checks: [],
    note: null,
    created_at: T0,
    ...overrides,
  } as TaskContract;
}

/** 與 Runtime 一致的缺項判定（只看契約欄位）：未提供是 field_missing，未知是 field_unknown。 */
function missingOf(c: TaskContract): MissingItem[] {
  return CONTRACT_FIELDS.flatMap((name): MissingItem[] => {
    const field = c[name];
    if (field === null) return [{ code: "field_missing", field: name, message: `${FIELD_NAMES[name]}尚未提供` }];
    if (field.status === "unknown") return [{ code: "field_unknown", field: name, message: `${FIELD_NAMES[name]}標為未知，尚未補足` }];
    return [];
  });
}

function task(id: string, title: string, overrides: Partial<TaskView> = {}): TaskView {
  const c = overrides.contract ?? contract(id);
  const status = overrides.status ?? "pending";
  return {
    id,
    workspace_id: workspace.id,
    goal_id: "goal_1",
    title,
    kind: "research",
    status,
    version: 1,
    contract_version: c.version,
    created_at: T0,
    updated_at: T0,
    contract: c,
    dependencies: [],
    unfinished_dependencies: [],
    missing: status === "pending" ? missingOf(c) : [],
    ...overrides,
  };
}

const filledContract = (id: string, overrides: Partial<TaskContract> = {}) =>
  contract(id, {
    ...Object.fromEntries(CONTRACT_FIELDS.map((name) => [name, { status: "filled", text: `${name} 內容` }])),
    ...overrides,
  } as Partial<TaskContract>);

function run(id: string, taskId: string, status: RunStatus, overrides: Partial<RunView> = {}): RunView {
  return {
    id,
    workspace_id: workspace.id,
    task_id: taskId,
    status,
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
    created_at: T0,
    updated_at: T0,
    started_at: null,
    ended_at: null,
    lease: null,
    latest_progress: null,
    latest_checkpoint: null,
    blocked: null,
    ...overrides,
  };
}

/** 以記憶體模擬 Runtime 的 Task 行為，讓測試只看使用者可見的流程。 */
function fakeClient(
  initial: {
    goals?: Goal[];
    tasks?: TaskView[];
    runs?: RunView[];
    history?: TaskContract[];
    accounts?: AccountScheduling[];
    queue?: QueueEntry[];
    /** 設定後，派工一律回「排隊中」並加入佇列（沒有名額）。 */
    queueOnDispatch?: WaitReason[];
    verifications?: VerificationRecord[];
    /** `taskVerification` 回傳的驗證資料。 */
    verification?: VerificationView;
    /** 看板回報的 Workspace 目前授權 revision；省略為 null（尚未確認）。 */
    authorizationRevision?: number;
    workdirs?: WorkdirView[];
    integrations?: IntegrationEntry[];
    budgets?: BudgetView[];
    /** 省略時看板不帶 `handoffs` 欄位（舊版 Runtime）。 */
    handoffs?: Handoff[];
    /** `runContext` 回傳的 context receipt。 */
    context?: RunContext;
  } = {},
) {
  let goals = initial.goals ?? [goal("goal_1", "整理文件")];
  let tasks = initial.tasks ?? [];
  let runs = initial.runs ?? [];
  let queue = initial.queue ?? [];
  let verifications = initial.verifications ?? [];
  const board = (): TaskBoardData => ({
    workspace_id: workspace.id,
    goals,
    tasks,
    runs,
    capabilities: CAPABILITIES,
    verifications,
    authorization_revision: initial.authorizationRevision ?? null,
    workdirs: initial.workdirs ?? [],
    integrations: initial.integrations ?? [],
    budgets: initial.budgets ?? [],
    handoffs: initial.handoffs,
  });
  const replace = (id: string, patch: Partial<TaskView>) => {
    tasks = tasks.map((t) => (t.id === id ? { ...t, ...patch } : t));
    return tasks.find((t) => t.id === id)!;
  };
  const client: TaskBoardClient = {
    getTaskBoard: vi.fn(async () => board()),
    schedulingOverview: vi.fn(async () => schedulingOverview({ accounts: initial.accounts ?? [accountScheduling()], queue })),
    createGoal: vi.fn(async (_ws: string, title: string) => {
      const created = goal(`goal_${goals.length + 1}`, title);
      goals = [...goals, created];
      return created;
    }),
    createTask: vi.fn(async (_ws: string, { goalId, title, kind, contract: input }) => {
      const id = `task_${tasks.length + 1}`;
      const created = task(id, title, { goal_id: goalId, kind: kind ?? "research", contract: contract(id, input as Partial<TaskContract>) });
      tasks = [...tasks, created];
      return created;
    }),
    getTask: vi.fn(async (_ws: string, id: string) => ({
      task: tasks.find((t) => t.id === id)!,
      contract_history: initial.history ?? [tasks.find((t) => t.id === id)!.contract],
      budget: initial.budgets?.find((b) => b.task_id === id) ?? budgetView({ task_id: id }),
    })),
    updateTaskContract: vi.fn(async (_ws: string, id: string, input: ContractInput, note?: string) => {
      const current = tasks.find((t) => t.id === id)!;
      const next = contract(id, { ...(input as Partial<TaskContract>), version: current.contract_version + 1, note: note ?? null });
      return replace(id, { contract: next, contract_version: next.version, missing: missingOf(next) });
    }),
    addTaskDependency: vi.fn(async (_ws: string, id: string, dependsOn: string) => {
      const current = tasks.find((t) => t.id === id)!;
      return replace(id, {
        dependencies: [...current.dependencies, dependsOn],
        unfinished_dependencies: [...current.unfinished_dependencies, dependsOn],
      });
    }),
    checkTaskAdmission: vi.fn(async (_ws: string, id: string) => {
      const missing = missingOf(tasks.find((t) => t.id === id)!.contract);
      return { admissible: missing.length === 0, missing };
    }),
    markTaskReady: vi.fn(async (_ws: string, id: string) => {
      const current = tasks.find((t) => t.id === id)!;
      if (current.missing.length > 0) throw new Error("Task 尚未符合派工條件");
      return replace(id, { status: "ready", missing: [] });
    }),
    cancelTask: vi.fn(async (_ws: string, id: string) => replace(id, { status: "cancelled", missing: [] })),
    dispatchTask: vi.fn(async (_ws: string, id: string) => {
      const current = tasks.find((t) => t.id === id)!;
      if (current.status !== "ready") throw new Error("Task 不是 ready，不能派工");
      if (initial.queueOnDispatch) {
        queue = [
          ...queue,
          {
            workspace_id: workspace.id,
            task_id: id,
            title: current.title,
            priority: 0,
            requested_at: T0,
            position: queue.length + 1,
            reasons: initial.queueOnDispatch,
          },
        ];
        return { kind: "queued" as const, task_id: id, priority: 0, reasons: initial.queueOnDispatch };
      }
      const created = run(`run_${runs.length + 1}`, id, "queued", { generation: runs.filter((r) => r.task_id === id).length + 1 });
      runs = [...runs, created];
      replace(id, { status: "running" });
      return { kind: "started" as const, run: created };
    }),
    cancelRun: vi.fn(async (_ws: string, runId: string) => {
      const current = runs.find((r) => r.id === runId)!;
      // 尚未啟動的 Run 立即取消、Task 回到 ready；已啟動的只記錄取消要求
      const next: RunView =
        current.status === "queued" ? { ...current, status: "cancelled", ended_at: T0 } : { ...current, cancel_requested_at: T0 };
      runs = runs.map((r) => (r.id === runId ? next : r));
      if (next.status === "cancelled") replace(current.task_id, { status: "ready" });
      return next;
    }),
    runContext: vi.fn(async () => initial.context ?? runContextView()),
    taskVerification: vi.fn(async (_ws: string, id: string) => ({
      ...(initial.verification ?? verificationView()),
      task_id: id,
      status: tasks.find((t) => t.id === id)!.status,
      records: verifications.filter((r) => r.task_id === id),
    })),
    acceptTask: vi.fn(async (_ws: string, id: string, { basis, note }) => {
      const record = verificationRecord({
        id: `ver_${verifications.length + 1}`,
        task_id: id,
        run_id: basis.run_id,
        generation: basis.generation,
        contract_version: basis.contract_version,
        method: "manual",
        accepted_by: "desktop",
        note: note ?? null,
        decided_at: T0 + 3_600_000,
      });
      verifications = [...verifications, record];
      replace(id, { status: "completed" });
      return { ...verificationView(), task_id: id, status: "completed" as const, basis: null, records: [record] };
    }),
  };
  return client;
}

async function renderBoard(client: TaskBoardClient) {
  const user = userEvent.setup();
  render(<TaskBoard client={client} workspace={workspace} timeZone="UTC" />);
  await screen.findByRole("heading", { name: "工作" });
  return user;
}

const card = (title: string) => screen.getByRole("article", { name: title });
/** 卡片「詳細資訊」外的一行狀態（徽章 + 說明）。 */
const statusLine = (title: string) => card(title).querySelector<HTMLElement>(":scope > .task-status")!;

/** 展開卡片的「更多」（沒有「更多」或已展開時不動）：「派工」「標記為可派工」以外的動作都收在裡面。 */
async function openMore(user: ReturnType<typeof userEvent.setup>, target: string | HTMLElement) {
  const element = typeof target === "string" ? card(target) : target;
  const toggle = within(element).queryByRole("button", { name: "更多" });
  if (toggle?.getAttribute("aria-expanded") === "false") await user.click(toggle);
}
const column = (label: string) => screen.getByRole("region", { name: label });

describe("工作面板", () => {
  it("依 9 個狀態分欄，欄標題含卡片數", async () => {
    await renderBoard(
      fakeClient({
        tasks: [
          task("task_1", "寫文件"),
          task("task_2", "整理資料"),
          task("task_3", "部署", { status: "ready", contract: filledContract("task_3") }),
        ],
      }),
    );

    for (const label of ["待處理（2）", "可派工（1）", "執行中（0）", "等待裁定（0）", "等待資源（0）", "受阻（0）", "驗證中（0）", "已完成（0）", "已取消（0）"]) {
      expect(column(label)).toBeInTheDocument();
    }
    expect(within(column("待處理（2）")).getByRole("article", { name: "寫文件" })).toBeInTheDocument();
    expect(within(column("可派工（1）")).getByRole("article", { name: "部署" })).toBeInTheDocument();
  });

  it("卡片顯示所屬 Goal 與未完成的相依（以對方標題）", async () => {
    await renderBoard(
      fakeClient({
        tasks: [
          task("task_1", "寫文件"),
          task("task_2", "整理資料", { dependencies: ["task_1"], unfinished_dependencies: ["task_1"] }),
        ],
      }),
    );

    expect(within(card("寫文件")).getByText("Goal：整理文件")).toBeInTheDocument();
    expect(within(card("整理資料")).getByText("等待相依：寫文件")).toBeInTheDocument();
    expect(within(card("寫文件")).queryByText(/等待相依/)).not.toBeInTheDocument();
  });

  it("pending 卡片列出缺少項目，「未知」和「未提供」分開標示", async () => {
    const c = filledContract("task_1", { scope: null, deliverables: { status: "unknown" } });
    await renderBoard(fakeClient({ tasks: [task("task_1", "寫文件", { contract: c })] }));

    const missing = within(card("寫文件")).getByRole("group", { name: "缺少項目" });
    const items = within(missing).getAllByRole("listitem");
    expect(items).toHaveLength(2);
    expect(items[0]).toHaveTextContent("未提供");
    expect(items[0]).toHaveTextContent("範圍與限制尚未提供");
    expect(items[1]).toHaveTextContent("未知");
    expect(items[1]).not.toHaveTextContent("未提供");
    expect(items[1]).toHaveTextContent("預期成果標為未知，尚未補足");
  });

  it("契約完整的 pending 卡片沒有缺項；非 pending 卡片沒有缺項與「標記為可派工」", async () => {
    await renderBoard(
      fakeClient({
        tasks: [
          task("task_1", "完整", { contract: filledContract("task_1") }),
          task("task_2", "已派工", { status: "ready", contract: filledContract("task_2") }),
        ],
      }),
    );

    expect(within(card("完整")).queryByRole("group", { name: "缺少項目" })).not.toBeInTheDocument();
    expect(within(card("完整")).getByRole("button", { name: "標記為可派工" })).toBeInTheDocument();
    expect(within(card("已派工")).queryByRole("button", { name: "標記為可派工" })).not.toBeInTheDocument();
  });

  it("標記為可派工成功後卡片移到「可派工」欄", async () => {
    const user = await renderBoard(fakeClient({ tasks: [task("task_1", "完整", { contract: filledContract("task_1") })] }));

    await user.click(within(card("完整")).getByRole("button", { name: "標記為可派工" }));

    expect(await within(column("可派工（1）")).findByRole("article", { name: "完整" })).toBeInTheDocument();
    expect(column("待處理（0）")).toBeInTheDocument();
  });

  it("標記為可派工被拒時顯示 Runtime 的說明並重新載入看板", async () => {
    const client = fakeClient({ tasks: [task("task_1", "寫文件")] });
    const user = await renderBoard(client);
    expect(client.getTaskBoard).toHaveBeenCalledTimes(1);

    await user.click(within(card("寫文件")).getByRole("button", { name: "標記為可派工" }));

    expect(await within(card("寫文件")).findByRole("alert")).toHaveTextContent("Task 尚未符合派工條件");
    expect(client.getTaskBoard).toHaveBeenCalledTimes(2);
    expect(column("待處理（1）")).toBeInTheDocument();
  });

  it("取消 Task：非終止狀態才有「取消」，取消後移到「已取消」欄", async () => {
    const client = fakeClient({ tasks: [task("task_1", "寫文件"), task("task_2", "舊的", { status: "completed" })] });
    const user = await renderBoard(client);
    await openMore(user, "舊的");
    expect(within(card("舊的")).queryByRole("button", { name: "取消" })).not.toBeInTheDocument();

    await openMore(user, "寫文件");
    await user.click(within(card("寫文件")).getByRole("button", { name: "取消" }));

    expect(await within(column("已取消（1）")).findByRole("article", { name: "寫文件" })).toBeInTheDocument();
    expect(client.cancelTask).toHaveBeenCalledWith("ws_1", "task_1");
  });

  it("卡片上只有一個主要動作與「更多」，其餘動作收在「更多」裡；一行狀態摘要缺少的項目", async () => {
    const user = await renderBoard(
      fakeClient({
        tasks: [
          task("task_1", "寫文件"),
          task("task_2", "部署", { status: "ready", contract: filledContract("task_2") }),
          task("task_3", "執行", { status: "running", contract: filledContract("task_3") }),
        ],
        runs: [run("run_1", "task_3", "running")],
      }),
    );
    // 收在「詳細資訊」與「更多」以外、直接看得到的按鈕
    const visibleButtons = (title: string) =>
      within(card(title))
        .getAllByRole("button")
        .filter((button) => button.closest("details") === null)
        .map((button) => button.textContent);

    expect(visibleButtons("寫文件")).toEqual(["標記為可派工", "更多"]);
    expect(visibleButtons("部署")).toEqual(["派工", "更多"]);
    expect(visibleButtons("執行")).toEqual(["更多"]);
    expect(within(card("寫文件")).getByText(/^缺少 \d+ 項$/)).toBeInTheDocument();

    await openMore(user, "執行");
    const menu = within(card("執行")).getByRole("group", { name: "更多動作" });
    expect(within(menu).getAllByRole("button").map((button) => button.textContent)).toEqual([
      "編輯契約",
      "新增相依",
      "取消 Run",
      "取消",
    ]);
  });

  it("「更多」可用鍵盤操作：Enter 展開、Tab 進入動作、Esc 收起並回到「更多」", async () => {
    const user = await renderBoard(fakeClient({ tasks: [task("task_1", "寫文件")] }));
    const toggle = within(card("寫文件")).getByRole("button", { name: "更多" });
    expect(toggle).toHaveAttribute("aria-expanded", "false");

    toggle.focus();
    await user.keyboard("{Enter}");
    expect(toggle).toHaveAttribute("aria-expanded", "true");
    const menu = within(card("寫文件")).getByRole("group", { name: "更多動作" });
    expect(toggle).toHaveAttribute("aria-controls", menu.id);

    await user.tab();
    expect(within(menu).getByRole("button", { name: "編輯契約" })).toHaveFocus();
    await user.keyboard("{Escape}");

    expect(within(card("寫文件")).queryByRole("group", { name: "更多動作" })).not.toBeInTheDocument();
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    expect(toggle).toHaveFocus();
  });

  it("新增相依：從同一 Workspace 的其他 Task 挑選，卡片顯示等待相依", async () => {
    const client = fakeClient({ tasks: [task("task_1", "寫文件"), task("task_2", "整理資料")] });
    const user = await renderBoard(client);

    await openMore(user, "整理資料");
    await user.click(within(card("整理資料")).getByRole("button", { name: "新增相依" }));
    const select = within(card("整理資料")).getByLabelText("相依的 Task");
    expect(within(select).queryByRole("option", { name: "整理資料" })).not.toBeInTheDocument();
    await user.selectOptions(select, "寫文件");
    await user.click(within(card("整理資料")).getByRole("button", { name: "確定新增" }));

    expect(client.addTaskDependency).toHaveBeenCalledWith("ws_1", "task_2", "task_1");
    expect(await within(card("整理資料")).findByText("等待相依：寫文件")).toBeInTheDocument();
  });

  it("從「更多」選「新增相依」後，焦點移到相依表單的選單", async () => {
    const client = fakeClient({ tasks: [task("task_1", "寫文件"), task("task_2", "整理資料")] });
    const user = await renderBoard(client);

    await openMore(user, "整理資料");
    within(card("整理資料")).getByRole("button", { name: "新增相依" }).focus();
    await user.keyboard("{Enter}");

    expect(within(card("整理資料")).getByLabelText("相依的 Task")).toHaveFocus();
  });

  it("新增相依失敗（例如循環）時顯示 Runtime 的說明", async () => {
    const client = fakeClient({ tasks: [task("task_1", "寫文件"), task("task_2", "整理資料")] });
    vi.mocked(client.addTaskDependency).mockRejectedValueOnce(new Error("相依會形成循環：寫文件 → 整理資料 → 寫文件"));
    const user = await renderBoard(client);

    await openMore(user, "整理資料");
    await user.click(within(card("整理資料")).getByRole("button", { name: "新增相依" }));
    await user.selectOptions(within(card("整理資料")).getByLabelText("相依的 Task"), "寫文件");
    await user.click(within(card("整理資料")).getByRole("button", { name: "確定新增" }));

    expect(await within(card("整理資料")).findByRole("alert")).toHaveTextContent("相依會形成循環");
  });

  it("契約版本號顯示在卡片上，展開可看每個版本的時間與說明", async () => {
    const history = [
      contract("task_1", { version: 1, created_at: T0 }),
      contract("task_1", { version: 2, created_at: T0 + 3_600_000, note: "補上目標" }),
    ];
    const client = fakeClient({ tasks: [task("task_1", "寫文件", { contract: history[1] })], history });
    const user = await renderBoard(client);
    expect(within(card("寫文件")).getByText("契約版本 v2")).toBeInTheDocument();
    // 契約版本是次要資訊，收在預設收合的「詳細資訊」
    expect(within(card("寫文件")).getByText("契約版本 v2").closest("details")).not.toHaveAttribute("open");

    await user.click(within(card("寫文件")).getByRole("button", { name: "契約版本歷史" }));

    const items = await within(within(card("寫文件")).getByRole("group", { name: "契約版本歷史" })).findAllByRole("listitem");
    expect(items).toHaveLength(2);
    expect(items[0]).toHaveTextContent("v2：2023/11/14 23:13，補上目標");
    expect(items[1]).toHaveTextContent("v1：2023/11/14 22:13");
    expect(client.getTask).toHaveBeenCalledWith("ws_1", "task_1");
  });

  it("讀不到工作面板時顯示原因", async () => {
    const client = fakeClient();
    vi.mocked(client.getTaskBoard).mockRejectedValueOnce(new Error("connection refused"));

    render(<TaskBoard client={client} workspace={workspace} />);

    expect(await screen.findByRole("alert")).toHaveTextContent("connection refused");
  });
});

describe("派工與 Run", () => {
  const readyTask = (id = "task_1", title = "部署") => task(id, title, { status: "ready", contract: filledContract(id) });
  const runStatus = (title: string) => within(card(title)).getByRole("group", { name: "Run 狀態" });

  it("ready 的卡片可派工：呼叫 client，Task 移到「執行中」並顯示排隊中的 Run", async () => {
    const client = fakeClient({ tasks: [readyTask()] });
    const user = await renderBoard(client);

    await user.click(within(card("部署")).getByRole("button", { name: "派工" }));

    expect(client.dispatchTask).toHaveBeenCalledWith("ws_1", "task_1");
    const running = await within(column("執行中（1）")).findByRole("article", { name: "部署" });
    expect(within(running).getByRole("group", { name: "Run 狀態" })).toHaveTextContent("排隊中");
    expect(within(running).queryByRole("button", { name: "派工" })).not.toBeInTheDocument();
    await openMore(user, running);
    expect(within(running).getByRole("button", { name: "取消 Run" })).toBeInTheDocument();
  });

  it("Run 的狀態列顯示使用的帳號名稱（以 profile_id 對照總覽的帳號）", async () => {
    await renderBoard(
      fakeClient({
        tasks: [task("task_1", "部署", { status: "running" })],
        runs: [run("run_1", "task_1", "running", { profile_id: "prof_2" })],
        accounts: [accountScheduling(), accountScheduling({ profile_id: "prof_2", name: "備用帳號" })],
      }),
    );

    expect(runStatus("部署")).toHaveTextContent("帳號 備用帳號");
  });

  it("帳號不在總覽（例如已封存）時退回顯示 profile_id", async () => {
    await renderBoard(
      fakeClient({
        tasks: [task("task_1", "部署", { status: "running" })],
        runs: [run("run_1", "task_1", "running", { profile_id: "prof_gone" })],
      }),
    );

    expect(runStatus("部署")).toHaveTextContent("帳號 prof_gone");
  });

  it("派工回傳排隊中時，卡片留在「可派工」，顯示排隊中與依類別分組的原因", async () => {
    const client = fakeClient({
      tasks: [readyTask()],
      queueOnDispatch: [
        { category: "capacity", code: "account_capacity_full", message: "主要帳號的並行名額已滿（2/2）", profile_id: "prof_1" },
        { category: "capacity", code: "global_capacity_full", message: "全域並行名額已滿（6/6）" },
        { category: "dependency", code: "dependencies_unfinished", message: "相依的 Task 尚未完成" },
      ],
    });
    const user = await renderBoard(client);

    await user.click(within(card("部署")).getByRole("button", { name: "派工" }));

    const queued = await within(column("可派工（1）")).findByRole("group", { name: "排隊原因" });
    expect(queued).toHaveTextContent("排隊中（第 1 位）");
    const capacity = within(queued).getByText("名額已滿").closest("li")!;
    expect(within(capacity).getByText("主要帳號的並行名額已滿（2/2）")).toBeInTheDocument();
    expect(within(capacity).getByText("全域並行名額已滿（6/6）")).toBeInTheDocument();
    const dependency = within(queued).getByText("相依").closest("li")!;
    expect(within(dependency).getByText("相依的 Task 尚未完成")).toBeInTheDocument();
    expect(within(queued).queryByText("額度")).not.toBeInTheDocument();
  });

  it("派工回傳排隊中但總覽讀不到時，仍顯示排隊中與原因", async () => {
    const client = fakeClient({
      tasks: [readyTask()],
      queueOnDispatch: [{ category: "quota", code: "quota_near_limit", message: "主要帳號的額度接近上限" }],
    });
    const user = await renderBoard(client);
    vi.mocked(client.schedulingOverview).mockRejectedValue(new Error("離線"));

    await user.click(within(card("部署")).getByRole("button", { name: "派工" }));

    const queued = await within(card("部署")).findByRole("group", { name: "排隊原因" });
    expect(queued).toHaveTextContent("排隊中");
    expect(queued).not.toHaveTextContent("第");
    expect(within(queued).getByText("額度")).toBeInTheDocument();
    expect(within(queued).getByText("主要帳號的額度接近上限")).toBeInTheDocument();
  });

  it("總覽的佇列中有這個 ready 的 Task 時，顯示「排隊中（第 N 位）」與原因；沒有的不顯示", async () => {
    await renderBoard(
      fakeClient({
        tasks: [readyTask("task_1", "部署"), readyTask("task_2", "測試"), readyTask("task_3", "發佈")],
        queue: [
          {
            workspace_id: "ws_1",
            task_id: "task_2",
            title: "測試",
            priority: 0,
            requested_at: T0,
            position: 3,
            reasons: [{ category: "authorization", code: "capability_not_granted", message: "尚未授權建置與測試" }],
          },
        ],
      }),
    );

    const queued = within(card("測試")).getByRole("group", { name: "排隊原因" });
    expect(queued).toHaveTextContent("排隊中（第 3 位）");
    expect(within(queued).getByText("授權")).toBeInTheDocument();
    expect(within(queued).getByText("尚未授權建置與測試")).toBeInTheDocument();
    expect(within(card("部署")).queryByRole("group", { name: "排隊原因" })).not.toBeInTheDocument();
    expect(within(card("發佈")).queryByRole("group", { name: "排隊原因" })).not.toBeInTheDocument();
  });

  it("狀態不明與帳號類別各自分開標示", async () => {
    await renderBoard(
      fakeClient({
        tasks: [readyTask()],
        queue: [
          {
            workspace_id: "ws_1",
            task_id: "task_1",
            title: "部署",
            priority: 0,
            requested_at: T0,
            position: 1,
            reasons: [
              { category: "state_unknown", code: "profile_unverified", message: "備用帳號尚未核對" },
              { category: "account", code: "no_account", message: "沒有可派工的帳號" },
            ],
          },
        ],
      }),
    );

    const queued = within(card("部署")).getByRole("group", { name: "排隊原因" });
    expect(within(queued.querySelector("li")!).getByText("狀態不明")).toBeInTheDocument();
    expect(within(queued).getByText("帳號")).toBeInTheDocument();
    expect(within(queued).getByText("備用帳號尚未核對")).toBeInTheDocument();
    expect(within(queued).getByText("沒有可派工的帳號")).toBeInTheDocument();
  });

  it("完全退出中或等待「繼續工作」的原因標示為「完全退出」", async () => {
    await renderBoard(
      fakeClient({
        tasks: [readyTask()],
        queue: [
          {
            workspace_id: "ws_1",
            task_id: "task_1",
            title: "部署",
            priority: 0,
            requested_at: T0,
            position: 1,
            reasons: [
              { category: "lifecycle", code: "awaiting_resume", message: "Workspace 在完全退出後等待「繼續工作」" },
            ],
          },
        ],
      }),
    );

    const queued = within(card("部署")).getByRole("group", { name: "排隊原因" });
    const lifecycle = within(queued).getByText("完全退出").closest("li")!;
    expect(within(lifecycle).getByText("Workspace 在完全退出後等待「繼續工作」")).toBeInTheDocument();
  });

  it("Guard 擋下的原因標示為「Guard 擋下」，並顯示來源教訓的標題", async () => {
    await renderBoard(
      fakeClient({
        tasks: [readyTask()],
        queue: [
          {
            workspace_id: "ws_1",
            task_id: "task_1",
            title: "部署",
            priority: 0,
            requested_at: T0,
            position: 1,
            reasons: [
              {
                category: "guard",
                code: "guard_violated",
                message: "契約的「範圍與限制」沒有提到「備份」（來源教訓「部署前先備份資料庫」）",
                guard: { guard_id: "gd_1", lesson_id: "kn_1", lesson_title: "部署前先備份資料庫" },
              },
            ],
          },
        ],
      }),
    );

    const queued = within(card("部署")).getByRole("group", { name: "排隊原因" });
    const guard = within(queued).getByText("Guard 擋下").closest("li")!;
    expect(guard).toHaveTextContent("契約的「範圍與限制」沒有提到「備份」");
    expect(within(guard).getByText("來源教訓：部署前先備份資料庫")).toBeInTheDocument();
  });

  it("派工被 Guard 擋下（202）時立即顯示原因與來源教訓", async () => {
    const user = await renderBoard(
      fakeClient({
        tasks: [readyTask()],
        queueOnDispatch: [
          {
            category: "guard",
            code: "guard_violated",
            message: "Task 要求了「部署」能力（來源教訓「不要直接部署」）",
            guard: { guard_id: "gd_2", lesson_id: "kn_2", lesson_title: "不要直接部署" },
          },
        ],
      }),
    );

    await user.click(within(card("部署")).getByRole("button", { name: "派工" }));

    const queued = await within(card("部署")).findByRole("group", { name: "排隊原因" });
    expect(within(queued).getByText("Guard 擋下")).toBeInTheDocument();
    expect(within(queued).getByText("來源教訓：不要直接部署")).toBeInTheDocument();
  });

  it("讀不到排程總覽時看板照常顯示，Run 狀態列改顯示 profile_id", async () => {
    const client = fakeClient({
      tasks: [task("task_1", "部署", { status: "running" })],
      runs: [run("run_1", "task_1", "running")],
    });
    vi.mocked(client.schedulingOverview).mockRejectedValue(new Error("離線"));

    await renderBoard(client);

    expect(runStatus("部署")).toHaveTextContent("帳號 prof_1");
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("非 ready 的卡片沒有「派工」", async () => {
    await renderBoard(
      fakeClient({
        tasks: [task("task_1", "待處理"), task("task_2", "執行中", { status: "running" }), task("task_3", "驗證", { status: "verifying" })],
        runs: [run("run_1", "task_2", "running"), run("run_2", "task_3", "succeeded")],
      }),
    );

    for (const title of ["待處理", "執行中", "驗證"]) {
      expect(within(card(title)).queryByRole("button", { name: "派工" })).not.toBeInTheDocument();
    }
  });

  it("從未派工的 Task 沒有 Run 狀態列", async () => {
    await renderBoard(fakeClient({ tasks: [readyTask()] }));

    expect(within(card("部署")).queryByRole("group", { name: "Run 狀態" })).not.toBeInTheDocument();
  });

  it.each<[RunStatus, string]>([
    ["queued", "排隊中"],
    ["starting", "啟動中"],
    ["running", "執行中"],
    ["succeeded", "成功"],
    ["failed", "失敗"],
    ["interrupted", "中斷"],
    ["cancelled", "已取消"],
  ])("Run 狀態 %s 顯示為「%s」", async (status, label) => {
    await renderBoard(fakeClient({ tasks: [task("task_1", "部署", { status: "running" })], runs: [run("run_1", "task_1", status)] }));

    expect(runStatus("部署")).toHaveTextContent(label);
  });

  it("只顯示 generation 最大的 Run，並顯示 pid、結束原因與最後心跳（當地時間）", async () => {
    const lease = {
      run_id: "run_2",
      task_id: "task_1",
      generation: 2,
      acquired_at: T0,
      last_heartbeat_at: T0 + 3_600_000,
      expires_at: T0 + 7_200_000,
      released_at: null,
      release_reason: null,
    };
    await renderBoard(
      fakeClient({
        tasks: [task("task_1", "部署", { status: "blocked" })],
        runs: [
          run("run_2", "task_1", "failed", {
            generation: 2,
            process: { pid: 4321, created_at_millis: T0, command_hash: "h" },
            end_reason: "程序結束碼 1",
            lease,
          }),
          run("run_1", "task_1", "succeeded", { generation: 1, end_reason: "舊的原因" }),
        ],
      }),
    );

    const status = runStatus("部署");
    expect(status).toHaveTextContent("失敗");
    expect(status).toHaveTextContent("pid 4321");
    // pid 與心跳是技術資訊，收在預設收合的「技術細節」
    const more = within(status).getByText("技術細節").closest("details")!;
    expect(more).not.toHaveAttribute("open");
    expect(more).toHaveTextContent("pid 4321");
    expect(more).toHaveTextContent("最後心跳");
    expect(status).toHaveTextContent("結束原因：程序結束碼 1");
    expect(status).toHaveTextContent("最後心跳：2023/11/14 23:13");
    expect(status).not.toHaveTextContent("舊的原因");
    expect(status).not.toHaveTextContent("成功");
  });

  it("沒有 pid、結束原因與 lease 時不顯示這些欄位", async () => {
    await renderBoard(fakeClient({ tasks: [task("task_1", "部署", { status: "running" })], runs: [run("run_1", "task_1", "queued")] }));

    const status = runStatus("部署");
    expect(status).not.toHaveTextContent("pid");
    expect(status).not.toHaveTextContent("結束原因");
    expect(status).not.toHaveTextContent("最後心跳");
  });

  it("Run 狀態列顯示最新進度、Checkpoint 與受阻原因（含當地時間）", async () => {
    await renderBoard(
      fakeClient({
        tasks: [task("task_1", "部署", { status: "blocked" })],
        runs: [
          run("run_1", "task_1", "running", {
            latest_progress: { summary: "已完成 3/5 個檔案", reported_at: T0 + 3_600_000 },
            latest_checkpoint: { summary: "資料表遷移完成", details: "遷移腳本 v3 已套用", saved_at: T0 + 7_200_000 },
            blocked: { reason: "缺少部署金鑰", reported_at: T0 + 10_800_000 },
          }),
        ],
      }),
    );

    const status = runStatus("部署");
    expect(status).toHaveTextContent("最新進度：已完成 3/5 個檔案（2023/11/14 23:13）");
    expect(status).toHaveTextContent("Checkpoint：資料表遷移完成（2023/11/15 00:13）");
    expect(within(status).getByRole("alert")).toHaveTextContent("受阻：缺少部署金鑰（2023/11/15 01:13）");
  });

  it("沒有回報時不顯示進度、Checkpoint 與受阻", async () => {
    await renderBoard(fakeClient({ tasks: [task("task_1", "部署", { status: "running" })], runs: [run("run_1", "task_1", "running")] }));

    const status = runStatus("部署");
    expect(status).not.toHaveTextContent("最新進度");
    expect(status).not.toHaveTextContent("Checkpoint");
    expect(status).not.toHaveTextContent("受阻");
    expect(within(status).queryByRole("alert")).not.toBeInTheDocument();
  });

  it("Checkpoint 細節預設收合，點開後可看到", async () => {
    const user = await renderBoard(
      fakeClient({
        tasks: [task("task_1", "部署", { status: "running" })],
        runs: [
          run("run_1", "task_1", "running", {
            latest_checkpoint: { summary: "資料表遷移完成", details: "遷移腳本 v3 已套用", saved_at: T0 },
          }),
        ],
      }),
    );

    const details = runStatus("部署").querySelector<HTMLElement>(".task-checkpoint details")!;
    expect(details).not.toHaveAttribute("open");
    await user.click(within(details).getByText("細節"));
    expect(details).toHaveAttribute("open");
    expect(details).toHaveTextContent("遷移腳本 v3 已套用");
  });

  it("Checkpoint 的細節為空時沒有可展開的區塊", async () => {
    await renderBoard(
      fakeClient({
        tasks: [task("task_1", "部署", { status: "running" })],
        runs: [run("run_1", "task_1", "running", { latest_checkpoint: { summary: "資料表遷移完成", details: "", saved_at: T0 } })],
      }),
    );

    expect(runStatus("部署")).toHaveTextContent("Checkpoint：資料表遷移完成");
    expect(runStatus("部署").querySelector(".task-checkpoint details")).toBeNull();
  });

  it("只顯示 generation 最大的 Run 的回報，舊世代的回報不顯示", async () => {
    await renderBoard(
      fakeClient({
        tasks: [task("task_1", "部署", { status: "running" })],
        runs: [
          run("run_1", "task_1", "failed", {
            generation: 1,
            latest_progress: { summary: "舊進度", reported_at: T0 },
            latest_checkpoint: { summary: "舊 checkpoint", details: "舊細節", saved_at: T0 },
            blocked: { reason: "舊受阻", reported_at: T0 },
          }),
          run("run_2", "task_1", "running", {
            generation: 2,
            latest_progress: { summary: "新進度", reported_at: T0 },
          }),
        ],
      }),
    );

    const status = runStatus("部署");
    expect(status).toHaveTextContent("最新進度：新進度");
    for (const old of ["舊進度", "舊 checkpoint", "舊細節", "舊受阻"]) {
      expect(status).not.toHaveTextContent(old);
    }
    expect(status).not.toHaveTextContent("受阻");
  });

  it("進行中的 Run 可取消：尚未啟動的立即變已取消，Task 回到「可派工」", async () => {
    const client = fakeClient({ tasks: [task("task_1", "部署", { status: "running" })], runs: [run("run_1", "task_1", "queued")] });
    const user = await renderBoard(client);

    await openMore(user, "部署");
    await user.click(within(card("部署")).getByRole("button", { name: "取消 Run" }));

    expect(client.cancelRun).toHaveBeenCalledWith("ws_1", "run_1");
    const ready = await within(column("可派工（1）")).findByRole("article", { name: "部署" });
    expect(within(ready).getByRole("group", { name: "Run 狀態" })).toHaveTextContent("已取消");
    await openMore(user, ready);
    expect(within(ready).queryByRole("button", { name: "取消 Run" })).not.toBeInTheDocument();
    expect(within(ready).getByRole("button", { name: "派工" })).toBeInTheDocument();
  });

  it("已要求取消的 Run 顯示「取消中，等待程序停止…」，不再有取消按鈕", async () => {
    const client = fakeClient({
      tasks: [task("task_1", "部署", { status: "running" })],
      runs: [run("run_1", "task_1", "running", { process: { pid: 7, created_at_millis: T0, command_hash: "h" } })],
    });
    const user = await renderBoard(client);

    await openMore(user, "部署");
    await user.click(within(card("部署")).getByRole("button", { name: "取消 Run" }));

    expect(await within(card("部署")).findByText("取消中，等待程序停止…")).toBeInTheDocument();
    expect(runStatus("部署")).toHaveTextContent("執行中");
    await openMore(user, "部署");
    expect(within(card("部署")).queryByRole("button", { name: "取消 Run" })).not.toBeInTheDocument();
  });

  it("載入時 cancel_requested_at 已有值的 Run 直接顯示「取消中」", async () => {
    const user = await renderBoard(
      fakeClient({
        tasks: [task("task_1", "部署", { status: "running" })],
        runs: [run("run_1", "task_1", "starting", { cancel_requested_at: T0 })],
      }),
    );

    expect(within(card("部署")).getByText("取消中，等待程序停止…")).toBeInTheDocument();
    await openMore(user, "部署");
    expect(within(card("部署")).queryByRole("button", { name: "取消 Run" })).not.toBeInTheDocument();
  });

  it("已結束的 Run 沒有取消按鈕；成功的 Run 之後 Task 在「驗證中」而不是已完成", async () => {
    const user = await renderBoard(fakeClient({ tasks: [task("task_1", "部署", { status: "verifying" })], runs: [run("run_1", "task_1", "succeeded")] }));

    await openMore(user, "部署");
    expect(within(card("部署")).queryByRole("button", { name: "取消 Run" })).not.toBeInTheDocument();
    expect(within(column("驗證中（1）")).getByRole("article", { name: "部署" })).toBeInTheDocument();
    expect(column("已完成（0）")).toBeInTheDocument();
  });

  it("已結束的 Run 不顯示「取消中」，即使有 cancel_requested_at", async () => {
    await renderBoard(
      fakeClient({
        tasks: [task("task_1", "部署", { status: "cancelled" })],
        runs: [run("run_1", "task_1", "cancelled", { cancel_requested_at: T0 })],
      }),
    );

    expect(within(card("部署")).queryByText("取消中，等待程序停止…")).not.toBeInTheDocument();
  });

  it("已准入且沿用較舊授權 revision 的進行中 Run 標示「沿用舊授權」", async () => {
    await renderBoard(
      fakeClient({
        tasks: [task("task_1", "部署", { status: "running" })],
        runs: [run("run_1", "task_1", "running", { admitted_at: T0, authorization_revision: 1 })],
        authorizationRevision: 2,
      }),
    );

    expect(runStatus("部署")).toHaveTextContent("沿用舊授權（第 1 版，目前第 2 版）");
  });

  it.each<[string, Partial<RunView>, RunStatus]>([
    ["revision 與目前相同", { admitted_at: T0, authorization_revision: 2 }, "running"],
    ["尚未准入", { admitted_at: null, authorization_revision: 1 }, "running"],
    ["Run 已結束", { admitted_at: T0, authorization_revision: 1 }, "succeeded"],
  ])("%s時不標示沿用舊授權", async (_case, overrides, status) => {
    await renderBoard(
      fakeClient({
        tasks: [task("task_1", "部署", { status: "running" })],
        runs: [run("run_1", "task_1", status, overrides)],
        authorizationRevision: 2,
      }),
    );

    expect(runStatus("部署")).not.toHaveTextContent("沿用舊授權");
  });

  it("派工被拒絕時顯示 Runtime 的說明並重新載入看板", async () => {
    const client = fakeClient({ tasks: [readyTask()] });
    vi.mocked(client.dispatchTask).mockRejectedValueOnce(new Error("相依的 Task 尚未完成，不能派工"));
    const user = await renderBoard(client);

    await user.click(within(card("部署")).getByRole("button", { name: "派工" }));

    expect(await within(card("部署")).findByRole("alert")).toHaveTextContent("相依的 Task 尚未完成，不能派工");
    expect(client.getTaskBoard).toHaveBeenCalledTimes(2);
    expect(column("可派工（1）")).toBeInTheDocument();
  });

  it("取消 Run 失敗時顯示 Runtime 的說明", async () => {
    const client = fakeClient({ tasks: [task("task_1", "部署", { status: "running" })], runs: [run("run_1", "task_1", "running")] });
    vi.mocked(client.cancelRun).mockRejectedValueOnce(new Error("Runtime 無法連線"));
    const user = await renderBoard(client);

    await openMore(user, "部署");
    await user.click(within(card("部署")).getByRole("button", { name: "取消 Run" }));

    expect(await within(card("部署")).findByRole("alert")).toHaveTextContent("Runtime 無法連線");
  });
});

describe("建立 Task 與契約編輯器", () => {
  async function openCreate(client: TaskBoardClient) {
    const user = await renderBoard(client);
    await user.click(screen.getByRole("button", { name: "建立 Task" }));
    return { user, form: screen.getByRole("dialog", { name: "建立 Task" }) };
  }

  it("選擇既有 Goal 建立 Task，契約欄位預設為未提供", async () => {
    const client = fakeClient();
    const { user, form } = await openCreate(client);

    await user.type(within(form).getByLabelText("Task 標題"), "寫文件");
    await user.click(within(form).getByRole("button", { name: "建立" }));

    await within(column("待處理（1）")).findByRole("article", { name: "寫文件" });
    expect(client.createGoal).not.toHaveBeenCalled();
    expect(client.createTask).toHaveBeenCalledWith("ws_1", {
      goalId: "goal_1",
      title: "寫文件",
      kind: "research",
      contract: { required_capabilities: [] },
    });
    expect(screen.queryByRole("dialog", { name: "建立 Task" })).not.toBeInTheDocument();
  });

  it("輸入新 Goal 名稱時，建立 Task 前先建立 Goal", async () => {
    const client = fakeClient();
    const { user, form } = await openCreate(client);

    await user.selectOptions(within(form).getByLabelText("Goal"), "新增 Goal…");
    await user.type(within(form).getByLabelText("新 Goal 名稱"), "第二個目標");
    await user.type(within(form).getByLabelText("Task 標題"), "寫文件");
    await user.click(within(form).getByRole("button", { name: "建立" }));

    await within(column("待處理（1）")).findByRole("article", { name: "寫文件" });
    expect(client.createGoal).toHaveBeenCalledWith("ws_1", "第二個目標");
    expect(client.createTask).toHaveBeenCalledWith("ws_1", expect.objectContaining({ goalId: "goal_2", title: "寫文件" }));
    expect(vi.mocked(client.createGoal).mock.invocationCallOrder[0]).toBeLessThan(
      vi.mocked(client.createTask).mock.invocationCallOrder[0],
    );
    expect(within(card("寫文件")).getByText("Goal：第二個目標")).toBeInTheDocument();
  });

  it("標題空白時不送出", async () => {
    const client = fakeClient();
    const { user, form } = await openCreate(client);

    await user.click(within(form).getByRole("button", { name: "建立" }));

    expect(client.createTask).not.toHaveBeenCalled();
    expect(within(form).getByRole("alert")).toHaveTextContent("請輸入 Task 標題");
  });

  it("六個欄位各有四種模式；目標與完成條件沒有「不適用」", async () => {
    const { form } = await openCreate(fakeClient());

    for (const label of Object.values(FIELD_NAMES)) {
      const options = within(within(form).getByLabelText(`${label}：模式`)).getAllByRole("option").map((o) => o.textContent);
      const forbidden = label === "目標" || label === "完成與驗證條件";
      expect(options).toEqual(forbidden ? ["未提供", "填寫", "未知"] : ["未提供", "填寫", "不適用", "未知"]);
    }
  });

  it("填寫、引用、不適用、未知與所需能力都送進契約", async () => {
    const client = fakeClient();
    const { user, form } = await openCreate(client);
    await user.type(within(form).getByLabelText("Task 標題"), "寫文件");

    await user.selectOptions(within(form).getByLabelText("目標：模式"), "填寫");
    await user.type(within(form).getByLabelText("目標：內容"), "整理文件");
    await user.type(
      within(form).getByLabelText("目標：引用（選填）"),
      "goal:goal_1@1{enter}task_contract:task_9@2",
    );
    await user.selectOptions(within(form).getByLabelText("輸入與依據：模式"), "不適用");
    await user.type(within(form).getByLabelText("輸入與依據：不適用的理由"), "不需要外部資料");
    await user.selectOptions(within(form).getByLabelText("範圍與限制：模式"), "未知");
    await user.type(within(form).getByLabelText("範圍與限制：未知的說明（選填）"), "等客戶回覆");
    await user.selectOptions(within(form).getByLabelText("阻礙處理：模式"), "未知");
    await user.click(within(form).getByRole("checkbox", { name: "建置與測試" }));
    await user.click(within(form).getByRole("button", { name: "建立" }));

    await waitFor(() => expect(client.createTask).toHaveBeenCalled());
    expect(client.createTask).toHaveBeenCalledWith("ws_1", {
      goalId: "goal_1",
      title: "寫文件",
      kind: "research",
      contract: {
        objective: {
          status: "filled",
          text: "整理文件",
          references: [
            { kind: "goal", target: "goal_1", version: 1 },
            { kind: "task_contract", target: "task_9", version: 2 },
          ],
        },
        inputs: { status: "not_applicable", reason: "不需要外部資料" },
        scope: { status: "unknown", note: "等客戶回覆" },
        blocker_handling: { status: "unknown" },
        required_capabilities: ["run_build_test" satisfies Capability],
      },
    });
  });

  it("引用格式錯誤、不適用沒有理由、填寫沒有內容時，送出前提示並且不送出", async () => {
    const client = fakeClient();
    const { user, form } = await openCreate(client);
    await user.type(within(form).getByLabelText("Task 標題"), "寫文件");
    await user.selectOptions(within(form).getByLabelText("目標：模式"), "填寫");
    await user.type(within(form).getByLabelText("目標：內容"), "整理文件");
    await user.type(within(form).getByLabelText("目標：引用（選填）"), "goal-1");
    await user.selectOptions(within(form).getByLabelText("範圍與限制：模式"), "不適用");
    await user.selectOptions(within(form).getByLabelText("預期成果：模式"), "填寫");

    await user.click(within(form).getByRole("button", { name: "建立" }));

    const alert = within(form).getByRole("alert");
    expect(alert).toHaveTextContent("目標：引用格式錯誤「goal-1」");
    expect(alert).toHaveTextContent("範圍與限制：不適用必須填寫理由");
    expect(alert).toHaveTextContent("預期成果：填寫模式需要內容");
    expect(client.createTask).not.toHaveBeenCalled();
  });

  it("建立失敗時顯示 Runtime 的說明並保留表單", async () => {
    const client = fakeClient();
    vi.mocked(client.createTask).mockRejectedValueOnce(new Error("Workspace 已封存"));
    const { user, form } = await openCreate(client);
    await user.type(within(form).getByLabelText("Task 標題"), "寫文件");

    await user.click(within(form).getByRole("button", { name: "建立" }));

    expect(await within(form).findByRole("alert")).toHaveTextContent("Workspace 已封存");
    expect(within(form).getByLabelText("Task 標題")).toHaveValue("寫文件");
  });

  it("沒有 Goal 時預設為新增 Goal", async () => {
    const { form } = await openCreate(fakeClient({ goals: [] }));

    expect(within(form).getByLabelText("新 Goal 名稱")).toBeInTheDocument();
  });

  it("所需能力的清單取自看板的能力標籤", async () => {
    const { form } = await openCreate(fakeClient());

    const fieldset = within(form).getByRole("group", { name: "所需能力" });
    expect(within(fieldset).getAllByRole("checkbox").map((c) => c.parentElement?.textContent)).toEqual(["讀取檔案", "建置與測試", "部署"]);
  });
});

describe("編輯契約", () => {
  const existing = () =>
    task("task_1", "寫文件", {
      contract: contract("task_1", {
        version: 2,
        objective: { status: "filled", text: "整理文件", references: [{ kind: "goal", target: "goal_1", version: 1 }] },
        inputs: { status: "not_applicable", reason: "不需要外部資料" },
        scope: { status: "unknown", note: "等客戶回覆" },
        required_capabilities: ["read_files"],
      }),
    });

  it("載入目前的契約，附上變更說明送出整份契約", async () => {
    const client = fakeClient({ tasks: [existing()] });
    const user = await renderBoard(client);

    await openMore(user, "寫文件");
    await user.click(within(card("寫文件")).getByRole("button", { name: "編輯契約" }));
    const form = screen.getByRole("dialog", { name: "編輯契約：寫文件" });

    expect(within(form).getByLabelText("目標：模式")).toHaveValue("filled");
    expect(within(form).getByLabelText("目標：內容")).toHaveValue("整理文件");
    expect(within(form).getByLabelText("目標：引用（選填）")).toHaveValue("goal:goal_1@1");
    expect(within(form).getByLabelText("輸入與依據：不適用的理由")).toHaveValue("不需要外部資料");
    expect(within(form).getByLabelText("範圍與限制：未知的說明（選填）")).toHaveValue("等客戶回覆");
    expect(within(form).getByLabelText("預期成果：模式")).toHaveValue("missing");
    expect(within(form).getByRole("checkbox", { name: "讀取檔案" })).toBeChecked();
    expect(within(form).queryByLabelText("Task 標題")).not.toBeInTheDocument();

    await user.selectOptions(within(form).getByLabelText("預期成果：模式"), "未知");
    await user.type(within(form).getByLabelText("變更說明（選填）"), "標記成果未知");
    await user.click(within(form).getByRole("button", { name: "儲存契約" }));

    await waitFor(() => expect(client.updateTaskContract).toHaveBeenCalled());
    expect(client.updateTaskContract).toHaveBeenCalledWith(
      "ws_1",
      "task_1",
      {
        objective: { status: "filled", text: "整理文件", references: [{ kind: "goal", target: "goal_1", version: 1 }] },
        inputs: { status: "not_applicable", reason: "不需要外部資料" },
        scope: { status: "unknown", note: "等客戶回覆" },
        deliverables: { status: "unknown" },
        required_capabilities: ["read_files"],
      },
      "標記成果未知",
    );
    expect(await within(card("寫文件")).findByText("契約版本 v3")).toBeInTheDocument();
    expect(screen.queryByRole("dialog", { name: "編輯契約：寫文件" })).not.toBeInTheDocument();
  });

  it("沒有變更說明時不送 note；Runtime 拒絕時顯示說明", async () => {
    const client = fakeClient({ tasks: [existing()] });
    vi.mocked(client.updateTaskContract).mockRejectedValueOnce(new Error("Task 已結束，不能修改契約"));
    const user = await renderBoard(client);

    await openMore(user, "寫文件");
    await user.click(within(card("寫文件")).getByRole("button", { name: "編輯契約" }));
    const form = screen.getByRole("dialog", { name: "編輯契約：寫文件" });
    await user.click(within(form).getByRole("button", { name: "儲存契約" }));

    expect(await within(form).findByRole("alert")).toHaveTextContent("Task 已結束，不能修改契約");
    expect(vi.mocked(client.updateTaskContract).mock.calls[0][3]).toBeUndefined();
  });

  it("契約以對話框開啟：焦點在對話框內的第一個欄位；Esc 直接關閉、不送出，焦點回到「更多」", async () => {
    const client = fakeClient({ tasks: [existing()] });
    const user = await renderBoard(client);

    await openMore(user, "寫文件");
    await user.click(within(card("寫文件")).getByRole("button", { name: "編輯契約" }));
    const dialog = screen.getByRole("dialog", { name: "編輯契約：寫文件" });

    expect(dialog).toContainElement(document.activeElement as HTMLElement);
    expect(within(dialog).getByLabelText("目標：模式")).toHaveFocus();
    // 看板不被推下：對話框不在看板的欄位裡
    expect(screen.getAllByRole("region").some((region) => region.contains(dialog) && region.classList.contains("task-column"))).toBe(
      false,
    );

    await user.type(within(dialog).getByLabelText("目標：內容"), "（未儲存）");
    await user.keyboard("{Escape}");

    expect(screen.queryByRole("dialog", { name: "編輯契約：寫文件" })).not.toBeInTheDocument();
    expect(client.updateTaskContract).not.toHaveBeenCalled();
    expect(within(card("寫文件")).getByRole("button", { name: "更多" })).toHaveFocus();
  });

  it("「關閉」與標題列的關閉鈕都直接關閉、不送出；未儲存的修改不保留", async () => {
    const client = fakeClient({ tasks: [existing()] });
    const user = await renderBoard(client);
    const openEditor = async () => {
      await openMore(user, "寫文件");
      await user.click(within(card("寫文件")).getByRole("button", { name: "編輯契約" }));
      return screen.getByRole("dialog", { name: "編輯契約：寫文件" });
    };

    let dialog = await openEditor();
    await user.type(within(dialog).getByLabelText("目標：內容"), "（未儲存）");
    await user.click(within(dialog).getByRole("button", { name: "關閉" }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();

    dialog = await openEditor();
    expect(within(dialog).getByLabelText("目標：內容")).toHaveValue("整理文件");
    await user.click(within(dialog).getByRole("button", { name: "關閉對話框" }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(client.updateTaskContract).not.toHaveBeenCalled();
  });

  it("建立 Task 的對話框：焦點在第一個欄位，Esc 關閉後焦點回到「建立 Task」", async () => {
    const client = fakeClient({ tasks: [] });
    const user = await renderBoard(client);
    const create = screen.getByRole("button", { name: "建立 Task" });

    await user.click(create);
    const dialog = screen.getByRole("dialog", { name: "建立 Task" });
    expect(within(dialog).getByLabelText("Goal")).toHaveFocus();

    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog", { name: "建立 Task" })).not.toBeInTheDocument();
    expect(client.createTask).not.toHaveBeenCalled();
    expect(create).toHaveFocus();
  });

  it("已完成或已取消的 Task 不能編輯契約", async () => {
    const user = await renderBoard(fakeClient({ tasks: [task("task_1", "舊的", { status: "completed" })] }));

    await openMore(user, "舊的");
    // 已完成且沒有進行中的 Run：沒有「更多」，也就沒有收在裡面的動作
    expect(within(card("舊的")).queryByRole("button", { name: "更多" })).not.toBeInTheDocument();
    expect(within(card("舊的")).queryByRole("button", { name: "編輯契約" })).not.toBeInTheDocument();
    expect(within(card("舊的")).queryByRole("button", { name: "新增相依" })).not.toBeInTheDocument();
  });
});

describe("成果驗證", () => {
  const verifyingTask = () => task("task_1", "部署", { status: "verifying", contract: filledContract("task_1") });
  const panel = (title: string) => within(card(title)).getByRole("group", { name: "成果驗證" });

  it("verifying 卡片顯示「驗證中」而非已完成；檢視驗證後列出檢查、成果 manifest 與自動檢查說明", async () => {
    const client = fakeClient({ tasks: [verifyingTask()], runs: [run("run_1", "task_1", "succeeded")] });
    const user = await renderBoard(client);

    expect(panel("部署")).toHaveTextContent("驗證中");
    expect(card("部署")).not.toHaveTextContent("已完成");
    expect(client.taskVerification).not.toHaveBeenCalled();

    await user.click(within(panel("部署")).getByRole("button", { name: "檢視驗證" }));

    expect(await within(panel("部署")).findByText("正在執行 2 項確定性檢查，全部通過後自動完成")).toBeInTheDocument();
    // 「驗證中」的解釋改放在「?」說明
    expect(within(panel("部署")).getByRole("button", { name: "驗證中的說明" })).toBeInTheDocument();
    expect(client.taskVerification).toHaveBeenCalledWith("ws_1", "task_1");
    expect(panel("部署")).toHaveTextContent("指令 cargo test（預期結束碼 0）");
    expect(panel("部署")).toHaveTextContent("成果 report.md（存在即可）");
    expect(panel("部署")).toHaveTextContent(`report.md v1，SHA-256 ${REPORT_SHA256.slice(0, 12)}…`);
    expect(panel("部署")).not.toHaveTextContent(REPORT_SHA256);
    expect(within(panel("部署")).getByRole("button", { name: "接受成果" })).toBeEnabled();
  });

  it("automatic 為 false 時說明只能手動接受，仍可接受成果", async () => {
    const user = await renderBoard(
      fakeClient({ tasks: [verifyingTask()], verification: verificationView({ automatic: false, checks: [] }) }),
    );

    await user.click(within(panel("部署")).getByRole("button", { name: "檢視驗證" }));

    expect(await within(panel("部署")).findByText("沒有自動檢查，需要你手動接受成果")).toBeInTheDocument();
    expect(within(panel("部署")).queryByText("宣告的檢查")).not.toBeInTheDocument();
    expect(within(panel("部署")).getByRole("button", { name: "接受成果" })).toBeEnabled();
  });

  it("接受成果時把 view 的 basis 原樣送回並附上附註；成功後移到「已完成」並顯示已通過驗證（手動接受）", async () => {
    const client = fakeClient({ tasks: [verifyingTask()] });
    const user = await renderBoard(client);

    await user.click(within(panel("部署")).getByRole("button", { name: "檢視驗證" }));
    await user.type(await within(panel("部署")).findByLabelText("附註（選填）"), "已人工確認報告");
    await user.click(within(panel("部署")).getByRole("button", { name: "接受成果" }));

    expect(client.acceptTask).toHaveBeenCalledWith("ws_1", "task_1", { basis: verificationView().basis, note: "已人工確認報告" });
    const completed = await within(column("已完成（1）")).findByRole("article", { name: "部署" });
    expect(within(completed).getByRole("group", { name: "驗證紀錄" })).toHaveTextContent("已通過驗證（手動接受）");
    expect(within(completed).queryByRole("group", { name: "成果驗證" })).not.toBeInTheDocument();
  });

  it("沒有附註時送出 null", async () => {
    const client = fakeClient({ tasks: [verifyingTask()] });
    const user = await renderBoard(client);

    await user.click(within(panel("部署")).getByRole("button", { name: "檢視驗證" }));
    await user.click(await within(panel("部署")).findByRole("button", { name: "接受成果" }));

    expect(client.acceptTask).toHaveBeenCalledWith("ws_1", "task_1", { basis: verificationView().basis, note: null });
  });

  it("接受被拒（409，依據過期）時顯示 Runtime 的說明並重新載入驗證資料", async () => {
    const client = fakeClient({ tasks: [verifyingTask()] });
    vi.mocked(client.acceptTask).mockRejectedValueOnce(new Error("Run 世代已變更，請重新檢視驗證後再接受"));
    const newer = verificationView({
      basis: { run_id: "run_2", generation: 2, contract_version: 1, artifacts: [{ artifact_id: "art_1", version: 2 }] },
      artifacts: [{ artifact_id: "art_1", name: "report.md", version: 2, sha256: "f".repeat(64) }],
    });
    vi.mocked(client.taskVerification).mockResolvedValueOnce(verificationView()).mockResolvedValueOnce(newer);
    const user = await renderBoard(client);

    await user.click(within(panel("部署")).getByRole("button", { name: "檢視驗證" }));
    await user.click(await within(panel("部署")).findByRole("button", { name: "接受成果" }));

    expect(await within(panel("部署")).findByRole("alert")).toHaveTextContent("Run 世代已變更，請重新檢視驗證後再接受");
    expect(await within(panel("部署")).findByText(/report\.md v2/)).toBeInTheDocument();
    expect(client.taskVerification).toHaveBeenCalledTimes(2);
    expect(column("驗證中（1）")).toBeInTheDocument();

    // 重新載入後再接受，送出的是新的依據
    await user.click(within(panel("部署")).getByRole("button", { name: "接受成果" }));
    expect(client.acceptTask).toHaveBeenLastCalledWith("ws_1", "task_1", { basis: newer.basis, note: null });
  });

  it("Task 已不在驗證中時，說明目前狀態且不能接受", async () => {
    const client = fakeClient({ tasks: [verifyingTask()] });
    vi.mocked(client.taskVerification).mockResolvedValueOnce(verificationView({ status: "completed", basis: null }));
    const user = await renderBoard(client);

    await user.click(within(panel("部署")).getByRole("button", { name: "檢視驗證" }));

    expect(await within(panel("部署")).findByText("Task 目前是「已完成」，不能接受成果")).toBeInTheDocument();
    expect(within(panel("部署")).getByRole("button", { name: "接受成果" })).toBeDisabled();
  });

  it("讀不到驗證資料時顯示原因", async () => {
    const client = fakeClient({ tasks: [verifyingTask()] });
    vi.mocked(client.taskVerification).mockRejectedValueOnce(new Error("Task 不存在"));
    const user = await renderBoard(client);

    await user.click(within(panel("部署")).getByRole("button", { name: "檢視驗證" }));

    expect(await within(panel("部署")).findByRole("alert")).toHaveTextContent("無法取得驗證資料：Task 不存在");
  });

  it("上次驗證未通過：ready 卡片列出未通過的檢查與證據，預設收合", async () => {
    const user = await renderBoard(
      fakeClient({
        tasks: [task("task_1", "部署", { status: "ready", contract: filledContract("task_1") })],
        verifications: [
          verificationRecord({
            outcome: "failed",
            decided_at: T0 + 3_600_000,
            checks: [
              {
                check: { kind: "command", program: "cargo", args: ["test"], expected_exit_code: 0 },
                status: "failed",
                detail: "結束碼 101：2 個測試失敗",
              },
              { check: { kind: "file", path: "out/report.md", sha256: null }, status: "passed", detail: "檔案存在" },
            ],
          }),
        ],
      }),
    );

    const record = within(card("部署")).getByRole("group", { name: "驗證紀錄" });
    expect(record).toHaveTextContent("上次驗證未通過（第 1 次 Run，2023/11/14 23:13）");
    const details = record.querySelector("details")!;
    expect(details).not.toHaveAttribute("open");
    await user.click(within(details).getByText("未通過的檢查（1）"));
    expect(details).toHaveAttribute("open");
    expect(details).toHaveTextContent("指令 cargo test（預期結束碼 0）：結束碼 101：2 個測試失敗");
    expect(details).not.toHaveTextContent("out/report.md");
  });

  it("上次驗證不完整：列出不完整的檢查與證據（預設收合），不顯示為未通過", async () => {
    const user = await renderBoard(
      fakeClient({
        tasks: [task("task_1", "部署", { status: "ready", contract: filledContract("task_1") })],
        verifications: [
          verificationRecord({
            outcome: "inconclusive",
            decided_at: T0 + 3_600_000,
            note: "測試環境無法連線",
            checks: [
              {
                check: { kind: "command", program: "cargo", args: ["test"], expected_exit_code: 0 },
                status: "inconclusive",
                detail: "無法啟動 cargo：找不到執行檔",
              },
              { check: { kind: "file", path: "out/report.md", sha256: null }, status: "passed", detail: "檔案存在" },
            ],
          }),
        ],
      }),
    );

    const record = within(card("部署")).getByRole("group", { name: "驗證紀錄" });
    expect(record).toHaveTextContent("上次驗證不完整（第 1 次 Run，2023/11/14 23:13）");
    expect(record).toHaveTextContent("測試環境無法連線");
    expect(record).not.toHaveTextContent("未通過");
    const details = record.querySelector("details")!;
    expect(details).not.toHaveAttribute("open");
    await user.click(within(details).getByText("不完整的檢查（1）"));
    expect(details).toHaveTextContent("指令 cargo test（預期結束碼 0）：無法啟動 cargo：找不到執行檔");
    expect(details).not.toHaveTextContent("out/report.md");
  });

  it("只看該 Task 最新的驗證紀錄：先未通過、後自動檢查通過時顯示已通過驗證（自動檢查）", async () => {
    await renderBoard(
      fakeClient({
        tasks: [task("task_1", "部署", { status: "completed" }), task("task_2", "其他", { status: "ready" })],
        verifications: [verificationRecord({ id: "ver_1", outcome: "failed" }), verificationRecord({ id: "ver_2", generation: 2 })],
      }),
    );

    const record = within(card("部署")).getByRole("group", { name: "驗證紀錄" });
    expect(record).toHaveTextContent("已通過驗證（自動檢查）");
    expect(record).not.toHaveTextContent("未通過");
    expect(within(card("其他")).queryByRole("group", { name: "驗證紀錄" })).not.toBeInTheDocument();
  });
});

describe("確定性檢查編輯", () => {
  const UPPER_HASH = "ABCDEF0123456789".repeat(4);

  async function openCreate(client: TaskBoardClient) {
    const user = await renderBoard(client);
    await user.click(screen.getByRole("button", { name: "建立 Task" }));
    const form = screen.getByRole("dialog", { name: "建立 Task" });
    await user.type(within(form).getByLabelText("Task 標題"), "寫文件");
    return { user, form };
  }

  it("三種檢查與自訂結束碼都解析進契約；雜湊轉成小寫", async () => {
    const client = fakeClient();
    const { user, form } = await openCreate(client);

    await user.click(within(form).getByLabelText("確定性檢查（每行一項）"));
    await user.paste(
      [
        "command: cargo test --all",
        "command[3]: pwsh -File check.ps1",
        "",
        "file: out/report.md",
        `file: out/data.csv ${UPPER_HASH}`,
        `artifact: report.md ${REPORT_SHA256}`,
      ].join("\n"),
    );
    await user.click(within(form).getByRole("button", { name: "建立" }));

    await waitFor(() => expect(client.createTask).toHaveBeenCalled());
    expect(vi.mocked(client.createTask).mock.calls[0][1].contract?.verification_checks).toEqual([
      { kind: "command", program: "cargo", args: ["test", "--all"], expected_exit_code: 0 },
      { kind: "command", program: "pwsh", args: ["-File", "check.ps1"], expected_exit_code: 3 },
      { kind: "file", path: "out/report.md", sha256: null },
      { kind: "file", path: "out/data.csv", sha256: UPPER_HASH.toLowerCase() },
      { kind: "artifact", name: "report.md", sha256: REPORT_SHA256 },
    ]);
  });

  it("格式錯誤時逐行列出並且不送出", async () => {
    const client = fakeClient();
    const { user, form } = await openCreate(client);

    await user.click(within(form).getByLabelText("確定性檢查（每行一項）"));
    await user.paste(
      ["cmd: cargo test", "command:", "file: a b c", "artifact: report.md xyz", "file[2]: out/a", "command: cargo build"].join("\n"),
    );
    await user.click(within(form).getByRole("button", { name: "建立" }));

    const alert = within(form).getByRole("alert");
    expect(alert).toHaveTextContent("確定性檢查：格式錯誤「cmd: cargo test」");
    expect(alert).toHaveTextContent("確定性檢查：「command:」缺少要執行的程式");
    expect(alert).toHaveTextContent("確定性檢查：「file: a b c」應為 file: 相對路徑 [sha256]");
    expect(alert).toHaveTextContent("確定性檢查：「artifact: report.md xyz」的 sha256 應為 64 位十六進位");
    expect(alert).toHaveTextContent("確定性檢查：「file[2]: out/a」只有 command 可以指定結束碼");
    expect(alert).not.toHaveTextContent("cargo build");
    expect(client.createTask).not.toHaveBeenCalled();
  });

  it("編輯契約時載入既有的檢查，原樣送回", async () => {
    const checks: TaskContract["verification_checks"] = [
      { kind: "command", program: "cargo", args: ["test"], expected_exit_code: 0 },
      { kind: "command", program: "pwsh", args: ["-File", "check.ps1"], expected_exit_code: 3 },
      { kind: "artifact", name: "report.md", sha256: REPORT_SHA256 },
    ];
    const client = fakeClient({ tasks: [task("task_1", "寫文件", { contract: contract("task_1", { verification_checks: checks }) })] });
    const user = await renderBoard(client);

    await openMore(user, "寫文件");
    await user.click(within(card("寫文件")).getByRole("button", { name: "編輯契約" }));
    const form = screen.getByRole("dialog", { name: "編輯契約：寫文件" });
    expect(within(form).getByLabelText("確定性檢查（每行一項）")).toHaveValue(
      ["command: cargo test", "command[3]: pwsh -File check.ps1", `artifact: report.md ${REPORT_SHA256}`].join("\n"),
    );
    await user.click(within(form).getByRole("button", { name: "儲存契約" }));

    await waitFor(() => expect(client.updateTaskContract).toHaveBeenCalled());
    expect(vi.mocked(client.updateTaskContract).mock.calls[0][2].verification_checks).toEqual(checks);
  });

  it("清空檢查後不送這個欄位（Runtime 視為空陣列）", async () => {
    const checks: TaskContract["verification_checks"] = [{ kind: "file", path: "out/report.md", sha256: null }];
    const client = fakeClient({ tasks: [task("task_1", "寫文件", { contract: contract("task_1", { verification_checks: checks }) })] });
    const user = await renderBoard(client);

    await openMore(user, "寫文件");
    await user.click(within(card("寫文件")).getByRole("button", { name: "編輯契約" }));
    const form = screen.getByRole("dialog", { name: "編輯契約：寫文件" });
    await user.clear(within(form).getByLabelText("確定性檢查（每行一項）"));
    await user.click(within(form).getByRole("button", { name: "儲存契約" }));

    await waitFor(() => expect(client.updateTaskContract).toHaveBeenCalled());
    expect(vi.mocked(client.updateTaskContract).mock.calls[0][2]).not.toHaveProperty("verification_checks");
  });
});

describe("Task 種類、工作目錄與整合佇列", () => {
  const devTask = (id = "task_1", title = "加登入頁", overrides: Partial<TaskView> = {}) =>
    task(id, title, { kind: "development", status: "ready", contract: filledContract(id), ...overrides });
  const workdir = (title: string) => within(card(title)).getByRole("group", { name: "工作目錄" });
  const integration = (title: string) => within(card(title)).getByRole("group", { name: "整合狀態" });
  const queueRegion = () => screen.getByRole("region", { name: "整合佇列" });

  it("建立 Task 時種類預設「研究」，可改選「程式開發」並送出 kind；卡片顯示種類", async () => {
    const client = fakeClient();
    const user = await renderBoard(client);
    await user.click(screen.getByRole("button", { name: "建立 Task" }));
    const form = screen.getByRole("dialog", { name: "建立 Task" });

    const kind = within(form).getByLabelText("種類");
    expect(kind).toHaveDisplayValue("研究");
    expect(within(kind).getAllByRole("option").map((o) => o.textContent)).toEqual(["研究", "程式開發", "文件"]);
    await user.selectOptions(kind, "程式開發");
    await user.type(within(form).getByLabelText("Task 標題"), "加登入頁");
    await user.click(within(form).getByRole("button", { name: "建立" }));

    const created = await within(column("待處理（1）")).findByRole("article", { name: "加登入頁" });
    expect(client.createTask).toHaveBeenCalledWith("ws_1", expect.objectContaining({ kind: "development" }));
    expect(within(created).getByText("程式開發")).toBeInTheDocument();
  });

  it("編輯契約時沒有種類選擇", async () => {
    const user = await renderBoard(fakeClient({ tasks: [task("task_1", "寫文件", { kind: "document" })] }));

    expect(within(card("寫文件")).getByText("文件")).toBeInTheDocument();
    await openMore(user, "寫文件");
    await user.click(within(card("寫文件")).getByRole("button", { name: "編輯契約" }));
    expect(within(screen.getByRole("dialog", { name: "編輯契約：寫文件" })).queryByLabelText("種類")).not.toBeInTheDocument();
  });

  it("開發 Task 的卡片顯示 worktree：路徑、平台分支與整合目標、狀態與在其中執行過的 Run", async () => {
    await renderBoard(
      fakeClient({
        tasks: [devTask("task_1", "加登入頁", { status: "running" }), task("task_2", "研究競品")],
        runs: [
          run("run_1", "task_1", "failed", { generation: 1 }),
          run("run_2", "task_1", "running", { generation: 2 }),
        ],
        workdirs: [workdirView({ run_ids: ["run_1", "run_2"] })],
      }),
    );

    const group = workdir("加登入頁");
    expect(group).toHaveTextContent("Worktree：C:\\work\\app\\.baton\\worktrees\\task_1");
    expect(group).toHaveTextContent("分支 baton/task_1 → 整合到 main");
    expect(group).toHaveTextContent("已就緒");
    expect(group).toHaveTextContent("在此目錄執行過的 Run：第 1 次、第 2 次");
    expect(within(group).queryByRole("alert")).not.toBeInTheDocument();
    // 工作目錄正常時收在預設收合的「詳細資訊」
    expect(group.closest("details")).not.toHaveAttribute("open");
    // 還沒有工作目錄的 Task 不顯示這一區
    expect(within(card("研究競品")).queryByRole("group", { name: "工作目錄" })).not.toBeInTheDocument();
  });

  it("研究 Task 的輸出目錄不顯示分支；尚未有 Run 時說明", async () => {
    await renderBoard(
      fakeClient({
        tasks: [task("task_1", "研究競品", { status: "ready", contract: filledContract("task_1") })],
        workdirs: [
          workdirView({
            kind: "output_dir",
            path: "C:\\work\\app\\.baton\\outputs\\task_1",
            branch: null,
            base_branch: null,
            run_ids: [],
          }),
        ],
      }),
    );

    const group = workdir("研究競品");
    expect(group).toHaveTextContent("輸出目錄：C:\\work\\app\\.baton\\outputs\\task_1");
    expect(group).not.toHaveTextContent("分支");
    expect(group).toHaveTextContent("尚未有 Run 在此目錄執行");
  });

  it("工作目錄準備失敗時醒目顯示原因（不會派工）", async () => {
    await renderBoard(
      fakeClient({
        tasks: [devTask()],
        workdirs: [
          workdirView({
            status: "failed",
            path: "C:\\work\\notes\\.baton\\worktrees\\task_1",
            branch: null,
            base_branch: null,
            failure: "Workspace 不是 Git repo，無法建立 worktree",
            ready_at: null,
            run_ids: [],
          }),
        ],
      }),
    );

    const group = workdir("加登入頁");
    expect(group).toHaveTextContent("準備失敗");
    expect(within(group).getByRole("alert")).toHaveTextContent(
      "工作目錄無法使用，不會派工：Workspace 不是 Git repo，無法建立 worktree",
    );
    // 失敗的摘要在「詳細資訊」外的狀態列，收合時也看得到
    expect(group.closest("details")).not.toHaveAttribute("open");
    const status = statusLine("加登入頁");
    expect(status.closest("details")).toBeNull();
    expect(status).toHaveTextContent("目錄無法使用Workspace 不是 Git repo，無法建立 worktree");
  });

  it("同時有多個 danger 時，狀態列在第一項後面加上第二個徽章，工作目錄失敗不被預算用盡蓋住", async () => {
    await renderBoard(
      fakeClient({
        tasks: [devTask("task_1", "加登入頁", { status: "blocked" })],
        workdirs: [workdirView({ status: "failed", failure: "Workspace 不是 Git repo，無法建立 worktree", run_ids: [] })],
        budgets: [budgetView({ exhausted: "implementation" })],
      }),
    );

    const status = statusLine("加登入頁");
    const badges = Array.from(status.querySelectorAll(".badge")).map((badge) => badge.textContent);
    expect(badges).toEqual(["預算用盡", "目錄無法使用"]);
    expect(status.querySelectorAll(".badge-danger")).toHaveLength(2);
    // 第二項的原因放在徽章的 title，主文字仍是第一項的說明
    expect(within(status).getByText("目錄無法使用")).toHaveAttribute("title", "Workspace 不是 Git repo，無法建立 worktree");
    expect(status).toHaveTextContent("實作失敗修正預算已用完");
  });

  it("超過兩個 danger 時，第三項以後收成「+N」徽章，可存取名稱列出其餘項目", async () => {
    await renderBoard(
      fakeClient({
        tasks: [devTask("task_1", "加登入頁", { status: "blocked" })],
        workdirs: [
          workdirView({
            status: "failed",
            failure: "無法建立 worktree",
            run_ids: [],
            cleanup: worktreeCleanup({ status: "failed", failure: "檔案被鎖住" }),
          }),
        ],
        budgets: [budgetView({ exhausted: "implementation" })],
      }),
    );

    const status = statusLine("加登入頁");
    const badges = Array.from(status.querySelectorAll(".badge")).map((badge) => badge.textContent);
    expect(badges).toEqual(["預算用盡", "目錄無法使用", "+1"]);
    expect(within(status).getByText("+1")).toHaveAttribute("aria-label", "另有 1 項：清理失敗");
    expect(within(status).getByText("+1")).toHaveAttribute("title", "清理失敗：檔案被鎖住");
  });

  it("等待原因有「工作目錄」類別時分開標示", async () => {
    await renderBoard(
      fakeClient({
        tasks: [devTask()],
        queue: [
          {
            workspace_id: "ws_1",
            task_id: "task_1",
            title: "加登入頁",
            priority: 0,
            requested_at: T0,
            position: 1,
            reasons: [{ category: "workdir", code: "workdir_unavailable", message: "Workspace 不是 Git repo" }],
          },
        ],
      }),
    );

    const queued = within(card("加登入頁")).getByRole("group", { name: "排隊原因" });
    const item = within(queued).getByText("工作目錄").closest("li")!;
    expect(within(item).getByText("Workspace 不是 Git repo")).toBeInTheDocument();
  });

  it("開發 Task 通過驗證、等待整合時說明「成果已通過驗證，等待整合」，不顯示手動接受面板也不是已完成", async () => {
    await renderBoard(
      fakeClient({
        tasks: [devTask("task_1", "加登入頁", { status: "verifying" })],
        runs: [run("run_1", "task_1", "succeeded")],
        verifications: [verificationRecord()],
        integrations: [integrationEntry({ status: "verifying", started_at: T0 })],
      }),
    );

    const group = integration("加登入頁");
    expect(group).toHaveTextContent("成果已通過驗證，等待整合");
    expect(group).toHaveTextContent("整合驗證中");
    expect(group).toHaveTextContent("baton/task_1 → main");
    expect(within(card("加登入頁")).queryByRole("group", { name: "成果驗證" })).not.toBeInTheDocument();
    expect(card("加登入頁")).not.toHaveTextContent("已完成");
  });

  it("未通過整合：顯示原因與未通過的整合檢查", async () => {
    await renderBoard(
      fakeClient({
        tasks: [devTask()],
        integrations: [
          integrationEntry({
            status: "failed",
            failure: "整合驗證未通過，目標分支未變更",
            checks: [
              {
                check: { kind: "command", program: "cargo", args: ["test"], expected_exit_code: 0 },
                status: "failed",
                detail: "結束碼 101：1 個測試失敗",
              },
              { check: { kind: "file", path: "out/report.md", sha256: null }, status: "passed", detail: "檔案存在" },
            ],
            finished_at: T0,
          }),
        ],
      }),
    );

    const group = integration("加登入頁");
    expect(group).toHaveTextContent("未通過整合");
    expect(group).toHaveTextContent("整合驗證未通過，目標分支未變更");
    expect(group).toHaveTextContent("指令 cargo test（預期結束碼 0）：結束碼 101：1 個測試失敗");
    expect(group).not.toHaveTextContent("out/report.md");
  });

  it("未通過整合時，不完整的整合檢查與未通過的分開列出", async () => {
    await renderBoard(
      fakeClient({
        tasks: [devTask()],
        integrations: [
          integrationEntry({
            status: "failed",
            failure: "整合驗證不完整，目標分支未變更",
            checks: [
              {
                check: { kind: "command", program: "cargo", args: ["test"], expected_exit_code: 0 },
                status: "inconclusive",
                detail: "磁碟空間不足，無法建置",
              },
            ],
            finished_at: T0,
          }),
        ],
      }),
    );

    const group = integration("加登入頁");
    expect(group).toHaveTextContent("不完整的整合檢查（1）");
    expect(group).toHaveTextContent("指令 cargo test（預期結束碼 0）：磁碟空間不足，無法建置");
    expect(group).not.toHaveTextContent("未通過的整合檢查");
  });

  it("已整合：顯示合併 commit 前 8 碼；只看該 Task 最新的整合項目", async () => {
    const merge = "a1b2c3d4e5f60718293a4b5c6d7e8f9012345678";
    await renderBoard(
      fakeClient({
        tasks: [devTask("task_1", "加登入頁", { status: "completed" })],
        integrations: [
          integrationEntry({ id: "int_1", seq: 1, status: "failed", failure: "合併衝突" }),
          integrationEntry({ id: "int_2", seq: 2, run_id: "run_2", generation: 2, status: "integrated", merge_commit: merge }),
        ],
      }),
    );

    const group = integration("加登入頁");
    expect(group).toHaveTextContent("已整合");
    expect(group).toHaveTextContent("合併 commit a1b2c3d4");
    expect(group).not.toHaveTextContent(merge);
    expect(group).not.toHaveTextContent("合併衝突");
  });

  it("整合佇列依 seq 列出尚未結束的項目並標出處理中的一個；已整合與未通過的不列出", async () => {
    await renderBoard(
      fakeClient({
        tasks: [devTask("task_1", "加登入頁"), devTask("task_2", "修 bug"), devTask("task_3", "改樣式"), devTask("task_4", "舊的")],
        integrations: [
          integrationEntry({ id: "int_4", seq: 4, task_id: "task_4", status: "integrated" }),
          integrationEntry({ id: "int_7", seq: 7, task_id: "task_3", branch: "baton/task_3" }),
          integrationEntry({ id: "int_5", seq: 5, task_id: "task_1", branch: "baton/task_1", status: "merging" }),
          integrationEntry({ id: "int_6", seq: 6, task_id: "task_2", branch: "baton/task_2" }),
          integrationEntry({ id: "int_3", seq: 3, task_id: "task_4", status: "failed" }),
        ],
      }),
    );

    const items = within(queueRegion()).getAllByRole("listitem");
    expect(items).toHaveLength(3);
    // 一項一列：標題、分支（來源 → 目標）、狀態依序排列
    expect(items[0]).toHaveTextContent(/^加登入頁baton\/task_1 → main合併中/);
    expect(items[0]).toHaveTextContent("處理中");
    expect(items[0]).toHaveAttribute("aria-current", "step");
    expect(items[1]).toHaveTextContent(/^修 bugbaton\/task_2 → main排隊等待整合/);
    expect(items[1]).not.toHaveAttribute("aria-current");
    expect(items[1]).not.toHaveTextContent("處理中");
    expect(items[2]).toHaveTextContent("改樣式");
    expect(queueRegion()).not.toHaveTextContent("舊的");
  });

  it("沒有等待整合的項目時說明", async () => {
    await renderBoard(fakeClient({ tasks: [devTask()], integrations: [integrationEntry({ status: "integrated" })] }));

    expect(queueRegion()).toHaveTextContent("沒有等待整合的項目");
    expect(within(queueRegion()).queryByRole("listitem")).not.toBeInTheDocument();
  });

  it("檢視驗證時顯示這個 Task 最新的整合項目", async () => {
    const user = await renderBoard(
      fakeClient({
        tasks: [devTask("task_1", "加登入頁", { status: "verifying" })],
        verification: verificationView({ integration: integrationEntry({ status: "failed", failure: "合併衝突" }) }),
      }),
    );

    const panel = within(card("加登入頁")).getByRole("group", { name: "成果驗證" });
    await user.click(within(panel).getByRole("button", { name: "檢視驗證" }));

    expect(await within(panel).findByText("最新整合項目：未通過整合（第 1 次 Run）")).toBeInTheDocument();
  });
});

describe("worktree 清理", () => {
  const completedDevTask = () =>
    task("task_1", "加登入頁", { kind: "development", status: "completed", contract: filledContract("task_1") });
  const cleanupRegion = () => within(card("加登入頁")).getByRole("status", { name: "清理狀態" });

  it("已完成的 Task 待清理且上次失敗：同時顯示 Task 已完成、待清理、失敗原因與自動重試時間", async () => {
    await renderBoard(
      fakeClient({
        tasks: [completedDevTask()],
        integrations: [integrationEntry({ status: "integrated" })],
        workdirs: [
          workdirView({
            cleanup: worktreeCleanup({
              attempts: 1,
              failure: "檔案可能被其他程式占用，請關閉後再試",
              next_attempt_at: T0 + 5 * 60_000,
            }),
          }),
        ],
      }),
    );

    // Task 完成與清理狀態分開顯示：卡片仍在「已完成」欄
    expect(within(column("已完成（1）")).getByRole("article", { name: "加登入頁" })).toBeInTheDocument();
    const region = cleanupRegion();
    expect(region).toHaveTextContent("待清理");
    expect(region).toHaveTextContent("上次清理失敗：檔案可能被其他程式占用，請關閉後再試");
    expect(region).toHaveTextContent("將於 2023/11/14 22:18 自動重試");
    expect(within(region).queryByRole("alert")).not.toBeInTheDocument();
  });

  it("待清理且沒有失敗時只顯示「待清理」；清理中顯示「清理中」", async () => {
    await renderBoard(
      fakeClient({
        tasks: [completedDevTask(), task("task_2", "修 bug", { kind: "development", status: "completed" })],
        workdirs: [
          // 剛登記的待清理：下一次嘗試時間就是登記時間，還沒有失敗，不顯示重試時間
          workdirView({ cleanup: worktreeCleanup({ next_attempt_at: T0 }) }),
          workdirView({
            task_id: "task_2",
            path: "C:\\work\\app\\.baton\\worktrees\\task_2",
            branch: "baton/task_2",
            cleanup: worktreeCleanup({ task_id: "task_2", status: "removing", attempts: 1 }),
          }),
        ],
      }),
    );

    expect(cleanupRegion()).toHaveTextContent("待清理");
    expect(cleanupRegion()).not.toHaveTextContent("上次清理失敗");
    expect(cleanupRegion()).not.toHaveTextContent("自動重試");
    expect(within(card("修 bug")).getByRole("status", { name: "清理狀態" })).toHaveTextContent("清理中");
  });

  it("清理失敗：以 alert 顯示原因且 worktree 保留，Task 仍顯示已完成", async () => {
    await renderBoard(
      fakeClient({
        tasks: [completedDevTask()],
        workdirs: [
          workdirView({
            cleanup: worktreeCleanup({
              status: "failed",
              attempts: 3,
              failure: "worktree 中有 2 個未保存的變更",
              finished_at: T0,
            }),
          }),
        ],
      }),
    );

    expect(within(column("已完成（1）")).getByRole("article", { name: "加登入頁" })).toBeInTheDocument();
    const region = cleanupRegion();
    expect(region).toHaveTextContent("清理失敗");
    expect(within(region).getByRole("alert")).toHaveTextContent("清理失敗，worktree 已保留：worktree 中有 2 個未保存的變更");
    // 失敗的摘要在「詳細資訊」外的狀態列，收合時也看得到
    expect(region.closest("details")).not.toHaveAttribute("open");
    expect(statusLine("加登入頁")).toHaveTextContent("清理失敗worktree 中有 2 個未保存的變更");
    expect(region).not.toHaveTextContent("自動重試");
    // worktree 保留：路徑不標成已移除
    expect(within(card("加登入頁")).getByRole("group", { name: "工作目錄" })).not.toHaveTextContent("已移除");
  });

  it("已清理：顯示清理完成時間，worktree 路徑標成已移除", async () => {
    await renderBoard(
      fakeClient({
        tasks: [completedDevTask()],
        workdirs: [workdirView({ cleanup: worktreeCleanup({ status: "cleaned", attempts: 1, finished_at: T0 }) })],
      }),
    );

    expect(cleanupRegion()).toHaveTextContent("已清理（2023/11/14 22:13）");
    expect(within(cleanupRegion()).queryByRole("alert")).not.toBeInTheDocument();
    expect(within(card("加登入頁")).getByRole("group", { name: "工作目錄" })).not.toHaveTextContent("已就緒");
    expect(within(card("加登入頁")).getByRole("group", { name: "工作目錄" })).toHaveTextContent(
      "Worktree：C:\\work\\app\\.baton\\worktrees\\task_1（已移除）",
    );
  });

  it("沒有清理登記時不顯示清理狀態", async () => {
    await renderBoard(fakeClient({ tasks: [completedDevTask()], workdirs: [workdirView()] }));

    expect(within(card("加登入頁")).getByRole("group", { name: "工作目錄" })).not.toHaveTextContent("已移除");
    expect(within(card("加登入頁")).queryByRole("status", { name: "清理狀態" })).not.toBeInTheDocument();
  });
});

describe("修正與重試預算", () => {
  const budgets = (taskTitle: string) => within(card(taskTitle)).queryByRole("group", { name: "修正與重試預算" });

  /** 依序設定三種計數的已用次數。 */
  const used = (implementation: number, verification: number, infrastructure: number) =>
    budgetView().counters.map((counter, index) => ({ ...counter, used: [implementation, verification, infrastructure][index] }));

  it("有計數時列出三種預算的已用次數與上限", async () => {
    await renderBoard(
      fakeClient({
        tasks: [task("task_1", "部署", { status: "ready", contract: filledContract("task_1") })],
        budgets: [budgetView({ counters: used(1, 0, 2) })],
      }),
    );

    const group = budgets("部署")!;
    const items = within(group).getAllByRole("listitem");
    expect(items.map((item) => item.textContent)).toEqual(["實作失敗修正 1/3", "驗證不完整重驗 0/2", "基礎設施重試 2/5"]);
    expect(within(group).queryByRole("alert")).not.toBeInTheDocument();
    expect(group).not.toHaveTextContent("下次重試");
  });

  it("三種計數都是 0 且沒有用盡時不顯示；沒有預算資料的 Task 也不顯示", async () => {
    await renderBoard(
      fakeClient({
        tasks: [task("task_1", "部署", { status: "ready" }), task("task_2", "其他", { status: "ready" })],
        budgets: [budgetView()],
      }),
    );

    expect(budgets("部署")).not.toBeInTheDocument();
    expect(budgets("其他")).not.toBeInTheDocument();
  });

  it("基礎設施重試退避中：顯示下次重試不早於的時間（當地時間）", async () => {
    await renderBoard(
      fakeClient({
        tasks: [task("task_1", "部署", { status: "ready" })],
        budgets: [budgetView({ counters: used(0, 0, 1), retry_not_before: T0 + 600_000 })],
      }),
    );

    expect(budgets("部署")).toHaveTextContent("下次重試不早於 2023/11/14 22:23");
  });

  it("預算用盡：提示該類預算已用完，請到「裁定」決定下一步", async () => {
    await renderBoard(
      fakeClient({
        tasks: [task("task_1", "部署", { status: "blocked" })],
        budgets: [budgetView({ counters: used(3, 1, 0), exhausted: "implementation", decision_request_id: "dec_1" })],
      }),
    );

    const group = budgets("部署")!;
    expect(within(group).getByText("實作失敗修正 3/3")).toBeInTheDocument();
    expect(within(group).getByRole("alert")).toHaveTextContent("實作失敗修正預算已用完，請到「裁定」決定下一步");
  });

  it("用盡的種類即使計數資料為 0 也會顯示提示", async () => {
    await renderBoard(
      fakeClient({
        tasks: [task("task_1", "部署", { status: "blocked" })],
        budgets: [budgetView({ exhausted: "verification" })],
      }),
    );

    expect(within(budgets("部署")!).getByRole("alert")).toHaveTextContent("驗證不完整重驗預算已用完");
  });

  it("等待原因有「重試退避」類別時分開標示", async () => {
    await renderBoard(
      fakeClient({
        tasks: [task("task_1", "部署", { status: "ready" })],
        queue: [
          {
            workspace_id: "ws_1",
            task_id: "task_1",
            title: "部署",
            priority: 0,
            requested_at: T0,
            position: 1,
            reasons: [{ category: "backoff", code: "retry_backoff", message: "基礎設施重試退避中，10 分鐘後再派工" }],
          },
        ],
      }),
    );

    const queued = within(card("部署")).getByRole("group", { name: "排隊原因" });
    const item = within(queued).getByText("重試退避").closest("li")!;
    expect(within(item).getByText("基礎設施重試退避中，10 分鐘後再派工")).toBeInTheDocument();
  });
});

describe("交接狀態", () => {
  const accounts = [accountScheduling(), accountScheduling({ profile_id: "prof_2", name: "備用帳號" })];
  const runningTask = () => task("task_1", "部署", { status: "running", contract: filledContract("task_1") });
  /** 舊 Run 已交接，承接帳號的第 2 次 Run 執行中。 */
  const transferredRuns = () => [
    run("run_1", "task_1", "interrupted", { end_reason: "handed_off", ended_at: T0 }),
    run("run_2", "task_1", "running", { profile_id: "prof_2", generation: 2 }),
  ];
  const handoffRegion = () => within(card("部署")).getByRole("group", { name: "交接狀態" });
  const runStatus = () => within(card("部署")).getByRole("group", { name: "Run 狀態" });

  it("transferred：顯示階段、以帳號名稱顯示從哪個帳號到哪個帳號、checkpoint 摘要；只看最新一筆", async () => {
    await renderBoard(
      fakeClient({
        tasks: [runningTask()],
        runs: transferredRuns(),
        accounts,
        handoffs: [
          handoff({ id: "ho_0", phase: "abandoned", abandoned_reason: "沒有可承接的帳號" }),
          handoff({
            id: "ho_1",
            phase: "transferred",
            to_run_id: "run_2",
            to_generation: 2,
            stop_outcome: "stopped_gracefully",
            checkpoint: { run_id: "run_1", generation: 1, summary: "登入表單完成，剩驗證邏輯", details: "", saved_at: T0 },
            stopping_at: T0,
            transferred_at: T0,
          }),
        ],
      }),
    );

    const region = handoffRegion();
    expect(region).toHaveTextContent("交接：已交接，等待接手確認");
    expect(region).toHaveTextContent("從 主要帳號 交接到 備用帳號（達 85% 交接門檻）");
    expect(region).toHaveTextContent("交接 checkpoint：登入表單完成，剩驗證邏輯（2023/11/14 22:13）");
    expect(region).not.toHaveTextContent("交接已放棄");
    expect(region).not.toHaveTextContent("已完成：");
  });

  it("confirmed：顯示接手者確認的已完成、未完成與未知副作用", async () => {
    await renderBoard(
      fakeClient({
        tasks: [runningTask()],
        runs: transferredRuns(),
        accounts,
        handoffs: [
          handoff({
            phase: "confirmed",
            reason: "limit_warning",
            to_run_id: "run_2",
            to_generation: 2,
            confirmation: { completed: "登入表單", incomplete: "驗證邏輯", unknown_side_effects: "無" },
            transferred_at: T0,
            confirmed_at: T0,
          }),
        ],
      }),
    );

    const region = handoffRegion();
    expect(region).toHaveTextContent("交接：已交接並確認");
    expect(region).toHaveTextContent("（Claude Code 回報限流警告）");
    expect(within(region).getByText("已完成：登入表單")).toBeInTheDocument();
    expect(within(region).getByText("未完成：驗證邏輯")).toBeInTheDocument();
    expect(within(region).getByText("未知副作用：無")).toBeInTheDocument();
  });

  it("abandoned：顯示放棄原因；帳號不在總覽時退回顯示 profile_id", async () => {
    await renderBoard(
      fakeClient({
        tasks: [runningTask()],
        runs: [run("run_1", "task_1", "running")],
        accounts,
        handoffs: [handoff({ phase: "abandoned", to_profile_id: "prof_gone", abandoned_reason: "舊程序已自行結束" })],
      }),
    );

    const region = handoffRegion();
    expect(region).toHaveTextContent("交接：交接已放棄");
    expect(region).toHaveTextContent("從 主要帳號 交接到 prof_gone");
    expect(region).toHaveTextContent("放棄原因：舊程序已自行結束");
  });

  it("結束原因是 handed_off 的 Run 標成「已交接」，不是一般的中斷", async () => {
    await renderBoard(
      fakeClient({
        tasks: [runningTask()],
        runs: [run("run_1", "task_1", "interrupted", { end_reason: "handed_off", ended_at: T0 })],
        accounts,
        handoffs: [handoff({ phase: "stopping", stopping_at: T0 })],
      }),
    );

    expect(runStatus()).toHaveTextContent("Run（第 1 次）：已交接");
    expect(runStatus()).not.toHaveTextContent("中斷");
    expect(runStatus()).not.toHaveTextContent("handed_off");
    expect(handoffRegion()).toHaveTextContent("交接：交接中：停止舊程序");
  });

  /** 突然限流而所有合適帳號都受限：舊 Run 已停止，Task 等待資源。 */
  const waitingTask = () => task("task_1", "部署", { status: "waiting_resource", contract: filledContract("task_1") });
  const waitingRuns = () => [
    run("run_1", "task_1", "interrupted", { end_reason: "handed_off_waiting_resource", ended_at: T0 }),
  ];
  const waitingRegion = () => within(handoffRegion()).getByRole("group", { name: "等待資源" });

  it("waiting_resource：有重置時間時顯示預計重新評估時間、依據重置時間後 1 分鐘與等待原因；不顯示空的承接帳號", async () => {
    const resetAt = T0 + 2 * 60 * 60 * 1000;
    await renderBoard(
      fakeClient({
        tasks: [waitingTask()],
        runs: waitingRuns(),
        accounts,
        handoffs: [
          handoff({
            phase: "waiting_resource",
            reason: "limit_reached",
            to_profile_id: "",
            stop_outcome: "stopped_gracefully",
            stopping_at: T0,
            waiting_at: T0,
            next_check_at: resetAt + 60_000,
            wait_attempts: 0,
            wait_reset_at: resetAt,
            wait_reasons: [
              { category: "account", code: "profile_limited", message: "主要帳號已達上限", profile_id: "prof_1" },
              { category: "quota", code: "quota_gate_closed", message: "備用帳號達 75% 停止派工門檻", profile_id: "prof_2" },
            ],
          }),
        ],
      }),
    );

    const region = handoffRegion();
    expect(region).toHaveTextContent("交接：等待資源：所有合適帳號都受限");
    expect(region).toHaveTextContent("從 主要帳號 交接，尚未選定承接帳號（Claude Code 回報已達上限（突然限流））");
    expect(region).not.toHaveTextContent("交接到");
    const waiting = waitingRegion();
    expect(waiting).toHaveTextContent("預計 2023/11/15 00:14 重新評估");
    expect(waiting).toHaveTextContent("依帳號重置時間（2023/11/15 00:13）後 1 分鐘");
    expect(waiting).not.toHaveTextContent("退避");
    expect(within(waiting).getByText("主要帳號已達上限")).toBeInTheDocument();
    expect(within(waiting).getByText("備用帳號達 75% 停止派工門檻")).toBeInTheDocument();
  });

  it("waiting_resource：重置時間未知時說明以退避輪詢與第幾次評估", async () => {
    await renderBoard(
      fakeClient({
        tasks: [waitingTask()],
        runs: waitingRuns(),
        accounts,
        handoffs: [
          handoff({
            phase: "waiting_resource",
            reason: "limit_reached",
            to_profile_id: "",
            stopping_at: T0,
            waiting_at: T0,
            next_check_at: T0 + 10 * 60_000,
            wait_attempts: 1,
            wait_reset_at: null,
            wait_reasons: [],
          }),
        ],
      }),
    );

    const waiting = waitingRegion();
    expect(waiting).toHaveTextContent("預計 2023/11/14 22:23 重新評估");
    expect(waiting).toHaveTextContent("重置時間未知，退避輪詢（第 2 次評估）");
    expect(waiting).not.toHaveTextContent("依帳號重置時間");
  });

  it("其他階段不顯示等待資源的區塊", async () => {
    await renderBoard(
      fakeClient({
        tasks: [runningTask()],
        runs: transferredRuns(),
        accounts,
        handoffs: [handoff({ phase: "transferred", reason: "limit_reached", to_run_id: "run_2", transferred_at: T0 })],
      }),
    );

    const region = handoffRegion();
    expect(region).toHaveTextContent("從 主要帳號 交接到 備用帳號（Claude Code 回報已達上限（突然限流））");
    expect(within(region).queryByRole("group", { name: "等待資源" })).not.toBeInTheDocument();
  });

  it("結束原因是 handed_off_waiting_resource 的 Run 標成「已停止（等待資源）」，不另列結束原因", async () => {
    await renderBoard(
      fakeClient({
        tasks: [waitingTask()],
        runs: waitingRuns(),
        accounts,
        handoffs: [handoff({ phase: "waiting_resource", reason: "limit_reached", to_profile_id: "", next_check_at: T0 })],
      }),
    );

    expect(runStatus()).toHaveTextContent("Run（第 1 次）：已停止（等待資源）");
    expect(runStatus()).not.toHaveTextContent("中斷");
    expect(runStatus()).not.toHaveTextContent("結束原因");
  });

  it("沒有交接時不顯示交接狀態", async () => {
    await renderBoard(fakeClient({ tasks: [runningTask()], runs: [run("run_1", "task_1", "running")], handoffs: [] }));

    expect(within(card("部署")).queryByRole("group", { name: "交接狀態" })).not.toBeInTheDocument();
  });
});

describe("Context receipt", () => {
  const runningTask = () => task("task_1", "部署", { status: "running", contract: filledContract("task_1") });
  const receipt = () => within(card("部署")).getByRole("group", { name: "Context receipt" });

  it("展開後才載入：顯示預算、依優先序的已載入項目、省略項目與原因，以及實際 Prompt 原文", async () => {
    const client = fakeClient({ tasks: [runningTask()], runs: [run("run_1", "task_1", "running")] });
    const user = await renderBoard(client);

    expect(within(card("部署")).getByRole("group", { name: "Run 狀態" })).toContainElement(receipt());
    expect(client.runContext).not.toHaveBeenCalled();

    await user.click(within(receipt()).getByRole("button", { name: "Context receipt" }));

    expect(await within(receipt()).findByText("已用 1234 / 8000 tokens")).toBeInTheDocument();
    expect(client.runContext).toHaveBeenCalledWith("ws_1", "run_1");
    const loaded = within(within(receipt()).getByRole("group", { name: "已載入項目" })).getAllByRole("listitem");
    expect(loaded.map((item) => item.textContent)).toEqual([
      "Task 契約：部署 v3（120 tokens）",
      "決策：一律用 pnpm v1（40 tokens）",
      "未讀訊息摘要：未讀訊息 2 則（30 tokens）",
    ]);
    const omitted = within(within(receipt()).getByRole("group", { name: "省略項目" })).getAllByRole("listitem");
    expect(omitted.map((item) => item.textContent)).toEqual(["教訓：部署前先備份 v2（4100 tokens），超出預算"]);
    expect(within(receipt()).getByText("實際 Prompt")).toBeInTheDocument();
    expect(receipt().querySelector("pre")?.textContent).toBe("# Task 契約\n部署到正式環境");
  });

  it("沒有省略項目時不顯示省略清單；未知的省略原因原樣顯示", async () => {
    const base = runContextView();
    const allLoaded = runContextView({ receipt: { ...base.receipt, items: base.receipt.items.slice(0, 1) } });
    const client = fakeClient({ tasks: [runningTask()], runs: [run("run_1", "task_1", "running")], context: allLoaded });
    const user = await renderBoard(client);

    await user.click(within(receipt()).getByRole("button", { name: "Context receipt" }));

    expect(await within(receipt()).findByRole("group", { name: "已載入項目" })).toBeInTheDocument();
    expect(within(receipt()).queryByRole("group", { name: "省略項目" })).not.toBeInTheDocument();

    const unknownReason = runContextView({
      receipt: {
        ...base.receipt,
        items: [{ kind: "evidence", ref_id: "art_1", version: null, title: "report.md", tokens: 900, status: "omitted", omitted_reason: "stale" }],
      },
    });
    vi.mocked(client.runContext).mockResolvedValueOnce(unknownReason);
    await user.click(within(receipt()).getByRole("button", { name: "Context receipt" }));
    await user.click(within(receipt()).getByRole("button", { name: "Context receipt" }));

    const omitted = await within(receipt()).findByRole("group", { name: "省略項目" });
    expect(within(omitted).getByRole("listitem")).toHaveTextContent("證據：report.md（900 tokens），stale");
  });

  it("Context Builder 之前建立的 Run（404 context_receipt_not_found）顯示說明", async () => {
    const client = fakeClient({ tasks: [runningTask()], runs: [run("run_1", "task_1", "running")] });
    vi.mocked(client.runContext).mockRejectedValueOnce(
      Object.assign(new Error("找不到 context receipt"), { code: "context_receipt_not_found" }),
    );
    const user = await renderBoard(client);

    await user.click(within(receipt()).getByRole("button", { name: "Context receipt" }));

    expect(await within(receipt()).findByText("這個 Run 建立得較早，沒有 context receipt")).toBeInTheDocument();
    expect(within(receipt()).queryByRole("alert")).not.toBeInTheDocument();
  });

  it("其他錯誤顯示 Runtime 的說明", async () => {
    const client = fakeClient({ tasks: [runningTask()], runs: [run("run_1", "task_1", "running")] });
    vi.mocked(client.runContext).mockRejectedValueOnce(Object.assign(new Error("Run 不存在"), { code: "run_not_found" }));
    const user = await renderBoard(client);

    await user.click(within(receipt()).getByRole("button", { name: "Context receipt" }));

    expect(await within(receipt()).findByRole("alert")).toHaveTextContent("無法取得 context receipt：Run 不存在");
  });
});
