import { useEffect, useState } from "react";

import { AccountsPage } from "./AccountsPage";
import { ArtifactsPage } from "./ArtifactsPage";
import { AuthorizationPanel } from "./AuthorizationPanel";
import { KnowledgePage } from "./KnowledgePage";
import { PlatformPage } from "./PlatformPage";
import { SchedulingSettings } from "./SchedulingSettings";
import { SetupWizard } from "./SetupWizard";
import { SharedSettingsPage } from "./SharedSettingsPage";
import { StatusBar } from "./StatusBar";
import { createRuntimeClient, type AppClient, type HealthResponse, type Workspace } from "./runtimeClient";
import { TaskBoard } from "./TaskBoard";
import { WorkspaceSidebar } from "./WorkspaceSidebar";

/** 使用者選到某個 Workspace 後，主區顯示它的授權設定、工作面板、成果與驗證或知識。 */
type Opened = { view: "authorization" | "tasks" | "artifacts" | "knowledge"; workspace: Workspace };

export function App() {
  const [client, setClient] = useState<AppClient | null>(null);
  const [health, setHealth] = useState<HealthResponse | null>(null);
  // 精靈與帳號頁其中一邊改變了 Profile 狀態時，讓另一邊重新讀取
  const [accountsVersion, setAccountsVersion] = useState(0);
  const accountsChanged = () => setAccountsVersion((version) => version + 1);
  const [opened, setOpened] = useState<Opened | null>(null);

  useEffect(() => {
    void createRuntimeClient().then(setClient);
  }, []);

  return (
    <div className="app">
      {client ? <StatusBar client={client} onHealth={setHealth} /> : <header className="status-bar" />}
      <div className="body">
        {client && (
          <WorkspaceSidebar
            client={client}
            onSelect={(workspace) => setOpened({ view: "authorization", workspace })}
            onOpenTasks={(workspace) => setOpened({ view: "tasks", workspace })}
            onOpenArtifacts={(workspace) => setOpened({ view: "artifacts", workspace })}
            onOpenKnowledge={(workspace) => setOpened({ view: "knowledge", workspace })}
          />
        )}
        <main className="main">
          {/* 產品名稱取自 Runtime 回報，不在前端寫死（spec D-01） */}
          <h1>{health?.product ?? ""}</h1>
          {client && <SetupWizard client={client} onChange={accountsChanged} refreshKey={accountsVersion} />}
          {client && <AccountsPage client={client} onChange={accountsChanged} refreshKey={accountsVersion} />}
          {client && <SchedulingSettings client={client} />}
          {client && <SharedSettingsPage client={client} />}
          {client && opened?.view === "authorization" && <AuthorizationPanel client={client} workspace={opened.workspace} />}
          {client && opened?.view === "tasks" && <TaskBoard client={client} workspace={opened.workspace} />}
          {client && opened?.view === "artifacts" && <ArtifactsPage client={client} workspace={opened.workspace} />}
          {client && opened?.view === "knowledge" && <KnowledgePage client={client} workspace={opened.workspace} />}
          {client && <PlatformPage client={client} />}
        </main>
      </div>
    </div>
  );
}
