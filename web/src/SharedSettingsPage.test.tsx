import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { SharedSettingsPage } from "./SharedSettingsPage";
import { importPreview, settingsVersionSummary } from "./fixtures";
import type { SettingsVersion, SettingsVersionSummary, SharedSettingsClient } from "./runtimeClient";

type PageClient = Pick<SharedSettingsClient, "getSharedSettingsStatus" | "previewSharedSettings" | "adoptSharedSettings">;

/** 以記憶體模擬 Runtime 的已採用版本，讓測試只看使用者可見的行為。 */
function fakeClient(initial: SettingsVersionSummary[] = [], preview = importPreview()) {
  let versions = [...initial];
  const client: PageClient = {
    getSharedSettingsStatus: vi.fn(async () => ({ current: versions.at(-1) ?? null, versions: [...versions] })),
    previewSharedSettings: vi.fn(async () => preview),
    adoptSharedSettings: vi.fn(async () => {
      const created = settingsVersionSummary({
        version: versions.length + 1,
        content_hash: preview.candidate_hash,
        adopted_at: 1_700_100_000_000,
      });
      versions = [...versions, created];
      return {
        version: created.version,
        content_hash: created.content_hash,
        bundle: preview.bundle,
        sources: preview.sources,
        exclusions: preview.exclusions,
        adopted_at: created.adopted_at,
      } satisfies SettingsVersion;
    }),
  };
  return client;
}

describe("共同設定頁", () => {
  it("明講只讀取平常的 Claude Code 設定、不修改、不複製登入資料", async () => {
    render(<SharedSettingsPage client={fakeClient()} />);

    expect(screen.getByText(/只讀取你平常的 Claude Code 設定/)).toHaveTextContent(
      "不會修改它們，也不會複製任何登入資料",
    );
    await screen.findByText("尚未採用任何版本");
  });

  it("還沒有版本時顯示空狀態，且不預先載入預覽", async () => {
    const client = fakeClient();
    render(<SharedSettingsPage client={client} />);

    expect(await screen.findByText("尚未採用任何版本")).toBeInTheDocument();
    expect(client.previewSharedSettings).not.toHaveBeenCalled();
    expect(screen.queryByRole("button", { name: "採用" })).not.toBeInTheDocument();
  });

  it("列出已採用的版本（新的在前）：版本號、雜湊前 12 碼、採用時間、各類別項目數、排除數", async () => {
    const older = settingsVersionSummary({ version: 1, content_hash: "1111aaaabbbbccccdddd", adopted_at: 1_700_000_000_000 });
    const newer = settingsVersionSummary({
      version: 2,
      content_hash: "2222eeeeffff00001111",
      adopted_at: 1_700_100_000_000,
      counts: { preferences: 5, instructions: 0, hooks: 1, mcp_servers: 2, plugins: 3 },
      exclusion_count: 7,
    });
    render(<SharedSettingsPage client={fakeClient([older, newer])} timeZone="Asia/Taipei" />);

    const items = await screen.findAllByRole("listitem", { name: /^版本 \d+$/ });
    expect(items.map((item) => item.getAttribute("aria-label"))).toEqual(["版本 2", "版本 1"]);
    // 雜湊縮短為前 12 碼（加 … 表示截斷），完整值在 title
    expect(within(items[0]).getByText("2222eeeeffff…")).toHaveAttribute("title", "2222eeeeffff00001111");
    expect(within(items[0]).queryByText(/2222eeeeffff0000/)).not.toBeInTheDocument();
    expect(within(items[0]).getByText(/2023\/11\/16 10:00/)).toBeInTheDocument();
    expect(within(items[0]).getByText(/偏好 5/)).toBeInTheDocument();
    expect(within(items[0]).getByText(/指示 0/)).toBeInTheDocument();
    expect(within(items[0]).getByText(/hooks 1/)).toBeInTheDocument();
    expect(within(items[0]).getByText(/MCP 2/)).toBeInTheDocument();
    expect(within(items[0]).getByText(/plugins 3/)).toBeInTheDocument();
    expect(within(items[0]).getByText(/排除 7 項/)).toBeInTheDocument();
    expect(within(items[1]).getByText(/2023\/11\/15 06:13/)).toBeInTheDocument();
  });

  it("預覽匯入：顯示來源路徑與是否找到、被排除的項目與原因", async () => {
    const user = userEvent.setup();
    render(<SharedSettingsPage client={fakeClient()} />);
    await screen.findByText("尚未採用任何版本");

    await user.click(screen.getByRole("button", { name: "預覽匯入" }));

    const sources = await screen.findByRole("list", { name: "來源檔" });
    const settings = within(sources).getByText(/settings\.json/).closest("li")!;
    expect(within(settings).getByText("已找到")).toBeInTheDocument();
    const instructions = within(sources).getByText(/CLAUDE\.md/).closest("li")!;
    expect(within(instructions).getByText("找不到")).toBeInTheDocument();

    const exclusions = screen.getByRole("list", { name: "被排除的項目" });
    const login = within(exclusions).getByText(/oauthAccount/).closest("li")!;
    expect(login).toHaveTextContent("與登入／認證有關，不匯入");
    const secretKey = within(exclusions).getByText(/env\.MY_API_KEY/).closest("li")!;
    expect(secretKey).toHaveTextContent("欄位名稱像秘密，已略過");
  });

  it("三種排除原因各有自己的說明文案", async () => {
    const preview = importPreview({
      exclusions: [
        { source: "settings", item: "a", reason: "login_related" },
        { source: "settings", item: "b", reason: "secret_key" },
        { source: "settings", item: "c", reason: "secret_value" },
      ],
    });
    const user = userEvent.setup();
    render(<SharedSettingsPage client={fakeClient([], preview)} />);
    await user.click(await screen.findByRole("button", { name: "預覽匯入" }));

    const exclusions = await screen.findByRole("list", { name: "被排除的項目" });
    expect(exclusions).toHaveTextContent("與登入／認證有關，不匯入");
    expect(exclusions).toHaveTextContent("欄位名稱像秘密，已略過");
    expect(exclusions).toHaveTextContent("內容像秘密，整個項目不匯入");
  });

  it("沒有被排除項目時明說", async () => {
    const user = userEvent.setup();
    render(<SharedSettingsPage client={fakeClient([], importPreview({ exclusions: [] }))} />);
    await user.click(await screen.findByRole("button", { name: "預覽匯入" }));

    expect(await screen.findByText("沒有被排除的項目")).toBeInTheDocument();
  });

  it("差異依類別分組，標示新增、移除、變更", async () => {
    const user = userEvent.setup();
    render(<SharedSettingsPage client={fakeClient([settingsVersionSummary()])} />);
    await user.click(await screen.findByRole("button", { name: "預覽匯入" }));

    const preferences = (await screen.findByRole("heading", { name: "偏好" })).closest("section")!;
    const added = within(preferences).getByText("theme").closest("li")!;
    expect(added).toHaveTextContent("新增");
    expect(added).toHaveTextContent('"dark"');
    const changed = within(preferences).getByText("model").closest("li")!;
    expect(changed).toHaveTextContent("變更");
    expect(changed).toHaveTextContent('"sonnet"');
    expect(changed).toHaveTextContent('"opus"');

    const mcp = screen.getByRole("heading", { name: "MCP" }).closest("section")!;
    const removed = within(mcp).getByText("docs").closest("li")!;
    expect(removed).toHaveTextContent("移除");

    // 沒有差異的類別不顯示
    expect(screen.queryByRole("heading", { name: "hooks" })).not.toBeInTheDocument();
  });

  it("差異為空時說明與目前版本相同，並停用「採用」", async () => {
    const same = importPreview({ diff: [], changed: false });
    const user = userEvent.setup();
    render(<SharedSettingsPage client={fakeClient([settingsVersionSummary()], same)} />);
    await user.click(await screen.findByRole("button", { name: "預覽匯入" }));

    expect(await screen.findByText(/與目前版本相同/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "採用" })).toBeDisabled();
  });

  it("採用：帶預覽的雜湊建立新版本，清單更新並關閉預覽", async () => {
    const client = fakeClient([settingsVersionSummary()]);
    const user = userEvent.setup();
    render(<SharedSettingsPage client={client} />);
    await user.click(await screen.findByRole("button", { name: "預覽匯入" }));

    await user.click(await screen.findByRole("button", { name: "採用" }));

    expect(client.adoptSharedSettings).toHaveBeenCalledWith(importPreview().candidate_hash);
    expect(await screen.findByRole("listitem", { name: "版本 2" })).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent("已採用，建立版本 2");
    expect(screen.queryByRole("button", { name: "採用" })).not.toBeInTheDocument();
  });

  it("採用被 settings_source_changed 拒絕時，請使用者重新預覽，且不建立版本", async () => {
    const client = fakeClient([settingsVersionSummary()]);
    vi.mocked(client.adoptSharedSettings).mockRejectedValueOnce(
      Object.assign(new Error("來源設定在預覽之後已經改變；請重新預覽，確認差異後再採用"), {
        code: "settings_source_changed",
      }),
    );
    const user = userEvent.setup();
    render(<SharedSettingsPage client={client} />);
    await user.click(await screen.findByRole("button", { name: "預覽匯入" }));

    await user.click(await screen.findByRole("button", { name: "採用" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("來源在預覽後已改變，請重新預覽");
    expect(screen.queryByRole("listitem", { name: "版本 2" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "採用" })).not.toBeInTheDocument();

    // 重新預覽後可再採用
    await user.click(screen.getByRole("button", { name: "預覽匯入" }));
    await user.click(await screen.findByRole("button", { name: "採用" }));
    expect(await screen.findByRole("listitem", { name: "版本 2" })).toBeInTheDocument();
  });

  it("Tauri 形式的錯誤字串也能辨識為來源已改變", async () => {
    const client = fakeClient([settingsVersionSummary()]);
    vi.mocked(client.adoptSharedSettings).mockRejectedValueOnce(
      new Error("Runtime 拒絕連線（settings_source_changed）：來源設定在預覽之後已經改變"),
    );
    const user = userEvent.setup();
    render(<SharedSettingsPage client={client} />);
    await user.click(await screen.findByRole("button", { name: "預覽匯入" }));
    await user.click(await screen.findByRole("button", { name: "採用" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("來源在預覽後已改變，請重新預覽");
  });

  it("其他採用失敗顯示 Runtime 的說明，預覽保留", async () => {
    const client = fakeClient([settingsVersionSummary()]);
    vi.mocked(client.adoptSharedSettings).mockRejectedValueOnce(new Error("無法讀取使用者的設定檔：拒絕存取"));
    const user = userEvent.setup();
    render(<SharedSettingsPage client={client} />);
    await user.click(await screen.findByRole("button", { name: "預覽匯入" }));
    await user.click(await screen.findByRole("button", { name: "採用" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("拒絕存取");
    expect(screen.getByRole("button", { name: "採用" })).toBeEnabled();
  });

  it("來源檔格式錯誤時，預覽顯示 Runtime 的說明", async () => {
    const client = fakeClient();
    vi.mocked(client.previewSharedSettings).mockRejectedValueOnce(new Error("無法匯入設定：settings.json 不是有效的 JSON"));
    const user = userEvent.setup();
    render(<SharedSettingsPage client={client} />);
    await user.click(await screen.findByRole("button", { name: "預覽匯入" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("不是有效的 JSON");
  });

  it("讀不到版本清單時顯示原因，不假裝是空的", async () => {
    const client = fakeClient();
    vi.mocked(client.getSharedSettingsStatus).mockRejectedValueOnce(new Error("connection refused"));
    render(<SharedSettingsPage client={client} />);

    expect(await screen.findByRole("alert")).toHaveTextContent("connection refused");
    expect(screen.queryByText("尚未採用任何版本")).not.toBeInTheDocument();
  });

  it("預覽進行中不能重複送出", async () => {
    const client = fakeClient();
    let release: (value: ReturnType<typeof importPreview>) => void = () => {};
    vi.mocked(client.previewSharedSettings).mockReturnValueOnce(new Promise((resolve) => (release = resolve)));
    const user = userEvent.setup();
    render(<SharedSettingsPage client={client} />);
    await user.click(await screen.findByRole("button", { name: "預覽匯入" }));

    await waitFor(() => expect(screen.getByRole("button", { name: "預覽匯入" })).toBeDisabled());
    release(importPreview());
    await screen.findByRole("list", { name: "來源檔" });
    expect(screen.getByRole("button", { name: "預覽匯入" })).toBeEnabled();
  });
});
