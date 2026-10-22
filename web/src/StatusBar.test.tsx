import { act, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { StatusBar } from "./StatusBar";
import type { RuntimeClient } from "./runtimeClient";

const health = { status: "ok", product: "Baton", version: "0.1.0", boot_count: 1, started_at: 0, pid: 1 };

function clientReturning(...results: Array<"ok" | Error>): RuntimeClient {
  const queue = [...results];
  return {
    getHealth: vi.fn(async () => {
      const next = queue.length > 1 ? queue.shift()! : queue[0];
      if (next instanceof Error) throw next;
      return health;
    }),
  };
}

/** 在假計時器下讓排隊中的 Promise 與 React 更新全部完成。 */
async function settle(ms = 0) {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });
}

describe("頂部狀態列", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it("Runtime 回應健康檢查時顯示「Runtime 運作中」", async () => {
    render(<StatusBar client={clientReturning("ok")} pollMs={5000} />);

    await settle();

    expect(screen.getByText("Runtime 運作中")).toBeInTheDocument();
  });

  it("尚未得到結果時顯示檢查中", () => {
    render(<StatusBar client={clientReturning("ok")} pollMs={5000} />);

    expect(screen.getByText("Runtime 檢查中…")).toBeInTheDocument();
  });

  it("連不上 Runtime 時顯示未連線與原因，不假裝運作中", async () => {
    render(<StatusBar client={clientReturning(new Error("connection refused"))} pollMs={5000} />);

    await settle();

    expect(screen.getByText("Runtime 未連線")).toBeInTheDocument();
    expect(screen.getByText("connection refused")).toBeInTheDocument();
    expect(screen.queryByText("Runtime 運作中")).not.toBeInTheDocument();
  });

  it("定期重新檢查：Runtime 恢復後狀態列跟著更新", async () => {
    render(<StatusBar client={clientReturning(new Error("down"), "ok")} pollMs={5000} />);
    await settle();
    expect(screen.getByText("Runtime 未連線")).toBeInTheDocument();

    await settle(5000);

    expect(screen.getByText("Runtime 運作中")).toBeInTheDocument();
  });
});
