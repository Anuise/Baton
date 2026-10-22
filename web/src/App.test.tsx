import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { App } from "./App";
import { decisionView } from "./fixtures";
import type { AppClient, Workspace } from "./runtimeClient";

const workspace: Workspace = {
  id: "ws_1",
  name: "筆記",
  path: "C:\\work\\notes",
  status: "active",
  version: 1,
  created_at: 1_700_000_000_000,
  archived_at: null,
};

const client = {
  getHealth: vi.fn(async () => ({ status: "ok", product: "Baton", version: "0.1.0", boot_count: 1, started_at: 0, pid: 1 })),
  getPlatform: vi.fn(async () => {
    throw new Error("離線");
  }),
  listWorkspaces: vi.fn(async () => ({ workspaces: [workspace] })),
  getAuthorization: vi.fn(async () => {
    throw new Error("授權讀取失敗");
  }),
  getTaskBoard: vi.fn(async () => ({ workspace_id: "ws_1", goals: [], tasks: [], runs: [], capabilities: [] })),
  listDecisions: vi.fn(async () => ({ pending_count: 1, decisions: [decisionView()] })),
  pendingDecisions: vi.fn(async () => ({ pending_count: 1, by_workspace: [{ workspace_id: "ws_1", pending_count: 1 }] })),
  listArtifacts: vi.fn(async () => ({ artifacts: [], unregistered_files: [] })),
  listKnowledge: vi.fn(async () => ({ search_mode: "full_text", entries: [] })),
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
    expect(screen.queryByRole("heading", { name: "工作面板：筆記" })).not.toBeInTheDocument();
  });

  it("「裁定」入口開啟裁定中心，並在狀態列與側欄顯示待裁定數", async () => {
    const user = userEvent.setup();
    render(<App />);

    expect(await screen.findByText("待裁定：1")).toBeInTheDocument();
    expect(await screen.findByText("待裁定 1")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "裁定 筆記" }));

    expect(await screen.findByRole("heading", { name: "裁定：筆記" })).toBeInTheDocument();
    expect(client.listDecisions).toHaveBeenCalledWith("ws_1");
    expect(screen.getByRole("heading", { name: "要保留舊的目錄結構嗎？" })).toBeInTheDocument();
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

  it("可從 Workspace 清單開啟知識，並切回其他檢視", async () => {
    const user = userEvent.setup();
    render(<App />);

    await user.click(await screen.findByRole("button", { name: "知識 筆記" }));
    expect(await screen.findByRole("heading", { name: "知識：筆記" })).toBeInTheDocument();
    expect(client.listKnowledge).toHaveBeenCalledWith(expect.objectContaining({ workspaceId: "ws_1" }));

    await user.click(screen.getByRole("button", { name: "成果與驗證 筆記" }));
    expect(await screen.findByRole("heading", { name: "成果與驗證：筆記" })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "知識：筆記" })).not.toBeInTheDocument();
  });
});
