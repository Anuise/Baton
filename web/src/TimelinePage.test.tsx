import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { metrics, metricsResponse, profile, timelineEvent } from "./fixtures";
import { TimelinePage } from "./TimelinePage";
import type { AccountProfile, MetricsResponse, TaskBoard, TimelineEvent, Workspace } from "./runtimeClient";

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

/** 2023-11-14 22:13:20 UTC（台北是隔天 06:13）。 */
const T0 = 1_700_000_000_000;

const board = {
  workspace_id: "ws_1",
  goals: [],
  tasks: [
    { id: "task_1", title: "整理筆記" },
    { id: "task_2", title: "撰寫報告" },
  ],
} as unknown as TaskBoard;

/** 全部事件；過濾時 Runtime 回傳的是另外指定的事件，UI 不自行篩選。 */
const allEvents: TimelineEvent[] = [
  timelineEvent({ seq: 1, occurred_at: T0 + 60_000 }),
  timelineEvent({
    seq: 2,
    occurred_at: T0,
    category: "handoff",
    aggregate_type: "handoff",
    aggregate_id: "ho_1",
    event_type: "handoff_requested",
    task_ids: ["task_2"],
    run_id: "run_2",
  }),
  timelineEvent({
    seq: 3,
    occurred_at: T0 + 120_000,
    category: "decision",
    aggregate_type: "decision",
    aggregate_id: "dec_1",
    event_type: "decision_requested",
    task_ids: ["task_1", "task_2"],
    run_id: null,
  }),
];

function fakeClient({
  events = allEvents,
  filtered = {},
  metricsValue = metricsResponse(),
  profiles = [profile({ id: "prof_1", name: "Work-1" })],
}: {
  events?: TimelineEvent[] | Error;
  filtered?: Record<string, TimelineEvent[] | Error>;
  metricsValue?: MetricsResponse | Error;
  profiles?: AccountProfile[] | Error;
} = {}) {
  return {
    timeline: vi.fn(async (workspaceId: string, taskId?: string) => {
      const result = taskId === undefined ? events : filtered[taskId];
      if (result instanceof Error) throw result;
      return { workspace_id: workspaceId, task_id: taskId ?? null, events: result };
    }),
    metrics: vi.fn(async () => {
      if (metricsValue instanceof Error) throw metricsValue;
      return metricsValue;
    }),
    getTaskBoard: vi.fn(async () => board),
    listProfiles: vi.fn(async () => {
      if (profiles instanceof Error) throw profiles;
      return { profiles, limit: 10 };
    }),
  };
}

const renderPage = (client: ReturnType<typeof fakeClient>) =>
  render(<TimelinePage client={client} workspace={workspace} timeZone="Asia/Taipei" />);

const metric = (name: string) => screen.getByRole("group", { name });

describe("時間線", () => {
  it("觀測指標原樣顯示 Runtime 回傳的數字", async () => {
    const client = fakeClient();
    renderPage(client);

    expect(await screen.findByText("由 42 個時間線事件計算")).toBeInTheDocument();
    expect(client.metrics).toHaveBeenCalledWith("ws_1");
    expect(metric("工作完成率")).toHaveTextContent("33.3%（1／3）");
    expect(metric("工作完成率")).toHaveTextContent("已取消 1");
    expect(metric("交接成功率")).toHaveTextContent("50%（1／2）");
    expect(metric("交接成功率")).toHaveTextContent("已放棄 1・已確認接手 1・進行中 0");
    expect(metric("恢復耗時")).toHaveTextContent("平均 40 秒・最長 40 秒");
    expect(metric("恢復耗時")).toHaveTextContent("已恢復 1 次・尚未恢復 0");
    expect(metric("重複操作事件")).toHaveTextContent("被擋下 3 次");
    expect(metric("重複操作事件")).toHaveTextContent("已成功 2・結果未知 1・他處進行中 0");
    expect(metric("重複操作事件")).toHaveTextContent("變成結果未知 1 次");
    expect(metric("裁定等待時間")).toHaveTextContent("平均 40 秒・最長 2 分 30 秒");
    expect(metric("裁定等待時間")).toHaveTextContent("已回答 2・等待中 1");
    expect(metric("教訓重犯率")).toHaveTextContent("25%（1／4）");
    expect(metric("教訓重犯率")).toHaveTextContent("重犯 2 次・Guard 擋下 1・使用者回報 1");
    expect(metric("每階段用量")).toHaveTextContent("總計 2,000 tokens（3 個 Run）");
    expect(metric("每階段用量")).toHaveTextContent("implement：2,000 tokens（3 個 Run）");
    expect(metric("Context 使用量")).toHaveTextContent("平均 45,000 tokens・最長 60,000 tokens");
    expect(metric("Context 使用量")).toHaveTextContent("已回報 2 個 Run・未回報 1 個 Run");
    expect(screen.getByLabelText("服務端額度 Work-1")).toHaveTextContent("Work-1：五小時 42%・七日 10%");
  });

  it("沒有資料的比率、耗時與 Context 顯示「無資料」而不是 0；沒有 Stage 顯示「未分階段」", async () => {
    const empty = { numerator: 0, denominator: 0, rate: null };
    const noDuration = { count: 0, pending: 0, average_ms: null, max_ms: null };
    const client = fakeClient({
      metricsValue: metricsResponse({
        metrics: metrics({
          task_completion: { completion: empty, cancelled: 0 },
          handoff: { success: empty, abandoned: 0, confirmed: 0, in_flight: 0 },
          recovery: noDuration,
          decision_wait: { ...noDuration, pending: 1 },
          lesson_recurrence: { rate: empty, recurrences: 0, prevented: 0, reported: 0 },
          usage: { total_tokens: 500, runs: 1, by_stage: [{ stage: null, tokens: 500, runs: 1 }] },
          context: { runs_known: 0, runs_unknown: 1, average_tokens: null, max_tokens: null },
        }),
      }),
    });
    renderPage(client);

    await screen.findByText("由 42 個時間線事件計算");
    for (const name of ["工作完成率", "交接成功率", "教訓重犯率"]) {
      expect(metric(name)).toHaveTextContent("無資料");
      expect(metric(name)).not.toHaveTextContent("0%");
    }
    expect(metric("恢復耗時")).toHaveTextContent("平均 無資料・最長 無資料");
    expect(metric("裁定等待時間")).toHaveTextContent("平均 無資料・最長 無資料");
    expect(metric("裁定等待時間")).toHaveTextContent("等待中 1");
    expect(metric("Context 使用量")).toHaveTextContent("平均 無資料・最長 無資料");
    expect(metric("每階段用量")).toHaveTextContent("未分階段：500 tokens（1 個 Run）");
  });

  it("看不到的服務端額度顯示「未知」，看得到的顯示百分比", async () => {
    const client = fakeClient({
      metricsValue: metricsResponse({
        server_quota: [
          { profile_id: "prof_1", name: "Work-1", five_hour_used: 0.42, seven_day_used: null },
          { profile_id: "prof_2", name: "Work-2", five_hour_used: null, seven_day_used: null },
        ],
      }),
    });
    renderPage(client);

    expect(await screen.findByLabelText("服務端額度 Work-1")).toHaveTextContent("Work-1：五小時 42%・七日 未知");
    expect(screen.getByLabelText("服務端額度 Work-2")).toHaveTextContent("Work-2：五小時 未知・七日 未知");
  });

  it("依 Runtime 回傳的順序列出事件的時間、類別、事件類型、Task 與 Run，不依時間重排", async () => {
    const client = fakeClient();
    renderPage(client);

    const list = await screen.findByRole("list", { name: "事件" });
    const items = within(list).getAllByRole("listitem");
    expect(client.timeline).toHaveBeenCalledWith("ws_1", undefined);
    expect(items).toHaveLength(3);
    expect(items[0]).toHaveTextContent("2023/11/15 06:14");
    expect(items[0]).toHaveTextContent("派工");
    expect(items[0]).toHaveTextContent("run_created");
    expect(items[0]).toHaveTextContent("Task：整理筆記");
    expect(items[0]).toHaveTextContent("Run：run_1");
    expect(items[1]).toHaveTextContent("2023/11/15 06:13");
    expect(items[1]).toHaveTextContent("交接");
    expect(items[1]).toHaveTextContent("Task：撰寫報告");
    expect(items[2]).toHaveTextContent("裁定");
    expect(items[2]).toHaveTextContent("Task：整理筆記、撰寫報告");
    expect(items[2]).not.toHaveTextContent("Run：");
  });

  it("Controller CLI 的開啟與結束以「Controller」類別列出", async () => {
    const opened = timelineEvent({
      seq: 9,
      category: "controller",
      aggregate_type: "controller_cli",
      aggregate_id: "ctl_1",
      event_type: "controller_cli_opened",
      task_ids: [],
      run_id: null,
    });
    renderPage(fakeClient({ events: [opened] }));

    const list = await screen.findByRole("list", { name: "事件" });
    const [item] = within(list).getAllByRole("listitem");
    expect(item).toHaveTextContent("Controller");
    expect(item).toHaveTextContent("controller_cli_opened");
  });

  it("Controller CLI 事件顯示帳號名稱、Controller CLI id 與結束代碼", async () => {
    const opened = timelineEvent({
      seq: 9,
      category: "controller",
      aggregate_type: "controller_cli",
      aggregate_id: "ctl_1",
      event_type: "controller_cli_opened",
      task_ids: [],
      run_id: null,
      payload: { workspace_id: "ws_1", profile_id: "prof_1", profile_name: "Work-1", continue_session: false },
    });
    const ended = timelineEvent({
      seq: 10,
      category: "controller",
      aggregate_type: "controller_cli",
      aggregate_id: "ctl_1",
      event_type: "controller_cli_ended",
      task_ids: [],
      run_id: null,
      payload: { profile_id: "prof_1", end_code: "host_exited", end_reason: "Claude Code 已結束", exit_code: 0 },
    });
    renderPage(fakeClient({ events: [opened, ended] }));

    const list = await screen.findByRole("list", { name: "事件" });
    // 帳號清單與時間線各自載入，等到帳號名稱出現
    await waitFor(() => expect(within(list).getAllByRole("listitem")[0]).toHaveTextContent("帳號：Work-1"));
    const [openedItem, endedItem] = within(list).getAllByRole("listitem");
    expect(openedItem).toHaveTextContent("Controller CLI：ctl_1");
    expect(openedItem).not.toHaveTextContent("結束代碼");
    expect(endedItem).toHaveTextContent("帳號：Work-1");
    expect(endedItem).toHaveTextContent("Controller CLI：ctl_1");
    expect(endedItem).toHaveTextContent("結束代碼：host_exited");
  });

  it("找不到對應帳號或讀不到帳號清單時，Controller CLI 事件改顯示 profile_id", async () => {
    const opened = timelineEvent({
      seq: 9,
      category: "controller",
      aggregate_type: "controller_cli",
      aggregate_id: "ctl_1",
      event_type: "controller_cli_opened",
      task_ids: [],
      run_id: null,
      payload: { profile_id: "prof_gone" },
    });
    const client = fakeClient({ events: [opened], profiles: new Error("離線") });
    renderPage(client);

    const list = await screen.findByRole("list", { name: "事件" });
    await waitFor(() => expect(client.listProfiles).toHaveBeenCalled());
    expect(within(list).getByRole("listitem")).toHaveTextContent("帳號：prof_gone");
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("選擇 Task 後以 task_id 重新查詢並只顯示 Runtime 回傳的事件；清除過濾後回到全部", async () => {
    const filteredEvent = allEvents[1];
    const client = fakeClient({ filtered: { task_2: [filteredEvent] } });
    const user = userEvent.setup();
    renderPage(client);
    await screen.findByRole("list", { name: "事件" });

    await user.selectOptions(screen.getByLabelText("依 Task 過濾"), "撰寫報告");

    await waitFor(() => expect(within(screen.getByRole("list", { name: "事件" })).getAllByRole("listitem")).toHaveLength(1));
    expect(client.timeline).toHaveBeenLastCalledWith("ws_1", "task_2");
    expect(screen.getByRole("list", { name: "事件" })).toHaveTextContent("handoff_requested");
    // 過濾後仍可改選其他 Task
    expect(screen.getByRole("option", { name: "整理筆記" })).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "清除過濾" }));

    await waitFor(() => expect(within(screen.getByRole("list", { name: "事件" })).getAllByRole("listitem")).toHaveLength(3));
    expect(client.timeline).toHaveBeenLastCalledWith("ws_1", undefined);
    expect(screen.getByLabelText("依 Task 過濾")).toHaveValue("");
    expect(screen.queryByRole("button", { name: "清除過濾" })).not.toBeInTheDocument();
  });

  it("讀不到 Task 標題時過濾選項與事件改顯示 Task id", async () => {
    const client = fakeClient();
    client.getTaskBoard.mockRejectedValueOnce(new Error("離線"));
    renderPage(client);

    await screen.findByRole("list", { name: "事件" });

    expect(screen.getByRole("option", { name: "task_1" })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "task_2" })).toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("沒有事件時顯示空狀態", async () => {
    renderPage(fakeClient({ events: [] }));

    expect(await screen.findByText("這個 Workspace 還沒有事件")).toBeInTheDocument();
    expect(screen.queryByRole("list", { name: "事件" })).not.toBeInTheDocument();
  });

  it("過濾的 Task 沒有事件時另外說明", async () => {
    const client = fakeClient({ filtered: { task_1: [] } });
    const user = userEvent.setup();
    renderPage(client);
    await screen.findByRole("list", { name: "事件" });

    await user.selectOptions(screen.getByLabelText("依 Task 過濾"), "整理筆記");

    expect(await screen.findByText("這個 Task 沒有事件")).toBeInTheDocument();
  });

  describe("分頁與關鍵字搜尋", () => {
    /** 25 筆事件：第 23 筆是唯一的交接事件、唯一關聯到「撰寫報告」，落在第 3 頁。 */
    const manyEvents: TimelineEvent[] = Array.from({ length: 25 }, (_, i) =>
      i + 1 === 23
        ? timelineEvent({
            seq: 23,
            category: "handoff",
            aggregate_type: "handoff",
            aggregate_id: "ho_23",
            event_type: "handoff_requested",
            task_ids: ["task_2"],
            run_id: "run_23",
          })
        : timelineEvent({ seq: i + 1, aggregate_id: `run_${i + 1}`, run_id: `run_${i + 1}` }),
    );
    const items = () => within(screen.getByRole("list", { name: "事件" })).getAllByRole("listitem");
    const runs = () => items().map((item) => item.textContent?.match(/Run：(run_\d+)/)?.[1]);

    it("超過 10 筆時一頁顯示 10 筆，以上一頁／下一頁切換，第一頁與最後一頁的按鈕停用", async () => {
      const user = userEvent.setup();
      renderPage(fakeClient({ events: manyEvents }));
      await screen.findByRole("list", { name: "事件" });

      expect(items()).toHaveLength(10);
      expect(runs()[0]).toBe("run_1");
      expect(screen.getByText("第 1／3 頁")).toBeInTheDocument();
      expect(screen.getByRole("button", { name: "上一頁" })).toBeDisabled();
      expect(screen.getByRole("button", { name: "下一頁" })).toBeEnabled();

      await user.click(screen.getByRole("button", { name: "下一頁" }));
      expect(items()).toHaveLength(10);
      expect(runs()[0]).toBe("run_11");
      expect(screen.getByText("第 2／3 頁")).toBeInTheDocument();
      expect(screen.getByRole("button", { name: "上一頁" })).toBeEnabled();

      await user.click(screen.getByRole("button", { name: "下一頁" }));
      expect(items()).toHaveLength(5);
      expect(runs()).toEqual(["run_21", "run_22", "run_23", "run_24", "run_25"]);
      expect(screen.getByText("第 3／3 頁")).toBeInTheDocument();
      expect(screen.getByRole("button", { name: "下一頁" })).toBeDisabled();

      await user.click(screen.getByRole("button", { name: "上一頁" }));
      expect(screen.getByText("第 2／3 頁")).toBeInTheDocument();
    });

    it("關鍵字搜尋整個時間線且不分大小寫，第 3 頁的事件也搜得到", async () => {
      const user = userEvent.setup();
      renderPage(fakeClient({ events: manyEvents }));
      await screen.findByRole("list", { name: "事件" });

      await user.type(screen.getByRole("searchbox", { name: "搜尋事件" }), "HANDOFF_Requested");

      expect(items()).toHaveLength(1);
      expect(items()[0]).toHaveTextContent("Run：run_23");
      expect(screen.getByText("第 1／1 頁")).toBeInTheDocument();
      expect(screen.getByRole("button", { name: "上一頁" })).toBeDisabled();
      expect(screen.getByRole("button", { name: "下一頁" })).toBeDisabled();
    });

    it("關鍵字比對 Task 標題與類別名稱", async () => {
      const user = userEvent.setup();
      renderPage(fakeClient({ events: manyEvents }));
      const search = await screen.findByRole("searchbox", { name: "搜尋事件" });
      await screen.findByRole("list", { name: "事件" });

      await user.type(search, "撰寫報告");
      await waitFor(() => expect(items()).toHaveLength(1));
      expect(items()[0]).toHaveTextContent("Run：run_23");

      await user.clear(search);
      await user.type(search, "交接");
      expect(items()).toHaveLength(1);
      expect(items()[0]).toHaveTextContent("Run：run_23");
    });

    it("搜尋結果也以 10 筆一頁分頁，變更關鍵字時回到第 1 頁", async () => {
      const user = userEvent.setup();
      renderPage(fakeClient({ events: manyEvents }));
      await screen.findByRole("list", { name: "事件" });
      await user.click(screen.getByRole("button", { name: "下一頁" }));
      await user.click(screen.getByRole("button", { name: "下一頁" }));
      expect(screen.getByText("第 3／3 頁")).toBeInTheDocument();

      // 24 筆 run_created 符合，分成 3 頁並回到第 1 頁
      await user.type(screen.getByRole("searchbox", { name: "搜尋事件" }), "RUN_CREATED");

      expect(screen.getByText("第 1／3 頁")).toBeInTheDocument();
      expect(items()).toHaveLength(10);
      expect(runs()[0]).toBe("run_1");
      await user.click(screen.getByRole("button", { name: "下一頁" }));
      await user.click(screen.getByRole("button", { name: "下一頁" }));
      expect(runs()).toEqual(["run_21", "run_22", "run_24", "run_25"]);
    });

    it("沒有符合關鍵字的事件時顯示空狀態", async () => {
      const user = userEvent.setup();
      renderPage(fakeClient({ events: manyEvents }));
      await screen.findByRole("list", { name: "事件" });

      await user.type(screen.getByRole("searchbox", { name: "搜尋事件" }), "不存在的字");

      expect(screen.getByText("沒有符合「不存在的字」的事件")).toBeInTheDocument();
      expect(screen.queryByRole("list", { name: "事件" })).not.toBeInTheDocument();
      expect(screen.queryByRole("button", { name: "下一頁" })).not.toBeInTheDocument();
    });

    it("換 Task 過濾時回到第 1 頁", async () => {
      const user = userEvent.setup();
      const client = fakeClient({ events: manyEvents, filtered: { task_1: manyEvents.filter((e) => e.seq !== 23) } });
      renderPage(client);
      await screen.findByRole("list", { name: "事件" });
      await user.click(screen.getByRole("button", { name: "下一頁" }));

      await user.selectOptions(screen.getByLabelText("依 Task 過濾"), "整理筆記");

      await waitFor(() => expect(client.timeline).toHaveBeenLastCalledWith("ws_1", "task_1"));
      expect(await screen.findByText("第 1／3 頁")).toBeInTheDocument();
      expect(runs()[0]).toBe("run_1");
    });
  });

  it("讀不到時間線或觀測指標時顯示 Runtime 的說明", async () => {
    const client = fakeClient({
      events: new Error("找不到 Workspace：ws_1"),
      metricsValue: new Error("Runtime 拒絕連線"),
    });
    renderPage(client);

    expect(await screen.findByText("無法取得時間線：找不到 Workspace：ws_1")).toHaveAttribute("role", "alert");
    expect(await screen.findByText("無法取得觀測指標：Runtime 拒絕連線")).toHaveAttribute("role", "alert");
  });

  it("過濾的 Task 被 Runtime 拒絕時顯示它的說明", async () => {
    const client = fakeClient({ filtered: { task_1: new Error("找不到 Task：task_1") } });
    const user = userEvent.setup();
    renderPage(client);
    await screen.findByRole("list", { name: "事件" });

    await user.selectOptions(screen.getByLabelText("依 Task 過濾"), "整理筆記");

    expect(await screen.findByRole("alert")).toHaveTextContent("無法取得時間線：找不到 Task：task_1");
  });
});
