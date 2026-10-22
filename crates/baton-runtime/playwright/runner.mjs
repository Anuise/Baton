// Baton 的瀏覽器 Runner（票 46）：一個 Session 一個 Node 程序，以 stdin／stdout 的 JSON 行協定與 Runtime 溝通。
//
// - 每個要求一行、每個回應一行，回應帶回要求的 `id`；要求依序處理（前一個沒做完，下一個不開始）。
// - Playwright 從受驗專案解析（`working_dir` 的 `playwright` 或 `@playwright/test`），解析不到時不代為安裝。
// - `fill` 的文字可能是秘密：只留在這個程序的記憶體中，錯誤訊息與 trace 回傳前都會去掉它。
//   標準錯誤不交給 Runtime（Playwright 的除錯輸出可能含輸入的文字）。

import { createRequire } from 'node:module';
import fs from 'node:fs';
import path from 'node:path';
import readline from 'node:readline';
import zlib from 'node:zlib';

// 操作之後觀察畫面（等待載入、截圖）各自的等待上限
const SNAPSHOT_TIMEOUT_MS = 10000;
// 秘密在錯誤訊息與 trace 中被換成這個標記
const REDACTED = '***';
// trace 只去掉至少這麼長的輸入：更短的字串會把 trace 的結構一起換掉
const TRACE_REDACT_MIN_CHARS = 4;

const secrets = new Set();
let session = null;

const send = (message) => process.stdout.write(JSON.stringify(message) + '\n');

/** 去掉訊息中所有已輸入過的文字（與終端機的色彩控制碼）。 */
function redact(text) {
  let result = String(text ?? '').replace(/\u001b\[[0-9;]*m/g, '');
  for (const secret of secrets) {
    if (secret) result = result.split(secret).join(REDACTED);
  }
  return result;
}

/** 實際載入的 Playwright 套件版本（取自它的 package.json）；取不到時為 null。 */
function packageVersion(require, name) {
  try {
    return require(`${name}/package.json`).version || null;
  } catch {
    return null;
  }
}

/** 依序從受驗專案、`BATON_PLAYWRIGHT_DIR` 解析 Playwright 與其版本；都沒有時回傳 null。 */
function loadPlaywright(workingDir) {
  const bases = [workingDir, process.env.BATON_PLAYWRIGHT_DIR].filter(Boolean);
  for (const base of bases) {
    const require = createRequire(path.join(path.resolve(base), 'package.json'));
    for (const name of ['playwright', '@playwright/test']) {
      try {
        return { playwright: require(name), version: packageVersion(require, name) };
      } catch (error) {
        if (error?.code !== 'MODULE_NOT_FOUND') throw error;
      }
    }
  }
  return null;
}

async function open(request) {
  const loaded = loadPlaywright(request.workingDir);
  if (!loaded) {
    return {
      ok: false,
      missingPlaywright: true,
      message: `受驗專案（${request.workingDir}）找不到 Playwright；請在受驗專案安裝 playwright 或 @playwright/test 與其瀏覽器（Baton 不代為安裝或下載）`,
    };
  }
  const { playwright, version } = loaded;
  const browser = await playwright.chromium.launch({ headless: true });
  try {
    const options = request.videoDir ? { recordVideo: { dir: request.videoDir } } : {};
    const context = await browser.newContext(options);
    if (request.tracePath) await context.tracing.start({ screenshots: true, snapshots: true });
    const page = await context.newPage();
    session = { browser, context, page, status: null, tracePath: request.tracePath };
    // 最近一次主框架導覽的回應狀態（goto 與點擊造成的導覽都算；轉址以最後一個回應為準）
    page.on('response', (response) => {
      if (response.request().isNavigationRequest() && response.frame() === page.mainFrame()) {
        session.status = response.status();
      }
    });
    // 實際使用的引擎（錄製身分的一部分，票 48）；Playwright 版本取不到時省略
    const engine = ['playwright', version, 'chromium', browser.version()].filter(Boolean).join(' ');
    return { ok: true, engine };
  } catch (error) {
    await browser.close().catch(() => {});
    throw error;
  }
}

async function act(page, command, timeout) {
  switch (command.kind) {
    case 'goto': {
      const response = await page.goto(command.url, { timeout });
      if (response) session.status = response.status();
      return;
    }
    case 'click':
      return page.locator(command.selector).click({ timeout });
    case 'fill':
      return page.locator(command.selector).fill(command.text, { timeout });
    case 'press':
      return page.locator(command.selector).press(command.key, { timeout });
    case 'waitFor':
      return page.locator(command.selector).waitFor({ timeout });
    default:
      throw new Error(`不認識的操作：${command.kind}`);
  }
}

const isNavigationRace = (error) => /context was destroyed|navigat/i.test(String(error?.message));

/** 觀察當前畫面；操作引發的導覽還在進行時，等它載入後重試。 */
async function observe(page, visible, mask) {
  for (let attempt = 1; ; attempt++) {
    await page.waitForLoadState('domcontentloaded', { timeout: SNAPSHOT_TIMEOUT_MS }).catch(() => {});
    try {
      const title = await page.title();
      const text = await page.evaluate(() => (document.body ? document.body.innerText : ''));
      const shown = [];
      for (const selector of visible) shown.push(await page.locator(selector).isVisible());
      let screenshot = null;
      if (mask) {
        const png = await page.screenshot({ mask: mask.map((s) => page.locator(s)), timeout: SNAPSHOT_TIMEOUT_MS });
        screenshot = png.toString('base64');
      }
      return { url: page.url(), title, status: session.status, text, visible: shown, screenshot };
    } catch (error) {
      if (attempt >= 3 || !isNavigationRace(error)) throw error;
    }
  }
}

async function step(request) {
  const { command, timeoutMs, visible, mask } = request;
  const page = session.page;
  if (command.kind === 'fill') secrets.add(command.text);
  try {
    await act(page, command, timeoutMs);
  } catch (error) {
    const message = redact(error?.message);
    if (command.kind === 'goto' && /net::ERR_/.test(message)) {
      return { outcome: 'unreachable', reason: message };
    }
    const current = await observe(page, visible, mask).catch(() => null);
    return { outcome: 'actionFailed', error: error?.name || 'Error', message, page: current };
  }
  try {
    return { outcome: 'done', page: await observe(page, visible, mask) };
  } catch (error) {
    return { outcome: 'actionFailed', error: error?.name || 'Error', message: redact(error?.message), page: null };
  }
}

async function close() {
  if (!session) return { ok: true, files: [] };
  const { browser, context, page, tracePath } = session;
  const files = [];
  if (tracePath) {
    await context.tracing.stop({ path: tracePath });
    // 去不掉秘密的 trace 不交出去
    if (redactZip(tracePath)) files.push({ kind: 'trace', path: tracePath });
    else fs.rmSync(tracePath, { force: true });
  }
  const video = page.video();
  // 關閉 context 才會把錄影寫完
  await context.close();
  if (video) files.push({ kind: 'video', path: await video.path() });
  await browser.close();
  session = null;
  return { ok: true, files };
}

// ---- trace（zip）中的秘密 ----
// trace 的 DOM 快照會記下輸入欄位的值、操作紀錄會記下 fill 的文字、網路紀錄會記下送出的表單，
// 所以關閉時把每個項目解開、換掉輸入過的文字（含 JSON、URL 與表單編碼的形式）再重新壓縮。
// 畫面截圖與錄影是影像，無法以文字去除。

const CRC_TABLE = Array.from({ length: 256 }, (_, n) => {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c >>> 0;
});

function crc32(buffer) {
  let crc = 0xffffffff;
  for (const byte of buffer) crc = CRC_TABLE[(crc ^ byte) & 0xff] ^ (crc >>> 8);
  return (crc ^ 0xffffffff) >>> 0;
}

function secretForms(secret) {
  const forms = new Set([
    secret,
    JSON.stringify(secret).slice(1, -1),
    encodeURIComponent(secret),
    encodeURIComponent(secret).replace(/%20/g, '+'),
    new URLSearchParams({ v: secret }).toString().slice(2),
  ]);
  return [...forms].filter(Boolean).map((form) => Buffer.from(form, 'utf8'));
}

function replaceAll(buffer, needle, replacement) {
  let at = buffer.indexOf(needle);
  if (at < 0) return buffer;
  const parts = [];
  let from = 0;
  while (at >= 0) {
    parts.push(buffer.subarray(from, at), replacement);
    from = at + needle.length;
    at = buffer.indexOf(needle, from);
  }
  parts.push(buffer.subarray(from));
  return Buffer.concat(parts);
}

/** 讀 zip 的中央目錄，換掉秘密後重寫整個檔案；格式超出預期（例如 zip64）時回傳 false。 */
function redactZip(file) {
  const forms = [...secrets].filter((s) => [...s].length >= TRACE_REDACT_MIN_CHARS).flatMap(secretForms);
  if (forms.length === 0) return true;
  try {
    const zip = fs.readFileSync(file);
    let end = zip.length - 22;
    while (end >= 0 && zip.readUInt32LE(end) !== 0x06054b50) end--;
    if (end < 0) return false;
    const count = zip.readUInt16LE(end + 10);
    let at = zip.readUInt32LE(end + 16);
    if (count === 0xffff || at === 0xffffffff) return false;
    const replacement = Buffer.from(REDACTED);
    const locals = [];
    const centrals = [];
    let offset = 0;
    for (let i = 0; i < count; i++) {
      if (zip.readUInt32LE(at) !== 0x02014b50) return false;
      const method = zip.readUInt16LE(at + 10);
      const time = zip.readUInt16LE(at + 12);
      const date = zip.readUInt16LE(at + 14);
      const compressedSize = zip.readUInt32LE(at + 20);
      const nameLength = zip.readUInt16LE(at + 28);
      const skip = nameLength + zip.readUInt16LE(at + 30) + zip.readUInt16LE(at + 32);
      const localAt = zip.readUInt32LE(at + 42);
      const name = zip.subarray(at + 46, at + 46 + nameLength);
      at += 46 + skip;
      if (compressedSize === 0xffffffff || localAt === 0xffffffff) return false;
      if (method !== 0 && method !== 8) return false;
      const dataAt = localAt + 30 + zip.readUInt16LE(localAt + 26) + zip.readUInt16LE(localAt + 28);
      const raw = zip.subarray(dataAt, dataAt + compressedSize);
      let data = method === 8 ? zlib.inflateRawSync(raw) : raw;
      for (const form of forms) data = replaceAll(data, form, replacement);
      const compressed = zlib.deflateRawSync(data);
      const crc = crc32(data);
      const local = Buffer.alloc(30);
      local.writeUInt32LE(0x04034b50, 0);
      local.writeUInt16LE(20, 4);
      local.writeUInt16LE(0x0800, 6);
      local.writeUInt16LE(8, 8);
      local.writeUInt16LE(time, 10);
      local.writeUInt16LE(date, 12);
      local.writeUInt32LE(crc, 14);
      local.writeUInt32LE(compressed.length, 18);
      local.writeUInt32LE(data.length, 22);
      local.writeUInt16LE(name.length, 26);
      const central = Buffer.alloc(46);
      central.writeUInt32LE(0x02014b50, 0);
      central.writeUInt16LE(20, 4);
      central.writeUInt16LE(20, 6);
      central.writeUInt16LE(0x0800, 8);
      central.writeUInt16LE(8, 10);
      central.writeUInt16LE(time, 12);
      central.writeUInt16LE(date, 14);
      central.writeUInt32LE(crc, 16);
      central.writeUInt32LE(compressed.length, 20);
      central.writeUInt32LE(data.length, 24);
      central.writeUInt16LE(name.length, 28);
      central.writeUInt32LE(offset, 42);
      locals.push(local, name, compressed);
      centrals.push(central, name);
      offset += 30 + name.length + compressed.length;
    }
    const directory = Buffer.concat(centrals);
    const eocd = Buffer.alloc(22);
    eocd.writeUInt32LE(0x06054b50, 0);
    eocd.writeUInt16LE(count, 8);
    eocd.writeUInt16LE(count, 10);
    eocd.writeUInt32LE(directory.length, 12);
    eocd.writeUInt32LE(offset, 16);
    fs.writeFileSync(file, Buffer.concat([...locals, directory, eocd]));
    return true;
  } catch {
    return false;
  }
}

// ---- 協定 ----

async function handle(request) {
  switch (request.op) {
    case 'open':
      return open(request);
    case 'step':
      if (!session) return { outcome: 'failed', message: '瀏覽器 Session 尚未開啟' };
      return step(request);
    case 'close':
      return close();
    default:
      return { ok: false, message: `不認識的要求：${request.op}` };
  }
}

const lines = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
for await (const line of lines) {
  if (!line.trim()) continue;
  let request;
  try {
    request = JSON.parse(line.replace(/^\uFEFF/, ''));
  } catch {
    send({ id: null, ok: false, message: '無法解析要求' });
    continue;
  }
  let response;
  try {
    response = await handle(request);
  } catch (error) {
    response = { ok: false, outcome: 'failed', message: redact(`${error?.name || 'Error'}：${error?.message}`) };
  }
  send({ id: request.id, ...response });
  if (request.op === 'close') break;
}
// 輸出寫完才結束；標準輸入關閉（Runtime 不再需要這個 Session）時也在這裡結束，Playwright 隨之關閉瀏覽器
process.stdout.write('', () => process.exit(0));
