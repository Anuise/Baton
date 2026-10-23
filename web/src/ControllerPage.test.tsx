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

const viewOf = (run: ControllerCliRun | null): ControllerCliView => ({ workspace_id: "ws_1", run });

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

  it("Runtime 重啟後宿主仍存活（rebound_at 有值）時警示平台工具已失效，仍可結束", async () => {
    const user = userEvent.setup();
    const client = fakeClient(controllerCliRun({ rebound_at: CHECKED_AT + 90_000 }));
    renderPage(client);

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Runtime 重新啟動過，這個 Controller CLI 的平台工具已失效。請按「結束」後以同一個帳號重新開啟（會接續原生 Session）。",
    );
    expect(screen.queryByText("Controller CLI 已開啟，請切到它的終端機視窗")).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "結束" }));

    expect(client.endControllerCli).toHaveBeenCalledWith("ws_1");
  });

  it("Runtime 重啟時宿主已不在而中斷，以「中斷原因」顯示", async () => {
    const interrupted = controllerCliRun({
      status: "interrupted",
      ended_at: CHECKED_AT + 90_000,
      end_code: "runtime_restarted",
      end_reason: "Runtime 重新啟動時 Controller CLI 已不在執行",
    });
    renderPage(fakeClient(interrupted));

    expect(await screen.findByText("中斷原因：Runtime 重新啟動時 Controller CLI 已不在執行")).toBeInTheDocument();
    expect(fact("狀態")).toHaveTextContent("中斷");
  });

  it("接續原生 Session 時在狀態區顯示，沒有接續時不顯示", async () => {
    const { unmount } = renderPage(fakeClient(controllerCliRun({ continue_session: true })));
    expect(await screen.findByText("執行中")).toBeInTheDocument();
    expect(fact("原生 Session")).toHaveTextContent("接續上次");
    unmount();

    renderPage(fakeClient(controllerCliRun()));
    expect(await screen.findByText("執行中")).toBeInTheDocument();
    expect(screen.queryByText("原生 Session", { selector: "dt" })).not.toBeInTheDocument();
  });

  describe("帳號預設選最近一次 Run 的帳號（同帳號重開才會接續原生 Session）", () => {
    const second = profile({ id: "prof_2", name: "第二帳號", config_dir: "C:\Baton\profiles\prof_2" });
    const lastRun = controllerCliRun({
      profile_id: "prof_2",
      status: "ended",
      ended_at: CHECKED_AT + 61_000,
      end_code: "user_ended",
      end_reason: "已從 App 結束 Controller CLI",
    });

    /** 可手動決定何時回應的 Promise。 */
    function deferred<T>() {
      let resolve!: (value: T) => void;
      const promise = new Promise<T>((r) => (resolve = r));
      return { promise, resolve };
    }

    it("狀態先到、帳號清單後到", async () => {
      const client = fakeClient(lastRun, [notLoggedIn, profile(), second]);
      const profiles = deferred<{ profiles: AccountProfile[]; limit: number }>();
      client.listProfiles.mockReturnValue(profiles.promise);
      renderPage(client);
      expect(await screen.findByText("已結束")).toBeInTheDocument();

      await act(async () => profiles.resolve({ profiles: [notLoggedIn, profile(), second], limit: 8 }));

      expect(screen.getByLabelText("Controller 帳號")).toHaveValue("prof_2");
    });

    it("帳號清單先到、狀態後到", async () => {
      const client = fakeClient(lastRun, [notLoggedIn, profile(), second]);
      const view = deferred<ControllerCliView>();
      client.getControllerCli.mockReturnValue(view.promise);
      renderPage(client);
      await act(async () => {});
      expect(client.listProfiles).toHaveBeenCalled();

      await act(async () => view.resolve(viewOf(lastRun)));

      expect(await screen.findByLabelText("Controller 帳號")).toHaveValue("prof_2");
    });

    it("最近一次的帳號不可用時選第一個可用的", async () => {
      const unusable = profile({ ...second, dispatchable: false, blockers: [{ code: "unverified", message: "尚未核對" }] });
      renderPage(fakeClient(lastRun, [notLoggedIn, profile(), unusable]));

      await screen.findByText("已結束");
      expect(await screen.findByLabelText("Controller 帳號")).toHaveValue("prof_1");
    });

    it("使用者手動改選後，之後到達的狀態不覆蓋選擇", async () => {
      const user = userEvent.setup();
      const client = fakeClient(null, [notLoggedIn, profile(), second]);
      renderPage(client);
      const select = await screen.findByLabelText("Controller 帳號");
      expect(select).toHaveValue("prof_1");

      await user.selectOptions(select, "prof_2");
      // 例如開啟被拒後重新讀取，讀到另一個視窗以 prof_1 開過又結束的 Run
      client.getControllerCli.mockResolvedValue(viewOf({ ...lastRun, profile_id: "prof_1" }));
      client.openControllerCli.mockRejectedValueOnce(new Error("暫時無法開啟"));
      await user.click(screen.getByRole("button", { name: "開啟" }));
      expect(await screen.findByRole("alert")).toHaveTextContent("暫時無法開啟");

      expect(screen.getByLabelText("Controller 帳號")).toHaveValue("prof_2");
    });
  });

  it("開啟中每 2 秒重新讀取，變成執行中，卸載後停止", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const client = fakeClient(controllerCliRun({ status: "starting", host: null, claude_pid: null, running_at: null }));
    const { unmount } = renderPage(client);
    expect(await screen.findByText("開啟中")).toBeInTheDocument();
    // 畫面先出現、輪詢的 effect 可能還沒執行（負載高時）：先讓它註冊計時器，再推進時間
    await act(async () => {});
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
