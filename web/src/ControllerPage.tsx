import { useCallback, useEffect, useId, useRef, useState } from "react";

import { profileStateLabel } from "./AccountsPage";
import { splitFirstSentence, truncateText } from "./textFormat";
import { formatTimestamp } from "./time";
import type {
  AccountClient,
  AccountProfile,
  AccountScheduling,
  AuthorizationClient,
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
  Pick<SchedulingClient, "schedulingOverview"> &
  Pick<AuthorizationClient, "getAuthorization">;

interface Props {
  client: ControllerPageClient;
  workspace: Workspace;
  /** 顯示用的時區；省略時使用系統時區（spec D-33），只有測試會指定。 */
  timeZone?: string;
  /** 重新讀取的間隔（毫秒）；只有測試會指定。 */
  pollMs?: number;
  /** 切到其他頁面（授權未確認時的「前往授權」、沒有可用帳號時的「前往帳號」）；沒有傳入時不顯示按鈕。 */
  onOpenPage?: (page: "authorization" | "accounts") => void;
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

/** 一個視窗的剩餘額度文字：「五小時剩 60%」；未知顯示「五小時未知」。 */
function remainingLabel(account: AccountScheduling, window: QuotaWindow, short: string): string {
  const percent = remainingPercent(account, window);
  return percent === null ? `${short}未知` : `${short}剩 ${percent}%`;
}

/** 額度量表上的百分比；未知顯示「未知」。 */
const percentText = (percent: number | null) => (percent === null ? "未知" : `${percent}%`);

/** 最近一次 Run 是開啟失敗（Controller CLI 沒有成功開起來）。 */
const launchFailed = (run: ControllerCliRun) => run.end_code !== null && LAUNCH_FAILURE_CODES.has(run.end_code);

type Tone = "success" | "warning" | "danger" | "accent";

/** 狀態點的語意：開啟中為進行中（群青）、執行中為成功、中斷為危險；未開啟與已結束為中性。 */
const STATUS_TONES: Record<ControllerCliStatus, Tone | null> = {
  starting: "accent",
  running: "success",
  ended: null,
  interrupted: "danger",
};

/** 卡片左側色條：與狀態點相同；開啟失敗雖然是「已結束」，也以危險標示。 */
function stripeTone(run: ControllerCliRun | null): Tone | null {
  if (run === null) return null;
  return run.status === "ended" && launchFailed(run) ? "danger" : STATUS_TONES[run.status];
}

/**
 * 帳號名稱在選單選項中最多的字數：額度摘要另外接在後面，避免長名稱把選單撐寬（MASTER 6.1）。
 * 1440 寬時「換成帳號」選單只有半欄寬（約 425px），17 字的名稱加上五小時／七日額度剛好放得下，收合時看得到名稱與主要額度。
 */
const OPTION_NAME_MAX = 17;

/** 帳號的五小時／七日剩餘額度與執行中 Worker 數，例如「五小時剩 60%｜七日剩 85%｜執行中 Worker 1」（時間窗名稱與帳號頁、時間線一致）。 */
function quotaSummary(account: AccountScheduling): string {
  return `${remainingLabel(account, "five_hour", "五小時")}｜${remainingLabel(account, "seven_day", "七日")}｜執行中 Worker ${account.running}`;
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
export function ControllerPage({ client, workspace, timeZone, pollMs = CONTROLLER_REFRESH_INTERVAL_MS, onOpenPage }: Props) {
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
  // Workspace 授權尚未確認：需要能力的工作都不會開始。讀取失敗時維持 false，不提示
  const [authorizationUnconfirmed, setAuthorizationUnconfirmed] = useState(false);
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
    client.getAuthorization(workspace.id).then(
      (authorization) => {
        if (current) setAuthorizationUnconfirmed(authorization.status !== "confirmed");
      },
      // 讀不到授權狀態時不顯示提示：無法確定，就不以警示誤導
      () => {},
    );
    return () => {
      current = false;
    };
  }, [client, workspace.id]);

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
  const statusTone = run === null ? null : STATUS_TONES[run.status];
  // 開啟區的說明：從沒開過時沒有「上次」可接續；沒有可用的帳號時，有「前往帳號」按鈕就不再寫頁名
  const pickHint =
    available.length === 0
      ? onOpenPage
        ? "沒有可用的帳號，請新增帳號或重新核對"
        : "沒有可用的帳號，請到「帳號」新增或重新核對"
      : run === null
        ? "在外部終端機開啟互動式 Claude Code"
        : "在外部終端機開啟互動式 Claude Code；選上次用過的帳號會接續上次的 Session";
  const stripe = stripeTone(run);
  const openSelectId = useId();
  const switchSelectId = useId();
  const pickHintId = useId();
  const headingId = useId();

  return (
    <section className="page controller-page" aria-labelledby={headingId}>
      {/* 頁面標題只寫頁名；Workspace 名稱由外殼的情境列顯示 */}
      <header className="page-header">
        <h2 id={headingId}>Controller</h2>
      </header>
      {view === null && loadError === null && <p className="empty-note">載入中…</p>}
      {loadError && <LongNotice alert tone="danger" title="無法取得 Controller 狀態" message={loadError} />}
      {actionError && <LongNotice alert tone="danger" message={actionError} />}
      {/* 開著與未開啟都要提示：開著時「換帳號重開」需要帳號清單 */}
      {view && profilesError && <LongNotice alert tone="danger" title="無法取得帳號清單" message={profilesError} />}
      {view && (
        <>
          {/* 駕駛艙：最上面一列是目前狀態的讀數，主要動作緊跟在右；提示、額度、開啟表單與技術細節依序在下 */}
          <section
            className={stripe ? `card controller-run stripe-${stripe}` : "card controller-run"}
            aria-label="Controller 狀態"
          >
            <div className="controller-run-head">
              <dl className="controller-readout">
                <div>
                  <dt>狀態</dt>
                  <dd>
                    <span
                      className={`dot${statusTone ? ` dot-${statusTone}` : ""} controller-status controller-status-${
                        run === null ? "idle" : run.status
                      }`}
                    >
                      {run === null ? "未開啟" : STATUS_LABELS[run.status]}
                    </span>
                  </dd>
                </div>
                {run && (
                  <>
                    <div>
                      <dt>Controller 帳號</dt>
                      <dd>{profileName(run.profile_id)}</dd>
                    </div>
                    <div>
                      <dt>開啟時間</dt>
                      <dd className="tabular">{formatTimestamp(run.opened_at, timeZone)}</dd>
                    </div>
                    <div>
                      <dt>Session</dt>
                      <dd>{sessionLabel(run)}</dd>
                    </div>
                  </>
                )}
              </dl>
              {open && (
                <button
                  type="button"
                  className="no-shrink"
                  disabled={busy || ending}
                  onClick={() => void act(() => client.endControllerCli(workspace.id))}
                >
                  {ending ? "結束中…" : "結束"}
                </button>
              )}
            </div>

            {open &&
              (run?.rebound_at != null ? (
                <div role="alert" className="callout callout-warning">
                  <p>Runtime 重新啟動過，這個視窗的平台工具已失效。請按「結束」，再用同一個帳號重新開啟（會接續 Session）。</p>
                </div>
              ) : (
                // 開啟中（starting）時終端機視窗還沒開好，等執行中再提示切過去
                run?.status === "running" && <p className="controller-hint">已在終端機開啟，請切到該視窗操作</p>
              ))}
            {open && run !== null && run.switch_to_profile_id !== null && (
              <div className="callout callout-accent">
                <p>換帳號重開中：改用 {profileName(run.switch_to_profile_id)}</p>
              </div>
            )}
            {open && controllerAccount?.quota_gate?.state === "draining" && (
              <div role="status" className="callout callout-warning">
                <p>Controller 帳號的額度已接近上限，建議換帳號重開</p>
              </div>
            )}
            {/* 結束原因可能是整段 Runtime 錯誤：長警示，色調與狀態點、色條一致（開啟失敗與中斷是 danger，一般結束中性） */}
            {run && !open && run.end_reason !== null && (
              <LongNotice
                tone={launchFailed(run) || run.status === "interrupted" ? "danger" : null}
                title={launchFailed(run) ? "開啟失敗" : run.status === "interrupted" ? "中斷原因" : "結束原因"}
                message={run.end_reason}
              />
            )}

            {open && controllerAccount && (
              // 額度：數字在上、量表在下；未知時只寫「未知」，不畫空的量表
              <dl className="controller-gauges" role="group" aria-label="Controller 帳號額度">
                <QuotaGauge label="五小時剩餘" percent={remainingPercent(controllerAccount, "five_hour")} />
                <QuotaGauge label="七日剩餘" percent={remainingPercent(controllerAccount, "seven_day")} />
                <div className="controller-gauge">
                  <dt>執行中 Worker</dt>
                  <dd>
                    <span className="controller-gauge-value tabular">{controllerAccount.running}</span>
                  </dd>
                </div>
              </dl>
            )}
            {overviewError && <p className="controller-hint">無法取得額度：{overviewError}</p>}

            {/* 授權未確認時，開著或未開啟都提示：放在「開啟 Controller」區上方，開啟前就看得到。
                固定的短句：與授權頁同一種提示（MASTER 7.1：.callout-warning＋.callout-title） */}
            {authorizationUnconfirmed && (
              <div className="callout callout-warning controller-authorization">
                <p className="callout-title">尚未確認授權</p>
                <p>需要能力的工作都不會開始</p>
                {onOpenPage && (
                  <button type="button" onClick={() => onOpenPage("authorization")}>
                    前往授權
                  </button>
                )}
              </div>
            )}

            {!open && (
              <section className="inset controller-launch" aria-label="開啟 Controller">
                {profiles === null && profilesError === null && <p className="controller-hint">載入帳號中…</p>}
                <div className="controller-pick">
                  {profiles !== null && (
                    <div className="field">
                      <label htmlFor={openSelectId}>Controller 帳號</label>
                      <select
                        id={openSelectId}
                        aria-describedby={pickHintId}
                        value={profileId}
                        onChange={(e) => setPickedProfileId(e.target.value)}
                      >
                        {available.length === 0 && <option value="">（沒有可用的帳號）</option>}
                        {profiles.map((profile) => (
                          <option key={profile.id} value={profile.id} disabled={!profile.dispatchable}>
                            {optionLabel(profile)}
                          </option>
                        ))}
                      </select>
                    </div>
                  )}
                  <button
                    type="button"
                    className="btn-primary"
                    disabled={busy || !selectable}
                    onClick={() => void act(() => client.openControllerCli(workspace.id, profileId))}
                  >
                    開啟 Controller
                  </button>
                </div>
                {profiles !== null && (
                  <p id={pickHintId} className="field-hint">
                    {pickHint}
                  </p>
                )}
                {profiles !== null && available.length === 0 && onOpenPage && (
                  <button type="button" onClick={() => onOpenPage("accounts")}>
                    前往帳號
                  </button>
                )}
                {unavailable.length > 0 && (
                  <details className="controller-unavailable">
                    <summary>
                      不可用的帳號 <span className="count">{unavailable.length}</span>
                    </summary>
                    <ul aria-label="不可用的帳號">
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

            <details className="controller-more">
              <summary>詳細資訊</summary>
              <dl className="facts">
                <dt>操作手冊版本</dt>
                <dd>{view.playbook_version}</dd>
              </dl>
            </details>
          </section>

          {/* 換帳號重開是次要動作：放在駕駛艙下方，不與目前狀態搶位置 */}
          {open && profiles !== null && (
            <section className="section controller-switch" aria-label="換帳號重開">
              <div className="section-heading">
                <h3>換帳號重開</h3>
                <p>結束目前的終端機視窗，改用另一個帳號開啟新的 Session</p>
              </div>
              <div className="controller-pick">
                <div className="field">
                  <label htmlFor={switchSelectId}>換成帳號</label>
                  <select
                    id={switchSelectId}
                    value={switchTarget}
                    disabled={ending}
                    onChange={(e) => setSwitchTo(e.target.value)}
                  >
                    {switchTargets.map((profile) => (
                      <option key={profile.id} value={profile.id}>
                        {optionLabel(profile)}
                      </option>
                    ))}
                  </select>
                </div>
                <button
                  type="button"
                  disabled={busy || ending || switchTarget === ""}
                  onClick={() => void act(() => client.switchControllerCli(workspace.id, switchTarget))}
                >
                  換帳號重開
                </button>
              </div>
              {switchTargets.length === 0 && <p className="field-hint">沒有其他可用的帳號</p>}
            </section>
          )}
        </>
      )}
    </section>
  );
}

/** 一個視窗的剩餘額度：數字在上、量表在下；未知時只寫「未知」，不畫空的量表。 */
function QuotaGauge({ label, percent }: { label: string; percent: number | null }) {
  return (
    <div className="controller-gauge">
      <dt>{label}</dt>
      <dd>
        <span className="controller-gauge-value tabular">{percentText(percent)}</span>
        {percent !== null && (
          <div className="meter" aria-hidden="true">
            <span style={{ width: `${percent}%` }} />
          </div>
        )}
      </dd>
    </div>
  );
}

/**
 * 可能很長的錯誤或原因（例如整段 Runtime 錯誤）：長警示（MASTER 7：.notice），第一句完整顯示，其餘收進「詳細資訊」。
 * tone 為 null 時是中性（一般的結束原因）。
 */
function LongNotice({
  tone,
  title,
  message,
  alert = false,
}: {
  tone: "danger" | null;
  title?: string;
  message: string;
  alert?: boolean;
}) {
  const { summary, rest } = splitFirstSentence(message);
  return (
    <div role={alert ? "alert" : undefined} className={tone === null ? "notice" : `notice notice-${tone}`}>
      {title && <p className="notice-title">{title}</p>}
      <p>{summary}</p>
      {rest !== "" && (
        <details>
          <summary>詳細資訊</summary>
          <p className="pre-wrap">{rest}</p>
        </details>
      )}
    </div>
  );
}
