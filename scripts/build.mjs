// 一條指令建置 Runtime、Web UI 與 Tauri 桌面 App：pnpm build
// 加上 --bundle 會另外產生 NSIS 安裝程式（需要下載 NSIS，首次較慢）。
import { copyFileSync, mkdirSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";

import { cargo, repoRoot, withToolchain } from "./lib.mjs";

const bundle = process.argv.includes("--bundle");

// 1. Runtime、監護與 MCP shim（桌面 App 與它們放在同一個資料夾，啟動時由 App 拉起；shim 由 Runtime 從自己旁邊啟動）
let code = cargo(["build", "--release", "-p", "baton-runtime", "-p", "baton-guardian", "-p", "baton-mcp-shim"]);
if (code !== 0) process.exit(code);

const tauriArgs = ["tauri", "build"];
if (bundle) {
  // 安裝程式把 Runtime、監護與 MCP shim 當成 sidecar 一起打包，安裝後與 baton-desktop.exe 同資料夾
  const triple = "x86_64-pc-windows-msvc";
  const binaries = resolve(repoRoot, "src-tauri", "binaries");
  mkdirSync(binaries, { recursive: true });
  for (const name of ["baton-runtime", "baton-guardian", "baton-mcp-shim"]) {
    copyFileSync(resolve(repoRoot, "target", "release", `${name}.exe`), resolve(binaries, `${name}-${triple}.exe`));
  }
  tauriArgs.push("--config", "src-tauri/tauri.bundle.conf.json");
  // 有簽章私鑰與公鑰時另外產生更新用的簽章檔（*-setup.exe.sig），供更新清單 latest.json 引用。
  // Tauri CLI 產生簽章檔需要 plugins.updater 設定，並會核對私鑰與公鑰是否成對
  const { TAURI_SIGNING_PRIVATE_KEY, BATON_UPDATE_ENDPOINT, BATON_UPDATE_PUBKEY } = process.env;
  if (TAURI_SIGNING_PRIVATE_KEY && BATON_UPDATE_PUBKEY) {
    const updaterConf = "target/tauri.updater.conf.json";
    const updater = { pubkey: BATON_UPDATE_PUBKEY, endpoints: BATON_UPDATE_ENDPOINT ? [BATON_UPDATE_ENDPOINT] : [] };
    writeFileSync(
      resolve(repoRoot, updaterConf),
      JSON.stringify({ bundle: { createUpdaterArtifacts: true }, plugins: { updater } }),
    );
    tauriArgs.push("--config", updaterConf);
  } else if (TAURI_SIGNING_PRIVATE_KEY) {
    console.log("\n注意：有 TAURI_SIGNING_PRIVATE_KEY 但沒有 BATON_UPDATE_PUBKEY，不產生更新用的簽章檔。");
  }
  if (!BATON_UPDATE_ENDPOINT || !BATON_UPDATE_PUBKEY) {
    console.log(
      "\n注意：未設定 BATON_UPDATE_ENDPOINT（Tauri 格式 latest.json 的網址）與 BATON_UPDATE_PUBKEY（tauri signer generate 的公鑰），" +
        "這個版本無法檢查與驗證更新，更新狀態會顯示「未設定更新來源」。兩者在建置時讀入 Runtime。",
    );
  }
} else {
  tauriArgs.push("--no-bundle");
}

// 2. Web UI（由 tauri 的 beforeBuildCommand 建置）與桌面 App
code = withToolchain("pnpm", tauriArgs);
process.exit(code);
