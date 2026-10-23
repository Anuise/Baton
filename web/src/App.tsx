import { useEffect, useState } from "react";

import { AccountsPage } from "./AccountsPage";
import { ArtifactsPage } from "./ArtifactsPage";
import { AuthorizationPanel } from "./AuthorizationPanel";
import { BackupSettings } from "./BackupSettings";
import { ControllerPage } from "./ControllerPage";
import { ConversationPage } from "./ConversationPage";
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
import { TimelinePage } from "./TimelinePage";
import { UpdateNotice } from "./UpdateNotice";
import { UpdateSettings } from "./UpdateSettings";
import { ReviewPage } from "./ReviewPage";
import { VerificationPage } from "./VerificationPage";
import { WorkflowPage } from "./WorkflowPage";
import { WorkspaceSidebar } from "./WorkspaceSidebar";

/** 頂層頁簽。 */
type TopTab = "workspace" | "accounts" | "settings";
/** 選中 Workspace 後的功能頁簽；「對話」暫時保留，由票 08 移除。 */
type WorkspaceTab =
  | "controller"
  | "tasks"
  | "decisions"
  | "artifacts"
  | "quality"
  | "knowledge"
  | "workflows"
  | "timeline"
  | "authorization"
  | "conversation";
/** 「審查與驗證」頁簽內的小頁簽。 */
type QualityTab = "reviews" | "verification" | "exploration";

interface TabItem<T extends string> {
  id: T;
  label: string;
  /** 徽章數字；大於 0 才顯示。 */
  badge?: number;
}

const TOP_TABS: TabItem<TopTab>[] = [
  { id: "workspace", label: "Workspace" },
  { id: "accounts", label: "帳號" },
  { id: "settings", label: "設定" },
];

const WORKSPACE_TABS: TabItem<WorkspaceTab>[] = [
  { id: "controller", label: "Controller" },
  { id: "tasks", label: "工作" },
  { id: "decisions", label: "裁定" },
  { id: "artifacts", label: "成果" },
  { id: "quality", label: "審查與驗證" },
  { id: "knowledge", label: "知識" },
  { id: "workflows", label: "流程" },
  { id: "timeline", label: "時間線" },
  { id: "authorization", label: "授權" },
  { id: "conversation", label: "對話" },
];

const QUALITY_TABS: TabItem<QualityTab>[] = [
  { id: "reviews", label: "審查" },
  { id: "verification", label: "驗證" },
  { id: "exploration", label: "探索" },
];

/** 所在頁簽與 Workspace 存在本機（每個檢視者的偏好，spec Q20）。 */
export const SHELL_STORAGE_KEY = "baton.shell";

interface ShellState {
  top: TopTab;
  workspaceId: string | null;
  workspaceTab: WorkspaceTab;
}

const DEFAULT_SHELL: ShellState = { top: "workspace", workspaceId: null, workspaceTab: "controller" };

function isTab<T extends string>(tabs: TabItem<T>[], value: unknown): value is T {
  return tabs.some((tab) => tab.id === value);
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
      workspaceTab: isTab(WORKSPACE_TABS, saved?.workspaceTab) ? saved.workspaceTab : DEFAULT_SHELL.workspaceTab,
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
              <span className="tab-badge" title={`待裁定 ${badge}`}>
                {badge}
              </span>
            )}
          </button>
        );
      })}
    </div>
  );
}

/** 一排頁簽對應的內容區。 */
function TabPanel<T extends string>({ idPrefix, selected, children }: { idPrefix: string; selected: T; children: React.ReactNode }) {
  return (
    <div role="tabpanel" id={`${idPrefix}-panel`} aria-labelledby={`${idPrefix}-tab-${selected}`} className="tab-panel">
      {children}
    </div>
  );
}

export function App() {
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
  const [qualityTab, setQualityTab] = useState<QualityTab>("reviews");
  // 側欄最近一次讀到的使用中 Workspace；選中的 Workspace 不在其中（例如已封存）時視為未選
  const [workspaces, setWorkspaces] = useState<Workspace[]>([]);
  const selected = workspaces.find((workspace) => workspace.id === shell.workspaceId) ?? null;
  // 待裁定數由狀態列輪詢；回答裁定後要求它立即重新讀取
  const [pending, setPending] = useState<PendingDecisions | null>(null);
  const [pendingVersion, setPendingVersion] = useState(0);
  const pendingByWorkspace = Object.fromEntries((pending?.by_workspace ?? []).map((w) => [w.workspace_id, w.pending_count]));

  useEffect(() => {
    void createRuntimeClient().then(setClient);
  }, []);

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
    // 在 Workspace 之間切換時停在同一頁簽；從未選到選中時從 Controller 開始
    setShell((current) => ({
      ...current,
      workspaceId: workspace.id,
      workspaceTab: selected ? current.workspaceTab : "controller",
    }));

  const selectWorkspaceTab = (tab: WorkspaceTab) => {
    setShell((current) => ({ ...current, workspaceTab: tab }));
    if (tab === "decisions") setPendingVersion((version) => version + 1);
  };

  const workspaceTabs = WORKSPACE_TABS.map((tab) =>
    tab.id === "decisions" && selected ? { ...tab, badge: pendingByWorkspace[selected.id] ?? 0 } : tab,
  );

  return (
    <div className="app">
      {client ? <StatusBar client={client} onHealth={setHealth} onPending={setPending} refreshKey={pendingVersion} /> : <header className="status-bar" />}
      <div className="app-heading">
        {/* 產品名稱取自 Runtime 回報，不在前端寫死（spec D-01） */}
        <h1>{health?.product ?? ""}</h1>
        {/* 有新版本時只通知；更新維護中以覆蓋層顯示進度（spec D-19）。在頁簽之外，切換頁簽不影響 */}
        {client && <UpdateNotice client={client} runtimeStartedAt={health?.started_at ?? null} />}
      </div>
      {client && fullScreenSetup && (
        <main className="main setup-screen">
          <SetupWizard client={client} onChange={accountsChanged} refreshKey={accountsVersion} />
          <AccountsPage client={client} onChange={accountsChanged} refreshKey={accountsVersion} />
          <button type="button" className="setup-skip" onClick={() => setSetupSkipped(true)}>
            先進入主畫面
          </button>
        </main>
      )}
      {!fullScreenSetup && (
        <>
          <TabList
            label="主要區塊"
            idPrefix="top"
            tabs={TOP_TABS}
            selected={shell.top}
            onSelect={(top) => setShell((current) => ({ ...current, top }))}
          />
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
              <TabPanel idPrefix="top" selected={shell.top}>
                {client && shell.top === "workspace" && !selected && (
                  <p className="workspace-hint">請從左側選擇或建立 Workspace</p>
                )}
                {client && shell.top === "workspace" && selected && (
                  <>
                    <TabList
                      label="Workspace 功能"
                      idPrefix="workspace"
                      tabs={workspaceTabs}
                      selected={shell.workspaceTab}
                      onSelect={selectWorkspaceTab}
                    />
                    <TabPanel idPrefix="workspace" selected={shell.workspaceTab}>
                      <WorkspaceView
                        client={client}
                        workspace={selected}
                        tab={shell.workspaceTab}
                        qualityTab={qualityTab}
                        onQualityTab={setQualityTab}
                        onDecisionsChange={() => setPendingVersion((version) => version + 1)}
                      />
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
        </>
      )}
      {/* 系統匣「完全退出」的停止進度，以覆蓋層顯示 */}
      {client && <ExitProgress client={client} />}
    </div>
  );
}

/** 選中 Workspace 後，目前頁簽的功能；各功能元件原樣放入。 */
function WorkspaceView({
  client,
  workspace,
  tab,
  qualityTab,
  onQualityTab,
  onDecisionsChange,
}: {
  client: AppClient;
  workspace: Workspace;
  tab: WorkspaceTab;
  qualityTab: QualityTab;
  onQualityTab: (tab: QualityTab) => void;
  onDecisionsChange: () => void;
}) {
  // 換 Workspace 時以 key 重新掛載，上一個 Workspace 較晚回來的讀取結果不會蓋掉目前畫面與草稿
  switch (tab) {
    case "controller":
      return <ControllerPage key={workspace.id} client={client} workspace={workspace} />;
    case "tasks":
      return <TaskBoard key={workspace.id} client={client} workspace={workspace} />;
    case "decisions":
      return <DecisionCenter key={workspace.id} client={client} workspace={workspace} onChange={onDecisionsChange} />;
    case "artifacts":
      return <ArtifactsPage key={workspace.id} client={client} workspace={workspace} />;
    case "quality":
      return (
        <>
          <TabList label="審查與驗證" idPrefix="quality" tabs={QUALITY_TABS} selected={qualityTab} onSelect={onQualityTab} />
          <TabPanel idPrefix="quality" selected={qualityTab}>
            {qualityTab === "reviews" && <ReviewPage key={workspace.id} client={client} workspace={workspace} />}
            {/* 換 Workspace 時重新掛載，清掉上一個 Workspace 正在編輯的 Profile 與訊息 */}
            {qualityTab === "verification" && <VerificationPage key={workspace.id} client={client} workspace={workspace} />}
            {qualityTab === "exploration" && <ExplorationPage key={workspace.id} client={client} workspace={workspace} />}
          </TabPanel>
        </>
      );
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
    case "conversation":
      return <ConversationPage key={workspace.id} client={client} workspace={workspace} />;
  }
}
