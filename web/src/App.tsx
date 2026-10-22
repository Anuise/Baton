import { useEffect, useState } from "react";

import { AccountsPage } from "./AccountsPage";
import { PlatformPage } from "./PlatformPage";
import { StatusBar } from "./StatusBar";
import { createRuntimeClient, type AppClient, type HealthResponse } from "./runtimeClient";
import { SetupWizard } from "./SetupWizard";
import { WorkspaceSidebar } from "./WorkspaceSidebar";

export function App() {
  const [client, setClient] = useState<AppClient | null>(null);
  const [health, setHealth] = useState<HealthResponse | null>(null);
  // 精靈與帳號頁其中一邊改變了 Profile 狀態時，讓另一邊重新讀取
  const [accountsVersion, setAccountsVersion] = useState(0);
  const accountsChanged = () => setAccountsVersion((version) => version + 1);

  useEffect(() => {
    void createRuntimeClient().then(setClient);
  }, []);

  return (
    <div className="app">
      {client ? <StatusBar client={client} onHealth={setHealth} /> : <header className="status-bar" />}
      <div className="body">
        {client && <WorkspaceSidebar client={client} />}
        <main className="main">
          {/* 產品名稱取自 Runtime 回報，不在前端寫死（spec D-01） */}
          <h1>{health?.product ?? ""}</h1>
          {client && <SetupWizard client={client} onChange={accountsChanged} refreshKey={accountsVersion} />}
          {client && <AccountsPage client={client} onChange={accountsChanged} refreshKey={accountsVersion} />}
          {client && <PlatformPage client={client} />}
        </main>
      </div>
    </div>
  );
}
