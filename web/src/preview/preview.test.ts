import { describe, expect, it, vi } from "vitest";

import { createPreviewClient, previewData, unsupportedMessage } from "./client";
import { LONG_ACCOUNT, LONG_PATH, LONG_SHA256, LONG_URL } from "./longData";
import { PREVIEW_PAGES } from "./pages";

const TOP_TABS = ["workspace", "accounts", "settings"];
const WORKSPACE_PAGES = ["controller", "tasks", "decisions", "artifacts", "reviews", "verification", "exploration", "knowledge", "workflows", "timeline", "authorization"];

describe("預覽頁面清單", () => {
  it("涵蓋每個頂層頁、每個 Workspace 頁面與首次啟動精靈", () => {
    for (const top of TOP_TABS) expect(PREVIEW_PAGES.some((page) => page.top === top && !page.setup)).toBe(true);
    for (const id of WORKSPACE_PAGES) expect(PREVIEW_PAGES.some((page) => page.workspacePage === id)).toBe(true);
    expect(PREVIEW_PAGES.some((page) => page.setup)).toBe(true);
    expect(new Set(PREVIEW_PAGES.map((page) => page.id)).size).toBe(PREVIEW_PAGES.length);
  });
});

describe("long 資料集", () => {
  const data = previewData("long", false);
  const all = JSON.stringify(data);

  it("有 100 字以上的中文 Task 標題與裁定標題", () => {
    expect(Math.max(...data.board.tasks.map((task) => task.title.length))).toBeGreaterThanOrEqual(100);
    expect(Math.max(...data.decisions.decisions.map((d) => d.question.length))).toBeGreaterThanOrEqual(100);
  });

  it("契約是多段落的文字", () => {
    const objective = data.board.tasks[0].contract.objective;
    expect(objective?.status === "filled" && objective.text.split("\n\n").length).toBeGreaterThanOrEqual(3);
  });

  it("含沒有空白的長識別碼、Windows 路徑、SHA-256、URL 與長帳號名稱", () => {
    // JSON 字串中的反斜線會被跳脫，以同樣方式比對
    for (const text of [LONG_PATH, LONG_SHA256, LONG_URL, LONG_ACCOUNT]) expect(all).toContain(JSON.stringify(text).slice(1, -1));
    expect(LONG_SHA256).toMatch(/^[0-9a-f]{64}$/);
  });

  it("清單很多筆、徽章數字很大", () => {
    expect(data.board.tasks.length).toBeGreaterThanOrEqual(40);
    expect(data.decisions.decisions.length).toBeGreaterThanOrEqual(30);
    expect(data.timeline.events.length).toBeGreaterThanOrEqual(100);
    expect(data.pending.by_workspace[0].pending_count).toBeGreaterThanOrEqual(1000);
  });

  it("首次啟動精靈的資料沒有可派工的帳號", () => {
    for (const name of ["normal", "long"] as const) {
      const setup = previewData(name, true).setup;
      expect(setup.profiles.some((p) => p.status === "active" && p.dispatchable)).toBe(false);
    }
  });
});

describe("預覽用戶端", () => {
  it("讀取方法回傳資料集的內容", async () => {
    const data = previewData("normal", false);
    const client = createPreviewClient(data);
    expect(await client.getTaskBoard("ws_1")).toBe(data.board);
    expect((await client.listWorkspaces()).workspaces).toBe(data.workspaces);
  });

  it("沒有提供的方法拒絕並在 console 記錄，用戶端本身不是 thenable", async () => {
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    const client = createPreviewClient(previewData("normal", false));
    await expect(client.createWorkspace({ path: "C:\\x" })).rejects.toThrow(unsupportedMessage("createWorkspace"));
    expect(error).toHaveBeenCalledWith(`[preview] ${unsupportedMessage("createWorkspace")}`);
    expect(await Promise.resolve(client)).toBe(client);
    error.mockRestore();
  });
});
