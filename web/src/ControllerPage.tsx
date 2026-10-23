import { useCallback, useEffect, useRef, useState } from "react";

import { profileStateLabel } from "./AccountsPage";
import { formatTimestamp } from "./time";
import type {
  AccountClient,
  AccountProfile,
  AccountScheduling,
  ControllerCliClient,
  ControllerCliRun,
  ControllerCliStatus,
  ControllerCliView,
  QuotaWindow,
  SchedulingClient,
  SchedulingOverview,
  Workspace,
} from "./runtimeClient";

/** Controller CLI 開著（starting／running）時重新讀取狀態的間隔：開窗與結束由 Runtime 推進。 */
export const CONTROLLER_REFRESH_INTERVAL_MS = 2000;

export type ControllerPageClient = ControllerCliClient &
  Pick<AccountClient, "listProfiles"> &
  Pick<SchedulingClient, "schedulingOverview">;

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

/** 一個視窗的剩餘額度：1 − 已用比例，以百分比顯示；未知（null）顯示「未知」。 */
function remainingLabel(account: AccountScheduling, window: QuotaWindow, short: string): string {
  const used = account.quota.windows.find((w) => w.window === window)?.effective?.used ?? null;
  // 推估可能超過 1，剩餘不顯示負數
  return used === null ? `${short} 未知` : `${short} 剩 ${Math.round(Math.max(0, 1 - used) * 100)}%`;
}

/** 帳號的 5h／7d 剩餘額度與執行中 Worker 數，例如「5h 剩 60%｜7d 剩 85%｜執行中 Worker 1」。 */
function quotaSummary(account: AccountScheduling): string {
  return `${remainingLabel(account, "five_hour", "5h")}｜${remainingLabel(account, "seven_day", "7d")}｜執行中 Worker ${account.running}`;
}

/** Session 說明：接續同帳號的原生 Session、換帳號重開開的新 Session，或一般的新 Session。 */
function sessionLabel(run: ControllerCliRun): string {
  if (run.continue_session) return "接續上次的 Session";
  return run.switched_from_run_id !== null ? "由換帳號重開開啟的新 Session" : "新的 Session";
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
  const [overview, setOverview] = useState<SchedulingOverview | null>(null);
  const [overviewError, setOverviewError] = useState<string | null>(null);
  const [switchTo, setSwitchTo] = useState("");
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

  // 額度與執行中 Worker 數取自排程總覽；讀取失敗不擋住頁面，只提示無法取得額度
  const loadOverview = useCallback(async () => {
    try {
      setOverview(await client.schedulingOverview());
      setOverviewError(null);
    } catch (e) {
      setOverviewError(reasonOf(e));
    }
  }, [client]);

  useEffect(() => {
    void load();
  }, [load]);

  useEffect(() => {
    void loadOverview();
  }, [loadOverview]);

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

  // 開著時定期重新讀取，看到 running 或 ended（含開啟失敗）；額度與執行中 Worker 數一起更新。
  // 換帳號重開後 view 回傳的是最新的 Run（新 Run 為 starting），所以會繼續輪詢到新 Run 開好。
  useEffect(() => {
    if (!open) return;
    const timer = setInterval(() => {
      void load();
      void loadOverview();
    }, pollMs);
    return () => clearInterval(timer);
  }, [open, load, loadOverview, pollMs]);

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
  const accountOf = (id: string) => overview?.accounts.find((account) => account.profile_id === id) ?? null;
  const optionLabel = (profile: AccountProfile) => {
    if (!profile.dispatchable) return `${profile.name}（${profileStateLabel(profile)}）`;
    const account = accountOf(profile.id);
    return account === null ? profile.name : `${profile.name}（${quotaSummary(account)}）`;
  };
  const controllerAccount = run === null ? null : accountOf(run.profile_id);
  // 換帳號的目標：與目前帳號不同、可用的 Profile；預設第一個
  const switchTargets = available.filter((profile) => profile.id !== run?.profile_id);
  const switchTarget = switchTargets.some((profile) => profile.id === switchTo) ? switchTo : (switchTargets[0]?.id ?? "");

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
              <dt>Session</dt>
              <dd>{sessionLabel(run)}</dd>
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
          {controllerAccount && (
            <div className="controller-quota" role="group" aria-label="Controller 帳號額度">
              {quotaSummary(controllerAccount)}
            </div>
          )}
          {controllerAccount?.quota_gate?.state === "draining" && (
            <p role="status" className="controller-quota-warning">
              Controller 帳號的額度已接近上限，建議換帳號重開
            </p>
          )}
          <button type="button" disabled={busy || ending} onClick={() => void act(() => client.endControllerCli(workspace.id))}>
            {ending ? "結束中…" : "結束"}
          </button>
          {run !== null && run.switch_to_profile_id !== null && (
            <p className="controller-switching">換帳號重開中：改用 {profileName(run.switch_to_profile_id)}</p>
          )}
          {profiles !== null && (
            <div className="controller-switch">
              <label>
                換成帳號
                <select value={switchTarget} disabled={ending} onChange={(e) => setSwitchTo(e.target.value)}>
                  {switchTargets.map((profile) => (
                    <option key={profile.id} value={profile.id}>
                      {optionLabel(profile)}
                    </option>
                  ))}
                </select>
              </label>
              {switchTargets.length === 0 && <p className="controller-hint">沒有其他可用的帳號可換</p>}
              <button
                type="button"
                disabled={busy || ending || switchTarget === ""}
                onClick={() => void act(() => client.switchControllerCli(workspace.id, switchTarget))}
              >
                換帳號重開
              </button>
            </div>
          )}
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
                    {optionLabel(profile)}
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
      {overviewError && <p className="controller-quota-error">無法取得額度：{overviewError}</p>}
      {actionError && <p role="alert">{actionError}</p>}
    </section>
  );
}
