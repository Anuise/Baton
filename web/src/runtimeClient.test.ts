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
  it("HTTP 版管理帳號 Profile：新增送出 expected_account，其餘為 POST 並解析 Runtime 的回應", async () => {
    const fetchFn = vi.fn(async () => new Response(JSON.stringify({ profiles: [], limit: 8 }), { status: 200 }));
    const client = createHttpClient(fetchFn as unknown as typeof fetch);

    await client.listProfiles();
    await client.createProfile({ name: "主要帳號", expectedAccount: "me@example.com" });
    await client.verifyProfile("prof_1");
    await client.openLoginTerminal("prof_1");
    await client.archiveProfile("prof_1");

    expect(fetchFn).toHaveBeenNthCalledWith(1, "/api/v1/profiles");
    expect(fetchFn).toHaveBeenNthCalledWith(2, "/api/v1/profiles", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ name: "主要帳號", expected_account: "me@example.com" }),
    });
    expect(fetchFn).toHaveBeenNthCalledWith(3, "/api/v1/profiles/prof_1/verify", { method: "POST" });
    expect(fetchFn).toHaveBeenNthCalledWith(4, "/api/v1/profiles/prof_1/login-terminal", { method: "POST" });
    expect(fetchFn).toHaveBeenNthCalledWith(5, "/api/v1/profiles/prof_1/archive", { method: "POST" });
  });

  it("HTTP 版新增第 9 個 Profile 被拒絕時，錯誤訊息是 Runtime 說明的上限", async () => {
    const body = { error: "profile_limit_reached", message: "最多只能有 8 個帳號 Profile" };
    const fetchFn = vi.fn(async () => new Response(JSON.stringify(body), { status: 409 }));

    await expect(
      createHttpClient(fetchFn as unknown as typeof fetch).createProfile({ name: "第 9 個" }),
    ).rejects.toThrow("最多只能有 8 個帳號 Profile");
  });

  it("HTTP 版取得精靈狀態、重新檢查與確認背景行為告知", async () => {
    const fetchFn = vi.fn(async () => new Response(JSON.stringify({}), { status: 200 }));
    const client = createHttpClient(fetchFn as unknown as typeof fetch);

    await client.getSetup();
    await client.recheckSetup();
    await client.acknowledgeBackgroundNotice();

    expect(fetchFn).toHaveBeenNthCalledWith(1, "/api/v1/setup");
    expect(fetchFn).toHaveBeenNthCalledWith(2, "/api/v1/setup/recheck", { method: "POST" });
    expect(fetchFn).toHaveBeenNthCalledWith(3, "/api/v1/setup/background-notice/acknowledge", { method: "POST" });
  });

  it("Tauri 版以指令與參數管理帳號 Profile 與精靈", async () => {
    const invoke = vi.fn(async () => ({}));
    const client = createTauriClient(invoke as never);

    await client.listProfiles();
    await client.createProfile({ name: "主要帳號", expectedAccount: "me@example.com" });
    await client.verifyProfile("prof_1");
    await client.openLoginTerminal("prof_1");
    await client.archiveProfile("prof_1");
    await client.getSetup();
    await client.recheckSetup();
    await client.acknowledgeBackgroundNotice();

    expect(invoke.mock.calls).toEqual([
      ["profile_list"],
      ["profile_create", { name: "主要帳號", expectedAccount: "me@example.com" }],
      ["profile_verify", { id: "prof_1" }],
      ["profile_login_terminal", { id: "prof_1" }],
      ["profile_archive", { id: "prof_1" }],
      ["setup_status"],
      ["setup_recheck"],
      ["setup_acknowledge_notice"],
    ]);
  });
});
