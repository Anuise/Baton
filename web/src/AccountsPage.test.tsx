import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import { AccountsPage, QUOTA_REFRESH_INTERVAL_MS } from "./AccountsPage";
import {
  accountRun,
  accountScheduling,
  CHECKED_AT,
  profile,
  quotaAssessment,
  quotaObservation,
  schedulingOverview,
} from "./fixtures";
import type { AccountClient, AccountProfile, AccountScheduling, PlanLimits } from "./runtimeClient";

type PageClient = Pick<
  AccountClient,
  "listProfiles" | "createProfile" | "verifyProfile" | "openLoginTerminal" | "archiveProfile"
>;

/** 以記憶體模擬 Runtime 的 Profile 清單，讓測試只看使用者可見的行為。 */
function fakeClient(initial: AccountProfile[] = []) {
  let items = [...initial];
  const client: PageClient = {
    listProfiles: vi.fn(async () => ({ profiles: [...items], limit: 8 })),
    createProfile: vi.fn(async ({ name, expectedAccount }) => {
      const created = profile({
        id: `prof_${items.length + 1}`,
        name,
        expected_account: expectedAccount ?? null,
        verification: null,
        dispatchable: false,
        blockers: [{ code: "profile_unverified", message: "尚未核對登入狀態；請完成原生登入後按「重新核對」" }],
      });
      items = [...items, created];
      return created;
    }),
    verifyProfile: vi.fn(async (id) => items.find((p) => p.id === id)!),
    openLoginTerminal: vi.fn(async () => ({ opened: true })),
    archiveProfile: vi.fn(async (id) => {
      const archived = { ...items.find((p) => p.id === id)!, status: "archived" as const };
      items = items.filter((p) => p.id !== id);
      return archived;
    }),
  };
  return client;
}

const overridden = profile({
  id: "prof_2",
  name: "被覆蓋的帳號",
  verification: {
    checked_at: 1_700_000_000_000,
    logged_in: true,
    source: "api_key",
    source_detail: "ANTHROPIC_API_KEY",
    account: null,
    config_dir_matches: true,
  },
  dispatchable: false,
  blockers: [
    {
      code: "auth_source_overridden",
      message:
        "實際使用的認證來源是 API key（ANTHROPIC_API_KEY），不是這個 Profile 的原生登入；已阻擋派工。請移除覆蓋來源後重新核對",
    },
  ],
});

describe("帳號頁", () => {
  it("沒有 Profile 時顯示空狀態與目前數量上限", async () => {
    render(<AccountsPage client={fakeClient()} />);

    expect(await screen.findByText("還沒有帳號 Profile")).toBeInTheDocument();
    expect(screen.getByText("0 / 8")).toBeInTheDocument();
  });

  it("列出 Profile：名稱、實際認證來源與帳號身分、最近核對時間與可派工狀態", async () => {
    render(<AccountsPage client={fakeClient([profile()])} timeZone="Asia/Taipei" />);

    const item = (await screen.findByText("主要帳號")).closest("li")!;
    expect(within(item).getByText("可派工")).toBeInTheDocument();
    expect(within(item).getByText(/原生登入/)).toBeInTheDocument();
    expect(within(item).getByText(/me@example\.com/)).toBeInTheDocument();
    expect(within(item).getByText(/2023\/11\/15 06:13/)).toBeInTheDocument();
    expect(within(item).queryByRole("alert")).not.toBeInTheDocument();
  });

  it("認證來源被覆蓋時標示「已阻擋派工」，並顯示實際來源與原因", async () => {
    render(<AccountsPage client={fakeClient([overridden])} />);

    const item = (await screen.findByText("被覆蓋的帳號")).closest("li")!;
    expect(within(item).getByText("已阻擋派工")).toBeInTheDocument();
    expect(within(item).getByText(/API key（ANTHROPIC_API_KEY）/, { selector: ".profile-source" })).toBeInTheDocument();
    expect(within(item).getByRole("alert")).toHaveTextContent("已阻擋派工");
    expect(within(item).queryByText("可派工")).not.toBeInTheDocument();
  });

  it("身分不符與尚未核對都顯示各自的阻擋原因", async () => {
    const mismatch = profile({
      id: "prof_3",
      name: "身分不符",
      dispatchable: false,
      blockers: [
        {
          code: "identity_mismatch",
          message: "實際認證的帳號是 b@example.com，與這個 Profile 預期的 a@example.com 不符，已阻擋派工",
        },
      ],
    });
    const fresh = profile({
      id: "prof_4",
      name: "新帳號",
      verification: null,
      dispatchable: false,
      blockers: [{ code: "profile_unverified", message: "尚未核對登入狀態；請完成原生登入後按「重新核對」" }],
    });

    render(<AccountsPage client={fakeClient([mismatch, fresh])} />);

    expect(await screen.findByText(/b@example\.com，與這個 Profile 預期的 a@example\.com 不符/)).toBeInTheDocument();
    const freshItem = screen.getByText("新帳號").closest("li")!;
    expect(within(freshItem).getByText("尚未核對")).toBeInTheDocument();
    expect(within(freshItem).getByRole("alert")).toHaveTextContent("尚未核對登入狀態");
  });

  it("新增 Profile 後立刻出現在清單，表單清空，並通知上層", async () => {
    const client = fakeClient();
    const onChange = vi.fn();
    const user = userEvent.setup();
    render(<AccountsPage client={client} onChange={onChange} />);
    await screen.findByText("還沒有帳號 Profile");

    await user.type(screen.getByLabelText("名稱"), "工作帳號");
    await user.type(screen.getByLabelText("預期帳號（選填）"), "work@example.com");
    await user.click(screen.getByRole("button", { name: "新增 Profile" }));

    expect(await screen.findByText("工作帳號")).toBeInTheDocument();
    expect(client.createProfile).toHaveBeenCalledWith({ name: "工作帳號", expectedAccount: "work@example.com" });
    expect(screen.getByLabelText("名稱")).toHaveValue("");
    expect(screen.getByText("1 / 8")).toBeInTheDocument();
    expect(onChange).toHaveBeenCalled();
  });

  it("名稱是空的時不送出請求", async () => {
    const client = fakeClient();
    const user = userEvent.setup();
    render(<AccountsPage client={client} />);
    await screen.findByText("還沒有帳號 Profile");

    await user.click(screen.getByRole("button", { name: "新增 Profile" }));

    expect(client.createProfile).not.toHaveBeenCalled();
    expect(screen.getByRole("alert")).toHaveTextContent("請輸入名稱");
  });

  it("Runtime 拒絕新增時顯示它的說明並保留輸入", async () => {
    const client = fakeClient();
    vi.mocked(client.createProfile).mockRejectedValueOnce(
      new Error("Runtime 拒絕連線（profile_account_invalid）：預期帳號必須是電子郵件"),
    );
    const user = userEvent.setup();
    render(<AccountsPage client={client} />);
    await screen.findByText("還沒有帳號 Profile");

    await user.type(screen.getByLabelText("名稱"), "工作帳號");
    await user.type(screen.getByLabelText("預期帳號（選填）"), "nope");
    await user.click(screen.getByRole("button", { name: "新增 Profile" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("預期帳號必須是電子郵件");
    expect(screen.getByLabelText("名稱")).toHaveValue("工作帳號");
  });

  it("已有 8 個 Profile 時不能再新增，並說明上限；封存後恢復", async () => {
    const eight = Array.from({ length: 8 }, (_, n) => profile({ id: `prof_${n + 1}`, name: `帳號 ${n + 1}` }));
    const client = fakeClient(eight);
    const user = userEvent.setup();
    render(<AccountsPage client={client} />);

    expect(await screen.findByText("8 / 8")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "新增 Profile" })).toBeDisabled();
    expect(screen.getByText(/最多 8 個/)).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "封存 帳號 1" }));

    await waitFor(() => expect(screen.getByRole("button", { name: "新增 Profile" })).toBeEnabled());
    expect(screen.getByText("7 / 8")).toBeInTheDocument();
  });

  it("「在終端機登入」要求 Runtime 為該 Profile 開啟登入終端機，並說明完成後按重新核對", async () => {
    const client = fakeClient([profile({ verification: null, dispatchable: false, blockers: [] })]);
    const user = userEvent.setup();
    render(<AccountsPage client={client} />);
    await screen.findByText("主要帳號");

    await user.click(screen.getByRole("button", { name: "在終端機登入 主要帳號" }));

    expect(client.openLoginTerminal).toHaveBeenCalledWith("prof_1");
    expect(await screen.findByText(/已開啟終端機/)).toBeInTheDocument();
  });

  it("「重新核對」更新該 Profile 並通知上層", async () => {
    const client = fakeClient([overridden]);
    const onChange = vi.fn();
    const user = userEvent.setup();
    render(<AccountsPage client={client} onChange={onChange} />);
    await screen.findByText("被覆蓋的帳號");

    await user.click(screen.getByRole("button", { name: "重新核對 被覆蓋的帳號" }));

    await waitFor(() => expect(client.verifyProfile).toHaveBeenCalledWith("prof_2"));
    await waitFor(() => expect(onChange).toHaveBeenCalled());
  });

  it("核對失敗時顯示原因，清單不變", async () => {
    const client = fakeClient([profile()]);
    vi.mocked(client.verifyProfile).mockRejectedValueOnce(
      new Error("無法核對 Profile「主要帳號」的登入狀態：claude 沒有回應"),
    );
    const user = userEvent.setup();
    render(<AccountsPage client={client} />);
    await screen.findByText("主要帳號");

    await user.click(screen.getByRole("button", { name: "重新核對 主要帳號" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("claude 沒有回應");
    expect(screen.getByText("可派工")).toBeInTheDocument();
  });

  it("讀不到清單時顯示原因，不假裝是空的", async () => {
    const client = fakeClient();
    vi.mocked(client.listProfiles).mockRejectedValueOnce(new Error("connection refused"));

    render(<AccountsPage client={client} />);

    expect(await screen.findByRole("alert")).toHaveTextContent("connection refused");
    expect(screen.queryByText("還沒有帳號 Profile")).not.toBeInTheDocument();
  });

  it("畫面上不出現任何登入資料欄位", async () => {
    render(<AccountsPage client={fakeClient([profile()])} />);
    await screen.findByText("主要帳號");

    expect(screen.queryByLabelText(/密碼|token|金鑰|API key/i)).not.toBeInTheDocument();
  });

  describe("進行中的 Run 與並行容量", () => {
    /** 帶排程總覽的 client；帳號清單與 Profile 共用同一份 fake。 */
    function withOverview(accounts: AccountScheduling[], profiles: AccountProfile[] = [profile()]) {
      return {
        ...fakeClient(profiles),
        schedulingOverview: vi.fn(async () => schedulingOverview({ accounts })),
      };
    }

    it("每個帳號顯示「進行中 n/上限」與進行中的 Run（Task 標題、狀態）", async () => {
      const client = withOverview([
        accountScheduling({
          running: 2,
          limit: 2,
          runs: [
            accountRun({ run_id: "run_1", task_title: "整理資料", status: "running" }),
            accountRun({ run_id: "run_2", task_title: "寫報告", status: "starting", generation: 2 }),
          ],
        }),
      ]);

      render(<AccountsPage client={client} />);

      const item = (await screen.findByText("主要帳號")).closest("li")!;
      const load = await within(item).findByRole("group", { name: "主要帳號 的進行中 Run" });
      expect(within(load).getByText("進行中 2/2")).toBeInTheDocument();
      expect(within(load).getByText("整理資料（第 1 次）：執行中")).toBeInTheDocument();
      expect(within(load).getByText("寫報告（第 2 次）：啟動中")).toBeInTheDocument();
    });

    it("已要求取消的 Run 標示「已要求取消」，沒有的不標示", async () => {
      const client = withOverview([
        accountScheduling({
          running: 2,
          runs: [
            accountRun({ run_id: "run_1", task_title: "整理資料", cancel_requested: true }),
            accountRun({ run_id: "run_2", task_title: "寫報告" }),
          ],
        }),
      ]);

      render(<AccountsPage client={client} />);

      expect(await screen.findByText("整理資料（第 1 次）：執行中，已要求取消")).toBeInTheDocument();
      expect(screen.getByText("寫報告（第 1 次）：執行中")).toBeInTheDocument();
    });

    it("沒有進行中的 Run 時顯示 0/上限，不列出 Run", async () => {
      render(<AccountsPage client={withOverview([accountScheduling({ running: 0, limit: 3 })])} />);

      const load = await screen.findByRole("group", { name: "主要帳號 的進行中 Run" });
      expect(within(load).getByText("進行中 0/3")).toBeInTheDocument();
      expect(within(load).queryByRole("list")).not.toBeInTheDocument();
    });

    it("用量未知顯示「未知」而不是 0；已知時顯示百分比", async () => {
      const client = withOverview(
        [
          accountScheduling({ quota_used: null }),
          accountScheduling({
            profile_id: "prof_2",
            name: "備用帳號",
            quota_used: 0.42,
            quota: quotaAssessment({ used: 0.42, source: "observed", confidence: "observed" }),
          }),
        ],
        [profile(), profile({ id: "prof_2", name: "備用帳號" })],
      );

      render(<AccountsPage client={client} />);

      const unknown = await screen.findByRole("group", { name: "主要帳號 的額度" });
      expect(within(unknown).getByText("判斷用量：未知")).toBeInTheDocument();
      expect(within(unknown).queryByText(/\d+%/)).not.toBeInTheDocument();
      const known = screen.getByRole("group", { name: "備用帳號 的額度" });
      expect(within(known).getByText("判斷用量：42%")).toBeInTheDocument();
    });

    it("總覽沒有這個帳號時不顯示並行資訊", async () => {
      const client = withOverview([]);
      render(<AccountsPage client={client} />);

      await screen.findByText("主要帳號");
      await waitFor(() => expect(client.schedulingOverview).toHaveBeenCalled());
      expect(screen.queryByText(/進行中 \d+\/\d+/)).not.toBeInTheDocument();
    });

    it("讀不到排程總覽時說明原因，帳號清單照常顯示", async () => {
      const client = withOverview([]);
      client.schedulingOverview.mockRejectedValueOnce(new Error("connection refused"));

      render(<AccountsPage client={client} />);

      expect(await screen.findByRole("alert")).toHaveTextContent("無法取得排程總覽：connection refused");
      expect(screen.getByText("主要帳號")).toBeInTheDocument();
    });

    it("上層要求重新讀取時，並行資訊一併更新", async () => {
      const client = withOverview([accountScheduling({ running: 0 })]);
      const { rerender } = render(<AccountsPage client={client} refreshKey={0} />);
      expect(await screen.findByText("進行中 0/2")).toBeInTheDocument();

      client.schedulingOverview.mockResolvedValue(
        schedulingOverview({ accounts: [accountScheduling({ running: 1, runs: [accountRun()] })] }),
      );
      rerender(<AccountsPage client={client} refreshKey={1} />);

      expect(await screen.findByText("進行中 1/2")).toBeInTheDocument();
    });
  });

  describe("額度與方案上限", () => {
    /** 帶排程總覽與方案上限設定的 client；`setQuotaLimits` 回傳上限已更新的總覽。 */
    function withQuota(account: AccountScheduling = accountScheduling()) {
      return {
        ...fakeClient([profile()]),
        schedulingOverview: vi.fn(async () => schedulingOverview({ accounts: [account] })),
        setQuotaLimits: vi.fn(async (_id: string, limits: PlanLimits) =>
          schedulingOverview({ accounts: [{ ...account, plan_limits: limits }] }),
        ),
      };
    }

    /** 2023-11-15 01:13:20 UTC（台北 09:13），比觀測時間晚三小時。 */
    const RESET_AT = CHECKED_AT + 3 * 60 * 60 * 1000;

    afterEach(() => {
      vi.useRealTimers();
    });

    it("完全未知時判斷用量顯示「未知」而非 0%，並說明重置時間未知、尚未觀測", async () => {
      render(<AccountsPage client={withQuota()} />);

      const quota = await screen.findByRole("group", { name: "主要帳號 的額度" });
      expect(within(quota).getByText("判斷用量：未知")).toBeInTheDocument();
      expect(within(quota).getByText("來源：未知")).toBeInTheDocument();
      expect(within(quota).getByText("可信度：未知")).toBeInTheDocument();
      expect(within(quota).getByText("重置時間未知")).toBeInTheDocument();
      expect(within(quota).getByText("尚未觀測")).toBeInTheDocument();
      expect(within(quota).queryByText(/\d+%/)).not.toBeInTheDocument();
      const [fiveHour, sevenDay] = within(quota).getAllByRole("listitem");
      expect(fiveHour).toHaveTextContent("五小時已觀測：未知推估：未知（未設定方案上限）");
      expect(sevenDay).toHaveTextContent("七日已觀測：未知推估：未知（未設定方案上限）");
    });

    it("已觀測與推估並存時顯示判斷用量、兩個來源各自的數值與當地的重置、觀測時間", async () => {
      const observed = quotaObservation({ used: 0.3, reset_at: RESET_AT });
      const estimated = quotaObservation({
        origin: "estimate",
        source: "estimated",
        status: null,
        used: 0.45,
        tokens: 90_000,
        limit_tokens: 200_000,
        reset_at: RESET_AT,
        confidence: "estimated",
        raw_ref: "quota_usage:seq=1..4",
      });
      const weekly = quotaObservation({
        origin: "estimate",
        source: "estimated",
        window: "seven_day",
        status: null,
        used: null,
        tokens: 120_000,
        confidence: "unknown",
        raw_ref: "quota_usage:seq=1..4",
      });
      const account = accountScheduling({
        quota_used: 0.45,
        plan_limits: { five_hour_tokens: 200_000, seven_day_tokens: null },
        quota: quotaAssessment({
          used: 0.45,
          source: "estimated",
          confidence: "estimated",
          reset_at: RESET_AT,
          observed_at: CHECKED_AT,
          windows: [
            { window: "five_hour", label: "五小時", effective: estimated, observed, estimated },
            { window: "seven_day", label: "七日", effective: null, observed: null, estimated: weekly },
          ],
        }),
      });

      render(<AccountsPage client={withQuota(account)} timeZone="Asia/Taipei" />);

      const quota = await screen.findByRole("group", { name: "主要帳號 的額度" });
      expect(within(quota).getByText("判斷用量：45%")).toBeInTheDocument();
      expect(within(quota).getByText("來源：推估")).toBeInTheDocument();
      expect(within(quota).getByText("可信度：推估")).toBeInTheDocument();
      expect(within(quota).getByText("重置時間：2023/11/15 09:13")).toBeInTheDocument();
      expect(within(quota).getByText("觀測時間：2023/11/15 06:13")).toBeInTheDocument();
      const [fiveHour, sevenDay] = within(quota).getAllByRole("listitem");
      expect(within(fiveHour).getByText("已觀測：30%")).toBeInTheDocument();
      expect(fiveHour).toHaveTextContent("推估：45%（本機累計 90,000 token；方案上限 200,000 token）");
      expect(within(sevenDay).getByText("已觀測：未知")).toBeInTheDocument();
      expect(sevenDay).toHaveTextContent("推估：未知（本機累計 120,000 token；未設定方案上限）");
    });

    it("CLI 只回報已達上限或警告而沒有數字時，照實說明而不是顯示百分比", async () => {
      const exhausted = quotaObservation({ status: "exhausted", used: null, confidence: "observed" });
      const warning = quotaObservation({ window: "seven_day", status: "warning", used: null, confidence: "unknown" });
      const account = accountScheduling({
        quota_used: 1,
        quota: quotaAssessment({
          used: 1,
          source: "observed",
          confidence: "observed",
          observed_at: CHECKED_AT,
          windows: [
            { window: "five_hour", label: "五小時", effective: exhausted, observed: exhausted, estimated: null },
            { window: "seven_day", label: "七日", effective: warning, observed: warning, estimated: null },
          ],
        }),
      });

      render(<AccountsPage client={withQuota(account)} />);

      const quota = await screen.findByRole("group", { name: "主要帳號 的額度" });
      const [fiveHour, sevenDay] = within(quota).getAllByRole("listitem");
      expect(within(fiveHour).getByText("已觀測：已達上限")).toBeInTheDocument();
      expect(within(sevenDay).getByText("已觀測：接近上限（CLI 警告）")).toBeInTheDocument();
    });

    it("判斷用量只來自沒有數字的 CLI 警告時，標明那是保守的下限", async () => {
      const warning = quotaObservation({ status: "warning", used: null, confidence: "unknown" });
      const account = accountScheduling({
        quota_used: 0.75,
        quota: quotaAssessment({
          used: 0.75,
          source: "observed",
          confidence: "unknown",
          observed_at: CHECKED_AT,
          windows: [
            { window: "five_hour", label: "五小時", effective: warning, observed: warning, estimated: null },
            { window: "seven_day", label: "七日", effective: null, observed: null, estimated: null },
          ],
        }),
      });

      render(<AccountsPage client={withQuota(account)} />);

      const quota = await screen.findByRole("group", { name: "主要帳號 的額度" });
      expect(within(quota).getByText(/判斷用量/)).toHaveTextContent("判斷用量：75%（CLI 警告，至少此值）");
    });

    it("觀測不完整時以提示說明原因；完整時不顯示提示", async () => {
      const client = {
        ...withQuota(),
        schedulingOverview: vi.fn(async () =>
          schedulingOverview({
            accounts: [
              accountScheduling(),
              accountScheduling({
                profile_id: "prof_2",
                name: "備用帳號",
                quota: quotaAssessment({ complete: true, incomplete_reason: null }),
              }),
            ],
          }),
        ),
        listProfiles: vi.fn(async () => ({ profiles: [profile(), profile({ id: "prof_2", name: "備用帳號" })], limit: 8 })),
      };

      render(<AccountsPage client={client} />);

      const incomplete = await screen.findByRole("group", { name: "主要帳號 的額度" });
      const note = within(incomplete).getByRole("note");
      expect(note).toHaveClass("quota-incomplete");
      expect(note).toHaveTextContent(/^觀測不完整：五小時、七日視窗沒有來自 Claude Code 的已觀測用量/);
      const complete = screen.getByRole("group", { name: "備用帳號 的額度" });
      expect(within(complete).queryByRole("note")).not.toBeInTheDocument();
    });

    it("方案上限表單預先填入目前設定；送出時空白送 null，並以回傳的總覽更新畫面", async () => {
      const client = withQuota(accountScheduling({ plan_limits: { five_hour_tokens: null, seven_day_tokens: 3_000_000 } }));
      const user = userEvent.setup();
      render(<AccountsPage client={client} />);
      const quota = await screen.findByRole("group", { name: "主要帳號 的額度" });
      expect(within(quota).getByLabelText("七日方案上限")).toHaveValue(3_000_000);

      await user.type(within(quota).getByLabelText("五小時方案上限"), "200000");
      await user.clear(within(quota).getByLabelText("七日方案上限"));
      await user.click(within(quota).getByRole("button", { name: "儲存 主要帳號 的方案上限" }));

      expect(client.setQuotaLimits).toHaveBeenCalledWith("prof_1", { five_hour_tokens: 200_000, seven_day_tokens: null });
      const [fiveHour, sevenDay] = within(quota).getAllByRole("listitem");
      await waitFor(() => expect(fiveHour).toHaveTextContent("方案上限 200,000 token"));
      expect(sevenDay).toHaveTextContent("未設定方案上限");
      expect(within(quota).queryByRole("alert")).not.toBeInTheDocument();
    });

    it("方案上限被拒絕時顯示 Runtime 的說明並保留輸入", async () => {
      const client = withQuota();
      client.setQuotaLimits.mockRejectedValueOnce(new Error("Profile 已封存，不能設定方案上限"));
      const user = userEvent.setup();
      render(<AccountsPage client={client} />);
      const quota = await screen.findByRole("group", { name: "主要帳號 的額度" });

      await user.type(within(quota).getByLabelText("五小時方案上限"), "500000");
      await user.click(within(quota).getByRole("button", { name: "儲存 主要帳號 的方案上限" }));

      expect(await within(quota).findByRole("alert")).toHaveTextContent("無法儲存方案上限：Profile 已封存，不能設定方案上限");
      expect(within(quota).getByLabelText("五小時方案上限")).toHaveValue(500_000);
    });

    it("方案上限不是正整數時輸入框標示無效，不送出", async () => {
      const client = withQuota();
      const user = userEvent.setup();
      render(<AccountsPage client={client} />);
      const quota = await screen.findByRole("group", { name: "主要帳號 的額度" });

      await user.type(within(quota).getByLabelText("五小時方案上限"), "1.5");
      await user.type(within(quota).getByLabelText("七日方案上限"), "0");
      await user.click(within(quota).getByRole("button", { name: "儲存 主要帳號 的方案上限" }));

      expect(within(quota).getByLabelText("五小時方案上限")).toBeInvalid();
      expect(within(quota).getByLabelText("七日方案上限")).toBeInvalid();
      expect(client.setQuotaLimits).not.toHaveBeenCalled();
    });

    it("沒有提供 setQuotaLimits 時不顯示方案上限表單", async () => {
      const { setQuotaLimits: _omitted, ...client } = withQuota();
      render(<AccountsPage client={client} />);

      const quota = await screen.findByRole("group", { name: "主要帳號 的額度" });
      expect(within(quota).queryByLabelText("五小時方案上限")).not.toBeInTheDocument();
      expect(within(quota).queryByRole("button")).not.toBeInTheDocument();
    });

    it("每 5 分鐘重新讀取排程總覽，不到 5 分鐘不讀取", async () => {
      vi.useFakeTimers({ shouldAdvanceTime: true });
      const client = withQuota();
      render(<AccountsPage client={client} />);
      await screen.findByText("判斷用量：未知");
      expect(client.schedulingOverview).toHaveBeenCalledTimes(1);

      client.schedulingOverview.mockResolvedValue(
        schedulingOverview({
          accounts: [
            accountScheduling({
              quota_used: 0.6,
              quota: quotaAssessment({ used: 0.6, source: "observed", confidence: "observed" }),
            }),
          ],
        }),
      );
      // 留一秒餘裕，抵銷 shouldAdvanceTime 隨真實時間前進的部分
      act(() => vi.advanceTimersByTime(QUOTA_REFRESH_INTERVAL_MS - 1000));
      expect(client.schedulingOverview).toHaveBeenCalledTimes(1);

      act(() => vi.advanceTimersByTime(1000));
      expect(client.schedulingOverview).toHaveBeenCalledTimes(2);
      expect(await screen.findByText("判斷用量：60%")).toBeInTheDocument();
      expect(client.listProfiles).toHaveBeenCalledTimes(1);
    });

    it("離開帳號頁後不再輪詢", async () => {
      vi.useFakeTimers({ shouldAdvanceTime: true });
      const client = withQuota();
      const { unmount } = render(<AccountsPage client={client} />);
      await screen.findByText("判斷用量：未知");

      unmount();
      act(() => vi.advanceTimersByTime(QUOTA_REFRESH_INTERVAL_MS * 2));

      expect(client.schedulingOverview).toHaveBeenCalledTimes(1);
    });
  });
});
