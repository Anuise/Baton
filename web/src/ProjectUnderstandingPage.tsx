import { useEffect, useState } from "react";

import { LongText } from "./LongText";
import { shortPath } from "./textFormat";
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
  hypothesis: "假說・未確認",
  unknown: "未知・待確認",
  retired: "已移除",
};

const PROJECTION_STATUS_LABELS: Record<ProjectionStatus, string> = {
  in_sync: "已同步",
  conflict: "外部已修改・待協調",
  detached: "保留外部版本・Baton 停止寫入",
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

const PROJECTION_CHANGED_NOTICE = "投影檔在檢視後又被修改，已重新檢查，請確認最新狀態後再協調";

function reasonOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** 協調中檔案被外部刪除時另外說明。 */
function projectionStatusLabel(projection: Projection): string {
  if (projection.status === "conflict" && projection.external_hash === null) return "外部已刪除・待協調";
  return PROJECTION_STATUS_LABELS[projection.status];
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

  return (
    <section className="understanding-page" aria-label="專案理解">
      <header className="page-header">
        <h2>專案理解</h2>
        <button type="button" onClick={recheck} disabled={busy || loaded.state === "loading"}>
          重新檢查
        </button>
      </header>
      {conflicts > 0 && (
        <p className="understanding-conflict" role="status">
          {conflicts} 個投影檔被外部修改，Baton 沒有覆寫，請協調
        </p>
      )}
      {notice && (
        <p className="understanding-notice" role="status">
          {notice}
        </p>
      )}
      {error && <p role="alert">{error}</p>}
      {loaded.state === "loading" && <p>載入中…</p>}
      {loaded.state === "failed" && <p role="alert">無法取得專案理解：{loaded.reason}</p>}
      {view && (
        <>
          {view.items.length === 0 ? (
            <p>Controller 尚未建立專案理解；首次接入專案時會建立 Project Brief、Glossary 與 Project Map。</p>
          ) : (
            <>
              <label className="understanding-retired-toggle">
                <input type="checkbox" checked={showRetired} onChange={(e) => setShowRetired(e.target.checked)} />
                顯示已移除
              </label>
              {SECTIONS.map(({ section, title }) => (
                <ItemSection
                  key={section}
                  title={title}
                  items={view.items.filter((i) => i.section === section && (showRetired || i.status !== "retired"))}
                  timeZone={timeZone}
                />
              ))}
            </>
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
    <section className="understanding-section" aria-labelledby={id}>
      <h3 id={id}>{title}</h3>
      {items.length === 0 && <p className="empty-note">這個區塊還沒有項目</p>}
      {items.length > 0 && (
        <ul className="understanding-items">
          {items.map((item) => (
            <li key={item.id} aria-label={item.key} className={`understanding-item understanding-${item.status}`}>
              {/* 名稱與狀態分開：名稱等寬，狀態是徽章（audit P-KN-5） */}
              <div className="understanding-item-header">
                <code className="understanding-key mono">{item.key}</code>
                <span className={`understanding-status ${STATUS_TONES[item.status]}`}>{STATUS_LABELS[item.status]}</span>
                {item.stale && <span className="understanding-stale badge badge-warning">來源已變更・需重新查驗</span>}
              </div>
              <LongText text={item.text} className="understanding-text" />
              {item.sources.length > 0 && (
                <ul className="understanding-sources" aria-label={`${item.key} 的來源`}>
                  {item.sources.map((source, index) => (
                    <li key={index}>
                      <SourceView source={source} timeZone={timeZone} />
                    </li>
                  ))}
                </ul>
              )}
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

/** 來源的位置、版本與查驗時間；檔案版本是 SHA-256，只顯示前 12 碼。 */
function SourceView({ source, timeZone }: { source: UnderstandingSource; timeZone?: string }) {
  const location =
    source.kind === "file" ? (source.line === null ? source.path : `${source.path}:${source.line}`) : source.reference;
  const version = source.kind === "file" ? source.version.slice(0, 12) : source.version;
  return (
    <>
      <span className="understanding-source-location mono" title={location}>
        {shortPath(location)}
      </span>
      <span className="understanding-source-version">・版本 {version}</span>
      <span className="understanding-source-checked">・查驗於 {formatTimestamp(source.checked_at, timeZone)}</span>
      {source.changed && <span className="understanding-source-changed">・已變更</span>}
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
    <section className="understanding-projections" aria-labelledby="understanding-projections-title">
      <h3 id="understanding-projections-title">repo 投影（GLOSSARY／ADR）</h3>
      {!view.projection_enabled && <p className="empty-note">這個 Workspace 不是 Git repo，不產生 GLOSSARY／ADR 投影</p>}
      {view.projection_enabled && view.projections.length === 0 && <p className="empty-note">還沒有投影檔</p>}
      {view.projection_enabled && view.projections.length > 0 && (
        <ul className="understanding-projection-list">
          {view.projections.map((projection) => (
            <li key={projection.path} aria-label={projection.path} className={`projection-${projection.status}`}>
              <div className="projection-header">
                <code className="projection-path mono">{projection.path}</code>
                <span className={`projection-status ${PROJECTION_TONES[projection.status]}`}>
                  {projectionStatusLabel(projection)}
                </span>
                {/* 第 0 版：接入前就已存在的檔案，Baton 還沒寫入過 */}
                <span className="projection-version badge">
                  {projection.version === 0 ? "Baton 尚未寫入" : `第 ${projection.version} 版`}
                </span>
              </div>
              {projection.status === "conflict" && projection.detected_at !== null && (
                <span className="projection-detected">偵測於 {formatTimestamp(projection.detected_at, timeZone)}</span>
              )}
              {projection.status !== "in_sync" && (
                <div className="projection-actions">
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
          ))}
        </ul>
      )}
    </section>
  );
}
