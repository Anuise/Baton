import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { missingClaude, okTool, profile, setupStatus } from "./fixtures";
import { SetupWizard } from "./SetupWizard";
import type { AccountClient, SetupStatus } from "./runtimeClient";

type WizardClient = Pick<AccountClient, "getSetup" | "recheckSetup" | "acknowledgeBackgroundNotice">;

function fakeClient(initial: SetupStatus, afterRecheck?: SetupStatus): WizardClient {
  let current = initial;
  return {
    getSetup: vi.fn(async () => current),
    recheckSetup: vi.fn(async () => (current = afterRecheck ?? current)),
    acknowledgeBackgroundNotice: vi.fn(async () => {
      current = { ...current, background_notice: { ...current.background_notice, acknowledged_at: 1_700_000_000_000 } };
      return current;
    }),
  };
}

describe("首次啟動精靈", () => {
  it("缺少 Claude Code 時只顯示官方指令與重新檢查，不提供任何安裝或下載按鈕", async () => {
    const client = fakeClient(setupStatus({ claude_code: missingClaude, ready: false }));
    render(<SetupWizard client={client} />);

    const section = await screen.findByRole("region", { name: "Claude Code" });
    expect(within(section).getByText("缺少")).toBeInTheDocument();
    expect(within(section).getByText("irm https://claude.ai/install.ps1 | iex")).toBeInTheDocument();
    expect(within(section).getByText("winget install Anthropic.ClaudeCode")).toBeInTheDocument();
    expect(within(section).getByText(/Baton 不會代為下載或安裝/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /安裝|下載|執行/ })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "重新檢查" })).toBeInTheDocument();
  });

  it("版本過舊時顯示目前版本、最低版本與更新指令", async () => {
    const tooOld = {
      ...okTool,
      status: "too_old" as const,
      version: "2.0.9",
      minimum_version: "2.1.0",
      message: "Claude Code 版本 2.0.9 低於最低支援的 2.1.0。",
      guidance: [{ title: "更新 Claude Code", command: "claude update" }],
    };
    render(<SetupWizard client={fakeClient(setupStatus({ claude_code: tooOld, ready: false }))} />);

    const section = await screen.findByRole("region", { name: "Claude Code" });
    expect(within(section).getByText("版本過舊")).toBeInTheDocument();
    expect(within(section).getByText("目前版本 2.0.9（最低 2.1.0）")).toBeInTheDocument();
    expect(within(section).getByText("claude update")).toBeInTheDocument();
  });

  it("按「重新檢查」重新取得狀態：使用者自行安裝後缺項消失", async () => {
    const before = setupStatus({ claude_code: missingClaude, ready: false });
    const after = setupStatus();
    const client = fakeClient(before, after);
    const onChange = vi.fn();
    const user = userEvent.setup();
    render(<SetupWizard client={client} onChange={onChange} />);
    await screen.findByText("irm https://claude.ai/install.ps1 | iex");

    await user.click(screen.getByRole("button", { name: "重新檢查" }));

    await waitFor(() => expect(screen.queryByText("irm https://claude.ai/install.ps1 | iex")).not.toBeInTheDocument());
    expect(client.recheckSetup).toHaveBeenCalled();
    expect(onChange).toHaveBeenCalled();
    expect(screen.getByText("Claude Code 2.1.307 可用")).toBeInTheDocument();
  });

  it("列出每個 Profile 的登入與可派工狀態，需要處理的指向帳號頁", async () => {
    const loggedOut = profile({
      id: "prof_2",
      name: "尚未登入",
      verification: {
        checked_at: 1,
        logged_in: false,
        source: "none",
        source_detail: null,
        account: null,
        config_dir_matches: true,
        cli_version: "2.1.307 (Claude Code)",
      },
      dispatchable: false,
      blockers: [{ code: "profile_not_logged_in", message: "尚未登入；請在終端機完成原生登入後按「重新核對」" }],
    });
    render(<SetupWizard client={fakeClient(setupStatus({ profiles: [profile(), loggedOut], ready: true }))} />);

    const section = await screen.findByRole("region", { name: "帳號 Profile" });
    expect(within(section).getByText("主要帳號").closest("li")).toHaveTextContent("可派工");
    expect(within(section).getByText("尚未登入").closest("li")).toHaveTextContent("未登入");
    expect(within(section).getByText(/在下方「帳號」/)).toBeInTheDocument();
  });

  it("沒有任何 Profile 時引導新增", async () => {
    render(<SetupWizard client={fakeClient(setupStatus({ profiles: [], ready: false }))} />);

    expect(await screen.findByText(/還沒有帳號 Profile/)).toBeInTheDocument();
  });

  it("首次使用時顯示背景執行行為的告知，確認後不再顯示", async () => {
    const client = fakeClient(setupStatus());
    const user = userEvent.setup();
    render(<SetupWizard client={client} />);

    const notice = await screen.findByRole("region", { name: "背景執行" });
    expect(notice).toHaveTextContent("關閉視窗後，Baton 會在背景繼續執行已派出的工作");

    await user.click(within(notice).getByRole("button", { name: "我知道了" }));

    expect(client.acknowledgeBackgroundNotice).toHaveBeenCalled();
    await screen.findByText("首次設定已完成");
    expect(screen.queryByRole("region", { name: "背景執行" })).not.toBeInTheDocument();
  });

  it("全部完成且告知已確認時只顯示精簡的完成列，仍可重新檢查", async () => {
    const done = setupStatus({ background_notice: { text: "關閉視窗後…", acknowledged_at: 1_700_000_000_000 } });
    render(<SetupWizard client={fakeClient(done)} />);

    expect(await screen.findByText("首次設定已完成")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "重新檢查" })).toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "Claude Code" })).not.toBeInTheDocument();
  });

  it("讀不到狀態時顯示原因", async () => {
    const client = fakeClient(setupStatus());
    vi.mocked(client.getSetup).mockRejectedValueOnce(new Error("connection refused"));

    render(<SetupWizard client={client} />);

    expect(await screen.findByRole("alert")).toHaveTextContent("connection refused");
  });

  it("探測失敗（狀態未知）時顯示原因並提示重新檢查", async () => {
    const unknown = {
      ...okTool,
      status: "unknown" as const,
      version: null,
      message: "無法檢查 Claude Code：逾時",
      guidance: [],
    };
    render(<SetupWizard client={fakeClient(setupStatus({ claude_code: unknown, ready: false }))} />);

    const section = await screen.findByRole("region", { name: "Claude Code" });
    expect(within(section).getByText("無法確認")).toBeInTheDocument();
    expect(within(section).getByText(/逾時/)).toBeInTheDocument();
  });
});
