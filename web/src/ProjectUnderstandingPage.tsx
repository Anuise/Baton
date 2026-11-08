import { useEffect, useState } from "react";

import { LongText, ShortValue } from "./LongText";
import { formatTimestamp } from "./time";
import {
  errorCode,
  type Projection,
  type ProjectionStatus,
  type ReconcileProjectionRequest,
  type UnderstandingClient,
  type UnderstandingItemView,
  type UnderstandingSection,
  type UnderstandingSource,
  type UnderstandingStatus,
  type UnderstandingView,
  type Workspace,
} from "./runtimeClient";

type Loaded = { state: "loading" } | { state: "ready"; view: UnderstandingView } | { state: "failed"; reason: string };

interface Props {
  client: UnderstandingClient;
  workspace: Workspace;
  /** 顯示用的時區；省略時使用系統時區（spec D-33），只有測試會指定。 */
  timeZone?: string;
}

const SECTIONS: { section: UnderstandingSection; title: string }[] = [
  { section: "brief", title: "Project Brief" },
  { section: "glossary", title: "Glossary" },
  { section: "map", title: "Project Map" },
];

const STATUS_LABELS: Record<UnderstandingStatus, string> = {
  confirmed: "已確認",
  hypothesis: "假說，未確認",
  unknown: "未知，待確認",
  retired: "已移除",
};

/** 投影狀態的徽章文字（短）；完整說明在徽章下方一行。 */
const PROJECTION_STATUS_LABELS: Record<ProjectionStatus, string> = {
  in_sync: "已同步",
  conflict: "待協調",
  detached: "已保留外部版本",
};

/** 項目狀態徽章的語意色：已確認為成功、假說與未知要人確認、已移除為中性。 */
const STATUS_TONES: Record<UnderstandingStatus, string> = {
  confirmed: "badge badge-success",
  hypothesis: "badge badge-warning",
  unknown: "badge badge-warning",
  retired: "badge",
};

const PROJECTION_TONES: Record<ProjectionStatus, string> = {
  in_sync: "badge badge-success",
  conflict: "badge badge-warning",
  detached: "badge",
};

const PROJECTION_CHANGED_NOTICE = "檔案在檢視後又被修改，已重新檢查，請確認最新狀態後再協調";

/**
 * Project Brief 段落的中文名稱。key 是 Controller 寫的自由文字（MCP 工具只說明 Brief 有哪幾段：
 * 目標、使用者、成功條件、排除範圍、現況、外部整合、風險與限制），這裡收常見的英文寫法；不認得的照原樣顯示。
 */
const BRIEF_KEY_LABELS: Record<string, string> = {
  purpose: "產品目標",
  goal: "產品目標",
  goals: "產品目標",
  product_goal: "產品目標",
  product_goals: "產品目標",
  objective: "產品目標",
  objectives: "產品目標",
  users: "使用者",
  user: "使用者",
  audience: "使用者",
  target_users: "使用者",
  success_criteria: "成功條件",
  success: "成功條件",
  non_goals: "排除範圍",
  out_of_scope: "排除範圍",
  exclusions: "排除範圍",
  current_state: "系統現況",
  status_quo: "系統現況",
  integrations: "外部整合",
  external_integrations: "外部整合",
  risks: "風險與限制",
  constraints: "風險與限制",
  risks_and_constraints: "風險與限制",
  limitations: "風險與限制",
};

/** Brief 段落的中文名稱；其他文件的 key 本身就是名詞或模組名稱，不轉換。 */
function itemLabel(item: UnderstandingItemView): string | null {
  if (item.section !== "brief") return null;
  return BRIEF_KEY_LABELS[item.key.trim().toLowerCase().replace(/[\s-]+/g, "_")] ?? null;
}

/**
 * 沒有中文名稱對照、寫成英文代碼的 Brief 鍵（只含 ASCII）：標題就是原始鍵，依 MASTER 7.1 機器代碼以等寬次要色顯示。
 * 用中文寫的鍵本身就是名稱，照一般標題顯示。
 */
function isUnlabeledBriefCode(item: UnderstandingItemView): boolean {
  return item.section === "brief" && itemLabel(item) === null && /^[\x20-\x7e]+$/.test(item.key);
}

function reasonOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** 徽章下方的一句說明：發生什麼、Baton 做了什麼；協調中檔案被外部刪除時另外說明。 */
function projectionNote(projection: Projection): string | null {
  switch (projection.status) {
    case "in_sync":
      return null;
    case "conflict":
      return projection.external_hash === null ? "檔案已在外部被刪除，Baton 沒有重建" : "檔案已在外部被修改，Baton 沒有覆寫";
    case "detached":
      return "Baton 停止寫入這個檔案；需要時可以再改回以 Baton 版本覆寫";
  }
}

/**
 * 專案理解（票 42）：唯讀檢視 Controller 建立的 Project Brief、Glossary 與 Project Map 及其來源，
 * 並協調被外部修改的 repo 投影（GLOSSARY.md、docs/adr/）。開啟與「重新檢查」都會先重新比對投影檔。
 */
export function ProjectUnderstandingPage({ client, workspace, timeZone }: Props) {
  const [loaded, setLoaded] = useState<Loaded>({ state: "loading" });
  const [showRetired, setShowRetired] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  useEffect(() => {
    // 卸載後遲到的回應一律忽略
    let cancelled = false;
    client.syncUnderstanding(workspace.id).then(
      (view) => !cancelled && setLoaded({ state: "ready", view }),
      (e: unknown) => !cancelled && setLoaded({ state: "failed", reason: reasonOf(e) }),
    );
    return () => {
      cancelled = true;
    };
  }, [client, workspace.id]);

  /** 執行操作並以回傳的視圖取代目前內容；失敗時保留目前內容，只顯示說明。 */
  const run = async (operation: () => Promise<UnderstandingView>) => {
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      setLoaded({ state: "ready", view: await operation() });
    } catch (e) {
      if (errorCode(e) !== "projection_changed") {
        setError(reasonOf(e));
      } else {
        // 使用者看到的外部版本已過時：重新比對後讓使用者再決定
        setNotice(PROJECTION_CHANGED_NOTICE);
        try {
          setLoaded({ state: "ready", view: await client.syncUnderstanding(workspace.id) });
        } catch (syncError) {
          setError(reasonOf(syncError));
        }
      }
    } finally {
      setBusy(false);
    }
  };

  const recheck = () => void run(() => client.syncUnderstanding(workspace.id));
  const reconcile = (request: ReconcileProjectionRequest) =>
    void run(() => client.reconcileProjection(workspace.id, request));

  const view = loaded.state === "ready" ? loaded.view : null;
  const conflicts = view?.projections.filter((p) => p.status === "conflict").length ?? 0;

  // 版面：Project Brief → Glossary → Project Map（Controller 的理解，唯讀）→ repo 投影（需要你協調的在這裡）。
  // 投影被外部修改時，頁首先以一行提示告訴你往下協調
  return (
    <section className="page understanding-page" aria-label="專案理解">
      <header className="page-header">
        <h2>專案理解</h2>
        <div className="page-actions">
          {view !== null && view.items.length > 0 && (
            <label className="understanding-retired-toggle">
              <input type="checkbox" checked={showRetired} onChange={(e) => setShowRetired(e.target.checked)} />
              顯示已移除
            </label>
          )}
          <button type="button" onClick={recheck} disabled={busy || loaded.state === "loading"}>
            重新檢查
          </button>
        </div>
      </header>
      {conflicts > 0 && (
        <div className="callout callout-warning" role="status">
          <p>{conflicts} 個寫入 repo 的檔案被外部修改，Baton 沒有覆寫，請在下方協調</p>
        </div>
      )}
      {notice && (
        <div className="callout callout-warning" role="status">
          <p>{notice}</p>
        </div>
      )}
      {error && (
        <p className="field-error" role="alert">
          {error}
        </p>
      )}
      {loaded.state === "loading" && <p className="empty-note">載入中…</p>}
      {loaded.state === "failed" && (
        <p className="field-error" role="alert">
          無法取得專案理解：{loaded.reason}
        </p>
      )}
      {view && (
        <>
          {view.items.length === 0 ? (
            <div className="empty-state understanding-empty">
              <h3>Controller 尚未建立專案理解</h3>
              <p>首次接入專案時，Controller 會建立 Project Brief、Glossary 與 Project Map。</p>
            </div>
          ) : (
            SECTIONS.map(({ section, title }) => (
              <ItemSection
                key={section}
                title={title}
                items={view.items.filter((i) => i.section === section && (showRetired || i.status !== "retired"))}
                timeZone={timeZone}
              />
            ))
          )}
          <ProjectionSection view={view} busy={busy} timeZone={timeZone} onReconcile={reconcile} />
        </>
      )}
    </section>
  );
}

function ItemSection({ title, items, timeZone }: { title: string; items: UnderstandingItemView[]; timeZone?: string }) {
  const id = `understanding-${title.replace(/\s/g, "-").toLowerCase()}`;
  return (
    <section className="section" aria-labelledby={id}>
      <div className="section-heading">
        <h3 id={id}>{title}</h3>
        <span className="count">{items.length}</span>
      </div>
      {items.length === 0 && (
        <p className="empty-note">還沒有項目；Controller 接入專案時會建立，也可以在 Controller 中要求它補上</p>
      )}
      {items.length > 0 && (
        <ul className="row-list">
          {items.map((item) => {
            // Brief 段落以中文名稱當標題，Controller 寫的原始鍵以等寬次要色附在旁邊（MASTER 7.1 機器代碼）；
            // 沒有對照的英文代碼鍵直接當標題，但同樣等寬、次要色、不加粗
            const label = itemLabel(item);
            const keyClass = isUnlabeledBriefCode(item) ? "understanding-key understanding-key-code mono" : "understanding-key";
            const name = label === null ? item.key : `${label}（${item.key}）`;
            return (
              <li key={item.id} aria-label={name} className={`understanding-item understanding-${item.status}`}>
                {/* 名稱在左、狀態徽章在右；需要重新查驗的另加一個徽章 */}
                <div className="row-head">
                  <span className={keyClass}>
                    {label ?? item.key}
                    {label !== null && <span className="understanding-raw-key mono">{item.key}</span>}
                  </span>
                  <span className={STATUS_TONES[item.status]}>{STATUS_LABELS[item.status]}</span>
                  {item.stale && (
                    <span className="badge badge-warning" title="來源在查驗後改變">
                      來源已變更，需重新查驗
                    </span>
                  )}
                </div>
                <LongText text={item.text} className="understanding-text" />
                {item.sources.length > 0 && (
                  <ul className="understanding-sources" aria-label={`${name} 的來源`}>
                    {item.sources.map((source, index) => (
                      <li key={index} className="meta">
                        <SourceView source={source} timeZone={timeZone} />
                      </li>
                    ))}
                  </ul>
                )}
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}

/** 來源的位置、版本與查驗時間，各自一段（窄時以段為單位換行）；檔案版本是 SHA-256，只顯示前 12 碼。 */
function SourceView({ source, timeZone }: { source: UnderstandingSource; timeZone?: string }) {
  const location =
    source.kind === "file" ? (source.line === null ? source.path : `${source.path}:${source.line}`) : source.reference;
  const version = source.kind === "file" ? source.version.slice(0, 12) : source.version;
  return (
    <>
      <span className="understanding-source-location">
        <ShortValue value={location} kind="path" />
      </span>
      <span>
        版本 <span className="mono">{version}</span>
      </span>
      <span className="tabular">查驗於 {formatTimestamp(source.checked_at, timeZone)}</span>
      {source.changed && <span className="understanding-source-changed">已變更</span>}
    </>
  );
}

function ProjectionSection({
  view,
  busy,
  timeZone,
  onReconcile,
}: {
  view: UnderstandingView;
  busy: boolean;
  timeZone?: string;
  onReconcile: (request: ReconcileProjectionRequest) => void;
}) {
  return (
    <section className="section understanding-projections" aria-labelledby="understanding-projections-title">
      <div className="section-heading">
        <h3 id="understanding-projections-title">寫入 repo 的檔案（GLOSSARY.md、ADR）</h3>
        <p>
          Baton 把 Glossary 與決策寫進 repo 的 <span className="mono">GLOSSARY.md</span> 與{" "}
          <span className="mono">docs/adr/</span>；檔案在外部被改動時不會覆寫，等你決定。
        </p>
      </div>
      {!view.projection_enabled && (
        <p className="empty-note">這個 Workspace 不是 Git repo，Baton 不會寫入 GLOSSARY.md 與 ADR</p>
      )}
      {view.projection_enabled && view.projections.length === 0 && <p className="empty-note">Baton 還沒有寫入任何檔案</p>}
      {view.projection_enabled && view.projections.length > 0 && (
        <ul className="row-list">
          {view.projections.map((projection) => {
            const note = projectionNote(projection);
            return (
              <li
                key={projection.path}
                aria-label={projection.path}
                className={`projection-${projection.status}${projection.status === "conflict" ? " stripe-warning" : ""}`}
              >
                <div className="row-head">
                  <code className="projection-path mono">{projection.path}</code>
                  <span className={PROJECTION_TONES[projection.status]}>{PROJECTION_STATUS_LABELS[projection.status]}</span>
                  {/* 第 0 版：接入前就已存在的檔案，Baton 還沒寫入過 */}
                  <span className="badge badge-plain">
                    {projection.version === 0 ? "Baton 尚未寫入" : `第 ${projection.version} 版`}
                  </span>
                </div>
                {note !== null && (
                  <p className="meta">
                    <span>{note}</span>
                    {projection.status === "conflict" && projection.detected_at !== null && (
                      <span className="tabular">偵測於 {formatTimestamp(projection.detected_at, timeZone)}</span>
                    )}
                  </p>
                )}
                {projection.status !== "in_sync" && (
                  <div className="form-actions">
                    <button
                      type="button"
                      disabled={busy}
                      onClick={() =>
                        onReconcile({
                          path: projection.path,
                          action: "overwrite",
                          // 送使用者看到的外部版本（協調中與已保留外部版本都由最後一次比對記在 external_hash）
                          expected_external_hash: projection.external_hash,
                        })
                      }
                    >
                      以 Baton 版本覆寫
                    </button>
                    {projection.status === "conflict" && (
                      <button
                        type="button"
                        disabled={busy}
                        onClick={() =>
                          onReconcile({
                            path: projection.path,
                            action: "keep_external",
                            expected_external_hash: projection.external_hash,
                          })
                        }
                      >
                        保留外部版本
                      </button>
                    )}
                  </div>
                )}
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}
