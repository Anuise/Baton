import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";

import type {
  ExitReport,
  ExitRunOutcome,
  ExitRunResult,
  LifecycleClient,
  LifecyclePhase,
  LifecycleStatus,
  StopAnomaly,
} from "./runtimeClient";

type ExitState =
  | { state: "idle" }
  | { state: "requesting" }
  | { state: "request_failed"; reason: string }
  | { state: "progress"; status: LifecycleStatus }
  // 輪詢時連續連不上 Runtime：視為已結束
  | { state: "runtime_gone" };

interface Props {
  client: LifecycleClient;
  /** 輪詢生命週期的間隔（毫秒）。 */
  pollMs?: number;
  /** 沒有停止異常時，停在結果畫面多久才結束 App（毫秒），讓使用者看到結果。 */
  quitDelayMs?: number;
}

/** 連續這麼多次讀不到生命週期才視為 Runtime 已結束；暫時性錯誤不能讓使用者看不到之後的停止異常。 */
const GONE_AFTER_FAILURES = 3;

const PHASE_LABELS: Record<LifecyclePhase, string> = {
  running: "運作中",
  stopping: "停止中",
  stopped: "已停止",
};

const OUTCOME_LABELS: Record<ExitRunOutcome, string> = {
  pending: "等待中",
  not_started: "未啟動",
  stopped_gracefully: "已停止",
  force_killed: "已強制終止",
  anomaly: "停止異常",
};

const ANOMALY_LABELS: Record<StopAnomaly, string> = {
  still_alive: "程序仍在執行",
  stop_failed: "停止失敗",
  stop_unconfirmed: "無法確認已停止",
};

function reasonOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export const seconds = (ms: number) => Math.max(0, Math.ceil(ms / 1000));

/** 「停止異常：原因（說明）」；沒有原因或說明時省略。 */
export function outcomeText(run: ExitRunResult): string {
  const detail = [run.anomaly && ANOMALY_LABELS[run.anomaly], run.message].filter(Boolean).join("：");
  return detail === "" ? OUTCOME_LABELS[run.outcome] : `${OUTCOME_LABELS[run.outcome]}（${detail}）`;
}

/** Run 的標示：Workspace 名稱與 Run（識別碼或「Controller CLI」），兩者以間距分開（更新進度共用）。 */
export function RunLabel({ workspace, children }: { workspace: string; children: ReactNode }) {
  return (
    <span className="exit-run-label">
      <span title={workspace}>{workspace}</span> {children}
    </span>
  );
}

/**
 * 停止流程的階段列（更新進度共用）：階段徽章、距期限剩餘秒數與耗時，停止中另以長條顯示已經過的時間。
 * `deadline` 為 false 時不顯示剩餘秒數；`elapsedLabel` 是完成時耗時的前綴。
 */
export function StopPhase({
  label,
  tone,
  report,
  deadline,
  elapsedLabel,
}: {
  label: string;
  tone: "plain" | "warning" | "success" | "danger" | "accent";
  report: ExitReport | null;
  deadline: boolean;
  elapsedLabel: string;
}) {
  const total = report ? report.deadline_at - report.requested_at : 0;
  const elapsed = report ? Math.min(Math.max(Date.now() - report.requested_at, 0), total) : 0;
  return (
    <div className="exit-phase">
      <p className="exit-phase-line">
        <strong className={`badge badge-${tone}`}>{label}</strong>
        {report && deadline && (
          <span className="tabular">
            距期限剩餘 {seconds(report.deadline_at - Date.now())} 秒（上限 {seconds(total)} 秒）
          </span>
        )}
        {report?.completed_at != null && (
          <span className="tabular">
            {elapsedLabel} {seconds(report.completed_at - report.requested_at)} 秒
          </span>
        )}
      </p>
      {report && deadline && total > 0 && (
        <span className="meter" aria-hidden="true">
          <span style={{ width: `${(elapsed / total) * 100}%` }} />
        </span>
      )}
    </div>
  );
}

/** 每個 Run 的停止結果（更新進度共用）。 */
export function StopRuns({ report, label }: { report: ExitReport | null; label: (run: ExitRunResult) => ReactNode }) {
  if (!report) return null;
  return (
    <>
      {report.runs.length > 0 && (
        <ul className="exit-runs" aria-label="Run 停止結果">
          {report.runs.map((run) => (
            <li key={run.run_id} className={`exit-run exit-run-${run.outcome}`}>
              {label(run)}
              <span className="exit-run-outcome">{outcomeText(run)}</span>
            </li>
          ))}
        </ul>
      )}
      {report.notes.length > 0 && (
        <ul className="exit-notes">
          {report.notes.map((note, index) => (
            <li key={index}>{note}</li>
          ))}
        </ul>
      )}
    </>
  );
}

/**
 * 完全退出的進度（spec D-11、D-25）：收到系統匣的完全退出事件後要求 Runtime 停止並輪詢，
 * 顯示每個 Run 的停止結果；全部確認停止就結束 App，有停止異常則列出並由使用者按「關閉 Baton」。
 */
export function ExitProgress({ client, pollMs = 500, quitDelayMs = 1000 }: Props) {
  const [exit, setExit] = useState<ExitState>({ state: "idle" });
  // 退出進行中再收到事件時不重新開始
  const started = useRef(false);

  // 由畫面決定何時結束 App 時，取消殼的 60 秒後備強制關閉；取消失敗不影響畫面
  const cancelFallback = useCallback(() => void client.cancelFullExitFallback().catch(() => {}), [client]);

  const start = useCallback(async () => {
    started.current = true;
    setExit({ state: "requesting" });
    try {
      setExit({ state: "progress", status: await client.requestFullExit() });
      // Runtime 已接手停止；之後可能要停在停止異常畫面，不能讓殼 60 秒後強制關掉
      cancelFallback();
    } catch (error) {
      started.current = false;
      setExit({ state: "request_failed", reason: reasonOf(error) });
    }
  }, [client, cancelFallback]);

  useEffect(
    () =>
      client.onFullExitRequested(() => {
        if (!started.current) void start();
      }),
    [client, start],
  );

  const phase = exit.state === "progress" ? exit.status.phase : null;

  useEffect(() => {
    if (phase === null || phase === "stopped") return;
    // 階段改變（或卸載）後，遲到的回應一律忽略，避免蓋掉已停止的結果
    let cancelled = false;
    // 連續失敗次數；讀到一次就歸零
    let failures = 0;
    const timer = setInterval(() => {
      client.lifecycleStatus().then(
        (status) => {
          if (cancelled) return;
          failures = 0;
          setExit({ state: "progress", status });
        },
        () => {
          if (cancelled) return;
          failures += 1;
          if (failures >= GONE_AFTER_FAILURES) setExit({ state: "runtime_gone" });
        },
      );
    }, pollMs);
    return () => {
      cancelled = true;
      clearInterval(timer);
    };
  }, [client, phase, pollMs]);

  const report = exit.state === "progress" ? exit.status.exit : null;
  const anomalies = report?.runs.filter((run) => run.outcome === "anomaly") ?? [];
  const finishedCleanly = exit.state === "runtime_gone" || (phase === "stopped" && anomalies.length === 0);

  useEffect(() => {
    if (!finishedCleanly) return;
    const timer = setTimeout(() => void client.quitApp(), quitDelayMs);
    return () => clearTimeout(timer);
  }, [client, finishedCleanly, quitDelayMs]);

  if (exit.state === "idle") return null;

  const workspaceName = (id: string) =>
    (exit.state === "progress" && exit.status.workspaces.find((w) => w.workspace_id === id)?.name) || id;
  // Controller CLI 沒有 Task，以「Controller CLI」取代 Run ID
  const runLabel = (run: ExitRunResult) => (
    <RunLabel workspace={workspaceName(run.workspace_id)}>
      {run.controller_cli ? "Controller CLI" : <code className="mono">{run.run_id}</code>}
    </RunLabel>
  );
  const phaseTone =
    phase === "stopped" ? (anomalies.length > 0 ? "danger" : "success") : phase === "stopping" ? "warning" : "plain";

  return (
    <div className="overlay">
      <section className="dialog-panel exit-progress" role="dialog" aria-modal="true" aria-labelledby="exit-title">
        <h2 id="exit-title">完全退出</h2>
        {exit.state === "requesting" && <p className="dot dot-accent">正在要求 Runtime 停止…</p>}
        {exit.state === "request_failed" && (
          <>
            <p role="alert" className="callout callout-danger">
              無法開始完全退出：{exit.reason}
            </p>
            <div className="dialog-actions">
              <button type="button" className="btn-primary" onClick={() => void start()}>
                重試
              </button>
              <button
                type="button"
                onClick={() => {
                  setExit({ state: "idle" });
                  cancelFallback();
                }}
              >
                返回
              </button>
            </div>
          </>
        )}
        {exit.state === "runtime_gone" && <p className="dot dot-success">Runtime 已結束，正在關閉 Baton…</p>}
        {exit.state === "progress" && (
          <>
            <StopPhase
              label={PHASE_LABELS[exit.status.phase]}
              tone={phaseTone}
              report={report}
              deadline={phase !== "stopped"}
              elapsedLabel="耗時"
            />
            <StopRuns report={report} label={runLabel} />
            {phase === "stopped" && anomalies.length === 0 && (
              <p className="dot dot-success">所有程序都已停止，正在關閉 Baton…</p>
            )}
            {phase === "stopped" && anomalies.length > 0 && (
              <div className="exit-anomalies" role="alert">
                <p>
                  <strong>停止異常 {anomalies.length} 個</strong>：這些程序無法確認停止，仍可能在背景執行。
                </p>
                <ul>
                  {anomalies.map((run) => (
                    <li key={run.run_id}>
                      {runLabel(run)}：{outcomeText(run)}
                    </li>
                  ))}
                </ul>
                <div className="dialog-actions">
                  <button type="button" className="btn-primary" onClick={() => void client.quitApp()}>
                    關閉 Baton
                  </button>
                </div>
              </div>
            )}
          </>
        )}
      </section>
    </div>
  );
}
