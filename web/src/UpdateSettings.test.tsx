import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { availableUpdateStatus, updateStatus } from "./fixtures";
import type { UpdateClient, UpdateStatusView } from "./runtimeClient";
import { UpdateSettings } from "./UpdateSettings";

function fakeClient(status: UpdateStatusView = updateStatus()) {
  return { updateStatus: vi.fn<UpdateClient["updateStatus"]>(async () => status) };
}

// CHECKED_AT 是 2023-11-14 22:13:20 UTC，以台北時間顯示為 2023/11/15 06:13
const renderSettings = (client: ReturnType<typeof fakeClient>) =>
  render(<UpdateSettings client={client} timeZone="Asia/Taipei" />);

/** 兩欄 dl 中的一項，組成畫面上的說法「名稱：值」以便比對。 */
async function fact(term: string): Promise<string> {
  const dt = await screen.findByText(term, { selector: "dt" });
  return `${term}：${dt.nextElementSibling?.textContent ?? ""}`;
}

describe("更新設定", () => {
  it("顯示目前版本、上次與下次檢查時間，並說明只通知、不自動更新", async () => {
    renderSettings(fakeClient());

    expect(await fact("目前版本")).toBe("目前版本：0.1.0");
    expect(await fact("狀態")).toBe("狀態：已是最新版本");
    expect(await fact("上次檢查")).toBe("上次檢查：2023/11/15 06:13");
    expect(await fact("下次檢查")).toBe("下次檢查：2023/11/16 06:13");
    expect(screen.getByText("啟動時與每天自動檢查；有新版本只會通知，不會自動更新。")).toBeInTheDocument();
  });

  it.each([
    [updateStatus({ status: "unconfigured" }), "狀態：未設定更新來源（這個版本不會自動檢查）"],
    [availableUpdateStatus(), "狀態：有新版本 0.2.0"],
    [updateStatus({ status: "failed", error: "連不上更新伺服器" }), "狀態：檢查失敗：連不上更新伺服器（明天再試）"],
    [updateStatus({ status: "not_checked", checked_at: null, next_check_at: null }), "狀態：尚未檢查"],
  ])("依檢查結果顯示狀態文字（%#）", async (status, text) => {
    renderSettings(fakeClient(status));

    expect(await fact("狀態")).toBe(text);
  });

  it("還沒檢查過時，上次與下次檢查顯示為尚未檢查與未排定", async () => {
    renderSettings(fakeClient(updateStatus({ status: "not_checked", checked_at: null, next_check_at: null })));

    expect(await fact("上次檢查")).toBe("上次檢查：尚未檢查");
    expect(await fact("下次檢查")).toBe("下次檢查：未排定");
  });

  it("沒有「立即檢查」或任何操作按鈕", async () => {
    renderSettings(fakeClient(availableUpdateStatus()));

    expect(await fact("目前版本")).toBe("目前版本：0.1.0");
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
  });

  it("讀不到更新狀態時顯示原因", async () => {
    const client = fakeClient();
    client.updateStatus.mockRejectedValueOnce(new Error("connection refused"));

    renderSettings(client);

    expect(await screen.findByRole("alert")).toHaveTextContent("無法取得更新狀態：connection refused");
  });

  it("載入中先顯示提示", () => {
    renderSettings({ updateStatus: vi.fn(() => new Promise<UpdateStatusView>(() => {})) });

    expect(screen.getByText("載入中…")).toBeInTheDocument();
  });
});
