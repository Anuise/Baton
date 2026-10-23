import { useCallback, useEffect, useState, type FormEvent } from "react";

import { formatTimestamp } from "./time";
import type { LifecycleClient, PauseCause, Workspace, WorkspaceClient, WorkspaceLifecycleView } from "./runtimeClient";

type ListState = { state: "loading" } | { state: "ready"; workspaces: Workspace[] } | { state: "failed"; reason: string };

interface Props {
  client: WorkspaceClient &
    Pick<LifecycleClient, "lifecycleStatus" | "updateWorkspaceLifecycle" | "updateWorkspaceLoginResume" | "resumeWorkspace">;
  /** 顯示用的時區；省略時使用系統時區（spec D-33），只有測試會指定。 */
  timeZone?: string;
  /** 使用者選取某個 Workspace：點清單中的名稱，或剛建立完成（建立後直接選中）。 */
  onSelect?: (workspace: Workspace) => void;
  /** 目前選中的 Workspace id；以 aria-current 標示。 */
  selectedId?: string | null;
  /** 各 Workspace 的待裁定數（workspace id → 數量）；沒有的視為 0。 */
  pendingDecisions?: Record<string, number>;
  /** 每次讀到清單後告知上層目前使用中的 Workspace（例如封存後選中的 Workspace 已不在清單）。 */
  onWorkspaces?: (workspaces: Workspace[]) => void;
}

function reasonOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** 等待「繼續工作」時顯示的暫停原因；舊資料沒有原因時視為完全退出。 */
const PAUSE_LABELS: Record<PauseCause, string> = {
  full_exit: "完全退出後暫停中",
  new_login: "重新登入 Windows 後暫停中（未開啟跨登入自動續跑）",
  login_unverified: "無法核對是否跨登入，暫停中",
};

/** 左側 Workspace 清單與建立表單（spec D-21）。只列出使用中的 Workspace，並提供各自的完全退出後與跨登入自動續跑設定。 */
export function WorkspaceSidebar({ client, timeZone, onSelect, selectedId, pendingDecisions, onWorkspaces }: Props) {
  const [list, setList] = useState<ListState>({ state: "loading" });
  const [path, setPath] = useState("");
  const [name, setName] = useState("");
  const [formError, setFormError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  // 各 Workspace 的生命週期；讀不到時不顯示設定，不假裝是預設值
  const [lifecycles, setLifecycles] = useState<WorkspaceLifecycleView[]>([]);

  const refresh = useCallback(async () => {
    const lifecycle = client.lifecycleStatus().then(
      (status) => status.workspaces,
      () => [],
    );
    try {
      const { workspaces } = await client.listWorkspaces();
      setList({ state: "ready", workspaces });
      onWorkspaces?.(workspaces.filter((w) => w.status === "active"));
    } catch (error) {
      setList({ state: "failed", reason: reasonOf(error) });
    }
    setLifecycles(await lifecycle);
  }, [client, onWorkspaces]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const create = async (event: FormEvent) => {
    event.preventDefault();
    if (path.trim() === "") {
      setFormError("請輸入目錄");
      return;
    }
    setBusy(true);
    setFormError(null);
    try {
      const created = await client.createWorkspace({ path, name: name.trim() === "" ? undefined : name });
      setPath("");
      setName("");
      await refresh();
      onSelect?.(created);
    } catch (error) {
      // 保留輸入，讓使用者修正後重送
      setFormError(reasonOf(error));
    } finally {
      setBusy(false);
    }
  };

  const archive = async (workspace: Workspace) => {
    setFormError(null);
    try {
      await client.archiveWorkspace(workspace.id);
      await refresh();
    } catch (error) {
      setFormError(reasonOf(error));
    }
  };

  const setAutoResume = async (workspace: Workspace, autoResume: boolean) => {
    setFormError(null);
    try {
      const updated = await client.updateWorkspaceLifecycle(workspace.id, autoResume);
      setLifecycles((current) => current.map((view) => (view.workspace_id === updated.workspace_id ? updated : view)));
    } catch (error) {
      setFormError(reasonOf(error));
    }
  };

  const setLoginResume = async (workspace: Workspace, autoResume: boolean) => {
    setFormError(null);
    try {
      const updated = await client.updateWorkspaceLoginResume(workspace.id, autoResume);
      setLifecycles((current) => current.map((view) => (view.workspace_id === updated.workspace_id ? updated : view)));
    } catch (error) {
      setFormError(reasonOf(error));
    }
  };

  const resume = async (workspace: Workspace) => {
    setFormError(null);
    try {
      await client.resumeWorkspace(workspace.id);
      await refresh();
    } catch (error) {
      setFormError(reasonOf(error));
    }
  };

  /** 暫停狀態（依原因說明）與「繼續工作」，以及兩項各自獨立的自動續跑設定；沒有生命週期資料時不顯示。 */
  const lifecycleControls = (workspace: Workspace) => {
    const lifecycle = lifecycles.find((view) => view.workspace_id === workspace.id);
    if (!lifecycle) return null;
    return (
      <>
        {lifecycle.awaiting_resume && (
          <div className="workspace-paused">
            <span>{PAUSE_LABELS[lifecycle.paused_by ?? "full_exit"]}</span>
            <button type="button" aria-label={`繼續工作 ${workspace.name}`} onClick={() => void resume(workspace)}>
              繼續工作
            </button>
          </div>
        )}
        <label className="workspace-auto-resume">
          <input
            type="checkbox"
            aria-label={`完全退出後自動續跑 ${workspace.name}`}
            checked={lifecycle.auto_resume_after_exit}
            onChange={(e) => void setAutoResume(workspace, e.target.checked)}
          />
          完全退出後自動續跑
        </label>
        <span className="workspace-auto-resume-hint">只影響下次重新開啟，不會立即啟停目前工作</span>
        <label className="workspace-auto-resume">
          <input
            type="checkbox"
            aria-label={`跨登入自動續跑 ${workspace.name}`}
            checked={lifecycle.auto_resume_across_login}
            onChange={(e) => void setLoginResume(workspace, e.target.checked)}
          />
          跨登入自動續跑
        </label>
        <span className="workspace-auto-resume-hint">重新登入 Windows 後是否自動續跑；只影響之後的登入</span>
      </>
    );
  };

  const active = list.state === "ready" ? list.workspaces.filter((w) => w.status === "active") : [];

  return (
    <aside className="sidebar" aria-labelledby="workspaces-title">
      <h2 id="workspaces-title">Workspace</h2>
      {list.state === "loading" && <p>載入中…</p>}
      {list.state === "failed" && <p role="alert">無法取得 Workspace 清單：{list.reason}</p>}
      {list.state === "ready" && active.length === 0 && <p className="sidebar-empty">還沒有 Workspace</p>}
      {active.length > 0 && (
        <ul className="workspace-list">
          {active.map((workspace) => (
            <li key={workspace.id}>
              <div className="workspace-heading">
                <button
                  type="button"
                  className="workspace-select"
                  aria-label={`選擇 ${workspace.name}`}
                  aria-current={workspace.id === selectedId ? true : undefined}
                  onClick={() => onSelect?.(workspace)}
                >
                  <span className="workspace-name">{workspace.name}</span>
                </button>
                {(pendingDecisions?.[workspace.id] ?? 0) > 0 && (
                  <span className="decision-badge">待裁定 {pendingDecisions?.[workspace.id]}</span>
                )}
              </div>
              <span className="workspace-path">{workspace.path}</span>
              <span className="workspace-created">建立於 {formatTimestamp(workspace.created_at, timeZone)}</span>
              <div className="workspace-actions">
                <button type="button" aria-label={`封存 ${workspace.name}`} onClick={() => void archive(workspace)}>
                  封存
                </button>
              </div>
              {lifecycleControls(workspace)}
            </li>
          ))}
        </ul>
      )}
      <form className="workspace-form" onSubmit={(event) => void create(event)}>
        <label>
          目錄
          <input value={path} onChange={(e) => setPath(e.target.value)} placeholder="C:\work\my-folder" />
        </label>
        <label>
          名稱（選填）
          <input value={name} onChange={(e) => setName(e.target.value)} />
        </label>
        <button type="submit" disabled={busy}>
          建立 Workspace
        </button>
        {formError && <p role="alert">{formError}</p>}
      </form>
    </aside>
  );
}
