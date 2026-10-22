import { useEffect, useState, type FormEvent } from "react";

import { formatTimestamp } from "./time";
import {
  errorCode,
  type EvidenceBundle,
  type PublishProfileResponse,
  type RunVerificationRequest,
  type UncoveredItem,
  type VerificationAssertion,
  type VerificationOverview,
  type VerificationPhase,
  type VerificationProfileClient,
  type VerificationProfileVersion,
  type VerificationRequest,
  type VerificationStepRecord,
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
  build: "build 識別",
  doctor: "環境檢查",
  start: "啟動",
  readiness: "就緒條件",
  setup: "資料準備",
  journey: "旅程",
  teardown: "清理",
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
  }
}

function VerdictBadge({ verdict }: { verdict: VerificationVerdict }) {
  return <span className={`verdict verdict-${verdict}`}>{VERDICT_LABELS[verdict]}</span>;
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
      <h2>驗證：{workspace.name}</h2>
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
              <BundleCard key={bundle.id} bundle={bundle} profile={loaded.overview.profile} timeZone={timeZone} />
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
              <dd className="verification-proven">已實際跑過並通過代表旅程（由 Evidence Bundle {overview.proven_by} 證明）</dd>
            ) : (
              <dd className="verification-unproven">{UNPROVEN_NOTICE}</dd>
            )}
          </dl>
          <h4>Feature Map</h4>
          <ul className="verification-features" aria-label="Feature Map">
            {profile.definition.feature_map.map((feature) => (
              <li key={feature.key} aria-label={feature.title}>
                <span className="verification-feature-title">{feature.title}</span>
                <span>驗收準則：{feature.criterion}</span>
                {feature.journeys.length > 0 ? (
                  <span>旅程：{feature.journeys.map((key) => journeyTitle(profile, key)).join("、")}</span>
                ) : (
                  <span className="verification-feature-uncovered">沒有旅程（未覆蓋）</span>
                )}
              </li>
            ))}
          </ul>
          {overview.uncovered.length > 0 ? (
            <UncoveredList items={overview.uncovered} label="未覆蓋範圍" />
          ) : (
            <p>每項功能都有旅程，也沒有宣告未覆蓋的範圍。</p>
          )}
        </>
      )}
    </section>
  );
}

function journeyTitle(profile: VerificationProfileVersion | null, key: string): string {
  return profile?.definition.journeys.find((j) => j.key === key)?.title ?? key;
}

function UncoveredList({ items, label }: { items: UncoveredItem[]; label: string }) {
  return (
    <div className="verification-uncovered">
      <strong>{label}（不在這次自動驗證內）</strong>
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
        <label>
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
      const request = await onRun(task === "" ? { journeys: selected } : { task_id: task, journeys: selected });
      const titles = request.journeys.map((key) => journeyTitle(profile, key)).join("、");
      setQueued(`已排入：要求 ${request.id}（Profile 第 ${request.profile_version} 版，旅程：${titles}）`);
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
          <label>
            Task 識別（選填）
            <input value={taskId} onChange={(e) => setTaskId(e.target.value)} placeholder="不填時在 Workspace 目錄中驗證" />
          </label>
          <fieldset>
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
}: {
  bundle: EvidenceBundle;
  profile: VerificationProfileVersion | null;
  timeZone?: string;
}) {
  const headingId = `bundle-${bundle.id}`;
  const featureTitle = (key: string) => profile?.definition.feature_map.find((f) => f.key === key)?.title ?? key;
  const { cleanup } = bundle;
  return (
    <section className={`verification-bundle bundle-${bundle.verdict}`} aria-labelledby={headingId}>
      <h4 id={headingId}>Evidence Bundle {bundle.id}</h4>
      <p className="verification-bundle-verdict">
        判定：<VerdictBadge verdict={bundle.verdict} /> <code>{bundle.reason_code}</code>
      </p>
      <dl className="verification-summary">
        <dt>build</dt>
        <dd>{bundle.build ?? "無法取得"}</dd>
        <dt>candidate revision</dt>
        <dd>{bundle.candidate_revision ?? "無（不是 Git repo）"}</dd>
        <dt>需求版本</dt>
        <dd>
          {bundle.task_id === null
            ? "沒有綁定 Task"
            : `Task ${bundle.task_id} 契約第 ${bundle.requirement_version ?? "?"} 版`}
        </dd>
        <dt>Profile 版本</dt>
        <dd>第 {bundle.profile_version} 版</dd>
        <dt>受驗目錄</dt>
        <dd>{bundle.subject_dir}</dd>
        <dt>時間</dt>
        <dd>
          {formatTimestamp(bundle.started_at, timeZone)} – {formatTimestamp(bundle.finished_at, timeZone)}
        </dd>
      </dl>
      <ul className="verification-journeys" aria-label="旅程結果">
        {bundle.journeys.map((journey) => (
          <li key={journey.key}>
            {journey.title}
            {journey.representative && "（代表旅程）"}：<VerdictBadge verdict={journey.verdict} /> <code>{journey.reason_code}</code>
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

function StepView({ step, journeyTitle }: { step: VerificationStepRecord; journeyTitle: (key: string) => string }) {
  const name = step.title === "" ? step.key : step.title;
  return (
    <li aria-label={`${PHASE_LABELS[step.phase]}：${name}`} className={`verification-step step-${step.verdict}`}>
      <span>
        {PHASE_LABELS[step.phase]}
        {step.journey !== undefined && `（${journeyTitle(step.journey)}）`}・{name}：<VerdictBadge verdict={step.verdict} />{" "}
        <code>{step.reason_code}</code>
        {step.engine_code !== undefined && <> 引擎代碼 <code>{step.engine_code}</code></>}
        {step.attempts > 1 && `・嘗試 ${step.attempts} 次`}
      </span>
      <span className="verification-step-operation">{step.operation}</span>
      {step.assertions.length > 0 && (
        <ul aria-label="斷言結果">
          {step.assertions.map((result, index) => (
            <li key={index} className={result.passed ? "assertion-passed" : "assertion-failed"}>
              {describeAssertion(result.assertion)}：{result.passed ? "符合" : "不符合"}（實際：{result.actual}）
            </li>
          ))}
        </ul>
      )}
      {step.output !== "" && <pre className="verification-step-output">{step.output}</pre>}
    </li>
  );
}
