import { useCallback, useEffect, useId, useRef, useState, type FormEvent, type MouseEvent } from "react";

import { FailureNotice } from "./ExitProgress";
import { CopyButton } from "./LongText";
import { MoreMenu } from "./MoreMenu";
import { shortPath, splitFirstSentence } from "./textFormat";
import { formatTimestamp } from "./time";
import { GATE_REASON_LABELS, RUN_STATUS_LABELS } from "./TaskBoard";
import type {
  AccountClient,
  AccountProfile,
  AccountScheduling,
  AuthSource,
  CreateProfileFromDirRequest,
  GateState,
  PlanLimits,
  QuotaConfidence,
  QuotaGate,
  QuotaObservation,
  QuotaSource,
  SchedulingClient,
  SchedulingOverview,
} from "./runtimeClient";

/** 排程總覽是選用的：沒有提供時帳號頁只是不顯示進行中的 Run 與用量；沒有 `setQuotaLimits` 時不顯示方案上限表單。 */
export type AccountsClient = Pick<
  AccountClient,
  | "listProfiles"
  | "createProfile"
  | "createProfileFromExistingDir"
  | "verifyProfile"
  | "openLoginTerminal"
  | "archiveProfile"
  | "reorderProfiles"
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

/** 狀態徽章、色條與阻擋提示的語意：可派工為成功、尚未核對為警告、其他阻擋為危險（精靈與帳號頁共用）。 */
export function profileTone(profile: AccountProfile): "success" | "warning" | "danger" {
  if (profile.dispatchable) return "success";
  return profile.verification === null ? "warning" : "danger";
}

/** 選了「更多」裡的選項後收起選單，焦點回到「更多」（共用的 MoreMenu 不會自己收起）。 */
function closeMenu(event: MouseEvent<HTMLButtonElement>) {
  const menu = event.currentTarget.closest("details");
  if (menu === null) return;
  menu.open = false;
  menu.querySelector("summary")?.focus();
}

/** 帳號頁：新增最多 8 個 Profile、在終端機完成原生登入、核對實際認證來源與帳號身分（spec D-32、D-20）。 */
export function AccountsPage({ client, timeZone, onChange, refreshKey }: Props) {
  const [list, setList] = useState<ListState>({ state: "loading" });
  const [scheduling, setScheduling] = useState<OverviewState>({ state: "none" });
  const [name, setName] = useState("");
  const [expectedAccount, setExpectedAccount] = useState("");
  const [formError, setFormError] = useState<string | null>(null);
  // 動作失敗：狀態詞（哪個動作沒有完成）＋ Runtime 的說明
  const [actionError, setActionError] = useState<{ title: string; reason: string } | null>(null);
  const [terminalNotice, setTerminalNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [reordering, setReordering] = useState(false);
  // 新增表單平時收起；還沒有任何帳號時一直展開（首次設定的下一步就是新增）
  const [adding, setAdding] = useState(false);
  const addId = useId();
  const nameId = useId();
  const expectedId = useId();
  // 卡片增減後要聚焦的位置，不讓焦點掉到 body（WCAG 2.4.3）：
  // 新增成功、表單收起後是新帳號的「在終端機登入」；封存後是相鄰帳號的「更多」；沒有帳號可聚焦時為 null（回到新增）
  const [focusTarget, setFocusTarget] = useState<{ profileId: string | null; control: "login" | "more" } | null>(null);
  const loginButtons = useRef(new Map<string, HTMLButtonElement>());
  const cards = useRef(new Map<string, HTMLLIElement>());
  const addToggleRef = useRef<HTMLButtonElement>(null);
  const nameRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (focusTarget === null) return;
    const { profileId, control } = focusTarget;
    const target =
      profileId === null
        ? null
        : control === "login"
          ? loginButtons.current.get(profileId)
          : cards.current.get(profileId)?.querySelector<HTMLElement>(".more-menu > summary");
    // 目標不在清單上（例如重新讀取失敗）時，至少回到「新增帳號」；沒有帳號時它不顯示，改到新增表單的名稱欄
    (target ?? addToggleRef.current ?? nameRef.current)?.focus();
    setFocusTarget(null);
  }, [focusTarget, list]);

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
  const empty = list.state === "ready" && active.length === 0;
  const showAdd = empty || adding;

  const create = async (event: FormEvent) => {
    event.preventDefault();
    if (name.trim() === "") {
      setFormError("請輸入名稱");
      return;
    }
    setBusy(true);
    setFormError(null);
    try {
      const created = await client.createProfile({
        name,
        expectedAccount: expectedAccount.trim() === "" ? undefined : expectedAccount,
      });
      setName("");
      setExpectedAccount("");
      // 新帳號出現在清單上，下一步是在它的卡片上登入；表單收起，焦點移到它的「在終端機登入」
      setAdding(false);
      await refresh();
      setFocusTarget({ profileId: created.id, control: "login" });
      onChange?.();
    } catch (error) {
      // 保留輸入，讓使用者修正後重送
      setFormError(reasonOf(error));
    } finally {
      setBusy(false);
    }
  };

  /** 以既有目錄建立；失敗時把錯誤交給表單顯示。 */
  const createFromDir = async (request: CreateProfileFromDirRequest) => {
    const created = await client.createProfileFromExistingDir(request);
    setAdding(false);
    await refresh();
    setFocusTarget({ profileId: created.id, control: "login" });
    onChange?.();
  };

  /**
   * 把第 `index` 個帳號往上（-1）或往下（+1）移一格：先以新順序顯示，再把全部帳號的新順序存到 Runtime。
   * 被拒（例如清單已變動）時顯示 Runtime 的說明，並重新讀取它目前的順序。只影響顯示，不影響派工。
   */
  const move = async (index: number, offset: -1 | 1) => {
    if (list.state !== "ready") return;
    const order = [...active];
    [order[index], order[index + offset]] = [order[index + offset], order[index]];
    const archived = list.profiles.filter((p) => p.status !== "active");
    setList({ ...list, profiles: [...order, ...archived] });
    setActionError(null);
    setTerminalNotice(null);
    setReordering(true);
    try {
      const { profiles, limit } = await client.reorderProfiles(order.map((p) => p.id));
      setList({ state: "ready", profiles, limit });
    } catch (error) {
      setActionError({ title: "無法調整順序", reason: reasonOf(error) });
      await refresh();
    } finally {
      setReordering(false);
    }
  };

  /**
   * 執行一個針對單一 Profile 的動作；失敗時以 `failure` 為狀態詞顯示 Runtime 的說明，清單維持原樣。回傳是否成功。
   */
  const act = async (work: () => Promise<unknown>, failure: string, notice: string | null = null) => {
    setActionError(null);
    setTerminalNotice(null);
    try {
      await work();
      setTerminalNotice(notice);
      await refresh();
      onChange?.();
      return true;
    } catch (error) {
      setActionError({ title: failure, reason: reasonOf(error) });
      return false;
    }
  };

  /** 封存第 `index` 個帳號；成功後卡片消失，焦點移到下一張（沒有就上一張）卡片的「更多」。 */
  const archive = async (index: number) => {
    const neighbor = active[index + 1] ?? active[index - 1] ?? null;
    if (await act(() => client.archiveProfile(active[index].id), "無法封存")) {
      setFocusTarget({ profileId: neighbor?.id ?? null, control: "more" });
    }
  };

  return (
    <section className="section accounts-page" aria-labelledby="accounts-title">
      <div className="section-heading">
        <h3 id="accounts-title">帳號清單</h3>
        <span className="count accounts-count" title="使用中的帳號／上限">
          {active.length} / {limit}
        </span>
        {!empty && (
          <div className="section-actions">
            <button
              ref={addToggleRef}
              type="button"
              aria-expanded={adding}
              aria-controls={addId}
              // 已達上限時不能展開；已展開的仍可收起
              disabled={full && !adding}
              onClick={() => setAdding((open) => !open)}
            >
              新增帳號
            </button>
          </div>
        )}
        <p>
          每個帳號（Account Profile）各自在終端機登入 Claude Code。排列順序只影響帳號在畫面上的順序
          {/* 括號內不拆開，避免「帳號選／單」 */}
          <span className="nowrap">（包括 Controller 帳號選單）</span>，不影響派工。
        </p>
      </div>

      {list.state === "loading" && <p className="empty-note">載入中…</p>}
      {/* Runtime 的錯誤可能很長：長警示（狀態詞＋原因，其餘收進「詳細資訊」），不用淡底提示（MASTER 7.1） */}
      {list.state === "failed" && <FailureNotice title="無法取得帳號清單" reason={list.reason} />}
      {empty && <p className="empty-note">還沒有帳號</p>}
      {full && <p className="empty-note accounts-full">已達上限：最多 {limit} 個帳號，請先封存用不到的帳號再新增</p>}
      {scheduling.state === "failed" && <FailureNotice title="無法取得排程總覽" reason={scheduling.reason} />}
      {terminalNotice && (
        <p role="status" className="callout callout-accent">
          {terminalNotice}
        </p>
      )}
      {actionError && <FailureNotice title={actionError.title} reason={actionError.reason} />}

      <div className="accounts-add" id={addId} hidden={!showAdd}>
        <form className="panel accounts-add-form" aria-label="新增帳號" onSubmit={(event) => void create(event)}>
          <header>
            <h4>新增帳號</h4>
            <p className="field-hint">建立後在它的卡片上按「在終端機登入」，完成原生登入。</p>
          </header>
          <div className="field">
            <label htmlFor={nameId}>名稱</label>
            <input ref={nameRef} id={nameId} value={name} onChange={(e) => setName(e.target.value)} />
          </div>
          <div className="field">
            <label htmlFor={expectedId}>預期帳號（選填）</label>
            <input
              id={expectedId}
              value={expectedAccount}
              onChange={(e) => setExpectedAccount(e.target.value)}
              placeholder="name@example.com"
            />
          </div>
          {formError && (
            <p role="alert" className="field-error">
              {formError}
            </p>
          )}
          <div className="form-actions">
            <button type="submit" disabled={busy || full}>
              新增帳號
            </button>
          </div>
        </form>
        <ProfileFromDirForm disabled={full} onCreate={createFromDir} />
      </div>

      {active.length > 0 && (
        <ul className="accounts-list">
          {active.map((profile, index) => (
            <li
              key={profile.id}
              ref={(node) => {
                if (node) cards.current.set(profile.id, node);
                else cards.current.delete(profile.id);
              }}
              className={`card account-card${profile.dispatchable ? "" : ` stripe-${profileTone(profile)}`}`}
            >
              <div className="account-head">
                <div className="account-title">
                  <h4 className="profile-name clamp-2" title={profile.name}>
                    {profile.name}
                  </h4>
                  <span className={`badge profile-state badge-${profileTone(profile)}`}>{profileStateLabel(profile)}</span>
                </div>
                <div className="account-actions">
                  <button
                    ref={(node) => {
                      if (node) loginButtons.current.set(profile.id, node);
                      else loginButtons.current.delete(profile.id);
                    }}
                    type="button"
                    // 列內動作一律次要（MASTER 7.1）；「還沒登入、下一步是登入」由狀態徽章與色條表達
                    aria-label={`在終端機登入 ${profile.name}`}
                    onClick={() =>
                      void act(
                        () => client.openLoginTerminal(profile.id),
                        "無法開啟終端機",
                        `已開啟終端機，請在其中完成 ${profile.name} 的登入，再按「重新核對」`,
                      )
                    }
                  >
                    在終端機登入
                  </button>
                  <button
                    type="button"
                    aria-label={`重新核對 ${profile.name}`}
                    onClick={() => void act(() => client.verifyProfile(profile.id), "無法重新核對")}
                  >
                    重新核對
                  </button>
                  {/* 排序與封存很少用，收在「更多」；選了之後收起 */}
                  <MoreMenu ariaLabel={`更多（${profile.name}）`} align="end">
                    <button
                      type="button"
                      className="btn-quiet"
                      aria-label={`上移 ${profile.name}`}
                      disabled={reordering || index === 0}
                      onClick={(event) => {
                        closeMenu(event);
                        void move(index, -1);
                      }}
                    >
                      上移
                    </button>
                    <button
                      type="button"
                      className="btn-quiet"
                      aria-label={`下移 ${profile.name}`}
                      disabled={reordering || index === active.length - 1}
                      onClick={(event) => {
                        closeMenu(event);
                        void move(index, 1);
                      }}
                    >
                      下移
                    </button>
                    <button
                      type="button"
                      className="btn-quiet more-menu-danger"
                      aria-label={`封存 ${profile.name}`}
                      onClick={(event) => {
                        closeMenu(event);
                        void archive(index);
                      }}
                    >
                      封存
                    </button>
                  </MoreMenu>
                </div>
              </div>
              {/* 阻擋原因可能很長：長警示（只有色條上色），色調與卡片色條、狀態徽章同一個 tone */}
              {profile.blockers.length > 0 && (
                <div
                  role="alert"
                  className={`notice notice-${profileTone(profile) === "danger" ? "danger" : "warning"} profile-blockers`}
                >
                  {profile.blockers.map((blocker) => (
                    <p key={blocker.code} className="profile-blocker">
                      {blocker.message}
                    </p>
                  ))}
                </div>
              )}
              <div className="account-body">
                <ProfileFacts profile={profile} timeZone={timeZone} />
                {scheduling.state === "ready" && (
                  <AccountLoad
                    account={scheduling.overview.accounts.find((account) => account.profile_id === profile.id) ?? null}
                    timeZone={timeZone}
                    onSaveLimits={saveLimits}
                  />
                )}
              </div>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

/** 以已登入的既有設定目錄建立 Profile（不必重新登入）；失敗時顯示 Runtime 的說明並保留輸入。 */
function ProfileFromDirForm({
  disabled,
  onCreate,
}: {
  disabled: boolean;
  onCreate: (request: CreateProfileFromDirRequest) => Promise<void>;
}) {
  const [name, setName] = useState("");
  const [configDir, setConfigDir] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const nameId = useId();
  const dirId = useId();

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (name.trim() === "") {
      setError("請輸入名稱");
      return;
    }
    if (configDir.trim() === "") {
      setError("請輸入設定目錄");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await onCreate({ name, configDir });
      setName("");
      setConfigDir("");
    } catch (failure) {
      // 保留輸入，讓使用者修正後重送
      setError(reasonOf(failure));
    } finally {
      setBusy(false);
    }
  };

  return (
    <form className="panel accounts-add-form" aria-label="以既有目錄建立帳號" onSubmit={(event) => void submit(event)}>
      <header>
        <h4>以既有目錄建立</h4>
        <p className="field-hint">已經在別處登入過的 Claude Code 設定目錄，不必重新登入。</p>
      </header>
      <div className="field">
        <label htmlFor={nameId}>名稱</label>
        <input id={nameId} value={name} onChange={(e) => setName(e.target.value)} />
      </div>
      <div className="field">
        <label htmlFor={dirId}>設定目錄</label>
        <input
          id={dirId}
          className="mono"
          value={configDir}
          onChange={(e) => setConfigDir(e.target.value)}
          placeholder="C:\Users\me\.claude-Work-3"
        />
      </div>
      {error && (
        <p role="alert" className="field-error">
          {error}
        </p>
      )}
      <div className="form-actions">
        {/* 一頁只留一個主要動作（新增帳號），其餘送出鈕用次要樣式（MASTER 第 7 節） */}
        <button type="submit" className="btn-secondary" disabled={busy || disabled}>
          以既有目錄建立
        </button>
      </div>
    </form>
  );
}

/**
 * 一個 Profile 的事實（兩欄）：實際認證來源、帳號、最近核對時間、Claude Code 版本；
 * 帳號與預期不同或尚未核對時列出預期帳號；外部目錄縮短顯示（完整在 title，可複製）。
 */
function ProfileFacts({ profile, timeZone }: { profile: AccountProfile; timeZone?: string }) {
  const { verification } = profile;
  const showExpected =
    profile.expected_account !== null &&
    (verification === null || verification.account?.toLowerCase() !== profile.expected_account.toLowerCase());
  const external = profile.config_dir_origin === "external";
  if (!verification && !showExpected && !external) return null;
  return (
    <dl className="facts account-facts">
      {verification && (
        <>
          {verification.account && (
            <>
              <dt>帳號</dt>
              <dd>
                <OneLineValue value={verification.account} />
              </dd>
            </>
          )}
          <dt>認證來源</dt>
          {/* 說明可能帶長路徑（例：環境變數由哪個設定檔設定）：只佔一行，完整在 title，長的可複製 */}
          <dd className="profile-source">
            <OneLineValue value={sourceLabel(verification.source, verification.source_detail)} copyLabel="複製認證來源" />
          </dd>
          <dt>最近核對</dt>
          <dd className="tabular">{formatTimestamp(verification.checked_at, timeZone)}</dd>
          {verification.cli_version && (
            <>
              <dt>Claude Code 版本</dt>
              <dd className="mono">{verification.cli_version}</dd>
            </>
          )}
        </>
      )}
      {showExpected && profile.expected_account !== null && (
        <>
          <dt>預期帳號</dt>
          <dd className="profile-expected">
            <OneLineValue value={profile.expected_account} field="預期帳號" />
          </dd>
        </>
      )}
      {external && (
        <>
          <dt>外部目錄</dt>
          <dd className="profile-origin copy-text">
            <span className="mono truncate" title={profile.config_dir}>
              {shortPath(profile.config_dir)}
            </span>
            <CopyButton text={profile.config_dir} label="複製路徑" />
          </dd>
        </>
      )}
    </dl>
  );
}

/** 超過這個長度的值（帳號 email、認證來源說明）另附複製：窄欄放不下時會被截斷。 */
const LONG_VALUE_CHARS = 32;

/**
 * 事實欄的值只佔一行，放不下時截斷（完整值在 title）；長的另附複製按鈕，不在窄欄裡折成好幾段。
 * 帳號（email）與認證來源共用。
 */
function OneLineValue({ value, field, copyLabel = "複製帳號" }: { value: string; field?: string; copyLabel?: string }) {
  return (
    <span className="copy-text">
      <span className="truncate" title={value}>
        {value}
      </span>
      {value.length > LONG_VALUE_CHARS && <CopyButton text={value} label={copyLabel} field={field} />}
    </span>
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

/**
 * 觀測不完整的提示：Runtime 依固定句型說明哪些視窗沒有已觀測用量，是一句話的狀態說明，用提示（callout，MASTER 7.1）。
 * 狀態詞在前（Runtime 的說明已帶「觀測不完整」開頭時不重複）；萬一不只一句，第一句完整顯示，其餘收進「詳細資訊」。
 */
function IncompleteNotice({ reason }: { reason: string | null }) {
  const { summary, rest } = splitFirstSentence((reason ?? "").replace(/^觀測不完整[：:]?/, ""));
  return (
    <div role="note" className="callout callout-warning quota-incomplete">
      <p className="callout-title">觀測不完整</p>
      {summary !== "" && <p>{summary}</p>}
      {rest !== "" && (
        <details>
          <summary>詳細資訊</summary>
          <p>{rest}</p>
        </details>
      )}
    </div>
  );
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
    <div className="account-load">
      <div className="account-runs" role="group" aria-label={`${account.name} 的進行中 Run`}>
        <span className="account-running tabular">
          進行中 {account.running}/{account.limit}
        </span>
        {account.runs.length > 0 && (
          <ul>
            {account.runs.map((run) => (
              <li key={run.run_id}>
                {run.task_title}（第 {run.generation} 次）：{RUN_STATUS_LABELS[run.status]}
                {run.cancel_requested && "，已要求取消"}
              </li>
            ))}
          </ul>
        )}
      </div>
      <AccountGate name={account.name} gate={account.quota_gate} timeZone={timeZone} />
      <AccountQuota account={account} timeZone={timeZone} onSaveLimits={onSaveLimits} />
    </div>
  );
}

/**
 * 門檻狀態描述額度本身，不說「可派工」：帳號能不能派工還取決於登入與核對（狀態徽章），
 * 兩者並列時不能互相矛盾。代碼（open／paused／draining）只放在 title。
 */
const GATE_STATE_LABELS: Record<GateState, string> = {
  open: "未達門檻",
  paused: "已達門檻，停止派工",
  draining: "交接中",
};

/**
 * 一個帳號的額度門檻（spec D-13）：狀態、進入的原因與當時用量、進入時間；
 * 沒有紀錄（null）代表從未離開 open。停止派工或交接中時說明恢復條件（遲滯）與記下的重置時間。
 */
function AccountGate({ name, gate, timeZone }: { name: string; gate: QuotaGate | null; timeZone?: string }) {
  const state = gate?.state ?? "open";
  // 停止派工或交接中是要使用者知道的狀態，內容都是固定的短句與標籤：提示（callout），狀態詞上色（MASTER 7.1）。
  // 未達門檻只是一行次要資訊
  const tone = state === "open" ? "meta" : `callout ${state === "draining" ? "callout-danger" : "callout-warning"}`;
  return (
    <div className={`profile-gate profile-gate-${state} ${tone}`} role="group" aria-label={`${name} 的額度門檻`}>
      <span className={state === "open" ? "gate-state" : "gate-state callout-title"}>
        額度：<strong title={state}>{GATE_STATE_LABELS[state]}</strong>
      </span>
      {gate && (
        <span className="gate-reason">
          原因：{GATE_REASON_LABELS[gate.reason] ?? gate.reason}
          {gate.used !== null && `（當時用量 ${quotaLabel(gate.used)}）`}
        </span>
      )}
      {gate && <span className="gate-since">自 {formatTimestamp(gate.since, timeZone)} 起</span>}
      {gate && state !== "open" && (
        <>
          <span className="gate-hint">回到 60% 以下或重置後才恢復派工</span>
          {gate.reset_at !== null && <span className="gate-reset">重置時間：{formatTimestamp(gate.reset_at, timeZone)}</span>}
        </>
      )}
    </div>
  );
}

/** 一個視窗的用量長條：以較保守的判斷為準；未知時是虛線外框的空槽，不畫成 0%。數字另外寫出。 */
function WindowMeter({ observation }: { observation: QuotaObservation | null }) {
  const exhausted = observation?.status === "exhausted";
  const used = exhausted ? 1 : (observation?.used ?? null);
  if (used === null) return <span className="meter account-meter-unknown" aria-hidden="true" />;
  // 與額度門檻一致：60% 起是警告、75% 起停止派工
  const tone = used >= 0.75 ? " meter-danger" : used >= 0.6 ? " meter-warning" : "";
  return (
    <span className={`meter${tone}`} aria-hidden="true">
      <span style={{ width: `${Math.min(used, 1) * 100}%` }} />
    </span>
  );
}

/** 一個帳號的額度：各視窗已觀測與推估各自的數值（spec D-12），以及判斷用量（取較保守者）與來源、可信度、時間。 */
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
    <div className="account-quota" role="group" aria-label={`${account.name} 的額度`}>
      <ul className="account-windows">
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
              <span className="account-window-label">{w.label}</span>
              <WindowMeter observation={w.effective} />
              <span className="account-window-values">
                <span className="quota-observed">已觀測：{observationLabel(w.observed)}</span>
                <span className="quota-estimated">
                  推估：{observationLabel(w.estimated)}（{tokens === null ? "" : `本機累計 ${tokenCount(tokens)}；`}
                  {limit === null ? "未設定方案上限" : `方案上限 ${tokenCount(limit)}`}）
                </span>
              </span>
            </li>
          );
        })}
      </ul>
      <p className="meta account-quota-summary">
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
      </p>
      {!quota.complete && <IncompleteNotice reason={quota.incomplete_reason} />}
      {onSaveLimits && (
        <details className="account-limits">
          <summary>設定方案上限</summary>
          <PlanLimitsForm account={account} onSave={(limits) => onSaveLimits(account.profile_id, limits)} />
        </details>
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
  const fiveHourId = useId();
  const sevenDayId = useId();

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
    <form className="inset account-limits-form" onSubmit={(event) => void submit(event)}>
      <div className="field-row">
        <div className="field">
          <label htmlFor={fiveHourId}>五小時方案上限</label>
          <input
            id={fiveHourId}
            type="number"
            min={1}
            step={1}
            value={fiveHour}
            onChange={(e) => setFiveHour(e.target.value)}
          />
        </div>
        <div className="field">
          <label htmlFor={sevenDayId}>七日方案上限</label>
          <input
            id={sevenDayId}
            type="number"
            min={1}
            step={1}
            value={sevenDay}
            onChange={(e) => setSevenDay(e.target.value)}
          />
        </div>
      </div>
      {error && (
        <p role="alert" className="field-error">
          無法儲存方案上限：{error}
        </p>
      )}
      <div className="form-actions">
        <button type="submit" className="btn-secondary" disabled={busy} aria-label={`儲存 ${account.name} 的方案上限`}>
          儲存方案上限
        </button>
        <span className="field-hint">單位為 token；留空代表未設定</span>
      </div>
    </form>
  );
}
