import { useEffect, useRef, useState } from "react";

import type { LifecycleClient, MaintenanceOutcome, UpdateClient, UpdateStatusView } from "./runtimeClient";
import { UpdateProgress } from "./UpdateProgress";

/** 同意更新的流程：說明中、下載並驗證中、失敗（工作沒有停止）。`version` 是使用者按下時看到的版本。 */
type ApplyState =
  | { state: "closed" }
  | { state: "confirming"; version: string }
  | { state: "applying"; version: string }
  | { state: "failed"; version: string; reason: string };

interface Props {
  client: UpdateClient & Pick<LifecycleClient, "lifecycleStatus" | "quitApp">;
  /** 這次 Runtime 的啟動時間（UTC 毫秒）；用來判斷最近一次維護是不是剛在這次啟動時結束。未知時為 null。 */
  runtimeStartedAt: number | null;
  /** 重新讀取更新狀態的間隔（毫秒）。 */
  pollMs?: number;
}

function reasonOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** 重新開啟後的更新結果；不是更新（例如解除安裝後重新安裝）時為 null。 */
function outcomeText(outcome: MaintenanceOutcome): string | null {
  if (outcome.kind !== "update" || outcome.applied === null) return null;
  return outcome.applied
    ? `已從 ${outcome.from_version} 更新到 ${outcome.running_version}`
    : `更新沒有套用，仍是 ${outcome.running_version}`;
}

/**
 * 更新通知（spec D-19）：有新版本時只通知，使用者看過說明並確認後才同意更新；
 * 更新維護中改顯示更新進度；重新開啟後顯示這次更新的結果。
 */
export function UpdateNotice({ client, runtimeStartedAt, pollMs = 60_000 }: Props) {
  // null：尚未取得；之後讀取失敗時保留上一次的結果
  const [view, setView] = useState<UpdateStatusView | null>(null);
  const [apply, setApply] = useState<ApplyState>({ state: "closed" });
  // 已關閉的更新結果（以結束時間識別）
  const [dismissedAt, setDismissedAt] = useState<number | null>(null);
  const inMaintenance = view?.maintenance != null;

  useEffect(() => {
    // 維護中由更新進度自己輪詢；這裡停止，避免 Runtime 結束後還繼續查詢
    if (inMaintenance) return;
    let cancelled = false;
    const check = () =>
      client.updateStatus().then(
        (next) => {
          if (!cancelled) setView(next);
        },
        () => {},
      );
    void check();
    const timer = setInterval(() => void check(), pollMs);
    return () => {
      cancelled = true;
      clearInterval(timer);
    };
  }, [client, pollMs, inMaintenance]);

  const confirm = async (version: string) => {
    setApply({ state: "applying", version });
    try {
      setView(await client.applyUpdate(version));
      setApply({ state: "closed" });
    } catch (error) {
      setApply({ state: "failed", version, reason: reasonOf(error) });
    }
  };

  if (view?.maintenance) {
    return view.maintenance.kind === "update" ? <UpdateProgress client={client} maintenance={view.maintenance} /> : null;
  }

  const available = view?.status === "available" ? view.available : null;
  // 只顯示在這次 Runtime 啟動時才結束、還沒關閉的維護結果
  const outcome = view?.last_maintenance ?? null;
  const recent =
    outcome !== null &&
    runtimeStartedAt !== null &&
    outcome.finished_at >= runtimeStartedAt &&
    outcome.finished_at !== dismissedAt
      ? outcome
      : null;
  const outcomeMessage = recent && outcomeText(recent);

  return (
    <>
      {recent && outcomeMessage && (
        <div className="callout callout-success update-notice" role="status">
          <span className="update-notice-text">{outcomeMessage}</span>
          <button type="button" className="btn-sm" onClick={() => setDismissedAt(recent.finished_at)}>
            知道了
          </button>
        </div>
      )}
      {available && (
        <div className="callout callout-accent update-notice">
          <strong className="update-notice-text">Baton {available.version} 可以更新</strong>
          {available.notes && (
            <details className="update-notice-notes">
              <summary>更新說明</summary>
              <p className="pre-wrap">{available.notes}</p>
            </details>
          )}
          <button
            type="button"
            className="btn-sm"
            onClick={() => setApply({ state: "confirming", version: available.version })}
          >
            更新並重新開啟…
          </button>
        </div>
      )}
      {apply.state !== "closed" && (
        <UpdateConfirm
          apply={apply}
          onConfirm={() => void confirm(apply.version)}
          onCancel={() => setApply({ state: "closed" })}
        />
      )}
    </>
  );
}

/**
 * 更新確認對話框（原生 `<dialog>` 的 modal）：開啟時焦點移到「取消」（不會停止任何工作的選項）；
 * Esc 等同「取消」，下載並驗證中不能取消；關閉後焦點回到開啟它的按鈕。
 */
function UpdateConfirm({
  apply,
  onConfirm,
  onCancel,
}: {
  apply: Exclude<ApplyState, { state: "closed" }>;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  const cancelButton = useRef<HTMLButtonElement>(null);
  const applying = apply.state === "applying";

  useEffect(() => {
    const element = dialog.current;
    if (element === null) return;
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    // jsdom 沒有 showModal：退回以 open 屬性顯示（焦點與 Esc 由這裡自行處理，行為相同）
    if (typeof element.showModal === "function") element.showModal();
    else element.setAttribute("open", "");
    cancelButton.current?.focus();
    return () => {
      if (typeof element.close === "function" && element.open) element.close();
      if (previous?.isConnected) previous.focus();
    };
    // 只在開啟時執行一次
  }, []);

  const cancel = () => {
    if (!applying) onCancel();
  };

  return (
    <dialog
      ref={dialog}
      className="dialog-panel update-confirm"
      aria-labelledby="update-confirm-title"
      // 瀏覽器原生的 Esc（cancel 事件）也交給 React 狀態處理，不讓 dialog 自己關掉
      onCancel={(event) => {
        event.preventDefault();
        cancel();
      }}
      onKeyDown={(event) => {
        if (event.key !== "Escape") return;
        event.preventDefault();
        cancel();
      }}
    >
      <h2 id="update-confirm-title">更新到 Baton {apply.version}</h2>
      <ol className="update-steps">
        <li>先下載並驗證安裝程式；失敗不會停止任何工作。</li>
        <li>執行與「完全退出」相同的停止流程（最多 40 秒）。</li>
        <li>備份資料。</li>
        <li>安裝並重新開啟 Baton。</li>
      </ol>
      <p>重新開啟後依各 Workspace 的「完全退出後自動續跑」設定續跑，其餘等「繼續工作」。</p>
      <p className="field-hint">資料目錄不會被刪除。</p>
      {applying && <p className="dot dot-accent">下載並驗證中…</p>}
      {apply.state === "failed" && (
        <div className="callout callout-danger" role="alert">
          <p>更新沒有開始：{apply.reason}</p>
          <p>工作沒有停止，照常進行。</p>
        </div>
      )}
      <div className="dialog-actions">
        {/* Windows 慣例：主要動作在左、取消在右 */}
        <button type="button" className="btn-primary" disabled={applying} onClick={onConfirm}>
          更新並重新開啟
        </button>
        <button ref={cancelButton} type="button" disabled={applying} onClick={cancel}>
          取消
        </button>
      </div>
    </dialog>
  );
}
