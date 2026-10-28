import { useCallback, useEffect, useId, useState } from "react";

import { formatTimestamp } from "./time";
import type {
  AuthorizationChanges,
  AuthorizationClient,
  AuthorizationRevision,
  AuthorizationView,
  Capability,
  ConfirmationSummary,
  Grant,
  LegacyRun,
  TaskClient,
  Workspace,
} from "./runtimeClient";

/** 授權設定畫面另外需要暫停／取消沿用舊授權的 Run。 */
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
 * Workspace 授權設定（I2）：選取目錄不等於授權。
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

  if (loaded.state === "loading") return <section aria-label="授權設定">載入中…</section>;
  if (loaded.state === "failed") {
    return (
      <section aria-label="授權設定">
        <p role="alert">無法取得授權設定：{loaded.reason}</p>
      </section>
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
  return (
    <section className="authorization" aria-label="授權設定">
      <header className="more-header">
        <h2>授權設定</h2>
      </header>
      {effective ? (
        <p className="authorization-current">
          目前授權：revision {effective.revision}，確認於 {formatTimestamp(effective.confirmed_at, timeZone)}，來源：
          {sourceLabel(effective, view)}
        </p>
      ) : (
        <p className="authorization-unconfirmed">
          尚未確認授權：選取目錄不等於授權，確認之前任何需要能力的 Agent 都不會被准入。
        </p>
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

      <div className="authorization-templates">
        <h3>建議範本（可編輯）</h3>
        {view.templates.map((template) => (
          <div key={template.id} className="authorization-template">
            <button type="button" onClick={() => edit(draftFrom(template.grants), template.id)}>
              套用「{template.name}」
            </button>
            <span className="authorization-template-description">{template.description}</span>
          </div>
        ))}
      </div>

      <fieldset>
        <legend>能力、目標資源與限制</legend>
        {view.capabilities.map(({ capability, label, high_risk }) => {
          const entry = draft[capability];
          return (
            <div key={capability} className="authorization-capability">
              <label className="authorization-capability-toggle">
                <input type="checkbox" checked={entry !== undefined} onChange={(e) => toggle(capability, e.target.checked)} />
                {label}
                {high_risk && <span className="authorization-high-risk">（高風險，需要明確授權）</span>}
              </label>
              {/* 欄位就在能力下方，畫面上不重複能力名稱；可存取名稱仍帶能力名稱（MASTER 5.5） */}
              {entry && (
                <div className="authorization-capability-fields">
                  <label>
                    目標資源（每行一項）
                    <textarea
                      aria-label={`${label}：目標資源（每行一項）`}
                      value={entry.targets}
                      onChange={(e) => change(capability, "targets", e.target.value)}
                    />
                  </label>
                  <label>
                    限制（每行一項）
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
      </fieldset>

      <div className="form-actions">
        <button type="button" disabled={busy} onClick={() => void review()}>
          檢視確認摘要
        </button>
      </div>

      {summary && (
        <section className="authorization-summary" aria-label="確認摘要">
          <h3>確認摘要</h3>
          {summary.items.length === 0 ? (
            <p>沒有授予任何能力。</p>
          ) : (
            <ul>
              {summary.items.map((item) => (
                <li key={item.capability}>
                  <strong>{item.label}</strong>
                  {item.high_risk && <span className="authorization-high-risk">（高風險）</span>}
                  ：{item.targets.join("、")}
                  {item.restrictions.length > 0 && <>；限制：{item.restrictions.join("、")}</>}
                </li>
              ))}
            </ul>
          )}
          {summary.changes && <ChangesView changes={summary.changes} />}
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
        </section>
      )}

      {notice && <p role="status">{notice}</p>}
      {error && <p role="alert">{error}</p>}

      {view.revisions.length > 0 && (
        <section className="authorization-history" aria-label="授權歷史">
          <h3>授權歷史</h3>
          <ol>
            {[...view.revisions].reverse().map((revision) => (
              <li key={revision.revision}>
                revision {revision.revision}：{formatTimestamp(revision.confirmed_at, timeZone)}，
                {sourceLabel(revision, view)}
              </li>
            ))}
          </ol>
        </section>
      )}
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
    <section className="authorization-legacy-runs" role="status" aria-labelledby={headingId}>
      <h3 id={headingId}>後續執行已停用，仍有 {runs.length} 個 Run 沿用舊授權</h3>
      <p>已准入的 Run 撤權沒有固定完成時間，可以自行暫停或取消。</p>
      {/* 每列＝標題（2 行）與狀態說明｜按鈕組（audit P-AUTH-2） */}
      <ul className="authorization-legacy-list">
        {runs.map((run) => (
          <li key={run.run_id} className="authorization-legacy-run">
            <div className="authorization-legacy-text">
              <strong className="clamp-2" title={run.task_title}>
                {run.task_title}
              </strong>
              <p>
                沿用 revision {run.authorization_revision}
                {run.revoked_capabilities.length > 0 && <>；已撤銷：{run.revoked_capabilities.map(labelOf).join("、")}</>}
                {run.admitted_at !== null && <>；准入於 {formatTimestamp(run.admitted_at, timeZone)}</>}
              </p>
              {run.cancel_requested && <p>已要求停止，等待程序確認</p>}
            </div>
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
          </li>
        ))}
      </ul>
    </section>
  );
}

/** 修改既有授權時，相對於目前 revision 新增／移除的能力、目標與限制，以及影響說明。 */
function ChangesView({ changes }: { changes: AuthorizationChanges }) {
  const listed = (label: string, items: string[]) => items.length > 0 && <>；{label}：{items.join("、")}</>;
  return (
    <section className="authorization-changes" aria-label="授權變更">
      <h4>相對於目前授權（revision {changes.base_revision}）的變更</h4>
      {changes.changes.length === 0 ? (
        <p>內容與目前授權相同。</p>
      ) : (
        <ul>
          {changes.changes.map((change) => (
            <li key={change.capability}>
              <strong>{change.label}</strong>
              {change.new_capability && "（新增能力）"}
              {change.removed_capability && "（移除能力）"}
              {listed("新增目標", change.added_targets)}
              {listed("移除目標", change.removed_targets)}
              {listed("新增限制", change.added_restrictions)}
              {listed("移除限制", change.removed_restrictions)}
              {change.expands && <span className="authorization-high-risk">（擴大授權）</span>}
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
