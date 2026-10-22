import { act, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { CHECKED_AT, exitRun, lifecycleStatus, maintenance, updateStatus } from "./fixtures";
import type { ExitReport, LifecycleStatus, Maintenance, UpdateStatusView } from "./runtimeClient";
import { UpdateProgress } from "./UpdateProgress";

/** 開始維護的時間；停止流程的期限是 40 秒後。 */
const REQUESTED_AT = CHECKED_AT;

function report(overrides: Partial<ExitReport> = {}): ExitReport {
  return {
    requested_at: REQUESTED_AT,
    deadline_at: REQUESTED_AT + 40_000,
    completed_at: null,
    runs: [exitRun()],
    notes: [],
    ...overrides,
  };
}

const stopping = (overrides: Partial<ExitReport> = {}) =>
  lifecycleStatus({ phase: "stopping", stop_intent_active: true, active_runs: 1, exit: report(overrides) });

const stopped = (overrides: Partial<ExitReport> = {}) =>
  lifecycleStatus({
    phase: "stopped",
    stop_intent_active: true,
    exit: report({ completed_at: REQUESTED_AT + 5_000, runs: [exitRun({ outcome: "stopped_gracefully" })], ...overrides }),
  });

const inMaintenance = (overrides: Partial<Maintenance> = {}) => updateStatus({ maintenance: maintenance(overrides) });

type Poll = [UpdateStatusView, LifecycleStatus] | Error;

/** 以記憶體模擬 Runtime 與殼：每次輪詢依序回傳 `polls`（最後一個重複）。 */
function fakeClient(...polls: Poll[]) {
  const queue = [...polls];
  let current: Poll = queue[0];
  const next = () => {
    current = queue.length > 1 ? queue.shift()! : queue[0];
    return current;
  };
  return {
    updateStatus: vi.fn(async () => {
      const poll = next();
      if (poll instanceof Error) throw poll;
      return poll[0];
    }),
    lifecycleStatus: vi.fn(async () => {
      if (current instanceof Error) throw current;
      return current[1];
    }),
    installUpdate: vi.fn(async () => {}),
    quitApp: vi.fn(async () => {}),
  };
}

/** 在假計時器下讓排隊中的 Promise 與 React 更新全部完成。 */
async function settle(ms = 0) {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });
}

describe("更新進度", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(REQUESTED_AT);
  });
  afterEach(() => vi.useRealTimers());

  it("停止中：每 500ms 輪詢維護與停止進度，顯示目標版本、剩餘秒數與每個 Run 的結果", async () => {
    const client = fakeClient([inMaintenance(), stopping({ notes: ["已停止新派工、交接與重試"] })]);
    render(<UpdateProgress client={client} maintenance={maintenance()} />);
    await settle();

    const dialog = screen.getByRole("dialog", { name: "更新 Baton" });
    expect(dialog).toHaveTextContent("從 0.1.0 更新到 0.2.0");
    expect(dialog).toHaveTextContent("停止執行中的工作");
    expect(client.updateStatus).not.toHaveBeenCalled();

    await settle(500);

    expect(client.updateStatus).toHaveBeenCalledTimes(1);
    expect(client.lifecycleStatus).toHaveBeenCalledTimes(1);
    expect(dialog).toHaveTextContent("距期限剩餘 40 秒（上限 40 秒）");
    const runs = within(dialog).getByRole("list", { name: "Run 停止結果" });
    expect(within(runs).getByText("筆記 · run_1").closest("li")).toHaveTextContent("等待中");
    expect(dialog).toHaveTextContent("已停止新派工、交接與重試");

    await settle(500);

    expect(client.updateStatus).toHaveBeenCalledTimes(2);
    expect(client.installUpdate).not.toHaveBeenCalled();
  });

  it("工作都停止、維護仍在進行時說明正在備份資料", async () => {
    const client = fakeClient([inMaintenance(), stopped()]);
    render(<UpdateProgress client={client} maintenance={maintenance()} />);
    await settle(500);

    const dialog = screen.getByRole("dialog", { name: "更新 Baton" });
    expect(dialog).toHaveTextContent("備份資料");
    expect(dialog).toHaveTextContent("工作已停止，正在備份資料…");
    expect(within(dialog).getByText("筆記 · run_1").closest("li")).toHaveTextContent("已停止");
  });

  it("準備套用且沒有停止異常時自動安裝更新，之後不再輪詢", async () => {
    const client = fakeClient(
      [inMaintenance(), stopping()],
      [inMaintenance({ phase: "ready", backup_id: "bk_1", completed_at: REQUESTED_AT + 8_000 }), stopped()],
    );
    render(<UpdateProgress client={client} maintenance={maintenance()} />);
    await settle(500);
    expect(client.installUpdate).not.toHaveBeenCalled();

    await settle(500);

    expect(client.installUpdate).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("dialog")).toHaveTextContent("正在安裝更新並重新開啟…");

    await settle(3000);

    expect(client.updateStatus).toHaveBeenCalledTimes(2);
    expect(client.installUpdate).toHaveBeenCalledTimes(1);
    expect(client.quitApp).not.toHaveBeenCalled();
  });

  it("準備套用但有停止異常時列出異常，按「仍要安裝更新並重新開啟」才安裝", async () => {
    const client = fakeClient([
      inMaintenance({ phase: "ready" }),
      stopped({
        runs: [
          exitRun({ outcome: "stopped_gracefully" }),
          exitRun({ run_id: "run_2", outcome: "anomaly", anomaly: "still_alive", message: "pid 4242 在期限後仍存在" }),
        ],
      }),
    ]);
    render(<UpdateProgress client={client} maintenance={maintenance()} />);
    await settle(500);
    await settle(3000);

    const anomalies = screen.getByRole("alert");
    expect(anomalies).toHaveTextContent("停止異常 1 個");
    expect(anomalies).toHaveTextContent("筆記 · run_2：停止異常（程序仍在執行：pid 4242 在期限後仍存在）");
    expect(client.installUpdate).not.toHaveBeenCalled();

    await act(async () => {
      within(anomalies).getByRole("button", { name: "仍要安裝更新並重新開啟" }).click();
    });

    expect(client.installUpdate).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("dialog")).toHaveTextContent("正在安裝更新並重新開啟…");
  });

  it("無法執行安裝程式時顯示原因，可以重試或關閉 Baton", async () => {
    const client = fakeClient([inMaintenance({ phase: "ready" }), stopped()]);
    client.installUpdate
      .mockRejectedValueOnce(new Error("找不到安裝程式"))
      .mockRejectedValueOnce(new Error("維護尚未準備好"));
    render(<UpdateProgress client={client} maintenance={maintenance()} />);
    await settle(500);

    expect(screen.getByRole("alert")).toHaveTextContent("無法安裝更新：找不到安裝程式");

    await act(async () => {
      screen.getByRole("button", { name: "重試" }).click();
    });

    expect(client.installUpdate).toHaveBeenCalledTimes(2);
    expect(screen.getByRole("alert")).toHaveTextContent("無法安裝更新：維護尚未準備好");
    expect(client.quitApp).not.toHaveBeenCalled();

    await act(async () => {
      screen.getByRole("button", { name: "關閉 Baton" }).click();
    });
    expect(client.quitApp).toHaveBeenCalledTimes(1);
  });

  it("維護失敗時說明更新沒有套用、Baton 已完全退出，按「關閉 Baton」結束 App，不安裝", async () => {
    const client = fakeClient([inMaintenance({ phase: "failed", message: "更新前備份失敗：磁碟已滿" }), stopped()]);
    render(<UpdateProgress client={client} maintenance={maintenance()} />);
    await settle(500);
    await settle(2000);

    const dialog = screen.getByRole("dialog", { name: "更新 Baton" });
    expect(within(dialog).getByRole("alert")).toHaveTextContent("更新沒有套用：更新前備份失敗：磁碟已滿");
    expect(dialog).toHaveTextContent("Baton 已完全退出；重新開啟後會依各 Workspace 的設定繼續工作。");
    expect(client.installUpdate).not.toHaveBeenCalled();
    expect(client.updateStatus).toHaveBeenCalledTimes(1);

    await act(async () => {
      within(dialog).getByRole("button", { name: "關閉 Baton" }).click();
    });

    expect(client.quitApp).toHaveBeenCalledTimes(1);
  });

  it("輪詢失敗時說明暫時無法取得進度並繼續輪詢，恢復後清除說明", async () => {
    const client = fakeClient(new Error("connection refused"), [inMaintenance(), stopping()]);
    render(<UpdateProgress client={client} maintenance={maintenance()} />);
    await settle(500);

    expect(screen.getByRole("alert")).toHaveTextContent("暫時無法取得更新進度：connection refused");

    await settle(500);

    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(screen.getByRole("dialog")).toHaveTextContent("距期限剩餘 39 秒");
  });

  it("一開始就是準備套用（例如畫面重新載入）時先讀一次停止結果，沒有異常才自動安裝", async () => {
    const client = fakeClient([inMaintenance({ phase: "ready" }), stopped()]);
    render(<UpdateProgress client={client} maintenance={maintenance({ phase: "ready" })} />);
    await settle();
    expect(client.installUpdate).not.toHaveBeenCalled();

    await settle(500);
    await settle(2000);

    expect(client.installUpdate).toHaveBeenCalledTimes(1);
    expect(client.lifecycleStatus).toHaveBeenCalledTimes(1);
  });

  it("準備套用卻讀不到停止結果時不自動安裝，說明無法確認並由使用者決定", async () => {
    const client = fakeClient(new Error("connection refused"));
    render(<UpdateProgress client={client} maintenance={maintenance({ phase: "ready" })} />);
    await settle(500);

    const alert = screen.getByRole("alert");
    expect(alert).toHaveTextContent("無法確認工作是否都已停止：connection refused");
    expect(client.installUpdate).not.toHaveBeenCalled();

    await act(async () => {
      within(alert).getByRole("button", { name: "仍要安裝更新並重新開啟" }).click();
    });

    expect(client.installUpdate).toHaveBeenCalledTimes(1);
  });

  it("看到準備套用後讀不到停止結果時，不以停止中讀到的舊結果自動安裝", async () => {
    const client = fakeClient([inMaintenance(), stopped()], [inMaintenance({ phase: "ready" }), stopped()]);
    render(<UpdateProgress client={client} maintenance={maintenance()} />);
    await settle(500);
    expect(screen.getByRole("dialog")).toHaveTextContent("工作已停止，正在備份資料…");

    // 準備套用的那次輪詢讀不到停止進度
    client.lifecycleStatus.mockRejectedValueOnce(new Error("connection refused"));
    await settle(500);

    expect(client.installUpdate).not.toHaveBeenCalled();
    expect(screen.getByRole("alert")).toHaveTextContent("無法確認工作是否都已停止：無法取得停止進度");

    // 之後讀到最終的停止結果、沒有異常，才自動安裝
    await settle(500);

    expect(client.installUpdate).toHaveBeenCalledTimes(1);
  });
});
