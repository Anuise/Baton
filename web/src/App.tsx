import { useEffect, useState } from "react";

import { AuthorizationPanel } from "./AuthorizationPanel";
import { PlatformPage } from "./PlatformPage";
import { StatusBar } from "./StatusBar";
import { createRuntimeClient, type AppClient, type HealthResponse, type Workspace } from "./runtimeClient";
import { WorkspaceSidebar } from "./WorkspaceSidebar";

export function App() {
  const [client, setClient] = useState<AppClient | null>(null);
  const [health, setHealth] = useState<HealthResponse | null>(null);
  const [authorizing, setAuthorizing] = useState<Workspace | null>(null);

  useEffect(() => {
    void createRuntimeClient().then(setClient);
  }, []);

  return (
    <div className="app">
      {client ? <StatusBar client={client} onHealth={setHealth} /> : <header className="status-bar" />}
      <div className="body">
        {client && <WorkspaceSidebar client={client} onSelect={setAuthorizing} />}
        <main className="main">
          {/* 產品名稱取自 Runtime 回報，不在前端寫死（spec D-01） */}
          <h1>{health?.product ?? ""}</h1>
          {client && authorizing && <AuthorizationPanel client={client} workspace={authorizing} />}
          {client && <PlatformPage client={client} />}
        </main>
      </div>
    </div>
  );
}
