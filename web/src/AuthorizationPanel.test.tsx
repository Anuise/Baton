import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { AuthorizationPanel } from "./AuthorizationPanel";
import type {
  AuthorizationChanges,
  AuthorizationClient,
  AuthorizationRevision,
  AuthorizationTemplate,
  AuthorizationView,
  Capability,
  ConfirmationSummary,
  ConfirmAuthorizationRequest,
  Grant,
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
function fakeClient(initial: AuthorizationRevision[] = []) {
  let revisions = [...initial];
  const view = (): AuthorizationView => ({
    workspace_id: workspace.id,
    status: revisions.length > 0 ? "confirmed" : "unconfirmed",
    effective: revisions.at(-1) ?? null,
    revisions: [...revisions],
    templates,
    capabilities: ALL.map(([capability, label, high_risk]) => ({ capability, label, high_risk })),
    risk_disclosure: RISK,
  });
  const client: AuthorizationClient = {
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
    confirmAuthorization: vi.fn(async (_id: string, { templateId, grants }: ConfirmAuthorizationRequest) => {
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
  };
  return client;
}

async function renderPanel(client: AuthorizationClient) {
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
    expect(vi.mocked(client.confirmAuthorization).mock.calls[0][1]).toMatchObject({ acknowledgedRisk: false, acknowledgedExpansion: true });
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

  it("讀不到授權設定時顯示原因，不假裝是未確認", async () => {
    const client = fakeClient();
    vi.mocked(client.getAuthorization).mockRejectedValueOnce(new Error("connection refused"));

    render(<AuthorizationPanel client={client} workspace={workspace} />);

    expect(await screen.findByRole("alert")).toHaveTextContent("connection refused");
    expect(screen.queryByText(/尚未確認授權/)).not.toBeInTheDocument();
  });
});
