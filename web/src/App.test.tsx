import { act, cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeAll, beforeEach, describe, expect, it, onTestFinished, vi } from "vitest";

import { App, SHELL_STORAGE_KEY } from "./App";
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
  // 模型設定頁與 App 設定頁的可用模型清單
  listModels: vi.fn(async () => ({ models: [{ id: "opus", label: "Opus（最新版）" }], updated_at: null })),
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

/** 跑道欄上目前 Workspace 的頁面按鈕。 */
const pageButton = (name: string) => within(screen.getByRole("navigation", { name: "Workspace 頁面" })).getByRole("button", { name });
/** 跑道欄底部的平台頁面按鈕。 */
const globalButton = (name: string) => within(screen.getByRole("navigation", { name: "平台" })).getByRole("button", { name });
/** 主區（目前頁面的內容）。 */
const main = () => screen.getByRole("main");

/** 開啟目前 Workspace 的某一頁。 */
async function openPage(user: ReturnType<typeof userEvent.setup>, page: string) {
  await user.click(pageButton(page));
}

/** 從跑道欄的清單選取 Workspace。 */
async function selectWorkspace(user: ReturnType<typeof userEvent.setup>, name = "筆記") {
  await user.click(await screen.findByRole("button", { name: `選擇 ${name}` }));
}

const EMPTY_TITLE = "選一個 Workspace 開始";

/** 更新通知的文字（版本號是等寬的子元素，以整段文字比對）。 */
const updateNotice = (_: string, element: Element | null) =>
  element?.textContent === "Baton 0.2.0 可以更新" && !Array.from(element.children).some((child) => child.textContent === "Baton 0.2.0 可以更新");

describe("App 外殼", () => {
  // App 第一次 render 會初次執行側欄與全部設定頁元件；全部前端測試平行執行時這段冷啟動實測可超過 1 秒，
  // 會吃掉第一個測試 findBy* 的預設等待時間。先在這裡暖機一次，各測試的等待只反映畫面本身的反應。
  beforeAll(async () => {
    render(<App />);
    await screen.findByRole("button", { name: "選擇 筆記" }, { timeout: 10_000 });
    cleanup();
  });

  // 各測試以呼叫紀錄判斷行為（例如完全退出的事件訂閱），不能沿用暖機或前一個測試留下的紀錄；
  // 記住的位置也不能帶到下一個測試
  beforeEach(() => {
    vi.clearAllMocks();
    localStorage.clear();
    pendingCount = 1;
  });

  describe("跑道欄", () => {
    it("跑道欄有 Workspace 清單與三個平台頁面；未選 Workspace 時主區提示選擇", async () => {
      render(<App />);

      expect(await screen.findByRole("complementary", { name: "Workspace" })).toBeInTheDocument();
      const global = within(screen.getByRole("navigation", { name: "平台" })).getAllByRole("button");
      expect(global.map((button) => button.textContent)).toEqual(["帳號", "共同設定", "App 設定"]);
      expect(global.every((button) => !button.hasAttribute("aria-current"))).toBe(true);
      expect(within(main()).getByRole("heading", { name: EMPTY_TITLE })).toBeInTheDocument();
      // 沒有選 Workspace 時沒有它的頁面導覽，帳號與系統設定也不疊在主畫面
      expect(screen.queryByRole("navigation", { name: "Workspace 頁面" })).not.toBeInTheDocument();
      expect(screen.queryByRole("heading", { name: "平台狀態" })).not.toBeInTheDocument();
    });

    it("產品名稱取自 Runtime 回報", async () => {
      render(<App />);

      expect(await screen.findByRole("heading", { level: 1, name: "Baton" })).toBeInTheDocument();
    });

    it("帳號頁顯示首次設定、帳號與並行容量，不顯示共同設定與系統設定", async () => {
      const user = userEvent.setup();
      render(<App />);

      await user.click(await screen.findByRole("button", { name: "帳號" }));

      expect(globalButton("帳號")).toHaveAttribute("aria-current", "page");
      expect(within(main()).getByRole("heading", { level: 2, name: "帳號" })).toBeInTheDocument();
      expect(await screen.findByRole("heading", { name: "並行容量" })).toBeInTheDocument();
      expect(await screen.findByRole("region", { name: "首次設定" })).toBeInTheDocument();
      for (const name of ["共同設定", "登入時自動啟動", "更新", "備份與還原演練", "崩潰恢復", "平台狀態"]) {
        expect(within(main()).queryByRole("heading", { name })).not.toBeInTheDocument();
      }
      // Workspace 清單仍在跑道欄，可以直接切回
      expect(screen.getByRole("complementary", { name: "Workspace" })).toBeInTheDocument();
    });

    it("共同設定自成一頁", async () => {
      const user = userEvent.setup();
      render(<App />);

      await user.click(await screen.findByRole("button", { name: "共同設定" }));

      expect(globalButton("共同設定")).toHaveAttribute("aria-current", "page");
      expect(await within(main()).findByRole("heading", { name: "共同設定" })).toBeInTheDocument();
      expect(within(main()).queryByRole("heading", { name: "並行容量" })).not.toBeInTheDocument();
    });

    it("設定頁只顯示跨登入啟動、更新、備份、恢復與平台狀態", async () => {
      const user = userEvent.setup();
      render(<App />);

      await user.click(await screen.findByRole("button", { name: "App 設定" }));

      expect(within(main()).getByRole("heading", { level: 2, name: "App 設定" })).toBeInTheDocument();
      for (const name of ["登入時自動啟動", "更新", "備份與還原演練", "崩潰恢復", "平台狀態"]) {
        expect(await within(main()).findByRole("heading", { name })).toBeInTheDocument();
      }
      for (const name of ["帳號", "並行容量", "共同設定"]) {
        expect(within(main()).queryByRole("heading", { name })).not.toBeInTheDocument();
      }
    });

    it("從平台頁面點 Workspace 的頁面回到它", async () => {
      const user = userEvent.setup();
      render(<App />);
      await selectWorkspace(user);
      await openPage(user, "工作");
      await user.click(globalButton("App 設定"));
      const nav = screen.getByRole("navigation", { name: "Workspace 頁面" });
      expect(within(nav).getAllByRole("button").some((button) => button.hasAttribute("aria-current"))).toBe(false);

      await openPage(user, "工作");

      expect(pageButton("工作")).toHaveAttribute("aria-current", "page");
      expect(globalButton("App 設定")).not.toHaveAttribute("aria-current");
      expect(await within(main()).findByRole("heading", { name: "工作" })).toBeInTheDocument();
    });
  });

  describe("鍵盤", () => {
    it("最前面有「跳到主要內容」，指向可聚焦的主區", async () => {
      render(<App />);
      await screen.findByRole("button", { name: "選擇 筆記" });

      const skip = screen.getByRole("link", { name: "跳到主要內容" });
      const target = document.querySelector(skip.getAttribute("href")!);
      expect(target).toBe(main());
      expect(main()).toHaveAttribute("tabindex", "-1");
    });
  });

  describe("Workspace 情境列與空狀態", () => {
    it("未選 Workspace 時，空狀態的「新增 Workspace」展開跑道欄的建立表單並聚焦目錄欄位", async () => {
      const user = userEvent.setup();
      render(<App />);
      await screen.findByRole("button", { name: "選擇 筆記" });
      // 已經有 Workspace 時表單平時收起
      expect(screen.queryByLabelText("目錄")).not.toBeInTheDocument();

      const empty = screen.getByRole("region", { name: EMPTY_TITLE });
      await user.click(within(empty).getByRole("button", { name: "新增 Workspace" }));

      expect(await screen.findByLabelText("目錄")).toHaveFocus();
    });

    it("選中 Workspace 後，頁面上方的情境列顯示名稱與路徑（完整值在 title），路徑可複製", async () => {
      const user = userEvent.setup();
      render(<App />);
      await selectWorkspace(user);

      const context = await screen.findByLabelText("目前的 Workspace");
      expect(within(context).getByTitle("筆記")).toHaveTextContent("筆記");
      expect(within(context).getByTitle("C:\\work\\notes")).toHaveClass("path-text");
      expect(within(context).getByRole("button", { name: "複製路徑" })).toBeInTheDocument();
    });
  });

  describe("Workspace 頁面", () => {
    it("選中 Workspace 後在清單之後列出它的四組頁面，預設開在 Controller 頁，一次只顯示一頁", async () => {
      const user = userEvent.setup();
      render(<App />);

      await selectWorkspace(user);

      const nav = screen.getByRole("navigation", { name: "Workspace 頁面" });
      const groups = within(nav).getAllByRole("group");
      expect(groups.map((group) => group.getAttribute("aria-label"))).toEqual(["進行", "產出", "脈絡", "規則"]);
      expect(groups.map((group) => within(group).getAllByRole("button").map((button) => button.querySelector(".nav-label")?.textContent))).toEqual([
        ["Controller", "工作", "流程", "裁定", "時間線"],
        ["成果", "審查", "驗證", "探索測試"],
        ["知識", "專案理解"],
        ["授權", "決策權限", "模型設定"],
      ]);
      // 頁面導覽在 Workspace 清單之後（不夾在項目之間，其他 Workspace 永遠在上方看得到）
      const list = screen.getByRole("button", { name: "選擇 筆記" }).closest("ul")!;
      expect(list).not.toContainElement(nav);
      expect(list.compareDocumentPosition(nav) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
      expect(screen.getByRole("complementary", { name: "Workspace" })).toContainElement(nav);
      expect(pageButton("Controller")).toHaveAttribute("aria-current", "page");
      expect(within(main()).getByRole("heading", { name: "Controller" })).toBeInTheDocument();
      expect(client.getControllerCli).toHaveBeenCalledWith("ws_1");
      expect(screen.queryByText(EMPTY_TITLE)).not.toBeInTheDocument();
      expect(screen.getByRole("button", { name: "選擇 筆記" })).toHaveAttribute("aria-current", "true");
      // 其他頁面尚未開啟，不會讀取（Controller 頁本身會讀授權，用來提示尚未確認）
      expect(client.getTaskBoard).not.toHaveBeenCalled();
      expect(client.listDecisions).not.toHaveBeenCalled();
    });

    // 每一頁都可直接到達，且只顯示這一頁
    it.each([
      ["工作", () => expect(client.getTaskBoard).toHaveBeenCalledWith("ws_1")],
      ["流程", () => expect(client.workflows).toHaveBeenCalledWith("ws_1")],
      ["裁定", () => expect(client.listDecisions).toHaveBeenCalledWith("ws_1")],
      ["時間線", () => expect(client.metrics).toHaveBeenCalledWith("ws_1")],
      ["成果", () => expect(client.listArtifacts).toHaveBeenCalledWith("ws_1")],
      ["審查", () => expect(client.reviews).toHaveBeenCalledWith("ws_1")],
      ["驗證", () => expect(client.verificationOverview).toHaveBeenCalledWith("ws_1")],
      ["探索測試", () => expect(client.explorationOverview).toHaveBeenCalledWith("ws_1")],
      ["知識", () => expect(client.listKnowledge).toHaveBeenCalledWith(expect.objectContaining({ workspaceId: "ws_1" }))],
      ["專案理解", () => expect(client.syncUnderstanding).toHaveBeenCalledWith("ws_1")],
      ["授權", () => expect(client.getAuthorization).toHaveBeenCalledWith("ws_1")],
      ["決策權限", () => expect(client.decisionAuthority).toHaveBeenCalledWith("ws_1")],
      // 模型設定讀的是 Workspace 本身的覆寫值，不另外呼叫 API
      ["模型設定", () => {}],
    ])("「%s」開啟同名的頁面，且只顯示這一頁", async (page, called) => {
      const user = userEvent.setup();
      render(<App />);
      await selectWorkspace(user);

      await openPage(user, page);

      expect(pageButton(page)).toHaveAttribute("aria-current", "page");
      expect(await within(main()).findByRole("heading", { level: 2, name: page })).toBeInTheDocument();
      called();
      expect(within(main()).queryByRole("heading", { name: "Controller" })).not.toBeInTheDocument();
    });

    it("Controller 頁提示授權尚未確認時，「前往授權」切到同一個 Workspace 的授權頁", async () => {
      vi.mocked(client.getAuthorization).mockImplementation(async () => ({
        ...authorizationOf(workspace, "read_files"),
        status: "unconfirmed",
        effective: null,
        revisions: [],
      }));
      onlyInThisTest(client.getAuthorization);
      const user = userEvent.setup();
      render(<App />);
      await selectWorkspace(user);

      await user.click(await within(main()).findByRole("button", { name: "前往授權" }));

      expect(pageButton("授權")).toHaveAttribute("aria-current", "page");
      expect(await within(main()).findByRole("heading", { level: 2, name: "授權" })).toBeInTheDocument();
      // 原本聚焦的按鈕隨頁面消失，焦點移到主區
      await waitFor(() => expect(main()).toHaveFocus());
    });

    it("裁定頁開啟裁定中心", async () => {
      const user = userEvent.setup();
      render(<App />);
      await selectWorkspace(user);

      await openPage(user, "裁定");

      expect(await screen.findByRole("heading", { name: "裁定" })).toBeInTheDocument();
      expect(screen.getByRole("heading", { name: "要保留舊的目錄結構嗎？" })).toBeInTheDocument();
    });

    it("切換 Workspace 時停在同一頁", async () => {
      vi.mocked(client.listWorkspaces).mockImplementation(async () => ({ workspaces: [workspace, other] }));
      onlyInThisTest(client.listWorkspaces);
      const user = userEvent.setup();
      render(<App />);
      await selectWorkspace(user, "筆記");
      await openPage(user, "時間線");
      expect(await screen.findByRole("heading", { name: "時間線" })).toBeInTheDocument();

      await selectWorkspace(user, "文件");

      await waitFor(() => expect(client.timeline).toHaveBeenLastCalledWith("ws_2", undefined));
      expect(await screen.findByRole("heading", { name: "時間線" })).toBeInTheDocument();
      expect(pageButton("時間線")).toHaveAttribute("aria-current", "page");
      // 跑道欄標示新選中的 Workspace
      expect(screen.getByRole("button", { name: "選擇 文件" })).toHaveAttribute("aria-current", "true");
      expect(screen.getByRole("button", { name: "選擇 筆記" })).not.toHaveAttribute("aria-current");
    });

    it("選中的 Workspace 被封存後回到空狀態", async () => {
      vi.mocked(client.listWorkspaces)
        .mockImplementationOnce(async () => ({ workspaces: [workspace] }))
        .mockImplementation(async () => ({ workspaces: [{ ...workspace, status: "archived", archived_at: 1 }] }));
      onlyInThisTest(client.listWorkspaces);
      const user = userEvent.setup();
      render(<App />);
      await selectWorkspace(user);

      await user.click(screen.getByRole("button", { name: "封存 筆記" }));
      await user.click(screen.getByRole("button", { name: "確認封存 筆記" }));

      expect(await screen.findByText(EMPTY_TITLE)).toBeInTheDocument();
      expect(screen.queryByRole("navigation", { name: "Workspace 頁面" })).not.toBeInTheDocument();
    });

    it("切換 Workspace 後，上一個 Workspace 較晚回來的授權設定不會蓋掉目前的草稿", async () => {
      // 依 Workspace 回應（Controller 頁也會讀授權，不能依呼叫順序）：「筆記」的回應一直到最後才到
      const pendingFirst: Array<(view: AuthorizationView) => void> = [];
      vi.mocked(client.listWorkspaces).mockImplementation(async () => ({ workspaces: [workspace, other] }));
      vi.mocked(client.getAuthorization).mockImplementation((id: string) =>
        id === "ws_1"
          ? new Promise((resolve) => pendingFirst.push(resolve))
          : Promise.resolve(authorizationOf(other, "read_files")),
      );
      onlyInThisTest(client.listWorkspaces, client.getAuthorization);
      const user = userEvent.setup();
      render(<App />);

      await selectWorkspace(user, "筆記");
      await openPage(user, "授權");
      await selectWorkspace(user, "文件");
      expect(await screen.findByRole("heading", { level: 2, name: "授權" })).toBeInTheDocument();
      expect(client.getAuthorization).toHaveBeenLastCalledWith("ws_2");
      // 「筆記」的回應這時才到
      await act(async () => {
        for (const resolve of pendingFirst) resolve(authorizationOf(workspace, "deploy"));
      });

      expect(screen.getByRole("heading", { level: 2, name: "授權" })).toBeInTheDocument();
      expect(screen.getByRole("checkbox", { name: /^讀取檔案/ })).toBeChecked();
      expect(screen.getByRole("checkbox", { name: /^部署/ })).not.toBeChecked();
    });
  });

  describe("待裁定徽章", () => {
    it("裁定頁與跑道欄顯示待裁定數，回答裁定後隨之更新", async () => {
      const user = userEvent.setup();
      render(<App />);
      expect(await screen.findByText("待裁定：1")).toBeInTheDocument();
      expect(await screen.findByText("待裁定 1")).toBeInTheDocument();
      await selectWorkspace(user);
      // 徽章的數字以說明提供，按鈕名稱仍是頁名
      expect(pageButton("裁定")).toHaveAccessibleDescription("待裁定 1");

      await openPage(user, "裁定");
      await user.click(await screen.findByRole("radio", { name: "駁回" }));
      await user.click(screen.getByRole("button", { name: "送出裁定" }));

      expect(client.answerDecision).toHaveBeenCalledWith("ws_1", "dec_1", { action: "reject", expected_revision: 1 });
      await waitFor(() => expect(screen.queryByText("待裁定 1")).not.toBeInTheDocument());
      expect(pageButton("裁定")).not.toHaveAccessibleDescription();
    });
  });

  describe("記住所在位置", () => {
    it("重新開啟 App 回到上次的 Workspace 與頁面", async () => {
      const user = userEvent.setup();
      const first = render(<App />);
      await selectWorkspace(user);
      await openPage(user, "流程");
      await screen.findByRole("heading", { name: "流程" });
      first.unmount();

      render(<App />);

      expect(await screen.findByRole("heading", { name: "流程" })).toBeInTheDocument();
      expect(client.workflows).toHaveBeenLastCalledWith("ws_1");
      expect(pageButton("流程")).toHaveAttribute("aria-current", "page");
    });

    it("重新開啟 App 回到上次的平台頁面", async () => {
      const user = userEvent.setup();
      const first = render(<App />);
      await user.click(await screen.findByRole("button", { name: "App 設定" }));
      first.unmount();

      render(<App />);

      expect(await screen.findByRole("heading", { name: "平台狀態" })).toBeInTheDocument();
      expect(globalButton("App 設定")).toHaveAttribute("aria-current", "page");
    });

    it("舊版記住的頂層頁簽（top）照樣還原", async () => {
      localStorage.setItem(SHELL_STORAGE_KEY, JSON.stringify({ top: "accounts", workspaceId: null, workspaceTab: "controller" }));
      render(<App />);

      expect(await screen.findByRole("heading", { name: "並行容量" })).toBeInTheDocument();
      expect(globalButton("帳號")).toHaveAttribute("aria-current", "page");
    });

    it("記住的內容讀不懂時使用預設值，不報錯", async () => {
      localStorage.setItem(SHELL_STORAGE_KEY, "{不是 JSON");
      render(<App />);

      expect(await screen.findByText(EMPTY_TITLE)).toBeInTheDocument();
    });

    it("記住的頁面名稱不認得時使用預設值", async () => {
      localStorage.setItem(SHELL_STORAGE_KEY, JSON.stringify({ view: "不存在", workspaceId: "ws_1", workspaceTab: "不存在" }));
      render(<App />);

      expect(await within(main()).findByRole("heading", { name: "Controller" })).toBeInTheDocument();
      expect(pageButton("Controller")).toHaveAttribute("aria-current", "page");
    });

    it("記住的是已移除的「對話」頁簽時開在 Controller 頁", async () => {
      localStorage.setItem(SHELL_STORAGE_KEY, JSON.stringify({ top: "workspace", workspaceId: "ws_1", workspaceTab: "conversation" }));
      render(<App />);

      expect(await within(main()).findByRole("heading", { name: "Controller" })).toBeInTheDocument();
      expect(pageButton("Controller")).toHaveAttribute("aria-current", "page");
    });

    // 舊版記住的頁面 id 對應到現在的頁面，不會落到空白頁
    it.each([
      ["controller", "Controller"],
      ["tasks", "工作"],
      ["decisions", "裁定"],
      ["artifacts", "成果"],
      ["quality", "審查"],
      ["reviews", "審查"],
      ["verification", "驗證"],
      ["exploration", "探索測試"],
      ["knowledge", "知識"],
      ["workflows", "流程"],
      ["timeline", "時間線"],
      ["authorization", "授權"],
    ])("記住的舊位置「%s」開在「%s」頁", async (saved, page) => {
      localStorage.setItem(SHELL_STORAGE_KEY, JSON.stringify({ top: "workspace", workspaceId: "ws_1", workspaceTab: saved }));
      render(<App />);

      await screen.findByRole("navigation", { name: "Workspace 頁面" });
      expect(pageButton(page)).toHaveAttribute("aria-current", "page");
      expect(main()).not.toBeEmptyDOMElement();
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

      expect(await screen.findByText(EMPTY_TITLE)).toBeInTheDocument();
      await selectWorkspace(user);
      expect(pageButton("Controller")).toHaveAttribute("aria-current", "page");
    });

    it("上次的 Workspace 已不在清單時顯示空狀態", async () => {
      localStorage.setItem(SHELL_STORAGE_KEY, JSON.stringify({ view: "workspace", workspaceId: "ws_gone", workspaceTab: "workflows" }));
      render(<App />);

      expect(await screen.findByText(EMPTY_TITLE)).toBeInTheDocument();
    });
  });

  describe("首次精靈與主畫面之外的元素", () => {
    it("沒有可用的 Account Profile 時以全畫面顯示首次精靈與帳號，不顯示跑道欄", async () => {
      vi.mocked(client.getSetup).mockImplementation(async () =>
        setupStatus({ profiles: [profile({ dispatchable: false })], ready: false }),
      );
      onlyInThisTest(client.getSetup);
      render(<App />);

      expect(await screen.findByRole("heading", { name: "首次設定" })).toBeInTheDocument();
      expect(screen.queryByRole("navigation", { name: "平台" })).not.toBeInTheDocument();
      expect(screen.queryByRole("complementary", { name: "Workspace" })).not.toBeInTheDocument();
      // 狀態列與更新通知不受影響
      expect(await screen.findByText("待裁定：1")).toBeInTheDocument();
      expect(await screen.findByText(updateNotice)).toBeInTheDocument();
    });

    it("首次精靈中出現可用的 Profile 後回到主畫面", async () => {
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

      // 不立刻切走：先顯示完成畫面與下一步，使用者自己進入主畫面；焦點移到完成畫面的標題
      expect(await screen.findByRole("heading", { name: "帳號已就緒" })).toHaveFocus();
      expect(screen.getByText("確認授權")).toBeInTheDocument();
      expect(screen.queryByRole("navigation", { name: "平台" })).not.toBeInTheDocument();
      await user.click(screen.getByRole("button", { name: "進入主畫面" }));

      expect(await screen.findByRole("navigation", { name: "平台" })).toBeInTheDocument();
      // 離開全畫面設定後焦點在主區，不落到 body
      await waitFor(() => expect(main()).toHaveFocus());
    });

    it("全畫面精靈中重新讀取首次設定失敗時，不會誤顯示「帳號已就緒」", async () => {
      // App 與精靈都會讀取首次設定：按「重新檢查」之前一律回「還沒有帳號」，之後一律讀取失敗
      let failing = false;
      vi.mocked(client.getSetup).mockImplementation(async () => {
        if (failing) throw new Error("暫時無法連線");
        return setupStatus({ profiles: [], ready: false });
      });
      vi.mocked(client.recheckSetup).mockImplementation(async () => {
        throw new Error("暫時無法連線");
      });
      onlyInThisTest(client.getSetup, client.recheckSetup);
      const user = userEvent.setup();
      render(<App />);
      await screen.findByRole("region", { name: "首次設定" });

      failing = true;
      await user.click(await screen.findByRole("button", { name: "重新檢查" }));

      expect((await screen.findAllByText(/暫時無法連線/)).length).toBeGreaterThan(0);
      expect(screen.queryByRole("heading", { name: "帳號已就緒" })).not.toBeInTheDocument();
      expect(screen.queryByRole("navigation", { name: "平台" })).not.toBeInTheDocument();
    });

    it("首次設定完成畫面可以直接前往共同設定", async () => {
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

      await user.click(await screen.findByRole("button", { name: "前往共同設定" }));

      expect(await screen.findByRole("navigation", { name: "平台" })).toBeInTheDocument();
      expect(globalButton("共同設定")).toHaveAttribute("aria-current", "page");
    });

    it("全畫面首次精靈可以先進入主畫面", async () => {
      vi.mocked(client.getSetup).mockImplementation(async () => setupStatus({ profiles: [], ready: false }));
      onlyInThisTest(client.getSetup);
      const user = userEvent.setup();
      render(<App />);

      await user.click(await screen.findByRole("button", { name: "先進入主畫面" }));

      expect(await screen.findByRole("navigation", { name: "平台" })).toBeInTheDocument();
      expect(screen.getByRole("complementary", { name: "Workspace" })).toBeInTheDocument();
    });

    it("讀不到首次設定狀態時照常顯示主畫面", async () => {
      vi.mocked(client.getSetup).mockImplementation(async () => {
        throw new Error("離線");
      });
      onlyInThisTest(client.getSetup);
      render(<App />);

      expect(await screen.findByRole("navigation", { name: "平台" })).toBeInTheDocument();
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

    it("儲存故障停止派工時，主區頂端顯示整寬警示，跑道欄留一行狀態", async () => {
      vi.mocked(client.getHealth).mockImplementation(async () => ({
        status: "ok",
        product: "Baton",
        version: "0.1.0",
        boot_count: 1,
        started_at: 0,
        pid: 1,
        storage: {
          dispatch_halted: true,
          fault: { kind: "disk_full", code: "storage_disk_full", summary: "磁碟空間不足", action: "請釋放空間", detail: "SQLITE_FULL" },
        },
      }));
      onlyInThisTest(client.getHealth);
      render(<App />);

      const alert = await within(main()).findByRole("alert");
      expect(alert).toHaveTextContent("新派工已停止");
      expect(alert).toHaveTextContent("請釋放空間");
      expect(within(screen.getByRole("status")).getByText("新派工已停止")).toBeInTheDocument();
    });

    it("有新版本時在主區頂端通知、在設定頁顯示更新狀態，不會自己開始更新（spec D-19）", async () => {
      const user = userEvent.setup();
      render(<App />);

      expect(await screen.findByText(updateNotice)).toBeInTheDocument();
      await user.click(globalButton("App 設定"));
      const updates = await screen.findByRole("region", { name: "更新" });
      expect(within(updates).getByText("狀態", { selector: "dt" }).nextElementSibling).toHaveTextContent("有新版本 0.2.0");
      // 切換頁面不影響更新通知
      expect(screen.getByText(updateNotice)).toBeInTheDocument();
      expect(client.applyUpdate).not.toHaveBeenCalled();
    });
  });
});
