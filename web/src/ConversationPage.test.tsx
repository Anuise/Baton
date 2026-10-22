import { act, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import { CONVERSATION_REFRESH_INTERVAL_MS, ConversationPage } from "./ConversationPage";
import { CHECKED_AT, controllerRun, conversationTurn, conversationView } from "./fixtures";
import type { ConversationView, Workspace } from "./runtimeClient";

const workspace: Workspace = {
  id: "ws_1",
  name: "筆記",
  path: "C:\\work\\notes",
  status: "active",
  version: 1,
  created_at: CHECKED_AT,
  archived_at: null,
  model_overrides: { controller: null, worker: null, reviewer: null },
};

/** 一來一往的對話，加上一個工作事件。 */
const history = [
  conversationTurn(),
  conversationTurn({
    id: "turn_2",
    seq: 2,
    kind: "controller_reply",
    content: "已建立兩個 Task",
    controller_run_id: "crun_1",
    created_at: CHECKED_AT + 120_000,
  }),
  conversationTurn({
    id: "turn_3",
    seq: 3,
    kind: "work_event",
    content: "Worker 完成「整理文件」",
    source: "task_1",
    status: "pending",
    controller_run_id: null,
    processed_at: null,
    created_at: CHECKED_AT + 180_000,
  }),
];

function fakeClient(view: ConversationView | Error = conversationView()) {
  return {
    getConversation: vi.fn(async () => {
      if (view instanceof Error) throw view;
      return view;
    }),
    sendConversationMessage: vi.fn(async (_workspaceId: string, text: string) => ({
      turn: conversationTurn({ id: "turn_new", seq: 99, content: text, status: "pending", controller_run_id: null, processed_at: null }),
      busy: false,
    })),
  };
}

const renderPage = (client: ReturnType<typeof fakeClient>) =>
  render(<ConversationPage client={client} workspace={workspace} timeZone="Asia/Taipei" />);

afterEach(() => {
  vi.useRealTimers();
});

describe("ConversationPage", () => {
  it("依 seq 顯示使用者訊息、Controller 回覆與工作事件，時間以指定時區顯示", async () => {
    renderPage(fakeClient(conversationView({ conversation_id: "conv_1", turns: history })));

    const items = await screen.findAllByRole("listitem");
    expect(items.map((item) => item.getAttribute("aria-label"))).toEqual(["你 #1", "Controller #2", "工作事件 #3"]);
    expect(items[0]).toHaveTextContent("先整理文件目錄");
    expect(items[0]).toHaveTextContent("2023/11/15 06:13");
    expect(items[1]).toHaveTextContent("已建立兩個 Task");
    expect(items[2]).toHaveTextContent("Worker 完成「整理文件」");
    expect(items[2]).toHaveTextContent("來源：task_1");
    expect(items[2]).toHaveClass("conversation-work_event");
  });

  it("使用者訊息標示待處理、處理中、已處理，重新交付時加註次數", async () => {
    renderPage(
      fakeClient(
        conversationView({
          conversation_id: "conv_1",
          turns: [
            conversationTurn({ status: "processed" }),
            conversationTurn({ id: "turn_2", seq: 2, status: "processing", attempts: 2 }),
            conversationTurn({ id: "turn_3", seq: 3, status: "pending" }),
          ],
        }),
      ),
    );

    expect(await screen.findByRole("listitem", { name: "你 #1" })).toHaveTextContent("已處理");
    const processing = screen.getByRole("listitem", { name: "你 #2" });
    expect(processing).toHaveTextContent("處理中");
    expect(processing).toHaveTextContent("重新交付（第 2 次）");
    expect(screen.getByRole("listitem", { name: "你 #3" })).toHaveTextContent("已收到／待處理");
    expect(screen.getByRole("listitem", { name: "你 #1" })).not.toHaveTextContent("重新交付");
  });

  it("Controller 忙碌時在頂部說明新訊息會在本輪結束後依序處理", async () => {
    renderPage(fakeClient(conversationView({ conversation_id: "conv_1", busy: true, active_run: controllerRun(), turns: history })));

    expect(await screen.findByRole("status")).toHaveTextContent("Controller 正在處理上一輪，新訊息會在本輪結束後依序處理");
  });

  it("不忙碌而有預計喚醒時間時顯示預計處理時間", async () => {
    renderPage(fakeClient(conversationView({ conversation_id: "conv_1", next_wake_at: CHECKED_AT + 300_000, turns: history })));

    expect(await screen.findByRole("status")).toHaveTextContent("預計於 2023/11/15 06:18");
  });

  it("沒有可用帳號而等待時顯示等待原因與開始等待時間", async () => {
    renderPage(
      fakeClient(
        conversationView({
          conversation_id: "conv_1",
          waiting: {
            code: "quota_draining",
            message: "prof_1 接近額度上限；prof_2 尚未核對",
            since: CHECKED_AT + 240_000,
          },
          // 已到期而沒有喚起：不另外顯示預計喚醒時間
          next_wake_at: CHECKED_AT + 300_000,
          turns: history,
        }),
      ),
    );

    const waiting = await screen.findByRole("status");
    expect(waiting).toHaveTextContent("Controller 等待可用帳號：prof_1 接近額度上限；prof_2 尚未核對");
    expect(waiting).toHaveTextContent("2023/11/15 06:17");
    expect(screen.queryByText(/預計於/)).toBeNull();
  });

  it("換帳號後第一輪的 Controller 回覆顯示交接來源與原因，成功時另標已確認", async () => {
    const turns = [
      ...history,
      conversationTurn({ id: "turn_4", seq: 4, kind: "controller_reply", content: "接手繼續整理", controller_run_id: "crun_2" }),
      conversationTurn({ id: "turn_5", seq: 5, kind: "controller_reply", content: "還在處理", controller_run_id: "crun_3" }),
    ];
    renderPage(
      fakeClient(
        conversationView({
          conversation_id: "conv_1",
          turns,
          runs: [
            controllerRun({ status: "interrupted", end_reason: "handed_off: quota_draining" }),
            controllerRun({
              id: "crun_2",
              profile_id: "prof_2",
              status: "succeeded",
              handoff: { from_run_id: "crun_1", from_profile_id: "prof_1", reason: "quota_draining" },
            }),
            controllerRun({
              id: "crun_3",
              profile_id: "prof_3",
              status: "failed",
              handoff: { from_run_id: "crun_2", from_profile_id: "prof_2", reason: "weird_code" },
            }),
          ],
        }),
      ),
    );

    const handedOff = await screen.findByRole("listitem", { name: "Controller #2" });
    expect(handedOff).toHaveTextContent("已交接");
    expect(handedOff).not.toHaveTextContent("已從帳號");

    const confirmed = screen.getByRole("listitem", { name: "Controller #4" });
    expect(confirmed).toHaveTextContent("已從帳號 prof_1 交接（帳號接近上限）");
    expect(confirmed).toHaveTextContent("已確認");

    const unknown = screen.getByRole("listitem", { name: "Controller #5" });
    expect(unknown).toHaveTextContent("已從帳號 prof_2 交接（weird_code）");
    expect(unknown).not.toHaveTextContent("已確認");
  });

  it("進行中的一輪是交接後的第一輪時，在忙碌狀態旁顯示交接來源", async () => {
    const activeRun = controllerRun({
      id: "crun_2",
      profile_id: "prof_2",
      handoff: { from_run_id: "crun_1", from_profile_id: "prof_1", reason: "quota_near_limit" },
    });
    renderPage(fakeClient(conversationView({ conversation_id: "conv_1", busy: true, active_run: activeRun, runs: [activeRun], turns: history })));

    const status = await screen.findByRole("status");
    expect(status).toHaveTextContent("已從帳號 prof_1 交接（帳號達停止派工門檻）");
    expect(status).not.toHaveTextContent("已確認");
  });

  it("沒有對話時顯示空狀態，不顯示忙碌或預計時間", async () => {
    renderPage(fakeClient());

    expect(await screen.findByText("還沒有對話，輸入訊息開始與 Controller 對話")).toBeInTheDocument();
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });

  it("說明暫停與取消在工作面板直接生效，不排在對話佇列之後", async () => {
    renderPage(fakeClient());

    expect(screen.getByText(/暫停與取消請在「工作面板」操作，會直接生效，不會排在對話佇列之後/)).toBeInTheDocument();
  });

  it("讀不到對話時顯示原因", async () => {
    renderPage(fakeClient(new Error("找不到 Workspace")));

    expect(await screen.findByRole("alert")).toHaveTextContent("無法取得對話：找不到 Workspace");
  });

  it("送出後清空輸入、新訊息立即顯示為已收到／待處理，並重新讀取", async () => {
    const client = fakeClient(conversationView({ conversation_id: "conv_1", turns: history }));
    const user = userEvent.setup();
    renderPage(client);
    await screen.findByRole("listitem", { name: "你 #1" });
    // 送出後的重新讀取還沒回來，新訊息也要先出現
    client.getConversation.mockImplementationOnce(() => new Promise(() => {}));

    await user.type(screen.getByLabelText("訊息"), "也整理一下 README");
    await user.click(screen.getByRole("button", { name: "送出" }));

    expect(client.sendConversationMessage).toHaveBeenCalledWith("ws_1", "也整理一下 README");
    const sent = await screen.findByRole("listitem", { name: "你 #99" });
    expect(sent).toHaveTextContent("也整理一下 README");
    expect(sent).toHaveTextContent("已收到／待處理");
    expect(screen.getByLabelText("訊息")).toHaveValue("");
    expect(client.getConversation).toHaveBeenCalledTimes(2);
  });

  it("Controller 忙碌時仍可送出", async () => {
    const client = fakeClient(conversationView({ conversation_id: "conv_1", busy: true, active_run: controllerRun(), turns: history }));
    const user = userEvent.setup();
    renderPage(client);
    await screen.findByRole("status");

    await user.type(screen.getByLabelText("訊息"), "補充：先做 docs");
    const button = screen.getByRole("button", { name: "送出" });
    expect(button).toBeEnabled();
    await user.click(button);

    expect(client.sendConversationMessage).toHaveBeenCalledWith("ws_1", "補充：先做 docs");
  });

  it("空白訊息不送出", async () => {
    const client = fakeClient();
    const user = userEvent.setup();
    renderPage(client);

    await user.type(screen.getByLabelText("訊息"), "   ");
    await user.click(screen.getByRole("button", { name: "送出" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("請輸入訊息");
    expect(client.sendConversationMessage).not.toHaveBeenCalled();
  });

  it("伺服器拒絕時顯示它的說明並保留輸入", async () => {
    const client = fakeClient();
    client.sendConversationMessage.mockRejectedValueOnce(new Error("Workspace 已封存，不能再送出訊息"));
    const user = userEvent.setup();
    renderPage(client);

    await user.type(screen.getByLabelText("訊息"), "繼續");
    await user.click(screen.getByRole("button", { name: "送出" }));

    const form = screen.getByRole("button", { name: "送出" }).closest("form")!;
    expect(await within(form).findByRole("alert")).toHaveTextContent("Workspace 已封存，不能再送出訊息");
    expect(screen.getByLabelText("訊息")).toHaveValue("繼續");
  });

  it("每 3 秒重新讀取對話，卸載後停止", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const client = fakeClient(conversationView({ conversation_id: "conv_1", turns: history }));
    const { unmount } = renderPage(client);
    await screen.findByRole("listitem", { name: "你 #1" });
    expect(client.getConversation).toHaveBeenCalledTimes(1);
    expect(CONVERSATION_REFRESH_INTERVAL_MS).toBe(3000);

    const reply = conversationTurn({ id: "turn_4", seq: 4, kind: "controller_reply", content: "整理文件已完成" });
    client.getConversation.mockResolvedValue(conversationView({ conversation_id: "conv_1", turns: [...history, reply] }));
    // 留一秒餘裕，抵銷 shouldAdvanceTime 隨真實時間前進的部分
    act(() => vi.advanceTimersByTime(CONVERSATION_REFRESH_INTERVAL_MS - 1000));
    expect(client.getConversation).toHaveBeenCalledTimes(1);
    act(() => vi.advanceTimersByTime(1000));
    expect(client.getConversation).toHaveBeenCalledTimes(2);
    expect(await screen.findByRole("listitem", { name: "Controller #4" })).toHaveTextContent("整理文件已完成");

    unmount();
    act(() => vi.advanceTimersByTime(CONVERSATION_REFRESH_INTERVAL_MS * 3));
    expect(client.getConversation).toHaveBeenCalledTimes(2);
  });
});
