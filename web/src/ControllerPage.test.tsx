import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import { CONTROLLER_REFRESH_INTERVAL_MS, ControllerPage } from "./ControllerPage";
import {
  CHECKED_AT,
  accountScheduling,
  controllerCliRun,
  profile,
  quotaAssessment,
  quotaObservation,
  schedulingOverview,
} from "./fixtures";
import type {
  AccountProfile,
  AccountScheduling,
  ControllerCliRun,
  ControllerCliView,
  SchedulingOverview,
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

/** 尚未登入、不可派工的 Profile。 */
const notLoggedIn = profile({
  id: "prof_0",
  name: "備用帳號",
  verification: null,
  dispatchable: false,
  blockers: [{ code: "unverified", message: "尚未核對登入狀態；請完成原生登入後按「重新核對」" }],
});

/** 第二個可用的 Profile。 */
const spare = profile({ id: "prof_2", name: "第二帳號" });

const viewOf = (run: ControllerCliRun | null): ControllerCliView => ({ workspace_id: "ws_1", run, playbook_version: "1" });

/** 排程總覽裡的帳號：5h／7d 已用比例（null 是未知）與執行中 Worker 數。 */
function accountWith(
  profileId: string,
  fiveHourUsed: number | null,
  sevenDayUsed: number | null,
  running: number,
  overrides: Partial<AccountScheduling> = {},
): AccountScheduling {
  const observed = (window: "five_hour" | "seven_day", used: number | null) =>
    used === null ? null : quotaObservation({ profile_id: profileId, window, used });
  return accountScheduling({
    profile_id: profileId,
    running,
    quota: quotaAssessment({
      windows: [
        { window: "five_hour", label: "五小時", effective: observed("five_hour", fiveHourUsed), observed: null, estimated: null },
        { window: "seven_day", label: "七日", effective: observed("seven_day", sevenDayUsed), observed: null, estimated: null },
      ],
    }),
    ...overrides,
  });
}

function fakeClient(
  run: ControllerCliRun | null = null,
  profiles: AccountProfile[] = [notLoggedIn, profile()],
  // 預設排程總覽沒有帳號資料：選項只顯示名稱
  overview: SchedulingOverview = schedulingOverview({ accounts: [] }),
) {
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
    switchControllerCli: vi.fn(async (_workspaceId: string, profileId: string) =>
      viewOf(controllerCliRun({ end_requested_at: CHECKED_AT + 60_000, switch_to_profile_id: profileId })),
    ),
    listProfiles: vi.fn(async () => ({ profiles, limit: 8 })),
    schedulingOverview: vi.fn(async () => overview),
  };
}

const renderPage = (client: ReturnType<typeof fakeClient>, pollMs?: number) =>
  render(<ControllerPage client={client} workspace={workspace} timeZone="Asia/Taipei" pollMs={pollMs} />);

/** 在假計時器下讓排隊中的 Promise 與 React 更新全部完成。 */
async function settle(ms = 0) {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });
}

/** 狀態欄位的值。 */
const fact = (term: string) => screen.getByText(term, { selector: "dt" }).nextElementSibling;

/** 「開啟」按鈕：狀態與帳號清單的到達順序不定，等兩者都讀回、按鈕可以按了再回傳。 */
async function enabledOpenButton() {
  const button = await screen.findByRole("button", { name: "開啟" });
  await waitFor(() => expect(button).toBeEnabled());
  return button;
}

afterEach(() => {
  vi.useRealTimers();
});

describe("ControllerPage", () => {
  it("未開啟時列出使用中的帳號，不可用的停用並附原因，預設選第一個可用的", async () => {
    const archived = profile({ id: "prof_9", name: "舊帳號", status: "archived", dispatchable: false });
    renderPage(fakeClient(null, [notLoggedIn, profile(), archived]));

    expect(await screen.findByRole("heading", { name: "主控台" })).toBeInTheDocument();
    // 標題不等狀態讀回就會出現：等狀態區出現再斷言
    await waitFor(() => expect(fact("狀態")).toHaveTextContent("未開啟"));
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

  it("從未開啟過也顯示操作手冊版本，收在「詳細資訊」裡", async () => {
    renderPage(fakeClient(null));

    expect(await screen.findByText("操作手冊版本", { selector: "dt" })).toBeInTheDocument();
    expect(fact("狀態")).toHaveTextContent("未開啟");
    expect(fact("狀態")?.querySelector(".controller-status-idle")).not.toBeNull();
    expect(fact("操作手冊版本")).toHaveTextContent("1");
    const more = screen.getByText("詳細資訊").closest("details")!;
    expect(more).not.toHaveAttribute("open");
    expect(more).toHaveTextContent("操作手冊版本");
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

    expect(await screen.findByText("沒有可用的帳號，請到「帳號」新增或重新核對")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "開啟" })).toBeDisabled();
  });

  it("按開啟以選中的帳號呼叫 open，顯示開啟中並改為提示切到終端機", async () => {
    const user = userEvent.setup();
    const client = fakeClient();
    renderPage(client);

    await user.click(await enabledOpenButton());

    expect(client.openControllerCli).toHaveBeenCalledWith("ws_1", "prof_1");
    expect(fact("狀態")).toHaveTextContent("開啟中");
    expect(fact("Controller 帳號")).toHaveTextContent("主要帳號");
    expect(screen.getByText("已在終端機開啟，請切到該視窗操作")).toBeInTheDocument();
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

    await user.click(await enabledOpenButton());

    expect(await screen.findByRole("alert")).toHaveTextContent("這個 Workspace 已有開啟中的 Controller CLI");
  });

  it("執行中顯示帳號與開啟時間，按結束呼叫 end 並顯示結束中", async () => {
    const user = userEvent.setup();
    const client = fakeClient(controllerCliRun());
    renderPage(client);

    expect(await screen.findByText("執行中")).toBeInTheDocument();
    // 帳號名稱要等帳號清單讀回（可能比狀態晚到）
    await waitFor(() => expect(fact("Controller 帳號")).toHaveTextContent("主要帳號"));
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
    expect(await enabledOpenButton()).toBeEnabled();
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
      "Runtime 重新啟動過，這個視窗的平台工具已失效。請按「結束」，再用同一個帳號重新開啟（會接續 Session）。",
    );
    expect(screen.queryByText("已在終端機開啟，請切到該視窗操作")).not.toBeInTheDocument();
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
    // 假計時器不隨真實時間前進：輪詢只在推進時觸發，負載高低不影響呼叫次數
    vi.useFakeTimers();
    const client = fakeClient(controllerCliRun({ status: "starting", host: null, claude_pid: null, running_at: null }));
    const { unmount } = renderPage(client);
    await settle();
    expect(screen.getByText("開啟中")).toBeInTheDocument();
    expect(client.getControllerCli).toHaveBeenCalledTimes(1);
    expect(CONTROLLER_REFRESH_INTERVAL_MS).toBe(2000);

    client.getControllerCli.mockResolvedValue(viewOf(controllerCliRun()));
    await settle(CONTROLLER_REFRESH_INTERVAL_MS - 1);
    expect(client.getControllerCli).toHaveBeenCalledTimes(1);
    await settle(1);
    expect(client.getControllerCli).toHaveBeenCalledTimes(2);
    expect(screen.getByText("執行中")).toBeInTheDocument();

    unmount();
    await settle(CONTROLLER_REFRESH_INTERVAL_MS * 3);
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

  it("帳號選單顯示每個可用帳號的 5h／7d 剩餘額度與執行中 Worker 數，未知額度顯示未知", async () => {
    const overview = schedulingOverview({ accounts: [accountWith("prof_1", 0.4, 0.15, 1), accountWith("prof_2", null, 0.5, 0)] });
    const client = fakeClient(null, [notLoggedIn, profile(), spare], overview);
    renderPage(client);

    expect(await screen.findByRole("option", { name: "主要帳號（5h 剩 60%｜7d 剩 85%｜執行中 Worker 1）" })).toBeEnabled();
    expect(screen.getByRole("option", { name: "第二帳號（5h 未知｜7d 剩 50%｜執行中 Worker 0）" })).toBeEnabled();
    expect(screen.getByRole("option", { name: "備用帳號（尚未核對）" })).toBeDisabled();
    expect(client.schedulingOverview).toHaveBeenCalledTimes(1);
  });

  it("帳號名稱很長時選項只取前段（17 字含…）再接額度：半寬的「換成帳號」選單收合時仍看得到名稱與 5h／7d 額度", async () => {
    const longName = "長文字壓力測試專用標記的帳號 0：研發部門共用的高額度訂閱（請勿用於個人專案）";
    const overview = schedulingOverview({ accounts: [accountWith("prof_1", 0.4, 0.15, 1)] });
    renderPage(fakeClient(null, [profile({ name: longName })], overview));

    const option = await screen.findByRole("option", { name: /（5h 剩 60%｜7d 剩 85%｜執行中 Worker 1）$/ });
    expect(option.textContent).toBe("長文字壓力測試專用標記的帳號 0…（5h 剩 60%｜7d 剩 85%｜執行中 Worker 1）");
  });

  it("額度讀取失敗時仍可開啟，只提示無法取得額度", async () => {
    const client = fakeClient();
    client.schedulingOverview.mockRejectedValue(new Error("Runtime 離線"));
    renderPage(client);

    expect(await screen.findByText("無法取得額度：Runtime 離線")).toBeInTheDocument();
    // 額度的錯誤可能比狀態與帳號清單先到
    expect(await screen.findByRole("option", { name: "主要帳號" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "開啟" })).toBeEnabled();
  });

  it("開著時顯示 Controller 帳號的額度與執行中 Worker 數，輪詢時一起更新", async () => {
    const overview = schedulingOverview({ accounts: [accountWith("prof_1", 0.3, 0.2, 0)] });
    const client = fakeClient(controllerCliRun(), [profile(), spare], overview);
    // 輪詢讀到的新額度等確認過第一次的顯示後才放行
    let release = () => {};
    const released = new Promise<void>((resolve) => (release = resolve));
    client.schedulingOverview.mockResolvedValueOnce(overview).mockImplementation(async () => {
      await released;
      return schedulingOverview({ accounts: [accountWith("prof_1", 0.55, 0.25, 2)] });
    });
    // 用很短的輪詢間隔與真實計時器，避免假計時器在負載下多跑一輪
    renderPage(client, 50);

    // 額度是三個指標（dt 標籤、dd 數值）
    await screen.findByRole("group", { name: "Controller 帳號額度" });
    expect(fact("5h 剩餘")).toHaveTextContent("70%");
    expect(fact("7d 剩餘")).toHaveTextContent("80%");
    expect(fact("執行中 Worker")).toHaveTextContent("0");
    expect(screen.queryByRole("status")).not.toBeInTheDocument();

    release();
    await waitFor(() => expect(fact("5h 剩餘")).toHaveTextContent("45%"));
    expect(fact("7d 剩餘")).toHaveTextContent("75%");
    expect(fact("執行中 Worker")).toHaveTextContent("2");
    expect(client.schedulingOverview.mock.calls.length).toBeGreaterThanOrEqual(2);
  });

  it("Controller 帳號達到 draining 門檻時提示換帳號重開", async () => {
    const draining = accountWith("prof_1", 0.95, 0.4, 0, {
      quota_gate: {
        profile_id: "prof_1",
        state: "draining",
        reason: "drain_threshold",
        used: 0.95,
        reset_at: null,
        since: CHECKED_AT,
        version: 2,
      },
    });
    renderPage(fakeClient(controllerCliRun(), [profile(), spare], schedulingOverview({ accounts: [draining] })));

    expect(await screen.findByRole("status")).toHaveTextContent("Controller 帳號的額度已接近上限，建議換帳號重開");
  });

  it("換帳號重開只能選其他可用的帳號，按下呼叫 switch 並顯示換帳號中", async () => {
    const user = userEvent.setup();
    const client = fakeClient(controllerCliRun(), [notLoggedIn, profile(), spare]);
    renderPage(client);

    const target = await screen.findByLabelText("換成帳號");
    expect(within(target).getAllByRole("option").map((option) => option.textContent)).toEqual(["第二帳號"]);
    expect(target).toHaveValue("prof_2");
    await user.click(screen.getByRole("button", { name: "換帳號重開" }));

    expect(client.switchControllerCli).toHaveBeenCalledWith("ws_1", "prof_2");
    expect(screen.getByText("換帳號重開中：改用 第二帳號")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "換帳號重開" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "結束中…" })).toBeDisabled();
  });

  it("沒有其他可用的帳號時不能換帳號重開", async () => {
    renderPage(fakeClient(controllerCliRun(), [notLoggedIn, profile()]));

    expect(await screen.findByText("沒有其他可用的帳號")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "換帳號重開" })).toBeDisabled();
  });

  it("開著時帳號清單讀取失敗也提示無法取得帳號清單，仍可結束", async () => {
    const client = fakeClient(controllerCliRun());
    client.listProfiles.mockRejectedValue(new Error("Runtime 離線"));
    renderPage(client);

    expect(await screen.findByText("執行中")).toBeInTheDocument();
    expect(await screen.findByRole("alert")).toHaveTextContent("無法取得帳號清單：Runtime 離線");
    expect(screen.queryByLabelText("換成帳號")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "結束" })).toBeEnabled();
  });

  it("換帳號重開被拒時以警示顯示原因", async () => {
    const user = userEvent.setup();
    const client = fakeClient(controllerCliRun(), [profile(), spare]);
    client.switchControllerCli.mockRejectedValue(
      Object.assign(new Error("這個帳號目前不可用"), { code: "profile_unavailable" }),
    );
    renderPage(client);

    await user.click(await screen.findByRole("button", { name: "換帳號重開" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("這個帳號目前不可用");
  });

  it("顯示這次開啟是接續上次的 Session、由換帳號重開開啟或新的 Session", async () => {
    const resumed = renderPage(fakeClient(controllerCliRun({ continue_session: true })));
    expect(await screen.findByText("接續上次的 Session")).toBeInTheDocument();
    resumed.unmount();

    const switched = renderPage(
      fakeClient(controllerCliRun({ id: "ctl_2", profile_id: "prof_2", switched_from_run_id: "ctl_1" }), [profile(), spare]),
    );
    expect(await screen.findByText("由換帳號重開開啟的新 Session")).toBeInTheDocument();
    await waitFor(() => expect(fact("Controller 帳號")).toHaveTextContent("第二帳號"));
    switched.unmount();

    renderPage(fakeClient(controllerCliRun()));
    expect(await screen.findByText("新的 Session")).toBeInTheDocument();
  });

  it("換帳號重開而結束的 Run 顯示結束原因，不當成開啟失敗", async () => {
    const ended = controllerCliRun({
      status: "ended",
      end_requested_at: CHECKED_AT + 60_000,
      ended_at: CHECKED_AT + 61_000,
      end_code: "switched",
      end_reason: "已換帳號重開",
      switch_to_profile_id: "prof_2",
    });
    renderPage(fakeClient(ended, [profile(), spare]));

    expect(await screen.findByText("結束原因：已換帳號重開")).toBeInTheDocument();
    expect(screen.queryByText(/換帳號重開中/)).not.toBeInTheDocument();
  });
});
