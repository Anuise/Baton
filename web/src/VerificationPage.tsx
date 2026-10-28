import { useEffect, useState, type FormEvent } from "react";

import { CopyButton, ShortValue } from "./LongText";
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

interface Props {
  client: VerificationProfileClient;
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
  live_model_disabled: "這次驗證不呼叫模型（no_live_model），無法判定，不是產品缺陷",
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

/** reason_code 與（有的話）中文說明。 */
function ReasonCode({ code }: { code: string }) {
  const label = REASON_LABELS[code];
  return (
    <>
      <code>{code}</code>
      {label !== undefined && <span className="verification-reason">（{label}）</span>}
    </>
  );
}

/** Profile 中 persona／登入身分的顯示名稱：找得到標題時為「標題（key）」。 */
function titledKey(items: { key: string; title: string }[] | undefined, key: string): string {
  const title = items?.find((item) => item.key === key)?.title;
  return title === undefined ? key : `${title}（${key}）`;
}

function VerdictBadge({ verdict }: { verdict: VerificationVerdict }) {
  return <span className={`badge verdict verdict-${verdict}`}>{VERDICT_LABELS[verdict]}</span>;
}

/**
 * 驗證（票 45，spec D-29）：Workspace 的 Verification Profile、Feature Map 與未覆蓋範圍；使用者在這裡發布 Profile 新版本、
 * 要求執行 CLI／API 驗證，並檢視最近的 Evidence Bundle 與每個步驟的判定。
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

  // 重新載入時不切回「載入中」，保留使用者正在編輯的 Profile 內容
  const reload = async () => {
    try {
      setLoaded({ state: "ready", overview: await client.verificationOverview(workspace.id) });
    } catch (e) {
      setLoaded({ state: "failed", reason: reasonOf(e) });
    }
  };

  return (
    <section className="verification-page" aria-label="驗證">
      <h2>驗證</h2>
      {loaded.state === "loading" && <p>載入中…</p>}
      {loaded.state === "failed" && <p role="alert">無法取得驗證資料：{loaded.reason}</p>}
      {loaded.state === "ready" && (
        <>
          <button type="button" className="verification-reload" onClick={() => void reload()}>
            重新整理
          </button>
          <ProfileStatus overview={loaded.overview} timeZone={timeZone} />
          <PublishForm
            profile={loaded.overview.profile}
            onPublish={(definition) => client.publishVerificationProfile(workspace.id, definition)}
            onPublished={reload}
          />
          <RunForm
            overview={loaded.overview}
            onRun={(request) => client.runVerification(workspace.id, request)}
            onQueued={reload}
          />
          <section className="verification-section" aria-labelledby="verification-bundles">
            <h3 id="verification-bundles">最近的 Evidence Bundle</h3>
            {loaded.overview.bundles.length === 0 && <p>還沒有 Evidence Bundle；要求執行驗證後，結果會在這裡顯示。</p>}
            {loaded.overview.bundles.map((bundle) => (
              <BundleCard
                key={bundle.id}
                bundle={bundle}
                profile={loaded.overview.profile}
                timeZone={timeZone}
                onLoadArtifact={(index) => client.evidenceArtifact(workspace.id, bundle.id, index)}
              />
            ))}
          </section>
        </>
      )}
    </section>
  );
}

function ProfileStatus({ overview, timeZone }: { overview: VerificationOverview; timeZone?: string }) {
  const profile = overview.profile;
  return (
    <section className="verification-section" aria-labelledby="verification-profile">
      <h3 id="verification-profile">Verification Profile</h3>
      {profile === null ? (
        <p>還沒有 Verification Profile；在下方填入 Profile 內容並發布後，才能要求執行驗證。</p>
      ) : (
        <>
          <dl className="verification-summary">
            <dt>版本</dt>
            <dd>第 {profile.version} 版</dd>
            <dt>發布時間</dt>
            <dd>{formatTimestamp(profile.created_at, timeZone)}</dd>
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
          </dl>
          <h4>Feature Map</h4>
          <ul className="verification-feature-map" aria-label="Feature Map">
            {profile.definition.feature_map.map((feature) => (
              <li key={feature.key} aria-label={feature.title}>
                <h5>{feature.title}</h5>
                <p>
                  <span className="verification-label">驗收準則：</span>
                  {feature.criterion}
                </p>
                {feature.journeys.length > 0 ? (
                  <p>
                    <span className="verification-label">旅程：</span>
                    {feature.journeys.map((key) => journeyTitle(profile, key)).join("、")}
                  </p>
                ) : (
                  <p className="verification-feature-uncovered">
                    沒有旅程（未覆蓋）
                  </p>
                )}
              </li>
            ))}
          </ul>
          {overview.uncovered.length > 0 ? (
            <UncoveredList items={overview.uncovered} label="未覆蓋範圍" />
          ) : (
            <p>每項功能都有旅程，也沒有宣告未覆蓋的範圍。</p>
          )}
          {profile.definition.interface === "web" && <WebProfileDetails definition={profile.definition} />}
        </>
      )}
    </section>
  );
}

/** web 介面的秘密參照、persona 與登入身分；秘密只顯示 key 與參照，前端拿不到值。 */
function WebProfileDetails({ definition }: { definition: VerificationProfileVersion["definition"] }) {
  const secrets = definition.secrets ?? [];
  const personas = definition.personas ?? [];
  const identities = definition.identities ?? [];
  return (
    <>
      <h4>秘密參照</h4>
      {secrets.length === 0 ? (
        <p>沒有秘密參照。</p>
      ) : (
        <>
          <p>只記錄參照位置，秘密的值不會顯示，也不會寫入證據。</p>
          <ul className="verification-features" aria-label="秘密參照">
            {secrets.map((secret) => (
              <li key={secret.key}>
                <code>{secret.key}</code>：<code>{secret.reference}</code>
              </li>
            ))}
          </ul>
        </>
      )}
      <h4>Persona</h4>
      {personas.length === 0 ? (
        <p>沒有 persona。</p>
      ) : (
        <ul className="verification-features" aria-label="Persona">
          {personas.map((persona) => (
            <li key={persona.key}>
              {persona.title}（{persona.key}）{persona.description !== "" && `：${persona.description}`}
            </li>
          ))}
        </ul>
      )}
      <h4>登入身分</h4>
      {identities.length === 0 ? (
        <p>沒有登入身分；旅程都不登入。</p>
      ) : (
        <ul className="verification-features" aria-label="登入身分">
          {identities.map((identity) => (
            <li key={identity.key}>
              {identity.title}（{identity.key}）：登入 {identity.login.length} 個步驟，以「
              {identity.check.title === "" ? identity.check.key : identity.check.title}」核對身分
            </li>
          ))}
        </ul>
      )}
    </>
  );
}

function journeyTitle(profile: VerificationProfileVersion | null, key: string): string {
  return profile?.definition.journeys.find((j) => j.key === key)?.title ?? key;
}

function UncoveredList({ items, label }: { items: UncoveredItem[]; label: string }) {
  return (
    <div className="verification-uncovered stripe-warning">
      <h5>{label}（不在這次自動驗證內）</h5>
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
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<string | null>(null);

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
    <section className="verification-section" aria-labelledby="verification-publish">
      <h3 id="verification-publish">發布新版本</h3>
      <p>Profile 決定怎麼驗收，只有你能發布；舊版本會保留，已有的證據仍綁定原本的版本。</p>
      <form className="verification-publish" onSubmit={(event) => void submit(event)}>
        <label className="field">
          Profile 內容（JSON）
          <textarea value={draft} onChange={(e) => setDraft(e.target.value)} rows={16} spellCheck={false} />
        </label>
        <button type="submit" disabled={busy}>
          發布新版本
        </button>
      </form>
      {error && <p role="alert">{error}</p>}
      {result && <p role="status">{result}</p>}
    </section>
  );
}

function RunForm({
  overview,
  onRun,
  onQueued,
}: {
  overview: VerificationOverview;
  onRun: (request: RunVerificationRequest) => Promise<VerificationRequest>;
  onQueued: () => Promise<void>;
}) {
  const [taskId, setTaskId] = useState("");
  const [selected, setSelected] = useState<string[]>([]);
  const [noLiveModel, setNoLiveModel] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [queued, setQueued] = useState<string | null>(null);
  const profile = overview.profile;

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
      const request = await onRun(body);
      const titles = request.journeys.map((key) => journeyTitle(profile, key)).join("、");
      const mode = request.no_live_model ? "；不呼叫模型" : "";
      setQueued(`已排入：要求 ${request.id}（Profile 第 ${request.profile_version} 版，旅程：${titles}${mode}）`);
      await onQueued();
    } catch (e) {
      setError(`無法要求執行：${describeError(e)}`);
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="verification-section" aria-labelledby="verification-run">
      <h3 id="verification-run">要求執行驗證</h3>
      {profile === null ? (
        <p>發布 Profile 後才能要求執行驗證。</p>
      ) : (
        <form className="verification-run" onSubmit={(event) => void submit(event)}>
          <label className="field">
            Task 識別（選填）
            <input
              className="select-fit"
              value={taskId}
              onChange={(e) => setTaskId(e.target.value)}
              placeholder="不填時在 Workspace 目錄中驗證"
            />
          </label>
          <fieldset>
            <legend>旅程（都不勾選時執行全部）</legend>
            {profile.definition.journeys.map((journey) => (
              <label key={journey.key} className="verification-check">
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
            <label className="verification-check">
              <input type="checkbox" checked={noLiveModel} onChange={(e) => setNoLiveModel(e.target.checked)} />
              不呼叫模型（no_live_model）
            </label>
            <p className="field-hint">只執行不需模型的斷言與已驗證動作的重播，需要模型的步驟記為證據不足。</p>
          </div>
          {overview.proven_by === null && <p>這個版本還沒被證明可用，代表旅程一定會一起執行。</p>}
          <button type="submit" disabled={busy}>
            要求執行
          </button>
        </form>
      )}
      {error && <p role="alert">{error}</p>}
      {queued && <p role="status">{queued}</p>}
    </section>
  );
}

function BundleCard({
  bundle,
  profile,
  timeZone,
  onLoadArtifact,
}: {
  bundle: EvidenceBundle;
  profile: VerificationProfileVersion | null;
  timeZone?: string;
  onLoadArtifact: (index: number) => Promise<EvidenceArtifactContent>;
}) {
  const headingId = `bundle-${bundle.id}`;
  const featureTitle = (key: string) => profile?.definition.feature_map.find((f) => f.key === key)?.title ?? key;
  const { cleanup } = bundle;
  // 舊的 Bundle 沒有 attempts；只有一次嘗試且沒有承接時不顯示交接區塊
  const attempts = bundle.attempts ?? [];
  const handedOver = attempts.length > 1 || attempts.some((attempt) => attempt.takeover !== null);
  return (
    <section className={`verification-bundle bundle-${bundle.verdict}`} aria-labelledby={headingId}>
      <header className="verification-bundle-header">
        {/* 標題顯示縮短的識別碼；可存取名稱保留完整識別碼 */}
        <h4 id={headingId} aria-label={`Evidence Bundle ${bundle.id}`}>
          Evidence Bundle{" "}
          <code className="mono" title={bundle.id}>
            {shortId(bundle.id)}
          </code>
        </h4>
        {shortId(bundle.id) !== bundle.id && <CopyButton text={bundle.id} label="複製識別碼" />}
      </header>
      <p className="verification-bundle-verdict">
        判定：<VerdictBadge verdict={bundle.verdict} /> <ReasonCode code={bundle.reason_code} />
      </p>
      <dl className="verification-summary">
        <dt>建置識別</dt>
        <dd>{bundle.build ?? "無法取得"}</dd>
        <dt>候選版本</dt>
        <dd>
          {bundle.candidate_revision === null ? "無（不是 Git repo）" : <ShortValue value={bundle.candidate_revision} kind="hash" />}
        </dd>
        <dt>需求版本</dt>
        <dd>
          {bundle.task_id === null ? (
            "沒有綁定 Task"
          ) : (
            <>
              Task <ShortValue value={bundle.task_id} kind="id" /> 契約第 {bundle.requirement_version ?? "?"} 版
            </>
          )}
        </dd>
        <dt>Profile 版本</dt>
        <dd>第 {bundle.profile_version} 版</dd>
        <dt>受驗目錄</dt>
        <dd>
          <ShortValue value={bundle.subject_dir} kind="path" />
        </dd>
        <dt>時間</dt>
        <dd>
          {formatTimestamp(bundle.started_at, timeZone)} – {formatTimestamp(bundle.finished_at, timeZone)}
        </dd>
      </dl>
      <ul className="verification-journeys" aria-label="旅程結果">
        {bundle.journeys.map((journey) => (
          <li key={journey.key} aria-label={journey.title}>
            {journey.title}
            {journey.representative && "（代表旅程）"}：<VerdictBadge verdict={journey.verdict} />{" "}
            <ReasonCode code={journey.reason_code} />
            {(journey.persona !== undefined || journey.identity !== undefined) && (
              <>
                {" "}
                <span className="verification-journey-actor">
                  persona：
                  {journey.persona === undefined ? "未指定" : titledKey(profile?.definition.personas, journey.persona)}
                </span>{" "}
                <span className="verification-journey-actor">
                  登入身分：
                  {journey.identity === undefined ? "未登入" : titledKey(profile?.definition.identities, journey.identity)}
                </span>
              </>
            )}
          </li>
        ))}
      </ul>
      <p>
        覆蓋的功能：{bundle.covered_features.length === 0 ? "無" : bundle.covered_features.map(featureTitle).join("、")}
      </p>
      {bundle.uncovered.length > 0 && <UncoveredList items={bundle.uncovered} label="這次未覆蓋" />}
      <div className="verification-cleanup" aria-label="清理結果">
        清理：{cleanup.state_dir_removed ? "已移除本次驗證的狀態目錄" : "本次驗證的狀態目錄沒有移除"}
        {cleanup.service_stopped !== undefined && (cleanup.service_stopped ? "；服務已停止" : "；服務沒有停止")}
        ；證據已保留
        {cleanup.problems.length > 0 && (
          <ul aria-label="清理問題">
            {cleanup.problems.map((problem, index) => (
              <li key={index}>{problem}</li>
            ))}
          </ul>
        )}
      </div>
      <UsageView bundle={bundle} />
      {bundle.actions !== undefined && bundle.actions.length > 0 && (
        <ActionsView actions={bundle.actions} bundle={bundle} profile={profile} />
      )}
      {handedOver && <AttemptsView attempts={attempts} journeyTitle={(key) => journeyTitle(profile, key)} />}
      {bundle.artifacts.length > 0 && (
        <div className="verification-artifacts">
          <h5>證據檔案</h5>
          <ul aria-label="證據檔案">
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
        <summary>步驟（{bundle.steps.length}）</summary>
        <ol aria-label={`${bundle.id} 的步驟`}>
          {bundle.steps.map((step, index) => (
            <StepView key={index} step={step} journeyTitle={(key) => journeyTitle(profile, key)} />
          ))}
        </ol>
      </details>
    </section>
  );
}

/** 舊的 Bundle 沒有用量時視為 0。 */
const NO_USAGE: VerificationUsage = { live_model_calls: 0, live_actions: 0, replayed_actions: 0, blocked_model_calls: 0 };

/** 用量（票 48）：即時模型與重播分開呈現，重播不呼叫模型。 */
function UsageView({ bundle }: { bundle: EvidenceBundle }) {
  const usage = { ...NO_USAGE, ...bundle.usage };
  return (
    <div className="verification-usage">
      <h5>用量</h5>
      {bundle.no_live_model && <p className="verification-no-live-model">這次驗證禁止即時模型（no_live_model）</p>}
      <ul aria-label="用量">
        <li aria-label="即時">
          即時：模型呼叫 {usage.live_model_calls} 次、模型決定的動作 {usage.live_actions} 個
        </li>
        <li aria-label="重播">重播：重播的動作 {usage.replayed_actions} 個（不呼叫模型）</li>
        <li aria-label="擋下的模型呼叫">no_live_model 擋下的模型呼叫 {usage.blocked_model_calls} 次</li>
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
    <div className="verification-actions">
      <h5>目標導向動作</h5>
      <p>重播是重新執行真實動作並檢查後置條件；快取命中不是通過，判定看步驟結果。</p>
      <table aria-label="目標導向動作">
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
  );
}

/** 步驟的標題（找不到或沒有標題時用 key）。 */
function stepTitle(bundle: EvidenceBundle, journey: string, key: string): string {
  const step = bundle.steps.find((s) => s.journey === journey && s.key === key);
  return step === undefined || step.title === "" ? key : step.title;
}

/** 嘗試與交接（票 49）：每次嘗試的狀態、judge 帳號、採用的旅程與步驟，以及承接時停止的程序與不重送的外部操作。 */
function AttemptsView({ attempts, journeyTitle }: { attempts: VerificationAttempt[]; journeyTitle: (key: string) => string }) {
  const titles = (keys: string[]) => (keys.length === 0 ? "無" : keys.map(journeyTitle).join("、"));
  const ids = (values: string[]) => (values.length === 0 ? "無" : values.join("、"));
  return (
    <div className="verification-attempts" role="group" aria-label="嘗試與交接">
      <h5>嘗試與交接</h5>
      <ol>
        {attempts.map((attempt) => {
          const { takeover } = attempt;
          return (
            <li key={attempt.attempt} aria-label={`第 ${attempt.attempt} 次嘗試`}>
              <span>
                第 {attempt.attempt} 次嘗試：{ATTEMPT_STATUS_LABELS[attempt.status]}
                {attempt.end_reason !== null && (
                  <>
                    ・結束原因：<code>{attempt.end_reason}</code>
                    {END_REASON_LABELS[attempt.end_reason] !== undefined && `（${END_REASON_LABELS[attempt.end_reason]}）`}
                  </>
                )}
              </span>
              <span>語意判斷帳號：{attempt.judge_profile_id ?? "沒有可用帳號（只執行不需模型的斷言）"}</span>
              <span>結果採用自這次的旅程：{titles(attempt.journeys)}</span>
              <span>完成的步驟：{attempt.steps.length} 個</span>
              {attempt.steps.length > 0 && (
                <ul aria-label={`第 ${attempt.attempt} 次嘗試的步驟`}>
                  {attempt.steps.map((step, index) => (
                    <li key={index}>
                      {PHASE_LABELS[step.phase]}
                      {step.journey !== undefined && `（${journeyTitle(step.journey)}）`}・{step.key}：
                      <VerdictBadge verdict={step.verdict} /> <ReasonCode code={step.reason_code} />
                    </li>
                  ))}
                </ul>
              )}
              {takeover !== null && (
                <div className="verification-takeover" role="group" aria-label={`第 ${attempt.attempt} 次嘗試的承接`}>
                  <span>承接第 {takeover.from_attempt} 次嘗試</span>
                  <span>停止的服務：{ids(takeover.stopped_services)}</span>
                  <span>關閉的瀏覽器 Session：{ids(takeover.closed_sessions)}</span>
                  <span>沿用的旅程：{titles(takeover.reused_journeys)}</span>
                  {takeover.problems.length > 0 && (
                    <ul aria-label="無法確認已停止">
                      {takeover.problems.map((problem, index) => (
                        <li key={index}>{problem}</li>
                      ))}
                    </ul>
                  )}
                  {takeover.not_resent.length > 0 && (
                    <ul aria-label="不重送的外部操作">
                      {takeover.not_resent.map((item, index) => (
                        <li key={index}>
                          旅程：{journeyTitle(item.journey)}／步驟：{item.step}（第 {item.attempt} 次嘗試）：
                          <ReasonCode code={item.reason} />
                        </li>
                      ))}
                    </ul>
                  )}
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
    <li aria-label={`${kind}：${artifact.name}`} className="verification-artifact">
      <span>
        {kind}・{artifact.name}・{location}
        {artifact.size_bytes !== undefined && `・${sizeLabel(artifact.size_bytes)}`}
      </span>
      {artifact.withheld !== undefined ? (
        <span className="verification-artifact-withheld">
          沒有保存：{WITHHELD_LABELS[artifact.withheld] ?? artifact.withheld}
        </span>
      ) : dataUrl === null ? (
        <button type="button" disabled={busy} onClick={() => void load()}>
          {screenshot ? "開啟截圖" : "準備下載"}
        </button>
      ) : screenshot ? (
        <img src={dataUrl} alt={`截圖：${artifact.name}`} />
      ) : (
        <a href={dataUrl} download={artifact.name}>
          下載 {artifact.name}
        </a>
      )}
      {error && <span role="alert">{error}</span>}
    </li>
  );
}

function StepView({ step, journeyTitle }: { step: VerificationStepRecord; journeyTitle: (key: string) => string }) {
  const name = step.title === "" ? step.key : step.title;
  return (
    <li aria-label={`${PHASE_LABELS[step.phase]}：${name}`} className={`verification-step step-${step.verdict}`}>
      <span>
        {PHASE_LABELS[step.phase]}
        {step.journey !== undefined && `（${journeyTitle(step.journey)}）`}・{name}：<VerdictBadge verdict={step.verdict} />{" "}
        <ReasonCode code={step.reason_code} />
        {step.engine_code !== undefined && <> 引擎代碼 <code>{step.engine_code}</code></>}
        {step.attempts > 1 && `・嘗試 ${step.attempts} 次`}
      </span>
      <span className="verification-step-operation">{step.operation}</span>
      {step.assertions.length > 0 && (
        <ul aria-label="斷言結果">
          {step.assertions.map((result, index) => {
            const judge = result.assertion.kind === "judge";
            // 語意判斷沒有做出判定（例如模型限流）時不能說成「不符合」
            const undecided = judge && !result.passed && UNDECIDED_JUDGE_REASONS.has(step.reason_code);
            const outcome = result.passed ? "符合" : undecided ? "無法判定" : "不符合";
            const className = result.passed ? "assertion-passed" : undecided ? "assertion-undecided" : "assertion-failed";
            return (
              <li key={index} className={className}>
                {describeAssertion(result.assertion)}：{outcome}（{judge ? "理由" : "實際"}：{result.actual}）
              </li>
            );
          })}
        </ul>
      )}
      {step.output !== "" && <pre className="verification-step-output">{step.output}</pre>}
    </li>
  );
}
