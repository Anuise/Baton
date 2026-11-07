/**
 * 開發用預覽可以呈現的畫面（只在 web/preview.html 使用，不進正式建置）。
 * 截圖腳本從 `window.__BATON_PREVIEW__.pages` 讀取同一份清單，畫面只在這裡維護一處。
 */
export interface PreviewPage {
  /** URL 參數 `page` 的值，也是截圖檔名的一部分。 */
  id: string;
  /** 給人看的名稱（index.html 的分組標題）。 */
  label: string;
  /** 所在檢視：Workspace 的頁面，或跑道欄底部的平台頁面。 */
  view: "workspace" | "accounts" | "shared" | "settings";
  /** Workspace 的頁面；為 null 時不選 Workspace（顯示空狀態）。 */
  workspacePage?: string | null;
  /** 以沒有可派工帳號的資料呈現，讓首次啟動精靈以全畫面顯示。 */
  setup?: boolean;
  /** 截圖前依序點擊的按鈕（各取第一個符合名稱者），用來呈現頁面內才展開的內容。 */
  click?: string[];
}

export const PREVIEW_PAGES: PreviewPage[] = [
  { id: "setup", label: "首次啟動精靈", view: "workspace", setup: true },
  { id: "workspace", label: "Workspace（未選擇）", view: "workspace", workspacePage: null },
  { id: "controller", label: "進行／Controller", view: "workspace", workspacePage: "controller" },
  { id: "tasks", label: "進行／工作", view: "workspace", workspacePage: "tasks" },
  { id: "tasks-contract", label: "進行／工作（編輯契約）", view: "workspace", workspacePage: "tasks", click: ["更多", "編輯契約"] },
  { id: "workflows", label: "進行／流程", view: "workspace", workspacePage: "workflows" },
  { id: "decisions", label: "進行／裁定", view: "workspace", workspacePage: "decisions" },
  { id: "timeline", label: "進行／時間線", view: "workspace", workspacePage: "timeline" },
  { id: "artifacts", label: "產出／成果", view: "workspace", workspacePage: "artifacts" },
  { id: "reviews", label: "產出／審查", view: "workspace", workspacePage: "reviews" },
  { id: "verification", label: "產出／驗證", view: "workspace", workspacePage: "verification" },
  { id: "exploration", label: "產出／探索測試", view: "workspace", workspacePage: "exploration" },
  { id: "knowledge", label: "脈絡／知識", view: "workspace", workspacePage: "knowledge" },
  { id: "understanding", label: "脈絡／專案理解", view: "workspace", workspacePage: "understanding" },
  { id: "authorization", label: "規則／授權", view: "workspace", workspacePage: "authorization" },
  { id: "authorization-confirm", label: "規則／授權（確認摘要）", view: "workspace", workspacePage: "authorization", click: ["檢視確認摘要"] },
  { id: "authority", label: "規則／決策權限", view: "workspace", workspacePage: "authority" },
  { id: "models", label: "規則／模型設定", view: "workspace", workspacePage: "models" },
  { id: "accounts", label: "平台／帳號", view: "accounts" },
  { id: "shared", label: "平台／共同設定", view: "shared" },
  { id: "settings", label: "平台／App 設定", view: "settings" },
];

export const PREVIEW_DATASETS = ["normal", "long"] as const;
export type PreviewDatasetName = (typeof PREVIEW_DATASETS)[number];
