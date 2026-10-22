import { useEffect, useState } from "react";

import { platformStatusLabel } from "./PlatformPage";
import type {
  AccountClient,
  DecisionClient,
  HealthResponse,
  PendingDecisions,
  PlatformResponse,
  RecoveryClient,
  RecoveryStatusView,
  RuntimeClient,
} from "./runtimeClient";

type RuntimeStatus =
  | { state: "checking" }
  | { state: "running"; health: HealthResponse }
  | { state: "offline"; reason: string };

interface Props {
  /** 有 `listProfiles` 時才顯示可用帳號數；有 `pendingDecisions` 時才顯示待裁定數；有 `recoveryStatus` 時才顯示恢復事件狀態。 */
  client: RuntimeClient &
    Partial<Pick<AccountClient, "listProfiles">> &
    Partial<Pick<DecisionClient, "pendingDecisions">> &
    Partial<Pick<RecoveryClient, "recoveryStatus">>;
  /** 重新檢查的間隔（毫秒）。 */
  pollMs?: number;
  onHealth?: (health: HealthResponse | null) => void;
  /** 每次取得待裁定數時通知外層（讀取失敗為 null）。 */
  onPending?: (pending: PendingDecisions | null) => void;
  /** 值改變時立即重新檢查（例如剛回答完裁定）。 */
  refreshKey?: number;
}

/** 監護不在、Runtime 又沒有附訊息時的降級提示。 */
const GUARDIAN_UNAVAILABLE_FALLBACK = "工作繼續，自動重啟保護不可用";

/** 頂部狀態列（spec D-21）：Runtime 狀態、平台支援狀態、可用帳號數、待裁定數、恢復事件、監護狀態與儲存故障。 */
export function StatusBar({ client, pollMs = 5000, onHealth, onPending, refreshKey }: Props) {
  const [status, setStatus] = useState<RuntimeStatus>({ state: "checking" });
  // null：尚未取得或讀取失敗，顯示為「未知」，不影響 Runtime 狀態
  const [platform, setPlatform] = useState<PlatformResponse | null>(null);
  // null：尚未取得或讀取失敗，不顯示可用帳號數
  const [usableAccounts, setUsableAccounts] = useState<number | null>(null);
  // null：尚未取得或讀取失敗，不顯示待裁定數
  const [pending, setPending] = useState<PendingDecisions | null>(null);
  // null：尚未取得或讀取失敗，不顯示恢復事件狀態
  const [recovery, setRecovery] = useState<RecoveryStatusView | null>(null);

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
        // 可派工（核對無誤、沒有阻擋原因）的帳號數；與 Runtime 排程的「可用帳號」相同，已封存的不算
        const profiles = (await client.listProfiles?.().catch(() => null)) ?? null;
        if (!cancelled) setUsableAccounts(profiles && profiles.profiles.filter((p) => p.dispatchable).length);
        const pending = (await client.pendingDecisions?.().catch(() => null)) ?? null;
        if (!cancelled) {
          setPending(pending);
          onPending?.(pending);
        }
        const recovery = (await client.recoveryStatus?.().catch(() => null)) ?? null;
        if (!cancelled) setRecovery(recovery);
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
      {status.state === "running" && usableAccounts !== null && (
        <span className="status status-accounts">可用帳號：{usableAccounts}</span>
      )}
      {status.state === "running" && pending !== null && (
        <span className={`status status-decisions${pending.pending_count > 0 ? " status-decisions-pending" : ""}`}>
          待裁定：{pending.pending_count}
        </span>
      )}
      {/* 恢復事件未結束時固定顯示（spec D-21）；已恢復就不再佔位 */}
      {status.state === "running" && recovery?.event != null && recovery.event.status !== "succeeded" && (
        <span className={`status status-recovery status-recovery-${recovery.event.status}`}>
          {recovery.event.status === "exhausted"
            ? "恢復額度用盡"
            : `恢復中（${recovery.event.attempts_used}/${recovery.max_attempts}）`}
        </span>
      )}
      {/* 監護狀態固定顯示（spec D-21）；未知（含舊版 Runtime 沒有欄位）不顯示，避免誤報 */}
      {status.state === "running" && status.health.guardian?.state === "active" && (
        <span className="status status-guardian-active">監護：運作中</span>
      )}
      {status.state === "running" && status.health.guardian?.state === "unavailable" && (
        <span className="status status-guardian-unavailable" role="alert">
          {status.health.guardian.message ?? GUARDIAN_UNAVAILABLE_FALLBACK}
        </span>
      )}
      {status.state === "running" && status.health.storage?.dispatch_halted && (
        <div className="status-storage-fault" role="alert">
          <strong>新派工已停止</strong>
          {status.health.storage.fault && (
            <>
              <span>{status.health.storage.fault.summary}</span>
              <span>{status.health.storage.fault.action}</span>
              <small className="status-storage-detail">{status.health.storage.fault.detail}</small>
            </>
          )}
        </div>
      )}
    </header>
  );
}
