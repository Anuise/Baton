import { useCallback, useEffect, useRef, useState } from "react";

import { outcomeText, seconds } from "./ExitProgress";
import type { LifecycleClient, LifecycleStatus, Maintenance, UpdateClient } from "./runtimeClient";

type InstallState = { state: "idle" } | { state: "installing" } | { state: "failed"; reason: string };

interface Props {
  client: Pick<UpdateClient, "updateStatus" | "installUpdate"> & Pick<LifecycleClient, "lifecycleStatus" | "quitApp">;
  /** 開始時的維護（同意更新的回應，或畫面載入時已在進行的維護）。 */
  maintenance: Maintenance;
  /** 輪詢維護與停止進度的間隔（毫秒）。 */
  pollMs?: number;
}

function reasonOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/**
 * 更新維護的進度（spec D-19）：與完全退出相同的停止流程 → 更新前備份 → 準備套用。
 * 準備套用且沒有停止異常就請桌面殼安裝並重新開啟；有停止異常時列出，由使用者決定是否仍要安裝。
 */
export function UpdateProgress({ client, maintenance: initial, pollMs = 500 }: Props) {
  const [maintenance, setMaintenance] = useState(initial);
  // null：還沒讀到停止進度
  const [lifecycle, setLifecycle] = useState<LifecycleStatus | null>(null);
  // null：還沒讀到「看到準備套用之後」的停止結果；只有它能判斷有沒有停止異常
  const [finalLifecycle, setFinalLifecycle] = useState<LifecycleStatus | null>(null);
  const [pollError, setPollError] = useState<string | null>(null);
  const [install, setInstall] = useState<InstallState>({ state: "idle" });
  const autoInstalled = useRef(false);
  // 一開始就是準備套用時，之後讀到的停止結果都是最終的
  const readySeen = useRef(initial.phase === "ready");

  const phase = maintenance.phase;
  const hasFinal = finalLifecycle !== null;
  // 停止中要持續輪詢；準備套用時也要先讀到最終的停止結果才能判斷有沒有停止異常。失敗或開始安裝後不再輪詢
  const polling = install.state === "idle" && (phase === "stopping" || (phase === "ready" && !hasFinal));

  useEffect(() => {
    if (!polling) return;
    // 階段改變（或卸載）後，遲到的回應一律忽略
    let cancelled = false;
    const timer = setInterval(() => {
      void (async () => {
        try {
          // 先讀維護再讀停止進度：看到「準備套用」時，之後讀到的停止結果一定是最終的
          const status = await client.updateStatus();
          if (status.maintenance?.phase === "ready") readySeen.current = true;
          const final = readySeen.current;
          const next = await client.lifecycleStatus().catch(() => null);
          if (cancelled) return;
          if (next) setLifecycle(next);
          if (next && final) setFinalLifecycle(next);
          if (status.maintenance) setMaintenance(status.maintenance);
          setPollError(next ? null : "無法取得停止進度");
        } catch (error) {
          if (!cancelled) setPollError(reasonOf(error));
        }
      })();
    }, pollMs);
    return () => {
      cancelled = true;
      clearInterval(timer);
    };
  }, [client, polling, pollMs]);

  const installNow = useCallback(async () => {
    setInstall({ state: "installing" });
    try {
      // 成功時桌面殼會結束 App，不會再回到這裡更新畫面
      await client.installUpdate();
    } catch (error) {
      setInstall({ state: "failed", reason: reasonOf(error) });
    }
  }, [client]);

  const report = lifecycle?.exit ?? null;
  const anomalies = finalLifecycle?.exit?.runs.filter((run) => run.outcome === "anomaly") ?? [];
  const canAutoInstall = phase === "ready" && hasFinal && anomalies.length === 0;

  useEffect(() => {
    if (!canAutoInstall || autoInstalled.current) return;
    autoInstalled.current = true;
    void installNow();
  }, [canAutoInstall, installNow]);

  const workspaceName = (id: string) => lifecycle?.workspaces.find((w) => w.workspace_id === id)?.name || id;
  const stopped = lifecycle?.phase === "stopped";
  const stage =
    phase === "failed"
      ? "更新沒有套用"
      : phase === "ready"
        ? "安裝並重新開啟"
        : stopped
          ? "備份資料"
          : "停止執行中的工作";
  // 準備套用卻讀不到最終的停止結果：無法確認工作都已停止，交給使用者決定
  const unconfirmed = phase === "ready" && !hasFinal && pollError !== null;

  return (
    <div className="exit-overlay">
      <section className="exit-progress" role="dialog" aria-modal="true" aria-labelledby="update-progress-title">
        <h2 id="update-progress-title">更新 Baton</h2>
        <p>
          {maintenance.target_version === null
            ? `從 ${maintenance.from_version} 更新`
            : `從 ${maintenance.from_version} 更新到 ${maintenance.target_version}`}
        </p>
        <p className="exit-phase">
          <strong>{stage}</strong>
          {report && phase === "stopping" && !stopped && (
            <span>
              距期限剩餘 {seconds(report.deadline_at - Date.now())} 秒（上限 {seconds(report.deadline_at - report.requested_at)}{" "}
              秒）
            </span>
          )}
          {report?.completed_at != null && <span>停止耗時 {seconds(report.completed_at - report.requested_at)} 秒</span>}
        </p>
        {report && report.runs.length > 0 && (
          <ul className="exit-runs" aria-label="Run 停止結果">
            {report.runs.map((run) => (
              <li key={run.run_id} className={`exit-run exit-run-${run.outcome}`}>
                <span>
                  {workspaceName(run.workspace_id)} · {run.run_id}
                </span>
                <strong>{outcomeText(run)}</strong>
              </li>
            ))}
          </ul>
        )}
        {report && report.notes.length > 0 && (
          <ul className="exit-notes">
            {report.notes.map((note, index) => (
              <li key={index}>{note}</li>
            ))}
          </ul>
        )}
        {phase === "stopping" && stopped && <p>工作已停止，正在備份資料…</p>}
        {phase === "stopping" && pollError && <p role="alert">暫時無法取得更新進度：{pollError}</p>}
        {install.state === "installing" && <p>正在安裝更新並重新開啟…</p>}
        {phase === "ready" && install.state === "idle" && anomalies.length > 0 && (
          <div className="exit-anomalies" role="alert">
            <p>
              <strong>停止異常 {anomalies.length} 個</strong>：這些程序無法確認停止，仍可能在背景執行。
            </p>
            <ul>
              {anomalies.map((run) => (
                <li key={run.run_id}>
                  {workspaceName(run.workspace_id)} · {run.run_id}：{outcomeText(run)}
                </li>
              ))}
            </ul>
            <button type="button" onClick={() => void installNow()}>
              仍要安裝更新並重新開啟
            </button>
          </div>
        )}
        {unconfirmed && install.state === "idle" && (
          <div className="exit-anomalies" role="alert">
            <p>無法確認工作是否都已停止：{pollError}</p>
            <button type="button" onClick={() => void installNow()}>
              仍要安裝更新並重新開啟
            </button>
          </div>
        )}
        {install.state === "failed" && (
          <>
            <p role="alert">無法安裝更新：{install.reason}</p>
            <div className="exit-actions">
              <button type="button" onClick={() => void installNow()}>
                重試
              </button>
              <button type="button" onClick={() => void client.quitApp()}>
                關閉 Baton
              </button>
            </div>
          </>
        )}
        {phase === "failed" && (
          <>
            <p role="alert">更新沒有套用：{maintenance.message ?? "原因不明"}</p>
            <p>Baton 已完全退出；重新開啟後會依各 Workspace 的設定繼續工作。</p>
            <div className="exit-actions">
              <button type="button" onClick={() => void client.quitApp()}>
                關閉 Baton
              </button>
            </div>
          </>
        )}
      </section>
    </div>
  );
}
