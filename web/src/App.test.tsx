import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { App } from "./App";
import type { AppClient, Workspace } from "./runtimeClient";

const workspace: Workspace = {
  id: "ws_1",
  name: "筆記",
  path: "C:\\work\\notes",
  status: "active",
  version: 1,
  created_at: 1_700_000_000_000,
  archived_at: null,
  model_overrides: { controller: null, worker: null, reviewer: null },
};

const client = {
  getHealth: vi.fn(async () => {
    throw new Error("離線");
  }),
  getPlatform: vi.fn(async () => {
    throw new Error("離線");
  }),
  listWorkspaces: vi.fn(async () => ({ workspaces: [workspace] })),
  getAuthorization: vi.fn(async () => {
    throw new Error("授權讀取失敗");
  }),
  getTaskBoard: vi.fn(async () => ({ workspace_id: "ws_1", goals: [], tasks: [], runs: [], capabilities: [] })),
  listArtifacts: vi.fn(async () => ({ artifacts: [], unregistered_files: [] })),
} as unknown as AppClient;

vi.mock("./runtimeClient", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./runtimeClient")>()),
  createRuntimeClient: async () => client,
}));

describe("App", () => {
  it("選到 Workspace 後可在授權設定與工作面板之間切換", async () => {
    const user = userEvent.setup();
    render(<App />);

    await user.click(await screen.findByRole("button", { name: "工作面板 筆記" }));
    expect(await screen.findByRole("heading", { name: "工作面板：筆記" })).toBeInTheDocument();
    expect(client.getTaskBoard).toHaveBeenCalledWith("ws_1");
    expect(client.getAuthorization).not.toHaveBeenCalled();

    await user.click(screen.getByRole("button", { name: "授權設定 筆記" }));
    expect(await screen.findByText(/授權讀取失敗/)).toBeInTheDocument();
    // 模型設定與授權設定放在同一個 Workspace 設定畫面
    expect(await screen.findByRole("heading", { name: "模型設定：筆記" })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "工作面板：筆記" })).not.toBeInTheDocument();
  });

  it("可從 Workspace 清單開啟成果與驗證，並切回其他檢視", async () => {
    const user = userEvent.setup();
    render(<App />);

    await user.click(await screen.findByRole("button", { name: "成果與驗證 筆記" }));
    expect(await screen.findByRole("heading", { name: "成果與驗證：筆記" })).toBeInTheDocument();
    expect(client.listArtifacts).toHaveBeenCalledWith("ws_1");

    await user.click(screen.getByRole("button", { name: "工作面板 筆記" }));
    expect(await screen.findByRole("heading", { name: "工作面板：筆記" })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "成果與驗證：筆記" })).not.toBeInTheDocument();
  });
});
