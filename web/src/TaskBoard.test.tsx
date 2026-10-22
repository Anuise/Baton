import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { TaskBoard } from "./TaskBoard";
import {
  CONTRACT_FIELDS,
  type Capability,
  type ContractInput,
  type Goal,
  type MissingItem,
  type TaskBoard as TaskBoardData,
  type TaskClient,
  type TaskContract,
  type TaskView,
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
  return { ...empty, task_id: taskId, version: 1, required_capabilities: [], note: null, created_at: T0, ...overrides } as TaskContract;
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

/** 以記憶體模擬 Runtime 的 Task 行為，讓測試只看使用者可見的流程。 */
function fakeClient(initial: { goals?: Goal[]; tasks?: TaskView[]; history?: TaskContract[] } = {}) {
  let goals = initial.goals ?? [goal("goal_1", "整理文件")];
  let tasks = initial.tasks ?? [];
  const board = (): TaskBoardData => ({ workspace_id: workspace.id, goals, tasks, capabilities: CAPABILITIES });
  const replace = (id: string, patch: Partial<TaskView>) => {
    tasks = tasks.map((t) => (t.id === id ? { ...t, ...patch } : t));
    return tasks.find((t) => t.id === id)!;
  };
  const client: TaskClient = {
    getTaskBoard: vi.fn(async () => board()),
    createGoal: vi.fn(async (_ws: string, title: string) => {
      const created = goal(`goal_${goals.length + 1}`, title);
      goals = [...goals, created];
      return created;
    }),
    createTask: vi.fn(async (_ws: string, { goalId, title, contract: input }) => {
      const id = `task_${tasks.length + 1}`;
      const created = task(id, title, { goal_id: goalId, contract: contract(id, input as Partial<TaskContract>) });
      tasks = [...tasks, created];
      return created;
    }),
    getTask: vi.fn(async (_ws: string, id: string) => ({
      task: tasks.find((t) => t.id === id)!,
      contract_history: initial.history ?? [tasks.find((t) => t.id === id)!.contract],
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
  };
  return client;
}

async function renderBoard(client: TaskClient) {
  const user = userEvent.setup();
  render(<TaskBoard client={client} workspace={workspace} timeZone="UTC" />);
  await screen.findByRole("heading", { name: "工作面板：筆記" });
  return user;
}

const card = (title: string) => screen.getByRole("article", { name: title });
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
    expect(within(card("舊的")).queryByRole("button", { name: "取消" })).not.toBeInTheDocument();

    await user.click(within(card("寫文件")).getByRole("button", { name: "取消" }));

    expect(await within(column("已取消（1）")).findByRole("article", { name: "寫文件" })).toBeInTheDocument();
    expect(client.cancelTask).toHaveBeenCalledWith("ws_1", "task_1");
  });

  it("新增相依：從同一 Workspace 的其他 Task 挑選，卡片顯示等待相依", async () => {
    const client = fakeClient({ tasks: [task("task_1", "寫文件"), task("task_2", "整理資料")] });
    const user = await renderBoard(client);

    await user.click(within(card("整理資料")).getByRole("button", { name: "新增相依" }));
    const select = within(card("整理資料")).getByLabelText("相依的 Task");
    expect(within(select).queryByRole("option", { name: "整理資料" })).not.toBeInTheDocument();
    await user.selectOptions(select, "寫文件");
    await user.click(within(card("整理資料")).getByRole("button", { name: "確定新增" }));

    expect(client.addTaskDependency).toHaveBeenCalledWith("ws_1", "task_2", "task_1");
    expect(await within(card("整理資料")).findByText("等待相依：寫文件")).toBeInTheDocument();
  });

  it("新增相依失敗（例如循環）時顯示 Runtime 的說明", async () => {
    const client = fakeClient({ tasks: [task("task_1", "寫文件"), task("task_2", "整理資料")] });
    vi.mocked(client.addTaskDependency).mockRejectedValueOnce(new Error("相依會形成循環：寫文件 → 整理資料 → 寫文件"));
    const user = await renderBoard(client);

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

describe("建立 Task 與契約編輯器", () => {
  async function openCreate(client: TaskClient) {
    const user = await renderBoard(client);
    await user.click(screen.getByRole("button", { name: "建立 Task" }));
    return { user, form: screen.getByRole("region", { name: "建立 Task" }) };
  }

  it("選擇既有 Goal 建立 Task，契約欄位預設為未提供", async () => {
    const client = fakeClient();
    const { user, form } = await openCreate(client);

    await user.type(within(form).getByLabelText("Task 標題"), "寫文件");
    await user.click(within(form).getByRole("button", { name: "建立" }));

    await within(column("待處理（1）")).findByRole("article", { name: "寫文件" });
    expect(client.createGoal).not.toHaveBeenCalled();
    expect(client.createTask).toHaveBeenCalledWith("ws_1", { goalId: "goal_1", title: "寫文件", contract: { required_capabilities: [] } });
    expect(screen.queryByRole("region", { name: "建立 Task" })).not.toBeInTheDocument();
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
      within(form).getByLabelText("目標：引用（每行 kind:target@version）"),
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
    await user.type(within(form).getByLabelText("目標：引用（每行 kind:target@version）"), "goal-1");
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

    await user.click(within(card("寫文件")).getByRole("button", { name: "編輯契約" }));
    const form = screen.getByRole("region", { name: "編輯契約：寫文件" });

    expect(within(form).getByLabelText("目標：模式")).toHaveValue("filled");
    expect(within(form).getByLabelText("目標：內容")).toHaveValue("整理文件");
    expect(within(form).getByLabelText("目標：引用（每行 kind:target@version）")).toHaveValue("goal:goal_1@1");
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
    expect(screen.queryByRole("region", { name: "編輯契約：寫文件" })).not.toBeInTheDocument();
  });

  it("沒有變更說明時不送 note；Runtime 拒絕時顯示說明", async () => {
    const client = fakeClient({ tasks: [existing()] });
    vi.mocked(client.updateTaskContract).mockRejectedValueOnce(new Error("Task 已結束，不能修改契約"));
    const user = await renderBoard(client);

    await user.click(within(card("寫文件")).getByRole("button", { name: "編輯契約" }));
    const form = screen.getByRole("region", { name: "編輯契約：寫文件" });
    await user.click(within(form).getByRole("button", { name: "儲存契約" }));

    expect(await within(form).findByRole("alert")).toHaveTextContent("Task 已結束，不能修改契約");
    expect(vi.mocked(client.updateTaskContract).mock.calls[0][3]).toBeUndefined();
  });

  it("已完成或已取消的 Task 不能編輯契約", async () => {
    await renderBoard(fakeClient({ tasks: [task("task_1", "舊的", { status: "completed" })] }));

    expect(within(card("舊的")).queryByRole("button", { name: "編輯契約" })).not.toBeInTheDocument();
    expect(within(card("舊的")).queryByRole("button", { name: "新增相依" })).not.toBeInTheDocument();
  });
});
