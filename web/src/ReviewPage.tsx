import { useEffect, useState } from "react";

import { ShortValue } from "./LongText";
import { shortHash, splitFirstSentence } from "./textFormat";
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

/** finding 狀態的徽章：等你裁定是琥珀色，仍成立（未分類、要修正）是紅色，已駁回是中性。 */
const FINDING_STATE_BADGES: Record<FindingState, string> = {
  awaiting_decision: "badge badge-warning",
  open: "badge badge-danger",
  fix: "badge badge-danger",
  dismissed: "badge badge-plain",
};

/** finding 的排列順序：要你裁定的最前，其次是仍成立的，已駁回的最後（finding 沒有嚴重度欄位，以處理狀態排序）。 */
const FINDING_ORDER: Record<FindingState, number> = { awaiting_decision: 0, open: 1, fix: 2, dismissed: 3 };

const DEVIATION_STATUS_LABELS: Record<DeviationStatus, string> = {
  proposed: "待處理",
  awaiting_decision: "等待裁定",
  accepted: "已接受",
  rejected: "已拒絕",
};

/** 設計偏離狀態的語意色（徽章與左側色條）；已拒絕是中性。 */
const DEVIATION_TONES: Record<DeviationStatus, string | null> = {
  proposed: "accent",
  awaiting_decision: "warning",
  accepted: "success",
  rejected: null,
};

/** 設計偏離的排列順序：待處理（等待裁定、待處理）在前，已處理的在後。 */
const DEVIATION_ORDER: Record<DeviationStatus, number> = { awaiting_decision: 0, proposed: 1, accepted: 2, rejected: 3 };

function reasonOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** 桌面裁定的是使用者本人。 */
function actorLabel(actor: string): string {
  return actor === "desktop" ? "使用者" : actor;
}

/**
 * 審查（票 44）：唯讀檢視 Workspace 的雙軸審查——規範（Standards）與需求（Spec）分開顯示，
 * 各自的結論與 finding；已過期的審查收在最後。另一個區段是實作偏離 Design Package 的回報與處理結果。
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
    <section className="page review-page" aria-label="審查">
      <header className="page-header">
        <h2>審查</h2>
      </header>
      {loaded.state === "loading" && <p className="empty-note">載入中…</p>}
      {loaded.state === "failed" && (
        <div className="notice notice-danger" role="alert">
          <p>無法取得審查：{loaded.reason}</p>
        </div>
      )}
      {loaded.state === "ready" && (
        <>
          <ReviewsSection reviews={loaded.list.reviews} timeZone={timeZone} />
          <DeviationsSection deviations={loaded.list.deviations} timeZone={timeZone} />
        </>
      )}
    </section>
  );
}

/** 雙軸審查：仍有效的審查攤開，已過期的（已有較新 candidate 的審查等）收進最後的「已過期的審查」。 */
function ReviewsSection({ reviews, timeZone }: { reviews: ReviewView[]; timeZone?: string }) {
  const current = reviews.filter((review) => review.stale_reason === null);
  const stale = reviews.filter((review) => review.stale_reason !== null);
  return (
    <section className="section" aria-labelledby="review-reviews">
      <div className="section-heading">
        <h3 id="review-reviews">雙軸審查</h3>
        <span className="count">{current.length}</span>
        <p>規範（Standards）與需求（Spec）兩軸各自判定，兩軸都通過，整份審查才通過。</p>
      </div>
      {reviews.length === 0 && <p className="empty-note">還沒有審查；reviewer 提交雙軸審查後會在這裡顯示</p>}
      {reviews.length > 0 && current.length === 0 && <p className="empty-note">目前沒有仍有效的審查，只有已過期的審查</p>}
      {current.map((review) => (
        <ReviewCard key={review.id} review={review} timeZone={timeZone} />
      ))}
      {stale.length > 0 && (
        <details className="review-stale-group">
          <summary>
            已過期的審查 <span className="count">{stale.length}</span>
          </summary>
          <div className="review-stale-list">
            {stale.map((review) => (
              <ReviewCard key={review.id} review={review} timeZone={timeZone} />
            ))}
          </div>
        </details>
      )}
    </section>
  );
}

/**
 * 整體結果：狀態詞做成徽章，卡片以左側色條標示（長的過期原因用一般文字，不整段上色）。
 * 已過期的審查不需要你處理，徽章與色條都用中性色（琥珀色只給要你處理的事，MASTER 7.1）。
 */
const OVERALL = {
  passed: { label: "通過", stripe: "stripe-success", badge: "badge badge-success" },
  failed: { label: "未通過", stripe: "stripe-danger", badge: "badge badge-danger" },
  stale: { label: "已過期", stripe: "stripe-muted", badge: "badge badge-plain" },
} as const;

function ReviewCard({ review, timeZone }: { review: ReviewView; timeZone?: string }) {
  const headingId = `review-${review.id}`;
  // 已過期的審查不論兩軸結果都不能當作證據
  const overall = OVERALL[review.stale_reason !== null ? "stale" : review.passed ? "passed" : "failed"];
  const subject = review.subject_task_title ?? review.subject_task_id;
  return (
    <section className={`card ${overall.stripe} review-card`} aria-labelledby={headingId}>
      <header className="card-header">
        <h4 id={headingId}>
          <span className="clamp-2" title={subject}>
            {subject}
          </span>
          <span className="review-candidate">
            （候選版本 <code className="mono">{shortHash(review.candidate_revision)}</code>）
          </span>
        </h4>
        <span className={`${overall.badge} review-overall`}>{overall.label}</span>
      </header>
      {review.stale_reason !== null && <StaleReason reason={review.stale_reason} />}
      <div className="review-axes">
        <AxisSection title="規範（Standards）" axis={review.standards} />
        <AxisSection title="需求（Spec）" axis={review.spec} />
      </div>
      <dl className="facts review-facts review-reference">
        <dt>被審 Task</dt>
        <dd>{subject}</dd>
        <dt>Reviewer</dt>
        <dd>{review.reviewer_task_title ?? review.reviewer_task_id}</dd>
        <dt>版本</dt>
        <dd>
          <ShortValue value={review.base_revision} kind="hash" field="基準版本" />
          {" → "}
          <ShortValue value={review.candidate_revision} kind="hash" field="候選版本" />
        </dd>
        <dt>規格</dt>
        <dd>規格第 {review.spec_version} 版</dd>
        <dt>規範來源</dt>
        <dd>
          <ShortValue value={review.standards_source} kind="path" field="規範來源" />
        </dd>
        <dt>審查於</dt>
        <dd>{formatTimestamp(review.created_at, timeZone)}</dd>
      </dl>
    </section>
  );
}

/** 過期原因（錯誤訊息）：第一句完整顯示，其餘收進「詳細資訊」。 */
function StaleReason({ reason }: { reason: string }) {
  const { summary, rest } = splitFirstSentence(reason);
  return (
    <div className="notice review-stale-reason">
      <p className="notice-title">過期原因</p>
      <p>{summary}</p>
      {rest !== "" && (
        <details>
          <summary>詳細資訊</summary>
          <p className="pre-wrap">{rest}</p>
        </details>
      )}
    </div>
  );
}

function AxisSection({ title, axis }: { title: string; axis: ReviewAxisView }) {
  const findings = [...axis.findings].sort((a, b) => FINDING_ORDER[a.state] - FINDING_ORDER[b.state]);
  // 審查卡本身已有色條：兩軸只用徽章表達各自的結論，不再各加一條色條（框中框）
  return (
    <section className="review-axis" aria-label={title}>
      <h5 className="review-axis-head">
        <span>{title}</span>
        <span className={axis.passed ? "badge badge-success" : "badge badge-danger"}>{axis.passed ? "通過" : "未通過"}</span>
      </h5>
      {axis.reused_from !== null && (
        <p className="meta">
          <span>
            沿用自 <code className="mono">{axis.reused_from}</code>
          </span>
        </p>
      )}
      {axis.summary !== "" && <p>{axis.summary}</p>}
      {findings.length > 0 && (
        <ul className="review-findings" aria-label={`${title}的 finding`}>
          {findings.map((finding) => (
            <FindingItem key={finding.id} finding={finding} />
          ))}
        </ul>
      )}
      {axis.paths.length > 0 && (
        <div className="review-paths">
          <span className="review-paths-label">檔案</span>
          <ul aria-label={`${title}的檔案`}>
            {axis.paths.map((path) => (
              <li key={path}>
                <code className="mono">{path}</code>
              </li>
            ))}
          </ul>
        </div>
      )}
    </section>
  );
}

function FindingItem({ finding }: { finding: ReviewFinding }) {
  return (
    <li aria-label={finding.title} className="review-finding">
      <div className="row-head">
        <p>{finding.title}</p>
        <span className={FINDING_STATE_BADGES[finding.state]}>{FINDING_STATE_LABELS[finding.state]}</span>
      </div>
      {finding.location !== null && (
        <p className="meta">
          <code className="mono">{finding.location}</code>
        </p>
      )}
      {finding.detail !== "" && <p>{finding.detail}</p>}
      {finding.reason !== null && <p className="review-finding-note">理由：{finding.reason}</p>}
      {finding.decided_by !== null && <p className="review-finding-note">裁定者：{actorLabel(finding.decided_by)}</p>}
    </li>
  );
}

/** 設計偏離：待處理的在前；每一項是一張卡。 */
function DeviationsSection({ deviations, timeZone }: { deviations: DeviationView[]; timeZone?: string }) {
  const sorted = [...deviations].sort((a, b) => DEVIATION_ORDER[a.status] - DEVIATION_ORDER[b.status]);
  return (
    <section className="section" aria-labelledby="review-deviations">
      <div className="section-heading">
        <h3 id="review-deviations">設計偏離</h3>
        <span className="count">{deviations.length}</span>
        <p>實作偏離 Design Package 時的回報與處理結果，待處理的排在前面。</p>
      </div>
      {deviations.length === 0 && <p className="empty-note">沒有設計偏離；實作偏離 Design Package 時會在這裡顯示</p>}
      {sorted.map((deviation) => (
        <DeviationCard key={deviation.id} deviation={deviation} timeZone={timeZone} />
      ))}
    </section>
  );
}

function DeviationCard({ deviation, timeZone }: { deviation: DeviationView; timeZone?: string }) {
  const headingId = `deviation-${deviation.id}`;
  const tone = DEVIATION_TONES[deviation.status];
  const accepted = deviation.status === "accepted";
  return (
    <section className={tone === null ? "card stripe-muted review-card" : `card stripe-${tone} review-card`} aria-labelledby={headingId}>
      <header className="card-header">
        <h4 id={headingId}>{deviation.deviation}</h4>
        <span className={tone === null ? "badge" : `badge badge-${tone}`}>{DEVIATION_STATUS_LABELS[deviation.status]}</span>
      </header>
      <dl className="facts review-facts">
        <dt>Task</dt>
        <dd>{deviation.task_title ?? deviation.task_id}</dd>
        <dt>設計</dt>
        <dd>
          {deviation.design_name ?? deviation.design_artifact_id} 第 {deviation.design_version} 版
        </dd>
        <dt>原因</dt>
        <dd>{deviation.reason}</dd>
        {deviation.affected_contracts.length > 0 && (
          <>
            <dt>受影響的契約</dt>
            <dd>
              <ul className="review-contracts" aria-label={`${deviation.deviation} 受影響的契約`}>
                {deviation.affected_contracts.map((contract, index) => (
                  <li key={index}>{contract}</li>
                ))}
              </ul>
            </dd>
          </>
        )}
        {accepted && deviation.accepted_by !== null && (
          <>
            <dt>接受者</dt>
            <dd>{actorLabel(deviation.accepted_by)}</dd>
          </>
        )}
        {accepted && deviation.accepted_design_version !== null && (
          <>
            <dt>設計更新為</dt>
            <dd>第 {deviation.accepted_design_version} 版</dd>
          </>
        )}
        {deviation.resolution_reason !== null && (
          <>
            <dt>處理理由</dt>
            <dd>{deviation.resolution_reason}</dd>
          </>
        )}
        <dt>回報於</dt>
        <dd>{formatTimestamp(deviation.created_at, timeZone)}</dd>
      </dl>
    </section>
  );
}
