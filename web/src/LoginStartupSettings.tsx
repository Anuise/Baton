import { useCallback, useEffect, useState } from "react";

import type { LifecycleClient, LoginStartupView } from "./runtimeClient";

type StartupState = { state: "loading" } | { state: "ready"; view: LoginStartupView } | { state: "failed"; reason: string };

interface Props {
  client: Pick<LifecycleClient, "lifecycleStatus" | "updateLoginStartup">;
}

function reasonOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** 「登入 Windows 時自動啟動 Baton」（票 33）：預設關閉，與各 Workspace 的自動續跑設定分開。 */
export function LoginStartupSettings({ client }: Props) {
  const [loaded, setLoaded] = useState<StartupState>({ state: "loading" });
  const [switchError, setSwitchError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(async () => {
    try {
      setLoaded({ state: "ready", view: (await client.lifecycleStatus()).login_startup });
    } catch (error) {
      setLoaded({ state: "failed", reason: reasonOf(error) });
    }
  }, [client]);

  useEffect(() => {
    void load();
  }, [load]);

  const toggle = async (enabled: boolean) => {
    setBusy(true);
    setSwitchError(null);
    try {
      setLoaded({ state: "ready", view: await client.updateLoginStartup(enabled) });
    } catch (error) {
      // Runtime 沒有變更偏好；開關維持原值，只說明失敗原因
      setSwitchError(reasonOf(error));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="login-startup-settings" aria-labelledby="login-startup-title">
      <h2 id="login-startup-title">登入時自動啟動</h2>
      {loaded.state === "loading" && <p>載入中…</p>}
      {loaded.state === "failed" && <p role="alert">無法取得登入時自動啟動設定：{loaded.reason}</p>}
      {loaded.state === "ready" && (
        <>
          <label className="login-startup-toggle">
            <input
              type="checkbox"
              checked={loaded.view.enabled}
              disabled={busy}
              onChange={(e) => void toggle(e.target.checked)}
            />
            登入 Windows 時自動啟動 Baton
          </label>
          <p className="login-startup-hint">只影響之後的 Windows 登入；完全退出後，登入時不會自動啟動，需手動開 App</p>
          {loaded.view.enabled && loaded.view.registered === false && (
            <p className="login-startup-warning">Windows 的登記遺失，下次啟動 Runtime 時會補回</p>
          )}
        </>
      )}
      {switchError && <p role="alert">無法變更登入時自動啟動：{switchError}</p>}
    </section>
  );
}
