import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { lifecycleStatus, workspaceLifecycle } from "./fixtures";
import { WorkspaceSidebar } from "./WorkspaceSidebar";
import type { Workspace, WorkspaceLifecycleView } from "./runtimeClient";

/** 2023-11-14 22:13:20 UTC（台北是隔天 06:13）。 */
const CREATED_AT = 1_700_000_000_000;

function workspace(overrides: Partial<Workspace> = {}): Workspace {
  return {
    id: "ws_1",
    name: "筆記",
    path: "C:\\work\\notes",
    status: "active",
    version: 1,
    created_at: CREATED_AT,
    archived_at: null,
    model_overrides: { controller: null, worker: null, reviewer: null },
    ...overrides,
  };
}

/** 以記憶體模擬 Runtime 的 Workspace 清單與各自的生命週期，讓測試只看使用者可見的行為。 */
function fakeClient(initial: Workspace[] = [], lifecycles: Partial<WorkspaceLifecycleView>[] = []) {
  let items = [...initial];
  let views = lifecycles.map((overrides) => workspaceLifecycle(overrides));
  // 沒指定的 Workspace 用預設值（不自動續跑、沒有暫停）
  const viewOf = (w: Workspace) =>
    views.find((view) => view.workspace_id === w.id) ?? workspaceLifecycle({ workspace_id: w.id, name: w.name });
  const replace = (updated: WorkspaceLifecycleView) => {
    views = [...views.filter((view) => view.workspace_id !== updated.workspace_id), updated];
    return updated;
  };
  const client = {
    listWorkspaces: vi.fn(async () => ({ workspaces: [...items] })),
    createWorkspace: vi.fn(async ({ path, name }) => {
      const created = workspace({ id: `ws_${items.length + 1}`, path, name: name || path.split("\\").pop()! });
      items = [...items, created];
      return created;
    }),
    archiveWorkspace: vi.fn(async (id) => {
      const archived = { ...items.find((w) => w.id === id)!, status: "archived" as const, archived_at: CREATED_AT + 1 };
      items = items.map((w) => (w.id === id ? archived : w));
      return archived;
    }),
    setModelOverrides: vi.fn(async () => {
      throw new Error("側欄不會設定模型");
    }),
    lifecycleStatus: vi.fn(async () =>
      lifecycleStatus({ workspaces: items.filter((w) => w.status === "active").map(viewOf) }),
    ),
    updateWorkspaceLifecycle: vi.fn(async (id: string, autoResume: boolean) =>
      replace({ ...viewOf(items.find((w) => w.id === id)!), auto_resume_after_exit: autoResume }),
    ),
    updateWorkspaceLoginResume: vi.fn(async (id: string, autoResume: boolean) =>
      replace({ ...viewOf(items.find((w) => w.id === id)!), auto_resume_across_login: autoResume }),
    ),
    resumeWorkspace: vi.fn(async (id: string) =>
      replace({ ...viewOf(items.find((w) => w.id === id)!), awaiting_resume: false, paused_since: null, paused_by: null }),
    ),
  };
  return client;
}

describe("左側 Workspace 清單", () => {
  it("沒有 Workspace 時顯示空狀態", async () => {
    render(<WorkspaceSidebar client={fakeClient()} />);

    expect(await screen.findByText("還沒有 Workspace")).toBeInTheDocument();
  });

  it("列出使用中的 Workspace 名稱與目錄，不列出已封存的", async () => {
    const client = fakeClient([
      workspace(),
      workspace({ id: "ws_2", name: "舊專案", path: "D:\\old", status: "archived", archived_at: CREATED_AT }),
    ]);

    render(<WorkspaceSidebar client={client} />);

    expect(await screen.findByText("筆記")).toBeInTheDocument();
    expect(screen.getByText("C:\\work\\notes")).toBeInTheDocument();
    expect(screen.queryByText("舊專案")).not.toBeInTheDocument();
  });

  it("輸入目錄後建立，新的 Workspace 立刻出現在清單，表單清空", async () => {
    const client = fakeClient();
    const user = userEvent.setup();
    render(<WorkspaceSidebar client={client} />);
    await screen.findByText("還沒有 Workspace");

    await user.type(screen.getByLabelText("目錄"), "C:\\work\\docs");
    await user.click(screen.getByRole("button", { name: "建立 Workspace" }));

    expect(await screen.findByText("docs")).toBeInTheDocument();
    expect(client.createWorkspace).toHaveBeenCalledWith({ path: "C:\\work\\docs", name: undefined });
    expect(screen.getByLabelText("目錄")).toHaveValue("");
    expect(screen.queryByText("還沒有 Workspace")).not.toBeInTheDocument();
  });

  it("可以指定名稱", async () => {
    const client = fakeClient();
    const user = userEvent.setup();
    render(<WorkspaceSidebar client={client} />);
    await screen.findByText("還沒有 Workspace");

    await user.type(screen.getByLabelText("目錄"), "C:\\work\\docs");
    await user.type(screen.getByLabelText("名稱（選填）"), "研究");
    await user.click(screen.getByRole("button", { name: "建立 Workspace" }));

    expect(await screen.findByText("研究")).toBeInTheDocument();
    expect(client.createWorkspace).toHaveBeenCalledWith({ path: "C:\\work\\docs", name: "研究" });
  });

  it("目錄是空的時不送出請求", async () => {
    const client = fakeClient();
    const user = userEvent.setup();
    render(<WorkspaceSidebar client={client} />);
    await screen.findByText("還沒有 Workspace");

    await user.click(screen.getByRole("button", { name: "建立 Workspace" }));

    expect(client.createWorkspace).not.toHaveBeenCalled();
    expect(screen.getByRole("alert")).toHaveTextContent("請輸入目錄");
  });

  it("Runtime 拒絕時顯示它的說明並保留輸入，清單不變", async () => {
    const client = fakeClient();
    vi.mocked(client.createWorkspace).mockRejectedValueOnce(
      new Error("Runtime 拒絕連線（workspace_path_not_found）：找不到目錄：C:\\nope"),
    );
    const user = userEvent.setup();
    render(<WorkspaceSidebar client={client} />);
    await screen.findByText("還沒有 Workspace");

    await user.type(screen.getByLabelText("目錄"), "C:\\nope");
    await user.click(screen.getByRole("button", { name: "建立 Workspace" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("找不到目錄：C:\\nope");
    expect(screen.getByLabelText("目錄")).toHaveValue("C:\\nope");
    expect(screen.getByText("還沒有 Workspace")).toBeInTheDocument();
  });

  it("封存後從清單消失", async () => {
    const client = fakeClient([workspace()]);
    const user = userEvent.setup();
    render(<WorkspaceSidebar client={client} />);
    const item = (await screen.findByText("筆記")).closest("li")!;

    await user.click(within(item).getByRole("button", { name: "封存 筆記" }));

    await waitFor(() => expect(screen.queryByText("筆記")).not.toBeInTheDocument());
    expect(client.archiveWorkspace).toHaveBeenCalledWith("ws_1");
    expect(screen.getByText("還沒有 Workspace")).toBeInTheDocument();
  });

  it("讀不到清單時顯示原因，不假裝是空的", async () => {
    const client = fakeClient();
    vi.mocked(client.listWorkspaces).mockRejectedValueOnce(new Error("connection refused"));

    render(<WorkspaceSidebar client={client} />);

    expect(await screen.findByRole("alert")).toHaveTextContent("connection refused");
    expect(screen.queryByText("還沒有 Workspace")).not.toBeInTheDocument();
  });

  it("建立時間由 UTC 轉成指定（預設為系統）時區顯示", async () => {
    const { unmount } = render(<WorkspaceSidebar client={fakeClient([workspace()])} timeZone="Asia/Taipei" />);
    expect(await screen.findByText(/2023\/11\/15 06:13/)).toBeInTheDocument();
    unmount();

    render(<WorkspaceSidebar client={fakeClient([workspace()])} timeZone="UTC" />);
    expect(await screen.findByText(/2023\/11\/14 22:13/)).toBeInTheDocument();
  });

  it("建立完成後直接進入該 Workspace 的授權設定", async () => {
    const client = fakeClient();
    const onSelect = vi.fn();
    const user = userEvent.setup();
    render(<WorkspaceSidebar client={client} onSelect={onSelect} />);
    await screen.findByText("還沒有 Workspace");

    await user.type(screen.getByLabelText("目錄"), "C:\\work\\docs");
    await user.click(screen.getByRole("button", { name: "建立 Workspace" }));

    await waitFor(() => expect(onSelect).toHaveBeenCalledTimes(1));
    expect(onSelect.mock.calls[0][0]).toMatchObject({ path: "C:\\work\\docs", name: "docs" });
  });

  it("建立失敗時不進入授權設定", async () => {
    const client = fakeClient();
    vi.mocked(client.createWorkspace).mockRejectedValueOnce(new Error("找不到目錄"));
    const onSelect = vi.fn();
    const user = userEvent.setup();
    render(<WorkspaceSidebar client={client} onSelect={onSelect} />);
    await screen.findByText("還沒有 Workspace");

    await user.type(screen.getByLabelText("目錄"), "C:\\nope");
    await user.click(screen.getByRole("button", { name: "建立 Workspace" }));

    await screen.findByRole("alert");
    expect(onSelect).not.toHaveBeenCalled();
  });

  it("既有的 Workspace 可以隨時重新開啟授權設定", async () => {
    const onSelect = vi.fn();
    const user = userEvent.setup();
    render(<WorkspaceSidebar client={fakeClient([workspace()])} onSelect={onSelect} />);

    await user.click(await screen.findByRole("button", { name: "授權設定 筆記" }));

    expect(onSelect).toHaveBeenCalledWith(workspace());
  });

  it("每個 Workspace 都有「工作面板」入口，不會觸發授權設定", async () => {
    const onSelect = vi.fn();
    const onOpenTasks = vi.fn();
    const user = userEvent.setup();
    render(<WorkspaceSidebar client={fakeClient([workspace()])} onSelect={onSelect} onOpenTasks={onOpenTasks} />);

    await user.click(await screen.findByRole("button", { name: "工作面板 筆記" }));

    expect(onOpenTasks).toHaveBeenCalledWith(workspace());
    expect(onSelect).not.toHaveBeenCalled();
  });

  it("每個 Workspace 有「完全退出後自動續跑」勾選，預設關閉並說明只影響下次重新開啟", async () => {
    render(<WorkspaceSidebar client={fakeClient([workspace()])} />);

    const item = (await screen.findByText("筆記")).closest("li")!;
    const checkbox = await within(item).findByRole("checkbox", { name: "完全退出後自動續跑 筆記" });
    expect(checkbox).not.toBeChecked();
    expect(within(item).getByText("只影響下次重新開啟，不會立即啟停目前工作")).toBeInTheDocument();
    expect(within(item).queryByRole("button", { name: "繼續工作 筆記" })).not.toBeInTheDocument();
  });

  it("勾選自動續跑時以 Workspace id 與 true 呼叫設定；取消勾選送出 false", async () => {
    const client = fakeClient([workspace(), workspace({ id: "ws_2", name: "研究", path: "D:\\research" })]);
    const user = userEvent.setup();
    render(<WorkspaceSidebar client={client} />);
    const checkbox = await screen.findByRole("checkbox", { name: "完全退出後自動續跑 研究" });

    await user.click(checkbox);

    expect(client.updateWorkspaceLifecycle).toHaveBeenCalledWith("ws_2", true);
    await waitFor(() => expect(checkbox).toBeChecked());
    expect(screen.getByRole("checkbox", { name: "完全退出後自動續跑 筆記" })).not.toBeChecked();

    await user.click(checkbox);

    expect(client.updateWorkspaceLifecycle).toHaveBeenLastCalledWith("ws_2", false);
    await waitFor(() => expect(checkbox).not.toBeChecked());
  });

  it("設定失敗時顯示原因，勾選維持原狀", async () => {
    const client = fakeClient([workspace()]);
    vi.mocked(client.updateWorkspaceLifecycle).mockRejectedValueOnce(new Error("Runtime 拒絕連線"));
    const user = userEvent.setup();
    render(<WorkspaceSidebar client={client} />);

    await user.click(await screen.findByRole("checkbox", { name: "完全退出後自動續跑 筆記" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("Runtime 拒絕連線");
    expect(screen.getByRole("checkbox", { name: "完全退出後自動續跑 筆記" })).not.toBeChecked();
  });

  it("完全退出後暫停中的 Workspace 顯示「繼續工作」，按下後呼叫 resume 並重新讀取", async () => {
    const client = fakeClient([workspace()], [{ awaiting_resume: true, paused_since: CREATED_AT }]);
    const user = userEvent.setup();
    render(<WorkspaceSidebar client={client} />);
    const item = (await screen.findByText("筆記")).closest("li")!;
    expect(await within(item).findByText("完全退出後暫停中")).toBeInTheDocument();

    await user.click(within(item).getByRole("button", { name: "繼續工作 筆記" }));

    expect(client.resumeWorkspace).toHaveBeenCalledWith("ws_1");
    await waitFor(() => expect(within(item).queryByText("完全退出後暫停中")).not.toBeInTheDocument());
    expect(client.lifecycleStatus).toHaveBeenCalledTimes(2);
  });

  it("停止意圖仍生效而無法繼續工作時顯示 Runtime 的說明，仍維持暫停中", async () => {
    const client = fakeClient([workspace()], [{ awaiting_resume: true, paused_since: CREATED_AT }]);
    vi.mocked(client.resumeWorkspace).mockRejectedValueOnce(new Error("Runtime 正在完全退出，無法繼續工作"));
    const user = userEvent.setup();
    render(<WorkspaceSidebar client={client} />);

    await user.click(await screen.findByRole("button", { name: "繼續工作 筆記" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("Runtime 正在完全退出，無法繼續工作");
    expect(screen.getByText("完全退出後暫停中")).toBeInTheDocument();
  });

  it("每個 Workspace 另有「跨登入自動續跑」勾選，預設關閉並說明只影響之後的登入", async () => {
    render(<WorkspaceSidebar client={fakeClient([workspace()])} />);

    const item = (await screen.findByText("筆記")).closest("li")!;
    const checkbox = await within(item).findByRole("checkbox", { name: "跨登入自動續跑 筆記" });
    expect(checkbox).not.toBeChecked();
    expect(within(item).getByText("重新登入 Windows 後是否自動續跑；只影響之後的登入")).toBeInTheDocument();
  });

  it("兩項自動續跑設定各自呼叫自己的 API，勾選一項不影響另一項", async () => {
    const client = fakeClient([workspace()]);
    const user = userEvent.setup();
    render(<WorkspaceSidebar client={client} />);
    const acrossLogin = await screen.findByRole("checkbox", { name: "跨登入自動續跑 筆記" });
    const afterExit = screen.getByRole("checkbox", { name: "完全退出後自動續跑 筆記" });

    await user.click(acrossLogin);

    expect(client.updateWorkspaceLoginResume).toHaveBeenCalledWith("ws_1", true);
    expect(client.updateWorkspaceLifecycle).not.toHaveBeenCalled();
    await waitFor(() => expect(acrossLogin).toBeChecked());
    expect(afterExit).not.toBeChecked();

    await user.click(afterExit);

    expect(client.updateWorkspaceLifecycle).toHaveBeenCalledWith("ws_1", true);
    expect(client.updateWorkspaceLoginResume).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(afterExit).toBeChecked());
    expect(acrossLogin).toBeChecked();

    await user.click(acrossLogin);

    expect(client.updateWorkspaceLoginResume).toHaveBeenLastCalledWith("ws_1", false);
    await waitFor(() => expect(acrossLogin).not.toBeChecked());
    expect(afterExit).toBeChecked();
  });

  it("跨登入自動續跑設定失敗時顯示原因，勾選維持原狀", async () => {
    const client = fakeClient([workspace()]);
    vi.mocked(client.updateWorkspaceLoginResume).mockRejectedValueOnce(new Error("Runtime 拒絕連線"));
    const user = userEvent.setup();
    render(<WorkspaceSidebar client={client} />);

    await user.click(await screen.findByRole("checkbox", { name: "跨登入自動續跑 筆記" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("Runtime 拒絕連線");
    expect(screen.getByRole("checkbox", { name: "跨登入自動續跑 筆記" })).not.toBeChecked();
  });

  it.each([
    ["full_exit", "完全退出後暫停中"],
    ["new_login", "重新登入 Windows 後暫停中（未開啟跨登入自動續跑）"],
    ["login_unverified", "無法核對是否跨登入，暫停中"],
  ] as const)("等待「繼續工作」時依暫停原因 %s 顯示說明", async (pausedBy, label) => {
    const client = fakeClient([workspace()], [{ awaiting_resume: true, paused_since: CREATED_AT, paused_by: pausedBy }]);
    render(<WorkspaceSidebar client={client} />);

    const item = (await screen.findByText("筆記")).closest("li")!;
    expect(await within(item).findByText(label)).toBeInTheDocument();
    expect(within(item).getByRole("button", { name: "繼續工作 筆記" })).toBeInTheDocument();
  });

  it("讀不到生命週期時清單照常顯示，不顯示自動續跑設定", async () => {
    const client = fakeClient([workspace()]);
    vi.mocked(client.lifecycleStatus).mockRejectedValueOnce(new Error("離線"));

    render(<WorkspaceSidebar client={client} />);

    expect(await screen.findByText("筆記")).toBeInTheDocument();
    await waitFor(() => expect(client.lifecycleStatus).toHaveBeenCalled());
    expect(screen.queryByRole("checkbox")).not.toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("每個 Workspace 都有「裁定」入口，不會觸發授權設定", async () => {
    const onSelect = vi.fn();
    const onOpenDecisions = vi.fn();
    const user = userEvent.setup();
    render(<WorkspaceSidebar client={fakeClient([workspace()])} onSelect={onSelect} onOpenDecisions={onOpenDecisions} />);

    await user.click(await screen.findByRole("button", { name: "裁定 筆記" }));

    expect(onOpenDecisions).toHaveBeenCalledWith(workspace());
    expect(onSelect).not.toHaveBeenCalled();
  });

  it("有待裁定時在該 Workspace 顯示數量徽章，沒有時不顯示", async () => {
    const client = fakeClient([workspace(), workspace({ id: "ws_2", name: "專案", path: "D:\proj" })]);
    render(<WorkspaceSidebar client={client} pendingDecisions={{ ws_1: 3, ws_2: 0 }} />);

    await screen.findByText("專案");

    expect(screen.getAllByText(/待裁定/)).toHaveLength(1);
    expect(screen.getByText("待裁定 3")).toBeInTheDocument();
  });

  it("每個 Workspace 都有「成果與驗證」入口，不會觸發授權設定或工作面板", async () => {
    const onSelect = vi.fn();
    const onOpenTasks = vi.fn();
    const onOpenArtifacts = vi.fn();
    const user = userEvent.setup();
    render(
      <WorkspaceSidebar
        client={fakeClient([workspace()])}
        onSelect={onSelect}
        onOpenTasks={onOpenTasks}
        onOpenArtifacts={onOpenArtifacts}
      />,
    );

    await user.click(await screen.findByRole("button", { name: "成果與驗證 筆記" }));

    expect(onOpenArtifacts).toHaveBeenCalledWith(workspace());
    expect(onSelect).not.toHaveBeenCalled();
    expect(onOpenTasks).not.toHaveBeenCalled();
  });

  it("每個 Workspace 都有「知識」入口，不會觸發其他檢視", async () => {
    const onSelect = vi.fn();
    const onOpenTasks = vi.fn();
    const onOpenArtifacts = vi.fn();
    const onOpenKnowledge = vi.fn();
    const user = userEvent.setup();
    render(
      <WorkspaceSidebar
        client={fakeClient([workspace()])}
        onSelect={onSelect}
        onOpenTasks={onOpenTasks}
        onOpenArtifacts={onOpenArtifacts}
        onOpenKnowledge={onOpenKnowledge}
      />,
    );

    await user.click(await screen.findByRole("button", { name: "知識 筆記" }));

    expect(onOpenKnowledge).toHaveBeenCalledWith(workspace());
    expect(onSelect).not.toHaveBeenCalled();
    expect(onOpenTasks).not.toHaveBeenCalled();
    expect(onOpenArtifacts).not.toHaveBeenCalled();
  });

  it("每個 Workspace 都有「時間線」入口，不會觸發其他檢視", async () => {
    const onSelect = vi.fn();
    const onOpenTasks = vi.fn();
    const onOpenKnowledge = vi.fn();
    const onOpenTimeline = vi.fn();
    const user = userEvent.setup();
    render(
      <WorkspaceSidebar
        client={fakeClient([workspace()])}
        onSelect={onSelect}
        onOpenTasks={onOpenTasks}
        onOpenKnowledge={onOpenKnowledge}
        onOpenTimeline={onOpenTimeline}
      />,
    );

    await user.click(await screen.findByRole("button", { name: "時間線 筆記" }));

    expect(onOpenTimeline).toHaveBeenCalledWith(workspace());
    expect(onSelect).not.toHaveBeenCalled();
    expect(onOpenTasks).not.toHaveBeenCalled();
    expect(onOpenKnowledge).not.toHaveBeenCalled();
  });

  it("每個 Workspace 都有「專案理解」入口，不會觸發其他檢視", async () => {
    const onSelect = vi.fn();
    const onOpenTimeline = vi.fn();
    const onOpenUnderstanding = vi.fn();
    const user = userEvent.setup();
    render(
      <WorkspaceSidebar
        client={fakeClient([workspace()])}
        onSelect={onSelect}
        onOpenTimeline={onOpenTimeline}
        onOpenUnderstanding={onOpenUnderstanding}
      />,
    );

    await user.click(await screen.findByRole("button", { name: "專案理解 筆記" }));

    expect(onOpenUnderstanding).toHaveBeenCalledWith(workspace());
    expect(onSelect).not.toHaveBeenCalled();
    expect(onOpenTimeline).not.toHaveBeenCalled();
  });

  it("每個 Workspace 都有「流程」入口，不會觸發其他檢視", async () => {
    const onSelect = vi.fn();
    const onOpenUnderstanding = vi.fn();
    const onOpenWorkflows = vi.fn();
    const user = userEvent.setup();
    render(
      <WorkspaceSidebar
        client={fakeClient([workspace()])}
        onSelect={onSelect}
        onOpenUnderstanding={onOpenUnderstanding}
        onOpenWorkflows={onOpenWorkflows}
      />,
    );

    await user.click(await screen.findByRole("button", { name: "流程 筆記" }));

    expect(onOpenWorkflows).toHaveBeenCalledWith(workspace());
    expect(onSelect).not.toHaveBeenCalled();
    expect(onOpenUnderstanding).not.toHaveBeenCalled();
  });

  it("每個 Workspace 都有「審查」入口，不會觸發其他檢視", async () => {
    const onSelect = vi.fn();
    const onOpenWorkflows = vi.fn();
    const onOpenReviews = vi.fn();
    const user = userEvent.setup();
    render(
      <WorkspaceSidebar
        client={fakeClient([workspace()])}
        onSelect={onSelect}
        onOpenWorkflows={onOpenWorkflows}
        onOpenReviews={onOpenReviews}
      />,
    );

    await user.click(await screen.findByRole("button", { name: "審查 筆記" }));

    expect(onOpenReviews).toHaveBeenCalledWith(workspace());
    expect(onSelect).not.toHaveBeenCalled();
    expect(onOpenWorkflows).not.toHaveBeenCalled();
  });

  it("每個 Workspace 都有「對話」入口，不會觸發其他檢視", async () => {
    const onSelect = vi.fn();
    const onOpenTasks = vi.fn();
    const onOpenConversation = vi.fn();
    const user = userEvent.setup();
    render(
      <WorkspaceSidebar
        client={fakeClient([workspace()])}
        onSelect={onSelect}
        onOpenTasks={onOpenTasks}
        onOpenConversation={onOpenConversation}
      />,
    );

    await user.click(await screen.findByRole("button", { name: "對話 筆記" }));

    expect(onOpenConversation).toHaveBeenCalledWith(workspace());
    expect(onSelect).not.toHaveBeenCalled();
    expect(onOpenTasks).not.toHaveBeenCalled();
  });
});
