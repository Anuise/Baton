import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { DecisionCenter } from "./DecisionCenter";
import { CHECKED_AT, decisionView } from "./fixtures";
import type {
  AuthorizationClient,
  AuthorizationRevision,
  AuthorizationView,
  ConfirmationSummary,
  ConfirmAuthorizationRequest,
  DecisionClient,
  DecisionPermission,
  DecisionRequestView,
  Grant,
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
    decision: { option_id: "keep", option_label: "保留", note: "先不動", decided_by: "desktop", decided_at: CHECKED_AT + 60_000 },
    ...overrides,
  });

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
    ...overrides,
  });

const currentRevision: AuthorizationRevision = {
  workspace_id: "ws_1",
  revision: 3,
  grants: [{ capability: "read_files", targets: [workspace.path], restrictions: [] }],
  source: { kind: "template_edited", template_id: "research_docs", base_revision: 2, confirmed_by: "desktop" },
  confirmed_at: CHECKED_AT,
};

/** 以記憶體模擬 Runtime：回答後該筆變成 answered，並恢復指定數量的 Task；授權可被修改。 */
function fakeClient(initial: DecisionRequestView[], options: { resumed?: string[]; revision?: AuthorizationRevision } = {}) {
  let items = [...initial];
  let revisions = [options.revision ?? currentRevision];
  const authorizationView = (): AuthorizationView => ({
    workspace_id: "ws_1",
    status: "confirmed",
    effective: revisions.at(-1)!,
    revisions: [...revisions],
    templates: [],
    capabilities: [],
    risk_disclosure: "",
  });
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

const renderCenter = (client: DecisionClient & AuthorizationClient, onChange?: () => void) =>
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

  it("權限請求把「允許本次操作」與「修改 Workspace 授權」分成兩個區塊，前者列出綁定的單一操作", async () => {
    renderCenter(fakeClient([permissionDecision()]));

    const once = await screen.findByRole("region", { name: "允許本次操作" });
    expect(within(once).getByText("Bash:git push")).toBeInTheDocument();
    expect(within(once).getByText("origin main")).toBeInTheDocument();
    expect(within(once).getByText('flags：["--force"]')).toBeInTheDocument();
    expect(within(once).getByText(HASH.slice(0, 12))).toBeInTheDocument();
    expect(within(once).queryByText(HASH)).not.toBeInTheDocument();
    expect(within(once).getByText("report.md@2,notes.md@1")).toBeInTheDocument();
    expect(within(once).getByText(/只允許這個 Task 的這一個操作，用過一次就失效，不會修改 Workspace 授權/)).toBeInTheDocument();
    const modify = screen.getByRole("region", { name: "修改 Workspace 授權" });
    expect(within(modify).getByRole("button", { name: "修改 Workspace 授權" })).toBeInTheDocument();
    // 回答仍用既有的選項
    expect(screen.getByRole("radio", { name: /允許本次操作/ })).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: /拒絕/ })).toBeInTheDocument();
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

    const once = await screen.findByRole("region", { name: "允許本次操作" });
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
    expect(await within(modify).findByText("無法歸類的操作不能透過修改授權放行，請用允許本次操作或拒絕")).toBeInTheDocument();
    expect(client.summarizeAuthorization).not.toHaveBeenCalled();
    expect(within(modify).queryByRole("button", { name: "確認修改授權" })).not.toBeInTheDocument();
  });

  it("能力已在目前授權中時，請使用者到授權設定頁調整目標或限制", async () => {
    const user = userEvent.setup();
    const client = fakeClient([permissionDecision()], {
      revision: { ...currentRevision, grants: [{ capability: "remote_write", targets: ["origin dev"], restrictions: [] }] },
    });
    renderCenter(client);

    await user.click(await screen.findByRole("button", { name: "修改 Workspace 授權" }));

    const modify = screen.getByRole("region", { name: "修改 Workspace 授權" });
    expect(await within(modify).findByText("這個能力已授權但目標或限制不涵蓋這次操作，請到授權設定頁調整")).toBeInTheDocument();
    expect(client.summarizeAuthorization).not.toHaveBeenCalled();
  });

  it("同一操作先前已放行過一次而結果未知時，醒目提醒先查證", async () => {
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
      "這個操作先前已經放行過一次（2023/11/15 06:13，Run run_7），結果未知。請先查證是否已經生效，再決定是否再允許一次。",
    );
    const deny = screen.getByRole("radio", { name: /拒絕（不要重送）/ }).closest("label")!;
    expect(within(deny).getByText("建議")).toBeInTheDocument();
  });

  it("先前的單次批准因綁定不符而不能沿用時說明原因", async () => {
    renderCenter(
      fakeClient([
        permissionDecision({
          permission: permissionOf({ rejected_approval: { request_id: "dec_old", reason: "target_changed" } }),
        }),
      ]),
    );

    expect(await screen.findByText("先前的單次批准（dec_old）不能沿用：目標資源不同")).toBeInTheDocument();
  });

  it("已回答的權限請求顯示批准已被哪個 Run 使用而失效，不再提供修改授權", async () => {
    renderCenter(
      fakeClient([
        permissionDecision({
          status: "answered",
          blocking: [],
          decision: { option_id: "allow_once", option_label: "允許本次操作", note: null, decided_by: "desktop", decided_at: CHECKED_AT },
          permission: permissionOf({ approval_use: { request_id: "dec_perm", run_id: "run_9", used_at: CHECKED_AT + 60_000 } }),
        }),
      ]),
    );

    const outcome = await screen.findByRole("region", { name: "裁定結果" });
    expect(within(outcome).getByText("這個批准已於 2023/11/15 06:14 由 Run run_9 使用，已失效")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "修改 Workspace 授權" })).not.toBeInTheDocument();
  });
});
