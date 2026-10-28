import { useEffect, useId, useState, type ReactNode } from "react";

import { optionLabel, shortId } from "./textFormat";
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

/** 時間線一頁顯示的事件數。 */
const PAGE_SIZE = 10;

/** 沒有資料（null）一律顯示的文字，不當成 0。 */
const NO_DATA = "無資料";

/** 過濾選單的選項超過這個字數時縮短（原生選單以最長選項為最小寬度，MASTER 6.1）。 */
const OPTION_MAX_CHARS = 60;

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

/** 比率指標的數值：百分比放大，分子／分母縮小接在後面。 */
function RatioValue({ ratio: { numerator, denominator, rate } }: { ratio: MetricRatio }) {
  if (rate === null) return <Value main={NO_DATA} />;
  return <Value main={percent(rate)} unit={`（${numerator}／${denominator}）`} />;
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

/** 耗時指標：平均值為主數值，最長值是下一行的說明。 */
function DurationValue({ stats }: { stats: DurationStats }) {
  return (
    <>
      <Value prefix="平均 " main={duration(stats.average_ms)} />
      <dd>最長 {duration(stats.max_ms)}</dd>
    </>
  );
}

/** token 數的主數值：數字放大，單位縮小；沒有資料時只顯示「無資料」。 */
function TokensValue({ prefix, value }: { prefix: string; value: number | null }) {
  if (value === null) return <Value prefix={prefix} main={NO_DATA} />;
  return <Value prefix={prefix} main={value.toLocaleString("en-US")} unit=" tokens" />;
}

/** 一項指標的主數值（20px、600、等寬數字）；前面的標籤與後面的單位小一級，其餘說明放在下一行的 dd。 */
function Value({ prefix, main, unit }: { prefix?: string; main: string; unit?: string }) {
  return (
    <dd className="timeline-metric-value">
      {prefix && <span className="timeline-metric-prefix">{prefix}</span>}
      {main}
      {unit && <span className="timeline-metric-unit">{unit}</span>}
    </dd>
  );
}

/** 服務端額度看不到（null）時顯示「未知」。 */
function quotaUsed(used: number | null): string {
  return used === null ? "未知" : percent(used);
}

/**
 * 時間線（票 41）：上方是由時間線事件計算的觀測指標，下方依 Runtime 回傳順序列出事件，
 * 可依 Task 過濾（過濾由 Runtime 做）；關鍵字搜尋與每頁 10 筆的分頁在 UI 端對整份時間線做。
 */
export function TimelinePage({ client, workspace, timeZone }: Props) {
  const [metrics, setMetrics] = useState<Loaded<MetricsResponse>>({ state: "loading" });
  const [timeline, setTimeline] = useState<Loaded<TimelineEvent[]>>({ state: "loading" });
  const [taskId, setTaskId] = useState<string | null>(null);
  const [keyword, setKeyword] = useState("");
  const [page, setPage] = useState(0);
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
    setPage(0);
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

  // 關鍵字比對畫面上看得到的文字（類別、事件類型、Task 標題、Run、帳號）與事件內容，不分大小寫
  const needle = keyword.trim().toLowerCase();
  const events = timeline.state === "ready" ? timeline.value : [];
  const matched =
    needle === "" ? events : events.filter((event) => searchText(event, taskLabel, profileLabel).includes(needle));
  const pageCount = Math.max(1, Math.ceil(matched.length / PAGE_SIZE));
  const currentPage = Math.min(page, pageCount - 1);
  const pageEvents = matched.slice(currentPage * PAGE_SIZE, (currentPage + 1) * PAGE_SIZE);

  return (
    <section className="timeline-page" aria-label="時間線">
      <header className="more-header">
        <h2>時間線</h2>
      </header>

      <section className="timeline-metrics" aria-labelledby="timeline-metrics-title">
        <h3 id="timeline-metrics-title">觀測指標</h3>
        {metrics.state === "loading" && <p>載入中…</p>}
        {metrics.state === "failed" && <p role="alert">無法取得觀測指標：{metrics.reason}</p>}
        {metrics.state === "ready" && <MetricsView metrics={metrics.value.metrics} quotas={metrics.value.server_quota} />}
      </section>

      <section className="timeline-events" aria-labelledby="timeline-events-title">
        <h3 id="timeline-events-title">事件時間線</h3>
        <div className="timeline-filter">
          <label className="timeline-filter-task">
            依 Task 過濾
            <select value={taskId ?? ""} onChange={(e) => setTaskId(e.target.value === "" ? null : e.target.value)}>
              <option value="">全部 Task</option>
              {taskOptions.map((id) => (
                <option key={id} value={id} title={taskLabel(id)}>
                  {taskOptionLabel(id, taskLabel(id))}
                </option>
              ))}
            </select>
          </label>
          {taskId !== null && (
            <button type="button" onClick={() => setTaskId(null)}>
              清除過濾
            </button>
          )}
          <label className="timeline-filter-search">
            搜尋事件
            <input
              type="search"
              value={keyword}
              placeholder="關鍵字"
              onChange={(e) => {
                setKeyword(e.target.value);
                setPage(0);
              }}
            />
          </label>
        </div>
        {timeline.state === "loading" && <p>載入中…</p>}
        {timeline.state === "failed" && <p role="alert">無法取得時間線：{timeline.reason}</p>}
        {timeline.state === "ready" && timeline.value.length === 0 && (
          <p>{taskId === null ? "這個 Workspace 還沒有事件" : "這個 Task 沒有事件"}</p>
        )}
        {timeline.state === "ready" && timeline.value.length > 0 && matched.length === 0 && (
          <p>沒有符合「{keyword.trim()}」的事件</p>
        )}
        {matched.length > 0 && (
          <ol className="timeline-list" aria-label="事件">
            {pageEvents.map((event) => (
              <li key={event.seq} className={`timeline-event timeline-category-${event.category}`}>
                <span className="timeline-time">{formatTimestamp(event.occurred_at, timeZone)}</span>
                <span className="timeline-category">{CATEGORY_LABELS[event.category]}</span>
                <code className="timeline-type mono" title={event.event_type}>
                  {event.event_type}
                </code>
                <span className="timeline-facts">
                  {event.task_ids.length > 0 && (
                    <span className="clamp-1" title={event.task_ids.map(taskLabel).join("、")}>
                      <span className="timeline-fact-label">Task：</span>
                      {event.task_ids.map(taskLabel).join("、")}
                    </span>
                  )}
                  {event.run_id !== null && (
                    <span>
                      <span className="timeline-fact-label">Run：</span>
                      <code className="mono" title={event.run_id}>
                        {shortId(event.run_id)}
                      </code>
                    </span>
                  )}
                  {event.category === "controller" && <ControllerEventFacts event={event} profileLabel={profileLabel} />}
                </span>
              </li>
            ))}
          </ol>
        )}
        {matched.length > 0 && (
          <nav className="timeline-pagination" aria-label="時間線分頁">
            <button type="button" disabled={currentPage === 0} onClick={() => setPage(currentPage - 1)}>
              上一頁
            </button>
            <span>
              第 {currentPage + 1}／{pageCount} 頁
            </span>
            <button type="button" disabled={currentPage === pageCount - 1} onClick={() => setPage(currentPage + 1)}>
              下一頁
            </button>
          </nav>
        )}
      </section>
    </section>
  );
}

/** 過濾選項的文字：太長的 Task 標題縮短並附識別碼後 4 碼，完整標題在選項的 title。 */
function taskOptionLabel(id: string, label: string): string {
  return [...label].length > OPTION_MAX_CHARS ? optionLabel(label, id, OPTION_MAX_CHARS) : label;
}

/** 關鍵字比對用的小寫文字：畫面上顯示的欄位加上事件內容。 */
function searchText(event: TimelineEvent, taskLabel: (id: string) => string, profileLabel: (id: string) => string): string {
  const profileId = payloadString(event.payload, "profile_id");
  return [
    CATEGORY_LABELS[event.category],
    event.event_type,
    event.aggregate_id,
    event.run_id ?? "",
    ...event.task_ids,
    ...event.task_ids.map(taskLabel),
    profileId === null ? "" : profileLabel(profileId),
    JSON.stringify(event.payload),
  ]
    .join("\n")
    .toLowerCase();
}

/** Controller CLI 事件的帳號、Controller CLI id 與結束代碼（Run id 為 null，帳號只在 payload）。 */
function ControllerEventFacts({ event, profileLabel }: { event: TimelineEvent; profileLabel: (id: string) => string }) {
  const profileId = payloadString(event.payload, "profile_id");
  const endCode = payloadString(event.payload, "end_code");
  return (
    <>
      {profileId !== null && (
        <span className="clamp-1" title={profileLabel(profileId)}>
          <span className="timeline-fact-label">帳號：</span>
          {profileLabel(profileId)}
        </span>
      )}
      <span>
        <span className="timeline-fact-label">Controller CLI：</span>
        <code className="mono" title={event.aggregate_id}>
          {shortId(event.aggregate_id)}
        </code>
      </span>
      {endCode !== null && (
        <span>
          <span className="timeline-fact-label">結束代碼：</span>
          {endCode}
        </span>
      )}
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
          <RatioValue ratio={task_completion.completion} />
          <dd>已取消 {task_completion.cancelled}</dd>
        </Metric>
        <Metric name="交接成功率">
          <RatioValue ratio={handoff.success} />
          <dd>
            已放棄 {handoff.abandoned}・已確認接手 {handoff.confirmed}・進行中 {handoff.in_flight}
          </dd>
        </Metric>
        <Metric name="恢復耗時">
          <DurationValue stats={recovery} />
          <dd>
            已恢復 {recovery.count} 次・尚未恢復 {recovery.pending}
          </dd>
        </Metric>
        <Metric name="重複操作事件">
          <Value prefix="被擋下 " main={String(duplicate_operations.blocked)} unit=" 次" />
          <dd>
            已成功 {duplicate_operations.already_succeeded}・結果未知 {duplicate_operations.result_unknown}・他處進行中{" "}
            {duplicate_operations.in_progress_elsewhere}
          </dd>
          <dd>變成結果未知 {duplicate_operations.unknown_outcomes} 次</dd>
        </Metric>
        <Metric name="裁定等待時間">
          <DurationValue stats={decision_wait} />
          <dd>
            已回答 {decision_wait.count}・等待中 {decision_wait.pending}
          </dd>
        </Metric>
        <Metric name="教訓重犯率">
          <RatioValue ratio={lesson_recurrence.rate} />
          <dd>
            重犯 {lesson_recurrence.recurrences} 次・Guard 擋下 {lesson_recurrence.prevented}・使用者回報{" "}
            {lesson_recurrence.reported}
          </dd>
        </Metric>
        <Metric name="每階段用量">
          <TokensValue prefix="總計 " value={usage.total_tokens} />
          <dd>共 {usage.runs} 個 Run</dd>
          {/* 階段多時在卡片內捲動，不把同列的卡片撐高 */}
          <dd className="timeline-metric-stages">
            {usage.by_stage.map((stage) => (
              <span key={stage.stage ?? ""} className="timeline-metric-stage">
                {stage.stage ?? "未分階段"}：{tokens(stage.tokens)}（{stage.runs} 個 Run）
              </span>
            ))}
          </dd>
        </Metric>
        <Metric name="Context 使用量">
          <TokensValue prefix="平均 " value={context.average_tokens} />
          <dd>最長 {tokens(context.max_tokens)}</dd>
          <dd>
            已回報 {context.runs_known} 個 Run・未回報 {context.runs_unknown} 個 Run
          </dd>
        </Metric>
      </dl>
      <QuotaTable quotas={quotas} />
    </>
  );
}

/** 服務端額度：每個帳號一列（帳號｜五小時｜七日），帳號名稱 1 行、完整名稱在 title。 */
function QuotaTable({ quotas }: { quotas: ServerQuota[] }) {
  const headingId = useId();
  return (
    <section className="timeline-quota" aria-labelledby={headingId}>
      <h4 id={headingId}>服務端額度</h4>
      {quotas.length === 0 ? (
        <p className="more-empty">沒有帳號</p>
      ) : (
        <div className="table-wrap">
          <table className="data-table" aria-labelledby={headingId}>
            <colgroup>
              <col />
              <col style={{ width: "7em" }} />
              <col style={{ width: "7em" }} />
            </colgroup>
            <thead>
              <tr>
                <th scope="col">帳號</th>
                <th scope="col" className="num">
                  五小時
                </th>
                <th scope="col" className="num">
                  七日
                </th>
              </tr>
            </thead>
            <tbody>
              {quotas.map((quota) => (
                <tr key={quota.profile_id} aria-label={`服務端額度 ${quota.name}`}>
                  <td>
                    <span className="clamp-1" title={quota.name}>
                      {quota.name}
                    </span>
                  </td>
                  <td className="num">{quotaUsed(quota.five_hour_used)}</td>
                  <td className="num">{quotaUsed(quota.seven_day_used)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
}
