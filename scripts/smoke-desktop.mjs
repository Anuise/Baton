// 桌面 App 冒煙測試：node scripts/smoke-desktop.mjs（需先 pnpm build）
//
// 以暫存資料目錄與隨機埠啟動真正的 baton-desktop.exe，透過 WebView2 的遠端除錯埠讀取畫面，確認：
//   1. App 啟動後自動啟動 Runtime，狀態列顯示「Runtime 運作中」
//   2. App 程序結束後 Runtime 仍在執行（關閉視窗只會隱藏到系統匣，所以這裡直接結束 App 程序）
//   3. 再次開啟 App 時連回同一個 Runtime（boot_count 不增加、pid 不變）
//   4. 在 App 內建立 Workspace 後進入授權設定，套用 P3 範本、檢視摘要、承認風險並確認保存；
//      確認之前需要「建置與測試」的 Run 被准入檢查拒絕，確認之後通過
//   5. 不帶憑證的請求被 Runtime 拒絕（桌面憑證由 App 自動讀取 DPAPI 保護檔，不需人工複製）
//   6. 首次設定精靈以本機真實的 claude 與 git 檢查；帳號頁新增 Profile 並以真實的 claude auth status 核對，
//      第 9 個 Profile 被拒絕（不會開啟登入終端機，避免在測試中彈出視窗）
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

/** 在頁面內操作 React 畫面（以文字找元素，模擬使用者操作）。 */
const ui = {
  type: (label, value) =>
    `(() => { const l = [...document.querySelectorAll('label')].find((l) => l.innerText.trim().startsWith(${JSON.stringify(label)})); const i = l?.querySelector('input'); if (!i) return false; Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(i, ${JSON.stringify(value)}); i.dispatchEvent(new Event('input', { bubbles: true })); return true; })()`,
  click: (text) =>
    `(() => { const b = [...document.querySelectorAll('button')].find((b) => b.innerText.trim() === ${JSON.stringify(text)}); if (!b || b.disabled) return false; b.click(); return true; })()`,
  check: (text) =>
    `(() => { const l = [...document.querySelectorAll('label')].find((l) => l.innerText.trim() === ${JSON.stringify(text)}); const i = l?.querySelector('input[type=checkbox]'); if (!i) return false; if (!i.checked) i.click(); return true; })()`,
  text: (selector) => `document.querySelector(${JSON.stringify(selector)})?.innerText ?? ''`,
};

const runtimeUrl = (port) => `http://127.0.0.1:${port}/api/v1/health`;

/** 帶桌面憑證呼叫 Runtime API，回傳狀態碼與 JSON。 */
async function callApi(port, path, method = "GET", body) {
  const response = await fetch(`http://127.0.0.1:${port}${path}`, {
    method,
    headers: {
      Authorization: `Bearer ${readDesktopCredential(dataDir)}`,
      "X-Baton-Client": "baton-desktop",
      ...(body ? { "content-type": "application/json" } : {}),
    },
    body: body ? JSON.stringify(body) : undefined,
  });
  return { status: response.status, body: await response.json() };
}

/** 在頁面中填入指定標籤的輸入欄位（React 受控元件需要原生 setter 與 input 事件）。 */
const fillExpression = (label, value) => `(() => {
  const input = [...document.querySelectorAll('label')].find((l) => l.innerText.trim() === ${JSON.stringify(label)})?.querySelector('input');
  if (!input) return false;
  Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(input, ${JSON.stringify(value)});
  input.dispatchEvent(new Event('input', { bubbles: true }));
  return true;
})()`;

const clickExpression = (ariaLabelOrText) => `(() => {
  const button = [...document.querySelectorAll('button')].find((b) => (b.getAttribute('aria-label') ?? b.innerText.trim()) === ${JSON.stringify(ariaLabelOrText)});
  if (!button || button.disabled) return false;
  button.click();
  return true;
})()`;

const health = async (port) =>
  (
    await fetch(runtimeUrl(port), {
      headers: { Authorization: `Bearer ${readDesktopCredential(dataDir)}`, "X-Baton-Client": "baton-desktop" },
    })
  ).json();

function launchApp(env) {
  return spawn(exe, [], { env: { ...process.env, ...env }, stdio: "ignore" });
}

/** 只結束指定程序，不含子程序（Runtime 不隨 App 結束）。強制結束不會觸發 CloseRequested，所以不受「關窗隱藏到系統匣」影響。 */
function killProcess(pid) {
  spawn("taskkill", ["/PID", String(pid), "/F"], { stdio: "ignore" });
}

/** 清掉使用指定 WebView2 資料夾的殘留程序（msedgewebview2），否則資料夾無法刪除。 */
function killWebviewProcesses(dir) {
  const script = `Get-CimInstance Win32_Process | Where-Object { $_.CommandLine -like '*${dir.replaceAll("'", "''")}*' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }`;
  spawnSync("powershell", ["-NoProfile", "-Command", script], { stdio: "ignore" });
}

const dataDir = mkdtempSync(join(tmpdir(), "baton-smoke-data-"));
const projectDir = mkdtempSync(join(tmpdir(), "baton-smoke-project-"));
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

  // 5. 首次設定精靈與帳號頁（真實的 claude、git；不開登入終端機）
  const wizardText = await waitFor("精靈顯示 Claude Code 與 Git 的檢查結果", async () => {
    const text = await evaluate(debug1, "document.querySelector('.setup-wizard')?.innerText ?? ''");
    return text.includes("Claude Code") && text.includes("Git for Windows") ? text : null;
  });
  console.log(`✔ 首次設定：${wizardText.replace(/\s+/g, " ").trim().slice(0, 160)}`);
  if (!wizardText.includes("背景")) throw new Error("精靈沒有顯示背景執行行為的告知");
  await waitFor("填入 Profile 名稱", () => evaluate(debug1, fillExpression("名稱", "冒煙測試帳號")));
  await waitFor("按下新增 Profile", () => evaluate(debug1, clickExpression("新增 Profile")));
  await waitFor("帳號頁列出新的 Profile", async () => {
    const text = await evaluate(debug1, "document.querySelector('.accounts-page')?.innerText ?? ''");
    return text.includes("冒煙測試帳號") && text.includes("尚未核對") ? text : null;
  }).catch(async (error) => {
    const page = await evaluate(debug1, "document.querySelector('.accounts-page')?.innerText ?? ''");
    throw new Error(`${error.message}；帳號頁目前內容：${String(page).replace(/\s+/g, " ")}`);
  });
  await waitFor("按下重新核對", () => evaluate(debug1, clickExpression("重新核對 冒煙測試帳號")));
  const verifiedText = await waitFor("核對後顯示登入狀態", async () => {
    const text = await evaluate(debug1, "document.querySelector('.accounts-page')?.innerText ?? ''");
    return /未登入|已阻擋派工|可派工/.test(text) && !text.includes("尚未核對") ? text : null;
  });
  console.log(`✔ 帳號頁核對（真實 claude auth status）：${verifiedText.replace(/\s+/g, " ").trim().slice(0, 160)}`);
  const listed = await callApi(runtimePort, "/api/v1/profiles");
  const created = listed.body.profiles.find((p) => p.name === "冒煙測試帳號");
  if (!created || !existsSync(created.config_dir) || !created.config_dir.startsWith(dataDir)) {
    throw new Error(`Profile 的設定目錄不在資料目錄之下或不存在：${JSON.stringify(created)}`);
  }
  for (let n = 2; n <= 8; n++) {
    const added = await callApi(runtimePort, "/api/v1/profiles", "POST", { name: `帳號 ${n}` });
    if (added.status !== 201) throw new Error(`第 ${n} 個 Profile 應該成功：${JSON.stringify(added)}`);
  }
  const ninth = await callApi(runtimePort, "/api/v1/profiles", "POST", { name: "帳號 9" });
  if (ninth.status !== 409 || ninth.body.error !== "profile_limit_reached" || !ninth.body.message.includes("8")) {
    throw new Error(`第 9 個 Profile 應該被拒絕並說明上限：${JSON.stringify(ninth)}`);
  }
  console.log(`✔ 第 9 個 Profile 被拒絕：${ninth.body.message}`);
  console.log(`✔ Runtime 已由 App 啟動（pid ${first.pid}，boot_count ${first.boot_count}）`);

  // 2. 結束 App 程序（關窗只會隱藏到系統匣，App 不會結束）：Runtime 繼續執行
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

  // 4. 在 App 內建立 Workspace → 授權設定 → 確認 P3；確認前後的准入檢查結果不同
  const api = async (path, init = {}) =>
    fetch(`http://127.0.0.1:${runtimePort}${path}`, {
      ...init,
      headers: {
        Authorization: `Bearer ${readDesktopCredential(dataDir)}`,
        "X-Baton-Client": "baton-desktop",
        "content-type": "application/json",
      },
    });
  const step = (description, expression) => waitFor(description, async () => (await evaluate(debug2, expression)) === true);
  await step("輸入目錄", ui.type("目錄", projectDir));
  await step("建立 Workspace", ui.click("建立 Workspace"));
  await waitFor("建立後進入授權設定", async () => (await evaluate(debug2, ui.text(".authorization"))).includes("尚未確認授權"));
  const { workspaces } = await (await api("/api/v1/workspaces")).json();
  const workspaceId = workspaces.at(-1).id;
  const needsBuild = () =>
    api(`/api/v1/workspaces/${workspaceId}/admission-check`, {
      method: "POST",
      body: JSON.stringify({ required_capabilities: ["run_build_test"] }),
    });
  const before = await needsBuild();
  if (before.status !== 409 || (await before.json()).error !== "authorization_unconfirmed") {
    throw new Error("確認授權之前，需要建置與測試的 Run 應被准入檢查拒絕");
  }
  console.log("✔ 建立 Workspace 後進入授權設定；未確認前准入檢查拒絕需要能力的 Run");
  await step("套用 P3 範本", ui.click("套用「可信專案開發（P3）」"));
  await step("檢視確認摘要", ui.click("檢視確認摘要"));
  await waitFor("摘要揭露可信程式風險", async () =>
    (await evaluate(debug2, ui.text(".authorization-summary"))).includes("Windows 使用者的身分執行"),
  );
  await step("承認風險", ui.check("我了解並接受上述風險"));
  await step("確認並保存授權", ui.click("確認並保存授權"));
  await waitFor("保存為 revision 1", async () => (await evaluate(debug2, ui.text(".authorization"))).includes("已保存為 revision 1"));
  const after = await needsBuild();
  if (after.status !== 200 || (await after.json()).authorization_revision !== 1) {
    throw new Error("確認授權之後，需要建置與測試的 Run 應通過准入檢查");
  }
  console.log("✔ 在 App 內確認 P3 授權（revision 1）後，准入檢查通過");

  // 5. 不帶憑證的請求（例如瀏覽器中的其他程式）被拒絕
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
  for (const dir of [dataDir, webviewDir, projectDir]) rmSync(dir, { recursive: true, force: true });
}
