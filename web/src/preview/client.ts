/**
 * 開發用預覽的假用戶端：讀取方法回傳 PreviewData，其他方法一律拒絕並在 console 記錄
 * （只在 web/preview.html 使用，不進正式建置）。
 */
import { REPORT_SHA256, runContextView, verificationView } from "../fixtures";
import type { AppClient, KnowledgeDetail } from "../runtimeClient";
import { asSetup, normalData, normalSetupProfiles, type PreviewData } from "./data";
import { longData, longSetup } from "./longData";
import type { PreviewDatasetName } from "./pages";

/** 依資料集與是否呈現首次啟動精靈組出資料。 */
export function previewData(name: PreviewDatasetName, setup: boolean): PreviewData {
  if (name === "long") return setup ? longSetup(longData()) : longData();
  return setup ? asSetup(normalData(), normalSetupProfiles()) : normalData();
}

/** 預覽沒有提供的方法（多半是寫入操作）被呼叫時的錯誤訊息。 */
export const unsupportedMessage = (method: string) => `預覽模式沒有提供 ${method}`;

export function createPreviewClient(data: PreviewData): AppClient {
  const reply = async <T>(value: T): Promise<T> => value;
  const reads: Partial<AppClient> = {
    getHealth: () => reply(data.health),
    getPlatform: () => reply(data.platform),
    listWorkspaces: () => reply({ workspaces: data.workspaces }),
    listModels: () => reply(data.models),
    getAuthorization: () => reply(data.authorization),
    // 確認摘要：依送來的能力組出，第一次確認之外都列出相對於目前 revision 的變動（只為了呈現版面）
    summarizeAuthorization: async (_workspaceId, grants) => {
      const capabilities = data.authorization.capabilities;
      const info = (capability: string) => capabilities.find((item) => item.capability === capability);
      const highRisk = grants.some((grant) => info(grant.capability)?.high_risk);
      const base = data.authorization.effective;
      const added = base ? grants.filter((grant) => !base.grants.some((old) => old.capability === grant.capability)) : [];
      return {
        items: grants.map((grant) => ({
          capability: grant.capability,
          label: info(grant.capability)?.label ?? grant.capability,
          targets: grant.targets,
          restrictions: grant.restrictions,
          high_risk: info(grant.capability)?.high_risk ?? false,
        })),
        risks: highRisk ? [data.authorization.risk_disclosure] : [],
        requires_risk_acknowledgement: highRisk,
        changes: base
          ? {
              base_revision: base.revision,
              expands: added.length > 0,
              impact: added.length > 0 ? ["新增能力會擴大這個 Workspace 的授權，之後派出的 Run 才會使用。"] : [],
              changes: added.map((grant) => ({
                  capability: grant.capability,
                  label: info(grant.capability)?.label ?? grant.capability,
                  high_risk: info(grant.capability)?.high_risk ?? false,
                  new_capability: true,
                  removed_capability: false,
                  added_targets: grant.targets,
                  removed_targets: [],
                  added_restrictions: grant.restrictions,
                  removed_restrictions: [],
                  expands: true,
                })),
            }
          : null,
      };
    },
    getTaskBoard: () => reply(data.board),
    getTask: async (_workspaceId, taskId) => {
      const task = data.board.tasks.find((item) => item.id === taskId) ?? data.board.tasks[0];
      const budget = data.board.budgets.find((item) => item.task_id === task.id) ?? data.board.budgets[0];
      return { task, contract_history: [task.contract], budget };
    },
    runContext: () => reply(runContextView()),
    taskVerification: async (_workspaceId, taskId) => verificationView({ task_id: taskId }),
    listDecisions: () => reply(data.decisions),
    decisionAuthority: () => reply(data.authority),
    pendingDecisions: () => reply(data.pending),
    listArtifacts: () => reply(data.artifacts),
    previewArtifact: async (_workspaceId, artifactId, version) => ({
      artifact_id: artifactId,
      version,
      name: "preview.txt",
      size_bytes: 12,
      sha256: REPORT_SHA256,
      preview: { kind: "text", text: "預覽內容", truncated: false },
    }),
    listKnowledge: () => reply(data.knowledge),
    knowledgeDetail: async (id): Promise<KnowledgeDetail> => {
      const entry = data.knowledge.entries.find((item) => item.id === id) ?? data.knowledge.entries[0];
      return { entry, versions: [{ version: entry.version, content: entry.content, note: null, created_at: entry.created_at }], history: [] };
    },
    listGuards: () => reply(data.guards),
    listRecurrences: () => reply({ recurrences: [] }),
    schedulingOverview: () => reply(data.scheduling),
    listProfiles: () => reply(data.profiles),
    getSetup: () => reply(data.setup),
    recheckSetup: () => reply(data.setup),
    lifecycleStatus: () => reply(data.lifecycle),
    onFullExitRequested: () => () => {},
    getSharedSettingsStatus: () => reply(data.sharedSettings),
    backupStatus: () => reply(data.backups),
    updateStatus: () => reply(data.updates),
    getControllerCli: () => reply(data.controller),
    recoveryStatus: () => reply(data.recovery),
    timeline: () => reply(data.timeline),
    metrics: () => reply(data.metrics),
    syncUnderstanding: () => reply(data.understanding),
    workflows: () => reply(data.workflows),
    reviews: () => reply(data.reviews),
    verificationOverview: () => reply(data.verification),
    explorationOverview: () => reply(data.exploration),
  };
  return new Proxy(reads as AppClient, {
    get(target, property) {
      if (property in target) return target[property as keyof AppClient];
      // 不是方法名稱（例如 Promise 解析時查的 then）時照實回答沒有
      if (typeof property !== "string" || property === "then") return undefined;
      return async () => {
        console.error(`[preview] ${unsupportedMessage(property)}`);
        throw new Error(unsupportedMessage(property));
      };
    },
  });
}
