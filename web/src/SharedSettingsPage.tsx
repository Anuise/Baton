import { useCallback, useEffect, useState } from "react";

import { shortHash } from "./textFormat";
import { formatTimestamp } from "./time";
import {
  errorCode,
  type ImportPreview,
  type SettingsCategory,
  type SettingsDiffEntry,
  type SettingsExclusion,
  type SettingsSourceKind,
  type SettingsVersionSummary,
  type SharedSettingsClient,
} from "./runtimeClient";

export type SettingsPageClient = Pick<
  SharedSettingsClient,
  "getSharedSettingsStatus" | "previewSharedSettings" | "adoptSharedSettings"
>;

type ListState =
  | { state: "loading" }
  | { state: "ready"; versions: SettingsVersionSummary[] }
  | { state: "failed"; reason: string };

interface Props {
  client: SettingsPageClient;
  /** 顯示用的時區；省略時使用系統時區（spec D-33），只有測試會指定。 */
  timeZone?: string;
}

const SOURCE_LABELS: Record<SettingsSourceKind, string> = {
  settings: "設定檔",
  instructions: "指示檔",
  user_state: "使用者狀態檔",
};

const REASON_LABELS: Record<SettingsExclusion["reason"], string> = {
  login_related: "與登入／認證有關，不匯入",
  secret_key: "欄位名稱像秘密，已略過",
  secret_value: "內容像秘密，整個項目不匯入",
};

const CATEGORIES: { category: SettingsCategory; label: string }[] = [
  { category: "preferences", label: "偏好" },
  { category: "instructions", label: "指示" },
  { category: "hooks", label: "hooks" },
  { category: "mcp", label: "MCP" },
  { category: "plugins", label: "plugins" },
];

const CHANGE_LABELS: Record<SettingsDiffEntry["change"], string> = {
  added: "新增",
  removed: "移除",
  changed: "變更",
};

/** 差異種類的徽章語意：新增為成功、移除為危險、變更為強調。 */
const CHANGE_BADGES: Record<SettingsDiffEntry["change"], string> = {
  added: "badge-success",
  removed: "badge-danger",
  changed: "badge-accent",
};

/** 頁面說明：先說這是什麼，再說它不會做什麼。 */
const SHARED_SETTINGS_LEDE =
  "所有 Account Profile 共用的偏好、指示、hooks、MCP 與 plugins，從你平常的 Claude Code 設定匯入。這只讀取你平常的 Claude Code 設定，不會修改它們，也不會複製任何登入資料。";

const SOURCE_CHANGED_MESSAGE = "來源在預覽後已改變，請重新預覽";

function reasonOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** 差異值的顯示：JSON 一行；太長的內容（例如指示全文）截斷。 */
function showValue(value: unknown): string {
  if (value === null || value === undefined) return "（無）";
  const text = JSON.stringify(value);
  return text.length > 120 ? `${text.slice(0, 120)}…` : text;
}

/** 截斷時的完整內容（放在 title，讓截斷的值仍可取得全文）；沒有截斷時不需要。 */
function fullValue(value: unknown): string | undefined {
  if (value === null || value === undefined) return undefined;
  const text = JSON.stringify(value);
  return text.length > 120 ? text : undefined;
}

/** 共同設定頁：唯讀預覽匯入、看差異、採用成不可變版本（spec D-09）。 */
export function SharedSettingsPage({ client, timeZone }: Props) {
  const [list, setList] = useState<ListState>({ state: "loading" });
  const [preview, setPreview] = useState<ImportPreview | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(async () => {
    try {
      const { versions } = await client.getSharedSettingsStatus();
      setList({ state: "ready", versions });
    } catch (failure) {
      setList({ state: "failed", reason: reasonOf(failure) });
    }
  }, [client]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const runPreview = async () => {
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      setPreview(await client.previewSharedSettings());
    } catch (failure) {
      setPreview(null);
      setError(reasonOf(failure));
    } finally {
      setBusy(false);
    }
  };

  const adopt = async (current: ImportPreview) => {
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      const created = await client.adoptSharedSettings(current.candidate_hash);
      setPreview(null);
      setNotice(`已採用，建立版本 ${created.version}`);
      await refresh();
    } catch (failure) {
      if (errorCode(failure) === "settings_source_changed") {
        // 預覽已過期：不能再採用，必須重新預覽
        setPreview(null);
        setError(SOURCE_CHANGED_MESSAGE);
      } else {
        setError(reasonOf(failure));
      }
    } finally {
      setBusy(false);
    }
  };

  const versions = list.state === "ready" ? [...list.versions].reverse() : [];

  return (
    <>
      <header className="page-header">
        <h2>共同設定</h2>
        <div className="page-actions">
          <button type="button" className="btn-primary" disabled={busy} onClick={() => void runPreview()}>
            預覽匯入
          </button>
        </div>
      </header>
      <p className="page-lede shared-settings-lede">
        {SHARED_SETTINGS_LEDE}
      </p>
      {notice && (
        <p role="status" className="callout callout-success">
          {notice}
        </p>
      )}
      {error && (
        <p role="alert" className="callout callout-danger">
          {error}
        </p>
      )}
      {preview && <PreviewPanel preview={preview} busy={busy} onAdopt={() => void adopt(preview)} />}

      <section className="section shared-settings-versions" aria-labelledby="shared-settings-versions-title">
        <div className="section-heading">
          <h3 id="shared-settings-versions-title">已採用的版本</h3>
          {versions.length > 0 && <span className="count">{versions.length}</span>}
          <p>每次採用都會建立一個不會再變動的版本，新的在前。</p>
        </div>
        {list.state === "loading" && <p className="empty-note">載入中…</p>}
        {list.state === "failed" && (
          <p role="alert" className="callout callout-danger">
            無法取得共同設定版本：{list.reason}
          </p>
        )}
        {list.state === "ready" && versions.length === 0 && <p className="empty-note">尚未採用任何版本</p>}
        {versions.length > 0 && (
          <ul className="row-list">
            {versions.map((item) => (
              <li key={item.version} aria-label={`版本 ${item.version}`} className="shared-settings-version">
                <div className="row-head">
                  <span>版本 {item.version}</span>
                  <span className="meta tabular">採用於 {formatTimestamp(item.adopted_at, timeZone)}</span>
                </div>
                <p className="meta">
                  <code className="mono" title={item.content_hash}>
                    {shortHash(item.content_hash)}
                  </code>
                  <span>偏好 {item.counts.preferences}</span>
                  <span>指示 {item.counts.instructions}</span>
                  <span>hooks {item.counts.hooks}</span>
                  <span>MCP {item.counts.mcp_servers}</span>
                  <span>plugins {item.counts.plugins}</span>
                  <span>排除 {item.exclusion_count} 項</span>
                </p>
              </li>
            ))}
          </ul>
        )}
      </section>
    </>
  );
}

function PreviewPanel({ preview, busy, onAdopt }: { preview: ImportPreview; busy: boolean; onAdopt: () => void }) {
  const noDiff = preview.diff.length === 0;
  return (
    <section className="panel shared-settings-preview" aria-labelledby="shared-settings-preview-title">
      <div className="section-heading">
        <h3 id="shared-settings-preview-title">匯入預覽</h3>
        <div className="section-actions">
          <button type="button" className="btn-primary" disabled={busy || noDiff} onClick={onAdopt}>
            採用
          </button>
        </div>
        <p>確認差異與被排除的項目；按「採用」才會建立新版本。</p>
      </div>

      <section className="shared-settings-diff" aria-labelledby="settings-diff-title">
        <h4 id="settings-diff-title">與目前版本的差異</h4>
        {noDiff && <p className="empty-note">與目前版本相同，沒有差異</p>}
        {CATEGORIES.map(({ category, label }) => {
          const entries = preview.diff.filter((entry) => entry.category === category);
          if (entries.length === 0) return null;
          return (
            <section key={category} aria-labelledby={`settings-diff-${category}`} className="shared-settings-diff-group">
              <h5 id={`settings-diff-${category}`}>{label}</h5>
              <ul>
                {entries.map((entry) => (
                  <li key={entry.key}>
                    <code className="mono">{entry.key}</code>
                    <span className={`badge ${CHANGE_BADGES[entry.change]}`}>{CHANGE_LABELS[entry.change]}</span>
                    <span className="settings-diff-values">
                      {entry.change !== "added" && (
                        <code className="mono" title={fullValue(entry.before)}>
                          {showValue(entry.before)}
                        </code>
                      )}
                      {entry.change === "changed" && (
                        <>
                          <span className="settings-diff-arrow" aria-hidden="true">
                            →
                          </span>
                          <span className="visually-hidden">改為</span>
                        </>
                      )}
                      {entry.change !== "removed" && (
                        <code className="mono" title={fullValue(entry.after)}>
                          {showValue(entry.after)}
                        </code>
                      )}
                    </span>
                  </li>
                ))}
              </ul>
            </section>
          );
        })}
      </section>

      <div className="split">
        <section className="shared-settings-sources" aria-labelledby="settings-sources-title">
          <h4 id="settings-sources-title">來源</h4>
          <ul aria-label="來源檔">
            {preview.sources.map((source) => (
              <li key={source.kind}>
                <span className="shared-settings-source-head">
                  <span>{SOURCE_LABELS[source.kind]}</span>
                  <span className={`badge ${source.found ? "badge-success" : "badge-plain"}`}>
                    {source.found ? "已找到" : "找不到"}
                  </span>
                </span>
                <code className="mono">{source.path}</code>
              </li>
            ))}
          </ul>
        </section>

        <section className="shared-settings-exclusions" aria-labelledby="settings-exclusions-title">
          <h4 id="settings-exclusions-title">被排除的項目</h4>
          {preview.exclusions.length === 0 ? (
            <p className="empty-note">沒有被排除的項目</p>
          ) : (
            <ul aria-label="被排除的項目">
              {preview.exclusions.map((exclusion) => (
                <li key={`${exclusion.source}:${exclusion.item}`}>
                  <code className="mono">{exclusion.item}</code>
                  <span className="meta">
                    <span>{SOURCE_LABELS[exclusion.source]}</span>
                    <span>{REASON_LABELS[exclusion.reason]}</span>
                  </span>
                </li>
              ))}
            </ul>
          )}
        </section>
      </div>
    </section>
  );
}
