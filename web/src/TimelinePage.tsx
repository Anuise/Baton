import { useEffect, useState, type ReactNode } from "react";

import { formatTimestamp } from "./time";
import type {
  AccountClient,
  DurationStats,
  MetricRatio,
  Metrics,
  MetricsResponse,
  ObservabilityClient,
  ServerQuota,
  TaskClient,
  TimelineCategory,
  TimelineEvent,
  Workspace,
} from "./runtimeClient";

type Loaded<T> = { state: "loading" } | { state: "ready"; value: T } | { state: "failed"; reason: string };

interface Props {
  /** Task 清單只用來顯示標題、帳號清單只用來顯示帳號名稱；讀不到時改顯示 id。 */
  client: ObservabilityClient & Pick<TaskClient, "getTaskBoard"> & Pick<AccountClient, "listProfiles">;
  workspace: Workspace;
  /** 顯示用的時區；省略時使用系統時區（spec D-33），只有測試會指定。 */
  timeZone?: string;
}

const CATEGORY_LABELS: Record<TimelineCategory, string> = {
  dispatch: "派工",
  handoff: "交接",
  decision: "裁定",
  recovery: "恢復",
  verification: "驗證",
  cleanup: "清理",
  task: "工作狀態",
  external_operation: "外部操作",
  lesson: "教訓重犯",
  usage: "用量",
  controller: "Controller",
};

/** 沒有資料（null）一律顯示的文字，不當成 0。 */
const NO_DATA = "無資料";

function reasonOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** 事件 payload 的字串欄位；不是物件或欄位不是字串時為 null。 */
function payloadString(payload: unknown, key: string): string | null {
  if (typeof payload !== "object" || payload === null) return null;
  const value = (payload as Record<string, unknown>)[key];
  return typeof value === "string" ? value : null;
}

/** 0–1 的比例顯示成百分比，最多一位小數。 */
function percent(value: number): string {
  return `${Math.round(value * 1000) / 10}%`;
}

function ratio({ numerator, denominator, rate }: MetricRatio): string {
  return rate === null ? NO_DATA : `${percent(rate)}（${numerator}／${denominator}）`;
}

/** 毫秒顯示成秒／分／小時。 */
function duration(ms: number | null): string {
  if (ms === null) return NO_DATA;
  const seconds = Math.round(ms / 1000);
  if (seconds < 60) return `${seconds} 秒`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return seconds % 60 === 0 ? `${minutes} 分` : `${minutes} 分 ${seconds % 60} 秒`;
  return minutes % 60 === 0 ? `${Math.floor(minutes / 60)} 小時` : `${Math.floor(minutes / 60)} 小時 ${minutes % 60} 分`;
}

function tokens(value: number | null): string {
  return value === null ? NO_DATA : `${value.toLocaleString("en-US")} tokens`;
}

function durationStats(stats: DurationStats): string {
  return `平均 ${duration(stats.average_ms)}・最長 ${duration(stats.max_ms)}`;
}

/** 服務端額度看不到（null）時顯示「未知」。 */
function quotaUsed(used: number | null): string {
  return used === null ? "未知" : percent(used);
}

/**
 * 時間線（票 41）：上方是由時間線事件計算的觀測指標，下方依 Runtime 回傳順序列出事件，
 * 可依 Task 過濾（過濾由 Runtime 做）。
 */
export function TimelinePage({ client, workspace, timeZone }: Props) {
  const [metrics, setMetrics] = useState<Loaded<MetricsResponse>>({ state: "loading" });
  const [timeline, setTimeline] = useState<Loaded<TimelineEvent[]>>({ state: "loading" });
  const [taskId, setTaskId] = useState<string | null>(null);
  // 過濾選項取自未過濾時間線出現過的 Task，過濾後仍保留
  const [taskOptions, setTaskOptions] = useState<string[]>([]);
  const [titles, setTitles] = useState<Record<string, string>>({});
  const [profileNames, setProfileNames] = useState<Record<string, string>>({});

  useEffect(() => {
    // 卸載或換 Workspace 後遲到的回應一律忽略
    let cancelled = false;
    setMetrics({ state: "loading" });
    client.metrics(workspace.id).then(
      (value) => !cancelled && setMetrics({ state: "ready", value }),
      (error: unknown) => !cancelled && setMetrics({ state: "failed", reason: reasonOf(error) }),
    );
    client.getTaskBoard(workspace.id).then(
      (board) => !cancelled && setTitles(Object.fromEntries(board.tasks.map((task) => [task.id, task.title]))),
      () => {},
    );
    // Controller CLI 事件只帶 profile_id，帳號名稱由帳號清單對照（含已封存的帳號）
    client.listProfiles().then(
      (list) => !cancelled && setProfileNames(Object.fromEntries(list.profiles.map((profile) => [profile.id, profile.name]))),
      () => {},
    );
    return () => {
      cancelled = true;
    };
  }, [client, workspace.id]);

  useEffect(() => {
    // 換過濾條件後，前一個條件遲到的回應一律忽略
    let cancelled = false;
    setTimeline({ state: "loading" });
    client.timeline(workspace.id, taskId ?? undefined).then(
      ({ events }) => {
        if (cancelled) return;
        setTimeline({ state: "ready", value: events });
        if (taskId === null) setTaskOptions([...new Set(events.flatMap((event) => event.task_ids))]);
      },
      (error: unknown) => !cancelled && setTimeline({ state: "failed", reason: reasonOf(error) }),
    );
    return () => {
      cancelled = true;
    };
  }, [client, workspace.id, taskId]);

  const taskLabel = (id: string) => titles[id] ?? id;
  const profileLabel = (id: string) => profileNames[id] ?? id;

  return (
    <section className="timeline-page" aria-label="時間線">
      <h2>時間線：{workspace.name}</h2>

      <section className="timeline-metrics" aria-labelledby="timeline-metrics-title">
        <h3 id="timeline-metrics-title">觀測指標</h3>
        {metrics.state === "loading" && <p>載入中…</p>}
        {metrics.state === "failed" && <p role="alert">無法取得觀測指標：{metrics.reason}</p>}
        {metrics.state === "ready" && <MetricsView metrics={metrics.value.metrics} quotas={metrics.value.server_quota} />}
      </section>

      <section className="timeline-events" aria-labelledby="timeline-events-title">
        <h3 id="timeline-events-title">事件時間線</h3>
        <div className="timeline-filter">
          <label>
            依 Task 過濾
            <select value={taskId ?? ""} onChange={(e) => setTaskId(e.target.value === "" ? null : e.target.value)}>
              <option value="">全部 Task</option>
              {taskOptions.map((id) => (
                <option key={id} value={id}>
                  {taskLabel(id)}
                </option>
              ))}
            </select>
          </label>
          {taskId !== null && (
            <button type="button" onClick={() => setTaskId(null)}>
              清除過濾
            </button>
          )}
        </div>
        {timeline.state === "loading" && <p>載入中…</p>}
        {timeline.state === "failed" && <p role="alert">無法取得時間線：{timeline.reason}</p>}
        {timeline.state === "ready" && timeline.value.length === 0 && (
          <p>{taskId === null ? "這個 Workspace 還沒有事件" : "這個 Task 沒有事件"}</p>
        )}
        {timeline.state === "ready" && timeline.value.length > 0 && (
          <ol className="timeline-list" aria-label="事件">
            {timeline.value.map((event) => (
              <li key={event.seq} className={`timeline-event timeline-category-${event.category}`}>
                <span className="timeline-time">{formatTimestamp(event.occurred_at, timeZone)}</span>
                <span className="timeline-category">{CATEGORY_LABELS[event.category]}</span>
                <span className="timeline-type">{event.event_type}</span>
                {event.task_ids.length > 0 && <span>Task：{event.task_ids.map(taskLabel).join("、")}</span>}
                {event.run_id !== null && <span>Run：{event.run_id}</span>}
                {event.category === "controller" && <ControllerEventFacts event={event} profileLabel={profileLabel} />}
              </li>
            ))}
          </ol>
        )}
      </section>
    </section>
  );
}

/** Controller CLI 事件的帳號、Controller CLI id 與結束代碼（Run id 為 null，帳號只在 payload）。 */
function ControllerEventFacts({ event, profileLabel }: { event: TimelineEvent; profileLabel: (id: string) => string }) {
  const profileId = payloadString(event.payload, "profile_id");
  const endCode = payloadString(event.payload, "end_code");
  return (
    <>
      {profileId !== null && <span>帳號：{profileLabel(profileId)}</span>}
      <span>Controller CLI：{event.aggregate_id}</span>
      {endCode !== null && <span>結束代碼：{endCode}</span>}
    </>
  );
}

/** 一項指標：標題與數值，以 group 讓輔助技術與測試能依名稱找到。 */
function Metric({ name, children }: { name: string; children: ReactNode }) {
  return (
    <div className="timeline-metric" role="group" aria-label={name}>
      <dt>{name}</dt>
      {children}
    </div>
  );
}

/** 觀測指標原樣顯示 Runtime 由事件算出的數字，不在前端重算。 */
function MetricsView({ metrics, quotas }: { metrics: Metrics; quotas: ServerQuota[] }) {
  const { task_completion, handoff, recovery, duplicate_operations, decision_wait, lesson_recurrence, usage, context } =
    metrics;
  return (
    <>
      <p className="timeline-metrics-source">由 {metrics.events} 個時間線事件計算</p>
      <dl className="timeline-metric-list">
        <Metric name="工作完成率">
          <dd>{ratio(task_completion.completion)}</dd>
          <dd>已取消 {task_completion.cancelled}</dd>
        </Metric>
        <Metric name="交接成功率">
          <dd>{ratio(handoff.success)}</dd>
          <dd>
            已放棄 {handoff.abandoned}・已確認接手 {handoff.confirmed}・進行中 {handoff.in_flight}
          </dd>
        </Metric>
        <Metric name="恢復耗時">
          <dd>{durationStats(recovery)}</dd>
          <dd>
            已恢復 {recovery.count} 次・尚未恢復 {recovery.pending}
          </dd>
        </Metric>
        <Metric name="重複操作事件">
          <dd>被擋下 {duplicate_operations.blocked} 次</dd>
          <dd>
            已成功 {duplicate_operations.already_succeeded}・結果未知 {duplicate_operations.result_unknown}・他處進行中{" "}
            {duplicate_operations.in_progress_elsewhere}
          </dd>
          <dd>變成結果未知 {duplicate_operations.unknown_outcomes} 次</dd>
        </Metric>
        <Metric name="裁定等待時間">
          <dd>{durationStats(decision_wait)}</dd>
          <dd>
            已回答 {decision_wait.count}・等待中 {decision_wait.pending}
          </dd>
        </Metric>
        <Metric name="教訓重犯率">
          <dd>{ratio(lesson_recurrence.rate)}</dd>
          <dd>
            重犯 {lesson_recurrence.recurrences} 次・Guard 擋下 {lesson_recurrence.prevented}・使用者回報{" "}
            {lesson_recurrence.reported}
          </dd>
        </Metric>
        <Metric name="每階段用量">
          <dd>
            總計 {tokens(usage.total_tokens)}（{usage.runs} 個 Run）
          </dd>
          {usage.by_stage.map((stage) => (
            <dd key={stage.stage ?? ""}>
              {stage.stage ?? "未分階段"}：{tokens(stage.tokens)}（{stage.runs} 個 Run）
            </dd>
          ))}
        </Metric>
        <Metric name="Context 使用量">
          <dd>
            平均 {tokens(context.average_tokens)}・最長 {tokens(context.max_tokens)}
          </dd>
          <dd>
            已回報 {context.runs_known} 個 Run・未回報 {context.runs_unknown} 個 Run
          </dd>
        </Metric>
        <Metric name="服務端額度">
          {quotas.length === 0 && <dd>沒有帳號</dd>}
          {quotas.map((quota) => (
            <dd key={quota.profile_id} aria-label={`服務端額度 ${quota.name}`}>
              {quota.name}：五小時 {quotaUsed(quota.five_hour_used)}・七日 {quotaUsed(quota.seven_day_used)}
            </dd>
          ))}
        </Metric>
      </dl>
    </>
  );
}
