// UI 預覽截圖：啟動預覽伺服器（web/preview.html），對每個頁面 × 寬度 × 主題 × 資料集截整頁圖，
// 輸出到 .scratch/ui-shots/<label>/，並產生 index.html（依頁面並排）與 results.json
// （每張圖的 console error 與水平捲軸偵測結果）。
//
// 用法：node scripts/ui-screenshots.mjs [--label <名稱>] [--pages <id,id,...>]
// Playwright 從 BATON_PLAYWRIGHT_DIR（或 repo 根目錄、web/）解析，不是本 repo 的相依。
import { mkdirSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { createServer as createNetServer } from "node:net";
import path from "node:path";
import { pathToFileURL } from "node:url";

import { repoRoot } from "./lib.mjs";

const VIEWPORTS = [
  { width: 1440, height: 900 },
  { width: 1024, height: 768 },
];
const THEMES = ["light", "dark"];
/** 同時開啟的瀏覽器分頁數。 */
const CONCURRENCY = 4;
/** 等待頁面讀取完成（畫面上不再有「載入中」）的上限。 */
const READY_TIMEOUT_MS = 10_000;

const webDir = path.join(repoRoot, "web");

function parseArgs(argv) {
  const args = { label: null, pages: null };
  for (let i = 0; i < argv.length; i++) {
    if (argv[i] === "--label") args.label = argv[++i];
    else if (argv[i] === "--pages") args.pages = argv[++i].split(",").map((id) => id.trim());
    else throw new Error(`不認識的參數：${argv[i]}`);
  }
  if (args.label !== null && !/^[\w.-]+$/.test(args.label)) throw new Error(`--label 只能用英數字、底線、點與連字號：${args.label}`);
  return args;
}

function timestamp() {
  const d = new Date();
  const pad = (n) => String(n).padStart(2, "0");
  return `${d.getFullYear()}${pad(d.getMonth() + 1)}${pad(d.getDate())}-${pad(d.getHours())}${pad(d.getMinutes())}${pad(d.getSeconds())}`;
}

/** 與 crates/baton-harness/tests/ui_e2e/driver.mjs 相同的解析順序。 */
function loadPlaywright() {
  const bases = [process.env.BATON_PLAYWRIGHT_DIR, repoRoot, webDir].filter(Boolean);
  for (const base of bases) {
    const require = createRequire(path.join(path.resolve(base), "package.json"));
    for (const name of ["playwright", "@playwright/test"]) {
      try {
        return require(name);
      } catch (error) {
        if (error?.code !== "MODULE_NOT_FOUND") throw error;
      }
    }
  }
  return null;
}

function freePort() {
  return new Promise((resolve, reject) => {
    const server = createNetServer();
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const { port } = server.address();
      server.close(() => resolve(port));
    });
  });
}

/** 以 web/ 的 Vite 設定啟動開發伺服器（預覽入口只由 dev server 提供）。 */
async function startPreviewServer() {
  const vitePath = createRequire(path.join(webDir, "package.json")).resolve("vite");
  const { createServer } = await import(pathToFileURL(vitePath).href);
  const port = await freePort();
  const server = await createServer({
    root: webDir,
    configFile: path.join(webDir, "vite.config.ts"),
    logLevel: "error",
    server: { host: "127.0.0.1", port, strictPort: true },
  });
  await server.listen();
  return { server, origin: `http://127.0.0.1:${port}` };
}

/** 依序取出工作，同時最多 `limit` 個。 */
async function runPool(items, limit, worker) {
  let next = 0;
  const lanes = Array.from({ length: Math.min(limit, items.length) }, async () => {
    while (next < items.length) await worker(items[next++]);
  });
  await Promise.all(lanes);
}

/** 截一張圖並記錄 console error 與水平捲軸。 */
async function capture(context, origin, job, outDir) {
  const page = await context.newPage();
  const consoleErrors = [];
  page.on("console", (message) => {
    if (message.type() === "error") consoleErrors.push(message.text());
  });
  page.on("pageerror", (error) => consoleErrors.push(`pageerror: ${error.message}`));
  const notes = [];
  try {
    await page.goto(`${origin}/preview.html?page=${job.page.id}&dataset=${job.dataset}`);
    const ready = () =>
      page.waitForFunction(() => document.querySelector(".app") && !document.body.innerText.includes("載入中"), null, {
        timeout: READY_TIMEOUT_MS,
      });
    await ready().catch(() => notes.push(`${READY_TIMEOUT_MS / 1000} 秒後畫面仍有「載入中」`));
    for (const name of job.page.click ?? []) {
      await page.getByRole("button", { name, exact: true }).first().click();
      await ready().catch(() => notes.push(`點擊「${name}」後仍有「載入中」`));
    }
    // 讓輪詢的第一輪結果與版面穩定下來
    await page.waitForTimeout(300);
    const overflow = await page.evaluate(() => {
      const root = document.documentElement;
      const viewport = root.clientWidth;
      // 列出右緣超出視窗最多的幾個元素，方便找到撐寬版面的來源；外層容器只是被撐開，只留最內層的元素
      const beyond = (element) => element.getBoundingClientRect().right > viewport + 1;
      const offenders = [...document.querySelectorAll("body *")]
        .filter((element) => beyond(element) && ![...element.children].some(beyond))
        .map((element) => ({ element, right: element.getBoundingClientRect().right }))
        .sort((a, b) => b.right - a.right)
        .slice(0, 5)
        .map(({ element, right }) => {
          const classes = typeof element.className === "string" && element.className ? `.${element.className.trim().split(/\s+/).join(".")}` : "";
          return { selector: `${element.tagName.toLowerCase()}${classes}`, right: Math.round(right), text: (element.textContent ?? "").trim().slice(0, 60) };
        });
      return { scrollWidth: root.scrollWidth, clientWidth: viewport, horizontalScroll: root.scrollWidth > viewport, offenders };
    });
    await page.screenshot({ path: path.join(outDir, job.file), fullPage: true });
    return { ...job, page: job.page.id, ok: true, consoleErrors, notes, ...overflow };
  } catch (error) {
    return { ...job, page: job.page.id, ok: false, consoleErrors, notes: [...notes, `截圖失敗：${error.message}`] };
  } finally {
    await page.close();
  }
}

const escapeHtml = (text) => String(text).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);

function renderIndex(label, pages, datasets, results) {
  const find = (pageId, dataset, theme, viewport) =>
    results.find((r) => r.page === pageId && r.dataset === dataset && r.theme === theme && r.width === viewport.width);
  const cell = (result) => {
    if (!result) return "<td>—</td>";
    const flags = [
      result.ok ? "" : '<span class="bad">截圖失敗</span>',
      result.horizontalScroll ? `<span class="bad">水平捲軸 ${result.scrollWidth}px &gt; ${result.clientWidth}px</span>` : "",
      result.consoleErrors.length ? `<span class="bad">console error ${result.consoleErrors.length}</span>` : "",
      ...result.notes.map((note) => `<span class="warn">${escapeHtml(note)}</span>`),
    ].join("");
    const details = [
      ...result.consoleErrors.map((e) => `<li>${escapeHtml(e)}</li>`),
      ...(result.offenders ?? []).map((o) => `<li>超出：${escapeHtml(o.selector)}（右緣 ${o.right}px）${escapeHtml(o.text)}</li>`),
    ].join("");
    const image = result.ok ? `<a href="${result.file}" target="_blank"><img src="${result.file}" loading="lazy" alt="${escapeHtml(result.file)}"></a>` : "";
    return `<td><div class="flags">${flags || '<span class="good">OK</span>'}</div>${details ? `<details><summary>細節</summary><ul>${details}</ul></details>` : ""}${image}</td>`;
  };
  const sections = pages
    .map((page) => {
      const rows = datasets
        .flatMap((dataset) =>
          THEMES.map(
            (theme) => `<tr><th scope="row">${dataset}<br>${theme}</th>${VIEWPORTS.map((v) => cell(find(page.id, dataset, theme, v))).join("")}</tr>`,
          ),
        )
        .join("");
      const head = VIEWPORTS.map((v) => `<th>${v.width}×${v.height}</th>`).join("");
      return `<section id="${page.id}"><h2>${escapeHtml(page.label)} <code>${page.id}</code></h2><table><thead><tr><th></th>${head}</tr></thead><tbody>${rows}</tbody></table></section>`;
    })
    .join("\n");
  const toc = pages.map((page) => `<a href="#${page.id}">${escapeHtml(page.label)}</a>`).join(" · ");
  const failed = results.filter((r) => !r.ok).length;
  const scroll = results.filter((r) => r.horizontalScroll).length;
  const errors = results.filter((r) => r.consoleErrors.length).length;
  return `<!doctype html>
<html lang="zh-Hant"><head><meta charset="UTF-8"><title>UI 截圖 ${escapeHtml(label)}</title>
<style>
  body { font-family: system-ui, "Microsoft JhengHei", sans-serif; margin: 16px; background: #f4f4f5; color: #18181b; }
  table { border-collapse: collapse; width: 100%; table-layout: fixed; }
  th, td { border: 1px solid #d4d4d8; padding: 6px; vertical-align: top; background: #fff; }
  thead th:first-child, tbody th { width: 72px; }
  img { width: 100%; max-height: 640px; object-fit: cover; object-position: top; border: 1px solid #e4e4e7; display: block; }
  .flags span { display: inline-block; margin: 0 4px 4px 0; padding: 1px 6px; border-radius: 4px; font-size: 12px; }
  .good { background: #dcfce7; } .bad { background: #fee2e2; } .warn { background: #fef9c3; }
  details { font-size: 12px; margin-bottom: 4px; word-break: break-all; }
  nav { margin-bottom: 16px; line-height: 1.8; }
</style></head><body>
<h1>UI 截圖 ${escapeHtml(label)}</h1>
<p>共 ${results.length} 張；截圖失敗 ${failed}、有水平捲軸 ${scroll}、有 console error ${errors}。點圖片開原圖（整頁）。</p>
<nav>${toc}</nav>
${sections}
</body></html>`;
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const playwright = loadPlaywright();
  if (!playwright) {
    console.error("找不到 Playwright：請設定 BATON_PLAYWRIGHT_DIR 指向已安裝 playwright 的目錄（repo 根目錄與 web/ 也都沒有）");
    return 1;
  }
  const label = args.label ?? timestamp();
  const outDir = path.join(repoRoot, ".scratch", "ui-shots", label);
  mkdirSync(outDir, { recursive: true });

  const { server, origin } = await startPreviewServer();
  const browser = await playwright.chromium.launch({ headless: true });
  try {
    // 頁面與資料集清單只在 web/src/preview/pages.ts 維護，從預覽入口讀取
    const listPage = await browser.newPage();
    await listPage.goto(`${origin}/preview.html`);
    await listPage.waitForFunction(() => window.__BATON_PREVIEW__);
    const preview = await listPage.evaluate(() => window.__BATON_PREVIEW__);
    await listPage.close();
    const unknown = (args.pages ?? []).filter((id) => !preview.pages.some((page) => page.id === id));
    if (unknown.length) throw new Error(`沒有這些頁面：${unknown.join("、")}`);
    const pages = args.pages ? preview.pages.filter((page) => args.pages.includes(page.id)) : preview.pages;

    const results = [];
    for (const theme of THEMES) {
      for (const viewport of VIEWPORTS) {
        const context = await browser.newContext({ viewport, colorScheme: theme, locale: "zh-TW", timezoneId: "Asia/Taipei" });
        const jobs = pages.flatMap((page) =>
          preview.datasets.map((dataset) => ({
            page,
            dataset,
            theme,
            width: viewport.width,
            height: viewport.height,
            file: `${page.id}__${dataset}__${theme}__${viewport.width}x${viewport.height}.png`,
          })),
        );
        await runPool(jobs, CONCURRENCY, async (job) => {
          const result = await capture(context, origin, job, outDir);
          results.push(result);
          const marks = [result.ok ? "" : "失敗", result.horizontalScroll ? "水平捲軸" : "", result.consoleErrors.length ? `console error ${result.consoleErrors.length}` : ""].filter(Boolean);
          console.log(`${result.file}${marks.length ? `  [${marks.join("、")}]` : ""}`);
        });
        await context.close();
      }
    }

    results.sort((a, b) => a.file.localeCompare(b.file));
    writeFileSync(path.join(outDir, "results.json"), JSON.stringify({ label, generated_at: new Date().toISOString(), results }, null, 2));
    writeFileSync(path.join(outDir, "index.html"), renderIndex(label, pages, preview.datasets, results));

    const failed = results.filter((r) => !r.ok);
    console.log(`\n輸出：${outDir}`);
    console.log(`截圖 ${results.length - failed.length} / ${results.length} 張`);
    console.log(`有水平捲軸：${results.filter((r) => r.horizontalScroll).length} 張`);
    console.log(`有 console error：${results.filter((r) => r.consoleErrors.length).length} 張`);
    console.log(`索引：${path.join(outDir, "index.html")}`);
    return failed.length ? 1 : 0;
  } finally {
    await browser.close();
    await server.close();
  }
}

main().then(
  (code) => process.exit(code),
  (error) => {
    console.error(error);
    process.exit(1);
  },
);
