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

  it("HTTP 版建立 Workspace：POST JSON 並回傳 Runtime 的 Workspace", async () => {
    const created = { id: "ws_1", name: "docs", path: "C:\\docs", status: "active" };
    const fetchFn = vi.fn(async () => new Response(JSON.stringify(created), { status: 201 }));

    const result = await createHttpClient(fetchFn as unknown as typeof fetch).createWorkspace({ path: "C:\\docs" });

    expect(result).toEqual(created);
    expect(fetchFn).toHaveBeenCalledWith("/api/v1/workspaces", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ path: "C:\\docs" }),
    });
  });

  it("HTTP 版把 Runtime 回報的說明當成錯誤訊息", async () => {
    const body = { error: "workspace_path_not_found", message: "找不到目錄：C:\\nope" };
    const fetchFn = vi.fn(async () => new Response(JSON.stringify(body), { status: 400 }));

    await expect(
      createHttpClient(fetchFn as unknown as typeof fetch).createWorkspace({ path: "C:\\nope" }),
    ).rejects.toThrow("找不到目錄：C:\\nope");
  });

  it("HTTP 版列出與封存 Workspace", async () => {
    const fetchFn = vi.fn(async () => new Response(JSON.stringify({ workspaces: [] }), { status: 200 }));
    const client = createHttpClient(fetchFn as unknown as typeof fetch);

    await client.listWorkspaces();
    await client.archiveWorkspace("ws_1");

    expect(fetchFn).toHaveBeenNthCalledWith(1, "/api/v1/workspaces");
    expect(fetchFn).toHaveBeenNthCalledWith(2, "/api/v1/workspaces/ws_1/archive", { method: "POST" });
  });

  it("Tauri 版以指令與參數管理 Workspace", async () => {
    const invoke = vi.fn(async () => ({ workspaces: [] }));
    const client = createTauriClient(invoke as never);

    await client.listWorkspaces();
    await client.createWorkspace({ path: "C:\\docs", name: "文件" });
    await client.archiveWorkspace("ws_1");

    expect(invoke).toHaveBeenNthCalledWith(1, "workspace_list");
    expect(invoke).toHaveBeenNthCalledWith(2, "workspace_create", { path: "C:\\docs", name: "文件" });
    expect(invoke).toHaveBeenNthCalledWith(3, "workspace_archive", { id: "ws_1" });
  });
});
