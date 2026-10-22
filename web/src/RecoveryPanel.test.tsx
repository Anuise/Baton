import { act, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import { CHECKED_AT, recoveryAttempt, recoveryEvent, recoveryStatus } from "./fixtures";
import { RecoveryPanel } from "./RecoveryPanel";
import type { RecoveryClient, RecoveryStatusView } from "./runtimeClient";

/** 輪詢依序回傳 `polls`（最後一個重複）；重新嘗試恢復回傳 `onRetry`。 */
function fakeClient(polls: Array<RecoveryStatusView | Error>, onRetry: RecoveryStatusView | Error = recoveryStatus()) {
  const queue = [...polls];
  const client = {
    recoveryStatus: vi.fn(async () => {
      const next = queue.length > 1 ? queue.shift()! : queue[0];
      if (next instanceof Error) throw next;
      return next;
    }),
    retryRecovery: vi.fn(async () => {
      if (onRetry instanceof Error) throw onRetry;
      return onRetry;
    }),
  } satisfies RecoveryClient;
  return client;
}

// CHECKED_AT 是 2023-11-14 22:13:20 UTC，以台北時間顯示為 2023/11/15 06:13
const renderPanel = (client: RecoveryClient, pollMs = 5000) =>
  render(<RecoveryPanel client={client} pollMs={pollMs} timeZone="Asia/Taipei" />);

const blocking = (code: string, message: string) => ({ category: "lifecycle" as const, code, message });

/** 第 1 輪三次都失敗、恢復額度用盡。 */
const exhausted = recoveryStatus({
  can_retry: true,
  blocking: blocking("crash_recovery_exhausted", "崩潰恢復額度用盡，等待重新嘗試恢復"),
  event: recoveryEvent({
    status: "exhausted",
    attempts_used: 3,
    last_failure: "無法確認舊程序停止",
    attempts: [1, 2, 3].map((seq) =>
      recoveryAttempt({
        id: `rcv_${seq}`,
        seq,
        outcome: "failed",
        finished_at: CHECKED_AT + seq * 60_000,
        failure: "無法確認舊程序停止",
        affected: [
          { run_id: "run_1", workspace_id: "ws_1", task_id: "task_1", disposition: "unconfirmed", message: "程序仍在執行" },
        ],
      }),
    ),
  }),
});

describe("崩潰恢復面板", () => {
  afterEach(() => vi.useRealTimers());

  it("沒有恢復事件時顯示簡短說明，不提供重新嘗試", async () => {
    renderPanel(fakeClient([recoveryStatus()]));

    expect(await screen.findByText("沒有恢復事件")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "重新嘗試恢復" })).not.toBeInTheDocument();
  });

  it("退避中顯示輪次、本輪次數、退避期限、最後失敗原因與派工阻擋", async () => {
    const status = recoveryStatus({
      blocking: blocking("crash_recovery_pending", "崩潰恢復進行中"),
      event: recoveryEvent({
        status: "waiting",
        attempts_used: 1,
        next_attempt_at: CHECKED_AT + 120_000,
        last_failure: "Runtime 再次終止",
        attempts: [recoveryAttempt({ outcome: "failed", finished_at: CHECKED_AT + 60_000, failure: "Runtime 再次終止" })],
      }),
    });
    renderPanel(fakeClient([status]));

    expect(await screen.findByText("退避中")).toBeInTheDocument();
    expect(screen.getByText("第 1 輪")).toBeInTheDocument();
    expect(screen.getByText("本輪嘗試：1/3")).toBeInTheDocument();
    expect(screen.getByText("偵測到崩潰：2023/11/15 06:13")).toBeInTheDocument();
    expect(screen.getByText("下一次嘗試最早：2023/11/15 06:15")).toBeInTheDocument();
    expect(screen.getByText("最後失敗原因：Runtime 再次終止")).toBeInTheDocument();
    expect(screen.getByRole("alert")).toHaveTextContent("新派工已暫停：崩潰恢復進行中");
    expect(screen.queryByRole("button", { name: "重新嘗試恢復" })).not.toBeInTheDocument();
  });

  it("第一次嘗試之前顯示等待嘗試", async () => {
    renderPanel(fakeClient([recoveryStatus({ event: recoveryEvent({ status: "waiting", attempts_used: 0, attempts: [] }) })]));

    expect(await screen.findByText("等待嘗試")).toBeInTheDocument();
    expect(screen.getByText("本輪嘗試：0/3")).toBeInTheDocument();
  });

  it("穩定觀察中顯示觀察期滿時間，以及每次嘗試與受影響工作的處置", async () => {
    const status = recoveryStatus({
      event: recoveryEvent({
        status: "observing",
        observe_until: CHECKED_AT + 600_000,
        attempts: [
          recoveryAttempt({
            outcome: "observing",
            affected: [
              { run_id: "run_1", workspace_id: "ws_1", task_id: "task_1", disposition: "requeued", message: null },
              { run_id: "run_2", workspace_id: "ws_1", task_id: "task_2", disposition: "cancelled", message: "崩潰前已要求取消" },
            ],
          }),
        ],
      }),
    });
    renderPanel(fakeClient([status]));

    expect(await screen.findByText("穩定觀察中", { selector: "strong" })).toBeInTheDocument();
    expect(screen.getByText("觀察期滿：2023/11/15 06:23")).toBeInTheDocument();
    const attempts = within(screen.getByRole("list", { name: "恢復嘗試" }));
    expect(attempts.getByText("第 1 輪第 1 次・穩定觀察中・開始 2023/11/15 06:13")).toBeInTheDocument();
    const affected = within(screen.getByRole("list", { name: "第 1 輪第 1 次的受影響工作" })).getAllByRole("listitem");
    expect(affected.map((item) => item.textContent)).toEqual([
      "Run run_1・Task task_1・已重新排隊",
      "Run run_2・Task task_2・已完成取消（崩潰前已要求取消）",
    ]);
  });

  it("恢復成功後顯示已恢復與完成時間", async () => {
    const status = recoveryStatus({
      event: recoveryEvent({
        status: "succeeded",
        closed_at: CHECKED_AT + 660_000,
        attempts: [recoveryAttempt({ outcome: "succeeded" })],
      }),
    });
    renderPanel(fakeClient([status]));

    expect(await screen.findByText("已恢復")).toBeInTheDocument();
    expect(screen.getByText("恢復完成：2023/11/15 06:24")).toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("恢復額度用盡時列出三次失敗與無法確認停止的工作，並提供重新嘗試恢復", async () => {
    renderPanel(fakeClient([exhausted]));

    expect(await screen.findByText("恢復額度用盡")).toBeInTheDocument();
    expect(screen.getByText("本輪嘗試：3/3")).toBeInTheDocument();
    expect(screen.getByRole("alert")).toHaveTextContent("新派工已暫停：崩潰恢復額度用盡，等待重新嘗試恢復");
    expect(within(screen.getByRole("list", { name: "恢復嘗試" })).getAllByText(/^第 1 輪第 \d 次・失敗/)).toHaveLength(3);
    expect(
      within(screen.getByRole("list", { name: "第 1 輪第 3 次的受影響工作" })).getByText(
        "Run run_1・Task task_1・無法確認停止（程序仍在執行）",
      ),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "重新嘗試恢復" })).toBeEnabled();
  });

  it("按下重新嘗試恢復後以回應更新畫面：進入第 2 輪，舊輪次的嘗試保留為歷史", async () => {
    const user = userEvent.setup();
    const retried = recoveryStatus({
      blocking: blocking("crash_recovery_pending", "崩潰恢復進行中"),
      event: recoveryEvent({
        round: 2,
        status: "attempting",
        attempts_used: 1,
        last_failure: "無法確認舊程序停止",
        attempts: [...exhausted.event!.attempts, recoveryAttempt({ id: "rcv_4", round: 2, seq: 1 })],
      }),
    });
    const client = fakeClient([exhausted], retried);
    renderPanel(client, 60_000);

    await user.click(await screen.findByRole("button", { name: "重新嘗試恢復" }));

    expect(client.retryRecovery).toHaveBeenCalledTimes(1);
    expect(await screen.findByText("第 2 輪")).toBeInTheDocument();
    expect(screen.getByText("嘗試中")).toBeInTheDocument();
    expect(screen.getByText("本輪嘗試：1/3")).toBeInTheDocument();
    expect(screen.getByText("第 1 輪第 3 次・失敗・開始 2023/11/15 06:13・結束 2023/11/15 06:16")).toBeInTheDocument();
    expect(screen.getByText("第 2 輪第 1 次・處置中・開始 2023/11/15 06:13")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "重新嘗試恢復" })).not.toBeInTheDocument();
  });

  it("重新嘗試恢復失敗時顯示原因，按鈕仍可再按", async () => {
    const user = userEvent.setup();
    const client = fakeClient([exhausted], new Error("恢復額度尚未用盡"));
    renderPanel(client, 60_000);

    await user.click(await screen.findByRole("button", { name: "重新嘗試恢復" }));

    expect(await screen.findByText("無法重新嘗試恢復：恢復額度尚未用盡")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "重新嘗試恢復" })).toBeEnabled();
  });

  it("讀不到恢復狀態時顯示原因", async () => {
    renderPanel(fakeClient([new Error("connection refused")]));

    expect(await screen.findByRole("alert")).toHaveTextContent("無法取得恢復狀態：connection refused");
  });

  it("定期輪詢：Supervisor 推進後畫面跟著更新", async () => {
    vi.useFakeTimers();
    const observing = recoveryEvent({ status: "observing", observe_until: CHECKED_AT + 600_000 });
    const client = fakeClient([recoveryStatus({ event: recoveryEvent() }), recoveryStatus({ event: observing })]);
    renderPanel(client, 5000);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(screen.getByText("嘗試中")).toBeInTheDocument();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(5000);
    });

    expect(client.recoveryStatus).toHaveBeenCalledTimes(2);
    expect(screen.getByText("觀察期滿：2023/11/15 06:23")).toBeInTheDocument();
  });
});
