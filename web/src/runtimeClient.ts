/** Runtime 健康端點的回應（對應 Rust 的 HealthResponse）。 */
export interface HealthResponse {
  status: string;
  product: string;
  version: string;
  boot_count: number;
  started_at: number;
  pid: number;
}

export interface PlatformIssue {
  code: string;
  message: string;
}

/** 平台狀態端點的回應（對應 Rust 的 PlatformResponse）。 */
export interface PlatformResponse {
  platform: {
    os_family: "windows11" | "windows10" | "windows_server" | "other";
    os_build: string;
    feature_version: string;
    edition: string;
    native_arch: string;
    process_arch: string;
  };
  app_version: string;
  assessment: {
    status: "supported" | "unsupported" | "unknown";
    issues: PlatformIssue[];
    support_ends_on: string | null;
    blocks_dispatch: boolean;
  };
  support_table_source: string;
  support_table_checked_on: string;
}

/** Workspace（對應 Rust 的 Workspace）；時間一律是 UTC 毫秒，顯示時才轉成當地時間。 */
export interface Workspace {
  id: string;
  name: string;
  path: string;
  status: "active" | "archived";
  version: number;
  created_at: number;
  archived_at: number | null;
}

export interface WorkspaceList {
  workspaces: Workspace[];
}

export interface CreateWorkspaceRequest {
  path: string;
  name?: string;
}

export type Capability =
  | "read_files"
  | "write_files"
  | "web_read"
  | "run_build_test"
  | "install_project_dependencies"
  | "admin_install"
  | "global_install"
  | "remote_write"
  | "deploy"
  | "external_message"
  | "payment"
  | "production_data";

/** 一項授權：能力 + 目標資源 + 限制（對應 Rust 的 Grant）。 */
export interface Grant {
  capability: Capability;
  targets: string[];
  restrictions: string[];
}

export interface AuthorizationTemplate {
  id: string;
  name: string;
  description: string;
  grants: Grant[];
}

export interface AuthorizationRevision {
  workspace_id: string;
  revision: number;
  grants: Grant[];
  source: {
    kind: "template" | "template_edited" | "custom";
    template_id: string | null;
    base_revision: number | null;
    confirmed_by: string;
  };
  confirmed_at: number;
}

/** 授權設定畫面需要的資料（對應 Rust 的 AuthorizationView）。 */
export interface AuthorizationView {
  workspace_id: string;
  status: "unconfirmed" | "confirmed";
  effective: AuthorizationRevision | null;
  revisions: AuthorizationRevision[];
  templates: AuthorizationTemplate[];
  capabilities: { capability: Capability; label: string; high_risk: boolean }[];
  risk_disclosure: string;
}

export interface ConfirmationSummary {
  items: { capability: Capability; label: string; targets: string[]; restrictions: string[]; high_risk: boolean }[];
  risks: string[];
  requires_risk_acknowledgement: boolean;
}

export interface ConfirmAuthorizationRequest {
  templateId: string | null;
  grants: Grant[];
  acknowledgedRisk: boolean;
}

export interface RuntimeClient {
  getHealth(): Promise<HealthResponse>;
  getPlatform(): Promise<PlatformResponse>;
}

/** Workspace 管理；與 `RuntimeClient` 分開，讓只關心狀態的畫面不必知道它。 */
export interface WorkspaceClient {
  listWorkspaces(): Promise<WorkspaceList>;
  createWorkspace(request: CreateWorkspaceRequest): Promise<Workspace>;
  archiveWorkspace(id: string): Promise<Workspace>;
}

/** Workspace 授權：讀取、確認前的摘要、確認並保存（產生新的 revision）。 */
export interface AuthorizationClient {
  getAuthorization(workspaceId: string): Promise<AuthorizationView>;
  summarizeAuthorization(workspaceId: string, grants: Grant[]): Promise<ConfirmationSummary>;
  confirmAuthorization(workspaceId: string, request: ConfirmAuthorizationRequest): Promise<AuthorizationRevision>;
}

export interface Goal {
  id: string;
  workspace_id: string;
  title: string;
  version: number;
  created_at: number;
}

/** 契約內的引用；儲存時不檢查是否存在，准入檢查時才判定失效。 */
export interface ContractReference {
  kind: "task_contract" | "goal" | "authorization";
  target: string;
  version: number;
}

/** 六項最小契約的單一欄位；欄位省略或為 null 代表「未提供」。 */
export type ContractField =
  | { status: "filled"; text: string; references?: ContractReference[] }
  | { status: "not_applicable"; reason: string }
  | { status: "unknown"; note?: string | null };

export const CONTRACT_FIELDS = [
  "objective",
  "inputs",
  "scope",
  "deliverables",
  "done_criteria",
  "blocker_handling",
] as const;

export type ContractFieldName = (typeof CONTRACT_FIELDS)[number];

export type ContractInput = { [K in ContractFieldName]?: ContractField | null } & {
  required_capabilities?: Capability[];
};

export type TaskContract = { [K in ContractFieldName]: ContractField | null } & {
  task_id: string;
  version: number;
  required_capabilities: Capability[];
  note: string | null;
  created_at: number;
};

export interface MissingItem {
  code:
    | "field_missing"
    | "field_unknown"
    | "reference_invalid"
    | "authorization_unconfirmed"
    | "capability_not_granted"
    | "workspace_archived";
  field: ContractFieldName | null;
  message: string;
}

export type TaskStatus =
  | "pending"
  | "ready"
  | "running"
  | "waiting_decision"
  | "waiting_resource"
  | "blocked"
  | "verifying"
  | "completed"
  | "cancelled";

export interface TaskView {
  id: string;
  workspace_id: string;
  goal_id: string;
  title: string;
  status: TaskStatus;
  version: number;
  contract_version: number;
  created_at: number;
  updated_at: number;
  contract: TaskContract;
  dependencies: string[];
  unfinished_dependencies: string[];
  /** 只有 pending 的 Task 才會計算缺項；其他狀態一律是空陣列。 */
  missing: MissingItem[];
}

export type RunStatus = "queued" | "starting" | "running" | "succeeded" | "failed" | "interrupted" | "cancelled";

/** 程序身分：PID + 程序建立時間 + 啟動指令雜湊。 */
export interface ProcessRecord {
  pid: number;
  created_at_millis: number;
  command_hash: string;
}

/** Run（對應 Rust 的 Run）；時間一律是 UTC 毫秒。 */
export interface Run {
  id: string;
  workspace_id: string;
  task_id: string;
  status: RunStatus;
  version: number;
  /** 同一個 Task 的 Run 依序遞增，最新的是最大者。 */
  generation: number;
  contract_version: number;
  authorization_revision: number | null;
  command_id: string | null;
  /** 啟動回執；回執寫入前為 null。 */
  process: ProcessRecord | null;
  /** 使用者要求取消的時間；程序確認停止之前 Run 仍是 starting／running。 */
  cancel_requested_at: number | null;
  end_reason: string | null;
  created_at: number;
  updated_at: number;
  started_at: number | null;
  ended_at: number | null;
}

/** execution lease：Task 的寫入所有權。 */
export interface ExecutionLease {
  run_id: string;
  task_id: string;
  generation: number;
  acquired_at: number;
  last_heartbeat_at: number;
  expires_at: number;
  released_at: number | null;
  release_reason: string | null;
}

/** Run 加上它的 lease（Run 的欄位展平在同一層）。 */
export type RunView = Run & { lease: ExecutionLease | null };

export interface TaskBoard {
  workspace_id: string;
  goals: Goal[];
  tasks: TaskView[];
  /** Workspace 全部 Run，依建立順序。 */
  runs: RunView[];
  capabilities: { capability: Capability; label: string; high_risk: boolean }[];
}

export interface TaskDetail {
  task: TaskView;
  /** 由舊到新，含目前版本。 */
  contract_history: TaskContract[];
}

export interface AdmissionReport {
  admissible: boolean;
  missing: MissingItem[];
}

export interface CreateTaskRequest {
  goalId: string;
  title: string;
  contract?: ContractInput;
}

/** Goal、Task、六項最小派工契約與准入（只有桌面憑證能用）。 */
export interface TaskClient {
  getTaskBoard(workspaceId: string): Promise<TaskBoard>;
  createGoal(workspaceId: string, title: string): Promise<Goal>;
  createTask(workspaceId: string, request: CreateTaskRequest): Promise<TaskView>;
  getTask(workspaceId: string, taskId: string): Promise<TaskDetail>;
  updateTaskContract(workspaceId: string, taskId: string, contract: ContractInput, note?: string): Promise<TaskView>;
  addTaskDependency(workspaceId: string, taskId: string, dependsOn: string): Promise<TaskView>;
  checkTaskAdmission(workspaceId: string, taskId: string): Promise<AdmissionReport>;
  markTaskReady(workspaceId: string, taskId: string): Promise<TaskView>;
  cancelTask(workspaceId: string, taskId: string): Promise<TaskView>;
  /** 對 ready 的 Task 派工：回傳 queued 的 Run（程序由 Supervisor 之後才啟動）。 */
  dispatchTask(workspaceId: string, taskId: string): Promise<RunView>;
  /** 要求取消 Run；已啟動的 Run 要等程序確認停止才會變 cancelled。 */
  cancelRun(workspaceId: string, runId: string): Promise<RunView>;
}

export type AppClient = RuntimeClient & WorkspaceClient & AuthorizationClient & TaskClient;

const HEALTH_PATH = "/api/v1/health";
const PLATFORM_PATH = "/api/v1/platform";
const WORKSPACES_PATH = "/api/v1/workspaces";
const authorizationPath = (workspaceId: string) => `${WORKSPACES_PATH}/${encodeURIComponent(workspaceId)}/authorization`;
const workspacePath = (workspaceId: string) => `${WORKSPACES_PATH}/${encodeURIComponent(workspaceId)}`;
const taskPath = (workspaceId: string, taskId: string) => `${workspacePath(workspaceId)}/tasks/${encodeURIComponent(taskId)}`;

/** 直接以 HTTP 查詢（開發時用瀏覽器開啟，由 Vite 代理到 Runtime）。 */
export function createHttpClient(fetchFn: typeof fetch = fetch): AppClient {
  /** 非 2xx 時優先採用 Runtime 回報的說明（`{ error, message }`），沒有才退回狀態碼。 */
  const failure = async (response: Response) => {
    const body = (await response.json().catch(() => null)) as { message?: string } | null;
    return new Error(body?.message ?? `Runtime 回應 ${response.status}`);
  };
  /** Task 相關端點共用：有 body 時以 JSON 送出（`undefined` 欄位不會出現在 JSON 裡）。 */
  const send = async <T>(path: string, request?: { method: string; body?: unknown }): Promise<T> => {
    const response = await (request === undefined
      ? fetchFn(path)
      : request.body === undefined
        ? fetchFn(path, { method: request.method })
        : fetchFn(path, {
            method: request.method,
            headers: { "content-type": "application/json" },
            body: JSON.stringify(request.body),
          }));
    if (!response.ok) throw await failure(response);
    return (await response.json()) as T;
  };
  return {
    async getHealth() {
      const response = await fetchFn(HEALTH_PATH);
      if (!response.ok) throw new Error(`Runtime 回應 ${response.status}`);
      return (await response.json()) as HealthResponse;
    },
    async getPlatform() {
      const response = await fetchFn(PLATFORM_PATH);
      if (!response.ok) throw new Error(`Runtime 回應 ${response.status}`);
      return (await response.json()) as PlatformResponse;
    },
    async listWorkspaces() {
      const response = await fetchFn(WORKSPACES_PATH);
      if (!response.ok) throw await failure(response);
      return (await response.json()) as WorkspaceList;
    },
    async createWorkspace(request) {
      const response = await fetchFn(WORKSPACES_PATH, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(request),
      });
      if (!response.ok) throw await failure(response);
      return (await response.json()) as Workspace;
    },
    async archiveWorkspace(id) {
      const response = await fetchFn(`${WORKSPACES_PATH}/${encodeURIComponent(id)}/archive`, { method: "POST" });
      if (!response.ok) throw await failure(response);
      return (await response.json()) as Workspace;
    },
    async getAuthorization(workspaceId) {
      const response = await fetchFn(authorizationPath(workspaceId));
      if (!response.ok) throw await failure(response);
      return (await response.json()) as AuthorizationView;
    },
    async summarizeAuthorization(workspaceId, grants) {
      const response = await fetchFn(`${authorizationPath(workspaceId)}/summary`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ grants }),
      });
      if (!response.ok) throw await failure(response);
      return (await response.json()) as ConfirmationSummary;
    },
    async confirmAuthorization(workspaceId, { templateId, grants, acknowledgedRisk }) {
      const response = await fetchFn(authorizationPath(workspaceId), {
        method: "PUT",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ template_id: templateId, grants, acknowledged_risk: acknowledgedRisk }),
      });
      if (!response.ok) throw await failure(response);
      return (await response.json()) as AuthorizationRevision;
    },
    getTaskBoard: (workspaceId) => send(`${workspacePath(workspaceId)}/tasks`),
    createGoal: (workspaceId, title) => send(`${workspacePath(workspaceId)}/goals`, { method: "POST", body: { title } }),
    createTask: (workspaceId, { goalId, title, contract }) =>
      send(`${workspacePath(workspaceId)}/tasks`, { method: "POST", body: { goal_id: goalId, title, contract } }),
    getTask: (workspaceId, taskId) => send(taskPath(workspaceId, taskId)),
    updateTaskContract: (workspaceId, taskId, contract, note) =>
      send(`${taskPath(workspaceId, taskId)}/contract`, { method: "PUT", body: { contract, note } }),
    addTaskDependency: (workspaceId, taskId, dependsOn) =>
      send(`${taskPath(workspaceId, taskId)}/dependencies`, { method: "POST", body: { depends_on: dependsOn } }),
    checkTaskAdmission: (workspaceId, taskId) => send(`${taskPath(workspaceId, taskId)}/admission-check`, { method: "POST" }),
    markTaskReady: (workspaceId, taskId) => send(`${taskPath(workspaceId, taskId)}/ready`, { method: "POST" }),
    cancelTask: (workspaceId, taskId) => send(`${taskPath(workspaceId, taskId)}/cancel`, { method: "POST" }),
    dispatchTask: (workspaceId, taskId) => send(`${taskPath(workspaceId, taskId)}/dispatch`, { method: "POST" }),
    cancelRun: (workspaceId, runId) =>
      send(`${workspacePath(workspaceId)}/runs/${encodeURIComponent(runId)}/cancel`, { method: "POST" }),
  };
}

type Invoke = <T>(command: string, args?: Record<string, unknown>) => Promise<T>;

/** 在 Tauri 內透過桌面殼的指令查詢；殼負責連回或啟動 Runtime。 */
export function createTauriClient(invoke: Invoke): AppClient {
  const call = async <T>(command: string, args?: Record<string, unknown>): Promise<T> => {
    try {
      return await (args === undefined ? invoke<T>(command) : invoke<T>(command, args));
    } catch (error) {
      // Tauri 指令的錯誤是純字串
      throw error instanceof Error ? error : new Error(String(error));
    }
  };
  return {
    getHealth: () => call<HealthResponse>("runtime_health"),
    getPlatform: () => call<PlatformResponse>("runtime_platform"),
    listWorkspaces: () => call<WorkspaceList>("workspace_list"),
    createWorkspace: ({ path, name }) => call<Workspace>("workspace_create", { path, name }),
    archiveWorkspace: (id) => call<Workspace>("workspace_archive", { id }),
    getAuthorization: (id) => call<AuthorizationView>("workspace_authorization", { id }),
    summarizeAuthorization: (id, grants) => call<ConfirmationSummary>("workspace_authorization_summary", { id, grants }),
    confirmAuthorization: (id, { templateId, grants, acknowledgedRisk }) =>
      call<AuthorizationRevision>("workspace_authorization_confirm", { id, templateId, grants, acknowledgedRisk }),
    getTaskBoard: (workspaceId) => call<TaskBoard>("task_board", { workspaceId }),
    createGoal: (workspaceId, title) => call<Goal>("goal_create", { workspaceId, title }),
    createTask: (workspaceId, { goalId, title, contract }) =>
      call<TaskView>("task_create", { workspaceId, goalId, title, contract }),
    getTask: (workspaceId, taskId) => call<TaskDetail>("task_detail", { workspaceId, taskId }),
    updateTaskContract: (workspaceId, taskId, contract, note) =>
      call<TaskView>("task_contract_update", { workspaceId, taskId, contract, note }),
    addTaskDependency: (workspaceId, taskId, dependsOn) =>
      call<TaskView>("task_dependency_add", { workspaceId, taskId, dependsOn }),
    checkTaskAdmission: (workspaceId, taskId) => call<AdmissionReport>("task_admission_check", { workspaceId, taskId }),
    markTaskReady: (workspaceId, taskId) => call<TaskView>("task_ready", { workspaceId, taskId }),
    cancelTask: (workspaceId, taskId) => call<TaskView>("task_cancel", { workspaceId, taskId }),
    dispatchTask: (workspaceId, taskId) => call<RunView>("task_dispatch", { workspaceId, taskId }),
    cancelRun: (workspaceId, runId) => call<RunView>("run_cancel", { workspaceId, runId }),
  };
}

/** 依執行環境挑選用戶端。 */
export async function createRuntimeClient(): Promise<AppClient> {
  if ("__TAURI_INTERNALS__" in window) {
    const { invoke } = await import("@tauri-apps/api/core");
    return createTauriClient(invoke);
  }
  return createHttpClient();
}
