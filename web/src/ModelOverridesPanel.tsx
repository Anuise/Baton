import { useCallback, useEffect, useId, useState, type FormEvent, type ReactNode } from "react";

import type { ModelCatalogClient, ModelOption, ModelOverrides, Workspace, WorkspaceClient } from "./runtimeClient";
import { truncateText } from "./textFormat";
import { formatTimestamp } from "./time";

type Role = keyof ModelOverrides;
type Draft = Record<Role, string>;
type LoadState = { state: "loading" } | { state: "ready"; workspace: Workspace } | { state: "failed"; reason: string };
/** 可選模型清單；取不到時只剩「預設（不指定）」與目前的值，頁面照常運作。`updatedAt` 為 null 是內建清單。 */
type Catalog = { models: ModelOption[]; updatedAt: number | null; failure: string | null; loaded: boolean };
type RefreshResult = { kind: "done"; count: number } | { kind: "failed"; reason: string };

interface Props {
  client: Pick<WorkspaceClient, "listWorkspaces" | "setModelOverrides"> & ModelCatalogClient;
  workspace: Workspace;
  /** 顯示更新時間用；不指定時使用系統時區（只供測試指定）。 */
  timeZone?: string;
}

/** 三個角色與一句話說明（依 GLOSSARY.md）。 */
const ROLES: { role: Role; label: string; hint: string }[] = [
  { role: "controller", label: "Controller", hint: "規劃工作、拆 Task、派工與評估成果" },
  { role: "worker", label: "Worker", hint: "執行 Task" },
  { role: "reviewer", label: "Reviewer", hint: "獨立審查成果" },
];

const DEFAULT_LABEL = "預設（不指定）";
const DEFAULT_HINT = `${DEFAULT_LABEL}＝沿用帳號的 Claude Code 預設模型`;

/** 選項文字的上限：原生選單以最長選項為最小寬度（MASTER 6.1），完整文字放在選項的 title。 */
const OPTION_MAX_CHARS = 60;

function reasonOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** 清單來源：內建，或最後一次以 API 金鑰更新的時間。 */
function catalogSource(updatedAt: number | null, timeZone?: string): string {
  return updatedAt === null ? "內建" : `更新於 ${formatTimestamp(updatedAt, timeZone)}`;
}

const draftFrom = (overrides: ModelOverrides): Draft => ({
  controller: overrides.controller ?? "",
  worker: overrides.worker ?? "",
  reviewer: overrides.reviewer ?? "",
});

// 留空（含只有空白）送 null，表示不覆寫
const valueOf = (text: string): string | null => (text.trim() === "" ? null : text.trim());

/**
 * Workspace 依角色的模型覆寫（spec D-30）。
 * 每次開啟都向 Runtime 讀取最新值，避免沿用側欄清單裡過時的 Workspace。
 * 可用模型清單的更新影響所有 Workspace，不在這裡，見 `ModelCatalogSettings`（App 設定）。
 */
export function ModelOverridesPanel({ client, workspace, timeZone }: Props) {
  const idPrefix = useId();
  const [loaded, setLoaded] = useState<LoadState>({ state: "loading" });
  const [draft, setDraft] = useState<Draft>(draftFrom(workspace.model_overrides));
  const [catalog, setCatalog] = useState<Catalog>({ models: [], updatedAt: null, failure: null, loaded: false });
  const [notice, setNotice] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const loadCatalog = useCallback(async () => {
    try {
      const { models, updated_at } = await client.listModels();
      setCatalog({ models, updatedAt: updated_at, failure: null, loaded: true });
    } catch (e) {
      setCatalog({ models: [], updatedAt: null, failure: reasonOf(e), loaded: false });
    }
  }, [client]);

  const load = useCallback(async () => {
    try {
      const { workspaces } = await client.listWorkspaces();
      const current = workspaces.find((w) => w.id === workspace.id);
      if (!current) {
        setLoaded({ state: "failed", reason: "找不到這個 Workspace" });
        return;
      }
      setLoaded({ state: "ready", workspace: current });
      setDraft(draftFrom(current.model_overrides));
    } catch (e) {
      setLoaded({ state: "failed", reason: reasonOf(e) });
    }
  }, [client, workspace.id]);

  useEffect(() => {
    setLoaded({ state: "loading" });
    setNotice(null);
    setError(null);
    void load();
    void loadCatalog();
  }, [load, loadCatalog]);

  if (loaded.state === "loading") {
    return (
      <PageFrame>
        <p className="empty-note">載入中…</p>
      </PageFrame>
    );
  }
  if (loaded.state === "failed") {
    return (
      <PageFrame>
        <p className="field-error" role="alert">
          無法取得模型設定：{loaded.reason}
        </p>
      </PageFrame>
    );
  }
  const current = loaded.workspace;
  const saved = draftFrom(current.model_overrides);
  const archived = current.status === "archived";
  // 與已保存的值不同的角色數；沒有變更時不能儲存（比照決策權限）
  const changedCount = ROLES.filter(({ role }) => draft[role] !== saved[role]).length;
  const changed = changedCount > 0;

  const save = async (event: FormEvent) => {
    event.preventDefault();
    setBusy(true);
    setNotice(null);
    setError(null);
    try {
      const updated = await client.setModelOverrides(current.id, {
        controller: valueOf(draft.controller),
        worker: valueOf(draft.worker),
        reviewer: valueOf(draft.reviewer),
      });
      setLoaded({ state: "ready", workspace: updated });
      setDraft(draftFrom(updated.model_overrides));
      setNotice("已保存模型設定");
    } catch (e) {
      // 保留輸入，讓使用者修正後重送
      setError(reasonOf(e));
    } finally {
      setBusy(false);
    }
  };

  // 版面：三個角色的模型（一張精簡表單）；可用模型清單的更新在 App 設定（影響所有 Workspace）
  return (
    <PageFrame>
      <section className="section model-overrides-section">
        <div className="section-heading">
          <h3>各角色使用的模型</h3>
          <p>{DEFAULT_HINT}。</p>
          <p>只套用在這個 Workspace，且只影響之後派出的 Run；已派出的 Run 維持派工時的模型。</p>
        </div>
        {/* Runtime 的錯誤說明可能很長：用長警示，不用淡底提示（MASTER 7） */}
        {catalog.failure && (
          <div className="notice notice-warning">
            <p className="notice-title">無法取得可用模型清單</p>
            <p>{catalog.failure}。仍可選擇預設或保留目前的設定。</p>
          </div>
        )}
        {archived && (
          <div className="callout">
            <p>這個 Workspace 已封存，不能變更模型設定。</p>
          </div>
        )}
        <form className="panel model-overrides-form" onSubmit={(event) => void save(event)}>
          <fieldset disabled={archived || busy}>
            <legend className="visually-hidden">各角色使用的模型</legend>
            {ROLES.map(({ role, label, hint }) => (
              <div key={role} className="model-overrides-role">
                <label htmlFor={`${idPrefix}-${role}`}>{label} 模型</label>
                <p id={`${idPrefix}-${role}-hint`} className="field-hint">
                  {hint}
                </p>
                <select
                  id={`${idPrefix}-${role}`}
                  aria-describedby={`${idPrefix}-${role}-hint`}
                  value={draft[role]}
                  onChange={(e) => {
                    setDraft({ ...draft, [role]: e.target.value });
                    setNotice(null);
                  }}
                >
                  <option value="">{DEFAULT_LABEL}</option>
                  {/* 已存的值不在清單裡時也列為選項，避免下拉選單把它靜默清掉 */}
                  {saved[role] !== "" && !catalog.models.some((m) => m.id === saved[role]) && (
                    <option value={saved[role]} title={saved[role]}>
                      {truncateText(saved[role], OPTION_MAX_CHARS - 16)}（目前的設定，不在可用清單中）
                    </option>
                  )}
                  {catalog.models.map((model) => (
                    <option key={model.id} value={model.id} title={`${model.label}（${model.id}）`}>
                      {truncateText(`${model.label}（${model.id}）`, OPTION_MAX_CHARS)}
                    </option>
                  ))}
                </select>
              </div>
            ))}
            <div className="form-actions">
              <button type="submit" disabled={!changed}>
                儲存模型設定
              </button>
              {changed && <span className="meta">已修改 {changedCount} 項，尚未儲存</span>}
              {notice && (
                <p className="model-overrides-saved" role="status">
                  {notice}
                </p>
              )}
            </div>
          </fieldset>
          {error && (
            <p className="field-error" role="alert">
              {error}
            </p>
          )}
        </form>
        <p className="section-hint">
          {catalog.loaded && `可用模型清單：${catalogSource(catalog.updatedAt, timeZone)}。`}
          要更新清單，請到「App 設定」。
        </p>
      </section>
    </PageFrame>
  );
}

/** 頁面外框：載入中、讀取失敗與正常內容共用同一個頁面標題（h2 與跑道欄的頁名相同）。 */
function PageFrame({ children }: { children: ReactNode }) {
  return (
    <section className="page model-overrides-page" aria-label="模型設定">
      <header className="page-header">
        <h2>模型設定</h2>
      </header>
      {children}
    </section>
  );
}

/**
 * 可用模型清單（AI-33）：以 Anthropic API 金鑰查詢可用的模型並保存清單；所有 Workspace 的「模型設定」共用這份清單。
 * 要輸入金鑰、影響所有 Workspace，所以放在平台的「App 設定」頁，不在 Workspace 的規則裡。
 */
export function ModelCatalogSettings({
  client,
  timeZone,
}: {
  client: ModelCatalogClient;
  /** 顯示更新時間用；不指定時使用系統時區（只供測試指定）。 */
  timeZone?: string;
}) {
  const titleId = useId();
  // 目前清單的更新時間（null＝內建）；讀不到時不顯示來源，仍可更新
  const [source, setSource] = useState<{ updatedAt: number | null } | null>(null);
  // API 金鑰只放在這個輸入框的 state，送出時立即清空，不寫入任何儲存空間（AI-33）
  const [apiKey, setApiKey] = useState("");
  const [refreshing, setRefreshing] = useState(false);
  const [result, setResult] = useState<RefreshResult | null>(null);

  useEffect(() => {
    let cancelled = false;
    client.listModels().then(
      ({ updated_at }) => !cancelled && setSource({ updatedAt: updated_at }),
      () => !cancelled && setSource(null),
    );
    return () => {
      cancelled = true;
    };
  }, [client]);

  const refresh = async (event: FormEvent) => {
    event.preventDefault();
    const key = apiKey.trim();
    setApiKey("");
    setRefreshing(true);
    setResult(null);
    try {
      const { models, updated_at } = await client.refreshModels(key);
      setSource({ updatedAt: updated_at });
      setResult({ kind: "done", count: models.length });
    } catch (e) {
      // 失敗時保留原清單；錯誤訊息由 Runtime 產生，不含金鑰
      setResult({ kind: "failed", reason: reasonOf(e) });
    } finally {
      setRefreshing(false);
    }
  };

  return (
    <section className="section model-catalog-settings" aria-labelledby={titleId}>
      <div className="section-heading">
        <h3 id={titleId}>可用模型清單</h3>
        <p>各 Workspace 的「模型設定」從這份清單選擇模型，所有 Workspace 共用。</p>
        {source !== null && <p>清單來源：{catalogSource(source.updatedAt, timeZone)}</p>}
      </div>
      <div className="panel">
        <form className="model-catalog-form" onSubmit={(event) => void refresh(event)}>
          <label className="field">
            <span className="field-label">Anthropic API 金鑰</span>
            <input type="password" autoComplete="off" value={apiKey} onChange={(e) => setApiKey(e.target.value)} />
          </label>
          <button type="submit" disabled={refreshing || apiKey.trim() === ""}>
            更新清單
          </button>
        </form>
        <p className="section-hint">金鑰只用於這一次查詢，不會保存；查到的模型清單會保存，之後重開 App 仍會使用。</p>
        {result?.kind === "done" && (
          <div className="callout callout-success" role="status">
            <p>已更新可用模型清單（{result.count} 個模型）</p>
          </div>
        )}
        {result?.kind === "failed" && (
          <p className="field-error" role="alert">
            {result.reason}
          </p>
        )}
      </div>
    </section>
  );
}
