import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { DecisionCenter } from "./DecisionCenter";
import { CHECKED_AT, decisionView } from "./fixtures";
import type { DecisionClient, DecisionRequestView, Workspace } from "./runtimeClient";

const workspace: Workspace = {
  id: "ws_1",
  name: "筆記",
  path: "C:\\work\\notes",
  status: "active",
  version: 1,
  created_at: CHECKED_AT,
  archived_at: null,
};

const answeredDecision = (overrides: Partial<DecisionRequestView> = {}) =>
  decisionView({
    id: "dec_done",
    question: "要不要升級相依套件？",
    status: "answered",
    blocking: [],
    decision: { option_id: "keep", option_label: "保留", note: "先不動", decided_by: "desktop", decided_at: CHECKED_AT + 60_000 },
    ...overrides,
  });

/** 以記憶體模擬 Runtime：回答後該筆變成 answered，並恢復指定數量的 Task。 */
function fakeClient(initial: DecisionRequestView[], options: { resumed?: string[] } = {}) {
  let items = [...initial];
  const client: DecisionClient = {
    listDecisions: vi.fn(async () => ({
      pending_count: items.filter((d) => d.status === "pending").length,
      decisions: [...items],
    })),
    getDecision: vi.fn(async (_ws, id) => items.find((d) => d.id === id)!),
    answerDecision: vi.fn(async (_ws, id, optionId, note) => {
      const current = items.find((d) => d.id === id)!;
      const option = current.options.find((o) => o.id === optionId)!;
      const decision = {
        ...current,
        status: "answered" as const,
        decision: { option_id: optionId, option_label: option.label, note: note ?? null, decided_by: "desktop", decided_at: CHECKED_AT + 1 },
      };
      items = items.map((d) => (d.id === id ? decision : d));
      return { decision, resumed_task_ids: options.resumed ?? ["task_1"] };
    }),
    pendingDecisions: vi.fn(async () => ({ pending_count: 0, by_workspace: [] })),
  };
  return client;
}

const renderCenter = (client: DecisionClient, onChange?: () => void) =>
  render(<DecisionCenter client={client} workspace={workspace} timeZone="Asia/Taipei" onChange={onChange} />);

describe("裁定中心", () => {
  it("待處理放前面、已回答收在另一區，列出問題、Task、緊急程度與種類", async () => {
    const client = fakeClient([
      answeredDecision(),
      decisionView({ id: "dec_p", kind: "permission", urgency: "high", question: "要允許執行 npm install 嗎？", task_title: "裝套件" }),
      decisionView(),
    ]);

    renderCenter(client);

    expect(await screen.findByRole("heading", { name: "待處理（2）" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "已回答（1）" })).toBeInTheDocument();
    const buttons = screen.getAllByRole("button", { name: /^查看裁定/ });
    expect(buttons.map((b) => b.getAttribute("aria-label"))).toEqual([
      "查看裁定 要允許執行 npm install 嗎？",
      "查看裁定 要保留舊的目錄結構嗎？",
      "查看裁定 要不要升級相依套件？",
    ]);
    expect(within(buttons[0]).getByText("裝套件")).toBeInTheDocument();
    expect(within(buttons[0]).getByText("權限提示")).toBeInTheDocument();
    expect(within(buttons[0]).getByText("緊急程度：高")).toBeInTheDocument();
    expect(within(buttons[1]).getByText("Agent 請求")).toBeInTheDocument();
  });

  it("沒有任何裁定時顯示空狀態", async () => {
    renderCenter(fakeClient([]));

    expect(await screen.findByText("沒有待處理的裁定")).toBeInTheDocument();
    expect(screen.queryByRole("article")).not.toBeInTheDocument();
  });

  it("讀取失敗時顯示原因", async () => {
    const client = fakeClient([]);
    client.listDecisions = vi.fn(async () => {
      throw new Error("Runtime 回應 503");
    });

    renderCenter(client);

    expect(await screen.findByRole("alert")).toHaveTextContent("無法取得裁定清單：Runtime 回應 503");
  });

  it("詳情顯示問題、原因、選項取捨、建議、證據與阻擋範圍", async () => {
    renderCenter(
      fakeClient([
        decisionView({
          blocking: [
            { task_id: "task_1", task_title: "整理文件", task_status: "waiting_decision" },
            { task_id: "task_2", task_title: "更新索引", task_status: "pending" },
          ],
        }),
      ]),
    );

    const detail = await screen.findByRole("article", { name: "裁定詳情" });
    expect(within(detail).getByRole("heading", { name: "要保留舊的目錄結構嗎？" })).toBeInTheDocument();
    expect(within(detail).getByText("兩種做法都符合契約，需要使用者決定")).toBeInTheDocument();
    expect(within(detail).getByText("取捨：不影響既有連結，但結構較亂")).toBeInTheDocument();
    // 只有建議的選項帶「建議」標示
    const keep = within(detail).getByRole("radio", { name: /保留/ }).closest("label")!;
    const restructure = within(detail).getByRole("radio", { name: /重整/ }).closest("label")!;
    expect(within(keep).getByText("建議")).toBeInTheDocument();
    expect(within(restructure).queryByText("建議")).not.toBeInTheDocument();
    expect(within(detail).getByText("docs/ 底下有 12 個檔案")).toBeInTheDocument();
    expect(within(detail).getByText("整理文件（等待裁定）")).toBeInTheDocument();
    expect(within(detail).getByText("更新索引（待處理）")).toBeInTheDocument();
    expect(within(detail).getByText("只有這些工作受影響，其他工作不受阻擋。")).toBeInTheDocument();
  });

  it("權限提示顯示工具、能力與摘要", async () => {
    renderCenter(
      fakeClient([
        decisionView({
          kind: "permission",
          permission: {
            tool_name: "Bash",
            input: { command: "npm install" },
            capability: "install_project_dependencies",
            capability_label: "安裝專案依賴",
            summary: "在專案目錄執行 npm install",
          },
        }),
      ]),
    );

    const permission = await screen.findByRole("region", { name: "權限提示" });
    expect(within(permission).getByText("Bash")).toBeInTheDocument();
    expect(within(permission).getByText("安裝專案依賴")).toBeInTheDocument();
    expect(within(permission).getByText("在專案目錄執行 npm install")).toBeInTheDocument();
  });

  it("逾期只標示「已逾期，尚未裁定」，沒有任何自動同意的字樣，也不會自行回答", async () => {
    const client = fakeClient([decisionView({ overdue: true, deadline_at: CHECKED_AT - 60_000 })]);

    const { container } = renderCenter(client);

    expect((await screen.findAllByText(/已逾期，尚未裁定/)).length).toBeGreaterThan(0);
    expect(container.textContent).not.toMatch(/自動|同意|核准|視為/);
    expect(client.answerDecision).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "送出裁定" })).toBeDisabled();
  });

  it("沒有選擇選項時不能送出；選了之後帶備註回答，成功顯示「已恢復：N 個工作」並重新載入", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    const client = fakeClient([decisionView(), decisionView({ id: "dec_2", question: "第二個問題" })], { resumed: ["task_1", "task_2"] });
    renderCenter(client, onChange);
    const submit = await screen.findByRole("button", { name: "送出裁定" });
    expect(submit).toBeDisabled();

    await user.click(screen.getByRole("radio", { name: /重整/ }));
    await user.type(screen.getByLabelText("備註（選填）"), "  先重整  ");
    await user.click(submit);

    expect(client.answerDecision).toHaveBeenCalledWith("ws_1", "dec_1", "restructure", "先重整");
    expect(await screen.findByText("已恢復：2 個工作")).toBeInTheDocument();
    expect(onChange).toHaveBeenCalledTimes(1);
    expect(client.listDecisions).toHaveBeenCalledTimes(2);
    // 留在剛回答的這筆，唯讀並顯示裁定結果
    const outcome = screen.getByRole("region", { name: "裁定結果" });
    expect(within(outcome).getByText(/選擇：重整/)).toBeInTheDocument();
    expect(within(outcome).getByText("備註：先重整")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "送出裁定" })).not.toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "待處理（1）" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "已回答（1）" })).toBeInTheDocument();
  });

  it("備註留空時不送 note", async () => {
    const user = userEvent.setup();
    const client = fakeClient([decisionView()]);
    renderCenter(client);

    await user.click(await screen.findByRole("radio", { name: /保留/ }));
    await user.click(screen.getByRole("button", { name: "送出裁定" }));

    await waitFor(() => expect(client.answerDecision).toHaveBeenCalledWith("ws_1", "dec_1", "keep", undefined));
  });

  it("已回答的詳情只讀，顯示裁定結果", async () => {
    const user = userEvent.setup();
    renderCenter(fakeClient([decisionView(), answeredDecision()]));

    await user.click(await screen.findByRole("button", { name: "查看裁定 要不要升級相依套件？" }));

    const outcome = screen.getByRole("region", { name: "裁定結果" });
    expect(within(outcome).getByText(/選擇：保留（desktop，/)).toBeInTheDocument();
    expect(within(outcome).getByText("備註：先不動")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "送出裁定" })).not.toBeInTheDocument();
    expect(screen.queryByRole("radio")).not.toBeInTheDocument();
  });

  it("回答被拒（已被回答過）時顯示說明並重新載入", async () => {
    const user = userEvent.setup();
    const client = fakeClient([decisionView()]);
    client.answerDecision = vi.fn(async () => {
      throw new Error("這個裁定已經回答過了");
    });
    renderCenter(client);

    await user.click(await screen.findByRole("radio", { name: /保留/ }));
    await user.click(screen.getByRole("button", { name: "送出裁定" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("這個裁定已經回答過了");
    expect(client.listDecisions).toHaveBeenCalledTimes(2);
    // 說明仍在，且可以再試
    expect(screen.getByRole("button", { name: "送出裁定" })).toBeEnabled();
  });
});
