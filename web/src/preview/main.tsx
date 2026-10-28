/**
 * 開發用預覽入口（web/preview.html）：以假資料呈現 App 的某一頁，不連 Runtime。
 * URL 參數：`page`（見 pages.ts）、`dataset`（normal／long）；沒有 `page` 時列出全部頁面的連結。
 * 只由 vite dev server 提供，`vite build` 只建置 index.html，這裡的程式碼與假資料不進正式產物。
 */
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { App, SHELL_STORAGE_KEY } from "../App";
import "../styles.css";
import { createPreviewClient, previewData } from "./client";
import { PREVIEW_WORKSPACE_ID } from "./data";
import { PREVIEW_DATASETS, PREVIEW_PAGES, type PreviewDatasetName } from "./pages";

declare global {
  interface Window {
    /** 截圖腳本從這裡讀取頁面與資料集清單。 */
    __BATON_PREVIEW__?: { pages: typeof PREVIEW_PAGES; datasets: typeof PREVIEW_DATASETS };
  }
}

window.__BATON_PREVIEW__ = { pages: PREVIEW_PAGES, datasets: PREVIEW_DATASETS };

const params = new URLSearchParams(window.location.search);
const page = PREVIEW_PAGES.find((item) => item.id === params.get("page"));
const dataset: PreviewDatasetName = params.get("dataset") === "long" ? "long" : "normal";
const root = createRoot(document.getElementById("root")!);

if (page) {
  // App 從本機記住的位置開啟，預覽以它指定要呈現的頁面
  localStorage.setItem(
    SHELL_STORAGE_KEY,
    JSON.stringify({
      top: page.top,
      workspaceId: page.workspacePage ? PREVIEW_WORKSPACE_ID : null,
      workspaceTab: page.workspacePage ?? "controller",
    }),
  );
  const client = createPreviewClient(previewData(dataset, page.setup ?? false));
  const createClient = async () => client;
  root.render(
    <StrictMode>
      <App createClient={createClient} />
    </StrictMode>,
  );
} else {
  root.render(
    <main className="main">
      <h1>UI 預覽</h1>
      <table>
        <tbody>
          {PREVIEW_PAGES.map((item) => (
            <tr key={item.id}>
              <th scope="row">{item.label}</th>
              {PREVIEW_DATASETS.map((name) => (
                <td key={name}>
                  <a href={`?page=${item.id}&dataset=${name}`}>{name}</a>
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </main>,
  );
}
