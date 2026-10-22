import { useEffect, useState } from "react";

import { AuthorizationPanel } from "./AuthorizationPanel";
import { PlatformPage } from "./PlatformPage";
import { StatusBar } from "./StatusBar";
import { createRuntimeClient, type AppClient, type HealthResponse, type Workspace } from "./runtimeClient";
import { TaskBoard } from "./TaskBoard";
import { WorkspaceSidebar } from "./WorkspaceSidebar";

/** 使用者選到某個 Workspace 後，主區顯示它的授權設定或工作面板。 */
type Opened = { view: "authorization" | "tasks"; workspace: Workspace };

export function App() {
  const [client, setClient] = useState<AppClient | null>(null);
  const [health, setHealth] = useState<HealthResponse | null>(null);
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
          />
        )}
        <main className="main">
          {/* 產品名稱取自 Runtime 回報，不在前端寫死（spec D-01） */}
          <h1>{health?.product ?? ""}</h1>
          {client && opened?.view === "authorization" && <AuthorizationPanel client={client} workspace={opened.workspace} />}
          {client && opened?.view === "tasks" && <TaskBoard client={client} workspace={opened.workspace} />}
          {client && <PlatformPage client={client} />}
        </main>
      </div>
    </div>
  );
}
