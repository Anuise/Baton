/**
 * 開發用預覽可以呈現的畫面（只在 web/preview.html 使用，不進正式建置）。
 * 截圖腳本從 `window.__BATON_PREVIEW__.pages` 讀取同一份清單，畫面只在這裡維護一處。
 */
export interface PreviewPage {
  /** URL 參數 `page` 的值，也是截圖檔名的一部分。 */
  id: string;
  /** 給人看的名稱（index.html 的分組標題）。 */
  label: string;
  /** 所在頂層頁簽；首次啟動精靈全畫面時沒有頁簽。 */
  top: "workspace" | "accounts" | "settings";
  /** Workspace 頁簽內的頁面；為 null 時不選 Workspace（顯示「請從左側選擇」）。 */
  workspacePage?: string | null;
  /** 以沒有可派工帳號的資料呈現，讓首次啟動精靈以全畫面顯示。 */
  setup?: boolean;
  /** 截圖前要點擊的按鈕（第一個符合名稱者），用來呈現頁面內才展開的內容。 */
  click?: string;
}

export const PREVIEW_PAGES: PreviewPage[] = [
  { id: "setup", label: "首次啟動精靈", top: "workspace", setup: true },
  { id: "workspace", label: "Workspace（未選擇）", top: "workspace", workspacePage: null },
  { id: "controller", label: "Workspace／主控台", top: "workspace", workspacePage: "controller" },
  { id: "tasks", label: "Workspace／工作", top: "workspace", workspacePage: "tasks" },
  { id: "tasks-contract", label: "Workspace／工作（編輯契約）", top: "workspace", workspacePage: "tasks", click: "編輯契約" },
  { id: "decisions", label: "Workspace／裁定", top: "workspace", workspacePage: "decisions" },
  { id: "artifacts", label: "成果與品質／成果", top: "workspace", workspacePage: "artifacts" },
  { id: "reviews", label: "成果與品質／審查", top: "workspace", workspacePage: "reviews" },
  { id: "verification", label: "成果與品質／驗證", top: "workspace", workspacePage: "verification" },
  { id: "exploration", label: "成果與品質／探索", top: "workspace", workspacePage: "exploration" },
  { id: "knowledge", label: "更多／知識", top: "workspace", workspacePage: "knowledge" },
  { id: "workflows", label: "更多／流程", top: "workspace", workspacePage: "workflows" },
  { id: "timeline", label: "更多／時間線", top: "workspace", workspacePage: "timeline" },
  { id: "authorization", label: "更多／授權", top: "workspace", workspacePage: "authorization" },
  { id: "accounts", label: "帳號", top: "accounts" },
  { id: "settings", label: "設定", top: "settings" },
];

export const PREVIEW_DATASETS = ["normal", "long"] as const;
export type PreviewDatasetName = (typeof PREVIEW_DATASETS)[number];
