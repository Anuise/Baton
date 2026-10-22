// 建置與測試腳本共用的小工具
import { spawnSync } from "node:child_process";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");

/** 執行指令並沿用目前的終端機輸出；失敗時回傳非 0 的結束碼。 */
export function run(command, args) {
  console.log(`\n> ${[command, ...args].join(" ")}`);
  const result = spawnSync(command, args, { cwd: repoRoot, stdio: "inherit", shell: command === "pnpm" });
  if (result.error) {
    console.error(`無法執行 ${command}：${result.error.message}`);
    return 1;
  }
  return result.status ?? 1;
}

/** 執行會呼叫 rustc 的指令；Windows 上經由 with-msvc.ps1 確保找得到 C++ 連結器與函式庫。 */
export function withToolchain(command, args) {
  if (process.platform === "win32") {
    return run("powershell", ["-NoProfile", "-File", "scripts/with-msvc.ps1", command, ...args]);
  }
  return run(command, args);
}

export const cargo = (args) => withToolchain("cargo", args);
