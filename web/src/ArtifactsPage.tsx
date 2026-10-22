import { useCallback, useEffect, useState } from "react";

import { formatTimestamp } from "./time";
import type {
  ArtifactClient,
  ArtifactFileStatus,
  ArtifactList,
  ArtifactPreview,
  ArtifactVersionView,
  ArtifactView,
  UnregisteredFile,
  Workspace,
} from "./runtimeClient";

type ViewState = { state: "loading" } | { state: "ready"; list: ArtifactList } | { state: "failed"; reason: string };

interface Props {
  client: ArtifactClient;
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
 * 成果與驗證（spec D-21）：列出 Workspace 的成果，預覽與下載每個版本。
 * 沒有 manifest 的檔案不是成果，獨立列在「未登記的檔案」。
 */
export function ArtifactsPage({ client, workspace, timeZone }: Props) {
  const [loaded, setLoaded] = useState<ViewState>({ state: "loading" });

  const load = useCallback(async () => {
    try {
      setLoaded({ state: "ready", list: await client.listArtifacts(workspace.id) });
    } catch (e) {
      setLoaded({ state: "failed", reason: reasonOf(e) });
    }
  }, [client, workspace.id]);

  useEffect(() => {
    setLoaded({ state: "loading" });
    void load();
  }, [load]);

  if (loaded.state === "loading") return <section aria-label="成果與驗證">載入中…</section>;
  if (loaded.state === "failed") {
    return (
      <section aria-label="成果與驗證">
        <p role="alert">無法取得成果：{loaded.reason}</p>
      </section>
    );
  }
  const { artifacts, unregistered_files } = loaded.list;

  return (
    <section className="artifacts-page" aria-label="成果與驗證">
      <h2>成果與驗證：{workspace.name}</h2>
      {artifacts.length === 0 && <p>這個 Workspace 還沒有成果</p>}
      {artifacts.map((artifact) => (
        <ArtifactCard key={artifact.id} client={client} workspaceId={workspace.id} artifact={artifact} timeZone={timeZone} />
      ))}
      {unregistered_files.length > 0 && (
        <section className="artifact-unregistered" aria-label="未登記的檔案">
          <h3>未登記的檔案（{unregistered_files.length}）</h3>
          <p>
            這些檔案是寫入中途中斷留下、沒有登記（沒有 manifest）的檔案，不是成果：未登記、不可使用。
          </p>
          <ul>
            {unregistered_files.map((file) => (
              <li key={file.storage_path}>
                <span className="artifact-path">{file.storage_path}</span>，{file.size_bytes} 位元組，
                {UNREGISTERED_KIND_LABELS[file.kind]}
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
  timeZone?: string;
}

/** 一個成果：預設只列最新版本，可展開全部版本（由新到舊）。 */
function ArtifactCard({ client, workspaceId, artifact, timeZone }: CardProps) {
  const [expanded, setExpanded] = useState(false);
  const newestFirst = [...artifact.versions].reverse();
  const shown = expanded ? newestFirst : newestFirst.slice(0, 1);

  return (
    <article className="artifact-card" aria-label={artifact.name}>
      <h3>{artifact.name}</h3>
      <p className="artifact-task">Task：{artifact.task_id}</p>
      {artifact.versions.length > 1 && (
        <button type="button" aria-expanded={expanded} onClick={() => setExpanded(!expanded)}>
          全部版本（{artifact.versions.length}）
        </button>
      )}
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
      <p>
        <strong>v{version.version}</strong>
        {latest && "（最新）"}，{version.size_bytes} 位元組，{formatTimestamp(version.created_at, timeZone)}
      </p>
      {version.file_status !== "ok" && (
        <p className="artifact-warning" role="alert">
          {FILE_STATUS_WARNINGS[version.file_status]}
        </p>
      )}
      <p>
        雜湊：<code className="artifact-sha">{version.sha256}</code>
      </p>
      <p>
        產生者 Run：{version.produced_by_run_id}（第 {version.produced_by_generation} 次）
      </p>
      {version.description !== "" && <p>{version.description}</p>}
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
