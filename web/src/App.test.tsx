import { act, cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeAll, beforeEach, describe, expect, it, onTestFinished, vi } from "vitest";

import { App, SHELL_STORAGE_KEY, workspaceSectionTabs } from "./App";
import {
  authorityTable,
  availableUpdateStatus,
  decisionView,
  explorationOverview,
  lifecycleStatus,
  metricsResponse,
  profile,
  reviewList,
  schedulingOverview,
  setupStatus,
  verificationOverview,
  understandingView,
  workflowList,
} from "./fixtures";
import type { AppClient, AuthorizationView, Capability, Workspace } from "./runtimeClient";

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

const other: Workspace = { ...workspace, id: "ws_2", name: "文件", path: "C:\\work\\docs" };

/** 只有一項已確認能力的授權設定。 */
function authorizationOf(target: Workspace, capability: Capability): AuthorizationView {
  const revision = {
    workspace_id: target.id,
    revision: 1,
    grants: [{ capability, targets: [target.path], restrictions: [] }],
    source: { kind: "custom" as const, template_id: null, base_revision: null, confirmed_by: "desktop" },
    confirmed_at: 1_700_000_000_000,
  };
  return {
    workspace_id: target.id,
    status: "confirmed",
    effective: revision,
    revisions: [revision],
    templates: [],
    capabilities: [
      { capability: "read_files", label: "讀取檔案", high_risk: false },
      { capability: "deploy", label: "部署", high_risk: true },
    ],
    risk_disclosure: "",
    legacy_runs: [],
  };
}

/** 相當於使用者在系統匣按「完全退出」。 */
let emitFullExit: () => void = () => {};

/** Runtime 目前的待裁定數；回答裁定後歸零。 */
let pendingCount = 1;

const client = {
  getHealth: vi.fn(async () => ({ status: "ok", product: "Baton", version: "0.1.0", boot_count: 1, started_at: 0, pid: 1 })),
  getPlatform: vi.fn(async () => {
    throw new Error("離線");
  }),
  // 預設已有可派工的 Profile，而且首次設定已完成
  getSetup: vi.fn(async () => setupStatus({ background_notice: { text: "", acknowledged_at: 1 } })),
  recheckSetup: vi.fn(async () => setupStatus({ background_notice: { text: "", acknowledged_at: 1 } })),
  listWorkspaces: vi.fn(async () => ({ workspaces: [workspace] })),
  archiveWorkspace: vi.fn(async () => ({ ...workspace, status: "archived", archived_at: 1 })),
  getAuthorization: vi.fn(async () => {
    throw new Error("授權讀取失敗");
  }),
  getTaskBoard: vi.fn(async () => ({ workspace_id: "ws_1", goals: [], tasks: [], runs: [], capabilities: [], verifications: [], workdirs: [], integrations: [], budgets: [] })),
  lifecycleStatus: vi.fn(async () => {
    throw new Error("離線");
  }),
  requestFullExit: vi.fn(async () => lifecycleStatus({ phase: "stopping", stop_intent_active: true })),
  cancelFullExitFallback: vi.fn(async () => {}),
  onFullExitRequested: vi.fn((handler: () => void) => {
    emitFullExit = handler;
    return () => {};
  }),
  listDecisions: vi.fn(async () => ({
    pending_count: 1,
    overview: { pending: 1, actionable: 1, deferred: 0, overdue: 0, high_urgency: 0, answered: 0, superseded: 0, merged: 0 },
    decisions: [decisionView()],
  })),
  answerDecision: vi.fn(async () => {
    pendingCount = 0;
    return { decision: decisionView({ status: "answered", allowed_actions: [] }), resumed_task_ids: [] };
  }),
  decisionAuthority: vi.fn(async () => authorityTable()),
  pendingDecisions: vi.fn(async () => ({
    pending_count: pendingCount,
    by_workspace: pendingCount > 0 ? [{ workspace_id: "ws_1", pending_count: pendingCount }] : [],
  })),
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
  verificationOverview: vi.fn(async () => verificationOverview()),
  explorationOverview: vi.fn(async () => explorationOverview()),
  getControllerCli: vi.fn(async () => ({ workspace_id: "ws_1", run: null, playbook_version: "1" })),
  listProfiles: vi.fn(async () => ({ profiles: [profile()], limit: 8 })),
  schedulingOverview: vi.fn(async () => schedulingOverview()),
  updateStatus: vi.fn(async () => availableUpdateStatus()),
  applyUpdate: vi.fn(),
} as unknown as AppClient;

vi.mock("./runtimeClient", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./runtimeClient")>()),
  createRuntimeClient: async () => client,
}));

/** 只影響目前這個測試：結束後還原成模組層級的預設行為。 */
function onlyInThisTest(...mocks: unknown[]) {
  onTestFinished(() => {
    for (const mock of mocks) vi.mocked(mock as () => unknown).mockReset();
  });
}

const topTab = (name: string) => within(screen.getByRole("tablist", { name: "主要區塊" })).getByRole("tab", { name });
const workspaceTab = (name: string) =>
  within(screen.getByRole("tablist", { name: "Workspace 功能" })).getByRole("tab", { name });
/** 「成果與品質」「更多」入口內的第二層頁簽。 */
const pageTab = (section: string, name: string) =>
  within(screen.getByRole("tablist", { name: section })).getByRole("tab", { name });

/** 依「入口／頁面」開啟 Workspace 的某一頁；只有一頁的入口不帶頁面。 */
async function openPage(user: ReturnType<typeof userEvent.setup>, section: string, page: string | null) {
  await user.click(workspaceTab(section));
  if (page) await user.click(pageTab(section, page));
}

/** 從左側清單選取 Workspace。 */
async function selectWorkspace(user: ReturnType<typeof userEvent.setup>, name = "筆記") {
  await user.click(await screen.findByRole("button", { name: `選擇 ${name}` }));
}

describe("App 外殼", () => {
  // App 第一次 render 會初次執行側欄與全部設定頁元件；全部前端測試平行執行時這段冷啟動實測可超過 1 秒，
  // 會吃掉第一個測試 findBy* 的預設等待時間。先在這裡暖機一次，各測試的等待只反映畫面本身的反應。
  beforeAll(async () => {
    render(<App />);
    await screen.findByRole("button", { name: "選擇 筆記" }, { timeout: 10_000 });
    cleanup();
  });

  // 各測試以呼叫紀錄判斷行為（例如完全退出的事件訂閱），不能沿用暖機或前一個測試留下的紀錄；
  // 記住的頁簽也不能帶到下一個測試
  beforeEach(() => {
    vi.clearAllMocks();
    localStorage.clear();
    pendingCount = 1;
  });

  describe("頂層頁簽", () => {
    it("頂層只有 Workspace、帳號、設定三個頁簽，預設在 Workspace 並提示選擇 Workspace", async () => {
      render(<App />);

      const tabs = within(await screen.findByRole("tablist", { name: "主要區塊" })).getAllByRole("tab");
      expect(tabs.map((tab) => tab.textContent)).toEqual(["Workspace", "帳號", "設定"]);
      expect(topTab("Workspace")).toHaveAttribute("aria-selected", "true");
      expect(screen.getByText("請從左側選擇或建立 Workspace")).toBeInTheDocument();
      expect(screen.getByRole("complementary", { name: "Workspace" })).toBeInTheDocument();
      // 帳號與系統設定不再疊在主畫面
      expect(screen.queryByRole("heading", { name: "帳號" })).not.toBeInTheDocument();
      expect(screen.queryByRole("heading", { name: "平台狀態" })).not.toBeInTheDocument();
      expect(screen.queryByRole("tablist", { name: "Workspace 功能" })).not.toBeInTheDocument();
    });

    it("帳號頁簽只顯示首次設定、帳號、排程與共同設定", async () => {
      const user = userEvent.setup();
      render(<App />);

      await user.click(await screen.findByRole("tab", { name: "帳號" }));

      expect(topTab("帳號")).toHaveAttribute("aria-selected", "true");
      expect(await screen.findByRole("heading", { name: "帳號" })).toBeInTheDocument();
      expect(screen.getByRole("heading", { name: "並行容量" })).toBeInTheDocument();
      expect(screen.getByRole("heading", { name: "共同設定" })).toBeInTheDocument();
      expect(await screen.findByRole("region", { name: "首次設定" })).toBeInTheDocument();
      for (const name of ["登入時自動啟動", "更新", "備份與還原演練", "崩潰恢復", "平台狀態"]) {
        expect(screen.queryByRole("heading", { name })).not.toBeInTheDocument();
      }
      // 左側 Workspace 清單只屬於 Workspace 頁簽
      expect(screen.queryByRole("complementary", { name: "Workspace" })).not.toBeInTheDocument();
    });

    it("設定頁簽只顯示跨登入啟動、更新、備份、恢復與平台狀態", async () => {
      const user = userEvent.setup();
      render(<App />);

      await user.click(await screen.findByRole("tab", { name: "設定" }));

      for (const name of ["登入時自動啟動", "更新", "備份與還原演練", "崩潰恢復", "平台狀態"]) {
        expect(await screen.findByRole("heading", { name })).toBeInTheDocument();
      }
      for (const name of ["帳號", "並行容量", "共同設定"]) {
        expect(screen.queryByRole("heading", { name })).not.toBeInTheDocument();
      }
      expect(screen.queryByRole("complementary", { name: "Workspace" })).not.toBeInTheDocument();
    });
  });

  describe("Workspace 頁簽", () => {
    it("選中 Workspace 後只有五個主要入口，預設開在主控台，一次只顯示一個功能", async () => {
      const user = userEvent.setup();
      render(<App />);

      await selectWorkspace(user);

      const tabs = within(screen.getByRole("tablist", { name: "Workspace 功能" })).getAllByRole("tab");
      expect(tabs.map((tab) => tab.getAttribute("aria-label") ?? tab.textContent)).toEqual([
        "主控台",
        "工作",
        "裁定",
        "成果與品質",
        "更多",
      ]);
      expect(workspaceTab("主控台")).toHaveAttribute("aria-selected", "true");
      expect(screen.getByRole("tabpanel", { name: "主控台" })).toHaveTextContent("主控台：筆記");
      expect(client.getControllerCli).toHaveBeenCalledWith("ws_1");
      expect(screen.queryByText("請從左側選擇或建立 Workspace")).not.toBeInTheDocument();
      expect(screen.getByRole("button", { name: "選擇 筆記" })).toHaveAttribute("aria-current", "true");
      // 只有一頁的入口沒有第二層頁簽
      expect(screen.queryAllByRole("tablist")).toHaveLength(2);
      // 其他功能尚未開啟，不會讀取
      expect(client.getTaskBoard).not.toHaveBeenCalled();
      expect(client.getAuthorization).not.toHaveBeenCalled();
    });

    it("「成果與品質」與「更多」以第二層頁簽列出原本的頁面", async () => {
      const user = userEvent.setup();
      render(<App />);
      await selectWorkspace(user);

      await user.click(workspaceTab("成果與品質"));
      const quality = screen.getByRole("tablist", { name: "成果與品質" });
      expect(within(quality).getAllByRole("tab").map((tab) => tab.textContent)).toEqual(["成果", "審查", "驗證", "探索"]);
      expect(pageTab("成果與品質", "成果")).toHaveAttribute("aria-selected", "true");

      await user.click(workspaceTab("更多"));
      const more = screen.getByRole("tablist", { name: "更多" });
      expect(within(more).getAllByRole("tab").map((tab) => tab.textContent)).toEqual(["知識", "流程", "時間線", "授權"]);
      expect(pageTab("更多", "知識")).toHaveAttribute("aria-selected", "true");
      expect(screen.queryByRole("tablist", { name: "成果與品質" })).not.toBeInTheDocument();
    });

    // 依序切換入口內的每一頁，每切一次上一頁的標題就不在畫面上
    it.each([
      [
        "成果與品質",
        [
          ["成果", "成果"],
          ["審查", "審查與設計偏離"],
          ["驗證", "驗證"],
          ["探索", "探索"],
        ],
      ],
      [
        "更多",
        [
          ["知識", "知識：筆記"],
          ["流程", "流程：筆記"],
          ["時間線", "時間線：筆記"],
          ["授權", "模型設定：筆記"],
        ],
      ],
    ])("「%s」入口以第二層頁簽切換頁面，一次只顯示一頁", async (section, pages) => {
      const user = userEvent.setup();
      render(<App />);
      await selectWorkspace(user);
      await user.click(workspaceTab(section));

      let previous: string | null = null;
      for (const [page, heading] of pages) {
        await user.click(pageTab(section, page));
        expect(await screen.findByRole("heading", { name: heading })).toBeInTheDocument();
        if (previous) expect(screen.queryByRole("heading", { name: previous })).not.toBeInTheDocument();
        previous = heading;
      }
    });

    // 原本九個頁簽（含「審查與驗證」的三個小頁簽）的內容全部仍可到達
    it.each([
      ["工作", null, "工作：筆記", () => expect(client.getTaskBoard).toHaveBeenCalledWith("ws_1")],
      ["裁定", null, "裁定：筆記", () => expect(client.listDecisions).toHaveBeenCalledWith("ws_1")],
      ["成果與品質", "成果", "成果", () => expect(client.listArtifacts).toHaveBeenCalledWith("ws_1")],
      ["成果與品質", "審查", "審查與設計偏離", () => expect(client.reviews).toHaveBeenCalledWith("ws_1")],
      ["成果與品質", "驗證", "驗證", () => expect(client.verificationOverview).toHaveBeenCalledWith("ws_1")],
      ["成果與品質", "探索", "探索", () => expect(client.explorationOverview).toHaveBeenCalledWith("ws_1")],
      ["更多", "知識", "知識：筆記", () => expect(client.listKnowledge).toHaveBeenCalledWith(expect.objectContaining({ workspaceId: "ws_1" }))],
      ["更多", "流程", "流程：筆記", () => expect(client.workflows).toHaveBeenCalledWith("ws_1")],
      ["更多", "時間線", "時間線：筆記", () => expect(client.metrics).toHaveBeenCalledWith("ws_1")],
      ["更多", "授權", "模型設定：筆記", () => expect(client.getAuthorization).toHaveBeenCalledWith("ws_1")],
    ])("「%s」入口的「%s」頁開啟「%s」，且只顯示這個功能", async (section, page, heading, called) => {
      const user = userEvent.setup();
      render(<App />);
      await selectWorkspace(user);

      await openPage(user, section, page);

      expect(workspaceTab(section)).toHaveAttribute("aria-selected", "true");
      if (page) expect(pageTab(section, page)).toHaveAttribute("aria-selected", "true");
      expect(await screen.findByRole("heading", { name: heading })).toBeInTheDocument();
      called();
      expect(screen.queryByRole("tabpanel", { name: "主控台" })).not.toBeInTheDocument();
      expect(screen.queryByText("主控台：筆記")).not.toBeInTheDocument();
    });

    it("裁定入口開啟裁定中心", async () => {
      const user = userEvent.setup();
      render(<App />);
      await selectWorkspace(user);

      await user.click(workspaceTab("裁定"));

      expect(await screen.findByRole("heading", { name: "裁定：筆記" })).toBeInTheDocument();
      expect(screen.getByRole("heading", { name: "要保留舊的目錄結構嗎？" })).toBeInTheDocument();
    });

    it("知識頁同時提供知識與專案理解", async () => {
      const user = userEvent.setup();
      render(<App />);
      await selectWorkspace(user);

      await openPage(user, "更多", "知識");

      expect(await screen.findByRole("heading", { name: "知識：筆記" })).toBeInTheDocument();
      expect(await screen.findByRole("heading", { name: "專案理解：筆記" })).toBeInTheDocument();
      expect(client.syncUnderstanding).toHaveBeenCalledWith("ws_1");
    });

    it("授權頁同時提供授權設定與模型設定", async () => {
      const user = userEvent.setup();
      render(<App />);
      await selectWorkspace(user);

      await openPage(user, "更多", "授權");

      expect(await screen.findByText(/授權讀取失敗/)).toBeInTheDocument();
      expect(await screen.findByRole("heading", { name: "模型設定：筆記" })).toBeInTheDocument();
    });

    it("離開再回到同一個入口時停在上次的頁面", async () => {
      const user = userEvent.setup();
      render(<App />);
      await selectWorkspace(user);
      await openPage(user, "成果與品質", "驗證");
      await screen.findByRole("heading", { name: "驗證" });

      await user.click(workspaceTab("工作"));
      await user.click(workspaceTab("成果與品質"));

      expect(pageTab("成果與品質", "驗證")).toHaveAttribute("aria-selected", "true");
      expect(await screen.findByRole("heading", { name: "驗證" })).toBeInTheDocument();
    });

    it("切換 Workspace 時停在同一個頁簽", async () => {
      vi.mocked(client.listWorkspaces).mockImplementation(async () => ({ workspaces: [workspace, other] }));
      onlyInThisTest(client.listWorkspaces);
      const user = userEvent.setup();
      render(<App />);
      await selectWorkspace(user, "筆記");
      await openPage(user, "更多", "時間線");
      expect(await screen.findByRole("heading", { name: "時間線：筆記" })).toBeInTheDocument();

      await selectWorkspace(user, "文件");

      expect(await screen.findByRole("heading", { name: "時間線：文件" })).toBeInTheDocument();
      expect(workspaceTab("更多")).toHaveAttribute("aria-selected", "true");
      expect(pageTab("更多", "時間線")).toHaveAttribute("aria-selected", "true");
      expect(client.timeline).toHaveBeenLastCalledWith("ws_2", undefined);
    });

    it("選中的 Workspace 被封存後回到提示文字", async () => {
      vi.mocked(client.listWorkspaces)
        .mockImplementationOnce(async () => ({ workspaces: [workspace] }))
        .mockImplementation(async () => ({ workspaces: [{ ...workspace, status: "archived", archived_at: 1 }] }));
      onlyInThisTest(client.listWorkspaces);
      const user = userEvent.setup();
      render(<App />);
      await selectWorkspace(user);

      await user.click(screen.getByRole("button", { name: "封存 筆記" }));

      expect(await screen.findByText("請從左側選擇或建立 Workspace")).toBeInTheDocument();
      expect(screen.queryByRole("tablist", { name: "Workspace 功能" })).not.toBeInTheDocument();
    });

    it("切換 Workspace 後，上一個 Workspace 較晚回來的授權設定不會蓋掉目前的草稿", async () => {
      let resolveFirst: (view: AuthorizationView) => void = () => {};
      vi.mocked(client.listWorkspaces).mockImplementation(async () => ({ workspaces: [workspace, other] }));
      vi.mocked(client.getAuthorization)
        .mockImplementationOnce(() => new Promise((resolve) => (resolveFirst = resolve)))
        .mockImplementationOnce(async () => authorizationOf(other, "read_files"));
      onlyInThisTest(client.listWorkspaces, client.getAuthorization);
      const user = userEvent.setup();
      render(<App />);

      await selectWorkspace(user, "筆記");
      await openPage(user, "更多", "授權");
      await selectWorkspace(user, "文件");
      expect(await screen.findByRole("heading", { name: "授權設定：文件" })).toBeInTheDocument();
      // 「筆記」的回應這時才到
      await act(async () => resolveFirst(authorizationOf(workspace, "deploy")));

      expect(screen.getByRole("heading", { name: "授權設定：文件" })).toBeInTheDocument();
      expect(screen.getByRole("checkbox", { name: /^讀取檔案/ })).toBeChecked();
      expect(screen.getByRole("checkbox", { name: /^部署/ })).not.toBeChecked();
    });
  });

  describe("待裁定徽章", () => {
    it("裁定入口與側欄顯示待裁定數，回答裁定後隨之更新", async () => {
      const user = userEvent.setup();
      render(<App />);
      expect(await screen.findByText("待裁定：1")).toBeInTheDocument();
      expect(await screen.findByText("待裁定 1")).toBeInTheDocument();
      await selectWorkspace(user);
      expect(within(workspaceTab("裁定")).getByTitle("待裁定 1")).toHaveTextContent("1");

      await user.click(workspaceTab("裁定"));
      await user.click(await screen.findByRole("radio", { name: "駁回" }));
      await user.click(screen.getByRole("button", { name: "送出裁定" }));

      expect(client.answerDecision).toHaveBeenCalledWith("ws_1", "dec_1", { action: "reject", expected_revision: 1 });
      await waitFor(() => expect(screen.queryByText("待裁定 1")).not.toBeInTheDocument());
      expect(within(workspaceTab("裁定")).queryByTitle(/待裁定/)).not.toBeInTheDocument();
    });

    it("頁面有徽章時，所在入口加總顯示；「更多」內的頁面有徽章時「更多」入口也有提示", () => {
      const tabs = workspaceSectionTabs({ decisions: 2, timeline: 3 }, { decisions: "待裁定 2", timeline: "新事件 3" });

      expect(tabs.find((tab) => tab.id === "decisions")).toMatchObject({ badge: 2, badgeTitle: "待裁定 2" });
      expect(tabs.find((tab) => tab.id === "more")).toMatchObject({ badge: 3, badgeTitle: "新事件 3" });
      expect(tabs.find((tab) => tab.id === "quality")?.badge ?? 0).toBe(0);
    });
  });

  describe("記住所在位置", () => {
    it("重新開啟 App 回到上次的 Workspace 與頁簽", async () => {
      const user = userEvent.setup();
      const first = render(<App />);
      await selectWorkspace(user);
      await openPage(user, "更多", "流程");
      await screen.findByRole("heading", { name: "流程：筆記" });
      first.unmount();

      render(<App />);

      expect(await screen.findByRole("heading", { name: "流程：筆記" })).toBeInTheDocument();
      expect(workspaceTab("更多")).toHaveAttribute("aria-selected", "true");
      expect(pageTab("更多", "流程")).toHaveAttribute("aria-selected", "true");
    });

    it("重新開啟 App 回到上次的頂層頁簽", async () => {
      const user = userEvent.setup();
      const first = render(<App />);
      await user.click(await screen.findByRole("tab", { name: "設定" }));
      first.unmount();

      render(<App />);

      expect(await screen.findByRole("heading", { name: "平台狀態" })).toBeInTheDocument();
      expect(topTab("設定")).toHaveAttribute("aria-selected", "true");
    });

    it("記住的內容讀不懂時使用預設值，不報錯", async () => {
      localStorage.setItem(SHELL_STORAGE_KEY, "{不是 JSON");
      render(<App />);

      expect(await screen.findByText("請從左側選擇或建立 Workspace")).toBeInTheDocument();
      expect(topTab("Workspace")).toHaveAttribute("aria-selected", "true");
    });

    it("記住的頁簽名稱不認得時使用預設值", async () => {
      localStorage.setItem(SHELL_STORAGE_KEY, JSON.stringify({ top: "不存在", workspaceId: "ws_1", workspaceTab: "不存在" }));
      render(<App />);

      expect(await screen.findByRole("tabpanel", { name: "主控台" })).toHaveTextContent("主控台：筆記");
      expect(topTab("Workspace")).toHaveAttribute("aria-selected", "true");
    });

    it("記住的是已移除的「對話」頁簽時開在主控台", async () => {
      localStorage.setItem(SHELL_STORAGE_KEY, JSON.stringify({ top: "workspace", workspaceId: "ws_1", workspaceTab: "conversation" }));
      render(<App />);

      expect(await screen.findByRole("tabpanel", { name: "主控台" })).toHaveTextContent("主控台：筆記");
      expect(workspaceTab("主控台")).toHaveAttribute("aria-selected", "true");
    });

    // 舊版九個頁簽與「審查與驗證」小頁簽記住的位置，對應到新的入口與頁面，不會落到空白頁
    it.each([
      ["controller", "主控台", null],
      ["tasks", "工作", null],
      ["decisions", "裁定", null],
      ["artifacts", "成果與品質", "成果"],
      ["quality", "成果與品質", "審查"],
      ["reviews", "成果與品質", "審查"],
      ["verification", "成果與品質", "驗證"],
      ["exploration", "成果與品質", "探索"],
      ["knowledge", "更多", "知識"],
      ["workflows", "更多", "流程"],
      ["timeline", "更多", "時間線"],
      ["authorization", "更多", "授權"],
    ])("記住的舊位置「%s」開在「%s」入口的「%s」頁", async (saved, section, page) => {
      localStorage.setItem(SHELL_STORAGE_KEY, JSON.stringify({ top: "workspace", workspaceId: "ws_1", workspaceTab: saved }));
      render(<App />);

      expect(await screen.findByRole("tabpanel", { name: page ?? section })).not.toBeEmptyDOMElement();
      expect(workspaceTab(section)).toHaveAttribute("aria-selected", "true");
      if (page) expect(pageTab(section, page)).toHaveAttribute("aria-selected", "true");
      else expect(screen.queryAllByRole("tablist")).toHaveLength(2);
    });

    it("本機儲存無法使用時使用預設值，不報錯", async () => {
      const getItem = vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
        throw new Error("拒絕存取");
      });
      const setItem = vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
        throw new Error("拒絕存取");
      });
      onTestFinished(() => {
        getItem.mockRestore();
        setItem.mockRestore();
      });
      const user = userEvent.setup();
      render(<App />);

      expect(await screen.findByText("請從左側選擇或建立 Workspace")).toBeInTheDocument();
      await selectWorkspace(user);
      expect(workspaceTab("主控台")).toHaveAttribute("aria-selected", "true");
    });

    it("上次的 Workspace 已不在清單時顯示提示文字", async () => {
      localStorage.setItem(SHELL_STORAGE_KEY, JSON.stringify({ top: "workspace", workspaceId: "ws_gone", workspaceTab: "workflows" }));
      render(<App />);

      expect(await screen.findByText("請從左側選擇或建立 Workspace")).toBeInTheDocument();
    });
  });

  describe("首次精靈與頁簽之外的元素", () => {
    it("沒有可用的 Account Profile 時以全畫面顯示首次精靈與帳號，不顯示頁簽", async () => {
      vi.mocked(client.getSetup).mockImplementation(async () =>
        setupStatus({ profiles: [profile({ dispatchable: false })], ready: false }),
      );
      onlyInThisTest(client.getSetup);
      render(<App />);

      expect(await screen.findByRole("heading", { name: "首次設定" })).toBeInTheDocument();
      expect(screen.getByRole("heading", { name: "帳號" })).toBeInTheDocument();
      expect(screen.queryByRole("tablist", { name: "主要區塊" })).not.toBeInTheDocument();
      expect(screen.queryByRole("complementary", { name: "Workspace" })).not.toBeInTheDocument();
      // 狀態列與更新通知不受影響
      expect(await screen.findByText("待裁定：1")).toBeInTheDocument();
      expect(await screen.findByText("Baton 0.2.0 可以更新")).toBeInTheDocument();
    });

    it("首次精靈中出現可用的 Profile 後回到頁簽畫面", async () => {
      vi.mocked(client.getSetup)
        .mockImplementationOnce(async () => setupStatus({ profiles: [], ready: false }))
        .mockImplementation(async () => setupStatus({ background_notice: { text: "", acknowledged_at: 1 } }));
      vi.mocked(client.recheckSetup).mockImplementation(async () =>
        setupStatus({ background_notice: { text: "", acknowledged_at: 1 } }),
      );
      onlyInThisTest(client.getSetup, client.recheckSetup);
      const user = userEvent.setup();
      render(<App />);
      await screen.findByRole("region", { name: "首次設定" });

      await user.click(await screen.findByRole("button", { name: "重新檢查" }));

      expect(await screen.findByRole("tablist", { name: "主要區塊" })).toBeInTheDocument();
    });

    it("全畫面首次精靈可以先進入主畫面", async () => {
      vi.mocked(client.getSetup).mockImplementation(async () => setupStatus({ profiles: [], ready: false }));
      onlyInThisTest(client.getSetup);
      const user = userEvent.setup();
      render(<App />);

      await user.click(await screen.findByRole("button", { name: "先進入主畫面" }));

      expect(await screen.findByRole("tablist", { name: "主要區塊" })).toBeInTheDocument();
      expect(screen.getByRole("complementary", { name: "Workspace" })).toBeInTheDocument();
    });

    it("讀不到首次設定狀態時照常顯示頁簽", async () => {
      vi.mocked(client.getSetup).mockImplementation(async () => {
        throw new Error("離線");
      });
      onlyInThisTest(client.getSetup);
      render(<App />);

      expect(await screen.findByRole("tablist", { name: "主要區塊" })).toBeInTheDocument();
    });

    it("系統匣要求完全退出時以覆蓋層顯示停止進度", async () => {
      render(<App />);
      await vi.waitFor(() => expect(client.onFullExitRequested).toHaveBeenCalled());

      act(() => emitFullExit());

      expect(await screen.findByRole("dialog", { name: "完全退出" })).toHaveTextContent("停止中");
      expect(client.requestFullExit).toHaveBeenCalledTimes(1);
    });

    it("指定 createClient 時改用它建立用戶端（開發用預覽注入假資料）", async () => {
      const injected = {
        ...client,
        getHealth: vi.fn(async () => ({ status: "ok", product: "預覽產品", version: "0.1.0", boot_count: 1, started_at: 0, pid: 1 })),
      } as AppClient;
      render(<App createClient={async () => injected} />);

      expect(await screen.findByRole("heading", { name: "預覽產品" })).toBeInTheDocument();
      expect(injected.getHealth).toHaveBeenCalled();
    });

    it("有新版本時在頁簽之外通知、在設定頁簽顯示更新狀態，不會自己開始更新（spec D-19）", async () => {
      const user = userEvent.setup();
      render(<App />);

      expect(await screen.findByText("Baton 0.2.0 可以更新")).toBeInTheDocument();
      await user.click(screen.getByRole("tab", { name: "設定" }));
      expect(await screen.findByText("狀態：有新版本 0.2.0")).toBeInTheDocument();
      // 切換頁簽不影響更新通知
      expect(screen.getByText("Baton 0.2.0 可以更新")).toBeInTheDocument();
      expect(client.applyUpdate).not.toHaveBeenCalled();
    });
  });
});
