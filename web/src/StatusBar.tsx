import { useEffect, useState } from "react";

import type { HealthResponse, RuntimeClient } from "./runtimeClient";

type RuntimeStatus =
  | { state: "checking" }
  | { state: "running"; health: HealthResponse }
  | { state: "offline"; reason: string };

interface Props {
  client: RuntimeClient;
  /** 重新檢查的間隔（毫秒）。 */
  pollMs?: number;
  onHealth?: (health: HealthResponse | null) => void;
}

/** 頂部狀態列：目前只顯示 Runtime 狀態，其餘欄位由後續的票加入（spec D-21）。 */
export function StatusBar({ client, pollMs = 5000, onHealth }: Props) {
  const [status, setStatus] = useState<RuntimeStatus>({ state: "checking" });

  useEffect(() => {
    let cancelled = false;
    const check = async () => {
      try {
        const health = await client.getHealth();
        if (cancelled) return;
        setStatus({ state: "running", health });
        onHealth?.(health);
      } catch (error) {
        if (cancelled) return;
        setStatus({ state: "offline", reason: error instanceof Error ? error.message : String(error) });
        onHealth?.(null);
      }
    };
    void check();
    const timer = setInterval(() => void check(), pollMs);
    return () => {
      cancelled = true;
      clearInterval(timer);
    };
  }, [client, pollMs, onHealth]);

  return (
    <header className="status-bar" role="status">
      {status.state === "checking" && <span className="status status-checking">Runtime 檢查中…</span>}
      {status.state === "running" && <span className="status status-running">Runtime 運作中</span>}
      {status.state === "offline" && (
        <>
          <span className="status status-offline">Runtime 未連線</span>
          <span className="status-reason">{status.reason}</span>
        </>
      )}
    </header>
  );
}
