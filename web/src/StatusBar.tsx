import { useEffect, useState } from "react";

import { platformStatusLabel } from "./PlatformPage";
import type { DecisionClient, HealthResponse, PendingDecisions, PlatformResponse, RuntimeClient } from "./runtimeClient";

type RuntimeStatus =
  | { state: "checking" }
  | { state: "running"; health: HealthResponse }
  | { state: "offline"; reason: string };

interface Props {
  /** 有 `pendingDecisions` 時才顯示待裁定數。 */
  client: RuntimeClient & Partial<Pick<DecisionClient, "pendingDecisions">>;
  /** 重新檢查的間隔（毫秒）。 */
  pollMs?: number;
  onHealth?: (health: HealthResponse | null) => void;
  /** 每次取得待裁定數時通知外層（讀取失敗為 null）。 */
  onPending?: (pending: PendingDecisions | null) => void;
  /** 值改變時立即重新檢查（例如剛回答完裁定）。 */
  refreshKey?: number;
}

/** 頂部狀態列：目前顯示 Runtime 狀態與平台支援狀態，其餘欄位由後續的票加入（spec D-21）。 */
export function StatusBar({ client, pollMs = 5000, onHealth, onPending, refreshKey }: Props) {
  const [status, setStatus] = useState<RuntimeStatus>({ state: "checking" });
  // null：尚未取得或讀取失敗，顯示為「未知」，不影響 Runtime 狀態
  const [platform, setPlatform] = useState<PlatformResponse | null>(null);
  // null：尚未取得或讀取失敗，不顯示待裁定數
  const [pending, setPending] = useState<PendingDecisions | null>(null);

  useEffect(() => {
    let cancelled = false;
    const check = async () => {
      try {
        const health = await client.getHealth();
        if (cancelled) return;
        setStatus({ state: "running", health });
        onHealth?.(health);
        const platform = await client.getPlatform().catch(() => null);
        if (!cancelled) setPlatform(platform);
        const pending = (await client.pendingDecisions?.().catch(() => null)) ?? null;
        if (!cancelled) {
          setPending(pending);
          onPending?.(pending);
        }
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
  }, [client, pollMs, onHealth, onPending, refreshKey]);

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
      {status.state === "running" && (
        <span className={`status status-platform-${platform?.assessment.status ?? "unknown"}`}>
          平台：{platformStatusLabel(platform?.assessment.status ?? "unknown")}
        </span>
      )}
      {status.state === "running" && pending !== null && (
        <span className={`status status-decisions${pending.pending_count > 0 ? " status-decisions-pending" : ""}`}>
          待裁定：{pending.pending_count}
        </span>
      )}
    </header>
  );
}
