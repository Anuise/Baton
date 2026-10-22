import { useEffect, useState } from "react";

import { StatusBar } from "./StatusBar";
import { createRuntimeClient, type HealthResponse, type RuntimeClient } from "./runtimeClient";

export function App() {
  const [client, setClient] = useState<RuntimeClient | null>(null);
  const [health, setHealth] = useState<HealthResponse | null>(null);

  useEffect(() => {
    void createRuntimeClient().then(setClient);
  }, []);

  return (
    <div className="app">
      {client ? <StatusBar client={client} onHealth={setHealth} /> : <header className="status-bar" />}
      <main className="main">
        {/* 產品名稱取自 Runtime 回報，不在前端寫死（spec D-01） */}
        <h1>{health?.product ?? ""}</h1>
      </main>
    </div>
  );
}
