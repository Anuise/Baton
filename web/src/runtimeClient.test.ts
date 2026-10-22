import { describe, expect, it, vi } from "vitest";

import { createHttpClient, createTauriClient, errorCode } from "./runtimeClient";

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

  it("HTTP 版讀取、預覽與確認 Workspace 授權", async () => {
    const fetchFn = vi.fn(async () => new Response(JSON.stringify({}), { status: 200 }));
    const client = createHttpClient(fetchFn as unknown as typeof fetch);
    const grants = [{ capability: "read_files" as const, targets: ["C:\\docs"], restrictions: [] }];

    await client.getAuthorization("ws_1");
    await client.summarizeAuthorization("ws_1", grants);
    await client.confirmAuthorization("ws_1", { templateId: "research_docs", grants, acknowledgedRisk: false });

    expect(fetchFn).toHaveBeenNthCalledWith(1, "/api/v1/workspaces/ws_1/authorization");
    expect(fetchFn).toHaveBeenNthCalledWith(2, "/api/v1/workspaces/ws_1/authorization/summary", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ grants }),
    });
    expect(fetchFn).toHaveBeenNthCalledWith(3, "/api/v1/workspaces/ws_1/authorization", {
      method: "PUT",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ template_id: "research_docs", grants, acknowledged_risk: false }),
    });
  });

  it("Tauri 版以指令與參數管理 Workspace 授權", async () => {
    const invoke = vi.fn(async () => ({}));
    const client = createTauriClient(invoke as never);
    const grants = [{ capability: "read_files" as const, targets: ["C:\\docs"], restrictions: [] }];

    await client.getAuthorization("ws_1");
    await client.summarizeAuthorization("ws_1", grants);
    await client.confirmAuthorization("ws_1", { templateId: null, grants, acknowledgedRisk: true });

    expect(invoke).toHaveBeenNthCalledWith(1, "workspace_authorization", { id: "ws_1" });
    expect(invoke).toHaveBeenNthCalledWith(2, "workspace_authorization_summary", { id: "ws_1", grants });
    expect(invoke).toHaveBeenNthCalledWith(3, "workspace_authorization_confirm", {
      id: "ws_1",
      templateId: null,
      grants,
      acknowledgedRisk: true,
    });
  });

  it("HTTP 版以契約的路徑與 body 操作 Goal 與 Task", async () => {
    const fetchFn = vi.fn(async () => new Response(JSON.stringify({}), { status: 200 }));
    const client = createHttpClient(fetchFn as unknown as typeof fetch);
    const json = { "content-type": "application/json" };
    const contract = { objective: { status: "filled" as const, text: "整理文件" }, required_capabilities: ["read_files" as const] };

    await client.getTaskBoard("ws_1");
    await client.createGoal("ws_1", "第一個目標");
    await client.createTask("ws_1", { goalId: "goal_1", title: "寫文件", contract });
    await client.createTask("ws_1", { goalId: "goal_1", title: "空契約" });
    await client.getTask("ws_1", "task_1");
    await client.updateTaskContract("ws_1", "task_1", contract, "補上目標");
    await client.updateTaskContract("ws_1", "task_1", contract);
    await client.addTaskDependency("ws_1", "task_1", "task_2");
    await client.checkTaskAdmission("ws_1", "task_1");
    await client.markTaskReady("ws_1", "task_1");
    await client.cancelTask("ws_1", "task_1");
    await client.dispatchTask("ws_1", "task_1");
    await client.cancelRun("ws_1", "run_1");

    const base = "/api/v1/workspaces/ws_1";
    expect(fetchFn).toHaveBeenNthCalledWith(1, `${base}/tasks`);
    expect(fetchFn).toHaveBeenNthCalledWith(2, `${base}/goals`, {
      method: "POST",
      headers: json,
      body: JSON.stringify({ title: "第一個目標" }),
    });
    expect(fetchFn).toHaveBeenNthCalledWith(3, `${base}/tasks`, {
      method: "POST",
      headers: json,
      body: JSON.stringify({ goal_id: "goal_1", title: "寫文件", contract }),
    });
    expect(fetchFn).toHaveBeenNthCalledWith(4, `${base}/tasks`, {
      method: "POST",
      headers: json,
      body: JSON.stringify({ goal_id: "goal_1", title: "空契約" }),
    });
    expect(fetchFn).toHaveBeenNthCalledWith(5, `${base}/tasks/task_1`);
    expect(fetchFn).toHaveBeenNthCalledWith(6, `${base}/tasks/task_1/contract`, {
      method: "PUT",
      headers: json,
      body: JSON.stringify({ contract, note: "補上目標" }),
    });
    expect(fetchFn).toHaveBeenNthCalledWith(7, `${base}/tasks/task_1/contract`, {
      method: "PUT",
      headers: json,
      body: JSON.stringify({ contract }),
    });
    expect(fetchFn).toHaveBeenNthCalledWith(8, `${base}/tasks/task_1/dependencies`, {
      method: "POST",
      headers: json,
      body: JSON.stringify({ depends_on: "task_2" }),
    });
    expect(fetchFn).toHaveBeenNthCalledWith(9, `${base}/tasks/task_1/admission-check`, { method: "POST" });
    expect(fetchFn).toHaveBeenNthCalledWith(10, `${base}/tasks/task_1/ready`, { method: "POST" });
    expect(fetchFn).toHaveBeenNthCalledWith(11, `${base}/tasks/task_1/cancel`, { method: "POST" });
    expect(fetchFn).toHaveBeenNthCalledWith(12, `${base}/tasks/task_1/dispatch`, { method: "POST" });
    expect(fetchFn).toHaveBeenNthCalledWith(13, `${base}/runs/run_1/cancel`, { method: "POST" });
  });

  it("HTTP 版派工與取消 Run 失敗時，以 Runtime 的說明當成錯誤訊息", async () => {
    const body = { error: "task_not_ready", message: "Task 不是 ready，不能派工" };
    const fetchFn = vi.fn(async () => new Response(JSON.stringify(body), { status: 409 }));
    const client = createHttpClient(fetchFn as unknown as typeof fetch);

    await expect(client.dispatchTask("ws_1", "task_1")).rejects.toThrow("Task 不是 ready，不能派工");
    await expect(client.cancelRun("ws_1", "run_1")).rejects.toThrow("Task 不是 ready，不能派工");
  });

  it("HTTP 版派工建立 Run（201）時回傳 started 與 Runtime 的 Run", async () => {
    const run = { id: "run_1", status: "queued", profile_id: "prof_1", lease: null };
    const fetchFn = vi.fn(async () => new Response(JSON.stringify(run), { status: 201 }));

    await expect(createHttpClient(fetchFn as unknown as typeof fetch).dispatchTask("ws_1", "task_1")).resolves.toEqual({
      kind: "started",
      run,
    });
  });

  it("HTTP 版派工排隊中（202）時回傳 queued 與等待原因", async () => {
    const queued = {
      queued: true,
      task_id: "task_1",
      priority: 0,
      reasons: [{ category: "capacity", code: "account_capacity_full", message: "帳號名額已滿", profile_id: "prof_1" }],
    };
    const fetchFn = vi.fn(async () => new Response(JSON.stringify(queued), { status: 202 }));

    await expect(createHttpClient(fetchFn as unknown as typeof fetch).dispatchTask("ws_1", "task_1")).resolves.toEqual({
      kind: "queued",
      task_id: "task_1",
      priority: 0,
      reasons: queued.reasons,
    });
  });

  it("Tauri 版派工以 queued 欄位區分已建立 Run 與排隊中", async () => {
    const run = { id: "run_1", status: "queued", profile_id: "prof_1", lease: null };
    const queued = { queued: true, task_id: "task_1", priority: 2, reasons: [] };
    const invoke = vi.fn().mockResolvedValueOnce(run).mockResolvedValueOnce(queued);
    const client = createTauriClient(invoke as never);

    await expect(client.dispatchTask("ws_1", "task_1")).resolves.toEqual({ kind: "started", run });
    await expect(client.dispatchTask("ws_1", "task_2")).resolves.toEqual({
      kind: "queued",
      task_id: "task_1",
      priority: 2,
      reasons: [],
    });
  });

  it("HTTP 版讀取排程總覽與切換容量預設", async () => {
    const fetchFn = vi.fn(async () => new Response(JSON.stringify({}), { status: 200 }));
    const client = createHttpClient(fetchFn as unknown as typeof fetch);

    await client.schedulingOverview();
    await client.setSchedulingMode("higher_concurrency");

    expect(fetchFn).toHaveBeenNthCalledWith(1, "/api/v1/scheduling");
    expect(fetchFn).toHaveBeenNthCalledWith(2, "/api/v1/scheduling/settings", {
      method: "PUT",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ mode: "higher_concurrency" }),
    });
  });

  it("Tauri 版以指令讀取排程總覽與切換容量預設", async () => {
    const invoke = vi.fn(async () => ({}));
    const client = createTauriClient(invoke as never);

    await client.schedulingOverview();
    await client.setSchedulingMode("reserve_handoff");

    expect(invoke.mock.calls).toEqual([["scheduling_overview"], ["scheduling_set_mode", { mode: "reserve_handoff" }]]);
  });

  it("HTTP 版設定方案上限：PUT 到 Profile 的 quota-limits，未設定的視窗送 null", async () => {
    const fetchFn = vi.fn(async () => new Response(JSON.stringify({}), { status: 200 }));
    const client = createHttpClient(fetchFn as unknown as typeof fetch);

    await client.setQuotaLimits("prof_1", { five_hour_tokens: 500_000, seven_day_tokens: null });

    expect(fetchFn).toHaveBeenCalledWith("/api/v1/profiles/prof_1/quota-limits", {
      method: "PUT",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ five_hour_tokens: 500_000, seven_day_tokens: null }),
    });
  });

  it("HTTP 版設定方案上限被拒絕時，錯誤訊息是 Runtime 的說明", async () => {
    const body = { error: "plan_limit_invalid", message: "方案上限必須是正整數（token）；不知道就留空" };
    const fetchFn = vi.fn(async () => new Response(JSON.stringify(body), { status: 400 }));

    await expect(
      createHttpClient(fetchFn as unknown as typeof fetch).setQuotaLimits("prof_1", {
        five_hour_tokens: 0,
        seven_day_tokens: null,
      }),
    ).rejects.toThrow("方案上限必須是正整數（token）；不知道就留空");
  });

  it("Tauri 版以 profile_set_quota_limits 指令設定方案上限", async () => {
    const invoke = vi.fn(async () => ({}));
    const limits = { five_hour_tokens: null, seven_day_tokens: 3_000_000 };

    await createTauriClient(invoke as never).setQuotaLimits("prof_1", limits);

    expect(invoke.mock.calls).toEqual([["profile_set_quota_limits", { id: "prof_1", limits }]]);
  });

  it("HTTP 版把 Task 操作失敗時 Runtime 的說明當成錯誤訊息", async () => {
    const body = { error: "dependency_cycle", message: "相依會形成循環：A → B → A" };
    const fetchFn = vi.fn(async () => new Response(JSON.stringify(body), { status: 409 }));

    await expect(
      createHttpClient(fetchFn as unknown as typeof fetch).addTaskDependency("ws_1", "task_1", "task_2"),
    ).rejects.toThrow("相依會形成循環：A → B → A");
  });

  it("HTTP 版在准入被拒時取得 Runtime 的說明", async () => {
    const body = { error: "task_admission_denied", message: "Task 尚未符合派工條件", missing: [] };
    const fetchFn = vi.fn(async () => new Response(JSON.stringify(body), { status: 409 }));

    await expect(createHttpClient(fetchFn as unknown as typeof fetch).markTaskReady("ws_1", "task_1")).rejects.toThrow(
      "Task 尚未符合派工條件",
    );
  });

  it("Tauri 版以指令與參數操作 Goal 與 Task", async () => {
    const invoke = vi.fn(async () => ({}));
    const client = createTauriClient(invoke as never);
    const contract = { objective: { status: "unknown" as const } };

    await client.getTaskBoard("ws_1");
    await client.createGoal("ws_1", "第一個目標");
    await client.createTask("ws_1", { goalId: "goal_1", title: "寫文件", contract });
    await client.getTask("ws_1", "task_1");
    await client.updateTaskContract("ws_1", "task_1", contract, "補上目標");
    await client.addTaskDependency("ws_1", "task_1", "task_2");
    await client.checkTaskAdmission("ws_1", "task_1");
    await client.markTaskReady("ws_1", "task_1");
    await client.cancelTask("ws_1", "task_1");
    await client.dispatchTask("ws_1", "task_1");
    await client.cancelRun("ws_1", "run_1");

    expect(invoke).toHaveBeenNthCalledWith(1, "task_board", { workspaceId: "ws_1" });
    expect(invoke).toHaveBeenNthCalledWith(2, "goal_create", { workspaceId: "ws_1", title: "第一個目標" });
    expect(invoke).toHaveBeenNthCalledWith(3, "task_create", {
      workspaceId: "ws_1",
      goalId: "goal_1",
      title: "寫文件",
      contract,
    });
    expect(invoke).toHaveBeenNthCalledWith(4, "task_detail", { workspaceId: "ws_1", taskId: "task_1" });
    expect(invoke).toHaveBeenNthCalledWith(5, "task_contract_update", {
      workspaceId: "ws_1",
      taskId: "task_1",
      contract,
      note: "補上目標",
    });
    expect(invoke).toHaveBeenNthCalledWith(6, "task_dependency_add", {
      workspaceId: "ws_1",
      taskId: "task_1",
      dependsOn: "task_2",
    });
    expect(invoke).toHaveBeenNthCalledWith(7, "task_admission_check", { workspaceId: "ws_1", taskId: "task_1" });
    expect(invoke).toHaveBeenNthCalledWith(8, "task_ready", { workspaceId: "ws_1", taskId: "task_1" });
    expect(invoke).toHaveBeenNthCalledWith(9, "task_cancel", { workspaceId: "ws_1", taskId: "task_1" });
    expect(invoke).toHaveBeenNthCalledWith(10, "task_dispatch", { workspaceId: "ws_1", taskId: "task_1" });
    expect(invoke).toHaveBeenNthCalledWith(11, "run_cancel", { workspaceId: "ws_1", runId: "run_1" });
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
  it("Tauri 版以指令與 camelCase 參數操作共同設定", async () => {
    const invoke = vi.fn(async () => ({}));
    const client = createTauriClient(invoke as never);

    await client.getSharedSettingsStatus();
    await client.previewSharedSettings();
    await client.adoptSharedSettings("hash_1");
    await client.getSharedSettingsVersion(3);

    expect(invoke.mock.calls).toEqual([
      ["shared_settings_status"],
      ["shared_settings_preview"],
      ["shared_settings_adopt", { expectedHash: "hash_1" }],
      ["shared_settings_version", { version: 3 }],
    ]);
  });

  it("HTTP 版以契約的路徑操作共同設定，採用送出 expected_hash", async () => {
    const fetchFn = vi.fn(async () => new Response(JSON.stringify({}), { status: 200 }));
    const client = createHttpClient(fetchFn as unknown as typeof fetch);

    await client.getSharedSettingsStatus();
    await client.previewSharedSettings();
    await client.adoptSharedSettings("hash_1");
    await client.getSharedSettingsVersion(3);

    expect(fetchFn).toHaveBeenNthCalledWith(1, "/api/v1/shared-settings");
    expect(fetchFn).toHaveBeenNthCalledWith(2, "/api/v1/shared-settings/import-preview", { method: "POST" });
    expect(fetchFn).toHaveBeenNthCalledWith(3, "/api/v1/shared-settings/adopt", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ expected_hash: "hash_1" }),
    });
    expect(fetchFn).toHaveBeenNthCalledWith(4, "/api/v1/shared-settings/versions/3");
  });

  it("兩種用戶端都能辨識採用被拒絕的穩定代碼 settings_source_changed", async () => {
    const body = { error: "settings_source_changed", message: "來源設定在預覽之後已經改變" };
    const fetchFn = vi.fn(async () => new Response(JSON.stringify(body), { status: 409 }));
    const invoke = vi.fn(async () => {
      throw "Runtime 拒絕連線（settings_source_changed）：來源設定在預覽之後已經改變";
    });

    const http = await createHttpClient(fetchFn as unknown as typeof fetch).adoptSharedSettings("h").catch((e) => e);
    const tauri = await createTauriClient(invoke as never).adoptSharedSettings("h").catch((e) => e);

    expect(errorCode(http)).toBe("settings_source_changed");
    expect(errorCode(tauri)).toBe("settings_source_changed");
    expect(errorCode(new Error("無法連線到 Runtime"))).toBeNull();
  });

  it("HTTP 版列出成果、預覽版本：路徑含 Workspace、成果與版本", async () => {
    const fetchFn = vi.fn(async () => new Response(JSON.stringify({ artifacts: [], unregistered_files: [] }), { status: 200 }));
    const client = createHttpClient(fetchFn as unknown as typeof fetch);

    await client.listArtifacts("ws_1");
    await client.previewArtifact("ws_1", "art_1", 2);

    expect(fetchFn).toHaveBeenNthCalledWith(1, "/api/v1/workspaces/ws_1/artifacts");
    expect(fetchFn).toHaveBeenNthCalledWith(2, "/api/v1/workspaces/ws_1/artifacts/art_1/versions/2/preview");
  });

  it("HTTP 版下載成果：以同一個 fetch 取得位元組與 X-Baton-Sha256，不把憑證放進網址", async () => {
    const fetchFn = vi.fn(
      async () => new Response("內容", { status: 200, headers: { "X-Baton-Sha256": "ab".repeat(32) } }),
    );

    const result = await createHttpClient(fetchFn as unknown as typeof fetch).downloadArtifact("ws_1", "art_1", 2);

    expect(fetchFn).toHaveBeenCalledWith("/api/v1/workspaces/ws_1/artifacts/art_1/versions/2/content");
    expect(result.sha256).toBe("ab".repeat(32));
    expect(await result.blob.text()).toBe("內容");
  });

  it("HTTP 版下載成果缺少 X-Baton-Sha256 標頭時，雜湊是空字串", async () => {
    const fetchFn = vi.fn(async () => new Response("x", { status: 200 }));

    const result = await createHttpClient(fetchFn as unknown as typeof fetch).downloadArtifact("ws_1", "art_1", 1);

    expect(result.sha256).toBe("");
  });

  it("HTTP 版成果操作失敗時，以 Runtime 的說明當成錯誤訊息", async () => {
    const body = { error: "artifact_integrity_failed", message: "成果檔案與登記的雜湊不符，拒絕提供" };
    const fetchFn = vi.fn(async () => new Response(JSON.stringify(body), { status: 409 }));
    const client = createHttpClient(fetchFn as unknown as typeof fetch);

    await expect(client.listArtifacts("ws_1")).rejects.toThrow("成果檔案與登記的雜湊不符，拒絕提供");
    await expect(client.previewArtifact("ws_1", "art_1", 1)).rejects.toThrow("成果檔案與登記的雜湊不符，拒絕提供");
    await expect(client.downloadArtifact("ws_1", "art_1", 1)).rejects.toThrow("成果檔案與登記的雜湊不符，拒絕提供");
  });

  it("Tauri 版以指令與參數瀏覽成果；下載由殼回傳 base64 內容與雜湊", async () => {
    const invoke = vi.fn(async (command: string) =>
      command === "artifact_download" ? { sha256: "cd".repeat(32), content_base64: btoa("hello") } : {},
    );
    const client = createTauriClient(invoke as never);

    await client.listArtifacts("ws_1");
    await client.previewArtifact("ws_1", "art_1", 2);
    const result = await client.downloadArtifact("ws_1", "art_1", 2);

    expect(invoke.mock.calls).toEqual([
      ["artifact_list", { workspaceId: "ws_1" }],
      ["artifact_preview", { workspaceId: "ws_1", artifactId: "art_1", version: 2 }],
      ["artifact_download", { workspaceId: "ws_1", artifactId: "art_1", version: 2 }],
    ]);
    expect(result.sha256).toBe("cd".repeat(32));
    expect(await result.blob.text()).toBe("hello");
  });

  const basis = { run_id: "run_1", generation: 2, contract_version: 3, artifacts: [{ artifact_id: "art_1", version: 1 }] };

  it("HTTP 版讀取驗證資料與手動接受：接受時以 JSON 送出 basis 與附註", async () => {
    const fetchFn = vi.fn(async () => new Response(JSON.stringify({}), { status: 200 }));
    const client = createHttpClient(fetchFn as unknown as typeof fetch);

    await client.taskVerification("ws_1", "task_1");
    await client.acceptTask("ws_1", "task_1", { basis, note: "已確認" });
    await client.acceptTask("ws_1", "task_1", { basis });

    const base = "/api/v1/workspaces/ws_1/tasks/task_1/verification";
    expect(fetchFn).toHaveBeenNthCalledWith(1, base);
    expect(fetchFn).toHaveBeenNthCalledWith(2, `${base}/accept`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ basis, note: "已確認" }),
    });
    expect(fetchFn).toHaveBeenNthCalledWith(3, `${base}/accept`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ basis }),
    });
  });

  it("HTTP 版接受被拒（409）時，以 Runtime 的說明當成錯誤訊息", async () => {
    const body = { error: "acceptance_manifest_stale", message: "成果 manifest 已變更，請重新檢視後再接受" };
    const fetchFn = vi.fn(async () => new Response(JSON.stringify(body), { status: 409 }));

    await expect(
      createHttpClient(fetchFn as unknown as typeof fetch).acceptTask("ws_1", "task_1", { basis, note: null }),
    ).rejects.toThrow("成果 manifest 已變更，請重新檢視後再接受");
  });

  it("Tauri 版以 task_verification 與 task_accept 指令讀取驗證資料與手動接受", async () => {
    const invoke = vi.fn(async () => ({}));
    const client = createTauriClient(invoke as never);

    await client.taskVerification("ws_1", "task_1");
    await client.acceptTask("ws_1", "task_1", { basis, note: null });

    expect(invoke.mock.calls).toEqual([
      ["task_verification", { workspaceId: "ws_1", taskId: "task_1" }],
      ["task_accept", { workspaceId: "ws_1", taskId: "task_1", request: { basis, note: null } }],
    ]);
  });
});
