/** Runtime 健康端點的回應（對應 Rust 的 HealthResponse）。 */
export interface HealthResponse {
  status: string;
  product: string;
  version: string;
  boot_count: number;
  started_at: number;
  pid: number;
  /** 控制資料庫儲存狀態；舊版 Runtime 沒有這個欄位，視為正常。 */
  storage?: StorageHealth;
  /** 監護程序狀態；舊版 Runtime 沒有這個欄位，視為未知。 */
  guardian?: GuardianStatus;
}

/** 監護程序狀態：`active` 執行中、`unavailable` 不在、`unknown` 舊版 Runtime 沒有回報。 */
export type GuardianState = "active" | "unavailable" | "unknown";

/** 監護程序狀態（對應 Rust 的 `baton_domain::guardian::GuardianStatus`）。 */
export interface GuardianStatus {
  state: GuardianState;
  /** 給使用者看的降級提示；只有 `unavailable` 時才有。 */
  message: string | null;
}

/** 控制資料庫的儲存健康狀態（對應 Rust 的 StorageHealth）。 */
export interface StorageHealth {
  /** 為真時 Runtime 已停止新派工。 */
  dispatch_halted: boolean;
  fault: StorageFault | null;
}

/** 控制資料庫無法寫入的原因與處理方式。 */
export interface StorageFault {
  kind: "disk_full" | "busy" | "unwritable";
  code: string;
  summary: string;
  /** 使用者可以怎麼處理。 */
  action: string;
  /** SQLite 原始訊息，僅供診斷。 */
  detail: string;
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
  model_overrides: ModelOverrides;
}

/** 依角色的模型覆寫（對應 Rust 的 ModelOverrides，spec D-30）；`null` 表示沿用帳號的 Claude Code 預設模型。 */
export interface ModelOverrides {
  controller: string | null;
  worker: string | null;
  reviewer: string | null;
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
  /** 已准入、尚未結束、仍沿用較舊 revision（且舊 revision 有目前已撤銷或縮小的能力）的 Run。 */
  legacy_runs: LegacyRun[];
}

/** 撤銷授權前已准入、仍沿用舊授權 revision 的 Run（對應 Rust 的 LegacyRun）。 */
export interface LegacyRun {
  run_id: string;
  task_id: string;
  task_title: string;
  /** 只會是 starting 或 running。 */
  status: RunStatus;
  /** 這個 Run 沿用的舊 revision。 */
  authorization_revision: number;
  /** 准入時間（UTC 毫秒）。 */
  admitted_at: number | null;
  /** 已要求停止、正在等程序確認停止。 */
  cancel_requested: boolean;
  /** 舊 revision 有、但目前 revision 已移除或縮小的能力。 */
  revoked_capabilities: Capability[];
}

export interface ConfirmationSummary {
  items: { capability: Capability; label: string; targets: string[]; restrictions: string[]; high_risk: boolean }[];
  risks: string[];
  requires_risk_acknowledgement: boolean;
  /** 相對於目前生效 revision 的變動；第一次確認（還沒有任何 revision）時為 null。 */
  changes: AuthorizationChanges | null;
}

/** 修改授權時，一項能力的變動（對應 Rust 的 GrantChange）。 */
export interface GrantChange {
  capability: Capability;
  label: string;
  high_risk: boolean;
  new_capability: boolean;
  removed_capability: boolean;
  added_targets: string[];
  removed_targets: string[];
  added_restrictions: string[];
  removed_restrictions: string[];
  expands: boolean;
}

/** 新內容相對於目前生效 revision 的變動（對應 Rust 的 AuthorizationChanges）。 */
export interface AuthorizationChanges {
  base_revision: number;
  changes: GrantChange[];
  /** 這次修改擴大授權：確認時必須帶 `acknowledgedExpansion: true`。 */
  expands: boolean;
  /** 給使用者看的影響說明。 */
  impact: string[];
}

export interface ConfirmAuthorizationRequest {
  templateId: string | null;
  grants: Grant[];
  acknowledgedRisk: boolean;
  /** 使用者已承認這次修改會擴大授權；修改既有授權且 `changes.expands` 為真時必須是 true。 */
  acknowledgedExpansion: boolean;
  /** 編輯時看到的目前 revision（確認摘要的 `changes.base_revision`）；另一個視窗先確認過時 Runtime 回 409。第一次確認時為 null。 */
  expectedRevision: number | null;
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
  /** 核對當下 `claude --version` 的輸出；取不到時為 `null`。 */
  cli_version: string | null;
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
  /** 設定目錄的來源：`managed` 是平台在資料目錄下建立的；`external` 是以既有目錄建立，封存時不刪除。 */
  config_dir_origin: "managed" | "external";
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

export interface CreateProfileFromDirRequest {
  name: string;
  configDir: string;
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
  /** 整組取代依角色的模型覆寫；只影響之後派出的 Run。 */
  setModelOverrides(id: string, overrides: ModelOverrides): Promise<Workspace>;
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
  /** decision：依據某個裁定請求（version 是引用時請求的內容版本）。 */
  kind: "task_contract" | "goal" | "authorization" | "decision";
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

/** 契約宣告的確定性檢查（對應 Rust 的 VerificationCheck）；`sha256` 為 null 代表只檢查存在。 */
export type VerificationCheck =
  | { kind: "command"; program: string; args: string[]; expected_exit_code: number }
  /** `path` 是工作目錄內的相對路徑。 */
  | { kind: "file"; path: string; sha256: string | null }
  /** `name` 是這個 Task 登記的成果名稱。 */
  | { kind: "artifact"; name: string; sha256: string | null };

export type ContractInput = { [K in ContractFieldName]?: ContractField | null } & {
  required_capabilities?: Capability[];
  /** 省略時視為空陣列。 */
  verification_checks?: VerificationCheck[];
};

export type TaskContract = { [K in ContractFieldName]: ContractField | null } & {
  task_id: string;
  version: number;
  required_capabilities: Capability[];
  verification_checks: VerificationCheck[];
  note: string | null;
  created_at: number;
};

/** 單項檢查的結果與證據；`inconclusive` 是證據不足或環境問題，不算通過也不算產品缺陷。 */
export interface CheckResult {
  check: VerificationCheck;
  status: "passed" | "failed" | "inconclusive";
  detail: string;
}

/** 成果 manifest 的一項：成果的某個版本與它的雜湊。 */
export interface ManifestEntry {
  artifact_id: string;
  name: string;
  version: number;
  sha256: string;
}

export interface ManifestRef {
  artifact_id: string;
  version: number;
}

/** 接受交易的依據：Runtime 以它校驗 Run 世代、契約版本與成果 manifest，不符回 409。 */
export interface AcceptanceBasis {
  run_id: string;
  generation: number;
  contract_version: number;
  artifacts: ManifestRef[];
}

/** 驗證紀錄（對應 Rust 的 VerificationRecord）；`decided_at` 是 UTC 毫秒。 */
export interface VerificationRecord {
  id: string;
  workspace_id: string;
  task_id: string;
  run_id: string;
  generation: number;
  contract_version: number;
  method: "checks" | "manual";
  /** `inconclusive`：驗證不完整（證據不足或環境問題），計入驗證不完整重驗的預算。 */
  outcome: "passed" | "failed" | "inconclusive";
  checks: CheckResult[];
  artifacts: ManifestEntry[];
  accepted_by: string | null;
  note: string | null;
  decided_at: number;
}

/** 驗證畫面需要的資料（對應 Rust 的 VerificationView）。 */
export interface VerificationView {
  task_id: string;
  status: TaskStatus;
  basis: AcceptanceBasis | null;
  checks: VerificationCheck[];
  artifacts: ManifestEntry[];
  /** true：Runtime 會自動執行檢查；false：只能手動接受。 */
  automatic: boolean;
  /** 由舊到新。 */
  records: VerificationRecord[];
  /** 這個 Task 最新的整合項目（只有開發 Task 會有）；沒有為 null。 */
  integration: IntegrationEntry | null;
}

/** 手動接受成果：`basis` 原樣送回 Runtime 取得的值。 */
export interface AcceptTaskRequest {
  basis: AcceptanceBasis;
  note?: string | null;
}

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

/** Task 種類：決定 Run 在哪種工作目錄執行（開發 Task 用 worktree，研究／文件用輸出目錄）。 */
export type TaskKind = "development" | "research" | "document";

export interface TaskView {
  id: string;
  workspace_id: string;
  goal_id: string;
  title: string;
  kind: TaskKind;
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
  /** 准入後代表准入時依據的授權 revision。 */
  authorization_revision: number | null;
  /** 准入時間（UTC 毫秒）；尚未准入（queued）或票 24 之前的舊資料為 null。 */
  admitted_at: number | null;
  /** 准入時固定的共同設定版本；沒有採用任何版本為 null。 */
  settings_version: number | null;
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

/** 不能派工（留在佇列）的原因類別；各自對應不同的處理方式。`lifecycle` 是完全退出中或等待「繼續工作」，`workdir` 是工作目錄不可用，`backoff` 是基礎設施重試的退避期間。 */
export type WaitCategory =
  | "capacity"
  | "quota"
  | "authorization"
  | "dependency"
  | "state_unknown"
  | "account"
  | "lifecycle"
  | "workdir"
  | "guard"
  | "backoff";

/** 等待原因（對應 Rust 的 WaitReason）；`code` 是穩定的機器可讀代碼。 */
export interface WaitReason {
  category: WaitCategory;
  code: string;
  message: string;
  profile_id?: string;
  /** 只有 Guard 擋下（`guard_violated`）時才有：擋下的 Guard 與它的來源教訓。 */
  guard?: { guard_id: string; lesson_id: string; lesson_title: string };
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
  /** Workspace 全部驗證紀錄，由舊到新。 */
  verifications: VerificationRecord[];
  /** Workspace 目前生效的授權 revision；尚未確認為 null。 */
  authorization_revision: number | null;
  /** 各 Task 在平台登記的工作目錄。 */
  workdirs: WorkdirView[];
  /** 這個 Workspace 的整合佇列項目，依排入順序。 */
  integrations: IntegrationEntry[];
  /** 各 Task 的修正與重試預算，每個 Task 一筆。 */
  budgets: BudgetView[];
  /** Workspace 全部交接，依建立順序；舊版 Runtime 沒有這個欄位，視為空陣列。 */
  handoffs?: Handoff[];
}

/** 修正與重試預算的種類（spec D-16）：三種計數互不影響。 */
export type BudgetKind = "implementation" | "verification" | "infrastructure";

/** 一種預算的計數；`used` 在 0..=limit 之間。 */
export interface BudgetCounter {
  kind: BudgetKind;
  /** 顯示用名稱，例如「實作失敗修正」。 */
  label: string;
  used: number;
  limit: number;
}

/** Task 的修正與重試預算（對應 Rust 的 BudgetView）；時間是 UTC 毫秒。 */
export interface BudgetView {
  task_id: string;
  /** 一律三項，依 implementation、verification、infrastructure 順序。 */
  counters: BudgetCounter[];
  /** 基礎設施重試的退避期限，之前不會派工；沒有退避為 null。 */
  retry_not_before: number | null;
  /** 已用盡而等待裁定的種類；沒有為 null。 */
  exhausted: BudgetKind | null;
  /** 用盡時建立的裁定請求。 */
  decision_request_id: string | null;
}

/**
 * 交接階段：requested 等待保存檢查點、stopping 停止舊程序、transferred 已轉移等待接手確認、confirmed 已確認、abandoned 已放棄、
 * waiting_resource 突然限流而所有合適帳號都受限（舊程序已停止，等資源恢復再接手）。
 */
export type HandoffPhase = "requested" | "stopping" | "transferred" | "confirmed" | "abandoned" | "waiting_resource";

/** 交接要延續的 checkpoint（對應 Rust 的 CheckpointRef）；時間是 UTC 毫秒。 */
export interface HandoffCheckpoint {
  run_id: string;
  generation: number;
  summary: string;
  details: string;
  saved_at: number;
}

/** 接手的 Worker 對交接的確認（對應 Rust 的 HandoffConfirmation）。 */
export interface HandoffConfirmation {
  completed: string;
  incomplete: string;
  unknown_side_effects: string;
}

/** 一次跨帳號交接（對應 Rust 的 Handoff）；時間是 UTC 毫秒。 */
export interface Handoff {
  id: string;
  workspace_id: string;
  task_id: string;
  from_run_id: string;
  from_profile_id: string;
  from_generation: number;
  /** 承接帳號；空字串表示尚未選定（突然限流而所有帳號都受限時）。 */
  to_profile_id: string;
  to_run_id: string | null;
  to_generation: number | null;
  phase: HandoffPhase;
  /** 帳號門檻的原因代碼，例如 drain_threshold、limit_warning、limit_reached（Claude Code 回報已達上限＝突然限流）。 */
  reason: string;
  checkpoint: HandoffCheckpoint | null;
  /** 舊程序怎麼停下的：stopped_gracefully、force_killed、exited、dead、already_ended。 */
  stop_outcome: string | null;
  confirmation: HandoffConfirmation | null;
  abandoned_reason: string | null;
  requested_at: number;
  stopping_at: number | null;
  transferred_at: number | null;
  confirmed_at: number | null;
  /** 進入等待資源的時間。以下等待欄位舊版 Runtime 沒有，視為 null／0／空陣列。 */
  waiting_at?: number | null;
  /** 預計重新評估的時間；waiting_resource 時一定有值。 */
  next_check_at?: number | null;
  /** 已重新評估但仍受限的次數（退避的指數）；0 表示還沒重新評估過。 */
  wait_attempts?: number;
  /** 依據的帳號重置時間（最早的那個）；null 表示重置時間未知、以退避輪詢。 */
  wait_reset_at?: number | null;
  /** 最近一次評估時不能派給任何帳號的原因。 */
  wait_reasons?: WaitReason[];
  updated_at: number;
  version: number;
}

/** Task 在平台登記的工作目錄（對應 Rust 的 WorkdirView）；時間是 UTC 毫秒。 */
export interface WorkdirView {
  task_id: string;
  workspace_id: string;
  /** worktree：開發 Task 的 Git worktree；output_dir：研究／文件 Task 的輸出目錄。 */
  kind: "worktree" | "output_dir";
  /** 絕對路徑。 */
  path: string;
  /** worktree 的平台分支。 */
  branch: string | null;
  /** 整合目標分支。 */
  base_branch: string | null;
  status: "ready" | "failed";
  /** 準備失敗的原因（給使用者看）。 */
  failure: string | null;
  version: number;
  attempted_at: number;
  ready_at: number | null;
  /** 在這個目錄中執行過的 Run，依建立順序。 */
  run_ids: string[];
  /** worktree 的清理登記；Task 尚未完成並整合、或是輸出目錄時為 null。 */
  cleanup: WorktreeCleanup | null;
}

/**
 * worktree 清理狀態：pending 待清理（failure 不為 null 時表示上一次失敗、會在 next_attempt_at 自動重試）、
 * removing 清理中、cleaned 已清理、failed 清理失敗（重試用完，不再自動重試，worktree 保留）。
 */
export type CleanupStatus = "pending" | "removing" | "cleaned" | "failed";

/** 開發 Task 完成並整合後的 worktree 清理登記（對應 Rust 的 WorktreeCleanup）；時間是 UTC 毫秒。 */
export interface WorktreeCleanup {
  task_id: string;
  workspace_id: string;
  /** 要移除的 worktree。 */
  path: string;
  /** 平台分支。 */
  branch: string;
  status: CleanupStatus;
  version: number;
  /** 已開始過的嘗試次數。 */
  attempts: number;
  /** 最近一次失敗的原因（給使用者看）。 */
  failure: string | null;
  requested_at: number;
  /** pending 時下一次自動重試的時間。 */
  next_attempt_at: number | null;
  finished_at: number | null;
}

export type IntegrationStatus = "queued" | "merging" | "verifying" | "advancing" | "integrated" | "failed";

/** 整合佇列的一個項目（對應 Rust 的 IntegrationEntry）；時間是 UTC 毫秒。 */
export interface IntegrationEntry {
  id: string;
  /** 排入順序；佇列依它處理。 */
  seq: number;
  workspace_id: string;
  task_id: string;
  run_id: string;
  generation: number;
  branch: string;
  target_branch: string;
  status: IntegrationStatus;
  version: number;
  merge_commit: string | null;
  target_tip: string | null;
  /** 整合驗證的結果。 */
  checks: CheckResult[];
  failure: string | null;
  queued_at: number;
  started_at: number | null;
  finished_at: number | null;
}

/** Context Builder 組裝 Prompt 的項目類型。 */
export type ContextItemKind = "contract" | "handoff" | "checkpoint" | "decision" | "lesson" | "messages" | "evidence";

/** context receipt 的一個項目；`omitted_reason` 目前只有 `budget_exceeded`。 */
export interface ContextItem {
  kind: ContextItemKind;
  ref_id: string;
  /** 沒有版本的項目（例如未讀訊息摘要）為 null。 */
  version: number | null;
  title: string;
  tokens: number;
  status: "loaded" | "omitted";
  omitted_reason: string | null;
}

/** Run 啟動時保存的 context receipt：載入了哪些項目與版本、因預算省略了什麼；`items` 依優先序排列。 */
export interface ContextReceipt {
  run_id: string;
  task_id: string;
  built_at: number;
  budget_tokens: number;
  used_tokens: number;
  prompt_sha256: string;
  items: ContextItem[];
}

/** Run 的 context receipt 與實際交給 Worker 的完整 Prompt。 */
export interface RunContext {
  receipt: ContextReceipt;
  prompt: string;
}

export interface TaskDetail {
  task: TaskView;
  /** 由舊到新，含目前版本。 */
  contract_history: TaskContract[];
  budget: BudgetView;
}

export interface AdmissionReport {
  admissible: boolean;
  missing: MissingItem[];
}

export interface CreateTaskRequest {
  goalId: string;
  title: string;
  /** 省略時 Runtime 預設為 research。 */
  kind?: TaskKind;
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
  /** Run 的 context receipt；Context Builder 之前建立的 Run 沒有，Runtime 回 404（context_receipt_not_found）。 */
  runContext(workspaceId: string, runId: string): Promise<RunContext>;
  /** verifying 的 Task 目前的驗證資料：宣告的檢查、成果 manifest、接受依據與驗證紀錄。 */
  taskVerification(workspaceId: string, taskId: string): Promise<VerificationView>;
  /** 手動接受成果；依據過期時 Runtime 回 409。 */
  acceptTask(workspaceId: string, taskId: string, request: AcceptTaskRequest): Promise<VerificationView>;
}

/** 裁定請求的種類：Agent 主動請求、工具權限提示，或 Controller 想處理「必須送裁定」事項而被權限表攔下。 */
export type DecisionKind = "agent" | "permission" | "authority";

/** superseded：目標版本已變更而過期；merged：已合併到另一個請求。 */
export type DecisionStatus = "pending" | "answered" | "superseded" | "merged";

/** 處理方式：接受方案、修改條件、回答、要求調查、延後、駁回。 */
export type DecisionAction = "accept" | "modify" | "answer" | "investigate" | "defer" | "reject";

export type DecisionUrgency = "low" | "normal" | "high";

export interface DecisionOption {
  id: string;
  label: string;
  tradeoff: string | null;
}

/** 這個裁定擋住的 Task；只有這些 Task 受影響。 */
export interface DecisionBlocking {
  task_id: string;
  task_title: string;
  task_status: TaskStatus;
}

/** 「允許本次操作」綁定的單一邏輯操作；執行時逐項核對，任何一項不同就不能沿用。 */
export interface ApprovalBinding {
  /** 例如 "Bash:git push"、"Write"。 */
  operation: string;
  /** 目標資源，例如 "origin main"、檔案路徑、網址。 */
  target: string | null;
  /** 重要參數，例如 { flags: ["--force"] }。 */
  params: Record<string, unknown>;
  /** SHA-256 小寫十六進位。 */
  request_hash: string;
  /** Task 的成果版本，例如 "report.md@2,notes.md@1"；沒有成果時是空字串。 */
  artifact_version: string;
}

/** 單次批准被某個 Run 使用的紀錄。 */
export interface ApprovalUse {
  request_id: string;
  run_id: string;
  used_at: number;
}

export type ApprovalMismatchReason =
  | "operation_changed"
  | "target_changed"
  | "params_changed"
  | "request_hash_changed"
  | "artifact_version_changed";

/** 先前有單次批准，但這次的操作與它的綁定不符而不能沿用。 */
export interface RejectedApproval {
  request_id: string;
  reason: ApprovalMismatchReason;
}

/** 權限提示的內容：哪個工具、對應哪項授權能力。 */
export interface DecisionPermission {
  tool_name: string;
  input: unknown;
  capability: Capability | null;
  capability_label: string | null;
  summary: string;
  /** 單次批准綁定的操作；票 23 之前建立的請求沒有。 */
  binding: ApprovalBinding | null;
  /** 同一操作先前已用掉一次批准、結果未知：要先查證再回答。 */
  prior_use: ApprovalUse | null;
  rejected_approval: RejectedApproval | null;
  /** 這個請求的裁定已被某個 Run 使用。 */
  approval_use: ApprovalUse | null;
}

/** 權限表攔截的內容：Controller 想自行處理的事項（spec D-15）。 */
export interface DecisionAuthority {
  matter: string;
  matter_label: string;
  summary: string;
}

/** 相似問題的連結或合併來源。 */
export interface DecisionLink {
  id: string;
  question: string;
  status: DecisionStatus;
}

/** 提出請求時的目標版本；版本變更後請求會被標為 superseded。 */
export interface DecisionTarget {
  task_id: string;
  contract_version: number;
  authorization_revision: number | null;
}

/** 使用者已作出的裁定（延後不算裁定，不會出現在這裡）。 */
export interface DecisionOutcome {
  action: Exclude<DecisionAction, "defer">;
  option_id: string | null;
  option_label: string | null;
  note: string | null;
  decided_by: string;
  decided_at: number;
  /** 批次處理時的批次識別；每一項仍各自保存。 */
  batch_id: string | null;
}

/** 裁定請求（對應 Rust 的 DecisionRequestView）；時間一律是 UTC 毫秒。 */
export interface DecisionRequestView {
  id: string;
  workspace_id: string;
  run_id: string;
  task_id: string;
  task_title: string;
  kind: DecisionKind;
  status: DecisionStatus;
  /** 提交時以 expected_revision 帶回，內容變更過就會被拒。 */
  revision: number;
  question: string;
  /** 為什麼 Agent 無法自行決定。 */
  reason: string;
  options: DecisionOption[];
  /** 建議的 option id。 */
  recommendation: string | null;
  evidence: string[];
  urgency: DecisionUrgency;
  /** 期限只用來標示逾期，逾期不代表任何決定。 */
  deadline_at: number | null;
  overdue: boolean;
  /** 延後不是同意：請求仍是 pending，工作仍在等待。 */
  deferred_until: number | null;
  deferred: boolean;
  merged_into: string | null;
  /** superseded 的原因。 */
  status_reason: string | null;
  links: DecisionLink[];
  merged_from: DecisionLink[];
  blocking: DecisionBlocking[];
  permission: DecisionPermission | null;
  authority: DecisionAuthority | null;
  target: DecisionTarget;
  /** pending 時可用的處理方式；已結束為空。 */
  allowed_actions: DecisionAction[];
  created_at: number;
  decision: DecisionOutcome | null;
}

/** 裁定中心頂部的總覽數字。 */
export interface DecisionOverview {
  pending: number;
  actionable: number;
  deferred: number;
  overdue: number;
  high_urgency: number;
  answered: number;
  superseded: number;
  merged: number;
}

/** 後端已排序：可處理的 pending、延後的 pending、已結束的。 */
export interface DecisionList {
  pending_count: number;
  overview: DecisionOverview;
  decisions: DecisionRequestView[];
}

/** 處理單筆裁定；accept/modify 需要 option_id，modify/answer/investigate 需要 note，defer 用 defer_minutes。 */
export interface AnswerRequest {
  action: DecisionAction;
  expected_revision: number;
  option_id?: string;
  note?: string;
  defer_minutes?: number;
}

export interface AnswerResponse {
  decision: DecisionRequestView;
  /** 因這次裁定而恢復的 Task。 */
  resumed_task_ids: string[];
}

/** 批次中的一項：各自指定處理方式，各自保存。 */
export type BatchItem = AnswerRequest & { workspace_id: string; request_id: string };

export type BatchResult =
  | { workspace_id: string; request_id: string; ok: true; decision: DecisionRequestView; resumed_task_ids: string[] }
  | { workspace_id: string; request_id: string; ok: false; error: string; message: string };

export interface BatchResponse {
  batch_id: string;
  results: BatchResult[];
}

/** 把請求合併到同種類、pending 的另一個請求；兩邊都要帶 revision。 */
export interface MergeRequest {
  into: string;
  expected_revision: number;
  into_expected_revision: number;
}

/** 決策權限表的層級：Controller 可自主處理、必須送裁定、只能人工發起。 */
export type AuthorityLevel = "autonomous" | "requires_decision" | "human_only";

export interface AuthorityEntry {
  matter: string;
  label: string;
  builtin: AuthorityLevel;
  effective: AuthorityLevel;
  narrowed: boolean;
}

/** 決策權限表（spec D-15）：Workspace 可收窄、不可放寬。 */
export interface AuthorityTable {
  revision: number;
  entries: AuthorityEntry[];
}

/** 取代式更新：沒列出的事項回到內建值。 */
export interface UpdateAuthorityRequest {
  expected_revision: number;
  entries: { matter: string; effective: AuthorityLevel }[];
}

/** 全部 Workspace 的待裁定數（頂部狀態列用）。 */
export interface PendingDecisions {
  pending_count: number;
  by_workspace: { workspace_id: string; pending_count: number }[];
}

/** 裁定中心：列出、處理、合併與連結裁定請求，以及決策權限表（只有桌面憑證能用）。 */
export interface DecisionClient {
  listDecisions(workspaceId: string): Promise<DecisionList>;
  getDecision(workspaceId: string, requestId: string): Promise<DecisionRequestView>;
  answerDecision(workspaceId: string, requestId: string, request: AnswerRequest): Promise<AnswerResponse>;
  /** 每一項各自保存，部分失敗不影響其他項。 */
  batchDecisions(items: BatchItem[]): Promise<BatchResponse>;
  /** 回傳合併的目標。 */
  mergeDecision(workspaceId: string, requestId: string, request: MergeRequest): Promise<{ decision: DecisionRequestView }>;
  linkDecision(workspaceId: string, requestId: string, otherId: string): Promise<DecisionRequestView>;
  decisionAuthority(workspaceId: string): Promise<AuthorityTable>;
  /** 放寬會被拒（authority_widening_refused）。 */
  updateDecisionAuthority(workspaceId: string, request: UpdateAuthorityRequest): Promise<AuthorityTable>;
  pendingDecisions(): Promise<PendingDecisions>;
}

/** 成果瀏覽（只有桌面憑證能用）：下載與其他端點走同一套帶憑證的機制。 */
export interface ArtifactClient {
  listArtifacts(workspaceId: string): Promise<ArtifactList>;
  previewArtifact(workspaceId: string, artifactId: string, version: number): Promise<ArtifactPreview>;
  downloadArtifact(workspaceId: string, artifactId: string, version: number): Promise<ArtifactDownload>;
}

/** 知識條目的種類：決策、教訓、知識。 */
export type KnowledgeKind = "decision" | "lesson" | "fact";
/** 候選、已驗證、已撤銷、已淘汰、已被取代。 */
export type KnowledgeStatus = "candidate" | "verified" | "revoked" | "retired" | "superseded";
/** 適用範圍：全域、Workspace、Goal、Task。 */
export type ScopeKind = "global" | "workspace" | "goal" | "task";
/** `like_fallback` 表示全文檢索無法使用，已退回逐筆比對。 */
export type SearchMode = "full_text" | "like_fallback";

export interface KnowledgeScope {
  kind: ScopeKind;
  workspace_id: string | null;
  goal_id: string | null;
  task_id: string | null;
}

export type KnowledgeSource =
  | { kind: "user" }
  | { kind: "agent"; run_id: string; generation: number; task_id: string }
  | { kind: "controller"; controller_run_id: string; task_id: string };

/** 教訓欄位：觸發條件、根因或待驗假說、解法。 */
export interface LessonDetail {
  trigger: string;
  root_cause: string;
  resolution: string;
}

export interface KnowledgeContent {
  title: string;
  body: string;
  tags: string[];
  evidence: string[];
  lesson: LessonDetail | null;
}

/** 知識條目（Rust 以 flatten 攤平在同一層）；`effective` 是「已驗證且現在在有效期間內」。 */
export interface KnowledgeEntryView {
  id: string;
  kind: KnowledgeKind;
  scope: KnowledgeScope;
  status: KnowledgeStatus;
  source: KnowledgeSource;
  valid_from: number;
  valid_until: number | null;
  version: number;
  revision: number;
  supersedes: string | null;
  superseded_by: string | null;
  status_reason: string | null;
  content: KnowledgeContent;
  created_at: number;
  updated_at: number;
  effective: boolean;
}

export interface KnowledgeList {
  search_mode: SearchMode;
  /** 最多 100 筆，依 updated_at 由新到舊。 */
  entries: KnowledgeEntryView[];
}

export interface KnowledgeVersion {
  version: number;
  content: KnowledgeContent;
  note: string | null;
  created_at: number;
}

/** 變更歷史；`event_type` 例如 knowledge_created、knowledge_revoked（payload.reason）。 */
export interface KnowledgeEvent {
  revision: number;
  event_type: string;
  occurred_at: number;
  payload: unknown;
}

export interface KnowledgeDetail {
  entry: KnowledgeEntryView;
  /** 由舊到新。 */
  versions: KnowledgeVersion[];
  /** 由舊到新。 */
  history: KnowledgeEvent[];
}

/** 列表查詢：`workspaceId` 省略時只列全域條目；`q` 空白時不過濾；`includeInactive` 為 true 時連無效條目一起回傳。 */
export interface KnowledgeListQuery {
  workspaceId?: string;
  q?: string;
  includeInactive: boolean;
}

export interface CreateKnowledgeRequest {
  kind: KnowledgeKind;
  scope: { kind: ScopeKind; workspace_id?: string; goal_id?: string; task_id?: string };
  title: string;
  body?: string;
  tags?: string[];
  evidence?: string[];
  /** 只有教訓可以帶。 */
  lesson?: LessonDetail;
  valid_from?: number;
  valid_until?: number;
  supersedes?: string;
}

/** 修正：以完整的新內容追加新版本；`valid_until` 省略表示沒有期限。 */
export interface CorrectKnowledgeRequest {
  expected_revision: number;
  title: string;
  body?: string;
  tags?: string[];
  evidence?: string[];
  lesson?: LessonDetail;
  valid_until?: number;
  note?: string;
}

/** 撤銷或淘汰（都要說明原因）。 */
export interface KnowledgeReasonRequest {
  expected_revision: number;
  reason: string;
}

/** 知識與教訓（只有桌面憑證能用）。 */
export interface KnowledgeClient {
  listKnowledge(query: KnowledgeListQuery): Promise<KnowledgeList>;
  /** 使用者建立的條目直接是已驗證。 */
  createKnowledge(request: CreateKnowledgeRequest): Promise<KnowledgeEntryView>;
  knowledgeDetail(id: string): Promise<KnowledgeDetail>;
  /** 候選 → 已驗證。 */
  confirmKnowledge(id: string, expectedRevision: number): Promise<KnowledgeEntryView>;
  correctKnowledge(id: string, request: CorrectKnowledgeRequest): Promise<KnowledgeEntryView>;
  revokeKnowledge(id: string, request: KnowledgeReasonRequest): Promise<KnowledgeEntryView>;
  retireKnowledge(id: string, request: KnowledgeReasonRequest): Promise<KnowledgeEntryView>;
}

/** Guard 規則（對應 Rust 的 GuardRule）：啟動前檢查、工具限制、重試政策。 */
export type GuardRule =
  /** 契約欄位必須提到 `must_mention`（不分大小寫）。 */
  | { kind: "preflight"; field: ContractFieldName; must_mention: string }
  /** Task 不得要求該能力。 */
  | { kind: "tool_restriction"; capability: Capability }
  /** 同一份契約失敗 `max_failed_runs` 次（1–20）後不再派工。 */
  | { kind: "retry_policy"; max_failed_runs: number };

export type GuardStatus = "enabled" | "disabled";

/** Guard 與來源教訓、生效狀態與成效（對應 Rust 的 GuardView）；時間是 UTC 毫秒。 */
export interface GuardView {
  id: string;
  lesson_id: string;
  rule: GuardRule;
  status: GuardStatus;
  revision: number;
  created_at: number;
  updated_at: number;
  lesson: { id: string; title: string; status: KnowledgeStatus; effective: boolean; scope: KnowledgeScope };
  /** 啟用且來源教訓現在有效。 */
  active: boolean;
  /** 不生效的原因；生效中為 null。 */
  inactive_reason: string | null;
  /** 規則的中文說明，直接顯示。 */
  description: string;
  /** 攔下的重犯次數。 */
  prevented_count: number;
  last_prevented_at: number | null;
  /** Guard 建立之後、沒被攔下而由使用者回報的重犯次數。 */
  reported_count: number;
}

export interface GuardList {
  guards: GuardView[];
}

export interface CreateGuardRequest {
  lesson_id: string;
  rule: GuardRule;
}

/** 重犯事件：被 Guard 攔下，或沒被攔下而由使用者回報。 */
export interface Recurrence {
  id: string;
  lesson_id: string;
  guard_id: string | null;
  workspace_id: string | null;
  task_id: string | null;
  contract_version: number | null;
  kind: "prevented" | "reported";
  message: string;
  occurred_at: number;
}

export interface RecurrenceList {
  /** 由新到舊。 */
  recurrences: Recurrence[];
}

export interface ReportRecurrenceRequest {
  note: string;
  workspace_id?: string;
  task_id?: string;
}

/** Guard 與重犯事件（只有桌面憑證能用）。 */
export interface GuardClient {
  /** `workspaceId` 省略時只列全域教訓的 Guard。 */
  listGuards(workspaceId?: string): Promise<GuardList>;
  /** 從有效的已驗證教訓建立 Guard；建立即啟用。 */
  createGuard(request: CreateGuardRequest): Promise<GuardView>;
  enableGuard(id: string, expectedRevision: number): Promise<GuardView>;
  disableGuard(id: string, expectedRevision: number): Promise<GuardView>;
  listRecurrences(lessonId: string): Promise<RecurrenceList>;
  /** 回報一次沒被攔下的重犯。 */
  reportRecurrence(lessonId: string, request: ReportRecurrenceRequest): Promise<Recurrence>;
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

/** 用量視窗：滾動五小時與七日。 */
export type QuotaWindow = "five_hour" | "seven_day";

/** 額度資料來源（spec D-12）：CLI 回報的「已觀測」，或依本機 Run 用量的「推估」。 */
export type QuotaSource = "observed" | "estimated";

/** 可信度；觀測不到時為 unknown，不填零。 */
export type QuotaConfidence = "observed" | "estimated" | "unknown";

/** CLI 回報的限流狀態。 */
export type LimitStatus = "allowed" | "warning" | "exhausted";

/** 一筆觀測的原始資料從哪裡來：Adapter 解析 CLI、Runtime 推估、外部觀測器輪詢。 */
export type ObservationOrigin = "adapter" | "estimate" | "observer";

/** 一筆額度觀測（對應 Rust 的 QuotaObservation）；觀測不到的欄位為 null，時間是 UTC 毫秒。 */
export interface QuotaObservation {
  profile_id: string;
  origin: ObservationOrigin;
  source: QuotaSource;
  window: QuotaWindow | null;
  status: LimitStatus | null;
  /** 已用比例（1 = 達到上限，推估可能超過 1）；null 是未知。 */
  used: number | null;
  /** 推估：視窗內本機 Run 累計的 token 數。 */
  tokens: number | null;
  /** 推估：計算時使用的方案上限（token）；沒有設定為 null。 */
  limit_tokens: number | null;
  reset_at: number | null;
  observed_at: number;
  confidence: QuotaConfidence;
  /** 原始資料參照，例如 `run:<run_id>`。 */
  raw_ref: string;
}

/** 一個視窗的判斷：兩個來源各自的最新觀測，與取較保守者後的結果。 */
export interface WindowQuota {
  window: QuotaWindow;
  label: string;
  effective: QuotaObservation | null;
  observed: QuotaObservation | null;
  estimated: QuotaObservation | null;
}

/** 一個帳號的額度判斷（對應 Rust 的 QuotaAssessment）。 */
export interface QuotaAssessment {
  /** 以較保守者判斷的用量比例；null 是未知（顯示「未知」，不是 0）。 */
  used: number | null;
  source: QuotaSource | null;
  confidence: QuotaConfidence;
  reset_at: number | null;
  observed_at: number | null;
  /** 是否看得到帳號的完整用量（含其他裝置）。 */
  complete: boolean;
  incomplete_reason: string | null;
  /** 固定兩個：five_hour、seven_day。 */
  windows: WindowQuota[];
  external: QuotaObservation | null;
}

/** 使用者為 Profile 設定的方案上限（推估的分母），單位為 token；未設定為 null。 */
export interface PlanLimits {
  five_hour_tokens: number | null;
  seven_day_tokens: number | null;
}

export interface AccountScheduling {
  profile_id: string;
  name: string;
  dispatchable: boolean;
  blockers: ProfileBlocker[];
  /** 已知的用量比例（0 到 1）；`null` 是尚未觀測，不是 0。與 `quota.used` 相同。 */
  quota_used: number | null;
  quota: QuotaAssessment;
  plan_limits: PlanLimits;
  running: number;
  limit: number;
  runs: AccountRun[];
  /** 額度門檻；`null` 代表帳號從未離開 open（可派工）。 */
  quota_gate: QuotaGate | null;
}

/** 額度門檻狀態（spec D-13）：open 可派工、paused 停止派工、draining 停止派工且交接進行中的 Run。 */
export type GateState = "open" | "paused" | "draining";

/** 一個帳號目前的額度門檻（對應 Rust 的 QuotaGate）；時間是 UTC 毫秒。 */
export interface QuotaGate {
  profile_id: string;
  state: GateState;
  /** 進入這個狀態的原因代碼：stop_dispatch_threshold、drain_threshold、limit_warning、limit_reached（突然限流）、recovered、reset_observed、usage_unknown。 */
  reason: string;
  /** 轉換當下的判斷用量；未知為 null。 */
  used: number | null;
  /** 進入 paused／draining 時記下的重置時間；過了就恢復。 */
  reset_at: number | null;
  /** 進入這個狀態的時間。 */
  since: number;
  version: number;
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

/** 排程：總覽（容量、各帳號進行中的 Run 與額度、佇列）、容量預設切換與方案上限設定。 */
export interface SchedulingClient {
  schedulingOverview(): Promise<SchedulingOverview>;
  setSchedulingMode(mode: SchedulingMode): Promise<SchedulingOverview>;
  /** 設定 Profile 的方案上限（推估的分母）；回傳更新後的排程總覽。 */
  setQuotaLimits(profileId: string, limits: PlanLimits): Promise<SchedulingOverview>;
}

/** 帳號 Profile 與首次啟動精靈；與其他用戶端分開，讓只關心帳號的畫面不必知道它們。 */
export interface AccountClient {
  listProfiles(): Promise<ProfileList>;
  createProfile(request: CreateProfileRequest): Promise<AccountProfile>;
  /** 以已登入的既有 Claude Code 設定目錄建立 Profile；Runtime 會立即核對，回傳的 Profile 已帶核對結果。 */
  createProfileFromExistingDir(request: CreateProfileFromDirRequest): Promise<AccountProfile>;
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
  /** 這一項是 Controller CLI，不是 Worker Run。 */
  controller_cli?: boolean;
}

/** 最近一次完全退出的經過；時間是 UTC 毫秒。 */
export interface ExitReport {
  requested_at: number;
  deadline_at: number;
  completed_at: number | null;
  runs: ExitRunResult[];
  notes: string[];
}

/** Workspace 被暫停的原因：完全退出、跨 Windows 登入（未開啟跨登入自動續跑），或無法核對是否跨登入。 */
export type PauseCause = "full_exit" | "new_login" | "login_unverified";

/** 單一 Workspace 的生命週期設定與狀態（對應 Rust 的 WorkspaceLifecycleView）。 */
export interface WorkspaceLifecycleView {
  workspace_id: string;
  name: string;
  /** 完全退出後重新開啟時是否自動續跑；預設關閉。 */
  auto_resume_after_exit: boolean;
  /** 重新登入 Windows 後是否自動續跑；預設關閉，與 auto_resume_after_exit 分開保存。 */
  auto_resume_across_login: boolean;
  paused_since: number | null;
  /** 暫停的原因；沒有暫停時為 null。 */
  paused_by: PauseCause | null;
  /** 暫停中，等使用者按「繼續工作」。 */
  awaiting_resume: boolean;
}

/** 「登入 Windows 時自動啟動 Baton」（對應 Rust 的 LoginStartupView）。 */
export interface LoginStartupView {
  /** 使用者的偏好；預設關閉。 */
  enabled: boolean;
  /** Windows（HKCU Run 機碼）目前是否已登記；查不到為 null。 */
  registered: boolean | null;
}

/** Runtime 生命週期（對應 Rust 的 LifecycleStatus）。 */
export interface LifecycleStatus {
  phase: LifecyclePhase;
  stop_intent_active: boolean;
  timing: { stop_grace_ms: number; kill_confirm_ms: number };
  exit: ExitReport | null;
  active_runs: number;
  login_startup: LoginStartupView;
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
  /** 變更「跨登入自動續跑」；只影響之後的登入，不立即啟停目前的工作。 */
  updateWorkspaceLoginResume(workspaceId: string, autoResumeAcrossLogin: boolean): Promise<WorkspaceLifecycleView>;
  /** 變更「登入 Windows 時自動啟動」；Windows 登記失敗時拋出錯誤，偏好不變。 */
  updateLoginStartup(enabled: boolean): Promise<LoginStartupView>;
  /** 結束桌面 App 程序。 */
  quitApp(): Promise<void>;
  /** 取消殼在完全退出時啟動的 60 秒後備強制關閉；Runtime 已接手停止或使用者選擇留在 App 時呼叫。 */
  cancelFullExitFallback(): Promise<void>;
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

/** 備份的觸發原因（對應 Rust 的 BackupTrigger）。 */
export type BackupTrigger = "daily" | "pre_migration" | "milestone" | "pre_update";

/** 備份被保留的原因（對應 Rust 的 RetentionReason）。 */
export type RetentionReason = "daily" | "weekly" | "latest_pre_migration";

/** 一份備份的摘要（對應 Rust 的 BackupSummary）。 */
export interface BackupSummary {
  id: string;
  trigger: BackupTrigger;
  created_at: number;
  artifact_count: number;
  retained_as: RetentionReason[];
}

export type DrillCheckKind = "restore" | "database_integrity" | "foreign_keys" | "artifact_hashes" | "artifact_relations";

/** 還原演練的一項核對；`detail` 是 Runtime 產生的說明。 */
export interface DrillCheck {
  kind: DrillCheckKind;
  passed: boolean;
  detail: string;
}

/** 一次還原演練的結果（對應 Rust 的 DrillReport）。 */
export interface DrillReport {
  backup_id: string | null;
  backup_created_at: number | null;
  performed_at: number;
  passed: boolean;
  checks: DrillCheck[];
  /** 還原時被標為中斷的 Run 數；還原後不會自動重做。 */
  interrupted_runs: number;
}

/** 備份狀態（對應 Rust 的 BackupStatus）；`backups` 由新到舊。 */
export interface BackupStatus {
  backups: BackupSummary[];
  latest_drill: DrillReport | null;
}

/** 備份與還原演練：唯讀查詢 Runtime 回報的狀態。 */
export interface BackupClient {
  backupStatus(): Promise<BackupStatus>;
}

/** 更新檢查的結果：尚未檢查、已是最新、有新版本、這個版本沒有設定更新來源、檢查失敗。 */
export type UpdateCheckStatus = "not_checked" | "up_to_date" | "available" | "unconfigured" | "failed";

export type MaintenanceKind = "update" | "uninstall";

/** 維護階段：停止中（與完全退出相同的停止流程）、準備套用（停止與更新前備份都完成）、失敗（不套用）。 */
export type MaintenancePhase = "stopping" | "ready" | "failed";

/** 可更新的版本（對應 Rust 的 AvailableUpdateView）。 */
export interface AvailableUpdate {
  version: string;
  notes: string | null;
  published_at: string | null;
}

/** 進行中的維護（對應 Rust 的 Maintenance）；時間是 UTC 毫秒。 */
export interface Maintenance {
  kind: MaintenanceKind;
  phase: MaintenancePhase;
  requested_at: number;
  from_version: string;
  /** 不經 App、由安裝程式直接要求的更新不知道目標版本。 */
  target_version: string | null;
  package_path: string | null;
  backup_id: string | null;
  /** 失敗原因或附註。 */
  message: string | null;
  completed_at: number | null;
}

/** 最近一次結束的維護（對應 Rust 的 MaintenanceOutcome）。 */
export interface MaintenanceOutcome {
  kind: MaintenanceKind;
  from_version: string;
  /** 結束維護時（重新開啟後）執行中的版本。 */
  running_version: string;
  target_version: string | null;
  /** 更新是否已套用；解除安裝後又重新安裝時為 null。 */
  applied: boolean | null;
  requested_at: number;
  finished_at: number;
  backup_id: string | null;
  message: string | null;
}

/** 更新狀態（對應 Rust 的 UpdateStatusView）：檢查結果（只通知）與維護。 */
export interface UpdateStatusView {
  current_version: string;
  status: UpdateCheckStatus;
  available: AvailableUpdate | null;
  checked_at: number | null;
  next_check_at: number | null;
  error: string | null;
  maintenance: Maintenance | null;
  last_maintenance: MaintenanceOutcome | null;
}

/** 更新（spec D-19）：Runtime 只檢查與通知，使用者同意後才下載、停止並由桌面殼安裝。 */
export interface UpdateClient {
  updateStatus(): Promise<UpdateStatusView>;
  /** 使用者同意更新到 `version`：先下載並驗證安裝程式（失敗時拋出錯誤，工作沒有停止），通過才開始維護。 */
  applyUpdate(version: string): Promise<UpdateStatusView>;
  /** 維護「準備套用」後由桌面殼等 Runtime 結束、執行安裝程式並結束 App；未準備好時拋出錯誤。 */
  installUpdate(): Promise<void>;
}

/** 對話輸入的種類：使用者訊息、工作事件（Worker 成果或阻礙）、Controller 的回覆。 */
export type ConversationTurnKind = "user_message" | "work_event" | "controller_reply";

/** 輸入的處理狀態：已收到待處理、某一輪 Controller 正在處理、已處理。 */
export type ConversationTurnStatus = "pending" | "processing" | "processed";

/** conversation_turns 的一筆；時間是 UTC 毫秒。 */
export interface ConversationTurn {
  id: string;
  conversation_id: string;
  seq: number;
  kind: ConversationTurnKind;
  content: string;
  status: ConversationTurnStatus;
  controller_run_id: string | null;
  /** 交付給 Controller 的次數；大於 1 表示上一輪中斷後重新交付。 */
  attempts: number;
  source: string | null;
  created_at: number;
  processed_at: number | null;
}

export type ControllerRunStatus = "starting" | "running" | "succeeded" | "failed" | "interrupted";

/** 這一輪是 Controller 換帳號後的第一輪：新的 Controller Session 從保存的對話、決策與檢查點重建（票 30）。 */
export interface ControllerRunHandoff {
  from_run_id: string | null;
  from_profile_id: string;
  /** "quota_draining"、"quota_near_limit" 或 "account_unavailable"。 */
  reason: string;
}

/** Controller 有到期的待處理輸入但沒有可用帳號；訊息已保存、等待中。 */
export interface ControllerWait {
  /** 例如 "quota_draining"、"quota_near_limit"、"profile_unverified"、"no_account"。 */
  code: string;
  /** 給使用者看的說明；多個原因以「；」串接。 */
  message: string;
  /** 開始等待的時間（UTC 毫秒）。 */
  since: number;
}

/** Controller 的一輪（一次 `claude -p`）；時間是 UTC 毫秒。 */
export interface ControllerRun {
  id: string;
  conversation_id: string;
  workspace_id: string;
  profile_id: string;
  status: ControllerRunStatus;
  version: number;
  command_id: string;
  prompt: string;
  working_dir: string;
  model: string | null;
  input_turn_ids: string[];
  agent_session_id: string;
  resume_native_session_id: string | null;
  native_session_id: string | null;
  process: { pid: number; created_at_millis: number; command_hash: string } | null;
  end_reason: string | null;
  /** 換帳號後的第一輪才有，其餘是 null。 */
  handoff: ControllerRunHandoff | null;
  created_at: number;
  updated_at: number;
  started_at: number | null;
  ended_at: number | null;
}

/** Workspace 的 Controller 對話（對應 GET /conversation）。 */
export interface ConversationView {
  /** 還沒有任何訊息時是 null。 */
  conversation_id: string | null;
  /** 有一輪 Controller 正在進行。 */
  busy: boolean;
  active_run: ControllerRun | null;
  /** 不忙碌且有待處理輸入時，預計喚醒 Controller 的時間（UTC 毫秒）。 */
  next_wake_at: number | null;
  /** 有到期的待處理輸入但沒有可用帳號時的等待原因；忙碌時一定是 null。 */
  waiting: ControllerWait | null;
  /** 依 seq 由小到大。 */
  turns: ConversationTurn[];
  /** 全部 Controller Run，依建立順序。 */
  runs: ControllerRun[];
}

export interface SendMessageResponse {
  turn: ConversationTurn;
  busy: boolean;
}

/** Workspace 的 Controller 長期對話（只有桌面憑證能用）。 */
export interface ConversationClient {
  getConversation(workspaceId: string): Promise<ConversationView>;
  /** 空白（message_empty）、過長（message_too_long）或 Workspace 已封存（workspace_archived）會被拒。 */
  sendConversationMessage(workspaceId: string, text: string): Promise<SendMessageResponse>;
}

/** 互動式 Controller CLI Run 的狀態：開啟中、執行中、已結束、中斷。 */
export type ControllerCliStatus = "starting" | "running" | "ended" | "interrupted";

/** 互動式 Controller CLI 的一次 Run（對應 Rust 的 ControllerCliRun）；時間是 UTC 毫秒。 */
export interface ControllerCliRun {
  id: string;
  workspace_id: string;
  /** Controller 帳號。 */
  profile_id: string;
  status: ControllerCliStatus;
  version: number;
  command_id: string;
  working_dir: string;
  settings_version: number | null;
  /** 使用者要求開啟的時間。 */
  opened_at: number;
  launched_at: number | null;
  /** 宿主程序身分（開窗回執）。 */
  host: ProcessRecord | null;
  claude_pid: number | null;
  running_at: number | null;
  /** 使用者按「結束」的時間；Runtime 結束宿主後才進入 ended。 */
  end_requested_at: number | null;
  ended_at: number | null;
  /** 例如 "host_exited"、"user_ended"、"claude_not_found"、"terminal_failed"。 */
  end_code: string | null;
  /** 給使用者看的結束原因。 */
  end_reason: string | null;
  exit_code: number | null;
  /** 同帳號重新開啟、以 `--continue` 接續上次的原生 Session。 */
  continue_session: boolean;
  /** Runtime 重啟後查驗宿主仍存活、重新綁定的時間；有值時這個 Controller CLI 的平台工具已失效，要重新開啟。 */
  rebound_at: number | null;
}

/** Controller 頁簽的狀態（對應 GET /controller-cli）：最近一次的 Run（含已結束），從未開啟過為 null。 */
export interface ControllerCliView {
  workspace_id: string;
  run: ControllerCliRun | null;
  /** 目前 App 內建 Controller 操作手冊的版本號；永遠存在（即使從未開啟過）。 */
  playbook_version: string;
}

/** Controller 頁簽：開啟／結束外部終端機中的互動式 Controller CLI（只有桌面憑證能用）。 */
export interface ControllerCliClient {
  getControllerCli(workspaceId: string): Promise<ControllerCliView>;
  /** 已有開著的（controller_cli_already_open）、Profile 不可用（profile_unavailable）或 Workspace 已封存會被拒。 */
  openControllerCli(workspaceId: string, profileId: string): Promise<ControllerCliView>;
  /** 沒有開著的 Controller CLI 時被拒（controller_cli_not_open）。 */
  endControllerCli(workspaceId: string): Promise<ControllerCliView>;
}

/** 恢復事件的狀態：等待（第一次立即、之後為退避）、嘗試中、穩定觀察中、已恢復、恢復額度用盡。 */
export type RecoveryStatus = "waiting" | "attempting" | "observing" | "succeeded" | "exhausted";

export type AttemptOutcome = "in_progress" | "observing" | "succeeded" | "failed";

/** 對受影響 Run 的處置：重新排隊、完成取消、無法確認舊程序停止。 */
export type RunDisposition = "requeued" | "cancelled" | "unconfirmed";

/** 受影響的工作（對應 Rust 的 AffectedRun）。 */
export interface AffectedRun {
  run_id: string;
  workspace_id: string;
  task_id: string;
  disposition: RunDisposition;
  message: string | null;
}

/** 一次恢復嘗試；`(round, seq)` 在同一事件中唯一。時間是 UTC 毫秒。 */
export interface RecoveryAttempt {
  id: string;
  round: number;
  seq: number;
  started_at: number;
  finished_at: number | null;
  outcome: AttemptOutcome;
  failure: string | null;
  affected: AffectedRun[];
}

/** 恢復事件（對應 Rust 的 RecoveryEvent）；`attempts` 含所有輪次，依開始順序。 */
export interface RecoveryEvent {
  id: string;
  /** 從 1 開始；每次「重新嘗試恢復」加一。 */
  round: number;
  status: RecoveryStatus;
  /** 本輪已用掉的嘗試次數。 */
  attempts_used: number;
  detected_at: number;
  /** 退避期限：只有 `waiting` 有值。 */
  next_attempt_at: number | null;
  /** 穩定觀察的期滿時間：只有 `observing` 有值。 */
  observe_until: number | null;
  last_failure: string | null;
  closed_at: number | null;
  version: number;
  updated_at: number;
  attempts: RecoveryAttempt[];
}

/** 崩潰恢復總覽（對應 Rust 的 RecoveryStatusView）。 */
export interface RecoveryStatusView {
  /** 從未發生過崩潰為 null。 */
  event: RecoveryEvent | null;
  max_attempts: number;
  /** 恢復額度用盡：可以「重新嘗試恢復」。 */
  can_retry: boolean;
  /** 不為 null 時派工被擋。 */
  blocking: WaitReason | null;
}

/** 崩潰恢復：查詢恢復事件與「重新嘗試恢復」（只在恢復額度用盡時被接受）。 */
export interface RecoveryClient {
  recoveryStatus(): Promise<RecoveryStatusView>;
  retryRecovery(): Promise<RecoveryStatusView>;
}

/** 時間線事件的類別：派工、交接、裁定、恢復、驗證、清理、工作狀態、外部操作、教訓重犯、用量。 */
export type TimelineCategory =
  | "dispatch"
  | "handoff"
  | "decision"
  | "recovery"
  | "verification"
  | "cleanup"
  | "task"
  | "external_operation"
  | "lesson"
  | "usage"
  | "controller";

/** 時間線上的一個事件（對應 Rust 的 TimelineEntry）；時間是 UTC 毫秒。 */
export interface TimelineEvent {
  seq: number;
  occurred_at: number;
  category: TimelineCategory;
  aggregate_type: string;
  aggregate_id: string;
  event_type: string;
  /** 事件相關的 Task；可能是空陣列。 */
  task_ids: string[];
  run_id: string | null;
  payload: unknown;
}

/** Workspace 的時間線；`events` 依 `seq` 遞增。`task_id` 是過濾條件，未過濾為 null。 */
export interface TimelineResponse {
  workspace_id: string;
  task_id: string | null;
  events: TimelineEvent[];
}

/** 比率；`rate` 為 null 代表分母為 0（沒有資料）。 */
export interface MetricRatio {
  numerator: number;
  denominator: number;
  rate: number | null;
}

/** 耗時統計；沒有已完成的樣本時平均與最大值為 null。 */
export interface DurationStats {
  count: number;
  /** 尚未結束（仍在等待）的件數。 */
  pending: number;
  average_ms: number | null;
  max_ms: number | null;
}

export interface TaskCompletionMetrics {
  completion: MetricRatio;
  cancelled: number;
}

export interface HandoffMetrics {
  success: MetricRatio;
  abandoned: number;
  confirmed: number;
  in_flight: number;
}

export interface DuplicateOperationMetrics {
  blocked: number;
  already_succeeded: number;
  result_unknown: number;
  in_progress_elsewhere: number;
  unknown_outcomes: number;
}

export interface LessonRecurrenceMetrics {
  rate: MetricRatio;
  recurrences: number;
  prevented: number;
  reported: number;
}

/** 一個階段的用量；`stage` 為 null 代表未分階段。 */
export interface StageUsage {
  stage: string | null;
  tokens: number;
  runs: number;
}

export interface UsageMetrics {
  total_tokens: number;
  runs: number;
  by_stage: StageUsage[];
}

export interface ContextMetrics {
  runs_known: number;
  runs_unknown: number;
  average_tokens: number | null;
  max_tokens: number | null;
}

/** 觀測指標（對應 Rust 的 Metrics）：全部由時間線事件計算而來。 */
export interface Metrics {
  events: number;
  task_completion: TaskCompletionMetrics;
  handoff: HandoffMetrics;
  recovery: DurationStats;
  duplicate_operations: DuplicateOperationMetrics;
  decision_wait: DurationStats;
  lesson_recurrence: LessonRecurrenceMetrics;
  usage: UsageMetrics;
  context: ContextMetrics;
}

/** 帳號的服務端額度（Claude Code 回報的帳號層級已用比例 0–1）；null 代表看不到。 */
export interface ServerQuota {
  profile_id: string;
  name: string;
  five_hour_used: number | null;
  seven_day_used: number | null;
}

export interface MetricsResponse {
  workspace_id: string;
  metrics: Metrics;
  server_quota: ServerQuota[];
}

/** 時間線與觀測指標（只有桌面憑證能用）。 */
export interface ObservabilityClient {
  /** `taskId` 指定時由 Runtime 只回傳該 Task 的事件。 */
  timeline(workspaceId: string, taskId?: string): Promise<TimelineResponse>;
  metrics(workspaceId: string): Promise<MetricsResponse>;
}

/** 專案理解的三種文件：Project Brief、Glossary、Project Map。 */
export type UnderstandingSection = "brief" | "glossary" | "map";

/** 未確認的關係是假說；retired 是已移除的項目。 */
export type UnderstandingStatus = "confirmed" | "hypothesis" | "unknown" | "retired";

/** 描述的來源：repo 內的檔案（版本是查驗時內容的 SHA-256），或外部參考。 */
export type UnderstandingSource =
  | {
      kind: "file";
      path: string;
      line: number | null;
      version: string;
      checked_at: number;
      /** 檔案內容已與查驗時不同或已不存在。 */
      changed: boolean;
    }
  | {
      kind: "external";
      reference: string;
      version: string;
      checked_at: number;
      /** 外部參考無法比對，一律 false。 */
      changed: boolean;
    };

export interface UnderstandingItemView {
  id: string;
  workspace_id: string;
  section: UnderstandingSection;
  key: string;
  text: string;
  status: UnderstandingStatus;
  sources: UnderstandingSource[];
  revision: number;
  recorded_by: string;
  created_at: number;
  updated_at: number;
  /** 已確認，但有檔案來源在查驗後改變：需要重新查驗。 */
  stale: boolean;
}

export type ProjectionKind = "glossary" | "adr";

/** conflict：外部修改了投影檔，等待使用者協調；detached：使用者保留外部版本，Baton 停止寫入。 */
export type ProjectionStatus = "in_sync" | "conflict" | "detached";

/** repo 內 GLOSSARY.md 或 docs/adr/ 的帶版本輸出投影。 */
export interface Projection {
  workspace_id: string;
  path: string;
  kind: ProjectionKind;
  version: number;
  body_hash: string;
  baseline_hash: string | null;
  status: ProjectionStatus;
  /** 協調中或已保留外部版本時，最後一次比對讀到的檔案雜湊（覆寫時送回核對）；檔案被刪除時是 null。 */
  external_hash: string | null;
  detected_at: number | null;
  revision: number;
  updated_at: number;
}

export interface UnderstandingView {
  workspace_id: string;
  /** Workspace 是 Git repo 時才會投影到 repo 內。 */
  projection_enabled: boolean;
  /** 依 section、key 排序，含 retired。 */
  items: UnderstandingItemView[];
  /** 依 path 排序。 */
  projections: Projection[];
}

/** 協調投影：以 Baton 版本覆寫，或保留外部版本。`expected_external_hash` 是使用者看到的那份外部版本。 */
export interface ReconcileProjectionRequest {
  path: string;
  action: "overwrite" | "keep_external";
  expected_external_hash: string | null;
}

/** 專案理解與 repo 投影協調（只有桌面憑證能用）。 */
export interface UnderstandingClient {
  understanding(workspaceId: string): Promise<UnderstandingView>;
  /** 重新比對投影檔（偵測外部編輯）後回傳。 */
  syncUnderstanding(workspaceId: string): Promise<UnderstandingView>;
  reconcileProjection(workspaceId: string, request: ReconcileProjectionRequest): Promise<UnderstandingView>;
}

/** Router 判斷的工作情境；ambiguous 是情境不明時先釐清。 */
export type WorkflowSituation =
  | "ambiguous"
  | "feature"
  | "defect"
  | "migration"
  | "performance"
  | "investigation"
  | "exploratory_testing";

export type WorkflowStatus = "active" | "blocked" | "completed";

/** decision：裁定 Gate；safety：安全 Gate。兩者都不能跳過。 */
export type StageGate = "none" | "decision" | "safety";

/** invalidated：先前通過，但依賴（規格、build、Verification Profile、被推翻的假設）改變而失效，等待重驗。 */
export type StageState = "pending" | "current" | "blocked" | "passed" | "skipped" | "invalidated";

/** 離開條件需要的證據種類。 */
export type EvidenceKind = "artifact" | "decision" | "task_accepted";

export interface ExitCheck {
  key: string;
  description: string;
  requires: EvidenceKind;
}

/** Stage 未通過時的去向：重試、回到某個 Stage，或阻擋 Workflow。 */
export type FailureRoute = { kind: "retry" } | { kind: "return_to"; stage: string } | { kind: "block" };

/** invalidated：Runtime 記錄的失效紀錄，missing 是失效原因。 */
export type StageAttemptOutcome = "passed" | "incomplete" | "blocked" | "failed" | "skipped" | "invalidated";

/** 一項證據與 Runtime 的查驗結果。 */
export interface StageEvidence {
  check: string;
  kind: EvidenceKind;
  artifact_id?: string;
  decision_request_id?: string;
  task_id?: string;
  /** build_changed：Evidence Bundle 驗證的 build 已不是目前的 revision。 */
  status: "valid" | "not_found" | "pending" | "not_accepted" | "build_changed";
}

export interface MissingEvidence {
  check: string;
  reason: string;
}

export interface StageAttempt {
  id: string;
  workflow_id: string;
  stage_key: string;
  attempt: number;
  outcome: StageAttemptOutcome;
  evidence: StageEvidence[];
  missing: MissingEvidence[];
  note: string;
  /** Controller 的 id、使用者在桌面操作時的 "desktop"，或 Runtime 記錄失效時的 "runtime"。 */
  recorded_by: string;
  profile_id: string | null;
  /** 通過當下 Runtime 觀察到的受驗目錄 revision；沒有時省略。 */
  build?: string;
  created_at: number;
}

export interface StageView {
  key: string;
  title: string;
  method?: string;
  entry_conditions: string[];
  expected_outputs: string[];
  allowed_actions: string[];
  budget: { max_attempts: number };
  exit_checks: ExitCheck[];
  failure_route: FailureRoute;
  gate: StageGate;
  state: StageState;
  /** 依 attempt 遞增。 */
  attempts: StageAttempt[];
}

export interface WorkflowView {
  id: string;
  workspace_id: string;
  goal_id: string;
  task_id: string | null;
  task_title: string | null;
  playbook_version_id: string;
  playbook_key: string;
  playbook_version: number;
  playbook_title: string;
  /** 這個 Playbook 目前的最新版本；大於 playbook_version 時 Workflow 仍維持原版本。 */
  latest_playbook_version: number;
  situation: WorkflowSituation;
  route_reason: string;
  status: WorkflowStatus;
  /** 完成後為 null。 */
  current_stage: string | null;
  blocked_reason: string | null;
  /** Gate 失效使 Workflow 退回較早的 Stage 時，失效前的目前 Stage；否則為 null。 */
  resume_stage: string | null;
  /** 跳過 Stage 時以 expected_revision 帶回核對。 */
  revision: number;
  created_by: string;
  created_at: number;
  updated_at: number;
  /** 依 Playbook 順序。 */
  stages: StageView[];
}

export interface WorkflowList {
  workspace_id: string;
  /** 依建立時間，最新的在前。 */
  workflows: WorkflowView[];
}

/** 使用者跳過目前 Stage；原因不能空白。 */
export interface SkipStageRequest {
  stage: string;
  expected_revision: number;
  reason: string;
}

/** Workflow 檢視與跳過 Stage（只有桌面憑證能用）。 */
export interface WorkflowClient {
  workflows(workspaceId: string): Promise<WorkflowList>;
  skipStage(workspaceId: string, workflowId: string, request: SkipStageRequest): Promise<WorkflowView>;
}

/** 審查的兩軸：standards 是規範（程式碼品質、專案規則與慣例），spec 是需求（是否滿足被審 Task 的契約）。 */
export type ReviewAxis = "standards" | "spec";

/** finding 的處理狀態：open 尚未分類；fix 要修正後重新審查；awaiting_decision 等待使用者裁定；dismissed 使用者同意駁回。 */
export type FindingState = "open" | "fix" | "awaiting_decision" | "dismissed";

export type FindingClassification = "fix" | "dismiss" | "needs_decision";

export interface ReviewFinding {
  id: string;
  review_id: string;
  axis: ReviewAxis;
  title: string;
  detail: string;
  location: string | null;
  state: FindingState;
  classification: FindingClassification | null;
  /** Controller 分類的理由與證據說明。 */
  reason: string | null;
  decision_request_id: string | null;
  /** 裁定者；使用者在桌面裁定時是 "desktop"。 */
  decided_by: string | null;
  classified_by: string | null;
  version: number;
  created_at: number;
  updated_at: number;
}

/** 一軸的結果與它的 finding。 */
export interface ReviewAxisView {
  summary: string;
  paths: string[];
  /** 沿用自哪一份審查。 */
  reused_from: string | null;
  reviewer_task_id: string;
  /** 沿用，或 finding 全部經使用者同意駁回。 */
  passed: boolean;
  findings: ReviewFinding[];
}

export interface ReviewView {
  id: string;
  workspace_id: string;
  subject_task_id: string;
  subject_task_title: string | null;
  reviewer_run_id: string;
  reviewer_task_id: string;
  reviewer_task_title: string | null;
  base_revision: string;
  candidate_revision: string;
  /** 對照的規格版本（被審 Task 的契約版本）。 */
  spec_version: number;
  standards_source: string;
  standards: ReviewAxisView;
  spec: ReviewAxisView;
  /** 兩軸都通過。 */
  passed: boolean;
  /** 已過期的原因；仍有效時為 null。 */
  stale_reason: string | null;
  created_at: number;
}

export type DeviationStatus = "proposed" | "awaiting_decision" | "accepted" | "rejected";

/** 實作偏離 Design Package 的回報，附設計成果與 Task 的名稱。 */
export interface DeviationView {
  id: string;
  workspace_id: string;
  task_id: string;
  reported_by_run_id: string;
  reported_generation: number;
  design_artifact_id: string;
  design_version: number;
  deviation: string;
  reason: string;
  affected_contracts: string[];
  proposed_design: string;
  status: DeviationStatus;
  /** Controller 處理時的理由。 */
  resolution_reason: string | null;
  decision_request_id: string | null;
  /** 接受者；使用者在桌面裁定時是 "desktop"。 */
  accepted_by: string | null;
  resolved_by: string | null;
  /** 接受後設計的新版本。 */
  accepted_design_version: number | null;
  version: number;
  created_at: number;
  updated_at: number;
  design_name: string | null;
  task_title: string | null;
}

export interface ReviewList {
  workspace_id: string;
  /** 最新的在前。 */
  reviews: ReviewView[];
  /** 最新的在前。 */
  deviations: DeviationView[];
}

/** 審查與設計偏離（唯讀，只有桌面憑證能用）。 */
export interface ReviewClient {
  reviews(workspaceId: string): Promise<ReviewList>;
}

/** 驗證的一個操作（對應 Rust 的 Operation）：Adapter 只執行並回報觀察，不做判定。 */
export type VerificationOperation =
  | { kind: "command"; program: string; args?: string[]; timeout_ms?: number }
  | {
      kind: "http";
      method: string;
      url: string;
      headers?: { name: string; value: string }[];
      body?: string;
      timeout_ms?: number;
    }
  /** 瀏覽器操作（只用於 web 介面）；之後觀察當前畫面。 */
  | { kind: "browser"; action: VerificationBrowserAction; timeout_ms?: number }
  /** 需要人工判斷的步驟；CLI／API Adapter 判定為 automation_unsupported。 */
  | { kind: "manual"; instruction: string };

/** 瀏覽器操作（對應 Rust 的 BrowserAction）；fill 的 text 可以用 `{{secret:KEY}}` 引用秘密。 */
export type VerificationBrowserAction =
  | { kind: "goto"; url: string }
  | { kind: "click"; selector: string }
  | { kind: "fill"; selector: string; text: string }
  | { kind: "press"; selector: string; key: string }
  | { kind: "wait_for"; selector: string }
  /**
   * 目標導向動作（票 48）：模型依當前畫面、目標與參數決定一個具體動作；只能用在旅程步驟，至少要有一個不靠模型的斷言。
   * replay_in_production：Profile 宣告為正式環境時仍允許重播這個步驟的錄製。
   */
  | { kind: "act"; goal: string; params?: Record<string, string>; replay_in_production?: boolean };

/** 程式斷言（對應 Rust 的 Assertion）。 */
export type VerificationAssertion =
  | { kind: "exit_code"; equals: number }
  | { kind: "status"; equals: number }
  | { kind: "contains"; text: string }
  | { kind: "not_contains"; text: string }
  | { kind: "json_equals"; pointer: string; value: unknown }
  /** 瀏覽器：selector 對應的元素在當前畫面上可見。 */
  | { kind: "visible"; selector: string }
  /** 瀏覽器：當前網址包含這段文字。 */
  | { kind: "url_contains"; text: string }
  /** 瀏覽器：受限的語意 judge，只看當前畫面與這段固定預期。 */
  | { kind: "judge"; expectation: string };

export interface VerificationStep {
  key: string;
  title: string;
  operation: VerificationOperation;
  assertions: VerificationAssertion[];
  /** 只用於旅程步驟：操作的副作用在本次驗證的隔離狀態之外，結果未知時不重送。 */
  external?: boolean;
}

export interface VerificationJourney {
  key: string;
  title: string;
  /** 代表旅程：新 Profile 版本要實際跑過並通過它才算可用。 */
  representative: boolean;
  /** 扮演的角色（Profile 的 personas 的 key）。 */
  persona?: string;
  /** 實際登入的身分（Profile 的 identities 的 key）；省略時不登入。 */
  identity?: string;
  steps: VerificationStep[];
}

/** 秘密參照（對應 Rust 的 SecretRef）：reference 形如 `env:NAME`，只是參照，不含值。 */
export interface VerificationSecretRef {
  key: string;
  reference: string;
}

/** 旅程扮演的角色（對應 Rust 的 Persona）。 */
export interface VerificationPersona {
  key: string;
  title: string;
  description: string;
}

/** 登入身分（對應 Rust 的 Identity）：login 是登入步驟，check 核對實際登入的身分。 */
export interface VerificationIdentity {
  key: string;
  title: string;
  login: VerificationStep[];
  check: VerificationStep;
}

/** Feature Map 的一項；journeys 空的就是未覆蓋。 */
export interface VerificationFeature {
  key: string;
  title: string;
  /** 驗收準則。 */
  criterion: string;
  journeys: string[];
}

/** Verification Profile 的內容（對應 Rust 的 ProfileDefinition）。 */
export interface VerificationProfileDefinition {
  interface: "cli_api" | "web";
  build?: { program: string; args: string[] };
  doctor: VerificationStep[];
  start?: { program: string; args: string[] };
  readiness?: { step: VerificationStep; attempts: number; interval_ms: number };
  setup: VerificationStep[];
  journeys: VerificationJourney[];
  teardown: VerificationOperation[];
  evidence: {
    output_max_chars: number;
    redact: string[];
    /** 瀏覽器截圖的保存時機；省略時為 on_failure。 */
    screenshots?: "on_failure" | "every_step" | "off";
    trace?: boolean;
    video?: boolean;
    /** 截圖時遮蔽的 selector。 */
    mask?: string[];
  };
  budget: { max_operations: number; max_model_calls?: number };
  feature_map: VerificationFeature[];
  /** 明確宣告不在自動驗證範圍內的部分。 */
  uncovered: { area: string; reason: string }[];
  secrets?: VerificationSecretRef[];
  personas?: VerificationPersona[];
  identities?: VerificationIdentity[];
  /** 錄製與重播的政策（票 48）：production 時錄製預設不重播；dataset 是資料集版本，改變時之前的錄製都是 stale。 */
  replay?: { production?: boolean; dataset?: string };
}

/** Profile 的一個不可變版本（對應 Rust 的 ProfileVersion）。 */
export interface VerificationProfileVersion {
  id: string;
  workspace_id: string;
  version: number;
  definition: VerificationProfileDefinition;
  content_hash: string;
  /** 發布者：桌面憑證的指紋。 */
  published_by: string;
  created_at: number;
}

/** 一項未覆蓋的範圍；明確宣告的範圍沒有 feature。 */
export interface UncoveredItem {
  feature?: string;
  area: string;
  reason: string;
}

/** 七種判定；只有 passed 是通過。 */
export type VerificationVerdict =
  | "passed"
  | "product_failure"
  | "inconclusive"
  | "environment_blocked"
  | "auth_blocked"
  | "budget_exhausted"
  | "automation_unsupported";

/** login 與 identity 屬於某條旅程（步驟的 journey 有值）。 */
export type VerificationPhase =
  | "build"
  | "doctor"
  | "start"
  | "readiness"
  | "setup"
  | "login"
  | "identity"
  | "journey"
  | "teardown";

export interface VerificationAssertionResult {
  assertion: VerificationAssertion;
  passed: boolean;
  /** 實際觀察到的值（已遮蔽）。 */
  actual: string;
}

/** 一個步驟的紀錄（對應 Rust 的 StepRecord）。 */
export interface VerificationStepRecord {
  phase: VerificationPhase;
  journey?: string;
  key: string;
  title: string;
  /** 操作描述（已遮蔽）。 */
  operation: string;
  verdict: VerificationVerdict;
  /** 更細的機器可讀原因，例如 assertion_failed。 */
  reason_code: string;
  /** 來源引擎的原始代碼，例如 exit:1、http:404、timeout。 */
  engine_code?: string;
  assertions: VerificationAssertionResult[];
  /** 輸出或回應本文的結尾（已遮蔽）。 */
  output: string;
  /** 實際執行的次數（就緒條件可能重試）。 */
  attempts: number;
}

export interface VerificationJourneyResult {
  key: string;
  title: string;
  representative: boolean;
  /** 扮演的角色；與 identity（實際登入的帳號）分開。 */
  persona?: string;
  /** 實際登入的身分；省略時沒有登入。 */
  identity?: string;
  verdict: VerificationVerdict;
  reason_code: string;
}

/** 清理結果：只移除本次驗證的狀態，證據保留。 */
export interface VerificationCleanup {
  state_dir: string;
  state_dir_removed: boolean;
  /** 沒有啟動服務時省略。 */
  service_stopped?: boolean;
  problems: string[];
}

/** 驗證的證據集合（對應 Rust 的 EvidenceBundle）：綁定 build、candidate revision、需求與 Profile 版本。 */
export interface EvidenceBundle {
  id: string;
  workspace_id: string;
  request_id: string;
  task_id: string | null;
  run_id: string | null;
  generation: number | null;
  /** 受驗 Task 驗證當時的契約版本。 */
  requirement_version: number | null;
  profile_version_id: string;
  profile_version: number;
  candidate_revision: string | null;
  build: string | null;
  subject_dir: string;
  journeys: VerificationJourneyResult[];
  steps: VerificationStepRecord[];
  verdict: VerificationVerdict;
  reason_code: string;
  covered_features: string[];
  uncovered: UncoveredItem[];
  cleanup: VerificationCleanup;
  /** 截圖、trace 與錄影；依 bundle 中的位置（從 0 起）以 evidenceArtifact 讀取內容。 */
  artifacts: EvidenceArtifact[];
  /** 這次驗證禁止即時模型呼叫（票 48）；只有 true 時出現。 */
  no_live_model?: boolean;
  /** 目標導向動作的來源與錄製的處理（票 48）；空的時候省略。 */
  actions?: ActionRecord[];
  /** 即時模型與重播分開計算的用量（票 48）；舊資料可能沒有，視為 0。 */
  usage?: VerificationUsage;
  started_at: number;
  finished_at: number;
  /** 同一個驗證要求的每次嘗試（帳號交接或 Runtime 中斷後由新的嘗試接手）；舊的 Bundle 沒有這個欄位。 */
  attempts?: VerificationAttempt[];
}

/** 一次嘗試完成的步驟與判定（對應 Rust 的 RecordedStep）。 */
export interface VerificationAttemptStep {
  phase: VerificationPhase;
  journey?: string;
  key: string;
  verdict: VerificationVerdict;
  reason_code: string;
}

/** 沒有重送的外部操作（對應 Rust 的 NotResent）；reason 是 external_outcome_unknown 或 external_already_sent。 */
export interface VerificationNotResent {
  journey: string;
  step: string;
  reason: string;
  /** 送出這個操作的嘗試。 */
  attempt: number;
}

/** 承接（對應 Rust 的 Takeover）：以安全的參照查驗並停止前一次嘗試留下的測試程序。 */
export interface VerificationTakeover {
  from_attempt: number;
  stopped_services: string[];
  closed_sessions: string[];
  /** 無法確認已停止的原因。 */
  problems: string[];
  /** 沿用先前嘗試已完成的旅程。 */
  reused_journeys: string[];
  not_resent: VerificationNotResent[];
}

/** 驗證要求的一次嘗試（對應 Rust 的 AttemptSummary）；running 時 ended_at 為 null。 */
export interface VerificationAttempt {
  attempt: number;
  status: "running" | "handed_off" | "interrupted" | "completed";
  /** 語意 judge 使用的帳號；null 表示沒有可用帳號，只執行不需模型的斷言。 */
  judge_profile_id: string | null;
  /** judge_rate_limited、runtime_interrupted 或 null。 */
  end_reason: string | null;
  /** Bundle 中結果採用自這次嘗試的旅程。 */
  journeys: string[];
  /** 這次完成的步驟（含交接前沒完成的旅程的步驟，這些不會拼進最後結果）。 */
  steps: VerificationAttemptStep[];
  takeover: VerificationTakeover | null;
  started_at: number;
  ended_at: number | null;
}

/** 錄製身分（對應 Rust 的 ReplayIdentity）：案例、目標、參數、persona、引擎與資料相容性。 */
export interface ReplayIdentity {
  case: string;
  goal: string;
  params: string;
  persona: string;
  engine: string;
  data: string;
}

/** 一個目標導向動作的紀錄（對應 Rust 的 ActionRecord）。 */
export interface ActionRecord {
  journey: string;
  step: string;
  goal: string;
  /** 執行前錄製的狀態。 */
  cache: "none" | "valid" | "stale" | "mismatch" | "production_not_replayable";
  /** 實際執行的動作來源；省略表示這次沒有執行任何動作。 */
  source?: "live" | "replay";
  /** 實際執行的具體動作描述（已遮蔽）。 */
  action?: string;
  /** 這次把錄製標為 stale 的原因，例如 engine_changed、postcondition_failed。 */
  stale_reasons?: string[];
  /** 這次錄製的動作；有值表示這次錄製了新的動作。 */
  recorded?: { identity: ReplayIdentity; action: VerificationBrowserAction };
}

/** 一次驗證的用量（對應 Rust 的 VerificationUsage）：即時模型與重播分開計算。 */
export interface VerificationUsage {
  /** 實際呼叫模型的次數（語意判斷與動作規劃）。 */
  live_model_calls: number;
  /** 由模型即時決定並執行的動作。 */
  live_actions: number;
  /** 重播錄製而執行的動作（沒有呼叫模型）。 */
  replayed_actions: number;
  /** no_live_model 擋下、沒有呼叫的模型呼叫。 */
  blocked_model_calls: number;
}

/** 證據 Artifact（對應 Rust 的 EvidenceArtifact）；沒有保存時 withheld 記下原因。 */
export interface EvidenceArtifact {
  kind: "screenshot" | "trace" | "video";
  journey?: string;
  /** 截圖所屬的步驟。 */
  step?: string;
  name: string;
  storage_path?: string;
  sha256?: string;
  size_bytes?: number;
  /** secret_detected、secret_input_recorded、too_large 或 storage_failed。 */
  withheld?: string;
}

/** 一個證據 Artifact 的內容（base64）。 */
export interface EvidenceArtifactContent {
  bundle_id: string;
  index: number;
  kind: EvidenceArtifact["kind"];
  name: string;
  content_type: string;
  size_bytes: number;
  content_base64: string;
}

/** 一次驗證的要求（對應 Rust 的 VerificationRequest）。 */
export interface VerificationRequest {
  id: string;
  workspace_id: string;
  task_id: string | null;
  profile_version_id: string;
  profile_version: number;
  journeys: string[];
  /** desktop 或 Controller 那一輪的 Run 識別。 */
  requested_by: string;
  status: "queued" | "completed";
  bundle_id: string | null;
  created_at: number;
  /** 不呼叫任何即時模型（票 48）。 */
  no_live_model: boolean;
}

/** Workspace 的驗證概況（對應 Rust 的 VerificationOverviewView）。 */
export interface VerificationOverview {
  workspace_id: string;
  /** 還沒有 Profile 時為 null。 */
  profile: VerificationProfileVersion | null;
  /** 證明最新版本可用的 Evidence Bundle；null 表示還沒有實際跑過並通過代表旅程。 */
  proven_by: string | null;
  uncovered: UncoveredItem[];
  /** 新的在前。 */
  requests: VerificationRequest[];
  /** 新的在前。 */
  bundles: EvidenceBundle[];
}

/** created 為 false 表示內容與最新版本相同，沒有新增版本。 */
export interface PublishProfileResponse {
  profile: VerificationProfileVersion;
  created: boolean;
}

/** 要求執行驗證；journeys 空的時候執行全部旅程。 */
export interface RunVerificationRequest {
  task_id?: string;
  journeys: string[];
  /** 這次驗證不呼叫任何即時模型（票 48）；只有 true 時才送。 */
  no_live_model?: boolean;
}

/** Verification Profile 與 CLI／API 驗證（Profile 只有使用者能從桌面發布）。 */
export interface VerificationProfileClient {
  verificationOverview(workspaceId: string): Promise<VerificationOverview>;
  /** definition 是使用者編輯的 JSON；內容由 Runtime 檢查，不正確時回 verification_profile_invalid。 */
  publishVerificationProfile(workspaceId: string, definition: unknown): Promise<PublishProfileResponse>;
  runVerification(workspaceId: string, request: RunVerificationRequest): Promise<VerificationRequest>;
  evidenceBundle(workspaceId: string, bundleId: string): Promise<EvidenceBundle>;
  /** index 是 bundle.artifacts 中的位置；沒有保存或找不到時回 evidence_artifact_not_found。 */
  evidenceArtifact(workspaceId: string, bundleId: string, index: number): Promise<EvidenceArtifactContent>;
}

/** 探索 Task 的一個章程版本（對應 Rust 的 CharterView）。 */
export interface ExplorationCharter {
  id: string;
  workspace_id: string;
  task_id: string;
  version: number;
  /** 要探索的區域。 */
  area: string;
  /** 扮演的使用者角色。 */
  role: string;
  goal: string;
  forbidden_side_effects: string[];
  /** 步數上限。 */
  max_steps: number;
  /** 成本上限：模型呼叫次數。 */
  max_model_calls: number;
  defined_by: string;
  created_at: number;
  task_title: string | null;
}

/** 疑點狀態：只有 confirmed、fixed、regressed 是經過重現確認的缺陷。 */
export type ExplorationFindingStatus = "suspected" | "confirmed" | "unverified_risk" | "excluded" | "fixed" | "regressed";

/** 排除原因；expected_design（預期設計）必須由使用者裁定。 */
export type ExclusionReason = "environment" | "seed_data" | "tool_limitation" | "expected_design";

export interface ExplorationExclusion {
  reason: ExclusionReason;
  rationale: string;
  /** 排除為預期設計時的裁定請求。 */
  decision_request_id?: string;
}

/** 確認缺陷時的重現案例。 */
export interface ReproductionCase {
  journey: VerificationJourney;
  request_id: string;
  /** 確認它的 Evidence Bundle。 */
  bundle_id: string;
  /** 失敗的步驟。 */
  failed_step?: string;
  confirmed_at: number;
}

export type ReproductionPurpose = "reproduce" | "verify_fix" | "regression";

export type ReproductionOutcome =
  | "reproduced"
  | "not_reproduced"
  | "inconclusive"
  | "fix_verified"
  | "still_failing"
  | "regressed"
  | "regression_passed";

/** 一次重現紀錄（對應 Rust 的 ReproductionAttempt）；沒有 outcome 表示還在排隊。 */
export interface ReproductionAttempt {
  finding_id: string;
  request_id: string;
  purpose: ReproductionPurpose;
  journey: VerificationJourney;
  bundle_id?: string;
  /** 案例的判定。 */
  verdict?: VerificationVerdict;
  reason_code?: string;
  /** 對照（代表旅程）的判定。 */
  control_verdict?: VerificationVerdict;
  outcome?: ReproductionOutcome;
  requested_by: string;
  created_at: number;
  finished_at?: number;
}

/** 一個疑點與它的重現紀錄（對應 Rust 的 FindingView）。 */
export interface ExplorationFinding {
  id: string;
  workspace_id: string;
  task_id: string;
  charter_id: string;
  charter_version: number;
  title: string;
  /** 對應的功能（Feature Map 的 key）。 */
  feature?: string;
  /** 重現步驟。 */
  steps: string[];
  expected: string;
  actual: string;
  environment: string;
  /** 證據識別。 */
  evidence: string[];
  status: ExplorationFindingStatus;
  exclusion?: ExplorationExclusion;
  unverified_reason?: string;
  case?: ReproductionCase;
  /** 回報者：Worker Run。 */
  reported_by: string;
  version: number;
  created_at: number;
  updated_at: number;
  task_title: string | null;
  /** 新的在前。 */
  attempts: ReproductionAttempt[];
}

/** Workspace 的探索概況（對應 Rust 的 ExplorationOverview）。 */
export interface ExplorationOverview {
  workspace_id: string;
  charters: ExplorationCharter[];
  /** 新的在前。 */
  findings: ExplorationFinding[];
}

/** 探索測試（唯讀）。 */
export interface ExplorationClient {
  explorationOverview(workspaceId: string): Promise<ExplorationOverview>;
}

export type AppClient = RuntimeClient &
  WorkspaceClient &
  AuthorizationClient &
  TaskClient &
  AccountClient &
  DecisionClient &
  SchedulingClient &
  LifecycleClient &
  ArtifactClient &
  SharedSettingsClient &
  BackupClient &
  RecoveryClient &
  KnowledgeClient &
  GuardClient &
  ObservabilityClient &
  UnderstandingClient &
  WorkflowClient &
  ReviewClient &
  VerificationProfileClient &
  ExplorationClient &
  ConversationClient &
  ControllerCliClient &
  UpdateClient;

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
const PENDING_DECISIONS_PATH = "/api/v1/decisions/pending";
const BATCH_DECISIONS_PATH = "/api/v1/decisions/batch";
const SCHEDULING_PATH = "/api/v1/scheduling";
const LIFECYCLE_PATH = "/api/v1/lifecycle";
/** 桌面殼在系統匣「完全退出」時發出的事件（先顯示主視窗再發出）。 */
export const FULL_EXIT_EVENT = "baton://full-exit-requested";
const SHARED_SETTINGS_PATH = "/api/v1/shared-settings";
const BACKUPS_PATH = "/api/v1/backups";
const RECOVERY_PATH = "/api/v1/recovery";
const UPDATES_PATH = "/api/v1/updates";
const authorizationPath = (workspaceId: string) => `${WORKSPACES_PATH}/${encodeURIComponent(workspaceId)}/authorization`;
const workspacePath = (workspaceId: string) => `${WORKSPACES_PATH}/${encodeURIComponent(workspaceId)}`;
const taskPath = (workspaceId: string, taskId: string) => `${workspacePath(workspaceId)}/tasks/${encodeURIComponent(taskId)}`;
const decisionPath = (workspaceId: string, requestId: string) =>
  `${workspacePath(workspaceId)}/decisions/${encodeURIComponent(requestId)}`;
const artifactVersionPath = (workspaceId: string, artifactId: string, version: number) =>
  `${workspacePath(workspaceId)}/artifacts/${encodeURIComponent(artifactId)}/versions/${version}`;
const KNOWLEDGE_PATH = "/api/v1/knowledge";
const knowledgePath = (id: string) => `${KNOWLEDGE_PATH}/${encodeURIComponent(id)}`;
const GUARDS_PATH = "/api/v1/guards";
const guardPath = (id: string) => `${GUARDS_PATH}/${encodeURIComponent(id)}`;
/** 未指定 Task 時不送出 task_id。 */
const timelinePath = (workspaceId: string, taskId?: string) =>
  `${workspacePath(workspaceId)}/timeline${taskId === undefined ? "" : `?task_id=${encodeURIComponent(taskId)}`}`;

/** 列表查詢字串：未指定的 workspace_id 與空白的 q 不送出。 */
function knowledgeListPath({ workspaceId, q, includeInactive }: KnowledgeListQuery): string {
  const params = new URLSearchParams();
  if (workspaceId !== undefined) params.set("workspace_id", workspaceId);
  if (q !== undefined && q.trim() !== "") params.set("q", q);
  params.set("include_inactive", String(includeInactive));
  return `${KNOWLEDGE_PATH}?${params.toString()}`;
}

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
    setModelOverrides: (id, overrides) =>
      send(`${workspacePath(id)}/model-overrides`, { method: "PUT", body: overrides }),
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
    async confirmAuthorization(workspaceId, { templateId, grants, acknowledgedRisk, acknowledgedExpansion, expectedRevision }) {
      const response = await fetchFn(authorizationPath(workspaceId), {
        method: "PUT",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
          template_id: templateId,
          grants,
          acknowledged_risk: acknowledgedRisk,
          acknowledged_expansion: acknowledgedExpansion,
          expected_revision: expectedRevision,
        }),
      });
      if (!response.ok) throw await failure(response);
      return (await response.json()) as AuthorizationRevision;
    },
    getTaskBoard: (workspaceId) => send(`${workspacePath(workspaceId)}/tasks`),
    createGoal: (workspaceId, title) => send(`${workspacePath(workspaceId)}/goals`, { method: "POST", body: { title } }),
    createTask: (workspaceId, { goalId, title, kind, contract }) =>
      send(`${workspacePath(workspaceId)}/tasks`, { method: "POST", body: { goal_id: goalId, title, kind, contract } }),
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
    createProfileFromExistingDir: ({ name, configDir }) =>
      send(`${PROFILES_PATH}/from-existing-dir`, { method: "POST", body: { name, config_dir: configDir } }),
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
    setQuotaLimits: (profileId, limits) =>
      send(`${PROFILES_PATH}/${encodeURIComponent(profileId)}/quota-limits`, { method: "PUT", body: limits }),
    backupStatus: () => send(BACKUPS_PATH),
    recoveryStatus: () => send(RECOVERY_PATH),
    retryRecovery: () => send(`${RECOVERY_PATH}/retry`, { method: "POST" }),
    cancelRun: (workspaceId, runId) =>
      send(`${workspacePath(workspaceId)}/runs/${encodeURIComponent(runId)}/cancel`, { method: "POST" }),
    runContext: (workspaceId, runId) => send(`${workspacePath(workspaceId)}/runs/${encodeURIComponent(runId)}/context`),
    lifecycleStatus: () => send(LIFECYCLE_PATH),
    requestFullExit: () => send(`${LIFECYCLE_PATH}/exit`, { method: "POST" }),
    updateWorkspaceLifecycle: (workspaceId, autoResumeAfterExit) =>
      send(`${workspacePath(workspaceId)}/lifecycle`, {
        method: "PUT",
        body: { auto_resume_after_exit: autoResumeAfterExit },
      }),
    resumeWorkspace: (workspaceId) => send(`${workspacePath(workspaceId)}/resume`, { method: "POST" }),
    updateWorkspaceLoginResume: (workspaceId, autoResumeAcrossLogin) =>
      send(`${workspacePath(workspaceId)}/login-resume`, {
        method: "PUT",
        body: { auto_resume_across_login: autoResumeAcrossLogin },
      }),
    updateLoginStartup: (enabled) => send(`${LIFECYCLE_PATH}/login-startup`, { method: "PUT", body: { enabled } }),
    // 瀏覽器開發模式沒有桌面程序可結束；window.close() 對使用者自己開的分頁無效，所以不做事
    quitApp: async () => {},
    // 後備強制關閉計時器只存在於桌面殼
    cancelFullExitFallback: async () => {},
    // 系統匣只存在於桌面殼，瀏覽器裡不會收到完全退出事件
    onFullExitRequested: () => () => {},
    updateStatus: () => send(UPDATES_PATH),
    applyUpdate: (version) => send(`${UPDATES_PATH}/apply`, { method: "POST", body: { version } }),
    // 安裝程式由桌面殼執行；瀏覽器開發模式沒有殼
    installUpdate: async () => {
      throw new Error("只有桌面 App 能安裝更新");
    },
    taskVerification: (workspaceId, taskId) => send(`${taskPath(workspaceId, taskId)}/verification`),
    acceptTask: (workspaceId, taskId, { basis, note }) =>
      send(`${taskPath(workspaceId, taskId)}/verification/accept`, { method: "POST", body: { basis, note } }),
    listDecisions: (workspaceId) => send(`${workspacePath(workspaceId)}/decisions`),
    getDecision: (workspaceId, requestId) => send(decisionPath(workspaceId, requestId)),
    answerDecision: (workspaceId, requestId, request) =>
      send(`${decisionPath(workspaceId, requestId)}/answer`, { method: "POST", body: request }),
    batchDecisions: (items) => send(BATCH_DECISIONS_PATH, { method: "POST", body: { items } }),
    mergeDecision: (workspaceId, requestId, request) =>
      send(`${decisionPath(workspaceId, requestId)}/merge`, { method: "POST", body: request }),
    linkDecision: (workspaceId, requestId, otherId) =>
      send(`${decisionPath(workspaceId, requestId)}/links`, { method: "POST", body: { other_id: otherId } }),
    decisionAuthority: (workspaceId) => send(`${workspacePath(workspaceId)}/decision-authority`),
    updateDecisionAuthority: (workspaceId, request) =>
      send(`${workspacePath(workspaceId)}/decision-authority`, { method: "PUT", body: request }),
    pendingDecisions: () => send(PENDING_DECISIONS_PATH),
    listArtifacts: (workspaceId) => send(`${workspacePath(workspaceId)}/artifacts`),
    previewArtifact: (workspaceId, artifactId, version) =>
      send(`${artifactVersionPath(workspaceId, artifactId, version)}/preview`),
    async downloadArtifact(workspaceId, artifactId, version) {
      const response = await fetchFn(`${artifactVersionPath(workspaceId, artifactId, version)}/content`);
      if (!response.ok) throw await failure(response);
      return { blob: await response.blob(), sha256: response.headers.get("X-Baton-Sha256") ?? "" };
    },
    listKnowledge: (query) => send(knowledgeListPath(query)),
    createKnowledge: (request) => send(KNOWLEDGE_PATH, { method: "POST", body: request }),
    knowledgeDetail: (id) => send(knowledgePath(id)),
    confirmKnowledge: (id, expectedRevision) =>
      send(`${knowledgePath(id)}/confirm`, { method: "POST", body: { expected_revision: expectedRevision } }),
    correctKnowledge: (id, request) => send(`${knowledgePath(id)}/correct`, { method: "POST", body: request }),
    revokeKnowledge: (id, request) => send(`${knowledgePath(id)}/revoke`, { method: "POST", body: request }),
    retireKnowledge: (id, request) => send(`${knowledgePath(id)}/retire`, { method: "POST", body: request }),
    listGuards: (workspaceId) =>
      send(workspaceId === undefined ? GUARDS_PATH : `${GUARDS_PATH}?workspace_id=${encodeURIComponent(workspaceId)}`),
    createGuard: (request) => send(GUARDS_PATH, { method: "POST", body: request }),
    enableGuard: (id, expectedRevision) =>
      send(`${guardPath(id)}/enable`, { method: "POST", body: { expected_revision: expectedRevision } }),
    disableGuard: (id, expectedRevision) =>
      send(`${guardPath(id)}/disable`, { method: "POST", body: { expected_revision: expectedRevision } }),
    listRecurrences: (lessonId) => send(`${knowledgePath(lessonId)}/recurrences`),
    reportRecurrence: (lessonId, request) =>
      send(`${knowledgePath(lessonId)}/recurrences`, { method: "POST", body: request }),
    timeline: (workspaceId, taskId) => send(timelinePath(workspaceId, taskId)),
    metrics: (workspaceId) => send(`${workspacePath(workspaceId)}/metrics`),
    understanding: (workspaceId) => send(`${workspacePath(workspaceId)}/understanding`),
    syncUnderstanding: (workspaceId) => send(`${workspacePath(workspaceId)}/understanding/sync`, { method: "POST" }),
    reconcileProjection: (workspaceId, request) =>
      send(`${workspacePath(workspaceId)}/understanding/projections/reconcile`, { method: "POST", body: request }),
    workflows: (workspaceId) => send(`${workspacePath(workspaceId)}/workflows`),
    skipStage: (workspaceId, workflowId, request) =>
      send(`${workspacePath(workspaceId)}/workflows/${encodeURIComponent(workflowId)}/skip`, { method: "POST", body: request }),
    reviews: (workspaceId) => send(`${workspacePath(workspaceId)}/reviews`),
    verificationOverview: (workspaceId) => send(`${workspacePath(workspaceId)}/verification`),
    publishVerificationProfile: (workspaceId, definition) =>
      send(`${workspacePath(workspaceId)}/verification/profile`, { method: "POST", body: definition }),
    runVerification: (workspaceId, request) =>
      send(`${workspacePath(workspaceId)}/verification/runs`, { method: "POST", body: request }),
    evidenceBundle: (workspaceId, bundleId) =>
      send(`${workspacePath(workspaceId)}/verification/bundles/${encodeURIComponent(bundleId)}`),
    evidenceArtifact: (workspaceId, bundleId, index) =>
      send(`${workspacePath(workspaceId)}/verification/bundles/${encodeURIComponent(bundleId)}/artifacts/${index}`),
    explorationOverview: (workspaceId) => send(`${workspacePath(workspaceId)}/exploration`),
    getConversation: (workspaceId) => send(`${workspacePath(workspaceId)}/conversation`),
    sendConversationMessage: (workspaceId, text) =>
      send(`${workspacePath(workspaceId)}/conversation/messages`, { method: "POST", body: { text } }),
    getControllerCli: (workspaceId) => send(`${workspacePath(workspaceId)}/controller-cli`),
    openControllerCli: (workspaceId, profileId) =>
      send(`${workspacePath(workspaceId)}/controller-cli`, { method: "POST", body: { profile_id: profileId } }),
    endControllerCli: (workspaceId) => send(`${workspacePath(workspaceId)}/controller-cli/end`, { method: "POST" }),
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
    setModelOverrides: (id, overrides) => call<Workspace>("workspace_set_model_overrides", { id, overrides }),
    getAuthorization: (id) => call<AuthorizationView>("workspace_authorization", { id }),
    summarizeAuthorization: (id, grants) => call<ConfirmationSummary>("workspace_authorization_summary", { id, grants }),
    confirmAuthorization: (id, { templateId, grants, acknowledgedRisk, acknowledgedExpansion, expectedRevision }) =>
      call<AuthorizationRevision>("workspace_authorization_confirm", {
        id,
        templateId,
        grants,
        acknowledgedRisk,
        acknowledgedExpansion,
        expectedRevision,
      }),
    getTaskBoard: (workspaceId) => call<TaskBoard>("task_board", { workspaceId }),
    createGoal: (workspaceId, title) => call<Goal>("goal_create", { workspaceId, title }),
    createTask: (workspaceId, { goalId, title, kind, contract }) =>
      call<TaskView>("task_create", { workspaceId, goalId, title, kind, contract }),
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
    createProfileFromExistingDir: ({ name, configDir }) =>
      call<AccountProfile>("profile_create_from_dir", { name, configDir }),
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
    setQuotaLimits: (id, limits) => call<SchedulingOverview>("profile_set_quota_limits", { id, limits }),
    backupStatus: () => call<BackupStatus>("backup_status"),
    recoveryStatus: () => call<RecoveryStatusView>("recovery_status"),
    retryRecovery: () => call<RecoveryStatusView>("recovery_retry"),
    cancelRun: (workspaceId, runId) => call<RunView>("run_cancel", { workspaceId, runId }),
    runContext: (workspaceId, runId) => call<RunContext>("run_context", { workspaceId, runId }),
    lifecycleStatus: () => call<LifecycleStatus>("lifecycle_status"),
    requestFullExit: () => call<LifecycleStatus>("lifecycle_exit"),
    updateWorkspaceLifecycle: (workspaceId, autoResumeAfterExit) =>
      call<WorkspaceLifecycleView>("workspace_lifecycle_update", { workspaceId, autoResumeAfterExit }),
    resumeWorkspace: (workspaceId) => call<WorkspaceLifecycleView>("workspace_resume", { workspaceId }),
    updateWorkspaceLoginResume: (workspaceId, autoResumeAcrossLogin) =>
      call<WorkspaceLifecycleView>("workspace_login_resume_update", { workspaceId, autoResumeAcrossLogin }),
    updateLoginStartup: (enabled) => call<LoginStartupView>("lifecycle_login_startup_update", { enabled }),
    quitApp: () => call<void>("app_quit"),
    cancelFullExitFallback: () => call<void>("full_exit_cancel_fallback"),
    onFullExitRequested: (handler) => {
      const registered = listen(FULL_EXIT_EVENT, () => handler());
      // 註冊是非同步的；取消訂閱時等註冊完成再解除
      return () => void registered.then((unlisten) => unlisten());
    },
    updateStatus: () => call<UpdateStatusView>("update_status"),
    applyUpdate: (version) => call<UpdateStatusView>("update_apply", { version }),
    installUpdate: () => call<void>("update_install"),
    taskVerification: (workspaceId, taskId) => call<VerificationView>("task_verification", { workspaceId, taskId }),
    acceptTask: (workspaceId, taskId, { basis, note }) =>
      call<VerificationView>("task_accept", { workspaceId, taskId, request: { basis, note } }),
    listDecisions: (workspaceId) => call<DecisionList>("decision_list", { workspaceId }),
    getDecision: (workspaceId, requestId) => call<DecisionRequestView>("decision_detail", { workspaceId, requestId }),
    answerDecision: (workspaceId, requestId, request) =>
      call<AnswerResponse>("decision_answer", { workspaceId, requestId, request }),
    batchDecisions: (items) => call<BatchResponse>("decisions_batch", { request: { items } }),
    mergeDecision: (workspaceId, requestId, request) =>
      call<{ decision: DecisionRequestView }>("decision_merge", { workspaceId, requestId, request }),
    linkDecision: (workspaceId, requestId, otherId) =>
      call<DecisionRequestView>("decision_link", { workspaceId, requestId, otherId }),
    decisionAuthority: (workspaceId) => call<AuthorityTable>("decision_authority", { workspaceId }),
    updateDecisionAuthority: (workspaceId, request) =>
      call<AuthorityTable>("decision_authority_update", { workspaceId, request }),
    pendingDecisions: () => call<PendingDecisions>("decisions_pending"),
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
    listKnowledge: ({ workspaceId, q, includeInactive }) =>
      call<KnowledgeList>("knowledge_list", { workspaceId, q, includeInactive }),
    createKnowledge: (request) => call<KnowledgeEntryView>("knowledge_create", { request }),
    knowledgeDetail: (id) => call<KnowledgeDetail>("knowledge_detail", { id }),
    confirmKnowledge: (id, expectedRevision) =>
      call<KnowledgeEntryView>("knowledge_confirm", { id, request: { expected_revision: expectedRevision } }),
    correctKnowledge: (id, request) => call<KnowledgeEntryView>("knowledge_correct", { id, request }),
    revokeKnowledge: (id, request) => call<KnowledgeEntryView>("knowledge_revoke", { id, request }),
    retireKnowledge: (id, request) => call<KnowledgeEntryView>("knowledge_retire", { id, request }),
    listGuards: (workspaceId) => call<GuardList>("guard_list", { workspaceId }),
    createGuard: (request) => call<GuardView>("guard_create", { request }),
    enableGuard: (id, expectedRevision) => call<GuardView>("guard_enable", { id, expectedRevision }),
    disableGuard: (id, expectedRevision) => call<GuardView>("guard_disable", { id, expectedRevision }),
    listRecurrences: (id) => call<RecurrenceList>("knowledge_recurrences", { id }),
    reportRecurrence: (id, request) => call<Recurrence>("knowledge_report_recurrence", { id, request }),
    timeline: (workspaceId, taskId) => call<TimelineResponse>("workspace_timeline", { workspaceId, taskId }),
    metrics: (workspaceId) => call<MetricsResponse>("workspace_metrics", { workspaceId }),
    understanding: (workspaceId) => call<UnderstandingView>("workspace_understanding", { workspaceId }),
    syncUnderstanding: (workspaceId) => call<UnderstandingView>("workspace_understanding_sync", { workspaceId }),
    reconcileProjection: (workspaceId, request) =>
      call<UnderstandingView>("workspace_projection_reconcile", { workspaceId, request }),
    workflows: (workspaceId) => call<WorkflowList>("workspace_workflows", { workspaceId }),
    skipStage: (workspaceId, workflowId, request) =>
      call<WorkflowView>("workflow_skip_stage", { workspaceId, workflowId, request }),
    reviews: (workspaceId) => call<ReviewList>("workspace_reviews", { workspaceId }),
    verificationOverview: (workspaceId) => call<VerificationOverview>("workspace_verification", { workspaceId }),
    publishVerificationProfile: (workspaceId, definition) =>
      call<PublishProfileResponse>("verification_publish_profile", { workspaceId, definition }),
    runVerification: (workspaceId, request) => call<VerificationRequest>("verification_run", { workspaceId, request }),
    evidenceBundle: (workspaceId, bundleId) => call<EvidenceBundle>("verification_bundle", { workspaceId, bundleId }),
    evidenceArtifact: (workspaceId, bundleId, index) =>
      call<EvidenceArtifactContent>("verification_artifact", { workspaceId, bundleId, index }),
    explorationOverview: (workspaceId) => call<ExplorationOverview>("workspace_exploration", { workspaceId }),
    getConversation: (workspaceId) => call<ConversationView>("conversation_view", { workspaceId }),
    sendConversationMessage: (workspaceId, text) =>
      call<SendMessageResponse>("conversation_send_message", { workspaceId, text }),
    getControllerCli: (workspaceId) => call<ControllerCliView>("controller_cli_view", { workspaceId }),
    openControllerCli: (workspaceId, profileId) =>
      call<ControllerCliView>("controller_cli_open", { workspaceId, profileId }),
    endControllerCli: (workspaceId) => call<ControllerCliView>("controller_cli_end", { workspaceId }),
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
