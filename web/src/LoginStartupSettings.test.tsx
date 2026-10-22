import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { lifecycleStatus } from "./fixtures";
import { LoginStartupSettings } from "./LoginStartupSettings";
import type { LoginStartupView } from "./runtimeClient";

/** 以記憶體模擬 Runtime 的「登入時自動啟動」：變更成功時 Windows 的登記跟著偏好。 */
function fakeClient(initial: LoginStartupView = lifecycleStatus().login_startup) {
  let current = initial;
  return {
    lifecycleStatus: vi.fn(async () => lifecycleStatus({ login_startup: current })),
    updateLoginStartup: vi.fn(async (enabled: boolean) => {
      current = { enabled, registered: enabled };
      return current;
    }),
  };
}

const SWITCH = { name: "登入 Windows 時自動啟動 Baton" };

describe("登入時自動啟動", () => {
  it("預設關閉，並說明完全退出後登入不會自動啟動", async () => {
    render(<LoginStartupSettings client={fakeClient()} />);

    expect(await screen.findByRole("checkbox", SWITCH)).not.toBeChecked();
    expect(screen.getByText(/完全退出後，登入時不會自動啟動，需手動開 App/)).toBeInTheDocument();
    expect(screen.queryByText(/登記遺失/)).not.toBeInTheDocument();
  });

  it("開啟時以 true 呼叫設定，再關閉時送出 false", async () => {
    const client = fakeClient();
    const user = userEvent.setup();
    render(<LoginStartupSettings client={client} />);
    const checkbox = await screen.findByRole("checkbox", SWITCH);

    await user.click(checkbox);

    expect(client.updateLoginStartup).toHaveBeenCalledWith(true);
    await waitFor(() => expect(checkbox).toBeChecked());

    await user.click(checkbox);

    expect(client.updateLoginStartup).toHaveBeenLastCalledWith(false);
    await waitFor(() => expect(checkbox).not.toBeChecked());
  });

  it("變更失敗（Windows 登記失敗）時顯示原因，開關回到原值", async () => {
    const client = fakeClient();
    vi.mocked(client.updateLoginStartup).mockRejectedValueOnce(new Error("無法變更 Windows 的登入啟動登記：拒絕存取"));
    const user = userEvent.setup();
    render(<LoginStartupSettings client={client} />);

    await user.click(await screen.findByRole("checkbox", SWITCH));

    expect(await screen.findByRole("alert")).toHaveTextContent("拒絕存取");
    expect(screen.getByRole("checkbox", SWITCH)).not.toBeChecked();
  });

  it("已開啟但 Windows 的登記遺失時提示會補回", async () => {
    render(<LoginStartupSettings client={fakeClient({ enabled: true, registered: false })} />);

    expect(await screen.findByRole("checkbox", SWITCH)).toBeChecked();
    expect(screen.getByText("Windows 的登記遺失，下次啟動 Runtime 時會補回")).toBeInTheDocument();
  });

  it("查不到 Windows 的登記時不顯示遺失提示", async () => {
    render(<LoginStartupSettings client={fakeClient({ enabled: true, registered: null })} />);

    expect(await screen.findByRole("checkbox", SWITCH)).toBeChecked();
    expect(screen.queryByText(/登記遺失/)).not.toBeInTheDocument();
  });

  it("讀不到設定時顯示原因，不顯示開關", async () => {
    const client = fakeClient();
    vi.mocked(client.lifecycleStatus).mockRejectedValueOnce(new Error("connection refused"));

    render(<LoginStartupSettings client={client} />);

    expect(await screen.findByRole("alert")).toHaveTextContent("connection refused");
    expect(screen.queryByRole("checkbox")).not.toBeInTheDocument();
  });
});
