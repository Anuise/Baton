import { useEffect, useState } from "react";

import { LongText, ShortValue } from "./LongText";
import { formatTimestamp } from "./time";
import type {
  ExclusionReason,
  ExplorationCharter,
  ExplorationClient,
  ExplorationFinding,
  ExplorationFindingStatus,
  ExplorationOverview,
  ReproductionAttempt,
  ReproductionCase,
  ReproductionOutcome,
  ReproductionPurpose,
  VerificationVerdict,
  Workspace,
} from "./runtimeClient";
import { VERDICT_LABELS } from "./VerificationPage";

type Loaded = { state: "loading" } | { state: "ready"; overview: ExplorationOverview } | { state: "failed"; reason: string };

interface Props {
  client: ExplorationClient;
  workspace: Workspace;
  /** 顯示用的時區；省略時使用系統時區（spec D-33），只有測試會指定。 */
  timeZone?: string;
}

/** 疑點依狀態分區；疑點（suspected）只會出現在自己的區塊，絕不放進已確認缺陷。 */
const GROUPS: { key: string; title: string; note: string; statuses: ExplorationFindingStatus[]; empty: string }[] = [
  {
    key: "defects",
    title: "已確認缺陷",
    note: "Runtime 在 Verification Profile 的環境中以重現案例證實：對照旅程通過，而且案例以產品行為斷言失敗。",
    statuses: ["confirmed", "regressed"],
    empty: "沒有已確認缺陷。",
  },
  {
    key: "suspected",
    title: "疑點（未經重現）",
    note: "探索回報的疑點，還沒有經過重現，不能當作缺陷處理。",
    statuses: ["suspected"],
    empty: "沒有待重現的疑點。",
  },
  {
    key: "risks",
    title: "未驗風險",
    note: "重現時沒有出現原症狀，或目前無法驗證；保留為風險，沒有被駁回。",
    statuses: ["unverified_risk"],
    empty: "沒有未驗風險。",
  },
  {
    key: "fixed",
    title: "已修復・納入回歸",
    note: "同一個重現案例已經通過；之後每一次驗證都會執行它。",
    statuses: ["fixed"],
    empty: "沒有已修復的缺陷。",
  },
  {
    key: "excluded",
    title: "已排除",
    note: "附原因排除的疑點仍然保留，可以重新開啟。",
    statuses: ["excluded"],
    empty: "沒有已排除的疑點。",
  },
];

const EXCLUSION_LABELS: Record<ExclusionReason, string> = {
  environment: "環境故障",
  seed_data: "seed data",
  tool_limitation: "工具限制",
  expected_design: "預期設計",
};

const PURPOSE_LABELS: Record<ReproductionPurpose, string> = {
  reproduce: "確認重現",
  verify_fix: "修復驗證",
  regression: "回歸",
};

const OUTCOME_LABELS: Record<ReproductionOutcome, string> = {
  reproduced: "已重現",
  not_reproduced: "無法重現",
  inconclusive: "證據不足（不是產品缺陷）",
  fix_verified: "修復已驗證",
  still_failing: "仍然失敗",
  regressed: "回歸失敗",
  regression_passed: "回歸通過",
};

function reasonOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/**
 * 探索（票 47）：唯讀檢視 Workspace 各探索 Task 的章程，以及依狀態分開的疑點——已確認缺陷、疑點（未經重現）、
 * 未驗風險、已修復與已排除——和它們的重現紀錄。疑點只有經過 Runtime 的重現才會出現在已確認缺陷。
 */
export function ExplorationPage({ client, workspace, timeZone }: Props) {
  const [loaded, setLoaded] = useState<Loaded>({ state: "loading" });

  useEffect(() => {
    // 卸載後遲到的回應一律忽略
    let cancelled = false;
    client.explorationOverview(workspace.id).then(
      (overview) => !cancelled && setLoaded({ state: "ready", overview }),
      (e: unknown) => !cancelled && setLoaded({ state: "failed", reason: reasonOf(e) }),
    );
    return () => {
      cancelled = true;
    };
  }, [client, workspace.id]);

  const reload = async () => {
    try {
      setLoaded({ state: "ready", overview: await client.explorationOverview(workspace.id) });
    } catch (e) {
      setLoaded({ state: "failed", reason: reasonOf(e) });
    }
  };

  return (
    <section className="exploration-page" aria-label="探索">
      <h2>探索</h2>
      {loaded.state === "loading" && <p>載入中…</p>}
      {loaded.state !== "loading" && (
        <button type="button" className="exploration-reload" onClick={() => void reload()}>
          重新整理
        </button>
      )}
      {loaded.state === "failed" && <p role="alert">無法取得探索資料：{loaded.reason}</p>}
      {loaded.state === "ready" && (
        <>
          <section className="exploration-section" aria-labelledby="exploration-charters">
            <h3 id="exploration-charters">探索章程</h3>
            {loaded.overview.charters.length === 0 && <p>還沒有探索章程；Controller 為探索 Task 定下章程後會在這裡顯示。</p>}
            {loaded.overview.charters.map((charter) => (
              <CharterCard key={charter.id} charter={charter} timeZone={timeZone} />
            ))}
          </section>
          {GROUPS.map((group) => {
            const findings = loaded.overview.findings.filter((finding) => group.statuses.includes(finding.status));
            const headingId = `exploration-group-${group.key}`;
            return (
              <section
                key={group.key}
                className={`exploration-section exploration-group exploration-group-${group.key}`}
                aria-labelledby={headingId}
              >
                <h3 id={headingId}>{group.title}</h3>
                <p className="exploration-note">{group.note}</p>
                {findings.length === 0 && <p>{group.empty}</p>}
                {findings.map((finding) => (
                  <FindingCard key={finding.id} finding={finding} timeZone={timeZone} />
                ))}
              </section>
            );
          })}
        </>
      )}
    </section>
  );
}

function CharterCard({ charter, timeZone }: { charter: ExplorationCharter; timeZone?: string }) {
  const headingId = `exploration-charter-${charter.id}`;
  const task = charter.task_title ?? charter.task_id;
  return (
    <section className="card exploration-card" aria-labelledby={headingId}>
      <h4 id={headingId}>{task}</h4>
      <dl className="verification-summary">
        <dt>區域</dt>
        <dd>{charter.area}</dd>
        <dt>角色</dt>
        <dd>{charter.role}</dd>
        <dt>目標</dt>
        <dd>{charter.goal}</dd>
        <dt>步數上限</dt>
        <dd>{charter.max_steps} 步</dd>
        <dt>成本上限</dt>
        <dd>模型呼叫 {charter.max_model_calls} 次</dd>
        <dt>版本</dt>
        <dd>第 {charter.version} 版</dd>
        <dt>定義於</dt>
        <dd>{formatTimestamp(charter.created_at, timeZone)}</dd>
      </dl>
      <h5>禁止的副作用</h5>
      {charter.forbidden_side_effects.length === 0 ? (
        <p>沒有列出禁止的副作用。</p>
      ) : (
        <ul className="exploration-list" aria-label={`${task} 禁止的副作用`}>
          {charter.forbidden_side_effects.map((effect, index) => (
            <li key={index}>{effect}</li>
          ))}
        </ul>
      )}
    </section>
  );
}

function FindingCard({ finding, timeZone }: { finding: ExplorationFinding; timeZone?: string }) {
  const headingId = `exploration-finding-${finding.id}`;
  return (
    <section className={`card exploration-card exploration-finding finding-status-${finding.status}`} aria-labelledby={headingId}>
      <header className="card-header">
        <h4 id={headingId}>{finding.title}</h4>
        {finding.status === "regressed" && <span className="badge badge-danger exploration-badge">回歸失敗</span>}
      </header>
      {/* 疑點與確認它的重現案例共用一個 dl，標籤欄對齊 */}
      <dl className="exploration-summary">
        <dt>Task</dt>
        <dd>{finding.task_title ?? <ShortValue value={finding.task_id} kind="id" field="Task" />}</dd>
        <dt>章程</dt>
        <dd>第 {finding.charter_version} 版</dd>
        {finding.feature !== undefined && (
          <>
            <dt>功能</dt>
            <dd>{finding.feature}</dd>
          </>
        )}
        <dt>預期</dt>
        <dd>
          <LongText text={finding.expected} />
        </dd>
        <dt>實際觀察</dt>
        <dd>
          <LongText text={finding.actual} />
        </dd>
        <dt>環境</dt>
        <dd>{finding.environment}</dd>
        <dt>證據</dt>
        <dd>
          {finding.evidence.length === 0
            ? "沒有附證據"
            : finding.evidence.map((id, index) => (
                <span key={index}>
                  {index > 0 && "、"}
                  <ShortValue value={id} kind="id" field={finding.evidence.length === 1 ? "證據" : `證據 ${index + 1}`} />
                </span>
              ))}
        </dd>
        <dt>回報者</dt>
        <dd>
          <ShortValue value={finding.reported_by} kind="id" field="回報者" />
        </dd>
        <dt>回報於</dt>
        <dd>{formatTimestamp(finding.created_at, timeZone)}</dd>
        {finding.unverified_reason !== undefined && (
          <>
            <dt>未驗原因</dt>
            <dd>{finding.unverified_reason}</dd>
          </>
        )}
        {finding.exclusion !== undefined && (
          <>
            <dt>排除原因</dt>
            <dd>
              {EXCLUSION_LABELS[finding.exclusion.reason]}
              {finding.exclusion.decision_request_id !== undefined && (
                <>
                  （裁定請求 <ShortValue value={finding.exclusion.decision_request_id} kind="id" field="裁定請求" />）
                </>
              )}
            </dd>
            <dt>排除理由</dt>
            <dd>{finding.exclusion.rationale}</dd>
          </>
        )}
        {finding.case !== undefined && <CaseRows reproduction={finding.case} timeZone={timeZone} />}
      </dl>
      <h5>重現步驟</h5>
      <ol className="exploration-list" aria-label={`${finding.title} 的重現步驟`}>
        {finding.steps.map((step, index) => (
          <li key={index}>{step}</li>
        ))}
      </ol>
      <AttemptList finding={finding} timeZone={timeZone} />
    </section>
  );
}

/** 確認缺陷時的重現案例：確認它的 Evidence Bundle 與失敗的步驟（放在疑點的 dl 中）。 */
function CaseRows({ reproduction, timeZone }: { reproduction: ReproductionCase; timeZone?: string }) {
  const failed = reproduction.journey.steps.find((step) => step.key === reproduction.failed_step);
  return (
    <>
      <dt>確認的 Evidence Bundle</dt>
      <dd>
        <ShortValue value={reproduction.bundle_id} kind="id" field="確認的 Evidence Bundle" />
      </dd>
      {reproduction.failed_step !== undefined && (
        <>
          <dt>失敗的步驟</dt>
          <dd>
            {failed !== undefined && failed.title !== "" ? `${failed.title}（${reproduction.failed_step}）` : reproduction.failed_step}
          </dd>
        </>
      )}
      <dt>確認於</dt>
      <dd>{formatTimestamp(reproduction.confirmed_at, timeZone)}</dd>
    </>
  );
}

function AttemptList({ finding, timeZone }: { finding: ExplorationFinding; timeZone?: string }) {
  const label = `${finding.title} 的重現紀錄`;
  return (
    <>
      <h5>重現紀錄</h5>
      {finding.attempts.length === 0 ? (
        <p>還沒有重現紀錄。</p>
      ) : (
        <ol className="exploration-attempts" aria-label={label}>
          {finding.attempts.map((attempt) => (
            <AttemptItem key={attempt.request_id} attempt={attempt} timeZone={timeZone} />
          ))}
        </ol>
      )}
    </>
  );
}

function AttemptItem({ attempt, timeZone }: { attempt: ReproductionAttempt; timeZone?: string }) {
  const outcome = attempt.outcome === undefined ? "排隊中" : OUTCOME_LABELS[attempt.outcome];
  return (
    <li aria-label={`驗證要求 ${attempt.request_id}`} className={`exploration-attempt outcome-${attempt.outcome ?? "queued"}`}>
      <span className="exploration-attempt-outcome">
        {PURPOSE_LABELS[attempt.purpose]}・{outcome}
      </span>
      <span>
        驗證要求 <ShortValue value={attempt.request_id} kind="id" field="驗證要求" />・Bundle{" "}
        {attempt.bundle_id === undefined ? "尚未產生" : <ShortValue value={attempt.bundle_id} kind="id" field="Bundle" />}
      </span>
      {attempt.verdict !== undefined && (
        <span>
          案例判定：<Verdict verdict={attempt.verdict} />
          {attempt.reason_code !== undefined && <span className="verification-reason">（{attempt.reason_code}）</span>}
        </span>
      )}
      {attempt.control_verdict !== undefined && (
        <span>
          對照判定：<Verdict verdict={attempt.control_verdict} />
        </span>
      )}
      <span className="verification-reason">
        {attempt.requested_by} 要求於 {formatTimestamp(attempt.created_at, timeZone)}
        {attempt.finished_at !== undefined && `・完成於 ${formatTimestamp(attempt.finished_at, timeZone)}`}
      </span>
    </li>
  );
}

function Verdict({ verdict }: { verdict: VerificationVerdict }) {
  return <span className={`badge verdict verdict-${verdict}`}>{VERDICT_LABELS[verdict]}</span>;
}
