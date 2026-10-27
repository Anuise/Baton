import { useEffect, useState } from "react";

import { formatTimestamp } from "./time";
import type {
  DeviationStatus,
  DeviationView,
  FindingState,
  ReviewAxisView,
  ReviewClient,
  ReviewFinding,
  ReviewList,
  ReviewView,
  Workspace,
} from "./runtimeClient";

type Loaded = { state: "loading" } | { state: "ready"; list: ReviewList } | { state: "failed"; reason: string };

interface Props {
  client: ReviewClient;
  workspace: Workspace;
  /** 顯示用的時區；省略時使用系統時區（spec D-33），只有測試會指定。 */
  timeZone?: string;
}

const FINDING_STATE_LABELS: Record<FindingState, string> = {
  open: "未分類",
  fix: "修正",
  awaiting_decision: "等待裁定",
  dismissed: "已駁回",
};

const DEVIATION_STATUS_LABELS: Record<DeviationStatus, string> = {
  proposed: "待處理",
  awaiting_decision: "等待裁定",
  accepted: "已接受",
  rejected: "已拒絕",
};

function reasonOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** 桌面裁定的是使用者本人。 */
function actorLabel(actor: string): string {
  return actor === "desktop" ? "使用者" : actor;
}

/**
 * 審查與設計偏離（票 44）：唯讀檢視 Workspace 的雙軸審查——規範（Standards）與需求（Spec）分開顯示，
 * 各自的 finding 與分類狀態、審查是否過期；以及實作偏離 Design Package 的回報與處理結果。
 */
export function ReviewPage({ client, workspace, timeZone }: Props) {
  const [loaded, setLoaded] = useState<Loaded>({ state: "loading" });

  useEffect(() => {
    // 卸載後遲到的回應一律忽略
    let cancelled = false;
    client.reviews(workspace.id).then(
      (list) => !cancelled && setLoaded({ state: "ready", list }),
      (e: unknown) => !cancelled && setLoaded({ state: "failed", reason: reasonOf(e) }),
    );
    return () => {
      cancelled = true;
    };
  }, [client, workspace.id]);

  return (
    <section className="review-page" aria-label="審查">
      <h2>審查與設計偏離：{workspace.name}</h2>
      {loaded.state === "loading" && <p>載入中…</p>}
      {loaded.state === "failed" && <p role="alert">無法取得審查：{loaded.reason}</p>}
      {loaded.state === "ready" && (
        <>
          <h3>審查</h3>
          {loaded.list.reviews.length === 0 && <p>還沒有審查；reviewer 提交雙軸審查後會在這裡顯示。</p>}
          {loaded.list.reviews.map((review) => (
            <ReviewCard key={review.id} review={review} timeZone={timeZone} />
          ))}
          <h3>設計偏離</h3>
          {loaded.list.deviations.length === 0 && <p>沒有設計偏離；實作偏離 Design Package 時會在這裡顯示。</p>}
          {loaded.list.deviations.map((deviation) => (
            <DeviationCard key={deviation.id} deviation={deviation} timeZone={timeZone} />
          ))}
        </>
      )}
    </section>
  );
}

function ReviewCard({ review, timeZone }: { review: ReviewView; timeZone?: string }) {
  const headingId = `review-${review.id}`;
  // 已過期的審查不論兩軸結果都不能當作證據
  const overall = review.stale_reason !== null ? "stale" : review.passed ? "passed" : "failed";
  return (
    <section className={`review review-${overall}`} aria-labelledby={headingId}>
      <h4 id={headingId}>
        {review.subject_task_title ?? review.subject_task_id}（candidate {review.candidate_revision}）
      </h4>
      <p className="review-badge">
        {overall === "stale" ? `已過期：${review.stale_reason}` : overall === "passed" ? "通過" : "未通過"}
      </p>
      <dl className="workflow-summary">
        <dt>被審 Task</dt>
        <dd>{review.subject_task_title ?? review.subject_task_id}</dd>
        <dt>Reviewer</dt>
        <dd>{review.reviewer_task_title ?? review.reviewer_task_id}</dd>
        <dt>版本</dt>
        <dd>
          {review.base_revision} → {review.candidate_revision}
        </dd>
        <dt>規格</dt>
        <dd>規格第 {review.spec_version} 版</dd>
        <dt>規範來源</dt>
        <dd>{review.standards_source}</dd>
        <dt>審查於</dt>
        <dd>{formatTimestamp(review.created_at, timeZone)}</dd>
      </dl>
      <AxisSection title="規範（Standards）" axis={review.standards} />
      <AxisSection title="需求（Spec）" axis={review.spec} />
    </section>
  );
}

function AxisSection({ title, axis }: { title: string; axis: ReviewAxisView }) {
  return (
    <section className={`review-axis ${axis.passed ? "axis-passed" : "axis-failed"}`} aria-label={title}>
      <h5>
        {title}・{axis.passed ? "通過" : "未通過"}
      </h5>
      {axis.reused_from !== null && <p className="review-reused">沿用自 {axis.reused_from}</p>}
      {axis.summary !== "" && <p>{axis.summary}</p>}
      {axis.paths.length > 0 && (
        <ul className="workflow-stage-list" aria-label={`${title}的檔案`}>
          {axis.paths.map((path) => (
            <li key={path}>{path}</li>
          ))}
        </ul>
      )}
      {axis.findings.length > 0 && (
        <ul className="workflow-stage-list" aria-label={`${title}的 finding`}>
          {axis.findings.map((finding) => (
            <FindingItem key={finding.id} finding={finding} />
          ))}
        </ul>
      )}
    </section>
  );
}

function FindingItem({ finding }: { finding: ReviewFinding }) {
  return (
    <li aria-label={finding.title} className={`review-finding finding-${finding.state}`}>
      <span>
        {finding.title}・{FINDING_STATE_LABELS[finding.state]}
        {finding.location !== null && `・${finding.location}`}
      </span>
      {finding.detail !== "" && <p>{finding.detail}</p>}
      {finding.reason !== null && <p>理由：{finding.reason}</p>}
      {finding.decided_by !== null && <p>裁定者：{actorLabel(finding.decided_by)}</p>}
    </li>
  );
}

function DeviationCard({ deviation, timeZone }: { deviation: DeviationView; timeZone?: string }) {
  const headingId = `deviation-${deviation.id}`;
  return (
    <section className={`review deviation-${deviation.status}`} aria-labelledby={headingId}>
      <h4 id={headingId}>{deviation.deviation}</h4>
      <p className="review-badge">{DEVIATION_STATUS_LABELS[deviation.status]}</p>
      <dl className="workflow-summary">
        <dt>Task</dt>
        <dd>{deviation.task_title ?? deviation.task_id}</dd>
        <dt>設計</dt>
        <dd>
          {deviation.design_name ?? deviation.design_artifact_id} 第 {deviation.design_version} 版
        </dd>
        <dt>原因</dt>
        <dd>{deviation.reason}</dd>
        <dt>回報於</dt>
        <dd>{formatTimestamp(deviation.created_at, timeZone)}</dd>
      </dl>
      {deviation.affected_contracts.length > 0 && (
        <ul className="workflow-stage-list" aria-label={`${deviation.deviation} 受影響的契約`}>
          {deviation.affected_contracts.map((contract, index) => (
            <li key={index}>{contract}</li>
          ))}
        </ul>
      )}
      {deviation.status === "accepted" && (
        <p>
          {deviation.accepted_by !== null && `接受者：${actorLabel(deviation.accepted_by)}`}
          {deviation.accepted_by !== null && deviation.accepted_design_version !== null && "・"}
          {deviation.accepted_design_version !== null && `設計更新為第 ${deviation.accepted_design_version} 版`}
        </p>
      )}
      {deviation.resolution_reason !== null && <p>處理理由：{deviation.resolution_reason}</p>}
    </section>
  );
}
