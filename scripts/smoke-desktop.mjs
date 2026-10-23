// 桌面 App 冒煙測試：node scripts/smoke-desktop.mjs（需先 pnpm build）
//
// 以暫存資料目錄與隨機埠啟動真正的 baton-desktop.exe，透過 WebView2 的遠端除錯埠讀取畫面，確認：
//   1. App 啟動後自動啟動 Runtime，狀態列顯示「Runtime 運作中」
//   2. App 程序結束後 Runtime 仍在執行（關閉視窗只會隱藏到系統匣，所以這裡直接結束 App 程序）
//   3. 再次開啟 App 時連回同一個 Runtime（boot_count 不增加、pid 不變）
//   4. 在 App 內建立 Workspace 後切到「授權」頁簽，套用 P3 範本、檢視摘要、承認風險並確認保存；
//      確認之前需要「建置與測試」的 Run 被准入檢查拒絕，確認之後通過
//   5. 不帶憑證的請求被 Runtime 拒絕（桌面憑證由 App 自動讀取 DPAPI 保護檔，不需人工複製）
//   6. 首次設定精靈以本機真實的 claude 與 git 檢查；帳號頁新增 Profile 並以真實的 claude auth status 核對，
//      第 9 個 Profile 被拒絕（不會開啟登入終端機，避免在測試中彈出視窗）
//   7. 強制結束 Runtime（不結束監護）：監護重新拉起 Runtime，恢復面板與狀態列顯示恢復事件
//   8. 完全退出（最後一步，會停掉 Runtime）：系統匣選單無法由腳本點擊，改在頁面內以 Tauri IPC 發出
//      系統匣送出的同一個事件；畫面顯示「完全退出」與停止進度、Runtime 進入 stopped，
//      最後 App、Runtime 與監護都結束
//   「派工到完成」與「裁定中心批次處理」需要 Worker／Controller 實際執行（只有 Agent 憑證能建立裁定請求），
//   不在本腳本範圍內，由 harness 情境測試涵蓋
import { spawn, spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
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

/** 以 CDP 在第一個頁面執行運算式並回傳結果；`awaitPromise` 時等運算式的 Promise 完成。 */
async function evaluate(debugPort, expression, awaitPromise = false) {
  const targets = await (await fetch(`http://127.0.0.1:${debugPort}/json`)).json();
  const page = targets.find((t) => t.type === "page");
  if (!page) throw new Error("尚無頁面");
  const socket = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise((ok, fail) => {
    socket.onopen = ok;
    socket.onerror = () => fail(new Error("CDP 連線失敗"));
  });
  const result = await new Promise((ok, fail) => {
    socket.onmessage = (event) => ok(JSON.parse(event.data));
    // 頁面在回應前關閉（例如 App 結束）時不要永遠等下去
    socket.onclose = () => fail(new Error("CDP 連線在回應前關閉"));
    socket.send(
      JSON.stringify({ id: 1, method: "Runtime.evaluate", params: { expression, returnByValue: true, awaitPromise } }),
    );
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
  // 點頁簽（role=tab，以文字比對）；畫面上沒有這個頁簽時回傳 false。
  // 冒煙環境的 Profile 都未登入，App 會以全畫面顯示首次精靈：這時先按「先進入主畫面」，下一輪再點頁簽
  tab: (text) =>
    `(() => { const t = [...document.querySelectorAll('[role=tab]')].find((t) => t.innerText.trim() === ${JSON.stringify(text)}); if (!t) { [...document.querySelectorAll('button')].find((b) => b.innerText.trim() === '先進入主畫面')?.click(); return false; } t.click(); return true; })()`,
};

/** 系統匣「完全退出」送給主視窗的事件（src-tauri 的 FULL_EXIT_EVENT）。 */
const FULL_EXIT_EVENT = "baton://full-exit-requested";

const runtimeUrl = (port) =>`http://127.0.0.1:${port}/api/v1/health`;

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

/** 監護與 Runtime 記在資料目錄 guardian\ 下的程序識別碼（guardian.json、runtime.json）；沒有記錄時為 undefined。 */
function recordedPid(dir, file) {
  try {
    return JSON.parse(readFileSync(join(dir, "guardian", file), "utf8")).pid;
  } catch {
    return undefined;
  }
}

/** 強制結束指定程序並等待 taskkill 完成。加上映像名稱篩選：記錄檔裡的 PID 若已被其他程序重用，不會誤殺。 */
function killImage(pid, image) {
  spawnSync("taskkill", ["/PID", String(pid), "/FI", `IMAGENAME eq ${image}`, "/F"], { stdio: "ignore" });
}

/** 指定 PID 的程序是否還在，且映像名稱相符。 */
function isRunning(pid, image) {
  const result = spawnSync("tasklist", ["/FI", `PID eq ${pid}`, "/FI", `IMAGENAME eq ${image}`, "/NH"], {
    encoding: "utf8",
  });
  return result.stdout.toLowerCase().includes(image);
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
// 在 try 外宣告：中途失敗時 finally 也要結束它們
let app1;
let app2;

try {
  const launchEnv = (debugPort) => ({
    BATON_DATA_DIR: dataDir,
    BATON_PORT: String(runtimePort),
    WEBVIEW2_USER_DATA_FOLDER: webviewDir,
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${debugPort}`,
  });

  // 1. 第一次開啟：App 啟動 Runtime，狀態列顯示運作中
  const debug1 = await freePort();
  app1 = launchApp(launchEnv(debug1));
  const bar = await waitFor("狀態列顯示 Runtime 運作中", async () => {
    const text = await evaluate(debug1, "document.querySelector('.status-bar')?.innerText ?? ''");
    return text.includes("Runtime 運作中") ? text : null;
  });
  console.log(`✔ 狀態列：${bar.trim()}`);
  // 平台辨識：狀態列顯示支援狀態（平台狀態頁在「設定」頁簽，新增 Profile 後再檢查）
  const platformBar = await waitFor("狀態列顯示平台狀態", async () => {
    const text = await evaluate(debug1, "document.querySelector('.status-bar')?.innerText ?? ''");
    return /平台：(正式支援|非正式支援)/.test(text) ? text : null;
  });
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
    // 有 Profile 後 App 離開全畫面首次精靈、改為頁簽外殼：帳號頁在「帳號」頁簽（仍是全畫面時沒有頁簽，不影響）
    await evaluate(debug1, ui.tab("帳號"));
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
  // 平台狀態頁（「設定」頁簽）顯示本機實際的診斷資訊
  const platformPage = await waitFor("平台狀態頁顯示診斷資訊", async () => {
    await evaluate(debug1, ui.tab("設定"));
    const text = await evaluate(debug1, "document.querySelector('.platform-page')?.innerText ?? ''");
    return text.includes("OS build") && text.includes("原生架構") && text.includes("App 版本") ? text : null;
  });
  console.log(`✔ ${platformBar.match(/平台：\S+/)[0]}；${platformPage.replace(/\s+/g, " ").trim()}`);
  console.log(`✔ Runtime 已由 App 啟動（pid ${first.pid}，boot_count ${first.boot_count}）`);

  // 2. 結束 App 程序（關窗只會隱藏到系統匣，App 不會結束）：Runtime 繼續執行
  killProcess(app1.pid);
  await sleep(2000);
  const afterClose = await health(runtimePort);
  if (afterClose.pid !== first.pid) throw new Error("關閉 App 後 Runtime 不是同一個程序");
  console.log("✔ 關閉 App 後 Runtime 仍在執行");

  // 3. 再次開啟：連回同一個 Runtime
  const debug2 = await freePort();
  app2 = launchApp(launchEnv(debug2));
  await waitFor("再次開啟後狀態列顯示 Runtime 運作中", async () => {
    const text = await evaluate(debug2, "document.querySelector('.status-bar')?.innerText ?? ''");
    return text.includes("Runtime 運作中");
  });
  const second = await health(runtimePort);
  if (second.pid !== first.pid || second.boot_count !== first.boot_count) {
    throw new Error(`再次開啟後不是同一個 Runtime：${JSON.stringify(second)}`);
  }
  console.log("✔ 再次開啟時連回同一個 Runtime（沒有重複啟動）");

  // 4. 在 App 內建立 Workspace →「授權」頁簽 → 確認 P3；確認前後的准入檢查結果不同
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
  // 同一個 WebView2 資料夾會記住上次的頂層頁簽（「設定」）：先切回「Workspace」頁簽才有側欄
  await step("切到 Workspace 頁簽", ui.tab("Workspace"));
  await step("輸入目錄", ui.type("目錄", projectDir));
  await step("建立 Workspace", ui.click("建立 Workspace"));
  // 建立後自動選中該 Workspace（預設 Controller 頁簽），切到「授權」頁簽
  await step("切到授權頁簽", ui.tab("授權"));
  await waitFor("授權頁簽顯示授權設定", async () => (await evaluate(debug2, ui.text(".authorization"))).includes("尚未確認授權"));
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
  console.log("✔ 建立 Workspace 後在「授權」頁簽顯示授權設定；未確認前准入檢查拒絕需要能力的 Run");
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

  // 7. 強制結束 Runtime、監護還在：監護確認終止後重新拉起 Runtime，新的 Runtime 判定崩潰並建立恢復事件
  await waitFor("監護運作中", async () => (await health(runtimePort)).guardian?.state === "active");
  killProcess(runtimePid);
  const restarted = await waitFor("監護重新拉起 Runtime", async () => {
    const now = await health(runtimePort);
    return now.pid !== first.pid && now.boot_count > first.boot_count ? now : null;
  });
  runtimePid = restarted.pid;
  const recovery = await waitFor("Runtime 接續監護的恢復嘗試", async () => {
    const { body } = await callApi(runtimePort, "/api/v1/recovery");
    return ["attempting", "observing"].includes(body.event?.status) ? body : null;
  });
  if (recovery.event.round !== 1 || recovery.event.attempts_used !== 1) {
    throw new Error(`恢復事件應是第 1 輪第 1 次嘗試（監護拉起的那次）：${JSON.stringify(recovery.event)}`);
  }
  console.log(
    `✔ 強制結束 Runtime 後由監護重新拉起（pid ${restarted.pid}，boot_count ${restarted.boot_count}），恢復事件 ${recovery.event.status}`,
  );
  const recoveryPanel = await waitFor("恢復面板顯示恢復事件", async () => {
    // 恢復面板在「設定」頁簽
    await evaluate(debug2, ui.tab("設定"));
    const text = await evaluate(debug2, ui.text(".recovery-panel"));
    return text.includes("偵測到崩潰") && text.includes("第 1 輪") ? text : null;
  });
  const recoveryBar = await waitFor("狀態列顯示恢復事件狀態", async () => {
    const text = await evaluate(debug2, ui.text(".status-recovery"));
    return text.includes("恢復中") ? text : null;
  });
  console.log(`✔ 狀態列「${recoveryBar.trim()}」；恢復面板：${recoveryPanel.replace(/\s+/g, " ").trim().slice(0, 160)}`);

  // 8. 完全退出（最後一步）：在頁面內發出系統匣「完全退出」送出的同一個 Tauri 事件，由 Web UI 接手停止並顯示進度
  const guardian = recordedPid(dataDir, "guardian.json");
  const token = readDesktopCredential(dataDir);
  const phases = new Set();
  // Runtime 進入 stopped 後只再提供 API 5 秒，所以與畫面同時輪詢
  const lifecycleStopped = waitFor("Runtime 的 lifecycle 進入 stopped", async () => {
    const response = await fetch(`http://127.0.0.1:${runtimePort}/api/v1/lifecycle`, {
      headers: { Authorization: `Bearer ${token}`, "X-Baton-Client": "baton-desktop" },
    });
    const status = await response.json();
    phases.add(status.phase);
    return status.phase === "stopped" ? status : null;
  });
  // 記下停止進度畫面的每個樣子；看到「正在關閉 Baton」就回傳（App 約 1 秒後結束，頁面也隨之消失）
  const exitScreens = evaluate(
    debug2,
    `new Promise((resolve) => {
      const screens = [];
      const finish = () => { observer.disconnect(); resolve(screens); };
      const observer = new MutationObserver(() => {
        const text = document.querySelector('.exit-progress')?.innerText;
        if (!text) return;
        if (screens.at(-1) !== text) screens.push(text);
        if (text.includes('正在關閉 Baton')) finish();
      });
      observer.observe(document.body, { subtree: true, childList: true, characterData: true });
      setTimeout(finish, 30000);
      window.__TAURI_INTERNALS__
        .invoke('plugin:event|emit', { event: ${JSON.stringify(FULL_EXIT_EVENT)}, payload: null })
        .catch((error) => { screens.push('發出事件失敗：' + error); finish(); });
    })`,
    true,
  );
  const [screens, stopped] = await Promise.all([exitScreens, lifecycleStopped]);
  const joined = screens.join("\n");
  if (!joined.includes("完全退出") || !/停止中|已停止/.test(joined) || !screens.at(-1)?.includes("正在關閉 Baton")) {
    throw new Error(`完全退出畫面沒有顯示停止進度：${JSON.stringify(screens)}`);
  }
  if (!stopped.stop_intent_active || !existsSync(join(dataDir, "lifecycle.json"))) {
    throw new Error(`完全退出後停止意圖應生效並鏡像到 lifecycle.json：${JSON.stringify(stopped)}`);
  }
  console.log(
    `✔ 完全退出畫面：${screens.at(-1).replace(/\s+/g, " ").trim()}；Runtime lifecycle：${[...phases].join(" → ")}`,
  );
  await waitFor("App 自行結束", () => app2.exitCode !== null, 20_000);
  await waitFor("Runtime 結束", () => !isRunning(runtimePid, "baton-runtime.exe"), 20_000);
  // 監護看到停止意圖就不重啟 Runtime，自己結束
  if (guardian) await waitFor("監護結束", () => !isRunning(guardian, "baton-guardian.exe"), 20_000);
  console.log("✔ 完全退出後 App、Runtime 與監護都已結束");
  console.log("冒煙測試通過");
} catch (error) {
  console.error(`冒煙測試失敗：${error.message}`);
  process.exitCode = 1;
} finally {
  // 中途失敗時 App 可能還在
  for (const app of [app1, app2]) {
    if (app && app.exitCode === null) killImage(app.pid, "baton-desktop.exe");
  }
  // 先結束監護再結束 Runtime：否則監護會把被強制結束的 Runtime 當成意外終止而重啟它
  const guardian = recordedPid(dataDir, "guardian.json");
  if (guardian) killImage(guardian, "baton-guardian.exe");
  // 取得 runtimePid 之前就失敗時，改用 Runtime 在資料目錄記下的程序識別碼；監護重啟過時兩者可能不同
  for (const pid of new Set([runtimePid, recordedPid(dataDir, "runtime.json")])) {
    if (pid) killImage(pid, "baton-runtime.exe");
  }
  killWebviewProcesses(webviewDir);
  await sleep(1000);
  for (const dir of [dataDir, webviewDir, projectDir]) rmSync(dir, { recursive: true, force: true });
}
