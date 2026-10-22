import { useCallback, useEffect, useState, type FormEvent } from "react";

import { formatTimestamp } from "./time";
import {
  hasErrorCode,
  type CorrectKnowledgeRequest,
  type KnowledgeClient,
  type KnowledgeContent,
  type KnowledgeDetail,
  type KnowledgeEntryView,
  type KnowledgeKind,
  type KnowledgeStatus,
  type LessonDetail,
  type Workspace,
} from "./runtimeClient";

interface Props {
  client: KnowledgeClient;
  workspace: Workspace;
  /** 顯示用的時區；省略時使用系統時區（spec D-33），只有測試會指定。 */
  timeZone?: string;
}

type ListState =
  | { state: "loading" }
  | { state: "ready"; degraded: boolean; effective: KnowledgeEntryView[]; all: KnowledgeEntryView[] }
  | { state: "failed"; reason: string };

/** 執行一個操作；成功或因衝突而重新整理時回傳 null，其餘失敗回傳要顯示的說明。 */
type Act = (operation: () => Promise<unknown>) => Promise<string | null>;

const KIND_LABELS: Record<KnowledgeKind, string> = { decision: "決策", lesson: "教訓", fact: "知識" };

const STATUS_LABELS: Record<KnowledgeStatus, string> = {
  candidate: "候選・尚未驗證",
  verified: "已驗證",
  revoked: "無效：已撤銷",
  retired: "無效：已淘汰",
  superseded: "無效：已被取代",
};

const EVENT_LABELS: Record<string, string> = {
  knowledge_created: "建立",
  knowledge_confirmed: "確認為已驗證",
  knowledge_corrected: "修正",
  knowledge_revoked: "撤銷",
  knowledge_retired: "淘汰",
  knowledge_superseded: "被取代",
};

const QUERY_TERM_MIN_CHARS = 2;
const CONFLICT_NOTICE = "條目已被修改，已重新整理；請確認最新內容後再操作";
const DEGRADED_NOTICE = "全文檢索無法使用，已改用逐筆比對（降級）：結果相同但較慢";

function reasonOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** 已驗證但不在有效期間內的條目分成「已過期」與「尚未生效」；其他依狀態。 */
function statusLabel(entry: KnowledgeEntryView): string {
  if (entry.status !== "verified" || entry.effective) return STATUS_LABELS[entry.status];
  return entry.valid_until !== null && entry.valid_until <= Date.now() ? "無效：已過期" : "無效：尚未生效";
}

function scopeLabel(entry: KnowledgeEntryView): string {
  const { scope } = entry;
  switch (scope.kind) {
    case "global":
      return "全域";
    case "workspace":
      return "Workspace";
    case "goal":
      return `Goal ${scope.goal_id ?? ""}`;
    case "task":
      return `Task ${scope.task_id ?? ""}`;
  }
}

function sourceLabel(entry: KnowledgeEntryView): string {
  const { source } = entry;
  return source.kind === "user" ? "使用者" : `Agent（Run ${source.run_id}，第 ${source.generation} 次，Task ${source.task_id}）`;
}

/** 搜尋前的檢查（與 Runtime 規則相同）：以空白分詞，每個詞至少 2 個字元。 */
function shortTerm(query: string): string | undefined {
  return query
    .split(/\s+/)
    .filter((term) => term !== "")
    .find((term) => [...term].length < QUERY_TERM_MIN_CHARS);
}

/**
 * 知識（spec D-21 主區分頁；D-26、D-28）：搜尋 Workspace 可見的知識、決策與教訓，
 * 確認或撤銷 Agent 提交的候選教訓，修正、撤銷、淘汰有效條目，並查看版本與變更歷史。
 */
export function KnowledgePage({ client, workspace, timeZone }: Props) {
  const [list, setList] = useState<ListState>({ state: "loading" });
  const [draftQuery, setDraftQuery] = useState("");
  const [query, setQuery] = useState("");
  const [queryError, setQueryError] = useState<string | null>(null);
  const [showHistory, setShowHistory] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [detailId, setDetailId] = useState<string | null>(null);
  // 操作後讓詳情重新讀取
  const [detailRefresh, setDetailRefresh] = useState(0);

  // 有效條目與「含無效」分兩次查詢：候選與歷史再多也不會擠掉有效條目（每次最多 100 筆）
  const load = useCallback(async () => {
    const base = { workspaceId: workspace.id, q: query === "" ? undefined : query };
    try {
      const [active, all] = await Promise.all([
        client.listKnowledge({ ...base, includeInactive: false }),
        client.listKnowledge({ ...base, includeInactive: true }),
      ]);
      setList({
        state: "ready",
        degraded: active.search_mode === "like_fallback" || all.search_mode === "like_fallback",
        effective: active.entries,
        all: all.entries,
      });
    } catch (e) {
      setList({ state: "failed", reason: reasonOf(e) });
    }
  }, [client, workspace.id, query]);

  useEffect(() => {
    void load();
  }, [load]);

  const act: Act = async (operation) => {
    setNotice(null);
    try {
      await operation();
    } catch (e) {
      if (!hasErrorCode(e, "knowledge_revision_conflict")) return reasonOf(e);
      setNotice(CONFLICT_NOTICE);
    }
    await load();
    setDetailRefresh((n) => n + 1);
    return null;
  };

  const search = (event: FormEvent) => {
    event.preventDefault();
    const short = shortTerm(draftQuery);
    if (short !== undefined) {
      setQueryError(`每個關鍵字至少 ${QUERY_TERM_MIN_CHARS} 個字元（「${short}」太短）`);
      return;
    }
    setQueryError(null);
    setQuery(draftQuery.trim());
  };

  const card = (entry: KnowledgeEntryView) => (
    <EntryCard key={entry.id} client={client} entry={entry} timeZone={timeZone} act={act} onOpenDetail={setDetailId} />
  );

  const candidates = list.state === "ready" ? list.all.filter((e) => e.status === "candidate") : [];
  const history = list.state === "ready" ? list.all.filter((e) => !e.effective && e.status !== "candidate") : [];

  return (
    <section className="knowledge-page" aria-label="知識">
      <h2>知識：{workspace.name}</h2>
      <form className="knowledge-search" role="search" onSubmit={search}>
        <input
          aria-label="搜尋關鍵字"
          placeholder="以空白分隔多個關鍵字，每個至少 2 個字"
          value={draftQuery}
          onChange={(e) => setDraftQuery(e.target.value)}
        />
        <button type="submit">搜尋</button>
      </form>
      {queryError && <p role="alert">{queryError}</p>}
      {notice && (
        <p className="knowledge-notice" role="status">
          {notice}
        </p>
      )}
      {list.state === "loading" && <p>載入中…</p>}
      {list.state === "failed" && <p role="alert">無法取得知識：{list.reason}</p>}
      {list.state === "ready" && (
        <>
          {list.degraded && (
            <p className="knowledge-degraded" role="status">
              {DEGRADED_NOTICE}
            </p>
          )}
          <section className="knowledge-section knowledge-candidates" aria-label="待確認的候選教訓">
            <h3>待確認的候選教訓（{candidates.length}）</h3>
            <p>Agent 提交的候選尚未驗證，確認後才會提供給後續的 Run 使用。</p>
            {candidates.length === 0 && <p>沒有待確認的候選</p>}
            {candidates.map(card)}
          </section>
          <section className="knowledge-section" aria-label="有效條目">
            <h3>有效條目（{list.effective.length}）</h3>
            {list.effective.length === 0 && <p>{query === "" ? "還沒有有效的條目" : "沒有符合搜尋的有效條目"}</p>}
            {list.effective.map(card)}
          </section>
          <label className="knowledge-history-toggle">
            <input type="checkbox" checked={showHistory} onChange={(e) => setShowHistory(e.target.checked)} />
            顯示歷史
          </label>
          {showHistory && (
            <section className="knowledge-section" aria-label="歷史條目">
              <h3>歷史條目（{history.length}）</h3>
              <p>已撤銷、已淘汰、已被取代或不在有效期間內的條目，不會提供給 Run 使用。</p>
              {history.length === 0 && <p>沒有歷史條目</p>}
              {history.map(card)}
            </section>
          )}
        </>
      )}
      {detailId !== null && (
        <DetailPanel
          key={detailId}
          client={client}
          id={detailId}
          refreshKey={detailRefresh}
          timeZone={timeZone}
          onClose={() => setDetailId(null)}
        />
      )}
      <CreateForm client={client} workspace={workspace} act={act} />
    </section>
  );
}

/** 表單中編輯的內容；標籤與證據先以文字編輯，送出時才拆成清單。 */
interface ContentDraft {
  title: string;
  body: string;
  tags: string;
  evidence: string;
  trigger: string;
  rootCause: string;
  resolution: string;
}

const EMPTY_DRAFT: ContentDraft = { title: "", body: "", tags: "", evidence: "", trigger: "", rootCause: "", resolution: "" };

function draftOf(content: KnowledgeContent): ContentDraft {
  return {
    title: content.title,
    body: content.body,
    tags: content.tags.join(", "),
    evidence: content.evidence.join("\n"),
    trigger: content.lesson?.trigger ?? "",
    rootCause: content.lesson?.root_cause ?? "",
    resolution: content.lesson?.resolution ?? "",
  };
}

const splitItems = (text: string, separator: RegExp) =>
  text
    .split(separator)
    .map((item) => item.trim())
    .filter((item) => item !== "");

/** 表單內容轉成請求欄位；教訓欄位只有教訓才帶。 */
function contentOf(draft: ContentDraft, kind: KnowledgeKind) {
  const lesson: LessonDetail | undefined =
    kind === "lesson" ? { trigger: draft.trigger, root_cause: draft.rootCause, resolution: draft.resolution } : undefined;
  return {
    title: draft.title.trim(),
    body: draft.body,
    // 標籤以逗號（含全形）或換行分隔；證據每行一項
    tags: splitItems(draft.tags, /[,，\n]/),
    evidence: splitItems(draft.evidence, /\n/),
    lesson,
  };
}

function ContentFields({ draft, onChange, lesson }: { draft: ContentDraft; onChange: (draft: ContentDraft) => void; lesson: boolean }) {
  const field = (key: keyof ContentDraft) => ({
    value: draft[key],
    onChange: (e: { target: { value: string } }) => onChange({ ...draft, [key]: e.target.value }),
  });
  return (
    <>
      <label>
        標題
        <input {...field("title")} />
      </label>
      <label>
        內文
        <textarea {...field("body")} />
      </label>
      {lesson && (
        <>
          <label>
            觸發條件
            <textarea {...field("trigger")} />
          </label>
          <label>
            根因或待驗假說
            <textarea {...field("rootCause")} />
          </label>
          <label>
            解法
            <textarea {...field("resolution")} />
          </label>
        </>
      )}
      <label>
        標籤（以逗號或換行分隔）
        <input {...field("tags")} />
      </label>
      <label>
        證據（每行一項）
        <textarea {...field("evidence")} />
      </label>
    </>
  );
}

function ContentView({ content }: { content: KnowledgeContent }) {
  return (
    <>
      {content.body !== "" && <p className="knowledge-body">{content.body}</p>}
      {content.lesson && (
        <dl className="knowledge-lesson">
          <dt>觸發條件</dt>
          <dd>{content.lesson.trigger || "（未填）"}</dd>
          <dt>根因或待驗假說</dt>
          <dd>{content.lesson.root_cause || "（未填）"}</dd>
          <dt>解法</dt>
          <dd>{content.lesson.resolution || "（未填）"}</dd>
        </dl>
      )}
      {content.tags.length > 0 && (
        <ul className="knowledge-tags" aria-label="標籤">
          {content.tags.map((tag) => (
            <li key={tag}>{tag}</li>
          ))}
        </ul>
      )}
      {content.evidence.length > 0 && (
        <div className="knowledge-evidence">
          證據：
          <ul>
            {content.evidence.map((item) => (
              <li key={item}>{item}</li>
            ))}
          </ul>
        </div>
      )}
    </>
  );
}

type CardMode = null | "correct" | "revoke" | "retire";

interface CardProps {
  client: KnowledgeClient;
  entry: KnowledgeEntryView;
  timeZone?: string;
  act: Act;
  onOpenDetail: (id: string) => void;
}

/** 一個條目；可用的操作依狀態：候選可確認或撤銷，已驗證可修正、撤銷或淘汰，其他只能看詳情。 */
function EntryCard({ client, entry, timeZone, act, onOpenDetail }: CardProps) {
  const [mode, setMode] = useState<CardMode>(null);
  const [error, setError] = useState<string | null>(null);
  const tone = entry.status === "candidate" ? "candidate" : entry.effective ? "effective" : "inactive";
  const fmt = (millis: number) => formatTimestamp(millis, timeZone);

  const run = async (operation: () => Promise<unknown>) => {
    setError(null);
    const failure = await act(operation);
    if (failure === null) setMode(null);
    else setError(failure);
  };

  return (
    <article className={`knowledge-card knowledge-${tone}`} aria-label={entry.content.title}>
      <h4>
        <span className="knowledge-badge">{statusLabel(entry)}</span>
        {entry.content.title}
      </h4>
      <p className="knowledge-meta">
        {KIND_LABELS[entry.kind]}・範圍：{scopeLabel(entry)}・v{entry.version}・有效期間：{fmt(entry.valid_from)} 起，
        {entry.valid_until === null ? "無期限" : `至 ${fmt(entry.valid_until)}`}
      </p>
      <p className="knowledge-meta">來源：{sourceLabel(entry)}</p>
      {entry.status_reason && <p>原因：{entry.status_reason}</p>}
      {entry.superseded_by && <p>已被 {entry.superseded_by} 取代</p>}
      <ContentView content={entry.content} />
      <div className="knowledge-actions">
        {entry.status === "candidate" && (
          <button type="button" onClick={() => void run(() => client.confirmKnowledge(entry.id, entry.revision))}>
            確認為已驗證
          </button>
        )}
        {entry.status === "verified" && (
          <button type="button" onClick={() => setMode("correct")}>
            修正
          </button>
        )}
        {(entry.status === "candidate" || entry.status === "verified") && (
          <button type="button" onClick={() => setMode("revoke")}>
            撤銷
          </button>
        )}
        {entry.status === "verified" && (
          <button type="button" onClick={() => setMode("retire")}>
            淘汰
          </button>
        )}
        <button type="button" onClick={() => onOpenDetail(entry.id)}>
          詳情
        </button>
      </div>
      {mode === "correct" && (
        <CorrectForm
          entry={entry}
          onCancel={() => setMode(null)}
          onSubmit={(request) => run(() => client.correctKnowledge(entry.id, request))}
        />
      )}
      {(mode === "revoke" || mode === "retire") && (
        <ReasonForm
          action={mode === "revoke" ? "撤銷" : "淘汰"}
          onCancel={() => setMode(null)}
          onSubmit={(reason) =>
            run(() =>
              (mode === "revoke" ? client.revokeKnowledge : client.retireKnowledge)(entry.id, {
                expected_revision: entry.revision,
                reason,
              }),
            )
          }
        />
      )}
      {error && <p role="alert">{error}</p>}
    </article>
  );
}

function ReasonForm({ action, onSubmit, onCancel }: { action: string; onSubmit: (reason: string) => Promise<void>; onCancel: () => void }) {
  const [reason, setReason] = useState("");
  const [error, setError] = useState<string | null>(null);
  const submit = (event: FormEvent) => {
    event.preventDefault();
    if (reason.trim() === "") {
      setError("請填寫原因");
      return;
    }
    void onSubmit(reason.trim());
  };
  return (
    <form className="knowledge-form" aria-label={action} onSubmit={submit}>
      <label>
        {action}原因
        <textarea value={reason} onChange={(e) => setReason(e.target.value)} />
      </label>
      <div className="knowledge-actions">
        <button type="submit">確認{action}</button>
        <button type="button" onClick={onCancel}>
          取消
        </button>
      </div>
      {error && <p role="alert">{error}</p>}
    </form>
  );
}

interface CorrectFormProps {
  entry: KnowledgeEntryView;
  onSubmit: (request: CorrectKnowledgeRequest) => Promise<void>;
  onCancel: () => void;
}

/** 修正：送出完整的新內容（追加新版本）；有效期限沿用原值。 */
function CorrectForm({ entry, onSubmit, onCancel }: CorrectFormProps) {
  const [draft, setDraft] = useState(() => draftOf(entry.content));
  const [note, setNote] = useState("");
  const [error, setError] = useState<string | null>(null);
  const submit = (event: FormEvent) => {
    event.preventDefault();
    if (draft.title.trim() === "") {
      setError("請輸入標題");
      return;
    }
    void onSubmit({
      expected_revision: entry.revision,
      ...contentOf(draft, entry.kind),
      valid_until: entry.valid_until ?? undefined,
      note: note.trim() === "" ? undefined : note.trim(),
    });
  };
  return (
    <form className="knowledge-form" aria-label="修正" onSubmit={submit}>
      <ContentFields draft={draft} onChange={setDraft} lesson={entry.kind === "lesson"} />
      <label>
        修正說明（選填）
        <input value={note} onChange={(e) => setNote(e.target.value)} />
      </label>
      <div className="knowledge-actions">
        <button type="submit">送出修正</button>
        <button type="button" onClick={onCancel}>
          取消
        </button>
      </div>
      {error && <p role="alert">{error}</p>}
    </form>
  );
}

interface CreateFormProps {
  client: KnowledgeClient;
  workspace: Workspace;
  act: Act;
}

/** 新增條目（使用者建立的直接是已驗證）；範圍只提供全域或此 Workspace。 */
function CreateForm({ client, workspace, act }: CreateFormProps) {
  const [kind, setKind] = useState<KnowledgeKind>("lesson");
  const [scope, setScope] = useState<"workspace" | "global">("workspace");
  const [draft, setDraft] = useState(EMPTY_DRAFT);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (draft.title.trim() === "") {
      setError("請輸入標題");
      return;
    }
    setBusy(true);
    setError(null);
    const failure = await act(() =>
      client.createKnowledge({
        kind,
        scope: scope === "global" ? { kind: "global" } : { kind: "workspace", workspace_id: workspace.id },
        ...contentOf(draft, kind),
      }),
    );
    setBusy(false);
    // 失敗時保留輸入，讓使用者修正後重送
    if (failure === null) setDraft(EMPTY_DRAFT);
    else setError(failure);
  };

  return (
    <form className="knowledge-form knowledge-create" aria-label="新增條目" onSubmit={(e) => void submit(e)}>
      <h3>新增條目</h3>
      <label>
        種類
        <select value={kind} onChange={(e) => setKind(e.target.value as KnowledgeKind)}>
          <option value="lesson">教訓</option>
          <option value="decision">決策</option>
          <option value="fact">知識</option>
        </select>
      </label>
      <label>
        範圍
        <select value={scope} onChange={(e) => setScope(e.target.value as "workspace" | "global")}>
          <option value="workspace">此 Workspace</option>
          <option value="global">全域</option>
        </select>
      </label>
      <ContentFields draft={draft} onChange={setDraft} lesson={kind === "lesson"} />
      <button type="submit" disabled={busy}>
        新增
      </button>
      {error && <p role="alert">{error}</p>}
    </form>
  );
}

type DetailState = { state: "loading" } | { state: "ready"; detail: KnowledgeDetail } | { state: "failed"; reason: string };

interface DetailProps {
  client: KnowledgeClient;
  id: string;
  refreshKey: number;
  timeZone?: string;
  onClose: () => void;
}

/** 條目詳情：全部內容版本（由舊到新）與變更歷史（由舊到新）。 */
function DetailPanel({ client, id, refreshKey, timeZone, onClose }: DetailProps) {
  const [loaded, setLoaded] = useState<DetailState>({ state: "loading" });
  const fmt = (millis: number) => formatTimestamp(millis, timeZone);

  useEffect(() => {
    let cancelled = false;
    client.knowledgeDetail(id).then(
      (detail) => !cancelled && setLoaded({ state: "ready", detail }),
      (e: unknown) => !cancelled && setLoaded({ state: "failed", reason: reasonOf(e) }),
    );
    return () => {
      cancelled = true;
    };
  }, [client, id, refreshKey]);

  return (
    <section className="knowledge-section knowledge-detail" aria-label="條目詳情">
      <button type="button" onClick={onClose}>
        關閉詳情
      </button>
      {loaded.state === "loading" && <p>載入中…</p>}
      {loaded.state === "failed" && <p role="alert">無法取得詳情：{loaded.reason}</p>}
      {loaded.state === "ready" && (
        <>
          <h3>詳情：{loaded.detail.entry.content.title}</h3>
          <h4>版本（由舊到新）</h4>
          <ol className="knowledge-versions">
            {[...loaded.detail.versions]
              .sort((a, b) => a.version - b.version)
              .map((version) => (
                <li key={version.version} aria-label={`v${version.version}`}>
                  <p>
                    <strong>v{version.version}</strong>，{fmt(version.created_at)}
                    {version.note && `，修正說明：${version.note}`}
                  </p>
                  <p>
                    <strong>{version.content.title}</strong>
                  </p>
                  <ContentView content={version.content} />
                </li>
              ))}
          </ol>
          <h4>變更歷史</h4>
          <ol className="knowledge-history">
            {loaded.detail.history.map((event) => {
              const payload = (event.payload ?? {}) as { reason?: unknown; superseded_by?: unknown };
              return (
                <li key={event.revision}>
                  {fmt(event.occurred_at)} {EVENT_LABELS[event.event_type] ?? event.event_type}
                  {typeof payload.reason === "string" && `：${payload.reason}`}
                  {typeof payload.superseded_by === "string" && `（由 ${payload.superseded_by} 取代）`}
                </li>
              );
            })}
          </ol>
        </>
      )}
    </section>
  );
}
