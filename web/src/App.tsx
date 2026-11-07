import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";

import { AccountsPage } from "./AccountsPage";
import { ArtifactsPage } from "./ArtifactsPage";
import { CopyButton, PathText } from "./LongText";
import { AuthorizationPanel } from "./AuthorizationPanel";
import { BackupSettings } from "./BackupSettings";
import { ControllerPage } from "./ControllerPage";
import { ModelOverridesPanel } from "./ModelOverridesPanel";
import { DecisionAuthorityPanel, DecisionCenter } from "./DecisionCenter";
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
import { StatusBar, StorageFault } from "./StatusBar";
import { createRuntimeClient, type AppClient, type HealthResponse, type PendingDecisions, type Workspace } from "./runtimeClient";
import { TaskBoard } from "./TaskBoard";
import { formatCount, formatNumber } from "./textFormat";
import { TimelinePage } from "./TimelinePage";
import { UpdateNotice } from "./UpdateNotice";
import { UpdateSettings } from "./UpdateSettings";
import { ReviewPage } from "./ReviewPage";
import { VerificationPage } from "./VerificationPage";
import { WorkflowPage } from "./WorkflowPage";
import { WorkspaceSidebar } from "./WorkspaceSidebar";

/** 選中 Workspace 後的各頁面；本機記住的是頁面 id。 */
export type WorkspacePage =
  | "controller"
  | "tasks"
  | "workflows"
  | "decisions"
  | "timeline"
  | "artifacts"
  | "reviews"
  | "verification"
  | "exploration"
  | "knowledge"
  | "understanding"
  | "authorization"
  | "authority"
  | "models";

/** 不屬於任何 Workspace 的平台頁面（跑道欄底部）。 */
export type GlobalPage = "accounts" | "shared" | "settings";

/** 目前看的是 Workspace 的頁面，或某個平台頁面。 */
type View = "workspace" | GlobalPage;

interface NavItem<T extends string> {
  id: T;
  label: string;
}

interface NavGroup {
  /** 群組名稱；跑道欄上顯示，也是群組的可存取名稱。 */
  label: string;
  pages: NavItem<WorkspacePage>[];
}

/**
 * Workspace 的頁面依使用者要做的事分組（MASTER 第 7 節）：
 * 進行（看現況、回答裁定）→ 產出（檢查成果與品質）→ 脈絡（累積的知識）→ 規則（Agent 能做什麼）。
 */
export const WORKSPACE_NAV: NavGroup[] = [
  {
    label: "進行",
    pages: [
      { id: "controller", label: "主控台" },
      { id: "tasks", label: "工作" },
      { id: "workflows", label: "流程" },
      { id: "decisions", label: "裁定" },
      { id: "timeline", label: "時間線" },
    ],
  },
  {
    label: "產出",
    pages: [
      { id: "artifacts", label: "成果" },
      { id: "reviews", label: "審查" },
      { id: "verification", label: "驗證" },
      { id: "exploration", label: "探索" },
    ],
  },
  {
    label: "脈絡",
    pages: [
      { id: "knowledge", label: "知識" },
      { id: "understanding", label: "專案理解" },
    ],
  },
  {
    label: "規則",
    pages: [
      { id: "authorization", label: "授權" },
      { id: "authority", label: "決策權限" },
      { id: "models", label: "模型設定" },
    ],
  },
];

export const GLOBAL_NAV: NavItem<GlobalPage>[] = [
  { id: "accounts", label: "帳號" },
  { id: "shared", label: "共同設定" },
  { id: "settings", label: "設定" },
];

const WORKSPACE_PAGES = WORKSPACE_NAV.flatMap((group) => group.pages);

/** 舊版記住的頁面 id 對應到現在的頁面（舊「審查與驗證」頁簽原本開在審查）。 */
const LEGACY_PAGES: Record<string, WorkspacePage> = { quality: "reviews" };

/** 所在頁面與 Workspace 存在本機（每個檢視者的偏好，spec Q20）。 */
export const SHELL_STORAGE_KEY = "baton.shell";

interface ShellState {
  view: View;
  workspaceId: string | null;
  /** 欄位名稱沿用舊版，內容是頁面 id，讓舊版記住的位置直接對應。 */
  workspaceTab: WorkspacePage;
}

const DEFAULT_SHELL: ShellState = { view: "workspace", workspaceId: null, workspaceTab: "controller" };

function isOneOf<T extends string>(items: NavItem<T>[], value: unknown): value is T {
  return items.some((item) => item.id === value);
}

/** 記住的頁面；舊版頁簽名稱換成對應頁面，不認得的用預設值。 */
function savedPage(value: unknown): WorkspacePage {
  if (isOneOf(WORKSPACE_PAGES, value)) return value;
  if (typeof value === "string" && Object.hasOwn(LEGACY_PAGES, value)) return LEGACY_PAGES[value];
  return DEFAULT_SHELL.workspaceTab;
}

/** 記住的檢視；舊版存的是頂層頁簽 `top`（workspace／accounts／settings），名稱相同的直接沿用。 */
function savedView(saved: Partial<Record<string, unknown>> | null): View {
  const value = saved?.view ?? saved?.top;
  if (value === "workspace" || isOneOf(GLOBAL_NAV, value)) return value;
  return DEFAULT_SHELL.view;
}

/** 讀取上次的位置；讀不到、讀不懂或名稱不認得的部分一律用預設值，不報錯。 */
function loadShell(): ShellState {
  try {
    const raw = localStorage.getItem(SHELL_STORAGE_KEY);
    if (!raw) return DEFAULT_SHELL;
    const saved = JSON.parse(raw) as Partial<Record<string, unknown>> | null;
    return {
      view: savedView(saved),
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

/** `createClient` 只供開發用預覽注入假資料；正式畫面不傳，依執行環境挑選用戶端。 */
export function App({ createClient = createRuntimeClient }: { createClient?: () => Promise<AppClient> } = {}) {
  const [client, setClient] = useState<AppClient | null>(null);
  const [health, setHealth] = useState<HealthResponse | null>(null);
  // 精靈與帳號頁其中一邊改變了 Profile 狀態時，讓另一邊重新讀取
  const [accountsVersion, setAccountsVersion] = useState(0);
  const accountsChanged = () => setAccountsVersion((version) => version + 1);
  // 沒有任何可用的 Account Profile 時，首次精靈以全畫面顯示；讀不到狀態時照常顯示主畫面。
  // 使用者可以先進入主畫面（只在這次開啟有效），避免登入失效時連 Workspace 都看不到
  const [needsSetup, setNeedsSetup] = useState(false);
  const [setupSkipped, setSetupSkipped] = useState(false);
  const fullScreenSetup = needsSetup && !setupSkipped;
  const [shell, setShell] = useState<ShellState>(loadShell);
  // 跑道欄最近一次讀到的使用中 Workspace；選中的 Workspace 不在其中（例如已封存）時視為未選
  const [workspaces, setWorkspaces] = useState<Workspace[]>([]);
  const selected = workspaces.find((workspace) => workspace.id === shell.workspaceId) ?? null;
  // 待裁定數由狀態列輪詢；回答裁定後要求它立即重新讀取
  const [pending, setPending] = useState<PendingDecisions | null>(null);
  const [pendingVersion, setPendingVersion] = useState(0);
  const pendingByWorkspace = Object.fromEntries((pending?.by_workspace ?? []).map((w) => [w.workspace_id, w.pending_count]));
  // 空狀態的「新增 Workspace」要求跑道欄展開建立表單並聚焦目錄欄位
  const [createRequest, setCreateRequest] = useState(0);

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
      view: "workspace",
      workspaceId: workspace.id,
      workspaceTab: selected ? current.workspaceTab : "controller",
    }));

  const selectWorkspacePage = (page: WorkspacePage) => {
    setShell((current) => ({ ...current, view: "workspace", workspaceTab: page }));
    if (page === "decisions") setPendingVersion((version) => version + 1);
  };

  const selectGlobal = (view: GlobalPage) => setShell((current) => ({ ...current, view }));

  const statusBar = client ? (
    <StatusBar client={client} onHealth={setHealth} onPending={setPending} refreshKey={pendingVersion} />
  ) : null;
  const storage = health?.storage;
  // 有新版本時只通知；更新維護中以覆蓋層顯示進度（spec D-19）。儲存故障停止派工時，整寬警示
  const notices = client && (
    <div className="app-notices">
      {storage?.dispatch_halted && <StorageFault fault={storage.fault ?? null} />}
      <UpdateNotice client={client} runtimeStartedAt={health?.started_at ?? null} />
    </div>
  );

  if (fullScreenSetup) {
    return (
      <div className="app app-setup">
        <header className="setup-topbar">
          {/* 產品名稱取自 Runtime 回報，不在前端寫死（spec D-01） */}
          <h1 className="product-name">{health?.product ?? ""}</h1>
          {statusBar}
        </header>
        <main className="main setup-screen">
          {notices}
          {client && (
            <>
              <SetupWizard client={client} onChange={accountsChanged} refreshKey={accountsVersion} />
              <AccountsPage client={client} onChange={accountsChanged} refreshKey={accountsVersion} />
              <button type="button" className="setup-skip btn-quiet" onClick={() => setSetupSkipped(true)}>
                先進入主畫面
              </button>
            </>
          )}
        </main>
        {client && <ExitProgress client={client} />}
      </div>
    );
  }

  const decisionCount = selected ? (pendingByWorkspace[selected.id] ?? 0) : 0;
  const currentNav = selected && (
    <WorkspaceNav
      current={shell.view === "workspace" ? shell.workspaceTab : null}
      decisionCount={decisionCount}
      onSelect={selectWorkspacePage}
    />
  );

  return (
    <div className="app">
      <Rail
        product={health?.product ?? ""}
        position={`${shell.view}:${shell.workspaceId}:${shell.workspaceTab}`}
        status={statusBar}
        global={<GlobalNav current={shell.view === "workspace" ? null : shell.view} onSelect={selectGlobal} />}
      >
        {client && (
          <WorkspaceSidebar
            client={client}
            onSelect={selectWorkspace}
            selectedId={selected?.id ?? null}
            pendingDecisions={pendingByWorkspace}
            onWorkspaces={setWorkspaces}
            currentNav={currentNav}
            createRequest={createRequest}
          />
        )}
      </Rail>
      <main className="main">
        {notices}
        {client && shell.view === "workspace" && !selected && <WorkspaceEmpty onCreate={() => setCreateRequest((n) => n + 1)} />}
        {client && shell.view === "workspace" && selected && (
          <>
            <WorkspaceContext workspace={selected} />
            <WorkspaceView
              client={client}
              workspace={selected}
              page={shell.workspaceTab}
              onDecisionsChange={() => setPendingVersion((version) => version + 1)}
            />
          </>
        )}
        {client && shell.view !== "workspace" && (
          <GlobalView view={shell.view} client={client} accountsVersion={accountsVersion} onAccountsChange={accountsChanged} />
        )}
      </main>
      {/* 系統匣「完全退出」的停止進度，以覆蓋層顯示 */}
      {client && <ExitProgress client={client} />}
    </div>
  );
}

/**
 * 跑道欄：產品名稱 → Workspace 清單（目前的 Workspace 展開它的頁面）→ 平台頁面 → 狀態。
 * 目前頁面以「接力棒」標記；換頁時棒子沿著跑道移過去（減少動態時直接出現）。
 */
function Rail({
  product,
  position,
  status,
  global,
  children,
}: {
  product: string;
  /** 目前位置的鍵；改變時重新量接力棒的位置。 */
  position: string;
  status: ReactNode;
  global: ReactNode;
  children: ReactNode;
}) {
  const trackRef = useRef<HTMLDivElement>(null);
  const batonRef = useRef<HTMLSpanElement>(null);

  useLayoutEffect(() => {
    const track = trackRef.current;
    const baton = batonRef.current;
    if (!track || !baton) return;
    const place = () => {
      const current = track.querySelector<HTMLElement>('.nav-item[aria-current="page"]');
      if (!current) {
        baton.style.opacity = "0";
        return;
      }
      const trackBox = track.getBoundingClientRect();
      const box = current.getBoundingClientRect();
      // 棒子第一次出現時直接就位，之後換頁才沿跑道移動
      const appearing = baton.style.opacity !== "1";
      if (appearing) baton.style.transition = "none";
      baton.style.opacity = "1";
      baton.style.height = `${box.height}px`;
      baton.style.transform = `translateY(${box.top - trackBox.top + track.scrollTop}px)`;
      if (appearing) {
        void baton.offsetHeight;
        baton.style.transition = "";
      }
    };
    place();
    if (typeof ResizeObserver === "undefined") return;
    // Workspace 清單非同步載入、展開管理選單時，上方內容高度改變，棒子跟著移動
    const observer = new ResizeObserver(place);
    for (const child of Array.from(track.children)) observer.observe(child);
    return () => observer.disconnect();
  }, [position]);

  return (
    <div className="rail">
      <div className="rail-head">
        {/* 產品名稱取自 Runtime 回報，不在前端寫死（spec D-01） */}
        <h1 className="product-name">{product}</h1>
      </div>
      <div className="rail-track" ref={trackRef}>
        <span className="rail-baton" ref={batonRef} aria-hidden="true" />
        {children}
        {global}
      </div>
      <div className="rail-foot">{status}</div>
    </div>
  );
}

/** 跑道欄上的一個頁面按鈕；有待處理數時附徽章（數字以說明提供給螢幕報讀器）。 */
function NavButton<T extends string>({
  item,
  current,
  badge = 0,
  badgeLabel,
  onSelect,
}: {
  item: NavItem<T>;
  current: boolean;
  badge?: number;
  badgeLabel?: string;
  onSelect: (id: T) => void;
}) {
  const badgeId = `nav-badge-${item.id}`;
  return (
    <button
      type="button"
      className="nav-item"
      aria-current={current ? "page" : undefined}
      // 有徽章時名稱仍只是頁名，數字以說明提供
      aria-label={badge > 0 ? item.label : undefined}
      aria-describedby={badge > 0 ? badgeId : undefined}
      onClick={() => onSelect(item.id)}
    >
      <span className="nav-label">{item.label}</span>
      {badge > 0 && (
        <span className="nav-badge" id={badgeId} title={badgeLabel}>
          <span className="visually-hidden">{badgeLabel}</span>
          <span aria-hidden="true">{formatCount(badge)}</span>
        </span>
      )}
    </button>
  );
}

/** 目前 Workspace 的頁面，依群組列出。 */
function WorkspaceNav({
  current,
  decisionCount,
  onSelect,
}: {
  current: WorkspacePage | null;
  decisionCount: number;
  onSelect: (page: WorkspacePage) => void;
}) {
  return (
    <nav className="workspace-nav" aria-label="Workspace 頁面">
      {WORKSPACE_NAV.map((group) => (
        <div key={group.label} className="nav-group" role="group" aria-label={group.label}>
          <span className="nav-group-label" aria-hidden="true">
            {group.label}
          </span>
          {group.pages.map((page) => (
            <NavButton
              key={page.id}
              item={page}
              current={page.id === current}
              badge={page.id === "decisions" ? decisionCount : 0}
              badgeLabel={page.id === "decisions" ? `待裁定 ${formatNumber(decisionCount)}` : undefined}
              onSelect={onSelect}
            />
          ))}
        </div>
      ))}
    </nav>
  );
}

/** 平台頁面：帳號、共同設定、App 設定。 */
function GlobalNav({ current, onSelect }: { current: GlobalPage | null; onSelect: (page: GlobalPage) => void }) {
  return (
    <nav className="global-nav" aria-label="平台">
      {GLOBAL_NAV.map((item) => (
        <NavButton key={item.id} item={item} current={item.id === current} onSelect={onSelect} />
      ))}
    </nav>
  );
}

/** 未選 Workspace 時的空狀態：「新增 Workspace」展開跑道欄的建立表單並聚焦目錄欄位。 */
function WorkspaceEmpty({ onCreate }: { onCreate: () => void }) {
  return (
    <section className="empty-state workspace-empty" aria-labelledby="workspace-empty-title">
      <h2 id="workspace-empty-title">選一個 Workspace 開始</h2>
      <p>Workspace 對應你電腦上的一個目錄。從左側選一個，或新增一個來放 Goal 與 Task。</p>
      <button type="button" className="btn-primary" onClick={onCreate}>
        新增 Workspace
      </button>
    </section>
  );
}

/** 頁面上方的情境列：目前 Workspace 的名稱與縮短路徑；完整值在 title，路徑可複製。 */
function WorkspaceContext({ workspace }: { workspace: Workspace }) {
  return (
    <div className="workspace-context" aria-label="目前的 Workspace">
      <span className="workspace-context-name truncate" title={workspace.name}>
        {workspace.name}
      </span>
      <span className="workspace-context-path copy-text">
        <PathText path={workspace.path} />
        <CopyButton text={workspace.path} label="複製路徑" />
      </span>
    </div>
  );
}

/** 選中 Workspace 後，目前頁面的功能。 */
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
    case "workflows":
      return <WorkflowPage key={workspace.id} client={client} workspace={workspace} />;
    case "decisions":
      return <DecisionCenter key={workspace.id} client={client} workspace={workspace} onChange={onDecisionsChange} />;
    case "timeline":
      return <TimelinePage key={workspace.id} client={client} workspace={workspace} />;
    case "artifacts":
      return <ArtifactsPage key={workspace.id} client={client} workspace={workspace} />;
    case "reviews":
      return <ReviewPage key={workspace.id} client={client} workspace={workspace} />;
    case "verification":
      return <VerificationPage key={workspace.id} client={client} workspace={workspace} />;
    case "exploration":
      return <ExplorationPage key={workspace.id} client={client} workspace={workspace} />;
    case "knowledge":
      return <KnowledgePage key={workspace.id} client={client} workspace={workspace} />;
    case "understanding":
      return <ProjectUnderstandingPage key={workspace.id} client={client} workspace={workspace} />;
    case "authorization":
      return <AuthorizationPanel key={workspace.id} client={client} workspace={workspace} />;
    case "authority":
      return <DecisionAuthorityPanel key={workspace.id} client={client} workspace={workspace} />;
    case "models":
      return <ModelOverridesPanel key={workspace.id} client={client} workspace={workspace} />;
  }
}

/** 平台頁面的內容；由多個元件組成的頁面（帳號、設定）在這裡加上頁面標題。 */
function GlobalView({
  view,
  client,
  accountsVersion,
  onAccountsChange,
}: {
  view: GlobalPage;
  client: AppClient;
  accountsVersion: number;
  onAccountsChange: () => void;
}) {
  switch (view) {
    case "accounts":
      return (
        <div className="page global-page">
          <header className="page-header">
            <h2>帳號</h2>
          </header>
          <SetupWizard client={client} onChange={onAccountsChange} refreshKey={accountsVersion} />
          <AccountsPage client={client} onChange={onAccountsChange} refreshKey={accountsVersion} />
          <SchedulingSettings client={client} />
        </div>
      );
    case "shared":
      return (
        <div className="page global-page">
          <SharedSettingsPage client={client} />
        </div>
      );
    case "settings":
      return (
        <div className="page global-page">
          <header className="page-header">
            <h2>設定</h2>
          </header>
          <LoginStartupSettings client={client} />
          <UpdateSettings client={client} />
          <BackupSettings client={client} />
          <RecoveryPanel client={client} />
          <PlatformPage client={client} />
        </div>
      );
  }
}
