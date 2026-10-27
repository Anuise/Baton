// UI E2E 的瀏覽器 driver（票 53）：以 stdin／stdout 的 JSON 行協定，替 Rust 測試操作 Playwright + Chromium。
//
// - 每個要求一行、每個回應一行，回應帶回要求的 `id`；要求依序處理。
// - Playwright 依序從 `BATON_PLAYWRIGHT_DIR`、repo 根目錄、`web/` 解析；都沒有時 `open` 回報 missingPlaywright，不代為安裝。
// - stdin 結束（測試結束或 panic）時關閉瀏覽器再結束，不留下 Chromium。
// - Chromium 以 launchServer 啟動，關閉後確認它的程序已結束（`browserExited`）。

import { createRequire } from 'node:module';
import path from 'node:path';
import readline from 'node:readline';

let server = null;
let browser = null;
let page = null;
let timeout = 15000;

const send = (message) => process.stdout.write(JSON.stringify(message) + '\n');

function loadPlaywright() {
  const bases = [process.env.BATON_PLAYWRIGHT_DIR, process.cwd(), path.join(process.cwd(), 'web')].filter(Boolean);
  for (const base of bases) {
    const require = createRequire(path.join(path.resolve(base), 'package.json'));
    for (const name of ['playwright', '@playwright/test']) {
      try {
        return require(name);
      } catch (error) {
        if (error?.code !== 'MODULE_NOT_FOUND') throw error;
      }
    }
  }
  return null;
}

/** 依定位鏈找到元素：每一節在前一節的範圍內再找。 */
function locate(target) {
  let current = page;
  for (const step of target) {
    if (step.role) current = current.getByRole(step.role, { name: step.name, exact: true });
    else if (step.label) current = current.getByLabel(step.label, { exact: true });
    else if (step.text) current = current.getByText(step.text);
    else if (step.exactText) current = current.getByText(step.exactText, { exact: true });
    else if (step.hasText) current = current.filter({ hasText: step.hasText });
    else if (step.nth !== undefined) current = current.nth(step.nth);
    else throw new Error(`不認識的定位：${JSON.stringify(step)}`);
  }
  return current;
}

const isAlive = (pid) => {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
};

/** 關閉瀏覽器並等它的程序結束（最多 10 秒）；回傳是否已結束。 */
async function closeBrowser() {
  if (!server) return true;
  const pid = server.process()?.pid;
  await browser?.close().catch(() => {});
  await server.close().catch(() => {});
  server = null;
  browser = null;
  page = null;
  if (!pid) return false;
  for (let waited = 0; waited < 10000 && isAlive(pid); waited += 100) {
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  return !isAlive(pid);
}

async function handle(request) {
  switch (request.op) {
    case 'open': {
      const playwright = loadPlaywright();
      if (!playwright) {
        return { ok: false, missingPlaywright: true, message: '找不到 Playwright（BATON_PLAYWRIGHT_DIR、repo 根目錄與 web/ 都沒有）' };
      }
      timeout = request.timeoutMs ?? timeout;
      server = await playwright.chromium.launchServer({ headless: true });
      browser = await playwright.chromium.connect(server.wsEndpoint());
      const context = await browser.newContext({ locale: 'zh-TW', timezoneId: 'Asia/Taipei' });
      page = await context.newPage();
      page.setDefaultTimeout(timeout);
      return { ok: true, version: browser.version() };
    }
    case 'goto':
      await page.goto(request.url);
      return { ok: true };
    case 'click':
      await locate(request.target).click();
      return { ok: true };
    case 'fill':
      await locate(request.target).fill(request.value);
      return { ok: true };
    case 'select':
      await locate(request.target).selectOption({ label: request.option });
      return { ok: true };
    case 'check':
      await locate(request.target).check();
      return { ok: true };
    case 'waitVisible':
      await locate(request.target).waitFor({ state: 'visible' });
      return { ok: true };
    case 'waitHidden':
      await locate(request.target).waitFor({ state: 'hidden' });
      return { ok: true };
    case 'text':
      return { ok: true, value: await locate(request.target).innerText() };
    case 'count':
      return { ok: true, value: await locate(request.target).count() };
    case 'close':
      return { ok: true, browserExited: await closeBrowser() };
    default:
      throw new Error(`不認識的操作：${request.op}`);
  }
}

/** 失敗時附上當下的畫面文字，方便看出畫面停在哪裡。 */
async function pageText() {
  try {
    return page ? (await page.evaluate(() => document.body?.innerText ?? '')).slice(0, 4000) : '';
  } catch {
    return '';
  }
}

const lines = readline.createInterface({ input: process.stdin });
const queue = [];
let busy = false;

async function drain() {
  if (busy) return;
  busy = true;
  while (queue.length > 0) {
    const request = queue.shift();
    try {
      send({ id: request.id, ...(await handle(request)) });
    } catch (error) {
      send({ id: request.id, ok: false, message: String(error?.message ?? error), page: await pageText() });
    }
  }
  busy = false;
}

lines.on('line', (line) => {
  try {
    queue.push(JSON.parse(line));
  } catch {
    return;
  }
  void drain();
});

lines.on('close', async () => {
  // 等手上的要求做完再關閉瀏覽器
  while (busy) await new Promise((resolve) => setTimeout(resolve, 50));
  await closeBrowser();
  process.exit(0);
});
