import { useEffect, useRef, useState, type FormEvent } from "react";

import { CopyButton, ShortValue } from "./LongText";
import { COLUMNS } from "./TaskBoard";
import { shortId, sizeLabel } from "./textFormat";
import { formatTimestamp } from "./time";
import {
  errorCode,
  type ActionRecord,
  type EvidenceArtifact,
  type EvidenceArtifactContent,
  type EvidenceBundle,
  type PublishProfileResponse,
  type RunVerificationRequest,
  type TaskClient,
  type TaskView,
  type UncoveredItem,
  type VerificationAssertion,
  type VerificationAttempt,
  type VerificationOverview,
  type VerificationPhase,
  type VerificationProfileClient,
  type VerificationProfileVersion,
  type VerificationRequest,
  type VerificationStepRecord,
  type VerificationUsage,
  type VerificationVerdict,
  type Workspace,
} from "./runtimeClient";

type Loaded = { state: "loading" } | { state: "ready"; overview: VerificationOverview } | { state: "failed"; reason: string };

/** 要求執行時可選的 Task；讀取失敗時退回自由輸入 Task 識別碼。 */
type TaskChoices = { state: "loading" } | { state: "ready"; tasks: TaskView[] } | { state: "failed" };

const TASK_STATUS_LABELS = new Map(COLUMNS);

interface Props {
  /** 要求執行的 Task 選單另外從 Task 清單取得。 */
  client: VerificationProfileClient & Pick<TaskClient, "getTaskBoard">;
  workspace: Workspace;
  /** 顯示用的時區；省略時使用系統時區（spec D-33），只有測試會指定。 */
  timeZone?: string;
}

/** 七種判定的中文標籤；只有「通過」算通過。 */
export const VERDICT_LABELS: Record<VerificationVerdict, string> = {
  passed: "通過",
  product_failure: "產品缺陷",
  inconclusive: "證據不足",
  environment_blocked: "環境阻礙",
  auth_blocked: "授權阻礙",
  budget_exhausted: "預算用盡",
  automation_unsupported: "無法自動化",
};

/**
 * 判定的色調：只有通過是綠色；產品缺陷紅色、環境阻礙橘色、授權阻礙洋紅、預算用盡琥珀色，
 * 證據不足與無法自動化是中性（不是產品缺陷，也不能看起來像通過）。
 */
const VERDICT_TONES: Record<VerificationVerdict, "success" | "danger" | "orange" | "magenta" | "warning" | null> = {
  passed: "success",
  product_failure: "danger",
  inconclusive: null,
  environment_blocked: "orange",
  auth_blocked: "magenta",
  budget_exhausted: "warning",
  automation_unsupported: null,
};

const PHASE_LABELS: Record<VerificationPhase, string> = {
  build: "建置識別",
  doctor: "環境檢查",
  start: "啟動",
  readiness: "就緒條件",
  setup: "資料準備",
  login: "登入",
  identity: "身分核對",
  journey: "旅程",
  teardown: "清理",
};

/** 票 46、49 新增的 reason_code 的中文說明；其他代碼只顯示代碼本身。 */
const REASON_LABELS: Record<string, string> = {
  judge_rate_limited: "模型限流，無法判定，不是產品缺陷",
  judge_unavailable: "語意判斷無法使用，無法判定，不是產品缺陷",
  model_call_budget_exhausted: "模型呼叫預算已用盡，沒有判定",
  action_failed: "瀏覽器操作沒有完成",
  browser_unavailable: "無法使用瀏覽器",
  login_failed: "登入失敗",
  identity_not_confirmed: "無法確認實際登入的身分",
  secret_unavailable: "取不到秘密的值",
  blocked_by_login: "登入失敗，旅程沒有執行",
  blocked_by_identity: "無法確認登入身分，旅程沒有執行",
  // 票 48：目標導向動作與 no_live_model
  live_model_disabled: "這次驗證不呼叫模型，無法判定，不是產品缺陷",
  planner_unavailable: "無法使用模型決定動作，無法判定，不是產品缺陷",
  planner_rate_limited: "模型限流，無法決定動作，不是產品缺陷",
  act_not_achievable: "模型判斷在目前畫面無法達成目標，無法判定",
  act_postcondition_failed: "模型決定的動作沒有通過後置條件，證據不足，不是產品缺陷",
  replay_postcondition_failed: "重播的動作沒有通過後置條件，錄製已標為 stale，證據不足",
  external_outcome_unknown: "外部操作結果未知，不重送",
  external_already_sent: "外部操作已送出、旅程沒完成，不重跑",
  journal_unavailable: "無法先記下外部操作，所以沒有送出",
  previous_session_unconfirmed: "前一次嘗試的測試程序無法確認已停止，這次不執行",
};

const ATTEMPT_STATUS_LABELS: Record<VerificationAttempt["status"], string> = {
  running: "執行中",
  handed_off: "已交接",
  interrupted: "已中斷",
  completed: "完成",
};

/** 嘗試結束原因的中文說明。 */
const END_REASON_LABELS: Record<string, string> = {
  judge_rate_limited: "語意判斷的帳號限流，交接給其他帳號",
  runtime_interrupted: "Runtime 在執行途中中斷",
};

/** 語意判斷沒有做出判定時步驟的 reason_code；這時 judge 斷言未通過不代表不符合。 */
const UNDECIDED_JUDGE_REASONS = new Set([
  "judge_rate_limited",
  "judge_unavailable",
  "model_call_budget_exhausted",
  "live_model_disabled",
]);

/** 目標導向動作執行前錄製的狀態（票 48）。 */
const CACHE_LABELS: Record<ActionRecord["cache"], string> = {
  none: "無錄製",
  valid: "已重播",
  stale: "先前已 stale",
  mismatch: "身分不一致",
  production_not_replayable: "正式環境不重播",
};

/** 動作的來源；沒有 source 表示這次沒有執行任何動作。 */
const SOURCE_LABELS: Record<NonNullable<ActionRecord["source"]>, string> = {
  live: "即時",
  replay: "重播",
};

/** 這次把錄製標為 stale 的原因；其他代碼只顯示代碼本身。 */
const STALE_REASON_LABELS: Record<string, string> = {
  case_changed: "案例改變",
  goal_changed: "目標改變",
  params_changed: "參數改變",
  persona_changed: "persona 或登入身分改變",
  engine_changed: "引擎改變",
  data_changed: "資料相容性改變",
  postcondition_failed: "重播後置條件不成立",
};

const ARTIFACT_KIND_LABELS: Record<EvidenceArtifact["kind"], string> = {
  screenshot: "截圖",
  trace: "trace",
  video: "錄影",
};

/** 證據 Artifact 沒有保存的原因。 */
const WITHHELD_LABELS: Record<string, string> = {
  secret_detected: "內容含有秘密的值",
  secret_input_recorded: "這個 Session 輸入過秘密，無法確認 trace／錄影已遮蔽",
  too_large: "檔案太大",
  storage_failed: "保存失敗",
};

const UNPROVEN_NOTICE = "這個版本還沒有實際跑過代表旅程，不能當作驗證證據";

/** 還沒有 Profile 時預填的最小範例：一條代表旅程與一項功能。 */
const STARTER_DEFINITION = {
  interface: "cli_api",
  journeys: [
    {
      key: "smoke",
      title: "代表旅程",
      representative: true,
      steps: [
        {
          key: "version",
          title: "顯示版本",
          operation: { kind: "command", program: "your-cli", args: ["--version"] },
          assertions: [{ kind: "exit_code", equals: 0 }],
        },
      ],
    },
  ],
  feature_map: [{ key: "cli", title: "命令列", criterion: "可以顯示版本", journeys: ["smoke"] }],
};

function reasonOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** 錯誤訊息加上 Runtime 的錯誤代碼（Tauri 版的訊息已含代碼時不重複）。 */
function describeError(error: unknown): string {
  const message = reasonOf(error);
  const code = errorCode(error);
  return code !== null && !message.includes(code) ? `${message}（${code}）` : message;
}

function describeAssertion(assertion: VerificationAssertion): string {
  switch (assertion.kind) {
    case "exit_code":
      return `結束碼為 ${assertion.equals}`;
    case "status":
      return `HTTP 狀態碼為 ${assertion.equals}`;
    case "contains":
      return `包含「${assertion.text}」`;
    case "not_contains":
      return `不包含「${assertion.text}」`;
    case "json_equals":
      return `JSON ${assertion.pointer} 等於 ${JSON.stringify(assertion.value)}`;
    case "visible":
      return `「${assertion.selector}」可見`;
    case "url_contains":
      return `網址包含「${assertion.text}」`;
    case "judge":
      return `語意判斷（預期：${assertion.expectation}）`;
  }
}

/** reason_code 的中文說明；沒有中文說明時是 undefined（探索頁的重現紀錄共用）。 */
export function reasonLabel(code: string): string | undefined {
  return REASON_LABELS[code];
}

/**
 * 判定徽章旁的 reason_code 中文說明，代碼放在 title；沒有中文說明時不顯示，
 * 代碼只在徽章的 title 與收合的「步驟」（ReasonCodeNote）裡，不當主要文字（MASTER 7.1）。
 */
function ReasonLabel({ code }: { code: string }) {
  const label = REASON_LABELS[code];
  return label === undefined ? null : (
    <span className="verification-reason" title={code}>
      {label}
    </span>
  );
}

/** 判定代碼：等寬。只用在預設收合的「步驟」裡；其他地方的判定已有中文徽章，代碼只放在徽章的 title（MASTER 7.1）。 */
function ReasonCodeNote({ code }: { code: string }) {
  return (
    <span>
      判定代碼 <code className="mono">{code}</code>
    </span>
  );
}

/** 沒有徽章的代碼（結束原因、不重送的原因）：有中文說明時顯示說明、代碼在 title；沒有時才顯示代碼。 */
function CodeLabel({ code, labels }: { code: string; labels: Record<string, string> }) {
  const label = labels[code];
  return label === undefined ? (
    <code className="mono verification-subtle">{code}</code>
  ) : (
    <span title={code}>{label}</span>
  );
}

/** Profile 中 persona／登入身分的顯示名稱：找得到標題時為「標題（key）」。 */
function titledKey(items: { key: string; title: string }[] | undefined, key: string): string {
  const title = items?.find((item) => item.key === key)?.title;
  return title === undefined ? key : `${title}（${key}）`;
}

/** 判定徽章（探索頁的重現紀錄共用）；`code` 是判定代碼（reason_code），放在 title，不當主要文字。 */
export function VerdictBadge({ verdict, code }: { verdict: VerificationVerdict; code?: string }) {
  const tone = VERDICT_TONES[verdict];
  return (
    <span
      className={tone === null ? `badge verdict verdict-${verdict}` : `badge badge-${tone} verdict verdict-${verdict}`}
      title={code}
    >
      {VERDICT_LABELS[verdict]}
    </span>
  );
}

/**
 * 驗證（票 45，spec D-29）：最上面是現況（Profile 版本、是否可用、最近一次判定），接著是 Evidence Bundle（最新的展開）、
 * 要求執行驗證；Feature Map 與未覆蓋範圍是參考區段；很少做的「發布新版本」放最後並預設收合。
 */
export function VerificationPage({ client, workspace, timeZone }: Props) {
  const [loaded, setLoaded] = useState<Loaded>({ state: "loading" });

  useEffect(() => {
    // 卸載後遲到的回應一律忽略
    let cancelled = false;
    client.verificationOverview(workspace.id).then(
      (overview) => !cancelled && setLoaded({ state: "ready", overview }),
      (e: unknown) => !cancelled && setLoaded({ state: "failed", reason: reasonOf(e) }),
    );
    return () => {
      cancelled = true;
    };
  }, [client, workspace.id]);

  const [taskChoices, setTaskChoices] = useState<TaskChoices>({ state: "loading" });

  useEffect(() => {
    // Task 清單只用於要求執行的選單：讀取失敗時退回文字輸入，不影響其他區段
    let cancelled = false;
    client.getTaskBoard(workspace.id).then(
      (board) => !cancelled && setTaskChoices({ state: "ready", tasks: board.tasks }),
      () => !cancelled && setTaskChoices({ state: "failed" }),
    );
    return () => {
      cancelled = true;
    };
  }, [client, workspace.id]);

  // Task 清單也用來把 Evidence Bundle 綁定的 Task 顯示成標題；讀不到時退回顯示識別碼
  const taskTitles = new Map(taskChoices.state === "ready" ? taskChoices.tasks.map((task) => [task.id, task.title]) : []);

  // 重新載入時不切回「載入中」，保留使用者正在編輯的 Profile 內容
  const reload = async () => {
    try {
      setLoaded({ state: "ready", overview: await client.verificationOverview(workspace.id) });
    } catch (e) {
      setLoaded({ state: "failed", reason: reasonOf(e) });
    }
  };

  return (
    <section className="page verification-page" aria-label="驗證">
      <header className="page-header">
        <h2>驗證</h2>
        {loaded.state === "ready" && (
          <div className="page-actions">
            <button type="button" onClick={() => void reload()}>
              重新整理
            </button>
          </div>
        )}
      </header>
      {loaded.state === "loading" && <p className="empty-note">載入中…</p>}
      {loaded.state === "failed" && (
        <div className="notice notice-danger" role="alert">
          <p>無法取得驗證資料：{loaded.reason}</p>
        </div>
      )}
      {loaded.state === "ready" && (
        <>
          <StatusSummary overview={loaded.overview} timeZone={timeZone} />
          <section className="section" aria-labelledby="verification-bundles">
            <div className="section-heading">
              <h3 id="verification-bundles">最近的 Evidence Bundle</h3>
              <span className="count">{loaded.overview.bundles.length}</span>
              {loaded.overview.bundles.length > 1 && <p>最新的一份完整展開；較舊的只列判定，需要時再展開詳細內容。</p>}
            </div>
            {loaded.overview.bundles.length === 0 && (
              <p className="empty-note">還沒有 Evidence Bundle；要求執行驗證後，結果會在這裡顯示。</p>
            )}
            {loaded.overview.bundles.map((bundle, index) => (
              <BundleCard
                key={bundle.id}
                bundle={bundle}
                latest={index === 0}
                profile={loaded.overview.profile}
                taskTitle={bundle.task_id === null ? undefined : taskTitles.get(bundle.task_id)}
                timeZone={timeZone}
                onLoadArtifact={(artifactIndex) => client.evidenceArtifact(workspace.id, bundle.id, artifactIndex)}
              />
            ))}
          </section>
          <RunForm
            overview={loaded.overview}
            taskChoices={taskChoices}
            onRun={(request) => client.runVerification(workspace.id, request)}
            onQueued={reload}
          />
          {loaded.overview.profile !== null && <FeatureMapSection overview={loaded.overview} profile={loaded.overview.profile} />}
          {loaded.overview.profile?.definition.interface === "web" && (
            <IdentitySection definition={loaded.overview.profile.definition} />
          )}
          <PublishForm
            profile={loaded.overview.profile}
            onPublish={(definition) => client.publishVerificationProfile(workspace.id, definition)}
            onPublished={reload}
          />
        </>
      )}
    </section>
  );
}

/** 現況：Profile 版本與發布時間、是否已被證明可用、最近一次 Evidence Bundle 的判定與時間。 */
function StatusSummary({ overview, timeZone }: { overview: VerificationOverview; timeZone?: string }) {
  const { profile } = overview;
  const latest = overview.bundles.at(0);
  return (
    <section className="section" aria-labelledby="verification-status">
      <div className="section-heading">
        <h3 id="verification-status">現況</h3>
      </div>
      {profile === null ? (
        <div className="callout callout-warning">
          <p className="callout-title">還沒有 Verification Profile</p>
          <p>在最下方的「發布新版本」填入 Verification Profile 內容並發布後，才能要求執行驗證。</p>
        </div>
      ) : (
        <dl className="facts verification-status">
          <dt>Verification Profile</dt>
          <dd>
            第 {profile.version} 版<span className="verification-subtle">（{formatTimestamp(profile.created_at, timeZone)} 發布）</span>
          </dd>
          <dt>是否可用</dt>
          {overview.proven_by !== null ? (
            <dd>
              <span className="badge badge-success">可用</span> 已實際跑過並通過代表旅程（由 Evidence Bundle{" "}
              <ShortValue value={overview.proven_by} kind="id" /> 證明）
            </dd>
          ) : (
            <dd>
              <span className="badge badge-warning">未證明</span> {UNPROVEN_NOTICE}
            </dd>
          )}
          <dt>最近一次驗證</dt>
          <dd>
            {latest === undefined ? (
              "還沒有執行過"
            ) : (
              <>
                <VerdictBadge verdict={latest.verdict} code={latest.reason_code} />{" "}
                <span className="verification-subtle">{formatTimestamp(latest.finished_at, timeZone)} 完成</span>
              </>
            )}
          </dd>
        </dl>
      )}
    </section>
  );
}

function journeyTitle(profile: VerificationProfileVersion | null, key: string): string {
  return profile?.definition.journeys.find((j) => j.key === key)?.title ?? key;
}

/** Feature Map（參考）：每項功能的驗收準則與旅程，以及 Profile 宣告的未覆蓋範圍。 */
function FeatureMapSection({ overview, profile }: { overview: VerificationOverview; profile: VerificationProfileVersion }) {
  const features = profile.definition.feature_map;
  return (
    <section className="section" aria-labelledby="verification-features">
      <div className="section-heading">
        <h3 id="verification-features">Feature Map</h3>
        <span className="count">{features.length}</span>
        <p>每項功能的驗收準則與驗證它的旅程；沒有旅程的功能不在自動驗證內。</p>
      </div>
      <div className="table-wrap">
        <table className="data-table verification-feature-table" aria-label="Feature Map">
          <thead>
            <tr>
              <th scope="col">功能</th>
              <th scope="col">驗收準則</th>
              <th scope="col">旅程</th>
            </tr>
          </thead>
          <tbody>
            {features.map((feature) => (
              <tr key={feature.key}>
                <th scope="row">{feature.title}</th>
                <td>{feature.criterion}</td>
                <td>
                  {feature.journeys.length > 0 ? (
                    feature.journeys.map((key) => journeyTitle(profile, key)).join("、")
                  ) : (
                    <span className="badge badge-warning">沒有旅程（未覆蓋）</span>
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      {overview.uncovered.length > 0 ? (
        <UncoveredList items={overview.uncovered} label="未覆蓋範圍" level={4} />
      ) : (
        <p className="empty-note">每項功能都有旅程，也沒有宣告未覆蓋的範圍。</p>
      )}
    </section>
  );
}

/** web 介面的秘密參照、persona 與登入身分；秘密只顯示 key 與參照，前端拿不到值。 */
function IdentitySection({ definition }: { definition: VerificationProfileVersion["definition"] }) {
  const secrets = definition.secrets ?? [];
  const personas = definition.personas ?? [];
  const identities = definition.identities ?? [];
  return (
    <section className="section" aria-labelledby="verification-identities">
      <div className="section-heading">
        <h3 id="verification-identities">身分與秘密</h3>
        <p>web 驗證使用的 persona、登入身分與秘密參照。</p>
      </div>
      <div className="verification-identity-grid">
        <div className="verification-block">
          <h4>Persona</h4>
          {personas.length === 0 ? (
            <p className="empty-note">沒有 persona。</p>
          ) : (
            <ul className="verification-plain-list" aria-label="Persona">
              {personas.map((persona) => (
                <li key={persona.key}>
                  {persona.title}（{persona.key}）{persona.description !== "" && `：${persona.description}`}
                </li>
              ))}
            </ul>
          )}
        </div>
        <div className="verification-block">
          <h4>登入身分</h4>
          {identities.length === 0 ? (
            <p className="empty-note">沒有登入身分；旅程都不登入。</p>
          ) : (
            <ul className="verification-plain-list" aria-label="登入身分">
              {identities.map((identity) => (
                <li key={identity.key}>
                  {identity.title}（{identity.key}）：登入 {identity.login.length} 個步驟，以「
                  {identity.check.title === "" ? identity.check.key : identity.check.title}」核對身分
                </li>
              ))}
            </ul>
          )}
        </div>
        <div className="verification-block">
          <h4>秘密參照</h4>
          {secrets.length === 0 ? (
            <p className="empty-note">沒有秘密參照。</p>
          ) : (
            <>
              <p className="field-hint">只記錄參照位置，秘密的值不會顯示，也不會寫入證據。</p>
              <ul className="verification-plain-list" aria-label="秘密參照">
                {secrets.map((secret) => (
                  <li key={secret.key}>
                    <code>{secret.key}</code>：<code>{secret.reference}</code>
                  </li>
                ))}
              </ul>
            </>
          )}
        </div>
      </div>
    </section>
  );
}

/** 未覆蓋的範圍：不在自動驗證內的部分不能被當成已驗證，以琥珀色左側色條標出。 */
function UncoveredList({ items, label, level }: { items: UncoveredItem[]; label: string; level: 4 | 5 }) {
  const Heading = level === 4 ? "h4" : "h5";
  return (
    <div className="verification-uncovered stripe-warning">
      <Heading>
        {label}
        <span className="verification-subtle">（不在這次自動驗證內）</span>
      </Heading>
      <ul aria-label={label}>
        {items.map((item, index) => (
          <li key={index}>
            {item.area}：{item.reason}
          </li>
        ))}
      </ul>
    </div>
  );
}

function PublishForm({
  profile,
  onPublish,
  onPublished,
}: {
  profile: VerificationProfileVersion | null;
  onPublish: (definition: unknown) => Promise<PublishProfileResponse>;
  onPublished: () => Promise<void>;
}) {
  // 只在第一次顯示時預填，之後重新載入不覆蓋使用者的編輯
  const [draft, setDraft] = useState(() => JSON.stringify(profile?.definition ?? STARTER_DEFINITION, null, 2));
  // 還沒有 Profile 時這是唯一能做的事，預設展開；之後重新載入不改變使用者的展開狀態
  const [openInitially] = useState(profile === null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<string | null>(null);
  const errorRef = useRef<HTMLDivElement>(null);
  const resultRef = useRef<HTMLDivElement>(null);

  // 送出時按鈕會停用、焦點跟著消失：結果或錯誤出現後把焦點移過去
  useEffect(() => {
    if (error !== null) errorRef.current?.focus();
  }, [error]);
  useEffect(() => {
    if (result !== null) resultRef.current?.focus();
  }, [result]);

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    setError(null);
    setResult(null);
    let definition: unknown;
    try {
      definition = JSON.parse(draft);
    } catch (e) {
      setError(`JSON 格式不正確：${reasonOf(e)}`);
      return;
    }
    setBusy(true);
    try {
      const published = await onPublish(definition);
      setResult(
        published.created
          ? `已發布第 ${published.profile.version} 版；實際跑過並通過代表旅程後才算可用`
          : `內容與第 ${published.profile.version} 版相同，沒有新增版本`,
      );
      await onPublished();
    } catch (e) {
      setError(`發布失敗：${describeError(e)}`);
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="section" aria-labelledby="verification-publish">
      <div className="section-heading">
        <h3 id="verification-publish">發布新版本</h3>
        <p>Verification Profile 決定怎麼驗收，只有你能發布；舊版本會保留，已有的證據仍綁定原本的版本。</p>
      </div>
      <details className="verification-publish" open={openInitially}>
        <summary>編輯 Verification Profile 內容</summary>
        <form className="panel measure-form" onSubmit={(event) => void submit(event)}>
          <div className="field">
            <label htmlFor="verification-profile-json">Verification Profile 內容（JSON）</label>
            <textarea
              id="verification-profile-json"
              className="verification-json"
              value={draft}
              onChange={(e) => setDraft(e.target.value)}
              rows={16}
              spellCheck={false}
            />
          </div>
          <div className="form-actions">
            <button type="submit" disabled={busy}>
              發布新版本
            </button>
          </div>
        </form>
      </details>
      {error && (
        <div ref={errorRef} className="notice notice-danger" role="alert" tabIndex={-1}>
          <p>{error}</p>
        </div>
      )}
      {result && (
        <div ref={resultRef} className="callout callout-success" role="status" tabIndex={-1}>
          <p>{result}</p>
        </div>
      )}
    </section>
  );
}

function RunForm({
  overview,
  taskChoices,
  onRun,
  onQueued,
}: {
  overview: VerificationOverview;
  taskChoices: TaskChoices;
  onRun: (request: RunVerificationRequest) => Promise<VerificationRequest>;
  onQueued: () => Promise<void>;
}) {
  const [taskId, setTaskId] = useState("");
  const [selected, setSelected] = useState<string[]>([]);
  const [noLiveModel, setNoLiveModel] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [queued, setQueued] = useState<VerificationRequest | null>(null);
  const errorRef = useRef<HTMLDivElement>(null);
  const queuedRef = useRef<HTMLDivElement>(null);
  const profile = overview.profile;

  // 送出時按鈕會停用、焦點跟著消失：已排入的訊息或錯誤出現後把焦點移過去
  useEffect(() => {
    if (error !== null) errorRef.current?.focus();
  }, [error]);
  useEffect(() => {
    if (queued !== null) queuedRef.current?.focus();
  }, [queued]);

  const toggle = (key: string, checked: boolean) =>
    setSelected((current) => (checked ? [...current, key] : current.filter((k) => k !== key)));

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    setBusy(true);
    setError(null);
    setQueued(null);
    try {
      const task = taskId.trim();
      const body: RunVerificationRequest = task === "" ? { journeys: selected } : { task_id: task, journeys: selected };
      // 沒勾選時不送 no_live_model，維持原本的要求內容
      if (noLiveModel) body.no_live_model = true;
      setQueued(await onRun(body));
      await onQueued();
    } catch (e) {
      setError(`無法要求執行：${describeError(e)}`);
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="section" aria-labelledby="verification-run">
      <div className="section-heading">
        <h3 id="verification-run">要求執行驗證</h3>
      </div>
      {profile === null ? (
        <p className="empty-note">發布 Verification Profile 後才能要求執行驗證。</p>
      ) : (
        <form className="panel measure-form verification-run" onSubmit={(event) => void submit(event)}>
          <div className="field">
            {taskChoices.state === "failed" ? (
              <>
                <label htmlFor="verification-task">Task 識別（選填）</label>
                <input
                  id="verification-task"
                  className="select-fit"
                  value={taskId}
                  onChange={(e) => setTaskId(e.target.value)}
                  placeholder="不填時在 Workspace 目錄中驗證"
                />
                <p className="field-hint">無法取得 Task 清單，請直接輸入 Task 識別碼。</p>
              </>
            ) : (
              <>
                <label htmlFor="verification-task">Task（選填）</label>
                <select id="verification-task" className="select-fit" value={taskId} onChange={(e) => setTaskId(e.target.value)}>
                  <option value="">不指定（在 Workspace 目錄中驗證）</option>
                  {taskChoices.state === "ready" &&
                    taskChoices.tasks.map((task) => (
                      <option key={task.id} value={task.id}>
                        {task.title}（{TASK_STATUS_LABELS.get(task.status) ?? task.status}）
                      </option>
                    ))}
                </select>
              </>
            )}
          </div>
          <fieldset className="verification-journey-choices">
            <legend>旅程（都不勾選時執行全部）</legend>
            {profile.definition.journeys.map((journey) => (
              <label key={journey.key}>
                <input
                  type="checkbox"
                  checked={selected.includes(journey.key)}
                  onChange={(e) => toggle(journey.key, e.target.checked)}
                />
                {journey.title}
                {journey.representative && "（代表旅程）"}
              </label>
            ))}
          </fieldset>
          <div className="field">
            {/* 設定鍵 no_live_model 只放在 title，畫面用中文（MASTER 7.1） */}
            <label className="verification-check" title="no_live_model">
              <input type="checkbox" checked={noLiveModel} onChange={(e) => setNoLiveModel(e.target.checked)} />
              不呼叫模型
            </label>
            <p className="field-hint">只執行不需模型的斷言與已驗證動作的重播，需要模型的步驟記為證據不足。</p>
          </div>
          {overview.proven_by === null && (
            <div className="callout callout-accent">
              <p>這個版本還沒被證明可用，代表旅程一定會一起執行。</p>
            </div>
          )}
          <div className="form-actions">
            <button type="submit" disabled={busy}>
              要求執行
            </button>
          </div>
        </form>
      )}
      {error && (
        <div ref={errorRef} className="notice notice-danger" role="alert" tabIndex={-1}>
          <p>{error}</p>
        </div>
      )}
      {queued && (
        <div ref={queuedRef} className="callout callout-success" role="status" tabIndex={-1}>
          {/* 驗證要求的識別碼只放在 title：使用者接下來要看的是結果出現在哪裡 */}
          <p title={`驗證要求 ${queued.id}`}>已排入，結果會出現在「最近的 Evidence Bundle」。</p>
          <p>
            旅程：{queued.journeys.map((key) => journeyTitle(profile, key)).join("、")}（Verification Profile 第{" "}
            {queued.profile_version} 版{queued.no_live_model && "；不呼叫模型"}）
          </p>
        </div>
      )}
    </section>
  );
}

/**
 * 一份 Evidence Bundle：判定一直顯示；最新的一份直接展開內容，較舊的把內容收進「詳細內容」。
 * 卡片左側色條跟著判定的色調。
 */
function BundleCard({
  bundle,
  latest,
  profile,
  taskTitle,
  timeZone,
  onLoadArtifact,
}: {
  bundle: EvidenceBundle;
  latest: boolean;
  profile: VerificationProfileVersion | null;
  /** 綁定的 Task 的標題；沒有綁定或 Task 清單裡找不到時是 undefined。 */
  taskTitle?: string;
  timeZone?: string;
  onLoadArtifact: (index: number) => Promise<EvidenceArtifactContent>;
}) {
  const headingId = `bundle-${bundle.id}`;
  const tone = VERDICT_TONES[bundle.verdict];
  const body = (
    <BundleBody bundle={bundle} profile={profile} taskTitle={taskTitle} timeZone={timeZone} onLoadArtifact={onLoadArtifact} />
  );
  return (
    <section className={`card verification-bundle verification-tone-${tone ?? "neutral"}`} aria-labelledby={headingId}>
      <header className="card-header">
        <div className="verification-bundle-title">
          {/* 標題顯示縮短的識別碼；可存取名稱保留完整識別碼 */}
          <h4 id={headingId} aria-label={`Evidence Bundle ${bundle.id}`}>
            Evidence Bundle{" "}
            <code className="mono" title={bundle.id}>
              {shortId(bundle.id)}
            </code>
          </h4>
          {shortId(bundle.id) !== bundle.id && <CopyButton text={bundle.id} label="複製識別碼" field="Evidence Bundle" />}
        </div>
        <span className="verification-subtle nowrap">{formatTimestamp(bundle.finished_at, timeZone)}</span>
      </header>
      <p className="verification-bundle-verdict">
        判定：<VerdictBadge verdict={bundle.verdict} code={bundle.reason_code} /> <ReasonLabel code={bundle.reason_code} />
      </p>
      {latest ? (
        body
      ) : (
        <details className="verification-bundle-more">
          <summary>詳細內容</summary>
          {body}
        </details>
      )}
    </section>
  );
}

function BundleBody({
  bundle,
  profile,
  taskTitle,
  timeZone,
  onLoadArtifact,
}: {
  bundle: EvidenceBundle;
  profile: VerificationProfileVersion | null;
  taskTitle?: string;
  timeZone?: string;
  onLoadArtifact: (index: number) => Promise<EvidenceArtifactContent>;
}) {
  const featureTitle = (key: string) => profile?.definition.feature_map.find((f) => f.key === key)?.title ?? key;
  const { cleanup } = bundle;
  // 舊的 Bundle 沒有 attempts；只有一次嘗試且沒有承接時不顯示交接區塊
  const attempts = bundle.attempts ?? [];
  const handedOver = attempts.length > 1 || attempts.some((attempt) => attempt.takeover !== null);
  return (
    <div className="verification-bundle-body">
      <div className="split">
        <div className="verification-block">
          <h5>旅程結果</h5>
          <ul className="verification-journeys" aria-label="旅程結果">
            {bundle.journeys.map((journey) => (
              <li key={journey.key} aria-label={journey.title}>
                <p>
                  {journey.title}
                  {journey.representative && "（代表旅程）"}：<VerdictBadge verdict={journey.verdict} code={journey.reason_code} />{" "}
                  <ReasonLabel code={journey.reason_code} />
                </p>
                {(journey.persona !== undefined || journey.identity !== undefined) && (
                  <p className="meta">
                    <span>
                      persona：
                      {journey.persona === undefined ? "未指定" : titledKey(profile?.definition.personas, journey.persona)}
                    </span>
                    <span>
                      登入身分：
                      {journey.identity === undefined ? "未登入" : titledKey(profile?.definition.identities, journey.identity)}
                    </span>
                  </p>
                )}
              </li>
            ))}
          </ul>
          <p className="verification-covered">
            覆蓋的功能：{bundle.covered_features.length === 0 ? "無" : bundle.covered_features.map(featureTitle).join("、")}
          </p>
          {bundle.uncovered.length > 0 && <UncoveredList items={bundle.uncovered} label="這次未覆蓋" level={5} />}
        </div>
        <dl className="facts">
          <dt>建置識別</dt>
          <dd>{bundle.build ?? "無法取得"}</dd>
          <dt>候選版本</dt>
          <dd>
            {bundle.candidate_revision === null ? "無（不是 Git repo）" : <ShortValue value={bundle.candidate_revision} kind="hash" field="候選版本" />}
          </dd>
          <dt>需求版本</dt>
          <dd>
            {bundle.task_id === null ? (
              "沒有綁定 Task"
            ) : (
              <>
                {/* 顯示 Task 標題；Task 清單裡找不到時才顯示識別碼 */}
                {taskTitle === undefined ? (
                  <>
                    Task <ShortValue value={bundle.task_id} kind="id" field="Task" />{" "}
                  </>
                ) : (
                  <span title={bundle.task_id}>「{taskTitle}」</span>
                )}
                契約第 {bundle.requirement_version ?? "?"} 版
              </>
            )}
          </dd>
          <dt>Verification Profile</dt>
          <dd>第 {bundle.profile_version} 版</dd>
          <dt>受驗目錄</dt>
          <dd>
            <ShortValue value={bundle.subject_dir} kind="path" field="受驗目錄" />
          </dd>
          <dt>時間</dt>
          <dd>
            {formatTimestamp(bundle.started_at, timeZone)} – {formatTimestamp(bundle.finished_at, timeZone)}
          </dd>
          <dt>清理</dt>
          <dd>
            {cleanup.state_dir_removed ? "已移除本次驗證的狀態目錄" : "本次驗證的狀態目錄沒有移除"}
            {cleanup.service_stopped !== undefined && (cleanup.service_stopped ? "；服務已停止" : "；服務沒有停止")}
            ；證據已保留
            {cleanup.problems.length > 0 && (
              <ul className="verification-problems" aria-label="清理問題">
                {cleanup.problems.map((problem, index) => (
                  <li key={index}>{problem}</li>
                ))}
              </ul>
            )}
          </dd>
        </dl>
      </div>
      <UsageView bundle={bundle} />
      {bundle.actions !== undefined && bundle.actions.length > 0 && (
        <ActionsView actions={bundle.actions} bundle={bundle} profile={profile} />
      )}
      {handedOver && <AttemptsView attempts={attempts} journeyTitle={(key) => journeyTitle(profile, key)} />}
      {bundle.artifacts.length > 0 && (
        <div className="verification-block">
          <h5>證據檔案</h5>
          <ul className="verification-files" aria-label="證據檔案">
            {bundle.artifacts.map((artifact, index) => (
              <ArtifactItem
                key={index}
                artifact={artifact}
                location={artifactLocation(artifact, bundle, profile)}
                onLoad={() => onLoadArtifact(index)}
              />
            ))}
          </ul>
        </div>
      )}
      <details className="verification-steps">
        <summary>
          步驟 <span className="count">{bundle.steps.length}</span>
        </summary>
        <ol aria-label={`${bundle.id} 的步驟`}>
          {bundle.steps.map((step, index) => (
            <StepView key={index} step={step} journeyTitle={(key) => journeyTitle(profile, key)} />
          ))}
        </ol>
      </details>
    </div>
  );
}

/** 舊的 Bundle 沒有用量時視為 0。 */
const NO_USAGE: VerificationUsage = { live_model_calls: 0, live_actions: 0, replayed_actions: 0, blocked_model_calls: 0 };

/** 用量（票 48）：即時模型與重播分開呈現，重播不呼叫模型。 */
function UsageView({ bundle }: { bundle: EvidenceBundle }) {
  const usage = { ...NO_USAGE, ...bundle.usage };
  return (
    <div className="verification-block">
      <h5>用量</h5>
      {bundle.no_live_model && (
        <div className="callout callout-warning">
          <p title="no_live_model">這次驗證設定為不呼叫模型</p>
        </div>
      )}
      <ul className="verification-usage" aria-label="用量">
        <li aria-label="即時">
          即時：模型呼叫 {usage.live_model_calls} 次、模型決定的動作 {usage.live_actions} 個
        </li>
        <li aria-label="重播">重播：重播的動作 {usage.replayed_actions} 個（不呼叫模型）</li>
        <li aria-label="擋下的模型呼叫" title="no_live_model">
          因不呼叫模型而擋下的模型呼叫 {usage.blocked_model_calls} 次
        </li>
      </ul>
    </div>
  );
}

/** 目標導向動作（票 48）：每個動作的來源與錄製的處理；判定仍看步驟結果。 */
function ActionsView({
  actions,
  bundle,
  profile,
}: {
  actions: ActionRecord[];
  bundle: EvidenceBundle;
  profile: VerificationProfileVersion | null;
}) {
  return (
    <div className="verification-block">
      <h5>目標導向動作</h5>
      <p className="field-hint">重播是重新執行真實動作並檢查後置條件；快取命中不是通過，判定看步驟結果。</p>
      <div className="table-wrap">
        <table className="data-table verification-actions-table" aria-label="目標導向動作">
          <thead>
            <tr>
              <th>旅程／步驟</th>
              <th>目標</th>
              <th>來源</th>
              <th>實際動作</th>
              <th>執行前錄製狀態</th>
              <th>這次標為 stale 的原因</th>
              <th>錄製</th>
            </tr>
          </thead>
          <tbody>
            {actions.map((record) => (
              <tr key={`${record.journey}/${record.step}`}>
                <td>
                  {journeyTitle(profile, record.journey)}／{stepTitle(bundle, record.journey, record.step)}
                </td>
                <td>{record.goal}</td>
                <td>{record.source === undefined ? "未執行" : SOURCE_LABELS[record.source]}</td>
                <td>{record.action ?? "—"}</td>
                <td>{CACHE_LABELS[record.cache]}</td>
                <td>
                  {record.stale_reasons === undefined || record.stale_reasons.length === 0
                    ? "—"
                    : record.stale_reasons.map((reason) => STALE_REASON_LABELS[reason] ?? reason).join("、")}
                </td>
                <td>{record.recorded === undefined ? "未錄製" : "已錄製"}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}

/** 步驟的標題（找不到或沒有標題時用 key）。 */
function stepTitle(bundle: EvidenceBundle, journey: string, key: string): string {
  const step = bundle.steps.find((s) => s.journey === journey && s.key === key);
  return step === undefined || step.title === "" ? key : step.title;
}

/** 步驟名稱：有旅程時是「旅程／步驟」。 */
function stepName(journey: string | undefined, name: string, journeyTitleOf: (key: string) => string): string {
  return journey === undefined ? name : `${journeyTitleOf(journey)}／${name}`;
}

/** 嘗試與交接（票 49）：每次嘗試的狀態、judge 帳號、採用的旅程與步驟，以及承接時停止的程序與不重送的外部操作。 */
function AttemptsView({ attempts, journeyTitle }: { attempts: VerificationAttempt[]; journeyTitle: (key: string) => string }) {
  const titles = (keys: string[]) => (keys.length === 0 ? "無" : keys.map(journeyTitle).join("、"));
  const ids = (values: string[]) => (values.length === 0 ? "無" : values.join("、"));
  return (
    <div className="verification-block" role="group" aria-label="嘗試與交接">
      <h5>嘗試與交接</h5>
      <ol className="verification-attempts">
        {attempts.map((attempt) => {
          const { takeover } = attempt;
          return (
            <li key={attempt.attempt} aria-label={`第 ${attempt.attempt} 次嘗試`}>
              <p className="verification-attempt-head">
                <strong>第 {attempt.attempt} 次嘗試</strong>：{ATTEMPT_STATUS_LABELS[attempt.status]}
              </p>
              <dl className="facts">
                {attempt.end_reason !== null && (
                  <>
                    <dt>結束原因</dt>
                    <dd>
                      <CodeLabel code={attempt.end_reason} labels={END_REASON_LABELS} />
                    </dd>
                  </>
                )}
                <dt>語意判斷帳號</dt>
                <dd>{attempt.judge_profile_id ?? "沒有可用帳號（只執行不需模型的斷言）"}</dd>
                <dt>結果採用自這次的旅程</dt>
                <dd>{titles(attempt.journeys)}</dd>
                <dt>完成的步驟</dt>
                <dd>{attempt.steps.length} 個</dd>
              </dl>
              {attempt.steps.length > 0 && (
                <ul className="verification-plain-list verification-attempt-steps" aria-label={`第 ${attempt.attempt} 次嘗試的步驟`}>
                  {attempt.steps.map((step, index) => (
                    <li key={index}>
                      <p>
                        <span className="badge badge-plain">{PHASE_LABELS[step.phase]}</span> {stepName(step.journey, step.key, journeyTitle)}：
                        <VerdictBadge verdict={step.verdict} code={step.reason_code} /> <ReasonLabel code={step.reason_code} />
                      </p>
                    </li>
                  ))}
                </ul>
              )}
              {takeover !== null && (
                <div className="inset" role="group" aria-label={`第 ${attempt.attempt} 次嘗試的承接`}>
                  <dl className="facts">
                    <dt>承接</dt>
                    <dd>第 {takeover.from_attempt} 次嘗試</dd>
                    <dt>停止的服務</dt>
                    <dd>{ids(takeover.stopped_services)}</dd>
                    <dt>關閉的瀏覽器 Session</dt>
                    <dd>{ids(takeover.closed_sessions)}</dd>
                    <dt>沿用的旅程</dt>
                    <dd>{titles(takeover.reused_journeys)}</dd>
                    {takeover.problems.length > 0 && (
                      <>
                        <dt>無法確認已停止</dt>
                        <dd>
                          <ul className="verification-problems" aria-label="無法確認已停止">
                            {takeover.problems.map((problem, index) => (
                              <li key={index}>{problem}</li>
                            ))}
                          </ul>
                        </dd>
                      </>
                    )}
                    {takeover.not_resent.length > 0 && (
                      <>
                        <dt>不重送的外部操作</dt>
                        <dd>
                          <ul className="verification-problems" aria-label="不重送的外部操作">
                            {takeover.not_resent.map((item, index) => (
                              <li key={index}>
                                旅程：{journeyTitle(item.journey)}／步驟：{item.step}（第 {item.attempt} 次嘗試）：
                                <CodeLabel code={item.reason} labels={REASON_LABELS} />
                              </li>
                            ))}
                          </ul>
                        </dd>
                      </>
                    )}
                  </dl>
                </div>
              )}
            </li>
          );
        })}
      </ol>
    </div>
  );
}

/** Artifact 所屬的旅程與步驟（找得到時用標題）。 */
function artifactLocation(artifact: EvidenceArtifact, bundle: EvidenceBundle, profile: VerificationProfileVersion | null): string {
  if (artifact.journey === undefined) return "整次驗證";
  const journey = `旅程：${journeyTitle(profile, artifact.journey)}`;
  if (artifact.step === undefined) return journey;
  return `${journey}／步驟：${stepTitle(bundle, artifact.journey, artifact.step)}`;
}

function ArtifactItem({
  artifact,
  location,
  onLoad,
}: {
  artifact: EvidenceArtifact;
  location: string;
  onLoad: () => Promise<EvidenceArtifactContent>;
}) {
  const [content, setContent] = useState<EvidenceArtifactContent | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const kind = ARTIFACT_KIND_LABELS[artifact.kind];
  const screenshot = artifact.kind === "screenshot";
  const buttonRef = useRef<HTMLButtonElement>(null);
  const linkRef = useRef<HTMLAnchorElement>(null);
  const imageRef = useRef<HTMLImageElement>(null);

  // 按下後按鈕先停用，成功時換成截圖或下載連結：焦點移到新出現的元素；失敗時回到按鈕
  useEffect(() => {
    if (busy) return;
    if (content !== null) (screenshot ? imageRef.current : linkRef.current)?.focus();
    else if (error !== null) buttonRef.current?.focus();
  }, [busy, content, error, screenshot]);

  const load = async () => {
    setBusy(true);
    setError(null);
    try {
      setContent(await onLoad());
    } catch (e) {
      setError(`無法讀取：${describeError(e)}`);
    } finally {
      setBusy(false);
    }
  };

  const dataUrl = content && `data:${content.content_type};base64,${content.content_base64}`;
  return (
    <li aria-label={`${kind}：${artifact.name}`} className="verification-file">
      <div className="row-head">
        <p>
          <span className="badge badge-plain">{kind}</span> <span className="mono">{artifact.name}</span>
        </p>
        {artifact.withheld === undefined && dataUrl === null && (
          <button ref={buttonRef} type="button" className="btn-sm" disabled={busy} onClick={() => void load()}>
            {screenshot ? "開啟截圖" : "準備下載"}
          </button>
        )}
        {artifact.withheld === undefined && dataUrl !== null && !screenshot && (
          <a ref={linkRef} href={dataUrl} download={artifact.name}>
            下載 {artifact.name}
          </a>
        )}
      </div>
      <p className="meta">
        <span>{location}</span>
        {artifact.size_bytes !== undefined && <span className="nowrap">{sizeLabel(artifact.size_bytes)}</span>}
      </p>
      {artifact.withheld !== undefined && (
        <p className="verification-withheld">沒有保存：{WITHHELD_LABELS[artifact.withheld] ?? artifact.withheld}</p>
      )}
      {artifact.withheld === undefined && dataUrl !== null && screenshot && (
        <img ref={imageRef} src={dataUrl} alt={`截圖：${artifact.name}`} tabIndex={-1} />
      )}
      {error && (
        <p className="verification-error" role="alert">
          {error}
        </p>
      )}
    </li>
  );
}

function StepView({ step, journeyTitle }: { step: VerificationStepRecord; journeyTitle: (key: string) => string }) {
  const name = step.title === "" ? step.key : step.title;
  return (
    <li aria-label={`${PHASE_LABELS[step.phase]}：${name}`} className="verification-step">
      <p>
        <span className="badge badge-plain">{PHASE_LABELS[step.phase]}</span> {stepName(step.journey, name, journeyTitle)}：
        <VerdictBadge verdict={step.verdict} /> <ReasonLabel code={step.reason_code} />
      </p>
      <p className="meta">
        <span className="verification-operation">{step.operation}</span>
        <ReasonCodeNote code={step.reason_code} />
        {step.engine_code !== undefined && (
          <span>
            引擎代碼 <code>{step.engine_code}</code>
          </span>
        )}
        {step.attempts > 1 && <span>嘗試 {step.attempts} 次</span>}
      </p>
      {step.assertions.length > 0 && (
        <ul className="verification-assertions" aria-label="斷言結果">
          {step.assertions.map((result, index) => {
            const judge = result.assertion.kind === "judge";
            // 語意判斷沒有做出判定（例如模型限流）時不能說成「不符合」
            const undecided = judge && !result.passed && UNDECIDED_JUDGE_REASONS.has(step.reason_code);
            const outcome = result.passed ? "符合" : undecided ? "無法判定" : "不符合";
            const className = result.passed ? "assertion-passed" : undecided ? "assertion-undecided" : "assertion-failed";
            return (
              <li key={index}>
                {describeAssertion(result.assertion)}：<span className={className}>{outcome}</span>（{judge ? "理由" : "實際"}：
                {result.actual}）
              </li>
            );
          })}
        </ul>
      )}
      {step.output !== "" && <pre className="verification-step-output">{step.output}</pre>}
    </li>
  );
}
