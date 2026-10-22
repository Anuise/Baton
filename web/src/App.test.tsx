import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { App } from "./App";
import {
  authorityTable,
  availableUpdateStatus,
  conversationView,
  decisionView,
  lifecycleStatus,
  metricsResponse,
  reviewList,
  understandingView,
  workflowList,
} from "./fixtures";
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

/** 相當於使用者在系統匣按「完全退出」。 */
let emitFullExit: () => void = () => {};

const client = {
  getHealth: vi.fn(async () => ({ status: "ok", product: "Baton", version: "0.1.0", boot_count: 1, started_at: 0, pid: 1 })),
  getPlatform: vi.fn(async () => {
    throw new Error("離線");
  }),
  listWorkspaces: vi.fn(async () => ({ workspaces: [workspace] })),
  getAuthorization: vi.fn(async () => {
    throw new Error("授權讀取失敗");
  }),
  getTaskBoard: vi.fn(async () => ({ workspace_id: "ws_1", goals: [], tasks: [], runs: [], capabilities: [], verifications: [], workdirs: [], integrations: [], budgets: [] })),
  lifecycleStatus: vi.fn(async () => {
    throw new Error("離線");
  }),
  requestFullExit: vi.fn(async () => lifecycleStatus({ phase: "stopping", stop_intent_active: true })),
  onFullExitRequested: vi.fn((handler: () => void) => {
    emitFullExit = handler;
    return () => {};
  }),
  listDecisions: vi.fn(async () => ({
    pending_count: 1,
    overview: { pending: 1, actionable: 1, deferred: 0, overdue: 0, high_urgency: 0, answered: 0, superseded: 0, merged: 0 },
    decisions: [decisionView()],
  })),
  answerDecision: vi.fn(async () => ({ decision: decisionView({ status: "answered", allowed_actions: [] }), resumed_task_ids: [] })),
  decisionAuthority: vi.fn(async () => authorityTable()),
  pendingDecisions: vi.fn(async () => ({ pending_count: 1, by_workspace: [{ workspace_id: "ws_1", pending_count: 1 }] })),
  listArtifacts: vi.fn(async () => ({ artifacts: [], unregistered_files: [] })),
  backupStatus: vi.fn(async () => ({ backups: [], latest_drill: null })),
  recoveryStatus: vi.fn(async () => ({ event: null, max_attempts: 3, can_retry: false, blocking: null })),
  listKnowledge: vi.fn(async () => ({ search_mode: "full_text", entries: [] })),
  listGuards: vi.fn(async () => ({ guards: [] })),
  timeline: vi.fn(async () => ({ workspace_id: "ws_1", task_id: null, events: [] })),
  metrics: vi.fn(async () => metricsResponse()),
  syncUnderstanding: vi.fn(async () => understandingView()),
  workflows: vi.fn(async () => workflowList()),
  reviews: vi.fn(async () => reviewList()),
  getConversation: vi.fn(async () => conversationView()),
  updateStatus: vi.fn(async () => availableUpdateStatus()),
  applyUpdate: vi.fn(),
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

  it("系統匣要求完全退出時以覆蓋層顯示停止進度", async () => {
    render(<App />);
    await vi.waitFor(() => expect(client.onFullExitRequested).toHaveBeenCalled());

    act(() => emitFullExit());

    expect(await screen.findByRole("dialog", { name: "完全退出" })).toHaveTextContent("停止中");
    expect(client.requestFullExit).toHaveBeenCalledTimes(1);
  });

  it("有新版本時在主畫面通知、在設定區顯示更新狀態，不會自己開始更新（spec D-19）", async () => {
    render(<App />);

    expect(await screen.findByText("Baton 0.2.0 可以更新")).toBeInTheDocument();
    expect(await screen.findByText("狀態：有新版本 0.2.0")).toBeInTheDocument();
    expect(client.applyUpdate).not.toHaveBeenCalled();
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

  it("在裁定中心處理後，狀態列立即重新讀取待裁定數（spec D-21）", async () => {
    const user = userEvent.setup();
    render(<App />);
    await user.click(await screen.findByRole("button", { name: "裁定 筆記" }));
    await user.click(await screen.findByRole("radio", { name: "駁回" }));
    const before = vi.mocked(client.pendingDecisions).mock.calls.length;

    await user.click(screen.getByRole("button", { name: "送出裁定" }));

    expect(client.answerDecision).toHaveBeenCalledWith("ws_1", "dec_1", { action: "reject", expected_revision: 1 });
    await waitFor(() => expect(vi.mocked(client.pendingDecisions).mock.calls.length).toBeGreaterThan(before));
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

  it("可從 Workspace 清單開啟時間線，並切回其他檢視", async () => {
    const user = userEvent.setup();
    render(<App />);

    await user.click(await screen.findByRole("button", { name: "時間線 筆記" }));
    expect(await screen.findByRole("heading", { name: "時間線：筆記" })).toBeInTheDocument();
    expect(client.timeline).toHaveBeenCalledWith("ws_1", undefined);
    expect(client.metrics).toHaveBeenCalledWith("ws_1");

    await user.click(screen.getByRole("button", { name: "知識 筆記" }));
    expect(await screen.findByRole("heading", { name: "知識：筆記" })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "時間線：筆記" })).not.toBeInTheDocument();
  });

  it("可從 Workspace 清單開啟專案理解（開啟時重新比對投影），並切回其他檢視", async () => {
    const user = userEvent.setup();
    render(<App />);

    await user.click(await screen.findByRole("button", { name: "專案理解 筆記" }));
    expect(await screen.findByRole("heading", { name: "專案理解：筆記" })).toBeInTheDocument();
    expect(client.syncUnderstanding).toHaveBeenCalledWith("ws_1");

    await user.click(screen.getByRole("button", { name: "時間線 筆記" }));
    expect(await screen.findByRole("heading", { name: "時間線：筆記" })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "專案理解：筆記" })).not.toBeInTheDocument();
  });

  it("可從 Workspace 清單開啟流程，並切回其他檢視", async () => {
    const user = userEvent.setup();
    render(<App />);

    await user.click(await screen.findByRole("button", { name: "流程 筆記" }));
    expect(await screen.findByRole("heading", { name: "流程：筆記" })).toBeInTheDocument();
    expect(client.workflows).toHaveBeenCalledWith("ws_1");
    expect(await screen.findByRole("heading", { name: "缺陷處理 第 1 版" })).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "專案理解 筆記" }));
    expect(await screen.findByRole("heading", { name: "專案理解：筆記" })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "流程：筆記" })).not.toBeInTheDocument();
  });

  it("可從 Workspace 清單開啟審查，並切回其他檢視", async () => {
    const user = userEvent.setup();
    render(<App />);

    await user.click(await screen.findByRole("button", { name: "審查 筆記" }));
    expect(await screen.findByRole("heading", { name: "審查與設計偏離：筆記" })).toBeInTheDocument();
    expect(client.reviews).toHaveBeenCalledWith("ws_1");
    expect(await screen.findByRole("heading", { name: "改用串流匯出" })).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "流程 筆記" }));
    expect(await screen.findByRole("heading", { name: "流程：筆記" })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "審查與設計偏離：筆記" })).not.toBeInTheDocument();
  });

  it("可從 Workspace 清單開啟對話，並切回其他檢視", async () => {
    const user = userEvent.setup();
    render(<App />);

    await user.click(await screen.findByRole("button", { name: "對話 筆記" }));
    expect(await screen.findByRole("heading", { name: "對話：筆記" })).toBeInTheDocument();
    expect(client.getConversation).toHaveBeenCalledWith("ws_1");

    await user.click(screen.getByRole("button", { name: "工作面板 筆記" }));
    expect(await screen.findByRole("heading", { name: "工作面板：筆記" })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "對話：筆記" })).not.toBeInTheDocument();
  });
});
