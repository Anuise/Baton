import { useCallback, useEffect, useState } from "react";

import { profileStateLabel } from "./AccountsPage";
import { CopyButton } from "./LongText";
import { shortPath } from "./textFormat";
import type { AccountClient, SetupStatus, ToolCheck } from "./runtimeClient";

export type SetupClient = Pick<AccountClient, "getSetup" | "recheckSetup" | "acknowledgeBackgroundNotice">;

type WizardState =
  | { state: "loading" }
  | { state: "ready"; status: SetupStatus }
  | { state: "failed"; reason: string };

interface Props {
  client: SetupClient;
  /** 使用者重新檢查或確認告知後通知上層（重新檢查會重新核對各 Profile）。 */
  onChange?: () => void;
  /** 上層要求重新讀取時改變這個值。 */
  refreshKey?: number;
}

const TOOL_STATE_LABEL: Record<ToolCheck["status"], string> = {
  ok: "可用",
  missing: "缺少",
  too_old: "版本過舊",
  unknown: "無法確認",
};

/** 狀態徽章與左側色條的語意：可用為成功，其餘都需要使用者處理。 */
const TOOL_TONE: Record<ToolCheck["status"], "success" | "warning"> = {
  ok: "success",
  missing: "warning",
  too_old: "warning",
  unknown: "warning",
};

/** 官方說明網址只顯示網域（完整網址在 title，另可複製）；不是網址時退回縮短路徑。 */
function docsHost(url: string): string {
  try {
    return new URL(url).host || shortPath(url);
  } catch {
    return shortPath(url);
  }
}

function reasonOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/**
 * 首次啟動精靈（spec D-20）：檢查 Claude Code、Git for Windows 與各 Profile 的登入，並告知背景執行行為。
 * 缺項只顯示官方指令與「重新檢查」，沒有任何會替使用者下載或安裝的按鈕。
 */
export function SetupWizard({ client, onChange, refreshKey }: Props) {
  const [wizard, setWizard] = useState<WizardState>({ state: "loading" });
  const [busy, setBusy] = useState(false);

  const load = useCallback(
    async (work: () => Promise<SetupStatus>) => {
      try {
        setWizard({ state: "ready", status: await work() });
      } catch (error) {
        setWizard({ state: "failed", reason: reasonOf(error) });
      }
    },
    [],
  );

  useEffect(() => {
    void load(() => client.getSetup());
  }, [client, load, refreshKey]);

  /** 使用者主動的動作：完成後通知上層。 */
  const run = async (work: () => Promise<SetupStatus>) => {
    setBusy(true);
    await load(work);
    setBusy(false);
    onChange?.();
  };

  if (wizard.state === "loading") return <section className="setup-wizard"><p>檢查中…</p></section>;
  if (wizard.state === "failed") {
    return (
      <section className="setup-wizard">
        <p role="alert">無法取得首次設定狀態：{wizard.reason}</p>
        <RecheckButton busy={busy} onClick={() => void run(() => client.recheckSetup())} />
      </section>
    );
  }

  const { status } = wizard;
  const acknowledged = status.background_notice.acknowledged_at !== null;
  if (status.ready && acknowledged) {
    return (
      <section className="setup-wizard setup-done stripe-success" aria-label="首次設定">
        <p>首次設定已完成</p>
        <RecheckButton busy={busy} onClick={() => void run(() => client.recheckSetup())} />
      </section>
    );
  }

  const needsAttention = status.profiles.some((profile) => !profile.dispatchable);
  return (
    <section className="setup-wizard" aria-labelledby="setup-title">
      <h2 id="setup-title">首次設定</h2>
      <ToolSection title="Claude Code" check={status.claude_code} />
      <ToolSection title="Git for Windows" check={status.git} />

      <section aria-labelledby="setup-profiles-title">
        <h3 id="setup-profiles-title">帳號 Profile</h3>
        {status.profiles.length === 0 ? (
          <p>還沒有帳號 Profile。請在下方「帳號」新增一個，並在終端機完成原生登入。</p>
        ) : (
          <ul className="setup-profiles">
            {status.profiles.map((profile) => (
              <li key={profile.id}>
                <span className="setup-profile-name clamp-2" title={profile.name}>
                  {profile.name}
                </span>
                <span className={`badge ${profile.dispatchable ? "badge-success" : "badge-warning"}`}>{profileStateLabel(profile)}</span>
              </li>
            ))}
          </ul>
        )}
        {status.profiles.length > 0 && needsAttention && <p>需要處理的 Profile 請在下方「帳號」處理。</p>}
      </section>

      <section aria-labelledby="setup-notice-title" className="setup-notice">
        <h3 id="setup-notice-title">背景執行</h3>
        <p>{status.background_notice.text}</p>
        {acknowledged ? (
          <p className="setup-acknowledged">已確認</p>
        ) : (
          <button type="button" disabled={busy} onClick={() => void run(() => client.acknowledgeBackgroundNotice())}>
            我知道了
          </button>
        )}
      </section>

      <RecheckButton busy={busy} onClick={() => void run(() => client.recheckSetup())} />
    </section>
  );
}

function RecheckButton({ busy, onClick }: { busy: boolean; onClick: () => void }) {
  return (
    <button type="button" disabled={busy} onClick={onClick}>
      重新檢查
    </button>
  );
}

/** 一項工具的檢查結果；指令只是文字，使用者自行在終端機執行。 */
function ToolSection({ title, check }: { title: string; check: ToolCheck }) {
  const id = `setup-tool-${title.replace(/\W+/g, "-")}`;
  return (
    <section aria-labelledby={id} className={`setup-tool card stripe-${TOOL_TONE[check.status]} setup-tool-${check.status}`}>
      <header className="card-header">
        <h3 id={id}>{title}</h3>
        <span className={`badge badge-${TOOL_TONE[check.status]}`}>{TOOL_STATE_LABEL[check.status]}</span>
      </header>
      {check.version && (
        <p>
          目前版本 {check.version}
          {check.minimum_version ? `（最低 ${check.minimum_version}）` : ""}
        </p>
      )}
      <p>{check.message}</p>
      {check.guidance.length > 0 && (
        <>
          <p>以下官方指令只供參考，請自行在終端機執行：</p>
          <ul className="setup-guidance">
            {check.guidance.map((item) => (
              <li key={item.command}>
                <span>{item.title}</span>
                <code className="pre-wrap">{item.command}</code>
              </li>
            ))}
          </ul>
        </>
      )}
      {check.status !== "ok" && (
        <p className="setup-docs copy-text">
          官方說明：
          <span className="mono truncate" title={check.docs_url}>
            {docsHost(check.docs_url)}
          </span>
          <CopyButton text={check.docs_url} label="複製網址" />
        </p>
      )}
    </section>
  );
}
