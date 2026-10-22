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

/** Claude Code 實際使用的認證來源（對應 Rust 的 AuthSource）；只有 `native_login` 可以派工。 */
export type AuthSource = "native_login" | "api_key" | "oauth_token" | "third_party_provider" | "other" | "none";

/** 最近一次核對的結果（對應 Rust 的 Verification）。 */
export interface ProfileVerification {
  checked_at: number;
  logged_in: boolean;
  source: AuthSource;
  source_detail: string | null;
  account: string | null;
  config_dir_matches: boolean;
}

export interface ProfileBlocker {
  code: string;
  message: string;
}

/** 帳號 Profile 與目前的可派工判定（對應 Rust 的 ProfileView）；不含任何登入資料。 */
export interface AccountProfile {
  id: string;
  name: string;
  config_dir: string;
  expected_account: string | null;
  status: "active" | "archived";
  verification: ProfileVerification | null;
  version: number;
  created_at: number;
  archived_at: number | null;
  dispatchable: boolean;
  blockers: ProfileBlocker[];
}

export interface ProfileList {
  profiles: AccountProfile[];
  limit: number;
}

export interface CreateProfileRequest {
  name: string;
  expectedAccount?: string;
}

export interface GuidanceCommand {
  title: string;
  command: string;
}

/** 單一工具的檢查結果（對應 Rust 的 ToolCheck）；`guidance` 只供顯示，App 不會執行。 */
export interface ToolCheck {
  status: "ok" | "missing" | "too_old" | "unknown";
  version: string | null;
  minimum_version: string | null;
  message: string;
  guidance: GuidanceCommand[];
  docs_url: string;
}

/** 首次啟動精靈的狀態（對應 Rust 的 SetupStatus）。 */
export interface SetupStatus {
  claude_code: ToolCheck;
  git: ToolCheck;
  profiles: AccountProfile[];
  profile_limit: number;
  background_notice: { text: string; acknowledged_at: number | null };
  ready: boolean;
}

/** 版本的檔案現況：`missing`／`size_mismatch` 代表磁碟上的檔案與登記不符，不可信。 */
export type ArtifactFileStatus = "ok" | "missing" | "size_mismatch";

/** 成果的單一版本（對應 Rust 的 ArtifactVersionView）；時間是 UTC 毫秒。 */
export interface ArtifactVersionView {
  version: number;
  description: string;
  size_bytes: number;
  /** 64 位小寫十六進位。 */
  sha256: string;
  storage_path: string;
  produced_by_run_id: string;
  produced_by_generation: number;
  created_at: number;
  file_status: ArtifactFileStatus;
}

/** 成果與它的全部版本（由舊到新）。 */
export interface ArtifactView {
  id: string;
  workspace_id: string;
  task_id: string;
  name: string;
  created_at: number;
  versions: ArtifactVersionView[];
}

/** 寫入中途中斷留下、沒有 manifest 的檔案：不是成果。`staging` 內容不完整；`unregistered` 完整但沒登記。 */
export interface UnregisteredFile {
  storage_path: string;
  size_bytes: number;
  kind: "staging" | "unregistered";
}

export interface ArtifactList {
  artifacts: ArtifactView[];
  unregistered_files: UnregisteredFile[];
}

export interface ArtifactPreview {
  artifact_id: string;
  version: number;
  name: string;
  size_bytes: number;
  sha256: string;
  preview: { kind: "text"; text: string; truncated: boolean } | { kind: "binary" };
}

/** 下載到的內容與 Runtime 回報的雜湊（`X-Baton-Sha256`）。 */
export interface ArtifactDownload {
  blob: Blob;
  sha256: string;
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
  /** 這個 Run 使用的帳號 Profile（Scheduler 派工時選定）。 */
  profile_id: string;
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

/** Worker 回報的最新進度；時間是 UTC 毫秒。 */
export interface ProgressNote {
  summary: string;
  reported_at: number;
}

/** Worker 儲存的最新 checkpoint；details 是可展開查看的細節。 */
export interface CheckpointNote {
  summary: string;
  details: string;
  saved_at: number;
}

/** Worker 回報的受阻原因。 */
export interface BlockedNote {
  reason: string;
  reported_at: number;
}

/** Run 加上它的 lease 與 Worker 最新的回報（Run 的欄位展平在同一層；沒有回報時為 null）。 */
export type RunView = Run & {
  lease: ExecutionLease | null;
  latest_progress: ProgressNote | null;
  latest_checkpoint: CheckpointNote | null;
  blocked: BlockedNote | null;
};

/** 不能派工（留在佇列）的原因類別；各自對應不同的處理方式。`lifecycle` 是完全退出中或等待「繼續工作」。 */
export type WaitCategory =
  | "capacity"
  | "quota"
  | "authorization"
  | "dependency"
  | "state_unknown"
  | "account"
  | "lifecycle";

/** 等待原因（對應 Rust 的 WaitReason）；`code` 是穩定的機器可讀代碼。 */
export interface WaitReason {
  category: WaitCategory;
  code: string;
  message: string;
  profile_id?: string;
}

/** 派工回 202：沒有名額或條件不足，Task 留在佇列（對應 Rust 的 QueuedDispatch）。 */
export interface QueuedDispatch {
  queued: true;
  task_id: string;
  priority: number;
  reasons: WaitReason[];
}

/** 派工的結果：已建立 Run，或排隊中。 */
export type DispatchResult =
  | { kind: "started"; run: RunView }
  | { kind: "queued"; task_id: string; priority: number; reasons: WaitReason[] };

/** Runtime 的派工回應以 `queued === true` 區分兩種結果。 */
function toDispatchResult(body: RunView | QueuedDispatch): DispatchResult {
  if ("queued" in body && body.queued === true) {
    return { kind: "queued", task_id: body.task_id, priority: body.priority, reasons: body.reasons };
  }
  return { kind: "started", run: body as RunView };
}

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
  /** 對 ready 的 Task 派工：有名額回傳 queued 的 Run（程序由 Supervisor 之後才啟動），否則回傳排隊中與原因。 */
  dispatchTask(workspaceId: string, taskId: string): Promise<DispatchResult>;
  /** 要求取消 Run；已啟動的 Run 要等程序確認停止才會變 cancelled。 */
  cancelRun(workspaceId: string, runId: string): Promise<RunView>;
}

/** 成果瀏覽（只有桌面憑證能用）：下載與其他端點走同一套帶憑證的機制。 */
export interface ArtifactClient {
  listArtifacts(workspaceId: string): Promise<ArtifactList>;
  previewArtifact(workspaceId: string, artifactId: string, version: number): Promise<ArtifactPreview>;
  downloadArtifact(workspaceId: string, artifactId: string, version: number): Promise<ArtifactDownload>;
}

/** 並行容量的預設：保留交接容量（預設）或較高並行。 */
export type SchedulingMode = "reserve_handoff" | "higher_concurrency";

export interface CapacityModeInfo {
  mode: SchedulingMode;
  label: string;
  description: string;
  per_account_limit: number;
  global_limit: number;
  handoff_reserve: number;
}

/** 目前生效的容量數字與使用情形（對應 Rust 的 CapacityView）。 */
export interface CapacityView {
  per_account_limit: number;
  global_limit: number;
  /** 實際保留的交接槽數；可用帳號不夠時為 0。 */
  handoff_reserve: number;
  /** 一般派工最多能佔用的全域名額（全域上限扣掉保留的交接槽）。 */
  normal_limit: number;
  running: number;
  usable_accounts: number;
}

/** 帳號上一個進行中的 Run。 */
export interface AccountRun {
  run_id: string;
  workspace_id: string;
  task_id: string;
  task_title: string;
  status: RunStatus;
  generation: number;
  /** 已要求取消，但程序還沒確認停止。 */
  cancel_requested: boolean;
}

export interface AccountScheduling {
  profile_id: string;
  name: string;
  dispatchable: boolean;
  blockers: ProfileBlocker[];
  /** 已知的用量比例（0 到 1）；`null` 是尚未觀測，不是 0。 */
  quota_used: number | null;
  running: number;
  limit: number;
  runs: AccountRun[];
}

/** 佇列中等待名額的 Task；`position` 從 1 起。 */
export interface QueueEntry {
  workspace_id: string;
  task_id: string;
  title: string;
  priority: number;
  requested_at: number;
  position: number;
  reasons: WaitReason[];
}

/** 排程總覽（對應 Rust 的 SchedulingOverview）。 */
export interface SchedulingOverview {
  settings: { mode: SchedulingMode; mode_label: string; modes: CapacityModeInfo[] };
  capacity: CapacityView;
  accounts: AccountScheduling[];
  queue: QueueEntry[];
}

/** 排程：總覽（容量、各帳號進行中的 Run、佇列）與容量預設切換。 */
export interface SchedulingClient {
  schedulingOverview(): Promise<SchedulingOverview>;
  setSchedulingMode(mode: SchedulingMode): Promise<SchedulingOverview>;
}

/** 帳號 Profile 與首次啟動精靈；與其他用戶端分開，讓只關心帳號的畫面不必知道它們。 */
export interface AccountClient {
  listProfiles(): Promise<ProfileList>;
  createProfile(request: CreateProfileRequest): Promise<AccountProfile>;
  verifyProfile(id: string): Promise<AccountProfile>;
  /** 開啟終端機，以該 Profile 自己的設定目錄執行原生登入。 */
  openLoginTerminal(id: string): Promise<{ opened: boolean }>;
  archiveProfile(id: string): Promise<AccountProfile>;
  getSetup(): Promise<SetupStatus>;
  /** 重新探測工具並重新核對每個使用中的 Profile。 */
  recheckSetup(): Promise<SetupStatus>;
  acknowledgeBackgroundNotice(): Promise<SetupStatus>;
}

export type LifecyclePhase = "running" | "stopping" | "stopped";

/** 完全退出時單一 Run 的停止結果；`anomaly` 是無法確認已停止的程序（停止異常）。 */
export type ExitRunOutcome = "pending" | "not_started" | "stopped_gracefully" | "force_killed" | "anomaly";

export type StopAnomaly = "still_alive" | "stop_failed" | "stop_unconfirmed";

export interface ExitRunResult {
  run_id: string;
  workspace_id: string;
  task_id: string;
  outcome: ExitRunOutcome;
  /** 只有 outcome 為 anomaly 時才有。 */
  anomaly?: StopAnomaly;
  message?: string;
}

/** 最近一次完全退出的經過；時間是 UTC 毫秒。 */
export interface ExitReport {
  requested_at: number;
  deadline_at: number;
  completed_at: number | null;
  runs: ExitRunResult[];
  notes: string[];
}

/** 單一 Workspace 的生命週期設定與狀態（對應 Rust 的 WorkspaceLifecycleView）。 */
export interface WorkspaceLifecycleView {
  workspace_id: string;
  name: string;
  /** 完全退出後重新開啟時是否自動續跑；預設關閉。 */
  auto_resume_after_exit: boolean;
  paused_since: number | null;
  /** 完全退出後暫停中，等使用者按「繼續工作」。 */
  awaiting_resume: boolean;
}

/** Runtime 生命週期（對應 Rust 的 LifecycleStatus）。 */
export interface LifecycleStatus {
  phase: LifecyclePhase;
  stop_intent_active: boolean;
  timing: { stop_grace_ms: number; kill_confirm_ms: number };
  exit: ExitReport | null;
  active_runs: number;
  workspaces: WorkspaceLifecycleView[];
}

/** 完全退出、各 Workspace 的自動續跑設定與「繼續工作」，以及桌面 App 的結束。 */
export interface LifecycleClient {
  lifecycleStatus(): Promise<LifecycleStatus>;
  /** 完全退出：Runtime 持久化停止意圖並開始停止。 */
  requestFullExit(): Promise<LifecycleStatus>;
  /** 只影響下次重新開啟，不立即啟停目前的工作。 */
  updateWorkspaceLifecycle(workspaceId: string, autoResumeAfterExit: boolean): Promise<WorkspaceLifecycleView>;
  resumeWorkspace(workspaceId: string): Promise<WorkspaceLifecycleView>;
  /** 結束桌面 App 程序。 */
  quitApp(): Promise<void>;
  /** 使用者在系統匣按「完全退出」時呼叫 handler；回傳取消訂閱的函式。 */
  onFullExitRequested(handler: () => void): () => void;
}

/** 共同設定的來源種類（對應 Rust 的 SourceKind）。 */
export type SettingsSourceKind = "settings" | "instructions" | "user_state";

/** 一個來源檔的讀取結果；只有路徑與雜湊，不含內容。 */
export interface SourceReport {
  kind: SettingsSourceKind;
  path: string;
  found: boolean;
  sha256: string | null;
}

/** 沒有被匯入的項目與原因；永遠不含值。 */
export interface SettingsExclusion {
  source: SettingsSourceKind;
  item: string;
  reason: "login_related" | "secret_key" | "secret_value";
}

export type SettingsCategory = "preferences" | "instructions" | "hooks" | "mcp" | "plugins";

export interface SettingsDiffEntry {
  category: SettingsCategory;
  key: string;
  change: "added" | "removed" | "changed";
  before: unknown;
  after: unknown;
}

/** 一個共同設定版本的內容。 */
export interface SettingsBundle {
  preferences: Record<string, unknown>;
  instructions: string | null;
  hooks: Record<string, unknown>;
  mcp_servers: Record<string, unknown>;
  plugins: Record<string, unknown>;
  marketplaces: Record<string, unknown>;
}

export interface SettingsBundleCounts {
  preferences: number;
  instructions: number;
  hooks: number;
  mcp_servers: number;
  plugins: number;
}

export interface SettingsVersionSummary {
  version: number;
  content_hash: string;
  adopted_at: number;
  counts: SettingsBundleCounts;
  exclusion_count: number;
}

/** 已採用的共同設定版本（不可變）；`versions` 由舊到新。 */
export interface SharedSettingsStatus {
  current: SettingsVersionSummary | null;
  versions: SettingsVersionSummary[];
}

/** 預覽匯入：不保存任何東西；採用時要帶回 `candidate_hash`。 */
export interface ImportPreview {
  candidate_hash: string;
  sources: SourceReport[];
  exclusions: SettingsExclusion[];
  bundle: SettingsBundle;
  base_version: number | null;
  diff: SettingsDiffEntry[];
  changed: boolean;
}

export interface SettingsVersion {
  version: number;
  content_hash: string;
  bundle: SettingsBundle;
  sources: SourceReport[];
  exclusions: SettingsExclusion[];
  adopted_at: number;
}

/** 共同設定：唯讀匯入使用者平常的 Claude Code 設定，採用後建立不可變版本。 */
export interface SharedSettingsClient {
  getSharedSettingsStatus(): Promise<SharedSettingsStatus>;
  previewSharedSettings(): Promise<ImportPreview>;
  /** 採用預覽的內容；來源在預覽後改變時以 `settings_source_changed` 拒絕。 */
  adoptSharedSettings(expectedHash: string): Promise<SettingsVersion>;
  getSharedSettingsVersion(version: number): Promise<SettingsVersion>;
}

export type AppClient = RuntimeClient &
  WorkspaceClient &
  AuthorizationClient &
  TaskClient &
  AccountClient &
  SchedulingClient &
  LifecycleClient &
  ArtifactClient &
  SharedSettingsClient;

/** 取出 Runtime 回報的穩定錯誤代碼：HTTP 版掛在 `code`，Tauri 版在訊息開頭的「（代碼）」。 */
export function errorCode(error: unknown): string | null {
  const code = (error as { code?: unknown } | null)?.code;
  if (typeof code === "string") return code;
  const message = error instanceof Error ? error.message : String(error);
  return /^Runtime 拒絕連線（([a-z0-9_]+)）/.exec(message)?.[1] ?? null;
}

const HEALTH_PATH = "/api/v1/health";
const PLATFORM_PATH = "/api/v1/platform";
const WORKSPACES_PATH = "/api/v1/workspaces";
const PROFILES_PATH = "/api/v1/profiles";
const SETUP_PATH = "/api/v1/setup";
const SCHEDULING_PATH = "/api/v1/scheduling";
const LIFECYCLE_PATH = "/api/v1/lifecycle";
/** 桌面殼在系統匣「完全退出」時發出的事件（先顯示主視窗再發出）。 */
export const FULL_EXIT_EVENT = "baton://full-exit-requested";
const SHARED_SETTINGS_PATH = "/api/v1/shared-settings";
const authorizationPath = (workspaceId: string) => `${WORKSPACES_PATH}/${encodeURIComponent(workspaceId)}/authorization`;
const workspacePath = (workspaceId: string) => `${WORKSPACES_PATH}/${encodeURIComponent(workspaceId)}`;
const taskPath = (workspaceId: string, taskId: string) => `${workspacePath(workspaceId)}/tasks/${encodeURIComponent(taskId)}`;
const artifactVersionPath = (workspaceId: string, artifactId: string, version: number) =>
  `${workspacePath(workspaceId)}/artifacts/${encodeURIComponent(artifactId)}/versions/${version}`;

/** 直接以 HTTP 查詢（開發時用瀏覽器開啟，由 Vite 代理到 Runtime）。 */
export function createHttpClient(fetchFn: typeof fetch = fetch): AppClient {
  /** 非 2xx 時優先採用 Runtime 回報的說明（`{ error, message }`），沒有才退回狀態碼。 */
  const failure = async (response: Response) => {
    const body = (await response.json().catch(() => null)) as { error?: string; message?: string } | null;
    // 穩定代碼掛在 `code`，讓畫面能辨識特定情況（例如 settings_source_changed）
    return Object.assign(new Error(body?.message ?? `Runtime 回應 ${response.status}`), { code: body?.error });
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
    listProfiles: () => send(PROFILES_PATH),
    createProfile: ({ name, expectedAccount }) =>
      send(PROFILES_PATH, { method: "POST", body: { name, expected_account: expectedAccount } }),
    verifyProfile: (id) => send(`${PROFILES_PATH}/${encodeURIComponent(id)}/verify`, { method: "POST" }),
    openLoginTerminal: (id) => send(`${PROFILES_PATH}/${encodeURIComponent(id)}/login-terminal`, { method: "POST" }),
    archiveProfile: (id) => send(`${PROFILES_PATH}/${encodeURIComponent(id)}/archive`, { method: "POST" }),
    getSetup: () => send(SETUP_PATH),
    recheckSetup: () => send(`${SETUP_PATH}/recheck`, { method: "POST" }),
    acknowledgeBackgroundNotice: () => send(`${SETUP_PATH}/background-notice/acknowledge`, { method: "POST" }),
    getSharedSettingsStatus: () => send(SHARED_SETTINGS_PATH),
    previewSharedSettings: () => send(`${SHARED_SETTINGS_PATH}/import-preview`, { method: "POST" }),
    adoptSharedSettings: (expectedHash) =>
      send(`${SHARED_SETTINGS_PATH}/adopt`, { method: "POST", body: { expected_hash: expectedHash } }),
    getSharedSettingsVersion: (version) => send(`${SHARED_SETTINGS_PATH}/versions/${version}`),
    dispatchTask: async (workspaceId, taskId) =>
      toDispatchResult(await send<RunView | QueuedDispatch>(`${taskPath(workspaceId, taskId)}/dispatch`, { method: "POST" })),
    schedulingOverview: () => send(SCHEDULING_PATH),
    setSchedulingMode: (mode) => send(`${SCHEDULING_PATH}/settings`, { method: "PUT", body: { mode } }),
    cancelRun: (workspaceId, runId) =>
      send(`${workspacePath(workspaceId)}/runs/${encodeURIComponent(runId)}/cancel`, { method: "POST" }),
    lifecycleStatus: () => send(LIFECYCLE_PATH),
    requestFullExit: () => send(`${LIFECYCLE_PATH}/exit`, { method: "POST" }),
    updateWorkspaceLifecycle: (workspaceId, autoResumeAfterExit) =>
      send(`${workspacePath(workspaceId)}/lifecycle`, {
        method: "PUT",
        body: { auto_resume_after_exit: autoResumeAfterExit },
      }),
    resumeWorkspace: (workspaceId) => send(`${workspacePath(workspaceId)}/resume`, { method: "POST" }),
    // 瀏覽器開發模式沒有桌面程序可結束；window.close() 對使用者自己開的分頁無效，所以不做事
    quitApp: async () => {},
    // 系統匣只存在於桌面殼，瀏覽器裡不會收到完全退出事件
    onFullExitRequested: () => () => {},
    listArtifacts: (workspaceId) => send(`${workspacePath(workspaceId)}/artifacts`),
    previewArtifact: (workspaceId, artifactId, version) =>
      send(`${artifactVersionPath(workspaceId, artifactId, version)}/preview`),
    async downloadArtifact(workspaceId, artifactId, version) {
      const response = await fetchFn(`${artifactVersionPath(workspaceId, artifactId, version)}/content`);
      if (!response.ok) throw await failure(response);
      return { blob: await response.blob(), sha256: response.headers.get("X-Baton-Sha256") ?? "" };
    },
  };
}

type Invoke = <T>(command: string, args?: Record<string, unknown>) => Promise<T>;

/** `@tauri-apps/api/event` 的 listen（只用到不帶 payload 的事件）。 */
type Listen = (event: string, handler: () => void) => Promise<() => void>;

/** 在 Tauri 內透過桌面殼的指令查詢；殼負責連回或啟動 Runtime。`listen` 省略時不接收殼的事件（只有測試會省略）。 */
export function createTauriClient(invoke: Invoke, listen: Listen = async () => () => {}): AppClient {
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
    listProfiles: () => call<ProfileList>("profile_list"),
    // Tauri 會把 JS 的 camelCase 參數對應到 Rust 的 snake_case
    createProfile: ({ name, expectedAccount }) => call<AccountProfile>("profile_create", { name, expectedAccount }),
    verifyProfile: (id) => call<AccountProfile>("profile_verify", { id }),
    openLoginTerminal: (id) => call<{ opened: boolean }>("profile_login_terminal", { id }),
    archiveProfile: (id) => call<AccountProfile>("profile_archive", { id }),
    getSetup: () => call<SetupStatus>("setup_status"),
    recheckSetup: () => call<SetupStatus>("setup_recheck"),
    acknowledgeBackgroundNotice: () => call<SetupStatus>("setup_acknowledge_notice"),
    getSharedSettingsStatus: () => call<SharedSettingsStatus>("shared_settings_status"),
    previewSharedSettings: () => call<ImportPreview>("shared_settings_preview"),
    adoptSharedSettings: (expectedHash) => call<SettingsVersion>("shared_settings_adopt", { expectedHash }),
    getSharedSettingsVersion: (version) => call<SettingsVersion>("shared_settings_version", { version }),
    dispatchTask: async (workspaceId, taskId) =>
      toDispatchResult(await call<RunView | QueuedDispatch>("task_dispatch", { workspaceId, taskId })),
    schedulingOverview: () => call<SchedulingOverview>("scheduling_overview"),
    setSchedulingMode: (mode) => call<SchedulingOverview>("scheduling_set_mode", { mode }),
    cancelRun: (workspaceId, runId) => call<RunView>("run_cancel", { workspaceId, runId }),
    lifecycleStatus: () => call<LifecycleStatus>("lifecycle_status"),
    requestFullExit: () => call<LifecycleStatus>("lifecycle_exit"),
    updateWorkspaceLifecycle: (workspaceId, autoResumeAfterExit) =>
      call<WorkspaceLifecycleView>("workspace_lifecycle_update", { workspaceId, autoResumeAfterExit }),
    resumeWorkspace: (workspaceId) => call<WorkspaceLifecycleView>("workspace_resume", { workspaceId }),
    quitApp: () => call<void>("app_quit"),
    onFullExitRequested: (handler) => {
      const registered = listen(FULL_EXIT_EVENT, () => handler());
      // 註冊是非同步的；取消訂閱時等註冊完成再解除
      return () => void registered.then((unlisten) => unlisten());
    },
    listArtifacts: (workspaceId) => call<ArtifactList>("artifact_list", { workspaceId }),
    previewArtifact: (workspaceId, artifactId, version) =>
      call<ArtifactPreview>("artifact_preview", { workspaceId, artifactId, version }),
    async downloadArtifact(workspaceId, artifactId, version) {
      // 桌面殼帶憑證取回內容，以 base64 交給 Web UI（Tauri 指令的回傳值是 JSON）
      const { sha256, content_base64 } = await call<{ sha256: string; content_base64: string }>("artifact_download", {
        workspaceId,
        artifactId,
        version,
      });
      const bytes = Uint8Array.from(atob(content_base64), (char) => char.charCodeAt(0));
      return { blob: new Blob([bytes]), sha256 };
    },
  };
}

/** 依執行環境挑選用戶端。 */
export async function createRuntimeClient(): Promise<AppClient> {
  if ("__TAURI_INTERNALS__" in window) {
    const [{ invoke }, { listen }] = await Promise.all([import("@tauri-apps/api/core"), import("@tauri-apps/api/event")]);
    return createTauriClient(invoke, listen);
  }
  return createHttpClient();
}
