import { useCallback, useEffect, useState, type FormEvent } from "react";

import { formatTimestamp } from "./time";
import type { Workspace, WorkspaceClient } from "./runtimeClient";

type ListState = { state: "loading" } | { state: "ready"; workspaces: Workspace[] } | { state: "failed"; reason: string };

interface Props {
  client: WorkspaceClient;
  /** 顯示用的時區；省略時使用系統時區（spec D-33），只有測試會指定。 */
  timeZone?: string;
  /** 使用者要設定某個 Workspace 的授權：點「授權設定」，或剛建立完成（建立後直接進入授權設定）。 */
  onSelect?: (workspace: Workspace) => void;
  /** 使用者要開啟某個 Workspace 的工作面板。 */
  onOpenTasks?: (workspace: Workspace) => void;
  /** 使用者要開啟某個 Workspace 的成果與驗證。 */
  onOpenArtifacts?: (workspace: Workspace) => void;
  /** 使用者要開啟某個 Workspace 的知識。 */
  onOpenKnowledge?: (workspace: Workspace) => void;
}

function reasonOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** 左側 Workspace 清單與建立表單（spec D-21）。只列出使用中的 Workspace。 */
export function WorkspaceSidebar({ client, timeZone, onSelect, onOpenTasks, onOpenArtifacts, onOpenKnowledge }: Props) {
  const [list, setList] = useState<ListState>({ state: "loading" });
  const [path, setPath] = useState("");
  const [name, setName] = useState("");
  const [formError, setFormError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(async () => {
    try {
      const { workspaces } = await client.listWorkspaces();
      setList({ state: "ready", workspaces });
    } catch (error) {
      setList({ state: "failed", reason: reasonOf(error) });
    }
  }, [client]);

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
              <span className="workspace-name">{workspace.name}</span>
              <span className="workspace-path">{workspace.path}</span>
              <span className="workspace-created">建立於 {formatTimestamp(workspace.created_at, timeZone)}</span>
              <div className="workspace-actions">
                <button type="button" aria-label={`授權設定 ${workspace.name}`} onClick={() => onSelect?.(workspace)}>
                  授權設定
                </button>
                <button type="button" aria-label={`工作面板 ${workspace.name}`} onClick={() => onOpenTasks?.(workspace)}>
                  工作面板
                </button>
                <button type="button" aria-label={`成果與驗證 ${workspace.name}`} onClick={() => onOpenArtifacts?.(workspace)}>
                  成果與驗證
                </button>
                <button type="button" aria-label={`知識 ${workspace.name}`} onClick={() => onOpenKnowledge?.(workspace)}>
                  知識
                </button>
                <button type="button" aria-label={`封存 ${workspace.name}`} onClick={() => void archive(workspace)}>
                  封存
                </button>
              </div>
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
