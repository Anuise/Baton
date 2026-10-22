import { act, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ExitProgress } from "./ExitProgress";
import { exitRun, lifecycleStatus } from "./fixtures";
import type { ExitReport, LifecycleClient, LifecycleStatus } from "./runtimeClient";

/** 要求完全退出的時間；期限是 40 秒後。 */
const REQUESTED_AT = 1_700_000_000_000;

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

function stopping(overrides: Partial<ExitReport> = {}): LifecycleStatus {
  return lifecycleStatus({ phase: "stopping", stop_intent_active: true, active_runs: 1, exit: report(overrides) });
}

function stopped(overrides: Partial<ExitReport> = {}): LifecycleStatus {
  return lifecycleStatus({
    phase: "stopped",
    stop_intent_active: true,
    exit: report({ completed_at: REQUESTED_AT + 5_000, ...overrides }),
  });
}

/** 以記憶體模擬殼與 Runtime：`emitFullExit` 相當於使用者在系統匣按「完全退出」；輪詢依序回傳 `polls`（最後一個重複）。 */
function fakeClient(onExit: LifecycleStatus, ...polls: Array<LifecycleStatus | Error>) {
  let handler: (() => void) | null = null;
  const queue = [...polls];
  const client = {
    requestFullExit: vi.fn(async () => onExit),
    lifecycleStatus: vi.fn(async () => {
      const next = queue.length > 1 ? queue.shift()! : queue[0];
      if (next instanceof Error) throw next;
      return next;
    }),
    quitApp: vi.fn(async () => {}),
    onFullExitRequested: vi.fn((h: () => void) => {
      handler = h;
      return () => {
        handler = null;
      };
    }),
    updateWorkspaceLifecycle: vi.fn(),
    resumeWorkspace: vi.fn(),
    updateWorkspaceLoginResume: vi.fn(),
    updateLoginStartup: vi.fn(),
  } satisfies LifecycleClient;
  return { client, emitFullExit: () => act(() => handler?.()) };
}

/** 在假計時器下讓排隊中的 Promise 與 React 更新全部完成。 */
async function settle(ms = 0) {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });
}

describe("完全退出進度", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(REQUESTED_AT);
  });
  afterEach(() => vi.useRealTimers());

  it("沒收到完全退出事件時不顯示，也不要求 Runtime 停止", async () => {
    const { client } = fakeClient(stopping());
    render(<ExitProgress client={client} />);
    await settle(2000);

    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(client.requestFullExit).not.toHaveBeenCalled();
  });

  it("收到事件後要求完全退出，每 500ms 輪詢並顯示階段、剩餘秒數與每個 Run 的結果", async () => {
    const { client, emitFullExit } = fakeClient(
      stopping({ runs: [exitRun(), exitRun({ run_id: "run_2", outcome: "not_started" })] }),
      stopping({
        runs: [exitRun({ outcome: "stopped_gracefully" }), exitRun({ run_id: "run_2", outcome: "not_started" })],
        notes: ["已停止新派工、交接與重試"],
      }),
    );
    render(<ExitProgress client={client} quitDelayMs={1000} />);

    await emitFullExit();
    await settle();

    const dialog = screen.getByRole("dialog", { name: "完全退出" });
    expect(client.requestFullExit).toHaveBeenCalledTimes(1);
    expect(dialog).toHaveTextContent("停止中");
    expect(dialog).toHaveTextContent("距期限剩餘 40 秒（上限 40 秒）");
    const runs = within(dialog).getByRole("list", { name: "Run 停止結果" });
    expect(within(runs).getByText("筆記 · run_1").closest("li")).toHaveTextContent("等待中");
    expect(within(runs).getByText("筆記 · run_2").closest("li")).toHaveTextContent("未啟動");
    expect(client.lifecycleStatus).not.toHaveBeenCalled();

    await settle(500);

    expect(client.lifecycleStatus).toHaveBeenCalledTimes(1);
    expect(within(runs).getByText("筆記 · run_1").closest("li")).toHaveTextContent("已停止");
    expect(dialog).toHaveTextContent("距期限剩餘 40 秒");
    expect(dialog).toHaveTextContent("已停止新派工、交接與重試");

    await settle(500);

    expect(client.lifecycleStatus).toHaveBeenCalledTimes(2);
    expect(dialog).toHaveTextContent("距期限剩餘 39 秒");
    expect(client.quitApp).not.toHaveBeenCalled();
  });

  it("停止完成且沒有異常時顯示結果，約 1 秒後自動結束 App", async () => {
    const { client, emitFullExit } = fakeClient(
      stopping(),
      stopped({ runs: [exitRun({ outcome: "force_killed" })] }),
    );
    render(<ExitProgress client={client} quitDelayMs={1000} />);
    await emitFullExit();
    await settle(500);

    const dialog = screen.getByRole("dialog", { name: "完全退出" });
    expect(dialog).toHaveTextContent("已停止");
    expect(dialog).toHaveTextContent("耗時 5 秒");
    expect(within(dialog).getByText("筆記 · run_1").closest("li")).toHaveTextContent("已強制終止");
    expect(dialog).toHaveTextContent("所有程序都已停止，正在關閉 Baton…");
    expect(client.quitApp).not.toHaveBeenCalled();

    await settle(1000);

    expect(client.quitApp).toHaveBeenCalledTimes(1);
    // 已停止後不再輪詢
    expect(client.lifecycleStatus).toHaveBeenCalledTimes(1);
  });

  it("有停止異常時醒目列出並說明可能仍在背景執行，不自動結束，按「關閉 Baton」才結束", async () => {
    const { client, emitFullExit } = fakeClient(
      stopping(),
      stopped({
        runs: [
          exitRun({ outcome: "stopped_gracefully" }),
          exitRun({ run_id: "run_2", outcome: "anomaly", anomaly: "still_alive", message: "pid 4242 在期限後仍存在" }),
        ],
      }),
    );
    render(<ExitProgress client={client} quitDelayMs={1000} />);
    await emitFullExit();
    await settle(500);
    await settle(3000);

    const anomalies = screen.getByRole("alert");
    expect(anomalies).toHaveTextContent("停止異常 1 個");
    expect(anomalies).toHaveTextContent("這些程序無法確認停止，仍可能在背景執行");
    expect(anomalies).toHaveTextContent("筆記 · run_2：停止異常（程序仍在執行：pid 4242 在期限後仍存在）");
    expect(anomalies).not.toHaveTextContent("run_1");
    expect(client.quitApp).not.toHaveBeenCalled();

    await act(async () => {
      within(anomalies).getByRole("button", { name: "關閉 Baton" }).click();
    });

    expect(client.quitApp).toHaveBeenCalledTimes(1);
  });

  it("輪詢時連不上 Runtime 視為已結束，結束 App", async () => {
    const { client, emitFullExit } = fakeClient(stopping(), new Error("connection refused"));
    render(<ExitProgress client={client} quitDelayMs={1000} />);
    await emitFullExit();
    await settle(500);

    expect(screen.getByRole("dialog")).toHaveTextContent("Runtime 已結束，正在關閉 Baton…");

    await settle(1000);

    expect(client.quitApp).toHaveBeenCalledTimes(1);
  });

  it("要求完全退出失敗時顯示原因，不結束 App；可以重試", async () => {
    const { client, emitFullExit } = fakeClient(stopping(), stopping());
    client.requestFullExit.mockRejectedValueOnce(new Error("Runtime 未回應"));
    render(<ExitProgress client={client} />);
    await emitFullExit();
    await settle();

    expect(screen.getByRole("alert")).toHaveTextContent("無法開始完全退出：Runtime 未回應");
    await settle(2000);
    expect(client.quitApp).not.toHaveBeenCalled();

    await act(async () => {
      screen.getByRole("button", { name: "重試" }).click();
    });
    await settle();

    expect(client.requestFullExit).toHaveBeenCalledTimes(2);
    expect(screen.getByRole("dialog")).toHaveTextContent("停止中");
  });

  it("退出進行中再收到事件不會重新要求", async () => {
    const { client, emitFullExit } = fakeClient(stopping(), stopping());
    render(<ExitProgress client={client} />);
    await emitFullExit();
    await settle();

    await emitFullExit();
    await settle();

    expect(client.requestFullExit).toHaveBeenCalledTimes(1);
  });

  it("卸載時取消訂閱完全退出事件", () => {
    const unsubscribe = vi.fn();
    const { client } = fakeClient(stopping());
    client.onFullExitRequested.mockReturnValue(unsubscribe);
    const { unmount } = render(<ExitProgress client={client} />);

    unmount();

    expect(unsubscribe).toHaveBeenCalledTimes(1);
  });
});
