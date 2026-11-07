import { useCallback, useEffect, useId, useRef, useState, type FormEvent, type ReactNode } from "react";

import { HelpTip } from "./HelpTip";
import { useDetailsDismiss } from "./MoreMenu";
import { CopyButton, PathText } from "./LongText";
import { formatCount, formatNumber } from "./textFormat";
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
  /** 選中的 Workspace 的頁面導覽；放在整份清單之後（App 提供）。 */
  currentNav?: ReactNode;
  /** 值改變（大於 0）時展開建立表單並聚焦目錄欄位（主區空狀態的「新增 Workspace」）。 */
  createRequest?: number;
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

/** 建立表單「目錄」欄位的 id。 */
export const WORKSPACE_PATH_INPUT_ID = "workspace-path-input";

/**
 * 跑道欄上的 Workspace 清單與建立表單（spec D-21）。只列出使用中的 Workspace，
 * 清單之後放選中的 Workspace 的頁面（`currentNav`）；各自的完全退出後與跨登入自動續跑設定收在「管理」。
 * 建立表單平時收起，按「新增」展開；還沒有任何 Workspace 時直接展開。
 */
export function WorkspaceSidebar({
  client,
  timeZone,
  onSelect,
  selectedId,
  pendingDecisions,
  onWorkspaces,
  currentNav,
  createRequest = 0,
}: Props) {
  const [list, setList] = useState<ListState>({ state: "loading" });
  const [path, setPath] = useState("");
  const [name, setName] = useState("");
  const [formError, setFormError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  // 各 Workspace 的生命週期；讀不到時不顯示設定，不假裝是預設值
  const [lifecycles, setLifecycles] = useState<WorkspaceLifecycleView[]>([]);
  const pathId = WORKSPACE_PATH_INPUT_ID;
  const formId = useId();
  // null：使用者還沒動過，依清單是否為空決定；之後依使用者的選擇
  const [formChoice, setFormChoice] = useState<boolean | null>(null);
  const pathRef = useRef<HTMLInputElement>(null);
  const [focusPath, setFocusPath] = useState(false);
  // 正在確認封存的 Workspace id
  const [confirmArchive, setConfirmArchive] = useState<string | null>(null);
  // 建立完成後要聚焦的 Workspace id（等它出現在清單中）
  const [focusId, setFocusId] = useState<string | null>(null);
  const listRef = useRef<HTMLUListElement>(null);
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
      setFormChoice(false);
      await refresh();
      onSelect?.(created);
      // 表單收起後焦點不落到 body：移到新 Workspace 的選擇按鈕（WCAG 2.4.3）
      setFocusId(created.id);
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
    // 封存後這一項（連同聚焦中的按鈕）會消失：焦點移到相鄰的 Workspace（優先下一個），沒有其他項目時移到目錄欄位
    const remaining = active.filter((item) => item.id !== workspace.id);
    const index = active.findIndex((item) => item.id === workspace.id);
    const neighbor = remaining[Math.min(index, remaining.length - 1)] ?? null;
    try {
      await client.archiveWorkspace(workspace.id);
      setConfirmArchive(null);
      // 回到「依清單決定」：封存最後一個 Workspace 後建立表單自動展開
      setFormChoice(null);
      await refresh();
      if (neighbor) setFocusId(neighbor.id);
      else setFocusPath(true);
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
      <WorkspaceMenu label={`管理 ${workspace.name}`} onClose={() => setConfirmArchive((id) => (id === workspace.id ? null : id))}>
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
        {/* 封存要再確認一次：封存後從清單移除、無法在 App 中復原 */}
        {confirmArchive === workspace.id ? (
          <div className="workspace-archive-confirm" role="group" aria-label={`確認封存 ${workspace.name}`}>
            <p>封存後這個 Workspace 會從清單移除，無法在 App 中復原；資料與目錄保留。同一個目錄可以重新新增，但會是新的 Workspace。</p>
            <div className="workspace-archive-actions">
              <button
                type="button"
                className="btn-danger btn-solid btn-sm"
                aria-label={`確認封存 ${workspace.name}`}
                onClick={() => void archive(workspace)}
              >
                確認封存
              </button>
              <button type="button" className="btn-quiet btn-sm" onClick={() => setConfirmArchive(null)}>
                取消
              </button>
            </div>
          </div>
        ) : (
          <button type="button" className="btn-danger btn-sm" aria-label={`封存 ${workspace.name}`} onClick={() => setConfirmArchive(workspace.id)}>
            封存…
          </button>
        )}
      </WorkspaceMenu>
    );
  };

  const active = list.state === "ready" ? list.workspaces.filter((w) => w.status === "active") : [];
  const keeps = distinctKeeps(active.map((w) => w.path));
  const formOpen = formChoice ?? (list.state === "ready" && active.length === 0);

  // 主區空狀態要求建立：展開表單，等表單出現後再聚焦目錄欄位
  useEffect(() => {
    if (createRequest <= 0) return;
    setFormChoice(true);
    setFocusPath(true);
  }, [createRequest]);

  useEffect(() => {
    if (!focusPath || !formOpen) return;
    pathRef.current?.focus();
    setFocusPath(false);
  }, [focusPath, formOpen]);

  useEffect(() => {
    if (focusId === null) return;
    const button = Array.from(listRef.current?.querySelectorAll<HTMLButtonElement>("[data-workspace-id]") ?? []).find(
      (element) => element.dataset.workspaceId === focusId,
    );
    if (!button) return;
    button.focus();
    setFocusId(null);
  });

  return (
    <aside className="rail-workspaces" aria-labelledby="workspaces-title">
      <div className="rail-section-head">
        <h2 id="workspaces-title">Workspace</h2>
        <button
          type="button"
          className="rail-add"
          aria-label="新增 Workspace"
          aria-expanded={formOpen}
          // 表單收起時不在 DOM 中，只在展開時指向它
          aria-controls={formOpen ? formId : undefined}
          onClick={() => setFormChoice(!formOpen)}
        >
          新增
        </button>
      </div>
      {list.state === "loading" && <p className="rail-note">載入中…</p>}
      {list.state === "failed" && (
        <p className="rail-note" role="alert">
          無法取得 Workspace 清單：{list.reason}
        </p>
      )}
      {list.state === "ready" && active.length === 0 && <p className="rail-note sidebar-empty">還沒有 Workspace</p>}
      {active.length > 0 && (
        <ul className="workspace-list" ref={listRef}>
          {active.map((workspace, index) => {
            const current = workspace.id === selectedId;
            return (
              <li key={workspace.id} className={`workspace-item${current ? " workspace-item-current" : ""}`}>
                <div className="workspace-heading">
                  {/* 名稱最多 2 行；整列都可點選（按鈕的 ::after 蓋滿標題列），完整名稱與路徑在 title */}
                  <button
                    type="button"
                    className="workspace-select"
                    data-workspace-id={workspace.id}
                    aria-label={`選擇 ${workspace.name}`}
                    aria-current={current ? true : undefined}
                    title={`${workspace.name}
${workspace.path}`}
                    onClick={() => onSelect?.(workspace)}
                  >
                    <span className="workspace-name clamp-2">{workspace.name}</span>
                  </button>
                  {manageMenu(workspace)}
                </div>
                {/* 路徑與待裁定數同一行：名稱拿滿寬，不被徽章擠窄 */}
                <div className="workspace-meta">
                  {/* 窄時只截斷中段，開頭與結尾一定顯示；完整路徑在 title 與「管理」選單 */}
                  <PathText path={workspace.path} keep={keeps[index]} className="workspace-path" />
                  {pendingBadge(pendingDecisions?.[workspace.id] ?? 0)}
                </div>
                {pausedRow(workspace)}
              </li>
            );
          })}
        </ul>
      )}
      {formOpen && (
        <form id={formId} className="workspace-form" aria-label="新增 Workspace" onSubmit={(event) => void create(event)}>
          <div className="workspace-path-field">
            <label htmlFor={pathId}>目錄</label>
            <div className="workspace-path-row">
              <input
                id={pathId}
                ref={pathRef}
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
      )}
      {!formOpen && formError && (
        <p className="rail-note" role="alert">
          {formError}
        </p>
      )}
      {/* 選中的 Workspace 的頁面放在整份清單之後：其他 Workspace 與它們的待裁定數永遠在上方看得到 */}
      {selectedId !== null && active.some((workspace) => workspace.id === selectedId) && currentNav}
    </aside>
  );
}

/** 路徑的段落（含開頭的磁碟）。 */
function segmentsOf(path: string): string[] {
  return path.split(/[\\/]/).filter((segment) => segment !== "");
}

/**
 * 側欄縮短路徑保留的結尾段數：足以和清單中其他 Workspace 區分的最少段數，至少 2 段（同 shortPath）。
 * 比較不分大小寫（Windows 路徑）；完全相同的路徑無從區分，只算 1 段。
 */
function distinctKeeps(paths: string[]): number[] {
  const keys = paths.map((path) => segmentsOf(path).map((segment) => segment.toLowerCase()));
  const suffix = (segments: string[], count: number) => (segments.length < count ? null : segments.slice(-count).join("\\"));
  return keys.map((own) => {
    const others = keys.filter((other) => other.join("\\") !== own.join("\\"));
    let count = 1;
    while (count < own.length && others.some((other) => suffix(other, count) === suffix(own, count))) count++;
    return Math.max(2, count);
  });
}

/** 待裁定數徽章：超過 999 顯示 999+，完整數字在 title。 */
function pendingBadge(count: number) {
  if (count <= 0) return null;
  return (
    // 看得到的文字就是說明（span 沒有角色，不能用 aria-label 命名）；超過 999 時完整數字在 title
    <span className="workspace-badge" title={`待裁定 ${formatNumber(count)}`}>
      待裁定 {formatCount(count)}
    </span>
  );
}

/**
 * 清單項目的「管理」選單（`<details>` 浮層）：Esc、點選單外面或 Tab 離開時收起，Esc 後焦點回到觸發按鈕
 * （useDetailsDismiss）。展開時若浮層落在跑道欄可視範圍外（例如最後一個 Workspace），把它捲進來。
 */
function WorkspaceMenu({ label, onClose, children }: { label: string; onClose?: () => void; children: ReactNode }) {
  const ref = useRef<HTMLDetailsElement>(null);
  const dismiss = useDetailsDismiss(ref, onClose);
  const onToggle = (event: React.SyntheticEvent<HTMLDetailsElement>) => {
    dismiss(event);
    if (event.currentTarget.open) {
      event.currentTarget.querySelector(".workspace-menu-panel")?.scrollIntoView?.({ block: "nearest" });
    }
  };
  return (
    <details ref={ref} className="workspace-menu" onToggle={onToggle}>
      <summary className="workspace-menu-trigger" aria-label={label} title="管理">
        <span aria-hidden="true">⋯</span>
      </summary>
      <div className="workspace-menu-panel">{children}</div>
    </details>
  );
}
