import { act, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { profile, recoveryEvent, recoveryStatus, supportedPlatform, unofficialPlatform } from "./fixtures";
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

  describe("可用帳號數", () => {
    it("顯示可派工的帳號數（不含被阻擋與已封存的帳號），並在輪詢時更新", async () => {
      const blocked = profile({ id: "prof_3", dispatchable: false, blockers: [{ code: "not_logged_in", message: "尚未登入" }] });
      const archived = profile({ id: "prof_4", status: "archived", dispatchable: false });
      const client = {
        ...clientReturning("ok"),
        listProfiles: vi
          .fn()
          .mockResolvedValueOnce({ profiles: [profile(), profile({ id: "prof_2" }), blocked, archived], limit: 8 })
          .mockResolvedValue({ profiles: [profile(), blocked], limit: 8 }),
      };
      render(<StatusBar client={client} pollMs={5000} />);

      await settle();
      expect(screen.getByText("可用帳號：2")).toBeInTheDocument();

      await settle(5000);
      expect(screen.getByText("可用帳號：1")).toBeInTheDocument();
    });

    it("讀不到帳號時不顯示該欄位，也不影響 Runtime 狀態", async () => {
      const client = { ...clientReturning("ok"), listProfiles: vi.fn(async () => Promise.reject(new Error("boom"))) };
      render(<StatusBar client={client} pollMs={5000} />);

      await settle();

      expect(screen.queryByText(/可用帳號/)).not.toBeInTheDocument();
      expect(screen.getByText("Runtime 運作中")).toBeInTheDocument();
    });
  });

  describe("崩潰恢復事件", () => {
    it("恢復進行中時顯示本輪次數，並在輪詢時更新", async () => {
      const client = {
        ...clientReturning("ok"),
        recoveryStatus: vi
          .fn()
          .mockResolvedValueOnce(recoveryStatus({ event: recoveryEvent({ status: "attempting", attempts_used: 1 }) }))
          .mockResolvedValue(recoveryStatus({ event: recoveryEvent({ status: "waiting", attempts_used: 2 }) })),
      };
      render(<StatusBar client={client} pollMs={5000} />);

      await settle();
      expect(screen.getByText("恢復中（1/3）")).toBeInTheDocument();

      await settle(5000);
      expect(screen.getByText("恢復中（2/3）")).toBeInTheDocument();
    });

    it("恢復額度用盡時顯示用盡", async () => {
      const event = recoveryEvent({ status: "exhausted", attempts_used: 3 });
      const client = { ...clientReturning("ok"), recoveryStatus: vi.fn(async () => recoveryStatus({ event, can_retry: true })) };
      render(<StatusBar client={client} pollMs={5000} />);

      await settle();

      expect(screen.getByText("恢復額度用盡")).toBeInTheDocument();
    });

    it("沒有恢復事件或已恢復時不顯示", async () => {
      const recovery = vi
        .fn()
        .mockResolvedValueOnce(recoveryStatus())
        .mockResolvedValue(recoveryStatus({ event: recoveryEvent({ status: "succeeded", closed_at: 1 }) }));
      render(<StatusBar client={{ ...clientReturning("ok"), recoveryStatus: recovery }} pollMs={5000} />);

      await settle();
      expect(screen.queryByText(/恢復/)).not.toBeInTheDocument();

      await settle(5000);
      expect(recovery).toHaveBeenCalledTimes(2);
      expect(screen.queryByText(/恢復/)).not.toBeInTheDocument();
    });

    it("讀不到恢復狀態時不顯示該欄位，也不影響 Runtime 狀態", async () => {
      const client = { ...clientReturning("ok"), recoveryStatus: vi.fn(async () => Promise.reject(new Error("boom"))) };
      render(<StatusBar client={client} pollMs={5000} />);

      await settle();

      expect(screen.queryByText(/恢復/)).not.toBeInTheDocument();
      expect(screen.getByText("Runtime 運作中")).toBeInTheDocument();
    });
  });

  describe("監護狀態", () => {
    function clientWithGuardian(guardian: HealthResponse["guardian"]): RuntimeClient {
      return {
        ...clientReturning("ok"),
        getHealth: vi.fn(async () => (guardian === undefined ? health : { ...health, guardian })),
      };
    }

    it("監護運作中時顯示「監護：運作中」", async () => {
      render(<StatusBar client={clientWithGuardian({ state: "active", message: null })} pollMs={5000} />);

      await settle();

      expect(screen.getByText("監護：運作中")).toBeInTheDocument();
      expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    });

    it("監護不在時以警示顯示 Runtime 回傳的降級訊息，Runtime 仍顯示運作中", async () => {
      const guardian = { state: "unavailable", message: "工作繼續，自動重啟保護不可用" } as const;
      render(<StatusBar client={clientWithGuardian(guardian)} pollMs={5000} />);

      await settle();

      expect(screen.getByRole("alert")).toHaveTextContent("工作繼續，自動重啟保護不可用");
      expect(screen.getByText("Runtime 運作中")).toBeInTheDocument();
    });

    it("監護不在但沒有訊息時退回固定文字", async () => {
      render(<StatusBar client={clientWithGuardian({ state: "unavailable", message: null })} pollMs={5000} />);

      await settle();

      expect(screen.getByRole("alert")).toHaveTextContent("工作繼續，自動重啟保護不可用");
    });

    it("舊版 Runtime 沒有 guardian 欄位時不顯示監護狀態", async () => {
      render(<StatusBar client={clientWithGuardian(undefined)} pollMs={5000} />);

      await settle();

      expect(screen.queryByText(/監護/)).not.toBeInTheDocument();
      expect(screen.queryByText(/自動重啟保護/)).not.toBeInTheDocument();
      expect(screen.getByText("Runtime 運作中")).toBeInTheDocument();
    });

    it("監護狀態未知時不顯示，避免誤報", async () => {
      render(<StatusBar client={clientWithGuardian({ state: "unknown", message: null })} pollMs={5000} />);

      await settle();

      expect(screen.queryByText(/監護/)).not.toBeInTheDocument();
      expect(screen.queryByText(/自動重啟保護/)).not.toBeInTheDocument();
    });

    it("Runtime 未連線時不顯示監護狀態", async () => {
      render(<StatusBar client={clientReturning(new Error("down"))} pollMs={5000} />);

      await settle();

      expect(screen.queryByText(/監護/)).not.toBeInTheDocument();
    });
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
