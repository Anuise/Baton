import { act, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  availableUpdateStatus,
  CHECKED_AT,
  lifecycleStatus,
  maintenance,
  maintenanceOutcome,
  updateStatus,
} from "./fixtures";
import type { UpdateStatusView } from "./runtimeClient";
import { UpdateNotice } from "./UpdateNotice";

/** 以記憶體模擬 Runtime：`updateStatus` 回傳 `status`；同意更新後開始維護（停止中）。 */
function fakeClient(status: UpdateStatusView = availableUpdateStatus()) {
  return {
    updateStatus: vi.fn(async () => status),
    applyUpdate: vi.fn(async (_version: string): Promise<UpdateStatusView> => ({ ...status, maintenance: maintenance() })),
    installUpdate: vi.fn(async () => {}),
    lifecycleStatus: vi.fn(async () => lifecycleStatus({ phase: "stopping", stop_intent_active: true })),
    quitApp: vi.fn(async () => {}),
  };
}

/** 在假計時器下讓排隊中的 Promise 與 React 更新全部完成。 */
async function settle(ms = 0) {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });
}

async function click(element: HTMLElement) {
  await act(async () => {
    element.click();
  });
}

/** 這次 Runtime 的啟動時間：在最近一次維護結束之前。 */
const STARTED_AT = CHECKED_AT + 30_000;

describe("更新通知", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(CHECKED_AT + 120_000);
  });
  afterEach(() => vi.useRealTimers());

  it("有新版本時顯示通知與可展開的更新說明，但不會自己開始更新", async () => {
    const client = fakeClient();
    render(<UpdateNotice client={client} runtimeStartedAt={STARTED_AT} />);
    await settle();

    expect(screen.getByText("Baton 0.2.0 可以更新")).toBeInTheDocument();
    expect(screen.getByText("更新說明")).toBeInTheDocument();
    expect(screen.getByText("修正完全退出的逾時")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "更新並重新開啟…" })).toBeInTheDocument();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();

    await settle(120_000);

    expect(client.applyUpdate).not.toHaveBeenCalled();
  });

  it("沒有更新說明時不顯示展開區塊", async () => {
    const status = availableUpdateStatus({ available: { version: "0.2.0", notes: null, published_at: null } });
    render(<UpdateNotice client={fakeClient(status)} runtimeStartedAt={STARTED_AT} />);
    await settle();

    expect(screen.getByText("Baton 0.2.0 可以更新")).toBeInTheDocument();
    expect(screen.queryByText("更新說明")).not.toBeInTheDocument();
  });

  it.each([
    ["已是最新版本", updateStatus()],
    ["檢查失敗（保留之前看到的版本）", updateStatus({ status: "failed", error: "逾時", available: availableUpdateStatus().available })],
    ["已有進行中的解除安裝", availableUpdateStatus({ maintenance: maintenance({ kind: "uninstall", target_version: null }) })],
  ])("%s時不顯示更新通知", async (_case, status) => {
    render(<UpdateNotice client={fakeClient(status)} runtimeStartedAt={STARTED_AT} />);
    await settle();

    expect(screen.queryByText(/可以更新/)).not.toBeInTheDocument();
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
  });

  it("每 60 秒重新讀取更新狀態，讀取失敗時不打擾、保留原本的通知", async () => {
    const client = fakeClient(updateStatus());
    client.updateStatus
      .mockResolvedValueOnce(updateStatus())
      .mockResolvedValueOnce(availableUpdateStatus())
      .mockRejectedValueOnce(new Error("connection refused"));
    render(<UpdateNotice client={client} runtimeStartedAt={STARTED_AT} />);
    await settle();
    expect(client.updateStatus).toHaveBeenCalledTimes(1);
    expect(screen.queryByText(/可以更新/)).not.toBeInTheDocument();

    await settle(59_000);
    expect(client.updateStatus).toHaveBeenCalledTimes(1);
    await settle(1_000);

    expect(client.updateStatus).toHaveBeenCalledTimes(2);
    expect(screen.getByText("Baton 0.2.0 可以更新")).toBeInTheDocument();

    await settle(60_000);

    expect(client.updateStatus).toHaveBeenCalledTimes(3);
    expect(screen.getByText("Baton 0.2.0 可以更新")).toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("按「更新並重新開啟…」先說明會發生什麼，取消不會更新", async () => {
    const client = fakeClient();
    render(<UpdateNotice client={client} runtimeStartedAt={STARTED_AT} />);
    await settle();

    await click(screen.getByRole("button", { name: "更新並重新開啟…" }));

    const dialog = screen.getByRole("dialog", { name: "更新到 Baton 0.2.0" });
    expect(dialog).toHaveTextContent("先下載並驗證安裝程式；失敗不會停止任何工作。");
    expect(dialog).toHaveTextContent("執行與「完全退出」相同的停止流程（最多 40 秒）。");
    expect(dialog).toHaveTextContent("備份資料。");
    expect(dialog).toHaveTextContent("安裝並重新開啟 Baton。");
    expect(dialog).toHaveTextContent(
      "重新開啟後依各 Workspace 的「完全退出後自動續跑」設定續跑，其餘等「繼續工作」。",
    );
    expect(dialog).toHaveTextContent("資料目錄不會被刪除。");

    await click(within(dialog).getByRole("button", { name: "取消" }));

    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(client.applyUpdate).not.toHaveBeenCalled();
  });

  it("確認後以看到的版本同意更新，下載並驗證期間顯示進度，開始維護後改顯示更新進度", async () => {
    const client = fakeClient();
    let finishDownload: (status: UpdateStatusView) => void = () => {};
    client.applyUpdate.mockImplementationOnce(
      () => new Promise<UpdateStatusView>((resolve) => (finishDownload = resolve)),
    );
    render(<UpdateNotice client={client} runtimeStartedAt={STARTED_AT} />);
    await settle();
    await click(screen.getByRole("button", { name: "更新並重新開啟…" }));

    await click(screen.getByRole("button", { name: "更新並重新開啟" }));

    expect(client.applyUpdate).toHaveBeenCalledWith("0.2.0");
    const dialog = screen.getByRole("dialog", { name: "更新到 Baton 0.2.0" });
    expect(dialog).toHaveTextContent("下載並驗證中…");
    expect(within(dialog).getByRole("button", { name: "更新並重新開啟" })).toBeDisabled();
    expect(within(dialog).getByRole("button", { name: "取消" })).toBeDisabled();

    await act(async () => finishDownload(availableUpdateStatus({ maintenance: maintenance() })));

    expect(screen.queryByRole("dialog", { name: "更新到 Baton 0.2.0" })).not.toBeInTheDocument();
    expect(screen.getByRole("dialog", { name: "更新 Baton" })).toHaveTextContent("停止執行中的工作");
    expect(screen.queryByText(/可以更新/)).not.toBeInTheDocument();

    // 維護中由更新進度輪詢；通知本身不再每 60 秒讀取
    const calls = client.updateStatus.mock.calls.length;
    await settle(60_000);
    expect(client.lifecycleStatus).toHaveBeenCalled();
    expect(client.updateStatus.mock.calls.length - calls).toBe(120);
  });

  it("下載或驗證失敗時顯示原因並說明工作沒有停止，可以再試", async () => {
    const client = fakeClient();
    client.applyUpdate.mockRejectedValueOnce(new Error("無法下載或驗證安裝程式（工作沒有停止）：簽章不符"));
    render(<UpdateNotice client={client} runtimeStartedAt={STARTED_AT} />);
    await settle();
    await click(screen.getByRole("button", { name: "更新並重新開啟…" }));

    await click(screen.getByRole("button", { name: "更新並重新開啟" }));

    const alert = screen.getByRole("alert");
    expect(alert).toHaveTextContent("更新沒有開始：無法下載或驗證安裝程式（工作沒有停止）：簽章不符");
    expect(alert).toHaveTextContent("工作沒有停止，照常進行。");
    expect(screen.queryByRole("dialog", { name: "更新 Baton" })).not.toBeInTheDocument();

    await click(screen.getByRole("button", { name: "更新並重新開啟" }));

    expect(client.applyUpdate).toHaveBeenCalledTimes(2);
    expect(screen.getByRole("dialog", { name: "更新 Baton" })).toBeInTheDocument();
  });

  it("畫面載入時已在更新維護中，直接顯示更新進度", async () => {
    render(
      <UpdateNotice client={fakeClient(availableUpdateStatus({ maintenance: maintenance() }))} runtimeStartedAt={STARTED_AT} />,
    );
    await settle();

    expect(screen.getByRole("dialog", { name: "更新 Baton" })).toHaveTextContent("從 0.1.0 更新到 0.2.0");
  });

  it("這次啟動前剛完成更新時顯示已更新，可以關閉", async () => {
    render(
      <UpdateNotice
        client={fakeClient(updateStatus({ current_version: "0.2.0", last_maintenance: maintenanceOutcome() }))}
        runtimeStartedAt={STARTED_AT}
      />,
    );
    await settle();

    expect(screen.getByText("已從 0.1.0 更新到 0.2.0")).toBeInTheDocument();

    await click(screen.getByRole("button", { name: "知道了" }));

    expect(screen.queryByText("已從 0.1.0 更新到 0.2.0")).not.toBeInTheDocument();
  });

  it("更新沒有套用時說明仍是原本的版本", async () => {
    const outcome = maintenanceOutcome({ running_version: "0.1.0", applied: false });
    render(<UpdateNotice client={fakeClient(updateStatus({ last_maintenance: outcome }))} runtimeStartedAt={STARTED_AT} />);
    await settle();

    expect(screen.getByText("更新沒有套用，仍是 0.1.0")).toBeInTheDocument();
  });

  it.each([
    ["維護在這次啟動之前就結束", maintenanceOutcome({ finished_at: STARTED_AT - 1 }), STARTED_AT],
    ["還不知道 Runtime 的啟動時間", maintenanceOutcome(), null],
    ["最近一次維護是解除安裝後重新安裝", maintenanceOutcome({ kind: "uninstall", applied: null }), STARTED_AT],
  ])("%s時不顯示更新結果", async (_case, outcome, startedAt) => {
    render(<UpdateNotice client={fakeClient(updateStatus({ last_maintenance: outcome }))} runtimeStartedAt={startedAt} />);
    await settle();

    expect(screen.queryByText(/已從|沒有套用/)).not.toBeInTheDocument();
  });
});
