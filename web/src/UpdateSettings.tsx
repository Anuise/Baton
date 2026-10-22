import { useEffect, useState } from "react";

import type { UpdateClient, UpdateStatusView } from "./runtimeClient";
import { formatTimestamp } from "./time";

type StatusState = { state: "loading" } | { state: "ready"; view: UpdateStatusView } | { state: "failed"; reason: string };

interface Props {
  client: Pick<UpdateClient, "updateStatus">;
  /** 只供測試指定時區；平常使用系統時區（spec D-33）。 */
  timeZone?: string;
}

function reasonOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** 依檢查結果說明目前狀態。 */
function statusText(view: UpdateStatusView): string {
  switch (view.status) {
    case "unconfigured":
      return "未設定更新來源（這個版本不會自動檢查）";
    case "up_to_date":
      return "已是最新版本";
    case "available":
      return `有新版本 ${view.available?.version ?? ""}`;
    case "failed":
      return `檢查失敗：${view.error ?? "原因不明"}（明天再試）`;
    case "not_checked":
      return "尚未檢查";
  }
}

/** 更新（spec D-19）：唯讀顯示目前版本與 Runtime 的檢查結果；不提供手動檢查。 */
export function UpdateSettings({ client, timeZone }: Props) {
  const [loaded, setLoaded] = useState<StatusState>({ state: "loading" });

  useEffect(() => {
    void (async () => {
      try {
        setLoaded({ state: "ready", view: await client.updateStatus() });
      } catch (error) {
        setLoaded({ state: "failed", reason: reasonOf(error) });
      }
    })();
  }, [client]);

  return (
    <section className="update-settings" aria-labelledby="update-settings-title">
      <h2 id="update-settings-title">更新</h2>
      <p className="update-policy">啟動時與每天自動檢查；有新版本只會通知，不會自動更新。</p>
      {loaded.state === "loading" && <p>載入中…</p>}
      {loaded.state === "failed" && <p role="alert">無法取得更新狀態：{loaded.reason}</p>}
      {loaded.state === "ready" && (
        <>
          <p>目前版本：{loaded.view.current_version}</p>
          <p>狀態：{statusText(loaded.view)}</p>
          <p>
            上次檢查：
            {loaded.view.checked_at === null ? "尚未檢查" : formatTimestamp(loaded.view.checked_at, timeZone)}
          </p>
          <p>
            下次檢查：
            {loaded.view.next_check_at === null ? "未排定" : formatTimestamp(loaded.view.next_check_at, timeZone)}
          </p>
        </>
      )}
    </section>
  );
}
