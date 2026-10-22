import { useEffect, useState } from "react";

import { AccountsPage } from "./AccountsPage";
import { ArtifactsPage } from "./ArtifactsPage";
import { AuthorizationPanel } from "./AuthorizationPanel";
import { DecisionCenter } from "./DecisionCenter";
import { PlatformPage } from "./PlatformPage";
import { SchedulingSettings } from "./SchedulingSettings";
import { SetupWizard } from "./SetupWizard";
import { StatusBar } from "./StatusBar";
import { createRuntimeClient, type AppClient, type HealthResponse, type PendingDecisions, type Workspace } from "./runtimeClient";
import { TaskBoard } from "./TaskBoard";
import { WorkspaceSidebar } from "./WorkspaceSidebar";

/** 使用者選到某個 Workspace 後，主區顯示它的授權設定、工作面板、裁定中心或成果與驗證。 */
type Opened = { view: "authorization" | "tasks" | "decisions" | "artifacts"; workspace: Workspace };

export function App() {
  const [client, setClient] = useState<AppClient | null>(null);
  const [health, setHealth] = useState<HealthResponse | null>(null);
  // 精靈與帳號頁其中一邊改變了 Profile 狀態時，讓另一邊重新讀取
  const [accountsVersion, setAccountsVersion] = useState(0);
  const accountsChanged = () => setAccountsVersion((version) => version + 1);
  const [opened, setOpened] = useState<Opened | null>(null);
  // 待裁定數由狀態列輪詢；回答裁定後要求它立即重新讀取
  const [pending, setPending] = useState<PendingDecisions | null>(null);
  const [pendingVersion, setPendingVersion] = useState(0);
  const pendingByWorkspace = Object.fromEntries((pending?.by_workspace ?? []).map((w) => [w.workspace_id, w.pending_count]));

  useEffect(() => {
    void createRuntimeClient().then(setClient);
  }, []);

  return (
    <div className="app">
      {client ? <StatusBar client={client} onHealth={setHealth} onPending={setPending} refreshKey={pendingVersion} /> : <header className="status-bar" />}
      <div className="body">
        {client && (
          <WorkspaceSidebar
            client={client}
            onSelect={(workspace) => setOpened({ view: "authorization", workspace })}
            onOpenTasks={(workspace) => setOpened({ view: "tasks", workspace })}
            onOpenDecisions={(workspace) => {
              setOpened({ view: "decisions", workspace });
              setPendingVersion((version) => version + 1);
            }}
            pendingDecisions={pendingByWorkspace}
            onOpenArtifacts={(workspace) => setOpened({ view: "artifacts", workspace })}
          />
        )}
        <main className="main">
          {/* 產品名稱取自 Runtime 回報，不在前端寫死（spec D-01） */}
          <h1>{health?.product ?? ""}</h1>
          {client && <SetupWizard client={client} onChange={accountsChanged} refreshKey={accountsVersion} />}
          {client && <AccountsPage client={client} onChange={accountsChanged} refreshKey={accountsVersion} />}
          {client && <SchedulingSettings client={client} />}
          {client && opened?.view === "authorization" && <AuthorizationPanel client={client} workspace={opened.workspace} />}
          {client && opened?.view === "tasks" && <TaskBoard client={client} workspace={opened.workspace} />}
          {client && opened?.view === "decisions" && (
            <DecisionCenter
              client={client}
              workspace={opened.workspace}
              onChange={() => setPendingVersion((version) => version + 1)}
            />
          )}
          {client && opened?.view === "artifacts" && <ArtifactsPage client={client} workspace={opened.workspace} />}
          {client && <PlatformPage client={client} />}
        </main>
      </div>
    </div>
  );
}
