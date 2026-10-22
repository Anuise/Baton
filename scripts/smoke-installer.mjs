// 安裝程式冒煙測試（手動驗收）：node scripts/smoke-installer.mjs（需先 pnpm build --bundle）
//
// 以目前的（非系統管理員）使用者身分，把 NSIS 安裝程式靜默安裝到暫存資料夾，確認：
//   1. 一般使用者帳號可完成安裝：結束碼 0，桌面 App、Runtime、監護與解除安裝程式都在安裝資料夾
//   2. 已安裝的 Runtime 能以暫存資料目錄啟動並回應
//   3. 開啟「登入時自動啟動」後解除安裝：Runtime 已結束、HKCU Run 值 Baton 已移除、資料目錄與控制資料庫仍在
//
// 會在目前使用者的登錄與捷徑留下安裝紀錄（解除安裝時移除）；已有 Baton 安裝或登入啟動登記時拒絕執行，避免動到真正的安裝。
import { spawn, spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, readdirSync, readFileSync, rmSync, statSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

import { repoRoot } from "./lib.mjs";

const RUN_KEY = "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const VALUE_NAME = "Baton";
const UNINSTALL_KEY = "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\Baton";
// Tauri 安裝程式記下安裝位置的機碼（發行者預設取 identifier 的第二段）；留著的話下次安裝會預設裝到暫存資料夾
const MANUFACTURER_KEY = "HKCU\\Software\\baton";
const INSTALL_LOCATION_KEY = `${MANUFACTURER_KEY}\\Baton`;

const bundleDir = resolve(repoRoot, "target", "release", "bundle", "nsis");
const setups = existsSync(bundleDir) ? readdirSync(bundleDir).filter((f) => f.endsWith("-setup.exe")) : [];
if (setups.length === 0) {
  console.error(`找不到 ${bundleDir}\\*-setup.exe，請先執行 pnpm build --bundle`);
  process.exit(1);
}
// 有多個版本時取最新建置的那個
const setup = setups.map((f) => join(bundleDir, f)).sort((a, b) => statSync(b).mtimeMs - statSync(a).mtimeMs)[0];

// `net session` 只有提升權限時才會成功；要驗證的是一般使用者身分
if (spawnSync("net", ["session"], { stdio: "ignore" }).status === 0) {
  console.error("目前是以系統管理員身分（提升權限）執行；請改用一般的終端機，才能驗證一般使用者可完成安裝");
  process.exit(1);
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const regExists = (key, value) =>
  spawnSync("reg", ["query", key, ...(value ? ["/v", value] : [])], { stdio: "ignore" }).status === 0;
const regDelete = (key, value) =>
  spawnSync("reg", ["delete", key, ...(value ? ["/v", value] : []), "/f"], { stdio: "ignore" });

for (const [key, value, what] of [
  [RUN_KEY, VALUE_NAME, "登入啟動登記（HKCU Run 值 Baton）"],
  [UNINSTALL_KEY, undefined, "Baton 的安裝紀錄"],
  [INSTALL_LOCATION_KEY, undefined, "Baton 的安裝位置紀錄"],
]) {
  if (regExists(key, value)) {
    console.error(`這台電腦的目前使用者已有${what}（${key}${value ? ` ${value}` : ""}），為避免影響真正的安裝，拒絕執行`);
    process.exit(1);
  }
}
const manufacturerKeyExisted = regExists(MANUFACTURER_KEY);

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

/** 以目前使用者身分還原 DPAPI 保護的桌面憑證（與 App 相同的來源）。 */
function readDesktopCredential(dir) {
  const file = join(dir, "auth", "desktop-credential.bin");
  const script = `Add-Type -AssemblyName System.Security; $b = [IO.File]::ReadAllBytes('${file.replaceAll("'", "''")}'); [Text.Encoding]::UTF8.GetString([Security.Cryptography.ProtectedData]::Unprotect($b, $null, 'CurrentUser'))`;
  const result = spawnSync("powershell", ["-NoProfile", "-Command", script], { encoding: "utf8" });
  if (result.status !== 0) throw new Error(`無法讀取桌面憑證：${result.stderr}`);
  return result.stdout.trim();
}

const isAlive = (pid) => {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
};

const installDir = mkdtempSync(join(tmpdir(), "baton-smoke-install-"));
const dataDir = mkdtempSync(join(tmpdir(), "baton-smoke-data-"));
const port = await freePort();
const env = { ...process.env, BATON_DATA_DIR: dataDir, BATON_PORT: String(port) };
const healthUrl = `http://127.0.0.1:${port}/api/v1/health`;
const uninstaller = join(installDir, "uninstall.exe");
let installed = false;
let uninstalled = false;
let runtime;

/** 解除安裝程式加上 `_?=` 會在原地同步執行（不複製到暫存再立即返回）。 */
function runUninstaller() {
  uninstalled = true;
  return spawnSync(uninstaller, ["/S", `_?=${installDir}`], { env, stdio: "inherit", timeout: 120_000 });
}

try {
  // 1. 以目前使用者身分靜默安裝（單使用者安裝，不需要系統管理員）
  console.log(`安裝程式：${setup}`);
  const install = spawnSync(setup, ["/S", `/D=${installDir}`], { stdio: "inherit", timeout: 300_000 });
  installed = true;
  if (install.status !== 0) throw new Error(`安裝程式結束碼 ${install.status}${install.error ? `：${install.error.message}` : ""}`);
  const files = ["baton-desktop.exe", "baton-runtime.exe", "baton-guardian.exe", "uninstall.exe"];
  const missing = files.filter((f) => !existsSync(join(installDir, f)));
  if (missing.length > 0) throw new Error(`安裝資料夾缺少 ${missing.join("、")}`);
  console.log(`✔ 一般使用者安裝完成：${installDir}（${files.join("、")}）`);

  // 2. 啟動已安裝的 Runtime；健康查詢需要憑證，任何 HTTP 回應（含 401／403）都表示它在監聽
  runtime = spawn(join(installDir, "baton-runtime.exe"), [], { env, stdio: "ignore", detached: true });
  runtime.unref();
  const status = await waitFor("已安裝的 Runtime 回應", async () => (await fetch(healthUrl)).status);
  const credential = await waitFor("讀取桌面憑證", async () => readDesktopCredential(dataDir));
  const health = await (
    await fetch(healthUrl, { headers: { Authorization: `Bearer ${credential}`, "X-Baton-Client": "baton-desktop" } })
  ).json();
  console.log(`✔ 已安裝的 Runtime 已啟動（未帶憑證回應 ${status}；pid ${health.pid}）`);

  // 3. 開啟「登入時自動啟動」：由 Runtime 以真正的路徑登記 HKCU Run 值 Baton
  const enable = await fetch(`http://127.0.0.1:${port}/api/v1/lifecycle/login-startup`, {
    method: "PUT",
    headers: { Authorization: `Bearer ${credential}`, "X-Baton-Client": "baton-desktop", "content-type": "application/json" },
    body: JSON.stringify({ enabled: true }),
  });
  if (enable.status !== 200) throw new Error(`開啟登入時自動啟動失敗：${enable.status} ${await enable.text()}`);
  if (!regExists(RUN_KEY, VALUE_NAME)) throw new Error("開啟登入時自動啟動後找不到 HKCU Run 值 Baton");
  console.log("✔ 已開啟登入時自動啟動（HKCU Run 值 Baton 已登記）");

  // 4. 靜默解除安裝：掛勾停止 Runtime 並移除登記；資料目錄保留
  const uninstall = runUninstaller();
  if (uninstall.status !== 0) {
    throw new Error(`解除安裝程式結束碼 ${uninstall.status}${uninstall.error ? `：${uninstall.error.message}` : ""}`);
  }
  await waitFor("Runtime 結束", async () => !isAlive(health.pid), 10_000);
  const stillAnswers = await fetch(healthUrl).then(
    () => true,
    () => false,
  );
  if (stillAnswers) throw new Error("解除安裝後 Runtime 仍在回應");
  console.log("✔ 解除安裝後 Runtime 已結束");
  if (regExists(RUN_KEY, VALUE_NAME)) throw new Error("解除安裝後 HKCU Run 值 Baton 仍在");
  console.log("✔ 解除安裝後 HKCU Run 值 Baton 已移除");
  if (existsSync(join(installDir, "baton-desktop.exe"))) throw new Error("解除安裝後安裝資料夾仍有 baton-desktop.exe");
  if (!existsSync(join(dataDir, "control.sqlite"))) throw new Error(`解除安裝後資料目錄不完整：${dataDir}`);
  console.log(`✔ 資料目錄保留（${join(dataDir, "control.sqlite")}）`);
  const lifecycleFile = join(dataDir, "lifecycle.json");
  if (existsSync(lifecycleFile)) console.log(`  生命週期檔：${readFileSync(lifecycleFile, "utf8").trim()}`);
  console.log("冒煙測試通過");
} catch (error) {
  console.error(`冒煙測試失敗：${error.message}`);
  process.exitCode = 1;
} finally {
  if (installed && !uninstalled && existsSync(uninstaller)) runUninstaller();
  if (runtime?.pid && isAlive(runtime.pid)) {
    spawnSync("taskkill", ["/PID", String(runtime.pid), "/FI", "IMAGENAME eq baton-runtime.exe", "/F"], { stdio: "ignore" });
  }
  // 測試前確認過這些都不存在，留下的都是這次測試產生的
  regDelete(RUN_KEY, VALUE_NAME);
  regDelete(UNINSTALL_KEY);
  regDelete(manufacturerKeyExisted ? INSTALL_LOCATION_KEY : MANUFACTURER_KEY);
  await sleep(1000);
  for (const dir of [installDir, dataDir]) rmSync(dir, { recursive: true, force: true });
}
