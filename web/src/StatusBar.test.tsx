import { act, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { supportedPlatform, unofficialPlatform } from "./fixtures";
import { StatusBar } from "./StatusBar";
import type { HealthResponse, PlatformResponse, RuntimeClient } from "./runtimeClient";

const health = { status: "ok", product: "Baton", version: "0.1.0", boot_count: 1, started_at: 0, pid: 1 };

function clientReturning(...results: Array<"ok" | Error>): RuntimeClient {
  return clientWithPlatform(supportedPlatform, ...results);
}

function clientWithPlatform(platform: PlatformResponse | Error, ...results: Array<"ok" | Error>): RuntimeClient {
  const queue = [...results];
  return {
    getPlatform: vi.fn(async () => {
      if (platform instanceof Error) throw platform;
      return platform;
    }),
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

  it("顯示平台支援狀態", async () => {
    render(<StatusBar client={clientReturning("ok")} pollMs={5000} />);

    await settle();

    expect(screen.getByText("平台：正式支援")).toBeInTheDocument();
  });

  it("非正式支援時顯示警告，但 Runtime 仍顯示運作中", async () => {
    render(<StatusBar client={clientWithPlatform(unofficialPlatform, "ok")} pollMs={5000} />);

    await settle();

    expect(screen.getByText("平台：非正式支援")).toBeInTheDocument();
    expect(screen.getByText("Runtime 運作中")).toBeInTheDocument();
  });

  it("讀不到平台狀態時不影響 Runtime 狀態，平台顯示未知", async () => {
    render(<StatusBar client={clientWithPlatform(new Error("boom"), "ok")} pollMs={5000} />);

    await settle();

    expect(screen.getByText("平台：未知")).toBeInTheDocument();
    expect(screen.getByText("Runtime 運作中")).toBeInTheDocument();
  });

  it("顯示待裁定數，並在輪詢時更新", async () => {
    const client = {
      ...clientReturning("ok"),
      pendingDecisions: vi
        .fn()
        .mockResolvedValueOnce({ pending_count: 2, by_workspace: [{ workspace_id: "ws_1", pending_count: 2 }] })
        .mockResolvedValue({ pending_count: 0, by_workspace: [] }),
    };
    render(<StatusBar client={client} pollMs={5000} />);

    await settle();
    expect(screen.getByText("待裁定：2")).toBeInTheDocument();

    await settle(5000);
    expect(screen.getByText("待裁定：0")).toBeInTheDocument();
  });

  it("讀不到待裁定數時不顯示該欄位，也不影響 Runtime 狀態", async () => {
    const client = { ...clientReturning("ok"), pendingDecisions: vi.fn(async () => Promise.reject(new Error("boom"))) };
    render(<StatusBar client={client} pollMs={5000} />);

    await settle();

    expect(screen.queryByText(/待裁定/)).not.toBeInTheDocument();
    expect(screen.getByText("Runtime 運作中")).toBeInTheDocument();
  });

  it("refreshKey 改變時立即重新讀取待裁定數", async () => {
    const pendingDecisions = vi
      .fn()
      .mockResolvedValueOnce({ pending_count: 1, by_workspace: [] })
      .mockResolvedValue({ pending_count: 0, by_workspace: [] });
    const client = { ...clientReturning("ok"), pendingDecisions };
    const { rerender } = render(<StatusBar client={client} pollMs={60000} refreshKey={0} />);
    await settle();
    expect(screen.getByText("待裁定：1")).toBeInTheDocument();

    rerender(<StatusBar client={client} pollMs={60000} refreshKey={1} />);
    await settle();

    expect(screen.getByText("待裁定：0")).toBeInTheDocument();
  });

  describe("控制資料庫儲存故障", () => {
    const diskFull: HealthResponse["storage"] = {
      dispatch_halted: true,
      fault: {
        kind: "disk_full",
        code: "storage_disk_full",
        summary: "資料目錄所在的磁碟空間不足，控制資料庫無法寫入",
        action: "請釋放資料目錄所在磁碟的空間；空間足夠後 Baton 會自動恢復派工",
        detail: "database or disk is full",
      },
    };

    function clientWithStorage(...storages: Array<HealthResponse["storage"]>): RuntimeClient {
      const queue = [...storages];
      return {
        ...clientReturning("ok"),
        getHealth: vi.fn(async () => {
          const storage = queue.length > 1 ? queue.shift() : queue[0];
          return storage === undefined ? health : { ...health, storage };
        }),
      };
    }

    it("磁碟滿時顯示新派工已停止、原因與處理方式", async () => {
      render(<StatusBar client={clientWithStorage(diskFull)} pollMs={5000} />);

      await settle();

      const alert = screen.getByRole("alert");
      expect(alert).toHaveTextContent("新派工已停止");
      expect(alert).toHaveTextContent("資料目錄所在的磁碟空間不足，控制資料庫無法寫入");
      expect(alert).toHaveTextContent("請釋放資料目錄所在磁碟的空間；空間足夠後 Baton 會自動恢復派工");
      expect(alert).toHaveTextContent("database or disk is full");
      expect(screen.getByText("Runtime 運作中")).toBeInTheDocument();
    });

    it("舊版 Runtime 沒有 storage 欄位時不顯示警示", async () => {
      render(<StatusBar client={clientWithStorage(undefined)} pollMs={5000} />);

      await settle();

      expect(screen.queryByRole("alert")).not.toBeInTheDocument();
      expect(screen.getByText("Runtime 運作中")).toBeInTheDocument();
    });

    it("沒有故障時不顯示警示", async () => {
      render(<StatusBar client={clientWithStorage({ dispatch_halted: false, fault: null })} pollMs={5000} />);

      await settle();

      expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    });

    it("下一次輪詢恢復後警示消失", async () => {
      render(<StatusBar client={clientWithStorage(diskFull, { dispatch_halted: false, fault: null })} pollMs={5000} />);
      await settle();
      expect(screen.getByRole("alert")).toHaveTextContent("新派工已停止");

      await settle(5000);

      expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    });
  });
});
