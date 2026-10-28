import { useCallback, useEffect, useRef, useState, type FormEvent, type ReactNode } from "react";

import { HelpTip } from "./HelpTip";
import { CopyButton } from "./LongText";
import { formatCount, formatNumber, shortPath } from "./textFormat";
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

/** 建立表單「目錄」欄位的 id；主區的空狀態以「建立 Workspace」把焦點移到這裡。 */
export const WORKSPACE_PATH_INPUT_ID = "workspace-path-input";

/** 左側 Workspace 清單與建立表單（spec D-21）。只列出使用中的 Workspace，並提供各自的完全退出後與跨登入自動續跑設定。 */
export function WorkspaceSidebar({ client, timeZone, onSelect, selectedId, pendingDecisions, onWorkspaces }: Props) {
  const [list, setList] = useState<ListState>({ state: "loading" });
  const [path, setPath] = useState("");
  const [name, setName] = useState("");
  const [formError, setFormError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  // 各 Workspace 的生命週期；讀不到時不顯示設定，不假裝是預設值
  const [lifecycles, setLifecycles] = useState<WorkspaceLifecycleView[]>([]);
  const pathId = WORKSPACE_PATH_INPUT_ID;
  // 系統的資料夾選擇對話框只有在 Tauri 視窗中可用；瀏覽器開發模式只能手動輸入
  const canBrowse = "__TAURI_INTERNALS__" in window;

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

  /** 開啟系統的資料夾選擇對話框；選定後填入目錄，取消時不變。 */
  const browse = async () => {
    setFormError(null);
    try {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const selected = await open({ directory: true, multiple: false });
      if (selected !== null) setPath(selected);
    } catch (error) {
      setFormError(reasonOf(error));
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

  /** 暫停狀態（依原因說明）與「繼續工作」：需要立即處理，留在清單項目內，一行。沒有生命週期資料時不顯示。 */
  const pausedRow = (workspace: Workspace) => {
    const lifecycle = lifecycles.find((view) => view.workspace_id === workspace.id);
    if (!lifecycle?.awaiting_resume) return null;
    const label = PAUSE_LABELS[lifecycle.paused_by ?? "full_exit"];
    return (
      <div className="workspace-paused">
        <span className="truncate" title={label}>
          {label}
        </span>
        <button type="button" className="btn-sm" aria-label={`繼續工作 ${workspace.name}`} onClick={() => void resume(workspace)}>
          繼續工作
        </button>
      </div>
    );
  };

  /** 「管理」選單：建立時間、封存，以及兩項各自獨立的自動續跑設定（沒有生命週期資料時不顯示設定）。 */
  const manageMenu = (workspace: Workspace) => {
    const lifecycle = lifecycles.find((view) => view.workspace_id === workspace.id);
    return (
      <WorkspaceMenu label={`管理 ${workspace.name}`}>
        <span className="workspace-created">建立於 {formatTimestamp(workspace.created_at, timeZone)}</span>
        {/* 完整路徑：清單只放縮短形式，這裡是不靠 title 的完整取得方式 */}
        <div className="workspace-menu-path">
          <span className="mono">{workspace.path}</span>
          <CopyButton text={workspace.path} label="複製路徑" field={workspace.name} />
        </div>
        {lifecycle && (
          <>
            <div className="workspace-auto-resume">
              <label>
                <input
                  type="checkbox"
                  aria-label={`完全退出後自動續跑 ${workspace.name}`}
                  checked={lifecycle.auto_resume_after_exit}
                  onChange={(e) => void setAutoResume(workspace, e.target.checked)}
                />
                完全退出後自動續跑
              </label>
              <HelpTip label="完全退出後自動續跑">只影響下次重新開啟，不會立即啟停目前工作</HelpTip>
            </div>
            <div className="workspace-auto-resume">
              <label>
                <input
                  type="checkbox"
                  aria-label={`跨登入自動續跑 ${workspace.name}`}
                  checked={lifecycle.auto_resume_across_login}
                  onChange={(e) => void setLoginResume(workspace, e.target.checked)}
                />
                跨登入自動續跑
              </label>
              <HelpTip label="跨登入自動續跑">重新登入 Windows 後是否自動續跑；只影響之後的登入</HelpTip>
            </div>
          </>
        )}
        <button type="button" className="btn-danger btn-sm" aria-label={`封存 ${workspace.name}`} onClick={() => void archive(workspace)}>
          封存
        </button>
      </WorkspaceMenu>
    );
  };

  const active = list.state === "ready" ? list.workspaces.filter((w) => w.status === "active") : [];
  const paths = distinctPaths(active.map((w) => w.path));

  return (
    <aside className="sidebar" aria-labelledby="workspaces-title">
      <h2 id="workspaces-title">Workspace</h2>
      {list.state === "loading" && <p>載入中…</p>}
      {list.state === "failed" && <p role="alert">無法取得 Workspace 清單：{list.reason}</p>}
      {list.state === "ready" && active.length === 0 && <p className="sidebar-empty">還沒有 Workspace</p>}
      {active.length > 0 && (
        <ul className="workspace-list">
          {active.map((workspace, index) => (
            <li key={workspace.id} className={`workspace-item${workspace.id === selectedId ? " workspace-item-current" : ""}`}>
              <div className="workspace-heading">
                {/* 名稱最多 2 行；整個項目都可點選（按鈕的 ::after 蓋滿項目），完整名稱與路徑在 title */}
                <button
                  type="button"
                  className="workspace-select"
                  aria-label={`選擇 ${workspace.name}`}
                  aria-current={workspace.id === selectedId ? true : undefined}
                  title={`${workspace.name}
${workspace.path}`}
                  onClick={() => onSelect?.(workspace)}
                >
                  <span className="workspace-name clamp-2">{workspace.name}</span>
                </button>
                {manageMenu(workspace)}
              </div>
              <div className="workspace-meta">
                {pendingBadge(pendingDecisions?.[workspace.id] ?? 0)}
                {/* 窄時先省略開頭，能和其他 Workspace 區分的結尾最後才截斷；完整路徑在 title 與「管理」選單 */}
                <span className="workspace-path mono" title={workspace.path}>
                  {paths[index].head !== "" && <span className="workspace-path-head">{paths[index].head}</span>}
                  <span className="workspace-path-tail">{paths[index].tail}</span>
                </span>
              </div>
              {pausedRow(workspace)}
            </li>
          ))}
        </ul>
      )}
      <form className="workspace-form" onSubmit={(event) => void create(event)}>
        <div className="workspace-path-field">
          <label htmlFor={pathId}>目錄</label>
          <div className="workspace-path-row">
            <input
              id={pathId}
              value={path}
              onChange={(e) => setPath(e.target.value)}
              placeholder="C:\work\my-folder"
            />
            {canBrowse && (
              <button type="button" disabled={busy} onClick={() => void browse()}>
                瀏覽…
              </button>
            )}
          </div>
        </div>
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

/** 路徑的段落（含開頭的磁碟）。 */
function segmentsOf(path: string): string[] {
  return path.split(/[\\/]/).filter((segment) => segment !== "");
}

/**
 * 側欄的縮短路徑：結尾保留足以和清單中其他 Workspace 區分的最少段數，整體至少保留結尾 2 段（同 shortPath）；
 * 分成開頭（窄時先省略）與結尾（能區分的部分，最後才截斷）。比較不分大小寫（Windows 路徑）；
 * 完全相同的路徑無從區分，只保留 1 段當結尾。
 */
function distinctPaths(paths: string[]): { head: string; tail: string }[] {
  const keys = paths.map((path) => segmentsOf(path).map((segment) => segment.toLowerCase()));
  const suffix = (segments: string[], count: number) => (segments.length < count ? null : segments.slice(-count).join("\\"));
  return paths.map((path, i) => {
    const own = keys[i];
    const others = keys.filter((other, j) => j !== i && other.join("\\") !== own.join("\\"));
    let count = 1;
    while (count < own.length && others.some((other) => suffix(other, count) === suffix(own, count))) count++;
    const sep = path.includes("\\") ? "\\" : "/";
    const tail = segmentsOf(path).slice(-count).join(sep);
    const short = shortPath(path, Math.max(2, count));
    return short.endsWith(tail) ? { head: short.slice(0, short.length - tail.length), tail } : { head: "", tail: short };
  });
}

/** 待裁定數徽章：超過 999 顯示 999+，完整數字在可存取名稱與 title。 */
function pendingBadge(count: number) {
  if (count <= 0) return null;
  return (
    <span className="badge badge-warning workspace-badge" aria-label={`待裁定 ${formatNumber(count)} 項`} title={`待裁定 ${formatNumber(count)}`}>
      待裁定 {formatCount(count)}
    </span>
  );
}

/** 清單項目的「管理」選單（`<details>` 浮層）：Esc 或點選單外面時收起，Esc 後焦點回到觸發按鈕。 */
function WorkspaceMenu({ label, children }: { label: string; children: ReactNode }) {
  const ref = useRef<HTMLDetailsElement>(null);
  const [open, setOpen] = useState(false);

  useEffect(() => {
    if (!open) return;
    const close = (event: PointerEvent) => {
      if (ref.current && !ref.current.contains(event.target as Node)) ref.current.open = false;
    };
    document.addEventListener("pointerdown", close);
    return () => document.removeEventListener("pointerdown", close);
  }, [open]);

  // Esc 常駐監聽在 document：點浮層空白處後焦點落到 body，按鍵事件不會經過 details；
  // 不等 toggle 事件更新 open 後才掛上，開啟後立刻按 Esc 也收得起來
  useEffect(() => {
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || !ref.current?.open) return;
      ref.current.open = false;
      ref.current.querySelector("summary")?.focus();
    };
    document.addEventListener("keydown", closeOnEscape);
    return () => document.removeEventListener("keydown", closeOnEscape);
  }, []);

  return (
    <details ref={ref} className="workspace-menu" onToggle={(event) => setOpen(event.currentTarget.open)}>
      <summary className="workspace-menu-trigger" aria-label={label} title="管理">
        <span aria-hidden="true">⋯</span>
      </summary>
      <div className="workspace-menu-panel">{children}</div>
    </details>
  );
}
