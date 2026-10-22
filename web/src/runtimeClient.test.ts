import { describe, expect, it, vi } from "vitest";

import { createHttpClient, createTauriClient } from "./runtimeClient";

const health = { status: "ok", product: "Baton", version: "0.1.0", boot_count: 1, started_at: 0, pid: 1 };

describe("Runtime 用戶端", () => {
  it("HTTP 版向 /api/v1/health 取得健康狀態", async () => {
    const fetchFn = vi.fn(async () => new Response(JSON.stringify(health), { status: 200 }));

    const result = await createHttpClient(fetchFn as unknown as typeof fetch).getHealth();

    expect(result).toEqual(health);
    expect(fetchFn).toHaveBeenCalledWith("/api/v1/health");
  });

  it("HTTP 版遇到非 2xx 時丟出錯誤", async () => {
    const fetchFn = vi.fn(async () => new Response("nope", { status: 503 }));

    await expect(createHttpClient(fetchFn as unknown as typeof fetch).getHealth()).rejects.toThrow("503");
  });

  it("Tauri 版透過 runtime_health 指令取得健康狀態", async () => {
    const invoke = vi.fn(async () => health);

    const result = await createTauriClient(invoke as never).getHealth();

    expect(result).toEqual(health);
    expect(invoke).toHaveBeenCalledWith("runtime_health");
  });

  it("Tauri 版把指令失敗的字串訊息轉成 Error", async () => {
    const invoke = vi.fn(async () => {
      throw "無法連線到 Runtime";
    });

    await expect(createTauriClient(invoke as never).getHealth()).rejects.toThrow("無法連線到 Runtime");
  });

  it("HTTP 版向 /api/v1/platform 取得平台狀態", async () => {
    const platform = { app_version: "0.1.0" };
    const fetchFn = vi.fn(async () => new Response(JSON.stringify(platform), { status: 200 }));

    const result = await createHttpClient(fetchFn as unknown as typeof fetch).getPlatform();

    expect(result).toEqual(platform);
    expect(fetchFn).toHaveBeenCalledWith("/api/v1/platform");
  });

  it("Tauri 版透過 runtime_platform 指令取得平台狀態", async () => {
    const invoke = vi.fn(async () => ({ app_version: "0.1.0" }));

    await createTauriClient(invoke as never).getPlatform();

    expect(invoke).toHaveBeenCalledWith("runtime_platform");
  });
});
