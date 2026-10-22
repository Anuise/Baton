import { useCallback, useEffect, useState, type FormEvent } from "react";

import { formatTimestamp } from "./time";
import { RUN_STATUS_LABELS } from "./TaskBoard";
import type {
  AccountClient,
  AccountProfile,
  AccountScheduling,
  AuthSource,
  PlanLimits,
  QuotaConfidence,
  QuotaObservation,
  QuotaSource,
  SchedulingClient,
  SchedulingOverview,
} from "./runtimeClient";

/** 排程總覽是選用的：沒有提供時帳號頁只是不顯示進行中的 Run 與用量；沒有 `setQuotaLimits` 時不顯示方案上限表單。 */
export type AccountsClient = Pick<
  AccountClient,
  "listProfiles" | "createProfile" | "verifyProfile" | "openLoginTerminal" | "archiveProfile"
> &
  Partial<Pick<SchedulingClient, "schedulingOverview" | "setQuotaLimits">>;

/** 額度輪詢間隔：每 5 分鐘重新讀取排程總覽（spec D-31）。 */
export const QUOTA_REFRESH_INTERVAL_MS = 5 * 60 * 1000;

type ListState =
  | { state: "loading" }
  | { state: "ready"; profiles: AccountProfile[]; limit: number }
  | { state: "failed"; reason: string };

/** 排程總覽讀取失敗不影響帳號清單，只在清單上說明讀不到。 */
type OverviewState = { state: "none" } | { state: "ready"; overview: SchedulingOverview } | { state: "failed"; reason: string };

interface Props {
  client: AccountsClient;
  /** 顯示用的時區；省略時使用系統時區（spec D-33），只有測試會指定。 */
  timeZone?: string;
  /** Profile 有變動（新增、核對、封存）時通知上層，例如讓精靈重新讀取。 */
  onChange?: () => void;
  /** 上層要求重新讀取時改變這個值。 */
  refreshKey?: number;
}

function reasonOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** 認證來源的顯示文字：實際用的是什麼，而不是使用者以為的是什麼。 */
function sourceLabel(source: AuthSource, detail: string | null): string {
  const suffix = detail ? `（${detail}）` : "";
  switch (source) {
    case "native_login":
      return "原生登入";
    case "api_key":
      return `API key${suffix}`;
    case "oauth_token":
      return `OAuth token${suffix || "（環境變數）"}`;
    case "third_party_provider":
      return `第三方供應商${suffix}`;
    case "other":
      return `未知的認證方式${suffix}`;
    default:
      return "無";
  }
}

/** 狀態標籤：可派工，或各種阻擋的概況（詳細原因在下方警示）。 */
export function profileStateLabel(profile: AccountProfile): string {
  if (profile.dispatchable) return "可派工";
  if (profile.verification === null) return "尚未核對";
  if (!profile.verification.logged_in) return "未登入";
  return "已阻擋派工";
}

/** 帳號頁：新增最多 8 個 Profile、在終端機完成原生登入、核對實際認證來源與帳號身分（spec D-32、D-20）。 */
export function AccountsPage({ client, timeZone, onChange, refreshKey }: Props) {
  const [list, setList] = useState<ListState>({ state: "loading" });
  const [scheduling, setScheduling] = useState<OverviewState>({ state: "none" });
  const [name, setName] = useState("");
  const [expectedAccount, setExpectedAccount] = useState("");
  const [formError, setFormError] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [terminalNotice, setTerminalNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const loadOverview = useCallback(async () => {
    if (!client.schedulingOverview) return;
    try {
      setScheduling({ state: "ready", overview: await client.schedulingOverview() });
    } catch (error) {
      setScheduling({ state: "failed", reason: reasonOf(error) });
    }
  }, [client]);

  const refresh = useCallback(async () => {
    try {
      const { profiles, limit } = await client.listProfiles();
      setList({ state: "ready", profiles, limit });
    } catch (error) {
      setList({ state: "failed", reason: reasonOf(error) });
    }
    await loadOverview();
  }, [client, loadOverview]);

  useEffect(() => {
    void refresh();
  }, [refresh, refreshKey]);

  // 額度會隨時間變化（重置、其他 Run 的用量），定期重新讀取排程總覽
  useEffect(() => {
    if (!client.schedulingOverview) return;
    const timer = setInterval(() => void loadOverview(), QUOTA_REFRESH_INTERVAL_MS);
    return () => clearInterval(timer);
  }, [client, loadOverview]);

  /** 儲存方案上限，成功後改用回傳的總覽；失敗時把錯誤交給表單顯示。 */
  const saveLimits =
    client.setQuotaLimits &&
    (async (profileId: string, limits: PlanLimits) => {
      setScheduling({ state: "ready", overview: await client.setQuotaLimits!(profileId, limits) });
    });

  const active = list.state === "ready" ? list.profiles.filter((p) => p.status === "active") : [];
  const limit = list.state === "ready" ? list.limit : 8;
  const full = list.state === "ready" && active.length >= limit;

  const create = async (event: FormEvent) => {
    event.preventDefault();
    if (name.trim() === "") {
      setFormError("請輸入名稱");
      return;
    }
    setBusy(true);
    setFormError(null);
    try {
      await client.createProfile({
        name,
        expectedAccount: expectedAccount.trim() === "" ? undefined : expectedAccount,
      });
      setName("");
      setExpectedAccount("");
      await refresh();
      onChange?.();
    } catch (error) {
      // 保留輸入，讓使用者修正後重送
      setFormError(reasonOf(error));
    } finally {
      setBusy(false);
    }
  };

  /** 執行一個針對單一 Profile 的動作；失敗時顯示 Runtime 的說明，清單維持原樣。 */
  const act = async (work: () => Promise<unknown>, notice: string | null = null) => {
    setActionError(null);
    setTerminalNotice(null);
    try {
      await work();
      setTerminalNotice(notice);
      await refresh();
      onChange?.();
    } catch (error) {
      setActionError(reasonOf(error));
    }
  };

  return (
    <section className="accounts-page" aria-labelledby="accounts-title">
      <h2 id="accounts-title">帳號</h2>
      <p className="accounts-count">
        <span>{active.length} / {limit}</span>
      </p>
      {list.state === "loading" && <p>載入中…</p>}
      {list.state === "failed" && <p role="alert">無法取得帳號清單：{list.reason}</p>}
      {list.state === "ready" && active.length === 0 && <p>還沒有帳號 Profile</p>}
      {scheduling.state === "failed" && <p role="alert">無法取得排程總覽：{scheduling.reason}</p>}
      {active.length > 0 && (
        <ul className="profile-list">
          {active.map((profile) => (
            <li key={profile.id} className={profile.dispatchable ? "profile-ok" : "profile-blocked"}>
              <span className="profile-name">{profile.name}</span>
              <span className="profile-state">{profileStateLabel(profile)}</span>
              <ProfileFacts profile={profile} timeZone={timeZone} />
              {scheduling.state === "ready" && (
                <AccountLoad
                  account={scheduling.overview.accounts.find((account) => account.profile_id === profile.id) ?? null}
                  timeZone={timeZone}
                  onSaveLimits={saveLimits}
                />
              )}
              {profile.blockers.map((blocker) => (
                <p key={blocker.code} role="alert" className="profile-blocker">
                  {blocker.message}
                </p>
              ))}
              <div className="profile-actions">
                <button
                  type="button"
                  aria-label={`在終端機登入 ${profile.name}`}
                  onClick={() =>
                    void act(
                      () => client.openLoginTerminal(profile.id),
                      `已開啟終端機，請在其中完成 ${profile.name} 的登入，再按「重新核對」`,
                    )
                  }
                >
                  在終端機登入
                </button>
                <button
                  type="button"
                  aria-label={`重新核對 ${profile.name}`}
                  onClick={() => void act(() => client.verifyProfile(profile.id))}
                >
                  重新核對
                </button>
                <button
                  type="button"
                  aria-label={`封存 ${profile.name}`}
                  onClick={() => void act(() => client.archiveProfile(profile.id))}
                >
                  封存
                </button>
              </div>
            </li>
          ))}
        </ul>
      )}
      {terminalNotice && <p role="status">{terminalNotice}</p>}
      {actionError && <p role="alert">{actionError}</p>}
      <form className="profile-form" onSubmit={(event) => void create(event)}>
        <label>
          名稱
          <input value={name} onChange={(e) => setName(e.target.value)} />
        </label>
        <label>
          預期帳號（選填）
          <input
            value={expectedAccount}
            onChange={(e) => setExpectedAccount(e.target.value)}
            placeholder="name@example.com"
          />
        </label>
        <button type="submit" disabled={busy || full}>
          新增 Profile
        </button>
        {full && <p>已達上限：最多 {limit} 個帳號 Profile。請先封存用不到的 Profile，再新增。</p>}
        {formError && <p role="alert">{formError}</p>}
      </form>
    </section>
  );
}

/** 一個 Profile 的事實：實際認證來源、帳號、最近核對時間；尚未核對時只顯示預期帳號。 */
function ProfileFacts({ profile, timeZone }: { profile: AccountProfile; timeZone?: string }) {
  const { verification } = profile;
  const showExpected =
    profile.expected_account !== null &&
    (verification === null || verification.account?.toLowerCase() !== profile.expected_account.toLowerCase());
  return (
    <>
      {verification && (
        <span className="profile-source">
          認證來源：{sourceLabel(verification.source, verification.source_detail)}
          {verification.account ? `；帳號：${verification.account}` : ""}；最近核對：
          {formatTimestamp(verification.checked_at, timeZone)}
        </span>
      )}
      {showExpected && <span className="profile-expected">預期帳號：{profile.expected_account}</span>}
    </>
  );
}

/** 用量比例的顯示：未觀測就是「未知」，不顯示 0。 */
function quotaLabel(quotaUsed: number | null): string {
  return quotaUsed === null ? "未知" : `${Math.round(quotaUsed * 100)}%`;
}

const QUOTA_SOURCE_LABELS: Record<QuotaSource, string> = { observed: "已觀測", estimated: "推估" };

const CONFIDENCE_LABELS: Record<QuotaConfidence, string> = { observed: "已觀測", estimated: "推估", unknown: "未知" };

/** 單一來源的用量：CLI 說已達上限、或只有警告沒有數字時照實說明，不換算成百分比。 */
function observationLabel(observation: QuotaObservation | null): string {
  if (observation?.status === "exhausted") return "已達上限";
  if (observation?.status === "warning" && observation.used === null) return "接近上限（CLI 警告）";
  return quotaLabel(observation?.used ?? null);
}

function tokenCount(tokens: number): string {
  return `${tokens.toLocaleString("en-US")} token`;
}

/** 觀測不完整的說明；Runtime 的說明已帶「觀測不完整」開頭時不重複。 */
function incompleteLabel(reason: string | null): string {
  if (reason === null) return "觀測不完整";
  return reason.startsWith("觀測不完整") ? reason : `觀測不完整：${reason}`;
}

/** 一個帳號目前的並行使用：進行中 n/上限，以及進行中的 Run（Task、狀態、是否已要求取消）；額度另外分組顯示。 */
function AccountLoad({
  account,
  timeZone,
  onSaveLimits,
}: {
  account: AccountScheduling | null;
  timeZone?: string;
  onSaveLimits?: (profileId: string, limits: PlanLimits) => Promise<void>;
}) {
  if (account === null) return null;
  return (
    <>
      <div className="profile-load" role="group" aria-label={`${account.name} 的進行中 Run`}>
        <span className="profile-running">
          進行中 {account.running}/{account.limit}
        </span>
        {account.runs.length > 0 && (
          <ul className="profile-runs">
            {account.runs.map((run) => (
              <li key={run.run_id}>
                {run.task_title}（第 {run.generation} 次）：{RUN_STATUS_LABELS[run.status]}
                {run.cancel_requested && "，已要求取消"}
              </li>
            ))}
          </ul>
        )}
      </div>
      <AccountQuota account={account} timeZone={timeZone} onSaveLimits={onSaveLimits} />
    </>
  );
}

/** 一個帳號的額度：判斷用量（取較保守者）與來源、可信度、時間，以及各視窗已觀測與推估各自的數值（spec D-12）。 */
function AccountQuota({
  account,
  timeZone,
  onSaveLimits,
}: {
  account: AccountScheduling;
  timeZone?: string;
  onSaveLimits?: (profileId: string, limits: PlanLimits) => Promise<void>;
}) {
  const { quota, plan_limits } = account;
  return (
    <div className="profile-quota" role="group" aria-label={`${account.name} 的額度`}>
      <span className="quota-used">
        判斷用量：{quotaLabel(quota.used)}
        {/* 有數字但可信度未知：CLI 只發警告沒給數字，Runtime 以停止派工門檻作為保守的下限 */}
        {quota.used !== null && quota.confidence === "unknown" && "（CLI 警告，至少此值）"}
      </span>
      <span className="quota-source">來源：{quota.source === null ? "未知" : QUOTA_SOURCE_LABELS[quota.source]}</span>
      <span className="quota-confidence">可信度：{CONFIDENCE_LABELS[quota.confidence]}</span>
      <span className="quota-reset">
        {quota.reset_at === null ? "重置時間未知" : `重置時間：${formatTimestamp(quota.reset_at, timeZone)}`}
      </span>
      <span className="quota-observed-at">
        {quota.observed_at === null ? "尚未觀測" : `觀測時間：${formatTimestamp(quota.observed_at, timeZone)}`}
      </span>
      {!quota.complete && (
        <p role="note" className="quota-incomplete">
          {incompleteLabel(quota.incomplete_reason)}
        </p>
      )}
      <ul className="quota-windows">
        {quota.windows.map((w) => {
          // 有推估時用它計算時的上限（與顯示的比例一致），還沒有推估時用目前的設定
          const limit =
            w.estimated !== null
              ? w.estimated.limit_tokens
              : w.window === "five_hour"
                ? plan_limits.five_hour_tokens
                : plan_limits.seven_day_tokens;
          const tokens = w.estimated?.tokens ?? null;
          return (
            <li key={w.window}>
              <span className="quota-window-label">{w.label}</span>
              <span className="quota-observed">已觀測：{observationLabel(w.observed)}</span>
              <span className="quota-estimated">
                推估：{observationLabel(w.estimated)}（{tokens === null ? "" : `本機累計 ${tokenCount(tokens)}；`}
                {limit === null ? "未設定方案上限" : `方案上限 ${tokenCount(limit)}`}）
              </span>
            </li>
          );
        })}
      </ul>
      {onSaveLimits && (
        <PlanLimitsForm account={account} onSave={(limits) => onSaveLimits(account.profile_id, limits)} />
      )}
    </div>
  );
}

/** 輸入框的文字 → 方案上限：空白是未設定（null）；正整數由輸入框的原生驗證（min、step）把關。 */
function parseLimit(text: string): number | null {
  return text.trim() === "" ? null : Number(text);
}

/** 方案上限表單（推估的分母）；留空代表未設定，失敗時保留輸入讓使用者修正。 */
function PlanLimitsForm({
  account,
  onSave,
}: {
  account: AccountScheduling;
  onSave: (limits: PlanLimits) => Promise<void>;
}) {
  const [fiveHour, setFiveHour] = useState(String(account.plan_limits.five_hour_tokens ?? ""));
  const [sevenDay, setSevenDay] = useState(String(account.plan_limits.seven_day_tokens ?? ""));
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    setBusy(true);
    setError(null);
    try {
      await onSave({ five_hour_tokens: parseLimit(fiveHour), seven_day_tokens: parseLimit(sevenDay) });
    } catch (failure) {
      setError(reasonOf(failure));
    } finally {
      setBusy(false);
    }
  };

  return (
    <form className="quota-limits" onSubmit={(event) => void submit(event)}>
      <label>
        五小時方案上限
        <input type="number" min={1} step={1} value={fiveHour} onChange={(e) => setFiveHour(e.target.value)} />
      </label>
      <label>
        七日方案上限
        <input type="number" min={1} step={1} value={sevenDay} onChange={(e) => setSevenDay(e.target.value)} />
      </label>
      <span className="quota-limits-hint">單位為 token；留空代表未設定</span>
      <button type="submit" disabled={busy} aria-label={`儲存 ${account.name} 的方案上限`}>
        儲存方案上限
      </button>
      {error && <p role="alert">無法儲存方案上限：{error}</p>}
    </form>
  );
}
