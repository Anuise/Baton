import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { schedulingOverview } from "./fixtures";
import { SchedulingSettings } from "./SchedulingSettings";
import type { SchedulingClient, SchedulingMode, SchedulingOverview } from "./runtimeClient";

/** 以記憶體模擬 Runtime 的容量預設：切換後總覽的 mode 與生效的容量一起改變。 */
function fakeClient(initial: SchedulingOverview = schedulingOverview()) {
  let current = initial;
  const client: SchedulingClient = {
    schedulingOverview: vi.fn(async () => current),
    setSchedulingMode: vi.fn(async (mode: SchedulingMode) => {
      const info = current.settings.modes.find((m) => m.mode === mode)!;
      current = {
        ...current,
        settings: { ...current.settings, mode, mode_label: info.label },
        capacity: {
          ...current.capacity,
          per_account_limit: info.per_account_limit,
          global_limit: info.global_limit,
          handoff_reserve: info.handoff_reserve,
          normal_limit: info.global_limit - info.handoff_reserve,
        },
      };
      return current;
    }),
  };
  return client;
}

describe("並行容量設定", () => {
  it("列出兩個選項與各自的說明，目前的預設已選取", async () => {
    render(<SchedulingSettings client={fakeClient()} />);

    const reserve = await screen.findByRole("radio", { name: /保留交接容量/ });
    const higher = screen.getByRole("radio", { name: /較高並行/ });
    expect(reserve).toBeChecked();
    expect(higher).not.toBeChecked();
    expect(screen.getByText("每帳號 2 個、全域 6 個並行 Run；帳號數達 3 個以上時保留 1 個交接槽")).toBeInTheDocument();
    expect(screen.getByText("每帳號 3 個、全域 8 個並行 Run，不保留交接槽")).toBeInTheDocument();
  });

  it("顯示目前使用量（進行中／一般派工上限）與保留的交接槽數", async () => {
    const overview = schedulingOverview();
    overview.capacity = { ...overview.capacity, running: 4, normal_limit: 5, handoff_reserve: 1, usable_accounts: 3 };

    render(<SchedulingSettings client={fakeClient(overview)} />);

    expect(await screen.findByText(/目前使用 4 \/ 5；保留交接槽 1 個/)).toBeInTheDocument();
  });

  it("切換選項後呼叫 setSchedulingMode，畫面改用新的預設與容量數字", async () => {
    const client = fakeClient();
    const user = userEvent.setup();
    render(<SchedulingSettings client={client} />);
    expect(await screen.findByText(/目前使用 0 \/ 6；保留交接槽 0 個/)).toBeInTheDocument();

    await user.click(screen.getByRole("radio", { name: /較高並行/ }));

    expect(client.setSchedulingMode).toHaveBeenCalledWith("higher_concurrency");
    await waitFor(() => expect(screen.getByRole("radio", { name: /較高並行/ })).toBeChecked());
    expect(screen.getByRole("radio", { name: /保留交接容量/ })).not.toBeChecked();
    expect(screen.getByText(/目前使用 0 \/ 8；保留交接槽 0 個/)).toBeInTheDocument();
  });

  it("切換失敗時顯示原因，仍停在原本的預設", async () => {
    const client = fakeClient();
    vi.mocked(client.setSchedulingMode).mockRejectedValueOnce(new Error("Runtime 拒絕連線"));
    const user = userEvent.setup();
    render(<SchedulingSettings client={client} />);
    await screen.findByRole("radio", { name: /保留交接容量/ });

    await user.click(screen.getByRole("radio", { name: /較高並行/ }));

    expect(await screen.findByRole("alert")).toHaveTextContent("Runtime 拒絕連線");
    expect(screen.getByRole("radio", { name: /保留交接容量/ })).toBeChecked();
  });

  it("讀不到設定時顯示原因，不顯示選項", async () => {
    const client = fakeClient();
    vi.mocked(client.schedulingOverview).mockRejectedValueOnce(new Error("connection refused"));

    render(<SchedulingSettings client={client} />);

    expect(await screen.findByRole("alert")).toHaveTextContent("connection refused");
    expect(screen.queryByRole("radio")).not.toBeInTheDocument();
  });
});
