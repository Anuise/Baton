import {
  useCallback,
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
  type FormEvent,
  type ReactNode,
  type Ref,
  type RefObject,
} from "react";

import { HelpTip } from "./HelpTip";
import { CopyButton } from "./LongText";
import { COLUMNS } from "./TaskBoard";
import { optionLabel, shortHash } from "./textFormat";
import { formatTimestamp } from "./time";
import {
  errorCode,
  type AnswerRequest,
  type ApprovalBinding,
  type ApprovalMismatchReason,
  type AuthorityLevel,
  type AuthorityTable,
  type AuthorizationClient,
  type BatchResponse,
  type Capability,
  type ConfirmationSummary,
  type DecisionAction,
  type DecisionClient,
  type DecisionKind,
  type DecisionList,
  type DecisionOverview,
  type DecisionRequestView,
  type DecisionStatus,
  type DecisionUrgency,
  type Grant,
  type Workspace,
} from "./runtimeClient";

type ViewState =
  | { state: "loading" }
  | { state: "ready"; list: DecisionList }
  | { state: "failed"; reason: string };

interface Props {
  client: DecisionClient & AuthorizationClient;
  workspace: Workspace;
  /** 顯示用的時區；省略時使用系統時區（spec D-33），只有測試會指定。 */
  timeZone?: string;
  /** 回答、批次或合併成功後通知外層（例如重新整理頂部的待裁定數，spec D-21）。 */
  onChange?: () => void;
  /** 前往其他頁（詳情的 Task 連結、「調整決策權限」）；沒有傳入時不顯示這些入口。 */
  onOpenPage?: (page: DecisionLinkPage) => void;
}

/** 裁定詳情可以前往的頁面。 */
type DecisionLinkPage = "tasks" | "authority";

const KIND_LABELS: Record<DecisionKind, string> = { permission: "權限提示", agent: "Agent 請求", authority: "決策權限攔截" };
const URGENCY_LABELS: Record<DecisionUrgency, string> = { low: "低", normal: "一般", high: "高" };
/** superseded 稱「已失效」（不是「已過期」，避免和「已逾期」只差一字）。 */
const STATUS_LABELS: Record<DecisionStatus, string> = { pending: "待裁定", answered: "已回答", superseded: "已失效", merged: "已合併" };
/** 「已失效」的意思：清單徽章的 title 與詳情直接寫出。 */
const SUPERSEDED_NOTE = "內容或目標版本已改變，這筆不能再回答。";
const TASK_STATUS_LABELS = new Map(COLUMNS);
const MISMATCH_LABELS: Record<ApprovalMismatchReason, string> = {
  operation_changed: "操作不同",
  target_changed: "目標資源不同",
  params_changed: "重要參數不同",
  request_hash_changed: "要求內容不同",
  artifact_version_changed: "成果版本已改變",
};

const ACTION_LABELS: Record<DecisionAction, string> = {
  accept: "接受方案",
  modify: "修改條件",
  answer: "回答",
  investigate: "要求調查",
  defer: "延後",
  reject: "駁回",
};
/** 各處理方式的文字欄位標題（延後沒有文字欄位）。 */
const NOTE_LABELS: Record<Exclude<DecisionAction, "defer">, string> = {
  accept: "備註（選填）",
  modify: "修改後的條件（必填）",
  answer: "回答內容（必填）",
  investigate: "要調查的事（必填）",
  reject: "駁回理由（選填）",
};
const OUTCOME_NOTE_LABELS: Record<Exclude<DecisionAction, "defer">, string> = {
  accept: "備註",
  modify: "修改後的條件",
  answer: "回答",
  investigate: "要調查的事",
  reject: "駁回理由",
};
const OPTION_REQUIRED = new Set<DecisionAction>(["accept", "modify"]);
const NOTE_REQUIRED = new Set<DecisionAction>(["modify", "answer", "investigate"]);
const DEFER_CHOICES = [
  { minutes: 240, label: "4 小時" },
  { minutes: 1440, label: "1 天" },
  { minutes: 4320, label: "3 天" },
];

/** 「全部待裁定」＝可處理＋已延後（跑道欄的待裁定數也是這個總數）。 */
const OVERVIEW_ITEMS: [keyof DecisionOverview, string][] = [
  ["pending", "全部待裁定"],
  ["actionable", "可處理"],
  ["deferred", "已延後"],
  ["overdue", "已逾期"],
  ["high_urgency", "高緊急"],
  ["answered", "已回答"],
  ["superseded", "已失效"],
  ["merged", "已合併"],
];
/** 大於 0 時要你注意的總覽數字（琥珀色）。 */
const OVERVIEW_ATTENTION = new Set<keyof DecisionOverview>(["overdue", "high_urgency"]);
/** 總覽中「已結束」這一組的第一項，前面加分隔。 */
const OVERVIEW_CLOSED_START: keyof DecisionOverview = "answered";

/** 狀態徽章：待裁定要你處理（琥珀）、已回答是完成，其餘中性。 */
const STATUS_TONES: Record<DecisionStatus, string> = {
  pending: "badge badge-warning",
  answered: "badge badge-success",
  superseded: "badge badge-plain",
  merged: "badge badge-plain",
};

const AUTHORITY_LABELS: Record<AuthorityLevel, string> = {
  autonomous: "Controller 可自主處理",
  requires_decision: "必須送裁定",
  human_only: "只能人工發起",
};

/** 由寬到嚴，用來標示 Workspace 設定是收窄還是放寬。 */
const AUTHORITY_ORDER: AuthorityLevel[] = ["autonomous", "requires_decision", "human_only"];

/** 和內建相比是收窄（更常送裁定）還是放寬（Controller 更能自行處理）；相同時為 null。 */
function overrideKind(builtin: AuthorityLevel, effective: AuthorityLevel): "narrowed" | "widened" | null {
  const diff = AUTHORITY_ORDER.indexOf(effective) - AUTHORITY_ORDER.indexOf(builtin);
  return diff > 0 ? "narrowed" : diff < 0 ? "widened" : null;
}

const OVERRIDE_LABELS = { narrowed: "已收窄", widened: "已放寬" } as const;
/** 放寬代表 Controller 能自行處理更多事，用琥珀色提醒；收窄用中性標籤。 */
const OVERRIDE_TONES = { narrowed: "badge badge-plain", widened: "badge badge-warning" } as const;

/** 需要特別說明的錯誤代碼；其餘沿用 Runtime 的說明。 */
const ERROR_MESSAGES: Record<string, string> = {
  decision_superseded: "內容或目標版本已改變，這筆已失效，不能再回答。",
  decision_revision_mismatch: "這個請求的內容已變更，已重新整理，請確認後再送出。",
  authority_revision_mismatch: "決策權限已被修改，已重新載入，請重新確認。",
};

function reasonOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function failureMessage(error: unknown): string {
  const code = errorCode(error);
  return (code !== null ? ERROR_MESSAGES[code] : undefined) ?? reasonOf(error);
}

/** 可能超過兩行的錯誤（MASTER 7：`.notice`）：狀態詞＋說明。`noticeRef` 有值時可被程式聚焦。 */
function ErrorNotice({ title, children, noticeRef }: { title: string; children: ReactNode; noticeRef?: Ref<HTMLDivElement> }) {
  return (
    <div ref={noticeRef} className="notice notice-danger" role="alert" tabIndex={noticeRef === undefined ? undefined : -1}>
      <p className="notice-title">{title}</p>
      <p>{children}</p>
    </div>
  );
}

/** 聚焦第一個還在畫面上、沒有停用的元素。 */
function focusFirst(candidates: (HTMLElement | null)[]) {
  candidates.find((el) => el?.isConnected && !(el instanceof HTMLButtonElement && el.disabled))?.focus();
}

/**
 * 非同步操作期間觸發的按鈕會被停用，結束後可能消失（例如處理區換成裁定結果）：
 * 操作結束時若焦點已掉到 body，移到第一個可用的候選元素（焦點規則：不能落到 body）。
 */
function useFocusAfterBusy(busy: boolean, candidates: () => (HTMLElement | null)[]) {
  const wasBusy = useRef(false);
  // 只在 busy 改變時判斷；candidates 每次 render 都是新的函式，不列為相依
  useEffect(() => {
    if (busy) {
      wasBusy.current = true;
      return;
    }
    if (!wasBusy.current) return;
    wasBusy.current = false;
    const active = document.activeElement;
    if (active !== null && active !== document.body) return;
    focusFirst(candidates());
  }, [busy]);
}

/** 可以「採用建議」時回傳建議的選項：待裁定、能接受方案，且建議的是其中一個選項。 */
function adoptableRecommendation(decision: DecisionRequestView): string | null {
  const { recommendation } = decision;
  if (decision.status !== "pending" || recommendation === null || !decision.allowed_actions.includes("accept")) return null;
  return decision.options.some((option) => option.id === recommendation) ? recommendation : null;
}

/** 文字中的雜湊（SHA-1、SHA-256、commit：40～64 個十六進位字元）；split 時以擷取群組留下雜湊本身。 */
const HASH_SPLIT = /\b([0-9a-f]{40,64})\b/i;

/**
 * 原樣輸出文字，只把其中的雜湊換成縮短的等寬形式（title 放完整值）；`copy` 時旁邊附「複製雜湊」。
 * 放在 label 裡時不附按鈕（label 內不能有另一個可標示的控制項）。
 */
function WithHashes({ text, copy = true }: { text: string; copy?: boolean }) {
  const parts = text.split(HASH_SPLIT);
  if (parts.length === 1) return <>{text}</>;
  return (
    <>
      {parts.map((part, index) =>
        index % 2 === 0 ? (
          part
        ) : (
          <span key={index} className="copy-text">
            <code className="mono" title={part}>
              {shortHash(part)}
            </code>
            {copy && <CopyButton text={part} label="複製雜湊" />}
          </span>
        ),
      )}
    </>
  );
}

/** 沒有空白、含路徑分隔字元的一整段（路徑或 URL）用等寬字顯示。 */
const isPathLike = (text: string) => /^\S+$/.test(text) && /[\\/]/.test(text);

/**
 * 緊急程度徽章（清單與詳情共用）：待裁定且緊急程度高＝要你處理，用琥珀色（與總覽的「高緊急」同色）；
 * 已結束的請求不再要你處理，一律中性。
 */
const urgencyBadge = ({ urgency, status }: DecisionRequestView) =>
  `badge decision-urgency decision-urgency-${urgency} ${urgency === "high" && status === "pending" ? "badge-warning" : "badge-plain"}`;

/** 一筆請求的處理草稿；詳情與批次面板各自保有一份。 */
interface Draft {
  action: DecisionAction | null;
  optionId: string | null;
  note: string;
  deferMinutes: number;
}

const EMPTY_DRAFT: Draft = { action: null, optionId: null, note: "", deferMinutes: 1440 };

/** 把草稿轉成送出內容（一律帶 expected_revision）；缺必填欄位時回傳 null。 */
function toAnswer(decision: DecisionRequestView, draft: Draft): AnswerRequest | null {
  const { action } = draft;
  if (action === null || !decision.allowed_actions.includes(action)) return null;
  const request: AnswerRequest = { action, expected_revision: decision.revision };
  if (action === "defer") return { ...request, defer_minutes: draft.deferMinutes };
  if (OPTION_REQUIRED.has(action)) {
    if (draft.optionId === null) return null;
    request.option_id = draft.optionId;
  }
  const note = draft.note.trim();
  if (NOTE_REQUIRED.has(action) && note === "") return null;
  if (note !== "") request.note = note;
  return request;
}

/** 批次中的一項還差什麼才能送出；已填妥時回傳 null。 */
function missingOf(decision: DecisionRequestView, draft: Draft): string | null {
  const { action } = draft;
  if (toAnswer(decision, draft) !== null) return null;
  if (action === null) return "還沒選處理方式";
  if (OPTION_REQUIRED.has(action) && draft.optionId === null) return "還沒選選項";
  if (action !== "defer" && NOTE_REQUIRED.has(action)) return `還沒填寫${NOTE_LABELS[action].replace("（必填）", "")}`;
  return "還缺必填欄位";
}

/** 依後端順序分組：可處理的 pending、延後的 pending、已結束的。 */
function partition(decisions: DecisionRequestView[]) {
  return {
    actionable: decisions.filter((d) => d.status === "pending" && !d.deferred),
    deferred: decisions.filter((d) => d.status === "pending" && d.deferred),
    closed: decisions.filter((d) => d.status !== "pending"),
  };
}

interface BatchOutcome {
  results: { requestId: string; question: string; ok: boolean; message: string | null }[];
}

/** 頁面外框：載入中、失敗與正常畫面共用同一個 region 與頁面標題。 */
function DecisionPage({ children }: { children: ReactNode }) {
  const headingId = useId();
  return (
    <section className="page decision-center" aria-labelledby={headingId}>
      <header className="page-header">
        <h2 id={headingId}>裁定</h2>
      </header>
      {children}
    </section>
  );
}

/**
 * 裁定（收件匣）：上方一列總覽數字；左欄分組列出請求（可處理、已延後、已結束），
 * 右欄是工作區：批次結果、批次處理與選取那筆的詳情（寬視窗停在視窗內、自己捲動）。
 * 可處理的請求能勾選後逐項批次處理。逾期與延後都只是標示，不代表任何決定。
 */
export function DecisionCenter({ client, workspace, timeZone, onChange, onOpenPage }: Props) {
  const [loaded, setLoaded] = useState<ViewState>({ state: "loading" });
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [resumed, setResumed] = useState<{ requestId: string; count: number } | null>(null);
  // 詳情裡正在填的草稿（換筆時清空）；勾進批次時沿用這份
  const [detailDraft, setDetailDraft] = useState<{ requestId: string; draft: Draft } | null>(null);
  // 勾選進批次的請求與各自的處理草稿
  const [batch, setBatch] = useState<Record<string, Draft>>({});
  const [batchOutcome, setBatchOutcome] = useState<BatchOutcome | null>(null);
  // 「已結束」一組可以收合；預設展開
  const [closedOpen, setClosedOpen] = useState(true);
  const closedListId = useId();
  const paneRef = useRef<HTMLDivElement>(null);
  const resultsHeadingRef = useRef<HTMLHeadingElement>(null);
  const detailHeadingRef = useRef<HTMLHeadingElement>(null);
  // 使用者換到另一筆時，記下那筆的 id，由詳情掛載時取用一次並捲到詳情開頭（從詳情裡的連結換筆時一併聚焦新詳情的標題）；
  // 程式自己換筆（例如批次送出後原本那筆已結束）不捲動，以免把上方的批次結果捲出可視範圍
  const navRequest = useRef<{ requestId: string; focus: boolean } | null>(null);

  // 批次送出後：右欄捲回頂端，焦點移到批次結果的標題
  useLayoutEffect(() => {
    if (batchOutcome === null) return;
    if (paneRef.current) paneRef.current.scrollTop = 0;
    resultsHeadingRef.current?.focus({ preventScroll: true });
  }, [batchOutcome]);

  const load = useCallback(async () => {
    try {
      const list = await client.listDecisions(workspace.id);
      setLoaded({ state: "ready", list });
    } catch (e) {
      setLoaded({ state: "failed", reason: reasonOf(e) });
    }
  }, [client, workspace.id]);

  useEffect(() => {
    setLoaded({ state: "loading" });
    setSelectedId(null);
    setResumed(null);
    setDetailDraft(null);
    setBatch({});
    setBatchOutcome(null);
    void load();
  }, [load]);

  if (loaded.state === "loading") {
    return (
      <DecisionPage>
        <p className="empty-note">載入中…</p>
      </DecisionPage>
    );
  }
  if (loaded.state === "failed") {
    return (
      <DecisionPage>
        <ErrorNotice title="無法取得裁定清單">{loaded.reason}</ErrorNotice>
      </DecisionPage>
    );
  }

  const { overview, decisions } = loaded.list;
  const { actionable, deferred, closed } = partition(decisions);
  const selected = decisions.find((d) => d.id === selectedId) ?? actionable[0] ?? deferred[0] ?? closed[0] ?? null;
  // 重新載入後已不可處理的請求自動離開批次
  const batchDecisions = actionable.filter((d) => d.id in batch);
  const draftOf = (requestId: string) => (detailDraft?.requestId === requestId ? detailDraft.draft : EMPTY_DRAFT);

  /**
   * 使用者選另一筆：換筆後詳情捲到開頭、詳情的草稿清空。
   * `focus`：從詳情裡的連結（相似問題、合併目標）換筆時，原本的連結隨舊詳情消失，焦點移到新詳情的標題。
   */
  const select = (requestId: string, focus = false) => {
    if (requestId !== selected?.id) {
      navRequest.current = { requestId, focus };
      setDetailDraft(null);
    }
    setSelectedId(requestId);
  };
  const takeNavRequest = (requestId: string) => {
    const request = navRequest.current;
    if (request === null || request.requestId !== requestId) return null;
    navRequest.current = null;
    return { focus: request.focus };
  };

  const toggleBatch = (decision: DecisionRequestView) =>
    setBatch((current) => {
      // 詳情已填的草稿一併帶進批次，不必重選
      if (!(decision.id in current)) return { ...current, [decision.id]: draftOf(decision.id) };
      const rest = { ...current };
      delete rest[decision.id];
      return rest;
    });

  const listItem = (decision: DecisionRequestView, selectable: boolean) => {
    const isSelected = decision.id === selected?.id;
    return (
      <li key={decision.id} className={isSelected ? "decision-item is-selected" : "decision-item"}>
        {selectable && (
          <input
            type="checkbox"
            className="decision-batch-check"
            aria-label={`批次選取 ${decision.question}`}
            checked={decision.id in batch}
            onChange={() => toggleBatch(decision)}
          />
        )}
        <button
          type="button"
          className="decision-item-button"
          aria-label={`查看裁定 ${decision.question}`}
          aria-current={isSelected}
          onClick={() => select(decision.id)}
        >
          {/* 清單只放摘要（問題 3 行、Task 1 行、一列徽章）；完整內容點開看詳情 */}
          <span className="decision-question clamp-3" title={decision.question}>
            {decision.question}
          </span>
          <span className="decision-task clamp-1" title={decision.task_title}>
            {decision.task_title}
          </span>
          <span className="decision-badges badge-group">
            <span className="badge badge-plain decision-kind">{KIND_LABELS[decision.kind]}</span>
            <span className={urgencyBadge(decision)}>緊急程度：{URGENCY_LABELS[decision.urgency]}</span>
            {decision.status === "pending" && decision.overdue && (
              <span className="badge badge-warning decision-overdue">已逾期，尚未裁定</span>
            )}
            {decision.status === "pending" && decision.deferred && decision.deferred_until !== null && (
              <span className="decision-deferred-until">延後至 {formatTimestamp(decision.deferred_until, timeZone)}</span>
            )}
            {decision.status !== "pending" && (
              <span
                className={`${STATUS_TONES[decision.status]} decision-status`}
                title={decision.status === "superseded" ? SUPERSEDED_NOTE : undefined}
              >
                {STATUS_LABELS[decision.status]}
              </span>
            )}
          </span>
        </button>
      </li>
    );
  };

  /**
   * 清單中的一組；`collapsible` 的一組可以收合（標題與計數仍在），控制項是收合摘要樣式（與 `<details>` 的 ▶ 一致）。
   * 計數只放數字、在標題外（MASTER 7.1）；輔助技術從 region 名稱「可處理（N）」讀到計數。
   */
  const group = (
    title: string,
    items: DecisionRequestView[],
    empty: string,
    { selectable = false, collapsible = false }: { selectable?: boolean; collapsible?: boolean } = {},
  ) => {
    const open = !collapsible || closedOpen;
    return (
      <section className="section decision-group" aria-label={`${title}（${items.length}）`}>
        <div className="section-heading">
          <h3>{title}</h3>
          <span className="count">{items.length}</span>
          {collapsible && items.length > 0 && (
            <div className="section-actions">
              <button
                type="button"
                className="decision-group-toggle"
                aria-expanded={open}
                aria-controls={closedListId}
                onClick={() => setClosedOpen((value) => !value)}
              >
                {open ? "收合" : "展開"}
              </button>
            </div>
          )}
        </div>
        {items.length === 0 ? (
          <p className="empty-note">{empty}</p>
        ) : (
          <ul className="row-list decision-list" id={collapsible ? closedListId : undefined} hidden={!open}>
            {items.map((d) => listItem(d, selectable))}
          </ul>
        )}
      </section>
    );
  };

  // 處理過的請求留在畫面上，讓使用者看到結果或錯誤說明
  const pinAndReload = (requestId: string) => async () => {
    setSelectedId(requestId);
    await load();
  };

  return (
    <DecisionPage>
      {/* 總覽精簡成一列：待裁定的幾項在前，已結束的幾項在分隔後 */}
      <section className="decision-overview" aria-label="裁定總覽">
        <dl className="decision-summary">
          {OVERVIEW_ITEMS.map(([key, label]) => {
            const classes = ["decision-summary-item"];
            if (key === OVERVIEW_CLOSED_START) classes.push("is-group-start");
            if (OVERVIEW_ATTENTION.has(key) && overview[key] > 0) classes.push("is-attention");
            if (overview[key] === 0) classes.push("is-zero");
            return (
              <div key={key} className={classes.join(" ")}>
                <dt>{label}</dt>
                <dd>{overview[key]}</dd>
              </div>
            );
          })}
        </dl>
      </section>

      <div className="decision-layout">
        <div className="decision-lists">
          {group("可處理", actionable, "沒有可處理的裁定", { selectable: true })}
          {/* 沒有延後的請求時不佔位置；總覽仍顯示「已延後 0」 */}
          {deferred.length > 0 && group("已延後", deferred, "沒有延後的裁定")}
          {group("已結束", closed, "還沒有已結束的裁定", { collapsible: true })}
        </div>

        {/* 右欄工作區：批次結果 → 批次處理 → 詳情；寬視窗停在視窗內並自己捲動 */}
        <div className="decision-pane" ref={paneRef}>
          {batchOutcome && (
            <section className="decision-batch-results" aria-label="批次結果">
              {/* 送出批次後焦點移到這裡（送出鈕已隨批次面板消失） */}
              <h3 ref={resultsHeadingRef} tabIndex={-1}>
                批次結果 <HelpTip label="批次結果">每一項各自保存，不會形成概括授權。</HelpTip>
              </h3>
              <ul>
                {batchOutcome.results.map((result) => (
                  <li key={result.requestId} className={result.ok ? "is-ok" : "is-failed"}>
                    {result.question}：{result.ok ? "已保存" : `失敗（${result.message}）`}
                  </li>
                ))}
              </ul>
            </section>
          )}
          {batchDecisions.length > 0 && (
            <BatchPanel
              client={client}
              decisions={batchDecisions}
              drafts={batch}
              onDraft={(id, draft) => setBatch((current) => ({ ...current, [id]: draft }))}
              onCancel={() => {
                // 批次面板（連同這顆按鈕）即將消失：焦點先移到詳情標題，詳情的處理區會在那下方重新出現
                detailHeadingRef.current?.focus();
                setBatch({});
              }}
              onSubmitted={async (response) => {
                const questions = new Map(batchDecisions.map((d) => [d.id, d.question]));
                setBatchOutcome({
                  results: response.results.map((result) => ({
                    requestId: result.request_id,
                    question: questions.get(result.request_id) ?? result.request_id,
                    ok: result.ok,
                    message: result.ok ? null : (ERROR_MESSAGES[result.error] ?? result.message),
                  })),
                });
                setBatch({});
                onChange?.();
                await load();
              }}
              reload={load}
            />
          )}
          {selected && (
            <DecisionDetail
              key={selected.id}
              client={client}
              workspace={workspace}
              decision={selected}
              all={decisions}
              timeZone={timeZone}
              resumedCount={resumed?.requestId === selected.id ? resumed.count : null}
              draft={draftOf(selected.id)}
              onDraft={(draft) => setDetailDraft({ requestId: selected.id, draft })}
              batchMode={batchDecisions.length > 0}
              inBatch={selected.id in batch}
              batchable={actionable.includes(selected)}
              headingRef={detailHeadingRef}
              takeNavRequest={takeNavRequest}
              onAnswered={async (count) => {
                // 留在剛處理的這筆，讓使用者看到裁定結果與「已恢復」
                setSelectedId(selected.id);
                setResumed({ requestId: selected.id, count });
                onChange?.();
                await load();
              }}
              onMerged={async (targetId) => {
                select(targetId, true);
                onChange?.();
                await load();
              }}
              onSelect={(requestId) => select(requestId, true)}
              onOpenPage={onOpenPage}
              reload={pinAndReload(selected.id)}
            />
          )}
        </div>
      </div>
    </DecisionPage>
  );
}

interface ActionFieldsProps {
  decision: DecisionRequestView;
  draft: Draft;
  onChange: (draft: Draft) => void;
  /** radio 群組名稱的前綴；詳情與批次面板可能同時顯示同一筆。 */
  name: string;
}

/** 依 allowed_actions 提供處理方式，並依選到的方式顯示選項、文字或延後時數。 */
function ActionFields({ decision, draft, onChange, name }: ActionFieldsProps) {
  const set = (patch: Partial<Draft>) => onChange({ ...draft, ...patch });
  const { action } = draft;
  return (
    <>
      {/* 處理方式：一排可換行的選項鈕，選到的那個以強調色標示 */}
      <fieldset className="decision-actions">
        <legend>處理方式</legend>
        <div className="decision-action-list">
          {decision.allowed_actions.map((item) => (
            <label key={item} className="decision-action">
              <input
                type="radio"
                name={`${name}-action`}
                value={item}
                checked={action === item}
                onChange={() => set({ action: item })}
              />
              {ACTION_LABELS[item]}
            </label>
          ))}
        </div>
      </fieldset>
      {action !== null && OPTION_REQUIRED.has(action) && (
        <fieldset className="decision-option-choices">
          <legend>選項</legend>
          <div className="choice-list">
            {decision.options.map((option) => {
              const recommended = option.id === decision.recommendation;
              return (
                <label key={option.id} className="choice decision-choice">
                  {/* 畫面上的雜湊會縮短、「建議」是徽章，所以可存取名稱明寫完整的選項文字 */}
                  <input
                    type="radio"
                    name={`${name}-option`}
                    value={option.id}
                    aria-label={`${option.label}${recommended ? "（建議）" : ""}`}
                    checked={draft.optionId === option.id}
                    onChange={() => set({ optionId: option.id })}
                  />
                  <span className="decision-choice-label">
                    <WithHashes text={option.label} copy={false} />
                    {recommended && <span className="badge badge-success">建議</span>}
                  </span>
                </label>
              );
            })}
          </div>
        </fieldset>
      )}
      {action === "defer" && (
        <div className="decision-inline">
          <label className="decision-inline-field">
            延後多久
            <select value={draft.deferMinutes} onChange={(e) => set({ deferMinutes: Number(e.target.value) })}>
              {DEFER_CHOICES.map((choice) => (
                <option key={choice.minutes} value={choice.minutes}>
                  {choice.label}
                </option>
              ))}
            </select>
          </label>
          {/* 帶有授權意涵，直接顯示，不收在「?」說明裡 */}
          <span className="decision-note">延後不是同意，工作仍在等待。</span>
        </div>
      )}
      {action !== null && action !== "defer" && (
        <label className="decision-note-field">
          {NOTE_LABELS[action]}
          <textarea value={draft.note} onChange={(e) => set({ note: e.target.value })} />
        </label>
      )}
    </>
  );
}

interface BatchPanelProps {
  client: DecisionClient;
  decisions: DecisionRequestView[];
  drafts: Record<string, Draft>;
  onDraft: (requestId: string, draft: Draft) => void;
  onCancel: () => void;
  onSubmitted: (response: BatchResponse) => Promise<void>;
  reload: () => Promise<void>;
}

/**
 * 批次處理：每一項各自選擇處理方式並各自保存，沒有「全部套用同一個答案」的捷徑。
 * 每一項各自有「採用建議」（只套用到那一項）。
 */
function BatchPanel({ client, decisions, drafts, onDraft, onCancel, onSubmitted, reload }: BatchPanelProps) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const headingRef = useRef<HTMLHeadingElement>(null);
  const submitRef = useRef<HTMLButtonElement>(null);
  const requests = decisions.map((d) => toAnswer(d, drafts[d.id] ?? EMPTY_DRAFT));
  const complete = requests.every((request) => request !== null);
  const ready = requests.filter((request) => request !== null).length;
  // 送出失敗：送出鈕送出中被停用，結束後回到送出鈕（仍不能送出時到批次標題）
  useFocusAfterBusy(busy, () => [submitRef.current, headingRef.current]);

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (!complete) return;
    setBusy(true);
    setError(null);
    try {
      const response = await client.batchDecisions(
        decisions.map((d, index) => ({ ...requests[index]!, workspace_id: d.workspace_id, request_id: d.id })),
      );
      await onSubmitted(response);
    } catch (e) {
      setError(failureMessage(e));
      await reload();
    } finally {
      setBusy(false);
    }
  };

  return (
    <form className="decision-batch" aria-label="批次處理" onSubmit={(event) => void submit(event)}>
      <div className="decision-batch-head">
        <h3 ref={headingRef} tabIndex={-1}>
          批次處理（{decisions.length} 項）{" "}
          <HelpTip label="批次處理">逐項選擇處理方式；每一項各自保存，不會形成概括授權。</HelpTip>
        </h3>
        {/* 選了幾項、填妥幾項：還差的每一項在下面各自標出 */}
        <p className="decision-batch-progress tabular">
          已填妥 {ready}／{decisions.length} 項
        </p>
      </div>
      {decisions.map((decision) => {
        const draft = drafts[decision.id] ?? EMPTY_DRAFT;
        const missing = missingOf(decision, draft);
        const recommendation = adoptableRecommendation(decision);
        return (
          <fieldset key={decision.id} className="decision-batch-item">
            {/* 問題可能很長：限 2 行，完整內容在 title 與詳情；legend 只放問題（它是這一組的名稱） */}
            <legend>
              <span className="clamp-2" title={decision.question}>
                {decision.question}
              </span>
            </legend>
            <div className="decision-batch-item-head">
              <p className={missing === null ? "dot dot-success decision-batch-state" : "dot dot-warning decision-batch-state"}>
                {missing ?? "已填妥"}
              </p>
              {/* 只設定這一項（接受方案＋建議的選項），備註保留 */}
              {recommendation !== null && (
                <button
                  type="button"
                  className="btn-sm"
                  onClick={() => onDraft(decision.id, { ...draft, action: "accept", optionId: recommendation })}
                >
                  採用建議
                </button>
              )}
            </div>
            <ActionFields
              decision={decision}
              draft={draft}
              onChange={(next) => onDraft(decision.id, next)}
              name={`batch-${decision.id}`}
            />
          </fieldset>
        );
      })}
      <div className="form-actions">
        <button type="submit" ref={submitRef} disabled={busy || !complete}>
          送出批次（{decisions.length} 項）
        </button>
        <button type="button" onClick={onCancel} disabled={busy}>
          取消批次
        </button>
        {!complete && <p className="decision-hint">每一項都選好處理方式、填妥必填欄位後才能送出</p>}
      </div>
      {error && <ErrorNotice title="批次沒有送出">{error}</ErrorNotice>}
    </form>
  );
}

interface DetailProps {
  client: DecisionClient & AuthorizationClient;
  workspace: Workspace;
  decision: DecisionRequestView;
  /** 同一 Workspace 的全部請求，用來找合併目標與可連結的請求。 */
  all: DecisionRequestView[];
  timeZone?: string;
  /** 剛回答完恢復了幾個工作；沒有剛回答時為 null。 */
  resumedCount: number | null;
  /** 處理區的草稿（由外層保存：勾進批次時沿用）。 */
  draft: Draft;
  onDraft: (draft: Draft) => void;
  /** 批次處理進行中：詳情只讀，處理方式一律在批次面板選，畫面上不會同時有兩份處理區。 */
  batchMode: boolean;
  /** 這筆已勾進批次。 */
  inBatch: boolean;
  /** 這筆可以勾進批次（可處理、未延後）。 */
  batchable: boolean;
  /** 詳情標題（外層在取消批次時聚焦它；從連結換筆時這裡聚焦它）。 */
  headingRef: RefObject<HTMLHeadingElement | null>;
  /** 掛載時呼叫一次：使用者剛換到這筆時回傳要不要聚焦標題（要捲到詳情開頭）；不是使用者換筆時回傳 null。 */
  takeNavRequest: (requestId: string) => { focus: boolean } | null;
  onAnswered: (resumedCount: number) => Promise<void>;
  onMerged: (targetId: string) => Promise<void>;
  onSelect: (requestId: string) => void;
  onOpenPage?: (page: DecisionLinkPage) => void;
  reload: () => Promise<void>;
}

function DecisionDetail({
  client,
  workspace,
  decision,
  all,
  timeZone,
  resumedCount,
  draft,
  onDraft,
  batchMode,
  inBatch,
  batchable,
  headingRef,
  takeNavRequest,
  onAnswered,
  onMerged,
  onSelect,
  onOpenPage,
  reload,
}: DetailProps) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const articleRef = useRef<HTMLElement>(null);
  const formRef = useRef<HTMLFormElement>(null);
  const submitRef = useRef<HTMLButtonElement>(null);
  const outcomeHeadingRef = useRef<HTMLHeadingElement>(null);
  // 按了「採用建議」：處理區依新的草稿展開後，確定處理區（連同「送出裁定」）在視窗內
  const revealForm = useRef(false);
  const isPending = decision.status === "pending";
  // 批次處理進行中時詳情只讀：處理區換成一句說明，處理方式在批次面板選
  const showForm = isPending && !batchMode;
  const permission = decision.permission;
  const request = toAnswer(decision, draft);
  // 「採用建議」：一鍵設定處理方式＝接受方案＋建議的選項
  const recommendation = showForm ? adoptableRecommendation(decision) : null;
  const adopt = (optionId: string) => {
    revealForm.current = true;
    onDraft({ ...draft, action: "accept", optionId });
  };

  useLayoutEffect(() => {
    if (!revealForm.current) return;
    revealForm.current = false;
    // 送出列黏在右欄底部；把整個處理區捲進來，選好的處理方式與送出鈕一起看得到（jsdom 沒有 scrollIntoView）
    formRef.current?.scrollIntoView?.({ block: "nearest" });
  }, [draft]);

  // 使用者換到這筆時（詳情以 key 重新掛載）把右欄捲到這筆的開頭；上方有批次面板時捲過它。
  // 從詳情裡的連結換筆時，原本的連結已隨舊詳情消失，焦點移到這筆的標題。
  // 只在掛載時判斷一次，所以 takeNavRequest 與 decision.id 不列為相依
  useLayoutEffect(() => {
    const nav = takeNavRequest(decision.id);
    if (nav === null) return;
    const article = articleRef.current;
    const pane = article?.parentElement;
    if (article && pane) pane.scrollTop += article.getBoundingClientRect().top - pane.getBoundingClientRect().top;
    if (nav.focus) headingRef.current?.focus({ preventScroll: true });
  }, []);

  // 送出後：成功時處理區換成裁定結果，焦點移到結果標題；失敗時回到送出鈕
  useFocusAfterBusy(busy, () => [outcomeHeadingRef.current, submitRef.current, headingRef.current]);

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (request === null) return;
    setBusy(true);
    setError(null);
    try {
      const response = await client.answerDecision(decision.workspace_id, decision.id, request);
      onDraft(EMPTY_DRAFT);
      await onAnswered(response.resumed_task_ids.length);
    } catch (e) {
      // 例如已被回答過、內容已變更或目標版本已變更（409）：顯示說明，並重新載入讓畫面反映最新狀態
      setError(failureMessage(e));
      await reload();
    } finally {
      setBusy(false);
    }
  };

  // 讀起來像一份簡報：問題 → 狀態提醒 → 為什麼需要你決定 → 要做的事 → 選項與取捨 → 證據 → 阻擋範圍
  // → 允許本次操作／修改授權 → 處理方式與送出（或裁定結果）→ 相似問題
  return (
    <article ref={articleRef} className="decision-detail" aria-label="裁定詳情">
      <header className="decision-detail-header">
        <h3 ref={headingRef} tabIndex={-1}>
          {decision.question}
        </h3>
        <div className="badge-group">
          <span className="badge badge-plain">{KIND_LABELS[decision.kind]}</span>
          <span className={STATUS_TONES[decision.status]}>{STATUS_LABELS[decision.status]}</span>
          <span className={urgencyBadge(decision)}>緊急程度：{URGENCY_LABELS[decision.urgency]}</span>
        </div>
        <p className="meta">
          <span className="decision-detail-task">
            Task：
            {onOpenPage ? (
              // 工作頁沒有單一 Task 的網址，前往工作頁後在看板上找這個 Task
              <button type="button" className="decision-link-button" title="前往工作頁" onClick={() => onOpenPage("tasks")}>
                {decision.task_title}
              </button>
            ) : (
              decision.task_title
            )}
          </span>
          <span className="tabular">提出於 {formatTimestamp(decision.created_at, timeZone)}</span>
          {isPending && !decision.overdue && decision.deadline_at !== null && (
            <span className="decision-deadline tabular">期限：{formatTimestamp(decision.deadline_at, timeZone)}</span>
          )}
        </p>
      </header>

      {isPending && decision.deferred && decision.deferred_until !== null && (
        <p className="callout decision-deferred" role="status">
          延後至 {formatTimestamp(decision.deferred_until, timeZone)}{" "}
          <span className="decision-note">延後不是同意，工作仍在等待。</span>
        </p>
      )}
      {isPending && decision.overdue && (
        <p className="callout callout-warning decision-overdue" role="status">
          已逾期，尚未裁定
          {decision.deadline_at !== null && `（期限 ${formatTimestamp(decision.deadline_at, timeZone)}）`}{" "}
          <HelpTip label="逾期">逾期只是提醒，工作仍在等你的裁定。</HelpTip>
        </p>
      )}
      {isPending && permission?.prior_use && (
        <div className="decision-block">
          <p className="callout callout-warning decision-prior-use" role="alert">
            這個操作先前已放行過一次（{formatTimestamp(permission.prior_use.used_at, timeZone)}），結果未知。請先確認是否已生效，再決定要不要再允許一次。
          </p>
          <details className="decision-more">
            <summary>技術細節</summary>
            <dl className="facts">
              <dt>先前放行的 Run</dt>
              <dd>
                <code className="mono">{permission.prior_use.run_id}</code>
              </dd>
            </dl>
          </details>
        </div>
      )}

      <div className="decision-block">
        <h4>為什麼需要你決定</h4>
        <p className="decision-reason">
          <WithHashes text={decision.reason} />
        </p>
      </div>

      {decision.permission && (
        <section className="decision-block decision-request" aria-label="權限提示">
          <h4>權限提示</h4>
          <dl className="facts">
            <dt>工具</dt>
            <dd>
              <span className="mono">{decision.permission.tool_name}</span>
            </dd>
            <dt>能力</dt>
            <dd>{decision.permission.capability_label ?? decision.permission.capability ?? "未對應授權能力"}</dd>
            <dt>摘要</dt>
            <dd>
              <WithHashes text={decision.permission.summary} />
            </dd>
          </dl>
          <details className="decision-more">
            <summary>工具輸入</summary>
            <pre>{JSON.stringify(decision.permission.input, null, 2)}</pre>
          </details>
        </section>
      )}

      {decision.authority && (
        <section className="decision-block decision-request" aria-label="決策權限攔截">
          <h4 className="decision-heading-tip">
            決策權限攔截 <HelpTip label="決策權限攔截">Controller 想自行處理這件事，但決策權限規定必須送裁定。</HelpTip>
          </h4>
          <dl className="facts">
            <dt>事項</dt>
            <dd>{decision.authority.matter_label}</dd>
            <dt>摘要</dt>
            <dd>
              <WithHashes text={decision.authority.summary} />
            </dd>
          </dl>
          {onOpenPage && (
            <button type="button" className="btn-sm" onClick={() => onOpenPage("authority")}>
              調整決策權限
            </button>
          )}
        </section>
      )}

      {decision.options.length > 0 && (
        <div className="decision-block">
          <h4>選項與取捨</h4>
          <ul className="decision-options">
            {decision.options.map((option) => {
              const recommended = option.id === decision.recommendation;
              return (
                <li key={option.id} className={recommended ? "decision-option is-recommended" : "decision-option"}>
                  <span className="decision-option-head">
                    <span className="decision-option-label">
                      <WithHashes text={option.label} />
                    </span>
                    {recommended && <span className="badge badge-success decision-recommended">建議</span>}
                    {recommended && recommendation !== null && (
                      <button
                        type="button"
                        className="btn-sm decision-adopt"
                        onClick={() => adopt(option.id)}
                      >
                        採用建議
                      </button>
                    )}
                  </span>
                  {option.tradeoff && (
                    <span className="decision-tradeoff">
                      取捨：
                      <WithHashes text={option.tradeoff} />
                    </span>
                  )}
                </li>
              );
            })}
          </ul>
        </div>
      )}

      {decision.evidence.length > 0 && (
        <div className="decision-block">
          <h4>證據</h4>
          <ul className="decision-evidence">
            {decision.evidence.map((item, index) => (
              <li key={index}>{isPathLike(item) ? <span className="mono">{item}</span> : <WithHashes text={item} />}</li>
            ))}
          </ul>
        </div>
      )}

      <div className="decision-block">
        <h4 className="decision-heading-tip">
          阻擋範圍 <HelpTip label="阻擋範圍">只有這些工作受影響，其他工作不受阻擋。</HelpTip>
        </h4>
        {decision.blocking.length === 0 ? (
          <p className="empty-note">沒有工作被這個裁定阻擋</p>
        ) : (
          <ul className="decision-blocking">
            {decision.blocking.map((item) => (
              <li key={item.task_id}>
                {item.task_title}（{TASK_STATUS_LABELS.get(item.task_status) ?? item.task_status}）
              </li>
            ))}
          </ul>
        )}
      </div>

      {/* 「允許本次操作」與「修改 Workspace 授權」分開：一次性批准不會變成永久擴權 */}
      {permission && (
        <section className="decision-block decision-approve-once" aria-label="允許本次操作">
          <h4>允許本次操作</h4>
          {/* 帶有授權意涵，直接顯示，不收在「?」說明裡 */}
          <p className="decision-note">只允許這個 Task 的這一個操作，用過一次就失效，不會修改 Workspace 授權。</p>
          {permission.binding ? (
            <BindingView binding={permission.binding} />
          ) : (
            <p>這個請求建立得較早，沒有綁定的操作；允許後也不會被沿用。</p>
          )}
          {permission.rejected_approval && (
            <p className="callout callout-warning decision-rejected-approval">
              先前的單次批准不能沿用：
              {MISMATCH_LABELS[permission.rejected_approval.reason]}
            </p>
          )}
          {(permission.binding || permission.rejected_approval) && (
            <details className="decision-more">
              <summary>技術細節</summary>
              <dl className="facts">
                {permission.binding && (
                  <>
                    <dt>要求雜湊</dt>
                    <dd>
                      <span className="copy-text">
                        <code className="mono" title={permission.binding.request_hash}>
                          {shortHash(permission.binding.request_hash)}
                        </code>
                        <CopyButton text={permission.binding.request_hash} label="複製要求雜湊" />
                      </span>
                    </dd>
                  </>
                )}
                {permission.rejected_approval && (
                  <>
                    <dt>不能沿用的批准</dt>
                    <dd>
                      <code className="mono">{permission.rejected_approval.request_id}</code>
                    </dd>
                  </>
                )}
              </dl>
            </details>
          )}
        </section>
      )}
      {permission && isPending && (
        <AuthorizationChange client={client} workspace={workspace} capability={permission.capability} />
      )}

      {/* 處理區的送出列停在右欄底部（sticky），處理區本身隨內容捲動、不蓋住上方內容；送出失敗的說明放在處理區裡一起看得到 */}
      {showForm ? (
        <form ref={formRef} className="decision-form" onSubmit={(event) => void submit(event)}>
          <ActionFields decision={decision} draft={draft} onChange={onDraft} name={`decision-${decision.id}`} />
          {error && <ErrorNotice title="送出失敗">{error}</ErrorNotice>}
          <div className="form-actions">
            <button type="submit" ref={submitRef} disabled={busy || request === null}>
              送出裁定
            </button>
          </div>
        </form>
      ) : isPending ? (
        <p className="callout decision-batch-note">
          {inBatch
            ? "這筆已加入批次，請在上方的「批次處理」選擇處理方式並送出。"
            : `批次處理進行中，送出或取消批次後才能在這裡單筆處理${batchable ? "；也可以在清單勾選，把這筆加入批次" : ""}。`}
        </p>
      ) : (
        <>
          <DecisionOutcomeView
            decision={decision}
            all={all}
            timeZone={timeZone}
            resumedCount={resumedCount}
            headingRef={outcomeHeadingRef}
            onSelect={onSelect}
          />
          {error && <ErrorNotice title="送出失敗">{error}</ErrorNotice>}
        </>
      )}

      {/* 相似問題與手動連結很少用，放在處理區之後 */}
      <RelatedDecisions client={client} decision={decision} all={all} onSelect={onSelect} onMerged={onMerged} reload={reload} />

      <details className="decision-more">
        <summary>詳細資訊</summary>
        <p className="meta">
          <span>目標版本：契約第 {decision.target.contract_version} 版</span>
          {decision.target.authorization_revision !== null && <span>授權第 {decision.target.authorization_revision} 版</span>}
        </p>
      </details>
    </article>
  );
}

/** 單次批准綁定的操作（要求雜湊在區塊的「技術細節」）。 */
function BindingView({ binding }: { binding: ApprovalBinding }) {
  const params = Object.entries(binding.params);
  return (
    <dl className="facts decision-binding">
      <dt>操作</dt>
      <dd>
        <code className="mono">{binding.operation}</code>
      </dd>
      <dt>目標資源</dt>
      <dd>{binding.target === null ? "無" : <code className="mono">{binding.target}</code>}</dd>
      <dt>重要參數</dt>
      <dd>
        {params.length === 0 ? (
          "無"
        ) : (
          <ul>
            {params.map(([key, value]) => (
              <li key={key}>
                {key}：{JSON.stringify(value)}
              </li>
            ))}
          </ul>
        )}
      </dd>
      <dt>成果版本</dt>
      <dd>{binding.artifact_version === "" ? "尚無成果" : <code className="mono">{binding.artifact_version}</code>}</dd>
    </dl>
  );
}

type ChangeState =
  | { step: "idle" }
  | { step: "blocked"; message: string }
  | { step: "review"; templateId: string | null; grants: Grant[]; summary: ConfirmationSummary }
  | { step: "saved"; revision: number };

interface ChangeProps {
  client: AuthorizationClient;
  workspace: Workspace;
  /** 這次操作對應的授權能力；null 表示無法歸類。 */
  capability: Capability | null;
}

/**
 * 修改 Workspace 授權：把這項能力（目標為 Workspace 目錄）加入目前授權，檢視變更與風險後確認。
 * 這是永久擴權，與單次批准分開；修改授權不會回答這個裁定請求。
 */
function AuthorizationChange({ client, workspace, capability }: ChangeProps) {
  const [state, setState] = useState<ChangeState>({ step: "idle" });
  const [acknowledgedExpansion, setAcknowledgedExpansion] = useState(false);
  const [acknowledgedRisk, setAcknowledgedRisk] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const headingRef = useRef<HTMLHeadingElement>(null);
  const startRef = useRef<HTMLButtonElement>(null);
  const confirmRef = useRef<HTMLButtonElement>(null);
  // 按鈕送出中被停用、成功後被檢視或結果取代：回到還能按的按鈕，否則到區塊標題
  useFocusAfterBusy(busy, () => [startRef.current, confirmRef.current, headingRef.current]);

  const start = async () => {
    setError(null);
    if (capability === null) {
      // 按鈕即將換成說明：焦點先移到區塊標題
      headingRef.current?.focus();
      setState({ step: "blocked", message: "無法歸類的操作不能透過修改授權放行，請用允許本次操作或拒絕" });
      return;
    }
    setBusy(true);
    try {
      const { effective } = await client.getAuthorization(workspace.id);
      if (effective === null) {
        setState({ step: "blocked", message: "這個 Workspace 尚未確認授權，請先到授權設定頁確認授權" });
      } else if (effective.grants.some((grant) => grant.capability === capability)) {
        setState({ step: "blocked", message: "這個能力已授權但目標或限制不涵蓋這次操作，請到授權設定頁調整" });
      } else {
        const grants = [...effective.grants, { capability, targets: [workspace.path], restrictions: [] }];
        const summary = await client.summarizeAuthorization(workspace.id, grants);
        setState({ step: "review", templateId: effective.source.template_id, grants, summary });
      }
    } catch (e) {
      setError(reasonOf(e));
    } finally {
      setBusy(false);
    }
  };

  const confirm = async (templateId: string | null, grants: Grant[], expectedRevision: number | null) => {
    setBusy(true);
    setError(null);
    try {
      const revision = await client.confirmAuthorization(workspace.id, {
        templateId,
        grants,
        acknowledgedRisk,
        acknowledgedExpansion: true,
        expectedRevision,
      });
      setState({ step: "saved", revision: revision.revision });
    } catch (e) {
      setError(reasonOf(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="decision-block decision-authorization-change" aria-label="修改 Workspace 授權">
      <h4 ref={headingRef} tabIndex={-1} className="decision-heading-tip">
        修改 Workspace 授權{" "}
        <HelpTip label="修改 Workspace 授權">
          把這項能力加入 Workspace 授權後，這個 Workspace 的所有 Task 都能使用，不再逐次詢問。修改授權不會回答這個請求。
        </HelpTip>
      </h4>
      {state.step === "idle" && (
        <button type="button" ref={startRef} disabled={busy} onClick={() => void start()}>
          修改 Workspace 授權
        </button>
      )}
      {state.step === "blocked" && <p className="callout">{state.message}</p>}
      {state.step === "review" && (
        <div className="inset decision-authorization-review">
          <div>
            <h5>新增的能力與目標</h5>
            <ul>
              {(state.summary.changes?.changes ?? [])
                .filter((change) => change.expands)
                .map((change) => (
                  <li key={change.capability}>
                    {change.label}：{change.added_targets.join("、")}
                  </li>
                ))}
            </ul>
          </div>
          {state.summary.changes && state.summary.changes.impact.length > 0 && (
            <div>
              <h5>影響</h5>
              <ul>
                {state.summary.changes.impact.map((line) => (
                  <li key={line}>{line}</li>
                ))}
              </ul>
            </div>
          )}
          {state.summary.risks.map((risk) => (
            <p key={risk} className="callout callout-warning">
              {risk}
            </p>
          ))}
          <div className="decision-acks">
            <label>
              <input
                type="checkbox"
                checked={acknowledgedExpansion}
                onChange={(e) => setAcknowledgedExpansion(e.target.checked)}
              />
              我了解這會擴大 Workspace 授權
            </label>
            {state.summary.requires_risk_acknowledgement && (
              <label>
                <input type="checkbox" checked={acknowledgedRisk} onChange={(e) => setAcknowledgedRisk(e.target.checked)} />
                我了解並接受上述風險
              </label>
            )}
          </div>
          <div className="form-actions">
            <button
              type="button"
              ref={confirmRef}
              disabled={busy || !acknowledgedExpansion || (state.summary.requires_risk_acknowledgement && !acknowledgedRisk)}
              onClick={() => void confirm(state.templateId, state.grants, state.summary.changes?.base_revision ?? null)}
            >
              確認修改授權
            </button>
          </div>
        </div>
      )}
      {state.step === "saved" && (
        <p className="callout callout-success" role="status">
          授權已更新為第 {state.revision} 版；這個裁定請求仍待回答
        </p>
      )}
      {error && <ErrorNotice title="無法修改授權">{error}</ErrorNotice>}
    </section>
  );
}

interface OutcomeProps {
  decision: DecisionRequestView;
  all: DecisionRequestView[];
  timeZone?: string;
  resumedCount: number | null;
  /** 結果標題：單筆送出成功後處理區換成這裡，焦點移到標題。 */
  headingRef: Ref<HTMLHeadingElement>;
  onSelect: (requestId: string) => void;
}

/** 已結束的請求：顯示裁定結果、已失效原因或合併目標，不提供任何處理。 */
function DecisionOutcomeView({ decision, all, timeZone, resumedCount, headingRef, onSelect }: OutcomeProps) {
  if (decision.status === "superseded") {
    return (
      <section className="decision-outcome decision-superseded" aria-label="裁定結果">
        <h4 ref={headingRef} tabIndex={-1}>
          {STATUS_LABELS.superseded}
        </h4>
        <p>{SUPERSEDED_NOTE}</p>
        {decision.status_reason && <p>原因：{decision.status_reason}</p>}
      </section>
    );
  }
  if (decision.status === "merged") {
    const targetId = decision.merged_into;
    const target = all.find((d) => d.id === targetId);
    return (
      <section className="decision-outcome" aria-label="裁定結果">
        <h4 ref={headingRef} tabIndex={-1}>
          已合併
        </h4>
        {targetId !== null && (
          <p>
            已合併到{" "}
            <button type="button" className="decision-link-button" onClick={() => onSelect(targetId)}>
              {target?.question ?? targetId}
            </button>
            ，請在那邊一併處理。
          </p>
        )}
      </section>
    );
  }
  const outcome = decision.decision;
  return (
    <section className="decision-outcome" aria-label="裁定結果">
      <h4 ref={headingRef} tabIndex={-1}>
        裁定結果
      </h4>
      {outcome ? (
        <>
          <p>處理方式：{ACTION_LABELS[outcome.action]}</p>
          {outcome.option_label && <p>選擇：{outcome.option_label}</p>}
          {outcome.note && (
            <p>
              {OUTCOME_NOTE_LABELS[outcome.action]}：{outcome.note}
            </p>
          )}
          <p className="meta decision-outcome-meta">
            <span>{outcome.decided_by}</span>
            <span className="tabular">{formatTimestamp(outcome.decided_at, timeZone)}</span>
            {outcome.batch_id !== null && <span>批次處理（此項各自保存）</span>}
          </p>
        </>
      ) : (
        <p>已回答</p>
      )}
      {decision.permission?.approval_use && (
        <>
          <p>
            這個批准已於 {formatTimestamp(decision.permission.approval_use.used_at, timeZone)} 使用過，已失效
          </p>
          <details className="decision-more">
            <summary>技術細節</summary>
            <dl className="facts">
              <dt>使用批准的 Run</dt>
              <dd>
                <code className="mono">{decision.permission.approval_use.run_id}</code>
              </dd>
            </dl>
          </details>
        </>
      )}
      {resumedCount !== null && (
        <p className="callout callout-success" role="status">
          已恢復：{resumedCount} 個工作
        </p>
      )}
    </section>
  );
}

interface RelatedProps {
  client: DecisionClient;
  decision: DecisionRequestView;
  all: DecisionRequestView[];
  onSelect: (requestId: string) => void;
  onMerged: (targetId: string) => Promise<void>;
  reload: () => Promise<void>;
}

/**
 * 相似問題：顯示連結與合併來源；pending 的請求可以合併到同種類的連結請求，
 * 或手動連結另一個請求（很少用，收在預設收合的「手動連結其他請求」）。
 */
function RelatedDecisions({ client, decision, all, onSelect, onMerged, reload }: RelatedProps) {
  const [mergeInto, setMergeInto] = useState("");
  const [linkTo, setLinkTo] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<{ title: string; message: string } | null>(null);
  const headingRef = useRef<HTMLHeadingElement>(null);
  const mergeRef = useRef<HTMLButtonElement>(null);
  const linkSelectRef = useRef<HTMLSelectElement>(null);
  const linkRef = useRef<HTMLButtonElement>(null);
  const lastAction = useRef<"merge" | "link">("merge");
  // 合併、連結的按鈕送出中被停用；連結成功後選單清空、按鈕仍停用：焦點回到觸發的按鈕或連結選單，否則到區塊標題
  // （合併成功時換到目標請求，由新詳情聚焦它的標題）
  useFocusAfterBusy(busy, () =>
    lastAction.current === "merge"
      ? [mergeRef.current, headingRef.current]
      : [linkRef.current, linkSelectRef.current, headingRef.current],
  );
  const isPending = decision.status === "pending";
  const byId = new Map(all.map((d) => [d.id, d]));
  // 權限提示不能合併；目標必須是同種類、仍待處理的連結請求
  const mergeTargets =
    isPending && decision.kind !== "permission"
      ? decision.links
          .map((link) => byId.get(link.id))
          .filter((d): d is DecisionRequestView => d !== undefined && d.status === "pending" && d.kind === decision.kind)
      : [];
  const linked = new Set(decision.links.map((link) => link.id));
  const linkCandidates = isPending ? all.filter((d) => d.id !== decision.id && !linked.has(d.id)) : [];

  if (decision.links.length === 0 && decision.merged_from.length === 0 && linkCandidates.length === 0) return null;

  const run = async (kind: "merge" | "link", action: () => Promise<void>) => {
    lastAction.current = kind;
    setBusy(true);
    setError(null);
    try {
      await action();
    } catch (e) {
      setError({ title: kind === "merge" ? "合併失敗" : "連結失敗", message: failureMessage(e) });
      await reload();
    } finally {
      setBusy(false);
    }
  };

  const merge = () =>
    run("merge", async () => {
      const target = byId.get(mergeInto)!;
      const response = await client.mergeDecision(decision.workspace_id, decision.id, {
        into: target.id,
        expected_revision: decision.revision,
        into_expected_revision: target.revision,
      });
      await onMerged(response.decision.id);
    });

  const link = () =>
    run("link", async () => {
      await client.linkDecision(decision.workspace_id, decision.id, linkTo);
      setLinkTo("");
      await reload();
    });

  const linkList = (title: string, items: DecisionRequestView["links"]) =>
    items.length > 0 && (
      <div>
        <h5>{title}</h5>
        <ul className="decision-links">
          {items.map((item) => (
            <li key={item.id}>
              <button type="button" className="decision-link-button" onClick={() => onSelect(item.id)}>
                {item.question}
              </button>
              （{STATUS_LABELS[item.status]}）
            </li>
          ))}
        </ul>
      </div>
    );

  return (
    <section className="decision-block decision-related" aria-label="相似問題">
      <h4 ref={headingRef} tabIndex={-1}>
        相似問題
      </h4>
      {decision.links.length === 0 && decision.merged_from.length === 0 && (
        <p className="empty-note">還沒有連結的相似問題</p>
      )}
      {linkList("已連結", decision.links)}
      {linkList("已合併進來", decision.merged_from)}
      {mergeTargets.length > 0 && (
        <div className="decision-related-action">
          <label>
            合併到
            <select value={mergeInto} onChange={(e) => setMergeInto(e.target.value)}>
              <option value="">選擇要合併到的請求</option>
              {/* 原生選單以最長選項為最小寬度，選項文字縮短到 60 字以內 */}
              {mergeTargets.map((d) => (
                <option key={d.id} value={d.id}>
                  {optionLabel(d.question)}
                </option>
              ))}
            </select>
          </label>
          <button type="button" ref={mergeRef} disabled={busy || mergeInto === ""} onClick={() => void merge()}>
            合併
          </button>
          <HelpTip label="合併">合併後這個請求不再單獨處理，由目標請求一併裁定。</HelpTip>
        </div>
      )}
      {linkCandidates.length > 0 && (
        <details className="decision-more">
          <summary>手動連結其他請求</summary>
          <div className="decision-related-action">
            <label>
              連結另一個請求
              <select ref={linkSelectRef} value={linkTo} onChange={(e) => setLinkTo(e.target.value)}>
                <option value="">選擇要連結的請求</option>
                {linkCandidates.map((d) => (
                  <option key={d.id} value={d.id}>
                    {optionLabel(d.question)}
                  </option>
                ))}
              </select>
            </label>
            <button type="button" ref={linkRef} disabled={busy || linkTo === ""} onClick={() => void link()}>
              連結
            </button>
          </div>
        </details>
      )}
      {error && <ErrorNotice title={error.title}>{error.message}</ErrorNotice>}
    </section>
  );
}

/** 決策權限頁的外框：載入中、失敗與正常畫面共用同一個 region、頁面標題與一句用途說明。 */
function AuthorityPage({ children }: { children: ReactNode }) {
  const headingId = useId();
  return (
    <section className="page decision-authority" aria-labelledby={headingId}>
      <header className="page-header">
        <h2 id={headingId}>決策權限</h2>
      </header>
      <p className="page-lede">
        決定 Controller 遇到這些事時可以自己處理、必須送裁定，還是只能由你發起。每一項都能改成任何層級，收窄或放寬都可以；「內建」是預設值，供參考。
      </p>
      {children}
    </section>
  );
}

/** 決策權限表（spec D-15）：每個事項都可以設為任何層級，收窄或放寬都由使用者決定。 */
export function DecisionAuthorityPanel({ client, workspace }: { client: DecisionClient; workspace: Workspace }) {
  const [table, setTable] = useState<AuthorityTable | null>(null);
  const [draft, setDraft] = useState<Record<string, AuthorityLevel>>({});
  const [loadError, setLoadError] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);
  const [busy, setBusy] = useState(false);
  const saveRef = useRef<HTMLButtonElement>(null);
  const savedRef = useRef<HTMLSpanElement>(null);
  const errorRef = useRef<HTMLDivElement>(null);
  // 儲存後沒有未儲存的修改，「儲存決策權限」被停用：焦點移到「已儲存」或錯誤說明
  useFocusAfterBusy(busy, () => [saveRef.current, savedRef.current, errorRef.current]);

  const apply = (next: AuthorityTable) => {
    setTable(next);
    setDraft(Object.fromEntries(next.entries.map((entry) => [entry.matter, entry.effective])));
  };

  const load = useCallback(async () => {
    try {
      apply(await client.decisionAuthority(workspace.id));
      setLoadError(null);
    } catch (e) {
      setLoadError(reasonOf(e));
    }
  }, [client, workspace.id]);

  useEffect(() => {
    setTable(null);
    setError(null);
    setSaved(false);
    void load();
  }, [load]);

  if (loadError !== null) {
    return (
      <AuthorityPage>
        <ErrorNotice title="無法取得決策權限">{loadError}</ErrorNotice>
      </AuthorityPage>
    );
  }
  if (table === null) {
    return (
      <AuthorityPage>
        <p className="empty-note">載入中…</p>
      </AuthorityPage>
    );
  }

  const changedCount = table.entries.filter((entry) => draft[entry.matter] !== entry.effective).length;
  const changed = changedCount > 0;

  const save = async () => {
    setBusy(true);
    setError(null);
    setSaved(false);
    try {
      // 取代式：只列出和內建不同的事項，沒列出的回到內建值
      const entries = table.entries
        .filter((entry) => draft[entry.matter] !== entry.builtin)
        .map((entry) => ({ matter: entry.matter, effective: draft[entry.matter] }));
      apply(await client.updateDecisionAuthority(workspace.id, { expected_revision: table.revision, entries }));
      setSaved(true);
    } catch (e) {
      setError(failureMessage(e));
      await load();
    } finally {
      setBusy(false);
    }
  };

  return (
    <AuthorityPage>
      <div className="decision-authority-sheet">
        <div className="table-wrap">
          <table className="data-table decision-authority-table">
            <colgroup>
              <col />
              <col className="decision-authority-level" />
              <col className="decision-authority-level" />
              <col className="decision-authority-setting" />
            </colgroup>
            <thead>
              <tr>
                <th>事項</th>
                <th>內建</th>
                <th>目前生效</th>
                <th>Workspace 設定</th>
              </tr>
            </thead>
            <tbody>
              {table.entries.map((entry) => {
                // 和內建不同的列以左側色條與徽章標出：放寬（琥珀）或收窄（中性）
                const kind = overrideKind(entry.builtin, entry.effective);
                const classes = [kind === null ? "" : `is-${kind}`, draft[entry.matter] !== entry.effective ? "is-dirty" : ""]
                  .filter(Boolean)
                  .join(" ");
                return (
                  <tr key={entry.matter} className={classes || undefined}>
                    <td className="decision-authority-matter">{entry.label}</td>
                    <td>{AUTHORITY_LABELS[entry.builtin]}</td>
                    <td>
                      <span className="decision-authority-effective">
                        <span>{AUTHORITY_LABELS[entry.effective]}</span>
                        {kind !== null && <span className={OVERRIDE_TONES[kind]}>{OVERRIDE_LABELS[kind]}</span>}
                      </span>
                    </td>
                    <td>
                      <select
                        aria-label={`Workspace 設定：${entry.label}`}
                        value={draft[entry.matter]}
                        onChange={(e) => setDraft({ ...draft, [entry.matter]: e.target.value as AuthorityLevel })}
                      >
                        {AUTHORITY_ORDER.map((level) => (
                          <option key={level} value={level}>
                            {`${AUTHORITY_LABELS[level]}${level === entry.builtin ? "（內建）" : ""}`}
                          </option>
                        ))}
                      </select>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      </div>
      <div className="form-actions decision-authority-actions">
        <button type="button" ref={saveRef} className="btn-primary" disabled={busy || !changed} onClick={() => void save()}>
          儲存決策權限
        </button>
        {changed && <span className="meta">已修改 {changedCount} 項，尚未儲存</span>}
        {saved && !changed && (
          <span ref={savedRef} tabIndex={-1} className="decision-saved">
            已儲存（第 {table.revision} 版）
          </span>
        )}
      </div>
      {error && (
        <ErrorNotice title="儲存失敗" noticeRef={errorRef}>
          {error}
        </ErrorNotice>
      )}
    </AuthorityPage>
  );
}
