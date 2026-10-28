import { useEffect, useState, type FormEvent } from "react";

import { LongText } from "./LongText";
import { shortId, splitFirstSentence } from "./textFormat";
import { formatTimestamp } from "./time";
import {
  errorCode,
  type EvidenceKind,
  type FailureRoute,
  type SkipStageRequest,
  type StageAttempt,
  type StageAttemptOutcome,
  type StageEvidence,
  type StageGate,
  type StageState,
  type StageView,
  type WorkflowClient,
  type WorkflowList,
  type WorkflowStatus,
  type WorkflowView,
  type Workspace,
} from "./runtimeClient";

type Loaded = { state: "loading" } | { state: "ready"; list: WorkflowList } | { state: "failed"; reason: string };

interface Props {
  client: WorkflowClient;
  workspace: Workspace;
  /** 顯示用的時區；省略時使用系統時區（spec D-33），只有測試會指定。 */
  timeZone?: string;
}

const WORKFLOW_STATUS_LABELS: Record<WorkflowStatus, string> = {
  active: "進行中",
  blocked: "已阻擋",
  completed: "已完成",
};

const STAGE_STATE_LABELS: Record<StageState, string> = {
  pending: "待進行",
  current: "目前",
  blocked: "已阻擋",
  passed: "已通過",
  skipped: "已跳過",
  invalidated: "需重驗",
};

const GATE_LABELS: Record<Exclude<StageGate, "none">, string> = {
  decision: "裁定 Gate",
  safety: "安全 Gate",
};

const OUTCOME_LABELS: Record<StageAttemptOutcome, string> = {
  passed: "通過",
  incomplete: "缺少證據",
  blocked: "已阻擋",
  failed: "失敗",
  skipped: "已跳過",
  invalidated: "已失效",
};

const EVIDENCE_KIND_LABELS: Record<EvidenceKind, string> = {
  artifact: "產出物",
  decision: "裁定",
  task_accepted: "Task 已接受",
};

const EVIDENCE_STATUS_LABELS: Record<StageEvidence["status"], string> = {
  valid: "有效",
  not_found: "找不到",
  pending: "尚未完成",
  not_accepted: "未接受",
  build_changed: "build 已改變",
};

/** 狀態徽章的語意色（MASTER 5.4）。 */
const WORKFLOW_STATUS_TONES: Record<WorkflowStatus, string> = {
  active: "badge badge-accent",
  blocked: "badge badge-danger",
  completed: "badge badge-success",
};

const STAGE_STATE_TONES: Record<StageState, string> = {
  pending: "badge",
  current: "badge badge-accent",
  blocked: "badge badge-danger",
  passed: "badge badge-success",
  skipped: "badge",
  invalidated: "badge badge-danger",
};

const OUTCOME_TONES: Record<StageAttemptOutcome, string> = {
  passed: "badge badge-success",
  incomplete: "badge badge-warning",
  blocked: "badge badge-danger",
  failed: "badge badge-danger",
  skipped: "badge",
  invalidated: "badge badge-danger",
};

/** 使用者看到的 Workflow 已過時時的提示；重新載入後讓使用者再決定。 */
const REVISION_CHANGED_NOTICE = "Workflow 在檢視後已更新，已重新載入，請確認目前的 Stage 後再操作";

function reasonOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/**
 * 流程（票 43，spec D-21）：檢視 Workspace 的 Workflow，包括 Router 選擇的 Playbook 與版本、各 Stage 的條件與 Gate，
 * 以及 attempt 歷史。使用者可附原因跳過沒有 Gate 的目前 Stage；裁定與安全 Gate 不能跳過。
 */
export function WorkflowPage({ client, workspace, timeZone }: Props) {
  const [loaded, setLoaded] = useState<Loaded>({ state: "loading" });

  useEffect(() => {
    // 卸載後遲到的回應一律忽略
    let cancelled = false;
    client.workflows(workspace.id).then(
      (list) => !cancelled && setLoaded({ state: "ready", list }),
      (e: unknown) => !cancelled && setLoaded({ state: "failed", reason: reasonOf(e) }),
    );
    return () => {
      cancelled = true;
    };
  }, [client, workspace.id]);

  /** 跳過成功後以回應取代那個 Workflow。 */
  const replace = (view: WorkflowView) =>
    setLoaded((current) =>
      current.state === "ready"
        ? { state: "ready", list: { ...current.list, workflows: current.list.workflows.map((w) => (w.id === view.id ? view : w)) } }
        : current,
    );

  const reload = async () => {
    try {
      setLoaded({ state: "ready", list: await client.workflows(workspace.id) });
    } catch (e) {
      setLoaded({ state: "failed", reason: reasonOf(e) });
    }
  };

  return (
    <section className="workflow-page" aria-label="流程">
      <header className="more-header">
        <h2>流程</h2>
      </header>
      {loaded.state === "loading" && <p>載入中…</p>}
      {loaded.state === "failed" && <p role="alert">無法取得流程：{loaded.reason}</p>}
      {loaded.state === "ready" && loaded.list.workflows.length === 0 && (
        <p>還沒有 Workflow；Controller 開始工作時會依情境選擇 Playbook，並在這裡顯示各 Stage 的進度。</p>
      )}
      {loaded.state === "ready" &&
        loaded.list.workflows.map((workflow) => (
          <WorkflowCard
            key={workflow.id}
            workflow={workflow}
            timeZone={timeZone}
            onSkip={async (request) => replace(await client.skipStage(workspace.id, workflow.id, request))}
            onStale={reload}
          />
        ))}
    </section>
  );
}

function WorkflowCard({
  workflow,
  timeZone,
  onSkip,
  onStale,
}: {
  workflow: WorkflowView;
  timeZone?: string;
  onSkip: (request: SkipStageRequest) => Promise<void>;
  onStale: () => Promise<void>;
}) {
  const [reason, setReason] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const headingId = `workflow-${workflow.id}`;

  const skip = async (event: FormEvent, stage: StageView) => {
    event.preventDefault();
    if (reason.trim() === "") return;
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      await onSkip({ stage: stage.key, expected_revision: workflow.revision, reason });
      setReason("");
    } catch (e) {
      if (errorCode(e) === "workflow_revision_mismatch") {
        // 送出的 revision 已過時：重新載入，下次送出時帶最新的 revision
        setNotice(REVISION_CHANGED_NOTICE);
        await onStale();
      } else {
        setError(reasonOf(e));
      }
    } finally {
      setBusy(false);
    }
  };

  const stageTitle = (key: string) => workflow.stages.find((s) => s.key === key)?.title ?? key;

  return (
    <section className={`workflow card workflow-${workflow.status}`} aria-labelledby={headingId}>
      <header className="card-header">
        <h3 id={headingId}>
          {workflow.playbook_title} 第 {workflow.playbook_version} 版
        </h3>
        <span className={WORKFLOW_STATUS_TONES[workflow.status]}>{WORKFLOW_STATUS_LABELS[workflow.status]}</span>
      </header>
      {workflow.latest_playbook_version > workflow.playbook_version && (
        <p className="workflow-newer-version">
          已有第 {workflow.latest_playbook_version} 版；此 Workflow 維持第 {workflow.playbook_version} 版
        </p>
      )}
      <dl className="workflow-summary">
        <dt>Task</dt>
        <dd>
          {workflow.task_id === null ? (
            "尚未綁定 Task"
          ) : workflow.task_title === null ? (
            <code className="mono" title={workflow.task_id}>
              {shortId(workflow.task_id)}
            </code>
          ) : (
            <span className="clamp-2" title={workflow.task_title}>
              {workflow.task_title}
            </span>
          )}
        </dd>
        {workflow.status === "blocked" && workflow.blocked_reason !== null && (
          <>
            <dt>阻擋原因</dt>
            <dd className="workflow-status">
              <BlockedReason reason={workflow.blocked_reason} />
            </dd>
          </>
        )}
        <dt>Router 的理由</dt>
        <dd>
          <LongText text={workflow.route_reason} copyLabel="複製理由" className="workflow-route-reason" />
        </dd>
        <dt>建立於</dt>
        <dd>{formatTimestamp(workflow.created_at, timeZone)}</dd>
      </dl>
      {workflow.resume_stage !== null && (
        <p className="workflow-resume">
          Gate 失效後重驗中：重驗通過後，在「{stageTitle(workflow.resume_stage)}」之前仍有效的 Gate 會直接沿用
        </p>
      )}
      {notice && (
        <p className="workflow-notice" role="status">
          {notice}
        </p>
      )}
      {error && <p role="alert">{error}</p>}
      <ol className="workflow-stages" aria-label={`${workflow.playbook_title} 的 Stage`}>
        {workflow.stages.map((stage) => {
          // 只有未完成 Workflow 的目前 Stage 能操作；有 Gate 的不能跳過
          const isCurrent = workflow.status !== "completed" && stage.key === workflow.current_stage;
          return (
            <li key={stage.key} aria-label={stage.title} className={`workflow-stage stage-${stage.state}`}>
              <div className="workflow-stage-header">
                <span className="workflow-stage-title">{stage.title}</span>
                <span className={`workflow-stage-state ${STAGE_STATE_TONES[stage.state]}`}>
                  {STAGE_STATE_LABELS[stage.state]}
                </span>
                {stage.gate !== "none" && <span className="workflow-gate badge badge-warning">{GATE_LABELS[stage.gate]}</span>}
              </div>
              <dl className="workflow-stage-facts">
                {stage.method && (
                  <>
                    <dt>方法</dt>
                    <dd>{stage.method}</dd>
                  </>
                )}
                <TextList title="進入條件" stage={stage} items={stage.entry_conditions} />
                <TextList title="預期產出" stage={stage} items={stage.expected_outputs} />
                <TextList title="允許的動作" stage={stage} items={stage.allowed_actions} />
                {stage.exit_checks.length > 0 && (
                  <>
                    <dt>離開條件</dt>
                    <dd>
                      <ul className="workflow-stage-list" aria-label={`${stage.title} 的離開條件`}>
                        {stage.exit_checks.map((check) => (
                          <li key={check.key}>
                            {check.description}（需要：{EVIDENCE_KIND_LABELS[check.requires]}）
                          </li>
                        ))}
                      </ul>
                    </dd>
                  </>
                )}
                <dt>預算</dt>
                <dd>最多失敗 {stage.budget.max_attempts} 次</dd>
                <dt>失敗去向</dt>
                <dd>{failureRouteLabel(stage.failure_route, stageTitle)}</dd>
              </dl>
              {stage.attempts.length > 0 && (
                <div className="table-wrap">
                  <table className="data-table workflow-attempts" aria-label={`${stage.title} 的嘗試紀錄`}>
                    <colgroup>
                      <col style={{ width: "5.5em" }} />
                      <col style={{ width: "7em" }} />
                      <col style={{ width: "9.5em" }} />
                      <col />
                    </colgroup>
                    <thead>
                      <tr>
                        <th scope="col">次數</th>
                        <th scope="col">結果</th>
                        <th scope="col">時間</th>
                        <th scope="col">說明</th>
                      </tr>
                    </thead>
                    <tbody>
                      {stage.attempts.map((attempt) => (
                        <AttemptView key={attempt.id} attempt={attempt} timeZone={timeZone} />
                      ))}
                    </tbody>
                  </table>
                </div>
              )}
              {isCurrent && stage.gate !== "none" && <p className="workflow-gate-locked">此 Gate 不能跳過</p>}
              {isCurrent && stage.gate === "none" && (
                <form className="workflow-skip" onSubmit={(event) => void skip(event, stage)}>
                  <label>
                    跳過原因
                    <input value={reason} onChange={(e) => setReason(e.target.value)} />
                  </label>
                  <button type="submit" disabled={busy || reason.trim() === ""}>
                    跳過此 Stage
                  </button>
                </form>
              )}
            </li>
          );
        })}
      </ol>
    </section>
  );
}

/** Stage 事實中的一組清單（dt + dd），沒有項目時不顯示。 */
function TextList({ title, stage, items }: { title: string; stage: StageView; items: string[] }) {
  if (items.length === 0) return null;
  return (
    <>
      <dt>{title}</dt>
      <dd>
        <ul className="workflow-stage-list" aria-label={`${stage.title} 的${title}`}>
          {items.map((item, index) => (
            <li key={index}>{item}</li>
          ))}
        </ul>
      </dd>
    </>
  );
}

/** 阻擋原因：第一句完整顯示，其餘（技術細節）收合（MASTER 6.1 錯誤與警告）。 */
function BlockedReason({ reason }: { reason: string }) {
  const { summary, rest } = splitFirstSentence(reason);
  return (
    <div className="workflow-status-reason measure">
      <p>{summary}</p>
      {rest !== "" && (
        <details>
          <summary>技術細節</summary>
          <p>{rest}</p>
        </details>
      )}
    </div>
  );
}

function failureRouteLabel(route: FailureRoute, stageTitle: (key: string) => string): string {
  switch (route.kind) {
    case "retry":
      return "重試";
    case "return_to":
      return `回到「${stageTitle(route.stage)}」`;
    case "block":
      return "阻擋";
  }
}

/** 記錄者的顯示名稱；桌面記錄的是使用者本人，runtime 是 Runtime 記錄的失效。 */
const RECORDER_LABELS: Record<string, string> = { desktop: "使用者", runtime: "Runtime" };

/** 一次嘗試：結果、受驗 build、缺少的證據（失效時是失效原因）、證據與查驗狀態、說明與記錄者。 */
function AttemptView({ attempt, timeZone }: { attempt: StageAttempt; timeZone?: string }) {
  const recorder = RECORDER_LABELS[attempt.recorded_by] ?? attempt.recorded_by;
  return (
    <tr aria-label={`第 ${attempt.attempt} 次`} className={`workflow-attempt attempt-${attempt.outcome}`}>
      <td className="tabular">第 {attempt.attempt} 次</td>
      <td>
        <span className={OUTCOME_TONES[attempt.outcome]}>{OUTCOME_LABELS[attempt.outcome]}</span>
      </td>
      <td className="tabular">{formatTimestamp(attempt.created_at, timeZone)}</td>
      <td>
        <div className="workflow-attempt-detail">
          {attempt.build !== undefined && (
            <span>
              build：<code className="mono">{attempt.build}</code>
            </span>
          )}
          {attempt.missing.length > 0 && (
            <ul className="workflow-attempt-missing" aria-label={attempt.outcome === "invalidated" ? "失效原因" : "缺少的證據"}>
              {attempt.missing.map((missing, index) => (
                <li key={index}>
                  {missing.check}：{missing.reason}
                </li>
              ))}
            </ul>
          )}
          {attempt.evidence.length > 0 && (
            <ul className="workflow-attempt-evidence" aria-label="證據">
              {attempt.evidence.map((evidence, index) => (
                <li key={index}>
                  {evidence.check}：{EVIDENCE_KIND_LABELS[evidence.kind]}
                  {` ${evidence.artifact_id ?? evidence.decision_request_id ?? evidence.task_id ?? ""}`.trimEnd()}・
                  {EVIDENCE_STATUS_LABELS[evidence.status]}
                </li>
              ))}
            </ul>
          )}
          {attempt.note !== "" && <p className="workflow-attempt-note">{attempt.note}</p>}
          <span className="workflow-attempt-recorder">
            記錄者：{recorder}
            {attempt.profile_id !== null && `・帳號 ${attempt.profile_id}`}
          </span>
        </div>
      </td>
    </tr>
  );
}
