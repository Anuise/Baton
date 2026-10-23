import { useCallback, useEffect, useRef, useState } from "react";

import { profileStateLabel } from "./AccountsPage";
import { formatTimestamp } from "./time";
import type {
  AccountClient,
  AccountProfile,
  ControllerCliClient,
  ControllerCliRun,
  ControllerCliStatus,
  ControllerCliView,
  Workspace,
} from "./runtimeClient";

/** Controller CLI 開著（starting／running）時重新讀取狀態的間隔：開窗與結束由 Runtime 推進。 */
export const CONTROLLER_REFRESH_INTERVAL_MS = 2000;

export type ControllerPageClient = ControllerCliClient & Pick<AccountClient, "listProfiles">;

interface Props {
  client: ControllerPageClient;
  workspace: Workspace;
  /** 顯示用的時區；省略時使用系統時區（spec D-33），只有測試會指定。 */
  timeZone?: string;
  /** 重新讀取的間隔（毫秒）；只有測試會指定。 */
  pollMs?: number;
}

const STATUS_LABELS: Record<ControllerCliStatus, string> = {
  starting: "開啟中",
  running: "執行中",
  ended: "已結束",
  interrupted: "中斷",
};

/** 開啟失敗的結束代碼：Controller CLI 沒有成功開起來，原因以「開啟失敗」標示。 */
const LAUNCH_FAILURE_CODES = new Set([
  "claude_not_found",
  "profile_unavailable",
  "terminal_failed",
  "launch_failed",
  "launch_outcome_unknown",
]);

function reasonOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function isOpen(run: ControllerCliRun | null): boolean {
  return run !== null && (run.status === "starting" || run.status === "running");
}

/**
 * Workspace 的「Controller」頁簽（desktop-controller-cli 票 03）：選一個 Account Profile 當 Controller 帳號，
 * 由平台在外部終端機開啟互動式 Claude Code（Controller CLI），並可從 App 結束它。
 */
export function ControllerPage({ client, workspace, timeZone, pollMs = CONTROLLER_REFRESH_INTERVAL_MS }: Props) {
  const [view, setView] = useState<ControllerCliView | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [profiles, setProfiles] = useState<AccountProfile[] | null>(null);
  const [profilesError, setProfilesError] = useState<string | null>(null);
  const [profileId, setProfileId] = useState("");
  const [busy, setBusy] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  // 只採用最後發出的讀取結果，避免較早的輪詢回應蓋掉剛開啟／結束的狀態
  const latest = useRef(0);

  const load = useCallback(async () => {
    const request = ++latest.current;
    try {
      const loaded = await client.getControllerCli(workspace.id);
      if (request !== latest.current) return;
      setView(loaded);
      setLoadError(null);
    } catch (e) {
      if (request === latest.current) setLoadError(reasonOf(e));
    }
  }, [client, workspace.id]);

  useEffect(() => {
    void load();
  }, [load]);

  useEffect(() => {
    let current = true;
    client.listProfiles().then(
      (list) => {
        if (!current) return;
        const active = list.profiles.filter((profile) => profile.status === "active");
        setProfiles(active);
        // 預設選第一個可用的 Profile
        setProfileId(active.find((profile) => profile.dispatchable)?.id ?? "");
      },
      (e: unknown) => {
        if (current) setProfilesError(reasonOf(e));
      },
    );
    return () => {
      current = false;
    };
  }, [client]);

  const run = view?.run ?? null;
  const open = isOpen(run);

  // 開著時定期重新讀取，看到 running 或 ended（含開啟失敗）
  useEffect(() => {
    if (!open) return;
    const timer = setInterval(() => void load(), pollMs);
    return () => clearInterval(timer);
  }, [open, load, pollMs]);

  const act = async (action: () => Promise<ControllerCliView>) => {
    setBusy(true);
    setActionError(null);
    try {
      const result = await action();
      // 進行中的輪詢結果作廢，先顯示這次操作的結果
      latest.current += 1;
      setView(result);
    } catch (e) {
      setActionError(reasonOf(e));
      // 被拒時（例如另一個視窗已開啟）重新讀取目前狀態
      await load();
    } finally {
      setBusy(false);
    }
  };

  const available = profiles?.filter((profile) => profile.dispatchable) ?? [];
  const unavailable = profiles?.filter((profile) => !profile.dispatchable) ?? [];
  const profileName = (id: string) => profiles?.find((profile) => profile.id === id)?.name ?? id;
  const selectable = available.some((profile) => profile.id === profileId);
  const ending = open && run !== null && run.end_requested_at !== null;

  return (
    <section className="controller-page" aria-label="Controller">
      <h2>Controller：{workspace.name}</h2>
      {view === null && loadError === null && <p>載入中…</p>}
      {loadError && <p role="alert">無法取得 Controller 狀態：{loadError}</p>}
      {view && (
        <dl className="controller-facts">
          <dt>狀態</dt>
          <dd>{run === null ? "未開啟" : STATUS_LABELS[run.status]}</dd>
          {run && (
            <>
              <dt>Controller 帳號</dt>
              <dd>{profileName(run.profile_id)}</dd>
              <dt>開啟時間</dt>
              <dd>{formatTimestamp(run.opened_at, timeZone)}</dd>
            </>
          )}
        </dl>
      )}
      {run && !open && run.end_reason !== null && (
        <p className="controller-end-reason">
          {run.end_code !== null && LAUNCH_FAILURE_CODES.has(run.end_code) ? "開啟失敗：" : "結束原因："}
          {run.end_reason}
        </p>
      )}
      {view && open && (
        <div className="controller-actions">
          <p className="controller-hint">Controller CLI 已開啟，請切到它的終端機視窗</p>
          <button type="button" disabled={busy || ending} onClick={() => void act(() => client.endControllerCli(workspace.id))}>
            {ending ? "結束中…" : "結束"}
          </button>
        </div>
      )}
      {view && !open && (
        <div className="controller-actions">
          {profilesError && <p role="alert">無法取得帳號清單：{profilesError}</p>}
          {profiles === null && profilesError === null && <p>載入帳號中…</p>}
          {profiles !== null && (
            <label>
              Controller 帳號
              <select value={profileId} onChange={(e) => setProfileId(e.target.value)}>
                {available.length === 0 && <option value="">（沒有可用的帳號）</option>}
                {profiles.map((profile) => (
                  <option key={profile.id} value={profile.id} disabled={!profile.dispatchable}>
                    {profile.dispatchable ? profile.name : `${profile.name}（${profileStateLabel(profile)}）`}
                  </option>
                ))}
              </select>
            </label>
          )}
          {profiles !== null && available.length === 0 && (
            <p className="controller-hint">沒有可用的帳號，請到「帳號」頁簽新增帳號或重新核對</p>
          )}
          {unavailable.length > 0 && (
            <ul className="controller-unavailable" aria-label="不可用的帳號">
              {unavailable.map((profile) => (
                <li key={profile.id}>
                  {profile.name}：
                  {profile.blockers.length > 0
                    ? profile.blockers.map((blocker) => blocker.message).join("；")
                    : profileStateLabel(profile)}
                </li>
              ))}
            </ul>
          )}
          <button
            type="button"
            disabled={busy || !selectable}
            onClick={() => void act(() => client.openControllerCli(workspace.id, profileId))}
          >
            開啟
          </button>
        </div>
      )}
      {actionError && <p role="alert">{actionError}</p>}
    </section>
  );
}
