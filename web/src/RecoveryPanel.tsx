import { useEffect, useState } from "react";

import type {
  AffectedRun,
  AttemptOutcome,
  RecoveryAttempt,
  RecoveryClient,
  RecoveryEvent,
  RecoveryStatusView,
  RunDisposition,
} from "./runtimeClient";
import { formatTimestamp } from "./time";

type StatusState =
  | { state: "loading" }
  | { state: "ready"; status: RecoveryStatusView }
  | { state: "failed"; reason: string };

interface Props {
  client: RecoveryClient;
  /** 輪詢恢復狀態的間隔（毫秒）。 */
  pollMs?: number;
  /** 只供測試指定時區；平常使用系統時區（spec D-33）。 */
  timeZone?: string;
}

const OUTCOME_LABELS: Record<AttemptOutcome, string> = {
  in_progress: "處置中",
  observing: "穩定觀察中",
  succeeded: "成功",
  failed: "失敗",
};

const DISPOSITION_LABELS: Record<RunDisposition, string> = {
  requeued: "已重新排隊",
  cancelled: "已完成取消",
  unconfirmed: "無法確認停止",
};

function reasonOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** 恢復事件狀態的中文標籤；`waiting` 在本輪用過次數後就是退避期間。 */
function recoveryStatusLabel(event: RecoveryEvent): string {
  switch (event.status) {
    case "waiting":
      return event.attempts_used > 0 ? "退避中" : "等待嘗試";
    case "attempting":
      return "嘗試中";
    case "observing":
      return "穩定觀察中";
    case "succeeded":
      return "已恢復";
    case "exhausted":
      return "恢復額度用盡";
  }
}

/**
 * Runtime 意外崩潰後的自動恢復（票 31）：顯示恢復事件、本輪次數、退避期限、每次嘗試與受影響工作，
 * 並在恢復額度用盡時提供「重新嘗試恢復」。定期輪詢以跟上 Supervisor 的進度。
 */
export function RecoveryPanel({ client, pollMs = 5000, timeZone }: Props) {
  const [loaded, setLoaded] = useState<StatusState>({ state: "loading" });
  const [retrying, setRetrying] = useState(false);
  const [retryError, setRetryError] = useState<string | null>(null);

  useEffect(() => {
    // 卸載後遲到的回應一律忽略
    let cancelled = false;
    const check = () =>
      client.recoveryStatus().then(
        (status) => {
          if (!cancelled) setLoaded({ state: "ready", status });
        },
        (error: unknown) => {
          if (!cancelled) setLoaded({ state: "failed", reason: reasonOf(error) });
        },
      );
    void check();
    const timer = setInterval(() => void check(), pollMs);
    return () => {
      cancelled = true;
      clearInterval(timer);
    };
  }, [client, pollMs]);

  const retry = async () => {
    setRetrying(true);
    setRetryError(null);
    try {
      setLoaded({ state: "ready", status: await client.retryRecovery() });
    } catch (error) {
      setRetryError(reasonOf(error));
    } finally {
      setRetrying(false);
    }
  };

  const time = (utcMillis: number) => formatTimestamp(utcMillis, timeZone);

  return (
    <section className="recovery-panel" aria-labelledby="recovery-title">
      <h2 id="recovery-title">崩潰恢復</h2>
      {loaded.state === "loading" && <p>載入中…</p>}
      {loaded.state === "failed" && <p role="alert">無法取得恢復狀態：{loaded.reason}</p>}
      {loaded.state === "ready" && loaded.status.event === null && <p>沒有恢復事件</p>}
      {loaded.state === "ready" && loaded.status.event !== null && (
        <>
          <p className={`recovery-status recovery-status-${loaded.status.event.status}`}>
            <strong>{recoveryStatusLabel(loaded.status.event)}</strong>
            <span>第 {loaded.status.event.round} 輪</span>
            <span>
              本輪嘗試：{loaded.status.event.attempts_used}/{loaded.status.max_attempts}
            </span>
          </p>
          <p>偵測到崩潰：{time(loaded.status.event.detected_at)}</p>
          {loaded.status.event.next_attempt_at !== null && (
            <p>下一次嘗試最早：{time(loaded.status.event.next_attempt_at)}</p>
          )}
          {loaded.status.event.observe_until !== null && <p>觀察期滿：{time(loaded.status.event.observe_until)}</p>}
          {loaded.status.event.closed_at !== null && <p>恢復完成：{time(loaded.status.event.closed_at)}</p>}
          {loaded.status.event.last_failure !== null && <p>最後失敗原因：{loaded.status.event.last_failure}</p>}
          {loaded.status.blocking !== null && (
            <p className="recovery-blocking" role="alert">
              新派工已暫停：{loaded.status.blocking.message}
            </p>
          )}
          {loaded.status.can_retry && (
            <div className="recovery-actions">
              <button type="button" disabled={retrying} onClick={() => void retry()}>
                重新嘗試恢復
              </button>
            </div>
          )}
          {retryError !== null && <p role="alert">無法重新嘗試恢復：{retryError}</p>}
          {loaded.status.event.attempts.length > 0 && (
            <ul className="recovery-attempts" aria-label="恢復嘗試">
              {loaded.status.event.attempts.map((attempt) => (
                <Attempt key={attempt.id} attempt={attempt} time={time} />
              ))}
            </ul>
          )}
        </>
      )}
    </section>
  );
}

function Attempt({ attempt, time }: { attempt: RecoveryAttempt; time: (utcMillis: number) => string }) {
  const parts = [
    `第 ${attempt.round} 輪第 ${attempt.seq} 次`,
    OUTCOME_LABELS[attempt.outcome],
    `開始 ${time(attempt.started_at)}`,
  ];
  if (attempt.finished_at !== null) parts.push(`結束 ${time(attempt.finished_at)}`);
  return (
    <li className={`recovery-attempt recovery-attempt-${attempt.outcome}`}>
      <span>{parts.join("・")}</span>
      {attempt.failure !== null && <span className="recovery-failure">失敗原因：{attempt.failure}</span>}
      {attempt.affected.length > 0 && (
        <ul className="recovery-affected" aria-label={`第 ${attempt.round} 輪第 ${attempt.seq} 次的受影響工作`}>
          {attempt.affected.map((run) => (
            <li key={run.run_id}>{describeAffected(run)}</li>
          ))}
        </ul>
      )}
    </li>
  );
}

/** 受影響工作的一行說明；沒有補充訊息時省略括號。 */
function describeAffected(run: AffectedRun): string {
  const text = `Run ${run.run_id}・Task ${run.task_id}・${DISPOSITION_LABELS[run.disposition]}`;
  return run.message === null ? text : `${text}（${run.message}）`;
}
