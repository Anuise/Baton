import { act, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import { CONTROLLER_REFRESH_INTERVAL_MS, ControllerPage } from "./ControllerPage";
import { CHECKED_AT, controllerCliRun, profile } from "./fixtures";
import type { AccountProfile, ControllerCliRun, ControllerCliView, Workspace } from "./runtimeClient";

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

/** 尚未登入、不可派工的 Profile。 */
const notLoggedIn = profile({
  id: "prof_0",
  name: "備用帳號",
  verification: null,
  dispatchable: false,
  blockers: [{ code: "unverified", message: "尚未核對登入狀態；請完成原生登入後按「重新核對」" }],
});

const viewOf = (run: ControllerCliRun | null): ControllerCliView => ({ workspace_id: "ws_1", run, playbook_version: "1" });

function fakeClient(run: ControllerCliRun | null = null, profiles: AccountProfile[] = [notLoggedIn, profile()]) {
  return {
    getControllerCli: vi.fn(async () => viewOf(run)),
    openControllerCli: vi.fn(async (_workspaceId: string, profileId: string) =>
      viewOf(
        controllerCliRun({
          profile_id: profileId,
          status: "starting",
          launched_at: null,
          host: null,
          claude_pid: null,
          running_at: null,
        }),
      ),
    ),
    endControllerCli: vi.fn(async () => viewOf(controllerCliRun({ end_requested_at: CHECKED_AT + 60_000 }))),
    listProfiles: vi.fn(async () => ({ profiles, limit: 8 })),
  };
}

const renderPage = (client: ReturnType<typeof fakeClient>, pollMs?: number) =>
  render(<ControllerPage client={client} workspace={workspace} timeZone="Asia/Taipei" pollMs={pollMs} />);

/** 狀態欄位的值。 */
const fact = (term: string) => screen.getByText(term, { selector: "dt" }).nextElementSibling;

afterEach(() => {
  vi.useRealTimers();
});

describe("ControllerPage", () => {
  it("未開啟時列出使用中的帳號，不可用的停用並附原因，預設選第一個可用的", async () => {
    const archived = profile({ id: "prof_9", name: "舊帳號", status: "archived", dispatchable: false });
    renderPage(fakeClient(null, [notLoggedIn, profile(), archived]));

    expect(await screen.findByRole("heading", { name: "Controller：筆記" })).toBeInTheDocument();
    expect(fact("狀態")).toHaveTextContent("未開啟");
    const select = await screen.findByLabelText("Controller 帳號");
    expect(select).toHaveValue("prof_1");
    expect(screen.getByRole("option", { name: "備用帳號（尚未核對）" })).toBeDisabled();
    expect(screen.getByRole("option", { name: "主要帳號" })).toBeEnabled();
    expect(screen.queryByRole("option", { name: /舊帳號/ })).not.toBeInTheDocument();
    expect(screen.getByRole("list", { name: "不可用的帳號" })).toHaveTextContent(
      "備用帳號：尚未核對登入狀態；請完成原生登入後按「重新核對」",
    );
    expect(screen.getByRole("button", { name: "開啟" })).toBeEnabled();
  });

  it("從未開啟過也顯示操作手冊版本", async () => {
    renderPage(fakeClient(null));

    expect(await screen.findByText("操作手冊版本", { selector: "dt" })).toBeInTheDocument();
    expect(fact("狀態")).toHaveTextContent("未開啟");
    expect(fact("操作手冊版本")).toHaveTextContent("1");
  });

  it("有 Run 時也顯示操作手冊版本", async () => {
    const client = fakeClient(controllerCliRun());
    client.getControllerCli.mockResolvedValue({ ...viewOf(controllerCliRun()), playbook_version: "2" });
    renderPage(client);

    expect(await screen.findByText("執行中")).toBeInTheDocument();
    expect(fact("操作手冊版本")).toHaveTextContent("2");
  });

  it("狀態還沒載入時不顯示操作手冊版本", () => {
    const client = fakeClient();
    client.getControllerCli.mockReturnValue(new Promise(() => {}));
    renderPage(client);

    expect(screen.getByText("載入中…")).toBeInTheDocument();
    expect(screen.queryByText("操作手冊版本")).not.toBeInTheDocument();
  });

  it("沒有可用的帳號時不能開啟，並提示到帳號頁簽處理", async () => {
    renderPage(fakeClient(null, [notLoggedIn]));

    expect(await screen.findByText("沒有可用的帳號，請到「帳號」頁簽新增帳號或重新核對")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "開啟" })).toBeDisabled();
  });

  it("按開啟以選中的帳號呼叫 open，顯示開啟中並改為提示切到終端機", async () => {
    const user = userEvent.setup();
    const client = fakeClient();
    renderPage(client);

    await user.click(await screen.findByRole("button", { name: "開啟" }));

    expect(client.openControllerCli).toHaveBeenCalledWith("ws_1", "prof_1");
    expect(fact("狀態")).toHaveTextContent("開啟中");
    expect(fact("Controller 帳號")).toHaveTextContent("主要帳號");
    expect(screen.getByText("Controller CLI 已開啟，請切到它的終端機視窗")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "開啟" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "結束" })).toBeEnabled();
  });

  it("開啟被 Runtime 拒絕時以警示顯示原因", async () => {
    const user = userEvent.setup();
    const client = fakeClient();
    client.openControllerCli.mockRejectedValue(
      Object.assign(new Error("這個 Workspace 已有開啟中的 Controller CLI"), { code: "controller_cli_already_open" }),
    );
    renderPage(client);

    await user.click(await screen.findByRole("button", { name: "開啟" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("這個 Workspace 已有開啟中的 Controller CLI");
  });

  it("執行中顯示帳號與開啟時間，按結束呼叫 end 並顯示結束中", async () => {
    const user = userEvent.setup();
    const client = fakeClient(controllerCliRun());
    renderPage(client);

    expect(await screen.findByText("執行中")).toBeInTheDocument();
    expect(fact("Controller 帳號")).toHaveTextContent("主要帳號");
    expect(fact("開啟時間")).toHaveTextContent("2023/11/15 06:13");
    await user.click(screen.getByRole("button", { name: "結束" }));

    expect(client.endControllerCli).toHaveBeenCalledWith("ws_1");
    expect(screen.getByRole("button", { name: "結束中…" })).toBeDisabled();
  });

  it("最近一次開啟失敗時顯示原因，並可重新開啟", async () => {
    const failed = controllerCliRun({
      status: "ended",
      host: null,
      claude_pid: null,
      running_at: null,
      ended_at: CHECKED_AT + 5_000,
      end_code: "claude_not_found",
      end_reason: "找不到 claude 指令，請確認 Claude Code 已安裝",
    });
    renderPage(fakeClient(failed));

    expect(await screen.findByText("開啟失敗：找不到 claude 指令，請確認 Claude Code 已安裝")).toBeInTheDocument();
    expect(fact("狀態")).toHaveTextContent("已結束");
    expect(await screen.findByRole("button", { name: "開啟" })).toBeEnabled();
  });

  it("使用者結束後顯示結束原因", async () => {
    const ended = controllerCliRun({
      status: "ended",
      end_requested_at: CHECKED_AT + 60_000,
      ended_at: CHECKED_AT + 61_000,
      end_code: "user_ended",
      end_reason: "已從 App 結束 Controller CLI",
    });
    renderPage(fakeClient(ended));

    expect(await screen.findByText("結束原因：已從 App 結束 Controller CLI")).toBeInTheDocument();
  });

  it("開啟中每 2 秒重新讀取，變成執行中，卸載後停止", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const client = fakeClient(controllerCliRun({ status: "starting", host: null, claude_pid: null, running_at: null }));
    const { unmount } = renderPage(client);
    expect(await screen.findByText("開啟中")).toBeInTheDocument();
    expect(client.getControllerCli).toHaveBeenCalledTimes(1);
    expect(CONTROLLER_REFRESH_INTERVAL_MS).toBe(2000);

    client.getControllerCli.mockResolvedValue(viewOf(controllerCliRun()));
    // 留一秒餘裕，抵銷 shouldAdvanceTime 隨真實時間前進的部分
    act(() => vi.advanceTimersByTime(CONTROLLER_REFRESH_INTERVAL_MS - 1000));
    expect(client.getControllerCli).toHaveBeenCalledTimes(1);
    act(() => vi.advanceTimersByTime(1000));
    expect(client.getControllerCli).toHaveBeenCalledTimes(2);
    expect(await screen.findByText("執行中")).toBeInTheDocument();

    unmount();
    act(() => vi.advanceTimersByTime(CONTROLLER_REFRESH_INTERVAL_MS * 3));
    expect(client.getControllerCli).toHaveBeenCalledTimes(2);
  });

  it("沒有開著的 Controller CLI 時不輪詢", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const client = fakeClient();
    renderPage(client);
    await screen.findByText("未開啟");

    act(() => vi.advanceTimersByTime(CONTROLLER_REFRESH_INTERVAL_MS * 3));
    expect(client.getControllerCli).toHaveBeenCalledTimes(1);
    expect(within(screen.getByRole("region", { name: "Controller" })).queryByRole("alert")).not.toBeInTheDocument();
  });
});
