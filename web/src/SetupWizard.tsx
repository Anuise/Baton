import { useCallback, useEffect, useState } from "react";

import { profileStateLabel, profileTone } from "./AccountsPage";
import { CopyButton, LongText } from "./LongText";
import { docsHost } from "./textFormat";
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

/** 狀態徽章的語意：可用為成功，其餘都需要使用者處理。 */
const TOOL_TONE: Record<ToolCheck["status"], "success" | "warning"> = {
  ok: "success",
  missing: "warning",
  too_old: "warning",
  unknown: "warning",
};

function reasonOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/**
 * 首次啟動精靈（spec D-20）：檢查 Claude Code、Git for Windows 與各 Profile 的登入，並告知背景執行行為。
 * 缺項只顯示官方指令與「重新檢查」，沒有任何會替使用者下載或安裝的按鈕。
 *
 * 三種呈現：全部完成時精簡成一行；還沒有可派工的帳號時是歡迎畫面（全畫面首次設定的主角，標題用 h2）；
 * 其餘（例如只差確認背景執行告知）是帳號頁中的一般區段（h3）。
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

  const recheck = () => void run(() => client.recheckSetup());

  if (wizard.state === "loading") {
    return (
      <section className="setup-wizard">
        <p className="empty-note">檢查中…</p>
      </section>
    );
  }
  if (wizard.state === "failed") {
    return (
      <section className="setup-wizard setup-wizard-failed">
        <p role="alert" className="callout callout-danger">
          無法取得首次設定狀態：{wizard.reason}
        </p>
        <RecheckButton busy={busy} onClick={recheck} />
      </section>
    );
  }

  const { status } = wizard;
  const acknowledged = status.background_notice.acknowledged_at !== null;
  if (status.ready && acknowledged) {
    return (
      <section className="setup-wizard setup-done" aria-label="首次設定">
        <p className="dot dot-success">首次設定已完成</p>
        <RecheckButton busy={busy} onClick={recheck} quiet />
      </section>
    );
  }

  const dispatchable = status.profiles.filter((profile) => profile.dispatchable).length;
  const needsAttention = status.profiles.some((profile) => !profile.dispatchable);
  // 還沒有可派工的帳號：這是使用者第一次見到 Baton，以歡迎畫面呈現
  const welcome = dispatchable === 0;
  const Title = welcome ? "h2" : "h3";
  const Step = welcome ? "h3" : "h4";

  return (
    <section className={welcome ? "setup-wizard setup-welcome" : "setup-wizard section"} aria-labelledby="setup-title">
      <header className="setup-intro">
        <Title id="setup-title">首次設定</Title>
        <p>Baton 需要 Claude Code、Git for Windows，以及至少一個登入完成的帳號。缺少的項目請依指示自行處理，再按「重新檢查」。</p>
      </header>

      <div className="row-list setup-steps">
        <ToolStep title="Claude Code" check={status.claude_code} heading={Step} />
        <ToolStep title="Git for Windows" check={status.git} heading={Step} />

        <section className="row setup-step" aria-labelledby="setup-profiles-title">
          <div className="row-head">
            <Step id="setup-profiles-title">帳號登入</Step>
            <span className={`badge ${dispatchable > 0 ? "badge-success" : "badge-warning"}`}>
              {dispatchable > 0 ? `可派工 ${dispatchable} 個` : "還沒有可派工的帳號"}
            </span>
          </div>
          {status.profiles.length === 0 ? (
            <p>還沒有帳號 Profile。請在下方「Account Profile」新增一個，並在終端機完成原生登入。</p>
          ) : (
            <ul className="setup-profiles">
              {status.profiles.map((profile) => (
                <li key={profile.id}>
                  <span className="setup-profile-name clamp-2" title={profile.name}>
                    {profile.name}
                  </span>
                  <span className={`badge badge-${profileTone(profile)}`}>{profileStateLabel(profile)}</span>
                </li>
              ))}
            </ul>
          )}
          {status.profiles.length > 0 && needsAttention && (
            <p className="field-hint">需要處理的 Profile 請在下方「Account Profile」處理。</p>
          )}
        </section>

        <section className="row setup-step" aria-labelledby="setup-notice-title">
          <div className="row-head">
            <Step id="setup-notice-title">背景執行</Step>
            {acknowledged ? (
              <span className="badge badge-success setup-acknowledged">已確認</span>
            ) : (
              <span className="badge badge-warning">請閱讀</span>
            )}
          </div>
          {/* 告知是段落：限 3 行，超過時可展開；「展開」與「▶ 技術細節」同一種收合摘要樣式 */}
          <LongText text={status.background_notice.text} className="long-text-disclosure" />
          {!acknowledged && (
            <button type="button" disabled={busy} onClick={() => void run(() => client.acknowledgeBackgroundNotice())}>
              我知道了
            </button>
          )}
        </section>
      </div>

      <div className="setup-actions">
        <RecheckButton busy={busy} onClick={recheck} />
        <span className="field-hint">裝好工具或完成登入後按這裡，Baton 會重新檢查每一項。</span>
      </div>
    </section>
  );
}

function RecheckButton({ busy, onClick, quiet = false }: { busy: boolean; onClick: () => void; quiet?: boolean }) {
  return (
    <button type="button" className={quiet ? "btn-quiet btn-sm" : undefined} disabled={busy} onClick={onClick}>
      重新檢查
    </button>
  );
}

/** 一項工具的檢查結果；指令只是文字，使用者自行在終端機執行。 */
function ToolStep({ title, check, heading: Heading }: { title: string; check: ToolCheck; heading: "h3" | "h4" }) {
  const id = `setup-tool-${title.replace(/\W+/g, "-")}`;
  return (
    <section aria-labelledby={id} className={`row setup-step setup-tool-${check.status}`}>
      <div className="row-head">
        <Heading id={id}>{title}</Heading>
        <span className={`badge badge-${TOOL_TONE[check.status]}`}>{TOOL_STATE_LABEL[check.status]}</span>
      </div>
      <p className="setup-tool-message">{check.message}</p>
      {check.version && (
        <p className="meta">
          <span>
            目前版本 {check.version}
            {check.minimum_version ? `（最低 ${check.minimum_version}）` : ""}
          </span>
        </p>
      )}
      {check.guidance.length > 0 && (
        <div className="setup-guidance">
          <p className="field-hint">以下官方指令只供參考，請自行在終端機執行：</p>
          <ul>
            {check.guidance.map((item) => (
              <li key={item.command}>
                <span>{item.title}</span>
                <code className="pre-wrap">{item.command}</code>
              </li>
            ))}
          </ul>
        </div>
      )}
      {check.status !== "ok" && (
        <p className="meta setup-docs">
          <span className="copy-text">
            官方說明：
            <span className="mono truncate" title={check.docs_url}>
              {docsHost(check.docs_url)}
            </span>
            <CopyButton text={check.docs_url} label="複製網址" />
          </span>
        </p>
      )}
    </section>
  );
}
