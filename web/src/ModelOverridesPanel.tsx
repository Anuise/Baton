import { useCallback, useEffect, useState, type FormEvent } from "react";

import type { ModelCatalogClient, ModelOption, ModelOverrides, Workspace, WorkspaceClient } from "./runtimeClient";

type Role = keyof ModelOverrides;
type Draft = Record<Role, string>;
type LoadState = { state: "loading" } | { state: "ready"; workspace: Workspace } | { state: "failed"; reason: string };
/** 可選模型清單；取不到時只剩「預設（不指定）」與目前的值，頁面照常運作。 */
type Catalog = { models: ModelOption[]; failure: string | null };

interface Props {
  client: Pick<WorkspaceClient, "listWorkspaces" | "setModelOverrides"> & ModelCatalogClient;
  workspace: Workspace;
}

const ROLES: { role: Role; label: string }[] = [
  { role: "controller", label: "Controller" },
  { role: "worker", label: "Worker" },
  { role: "reviewer", label: "Reviewer" },
];

const DEFAULT_LABEL = "預設（不指定）";
const DEFAULT_HINT = `${DEFAULT_LABEL}＝沿用帳號的 Claude Code 預設模型`;

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
export function ModelOverridesPanel({ client, workspace }: Props) {
  const [loaded, setLoaded] = useState<LoadState>({ state: "loading" });
  const [draft, setDraft] = useState<Draft>(draftFrom(workspace.model_overrides));
  const [catalog, setCatalog] = useState<Catalog>({ models: [], failure: null });
  const [notice, setNotice] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const loadCatalog = useCallback(async () => {
    try {
      const { models } = await client.listModels();
      setCatalog({ models, failure: null });
    } catch (e) {
      setCatalog({ models: [], failure: reasonOf(e) });
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

  if (loaded.state === "loading") return <section aria-label="模型設定">載入中…</section>;
  if (loaded.state === "failed") {
    return (
      <section aria-label="模型設定">
        <p role="alert">無法取得模型設定：{loaded.reason}</p>
      </section>
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

  return (
    <section className="model-overrides" aria-label="模型設定">
      <h2>模型設定：{current.name}</h2>
      <p className="model-overrides-hint">{DEFAULT_HINT}。變更只影響之後派出的 Run，已派出的 Run 維持派工時的模型。</p>
      {catalog.failure && (
        <p className="model-overrides-hint">無法取得可用模型清單：{catalog.failure}。仍可選擇預設或保留目前的設定。</p>
      )}
      {archived && <p>這個 Workspace 已封存，不能變更模型設定。</p>}
      <form onSubmit={(event) => void save(event)}>
        <fieldset disabled={archived || busy}>
          {ROLES.map(({ role, label }) => (
            <label key={role}>
              {label} 模型
              <select value={draft[role]} onChange={(e) => setDraft({ ...draft, [role]: e.target.value })}>
                <option value="">{DEFAULT_LABEL}</option>
                {/* 已存的值不在清單裡時也列為選項，避免下拉選單把它靜默清掉 */}
                {saved[role] !== "" && !catalog.models.some((m) => m.id === saved[role]) && (
                  <option value={saved[role]}>{saved[role]}（目前的設定，不在可用清單中）</option>
                )}
                {catalog.models.map((model) => (
                  <option key={model.id} value={model.id}>
                    {model.label}（{model.id}）
                  </option>
                ))}
              </select>
            </label>
          ))}
          <button type="submit">儲存模型設定</button>
        </fieldset>
      </form>
      {notice && <p role="status">{notice}</p>}
      {error && <p role="alert">{error}</p>}
    </section>
  );
}
