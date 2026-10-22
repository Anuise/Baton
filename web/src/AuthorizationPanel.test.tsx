import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { AuthorizationPanel, type AuthorizationPanelClient } from "./AuthorizationPanel";
import { runView } from "./fixtures";
import type {
  AuthorizationChanges,
  AuthorizationRevision,
  AuthorizationTemplate,
  AuthorizationView,
  Capability,
  ConfirmationSummary,
  ConfirmAuthorizationRequest,
  Grant,
  LegacyRun,
  TaskView,
  Workspace,
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

const RISK = "專案與第三方依賴程式會以目前 Windows 使用者的身分執行。";

const ALL: [Capability, string, boolean][] = [
  ["read_files", "讀取檔案", false],
  ["write_files", "編輯檔案", false],
  ["web_read", "讀取公開網頁", false],
  ["run_build_test", "建置與測試", false],
  ["install_project_dependencies", "安裝專案依賴", false],
  ["deploy", "部署", true],
];

const grant = (capability: Capability, targets: string[] = [workspace.path]) => ({ capability, targets, restrictions: ["僅限專案內"] });

const templates: AuthorizationTemplate[] = [
  {
    id: "research_docs",
    name: "研究／文件",
    description: "讀取與整理資料、撰寫文件。",
    grants: [grant("read_files"), grant("write_files")],
  },
  {
    id: "trusted_dev_p3",
    name: "可信專案開發（P3）",
    description: "編輯、建置、測試、安裝專案依賴。",
    grants: [grant("read_files"), grant("write_files"), grant("run_build_test"), grant("install_project_dependencies")],
  },
];

const EXECUTES = new Set<Capability>(["run_build_test", "install_project_dependencies", "deploy"]);

const minus = (a: string[], b: string[]) => a.filter((x) => !b.includes(x));

/** 模擬 Runtime 比較目前 revision 與新內容：新增能力、目標或移除限制都算擴大授權。 */
function changesFrom(current: AuthorizationRevision, grants: Grant[]): AuthorizationChanges {
  const changes = ALL.flatMap(([capability, label, high_risk]) => {
    const before = current.grants.find((g) => g.capability === capability);
    const after = grants.find((g) => g.capability === capability);
    if (!before && !after) return [];
    const change = {
      capability,
      label,
      high_risk,
      new_capability: !before,
      removed_capability: !after,
      added_targets: minus(after?.targets ?? [], before?.targets ?? []),
      removed_targets: minus(before?.targets ?? [], after?.targets ?? []),
      added_restrictions: minus(after?.restrictions ?? [], before?.restrictions ?? []),
      removed_restrictions: minus(before?.restrictions ?? [], after?.restrictions ?? []),
    };
    const expands =
      !change.removed_capability &&
      (change.new_capability || change.added_targets.length > 0 || change.removed_restrictions.length > 0);
    const changed = expands || change.removed_capability || change.removed_targets.length > 0 || change.added_restrictions.length > 0;
    return changed ? [{ ...change, expands }] : [];
  });
  return {
    base_revision: current.revision,
    changes,
    expands: changes.some((c) => c.expands),
    impact: changes.map((c) => (c.expands ? `擴大：Agent 將可以「${c.label}」` : `縮小：「${c.label}」的範圍變小`)),
  };
}

/** 以記憶體模擬 Runtime 的授權行為，讓測試只看使用者可見的流程。 */
function fakeClient(initial: AuthorizationRevision[] = [], initialLegacy: LegacyRun[] = []) {
  let revisions = [...initial];
  let legacyRuns = [...initialLegacy];
  const view = (): AuthorizationView => ({
    workspace_id: workspace.id,
    status: revisions.length > 0 ? "confirmed" : "unconfirmed",
    effective: revisions.at(-1) ?? null,
    revisions: [...revisions],
    templates,
    capabilities: ALL.map(([capability, label, high_risk]) => ({ capability, label, high_risk })),
    risk_disclosure: RISK,
    legacy_runs: [...legacyRuns],
  });
  const client: AuthorizationPanelClient = {
    getAuthorization: vi.fn(async () => view()),
    summarizeAuthorization: vi.fn(async (_id: string, grants: Grant[]): Promise<ConfirmationSummary> => {
      const requires = grants.some((g) => EXECUTES.has(g.capability));
      return {
        items: grants.map((g) => ({
          capability: g.capability,
          label: ALL.find(([c]) => c === g.capability)![1],
          targets: g.targets,
          restrictions: g.restrictions,
          high_risk: g.capability === "deploy",
        })),
        risks: requires ? [RISK] : [],
        requires_risk_acknowledgement: requires,
        changes: revisions.length > 0 ? changesFrom(revisions.at(-1)!, grants) : null,
      };
    }),
    confirmAuthorization: vi.fn(async (_id: string, { templateId, grants, expectedRevision }: ConfirmAuthorizationRequest) => {
      // 與 Runtime 相同：帶了 expected revision 卻不是目前生效的，代表另一個視窗先確認過
      const current = revisions.at(-1)?.revision ?? null;
      if (expectedRevision !== null && expectedRevision !== current) {
        throw new Error(`授權已被更新（目前 revision ${current ?? "無"}，編輯時是 revision ${expectedRevision}），請重新載入後再確認`);
      }
      const revision: AuthorizationRevision = {
        workspace_id: workspace.id,
        revision: revisions.length + 1,
        grants,
        source: { kind: templateId ? "template" : "custom", template_id: templateId, base_revision: null, confirmed_by: "desktop" },
        confirmed_at: 1_700_000_000_000,
      };
      revisions = [...revisions, revision];
      return revision;
    }),
    // 已啟動的 Run 只記錄停止要求，等程序確認停止
    cancelRun: vi.fn(async (_id: string, runId: string) => {
      legacyRuns = legacyRuns.map((r) => (r.run_id === runId ? { ...r, cancel_requested: true } : r));
      return runView({ id: runId, cancel_requested_at: 1_700_000_000_000 });
    }),
    // 面板不使用回傳值，只回傳識別欄位
    cancelTask: vi.fn(async (_id: string, taskId: string) => {
      legacyRuns = legacyRuns.filter((r) => r.task_id !== taskId);
      return { id: taskId, status: "cancelled" } as TaskView;
    }),
  };
  return client;
}

async function renderPanel(client: AuthorizationPanelClient) {
  const user = userEvent.setup();
  render(<AuthorizationPanel client={client} workspace={workspace} timeZone="UTC" />);
  await screen.findByRole("heading", { name: "授權設定：筆記" });
  return user;
}

const checkbox = (label: string) => screen.getByRole("checkbox", { name: new RegExp(`^${label}`) });

describe("Workspace 授權設定", () => {
  it("新 Workspace 尚未確認：說明未確認前不准入，沒有任何能力被預先勾選", async () => {
    await renderPanel(fakeClient());

    expect(screen.getByText(/尚未確認授權/)).toBeInTheDocument();
    expect(screen.getByText(/都不會被准入/)).toBeInTheDocument();
    for (const [, label] of ALL) expect(checkbox(label)).not.toBeChecked();
    expect(screen.queryByRole("button", { name: "確認並保存授權" })).not.toBeInTheDocument();
  });

  it("同時提供研究／文件與可信專案開發兩個範本，套用後可以編輯", async () => {
    const user = await renderPanel(fakeClient());

    await user.click(screen.getByRole("button", { name: "套用「可信專案開發（P3）」" }));

    for (const label of ["讀取檔案", "編輯檔案", "建置與測試", "安裝專案依賴"]) expect(checkbox(label)).toBeChecked();
    expect(checkbox("部署")).not.toBeChecked();
    expect(screen.getByLabelText("建置與測試：目標資源（每行一項）")).toHaveValue("C:\\work\\notes");
    await user.click(checkbox("安裝專案依賴"));
    expect(checkbox("安裝專案依賴")).not.toBeChecked();
    expect(screen.getByRole("button", { name: "套用「研究／文件」" })).toBeInTheDocument();
  });

  it("高風險能力標示出來，不屬於任何範本", async () => {
    const user = await renderPanel(fakeClient());

    expect(screen.getByText("（高風險，需要明確授權）")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "套用「可信專案開發（P3）」" }));
    expect(checkbox("部署")).not.toBeChecked();
  });

  it("P3 的確認摘要揭露以 Windows 使用者身分執行的風險，承認後才能確認並保存", async () => {
    const client = fakeClient();
    const user = await renderPanel(client);
    await user.click(screen.getByRole("button", { name: "套用「可信專案開發（P3）」" }));

    await user.click(screen.getByRole("button", { name: "檢視確認摘要" }));

    const summary = await screen.findByRole("region", { name: "確認摘要" });
    expect(within(summary).getByText(/Windows 使用者的身分執行/)).toBeInTheDocument();
    const confirm = within(summary).getByRole("button", { name: "確認並保存授權" });
    expect(confirm).toBeDisabled();

    await user.click(within(summary).getByRole("checkbox", { name: "我了解並接受上述風險" }));
    await user.click(confirm);

    expect(await screen.findByRole("status")).toHaveTextContent("已保存為 revision 1");
    expect(client.confirmAuthorization).toHaveBeenCalledWith("ws_1", {
      templateId: "trusted_dev_p3",
      grants: templates[1].grants,
      acknowledgedRisk: true,
      acknowledgedExpansion: false,
      expectedRevision: null,
    });
    expect(screen.getByText(/目前授權：revision 1/)).toBeInTheDocument();
    expect(screen.queryByText(/尚未確認授權/)).not.toBeInTheDocument();
    expect(screen.getByRole("region", { name: "授權歷史" })).toBeInTheDocument();
  });

  it("研究／文件範本沒有執行風險，不必勾選承認就能確認", async () => {
    const client = fakeClient();
    const user = await renderPanel(client);
    await user.click(screen.getByRole("button", { name: "套用「研究／文件」" }));

    await user.click(screen.getByRole("button", { name: "檢視確認摘要" }));
    const summary = await screen.findByRole("region", { name: "確認摘要" });

    expect(within(summary).queryByRole("checkbox")).not.toBeInTheDocument();
    expect(within(summary).queryByRole("region", { name: "授權變更" })).not.toBeInTheDocument();
    await user.click(within(summary).getByRole("button", { name: "確認並保存授權" }));
    await waitFor(() => expect(client.confirmAuthorization).toHaveBeenCalled());
    expect(vi.mocked(client.confirmAuthorization).mock.calls[0][1].grants.map((g) => g.capability)).toEqual([
      "read_files",
      "write_files",
    ]);
  });

  it("編輯內容後先前的摘要作廢，必須重新檢視並重新承認風險", async () => {
    const user = await renderPanel(fakeClient());
    await user.click(screen.getByRole("button", { name: "套用「可信專案開發（P3）」" }));
    await user.click(screen.getByRole("button", { name: "檢視確認摘要" }));
    await user.click(await screen.findByRole("checkbox", { name: "我了解並接受上述風險" }));

    await user.click(checkbox("讀取公開網頁"));

    expect(screen.queryByRole("region", { name: "確認摘要" })).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "檢視確認摘要" }));
    expect(await screen.findByRole("checkbox", { name: "我了解並接受上述風險" })).not.toBeChecked();
  });

  it("已確認的授權載入到編輯區，並列出歷史與來源", async () => {
    const revision: AuthorizationRevision = {
      workspace_id: "ws_1",
      revision: 2,
      grants: [grant("read_files")],
      source: { kind: "template_edited", template_id: "research_docs", base_revision: 1, confirmed_by: "desktop" },
      confirmed_at: 1_700_000_000_000,
    };
    await renderPanel(fakeClient([{ ...revision, revision: 1, source: { ...revision.source, kind: "template", base_revision: null } }, revision]));

    expect(screen.getByText(/目前授權：revision 2.*範本「研究／文件」（已編輯）/)).toBeInTheDocument();
    expect(checkbox("讀取檔案")).toBeChecked();
    expect(checkbox("編輯檔案")).not.toBeChecked();
    const history = screen.getByRole("region", { name: "授權歷史" });
    expect(within(history).getAllByRole("listitem")).toHaveLength(2);
    expect(within(history).getAllByRole("listitem")[0]).toHaveTextContent("revision 2");
  });

  it("修改既有授權而擴大範圍時，列出變更與影響，必須另外承認擴大授權才能確認", async () => {
    const revision: AuthorizationRevision = {
      workspace_id: "ws_1",
      revision: 1,
      grants: [grant("read_files")],
      source: { kind: "custom", template_id: null, base_revision: null, confirmed_by: "desktop" },
      confirmed_at: 1_700_000_000_000,
    };
    const client = fakeClient([revision]);
    const user = await renderPanel(client);
    await user.click(checkbox("讀取公開網頁"));
    await user.type(screen.getByLabelText("讀取公開網頁：目標資源（每行一項）"), "https://example.com");

    await user.click(screen.getByRole("button", { name: "檢視確認摘要" }));

    const summary = await screen.findByRole("region", { name: "確認摘要" });
    const changes = within(summary).getByRole("region", { name: "授權變更" });
    expect(within(changes).getByText(/相對於目前授權（revision 1）/)).toBeInTheDocument();
    const item = within(changes).getByText("讀取公開網頁").closest("li")!;
    expect(item).toHaveTextContent("新增能力");
    expect(item).toHaveTextContent("新增目標：https://example.com");
    expect(item).toHaveTextContent("擴大授權");
    expect(within(changes).getByText("擴大：Agent 將可以「讀取公開網頁」")).toBeInTheDocument();
    const confirm = within(summary).getByRole("button", { name: "確認並保存授權" });
    expect(confirm).toBeDisabled();

    await user.click(within(summary).getByRole("checkbox", { name: "我了解這會擴大 Workspace 授權" }));
    await user.click(confirm);

    expect(await screen.findByRole("status")).toHaveTextContent("已保存為 revision 2");
    expect(vi.mocked(client.confirmAuthorization).mock.calls[0][1]).toMatchObject({
      acknowledgedRisk: false,
      acknowledgedExpansion: true,
      expectedRevision: 1,
    });
  });

  it("修改既有授權時帶上確認摘要的 base revision：另一個視窗先確認後，這次送出被拒絕，不覆蓋對方的授權", async () => {
    const revision: AuthorizationRevision = {
      workspace_id: "ws_1",
      revision: 1,
      grants: [grant("read_files"), grant("write_files")],
      source: { kind: "custom", template_id: null, base_revision: null, confirmed_by: "desktop" },
      confirmed_at: 1_700_000_000_000,
    };
    const client = fakeClient([revision]);
    const user = await renderPanel(client);
    await user.click(checkbox("編輯檔案"));
    await user.click(screen.getByRole("button", { name: "檢視確認摘要" }));
    const summary = await screen.findByRole("region", { name: "確認摘要" });
    // 另一個視窗在這之後確認了 revision 2
    await client.confirmAuthorization("ws_1", {
      templateId: null,
      grants: [grant("read_files")],
      acknowledgedRisk: false,
      acknowledgedExpansion: false,
      expectedRevision: 1,
    });

    await user.click(within(summary).getByRole("button", { name: "確認並保存授權" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("授權已被更新（目前 revision 2，編輯時是 revision 1）");
    expect(vi.mocked(client.confirmAuthorization).mock.calls[1][1]).toMatchObject({ expectedRevision: 1 });
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
    expect((await client.getAuthorization("ws_1")).revisions).toHaveLength(2);
  });

  it("修改既有授權而只縮小範圍時，列出移除的項目，不需要承認擴大授權", async () => {
    const revision: AuthorizationRevision = {
      workspace_id: "ws_1",
      revision: 1,
      grants: [grant("read_files"), grant("write_files")],
      source: { kind: "custom", template_id: null, base_revision: null, confirmed_by: "desktop" },
      confirmed_at: 1_700_000_000_000,
    };
    const client = fakeClient([revision]);
    const user = await renderPanel(client);
    await user.click(checkbox("編輯檔案"));
    const restrictions = screen.getByLabelText("讀取檔案：限制（每行一項）");
    await user.clear(restrictions);
    await user.type(restrictions, "僅限專案內{Enter}不讀隱藏檔");

    await user.click(screen.getByRole("button", { name: "檢視確認摘要" }));

    const summary = await screen.findByRole("region", { name: "確認摘要" });
    const changes = within(summary).getByRole("region", { name: "授權變更" });
    expect(within(changes).getByText("編輯檔案").closest("li")).toHaveTextContent("移除能力");
    expect(within(changes).getByText("讀取檔案").closest("li")).toHaveTextContent(/新增限制：不讀隱藏檔$/);
    expect(within(changes).queryByText(/擴大授權/)).not.toBeInTheDocument();
    expect(within(summary).queryByRole("checkbox", { name: "我了解這會擴大 Workspace 授權" })).not.toBeInTheDocument();

    await user.click(within(summary).getByRole("button", { name: "確認並保存授權" }));

    await waitFor(() => expect(client.confirmAuthorization).toHaveBeenCalled());
    expect(vi.mocked(client.confirmAuthorization).mock.calls[0][1]).toMatchObject({ acknowledgedExpansion: false });
  });

  it("Runtime 拒絕時顯示它的說明，不宣稱已保存", async () => {
    const client = fakeClient();
    vi.mocked(client.confirmAuthorization).mockRejectedValueOnce(new Error("能力「讀取檔案」必須指定至少一個目標資源"));
    const user = await renderPanel(client);
    await user.click(checkbox("讀取檔案"));
    await user.click(screen.getByRole("button", { name: "檢視確認摘要" }));

    await user.click(await screen.findByRole("button", { name: "確認並保存授權" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("必須指定至少一個目標資源");
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
    expect(screen.getByText(/尚未確認授權/)).toBeInTheDocument();
  });

  describe("撤銷授權後仍沿用舊授權的 Run", () => {
    const revisions: AuthorizationRevision[] = [
      {
        workspace_id: "ws_1",
        revision: 1,
        grants: [grant("read_files"), grant("write_files"), grant("deploy")],
        source: { kind: "custom", template_id: null, base_revision: null, confirmed_by: "desktop" },
        confirmed_at: 1_700_000_000_000,
      },
      {
        workspace_id: "ws_1",
        revision: 2,
        grants: [grant("read_files")],
        source: { kind: "custom", template_id: null, base_revision: 1, confirmed_by: "desktop" },
        confirmed_at: 1_700_003_600_000,
      },
    ];
    const legacy = (overrides: Partial<LegacyRun> = {}): LegacyRun => ({
      run_id: "run_1",
      task_id: "task_1",
      task_title: "整理報告",
      status: "running",
      authorization_revision: 1,
      admitted_at: 1_700_000_000_000,
      cancel_requested: false,
      revoked_capabilities: ["write_files", "deploy"],
      ...overrides,
    });

    it("列出沿用舊授權的 Run：Task 標題、revision、被撤銷的能力與准入時間", async () => {
      const client = fakeClient(revisions, [
        legacy(),
        legacy({ run_id: "run_2", task_id: "task_2", task_title: "部署網站", status: "starting", revoked_capabilities: ["deploy"] }),
      ]);
      await renderPanel(client);

      const notice = screen.getByRole("status", { name: "後續執行已停用，仍有 2 個 Run 沿用舊授權" });
      expect(within(notice).getByText(/沒有固定完成時間/)).toBeInTheDocument();
      const items = within(notice).getAllByRole("listitem");
      expect(items).toHaveLength(2);
      expect(items[0]).toHaveTextContent("整理報告");
      expect(items[0]).toHaveTextContent("沿用 revision 1");
      expect(items[0]).toHaveTextContent("編輯檔案、部署");
      expect(items[0]).toHaveTextContent("2023/11/14 22:13");
      expect(items[1]).toHaveTextContent("部署網站");
    });

    it("按「暫停」停止這個 Run，完成後重新載入並顯示等待程序確認", async () => {
      const client = fakeClient(revisions, [legacy()]);
      const user = await renderPanel(client);

      await user.click(screen.getByRole("button", { name: "暫停" }));

      expect(client.cancelRun).toHaveBeenCalledWith("ws_1", "run_1");
      expect(await screen.findByText(/已要求停止，等待程序確認/)).toBeInTheDocument();
      expect(client.getAuthorization).toHaveBeenCalledTimes(2);
      expect(client.cancelTask).not.toHaveBeenCalled();
    });

    it("按「取消」取消整個 Task，完成後重新載入；沒有剩下的 Run 時提示消失", async () => {
      const client = fakeClient(revisions, [legacy()]);
      const user = await renderPanel(client);

      await user.click(screen.getByRole("button", { name: "取消" }));

      expect(client.cancelTask).toHaveBeenCalledWith("ws_1", "task_1");
      await waitFor(() => expect(screen.queryByText(/沿用舊授權/)).not.toBeInTheDocument());
      expect(client.getAuthorization).toHaveBeenCalledTimes(2);
      expect(client.cancelRun).not.toHaveBeenCalled();
    });

    it("操作失敗時顯示 Runtime 的說明", async () => {
      const client = fakeClient(revisions, [legacy()]);
      vi.mocked(client.cancelRun).mockRejectedValueOnce(new Error("Runtime 無法連線"));
      const user = await renderPanel(client);

      await user.click(screen.getByRole("button", { name: "暫停" }));

      expect(await screen.findByRole("alert")).toHaveTextContent("Runtime 無法連線");
    });

    it("已要求停止的 Run 兩個按鈕都停用，並說明正在等待程序確認", async () => {
      await renderPanel(fakeClient(revisions, [legacy({ cancel_requested: true })]));

      const item = within(screen.getByRole("status", { name: /沿用舊授權/ })).getByRole("listitem");
      expect(within(item).getByRole("button", { name: "暫停" })).toBeDisabled();
      expect(within(item).getByRole("button", { name: "取消" })).toBeDisabled();
      expect(item).toHaveTextContent("已要求停止，等待程序確認");
    });

    it("沒有沿用舊授權的 Run 時不顯示提示", async () => {
      await renderPanel(fakeClient(revisions));

      expect(screen.queryByText(/沿用舊授權/)).not.toBeInTheDocument();
      expect(screen.queryByRole("status")).not.toBeInTheDocument();
    });
  });

  it("讀不到授權設定時顯示原因，不假裝是未確認", async () => {
    const client = fakeClient();
    vi.mocked(client.getAuthorization).mockRejectedValueOnce(new Error("connection refused"));

    render(<AuthorizationPanel client={client} workspace={workspace} />);

    expect(await screen.findByRole("alert")).toHaveTextContent("connection refused");
    expect(screen.queryByText(/尚未確認授權/)).not.toBeInTheDocument();
  });
});
