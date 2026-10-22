import { useCallback, useEffect, useState } from "react";

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
    <section className="shared-settings-page" aria-labelledby="shared-settings-title">
      <h2 id="shared-settings-title">共同設定</h2>
      <p>這只讀取你平常的 Claude Code 設定，不會修改它們，也不會複製任何登入資料。</p>
      <button type="button" disabled={busy} onClick={() => void runPreview()}>
        預覽匯入
      </button>
      {notice && <p role="status">{notice}</p>}
      {error && <p role="alert">{error}</p>}
      {preview && <PreviewPanel preview={preview} busy={busy} onAdopt={() => void adopt(preview)} />}

      <h3>已採用的版本</h3>
      {list.state === "loading" && <p>載入中…</p>}
      {list.state === "failed" && <p role="alert">無法取得共同設定版本：{list.reason}</p>}
      {list.state === "ready" && versions.length === 0 && <p>尚未採用任何版本</p>}
      {versions.length > 0 && (
        <ul className="settings-version-list">
          {versions.map((item) => (
            <li key={item.version} aria-label={`版本 ${item.version}`}>
              <span className="settings-version-number">版本 {item.version}</span>{" "}
              <code>{item.content_hash.slice(0, 12)}</code>{" "}
              <span>採用於 {formatTimestamp(item.adopted_at, timeZone)}</span>
              <span className="settings-version-counts">
                偏好 {item.counts.preferences}、指示 {item.counts.instructions}、hooks {item.counts.hooks}、MCP{" "}
                {item.counts.mcp_servers}、plugins {item.counts.plugins}
              </span>
              <span>排除 {item.exclusion_count} 項</span>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

function PreviewPanel({ preview, busy, onAdopt }: { preview: ImportPreview; busy: boolean; onAdopt: () => void }) {
  const noDiff = preview.diff.length === 0;
  return (
    <div className="settings-preview">
      <h3>來源</h3>
      <ul aria-label="來源檔">
        {preview.sources.map((source) => (
          <li key={source.kind}>
            {SOURCE_LABELS[source.kind]}：<code>{source.path}</code> <span>{source.found ? "已找到" : "找不到"}</span>
          </li>
        ))}
      </ul>

      <h3>被排除的項目</h3>
      {preview.exclusions.length === 0 ? (
        <p>沒有被排除的項目</p>
      ) : (
        <ul aria-label="被排除的項目">
          {preview.exclusions.map((exclusion) => (
            <li key={`${exclusion.source}:${exclusion.item}`}>
              <code>{exclusion.item}</code>（{SOURCE_LABELS[exclusion.source]}）：{REASON_LABELS[exclusion.reason]}
            </li>
          ))}
        </ul>
      )}

      <h3>與目前版本的差異</h3>
      {noDiff && <p>與目前版本相同，沒有差異</p>}
      {CATEGORIES.map(({ category, label }) => {
        const entries = preview.diff.filter((entry) => entry.category === category);
        if (entries.length === 0) return null;
        return (
          <section key={category} aria-labelledby={`settings-diff-${category}`}>
            <h4 id={`settings-diff-${category}`}>{label}</h4>
            <ul>
              {entries.map((entry) => (
                <li key={entry.key}>
                  <code>{entry.key}</code> <span>{CHANGE_LABELS[entry.change]}</span>{" "}
                  <span className="settings-diff-values">
                    {entry.change !== "added" && <code>{showValue(entry.before)}</code>}
                    {entry.change === "changed" && " → "}
                    {entry.change !== "removed" && <code>{showValue(entry.after)}</code>}
                  </span>
                </li>
              ))}
            </ul>
          </section>
        );
      })}
      <button type="button" disabled={busy || noDiff} onClick={onAdopt}>
        採用
      </button>
    </div>
  );
}
