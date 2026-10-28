import { useCallback, useEffect, useState } from "react";

import type { SchedulingClient, SchedulingMode, SchedulingOverview } from "./runtimeClient";

type OverviewState =
  | { state: "loading" }
  | { state: "ready"; overview: SchedulingOverview }
  | { state: "failed"; reason: string };

interface Props {
  client: SchedulingClient;
}

function reasonOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** 並行容量設定：在「保留交接容量」與「較高並行」之間切換，並顯示目前的使用量（spec D-14）。 */
export function SchedulingSettings({ client }: Props) {
  const [loaded, setLoaded] = useState<OverviewState>({ state: "loading" });
  const [switchError, setSwitchError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(async () => {
    try {
      setLoaded({ state: "ready", overview: await client.schedulingOverview() });
    } catch (error) {
      setLoaded({ state: "failed", reason: reasonOf(error) });
    }
  }, [client]);

  useEffect(() => {
    void load();
  }, [load]);

  const choose = async (mode: SchedulingMode) => {
    setBusy(true);
    setSwitchError(null);
    try {
      setLoaded({ state: "ready", overview: await client.setSchedulingMode(mode) });
    } catch (error) {
      // 保留目前的設定顯示，說明沒有切換成功
      setSwitchError(reasonOf(error));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="scheduling-settings" aria-labelledby="scheduling-title">
      <h2 id="scheduling-title">並行容量</h2>
      {loaded.state === "loading" && <p>載入中…</p>}
      {loaded.state === "failed" && <p role="alert">無法取得並行容量設定：{loaded.reason}</p>}
      {loaded.state === "ready" && (
        <>
          <p className="scheduling-usage">
            目前使用 {loaded.overview.capacity.running} / {loaded.overview.capacity.normal_limit}；保留交接槽{" "}
            {loaded.overview.capacity.handoff_reserve} 個
          </p>
          <div role="radiogroup" aria-label="並行容量預設" className="scheduling-modes">
            {loaded.overview.settings.modes.map((info) => (
              <label key={info.mode} className="scheduling-mode">
                <input
                  type="radio"
                  name="scheduling-mode"
                  value={info.mode}
                  checked={loaded.overview.settings.mode === info.mode}
                  disabled={busy}
                  onChange={() => void choose(info.mode)}
                />
                <strong>{info.label}</strong>
                <span className="scheduling-mode-description">{info.description}</span>
              </label>
            ))}
          </div>
        </>
      )}
      {switchError && <p role="alert">無法切換並行容量：{switchError}</p>}
    </section>
  );
}
