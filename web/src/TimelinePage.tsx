import { useEffect, useId, useState, type ReactNode } from "react";

import { ShortValue } from "./LongText";
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

/**
 * 事件類型的中文名稱（MASTER 7.1：機器代碼不當主要文字）。種類以 Runtime 會放上時間線的事件為準
 * （crates/baton-domain/src/timeline.rs 的 category_of）；沒有對照的才顯示原始代碼。
 */
const EVENT_TYPE_LABELS: Record<string, string> = {
  // 派工（Run）
  run_created: "建立 Run",
  run_admission_denied: "Run 未通過准入",
  run_launch_started: "開始啟動 Run",
  run_started: "Run 已開始",
  run_succeeded: "Run 成功",
  run_failed: "Run 失敗",
  run_interrupted: "Run 中斷",
  run_cancel_requested: "要求取消 Run",
  run_cancelled: "Run 已取消",
  // 交接
  run_handed_off: "Run 已交接",
  handoff_requested: "要求交接",
  // 與工作頁的交接階段同一套詞彙（TaskBoard.tsx 的 HANDOFF_PHASE_LABELS）
  handoff_stopping: "交接中：停止舊程序",
  handoff_transferred: "已交接，等待接手確認",
  handoff_confirmed: "已交接並確認",
  handoff_abandoned: "交接已放棄",
  handoff_waiting_resource: "交接等待資源",
  handoff_wait_rescheduled: "交接重新排定等待",
  // 裁定
  decision_requested: "提出裁定",
  decision_answered: "裁定已回答",
  decision_deferred: "裁定已延後",
  decision_superseded: "裁定已失效",
  decision_merged: "裁定已合併",
  decision_merge_received: "併入其他裁定",
  decision_linked: "連結相似裁定",
  // 恢復
  runtime_crash_detected: "偵測到 Runtime 意外終止",
  recovery_attempt_started: "開始恢復嘗試",
  recovery_attempt_completed: "恢復嘗試完成",
  recovery_attempt_failed: "恢復嘗試失敗",
  recovery_succeeded: "恢復成功",
  recovery_exhausted: "恢復次數用盡",
  recovery_retry_requested: "要求重新恢復",
  // 驗證與 Task 狀態
  task_verification_recorded: "記錄驗證結果",
  task_created: "建立 Task",
  task_status_changed: "Task 狀態改變",
  // 清理
  worktree_cleanup_requested: "要求清理 Worktree",
  worktree_cleanup_started: "開始清理 Worktree",
  worktree_cleanup_retry_scheduled: "排定重試清理 Worktree",
  worktree_cleaned: "Worktree 已清理",
  worktree_cleanup_failed: "清理 Worktree 失敗",
  // 外部操作
  external_operation_begun: "開始外部操作",
  external_operation_succeeded: "外部操作成功",
  external_operation_failed: "外部操作失敗",
  external_operation_result_unknown: "外部操作結果未知",
  external_operation_verified: "外部操作已查證",
  external_operation_duplicate_blocked: "擋下重複的外部操作",
  // 教訓、用量
  lesson_recurrence_recorded: "記錄教訓重犯",
  run_usage_reported: "回報 Run 用量",
  // Controller CLI
  controller_cli_opened: "開啟 Controller CLI",
  controller_cli_launch_started: "開始啟動 Controller CLI",
  controller_cli_running: "Controller CLI 執行中",
  controller_cli_end_requested: "要求結束 Controller CLI",
  controller_cli_switch_requested: "要求換 Controller 帳號",
  controller_cli_ended: "Controller CLI 已結束",
  controller_cli_interrupted: "Controller CLI 中斷",
  controller_cli_rebound: "Controller CLI 重新綁定",
};

/**
 * Token 用量各階段的名稱（MASTER 7.1）：Runtime 回報的是 Stage key，這裡對照內建 Playbook 的 Stage 名稱
 * （crates/baton-domain/src/workflow/builtin.rs）。同一個 key 在不同 Playbook 名稱不同時用共通的說法；
 * 沒有對照的 key（自訂 Playbook）才顯示原始代碼。
 */
const STAGE_LABELS: Record<string, string> = {
  // 需求仍模糊
  read_current_state: "閱讀現況",
  sharp_questions: "尖銳問題與情境",
  research_prototype: "研究／原型",
  decision: "裁定",
  update_brief: "更新 Brief 與詞彙",
  // 新功能
  requirements_contract: "需求契約",
  design: "設計",
  slicing: "垂直切片與依賴",
  tests_and_implementation: "行為測試及實作",
  real_interface_verification: "真實介面驗證",
  independent_review: "獨立審查",
  integration: "整合",
  // 缺陷處理（reproduction 也出現在探索測試，名稱是「重現確認」）
  reproduction: "重現",
  minimization: "最小化",
  hypothesis: "可否證假說",
  probe: "探測",
  fix: "修復",
  regression_verification: "原案例與回歸驗證",
  // 遷移（current_state 在需求仍模糊時叫「閱讀現況」）
  current_state: "現況",
  alternatives: "替代設計",
  impact: "影響範圍",
  verifiable_units: "可驗證單位",
  migrate: "遷移",
  contract_check: "契約核對",
  remove_old_path: "刪除舊路徑",
  // 效能
  metrics_and_load: "定義指標及代表負載",
  baseline: "基線",
  hypothesis_experiment: "假說與實驗",
  comparison: "同條件比較",
  explanation: "解釋變化",
  repeatable_measurement: "保留可重跑量測",
  // 調查與研究
  question: "明確問題",
  sources_experiments: "一手來源／實驗",
  evidence_uncertainty: "證據與不確定性",
  conclusion: "結論或裁定",
  // 探索測試
  charter: "劃分探索章程與角色",
  isolated_execution: "隔離執行",
  collect_suspicions: "收集疑點",
  triage: "排除環境與預期行為",
  fix_and_regression: "修復及回歸",
};

/** 事件類型的中文名稱；沒有對照時為 null。 */
function eventTypeLabel(eventType: string): string | null {
  return Object.hasOwn(EVENT_TYPE_LABELS, eventType) ? EVENT_TYPE_LABELS[eventType] : null;
}

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

/** 一項指標的主數值（title 字級、粗體、等寬數字）；前面的標籤與後面的單位小一級，其餘說明放在下一行的 dd。 */
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
    <section className="page timeline-page" aria-label="時間線">
      <header className="page-header">
        <h2>時間線</h2>
      </header>

      {/* 指標摘要在上（精簡的一張數字表），事件清單在下 */}
      <section className="section timeline-metrics" aria-labelledby="timeline-metrics-title">
        <div className="section-heading">
          <h3 id="timeline-metrics-title">觀測指標</h3>
          {metrics.state === "ready" && <p>由 {metrics.value.metrics.events} 個時間線事件計算</p>}
        </div>
        {metrics.state === "loading" && <p className="empty-note">載入中…</p>}
        {metrics.state === "failed" && <ErrorNotice title="無法取得觀測指標" reason={metrics.reason} />}
        {metrics.state === "ready" && <MetricsView metrics={metrics.value.metrics} quotas={metrics.value.server_quota} />}
      </section>

      <section className="section timeline-events" aria-labelledby="timeline-events-title">
        <div className="section-heading">
          <h3 id="timeline-events-title">事件時間線</h3>
          {timeline.state === "ready" && <span className="count">{matched.length}</span>}
        </div>
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
        {timeline.state === "loading" && <p className="empty-note">載入中…</p>}
        {timeline.state === "failed" && <ErrorNotice title="無法取得時間線" reason={timeline.reason} />}
        {timeline.state === "ready" && timeline.value.length === 0 && (
          <p className="empty-note">{taskId === null ? "這個 Workspace 還沒有事件" : "這個 Task 沒有事件"}</p>
        )}
        {timeline.state === "ready" && timeline.value.length > 0 && matched.length === 0 && (
          <p className="empty-note">沒有符合「{keyword.trim()}」的事件</p>
        )}
        {matched.length > 0 && (
          // 每列可快速掃讀：時間｜類別｜事件類型與摘要；對象、完整 Task 與事件內容收在「詳細資訊」
          <ol className="row-list timeline-list" aria-label="事件">
            {pageEvents.map((event) => (
              <li key={event.seq} className={`timeline-event timeline-category-${event.category}`}>
                <span className="timeline-time tabular">{formatTimestamp(event.occurred_at, timeZone)}</span>
                <span className="timeline-category">
                  <span className="badge badge-plain">{CATEGORY_LABELS[event.category]}</span>
                </span>
                <div className="timeline-event-main">
                  <EventType eventType={event.event_type} />
                  <p className="meta timeline-facts">
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
                  </p>
                  <EventDetails event={event} taskLabel={taskLabel} />
                </div>
              </li>
            ))}
          </ol>
        )}
        {matched.length > 0 && (
          <nav className="timeline-pagination" aria-label="時間線分頁">
            <button type="button" disabled={currentPage === 0} onClick={() => setPage(currentPage - 1)}>
              上一頁
            </button>
            <span className="tabular">
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

/** 讀取失敗（Runtime 的說明可能超過兩行，MASTER 7：`.notice`）。 */
function ErrorNotice({ title, reason }: { title: string; reason: string }) {
  return (
    <div className="notice notice-danger" role="alert">
      <p className="notice-title">{title}</p>
      <p>{reason}</p>
    </div>
  );
}

/** 事件類型：中文名稱為主要文字（原始代碼在 title 與「詳細資訊」）；沒有中文名稱時顯示代碼（等寬、次要色）。 */
function EventType({ eventType }: { eventType: string }) {
  const label = eventTypeLabel(eventType);
  if (label === null) return <code className="timeline-type timeline-type-code mono">{eventType}</code>;
  return (
    <span className="timeline-type" title={eventType}>
      {label}
    </span>
  );
}

/** 事件的詳細資訊（預設收合）：事件類型代碼、對象、完整的 Task 名稱與事件內容。 */
function EventDetails({ event, taskLabel }: { event: TimelineEvent; taskLabel: (id: string) => string }) {
  const { payload } = event;
  const hasPayload =
    payload !== null && payload !== undefined && (typeof payload !== "object" || Object.keys(payload).length > 0);
  return (
    <details className="timeline-event-more">
      <summary>詳細資訊</summary>
      <dl className="facts">
        <dt>事件類型</dt>
        <dd>
          <code className="mono">{event.event_type}</code>
        </dd>
        <dt>對象</dt>
        <dd className="timeline-aggregate">
          <span>{event.aggregate_type}</span>
          <ShortValue value={event.aggregate_id} kind="id" />
        </dd>
        {event.task_ids.length > 0 && (
          <>
            <dt>Task</dt>
            <dd>{event.task_ids.map(taskLabel).join("、")}</dd>
          </>
        )}
        {hasPayload && (
          <>
            <dt>內容</dt>
            <dd>
              <pre>{JSON.stringify(payload, null, 2)}</pre>
            </dd>
          </>
        )}
      </dl>
    </details>
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
    eventTypeLabel(event.event_type) ?? "",
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

/** 階段名稱：中文名稱為主要文字（key 在 title）；沒有對照時顯示 key（等寬、次要色）；null 是未分階段。 */
function StageName({ stage }: { stage: string | null }) {
  if (stage === null) return <>未分階段</>;
  if (!Object.hasOwn(STAGE_LABELS, stage)) return <code className="mono timeline-stage-code">{stage}</code>;
  return <span title={stage}>{STAGE_LABELS[stage]}</span>;
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
      <dl className="timeline-metric-list">
        <Metric name="工作完成率">
          <RatioValue ratio={task_completion.completion} />
          <dd>已取消 {task_completion.cancelled}</dd>
        </Metric>
        <Metric name="交接成功率">
          <RatioValue ratio={handoff.success} />
          <dd className="meta">
            <span>已放棄 {handoff.abandoned}</span>
            <span>已確認接手 {handoff.confirmed}</span>
            <span>進行中 {handoff.in_flight}</span>
          </dd>
        </Metric>
        <Metric name="恢復耗時">
          <DurationValue stats={recovery} />
          <dd className="meta">
            <span>已恢復 {recovery.count} 次</span>
            <span>尚未恢復 {recovery.pending}</span>
          </dd>
        </Metric>
        <Metric name="重複操作事件">
          <Value prefix="被擋下 " main={String(duplicate_operations.blocked)} unit=" 次" />
          <dd className="meta">
            <span>已成功 {duplicate_operations.already_succeeded}</span>
            <span>結果未知 {duplicate_operations.result_unknown}</span>
            <span>他處進行中 {duplicate_operations.in_progress_elsewhere}</span>
          </dd>
          <dd>變成結果未知 {duplicate_operations.unknown_outcomes} 次</dd>
        </Metric>
        <Metric name="裁定等待時間">
          <DurationValue stats={decision_wait} />
          <dd className="meta">
            <span>已回答 {decision_wait.count}</span>
            <span>等待中 {decision_wait.pending}</span>
          </dd>
        </Metric>
        <Metric name="教訓重犯率">
          <RatioValue ratio={lesson_recurrence.rate} />
          <dd className="meta">
            <span>重犯 {lesson_recurrence.recurrences} 次</span>
            <span>Guard 擋下 {lesson_recurrence.prevented}</span>
            <span>使用者回報 {lesson_recurrence.reported}</span>
          </dd>
        </Metric>
        <Metric name="Token 用量">
          <TokensValue prefix="總計 " value={usage.total_tokens} />
          <dd>共 {usage.runs} 個 Run</dd>
          {/* 目前 Runtime 回報用量時不帶 Stage，只會有一列「未分階段」，和總計重複：有 Stage 時才列出各階段。
              階段多時在格子內捲動，不把同列的格子撐高 */}
          {usage.by_stage.some((stage) => stage.stage !== null) && (
            <dd className="timeline-metric-stages">
              {usage.by_stage.map((stage) => (
                <span key={stage.stage ?? ""} className="timeline-metric-stage">
                  <StageName stage={stage.stage} />：{tokens(stage.tokens)}（{stage.runs} 個 Run）
                </span>
              ))}
            </dd>
          )}
        </Metric>
        <Metric name="Context 使用量">
          <TokensValue prefix="平均 " value={context.average_tokens} />
          <dd>最長 {tokens(context.max_tokens)}</dd>
          <dd className="meta">
            <span>已回報 {context.runs_known} 個 Run</span>
            <span>未回報 {context.runs_unknown} 個 Run</span>
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
        <p className="empty-note">沒有帳號</p>
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
