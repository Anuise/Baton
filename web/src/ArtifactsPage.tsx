import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";

import { CopyButton, LongText, ShortValue } from "./LongText";
import { shortId, sizeLabel, splitExtension } from "./textFormat";
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

/** `taskTitles`：Task 識別碼 → 標題；讀不到 Task 清單時是空的，分組標題退回顯示識別碼。 */
type ViewState =
  | { state: "loading" }
  | { state: "ready"; list: ArtifactList; taskTitles: Map<string, string> }
  | { state: "failed"; reason: string };

interface Props {
  /** 分組的 Task 標題另外從 Task 清單取得（ArtifactView 只有 task_id）。 */
  client: ArtifactClient & Pick<TaskClient, "getTaskBoard">;
  workspace: Workspace;
  /** 顯示用的時區；省略時使用系統時區（spec D-33），只有測試會指定。 */
  timeZone?: string;
  /** 前往其他頁面（分組標題旁的「在工作頁查看」）；沒有傳入時不顯示。 */
  onOpenPage?: (page: "tasks") => void;
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

/** 依所屬 Task 分組，保留成果清單原本的順序（分組依第一次出現的位置）。 */
function groupByTask(artifacts: ArtifactView[]): { taskId: string; artifacts: ArtifactView[] }[] {
  const groups = new Map<string, ArtifactView[]>();
  for (const artifact of artifacts) {
    const group = groups.get(artifact.task_id);
    if (group === undefined) groups.set(artifact.task_id, [artifact]);
    else group.push(artifact);
  }
  return [...groups].map(([taskId, items]) => ({ taskId, artifacts: items }));
}

/**
 * 成果（spec D-21）：列出 Workspace 的成果，依所屬 Task 分組，預覽與下載每個版本。
 * 沒有 manifest 的檔案不是成果，獨立列在最上方的「未登記的檔案」。
 */
export function ArtifactsPage({ client, workspace, timeZone, onOpenPage }: Props) {
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

  return (
    <section className="page artifacts-page" aria-label="成果">
      <header className="page-header">
        <h2>成果</h2>
      </header>
      {loaded.state === "loading" && <p className="empty-note">載入中…</p>}
      {loaded.state === "failed" && (
        <div className="notice notice-danger" role="alert">
          <p>無法取得成果：{loaded.reason}</p>
        </div>
      )}
      {loaded.state === "ready" && (
        <>
          {loaded.list.unregistered_files.length > 0 && <UnregisteredFiles files={loaded.list.unregistered_files} />}
          {loaded.list.artifacts.length === 0 ? (
            <div className="empty-state">
              <h3>這個 Workspace 還沒有成果</h3>
              <p>Task 執行時登記的成果會依 Task 列在這裡，可以預覽與下載每個版本。</p>
            </div>
          ) : (
            groupByTask(loaded.list.artifacts).map((group) => (
              <TaskGroup
                key={group.taskId}
                client={client}
                workspaceId={workspace.id}
                taskId={group.taskId}
                taskTitle={loaded.taskTitles.get(group.taskId)}
                artifacts={group.artifacts}
                timeZone={timeZone}
                onOpenPage={onOpenPage}
              />
            ))
          )}
        </>
      )}
    </section>
  );
}

/** 未登記的檔案：寫入中途中斷留下、沒有 manifest 的檔案；不是成果，沒有預覽與下載。 */
function UnregisteredFiles({ files }: { files: UnregisteredFile[] }) {
  return (
    <section className="section" aria-labelledby="artifacts-unregistered">
      <div className="section-heading">
        <h3 id="artifacts-unregistered">未登記的檔案</h3>
        <span className="count">{files.length}</span>
        <p>寫入中途中斷留下、沒有登記到成果清單的檔案；它們不是成果，不可使用。</p>
      </div>
      <ul className="row-list artifacts-unregistered">
        {files.map((file) => (
          <li key={file.storage_path}>
            <ShortValue value={file.storage_path} kind="path" />
            <p className="meta">
              <span className="nowrap">{sizeLabel(file.size_bytes)}</span>
              <span>{UNREGISTERED_KIND_LABELS[file.kind]}</span>
            </p>
          </li>
        ))}
      </ul>
    </section>
  );
}

interface GroupProps {
  client: ArtifactClient;
  workspaceId: string;
  taskId: string;
  /** 所屬 Task 的標題；取不到時顯示縮短的識別碼。 */
  taskTitle?: string;
  artifacts: ArtifactView[];
  timeZone?: string;
  onOpenPage?: (page: "tasks") => void;
}

/**
 * 一個 Task 的成果：區段標題是 Task 標題（長標題最多兩行，完整標題在 title），計數與「在工作頁查看」接在標題同一行的末端；
 * 下面是成果清單。
 */
function TaskGroup({ client, workspaceId, taskId, taskTitle, artifacts, timeZone, onOpenPage }: GroupProps) {
  const headingId = `artifacts-task-${taskId}`;
  const short = shortId(taskId);
  return (
    <section className="section" aria-labelledby={headingId}>
      <div className="section-heading artifacts-task-heading">
        {taskTitle === undefined ? (
          <>
            <h3 id={headingId}>
              <code className="mono" title={taskId}>
                {short}
              </code>
            </h3>
            {short !== taskId && <CopyButton text={taskId} label="複製識別碼" field="Task" />}
          </>
        ) : (
          <h3 id={headingId} className="clamp-2" title={taskTitle}>
            {taskTitle}
          </h3>
        )}
        <span className="count">{artifacts.length}</span>
        {onOpenPage !== undefined && (
          <button
            type="button"
            className="btn-quiet btn-sm"
            aria-label={`在工作頁查看（${taskTitle ?? short}）`}
            onClick={() => onOpenPage("tasks")}
          >
            在工作頁查看
          </button>
        )}
      </div>
      <ul className="row-list">
        {artifacts.map((artifact) => (
          <li key={artifact.id}>
            <ArtifactItem client={client} workspaceId={workspaceId} artifact={artifact} timeZone={timeZone} />
          </li>
        ))}
      </ul>
    </section>
  );
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
      <h4 ref={ref} className="clamp-2" title={name}>
        {name}
      </h4>
    );
  }
  return (
    <h4 ref={ref} className="artifact-name-split" title={name}>
      <span className="clamp-2">{base}</span>
      <span className="artifact-name-ext">{ext}</span>
    </h4>
  );
}

interface ItemProps {
  client: ArtifactClient;
  workspaceId: string;
  artifact: ArtifactView;
  timeZone?: string;
}

/** 一個成果：預設只列最新版本，可展開全部版本（由新到舊）。 */
function ArtifactItem({ client, workspaceId, artifact, timeZone }: ItemProps) {
  const [expanded, setExpanded] = useState(false);
  const newestFirst = [...artifact.versions].reverse();
  const shown = expanded ? newestFirst : newestFirst.slice(0, 1);

  return (
    <article className="artifact" aria-label={artifact.name}>
      <header className="row-head">
        <ArtifactName name={artifact.name} />
        {artifact.versions.length > 1 && (
          <button type="button" className="btn-quiet btn-sm" aria-expanded={expanded} onClick={() => setExpanded(!expanded)}>
            全部版本 <span className="count">{artifact.versions.length}</span>
          </button>
        )}
      </header>
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
    <li className="artifact-version" aria-label={label}>
      <div className="artifact-version-head">
        <p className="artifact-version-name">
          <strong>v{version.version}</strong>
          {latest && <span className="badge badge-plain">最新</span>}
        </p>
        <div className="artifact-actions">
          <button
            type="button"
            className="btn-sm"
            aria-label={`預覽 ${label}`}
            aria-expanded={preview !== null}
            onClick={() => void togglePreview()}
          >
            預覽
          </button>
          <button type="button" className="btn-sm" aria-label={`下載 ${label}`} onClick={() => void download()}>
            下載
          </button>
        </div>
      </div>
      {version.file_status !== "ok" && (
        <div className="notice notice-danger" role="alert">
          <p>{FILE_STATUS_WARNINGS[version.file_status]}</p>
        </div>
      )}
      <p className="meta">
        <span className="nowrap">{sizeLabel(version.size_bytes)}</span>
        <span className="nowrap">{formatTimestamp(version.created_at, timeZone)}</span>
        <span>
          產生者 Run（第 {version.produced_by_generation} 次）{" "}
          <ShortValue value={version.produced_by_run_id} kind="id" field={`v${version.version} 產生者 Run`} />
        </span>
        <span>
          雜湊 <ShortValue value={version.sha256} kind="hash" field={`v${version.version}`} />
        </span>
      </p>
      {version.description !== "" && <LongText text={version.description} />}
      {downloadError && (
        <div className="notice notice-danger" role="alert">
          <p>{downloadError}</p>
        </div>
      )}
      {preview?.state === "loading" && <p className="empty-note">載入預覽中…</p>}
      {preview?.state === "failed" && (
        <div className="notice notice-danger" role="alert">
          <p>無法預覽：{preview.reason}</p>
        </div>
      )}
      {preview?.state === "ready" && (
        <div className="artifact-preview" role="group" aria-label={`預覽 ${label}`}>
          {preview.content.kind === "text" ? (
            <>
              <pre>{preview.content.text}</pre>
              {preview.content.truncated && <p className="field-hint">內容過長，只顯示前段；完整內容請下載</p>}
            </>
          ) : (
            <p className="empty-note">非文字內容，請下載</p>
          )}
        </div>
      )}
    </li>
  );
}
