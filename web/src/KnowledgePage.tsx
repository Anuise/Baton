import { useCallback, useEffect, useState, type FormEvent } from "react";

import { FIELD_LABELS } from "./ContractEditor";
import { formatTimestamp } from "./time";
import {
  CONTRACT_FIELDS,
  errorCode,
  type Capability,
  type ContractFieldName,
  type CorrectKnowledgeRequest,
  type GuardClient,
  type GuardRule,
  type GuardView,
  type KnowledgeClient,
  type KnowledgeContent,
  type KnowledgeDetail,
  type KnowledgeEntryView,
  type KnowledgeKind,
  type KnowledgeStatus,
  type LessonDetail,
  type Recurrence,
  type Workspace,
} from "./runtimeClient";

type Client = KnowledgeClient & GuardClient;

interface Props {
  client: Client;
  workspace: Workspace;
  /** 顯示用的時區；省略時使用系統時區（spec D-33），只有測試會指定。 */
  timeZone?: string;
}

type ListState =
  | { state: "loading" }
  | { state: "ready"; degraded: boolean; effective: KnowledgeEntryView[]; all: KnowledgeEntryView[] }
  | { state: "failed"; reason: string };

type GuardsState = { state: "loading" } | { state: "ready"; guards: GuardView[] } | { state: "failed"; reason: string };

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

/** 各規則種類的名稱。 */
const RULE_KIND_LABELS: Record<GuardRule["kind"], string> = {
  preflight: "啟動前檢查",
  tool_restriction: "工具限制",
  retry_policy: "重試政策",
};

/** 能力名稱（與 Runtime 的 Capability::label 相同）。 */
const CAPABILITY_LABELS: Record<Capability, string> = {
  read_files: "讀取檔案",
  write_files: "編輯檔案",
  web_read: "讀取公開網頁",
  run_build_test: "建置與測試",
  install_project_dependencies: "安裝專案依賴",
  admin_install: "管理員安裝",
  global_install: "全域安裝",
  remote_write: "遠端寫入",
  deploy: "部署",
  external_message: "對外發訊",
  payment: "付費",
  production_data: "正式資料操作",
};

const RECURRENCE_KIND_LABELS: Record<Recurrence["kind"], string> = { prevented: "已攔下", reported: "回報" };

const QUERY_TERM_MIN_CHARS = 2;
const RETRY_LIMIT_MIN = 1;
const RETRY_LIMIT_MAX = 20;
const CONFLICT_NOTICE = "條目已被修改，已重新整理；請確認最新內容後再操作";
/** 版本衝突時改為提示並重新整理，不當成錯誤。 */
const CONFLICT_NOTICES: Record<string, string> = {
  knowledge_revision_conflict: CONFLICT_NOTICE,
  guard_revision_conflict: "Guard 已被修改，已重新整理；請確認最新狀態後再操作",
};
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
  switch (source.kind) {
    case "user":
      return "使用者";
    case "agent":
      return `Agent（Run ${source.run_id}，第 ${source.generation} 次，Task ${source.task_id}）`;
    case "controller":
      return `Controller（Run ${source.controller_run_id}，Task ${source.task_id}）`;
  }
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
 * 有效的已驗證教訓可轉為 Guard；Guard 區塊列出生效狀態與成效，教訓詳情列出重犯紀錄。
 */
export function KnowledgePage({ client, workspace, timeZone }: Props) {
  const [list, setList] = useState<ListState>({ state: "loading" });
  const [guards, setGuards] = useState<GuardsState>({ state: "loading" });
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

  const loadGuards = useCallback(async () => {
    try {
      const { guards } = await client.listGuards(workspace.id);
      setGuards({ state: "ready", guards });
    } catch (e) {
      setGuards({ state: "failed", reason: reasonOf(e) });
    }
  }, [client, workspace.id]);

  useEffect(() => {
    void loadGuards();
  }, [loadGuards]);

  // 知識與 Guard 互相影響（撤銷教訓會讓 Guard 不生效、回報重犯會改變成效），所以操作後兩者都重新讀取
  const act: Act = async (operation) => {
    setNotice(null);
    try {
      await operation();
    } catch (e) {
      const conflict = CONFLICT_NOTICES[errorCode(e) ?? ""];
      if (conflict === undefined) return reasonOf(e);
      setNotice(conflict);
    }
    await Promise.all([load(), loadGuards()]);
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
          <GuardSection guards={guards} client={client} timeZone={timeZone} act={act} />
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
          workspace={workspace}
          refreshKey={detailRefresh}
          timeZone={timeZone}
          act={act}
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

type CardMode = null | "correct" | "revoke" | "retire" | "guard";

interface CardProps {
  client: Client;
  entry: KnowledgeEntryView;
  timeZone?: string;
  act: Act;
  onOpenDetail: (id: string) => void;
}

/** 一個條目；可用的操作依狀態：候選可確認或撤銷，已驗證可修正、撤銷或淘汰（有效的教訓還可轉為 Guard），其他只能看詳情。 */
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
        {entry.kind === "lesson" && entry.effective && (
          <button type="button" onClick={() => setMode("guard")}>
            轉為 Guard
          </button>
        )}
        <button type="button" onClick={() => onOpenDetail(entry.id)}>
          詳情
        </button>
      </div>
      {mode === "guard" && (
        <GuardForm
          onCancel={() => setMode(null)}
          onSubmit={(rule) => run(() => client.createGuard({ lesson_id: entry.id, rule }))}
        />
      )}
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
  client: Client;
  id: string;
  workspace: Workspace;
  refreshKey: number;
  timeZone?: string;
  act: Act;
  onClose: () => void;
}

/** 條目詳情：全部內容版本（由舊到新）與變更歷史（由舊到新）；教訓另有重犯紀錄。 */
function DetailPanel({ client, id, workspace, refreshKey, timeZone, act, onClose }: DetailProps) {
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
          {loaded.detail.entry.kind === "lesson" && (
            <RecurrencePanel
              client={client}
              lesson={loaded.detail.entry}
              workspace={workspace}
              refreshKey={refreshKey}
              timeZone={timeZone}
              act={act}
            />
          )}
        </>
      )}
    </section>
  );
}

interface GuardSectionProps {
  guards: GuardsState;
  client: GuardClient;
  timeZone?: string;
  act: Act;
}

/** 此 Workspace 可見的 Guard：規則說明、來源教訓、生效狀態、成效，以及停用／啟用。 */
function GuardSection({ guards, client, timeZone, act }: GuardSectionProps) {
  return (
    <section className="knowledge-section knowledge-guards" aria-label="Guard">
      <h3>Guard{guards.state === "ready" && `（${guards.guards.length}）`}</h3>
      <p>派工前會檢查生效中的 Guard，違反就擋下並顯示來源教訓。</p>
      {guards.state === "loading" && <p>載入中…</p>}
      {guards.state === "failed" && <p role="alert">無法取得 Guard：{guards.reason}</p>}
      {guards.state === "ready" && guards.guards.length === 0 && <p>還沒有 Guard；可在有效的教訓上「轉為 Guard」</p>}
      {guards.state === "ready" && guards.guards.length > 0 && (
        <ul className="knowledge-guard-list">
          {guards.guards.map((guard) => (
            <GuardItem key={guard.id} guard={guard} client={client} timeZone={timeZone} act={act} />
          ))}
        </ul>
      )}
    </section>
  );
}

function GuardItem({ guard, client, timeZone, act }: { guard: GuardView; client: GuardClient; timeZone?: string; act: Act }) {
  const [error, setError] = useState<string | null>(null);
  const enabled = guard.status === "enabled";

  const toggle = async () => {
    setError(null);
    const failure = await act(() =>
      enabled ? client.disableGuard(guard.id, guard.revision) : client.enableGuard(guard.id, guard.revision),
    );
    if (failure !== null) setError(failure);
  };

  return (
    <li className={`knowledge-guard ${guard.active ? "knowledge-effective" : "knowledge-inactive"}`} aria-label={guard.description}>
      <p>
        <span className="knowledge-badge">{guard.active ? "生效中" : `不生效：${guard.inactive_reason ?? "原因不明"}`}</span>
        <strong>{guard.description}</strong>
      </p>
      <p className="knowledge-meta">
        {RULE_KIND_LABELS[guard.rule.kind]}・來源教訓：{guard.lesson.title}
      </p>
      <p className="knowledge-meta">
        成效：攔下 {guard.prevented_count} 次・最近攔下：
        {guard.last_prevented_at === null ? "尚無" : formatTimestamp(guard.last_prevented_at, timeZone)}・未攔下的回報{" "}
        {guard.reported_count} 次
      </p>
      <div className="knowledge-actions">
        <button type="button" onClick={() => void toggle()}>
          {enabled ? "停用" : "啟用"}
        </button>
      </div>
      {error && <p role="alert">{error}</p>}
    </li>
  );
}

/** 轉為 Guard：選規則種類並填寫該種類的參數。 */
function GuardForm({ onSubmit, onCancel }: { onSubmit: (rule: GuardRule) => Promise<void>; onCancel: () => void }) {
  const [kind, setKind] = useState<GuardRule["kind"]>("preflight");
  const [field, setField] = useState<ContractFieldName>("objective");
  const [mention, setMention] = useState("");
  const [capability, setCapability] = useState<Capability>("read_files");
  const [maxFailed, setMaxFailed] = useState("3");
  const [error, setError] = useState<string | null>(null);

  /** 依種類組出規則；輸入不合法時回傳要顯示的說明。 */
  const ruleOf = (): GuardRule | string => {
    switch (kind) {
      case "preflight":
        return mention.trim() === "" ? "請輸入契約欄位必須提到的文字" : { kind, field, must_mention: mention.trim() };
      case "tool_restriction":
        return { kind, capability };
      case "retry_policy": {
        const count = Number(maxFailed);
        return Number.isInteger(count) && count >= RETRY_LIMIT_MIN && count <= RETRY_LIMIT_MAX
          ? { kind, max_failed_runs: count }
          : `失敗次數須為 ${RETRY_LIMIT_MIN} 到 ${RETRY_LIMIT_MAX} 的整數`;
      }
    }
  };

  const submit = (event: FormEvent) => {
    event.preventDefault();
    const rule = ruleOf();
    if (typeof rule === "string") {
      setError(rule);
      return;
    }
    setError(null);
    void onSubmit(rule);
  };

  return (
    <form className="knowledge-form" aria-label="轉為 Guard" onSubmit={submit}>
      <label>
        規則種類
        <select value={kind} onChange={(e) => setKind(e.target.value as GuardRule["kind"])}>
          {(Object.keys(RULE_KIND_LABELS) as GuardRule["kind"][]).map((k) => (
            <option key={k} value={k}>
              {RULE_KIND_LABELS[k]}
            </option>
          ))}
        </select>
      </label>
      {kind === "preflight" && (
        <>
          <label>
            契約欄位
            <select value={field} onChange={(e) => setField(e.target.value as ContractFieldName)}>
              {CONTRACT_FIELDS.map((name) => (
                <option key={name} value={name}>
                  {FIELD_LABELS[name]}
                </option>
              ))}
            </select>
          </label>
          <label>
            必須提到的文字（不分大小寫）
            <input value={mention} onChange={(e) => setMention(e.target.value)} />
          </label>
        </>
      )}
      {kind === "tool_restriction" && (
        <label>
          禁止要求的能力
          <select value={capability} onChange={(e) => setCapability(e.target.value as Capability)}>
            {(Object.keys(CAPABILITY_LABELS) as Capability[]).map((c) => (
              <option key={c} value={c}>
                {CAPABILITY_LABELS[c]}
              </option>
            ))}
          </select>
        </label>
      )}
      {kind === "retry_policy" && (
        <label>
          同一份契約失敗幾次後不再派工
          {/* 範圍由送出時檢查並以既有的錯誤訊息呈現，不用瀏覽器內建的 min/max 驗證 */}
          <input type="number" value={maxFailed} onChange={(e) => setMaxFailed(e.target.value)} />
        </label>
      )}
      <div className="knowledge-actions">
        <button type="submit">建立 Guard</button>
        <button type="button" onClick={onCancel}>
          取消
        </button>
      </div>
      {error && <p role="alert">{error}</p>}
    </form>
  );
}

type RecurrenceState = { state: "loading" } | { state: "ready"; recurrences: Recurrence[] } | { state: "failed"; reason: string };

interface RecurrenceProps {
  client: GuardClient;
  lesson: KnowledgeEntryView;
  workspace: Workspace;
  refreshKey: number;
  timeZone?: string;
  act: Act;
}

/** 教訓的重犯紀錄（由新到舊）；有效的教訓可回報一次沒被攔下的重犯。 */
function RecurrencePanel({ client, lesson, workspace, refreshKey, timeZone, act }: RecurrenceProps) {
  const [loaded, setLoaded] = useState<RecurrenceState>({ state: "loading" });
  const [note, setNote] = useState("");
  const [taskId, setTaskId] = useState("");
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    client.listRecurrences(lesson.id).then(
      ({ recurrences }) => !cancelled && setLoaded({ state: "ready", recurrences }),
      (e: unknown) => !cancelled && setLoaded({ state: "failed", reason: reasonOf(e) }),
    );
    return () => {
      cancelled = true;
    };
  }, [client, lesson.id, refreshKey]);

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (note.trim() === "") {
      setError("請填寫重犯說明");
      return;
    }
    setError(null);
    // 成功後 act 會讓詳情（含重犯紀錄）與 Guard 成效重新讀取
    const failure = await act(() =>
      client.reportRecurrence(lesson.id, {
        note: note.trim(),
        workspace_id: workspace.id,
        task_id: taskId.trim() === "" ? undefined : taskId.trim(),
      }),
    );
    // 失敗時保留輸入，讓使用者修正後重送
    if (failure === null) {
      setNote("");
      setTaskId("");
    } else setError(failure);
  };

  return (
    <section className="knowledge-recurrences" aria-label="重犯紀錄">
      <h4>重犯紀錄</h4>
      {loaded.state === "loading" && <p>載入中…</p>}
      {loaded.state === "failed" && <p role="alert">無法取得重犯紀錄：{loaded.reason}</p>}
      {loaded.state === "ready" && loaded.recurrences.length === 0 && <p>沒有重犯紀錄</p>}
      {loaded.state === "ready" && loaded.recurrences.length > 0 && (
        <ol>
          {loaded.recurrences.map((recurrence) => (
            <li key={recurrence.id}>
              <strong>{RECURRENCE_KIND_LABELS[recurrence.kind]}</strong>・{formatTimestamp(recurrence.occurred_at, timeZone)}
              ・Task：{recurrence.task_id ?? "未指定"}・{recurrence.message}
            </li>
          ))}
        </ol>
      )}
      {lesson.effective && (
        <form className="knowledge-form" aria-label="回報重犯" onSubmit={(e) => void submit(e)}>
          <label>
            重犯說明
            <textarea value={note} onChange={(e) => setNote(e.target.value)} />
          </label>
          <label>
            Task 識別（選填）
            <input value={taskId} onChange={(e) => setTaskId(e.target.value)} />
          </label>
          <button type="submit">回報重犯</button>
          {error && <p role="alert">{error}</p>}
        </form>
      )}
    </section>
  );
}
