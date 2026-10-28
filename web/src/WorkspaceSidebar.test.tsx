import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { open } from "@tauri-apps/plugin-dialog";

import { lifecycleStatus, workspaceLifecycle } from "./fixtures";
import { WorkspaceSidebar } from "./WorkspaceSidebar";
import type { Workspace, WorkspaceLifecycleView } from "./runtimeClient";

vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

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

  it("建立完成後選中新的 Workspace", async () => {
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

  it("建立失敗時不選取任何 Workspace", async () => {
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

  it("點 Workspace 即選取它", async () => {
    const onSelect = vi.fn();
    const user = userEvent.setup();
    render(<WorkspaceSidebar client={fakeClient([workspace()])} onSelect={onSelect} />);

    await user.click(await screen.findByRole("button", { name: "選擇 筆記" }));

    expect(onSelect).toHaveBeenCalledWith(workspace());
  });

  it("清單只負責選擇 Workspace，不再列出各功能的按鈕", async () => {
    render(<WorkspaceSidebar client={fakeClient([workspace()])} />);
    const item = (await screen.findByRole("button", { name: "選擇 筆記" })).closest("li")!;

    const buttons = within(item)
      .getAllByRole("button")
      .map((button) => button.getAttribute("aria-label"))
      // 「?」說明提示不是功能按鈕
      .filter((label) => !label?.endsWith("的說明"));
    expect(buttons).toEqual(["選擇 筆記", "封存 筆記"]);
  });

  it("標示目前選中的 Workspace", async () => {
    const client = fakeClient([workspace(), workspace({ id: "ws_2", name: "專案", path: "D:\\proj" })]);
    render(<WorkspaceSidebar client={client} selectedId="ws_2" />);

    expect(await screen.findByRole("button", { name: "選擇 專案" })).toHaveAttribute("aria-current", "true");
    expect(screen.getByRole("button", { name: "選擇 筆記" })).not.toHaveAttribute("aria-current");
  });

  it("封存、自動續跑與建立時間收在項目的「管理」選單；Esc 收起並把焦點還給觸發鈕", async () => {
    const user = userEvent.setup();
    render(<WorkspaceSidebar client={fakeClient([workspace()])} timeZone="Asia/Taipei" />);

    const item = (await screen.findByRole("button", { name: "選擇 筆記" })).closest("li")!;
    const menu = within(item).getByText("建立於 2023/11/15 06:13").closest("details")!;
    const trigger = within(menu).getByLabelText("管理 筆記");
    expect(menu).not.toHaveAttribute("open");
    expect(within(menu).getByRole("button", { name: "封存 筆記" })).toBeInTheDocument();
    expect(await within(menu).findByRole("checkbox", { name: "完全退出後自動續跑 筆記" })).toBeInTheDocument();
    expect(within(menu).getByRole("checkbox", { name: "跨登入自動續跑 筆記" })).toBeInTheDocument();

    // <summary> 在瀏覽器中原生可用 Enter／空白鍵開啟；jsdom 只模擬點擊
    await user.click(trigger);
    expect(menu).toHaveAttribute("open");

    within(menu).getByRole("checkbox", { name: "跨登入自動續跑 筆記" }).focus();
    await user.keyboard("{Escape}");
    expect(menu).not.toHaveAttribute("open");
    expect(trigger).toHaveFocus();
  });

  it("展開的「管理」選單不被暫停列與下一個項目蓋住：浮層以項目定位，中間沒有層級較低的堆疊脈絡", async () => {
    // jsdom 不排版，只能檢查決定蓋疊的樣式；實際點擊命中以 Playwright 探針驗證
    // Vitest 會把 CSS 匯入（含 ?raw）換成空字串，專案也沒有 @types/node，所以以非字面的動態匯入讀檔
    const fs: { readFileSync(path: string, encoding: "utf8"): string } = await import(/* @vite-ignore */ `node:${"fs"}`);
    const style = document.createElement("style");
    style.textContent = fs.readFileSync("src/styles/shell.css", "utf8"); // 測試在 web/ 下執行
    document.head.append(style);
    try {
      const user = userEvent.setup();
      const paused = { workspace_id: "ws_1", name: "筆記", awaiting_resume: true, paused_by: "full_exit" as const };
      render(<WorkspaceSidebar client={fakeClient([workspace(), workspace({ id: "ws_2", name: "專案" })], [paused])} />);

      const item = (await screen.findByRole("button", { name: "繼續工作 筆記" })).closest("li")!;
      await user.click(within(item).getByLabelText("管理 筆記"));
      const panel = item.querySelector(".workspace-menu-panel")!;

      // 浮層與項目之間的祖先都不定位、不設 z-index：浮層的定位基準是整個項目（寬度以項目為上限），
      // 它的 --z-popover 也不會被困在 z-index: 1 的選單裡、輸給同一項目的暫停列或下一個項目
      for (let el = panel.parentElement!; el !== item; el = el.parentElement!) {
        expect(getComputedStyle(el).position).toBe("static");
        expect(getComputedStyle(el).zIndex).toMatch(/^(auto|)$/);
      }
      expect(getComputedStyle(item).position).toBe("relative");
      expect(getComputedStyle(item).zIndex).toMatch(/^(auto|)$/);
      expect(getComputedStyle(panel).zIndex).toBe("var(--z-popover)");
      expect(getComputedStyle(item.querySelector(".workspace-paused")!).zIndex).toBe("1");
    } finally {
      style.remove();
    }
  });

  it("待裁定數超過 999 時徽章顯示 999+，完整數字在可存取名稱", async () => {
    render(<WorkspaceSidebar client={fakeClient([workspace()])} pendingDecisions={{ ws_1: 1284 }} />);

    const badge = await screen.findByText("待裁定 999+");
    expect(badge).toHaveAttribute("aria-label", "待裁定 1,284 項");
  });

  it("讀到清單後把使用中的 Workspace 告訴上層，封存後再告知一次", async () => {
    const onWorkspaces = vi.fn();
    const archived = workspace({ id: "ws_2", name: "舊的", status: "archived", archived_at: CREATED_AT + 1 });
    const user = userEvent.setup();
    render(<WorkspaceSidebar client={fakeClient([workspace(), archived])} onWorkspaces={onWorkspaces} />);

    await waitFor(() => expect(onWorkspaces).toHaveBeenLastCalledWith([workspace()]));
    await user.click(screen.getByRole("button", { name: "封存 筆記" }));

    await waitFor(() => expect(onWorkspaces).toHaveBeenLastCalledWith([]));
  });

  it("每個 Workspace 有「完全退出後自動續跑」勾選，預設關閉，「?」提示說明只影響下次重新開啟", async () => {
    render(<WorkspaceSidebar client={fakeClient([workspace()])} />);

    const item = (await screen.findByText("筆記")).closest("li")!;
    const checkbox = await within(item).findByRole("checkbox", { name: "完全退出後自動續跑 筆記" });
    expect(checkbox).not.toBeChecked();
    // 說明改成「?」提示，平時不佔版面
    expect(within(item).getByRole("button", { name: "完全退出後自動續跑的說明" })).toHaveAccessibleDescription(
      "只影響下次重新開啟，不會立即啟停目前工作",
    );
    expect(within(item).queryByRole("tooltip")).not.toBeInTheDocument();
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

  it("每個 Workspace 另有「跨登入自動續跑」勾選，預設關閉，「?」提示說明只影響之後的登入", async () => {
    render(<WorkspaceSidebar client={fakeClient([workspace()])} />);

    const item = (await screen.findByText("筆記")).closest("li")!;
    const checkbox = await within(item).findByRole("checkbox", { name: "跨登入自動續跑 筆記" });
    expect(checkbox).not.toBeChecked();
    expect(within(item).getByRole("button", { name: "跨登入自動續跑的說明" })).toHaveAccessibleDescription(
      "重新登入 Windows 後是否自動續跑；只影響之後的登入",
    );
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

  describe("以檔案總管選擇目錄", () => {
    afterEach(() => {
      delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
      vi.mocked(open).mockReset();
    });

    /** 模擬在 Tauri 視窗中執行。 */
    function inTauri() {
      (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    }

    it("不在 Tauri 中時不顯示「瀏覽…」", async () => {
      render(<WorkspaceSidebar client={fakeClient()} />);
      await screen.findByText("還沒有 Workspace");

      expect(screen.queryByRole("button", { name: "瀏覽…" })).not.toBeInTheDocument();
    });

    it("選定資料夾後填入目錄欄位", async () => {
      inTauri();
      vi.mocked(open).mockResolvedValueOnce("D:\\picked\\folder");
      const user = userEvent.setup();
      render(<WorkspaceSidebar client={fakeClient()} />);
      await screen.findByText("還沒有 Workspace");

      await user.click(screen.getByRole("button", { name: "瀏覽…" }));

      expect(open).toHaveBeenCalledWith(expect.objectContaining({ directory: true, multiple: false }));
      await waitFor(() => expect(screen.getByLabelText("目錄")).toHaveValue("D:\\picked\\folder"));
    });

    it("取消選擇時目錄欄位維持原值", async () => {
      inTauri();
      vi.mocked(open).mockResolvedValueOnce(null);
      const user = userEvent.setup();
      render(<WorkspaceSidebar client={fakeClient()} />);
      await screen.findByText("還沒有 Workspace");
      await user.type(screen.getByLabelText("目錄"), "C:\\work\\docs");

      await user.click(screen.getByRole("button", { name: "瀏覽…" }));

      await waitFor(() => expect(open).toHaveBeenCalled());
      expect(screen.getByLabelText("目錄")).toHaveValue("C:\\work\\docs");
      expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    });

    it("對話框開不起來時顯示原因，不丟出例外", async () => {
      inTauri();
      vi.mocked(open).mockRejectedValueOnce(new Error("dialog.open not allowed"));
      const user = userEvent.setup();
      render(<WorkspaceSidebar client={fakeClient()} />);
      await screen.findByText("還沒有 Workspace");

      await user.click(screen.getByRole("button", { name: "瀏覽…" }));

      expect(await screen.findByRole("alert")).toHaveTextContent("dialog.open not allowed");
      expect(screen.getByLabelText("目錄")).toHaveValue("");
    });
  });

  it("有待裁定時在該 Workspace 顯示數量徽章，沒有時不顯示", async () => {
    const client = fakeClient([workspace(), workspace({ id: "ws_2", name: "專案", path: "D:\proj" })]);
    render(<WorkspaceSidebar client={client} pendingDecisions={{ ws_1: 3, ws_2: 0 }} />);

    await screen.findByText("專案");

    expect(screen.getAllByText(/待裁定/)).toHaveLength(1);
    expect(screen.getByText("待裁定 3")).toBeInTheDocument();
  });
});
