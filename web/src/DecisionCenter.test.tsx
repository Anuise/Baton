import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import { clearDecisionDrafts, DecisionAuthorityPanel, DecisionCenter } from "./DecisionCenter";
import { authorityTable, CHECKED_AT, decisionView } from "./fixtures";
import type {
  AnswerRequest,
  AuthorityTable,
  AuthorizationClient,
  AuthorizationRevision,
  AuthorizationView,
  BatchItem,
  ConfirmationSummary,
  ConfirmAuthorizationRequest,
  DecisionClient,
  DecisionOverview,
  DecisionPermission,
  DecisionRequestView,
  Grant,
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
  model_overrides: { controller: null, worker: null, reviewer: null },
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

const HASH = "3f2a9c1b7d4e5f60718293a4b5c6d7e8f90123456789abcdef0123456789abcd";

/** 權限請求：git push 到 origin main，綁定單一邏輯操作。 */
const permissionOf = (overrides: Partial<DecisionPermission> = {}): DecisionPermission => ({
  tool_name: "Bash",
  input: { command: "git push --force origin main" },
  capability: "remote_write",
  capability_label: "寫入遠端",
  summary: "推送到 origin main",
  binding: {
    operation: "Bash:git push",
    target: "origin main",
    params: { flags: ["--force"] },
    request_hash: HASH,
    artifact_version: "report.md@2,notes.md@1",
  },
  prior_use: null,
  rejected_approval: null,
  approval_use: null,
  ...overrides,
});

const permissionDecision = (overrides: Partial<DecisionRequestView> = {}) =>
  decisionView({
    id: "dec_perm",
    kind: "permission",
    question: "要允許推送到 origin main 嗎？",
    options: [
      { id: "allow_once", label: "允許本次操作", tradeoff: null },
      { id: "deny", label: "拒絕", tradeoff: null },
    ],
    recommendation: null,
    permission: permissionOf(),
    allowed_actions: ["accept", "defer", "reject"],
    ...overrides,
  });

const currentRevision: AuthorizationRevision = {
  workspace_id: "ws_1",
  revision: 3,
  grants: [{ capability: "read_files", targets: [workspace.path], restrictions: [] }],
  source: { kind: "template_edited", template_id: "research_docs", base_revision: 2, confirmed_by: "desktop" },
  confirmed_at: CHECKED_AT,
};

interface FakeOptions {
  resumed?: string[];
  authority?: AuthorityTable;
  /** 送出時目標版本已變更的請求：Runtime 會把它標為 superseded 並拒絕。 */
  supersedeOnSubmit?: string[];
  /** 目前生效的 Workspace 授權；省略時用 currentRevision。 */
  revision?: AuthorizationRevision;
}

/** 以記憶體模擬 Runtime：處理後更新請求狀態與 revision，並檢查 expected_revision；授權可被修改。 */
function fakeClient(initial: DecisionRequestView[], options: FakeOptions = {}) {
  let items = [...initial];
  let table = options.authority ?? authorityTable();
  let revisions = [options.revision ?? currentRevision];
  const authorizationView = (): AuthorizationView => ({
    workspace_id: "ws_1",
    status: "confirmed",
    effective: revisions.at(-1)!,
    revisions: [...revisions],
    templates: [],
    capabilities: [],
    risk_disclosure: "",
    legacy_runs: [],
  });
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
  const client: DecisionClient & AuthorizationClient = {
    getAuthorization: vi.fn(async () => authorizationView()),
    summarizeAuthorization: vi.fn(
      async (_ws: string, grants: Grant[]): Promise<ConfirmationSummary> => ({
        items: grants.map((g) => ({ ...g, label: g.capability, high_risk: false })),
        risks: ["推送會改變遠端儲存庫，其他人會看到。"],
        requires_risk_acknowledgement: true,
        changes: {
          base_revision: revisions.at(-1)!.revision,
          changes: [
            {
              capability: "remote_write",
              label: "寫入遠端",
              high_risk: true,
              new_capability: true,
              removed_capability: false,
              added_targets: [workspace.path],
              removed_targets: [],
              added_restrictions: [],
              removed_restrictions: [],
              expands: true,
            },
          ],
          expands: true,
          impact: ["Agent 之後在這個 Workspace 的所有 Task 都能寫入遠端，不必再逐次詢問。"],
        },
      }),
    ),
    confirmAuthorization: vi.fn(async (_ws: string, { grants }: ConfirmAuthorizationRequest) => {
      const revision = { ...revisions.at(-1)!, revision: revisions.at(-1)!.revision + 1, grants };
      revisions = [...revisions, revision];
      return revision;
    }),
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
          return { ...entry, effective: level, overridden: level !== entry.builtin };
        }),
      };
      return table;
    }),
    pendingDecisions: vi.fn(async () => ({ pending_count: 0, by_workspace: [] })),
  };
  return client;
}

// 草稿保存在模組層級（換頁再回來還在）：每則測試之間清空
afterEach(clearDecisionDrafts);

const renderCenter = (client: DecisionClient & AuthorizationClient, onChange?: () => void) =>
  render(<DecisionCenter client={client} workspace={workspace} timeZone="Asia/Taipei" onChange={onChange} />);

const detail = () => screen.getByRole("article", { name: "裁定詳情" });
const form = () => detail().querySelector("form")!;

/** 直接顯示在畫面上，不是收在「?」說明裡（說明內容平時是 hidden）。 */
function expectShownDirectly(element: HTMLElement) {
  expect(element).toBeVisible();
  expect(element.closest(".help-tip")).toBeNull();
}

/** 收在預設收合的「技術細節」裡。 */
function expectInTechDetails(element: HTMLElement) {
  const details = element.closest("details");
  expect(details).not.toBeNull();
  expect(details!.querySelector("summary")).toHaveTextContent(/^技術細節$/);
  expect(details).not.toHaveAttribute("open");
  expect(element).not.toBeVisible();
}
/** 等決策權限表載入完成（區塊會先顯示「載入中…」）。 */
const authorityPanel = async () => {
  const panel = await screen.findByRole("region", { name: "決策權限" });
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
    expect(count("全部待裁定")).toBe("3");
    expect(count("可處理")).toBe("2");
    expect(count("已延後")).toBe("1");
    expect(count("已逾期")).toBe("1");
    expect(count("高緊急")).toBe("1");
    expect(count("已回答")).toBe("1");
    expect(count("已失效")).toBe("1");
    expect(count("已合併")).toBe("1");
    // 「待處理」與「可處理」並列看不出差別、「已過期」與「已逾期」只差一字：都不再使用
    expect(overview).not.toHaveTextContent("待處理");
    expect(overview).not.toHaveTextContent("已過期");
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

    expect(await screen.findByRole("region", { name: "可處理（2）" })).toBeInTheDocument();
    expect(screen.getByRole("region", { name: "已延後（1）" })).toBeInTheDocument();
    expect(screen.getByRole("region", { name: "已結束（2）" })).toBeInTheDocument();
    // 標題只寫組名，計數只放數字、在標題外（MASTER 7.1）
    const heading = within(screen.getByRole("region", { name: "可處理（2）" })).getByRole("heading", { name: "可處理" });
    expect(heading.nextElementSibling).toHaveClass("count");
    expect(heading.nextElementSibling).toHaveTextContent(/^2$/);
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
    expect(within(buttons[4]).getByText("已失效")).toHaveAttribute("title", "內容或目標版本已改變，這筆不能再回答。");
    expect(screen.getAllByRole("checkbox", { name: /^批次選取/ }).map((c) => c.getAttribute("aria-label"))).toEqual([
      "批次選取 要允許執行 npm install 嗎？",
      "批次選取 要保留舊的目錄結構嗎？",
    ]);
  });

  it("沒有延後的裁定時不顯示「已延後」一組；「已結束」一組可以收合再展開（收合摘要樣式）", async () => {
    const user = userEvent.setup();
    renderCenter(fakeClient([decisionView(), answeredDecision()]));

    expect(await screen.findByRole("region", { name: "可處理（1）" })).toBeInTheDocument();
    expect(screen.queryByRole("region", { name: /^已延後/ })).not.toBeInTheDocument();
    // 總覽仍列出已延後的數字
    expect(within(screen.getByRole("region", { name: "裁定總覽" })).getByText("已延後")).toBeInTheDocument();

    const toggle = screen.getByRole("button", { name: "收合" });
    expect(toggle).toHaveAttribute("aria-expanded", "true");
    // 與 <details> 的 ▶ 一致的收合摘要樣式，不是低調按鈕
    expect(toggle).toHaveClass("decision-group-toggle");
    expect(toggle).not.toHaveClass("btn-quiet");
    await user.click(toggle);
    expect(screen.getByRole("region", { name: "已結束（1）" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "查看裁定 要不要升級相依套件？" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "展開" })).toHaveAttribute("aria-expanded", "false");

    await user.click(screen.getByRole("button", { name: "展開" }));
    expect(screen.getByRole("button", { name: "查看裁定 要不要升級相依套件？" })).toBeInTheDocument();
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

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveClass("notice", "notice-danger");
    expect(within(alert).getByText("無法取得裁定清單")).toHaveClass("notice-title");
    expect(alert).toHaveTextContent("Runtime 回應 503");
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
    // 目標版本的兩項各自一段（不以「・」串接）
    expect(within(article).getByText("目標版本：契約第 3 版")).toBeInTheDocument();
    expect(within(article).getByText("授權第 2 版")).toBeInTheDocument();
    // 解釋改放在「?」說明；目標版本收在預設收合的「詳細資訊」
    expect(within(article).getByRole("button", { name: "阻擋範圍的說明" })).toBeInTheDocument();
    const more = within(article).getByText("目標版本：契約第 3 版").closest("details")!;
    expect(more).not.toHaveAttribute("open");
    expect(more.querySelector("summary")).toHaveTextContent(/^詳細資訊$/);
    expect(more).toContainElement(within(article).getByText("授權第 2 版"));
  });

  it("權限提示顯示工具、能力與摘要", async () => {
    renderCenter(
      fakeClient([
        decisionView({
          kind: "permission",
          permission: permissionOf({
            tool_name: "Bash",
            input: { command: "npm install" },
            capability: "install_project_dependencies",
            capability_label: "安裝專案依賴",
            summary: "在專案目錄執行 npm install",
          }),
        }),
      ]),
    );

    const permission = await screen.findByRole("region", { name: "權限提示" });
    expect(within(permission).getByText("Bash")).toBeInTheDocument();
    expect(within(permission).getByText("安裝專案依賴")).toBeInTheDocument();
    expect(within(permission).getByText("在專案目錄執行 npm install")).toBeInTheDocument();
  });

  it("決策權限攔截的請求顯示事項與摘要", async () => {
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
    expect(within(button).getByText("決策權限攔截")).toBeInTheDocument();
    const authority = screen.getByRole("region", { name: "決策權限攔截" });
    expect(within(authority).getByText("修改完成條件或驗收標準")).toBeInTheDocument();
    expect(within(authority).getByText("把覆蓋率門檻從 80% 降到 60%")).toBeInTheDocument();
  });

  it("預算用盡的請求（後端以 authority 種類建立，不看決策權限表）標成「預算用盡」，不提供「調整決策權限」", async () => {
    const onOpenPage = vi.fn();
    render(
      <DecisionCenter
        client={fakeClient([
          decisionView({
            kind: "authority",
            question: "Task「整理文件」的重試預算（3 次）已用完，下一步怎麼做？",
            authority: { matter: "next_step_after_budget", matter_label: "預算用盡後的下一步", summary: "「整理文件」的重試預算（3 次）已用完" },
          }),
        ])}
        workspace={workspace}
        timeZone="Asia/Taipei"
        onOpenPage={onOpenPage}
      />,
    );

    const button = await screen.findByRole("button", { name: "查看裁定 Task「整理文件」的重試預算（3 次）已用完，下一步怎麼做？" });
    expect(within(button).getByText("預算用盡")).toBeInTheDocument();
    expect(within(button).queryByText("決策權限攔截")).not.toBeInTheDocument();
    expect(within(detail()).getByText("預算用盡", { selector: ".badge" })).toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "決策權限攔截" })).not.toBeInTheDocument();
    const budget = screen.getByRole("region", { name: "預算用盡" });
    expect(within(budget).getByText("Runtime 已停止自動重試；只有選「再給一輪」Task 才會繼續。")).toBeInTheDocument();
    expect(within(budget).getByText("預算用盡後的下一步")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "調整決策權限" })).not.toBeInTheDocument();
  });

  it("逾期只標示「已逾期，尚未裁定」，沒有任何自動同意的字樣，也不會自行回答", async () => {
    const client = fakeClient([decisionView({ overdue: true, deadline_at: CHECKED_AT - 60_000 })]);

    const { container } = renderCenter(client);

    expect((await screen.findAllByText(/已逾期，尚未裁定/)).length).toBeGreaterThan(0);
    expect(container.textContent).not.toMatch(/自動|同意|核准|視為/);
    expect(client.answerDecision).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "送出裁定" })).toBeDisabled();
  });

  it("已失效（superseded）的請求說明不能再回答與原因，不提供任何處理", async () => {
    const user = userEvent.setup();
    renderCenter(
      fakeClient([
        decisionView(),
        decisionView({ id: "dec_s", question: "過期的問題", status: "superseded", status_reason: "契約已更新到第 2 版", allowed_actions: [] }),
      ]),
    );

    await user.click(await screen.findByRole("button", { name: "查看裁定 過期的問題" }));

    const outcome = within(detail()).getByRole("region", { name: "裁定結果" });
    expect(within(outcome).getByRole("heading", { name: "已失效" })).toBeInTheDocument();
    expect(within(outcome).getByText("內容或目標版本已改變，這筆不能再回答。")).toBeInTheDocument();
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

  it("延後的請求顯示「延後至 …」並直接說明延後不是同意，仍可處理", async () => {
    renderCenter(fakeClient([decisionView({ deferred: true, deferred_until: CHECKED_AT + 3_600_000 })]));

    const deferred = await screen.findByText(/^延後至 2023\/11\/15 07:13/, { selector: "p" });
    expect(deferred).toHaveTextContent("延後不是同意，工作仍在等待。");
    // 延後不是同意帶有授權意涵，直接顯示，不收在「?」說明裡
    expectShownDirectly(within(deferred).getByText("延後不是同意，工作仍在等待。"));
    expect(within(deferred).queryByRole("button", { name: "延後的說明" })).not.toBeInTheDocument();
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
    // 誰、何時、是否批次：同一列的三段次要資訊（不以「・」串接）
    const meta = within(outcome).getByText("desktop").parentElement!;
    expect(meta).toHaveClass("meta");
    expect(within(meta).getByText("2023/11/15 06:14")).toBeInTheDocument();
    expect(within(meta).getByText("批次處理（此項各自保存）")).toBeInTheDocument();
    expect(outcome).not.toHaveTextContent("・");
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
    expect(screen.getByRole("region", { name: "可處理（1）" })).toBeInTheDocument();
    expect(screen.getByRole("region", { name: "已結束（1）" })).toBeInTheDocument();
  });

  it("送出裁定後處理區換成裁定結果，焦點移到「裁定結果」標題（不落到 body）", async () => {
    const user = userEvent.setup();
    renderCenter(fakeClient([decisionView()]));

    await user.click(await screen.findByRole("radio", { name: "駁回" }));
    await user.click(screen.getByRole("button", { name: "送出裁定" }));

    const outcome = await screen.findByRole("region", { name: "裁定結果" });
    await waitFor(() => expect(within(outcome).getByRole("heading", { name: "裁定結果" })).toHaveFocus());
  });

  it("從詳情裡的連結（已合併到…、相似問題）換筆時，焦點移到新詳情的標題；從清單換筆時焦點留在清單", async () => {
    const user = userEvent.setup();
    renderCenter(
      fakeClient([
        decisionView({ id: "dec_target", question: "目標問題", links: [{ id: "dec_other", question: "另一個問法", status: "pending" }] }),
        decisionView({ id: "dec_other", question: "另一個問法" }),
        decisionView({ id: "dec_m", question: "被合併的問題", status: "merged", merged_into: "dec_target", allowed_actions: [] }),
      ]),
    );

    const listButton = await screen.findByRole("button", { name: "查看裁定 被合併的問題" });
    await user.click(listButton);
    expect(listButton).toHaveFocus();

    await user.click(within(within(detail()).getByRole("region", { name: "裁定結果" })).getByRole("button", { name: "目標問題" }));
    expect(within(detail()).getByRole("heading", { level: 3, name: "目標問題" })).toHaveFocus();

    await user.click(within(within(detail()).getByRole("region", { name: "相似問題" })).getByRole("button", { name: "另一個問法" }));
    expect(within(detail()).getByRole("heading", { level: 3, name: "另一個問法" })).toHaveFocus();
  });

  it("建議的選項在選擇時也標示「建議」", async () => {
    const user = userEvent.setup();
    renderCenter(fakeClient([decisionView()]));

    await user.click(await screen.findByRole("radio", { name: "接受方案" }));

    expect(within(form()).getByRole("radio", { name: "保留（建議）" })).toBeInTheDocument();
    expect(within(form()).getByRole("radio", { name: "重整" })).toBeInTheDocument();
  });

  it("「採用建議」一鍵設定處理方式＝接受方案＋建議的選項，不必選兩次；備註保留", async () => {
    const user = userEvent.setup();
    const client = fakeClient([decisionView({ revision: 2 })]);
    renderCenter(client);
    const submit = await screen.findByRole("button", { name: "送出裁定" });

    // 「採用建議」在選項與取捨中建議的那一個選項旁
    const keep = within(detail()).getByText("保留").closest("li")!;
    const restructure = within(detail()).getByText("重整").closest("li")!;
    expect(within(restructure).queryByRole("button", { name: "採用建議" })).not.toBeInTheDocument();
    await user.click(within(form()).getByRole("radio", { name: "回答" }));
    await user.type(within(form()).getByLabelText("回答內容（必填）"), "照建議");
    // jsdom 沒有 scrollIntoView：暫時補上，檢查採用後會把處理區（連同「送出裁定」）捲進視窗
    const scrollIntoView = vi.fn();
    Object.defineProperty(HTMLElement.prototype, "scrollIntoView", { value: scrollIntoView, configurable: true });
    try {
      await user.click(within(keep).getByRole("button", { name: "採用建議" }));
    } finally {
      delete (HTMLElement.prototype as { scrollIntoView?: unknown }).scrollIntoView;
    }
    expect(scrollIntoView.mock.contexts).toEqual([form()]);

    expect(within(form()).getByRole("radio", { name: "接受方案" })).toBeChecked();
    expect(within(form()).getByRole("radio", { name: "保留（建議）" })).toBeChecked();
    expect(within(form()).getByLabelText("備註（選填）")).toHaveValue("照建議");
    expect(submit).toBeEnabled();
    await user.click(submit);

    expect(client.answerDecision).toHaveBeenCalledWith("ws_1", "dec_1", {
      action: "accept",
      expected_revision: 2,
      option_id: "keep",
      note: "照建議",
    });
  });

  it.each([
    ["沒有建議的選項", decisionView({ recommendation: null })],
    ["不能接受方案", decisionView({ allowed_actions: ["answer", "defer", "reject"] })],
  ])("%s時不提供「採用建議」", async (_label, decision) => {
    renderCenter(fakeClient([decision]));

    await screen.findByRole("button", { name: "送出裁定" });
    expect(screen.queryByRole("button", { name: "採用建議" })).not.toBeInTheDocument();
  });

  it("已結束的請求不提供「採用建議」", async () => {
    const user = userEvent.setup();
    renderCenter(fakeClient([decisionView({ id: "dec_open", question: "另一個問題" }), answeredDecision({ options: decisionView().options, recommendation: "keep" })]));

    await user.click(await screen.findByRole("button", { name: "查看裁定 要不要升級相依套件？" }));

    expect(within(detail()).getByText("建議")).toBeInTheDocument();
    expect(within(detail()).queryByRole("button", { name: "採用建議" })).not.toBeInTheDocument();
  });

  it("處理區在相似問題之前（相似問題與手動連結很少用）", async () => {
    renderCenter(fakeClient([decisionView(), decisionView({ id: "dec_2", question: "第二個問題" })]));

    const related = await screen.findByRole("region", { name: "相似問題" });
    expect(form().compareDocumentPosition(related) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
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
    expectShownDirectly(within(form()).getByText("延後不是同意，工作仍在等待。"));
    expect(within(form()).queryByRole("button", { name: "延後的說明" })).not.toBeInTheDocument();
    expect(within(form()).getByLabelText("延後多久")).toHaveValue("1440");
    await user.selectOptions(within(form()).getByLabelText("延後多久"), "3 天");
    await user.click(screen.getByRole("button", { name: "送出裁定" }));

    expect(client.answerDecision).toHaveBeenCalledWith("ws_1", "dec_1", { action: "defer", expected_revision: 1, defer_minutes: 4320 });
    expect(await screen.findByRole("region", { name: "已延後（1）" })).toBeInTheDocument();
    expect(onChange).toHaveBeenCalledTimes(1);
    // 仍停在這筆，顯示延後狀態，且仍可處理
    expect(within(detail()).getByRole("heading", { name: "要保留舊的目錄結構嗎？" })).toBeInTheDocument();
    expect(within(detail()).getByText(/^延後至 2023\/11\/18 06:13/, { selector: "p" })).toHaveTextContent("延後不是同意，工作仍在等待。");
    expectShownDirectly(
      within(within(detail()).getByText(/^延後至 2023\/11\/18 06:13/, { selector: "p" })).getByText("延後不是同意，工作仍在等待。"),
    );
    expect(within(detail()).getByRole("button", { name: "送出裁定" })).toBeDisabled();
  });

  it("填到一半換筆、或換到別頁再回來，草稿都還在；送出成功後清掉", async () => {
    const user = userEvent.setup();
    const client = fakeClient([decisionView(), decisionView({ id: "dec_2", question: "第二個問題" })]);
    const first = renderCenter(client);
    await screen.findByRole("article", { name: "裁定詳情" });

    await user.click(within(form()).getByRole("radio", { name: "回答" }));
    await user.type(within(form()).getByLabelText("回答內容（必填）"), "用新結構");
    // 換筆：另一筆是空白的；換回來草稿還在
    await user.click(screen.getByRole("button", { name: "查看裁定 第二個問題" }));
    expect(within(form()).getByRole("radio", { name: "回答" })).not.toBeChecked();
    await user.click(within(form()).getByRole("radio", { name: "延後" }));
    await user.click(screen.getByRole("button", { name: "查看裁定 要保留舊的目錄結構嗎？" }));
    expect(within(form()).getByRole("radio", { name: "回答" })).toBeChecked();
    expect(within(form()).getByLabelText("回答內容（必填）")).toHaveValue("用新結構");

    // 換到別頁（裁定頁卸載）再回來：兩筆的草稿都還在
    first.unmount();
    const second = renderCenter(client);
    await screen.findByRole("article", { name: "裁定詳情" });
    expect(within(form()).getByLabelText("回答內容（必填）")).toHaveValue("用新結構");
    await user.click(screen.getByRole("button", { name: "查看裁定 第二個問題" }));
    expect(within(form()).getByRole("radio", { name: "延後" })).toBeChecked();

    // 送出成功（延後後仍待裁定、處理區還在）：這筆的草稿清掉，換頁再回來也是空白；沒送出的那筆不受影響
    await user.click(screen.getByRole("button", { name: "送出裁定" }));
    expect(await within(await screen.findByRole("region", { name: "已延後（1）" })).findByRole("button", { name: "查看裁定 第二個問題" })).toBeInTheDocument();
    await waitFor(() => expect(within(form()).getByRole("radio", { name: "延後" })).not.toBeChecked());
    second.unmount();
    renderCenter(client);
    await screen.findByRole("article", { name: "裁定詳情" });
    expect(within(form()).getByLabelText("回答內容（必填）")).toHaveValue("用新結構");
    await user.click(screen.getByRole("button", { name: "查看裁定 第二個問題" }));
    expect(within(form()).getByRole("radio", { name: "延後" })).not.toBeChecked();
  });

  it("目標版本已變更時顯示清楚說明，重新載入後標為已失效、不能再回答", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    const client = fakeClient([decisionView(), decisionView({ id: "dec_2", question: "第二個問題" })], { supersedeOnSubmit: ["dec_1"] });
    renderCenter(client, onChange);

    await user.click(await screen.findByRole("radio", { name: "接受方案" }));
    await user.click(within(form()).getByRole("radio", { name: "保留（建議）" }));
    await user.click(screen.getByRole("button", { name: "送出裁定" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("內容或目標版本已改變，這筆已失效，不能再回答。");
    expect(client.listDecisions).toHaveBeenCalledTimes(2);
    expect(onChange).not.toHaveBeenCalled();
    // 仍停在這筆（已移到已結束），顯示已失效，沒有任何處理
    expect(within(detail()).getByRole("heading", { name: "要保留舊的目錄結構嗎？" })).toBeInTheDocument();
    expect(within(detail()).getByRole("heading", { name: "已失效" })).toBeInTheDocument();
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

describe("裁定中心：允許這一次與修改 Workspace 授權", () => {
  it("權限請求把「允許這一次」與「修改 Workspace 授權」分成兩個區塊，前者列出綁定的單一操作", async () => {
    renderCenter(fakeClient([permissionDecision()]));

    const once = await screen.findByRole("region", { name: "允許這一次" });
    expect(within(once).getByText("Bash:git push")).toBeInTheDocument();
    expect(within(once).getByText("origin main")).toBeInTheDocument();
    expect(within(once).getByText('flags：["--force"]')).toBeInTheDocument();
    // 要求雜湊以 textFormat 的 shortHash 縮短（前 12 字 + …）
    expect(within(once).getByText(`${HASH.slice(0, 12)}…`)).toBeInTheDocument();
    // 要求雜湊是技術資訊，收在預設收合的「技術細節」
    expect(within(once).getByText(`${HASH.slice(0, 12)}…`).closest("details")).not.toHaveAttribute("open");
    expect(within(once).queryByText(HASH)).not.toBeInTheDocument();
    expect(within(once).getByText("report.md@2,notes.md@1")).toBeInTheDocument();
    expect(within(once).getByText(/只允許這個 Task 的這一個操作，用過一次就失效，不會修改 Workspace 授權/)).toBeInTheDocument();
    // 「不會修改 Workspace 授權」帶有授權意涵，直接顯示，不收在「?」說明裡
    expectShownDirectly(within(once).getByText(/只允許這個 Task 的這一個操作，用過一次就失效，不會修改 Workspace 授權/));
    expect(within(once).queryByRole("button", { name: "允許這一次的說明" })).not.toBeInTheDocument();
    const modify = screen.getByRole("region", { name: "修改 Workspace 授權" });
    expect(within(modify).getByRole("button", { name: "修改 Workspace 授權" })).toBeInTheDocument();
  });

  it("權限提示直接是三個處理方式：允許這一次、拒絕、延後（沒有「接受方案 › 選項」，也沒有另一條「駁回」）", async () => {
    const user = userEvent.setup();
    const client = fakeClient([permissionDecision()]);
    renderCenter(client);

    await screen.findByRole("article", { name: "裁定詳情" });
    const actions = within(form()).getByRole("group", { name: "處理方式" });
    expect(within(actions).getAllByRole("radio").map((radio) => radio.closest("label")!.textContent)).toEqual([
      "允許這一次",
      "拒絕",
      "延後",
    ]);
    // 選了就能送出，不必再選選項
    expect(within(form()).queryByRole("group", { name: "選項" })).not.toBeInTheDocument();
    await user.click(within(actions).getByRole("radio", { name: "允許這一次" }));
    expect(within(form()).getByLabelText("備註（選填）")).toBeInTheDocument();
    await user.click(within(actions).getByRole("radio", { name: "拒絕" }));
    await user.type(within(form()).getByLabelText("拒絕理由（選填）"), "不要推到 main");
    await user.click(screen.getByRole("button", { name: "送出裁定" }));

    // 拒絕送 accept＋deny（後端的「拒絕」選項；與建議一致時不算推翻建議）
    expect(client.answerDecision).toHaveBeenCalledWith("ws_1", "dec_perm", {
      action: "accept",
      option_id: "deny",
      note: "不要推到 main",
      expected_revision: 1,
    });
    const outcome = await screen.findByRole("region", { name: "裁定結果" });
    expect(within(outcome).getByText("處理方式：拒絕")).toBeInTheDocument();
    expect(within(outcome).getByText("拒絕理由：不要推到 main")).toBeInTheDocument();
    expect(within(outcome).queryByText(/接受方案|選擇：/)).not.toBeInTheDocument();
  });

  it("權限提示選「允許這一次」送 accept＋allow_once；「延後」照常選時數", async () => {
    const user = userEvent.setup();
    const client = fakeClient([permissionDecision(), permissionDecision({ id: "dec_perm2", question: "要允許刪除暫存檔嗎？" })]);
    renderCenter(client);

    await screen.findByRole("article", { name: "裁定詳情" });
    await user.click(within(form()).getByRole("radio", { name: "允許這一次" }));
    await user.click(screen.getByRole("button", { name: "送出裁定" }));
    expect(client.answerDecision).toHaveBeenLastCalledWith("ws_1", "dec_perm", { action: "accept", option_id: "allow_once", expected_revision: 1 });
    expect(await within(await screen.findByRole("region", { name: "裁定結果" })).findByText("處理方式：允許這一次")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "查看裁定 要允許刪除暫存檔嗎？" }));
    await user.click(within(form()).getByRole("radio", { name: "延後" }));
    await user.selectOptions(within(form()).getByLabelText("延後多久"), "4 小時");
    await user.click(screen.getByRole("button", { name: "送出裁定" }));
    expect(client.answerDecision).toHaveBeenLastCalledWith("ws_1", "dec_perm2", { action: "defer", defer_minutes: 240, expected_revision: 1 });
  });

  it("批次中的權限提示也是三個處理方式，「採用建議」選到「拒絕」", async () => {
    const user = userEvent.setup();
    const client = fakeClient([permissionDecision({ recommendation: "deny" })]);
    renderCenter(client);

    await user.click(await screen.findByRole("checkbox", { name: "批次選取 要允許推送到 origin main 嗎？" }));
    const item = within(screen.getByRole("form", { name: "批次處理" })).getByRole("group", { name: "要允許推送到 origin main 嗎？" });
    expect(within(item).getAllByRole("radio").map((radio) => radio.closest("label")!.textContent)).toEqual(["允許這一次", "拒絕", "延後"]);
    await user.click(within(item).getByRole("button", { name: "採用建議" }));
    expect(within(item).getByRole("radio", { name: "拒絕" })).toBeChecked();
    await user.click(screen.getByRole("button", { name: "送出批次（1 項）" }));

    expect(client.batchDecisions).toHaveBeenCalledWith([
      { workspace_id: "ws_1", request_id: "dec_perm", action: "accept", expected_revision: 1, option_id: "deny" },
    ]);
  });

  it("沒有成果時成果版本顯示「尚無成果」，沒有目標資源與重要參數時標示「無」", async () => {
    renderCenter(
      fakeClient([
        permissionDecision({
          permission: permissionOf({
            binding: { operation: "Write", target: null, params: {}, request_hash: HASH, artifact_version: "" },
          }),
        }),
      ]),
    );

    const once = await screen.findByRole("region", { name: "允許這一次" });
    expect(within(once).getByText("尚無成果")).toBeInTheDocument();
    expect(within(once).getAllByText("無")).toHaveLength(2);
  });

  it("修改 Workspace 授權：檢視新增的能力、影響與風險，承認擴大授權後才能確認，且不會回答這個請求", async () => {
    const user = userEvent.setup();
    const client = fakeClient([permissionDecision()]);
    renderCenter(client);
    expect(client.getAuthorization).not.toHaveBeenCalled();

    await user.click(await screen.findByRole("button", { name: "修改 Workspace 授權" }));

    const modify = screen.getByRole("region", { name: "修改 Workspace 授權" });
    expect(await within(modify).findByText("寫入遠端：C:\\work\\notes")).toBeInTheDocument();
    const proposed: Grant[] = [...currentRevision.grants, { capability: "remote_write", targets: [workspace.path], restrictions: [] }];
    expect(client.summarizeAuthorization).toHaveBeenCalledWith("ws_1", proposed);
    expect(within(modify).getByText("Agent 之後在這個 Workspace 的所有 Task 都能寫入遠端，不必再逐次詢問。")).toBeInTheDocument();
    expect(within(modify).getByText("推送會改變遠端儲存庫，其他人會看到。")).toBeInTheDocument();
    const confirm = within(modify).getByRole("button", { name: "確認修改授權" });
    expect(confirm).toBeDisabled();

    await user.click(within(modify).getByRole("checkbox", { name: "我了解這會擴大 Workspace 授權" }));
    expect(confirm).toBeDisabled();
    await user.click(within(modify).getByRole("checkbox", { name: "我了解並接受上述風險" }));
    await user.click(confirm);

    expect(await within(modify).findByRole("status")).toHaveTextContent("授權已更新為第 4 版；這個裁定請求仍待回答");
    expect(client.confirmAuthorization).toHaveBeenCalledWith("ws_1", {
      templateId: "research_docs",
      grants: proposed,
      acknowledgedRisk: true,
      acknowledgedExpansion: true,
      expectedRevision: 3,
    });
    // 修改授權不會回答這個請求，選項仍可作答
    expect(client.answerDecision).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "送出裁定" })).toBeInTheDocument();
  });

  it("無法歸類的操作不能透過修改授權放行", async () => {
    const user = userEvent.setup();
    const client = fakeClient([permissionDecision({ permission: permissionOf({ capability: null, capability_label: null }) })]);
    renderCenter(client);

    await user.click(await screen.findByRole("button", { name: "修改 Workspace 授權" }));

    const modify = screen.getByRole("region", { name: "修改 Workspace 授權" });
    expect(await within(modify).findByText("無法歸類的操作不能透過修改授權放行，請在處理方式選「允許這一次」或「拒絕」")).toBeInTheDocument();
    expect(client.summarizeAuthorization).not.toHaveBeenCalled();
    expect(within(modify).queryByRole("button", { name: "確認修改授權" })).not.toBeInTheDocument();
  });

  it("能力已在目前授權中時，請使用者到授權頁調整目標或限制", async () => {
    const user = userEvent.setup();
    const client = fakeClient([permissionDecision()], {
      revision: { ...currentRevision, grants: [{ capability: "remote_write", targets: ["origin dev"], restrictions: [] }] },
    });
    renderCenter(client);

    await user.click(await screen.findByRole("button", { name: "修改 Workspace 授權" }));

    const modify = screen.getByRole("region", { name: "修改 Workspace 授權" });
    expect(await within(modify).findByText("這個能力已授權但目標或限制不涵蓋這次操作，請到授權頁調整")).toBeInTheDocument();
    expect(client.summarizeAuthorization).not.toHaveBeenCalled();
  });

  it("同一操作先前已放行過一次而結果未知時，醒目提醒先查證", async () => {
    const user = userEvent.setup();
    renderCenter(
      fakeClient([
        permissionDecision({
          options: [
            { id: "allow_once", label: "已查證，允許再執行一次", tradeoff: null },
            { id: "deny", label: "拒絕（不要重送）", tradeoff: null },
          ],
          recommendation: "deny",
          permission: permissionOf({ prior_use: { request_id: "dec_old", run_id: "run_7", used_at: CHECKED_AT } }),
        }),
      ]),
    );

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "這個操作先前已放行過一次（2023/11/15 06:13），結果未知。請先確認是否已生效，再決定要不要再允許一次。",
    );
    // 先前放行的 Run id 收在預設收合的「技術細節」，不在警示文字裡
    expect(screen.getByRole("alert")).not.toHaveTextContent("run_7");
    expectInTechDetails(within(detail()).getByText("run_7"));
    // 建議拒絕：選項與取捨列出「建議」；「採用建議」選到處理方式「拒絕」
    const deny = within(detail()).getByText("拒絕（不要重送）").closest("li")!;
    expect(within(deny).getByText("建議")).toBeInTheDocument();
    await user.click(within(deny).getByRole("button", { name: "採用建議" }));
    expect(within(form()).getByRole("radio", { name: "拒絕" })).toBeChecked();
  });

  it("先前的單次批准因綁定不符而不能沿用時說明原因", async () => {
    renderCenter(
      fakeClient([
        permissionDecision({
          permission: permissionOf({ rejected_approval: { request_id: "dec_old", reason: "target_changed" } }),
        }),
      ]),
    );

    expect(await screen.findByText("先前的單次批准不能沿用：目標資源不同")).toBeInTheDocument();
    // 識別碼不在主要文字裡，收在預設收合的「技術細節」（和要求雜湊同一處）
    expect(screen.getByText("先前的單次批准不能沿用：目標資源不同")).not.toHaveTextContent("dec_old");
    const once = screen.getByRole("region", { name: "允許這一次" });
    expectInTechDetails(within(once).getByText("dec_old"));
    expect(within(once).getAllByText("技術細節")).toHaveLength(1);
  });

  it("已回答的權限請求顯示批准已被哪個 Run 使用而失效，不再提供修改授權", async () => {
    renderCenter(
      fakeClient([
        permissionDecision({
          status: "answered",
          blocking: [],
          allowed_actions: [],
          decision: {
            action: "accept",
            option_id: "allow_once",
            option_label: "允許本次操作",
            note: null,
            decided_by: "desktop",
            decided_at: CHECKED_AT,
            batch_id: null,
          },
          permission: permissionOf({ approval_use: { request_id: "dec_perm", run_id: "run_9", used_at: CHECKED_AT + 60_000 } }),
        }),
      ]),
    );

    const outcome = await screen.findByRole("region", { name: "裁定結果" });
    expect(within(outcome).getByText("這個批准已於 2023/11/15 06:14 使用過，已失效")).toBeInTheDocument();
    // 使用批准的 Run id 收在預設收合的「技術細節」，不在主要文字裡
    expect(within(outcome).getByText("這個批准已於 2023/11/15 06:14 使用過，已失效")).not.toHaveTextContent("run_9");
    expectInTechDetails(within(outcome).getByText("run_9"));
    expect(screen.queryByRole("button", { name: "修改 Workspace 授權" })).not.toBeInTheDocument();
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
    expect(within(related).getByText("（待裁定）")).toBeInTheDocument();
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
    expect(screen.getByRole("region", { name: "可處理（1）" })).toBeInTheDocument();
    expect(screen.getByRole("region", { name: "已結束（2）" })).toBeInTheDocument();
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
    expect(within(results).getByText("第二個問題：失敗（內容或目標版本已改變，這筆已失效，不能再回答。）")).toBeInTheDocument();
    expect(screen.getByRole("region", { name: "已結束（2）" })).toBeInTheDocument();
  });

  it("批次面板說明填妥了幾項，並逐項標出還差什麼", async () => {
    const user = userEvent.setup();
    renderCenter(fakeClient(twoPending()));
    await screen.findByRole("article", { name: "裁定詳情" });

    await user.click(screen.getByRole("checkbox", { name: "批次選取 要保留舊的目錄結構嗎？" }));
    await user.click(screen.getByRole("checkbox", { name: "批次選取 第二個問題" }));
    const panel = screen.getByRole("form", { name: "批次處理" });
    const first = within(panel).getByRole("group", { name: "要保留舊的目錄結構嗎？" });
    const second = within(panel).getByRole("group", { name: "第二個問題" });
    expect(within(panel).getByText("已填妥 0／2 項")).toBeInTheDocument();
    expect(within(first).getByText("還沒選處理方式")).toBeInTheDocument();

    await user.click(within(first).getByRole("radio", { name: "接受方案" }));
    expect(within(first).getByText("還沒選選項")).toBeInTheDocument();
    await user.click(within(first).getByRole("radio", { name: "保留（建議）" }));
    expect(within(first).getByText("已填妥")).toBeInTheDocument();
    await user.click(within(second).getByRole("radio", { name: "要求調查" }));
    expect(within(second).getByText("還沒填寫要調查的事")).toBeInTheDocument();
    expect(within(panel).getByText("已填妥 1／2 項")).toBeInTheDocument();
    expect(within(panel).getByText("每一項都選好處理方式、填妥必填欄位後才能送出")).toBeInTheDocument();

    await user.type(within(second).getByLabelText("要調查的事（必填）"), "看引用");
    expect(within(panel).getByText("已填妥 2／2 項")).toBeInTheDocument();
    expect(within(panel).queryByText("每一項都選好處理方式、填妥必填欄位後才能送出")).not.toBeInTheDocument();
    expect(within(panel).getByRole("button", { name: "送出批次（2 項）" })).toBeEnabled();
  });

  /**
   * jsdom 沒有版面：把右欄的 scrollTop 換成可讀寫的值，並讓詳情的上緣位在右欄上緣下方 400px
   * （就像上方有批次面板或批次結果），以便檢查換筆時有沒有捲動右欄。
   */
  const withPaneLayout = async (run: (pane: HTMLElement) => Promise<void>) => {
    const rect = vi.spyOn(Element.prototype, "getBoundingClientRect").mockImplementation(function (this: Element) {
      const top = this.classList.contains("decision-detail") ? 400 : 0;
      return { top, bottom: top, left: 0, right: 0, width: 0, height: 0, x: 0, y: top, toJSON: () => ({}) } as DOMRect;
    });
    try {
      const pane = (await screen.findByRole("article", { name: "裁定詳情" })).parentElement!;
      let scrollTop = 0;
      Object.defineProperty(pane, "scrollTop", {
        configurable: true,
        get: () => scrollTop,
        set: (value: number) => {
          scrollTop = value;
        },
      });
      await run(pane);
    } finally {
      rect.mockRestore();
    }
  };

  it("批次送出後右欄捲回頂端、焦點移到批次結果標題；原本選的那筆結束而換筆時不會把結果捲走", async () => {
    const user = userEvent.setup();
    renderCenter(fakeClient(twoPending()));

    await withPaneLayout(async (pane) => {
      await user.click(screen.getByRole("checkbox", { name: "批次選取 要保留舊的目錄結構嗎？" }));
      const panel = screen.getByRole("form", { name: "批次處理" });
      await user.click(within(within(panel).getByRole("group", { name: "要保留舊的目錄結構嗎？" })).getByRole("radio", { name: "駁回" }));
      // 使用者在右欄裡往下捲到送出鈕
      pane.scrollTop = 250;
      await user.click(within(panel).getByRole("button", { name: "送出批次（1 項）" }));

      const results = await screen.findByRole("region", { name: "批次結果" });
      // 預設選的第一筆已結束，詳情換成下一筆可處理的（第二個問題），但右欄仍停在頂端
      await waitFor(() => expect(within(detail()).getByRole("heading", { name: "第二個問題" })).toBeInTheDocument());
      expect(pane.scrollTop).toBe(0);
      expect(pane.firstElementChild).toBe(results);
      expect(within(results).getByRole("heading", { name: /^批次結果/ })).toHaveFocus();
    });
  });

  it("使用者從清單換筆時，右欄捲到詳情開頭（捲過上方的批次面板）", async () => {
    const user = userEvent.setup();
    renderCenter(fakeClient(twoPending()));

    await withPaneLayout(async (pane) => {
      await user.click(screen.getByRole("checkbox", { name: "批次選取 第二個問題" }));
      expect(pane.scrollTop).toBe(0);

      await user.click(screen.getByRole("button", { name: "查看裁定 第三個問題" }));

      expect(within(detail()).getByRole("heading", { name: "第三個問題" })).toBeInTheDocument();
      expect(pane.scrollTop).toBe(400);
    });
  });

  it("批次處理時詳情只讀：不顯示詳情的處理區與「採用建議」，畫面上只有批次的送出鈕；取消批次後焦點移到詳情標題", async () => {
    const user = userEvent.setup();
    renderCenter(fakeClient(twoPending()));
    await screen.findByRole("article", { name: "裁定詳情" });
    expect(within(detail()).getByRole("button", { name: "採用建議" })).toBeInTheDocument();

    // 選取的第一筆不在批次裡
    await user.click(screen.getByRole("checkbox", { name: "批次選取 第二個問題" }));
    expect(within(detail()).queryByRole("radio")).not.toBeInTheDocument();
    expect(within(detail()).queryByRole("button", { name: "送出裁定" })).not.toBeInTheDocument();
    expect(within(detail()).queryByRole("button", { name: "採用建議" })).not.toBeInTheDocument();
    expect(within(detail()).getByText(/^批次處理進行中，送出或取消批次後才能在這裡單筆處理；也可以在清單勾選/)).toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: /^送出/ }).map((button) => button.textContent)).toEqual(["送出批次（1 項）"]);

    await user.click(screen.getByRole("checkbox", { name: "批次選取 要保留舊的目錄結構嗎？" }));
    expect(within(detail()).getByText("這筆已加入批次，請在上方的「批次處理」選擇處理方式並送出。")).toBeInTheDocument();

    // 取消鈕隨面板消失：焦點移到詳情標題，詳情的處理區回來
    await user.click(screen.getByRole("button", { name: "取消批次" }));
    expect(within(detail()).getByRole("button", { name: "送出裁定" })).toBeInTheDocument();
    expect(within(detail()).getByRole("heading", { level: 3, name: "要保留舊的目錄結構嗎？" })).toHaveFocus();
  });

  it("勾進批次時沿用詳情已填的草稿；批次每一項各自有「採用建議」，只套用到那一項", async () => {
    const user = userEvent.setup();
    const client = fakeClient(twoPending());
    renderCenter(client);
    await screen.findByRole("article", { name: "裁定詳情" });

    // 詳情先按「採用建議」並寫備註，再把這筆勾進批次
    await user.click(within(detail()).getByRole("button", { name: "採用建議" }));
    await user.type(within(form()).getByLabelText("備註（選填）"), "照建議");
    await user.click(screen.getByRole("checkbox", { name: "批次選取 要保留舊的目錄結構嗎？" }));
    const panel = screen.getByRole("form", { name: "批次處理" });
    const first = within(panel).getByRole("group", { name: "要保留舊的目錄結構嗎？" });
    expect(within(first).getByText("已填妥")).toBeInTheDocument();
    expect(within(first).getByRole("radio", { name: "接受方案" })).toBeChecked();
    expect(within(first).getByRole("radio", { name: "保留（建議）" })).toBeChecked();
    expect(within(first).getByLabelText("備註（選填）")).toHaveValue("照建議");

    // 沒在詳情填過的一筆從空白開始；它自己的「採用建議」不影響其他項
    await user.click(screen.getByRole("checkbox", { name: "批次選取 第二個問題" }));
    const second = within(panel).getByRole("group", { name: "第二個問題" });
    expect(within(second).getByText("還沒選處理方式")).toBeInTheDocument();
    await user.click(within(second).getByRole("button", { name: "採用建議" }));
    expect(within(second).getByRole("radio", { name: "保留（建議）" })).toBeChecked();
    expect(within(second).getByLabelText("備註（選填）")).toHaveValue("");
    expect(within(panel).getByText("已填妥 2／2 項")).toBeInTheDocument();
    await user.click(within(panel).getByRole("button", { name: "送出批次（2 項）" }));

    expect(client.batchDecisions).toHaveBeenCalledWith([
      { workspace_id: "ws_1", request_id: "dec_1", action: "accept", expected_revision: 3, option_id: "keep", note: "照建議" },
      { workspace_id: "ws_1", request_id: "dec_2", action: "accept", expected_revision: 7, option_id: "keep" },
    ]);
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

describe("裁定中心：前往其他頁", () => {
  const authorityDecision = () =>
    decisionView({
      kind: "authority",
      authority: { matter: "change_acceptance", matter_label: "修改完成條件或驗收標準", summary: "把覆蓋率門檻從 80% 降到 60%" },
    });

  it("詳情的 Task 是前往工作頁的連結；決策權限攔截可以前往決策權限頁調整", async () => {
    const user = userEvent.setup();
    const onOpenPage = vi.fn();
    render(<DecisionCenter client={fakeClient([authorityDecision()])} workspace={workspace} timeZone="Asia/Taipei" onOpenPage={onOpenPage} />);

    await user.click(await within(await screen.findByRole("article", { name: "裁定詳情" })).findByRole("button", { name: "整理文件" }));
    expect(onOpenPage).toHaveBeenCalledWith("tasks");

    await user.click(within(screen.getByRole("region", { name: "決策權限攔截" })).getByRole("button", { name: "調整決策權限" }));
    expect(onOpenPage).toHaveBeenLastCalledWith("authority");
  });

  it("沒有傳入 onOpenPage 時 Task 是純文字，也沒有「調整決策權限」", async () => {
    renderCenter(fakeClient([authorityDecision()]));

    await screen.findByRole("article", { name: "裁定詳情" });
    expect(within(detail()).getByText("Task：整理文件")).toBeInTheDocument();
    expect(within(detail()).queryByRole("button", { name: "整理文件" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "調整決策權限" })).not.toBeInTheDocument();
  });
});

describe("裁定中心：從別頁「前往裁定」", () => {
  const items = () => [
    decisionView(),
    decisionView({
      id: "dec_2",
      question: "第二個問題",
      task_id: "task_2",
      task_title: "寫測試",
      blocking: [{ task_id: "task_2", task_title: "寫測試", task_status: "waiting_decision" }],
    }),
    // 由另一個 Task 提出、擋住 task_3
    decisionView({
      id: "dec_3",
      question: "第三個問題",
      task_id: "task_9",
      task_title: "其他",
      blocking: [{ task_id: "task_3", task_title: "被擋的 Task", task_status: "waiting_decision" }],
    }),
    answeredDecision({ task_id: "task_7" }),
  ];

  /** jsdom 沒有 scrollIntoView：記下被捲進可視範圍的元素。 */
  const withScrollSpy = async (run: (scrolled: Element[]) => Promise<void>) => {
    const scrolled: Element[] = [];
    Object.defineProperty(Element.prototype, "scrollIntoView", {
      configurable: true,
      value(this: Element) {
        scrolled.push(this);
      },
    });
    try {
      await run(scrolled);
    } finally {
      delete (Element.prototype as { scrollIntoView?: unknown }).scrollIntoView;
    }
  };

  const renderWithTarget = (client: DecisionClient & AuthorizationClient, target: { taskId?: string; decisionId?: string; nonce: number }) =>
    render(<DecisionCenter client={client} workspace={workspace} timeZone="Asia/Taipei" target={target} />);

  const listItem = (question: string) => screen.getByRole("button", { name: `查看裁定 ${question}` });

  it("decisionId：選中那一筆並把清單項目捲進可視範圍；nonce 改變時再定位一次，相同 nonce 不重複", async () => {
    const user = userEvent.setup();
    const client = fakeClient(items());
    await withScrollSpy(async (scrolled) => {
      const { rerender } = renderWithTarget(client, { decisionId: "dec_2", nonce: 1 });

      expect(await within(await screen.findByRole("article", { name: "裁定詳情" })).findByRole("heading", { level: 3, name: "第二個問題" })).toBeInTheDocument();
      expect(listItem("第二個問題")).toHaveAttribute("aria-current", "true");
      expect(scrolled).toEqual([listItem("第二個問題").closest("li")]);

      // 使用者換到別筆；同一個 nonce 再傳一次不會被拉回去
      await user.click(listItem("要保留舊的目錄結構嗎？"));
      rerender(<DecisionCenter client={client} workspace={workspace} timeZone="Asia/Taipei" target={{ decisionId: "dec_2", nonce: 1 }} />);
      expect(within(detail()).getByRole("heading", { level: 3, name: "要保留舊的目錄結構嗎？" })).toBeInTheDocument();

      // 同一個對象再要求一次（新的 nonce）：重新選中並捲動
      rerender(<DecisionCenter client={client} workspace={workspace} timeZone="Asia/Taipei" target={{ decisionId: "dec_2", nonce: 2 }} />);
      expect(within(detail()).getByRole("heading", { level: 3, name: "第二個問題" })).toBeInTheDocument();
      expect(scrolled).toHaveLength(2);
    });
  });

  it("只有 taskId：選中這個 Task 的第一筆待裁定（Task 提出的，或阻擋範圍含這個 Task）；只有已結束的時候不改選取", async () => {
    await withScrollSpy(async () => {
      const { unmount } = renderWithTarget(fakeClient(items()), { taskId: "task_2", nonce: 1 });
      expect(await within(await screen.findByRole("article", { name: "裁定詳情" })).findByRole("heading", { level: 3, name: "第二個問題" })).toBeInTheDocument();
      unmount();

      const blocked = renderWithTarget(fakeClient(items()), { taskId: "task_3", nonce: 1 });
      expect(await within(await screen.findByRole("article", { name: "裁定詳情" })).findByRole("heading", { level: 3, name: "第三個問題" })).toBeInTheDocument();
      blocked.unmount();

      renderWithTarget(fakeClient(items()), { taskId: "task_7", nonce: 1 });
      await screen.findByRole("article", { name: "裁定詳情" });
      expect(within(detail()).getByRole("heading", { level: 3, name: "要保留舊的目錄結構嗎？" })).toBeInTheDocument();
    });
  });
});

describe("決策權限頁", () => {
  /** 決策權限是獨立的一頁（App 直接渲染 DecisionAuthorityPanel），不在裁定中心裡。 */
  const renderAuthority = (client: DecisionClient & AuthorizationClient) =>
    render(<DecisionAuthorityPanel client={client} workspace={workspace} />);
  /** 「目前生效」欄的層級文字（同一列的下拉選項也有同樣的文字）。 */
  const EFFECTIVE = ".decision-authority-effective > span";

  it("是完整的一頁：頁面標題「決策權限」與一句用途說明；裁定中心不再顯示權限表", async () => {
    const client = fakeClient([]);
    const { unmount } = renderAuthority(client);

    const panel = await authorityPanel();
    expect(within(panel).getByRole("heading", { level: 2, name: "決策權限" })).toBeInTheDocument();
    expect(panel).toHaveTextContent("決定 Controller 遇到這些事時可以自己處理、必須送裁定，還是只能由你發起。");
    unmount();

    renderCenter(fakeClient([decisionView()]));
    await screen.findByRole("article", { name: "裁定詳情" });
    expect(screen.queryByRole("region", { name: "決策權限" })).not.toBeInTheDocument();
  });

  it("列出事項的內建與目前生效層級；每一列都能選三種層級，沒有鎖定的列", async () => {
    renderAuthority(fakeClient([]));

    const panel = await authorityPanel();
    const row = (label: string) => within(panel).getByText(label).closest("tr")!;
    const setting = (label: string) => within(panel).getByRole("combobox", { name: `Workspace 設定：${label}` });
    expect(within(row("拆分、合併、排序 Task")).getAllByText("Controller 可自主處理")).toHaveLength(2);
    expect(setting("拆分、合併、排序 Task")).toHaveValue("autonomous");
    // 和內建不同的列：目前生效旁有「已收窄」徽章，整列標上收窄
    expect(within(row("重試 Task")).getByText("必須送裁定", { selector: EFFECTIVE })).toBeInTheDocument();
    expect(within(row("重試 Task")).getByText("已收窄")).toHaveClass("badge");
    expect(row("重試 Task")).toHaveClass("is-narrowed");
    expect(within(row("拆分、合併、排序 Task")).queryByText(/已收窄|已放寬/)).not.toBeInTheDocument();
    expect(setting("重試 Task")).toHaveValue("requires_decision");
    expect(setting("修改完成條件或驗收標準")).toHaveValue("requires_decision");
    expect(setting("三次恢復失敗後的重試")).toHaveValue("human_only");
    for (const tr of within(panel).getAllByRole("row").slice(1)) {
      const select = within(tr).getByRole("combobox");
      expect(select).toBeEnabled();
      expect(within(select).getAllByRole("option").map((o) => (o as HTMLOptionElement).value)).toEqual([
        "autonomous",
        "requires_decision",
        "human_only",
      ]);
    }
    expect(within(panel).queryByText("不能放寬")).not.toBeInTheDocument();
    // 選項標出內建層級供參考
    expect(within(setting("修改完成條件或驗收標準")).getByRole("option", { name: "必須送裁定（內建）" })).toBeInTheDocument();
    expect(within(panel).getAllByRole("columnheader")[1]).toHaveTextContent("內建");
    expect(within(row("修改完成條件或驗收標準")).getAllByRole("cell")[1]).toHaveTextContent(/^必須送裁定$/);
    expect(within(row("三次恢復失敗後的重試")).getAllByRole("cell")[1]).toHaveTextContent(/^只能人工發起$/);
    expect(within(panel).getByRole("button", { name: "儲存決策權限" })).toBeDisabled();
  });

  it("收窄後存檔帶 expected_revision，只列出和內建不同的事項", async () => {
    const user = userEvent.setup();
    const client = fakeClient([]);
    renderAuthority(client);
    const panel = await authorityPanel();

    await user.selectOptions(within(panel).getByRole("combobox", { name: "Workspace 設定：拆分、合併、排序 Task" }), "requires_decision");
    // 還沒儲存時說明改了幾項
    expect(within(panel).getByText("已修改 1 項，尚未儲存")).toBeInTheDocument();
    await user.click(within(panel).getByRole("button", { name: "儲存決策權限" }));

    expect(client.updateDecisionAuthority).toHaveBeenCalledWith("ws_1", {
      expected_revision: 1,
      entries: [
        { matter: "split_task", effective: "requires_decision" },
        { matter: "retry_task", effective: "requires_decision" },
      ],
    });
    expect(await within(panel).findByText("已儲存（第 2 版）")).toBeInTheDocument();
    expect(within(panel).queryByText(/尚未儲存/)).not.toBeInTheDocument();
    const split = within(panel).getByText("拆分、合併、排序 Task").closest("tr")!;
    expect(within(split).getByText("必須送裁定", { selector: EFFECTIVE })).toBeInTheDocument();
    expect(within(split).getByText("已收窄")).toBeInTheDocument();
  });

  it("放寬內建必須送裁定與只能人工發起的事項，存檔後顯示已放寬", async () => {
    const user = userEvent.setup();
    const client = fakeClient([]);
    renderAuthority(client);
    const panel = await authorityPanel();

    await user.selectOptions(within(panel).getByRole("combobox", { name: "Workspace 設定：修改完成條件或驗收標準" }), "autonomous");
    await user.selectOptions(within(panel).getByRole("combobox", { name: "Workspace 設定：三次恢復失敗後的重試" }), "requires_decision");
    await user.click(within(panel).getByRole("button", { name: "儲存決策權限" }));

    expect(client.updateDecisionAuthority).toHaveBeenCalledWith("ws_1", {
      expected_revision: 1,
      entries: [
        { matter: "retry_task", effective: "requires_decision" },
        { matter: "change_acceptance", effective: "autonomous" },
        { matter: "retry_after_recovery_failures", effective: "requires_decision" },
      ],
    });
    expect(await within(panel).findByText("已儲存（第 2 版）")).toBeInTheDocument();
    const row = (label: string) => within(panel).getByText(label).closest("tr")!;
    expect(within(row("修改完成條件或驗收標準")).getByText("Controller 可自主處理", { selector: EFFECTIVE })).toBeInTheDocument();
    expect(within(row("修改完成條件或驗收標準")).getByText("已放寬")).toHaveClass("badge", "badge-warning");
    expect(row("修改完成條件或驗收標準")).toHaveClass("is-widened");
    expect(within(row("三次恢復失敗後的重試")).getAllByRole("cell")[2]).toHaveTextContent(/^必須送裁定已放寬$/);
  });

  it("改回內建層級就回到內建值（不列在 entries 裡）", async () => {
    const user = userEvent.setup();
    const client = fakeClient([]);
    renderAuthority(client);
    const panel = await authorityPanel();

    await user.selectOptions(within(panel).getByRole("combobox", { name: "Workspace 設定：重試 Task" }), "autonomous");
    await user.click(within(panel).getByRole("button", { name: "儲存決策權限" }));

    expect(client.updateDecisionAuthority).toHaveBeenCalledWith("ws_1", { expected_revision: 1, entries: [] });
  });

  it("版本不符時說明並重新載入", async () => {
    const user = userEvent.setup();
    const client = fakeClient([], { authority: authorityTable({ revision: 3 }) });
    client.updateDecisionAuthority = vi.fn(async () => {
      throw conflict("authority_revision_mismatch", "權限表版本不符");
    });
    renderAuthority(client);
    const panel = await authorityPanel();

    await user.selectOptions(within(panel).getByRole("combobox", { name: "Workspace 設定：拆分、合併、排序 Task" }), "requires_decision");
    await user.click(within(panel).getByRole("button", { name: "儲存決策權限" }));

    expect(await within(panel).findByRole("alert")).toHaveTextContent("決策權限已被修改，已重新載入，請重新確認。");
    expect(client.decisionAuthority).toHaveBeenCalledTimes(2);
  });

  it("讀不到權限表時在頁面中顯示原因，頁面標題仍在", async () => {
    const client = fakeClient([decisionView()]);
    client.decisionAuthority = vi.fn(async () => {
      throw new Error("Runtime 回應 503");
    });
    renderAuthority(client);

    const panel = await authorityPanel();
    const alert = await within(panel).findByRole("alert");
    expect(alert).toHaveClass("notice", "notice-danger");
    expect(within(alert).getByText("無法取得決策權限")).toHaveClass("notice-title");
    expect(alert).toHaveTextContent("Runtime 回應 503");
    expect(within(panel).getByRole("heading", { level: 2, name: "決策權限" })).toBeInTheDocument();
  });
});

describe("裁定中心：長文字的呈現", () => {
  const SHA = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
  const LONG_QUESTION = `${"整合佇列中第 17 個項目的合併提交與目標分支的最新提交衝突，".repeat(3)}要怎麼處理？`;

  it("頁面標題只寫頁名，不重複 Workspace 名稱；頁面 region 以標題命名", async () => {
    renderCenter(fakeClient([decisionView()]));

    expect(await screen.findByRole("heading", { level: 2, name: "裁定" })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: /筆記/ })).not.toBeInTheDocument();
    expect(screen.getByRole("region", { name: "裁定" })).toContainElement(screen.getByRole("heading", { level: 2, name: "裁定" }));
  });

  it("緊急程度高：待裁定的用琥珀色（要你處理），已結束的改用中性", async () => {
    renderCenter(fakeClient([decisionView({ urgency: "high" }), answeredDecision({ urgency: "high" })]));

    const pending = await screen.findByRole("button", { name: "查看裁定 要保留舊的目錄結構嗎？" });
    expect(within(pending).getByText("緊急程度：高")).toHaveClass("badge-warning");
    // 詳情與清單同一個色調
    expect(within(detail()).getByText("緊急程度：高")).toHaveClass("badge-warning");
    const answered = screen.getByRole("button", { name: "查看裁定 要不要升級相依套件？" });
    expect(within(answered).getByText("緊急程度：高")).toHaveClass("badge-plain");
    expect(within(answered).getByText("緊急程度：高")).not.toHaveClass("badge-warning");
  });

  it("清單項只放摘要：問題限 3 行、Task 名限 1 行（title 放全文），種類與緊急程度是同一列的徽章", async () => {
    renderCenter(fakeClient([decisionView({ question: LONG_QUESTION, urgency: "high", overdue: true })]));

    const button = await screen.findByRole("button", { name: `查看裁定 ${LONG_QUESTION}` });
    const question = within(button).getByText(LONG_QUESTION);
    expect(question).toHaveClass("clamp-3");
    expect(question).toHaveAttribute("title", LONG_QUESTION);
    const task = within(button).getByText("整理文件");
    expect(task).toHaveClass("clamp-1");
    expect(task).toHaveAttribute("title", "整理文件");
    const badges = within(button).getByText("緊急程度：高").closest(".badge-group")!;
    expect(within(badges as HTMLElement).getByText("Agent 請求")).toHaveClass("badge");
    // 緊急程度高要你處理：琥珀色，與總覽的「高緊急」同色
    expect(within(badges as HTMLElement).getByText("緊急程度：高")).toHaveClass("badge", "badge-warning");
    expect(within(badges as HTMLElement).getByText("已逾期，尚未裁定")).toHaveClass("badge", "badge-warning");
    // 詳情中問題完整顯示，不截斷
    expect(within(detail()).getByRole("heading", { name: LONG_QUESTION })).not.toHaveClass("clamp-3");
  });

  it("選項、證據與原因中的 SHA-256 以縮短的等寬形式顯示，title 與複製按鈕提供完整值", async () => {
    const user = userEvent.setup();
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    renderCenter(
      fakeClient([
        decisionView({
          reason: `基準報告的 SHA-256 為 ${SHA}，需要確認`,
          options: [{ id: "revert", label: SHA, tradeoff: null }],
          evidence: [`SHA-256 ${SHA}`],
        }),
      ]),
    );

    const article = await screen.findByRole("article", { name: "裁定詳情" });
    expect(within(article).queryByText(SHA)).not.toBeInTheDocument();
    const shorts = within(article).getAllByText(`${SHA.slice(0, 12)}…`);
    // 原因、選項、證據各一處
    expect(shorts).toHaveLength(3);
    for (const short of shorts) {
      expect(short.tagName).toBe("CODE");
      expect(short).toHaveClass("mono");
      expect(short).toHaveAttribute("title", SHA);
    }
    expect(within(article).getByText(/^基準報告的 SHA-256 為/)).toHaveTextContent(`基準報告的 SHA-256 為 ${SHA.slice(0, 12)}…`);
    const copies = within(article).getAllByRole("button", { name: "複製雜湊" });
    expect(copies).toHaveLength(3);
    await user.click(copies[0]);
    expect(writeText).toHaveBeenCalledWith(SHA);
  });

  it("處理方式的選項是雜湊時也縮短（label 內不放按鈕），radio 的名稱仍是完整值", async () => {
    const user = userEvent.setup();
    renderCenter(fakeClient([decisionView({ options: [{ id: "revert", label: SHA, tradeoff: null }], recommendation: null })]));

    await user.click(await screen.findByRole("radio", { name: "接受方案" }));
    const radio = within(form()).getByRole("radio", { name: SHA });
    const label = radio.closest("label")!;
    expect(label).toHaveTextContent(`${SHA.slice(0, 12)}…`);
    expect(within(label).queryByRole("button")).not.toBeInTheDocument();
  });

  it("權限提示的工具名稱以等寬字顯示；工具輸入收在 details 的捲動框", async () => {
    renderCenter(fakeClient([permissionDecision()]));

    const permission = await screen.findByRole("region", { name: "權限提示" });
    expect(within(permission).getByText("Bash")).toHaveClass("mono");
    const input = permission.querySelector("details > pre")!;
    expect(input).toHaveTextContent("git push --force origin main");
  });

  it("要求雜湊在技術細節中以縮短形式顯示並可複製完整值", async () => {
    const user = userEvent.setup();
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    renderCenter(fakeClient([permissionDecision()]));

    const once = await screen.findByRole("region", { name: "允許這一次" });
    const hash = within(once).getByText(`${HASH.slice(0, 12)}…`);
    expect(hash).toHaveClass("mono");
    expect(hash).toHaveAttribute("title", HASH);
    await user.click(within(once).getByRole("button", { name: "複製要求雜湊", hidden: true }));
    expect(writeText).toHaveBeenCalledWith(HASH);
  });

  it("連結與合併的選單選項縮短到 60 字以內", async () => {
    renderCenter(fakeClient([decisionView(), decisionView({ id: "dec_long", question: LONG_QUESTION })]));

    const related = await screen.findByRole("region", { name: "相似問題" });
    const options = within(within(related).getByLabelText("連結另一個請求")).getAllByRole("option");
    expect(options.map((o) => Array.from(o.textContent ?? "").length).every((length) => length <= 60)).toBe(true);
    expect(options[1].textContent).toMatch(/…$/);
  });
});

describe("裁定中心：版面樣式", () => {
  /** 讀 decisions.css（Vitest 會把 CSS 匯入換成空字串，所以以非字面的動態匯入讀檔，同 WorkspaceSidebar.test.tsx）。 */
  const decisionsCss = async () => {
    const fs: { readFileSync(path: string, encoding: "utf8"): string } = await import(/* @vite-ignore */ `node:${"fs"}`);
    const url: { fileURLToPath(url: string): string } = await import(/* @vite-ignore */ `node:${"url"}`);
    const here = import.meta.url;
    return fs.readFileSync(url.fileURLToPath(new URL("./styles/decisions.css", here).href), "utf8");
  };

  /** 掛上 decisions.css，回傳解析後的樣式表；測試結束時移除。 */
  const withStyles = async (run: (sheet: CSSStyleSheet) => Promise<void>) => {
    const style = document.createElement("style");
    style.textContent = await decisionsCss();
    document.head.append(style);
    try {
      await run(style.sheet!);
    } finally {
      style.remove();
    }
  };

  /** 1100px 以下的規則：選擇器 → 樣式。 */
  const narrowRules = (sheet: CSSStyleSheet) => {
    const narrow = Array.from(sheet.cssRules).find(
      (rule): rule is CSSMediaRule => rule instanceof CSSMediaRule && rule.media.mediaText.includes("max-width: 1100px"),
    )!;
    return new Map(
      Array.from(narrow.cssRules)
        .filter((rule): rule is CSSStyleRule => rule instanceof CSSStyleRule)
        .flatMap((rule) => rule.selectorText.split(",").map((selector) => [selector.trim(), rule.style] as const)),
    );
  };

  it("右欄工作區（批次與詳情）在清單很長時保持可見：sticky、高度不超過視窗，內容在欄內自己捲動", async () => {
    await withStyles(async () => {
      const user = userEvent.setup();
      renderCenter(fakeClient([decisionView(), decisionView({ id: "dec_2", question: "第二個問題" })]));
      const pane = (await screen.findByRole("article", { name: "裁定詳情" })).parentElement!;
      expect(pane).toHaveClass("decision-pane");
      const style = getComputedStyle(pane);
      expect(style.position).toBe("sticky");
      expect(style.overflowY).toBe("auto");
      // 勾選進批次時，批次面板出現在同一欄的最上方（清單不被推動）
      await user.click(screen.getByRole("checkbox", { name: "批次選取 第二個問題" }));
      expect(pane.firstElementChild).toBe(screen.getByRole("form", { name: "批次處理" }));
    });
    // jsdom 不解析 calc()，高度上限直接檢查規則文字；新加入的批次項目不被捲動錨點藏起來
    const css = await decisionsCss();
    expect(css).toMatch(/\.decision-pane\s*\{[^}]*max-block-size:\s*calc\(100vh\s*-/);
    expect(css).toMatch(/\.decision-pane\s*\{[^}]*overflow-anchor:\s*none/);
    // 右欄從頁首與總覽下方開始：捲到底後滾輪要交給文件，文件捲動後右欄底部的送出列才進得了視窗
    expect(css).not.toMatch(/\.decision-pane\s*\{[^}]*overscroll-behavior/);
  });

  it("批次的送出列也停在右欄底部（sticky），送出失敗的說明在它上方", async () => {
    await withStyles(async () => {
      const user = userEvent.setup();
      renderCenter(fakeClient([decisionView(), decisionView({ id: "dec_2", question: "第二個問題" })]));
      await user.click(await screen.findByRole("checkbox", { name: "批次選取 第二個問題" }));
      const panel = screen.getByRole("form", { name: "批次處理" });
      const actions = within(panel).getByRole("button", { name: "送出批次（1 項）" }).parentElement!;
      expect(actions).toHaveClass("form-actions");
      expect(getComputedStyle(actions).position).toBe("sticky");
      expect(panel.lastElementChild).toBe(actions);
    });
  });

  it("只有送出列停在右欄底部（sticky），處理區本身隨內容捲動、不蓋住上方內容；焦點捲到送出列上方；處理區與嵌入區都和其他區塊同寬", async () => {
    await withStyles(async () => {
      renderCenter(fakeClient([permissionDecision()]));
      await screen.findByRole("article", { name: "裁定詳情" });
      const style = getComputedStyle(form());
      // 整個處理區 sticky 時，Tab 走過詳情的焦點元素會被捲到它後面（1024×768 時最高佔 60% 的右欄）
      expect(style.position).not.toBe("sticky");
      expect(style.boxSizing).toBe("border-box");
      expect(getComputedStyle(form().querySelector(".form-actions")!).position).toBe("sticky");
      // 有 padding 的嵌入區（權限提示）用 border-box，不比其他區塊寬
      expect(getComputedStyle(within(detail()).getByRole("region", { name: "權限提示" })).boxSizing).toBe("border-box");
    });
    // 右欄保留送出列的高度當 scroll-padding：瀏覽器把焦點元素捲到送出列上方（jsdom 不解析 calc()，直接檢查規則文字）
    const css = await decisionsCss();
    expect(css).toMatch(/\.decision-pane\s*\{[^}]*scroll-padding-bottom:\s*calc\(var\(--control-height\)/);
    // 處理區不再限高、不自己捲動
    expect(css).not.toMatch(/\.decision-form\s*\{[^}]*(max-block-size|overflow-y)/);
  });

  it("1024 寬時清單與右欄仍並排，清單最多佔 34%", async () => {
    await withStyles(async (sheet) => {
      const rules = narrowRules(sheet);
      expect(rules.get(".decision-center .decision-layout")?.getPropertyValue("grid-template-columns")).toMatch(/34%/);
    });
  });

  it("1024 寬的決策權限表每列上下排列：事項佔整列，內建、目前生效、設定在下一列，設定欄寬不變", async () => {
    await withStyles(async (sheet) => {
      const rules = narrowRules(sheet);
      const row = rules.get(".decision-authority .decision-authority-table tr");
      expect(row?.getPropertyValue("display")).toBe("grid");
      expect(row?.getPropertyValue("grid-template-columns")).toMatch(/19em$/);
      expect(rules.get(".decision-authority .decision-authority-table td:first-child")?.getPropertyValue("grid-column")).toBe("1 / -1");
    });
  });
});
