import { useEffect, useState } from "react";

import { AccountsPage } from "./AccountsPage";
import { ArtifactsPage } from "./ArtifactsPage";
import { CopyButton } from "./LongText";
import { AuthorizationPanel } from "./AuthorizationPanel";
import { BackupSettings } from "./BackupSettings";
import { ControllerPage } from "./ControllerPage";
import { ModelOverridesPanel } from "./ModelOverridesPanel";
import { DecisionCenter } from "./DecisionCenter";
import { ExitProgress } from "./ExitProgress";
import { ExplorationPage } from "./ExplorationPage";
import { KnowledgePage } from "./KnowledgePage";
import { LoginStartupSettings } from "./LoginStartupSettings";
import { PlatformPage } from "./PlatformPage";
import { ProjectUnderstandingPage } from "./ProjectUnderstandingPage";
import { RecoveryPanel } from "./RecoveryPanel";
import { SchedulingSettings } from "./SchedulingSettings";
import { SetupWizard } from "./SetupWizard";
import { SharedSettingsPage } from "./SharedSettingsPage";
import { StatusBar } from "./StatusBar";
import { createRuntimeClient, type AppClient, type HealthResponse, type PendingDecisions, type Workspace } from "./runtimeClient";
import { TaskBoard } from "./TaskBoard";
import { formatCount, shortPath } from "./textFormat";
import { TimelinePage } from "./TimelinePage";
import { UpdateNotice } from "./UpdateNotice";
import { UpdateSettings } from "./UpdateSettings";
import { ReviewPage } from "./ReviewPage";
import { VerificationPage } from "./VerificationPage";
import { WorkflowPage } from "./WorkflowPage";
import { WORKSPACE_PATH_INPUT_ID, WorkspaceSidebar } from "./WorkspaceSidebar";

/** 頂層頁簽。 */
type TopTab = "workspace" | "accounts" | "settings";
/** 選中 Workspace 後的各頁面；本機記住的是頁面 id，入口由頁面推得。 */
type WorkspacePage =
  | "controller"
  | "tasks"
  | "decisions"
  | "artifacts"
  | "reviews"
  | "verification"
  | "exploration"
  | "knowledge"
  | "workflows"
  | "timeline"
  | "authorization";
/** Workspace 的五個主要入口（AI-02）。 */
type WorkspaceSection = "controller" | "tasks" | "decisions" | "quality" | "more";

interface TabItem<T extends string> {
  id: T;
  label: string;
  /** 徽章數字；大於 0 才顯示。 */
  badge?: number;
  /** 徽章的說明（滑鼠停留時顯示）。 */
  badgeTitle?: string;
}

interface SectionItem {
  id: WorkspaceSection;
  label: string;
  /** 入口內的頁面；超過一頁時以第二層頁簽切換。 */
  pages: TabItem<WorkspacePage>[];
}

const TOP_TABS: TabItem<TopTab>[] = [
  { id: "workspace", label: "Workspace" },
  { id: "accounts", label: "帳號" },
  { id: "settings", label: "設定" },
];

const WORKSPACE_SECTIONS: SectionItem[] = [
  { id: "controller", label: "主控台", pages: [{ id: "controller", label: "主控台" }] },
  { id: "tasks", label: "工作", pages: [{ id: "tasks", label: "工作" }] },
  { id: "decisions", label: "裁定", pages: [{ id: "decisions", label: "裁定" }] },
  {
    id: "quality",
    label: "成果與品質",
    pages: [
      { id: "artifacts", label: "成果" },
      { id: "reviews", label: "審查" },
      { id: "verification", label: "驗證" },
      { id: "exploration", label: "探索" },
    ],
  },
  {
    id: "more",
    label: "更多",
    pages: [
      { id: "knowledge", label: "知識" },
      { id: "workflows", label: "流程" },
      { id: "timeline", label: "時間線" },
      { id: "authorization", label: "授權" },
    ],
  },
];

const WORKSPACE_PAGES = WORKSPACE_SECTIONS.flatMap((section) => section.pages);

/** 舊版記住的頁簽中已不是頁面 id 的值；舊「審查與驗證」頁簽原本開在審查。 */
const LEGACY_PAGES: Record<string, WorkspacePage> = { quality: "reviews" };

function sectionOf(page: WorkspacePage): SectionItem {
  return WORKSPACE_SECTIONS.find((section) => section.pages.some((item) => item.id === page))!;
}

/**
 * 主要入口的頁簽；入口的徽章是其中各頁徽章的加總，
 * 讓收在「更多」等入口內的頁面有徽章時，入口本身也看得到提示。
 */
export function workspaceSectionTabs(
  badges: Partial<Record<WorkspacePage, number>>,
  titles: Partial<Record<WorkspacePage, string>>,
): TabItem<WorkspaceSection>[] {
  return WORKSPACE_SECTIONS.map(({ id, label, pages }) => {
    const marked = pages.filter((page) => (badges[page.id] ?? 0) > 0);
    return {
      id,
      label,
      badge: marked.reduce((sum, page) => sum + (badges[page.id] ?? 0), 0),
      badgeTitle: marked.map((page) => titles[page.id]).join("、"),
    };
  });
}

/** 所在頁簽與 Workspace 存在本機（每個檢視者的偏好，spec Q20）。 */
export const SHELL_STORAGE_KEY = "baton.shell";

interface ShellState {
  top: TopTab;
  workspaceId: string | null;
  /** 欄位名稱沿用舊版，內容是頁面 id，讓舊版記住的位置直接對應。 */
  workspaceTab: WorkspacePage;
}

const DEFAULT_SHELL: ShellState = { top: "workspace", workspaceId: null, workspaceTab: "controller" };

function isTab<T extends string>(tabs: TabItem<T>[], value: unknown): value is T {
  return tabs.some((tab) => tab.id === value);
}

/** 記住的頁面；舊版頁簽名稱換成對應頁面，不認得的用預設值。 */
function savedPage(value: unknown): WorkspacePage {
  if (isTab(WORKSPACE_PAGES, value)) return value;
  if (typeof value === "string" && Object.hasOwn(LEGACY_PAGES, value)) return LEGACY_PAGES[value];
  return DEFAULT_SHELL.workspaceTab;
}

/** 讀取上次的位置；讀不到、讀不懂或名稱不認得的部分一律用預設值，不報錯。 */
function loadShell(): ShellState {
  try {
    const raw = localStorage.getItem(SHELL_STORAGE_KEY);
    if (!raw) return DEFAULT_SHELL;
    const saved = JSON.parse(raw) as Partial<Record<keyof ShellState, unknown>> | null;
    return {
      top: isTab(TOP_TABS, saved?.top) ? saved.top : DEFAULT_SHELL.top,
      workspaceId: typeof saved?.workspaceId === "string" ? saved.workspaceId : null,
      workspaceTab: savedPage(saved?.workspaceTab),
    };
  } catch {
    return DEFAULT_SHELL;
  }
}

function saveShell(shell: ShellState) {
  try {
    localStorage.setItem(SHELL_STORAGE_KEY, JSON.stringify(shell));
  } catch {
    // 本機儲存無法使用時只是下次不會回到原位
  }
}

/** 一排頁簽；選中的頁簽控制緊接在後的 tabpanel（id 為 `${idPrefix}-panel`）。 */
function TabList<T extends string>({
  label,
  idPrefix,
  tabs,
  selected,
  onSelect,
}: {
  label: string;
  idPrefix: string;
  tabs: TabItem<T>[];
  selected: T;
  onSelect: (tab: T) => void;
}) {
  return (
    <div role="tablist" aria-label={label} className={`tabs tabs-${idPrefix}`}>
      {tabs.map((tab) => {
        const badge = tab.badge ?? 0;
        return (
          <button
            key={tab.id}
            type="button"
            role="tab"
            id={`${idPrefix}-tab-${tab.id}`}
            aria-selected={tab.id === selected}
            aria-controls={tab.id === selected ? `${idPrefix}-panel` : undefined}
            // 有徽章時名稱仍只是頁簽名稱，數字以 title 說明
            aria-label={badge > 0 ? tab.label : undefined}
            onClick={() => onSelect(tab.id)}
          >
            {tab.label}
            {badge > 0 && (
              <span className="tab-badge" title={tab.badgeTitle}>
                {formatCount(badge)}
              </span>
            )}
          </button>
        );
      })}
    </div>
  );
}

/** 一排頁簽對應的內容區；`className` 加在 `.tab-panel` 之後（例如帳號、設定頁的欄寬）。 */
function TabPanel<T extends string>({
  idPrefix,
  selected,
  className,
  children,
}: {
  idPrefix: string;
  selected: T;
  className?: string;
  children: React.ReactNode;
}) {
  return (
    <div
      role="tabpanel"
      id={`${idPrefix}-panel`}
      aria-labelledby={`${idPrefix}-tab-${selected}`}
      className={className ? `tab-panel ${className}` : "tab-panel"}
    >
      {children}
    </div>
  );
}

/** `createClient` 只供開發用預覽注入假資料；正式畫面不傳，依執行環境挑選用戶端。 */
export function App({ createClient = createRuntimeClient }: { createClient?: () => Promise<AppClient> } = {}) {
  const [client, setClient] = useState<AppClient | null>(null);
  const [health, setHealth] = useState<HealthResponse | null>(null);
  // 精靈與帳號頁其中一邊改變了 Profile 狀態時，讓另一邊重新讀取
  const [accountsVersion, setAccountsVersion] = useState(0);
  const accountsChanged = () => setAccountsVersion((version) => version + 1);
  // 沒有任何可用的 Account Profile 時，首次精靈以全畫面顯示；讀不到狀態時照常顯示頁簽。
  // 使用者可以先進入主畫面（只在這次開啟有效），避免登入失效時連 Workspace 都看不到
  const [needsSetup, setNeedsSetup] = useState(false);
  const [setupSkipped, setSetupSkipped] = useState(false);
  const fullScreenSetup = needsSetup && !setupSkipped;
  const [shell, setShell] = useState<ShellState>(loadShell);
  // 各入口最後停留的頁面（只在這次開啟有效），回到入口時停在原頁
  const [sectionPages, setSectionPages] = useState<Partial<Record<WorkspaceSection, WorkspacePage>>>(() => ({
    [sectionOf(shell.workspaceTab).id]: shell.workspaceTab,
  }));
  // 側欄最近一次讀到的使用中 Workspace；選中的 Workspace 不在其中（例如已封存）時視為未選
  const [workspaces, setWorkspaces] = useState<Workspace[]>([]);
  const selected = workspaces.find((workspace) => workspace.id === shell.workspaceId) ?? null;
  // 待裁定數由狀態列輪詢；回答裁定後要求它立即重新讀取
  const [pending, setPending] = useState<PendingDecisions | null>(null);
  const [pendingVersion, setPendingVersion] = useState(0);
  const pendingByWorkspace = Object.fromEntries((pending?.by_workspace ?? []).map((w) => [w.workspace_id, w.pending_count]));

  useEffect(() => {
    void createClient().then(setClient);
  }, [createClient]);

  useEffect(() => saveShell(shell), [shell]);

  useEffect(() => {
    if (!client) return;
    let current = true;
    client.getSetup().then(
      (status) => {
        if (current) setNeedsSetup(!status.profiles.some((profile) => profile.status === "active" && profile.dispatchable));
      },
      () => {
        if (current) setNeedsSetup(false);
      },
    );
    return () => {
      current = false;
    };
  }, [client, accountsVersion]);

  const selectWorkspace = (workspace: Workspace) =>
    // 在 Workspace 之間切換時停在同一頁面；從未選到選中時從主控台開始
    setShell((current) => ({
      ...current,
      workspaceId: workspace.id,
      workspaceTab: selected ? current.workspaceTab : "controller",
    }));

  const selectWorkspacePage = (page: WorkspacePage) => {
    setShell((current) => ({ ...current, workspaceTab: page }));
    setSectionPages((current) => ({ ...current, [sectionOf(page).id]: page }));
    if (page === "decisions") setPendingVersion((version) => version + 1);
  };

  const selectSection = (id: WorkspaceSection) => {
    const target = WORKSPACE_SECTIONS.find((item) => item.id === id)!;
    selectWorkspacePage(sectionPages[id] ?? target.pages[0].id);
  };

  const decisionCount = selected ? (pendingByWorkspace[selected.id] ?? 0) : 0;
  const sectionTabs = workspaceSectionTabs({ decisions: decisionCount }, { decisions: `待裁定 ${decisionCount}` });
  const section = sectionOf(shell.workspaceTab);
  const workspaceView = selected && client && (
    <WorkspaceView
      client={client}
      workspace={selected}
      page={shell.workspaceTab}
      onDecisionsChange={() => setPendingVersion((version) => version + 1)}
    />
  );

  return (
    <div className="app">
      {client ? <StatusBar client={client} onHealth={setHealth} onPending={setPending} refreshKey={pendingVersion} /> : <header className="status-bar" />}
      {/* 產品名稱與頂層頁簽同一列；首次精靈全畫面時不顯示頁簽 */}
      <div className="app-heading">
        {/* 產品名稱取自 Runtime 回報，不在前端寫死（spec D-01） */}
        <h1>{health?.product ?? ""}</h1>
        {!fullScreenSetup && (
          <TabList
            label="主要區塊"
            idPrefix="top"
            tabs={TOP_TABS}
            selected={shell.top}
            onSelect={(top) => setShell((current) => ({ ...current, top }))}
          />
        )}
      </div>
      {/* 有新版本時只通知；更新維護中以覆蓋層顯示進度（spec D-19）。在頁簽之外，切換頁簽不影響 */}
      <div className="app-notices">
        {client && <UpdateNotice client={client} runtimeStartedAt={health?.started_at ?? null} />}
      </div>
      {client && fullScreenSetup && (
        <main className="main setup-screen settings-column">
          <SetupWizard client={client} onChange={accountsChanged} refreshKey={accountsVersion} />
          <AccountsPage client={client} onChange={accountsChanged} refreshKey={accountsVersion} />
          <button type="button" className="setup-skip" onClick={() => setSetupSkipped(true)}>
            先進入主畫面
          </button>
        </main>
      )}
      {!fullScreenSetup && (
        <div className="body">
          {client && shell.top === "workspace" && (
            <WorkspaceSidebar
              client={client}
              onSelect={selectWorkspace}
              selectedId={selected?.id ?? null}
              pendingDecisions={pendingByWorkspace}
              onWorkspaces={setWorkspaces}
            />
          )}
          <main className="main">
            <TabPanel idPrefix="top" selected={shell.top} className={shell.top === "workspace" ? undefined : "settings-column"}>
              {client && shell.top === "workspace" && !selected && <WorkspaceEmpty />}
              {client && shell.top === "workspace" && selected && (
                <>
                  <WorkspaceContext workspace={selected} />
                  <TabList
                    label="Workspace 功能"
                    idPrefix="workspace"
                    tabs={sectionTabs}
                    selected={section.id}
                    onSelect={selectSection}
                  />
                  <TabPanel idPrefix="workspace" selected={section.id}>
                    {/* 超過一頁的入口（成果與品質、更多）以第二層頁簽切換 */}
                    {section.pages.length > 1 ? (
                      <>
                        <TabList
                          label={section.label}
                          idPrefix="page"
                          tabs={section.pages}
                          selected={shell.workspaceTab}
                          onSelect={selectWorkspacePage}
                        />
                        <TabPanel idPrefix="page" selected={shell.workspaceTab}>
                          {workspaceView}
                        </TabPanel>
                      </>
                    ) : (
                      workspaceView
                    )}
                  </TabPanel>
                </>
              )}
              {client && shell.top === "accounts" && (
                <>
                  <SetupWizard client={client} onChange={accountsChanged} refreshKey={accountsVersion} />
                  <AccountsPage client={client} onChange={accountsChanged} refreshKey={accountsVersion} />
                  <SchedulingSettings client={client} />
                  <SharedSettingsPage client={client} />
                </>
              )}
              {client && shell.top === "settings" && (
                <>
                  <LoginStartupSettings client={client} />
                  <UpdateSettings client={client} />
                  <BackupSettings client={client} />
                  <RecoveryPanel client={client} />
                  <PlatformPage client={client} />
                </>
              )}
            </TabPanel>
          </main>
        </div>
      )}
      {/* 系統匣「完全退出」的停止進度，以覆蓋層顯示 */}
      {client && <ExitProgress client={client} />}
    </div>
  );
}

/** 未選 Workspace 時的空狀態：「建立 Workspace」把焦點移到側欄建立表單的目錄欄位。 */
function WorkspaceEmpty() {
  return (
    <section className="workspace-empty" aria-labelledby="workspace-empty-title">
      <h2 id="workspace-empty-title">尚未選擇 Workspace</h2>
      <p className="workspace-hint">請從左側選擇或建立 Workspace</p>
      <button type="button" className="btn-primary" onClick={() => document.getElementById(WORKSPACE_PATH_INPUT_ID)?.focus()}>
        建立 Workspace
      </button>
    </section>
  );
}

/** 頁簽列上方的情境列：目前 Workspace 的名稱與縮短路徑，一行；完整值在 title，路徑可複製（完整名稱在側欄）。 */
function WorkspaceContext({ workspace }: { workspace: Workspace }) {
  return (
    <div className="workspace-context" aria-label="目前的 Workspace">
      <span className="workspace-context-name truncate" title={workspace.name}>
        {workspace.name}
      </span>
      <span className="workspace-context-path copy-text">
        <span className="mono truncate" title={workspace.path}>
          {shortPath(workspace.path)}
        </span>
        <CopyButton text={workspace.path} label="複製路徑" />
      </span>
    </div>
  );
}

/** 選中 Workspace 後，目前頁面的功能；各功能元件原樣放入。 */
function WorkspaceView({
  client,
  workspace,
  page,
  onDecisionsChange,
}: {
  client: AppClient;
  workspace: Workspace;
  page: WorkspacePage;
  onDecisionsChange: () => void;
}) {
  // 換 Workspace 時以 key 重新掛載，上一個 Workspace 較晚回來的讀取結果不會蓋掉目前畫面與草稿
  switch (page) {
    case "controller":
      return <ControllerPage key={workspace.id} client={client} workspace={workspace} />;
    case "tasks":
      return <TaskBoard key={workspace.id} client={client} workspace={workspace} />;
    case "decisions":
      return <DecisionCenter key={workspace.id} client={client} workspace={workspace} onChange={onDecisionsChange} />;
    case "artifacts":
      return <ArtifactsPage key={workspace.id} client={client} workspace={workspace} />;
    case "reviews":
      return <ReviewPage key={workspace.id} client={client} workspace={workspace} />;
    case "verification":
      // 換 Workspace 時重新掛載，清掉上一個 Workspace 正在編輯的 Profile 與訊息
      return <VerificationPage key={workspace.id} client={client} workspace={workspace} />;
    case "exploration":
      return <ExplorationPage key={workspace.id} client={client} workspace={workspace} />;
    case "knowledge":
      return (
        <>
          <KnowledgePage key={workspace.id} client={client} workspace={workspace} />
          {/* 換 Workspace 時重新掛載，清掉上一個 Workspace 的協調提示與錯誤 */}
          <ProjectUnderstandingPage key={`understanding-${workspace.id}`} client={client} workspace={workspace} />
        </>
      );
    case "workflows":
      // 換 Workspace 時重新掛載，清掉上一個 Workspace 的跳過原因與錯誤
      return <WorkflowPage key={workspace.id} client={client} workspace={workspace} />;
    case "timeline":
      // 換 Workspace 時重新掛載，清掉上一個 Workspace 的 Task 過濾
      return <TimelinePage key={workspace.id} client={client} workspace={workspace} />;
    case "authorization":
      return (
        <>
          <AuthorizationPanel key={workspace.id} client={client} workspace={workspace} />
          <ModelOverridesPanel key={`models-${workspace.id}`} client={client} workspace={workspace} />
        </>
      );
  }
}
