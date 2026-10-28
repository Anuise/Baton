import { useCallback, useEffect, useRef, useState } from "react";

import { profileStateLabel } from "./AccountsPage";
import { truncateText } from "./textFormat";
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

/** 一個視窗的剩餘額度百分比（1 − 已用比例）；未知為 null。 */
function remainingPercent(account: AccountScheduling, window: QuotaWindow): number | null {
  const used = account.quota.windows.find((w) => w.window === window)?.effective?.used ?? null;
  // 推估可能超過 1，剩餘不顯示負數
  return used === null ? null : Math.round(Math.max(0, 1 - used) * 100);
}

/** 一個視窗的剩餘額度文字：「5h 剩 60%」；未知顯示「5h 未知」。 */
function remainingLabel(account: AccountScheduling, window: QuotaWindow, short: string): string {
  const percent = remainingPercent(account, window);
  return percent === null ? `${short} 未知` : `${short} 剩 ${percent}%`;
}

/** 指標卡上的百分比；未知顯示「未知」。 */
const percentText = (percent: number | null) => (percent === null ? "未知" : `${percent}%`);

/** 帳號名稱在選單選項中最多的字數：額度摘要另外接在後面，避免長名稱把選單撐寬（MASTER 6.1）。 */
const OPTION_NAME_MAX = 24;

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
  // 使用者手動選的帳號；沒選過時用預設（見下方 profileId）
  const [pickedProfileId, setPickedProfileId] = useState<string | null>(null);
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
  // 預設選最近一次 Run 的帳號（同帳號重開才會接續原生 Session）；不可用時選第一個可用的。
  // 由目前的 view 與 profiles 推導，兩者的到達順序不影響結果
  const profileId =
    pickedProfileId ??
    (available.find((profile) => profile.id === run?.profile_id) ?? available[0])?.id ??
    "";
  const selectable = available.some((profile) => profile.id === profileId);
  const ending = open && run !== null && run.end_requested_at !== null;
  const accountOf = (id: string) => overview?.accounts.find((account) => account.profile_id === id) ?? null;
  const optionLabel = (profile: AccountProfile) => {
    const name = truncateText(profile.name, OPTION_NAME_MAX);
    if (!profile.dispatchable) return `${name}（${profileStateLabel(profile)}）`;
    const account = accountOf(profile.id);
    return account === null ? name : `${name}（${quotaSummary(account)}）`;
  };
  const controllerAccount = run === null ? null : accountOf(run.profile_id);
  // 換帳號的目標：與目前帳號不同、可用的 Profile；預設第一個
  const switchTargets = available.filter((profile) => profile.id !== run?.profile_id);
  const switchTarget = switchTargets.some((profile) => profile.id === switchTo) ? switchTo : (switchTargets[0]?.id ?? "");

  return (
    <section className="controller-page" aria-label="Controller">
      {/* 頁面標題列：頁名 + 開著時的「結束」（Workspace 名稱由外殼的情境列顯示） */}
      <header className="controller-header">
        <h2>主控台</h2>
        {view && open && (
          <button type="button" disabled={busy || ending} onClick={() => void act(() => client.endControllerCli(workspace.id))}>
            {ending ? "結束中…" : "結束"}
          </button>
        )}
      </header>
      {view === null && loadError === null && <p>載入中…</p>}
      {loadError && <p role="alert">無法取得 Controller 狀態：{loadError}</p>}
      {actionError && <p role="alert">{actionError}</p>}
      {/* 開著與未開啟都要提示：開著時「換帳號重開」需要帳號清單 */}
      {view && profilesError && <p role="alert">無法取得帳號清單：{profilesError}</p>}
      {view && (
        <div className="controller-layout">
          <section className="controller-card" aria-label="Controller 狀態">
            <h3>狀態</h3>
            <dl className="controller-facts">
              <dt>狀態</dt>
              <dd>
                <span className={`controller-status controller-status-${run === null ? "idle" : run.status}`}>
                  {run === null ? "未開啟" : STATUS_LABELS[run.status]}
                </span>
              </dd>
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
            {run && !open && run.end_reason !== null && (
              <p className="controller-end-reason">
                {run.end_code !== null && LAUNCH_FAILURE_CODES.has(run.end_code)
                  ? "開啟失敗："
                  : run.status === "interrupted"
                    ? "中斷原因："
                    : "結束原因："}
                {run.end_reason}
              </p>
            )}
            {open &&
              (run?.rebound_at != null ? (
                <p role="alert">Runtime 重新啟動過，這個視窗的平台工具已失效。請按「結束」，再用同一個帳號重新開啟（會接續 Session）。</p>
              ) : (
                <p className="controller-hint">已在終端機開啟，請切到該視窗操作</p>
              ))}
            {open && controllerAccount && (
              // 額度以三個小指標呈現：數值在上、標籤在下
              <dl className="controller-quota" role="group" aria-label="Controller 帳號額度">
                <div className="controller-metric">
                  <dt>5h 剩餘</dt>
                  <dd className="tabular">{percentText(remainingPercent(controllerAccount, "five_hour"))}</dd>
                </div>
                <div className="controller-metric">
                  <dt>7d 剩餘</dt>
                  <dd className="tabular">{percentText(remainingPercent(controllerAccount, "seven_day"))}</dd>
                </div>
                <div className="controller-metric">
                  <dt>執行中 Worker</dt>
                  <dd className="tabular">{controllerAccount.running}</dd>
                </div>
              </dl>
            )}
            {open && run !== null && run.switch_to_profile_id !== null && (
              <p className="controller-switching">換帳號重開中：改用 {profileName(run.switch_to_profile_id)}</p>
            )}
            {open && controllerAccount?.quota_gate?.state === "draining" && (
              <p role="status" className="controller-quota-warning">
                Controller 帳號的額度已接近上限，建議換帳號重開
              </p>
            )}
            {overviewError && <p className="controller-quota-error">無法取得額度：{overviewError}</p>}
            <details className="controller-more">
              <summary>詳細資訊</summary>
              <dl className="controller-facts">
                <dt>操作手冊版本</dt>
                <dd>{view.playbook_version}</dd>
              </dl>
            </details>
          </section>

          {open && profiles !== null && (
            <section className="controller-card" aria-label="換帳號重開">
              <h3>換帳號重開</h3>
              <div className="controller-row">
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
                <button
                  type="button"
                  disabled={busy || ending || switchTarget === ""}
                  onClick={() => void act(() => client.switchControllerCli(workspace.id, switchTarget))}
                >
                  換帳號重開
                </button>
              </div>
              {switchTargets.length === 0 && <p className="controller-hint">沒有其他可用的帳號</p>}
            </section>
          )}

          {!open && (
            <section className="controller-card" aria-label="開啟 Controller">
              <h3>開啟 Controller</h3>
              {profiles === null && profilesError === null && <p>載入帳號中…</p>}
              <div className="controller-row">
                {profiles !== null && (
                  <label>
                    Controller 帳號
                    <select value={profileId} onChange={(e) => setPickedProfileId(e.target.value)}>
                      {available.length === 0 && <option value="">（沒有可用的帳號）</option>}
                      {profiles.map((profile) => (
                        <option key={profile.id} value={profile.id} disabled={!profile.dispatchable}>
                          {optionLabel(profile)}
                        </option>
                      ))}
                    </select>
                  </label>
                )}
                <button
                  type="button"
                  className="primary-action"
                  disabled={busy || !selectable}
                  onClick={() => void act(() => client.openControllerCli(workspace.id, profileId))}
                >
                  開啟
                </button>
              </div>
              {profiles !== null && available.length === 0 && (
                <p className="controller-hint">沒有可用的帳號，請到「帳號」新增或重新核對</p>
              )}
              {unavailable.length > 0 && (
                <details className="controller-unavailable-details">
                  <summary>不可用的帳號（{unavailable.length}）</summary>
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
                </details>
              )}
            </section>
          )}
        </div>
      )}
    </section>
  );
}
