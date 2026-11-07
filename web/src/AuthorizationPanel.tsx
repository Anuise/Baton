import { Fragment, useCallback, useEffect, useId, useState, type ReactNode } from "react";

import { formatTimestamp } from "./time";
import type {
  AuthorizationChanges,
  AuthorizationClient,
  AuthorizationRevision,
  AuthorizationView,
  Capability,
  ConfirmationSummary,
  Grant,
  GrantChange,
  LegacyRun,
  TaskClient,
  Workspace,
} from "./runtimeClient";

/** 授權頁另外需要暫停／取消沿用舊授權的 Run。 */
export type AuthorizationPanelClient = AuthorizationClient & Pick<TaskClient, "cancelRun" | "cancelTask">;

type ViewState = { state: "loading" } | { state: "ready"; view: AuthorizationView } | { state: "failed"; reason: string };

/** 已勾選的能力與它的目標資源、限制（每行一項）；沒有出現在這裡的能力就是沒有授權。 */
type Draft = Partial<Record<Capability, { targets: string; restrictions: string }>>;

interface Props {
  client: AuthorizationPanelClient;
  workspace: Workspace;
  /** 顯示用的時區；省略時使用系統時區（spec D-33），只有測試會指定。 */
  timeZone?: string;
}

const lines = (text: string) =>
  text
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line !== "");

function draftFrom(grants: Grant[]): Draft {
  const draft: Draft = {};
  for (const grant of grants) {
    draft[grant.capability] = { targets: grant.targets.join("\n"), restrictions: grant.restrictions.join("\n") };
  }
  return draft;
}

function reasonOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function sourceLabel(revision: AuthorizationRevision, view: AuthorizationView): string {
  const template = view.templates.find((t) => t.id === revision.source.template_id);
  const name = template?.name ?? revision.source.template_id ?? "";
  switch (revision.source.kind) {
    case "template":
      return `範本「${name}」`;
    case "template_edited":
      return `範本「${name}」（已編輯）`;
    case "custom":
      return "自行組成";
  }
}

/**
 * Workspace 授權（I2）：選取目錄不等於授權。
 * 使用者從建議範本或空白開始編輯，檢視確認摘要（含風險揭露）後確認並保存，授權才生效。
 */
export function AuthorizationPanel({ client, workspace, timeZone }: Props) {
  const [loaded, setLoaded] = useState<ViewState>({ state: "loading" });
  const [draft, setDraft] = useState<Draft>({});
  const [templateId, setTemplateId] = useState<string | null>(null);
  const [summary, setSummary] = useState<ConfirmationSummary | null>(null);
  const [acknowledged, setAcknowledged] = useState(false);
  const [acknowledgedExpansion, setAcknowledgedExpansion] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(async () => {
    try {
      const view = await client.getAuthorization(workspace.id);
      setLoaded({ state: "ready", view });
      setDraft(draftFrom(view.effective?.grants ?? []));
      setTemplateId(view.effective?.source.template_id ?? null);
    } catch (e) {
      setLoaded({ state: "failed", reason: reasonOf(e) });
    }
  }, [client, workspace.id]);

  useEffect(() => {
    setLoaded({ state: "loading" });
    setSummary(null);
    setAcknowledged(false);
    setAcknowledgedExpansion(false);
    setNotice(null);
    setError(null);
    void load();
  }, [load]);

  if (loaded.state === "loading") {
    return (
      <PageFrame>
        <p className="empty-note">載入中…</p>
      </PageFrame>
    );
  }
  if (loaded.state === "failed") {
    return (
      <PageFrame>
        <p className="field-error" role="alert">
          無法取得授權：{loaded.reason}
        </p>
      </PageFrame>
    );
  }
  const view = loaded.view;

  /** 內容一改，先前看過的摘要與風險確認都作廢，必須重新檢視。 */
  const edit = (next: Draft, nextTemplateId: string | null = templateId) => {
    setDraft(next);
    setTemplateId(nextTemplateId);
    setSummary(null);
    setAcknowledged(false);
    setAcknowledgedExpansion(false);
    setNotice(null);
    setError(null);
  };

  const grants = (): Grant[] =>
    view.capabilities.flatMap(({ capability }) => {
      const entry = draft[capability];
      return entry ? [{ capability, targets: lines(entry.targets), restrictions: lines(entry.restrictions) }] : [];
    });

  const toggle = (capability: Capability, enabled: boolean) => {
    const next = { ...draft };
    if (enabled) next[capability] = { targets: "", restrictions: "" };
    else delete next[capability];
    edit(next);
  };

  const change = (capability: Capability, field: "targets" | "restrictions", value: string) =>
    edit({ ...draft, [capability]: { ...draft[capability]!, [field]: value } });

  const review = async () => {
    setBusy(true);
    setError(null);
    try {
      setSummary(await client.summarizeAuthorization(workspace.id, grants()));
      setAcknowledged(false);
      setAcknowledgedExpansion(false);
    } catch (e) {
      setError(reasonOf(e));
    } finally {
      setBusy(false);
    }
  };

  const confirm = async () => {
    setBusy(true);
    setError(null);
    try {
      const revision = await client.confirmAuthorization(workspace.id, {
        templateId,
        grants: grants(),
        acknowledgedRisk: acknowledged,
        acknowledgedExpansion,
        // 檢視摘要時的目前 revision：另一個視窗在那之後確認過，就拒絕而不覆蓋
        expectedRevision: summary?.changes?.base_revision ?? null,
      });
      await load();
      setSummary(null);
      setAcknowledged(false);
      setAcknowledgedExpansion(false);
      setNotice(`已保存為 revision ${revision.revision}`);
    } catch (e) {
      setError(reasonOf(e));
    } finally {
      setBusy(false);
    }
  };

  /** 暫停或取消沿用舊授權的 Run 後只重新載入畫面資料，不覆蓋編輯中的內容。 */
  const actOnLegacyRun = async (action: () => Promise<unknown>) => {
    setBusy(true);
    setError(null);
    try {
      await action();
      setLoaded({ state: "ready", view: await client.getAuthorization(workspace.id) });
    } catch (e) {
      setError(reasonOf(e));
    } finally {
      setBusy(false);
    }
  };

  const effective = view.effective;
  // 修改既有授權而擴大範圍時，另外要承認擴大授權；第一次確認沒有 changes，不需要
  const expands = summary?.changes?.expands ?? false;
  // 能力依風險分組：一般能力在上，高風險能力（範本不會包含）在下
  const groups = [
    { title: "一般能力", risky: false, capabilities: view.capabilities.filter((c) => !c.high_risk) },
    { title: "高風險能力", risky: true, capabilities: view.capabilities.filter((c) => c.high_risk) },
  ].filter((group) => group.capabilities.length > 0);

  // 版面：現況 → 沿用舊授權的 Run（要你處理時才出現）→ 從範本開始 → 能力與範圍 → 確認與保存 → 歷史（收起）
  return (
    <PageFrame>
      {effective ? (
        <div className="callout callout-success">
          <p className="callout-title">目前授權：revision {effective.revision}</p>
          <p className="meta">
            <span>確認於 {formatTimestamp(effective.confirmed_at, timeZone)}</span>
            <span>來源：{sourceLabel(effective, view)}</span>
          </p>
        </div>
      ) : (
        <div className="callout callout-warning">
          <p className="callout-title">尚未確認授權</p>
          <p>選取目錄不等於授權；確認之前，任何需要能力的 Agent 都不會被准入。</p>
        </div>
      )}

      {view.legacy_runs.length > 0 && (
        <LegacyRunsNotice
          runs={view.legacy_runs}
          capabilities={view.capabilities}
          busy={busy}
          timeZone={timeZone}
          onPause={(run) => void actOnLegacyRun(() => client.cancelRun(workspace.id, run.run_id))}
          onCancel={(run) => void actOnLegacyRun(() => client.cancelTask(workspace.id, run.task_id))}
        />
      )}

      <section className="section">
        <div className="section-heading">
          <h3>從範本開始</h3>
          <p>套用範本會以範本的內容取代下方的編輯內容，套用後仍可逐項調整。</p>
        </div>
        <ul className="row-list authorization-templates">
          {view.templates.map((template) => (
            <li key={template.id}>
              <div className="row-head">
                <strong>{template.name}</strong>
                {template.id === templateId && <span className="badge badge-accent">目前依據</span>}
                <button
                  type="button"
                  aria-label={`套用「${template.name}」`}
                  onClick={() => edit(draftFrom(template.grants), template.id)}
                >
                  套用
                </button>
              </div>
              <p className="authorization-template-description">{template.description}</p>
            </li>
          ))}
        </ul>
      </section>

      <section className="section">
        <div className="section-heading">
          <h3>能力與範圍</h3>
          <p>勾選要授予的能力，再寫出目標資源與限制，每行一項。</p>
        </div>
        {groups.map((group) => (
          <fieldset key={group.title} className="authorization-group">
            <legend>{group.title}</legend>
            {group.risky && <p className="section-hint">範本不會包含這些能力；需要時逐項勾選，並寫明目標資源。</p>}
            <div className="row-list">
              {group.capabilities.map(({ capability, label, high_risk }) => {
                const entry = draft[capability];
                return (
                  <div key={capability} className="row authorization-capability">
                    {/* 可存取名稱以能力名稱開頭（「部署 高風險」），高風險只以徽章標示，不整列上色 */}
                    <label className="authorization-capability-toggle">
                      <input
                        type="checkbox"
                        checked={entry !== undefined}
                        onChange={(e) => toggle(capability, e.target.checked)}
                      />
                      <span>{label}</span>
                      {high_risk && <span className="badge badge-danger">高風險</span>}
                    </label>
                    {/* 欄位就在能力下方，畫面上不重複能力名稱；可存取名稱仍帶能力名稱 */}
                    {entry && (
                      <div className="field-row authorization-capability-fields">
                        <label className="field">
                          <span className="field-label">目標資源（每行一項）</span>
                          <textarea
                            aria-label={`${label}：目標資源（每行一項）`}
                            value={entry.targets}
                            onChange={(e) => change(capability, "targets", e.target.value)}
                          />
                        </label>
                        <label className="field">
                          <span className="field-label">限制（每行一項）</span>
                          <textarea
                            aria-label={`${label}：限制（每行一項）`}
                            value={entry.restrictions}
                            onChange={(e) => change(capability, "restrictions", e.target.value)}
                          />
                        </label>
                      </div>
                    )}
                  </div>
                );
              })}
            </div>
          </fieldset>
        ))}
      </section>

      <section className="section">
        <div className="section-heading">
          <h3>確認與保存</h3>
          <p>保存前先檢視確認摘要；內容一改，摘要就要重新檢視。</p>
        </div>
        <div className="form-actions">
          <button type="button" disabled={busy} onClick={() => void review()}>
            檢視確認摘要
          </button>
        </div>

        {summary && (
          <section className="panel authorization-summary" aria-label="確認摘要">
            <h4>確認摘要</h4>
            {summary.items.length === 0 ? (
              <p>沒有授予任何能力。</p>
            ) : (
              <dl className="facts authorization-summary-items">
                {summary.items.map((item) => (
                  <Fragment key={item.capability}>
                    <dt>
                      {item.label}
                      {item.high_risk && <span className="badge badge-danger">高風險</span>}
                    </dt>
                    <dd>
                      <span className="authorization-targets">
                        {item.targets.length > 0 ? item.targets.join("、") : "未指定目標資源"}
                      </span>
                      {item.restrictions.length > 0 && (
                        <span className="authorization-restrictions">限制：{item.restrictions.join("、")}</span>
                      )}
                    </dd>
                  </Fragment>
                ))}
              </dl>
            )}
            {summary.changes && <ChangesView changes={summary.changes} />}
            <div className="authorization-confirm-area">
              {summary.risks.map((risk) => (
                <p key={risk} className="authorization-risk">
                  {risk}
                </p>
              ))}
              {summary.requires_risk_acknowledgement && (
                <label className="authorization-ack">
                  <input type="checkbox" checked={acknowledged} onChange={(e) => setAcknowledged(e.target.checked)} />
                  我了解並接受上述風險
                </label>
              )}
              {expands && (
                <label className="authorization-ack">
                  <input
                    type="checkbox"
                    checked={acknowledgedExpansion}
                    onChange={(e) => setAcknowledgedExpansion(e.target.checked)}
                  />
                  我了解這會擴大 Workspace 授權
                </label>
              )}
              <div className="form-actions">
                <button
                  type="button"
                  className="btn-primary"
                  disabled={busy || (summary.requires_risk_acknowledgement && !acknowledged) || (expands && !acknowledgedExpansion)}
                  onClick={() => void confirm()}
                >
                  確認並保存授權
                </button>
              </div>
            </div>
          </section>
        )}

        {notice && (
          <div className="callout callout-success" role="status">
            <p>{notice}</p>
          </div>
        )}
        {error && (
          <p className="field-error" role="alert">
            {error}
          </p>
        )}
      </section>

      {view.revisions.length > 0 && (
        <section className="section authorization-history" aria-label="授權歷史">
          <details>
            <summary>
              授權歷史 <span className="count">{view.revisions.length}</span>
            </summary>
            <ol className="row-list">
              {[...view.revisions].reverse().map((revision) => (
                <li key={revision.revision}>
                  <div className="row-head">
                    <strong>revision {revision.revision}</strong>
                    {revision.revision === effective?.revision && <span className="badge badge-success">目前生效</span>}
                  </div>
                  <p className="meta">
                    <span>確認於 {formatTimestamp(revision.confirmed_at, timeZone)}</span>
                    <span>來源：{sourceLabel(revision, view)}</span>
                  </p>
                </li>
              ))}
            </ol>
          </details>
        </section>
      )}
    </PageFrame>
  );
}

/** 頁面外框：載入中、讀取失敗與正常內容共用同一個頁面標題（h2 與跑道欄的頁名相同）。 */
function PageFrame({ children }: { children: ReactNode }) {
  return (
    <section className="page authorization-page" aria-label="授權">
      <header className="page-header">
        <h2>授權</h2>
      </header>
      {children}
    </section>
  );
}

/**
 * 撤銷授權前已准入的 Run 沿用原 revision（ADR-0003 R1-a），撤權沒有固定完成時間；
 * 列出這些 Run，讓使用者自行暫停（停止 Run，之後重新派工依新授權准入）或取消整個 Task。
 */
function LegacyRunsNotice({
  runs,
  capabilities,
  busy,
  timeZone,
  onPause,
  onCancel,
}: {
  runs: LegacyRun[];
  capabilities: AuthorizationView["capabilities"];
  busy: boolean;
  timeZone?: string;
  onPause: (run: LegacyRun) => void;
  onCancel: (run: LegacyRun) => void;
}) {
  const headingId = useId();
  const labelOf = (capability: Capability) => capabilities.find((c) => c.capability === capability)?.label ?? capability;
  return (
    <section className="section authorization-legacy" role="status" aria-labelledby={headingId}>
      <div className="section-heading">
        <h3 id={headingId}>後續執行已停用，仍有 {runs.length} 個 Run 沿用舊授權</h3>
        <p>已准入的 Run 撤權沒有固定完成時間，可以自行暫停或取消。</p>
      </div>
      {/* 每列＝Task 標題（2 行）與按鈕組 → 沿用的 revision、被撤銷的能力、准入時間、停止要求 */}
      <ul className="row-list">
        {runs.map((run) => (
          <li key={run.run_id} className="authorization-legacy-run stripe-warning">
            <div className="row-head">
              <strong className="clamp-2" title={run.task_title}>
                {run.task_title}
              </strong>
              <div className="authorization-legacy-actions">
                <button
                  type="button"
                  disabled={busy || run.cancel_requested}
                  title="停止這個 Run，Task 回到可派工；之後重新派工時依目前授權准入"
                  onClick={() => onPause(run)}
                >
                  暫停
                </button>
                <button
                  type="button"
                  className="btn-danger"
                  disabled={busy || run.cancel_requested}
                  title="取消整個 Task"
                  onClick={() => onCancel(run)}
                >
                  取消
                </button>
              </div>
            </div>
            <p className="meta">
              <span>沿用 revision {run.authorization_revision}</span>
              {run.revoked_capabilities.length > 0 && <span>已撤銷：{run.revoked_capabilities.map(labelOf).join("、")}</span>}
              {run.admitted_at !== null && <span>准入於 {formatTimestamp(run.admitted_at, timeZone)}</span>}
              {run.cancel_requested && <span className="dot dot-warning">已要求停止，等待程序確認</span>}
            </p>
          </li>
        ))}
      </ul>
    </section>
  );
}

/** 修改既有授權時，相對於目前 revision 新增／移除的能力、目標與限制，以及影響說明。 */
function ChangesView({ changes }: { changes: AuthorizationChanges }) {
  // 每一種變動各自一段（MASTER：次要資訊以間距分開，不以符號串接）
  const parts = (change: GrantChange) =>
    (
      [
        ["新增目標", change.added_targets],
        ["移除目標", change.removed_targets],
        ["新增限制", change.added_restrictions],
        ["移除限制", change.removed_restrictions],
      ] as const
    )
      .filter(([, items]) => items.length > 0)
      .map(([label, items]) => (
        <span key={label}>
          {label}：{items.join("、")}
        </span>
      ));
  return (
    <section className="authorization-changes" aria-label="授權變更">
      <h5>相對於目前授權（revision {changes.base_revision}）的變更</h5>
      {changes.changes.length === 0 ? (
        <p>內容與目前授權相同。</p>
      ) : (
        <ul className="authorization-change-list">
          {changes.changes.map((change) => (
            <li key={change.capability}>
              <p className="authorization-change-head">
                <strong>{change.label}</strong>
                {change.new_capability && <span className="badge badge-plain">新增能力</span>}
                {change.removed_capability && <span className="badge badge-plain">移除能力</span>}
                {change.expands && <span className="badge badge-warning">擴大授權</span>}
              </p>
              {parts(change).length > 0 && <p className="meta">{parts(change)}</p>}
            </li>
          ))}
        </ul>
      )}
      {changes.impact.length > 0 && (
        <ul className="authorization-impact">
          {changes.impact.map((line) => (
            <li key={line}>{line}</li>
          ))}
        </ul>
      )}
    </section>
  );
}
