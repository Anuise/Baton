import { useCallback, useEffect, useState, type FormEvent } from "react";

import { COLUMNS } from "./TaskBoard";
import { formatTimestamp } from "./time";
import {
  errorCode,
  type AnswerRequest,
  type AuthorityLevel,
  type AuthorityTable,
  type BatchResponse,
  type DecisionAction,
  type DecisionClient,
  type DecisionKind,
  type DecisionList,
  type DecisionOverview,
  type DecisionRequestView,
  type DecisionStatus,
  type DecisionUrgency,
  type Workspace,
} from "./runtimeClient";

type ViewState =
  | { state: "loading" }
  | { state: "ready"; list: DecisionList }
  | { state: "failed"; reason: string };

interface Props {
  client: DecisionClient;
  workspace: Workspace;
  /** 顯示用的時區；省略時使用系統時區（spec D-33），只有測試會指定。 */
  timeZone?: string;
  /** 回答、批次或合併成功後通知外層（例如重新整理頂部的待裁定數，spec D-21）。 */
  onChange?: () => void;
}

const KIND_LABELS: Record<DecisionKind, string> = { permission: "權限提示", agent: "Agent 請求", authority: "權限表攔截" };
const URGENCY_LABELS: Record<DecisionUrgency, string> = { low: "低", normal: "一般", high: "高" };
const STATUS_LABELS: Record<DecisionStatus, string> = { pending: "待處理", answered: "已回答", superseded: "已過期", merged: "已合併" };
const TASK_STATUS_LABELS = new Map(COLUMNS);

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

const OVERVIEW_ITEMS: [keyof DecisionOverview, string][] = [
  ["pending", "待處理"],
  ["actionable", "可處理"],
  ["deferred", "已延後"],
  ["overdue", "已逾期"],
  ["high_urgency", "高緊急"],
  ["answered", "已回答"],
  ["superseded", "已過期"],
  ["merged", "已合併"],
];

const AUTHORITY_LABELS: Record<AuthorityLevel, string> = {
  autonomous: "Controller 可自主處理",
  requires_decision: "必須送裁定",
  human_only: "只能人工發起",
};

/** 需要特別說明的錯誤代碼；其餘沿用 Runtime 的說明。 */
const ERROR_MESSAGES: Record<string, string> = {
  decision_superseded: "目標版本已變更，這個請求已過期，不能套用。",
  decision_revision_mismatch: "這個請求的內容已變更，已重新整理，請確認後再送出。",
  authority_revision_mismatch: "決策權限表已被修改，已重新載入，請重新確認。",
};

function reasonOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function failureMessage(error: unknown): string {
  const code = errorCode(error);
  return (code !== null ? ERROR_MESSAGES[code] : undefined) ?? reasonOf(error);
}

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

/**
 * 裁定分頁：先看總覽，再逐項處理。左側分組列出請求，右側顯示選取那筆的詳情與處理方式；
 * 可處理的請求能勾選後逐項批次處理。逾期與延後都只是標示，不代表任何決定。
 */
export function DecisionCenter({ client, workspace, timeZone, onChange }: Props) {
  const [loaded, setLoaded] = useState<ViewState>({ state: "loading" });
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [resumed, setResumed] = useState<{ requestId: string; count: number } | null>(null);
  // 勾選進批次的請求與各自的處理草稿
  const [batch, setBatch] = useState<Record<string, Draft>>({});
  const [batchOutcome, setBatchOutcome] = useState<BatchOutcome | null>(null);

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
    setBatch({});
    setBatchOutcome(null);
    void load();
  }, [load]);

  if (loaded.state === "loading") return <section aria-label="裁定中心">載入中…</section>;
  if (loaded.state === "failed") {
    return (
      <section aria-label="裁定中心">
        <p role="alert">無法取得裁定清單：{loaded.reason}</p>
      </section>
    );
  }

  const { overview, decisions } = loaded.list;
  const { actionable, deferred, closed } = partition(decisions);
  const selected = decisions.find((d) => d.id === selectedId) ?? actionable[0] ?? deferred[0] ?? closed[0] ?? null;
  // 重新載入後已不可處理的請求自動離開批次
  const batchDecisions = actionable.filter((d) => d.id in batch);

  const toggleBatch = (decision: DecisionRequestView) =>
    setBatch((current) => {
      if (!(decision.id in current)) return { ...current, [decision.id]: EMPTY_DRAFT };
      const rest = { ...current };
      delete rest[decision.id];
      return rest;
    });

  const listItem = (decision: DecisionRequestView, selectable: boolean) => (
    <li key={decision.id} className={decision.id === selected?.id ? "decision-selected" : undefined}>
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
        aria-label={`查看裁定 ${decision.question}`}
        aria-current={decision.id === selected?.id}
        onClick={() => setSelectedId(decision.id)}
      >
        <span className="decision-question">{decision.question}</span>
        <span className="decision-task">{decision.task_title}</span>
        <span className="decision-meta">
          <span className="decision-kind">{KIND_LABELS[decision.kind]}</span>
          <span className={`decision-urgency decision-urgency-${decision.urgency}`}>
            緊急程度：{URGENCY_LABELS[decision.urgency]}
          </span>
          {decision.status === "pending" && decision.overdue && <span className="decision-overdue">已逾期，尚未裁定</span>}
          {decision.status === "pending" && decision.deferred && decision.deferred_until !== null && (
            <span className="decision-deferred">延後至 {formatTimestamp(decision.deferred_until, timeZone)}</span>
          )}
          {decision.status !== "pending" && (
            <span className={`decision-status decision-status-${decision.status}`}>{STATUS_LABELS[decision.status]}</span>
          )}
        </span>
      </button>
    </li>
  );

  const group = (title: string, items: DecisionRequestView[], empty: string, selectable = false) => (
    <>
      <h3>
        {title}（{items.length}）
      </h3>
      {items.length === 0 ? (
        <p className="decision-empty">{empty}</p>
      ) : (
        <ul className="decision-list">{items.map((d) => listItem(d, selectable))}</ul>
      )}
    </>
  );

  // 處理過的請求留在畫面上，讓使用者看到結果或錯誤說明
  const pinAndReload = (requestId: string) => async () => {
    setSelectedId(requestId);
    await load();
  };

  return (
    <section className="decision-center" aria-label="裁定中心">
      <h2>裁定：{workspace.name}</h2>
      <section className="decision-overview" aria-label="裁定總覽">
        <dl>
          {OVERVIEW_ITEMS.map(([key, label]) => (
            <div key={key} className={`decision-overview-item decision-overview-${key}`}>
              <dt>{label}</dt>
              <dd>{overview[key]}</dd>
            </div>
          ))}
        </dl>
      </section>

      {batchDecisions.length > 0 && (
        <BatchPanel
          client={client}
          decisions={batchDecisions}
          drafts={batch}
          onDraft={(id, draft) => setBatch((current) => ({ ...current, [id]: draft }))}
          onCancel={() => setBatch({})}
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
      {batchOutcome && (
        <section className="decision-batch-results" aria-label="批次結果">
          <h3>批次結果</h3>
          <p className="decision-scope-note">每一項各自保存，不會形成概括授權。</p>
          <ul>
            {batchOutcome.results.map((result) => (
              <li key={result.requestId} className={result.ok ? "decision-batch-ok" : "decision-batch-failed"}>
                {result.question}：{result.ok ? "已保存" : `失敗（${result.message}）`}
              </li>
            ))}
          </ul>
        </section>
      )}

      <div className="decision-layout">
        <div className="decision-lists">
          {group("可處理", actionable, "沒有可處理的裁定", true)}
          {group("已延後", deferred, "沒有延後的裁定")}
          {group("已結束", closed, "還沒有已結束的裁定")}
        </div>
        {selected && (
          <DecisionDetail
            key={selected.id}
            client={client}
            decision={selected}
            all={decisions}
            timeZone={timeZone}
            resumedCount={resumed?.requestId === selected.id ? resumed.count : null}
            onAnswered={async (count) => {
              // 留在剛處理的這筆，讓使用者看到裁定結果與「已恢復」
              setSelectedId(selected.id);
              setResumed({ requestId: selected.id, count });
              onChange?.();
              await load();
            }}
            onMerged={async (targetId) => {
              setSelectedId(targetId);
              onChange?.();
              await load();
            }}
            onSelect={setSelectedId}
            reload={pinAndReload(selected.id)}
          />
        )}
      </div>

      <DecisionAuthorityPanel client={client} workspace={workspace} />
    </section>
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
      <fieldset className="decision-actions">
        <legend>處理方式</legend>
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
      </fieldset>
      {action !== null && OPTION_REQUIRED.has(action) && (
        <fieldset>
          <legend>選項</legend>
          {decision.options.map((option) => (
            <label key={option.id} className="decision-option">
              <input
                type="radio"
                name={`${name}-option`}
                value={option.id}
                checked={draft.optionId === option.id}
                onChange={() => set({ optionId: option.id })}
              />
              {option.label}
              {option.id === decision.recommendation && "（建議）"}
            </label>
          ))}
        </fieldset>
      )}
      {action === "defer" && (
        <>
          <label>
            延後多久
            <select value={draft.deferMinutes} onChange={(e) => set({ deferMinutes: Number(e.target.value) })}>
              {DEFER_CHOICES.map((choice) => (
                <option key={choice.minutes} value={choice.minutes}>
                  {choice.label}
                </option>
              ))}
            </select>
          </label>
          <p className="decision-scope-note">延後不是同意，工作仍在等待。</p>
        </>
      )}
      {action !== null && action !== "defer" && (
        <label>
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

/** 批次處理：每一項各自選擇處理方式並各自保存，沒有「全部套用同一個答案」的捷徑。 */
function BatchPanel({ client, decisions, drafts, onDraft, onCancel, onSubmitted, reload }: BatchPanelProps) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const requests = decisions.map((d) => toAnswer(d, drafts[d.id] ?? EMPTY_DRAFT));
  const complete = requests.every((request) => request !== null);

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
      <h3>批次處理（{decisions.length} 項）</h3>
      <p className="decision-scope-note">逐項選擇處理方式；每一項各自保存，不會形成概括授權。</p>
      {decisions.map((decision) => (
        <fieldset key={decision.id} className="decision-batch-item">
          <legend>{decision.question}</legend>
          <ActionFields
            decision={decision}
            draft={drafts[decision.id] ?? EMPTY_DRAFT}
            onChange={(draft) => onDraft(decision.id, draft)}
            name={`batch-${decision.id}`}
          />
        </fieldset>
      ))}
      {!complete && <p className="decision-scope-note">每一項都要選好處理方式並填妥必填欄位才能送出。</p>}
      <button type="submit" disabled={busy || !complete}>
        送出批次（{decisions.length} 項）
      </button>
      <button type="button" onClick={onCancel} disabled={busy}>
        取消批次
      </button>
      {error && <p role="alert">{error}</p>}
    </form>
  );
}

interface DetailProps {
  client: DecisionClient;
  decision: DecisionRequestView;
  /** 同一 Workspace 的全部請求，用來找合併目標與可連結的請求。 */
  all: DecisionRequestView[];
  timeZone?: string;
  /** 剛回答完恢復了幾個工作；沒有剛回答時為 null。 */
  resumedCount: number | null;
  onAnswered: (resumedCount: number) => Promise<void>;
  onMerged: (targetId: string) => Promise<void>;
  onSelect: (requestId: string) => void;
  reload: () => Promise<void>;
}

function DecisionDetail({ client, decision, all, timeZone, resumedCount, onAnswered, onMerged, onSelect, reload }: DetailProps) {
  const [draft, setDraft] = useState<Draft>(EMPTY_DRAFT);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const isPending = decision.status === "pending";
  const request = toAnswer(decision, draft);

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (request === null) return;
    setBusy(true);
    setError(null);
    try {
      const response = await client.answerDecision(decision.workspace_id, decision.id, request);
      setDraft(EMPTY_DRAFT);
      await onAnswered(response.resumed_task_ids.length);
    } catch (e) {
      // 例如已被回答過、內容已變更或目標版本已變更（409）：顯示說明，並重新載入讓畫面反映最新狀態
      setError(failureMessage(e));
      await reload();
    } finally {
      setBusy(false);
    }
  };

  return (
    <article className="decision-detail" aria-label="裁定詳情">
      <h3>{decision.question}</h3>
      <p className="decision-detail-meta">
        {KIND_LABELS[decision.kind]}・{STATUS_LABELS[decision.status]}・Task：{decision.task_title}・緊急程度：
        {URGENCY_LABELS[decision.urgency]}・提出於 {formatTimestamp(decision.created_at, timeZone)}
      </p>
      <p className="decision-detail-meta">
        目標版本：契約第 {decision.target.contract_version} 版
        {decision.target.authorization_revision !== null && `・授權第 ${decision.target.authorization_revision} 版`}
      </p>
      {isPending && decision.deferred && decision.deferred_until !== null && (
        <p className="decision-deferred" role="status">
          延後至 {formatTimestamp(decision.deferred_until, timeZone)}（延後不是同意，工作仍在等待）
        </p>
      )}
      {isPending && decision.overdue && (
        <p className="decision-overdue" role="status">
          已逾期，尚未裁定
          {decision.deadline_at !== null && `（期限 ${formatTimestamp(decision.deadline_at, timeZone)}）`}。逾期只是提醒，工作仍在等你的裁定。
        </p>
      )}
      {isPending && !decision.overdue && decision.deadline_at !== null && (
        <p className="decision-deadline">期限：{formatTimestamp(decision.deadline_at, timeZone)}</p>
      )}

      <h4>為什麼需要你決定</h4>
      <p>{decision.reason}</p>

      {decision.permission && (
        <section className="decision-permission" aria-label="權限提示">
          <h4>權限提示</h4>
          <dl>
            <dt>工具</dt>
            <dd>{decision.permission.tool_name}</dd>
            <dt>能力</dt>
            <dd>{decision.permission.capability_label ?? decision.permission.capability ?? "未對應授權能力"}</dd>
            <dt>摘要</dt>
            <dd>{decision.permission.summary}</dd>
          </dl>
          <details>
            <summary>工具輸入</summary>
            <pre>{JSON.stringify(decision.permission.input, null, 2)}</pre>
          </details>
        </section>
      )}

      {decision.authority && (
        <section className="decision-permission" aria-label="權限表攔截">
          <h4>權限表攔截</h4>
          <p className="decision-scope-note">Controller 想自行處理這件事，但決策權限表規定必須送裁定。</p>
          <dl>
            <dt>事項</dt>
            <dd>{decision.authority.matter_label}</dd>
            <dt>摘要</dt>
            <dd>{decision.authority.summary}</dd>
          </dl>
        </section>
      )}

      {decision.options.length > 0 && (
        <>
          <h4>選項與取捨</h4>
          <ul className="decision-options">
            {decision.options.map((option) => (
              <li key={option.id} className="decision-option">
                <span className="decision-option-label">{option.label}</span>
                {option.id === decision.recommendation && <span className="decision-recommended">建議</span>}
                {option.tradeoff && <span className="decision-tradeoff">取捨：{option.tradeoff}</span>}
              </li>
            ))}
          </ul>
        </>
      )}

      {decision.evidence.length > 0 && (
        <>
          <h4>證據</h4>
          <ul className="decision-evidence">
            {decision.evidence.map((item, index) => (
              <li key={index}>{item}</li>
            ))}
          </ul>
        </>
      )}

      <h4>阻擋範圍</h4>
      {decision.blocking.length === 0 ? (
        <p>目前沒有工作被這個裁定阻擋。</p>
      ) : (
        <ul className="decision-blocking">
          {decision.blocking.map((item) => (
            <li key={item.task_id}>
              {item.task_title}（{TASK_STATUS_LABELS.get(item.task_status) ?? item.task_status}）
            </li>
          ))}
        </ul>
      )}
      <p className="decision-scope-note">只有這些工作受影響，其他工作不受阻擋。</p>

      <RelatedDecisions client={client} decision={decision} all={all} onSelect={onSelect} onMerged={onMerged} reload={reload} />

      {isPending ? (
        <form className="decision-form" onSubmit={(event) => void submit(event)}>
          <ActionFields decision={decision} draft={draft} onChange={setDraft} name={`decision-${decision.id}`} />
          <button type="submit" disabled={busy || request === null}>
            送出裁定
          </button>
        </form>
      ) : (
        <DecisionOutcomeView decision={decision} all={all} timeZone={timeZone} resumedCount={resumedCount} onSelect={onSelect} />
      )}
      {error && (
        <p className="decision-error" role="alert">
          {error}
        </p>
      )}
    </article>
  );
}

interface OutcomeProps {
  decision: DecisionRequestView;
  all: DecisionRequestView[];
  timeZone?: string;
  resumedCount: number | null;
  onSelect: (requestId: string) => void;
}

/** 已結束的請求：顯示裁定結果、已過期原因或合併目標，不提供任何處理。 */
function DecisionOutcomeView({ decision, all, timeZone, resumedCount, onSelect }: OutcomeProps) {
  if (decision.status === "superseded") {
    return (
      <section className="decision-outcome decision-superseded" aria-label="裁定結果">
        <h4>已過期</h4>
        <p>目標版本已變更，這個請求不能套用，也不提供任何處理。</p>
        {decision.status_reason && <p>原因：{decision.status_reason}</p>}
      </section>
    );
  }
  if (decision.status === "merged") {
    const targetId = decision.merged_into;
    const target = all.find((d) => d.id === targetId);
    return (
      <section className="decision-outcome" aria-label="裁定結果">
        <h4>已合併</h4>
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
      <h4>裁定結果</h4>
      {outcome ? (
        <>
          <p>處理方式：{ACTION_LABELS[outcome.action]}</p>
          {outcome.option_label && <p>選擇：{outcome.option_label}</p>}
          {outcome.note && (
            <p>
              {OUTCOME_NOTE_LABELS[outcome.action]}：{outcome.note}
            </p>
          )}
          <p className="decision-detail-meta">
            {outcome.decided_by}・{formatTimestamp(outcome.decided_at, timeZone)}
            {outcome.batch_id !== null && "・批次處理（此項各自保存）"}
          </p>
        </>
      ) : (
        <p>已回答</p>
      )}
      {resumedCount !== null && <p role="status">已恢復：{resumedCount} 個工作</p>}
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

/** 相似問題：顯示連結與合併來源；pending 的請求可以合併到同種類的連結請求，或手動連結另一個請求。 */
function RelatedDecisions({ client, decision, all, onSelect, onMerged, reload }: RelatedProps) {
  const [mergeInto, setMergeInto] = useState("");
  const [linkTo, setLinkTo] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
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

  const run = async (action: () => Promise<void>) => {
    setBusy(true);
    setError(null);
    try {
      await action();
    } catch (e) {
      setError(failureMessage(e));
      await reload();
    } finally {
      setBusy(false);
    }
  };

  const merge = () =>
    run(async () => {
      const target = byId.get(mergeInto)!;
      const response = await client.mergeDecision(decision.workspace_id, decision.id, {
        into: target.id,
        expected_revision: decision.revision,
        into_expected_revision: target.revision,
      });
      await onMerged(response.decision.id);
    });

  const link = () =>
    run(async () => {
      await client.linkDecision(decision.workspace_id, decision.id, linkTo);
      setLinkTo("");
      await reload();
    });

  const linkList = (title: string, items: DecisionRequestView["links"]) =>
    items.length > 0 && (
      <>
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
      </>
    );

  return (
    <section className="decision-related" aria-label="相似問題">
      <h4>相似問題</h4>
      {linkList("已連結", decision.links)}
      {linkList("已合併進來", decision.merged_from)}
      {mergeTargets.length > 0 && (
        <div className="decision-related-action">
          <label>
            合併到
            <select value={mergeInto} onChange={(e) => setMergeInto(e.target.value)}>
              <option value="">選擇要合併到的請求</option>
              {mergeTargets.map((d) => (
                <option key={d.id} value={d.id}>
                  {d.question}
                </option>
              ))}
            </select>
          </label>
          <button type="button" disabled={busy || mergeInto === ""} onClick={() => void merge()}>
            合併
          </button>
          <p className="decision-scope-note">合併後這個請求不再單獨處理，由目標請求一併裁定。</p>
        </div>
      )}
      {linkCandidates.length > 0 && (
        <div className="decision-related-action">
          <label>
            連結另一個請求
            <select value={linkTo} onChange={(e) => setLinkTo(e.target.value)}>
              <option value="">選擇要連結的請求</option>
              {linkCandidates.map((d) => (
                <option key={d.id} value={d.id}>
                  {d.question}
                </option>
              ))}
            </select>
          </label>
          <button type="button" disabled={busy || linkTo === ""} onClick={() => void link()}>
            連結
          </button>
        </div>
      )}
      {error && <p role="alert">{error}</p>}
    </section>
  );
}

/** 決策權限表（spec D-15）：Workspace 只能把內建可自主處理的事項收窄為必須送裁定，不能放寬。 */
function DecisionAuthorityPanel({ client, workspace }: { client: DecisionClient; workspace: Workspace }) {
  const [table, setTable] = useState<AuthorityTable | null>(null);
  const [draft, setDraft] = useState<Record<string, AuthorityLevel>>({});
  const [loadError, setLoadError] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);
  const [busy, setBusy] = useState(false);

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
      <section className="decision-authority" aria-label="決策權限表">
        <h3>決策權限表</h3>
        <p role="alert">無法取得決策權限表：{loadError}</p>
      </section>
    );
  }
  if (table === null) {
    return (
      <section className="decision-authority" aria-label="決策權限表">
        <h3>決策權限表</h3>
        <p>載入中…</p>
      </section>
    );
  }

  const changed = table.entries.some((entry) => draft[entry.matter] !== entry.effective);

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
    <section className="decision-authority" aria-label="決策權限表">
      <h3>決策權限表</h3>
      <p className="decision-scope-note">
        列出哪些事情 Controller 可以自行處理、哪些一定會送裁定。Workspace 只能收窄，不能放寬。
      </p>
      <table>
        <thead>
          <tr>
            <th>事項</th>
            <th>內建</th>
            <th>目前生效</th>
            <th>Workspace 設定</th>
          </tr>
        </thead>
        <tbody>
          {table.entries.map((entry) => (
            <tr key={entry.matter}>
              <td>{entry.label}</td>
              <td>{AUTHORITY_LABELS[entry.builtin]}</td>
              <td>
                {AUTHORITY_LABELS[entry.effective]}
                {entry.narrowed && "（已收窄）"}
              </td>
              <td>
                {entry.builtin === "autonomous" ? (
                  <label>
                    <input
                      type="checkbox"
                      aria-label={`收窄為必須送裁定：${entry.label}`}
                      checked={draft[entry.matter] !== "autonomous"}
                      onChange={(e) =>
                        setDraft({ ...draft, [entry.matter]: e.target.checked ? "requires_decision" : "autonomous" })
                      }
                    />
                    收窄為必須送裁定
                  </label>
                ) : (
                  <span className="authority-locked">內建{AUTHORITY_LABELS[entry.builtin]}，Workspace 不能放寬</span>
                )}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      <button type="button" disabled={busy || !changed} onClick={() => void save()}>
        儲存權限表
      </button>
      {saved && <p className="decision-scope-note">已儲存（第 {table.revision} 版）</p>}
      {error && <p role="alert">{error}</p>}
    </section>
  );
}
