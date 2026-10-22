// 一條指令建置 Runtime、Web UI 與 Tauri 桌面 App：pnpm build
// 加上 --bundle 會另外產生 NSIS 安裝程式（需要下載 NSIS，首次較慢）。
import { copyFileSync, mkdirSync } from "node:fs";
import { resolve } from "node:path";

import { cargo, repoRoot, withToolchain } from "./lib.mjs";

const bundle = process.argv.includes("--bundle");

// 1. Runtime 與監護（桌面 App 與它們放在同一個資料夾，啟動時由 App 拉起）
let code = cargo(["build", "--release", "-p", "baton-runtime", "-p", "baton-guardian"]);
if (code !== 0) process.exit(code);

const tauriArgs = ["tauri", "build"];
if (bundle) {
  // 安裝程式把 Runtime 與監護當成 sidecar 一起打包，安裝後與 baton-desktop.exe 同資料夾
  const triple = "x86_64-pc-windows-msvc";
  const binaries = resolve(repoRoot, "src-tauri", "binaries");
  mkdirSync(binaries, { recursive: true });
  for (const name of ["baton-runtime", "baton-guardian"]) {
    copyFileSync(resolve(repoRoot, "target", "release", `${name}.exe`), resolve(binaries, `${name}-${triple}.exe`));
  }
  tauriArgs.push("--config", "src-tauri/tauri.bundle.conf.json");
} else {
  tauriArgs.push("--no-bundle");
}

// 2. Web UI（由 tauri 的 beforeBuildCommand 建置）與桌面 App
code = withToolchain("pnpm", tauriArgs);
process.exit(code);
