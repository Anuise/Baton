import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";

import { LongText, ShortValue } from "./LongText";
import { sizeLabel, splitExtension } from "./textFormat";
import { formatTimestamp } from "./time";
import type {
  ArtifactClient,
  ArtifactFileStatus,
  ArtifactList,
  ArtifactPreview,
  ArtifactVersionView,
  ArtifactView,
  TaskClient,
  UnregisteredFile,
  Workspace,
} from "./runtimeClient";

/** `taskTitles`：Task 識別碼 → 標題；讀不到 Task 清單時是空的，卡片退回顯示識別碼。 */
type ViewState =
  | { state: "loading" }
  | { state: "ready"; list: ArtifactList; taskTitles: Map<string, string> }
  | { state: "failed"; reason: string };

interface Props {
  /** 成果卡的 Task 標題另外從 Task 清單取得（ArtifactView 只有 task_id）。 */
  client: ArtifactClient & Pick<TaskClient, "getTaskBoard">;
  workspace: Workspace;
  /** 顯示用的時區；省略時使用系統時區（spec D-33），只有測試會指定。 */
  timeZone?: string;
}

/** 檔案現況不是 ok 時的警示；ok 不顯示。 */
const FILE_STATUS_WARNINGS: Record<Exclude<ArtifactFileStatus, "ok">, string> = {
  missing: "檔案遺失，不可信",
  size_mismatch: "檔案大小與登記不符，不可信",
};

const UNREGISTERED_KIND_LABELS: Record<UnregisteredFile["kind"], string> = {
  staging: "暫存檔，內容不完整",
  unregistered: "檔案完整但沒有登記",
};

function reasonOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/**
 * 成果（spec D-21）：列出 Workspace 的成果，預覽與下載每個版本。
 * 沒有 manifest 的檔案不是成果，獨立列在「未登記的檔案」。
 */
export function ArtifactsPage({ client, workspace, timeZone }: Props) {
  const [loaded, setLoaded] = useState<ViewState>({ state: "loading" });

  const load = useCallback(async () => {
    try {
      // Task 清單只用來顯示標題：讀取失敗不影響成果列表
      const [list, board] = await Promise.all([
        client.listArtifacts(workspace.id),
        client.getTaskBoard(workspace.id).catch(() => null),
      ]);
      const taskTitles = new Map((board?.tasks ?? []).map((task) => [task.id, task.title]));
      setLoaded({ state: "ready", list, taskTitles });
    } catch (e) {
      setLoaded({ state: "failed", reason: reasonOf(e) });
    }
  }, [client, workspace.id]);

  useEffect(() => {
    setLoaded({ state: "loading" });
    void load();
  }, [load]);

  if (loaded.state === "loading") return <section aria-label="成果">載入中…</section>;
  if (loaded.state === "failed") {
    return (
      <section aria-label="成果">
        <p role="alert">無法取得成果：{loaded.reason}</p>
      </section>
    );
  }
  const { artifacts, unregistered_files } = loaded.list;

  return (
    <section className="artifacts-page" aria-label="成果">
      <h2>成果</h2>
      {artifacts.length === 0 && <p>這個 Workspace 還沒有成果</p>}
      {artifacts.map((artifact) => (
        <ArtifactCard
          key={artifact.id}
          client={client}
          workspaceId={workspace.id}
          artifact={artifact}
          taskTitle={loaded.taskTitles.get(artifact.task_id)}
          timeZone={timeZone}
        />
      ))}
      {unregistered_files.length > 0 && (
        <section className="card stripe-warning artifact-unregistered" aria-label="未登記的檔案">
          <h3>未登記的檔案（{unregistered_files.length}）</h3>
          <p>
            這些檔案是寫入中途中斷留下、沒有登記（沒有 manifest）的檔案，不是成果：未登記、不可使用。
          </p>
          <ul>
            {unregistered_files.map((file) => (
              <li key={file.storage_path}>
                <ShortValue value={file.storage_path} kind="path" />
                <span className="artifact-meta">
                  <span className="artifact-size">{sizeLabel(file.size_bytes)}</span>・{UNREGISTERED_KIND_LABELS[file.kind]}
                </span>
              </li>
            ))}
          </ul>
        </section>
      )}
    </section>
  );
}

interface CardProps {
  client: ArtifactClient;
  workspaceId: string;
  artifact: ArtifactView;
  /** 所屬 Task 的標題；取不到時顯示縮短的識別碼。 */
  taskTitle?: string;
  timeZone?: string;
}

/**
 * 成果名稱：長檔名最多兩行，完整名稱在 title。超過兩行時改成「主檔名（兩行，以 … 截斷）＋副檔名」，截斷後仍看得到副檔名。
 * 分開後主檔名少了副檔名的寬度仍放得下時，代表整個名稱也放得下，改回不分開。
 */
function ArtifactName({ name }: { name: string }) {
  const ref = useRef<HTMLHeadingElement>(null);
  const [split, setSplit] = useState(false);
  const { base, ext } = splitExtension(name);

  useLayoutEffect(() => {
    const heading = ref.current;
    if (heading === null || ext === "") return;
    const measure = () => {
      const body = split ? heading.firstElementChild : heading;
      if (body !== null) setSplit(body.scrollHeight > body.clientHeight + 1);
    };
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(measure);
    observer.observe(heading);
    return () => observer.disconnect();
  }, [name, ext, split]);

  if (!split) {
    return (
      <h3 ref={ref} className="clamp-2" title={name}>
        {name}
      </h3>
    );
  }
  return (
    <h3 ref={ref} className="artifact-name-split" title={name}>
      <span className="clamp-2">{base}</span>
      <span className="artifact-name-ext">{ext}</span>
    </h3>
  );
}

/** 一個成果：預設只列最新版本，可展開全部版本（由新到舊）。 */
function ArtifactCard({ client, workspaceId, artifact, taskTitle, timeZone }: CardProps) {
  const [expanded, setExpanded] = useState(false);
  const newestFirst = [...artifact.versions].reverse();
  const shown = expanded ? newestFirst : newestFirst.slice(0, 1);

  return (
    <article className="card artifact-card" aria-label={artifact.name}>
      <header className="card-header">
        <ArtifactName name={artifact.name} />
        {artifact.versions.length > 1 && (
          <button type="button" className="btn-quiet btn-sm" aria-expanded={expanded} onClick={() => setExpanded(!expanded)}>
            全部版本（{artifact.versions.length}）
          </button>
        )}
      </header>
      {/* 長的 Task 標題最多兩行（完整標題在 title）；識別碼本來就縮短，不截行以免遮住複製按鈕的焦點框 */}
      <p className={taskTitle === undefined ? "artifact-task" : "artifact-task clamp-2"}>
        Task：
        {taskTitle === undefined ? (
          <ShortValue value={artifact.task_id} kind="id" field="Task" />
        ) : (
          <span title={taskTitle}>{taskTitle}</span>
        )}
      </p>
      <ul className="artifact-versions">
        {shown.map((version) => (
          <VersionRow
            key={version.version}
            client={client}
            workspaceId={workspaceId}
            artifact={artifact}
            version={version}
            latest={version === newestFirst[0]}
            timeZone={timeZone}
          />
        ))}
      </ul>
    </article>
  );
}

type PreviewState = { state: "loading" } | { state: "ready"; content: ArtifactPreview["preview"] } | { state: "failed"; reason: string };

interface RowProps {
  client: ArtifactClient;
  workspaceId: string;
  artifact: ArtifactView;
  version: ArtifactVersionView;
  latest: boolean;
  timeZone?: string;
}

function VersionRow({ client, workspaceId, artifact, version, latest, timeZone }: RowProps) {
  const label = `${artifact.name} v${version.version}`;
  const [preview, setPreview] = useState<PreviewState | null>(null);
  const [downloadError, setDownloadError] = useState<string | null>(null);

  const togglePreview = async () => {
    if (preview !== null) {
      setPreview(null);
      return;
    }
    setPreview({ state: "loading" });
    try {
      const { preview: content } = await client.previewArtifact(workspaceId, artifact.id, version.version);
      setPreview({ state: "ready", content });
    } catch (e) {
      setPreview({ state: "failed", reason: reasonOf(e) });
    }
  };

  const download = async () => {
    setDownloadError(null);
    try {
      const { blob, sha256 } = await client.downloadArtifact(workspaceId, artifact.id, version.version);
      // 內容與登記的雜湊不一致就不儲存
      if (sha256.toLowerCase() !== version.sha256.toLowerCase()) {
        throw new Error("下載內容的雜湊與登記不符，已拒絕儲存");
      }
      // 憑證只走 runtimeClient；這裡只把已取得的內容交給瀏覽器儲存
      const url = URL.createObjectURL(blob);
      const link = document.createElement("a");
      link.href = url;
      link.download = artifact.name;
      link.click();
      URL.revokeObjectURL(url);
    } catch (e) {
      setDownloadError(`無法下載：${reasonOf(e)}`);
    }
  };

  return (
    <li className={version.file_status === "ok" ? "artifact-version" : "artifact-version stripe-danger"} aria-label={label}>
      <p>
        <strong>v{version.version}</strong>
        {latest && "（最新）"}
        <span className="artifact-meta">
          ・{sizeLabel(version.size_bytes)}・{formatTimestamp(version.created_at, timeZone)}
        </span>
      </p>
      {version.file_status !== "ok" && (
        <p className="artifact-warning" role="alert">
          {FILE_STATUS_WARNINGS[version.file_status]}
        </p>
      )}
      <p>
        <span className="artifact-label">雜湊：</span>
        <ShortValue value={version.sha256} kind="hash" field={`v${version.version}`} />
      </p>
      <p>
        <span className="artifact-label">產生者 Run（第 {version.produced_by_generation} 次）：</span>
        <ShortValue value={version.produced_by_run_id} kind="id" field={`v${version.version} 產生者 Run`} />
      </p>
      {version.description !== "" && <LongText text={version.description} />}
      <div className="artifact-actions">
        <button type="button" aria-label={`預覽 ${label}`} aria-expanded={preview !== null} onClick={() => void togglePreview()}>
          預覽
        </button>
        <button type="button" aria-label={`下載 ${label}`} onClick={() => void download()}>
          下載
        </button>
      </div>
      {downloadError && <p role="alert">{downloadError}</p>}
      {preview?.state === "loading" && <p>載入預覽中…</p>}
      {preview?.state === "failed" && <p role="alert">無法預覽：{preview.reason}</p>}
      {preview?.state === "ready" && (
        <div className="artifact-preview" role="group" aria-label={`預覽 ${label}`}>
          {preview.content.kind === "text" ? (
            <>
              <pre>{preview.content.text}</pre>
              {preview.content.truncated && <p>內容過長，只顯示前段；完整內容請下載</p>}
            </>
          ) : (
            <p>非文字內容，請下載</p>
          )}
        </div>
      )}
    </li>
  );
}
