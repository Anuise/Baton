/** Runtime 健康端點的回應（對應 Rust 的 HealthResponse）。 */
export interface HealthResponse {
  status: string;
  product: string;
  version: string;
  boot_count: number;
  started_at: number;
}

export interface RuntimeClient {
  getHealth(): Promise<HealthResponse>;
}

const HEALTH_PATH = "/api/v1/health";

/** 直接以 HTTP 查詢（開發時用瀏覽器開啟，由 Vite 代理到 Runtime）。 */
export function createHttpClient(fetchFn: typeof fetch = fetch): RuntimeClient {
  return {
    async getHealth() {
      const response = await fetchFn(HEALTH_PATH);
      if (!response.ok) throw new Error(`Runtime 回應 ${response.status}`);
      return (await response.json()) as HealthResponse;
    },
  };
}

type Invoke = <T>(command: string) => Promise<T>;

/** 在 Tauri 內透過桌面殼的指令查詢；殼負責連回或啟動 Runtime。 */
export function createTauriClient(invoke: Invoke): RuntimeClient {
  return {
    async getHealth() {
      try {
        return await invoke<HealthResponse>("runtime_health");
      } catch (error) {
        // Tauri 指令的錯誤是純字串
        throw error instanceof Error ? error : new Error(String(error));
      }
    },
  };
}

/** 依執行環境挑選用戶端。 */
export async function createRuntimeClient(): Promise<RuntimeClient> {
  if ("__TAURI_INTERNALS__" in window) {
    const { invoke } = await import("@tauri-apps/api/core");
    return createTauriClient(invoke);
  }
  return createHttpClient();
}
