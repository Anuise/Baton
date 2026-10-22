import { useCallback, useEffect, useState, type FormEvent } from "react";

import { formatTimestamp } from "./time";
import type { AccountClient, AccountProfile, AuthSource } from "./runtimeClient";

export type AccountsClient = Pick<
  AccountClient,
  "listProfiles" | "createProfile" | "verifyProfile" | "openLoginTerminal" | "archiveProfile"
>;

type ListState =
  | { state: "loading" }
  | { state: "ready"; profiles: AccountProfile[]; limit: number }
  | { state: "failed"; reason: string };

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
  const [name, setName] = useState("");
  const [expectedAccount, setExpectedAccount] = useState("");
  const [formError, setFormError] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [terminalNotice, setTerminalNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(async () => {
    try {
      const { profiles, limit } = await client.listProfiles();
      setList({ state: "ready", profiles, limit });
    } catch (error) {
      setList({ state: "failed", reason: reasonOf(error) });
    }
  }, [client]);

  useEffect(() => {
    void refresh();
  }, [refresh, refreshKey]);

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
      {active.length > 0 && (
        <ul className="profile-list">
          {active.map((profile) => (
            <li key={profile.id} className={profile.dispatchable ? "profile-ok" : "profile-blocked"}>
              <span className="profile-name">{profile.name}</span>
              <span className="profile-state">{profileStateLabel(profile)}</span>
              <ProfileFacts profile={profile} timeZone={timeZone} />
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
