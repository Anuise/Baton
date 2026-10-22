import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { DecisionCenter } from "./DecisionCenter";
import { authorityTable, CHECKED_AT, decisionView } from "./fixtures";
import type {
  AnswerRequest,
  AuthorityTable,
  BatchItem,
  DecisionClient,
  DecisionOverview,
  DecisionRequestView,
  UpdateAuthorityRequest,
  Workspace,
} from "./runtimeClient";

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
    allowed_actions: [],
    decision: {
      action: "accept",
      option_id: "keep",
      option_label: "保留",
      note: "先不動",
      decided_by: "desktop",
      decided_at: CHECKED_AT + 60_000,
      batch_id: null,
    },
    ...overrides,
  });

/** Runtime 回報的錯誤：訊息加上穩定代碼。 */
const conflict = (code: string, message: string) => Object.assign(new Error(message), { code });

function overviewOf(items: DecisionRequestView[]): DecisionOverview {
  const pending = items.filter((d) => d.status === "pending");
  return {
    pending: pending.length,
    actionable: pending.filter((d) => !d.deferred).length,
    deferred: pending.filter((d) => d.deferred).length,
    overdue: pending.filter((d) => d.overdue).length,
    high_urgency: pending.filter((d) => d.urgency === "high").length,
    answered: items.filter((d) => d.status === "answered").length,
    superseded: items.filter((d) => d.status === "superseded").length,
    merged: items.filter((d) => d.status === "merged").length,
  };
}

interface FakeOptions {
  resumed?: string[];
  authority?: AuthorityTable;
  /** 送出時目標版本已變更的請求：Runtime 會把它標為 superseded 並拒絕。 */
  supersedeOnSubmit?: string[];
}

/** 以記憶體模擬 Runtime：處理後更新請求狀態與 revision，並檢查 expected_revision。 */
function fakeClient(initial: DecisionRequestView[], options: FakeOptions = {}) {
  let items = [...initial];
  let table = options.authority ?? authorityTable();
  const find = (id: string) => items.find((d) => d.id === id)!;
  const replace = (next: DecisionRequestView) => {
    items = items.map((d) => (d.id === next.id ? next : d));
    return next;
  };
  const apply = (id: string, request: AnswerRequest, batchId: string | null) => {
    const current = find(id);
    if (options.supersedeOnSubmit?.includes(id)) {
      replace({ ...current, status: "superseded", status_reason: "契約已更新到第 2 版", allowed_actions: [] });
      throw conflict("decision_superseded", "目標版本已變更");
    }
    if (current.status !== "pending") throw conflict("decision_already_answered", "這個裁定已經回答過了");
    if (request.expected_revision !== current.revision) throw conflict("decision_revision_mismatch", "內容已變更");
    if (request.action === "defer") {
      return replace({
        ...current,
        revision: current.revision + 1,
        deferred: true,
        deferred_until: CHECKED_AT + (request.defer_minutes ?? 1440) * 60_000,
      });
    }
    const option = current.options.find((o) => o.id === request.option_id);
    return replace({
      ...current,
      status: "answered",
      revision: current.revision + 1,
      allowed_actions: [],
      decision: {
        action: request.action,
        option_id: option?.id ?? null,
        option_label: option?.label ?? null,
        note: request.note ?? null,
        decided_by: "desktop",
        decided_at: CHECKED_AT + 1,
        batch_id: batchId,
      },
    });
  };
  const client: DecisionClient = {
    listDecisions: vi.fn(async () => ({
      pending_count: items.filter((d) => d.status === "pending").length,
      overview: overviewOf(items),
      decisions: [...items],
    })),
    getDecision: vi.fn(async (_ws, id) => find(id)),
    answerDecision: vi.fn(async (_ws, id, request) => {
      const decision = apply(id, request, null);
      return { decision, resumed_task_ids: request.action === "defer" ? [] : (options.resumed ?? ["task_1"]) };
    }),
    batchDecisions: vi.fn(async (batchItems: BatchItem[]) => ({
      batch_id: "batch_1",
      results: batchItems.map((item) => {
        try {
          const decision = apply(item.request_id, item, "batch_1");
          return { workspace_id: item.workspace_id, request_id: item.request_id, ok: true as const, decision, resumed_task_ids: [] };
        } catch (e) {
          const { code, message } = e as Error & { code: string };
          return { workspace_id: item.workspace_id, request_id: item.request_id, ok: false as const, error: code, message };
        }
      }),
    })),
    mergeDecision: vi.fn(async (_ws, id, { into }) => {
      const source = find(id);
      const target = find(into);
      replace({ ...source, status: "merged", merged_into: into, allowed_actions: [] });
      const decision = replace({
        ...target,
        revision: target.revision + 1,
        merged_from: [...target.merged_from, { id, question: source.question, status: "merged" }],
      });
      return { decision };
    }),
    linkDecision: vi.fn(async (_ws, id, otherId) => {
      const source = find(id);
      const other = find(otherId);
      replace({ ...other, links: [...other.links, { id, question: source.question, status: source.status }] });
      return replace({ ...source, links: [...source.links, { id: otherId, question: other.question, status: other.status }] });
    }),
    decisionAuthority: vi.fn(async () => table),
    updateDecisionAuthority: vi.fn(async (_ws: string, request: UpdateAuthorityRequest) => {
      if (request.expected_revision !== table.revision) throw conflict("authority_revision_mismatch", "權限表版本不符");
      const effective = new Map(request.entries.map((e) => [e.matter, e.effective]));
      table = {
        revision: table.revision + 1,
        entries: table.entries.map((entry) => {
          const level = effective.get(entry.matter) ?? entry.builtin;
          return { ...entry, effective: level, narrowed: level !== entry.builtin };
        }),
      };
      return table;
    }),
    pendingDecisions: vi.fn(async () => ({ pending_count: 0, by_workspace: [] })),
  };
  return client;
}

const renderCenter = (client: DecisionClient, onChange?: () => void) =>
  render(<DecisionCenter client={client} workspace={workspace} timeZone="Asia/Taipei" onChange={onChange} />);

const detail = () => screen.getByRole("article", { name: "裁定詳情" });
const form = () => detail().querySelector("form")!;
/** 等決策權限表載入完成（區塊會先顯示「載入中…」）。 */
const authorityPanel = async () => {
  const panel = await screen.findByRole("region", { name: "決策權限表" });
  await waitFor(() => expect(panel).not.toHaveTextContent("載入中…"));
  return panel;
};

describe("裁定中心：總覽與清單", () => {
  it("先顯示總覽數字", async () => {
    renderCenter(
      fakeClient([
        decisionView({ id: "dec_a", urgency: "high", overdue: true }),
        decisionView({ id: "dec_b" }),
        decisionView({ id: "dec_c", deferred: true, deferred_until: CHECKED_AT + 3_600_000 }),
        answeredDecision(),
        decisionView({ id: "dec_s", status: "superseded", allowed_actions: [] }),
        decisionView({ id: "dec_m", status: "merged", merged_into: "dec_b", allowed_actions: [] }),
      ]),
    );

    const overview = await screen.findByRole("region", { name: "裁定總覽" });
    const count = (label: string) => within(overview).getByText(label).nextElementSibling?.textContent;
    expect(count("待處理")).toBe("3");
    expect(count("可處理")).toBe("2");
    expect(count("已延後")).toBe("1");
    expect(count("已逾期")).toBe("1");
    expect(count("高緊急")).toBe("1");
    expect(count("已回答")).toBe("1");
    expect(count("已過期")).toBe("1");
    expect(count("已合併")).toBe("1");
  });

  it("分成可處理、已延後、已結束三組，列出問題、Task、緊急程度與種類；只有可處理的能勾選批次", async () => {
    const client = fakeClient([
      decisionView({ id: "dec_p", kind: "permission", urgency: "high", question: "要允許執行 npm install 嗎？", task_title: "裝套件" }),
      decisionView(),
      decisionView({ id: "dec_def", question: "晚點再說的問題", deferred: true, deferred_until: CHECKED_AT + 3_600_000 }),
      answeredDecision(),
      decisionView({ id: "dec_s", question: "過期的問題", status: "superseded", allowed_actions: [] }),
    ]);

    renderCenter(client);

    expect(await screen.findByRole("heading", { name: "可處理（2）" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "已延後（1）" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "已結束（2）" })).toBeInTheDocument();
    const buttons = screen.getAllByRole("button", { name: /^查看裁定/ });
    expect(buttons.map((b) => b.getAttribute("aria-label"))).toEqual([
      "查看裁定 要允許執行 npm install 嗎？",
      "查看裁定 要保留舊的目錄結構嗎？",
      "查看裁定 晚點再說的問題",
      "查看裁定 要不要升級相依套件？",
      "查看裁定 過期的問題",
    ]);
    expect(within(buttons[0]).getByText("裝套件")).toBeInTheDocument();
    expect(within(buttons[0]).getByText("權限提示")).toBeInTheDocument();
    expect(within(buttons[0]).getByText("緊急程度：高")).toBeInTheDocument();
    expect(within(buttons[1]).getByText("Agent 請求")).toBeInTheDocument();
    expect(within(buttons[2]).getByText("延後至 2023/11/15 07:13")).toBeInTheDocument();
    expect(within(buttons[3]).getByText("已回答")).toBeInTheDocument();
    expect(within(buttons[4]).getByText("已過期")).toBeInTheDocument();
    expect(screen.getAllByRole("checkbox", { name: /^批次選取/ }).map((c) => c.getAttribute("aria-label"))).toEqual([
      "批次選取 要允許執行 npm install 嗎？",
      "批次選取 要保留舊的目錄結構嗎？",
    ]);
  });

  it("沒有任何裁定時顯示空狀態", async () => {
    renderCenter(fakeClient([]));

    expect(await screen.findByText("沒有可處理的裁定")).toBeInTheDocument();
    expect(screen.getByText("還沒有已結束的裁定")).toBeInTheDocument();
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
});

describe("裁定中心：詳情", () => {
  it("詳情顯示問題、原因、選項取捨、建議、證據、阻擋範圍與目標版本", async () => {
    renderCenter(
      fakeClient([
        decisionView({
          blocking: [
            { task_id: "task_1", task_title: "整理文件", task_status: "waiting_decision" },
            { task_id: "task_2", task_title: "更新索引", task_status: "pending" },
          ],
          target: { task_id: "task_1", contract_version: 3, authorization_revision: 2 },
        }),
      ]),
    );

    const article = await screen.findByRole("article", { name: "裁定詳情" });
    expect(within(article).getByRole("heading", { name: "要保留舊的目錄結構嗎？" })).toBeInTheDocument();
    expect(within(article).getByText("兩種做法都符合契約，需要使用者決定")).toBeInTheDocument();
    expect(within(article).getByText("取捨：不影響既有連結，但結構較亂")).toBeInTheDocument();
    // 只有建議的選項帶「建議」標示
    const keep = within(article).getByText("保留").closest("li")!;
    const restructure = within(article).getByText("重整").closest("li")!;
    expect(within(keep).getByText("建議")).toBeInTheDocument();
    expect(within(restructure).queryByText("建議")).not.toBeInTheDocument();
    expect(within(article).getByText("docs/ 底下有 12 個檔案")).toBeInTheDocument();
    expect(within(article).getByText("整理文件（等待裁定）")).toBeInTheDocument();
    expect(within(article).getByText("更新索引（待處理）")).toBeInTheDocument();
    expect(within(article).getByText("只有這些工作受影響，其他工作不受阻擋。")).toBeInTheDocument();
    expect(within(article).getByText("目標版本：契約第 3 版・授權第 2 版")).toBeInTheDocument();
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

  it("權限表攔截的請求顯示事項與摘要", async () => {
    renderCenter(
      fakeClient([
        decisionView({
          kind: "authority",
          question: "Controller 想修改完成條件",
          authority: { matter: "change_acceptance", matter_label: "修改完成條件或驗收標準", summary: "把覆蓋率門檻從 80% 降到 60%" },
        }),
      ]),
    );

    const button = await screen.findByRole("button", { name: "查看裁定 Controller 想修改完成條件" });
    expect(within(button).getByText("權限表攔截")).toBeInTheDocument();
    const authority = screen.getByRole("region", { name: "權限表攔截" });
    expect(within(authority).getByText("修改完成條件或驗收標準")).toBeInTheDocument();
    expect(within(authority).getByText("把覆蓋率門檻從 80% 降到 60%")).toBeInTheDocument();
  });

  it("逾期只標示「已逾期，尚未裁定」，沒有任何自動同意的字樣，也不會自行回答", async () => {
    const client = fakeClient([decisionView({ overdue: true, deadline_at: CHECKED_AT - 60_000 })]);

    const { container } = renderCenter(client);

    expect((await screen.findAllByText(/已逾期，尚未裁定/)).length).toBeGreaterThan(0);
    await authorityPanel();
    expect(container.textContent).not.toMatch(/自動|同意|核准|視為/);
    expect(client.answerDecision).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "送出裁定" })).toBeDisabled();
  });

  it("已過期（superseded）的請求顯示原因，不提供任何處理", async () => {
    const user = userEvent.setup();
    renderCenter(
      fakeClient([
        decisionView(),
        decisionView({ id: "dec_s", question: "過期的問題", status: "superseded", status_reason: "契約已更新到第 2 版", allowed_actions: [] }),
      ]),
    );

    await user.click(await screen.findByRole("button", { name: "查看裁定 過期的問題" }));

    const outcome = within(detail()).getByRole("region", { name: "裁定結果" });
    expect(within(outcome).getByRole("heading", { name: "已過期" })).toBeInTheDocument();
    expect(within(outcome).getByText("原因：契約已更新到第 2 版")).toBeInTheDocument();
    expect(within(detail()).queryByRole("radio")).not.toBeInTheDocument();
    expect(within(detail()).queryByRole("button", { name: "送出裁定" })).not.toBeInTheDocument();
    expect(within(detail()).queryByRole("button", { name: "合併" })).not.toBeInTheDocument();
    expect(within(detail()).queryByRole("button", { name: "連結" })).not.toBeInTheDocument();
  });

  it("已合併的請求顯示合併目標，點了可前往目標", async () => {
    const user = userEvent.setup();
    renderCenter(
      fakeClient([
        decisionView({ id: "dec_target", question: "目標問題" }),
        decisionView({ id: "dec_m", question: "被合併的問題", status: "merged", merged_into: "dec_target", allowed_actions: [] }),
      ]),
    );

    await user.click(await screen.findByRole("button", { name: "查看裁定 被合併的問題" }));
    const outcome = within(detail()).getByRole("region", { name: "裁定結果" });
    expect(outcome).toHaveTextContent("已合併到 目標問題");
    expect(within(detail()).queryByRole("button", { name: "送出裁定" })).not.toBeInTheDocument();

    await user.click(within(outcome).getByRole("button", { name: "目標問題" }));

    expect(within(detail()).getByRole("heading", { name: "目標問題" })).toBeInTheDocument();
  });

  it("延後的請求顯示「延後至 …（延後不是同意，工作仍在等待）」，仍可處理", async () => {
    renderCenter(fakeClient([decisionView({ deferred: true, deferred_until: CHECKED_AT + 3_600_000 })]));

    expect(await screen.findByText("延後至 2023/11/15 07:13（延後不是同意，工作仍在等待）")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "送出裁定" })).toBeInTheDocument();
  });

  it("已回答的詳情只讀，顯示處理方式與結果", async () => {
    const user = userEvent.setup();
    renderCenter(fakeClient([decisionView(), answeredDecision({ decision: { ...answeredDecision().decision!, batch_id: "batch_1" } })]));

    await user.click(await screen.findByRole("button", { name: "查看裁定 要不要升級相依套件？" }));

    const outcome = screen.getByRole("region", { name: "裁定結果" });
    expect(within(outcome).getByText("處理方式：接受方案")).toBeInTheDocument();
    expect(within(outcome).getByText("選擇：保留")).toBeInTheDocument();
    expect(within(outcome).getByText("備註：先不動")).toBeInTheDocument();
    expect(within(outcome).getByText(/^desktop・.*・批次處理（此項各自保存）$/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "送出裁定" })).not.toBeInTheDocument();
    expect(within(detail()).queryByRole("radio")).not.toBeInTheDocument();
  });
});

describe("裁定中心：處理單筆", () => {
  it("只提供 allowed_actions 列出的處理方式", async () => {
    renderCenter(fakeClient([decisionView({ allowed_actions: ["answer", "defer", "reject"] })]));

    await screen.findByRole("article", { name: "裁定詳情" });
    const actions = within(form()).getByRole("group", { name: "處理方式" });
    expect(within(actions).getAllByRole("radio").map((r) => r.closest("label")!.textContent)).toEqual(["回答", "延後", "駁回"]);
  });

  it("接受方案：要選選項才能送出；送出帶 expected_revision，成功顯示「已恢復：N 個工作」並重新載入", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    const client = fakeClient([decisionView({ revision: 4 }), decisionView({ id: "dec_2", question: "第二個問題" })], {
      resumed: ["task_1", "task_2"],
    });
    renderCenter(client, onChange);
    const submit = await screen.findByRole("button", { name: "送出裁定" });
    expect(submit).toBeDisabled();

    await user.click(within(form()).getByRole("radio", { name: "接受方案" }));
    expect(submit).toBeDisabled();
    await user.click(within(form()).getByRole("radio", { name: "重整" }));
    await user.type(within(form()).getByLabelText("備註（選填）"), "  先重整  ");
    await user.click(submit);

    expect(client.answerDecision).toHaveBeenCalledWith("ws_1", "dec_1", {
      action: "accept",
      expected_revision: 4,
      option_id: "restructure",
      note: "先重整",
    });
    expect(await screen.findByText("已恢復：2 個工作")).toBeInTheDocument();
    expect(onChange).toHaveBeenCalledTimes(1);
    expect(client.listDecisions).toHaveBeenCalledTimes(2);
    // 留在剛處理的這筆，唯讀並顯示裁定結果
    const outcome = screen.getByRole("region", { name: "裁定結果" });
    expect(within(outcome).getByText("處理方式：接受方案")).toBeInTheDocument();
    expect(within(outcome).getByText("選擇：重整")).toBeInTheDocument();
    expect(within(outcome).getByText("備註：先重整")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "送出裁定" })).not.toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "可處理（1）" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "已結束（1）" })).toBeInTheDocument();
  });

  it("建議的選項在選擇時也標示「建議」", async () => {
    const user = userEvent.setup();
    renderCenter(fakeClient([decisionView()]));

    await user.click(await screen.findByRole("radio", { name: "接受方案" }));

    expect(within(form()).getByRole("radio", { name: "保留（建議）" })).toBeInTheDocument();
    expect(within(form()).getByRole("radio", { name: "重整" })).toBeInTheDocument();
  });

  it("修改條件：要選選項並填寫修改後的條件才能送出", async () => {
    const user = userEvent.setup();
    const client = fakeClient([decisionView()]);
    renderCenter(client);
    const submit = await screen.findByRole("button", { name: "送出裁定" });

    await user.click(within(form()).getByRole("radio", { name: "修改條件" }));
    await user.click(within(form()).getByRole("radio", { name: "保留（建議）" }));
    expect(submit).toBeDisabled();
    await user.type(within(form()).getByLabelText("修改後的條件（必填）"), "只保留 docs/api");
    await user.click(submit);

    expect(client.answerDecision).toHaveBeenCalledWith("ws_1", "dec_1", {
      action: "modify",
      expected_revision: 1,
      option_id: "keep",
      note: "只保留 docs/api",
    });
    const outcome = await screen.findByRole("region", { name: "裁定結果" });
    expect(within(outcome).getByText("修改後的條件：只保留 docs/api")).toBeInTheDocument();
  });

  it.each([
    ["回答", "回答內容（必填）", "answer"],
    ["要求調查", "要調查的事（必填）", "investigate"],
  ] as const)("%s：必須填寫文字，不需要選項", async (label, field, action) => {
    const user = userEvent.setup();
    const client = fakeClient([decisionView()]);
    renderCenter(client);
    const submit = await screen.findByRole("button", { name: "送出裁定" });

    await user.click(within(form()).getByRole("radio", { name: label }));
    expect(within(form()).queryByRole("group", { name: "選項" })).not.toBeInTheDocument();
    await user.type(within(form()).getByLabelText(field), "   ");
    expect(submit).toBeDisabled();
    await user.type(within(form()).getByLabelText(field), "看一下 README 的引用");
    await user.click(submit);

    expect(client.answerDecision).toHaveBeenCalledWith("ws_1", "dec_1", {
      action,
      expected_revision: 1,
      note: "看一下 README 的引用",
    });
  });

  it("駁回：理由選填，留空時不送 note", async () => {
    const user = userEvent.setup();
    const client = fakeClient([decisionView()]);
    renderCenter(client);

    await user.click(await screen.findByRole("radio", { name: "駁回" }));
    expect(within(form()).getByLabelText("駁回理由（選填）")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "送出裁定" }));

    await waitFor(() => expect(client.answerDecision).toHaveBeenCalledWith("ws_1", "dec_1", { action: "reject", expected_revision: 1 }));
    expect(await within(detail()).findByText("處理方式：駁回")).toBeInTheDocument();
  });

  it("延後：選擇時數，送出後移到「已延後」並說明延後不是同意", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    const client = fakeClient([decisionView(), decisionView({ id: "dec_2", question: "第二個問題" })]);
    renderCenter(client, onChange);

    await user.click(await screen.findByRole("radio", { name: "延後" }));
    expect(within(form()).getByText("延後不是同意，工作仍在等待。")).toBeInTheDocument();
    expect(within(form()).getByLabelText("延後多久")).toHaveValue("1440");
    await user.selectOptions(within(form()).getByLabelText("延後多久"), "3 天");
    await user.click(screen.getByRole("button", { name: "送出裁定" }));

    expect(client.answerDecision).toHaveBeenCalledWith("ws_1", "dec_1", { action: "defer", expected_revision: 1, defer_minutes: 4320 });
    expect(await screen.findByRole("heading", { name: "已延後（1）" })).toBeInTheDocument();
    expect(onChange).toHaveBeenCalledTimes(1);
    // 仍停在這筆，顯示延後狀態，且仍可處理
    expect(within(detail()).getByRole("heading", { name: "要保留舊的目錄結構嗎？" })).toBeInTheDocument();
    expect(within(detail()).getByText(/^延後至 2023\/11\/18 06:13（延後不是同意，工作仍在等待）$/)).toBeInTheDocument();
    expect(within(detail()).getByRole("button", { name: "送出裁定" })).toBeDisabled();
  });

  it("目標版本已變更時顯示清楚說明，重新載入後標為已過期、不能再套用", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    const client = fakeClient([decisionView(), decisionView({ id: "dec_2", question: "第二個問題" })], { supersedeOnSubmit: ["dec_1"] });
    renderCenter(client, onChange);

    await user.click(await screen.findByRole("radio", { name: "接受方案" }));
    await user.click(within(form()).getByRole("radio", { name: "保留（建議）" }));
    await user.click(screen.getByRole("button", { name: "送出裁定" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("目標版本已變更，這個請求已過期，不能套用。");
    expect(client.listDecisions).toHaveBeenCalledTimes(2);
    expect(onChange).not.toHaveBeenCalled();
    // 仍停在這筆（已移到已結束），顯示已過期，沒有任何處理
    expect(within(detail()).getByRole("heading", { name: "要保留舊的目錄結構嗎？" })).toBeInTheDocument();
    expect(within(detail()).getByRole("heading", { name: "已過期" })).toBeInTheDocument();
    expect(within(detail()).getByText("原因：契約已更新到第 2 版")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "送出裁定" })).not.toBeInTheDocument();
  });

  it("內容已變更（revision 不符）時說明並重新整理，可以再送一次", async () => {
    const user = userEvent.setup();
    const client = fakeClient([decisionView()]);
    client.answerDecision = vi.fn(async () => {
      throw conflict("decision_revision_mismatch", "內容已變更");
    });
    renderCenter(client);

    await user.click(await screen.findByRole("radio", { name: "駁回" }));
    await user.click(screen.getByRole("button", { name: "送出裁定" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("這個請求的內容已變更，已重新整理，請確認後再送出。");
    expect(client.listDecisions).toHaveBeenCalledTimes(2);
    expect(screen.getByRole("button", { name: "送出裁定" })).toBeEnabled();
  });

  it("其他錯誤（已被回答過）顯示 Runtime 的說明並重新載入", async () => {
    const user = userEvent.setup();
    const client = fakeClient([decisionView()]);
    client.answerDecision = vi.fn(async () => {
      throw new Error("這個裁定已經回答過了");
    });
    renderCenter(client);

    await user.click(await screen.findByRole("radio", { name: "駁回" }));
    await user.click(screen.getByRole("button", { name: "送出裁定" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("這個裁定已經回答過了");
    expect(client.listDecisions).toHaveBeenCalledTimes(2);
    expect(screen.getByRole("button", { name: "送出裁定" })).toBeEnabled();
  });
});

describe("裁定中心：相似問題", () => {
  it("顯示連結與合併來源，點了可前往", async () => {
    const user = userEvent.setup();
    renderCenter(
      fakeClient([
        decisionView({
          links: [{ id: "dec_2", question: "第二個問題", status: "pending" }],
          merged_from: [{ id: "dec_old", question: "重複的問題", status: "merged" }],
        }),
        decisionView({ id: "dec_2", question: "第二個問題" }),
        decisionView({ id: "dec_old", question: "重複的問題", status: "merged", merged_into: "dec_1", allowed_actions: [] }),
      ]),
    );

    const related = await screen.findByRole("region", { name: "相似問題" });
    expect(within(related).getByText("（待處理）")).toBeInTheDocument();
    expect(within(related).getByText("（已合併）")).toBeInTheDocument();

    await user.click(within(related).getByRole("button", { name: "第二個問題" }));

    expect(within(detail()).getByRole("heading", { name: "第二個問題" })).toBeInTheDocument();
  });

  it("可以合併到同種類、待處理的連結請求，送出兩邊的 revision，之後前往目標", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    const client = fakeClient(
      [
        decisionView({
          revision: 2,
          links: [
            { id: "dec_b", question: "同一件事的另一個問法", status: "pending" },
            { id: "dec_p", question: "權限提示", status: "pending" },
            { id: "dec_done", question: "要不要升級相依套件？", status: "answered" },
          ],
        }),
        decisionView({ id: "dec_b", question: "同一件事的另一個問法", revision: 5 }),
        decisionView({ id: "dec_p", question: "權限提示", kind: "permission" }),
        answeredDecision(),
      ],
    );
    renderCenter(client, onChange);

    const related = await screen.findByRole("region", { name: "相似問題" });
    const select = within(related).getByLabelText("合併到");
    expect(within(select).getAllByRole("option").map((o) => o.textContent)).toEqual(["選擇要合併到的請求", "同一件事的另一個問法"]);
    expect(within(related).getByRole("button", { name: "合併" })).toBeDisabled();

    await user.selectOptions(select, "同一件事的另一個問法");
    await user.click(within(related).getByRole("button", { name: "合併" }));

    expect(client.mergeDecision).toHaveBeenCalledWith("ws_1", "dec_1", { into: "dec_b", expected_revision: 2, into_expected_revision: 5 });
    expect(await within(detail()).findByRole("heading", { name: "同一件事的另一個問法" })).toBeInTheDocument();
    expect(onChange).toHaveBeenCalledTimes(1);
    const mergedFrom = within(detail()).getByRole("region", { name: "相似問題" });
    expect(within(mergedFrom).getByText("已合併進來")).toBeInTheDocument();
    expect(within(mergedFrom).getByRole("button", { name: "要保留舊的目錄結構嗎？" })).toBeInTheDocument();
  });

  it("權限提示不能合併", async () => {
    renderCenter(
      fakeClient([
        decisionView({ kind: "permission", links: [{ id: "dec_p2", question: "另一個權限提示", status: "pending" }] }),
        decisionView({ id: "dec_p2", kind: "permission", question: "另一個權限提示" }),
      ]),
    );

    const related = await screen.findByRole("region", { name: "相似問題" });
    expect(within(related).queryByLabelText("合併到")).not.toBeInTheDocument();
  });

  it("合併失敗時顯示說明並重新載入", async () => {
    const user = userEvent.setup();
    const client = fakeClient([
      decisionView({ links: [{ id: "dec_b", question: "另一個問法", status: "pending" }] }),
      decisionView({ id: "dec_b", question: "另一個問法" }),
    ]);
    client.mergeDecision = vi.fn(async () => {
      throw conflict("decision_revision_mismatch", "內容已變更");
    });
    renderCenter(client);

    const related = await screen.findByRole("region", { name: "相似問題" });
    await user.selectOptions(within(related).getByLabelText("合併到"), "另一個問法");
    await user.click(within(related).getByRole("button", { name: "合併" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("這個請求的內容已變更，已重新整理，請確認後再送出。");
    expect(client.listDecisions).toHaveBeenCalledTimes(2);
  });

  it("可以從清單中選另一個請求手動連結", async () => {
    const user = userEvent.setup();
    const client = fakeClient([
      decisionView({ links: [{ id: "dec_2", question: "已連結的問題", status: "pending" }] }),
      decisionView({ id: "dec_2", question: "已連結的問題" }),
      decisionView({ id: "dec_3", question: "第三個問題" }),
      answeredDecision(),
    ]);
    renderCenter(client);

    const related = await screen.findByRole("region", { name: "相似問題" });
    const select = within(related).getByLabelText("連結另一個請求");
    // 不含自己與已連結的請求
    expect(within(select).getAllByRole("option").map((o) => o.textContent)).toEqual([
      "選擇要連結的請求",
      "第三個問題",
      "要不要升級相依套件？",
    ]);

    await user.selectOptions(select, "第三個問題");
    await user.click(within(related).getByRole("button", { name: "連結" }));

    expect(client.linkDecision).toHaveBeenCalledWith("ws_1", "dec_1", "dec_3");
    expect(await within(related).findByRole("button", { name: "第三個問題" })).toBeInTheDocument();
  });
});

describe("裁定中心：批次處理", () => {
  const twoPending = () => [
    decisionView({ revision: 3 }),
    decisionView({ id: "dec_2", question: "第二個問題", revision: 7 }),
    decisionView({ id: "dec_3", question: "第三個問題" }),
  ];

  it("勾選後出現批次面板，逐項選擇處理方式後各自送出並顯示每一項的結果", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    const client = fakeClient(twoPending());
    renderCenter(client, onChange);
    await screen.findByRole("article", { name: "裁定詳情" });
    expect(screen.queryByRole("form", { name: "批次處理" })).not.toBeInTheDocument();

    await user.click(screen.getByRole("checkbox", { name: "批次選取 要保留舊的目錄結構嗎？" }));
    await user.click(screen.getByRole("checkbox", { name: "批次選取 第二個問題" }));

    const panel = screen.getByRole("form", { name: "批次處理" });
    expect(within(panel).getByText("逐項選擇處理方式；每一項各自保存，不會形成概括授權。")).toBeInTheDocument();
    // 沒有「全部套用」的捷徑
    expect(within(panel).queryByRole("button", { name: /全部/ })).not.toBeInTheDocument();
    const submit = within(panel).getByRole("button", { name: "送出批次（2 項）" });
    expect(submit).toBeDisabled();

    const first = within(panel).getByRole("group", { name: "要保留舊的目錄結構嗎？" });
    const second = within(panel).getByRole("group", { name: "第二個問題" });
    await user.click(within(first).getByRole("radio", { name: "接受方案" }));
    await user.click(within(first).getByRole("radio", { name: "保留（建議）" }));
    expect(submit).toBeDisabled();
    await user.click(within(second).getByRole("radio", { name: "回答" }));
    await user.type(within(second).getByLabelText("回答內容（必填）"), "用新結構");
    await user.click(submit);

    expect(client.batchDecisions).toHaveBeenCalledWith([
      { workspace_id: "ws_1", request_id: "dec_1", action: "accept", expected_revision: 3, option_id: "keep" },
      { workspace_id: "ws_1", request_id: "dec_2", action: "answer", expected_revision: 7, note: "用新結構" },
    ]);
    const results = await screen.findByRole("region", { name: "批次結果" });
    expect(within(results).getByText("要保留舊的目錄結構嗎？：已保存")).toBeInTheDocument();
    expect(within(results).getByText("第二個問題：已保存")).toBeInTheDocument();
    expect(within(results).getByText("每一項各自保存，不會形成概括授權。")).toBeInTheDocument();
    expect(onChange).toHaveBeenCalledTimes(1);
    expect(client.listDecisions).toHaveBeenCalledTimes(2);
    expect(screen.queryByRole("form", { name: "批次處理" })).not.toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "可處理（1）" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "已結束（2）" })).toBeInTheDocument();
  });

  it("部分失敗時每一項各自顯示結果，成功的照樣保存", async () => {
    const user = userEvent.setup();
    const client = fakeClient(twoPending(), { supersedeOnSubmit: ["dec_2"] });
    renderCenter(client);
    await screen.findByRole("article", { name: "裁定詳情" });

    await user.click(screen.getByRole("checkbox", { name: "批次選取 要保留舊的目錄結構嗎？" }));
    await user.click(screen.getByRole("checkbox", { name: "批次選取 第二個問題" }));
    const panel = screen.getByRole("form", { name: "批次處理" });
    await user.click(within(within(panel).getByRole("group", { name: "要保留舊的目錄結構嗎？" })).getByRole("radio", { name: "駁回" }));
    await user.click(within(within(panel).getByRole("group", { name: "第二個問題" })).getByRole("radio", { name: "駁回" }));
    await user.click(within(panel).getByRole("button", { name: "送出批次（2 項）" }));

    const results = await screen.findByRole("region", { name: "批次結果" });
    expect(within(results).getByText("要保留舊的目錄結構嗎？：已保存")).toBeInTheDocument();
    expect(within(results).getByText("第二個問題：失敗（目標版本已變更，這個請求已過期，不能套用。）")).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "已結束（2）" })).toBeInTheDocument();
  });

  it("取消勾選或取消批次會收起面板", async () => {
    const user = userEvent.setup();
    renderCenter(fakeClient(twoPending()));
    await screen.findByRole("article", { name: "裁定詳情" });

    await user.click(screen.getByRole("checkbox", { name: "批次選取 第二個問題" }));
    await user.click(screen.getByRole("checkbox", { name: "批次選取 第二個問題" }));
    expect(screen.queryByRole("form", { name: "批次處理" })).not.toBeInTheDocument();

    await user.click(screen.getByRole("checkbox", { name: "批次選取 第三個問題" }));
    await user.click(screen.getByRole("button", { name: "取消批次" }));
    expect(screen.queryByRole("form", { name: "批次處理" })).not.toBeInTheDocument();
    expect(screen.getByRole("checkbox", { name: "批次選取 第三個問題" })).not.toBeChecked();
  });
});

describe("裁定中心：決策權限表", () => {
  it("列出事項的內建與目前生效層級；只有內建可自主處理的能收窄，其他顯示鎖定", async () => {
    renderCenter(fakeClient([]));

    const panel = await authorityPanel();
    const row = (label: string) => within(panel).getByText(label).closest("tr")!;
    expect(within(row("拆分、合併、排序 Task")).getAllByText("Controller 可自主處理")).toHaveLength(2);
    expect(within(row("拆分、合併、排序 Task")).getByRole("checkbox", { name: "收窄為必須送裁定：拆分、合併、排序 Task" })).not.toBeChecked();
    expect(within(row("重試 Task")).getByText("必須送裁定（已收窄）")).toBeInTheDocument();
    expect(within(row("重試 Task")).getByRole("checkbox", { name: "收窄為必須送裁定：重試 Task" })).toBeChecked();
    expect(within(row("修改完成條件或驗收標準")).queryByRole("checkbox")).not.toBeInTheDocument();
    expect(within(row("修改完成條件或驗收標準")).getByText("內建必須送裁定，Workspace 不能放寬")).toBeInTheDocument();
    expect(within(row("三次恢復失敗後的重試")).getByText("內建只能人工發起，Workspace 不能放寬")).toBeInTheDocument();
    expect(within(panel).getByRole("button", { name: "儲存權限表" })).toBeDisabled();
  });

  it("收窄後存檔帶 expected_revision，只列出和內建不同的事項", async () => {
    const user = userEvent.setup();
    const client = fakeClient([]);
    renderCenter(client);
    const panel = await authorityPanel();

    await user.click(within(panel).getByRole("checkbox", { name: "收窄為必須送裁定：拆分、合併、排序 Task" }));
    await user.click(within(panel).getByRole("button", { name: "儲存權限表" }));

    expect(client.updateDecisionAuthority).toHaveBeenCalledWith("ws_1", {
      expected_revision: 1,
      entries: [
        { matter: "split_task", effective: "requires_decision" },
        { matter: "retry_task", effective: "requires_decision" },
      ],
    });
    expect(await within(panel).findByText("已儲存（第 2 版）")).toBeInTheDocument();
    expect(within(panel).getByText("拆分、合併、排序 Task").closest("tr")).toHaveTextContent("必須送裁定（已收窄）");
  });

  it("取消收窄就回到內建值（不列在 entries 裡）", async () => {
    const user = userEvent.setup();
    const client = fakeClient([]);
    renderCenter(client);
    const panel = await authorityPanel();

    await user.click(within(panel).getByRole("checkbox", { name: "收窄為必須送裁定：重試 Task" }));
    await user.click(within(panel).getByRole("button", { name: "儲存權限表" }));

    expect(client.updateDecisionAuthority).toHaveBeenCalledWith("ws_1", { expected_revision: 1, entries: [] });
  });

  it("版本不符時說明並重新載入", async () => {
    const user = userEvent.setup();
    const client = fakeClient([], { authority: authorityTable({ revision: 3 }) });
    client.updateDecisionAuthority = vi.fn(async () => {
      throw conflict("authority_revision_mismatch", "權限表版本不符");
    });
    renderCenter(client);
    const panel = await authorityPanel();

    await user.click(within(panel).getByRole("checkbox", { name: "收窄為必須送裁定：拆分、合併、排序 Task" }));
    await user.click(within(panel).getByRole("button", { name: "儲存權限表" }));

    expect(await within(panel).findByRole("alert")).toHaveTextContent("決策權限表已被修改，已重新載入，請重新確認。");
    expect(client.decisionAuthority).toHaveBeenCalledTimes(2);
  });

  it("讀不到權限表時只在該區塊顯示原因，不影響裁定清單", async () => {
    const client = fakeClient([decisionView()]);
    client.decisionAuthority = vi.fn(async () => {
      throw new Error("Runtime 回應 503");
    });
    renderCenter(client);

    const panel = await authorityPanel();
    expect(await within(panel).findByRole("alert")).toHaveTextContent("無法取得決策權限表：Runtime 回應 503");
    expect(screen.getByRole("article", { name: "裁定詳情" })).toBeInTheDocument();
  });
});
