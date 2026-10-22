import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { ModelOverridesPanel } from "./ModelOverridesPanel";
import type { ModelOverrides, Workspace, WorkspaceClient } from "./runtimeClient";

const NONE: ModelOverrides = { controller: null, worker: null, reviewer: null };

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

type PanelClient = Pick<WorkspaceClient, "listWorkspaces" | "setModelOverrides">;

/** 以記憶體模擬 Runtime：保存後再讀取會看到新值。 */
function fakeClient(initial: Workspace) {
  let current = initial;
  const client: PanelClient = {
    listWorkspaces: vi.fn(async () => ({ workspaces: [current] })),
    setModelOverrides: vi.fn(async (_id, overrides) => {
      current = { ...current, version: current.version + 1, model_overrides: overrides };
      return current;
    }),
  };
  return client;
}

describe("模型設定", () => {
  it("顯示 Runtime 目前的覆寫值，並說明留空的意義與只影響之後的 Run", async () => {
    // 側欄傳進來的可能是過時的 Workspace，畫面以 Runtime 的最新值為準
    const client = fakeClient(workspace({ model_overrides: { controller: "opus", worker: null, reviewer: "sonnet" } }));
    render(<ModelOverridesPanel client={client} workspace={workspace()} />);

    expect(await screen.findByLabelText("Controller 模型")).toHaveValue("opus");
    expect(screen.getByLabelText("Worker 模型")).toHaveValue("");
    expect(screen.getByLabelText("Worker 模型")).toHaveAttribute("placeholder", "留空＝沿用帳號的 Claude Code 預設模型");
    expect(screen.getByLabelText("Reviewer 模型")).toHaveValue("sonnet");
    expect(screen.getByText(/只影響之後派出的 Run/)).toBeInTheDocument();
  });

  it("儲存時送出三個角色，留空或只有空白的送 null，成功後顯示新值", async () => {
    const user = userEvent.setup();
    const client = fakeClient(workspace({ model_overrides: { controller: "opus", worker: null, reviewer: "sonnet" } }));
    render(<ModelOverridesPanel client={client} workspace={workspace()} />);

    await user.clear(await screen.findByLabelText("Controller 模型"));
    await user.type(screen.getByLabelText("Worker 模型"), " claude-sonnet-4-5 ");
    await user.clear(screen.getByLabelText("Reviewer 模型"));
    await user.type(screen.getByLabelText("Reviewer 模型"), "   ");
    await user.click(screen.getByRole("button", { name: "儲存模型設定" }));

    expect(client.setModelOverrides).toHaveBeenCalledWith("ws_1", {
      controller: null,
      worker: "claude-sonnet-4-5",
      reviewer: null,
    });
    expect(await screen.findByRole("status")).toHaveTextContent("已保存模型設定");
    expect(screen.getByLabelText("Worker 模型")).toHaveValue("claude-sonnet-4-5");
    expect(screen.getByLabelText("Reviewer 模型")).toHaveValue("");
  });

  it("Runtime 拒絕時顯示它的說明並保留輸入", async () => {
    const user = userEvent.setup();
    const client = fakeClient(workspace());
    client.setModelOverrides = vi.fn(async () => {
      throw new Error("模型名稱「opus;rm」含有不允許的字元");
    });
    render(<ModelOverridesPanel client={client} workspace={workspace()} />);

    await user.type(await screen.findByLabelText("Worker 模型"), "opus;rm");
    await user.click(screen.getByRole("button", { name: "儲存模型設定" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("模型名稱「opus;rm」含有不允許的字元");
    expect(screen.getByLabelText("Worker 模型")).toHaveValue("opus;rm");
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });

  it("已封存的 Workspace 只顯示現值，不能變更", async () => {
    const archived = workspace({ status: "archived", archived_at: 1_700_000_000_001, model_overrides: { ...NONE, worker: "haiku" } });
    const client = fakeClient(archived);
    render(<ModelOverridesPanel client={client} workspace={workspace()} />);

    expect(await screen.findByLabelText("Worker 模型")).toHaveValue("haiku");
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
