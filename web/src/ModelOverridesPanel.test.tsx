import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { ModelOverridesPanel } from "./ModelOverridesPanel";
import type { ModelCatalog, ModelCatalogClient, ModelOverrides, Workspace, WorkspaceClient } from "./runtimeClient";

const NONE: ModelOverrides = { controller: null, worker: null, reviewer: null };

const CATALOG: ModelCatalog = {
  models: [
    { id: "opus", label: "Opus（最新版）" },
    { id: "sonnet", label: "Sonnet（最新版）" },
    { id: "claude-haiku-5-5", label: "Haiku 5.5" },
  ],
};

function workspace(overrides: Partial<Workspace> = {}): Workspace {
  return {
    id: "ws_1",
    name: "筆記",
    path: "C:\\work\\notes",
    status: "active",
    version: 1,
    created_at: 1_700_000_000_000,
    archived_at: null,
    model_overrides: NONE,
    ...overrides,
  };
}

type PanelClient = Pick<WorkspaceClient, "listWorkspaces" | "setModelOverrides"> & ModelCatalogClient;

/** 以記憶體模擬 Runtime：保存後再讀取會看到新值。 */
function fakeClient(initial: Workspace) {
  let current = initial;
  const client: PanelClient = {
    listWorkspaces: vi.fn(async () => ({ workspaces: [current] })),
    setModelOverrides: vi.fn(async (_id, overrides) => {
      current = { ...current, version: current.version + 1, model_overrides: overrides };
      return current;
    }),
    listModels: vi.fn(async () => CATALOG),
  };
  return client;
}

const optionTexts = (label: string) =>
  within(screen.getByLabelText(label))
    .getAllByRole("option")
    .map((option) => option.textContent);

describe("模型設定", () => {
  it("每個角色都是下拉選單，選項是「預設（不指定）」加上 Runtime 提供的模型，並選取目前的覆寫值", async () => {
    // 側欄傳進來的可能是過時的 Workspace，畫面以 Runtime 的最新值為準
    const client = fakeClient(workspace({ model_overrides: { controller: "opus", worker: null, reviewer: "sonnet" } }));
    render(<ModelOverridesPanel client={client} workspace={workspace()} />);

    const controller = await screen.findByLabelText("Controller 模型");
    expect(controller.tagName).toBe("SELECT");
    expect(controller).toHaveValue("opus");
    expect(screen.getByLabelText("Worker 模型")).toHaveValue("");
    expect(screen.getByLabelText("Reviewer 模型")).toHaveValue("sonnet");
    for (const label of ["Controller 模型", "Worker 模型", "Reviewer 模型"]) {
      expect(optionTexts(label)).toEqual([
        "預設（不指定）",
        "Opus（最新版）（opus）",
        "Sonnet（最新版）（sonnet）",
        "Haiku 5.5（claude-haiku-5-5）",
      ]);
    }
    expect(screen.getByText(/沿用帳號的 Claude Code 預設模型/)).toBeInTheDocument();
    expect(screen.getByText(/只影響之後派出的 Run/)).toBeInTheDocument();
  });

  it("儲存時送出三個角色，選「預設（不指定）」的送 null，成功後顯示新值", async () => {
    const user = userEvent.setup();
    const client = fakeClient(workspace({ model_overrides: { controller: "opus", worker: null, reviewer: "sonnet" } }));
    render(<ModelOverridesPanel client={client} workspace={workspace()} />);

    await user.selectOptions(await screen.findByLabelText("Controller 模型"), "預設（不指定）");
    await user.selectOptions(screen.getByLabelText("Worker 模型"), "Haiku 5.5（claude-haiku-5-5）");
    await user.click(screen.getByRole("button", { name: "儲存模型設定" }));

    expect(client.setModelOverrides).toHaveBeenCalledWith("ws_1", {
      controller: null,
      worker: "claude-haiku-5-5",
      reviewer: "sonnet",
    });
    expect(await screen.findByRole("status")).toHaveTextContent("已保存模型設定");
    expect(screen.getByLabelText("Controller 模型")).toHaveValue("");
    expect(screen.getByLabelText("Worker 模型")).toHaveValue("claude-haiku-5-5");
  });

  it("已存的值不在清單中時仍顯示為選取的選項，換掉後還能選回來，存檔不會清掉它", async () => {
    const user = userEvent.setup();
    const client = fakeClient(workspace({ model_overrides: { ...NONE, worker: "claude-sonnet-4-5[1m]" } }));
    render(<ModelOverridesPanel client={client} workspace={workspace()} />);

    const worker = await screen.findByLabelText("Worker 模型");
    expect(worker).toHaveValue("claude-sonnet-4-5[1m]");
    expect(optionTexts("Worker 模型")).toContain("claude-sonnet-4-5[1m]（目前的設定，不在可用清單中）");
    // 其他角色不會多出這個選項
    expect(optionTexts("Controller 模型")).not.toContain("claude-sonnet-4-5[1m]（目前的設定，不在可用清單中）");

    await user.selectOptions(worker, "opus");
    await user.selectOptions(worker, "claude-sonnet-4-5[1m]");
    await user.click(screen.getByRole("button", { name: "儲存模型設定" }));

    expect(client.setModelOverrides).toHaveBeenCalledWith("ws_1", { ...NONE, worker: "claude-sonnet-4-5[1m]" });
    expect(await screen.findByRole("status")).toHaveTextContent("已保存模型設定");
    expect(screen.getByLabelText("Worker 模型")).toHaveValue("claude-sonnet-4-5[1m]");
  });

  it("取不到可用模型清單時頁面照常運作：仍可選預設或保留目前的值，並說明原因", async () => {
    const user = userEvent.setup();
    const client = fakeClient(workspace({ model_overrides: { ...NONE, reviewer: "sonnet" } }));
    client.listModels = vi.fn(async () => {
      throw new Error("Runtime 離線");
    });
    render(<ModelOverridesPanel client={client} workspace={workspace()} />);

    expect(await screen.findByText(/無法取得可用模型清單：Runtime 離線/)).toBeInTheDocument();
    expect(screen.getByLabelText("Reviewer 模型")).toHaveValue("sonnet");
    expect(optionTexts("Worker 模型")).toEqual(["預設（不指定）"]);

    await user.selectOptions(screen.getByLabelText("Reviewer 模型"), "預設（不指定）");
    await user.click(screen.getByRole("button", { name: "儲存模型設定" }));

    expect(client.setModelOverrides).toHaveBeenCalledWith("ws_1", NONE);
    expect(await screen.findByRole("status")).toHaveTextContent("已保存模型設定");
  });

  it("Runtime 拒絕時顯示它的說明並保留選擇", async () => {
    const user = userEvent.setup();
    const client = fakeClient(workspace());
    client.setModelOverrides = vi.fn(async () => {
      throw new Error("Workspace 已封存，不能變更模型設定");
    });
    render(<ModelOverridesPanel client={client} workspace={workspace()} />);

    await user.selectOptions(await screen.findByLabelText("Worker 模型"), "sonnet");
    await user.click(screen.getByRole("button", { name: "儲存模型設定" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("Workspace 已封存，不能變更模型設定");
    expect(screen.getByLabelText("Worker 模型")).toHaveValue("sonnet");
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });

  it("已封存的 Workspace 只顯示現值，不能變更", async () => {
    const archived = workspace({ status: "archived", archived_at: 1_700_000_000_001, model_overrides: { ...NONE, worker: "opus" } });
    const client = fakeClient(archived);
    render(<ModelOverridesPanel client={client} workspace={workspace()} />);

    expect(await screen.findByLabelText("Worker 模型")).toHaveValue("opus");
    expect(screen.getByLabelText("Worker 模型")).toBeDisabled();
    expect(screen.getByRole("button", { name: "儲存模型設定" })).toBeDisabled();
    expect(screen.getByText(/已封存/)).toBeInTheDocument();
  });

  it("讀不到 Workspace 時說明原因", async () => {
    const client = fakeClient(workspace());
    client.listWorkspaces = vi.fn(async () => {
      throw new Error("Runtime 離線");
    });
    render(<ModelOverridesPanel client={client} workspace={workspace()} />);

    expect(await screen.findByRole("alert")).toHaveTextContent("無法取得模型設定：Runtime 離線");
  });
});
