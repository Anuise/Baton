// 桌面 App 冒煙測試：node scripts/smoke-desktop.mjs（需先 pnpm build）
//
// 以暫存資料目錄與隨機埠啟動真正的 baton-desktop.exe，透過 WebView2 的遠端除錯埠讀取畫面，確認：
//   1. App 啟動後自動啟動 Runtime，狀態列顯示「Runtime 運作中」
//   2. 關閉 App 視窗後 Runtime 仍在執行
//   3. 再次開啟 App 時連回同一個 Runtime（boot_count 不增加、pid 不變）
//   4. 不帶憑證的請求被 Runtime 拒絕（桌面憑證由 App 自動讀取 DPAPI 保護檔，不需人工複製）
import { spawn, spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, rmSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

import { repoRoot } from "./lib.mjs";

const exe = resolve(repoRoot, "target", "release", "baton-desktop.exe");
if (!existsSync(exe)) {
  console.error(`找不到 ${exe}，請先執行 pnpm build`);
  process.exit(1);
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function freePort() {
  return new Promise((resolvePort) => {
    const server = createServer().listen(0, "127.0.0.1", () => {
      const { port } = server.address();
      server.close(() => resolvePort(port));
    });
  });
}

async function waitFor(description, probe, timeoutMs = 40_000) {
  const deadline = Date.now() + timeoutMs;
  let last;
  while (Date.now() < deadline) {
    try {
      const value = await probe();
      if (value) return value;
    } catch (error) {
      last = error;
    }
    await sleep(500);
  }
  throw new Error(`等待逾時：${description}${last ? `（${last.message}）` : ""}`);
}

/** 以 CDP 在第一個頁面執行運算式並回傳結果。 */
async function evaluate(debugPort, expression) {
  const targets = await (await fetch(`http://127.0.0.1:${debugPort}/json`)).json();
  const page = targets.find((t) => t.type === "page");
  if (!page) throw new Error("尚無頁面");
  const socket = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise((ok, fail) => {
    socket.onopen = ok;
    socket.onerror = () => fail(new Error("CDP 連線失敗"));
  });
  const result = await new Promise((ok) => {
    socket.onmessage = (event) => ok(JSON.parse(event.data));
    socket.send(JSON.stringify({ id: 1, method: "Runtime.evaluate", params: { expression, returnByValue: true } }));
  });
  socket.close();
  return result.result?.result?.value;
}

/** 以目前使用者身分還原 DPAPI 保護的桌面憑證（與 App 相同的來源）。 */
function readDesktopCredential(dir) {
  const file = join(dir, "auth", "desktop-credential.bin");
  const script = `Add-Type -AssemblyName System.Security; $b = [IO.File]::ReadAllBytes('${file.replaceAll("'", "''")}'); [Text.Encoding]::UTF8.GetString([Security.Cryptography.ProtectedData]::Unprotect($b, $null, 'CurrentUser'))`;
  const result = spawnSync("powershell", ["-NoProfile", "-Command", script], { encoding: "utf8" });
  if (result.status !== 0) throw new Error(`無法讀取桌面憑證：${result.stderr}`);
  return result.stdout.trim();
}

const runtimeUrl = (port) => `http://127.0.0.1:${port}/api/v1/health`;

const health = async (port) =>
  (
    await fetch(runtimeUrl(port), {
      headers: { Authorization: `Bearer ${readDesktopCredential(dataDir)}`, "X-Baton-Client": "baton-desktop" },
    })
  ).json();

function launchApp(env) {
  return spawn(exe, [], { env: { ...process.env, ...env }, stdio: "ignore" });
}

/** 只結束指定程序，不含子程序（模擬使用者關閉視窗：Runtime 不隨 App 結束）。 */
function killProcess(pid) {
  spawn("taskkill", ["/PID", String(pid), "/F"], { stdio: "ignore" });
}

/** 清掉使用指定 WebView2 資料夾的殘留程序（msedgewebview2），否則資料夾無法刪除。 */
function killWebviewProcesses(dir) {
  const script = `Get-CimInstance Win32_Process | Where-Object { $_.CommandLine -like '*${dir.replaceAll("'", "''")}*' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }`;
  spawnSync("powershell", ["-NoProfile", "-Command", script], { stdio: "ignore" });
}

const dataDir = mkdtempSync(join(tmpdir(), "baton-smoke-data-"));
const webviewDir = mkdtempSync(join(tmpdir(), "baton-smoke-webview-"));
const runtimePort = await freePort();
let runtimePid;

try {
  const launchEnv = (debugPort) => ({
    BATON_DATA_DIR: dataDir,
    BATON_PORT: String(runtimePort),
    WEBVIEW2_USER_DATA_FOLDER: webviewDir,
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${debugPort}`,
  });

  // 1. 第一次開啟：App 啟動 Runtime，狀態列顯示運作中
  const debug1 = await freePort();
  const app1 = launchApp(launchEnv(debug1));
  const bar = await waitFor("狀態列顯示 Runtime 運作中", async () => {
    const text = await evaluate(debug1, "document.querySelector('.status-bar')?.innerText ?? ''");
    return text.includes("Runtime 運作中") ? text : null;
  });
  console.log(`✔ 狀態列：${bar.trim()}`);
  // 平台辨識：狀態列顯示支援狀態，平台狀態頁顯示本機實際的診斷資訊
  const platformBar = await waitFor("狀態列顯示平台狀態", async () => {
    const text = await evaluate(debug1, "document.querySelector('.status-bar')?.innerText ?? ''");
    return /平台：(正式支援|非正式支援)/.test(text) ? text : null;
  });
  const platformPage = await waitFor("平台狀態頁顯示診斷資訊", async () => {
    const text = await evaluate(debug1, "document.querySelector('.platform-page')?.innerText ?? ''");
    return text.includes("OS build") && text.includes("原生架構") && text.includes("App 版本") ? text : null;
  });
  console.log(`✔ ${platformBar.match(/平台：\S+/)[0]}；${platformPage.replace(/\s+/g, " ").trim()}`);
  const first = await health(runtimePort);
  runtimePid = first.pid;
  console.log(`✔ Runtime 已由 App 啟動（pid ${first.pid}，boot_count ${first.boot_count}）`);

  // 2. 關閉 App：Runtime 繼續執行
  killProcess(app1.pid);
  await sleep(2000);
  const afterClose = await health(runtimePort);
  if (afterClose.pid !== first.pid) throw new Error("關閉 App 後 Runtime 不是同一個程序");
  console.log("✔ 關閉 App 後 Runtime 仍在執行");

  // 3. 再次開啟：連回同一個 Runtime
  const debug2 = await freePort();
  const app2 = launchApp(launchEnv(debug2));
  await waitFor("再次開啟後狀態列顯示 Runtime 運作中", async () => {
    const text = await evaluate(debug2, "document.querySelector('.status-bar')?.innerText ?? ''");
    return text.includes("Runtime 運作中");
  });
  const second = await health(runtimePort);
  if (second.pid !== first.pid || second.boot_count !== first.boot_count) {
    throw new Error(`再次開啟後不是同一個 Runtime：${JSON.stringify(second)}`);
  }
  console.log("✔ 再次開啟時連回同一個 Runtime（沒有重複啟動）");

  // 4. 不帶憑證的請求（例如瀏覽器中的其他程式）被拒絕
  const anonymous = await fetch(runtimeUrl(runtimePort));
  if (anonymous.status !== 403 && anonymous.status !== 401) {
    throw new Error(`未帶憑證的請求應被拒絕，實際回應 ${anonymous.status}`);
  }
  console.log(`✔ 未帶憑證的請求被拒絕（${anonymous.status}）`);
  killProcess(app2.pid);
  console.log("冒煙測試通過");
} catch (error) {
  console.error(`冒煙測試失敗：${error.message}`);
  process.exitCode = 1;
} finally {
  if (runtimePid) killProcess(runtimePid);
  killWebviewProcesses(webviewDir);
  await sleep(1000);
  for (const dir of [dataDir, webviewDir]) rmSync(dir, { recursive: true, force: true });
}
