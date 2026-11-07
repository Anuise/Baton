import { Fragment, useCallback, useEffect, useRef, useState, type FormEvent, type ReactNode } from "react";

import { FIELD_LABELS } from "./ContractEditor";
import { LongText } from "./LongText";
import { formatNumber, shortId } from "./textFormat";
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

/** 狀態徽章的文字：一行、最多 6 個全形字（MASTER 5.4）；候選的完整說明放在徽章的 title。 */
const STATUS_LABELS: Record<KnowledgeStatus, string> = {
  candidate: "未驗證",
  verified: "已驗證",
  revoked: "無效：已撤銷",
  retired: "無效：已淘汰",
  superseded: "無效：已被取代",
};

const CANDIDATE_TITLE = "候選，尚未驗證：確認後才會提供給後續的 Run 使用";

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
  return entry.valid_until !== null && entry.valid_until <= Date.now() ? "無效：已過期" : "無效：未生效";
}

function scopeLabel(entry: KnowledgeEntryView): string {
  const { scope } = entry;
  switch (scope.kind) {
    case "global":
      return "全域";
    case "workspace":
      return "Workspace";
    case "goal":
      return `Goal ${shortId(scope.goal_id ?? "")}`;
    case "task":
      return `Task ${shortId(scope.task_id ?? "")}`;
  }
}

function sourceLabel(entry: KnowledgeEntryView): string {
  const { source } = entry;
  switch (source.kind) {
    case "user":
      return "使用者";
    case "agent":
      return `Agent（Run ${shortId(source.run_id)}，第 ${source.generation} 次，Task ${shortId(source.task_id)}）`;
    case "controller":
      return `Controller（Run ${shortId(source.controller_run_id)}，Task ${shortId(source.task_id)}）`;
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
  const [detail, setDetail] = useState<DetailTarget | null>(null);
  // 新增條目的表單收在標題列的按鈕後面，不常駐展開（audit P-KN-6）
  const [creating, setCreating] = useState(false);
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

  /** 一組條目（分隔清單）；詳情展開在開啟它的那一列後面（同一條目可能同時出現在兩個清單，所以連清單一起記）。 */
  const cards = (listName: DetailTarget["list"], entries: KnowledgeEntryView[]) => (
    <div className="row-list knowledge-entries">
      {entries.map((entry) => (
        <Fragment key={entry.id}>
          <EntryCard
            client={client}
            entry={entry}
            timeZone={timeZone}
            act={act}
            onOpenDetail={(id) => setDetail({ id, list: listName })}
          />
          {detail !== null && detail.id === entry.id && detail.list === listName && (
            <DetailPanel
              client={client}
              id={detail.id}
              workspace={workspace}
              refreshKey={detailRefresh}
              timeZone={timeZone}
              act={act}
              onClose={() => setDetail(null)}
            />
          )}
        </Fragment>
      ))}
    </div>
  );

  const candidates = list.state === "ready" ? list.all.filter((e) => e.status === "candidate") : [];
  const history = list.state === "ready" ? list.all.filter((e) => !e.effective && e.status !== "candidate") : [];

  // 版面（依使用者的工作排序）：等你確認的候選 → 有效條目（含歷史開關）→ Guard。
  // 搜尋同時篩選候選、有效與歷史條目，所以放在頁面標題列，而不是某一個區段裡
  return (
    <section className="page knowledge-page" aria-label="知識">
      <header className="page-header">
        <h2>知識</h2>
        <div className="page-actions">
          <form className="knowledge-search" role="search" onSubmit={search}>
            <input
              aria-label="搜尋關鍵字"
              placeholder="以空白分隔多個關鍵字，每個至少 2 個字"
              value={draftQuery}
              onChange={(e) => setDraftQuery(e.target.value)}
            />
            <button type="submit">搜尋</button>
          </form>
          <button
            type="button"
            className={creating ? undefined : "btn-primary"}
            aria-expanded={creating}
            onClick={() => setCreating((value) => !value)}
          >
            {creating ? "收起新增條目" : "新增條目"}
          </button>
        </div>
      </header>
      {queryError && (
        <p className="field-error" role="alert">
          {queryError}
        </p>
      )}
      {notice && (
        <div className="callout callout-warning" role="status">
          <p>{notice}</p>
        </div>
      )}
      {creating && <CreateForm client={client} workspace={workspace} act={act} onCancel={() => setCreating(false)} />}
      {list.state === "loading" && <p className="empty-note">載入中…</p>}
      {list.state === "failed" && (
        <p className="field-error" role="alert">
          無法取得知識：{list.reason}
        </p>
      )}
      {list.state === "ready" && (
        <>
          {list.degraded && (
            <div className="callout callout-warning" role="status">
              <p>{DEGRADED_NOTICE}</p>
            </div>
          )}
          <section className="section knowledge-candidates" aria-label="待確認的候選教訓">
            <div className="section-heading">
              <h3>待確認的候選教訓</h3>
              <span className="count">{candidates.length}</span>
              <p>Agent 提交的候選尚未驗證，確認後才會提供給後續的 Run 使用。</p>
            </div>
            {candidates.length === 0 && <p className="empty-note">沒有待確認的候選</p>}
            {candidates.length > 0 && cards("candidates", candidates)}
          </section>
          <section className="section" aria-label="有效條目">
            <div className="section-heading">
              <h3>有效條目</h3>
              <span className="count">{list.effective.length}</span>
              <div className="section-actions">
                <label className="knowledge-history-toggle">
                  <input type="checkbox" checked={showHistory} onChange={(e) => setShowHistory(e.target.checked)} />
                  顯示歷史
                </label>
              </div>
            </div>
            {list.effective.length === 0 && (
              <p className="empty-note">{query === "" ? "還沒有有效的條目" : "沒有符合搜尋的有效條目"}</p>
            )}
            {list.effective.length > 0 && cards("effective", list.effective)}
          </section>
          {showHistory && (
            <section className="section" aria-label="歷史條目">
              <div className="section-heading">
                <h3>歷史條目</h3>
                <span className="count">{history.length}</span>
                <p>已撤銷、已淘汰、已被取代或不在有效期間內的條目，不會提供給 Run 使用。</p>
              </div>
              {history.length === 0 && <p className="empty-note">沒有歷史條目</p>}
              {history.length > 0 && cards("history", history)}
            </section>
          )}
          <GuardSection guards={guards} client={client} timeZone={timeZone} act={act} />
        </>
      )}
    </section>
  );
}

/** 開啟中的詳情：條目與開啟它的清單。 */
interface DetailTarget {
  id: string;
  list: "candidates" | "effective" | "history";
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
      <Field label="標題">
        <input {...field("title")} />
      </Field>
      <Field label="內文">
        <textarea {...field("body")} />
      </Field>
      {lesson && (
        <>
          <Field label="觸發條件">
            <textarea {...field("trigger")} />
          </Field>
          <Field label="根因或待驗假說">
            <textarea {...field("rootCause")} />
          </Field>
          <Field label="解法">
            <textarea {...field("resolution")} />
          </Field>
        </>
      )}
      <Field label="標籤（以逗號或換行分隔）">
        <input {...field("tags")} />
      </Field>
      <Field label="證據（每行一項）">
        <textarea {...field("evidence")} />
      </Field>
    </>
  );
}

/** 一個表單欄位（MASTER 第 7 節的 .field）：label 包住控制項，可存取名稱就是欄位名稱。 */
function Field({ label, children }: { label: string; children: ReactNode }) {
  return (
    <label className="field">
      <span className="field-label">{label}</span>
      {children}
    </label>
  );
}

/** 詳情中的完整內容：內文、教訓欄位、標籤與證據都完整顯示。 */
function ContentView({ content }: { content: KnowledgeContent }) {
  const facts = lessonFacts(content);
  return (
    <>
      {content.body !== "" && <p className="knowledge-body pre-wrap">{content.body}</p>}
      {facts.length > 0 && <FactList facts={facts} />}
      {content.tags.length > 0 && <TagList tags={content.tags} full />}
    </>
  );
}

type Fact = [label: string, value: ReactNode];

/** 教訓的觸發條件、根因與解法，以及證據（每項是路徑或網址，等寬、可在任何字元斷開）。 */
function lessonFacts(content: KnowledgeContent): Fact[] {
  const facts: Fact[] = [];
  if (content.lesson) {
    facts.push(
      ["觸發條件", content.lesson.trigger || "（未填）"],
      ["根因或待驗假說", content.lesson.root_cause || "（未填）"],
      ["解法", content.lesson.resolution || "（未填）"],
    );
  }
  if (content.evidence.length > 0) {
    facts.push([
      "證據",
      <ul className="knowledge-evidence">
        {content.evidence.map((item) => (
          <li key={item} className="mono">
            {item}
          </li>
        ))}
      </ul>,
    ]);
  }
  return facts;
}

/** 標籤：值（MASTER 第 7 節的 .facts）。長段落保留換行。 */
function FactList({ facts }: { facts: Fact[] }) {
  return (
    <dl className="facts knowledge-facts">
      {facts.map(([label, value]) => (
        <Fragment key={label}>
          <dt>{label}</dt>
          <dd>{value}</dd>
        </Fragment>
      ))}
    </dl>
  );
}

/** 標籤膠囊：卡片中過長的以 … 結尾（完整文字在 title 與詳情）；詳情中（full）完整顯示、可換行。 */
function TagList({ tags, full = false }: { tags: string[]; full?: boolean }) {
  return (
    <ul className={full ? "knowledge-tags knowledge-tags-full" : "knowledge-tags"} aria-label="標籤">
      {tags.map((tag) => (
        <li key={tag} title={full ? undefined : tag}>
          {tag}
        </li>
      ))}
    </ul>
  );
}

/** 「更多」選單：卡片中第二順位以後的動作；選了之後收起選單。Esc 收起並把焦點移回「更多」，點選單外面也會收起。 */
function MoreMenu({ items }: { items: { label: string; onSelect: () => void; danger?: boolean }[] }) {
  const ref = useRef<HTMLDetailsElement>(null);

  // 直接讀 details.open 而不追蹤 toggle 事件：toggle 是排入佇列的 task，開啟後立刻點外面時會晚於 pointerdown
  useEffect(() => {
    const onPointerDown = (event: PointerEvent) => {
      const menu = ref.current;
      if (menu?.open && !menu.contains(event.target as Node)) menu.open = false;
    };
    document.addEventListener("pointerdown", onPointerDown);
    return () => document.removeEventListener("pointerdown", onPointerDown);
  }, []);

  return (
    <details
      className="more-menu"
      ref={ref}
      onKeyDown={(event) => {
        if (event.key !== "Escape" || !event.currentTarget.open) return;
        event.currentTarget.open = false;
        event.currentTarget.querySelector("summary")?.focus();
      }}
    >
      <summary>更多</summary>
      <div className="more-menu-list">
        {items.map((item) => (
          <button
            key={item.label}
            type="button"
            className={item.danger ? "btn-quiet more-menu-danger" : "btn-quiet"}
            onClick={() => {
              if (ref.current) ref.current.open = false;
              item.onSelect();
            }}
          >
            {item.label}
          </button>
        ))}
      </div>
    </details>
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

/**
 * 一個條目；可用的操作依狀態：候選可確認或撤銷，已驗證可修正、撤銷或淘汰（有效的教訓還可轉為 Guard），其他只能看詳情。
 * 直接看得到的動作最多 3 個（MASTER 5.1）：候選＝確認、撤銷、詳情；已驗證＝修正、詳情、更多。
 */
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

  const { content } = entry;
  // 收合的細節裡有什麼：教訓欄位、來源、證據
  const extraLabel = [
    content.lesson ? "教訓欄位" : null,
    "來源",
    content.evidence.length > 0 ? `證據（${content.evidence.length}）` : null,
  ]
    .filter((part) => part !== null)
    .join("、");
  const badgeTone = tone === "candidate" ? " badge-warning" : tone === "effective" ? " badge-success" : "";

  // 一列＝標題列（標題 2 行＋狀態徽章）→ 次要資訊 → 內文 3 行＋展開 → 標籤 → 收合的細節 → 動作；
  // 候選以左側琥珀色條標示「等你確認」（MASTER 第 3 節：語意色只標示狀態）
  return (
    <article
      className={`row knowledge-entry knowledge-${tone}${tone === "candidate" ? " stripe-warning" : ""}`}
      aria-label={content.title}
    >
      <div className="row-head">
        <h4 className="clamp-2" title={content.title}>
          {content.title}
        </h4>
        <span className={`badge${badgeTone}`} title={tone === "candidate" ? CANDIDATE_TITLE : undefined}>
          {statusLabel(entry)}
        </span>
      </div>
      <p className="meta">
        <span>{KIND_LABELS[entry.kind]}</span>
        <span>範圍：{scopeLabel(entry)}</span>
        <span>v{entry.version}</span>
        <span>
          有效期間：{fmt(entry.valid_from)} 起，{entry.valid_until === null ? "無期限" : `至 ${fmt(entry.valid_until)}`}
        </span>
      </p>
      {entry.status_reason && <LongText text={`原因：${entry.status_reason}`} lines={2} className="knowledge-reason" />}
      {entry.superseded_by && (
        <p className="meta">
          <span>
            已被{" "}
            <code className="mono" title={entry.superseded_by}>
              {shortId(entry.superseded_by)}
            </code>{" "}
            取代
          </span>
        </p>
      )}
      {content.body !== "" && <LongText text={content.body} />}
      {content.tags.length > 0 && <TagList tags={content.tags} />}
      <details className="knowledge-extra">
        <summary>{extraLabel}</summary>
        <FactList facts={[["來源", sourceLabel(entry)], ...lessonFacts(content)]} />
      </details>
      <div className="knowledge-actions">
        {entry.status === "candidate" && (
          <>
            <button
              type="button"
              className="btn-primary"
              onClick={() => void run(() => client.confirmKnowledge(entry.id, entry.revision))}
            >
              確認為已驗證
            </button>
            <button type="button" className="btn-danger" onClick={() => setMode("revoke")}>
              撤銷
            </button>
          </>
        )}
        {entry.status === "verified" && (
          <button type="button" onClick={() => setMode("correct")}>
            修正
          </button>
        )}
        <button type="button" className="btn-quiet" onClick={() => onOpenDetail(entry.id)}>
          詳情
        </button>
        {/* 轉為 Guard、撤銷、淘汰是第二順位以後的動作，收進「更多」（MASTER 5.1）；只有有效的已驗證教訓能轉為 Guard */}
        {entry.status === "verified" && (
          <MoreMenu
            items={[
              ...(entry.kind === "lesson" && entry.effective ? [{ label: "轉為 Guard", onSelect: () => setMode("guard") }] : []),
              { label: "撤銷", onSelect: () => setMode("revoke"), danger: true },
              { label: "淘汰", onSelect: () => setMode("retire"), danger: true },
            ]}
          />
        )}
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
      {error && (
        <p className="field-error" role="alert">
          {error}
        </p>
      )}
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
    <form className="inset knowledge-form" aria-label={action} onSubmit={submit}>
      <Field label={`${action}原因`}>
        {/* 從「更多」選單或撤銷按鈕開啟時，焦點直接到原因欄，不會掉到 body */}
        <textarea value={reason} onChange={(e) => setReason(e.target.value)} autoFocus />
      </Field>
      <div className="form-actions">
        <button type="submit">確認{action}</button>
        <button type="button" onClick={onCancel}>
          取消
        </button>
      </div>
      {error && (
        <p className="field-error" role="alert">
          {error}
        </p>
      )}
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
    <form className="inset knowledge-form" aria-label="修正" onSubmit={submit}>
      <ContentFields draft={draft} onChange={setDraft} lesson={entry.kind === "lesson"} />
      <Field label="修正說明（選填）">
        <input value={note} onChange={(e) => setNote(e.target.value)} />
      </Field>
      <div className="form-actions">
        <button type="submit">送出修正</button>
        <button type="button" onClick={onCancel}>
          取消
        </button>
      </div>
      {error && (
        <p className="field-error" role="alert">
          {error}
        </p>
      )}
    </form>
  );
}

interface CreateFormProps {
  client: KnowledgeClient;
  workspace: Workspace;
  act: Act;
  onCancel: () => void;
}

/** 新增條目（使用者建立的直接是已驗證）；範圍只提供全域或此 Workspace。 */
function CreateForm({ client, workspace, act, onCancel }: CreateFormProps) {
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
    <form className="panel knowledge-form knowledge-create" aria-label="新增條目" onSubmit={(e) => void submit(e)}>
      <h3>新增條目</h3>
      <p className="section-hint">你建立的條目直接是已驗證，之後的 Run 就會使用。</p>
      <div className="field-row">
        <Field label="種類">
          <select value={kind} onChange={(e) => setKind(e.target.value as KnowledgeKind)}>
            <option value="lesson">教訓</option>
            <option value="decision">決策</option>
            <option value="fact">知識</option>
          </select>
        </Field>
        <Field label="範圍">
          <select value={scope} onChange={(e) => setScope(e.target.value as "workspace" | "global")}>
            <option value="workspace">此 Workspace</option>
            <option value="global">全域</option>
          </select>
        </Field>
      </div>
      <ContentFields draft={draft} onChange={setDraft} lesson={kind === "lesson"} />
      <div className="form-actions">
        <button type="submit" disabled={busy}>
          新增
        </button>
        <button type="button" onClick={onCancel}>
          取消
        </button>
      </div>
      {error && (
        <p className="field-error" role="alert">
          {error}
        </p>
      )}
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

  // 詳情是清單中的一列，接在開啟它的條目後面；左側群青色條表示「這是上一列展開的內容」
  return (
    <section className="row knowledge-detail stripe-accent" aria-label="條目詳情">
      <div className="row-head">
        {loaded.state === "ready" ? <h4>詳情：{loaded.detail.entry.content.title}</h4> : <span />}
        <button type="button" className="btn-sm" onClick={onClose}>
          關閉詳情
        </button>
      </div>
      {loaded.state === "loading" && <p className="empty-note">載入中…</p>}
      {loaded.state === "failed" && (
        <p className="field-error" role="alert">
          無法取得詳情：{loaded.reason}
        </p>
      )}
      {loaded.state === "ready" && (
        <>
          <section className="knowledge-detail-part">
            <h5>版本（由舊到新）</h5>
            <ol className="knowledge-versions">
              {[...loaded.detail.versions]
                .sort((a, b) => a.version - b.version)
                .map((version) => (
                  <li key={version.version} aria-label={`v${version.version}`}>
                    <p className="meta">
                      <span>
                        <strong>v{version.version}</strong>
                      </span>
                      <span className="tabular">{fmt(version.created_at)}</span>
                      {version.note && <span>修正說明：{version.note}</span>}
                    </p>
                    <p className="knowledge-version-title">{version.content.title}</p>
                    <ContentView content={version.content} />
                  </li>
                ))}
            </ol>
          </section>
          <section className="knowledge-detail-part">
            <h5>變更歷史</h5>
            {loaded.detail.history.length === 0 && <p className="empty-note">沒有變更紀錄</p>}
            <ol className="knowledge-history">
              {loaded.detail.history.map((event) => {
                const payload = (event.payload ?? {}) as { reason?: unknown; superseded_by?: unknown };
                return (
                  <li key={event.revision}>
                    <span className="knowledge-history-time">{fmt(event.occurred_at)}</span>{" "}
                    {EVENT_LABELS[event.event_type] ?? event.event_type}
                    {typeof payload.reason === "string" && `：${payload.reason}`}
                    {typeof payload.superseded_by === "string" && `（由 ${shortId(payload.superseded_by)} 取代）`}
                  </li>
                );
              })}
            </ol>
          </section>
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
    <section className="section knowledge-guards" aria-label="Guard">
      <div className="section-heading">
        <h3>Guard</h3>
        {guards.state === "ready" && <span className="count">{guards.guards.length}</span>}
        <p>派工前會檢查生效中的 Guard，違反就擋下並顯示來源教訓。</p>
      </div>
      {guards.state === "loading" && <p className="empty-note">載入中…</p>}
      {guards.state === "failed" && (
        <p className="field-error" role="alert">
          無法取得 Guard：{guards.reason}
        </p>
      )}
      {guards.state === "ready" && guards.guards.length === 0 && (
        <p className="empty-note">還沒有 Guard；可在有效教訓的「更多」選單中「轉為 Guard」</p>
      )}
      {guards.state === "ready" && guards.guards.length > 0 && (
        <ul className="row-list">
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

  // 一列＝規則說明＋狀態＋停用／啟用 → 不生效的原因 → 規則種類與來源教訓 → 成效
  return (
    <li className={`knowledge-guard ${guard.active ? "knowledge-effective" : "knowledge-inactive"}`} aria-label={guard.description}>
      <div className="row-head">
        <strong className="clamp-2" title={guard.description}>
          {guard.description}
        </strong>
        <span className={guard.active ? "badge badge-success" : "badge"}>{guard.active ? "生效中" : "不生效"}</span>
        <button type="button" className="btn-sm" onClick={() => void toggle()}>
          {enabled ? "停用" : "啟用"}
        </button>
      </div>
      {/* 不生效的原因可能是整段錯誤訊息：2 行＋展開 */}
      {!guard.active && (
        <LongText text={`原因：${guard.inactive_reason ?? "原因不明"}`} lines={2} className="knowledge-reason" />
      )}
      <p className="meta">
        <span>{RULE_KIND_LABELS[guard.rule.kind]}</span>
        <span className="knowledge-guard-lesson clamp-1" title={guard.lesson.title}>
          來源教訓：{guard.lesson.title}
        </span>
      </p>
      <p className="meta knowledge-guard-effect">
        <span>
          攔下 <strong className="tabular">{formatNumber(guard.prevented_count)}</strong> 次
        </span>
        <span>
          最近攔下：{guard.last_prevented_at === null ? "尚無" : formatTimestamp(guard.last_prevented_at, timeZone)}
        </span>
        <span>
          未攔下的回報 <span className="tabular">{formatNumber(guard.reported_count)}</span> 次
        </span>
      </p>
      {error && (
        <p className="field-error" role="alert">
          {error}
        </p>
      )}
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
    <form className="inset knowledge-form" aria-label="轉為 Guard" onSubmit={submit}>
      <div className="field-row">
        <Field label="規則種類">
          {/* 從「更多」選單開啟時，焦點直接到第一個欄位，不會掉到 body */}
          <select value={kind} onChange={(e) => setKind(e.target.value as GuardRule["kind"])} autoFocus>
            {(Object.keys(RULE_KIND_LABELS) as GuardRule["kind"][]).map((k) => (
              <option key={k} value={k}>
                {RULE_KIND_LABELS[k]}
              </option>
            ))}
          </select>
        </Field>
        {kind === "preflight" && (
          <Field label="契約欄位">
            <select value={field} onChange={(e) => setField(e.target.value as ContractFieldName)}>
              {CONTRACT_FIELDS.map((name) => (
                <option key={name} value={name}>
                  {FIELD_LABELS[name]}
                </option>
              ))}
            </select>
          </Field>
        )}
        {kind === "tool_restriction" && (
          <Field label="禁止要求的能力">
            <select value={capability} onChange={(e) => setCapability(e.target.value as Capability)}>
              {(Object.keys(CAPABILITY_LABELS) as Capability[]).map((c) => (
                <option key={c} value={c}>
                  {CAPABILITY_LABELS[c]}
                </option>
              ))}
            </select>
          </Field>
        )}
        {kind === "retry_policy" && (
          <Field label="同一份契約失敗幾次後不再派工">
            {/* 範圍由送出時檢查並以既有的錯誤訊息呈現，不用瀏覽器內建的 min/max 驗證 */}
            <input type="number" className="input-short" value={maxFailed} onChange={(e) => setMaxFailed(e.target.value)} />
          </Field>
        )}
      </div>
      {kind === "preflight" && (
        <Field label="必須提到的文字（不分大小寫）">
          <input value={mention} onChange={(e) => setMention(e.target.value)} />
        </Field>
      )}
      <div className="form-actions">
        <button type="submit">建立 Guard</button>
        <button type="button" onClick={onCancel}>
          取消
        </button>
      </div>
      {error && (
        <p className="field-error" role="alert">
          {error}
        </p>
      )}
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
    <section className="knowledge-detail-part knowledge-recurrences" aria-label="重犯紀錄">
      <h5>重犯紀錄</h5>
      {loaded.state === "loading" && <p className="empty-note">載入中…</p>}
      {loaded.state === "failed" && (
        <p className="field-error" role="alert">
          無法取得重犯紀錄：{loaded.reason}
        </p>
      )}
      {loaded.state === "ready" && loaded.recurrences.length === 0 && <p className="empty-note">沒有重犯紀錄</p>}
      {loaded.state === "ready" && loaded.recurrences.length > 0 && (
        <ol className="knowledge-recurrence-list">
          {loaded.recurrences.map((recurrence) => (
            <li key={recurrence.id}>
              <p className="meta">
                <span className="knowledge-recurrence-kind">{RECURRENCE_KIND_LABELS[recurrence.kind]}</span>
                <span className="tabular">{formatTimestamp(recurrence.occurred_at, timeZone)}</span>
                <span>Task：{recurrence.task_id === null ? "未指定" : shortId(recurrence.task_id)}</span>
              </p>
              <p className="knowledge-recurrence-message">{recurrence.message}</p>
            </li>
          ))}
        </ol>
      )}
      {lesson.effective && (
        <form className="inset knowledge-form" aria-label="回報重犯" onSubmit={(e) => void submit(e)}>
          <Field label="重犯說明">
            <textarea value={note} onChange={(e) => setNote(e.target.value)} />
          </Field>
          <Field label="Task 識別（選填）">
            <input value={taskId} onChange={(e) => setTaskId(e.target.value)} />
          </Field>
          <div className="form-actions">
            <button type="submit">回報重犯</button>
          </div>
          {error && (
            <p className="field-error" role="alert">
              {error}
            </p>
          )}
        </form>
      )}
    </section>
  );
}
