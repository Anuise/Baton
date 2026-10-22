import { useCallback, useEffect, useState, type FormEvent } from "react";

import { COLUMNS } from "./TaskBoard";
import { formatTimestamp } from "./time";
import type {
  ApprovalBinding,
  ApprovalMismatchReason,
  AuthorizationClient,
  Capability,
  ConfirmationSummary,
  DecisionClient,
  DecisionKind,
  DecisionRequestView,
  DecisionUrgency,
  Grant,
  Workspace,
} from "./runtimeClient";

type ViewState =
  | { state: "loading" }
  | { state: "ready"; decisions: DecisionRequestView[] }
  | { state: "failed"; reason: string };

interface Props {
  client: DecisionClient & AuthorizationClient;
  workspace: Workspace;
  /** 顯示用的時區；省略時使用系統時區（spec D-33），只有測試會指定。 */
  timeZone?: string;
  /** 回答成功後通知外層（例如重新整理頂部的待裁定數）。 */
  onChange?: () => void;
}

const KIND_LABELS: Record<DecisionKind, string> = { permission: "權限提示", agent: "Agent 請求" };
const URGENCY_LABELS: Record<DecisionUrgency, string> = { low: "低", normal: "一般", high: "高" };
const TASK_STATUS_LABELS = new Map(COLUMNS);
const MISMATCH_LABELS: Record<ApprovalMismatchReason, string> = {
  operation_changed: "操作不同",
  target_changed: "目標資源不同",
  params_changed: "重要參數不同",
  request_hash_changed: "要求內容不同",
  artifact_version_changed: "成果版本已改變",
};

function reasonOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** 裁定中心：待處理的放前面（各組內維持後端順序），已回答的收在另一區。 */
function partition(decisions: DecisionRequestView[]) {
  return {
    pending: decisions.filter((d) => d.status === "pending"),
    answered: decisions.filter((d) => d.status !== "pending"),
  };
}

/**
 * 裁定分頁：左側列出待處理與已回答的裁定請求，右側顯示選取那筆的詳情並讓使用者回答。
 * 逾期只是標示，不代表任何決定。
 */
export function DecisionCenter({ client, workspace, timeZone, onChange }: Props) {
  const [loaded, setLoaded] = useState<ViewState>({ state: "loading" });
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [resumed, setResumed] = useState<{ requestId: string; count: number } | null>(null);

  const load = useCallback(async () => {
    try {
      const { decisions } = await client.listDecisions(workspace.id);
      setLoaded({ state: "ready", decisions });
    } catch (e) {
      setLoaded({ state: "failed", reason: reasonOf(e) });
    }
  }, [client, workspace.id]);

  useEffect(() => {
    setLoaded({ state: "loading" });
    setSelectedId(null);
    setResumed(null);
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

  const { pending, answered } = partition(loaded.decisions);
  const selected = [...pending, ...answered].find((d) => d.id === selectedId) ?? pending[0] ?? answered[0] ?? null;

  const listItem = (decision: DecisionRequestView) => (
    <li key={decision.id} className={decision.id === selected?.id ? "decision-selected" : undefined}>
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
        </span>
      </button>
    </li>
  );

  return (
    <section className="decision-center" aria-label="裁定中心">
      <h2>裁定：{workspace.name}</h2>
      <div className="decision-layout">
        <div className="decision-lists">
          <h3>待處理（{pending.length}）</h3>
          {pending.length === 0 ? <p className="decision-empty">沒有待處理的裁定</p> : <ul className="decision-list">{pending.map(listItem)}</ul>}
          <h3>已回答（{answered.length}）</h3>
          {answered.length === 0 ? <p className="decision-empty">還沒有已回答的裁定</p> : <ul className="decision-list">{answered.map(listItem)}</ul>}
        </div>
        {selected && (
          <DecisionDetail
            key={selected.id}
            client={client}
            workspace={workspace}
            decision={selected}
            timeZone={timeZone}
            resumedCount={resumed?.requestId === selected.id ? resumed.count : null}
            onAnswered={async (count) => {
              // 留在剛回答的這筆，讓使用者看到裁定結果與「已恢復」
              setSelectedId(selected.id);
              setResumed({ requestId: selected.id, count });
              onChange?.();
              await load();
            }}
            reload={load}
          />
        )}
      </div>
    </section>
  );
}

interface DetailProps {
  client: DecisionClient & AuthorizationClient;
  workspace: Workspace;
  decision: DecisionRequestView;
  timeZone?: string;
  /** 剛回答完恢復了幾個工作；沒有剛回答時為 null。 */
  resumedCount: number | null;
  onAnswered: (resumedCount: number) => Promise<void>;
  reload: () => Promise<void>;
}

function DecisionDetail({ client, workspace, decision, timeZone, resumedCount, onAnswered, reload }: DetailProps) {
  const [optionId, setOptionId] = useState<string | null>(null);
  const [note, setNote] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const isPending = decision.status === "pending";
  const permission = decision.permission;

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (optionId === null) return;
    setBusy(true);
    setError(null);
    try {
      const response = await client.answerDecision(
        decision.workspace_id,
        decision.id,
        optionId,
        note.trim() === "" ? undefined : note.trim(),
      );
      await onAnswered(response.resumed_task_ids.length);
    } catch (e) {
      // 例如已被回答過（409）：顯示說明，並重新載入讓畫面反映最新狀態
      setError(reasonOf(e));
      await reload();
    } finally {
      setBusy(false);
    }
  };

  return (
    <article className="decision-detail" aria-label="裁定詳情">
      <h3>{decision.question}</h3>
      <p className="decision-detail-meta">
        {KIND_LABELS[decision.kind]}・Task：{decision.task_title}・緊急程度：{URGENCY_LABELS[decision.urgency]}・提出於{" "}
        {formatTimestamp(decision.created_at, timeZone)}
      </p>
      {isPending && decision.overdue && (
        <p className="decision-overdue" role="status">
          已逾期，尚未裁定
          {decision.deadline_at !== null && `（期限 ${formatTimestamp(decision.deadline_at, timeZone)}）`}。逾期只是提醒，工作仍在等你的裁定。
        </p>
      )}
      {isPending && !decision.overdue && decision.deadline_at !== null && (
        <p className="decision-deadline">期限：{formatTimestamp(decision.deadline_at, timeZone)}</p>
      )}
      {isPending && permission?.prior_use && (
        <p className="decision-prior-use" role="alert">
          這個操作先前已經放行過一次（{formatTimestamp(permission.prior_use.used_at, timeZone)}，Run{" "}
          {permission.prior_use.run_id}），結果未知。請先查證是否已經生效，再決定是否再允許一次。
        </p>
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

      {/* 「允許本次操作」與「修改 Workspace 授權」分開：一次性批准不會變成永久擴權 */}
      {permission && (
        <section className="decision-approve-once" aria-label="允許本次操作">
          <h4>允許本次操作</h4>
          <p>只允許這個 Task 的這一個操作，用過一次就失效，不會修改 Workspace 授權。</p>
          {permission.binding ? (
            <BindingView binding={permission.binding} />
          ) : (
            <p>這個請求建立於單次批准綁定之前，沒有綁定的操作，允許後也不會被沿用。</p>
          )}
          {permission.rejected_approval && (
            <p className="decision-rejected-approval">
              先前的單次批准（{permission.rejected_approval.request_id}）不能沿用：
              {MISMATCH_LABELS[permission.rejected_approval.reason]}
            </p>
          )}
        </section>
      )}
      {permission && isPending && (
        <AuthorizationChange client={client} workspace={workspace} capability={permission.capability} />
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

      {isPending ? (
        <form className="decision-form" onSubmit={(event) => void submit(event)}>
          <fieldset>
            <legend>選項</legend>
            {decision.options.map((option) => (
              <label key={option.id} className="decision-option">
                <input
                  type="radio"
                  name={`decision-${decision.id}`}
                  value={option.id}
                  checked={optionId === option.id}
                  onChange={() => setOptionId(option.id)}
                />
                <span className="decision-option-label">{option.label}</span>
                {option.id === decision.recommendation && <span className="decision-recommended">建議</span>}
                {option.tradeoff && <span className="decision-tradeoff">取捨：{option.tradeoff}</span>}
              </label>
            ))}
          </fieldset>
          <label>
            備註（選填）
            <textarea value={note} onChange={(e) => setNote(e.target.value)} />
          </label>
          <button type="submit" disabled={busy || optionId === null}>
            送出裁定
          </button>
          {error && <p role="alert">{error}</p>}
        </form>
      ) : (
        <section className="decision-outcome" aria-label="裁定結果">
          <h4>裁定結果</h4>
          {decision.decision ? (
            <>
              <p>
                選擇：{decision.decision.option_label}（{decision.decision.decided_by}，{formatTimestamp(decision.decision.decided_at, timeZone)}）
              </p>
              {decision.decision.note && <p>備註：{decision.decision.note}</p>}
            </>
          ) : (
            <p>已回答</p>
          )}
          {permission?.approval_use && (
            <p>
              這個批准已於 {formatTimestamp(permission.approval_use.used_at, timeZone)} 由 Run{" "}
              {permission.approval_use.run_id} 使用，已失效
            </p>
          )}
          {resumedCount !== null && <p role="status">已恢復：{resumedCount} 個工作</p>}
        </section>
      )}
    </article>
  );
}

/** 單次批准綁定的操作；要求雜湊只顯示前 12 碼。 */
function BindingView({ binding }: { binding: ApprovalBinding }) {
  const params = Object.entries(binding.params);
  return (
    <dl>
      <dt>操作</dt>
      <dd>{binding.operation}</dd>
      <dt>目標資源</dt>
      <dd>{binding.target ?? "無"}</dd>
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
      <dt>要求雜湊</dt>
      <dd title={binding.request_hash}>{binding.request_hash.slice(0, 12)}</dd>
      <dt>成果版本</dt>
      <dd>{binding.artifact_version === "" ? "尚無成果" : binding.artifact_version}</dd>
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

  const start = async () => {
    setError(null);
    if (capability === null) {
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

  const confirm = async (templateId: string | null, grants: Grant[]) => {
    setBusy(true);
    setError(null);
    try {
      const revision = await client.confirmAuthorization(workspace.id, {
        templateId,
        grants,
        acknowledgedRisk,
        acknowledgedExpansion: true,
      });
      setState({ step: "saved", revision: revision.revision });
    } catch (e) {
      setError(reasonOf(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="decision-authorization-change" aria-label="修改 Workspace 授權">
      <h4>修改 Workspace 授權</h4>
      <p>把這項能力加入 Workspace 授權後，這個 Workspace 的所有 Task 都能使用，不再逐次詢問。修改授權不會回答這個請求。</p>
      {state.step === "idle" && (
        <button type="button" disabled={busy} onClick={() => void start()}>
          修改 Workspace 授權
        </button>
      )}
      {state.step === "blocked" && <p>{state.message}</p>}
      {state.step === "review" && (
        <>
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
          {state.summary.changes && state.summary.changes.impact.length > 0 && (
            <>
              <h5>影響</h5>
              <ul>
                {state.summary.changes.impact.map((line) => (
                  <li key={line}>{line}</li>
                ))}
              </ul>
            </>
          )}
          {state.summary.risks.map((risk) => (
            <p key={risk} className="authorization-risk">
              {risk}
            </p>
          ))}
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
          <button
            type="button"
            disabled={busy || !acknowledgedExpansion || (state.summary.requires_risk_acknowledgement && !acknowledgedRisk)}
            onClick={() => void confirm(state.templateId, state.grants)}
          >
            確認修改授權
          </button>
        </>
      )}
      {state.step === "saved" && <p role="status">授權已更新為第 {state.revision} 版；這個裁定請求仍待回答</p>}
      {error && <p role="alert">{error}</p>}
    </section>
  );
}
