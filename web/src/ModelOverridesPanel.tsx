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
 */
export function ModelOverridesPanel({ client, workspace, timeZone }: Props) {
  const idPrefix = useId();
  const [loaded, setLoaded] = useState<LoadState>({ state: "loading" });
  const [draft, setDraft] = useState<Draft>(draftFrom(workspace.model_overrides));
  const [catalog, setCatalog] = useState<Catalog>({ models: [], updatedAt: null, failure: null, loaded: false });
  // API 金鑰只放在這個輸入框的 state，送出時立即清空，不寫入任何儲存空間（AI-33）
  const [apiKey, setApiKey] = useState("");
  const [refreshing, setRefreshing] = useState(false);
  const [refreshResult, setRefreshResult] = useState<RefreshResult | null>(null);
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

  const refresh = async (event: FormEvent) => {
    event.preventDefault();
    const key = apiKey.trim();
    setApiKey("");
    setRefreshing(true);
    setRefreshResult(null);
    try {
      const { models, updated_at } = await client.refreshModels(key);
      setCatalog({ models, updatedAt: updated_at, failure: null, loaded: true });
      setRefreshResult({ kind: "done", count: models.length });
    } catch (e) {
      // 失敗時保留原清單；錯誤訊息由 Runtime 產生，不含金鑰
      setRefreshResult({ kind: "failed", reason: reasonOf(e) });
    } finally {
      setRefreshing(false);
    }
  };

  // 版面：三個角色的模型（一張精簡表單）→ 可用模型清單的來源與更新（偶爾才做）
  return (
    <PageFrame>
      <section className="section">
        <div className="section-heading">
          <h3>各角色使用的模型</h3>
          <p>{DEFAULT_HINT}。</p>
          <p>變更只影響之後派出的 Run，已派出的 Run 維持派工時的模型。</p>
        </div>
        {catalog.failure && (
          <div className="callout callout-warning">
            <p>無法取得可用模型清單：{catalog.failure}。仍可選擇預設或保留目前的設定。</p>
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
                  onChange={(e) => setDraft({ ...draft, [role]: e.target.value })}
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
              <button type="submit">儲存模型設定</button>
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
      </section>

      <section className="section model-catalog-refresh" aria-label="更新可用模型清單">
        <div className="section-heading">
          <h3>更新可用模型清單</h3>
          {catalog.loaded && (
            <p>清單來源：{catalog.updatedAt === null ? "內建" : `更新於 ${formatTimestamp(catalog.updatedAt, timeZone)}`}</p>
          )}
        </div>
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
        {refreshResult?.kind === "done" && (
          <div className="callout callout-success" role="status">
            <p>已更新可用模型清單（{refreshResult.count} 個模型）</p>
          </div>
        )}
        {refreshResult?.kind === "failed" && (
          <p className="field-error" role="alert">
            {refreshResult.reason}
          </p>
        )}
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
