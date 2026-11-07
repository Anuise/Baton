import { useEffect, useState } from "react";

import type {
  AttemptOutcome,
  RecoveryAttempt,
  RecoveryClient,
  RecoveryEvent,
  RecoveryStatusView,
  RunDisposition,
} from "./runtimeClient";
import { LongText } from "./LongText";
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

/** 恢復事件狀態徽章的語意色。 */
const RECOVERY_BADGE: Record<RecoveryEvent["status"], string> = {
  waiting: "badge-warning",
  attempting: "badge-warning",
  observing: "badge-accent",
  succeeded: "badge-success",
  exhausted: "badge-danger",
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
  const event = loaded.state === "ready" ? loaded.status.event : null;

  return (
    <section className="section recovery-panel" aria-labelledby="recovery-title">
      <div className="section-heading">
        <h3 id="recovery-title">崩潰恢復</h3>
        {loaded.state === "ready" && event !== null && (
          <span className={`recovery-status recovery-status-${event.status}`}>
            <strong className={`badge ${RECOVERY_BADGE[event.status]}`}>{recoveryStatusLabel(event)}</strong>
            <span>第 {event.round} 輪</span>
            <span className="tabular">
              本輪嘗試：{event.attempts_used}/{loaded.status.max_attempts}
            </span>
          </span>
        )}
        {loaded.state === "ready" && event !== null && loaded.status.can_retry && (
          <div className="section-actions">
            <button type="button" disabled={retrying} onClick={() => void retry()}>
              重新嘗試恢復
            </button>
          </div>
        )}
        {/* 與並排的平台狀態一樣有一行說明，兩個面板上緣對齊 */}
        <p>Runtime 意外結束時自動重新啟動並恢復工作；恢復額度用盡時可以重新嘗試。</p>
      </div>
      {loaded.state === "loading" && <p className="empty-note">載入中…</p>}
      {/* 原因可能很長：長警示（只有色條與狀態詞上色），不整塊上色 */}
      {loaded.state === "failed" && (
        <div role="alert" className="notice notice-danger">
          <p className="notice-title">無法取得恢復狀態</p>
          <p>{loaded.reason}</p>
        </div>
      )}
      {loaded.state === "ready" && event === null && <p className="empty-note">沒有恢復事件</p>}
      {loaded.state === "ready" && event !== null && (
        <>
          {loaded.status.blocking !== null && (
            <div className="notice notice-danger recovery-blocking" role="alert">
              <p className="notice-title">新派工已暫停</p>
              <p>{loaded.status.blocking.message}</p>
            </div>
          )}
          {retryError !== null && (
            <div role="alert" className="notice notice-danger">
              <p className="notice-title">無法重新嘗試恢復</p>
              <p>{retryError}</p>
            </div>
          )}
          <div className="panel">
            <dl className="facts">
              <dt>偵測到崩潰</dt>
              <dd className="tabular">{time(event.detected_at)}</dd>
              {event.next_attempt_at !== null && (
                <>
                  <dt>下一次嘗試最早</dt>
                  <dd className="tabular">{time(event.next_attempt_at)}</dd>
                </>
              )}
              {event.observe_until !== null && (
                <>
                  <dt>觀察期滿</dt>
                  <dd className="tabular">{time(event.observe_until)}</dd>
                </>
              )}
              {event.closed_at !== null && (
                <>
                  <dt>恢復完成</dt>
                  <dd className="tabular">{time(event.closed_at)}</dd>
                </>
              )}
              {event.last_failure !== null && (
                <>
                  <dt>最後失敗原因</dt>
                  {/* 原因可能很長：限 2 行、可展開，不在窄欄裡撐成一大段 */}
                  <dd>
                    <LongText text={event.last_failure} lines={2} />
                  </dd>
                </>
              )}
            </dl>
          </div>
          {event.attempts.length > 0 && (
            <ul className="row-list recovery-attempts" aria-label="恢復嘗試">
              {event.attempts.map((attempt) => (
                <Attempt key={attempt.id} attempt={attempt} time={time} />
              ))}
            </ul>
          )}
        </>
      )}
    </section>
  );
}

/** 嘗試結果徽章的語意色。 */
const OUTCOME_BADGES: Record<AttemptOutcome, string> = {
  in_progress: "badge-warning",
  observing: "badge-accent",
  succeeded: "badge-success",
  failed: "badge-danger",
};

function Attempt({ attempt, time }: { attempt: RecoveryAttempt; time: (utcMillis: number) => string }) {
  return (
    <li className={`recovery-attempt recovery-attempt-${attempt.outcome}`}>
      <div className="row-head">
        <span>
          第 {attempt.round} 輪第 {attempt.seq} 次
        </span>
        <span className={`badge ${OUTCOME_BADGES[attempt.outcome]}`}>{OUTCOME_LABELS[attempt.outcome]}</span>
      </div>
      <p className="meta">
        <span className="tabular">開始 {time(attempt.started_at)}</span>
        {attempt.finished_at !== null && <span className="tabular">結束 {time(attempt.finished_at)}</span>}
      </p>
      {attempt.failure !== null && <p className="recovery-failure">失敗原因：{attempt.failure}</p>}
      {attempt.affected.length > 0 && (
        <ul className="recovery-affected" aria-label={`第 ${attempt.round} 輪第 ${attempt.seq} 次的受影響工作`}>
          {attempt.affected.map((run) => (
            <li key={run.run_id} className="meta">
              <span>
                Run <code className="mono">{run.run_id}</code>
              </span>
              <span>
                Task <code className="mono">{run.task_id}</code>
              </span>
              <span className="recovery-disposition">
                {DISPOSITION_LABELS[run.disposition]}
                {run.message !== null && `（${run.message}）`}
              </span>
            </li>
          ))}
        </ul>
      )}
    </li>
  );
}
