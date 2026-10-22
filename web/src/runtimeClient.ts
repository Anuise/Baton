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

export type AppClient = RuntimeClient & WorkspaceClient & AccountClient;

const HEALTH_PATH = "/api/v1/health";
const PLATFORM_PATH = "/api/v1/platform";
const WORKSPACES_PATH = "/api/v1/workspaces";
const PROFILES_PATH = "/api/v1/profiles";
const SETUP_PATH = "/api/v1/setup";

/** 直接以 HTTP 查詢（開發時用瀏覽器開啟，由 Vite 代理到 Runtime）。 */
export function createHttpClient(fetchFn: typeof fetch = fetch): AppClient {
  /** 非 2xx 時優先採用 Runtime 回報的說明（`{ error, message }`），沒有才退回狀態碼。 */
  const failure = async (response: Response) => {
    const body = (await response.json().catch(() => null)) as { message?: string } | null;
    return new Error(body?.message ?? `Runtime 回應 ${response.status}`);
  };
  /** GET（省略 method）或帶 JSON 本文的 POST；失敗時丟出 Runtime 的說明。 */
  const send = async <T>(path: string, method?: "POST", body?: unknown): Promise<T> => {
    const response =
      method === undefined
        ? await fetchFn(path)
        : await fetchFn(
            path,
            body === undefined
              ? { method }
              : { method, headers: { "content-type": "application/json" }, body: JSON.stringify(body) },
          );
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
    async listProfiles() {
      return send<ProfileList>(PROFILES_PATH);
    },
    async createProfile({ name, expectedAccount }) {
      return send<AccountProfile>(PROFILES_PATH, "POST", { name, expected_account: expectedAccount });
    },
    async verifyProfile(id) {
      return send<AccountProfile>(`${PROFILES_PATH}/${encodeURIComponent(id)}/verify`, "POST");
    },
    async openLoginTerminal(id) {
      return send<{ opened: boolean }>(`${PROFILES_PATH}/${encodeURIComponent(id)}/login-terminal`, "POST");
    },
    async archiveProfile(id) {
      return send<AccountProfile>(`${PROFILES_PATH}/${encodeURIComponent(id)}/archive`, "POST");
    },
    async getSetup() {
      return send<SetupStatus>(SETUP_PATH);
    },
    async recheckSetup() {
      return send<SetupStatus>(`${SETUP_PATH}/recheck`, "POST");
    },
    async acknowledgeBackgroundNotice() {
      return send<SetupStatus>(`${SETUP_PATH}/background-notice/acknowledge`, "POST");
    },
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
    listProfiles: () => call<ProfileList>("profile_list"),
    // Tauri 會把 JS 的 camelCase 參數對應到 Rust 的 snake_case
    createProfile: ({ name, expectedAccount }) => call<AccountProfile>("profile_create", { name, expectedAccount }),
    verifyProfile: (id) => call<AccountProfile>("profile_verify", { id }),
    openLoginTerminal: (id) => call<{ opened: boolean }>("profile_login_terminal", { id }),
    archiveProfile: (id) => call<AccountProfile>("profile_archive", { id }),
    getSetup: () => call<SetupStatus>("setup_status"),
    recheckSetup: () => call<SetupStatus>("setup_recheck"),
    acknowledgeBackgroundNotice: () => call<SetupStatus>("setup_acknowledge_notice"),
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
