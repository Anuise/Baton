// UI E2E（票 53）：node scripts/test-e2e.mjs（或 pnpm test:e2e）；pnpm test 也會呼叫。
//
// 以真實瀏覽器操作 web/dist，連到以 fake Port 啟動的真實 Runtime（crates/baton-harness/tests/ui_e2e）。
// - 找不到 Playwright 時標為「略過」，不顯示為通過；設定了 BATON_PLAYWRIGHT_DIR 卻找不到時失敗。
// - 測試在專用的暫存目錄（TEMP／TMP）中執行，結束後確認裡面沒有遺留任何東西再刪除。
import { mkdtempSync, readdirSync, rmSync } from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

import { cargo, repoRoot, run } from "./lib.mjs";

/** Playwright 從哪裡解析（與 driver.mjs 的順序相同）；找不到時為 null。 */
function findPlaywright(explicit) {
  const bases = explicit ? [explicit] : [repoRoot, resolve(repoRoot, "web")];
  for (const base of bases) {
    const require = createRequire(join(resolve(base), "package.json"));
    for (const name of ["playwright", "@playwright/test"]) {
      try {
        require.resolve(name);
        return base;
      } catch {
        // 換下一個位置
      }
    }
  }
  return null;
}

/** 暫存目錄中遺留的項目；harness 共用的空 `baton-harness` 目錄不算（見 docs/development.md）。 */
function leftovers(dir) {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const path = join(dir, entry.name);
    if (entry.name === "baton-harness" && entry.isDirectory() && readdirSync(path).length === 0) return [];
    return [path];
  });
}

/** 執行 UI E2E；回傳 { code, skipped, reason }。 */
export function runUiE2e() {
  const explicit = process.env.BATON_PLAYWRIGHT_DIR || "";
  const playwright = findPlaywright(explicit);
  if (!playwright) {
    if (explicit) {
      console.error(`\n已設定 BATON_PLAYWRIGHT_DIR=${explicit}，但那裡找不到 playwright。`);
      return { code: 1, skipped: false };
    }
    const reason = "找不到 Playwright；設定 BATON_PLAYWRIGHT_DIR 指向已安裝 playwright 與其瀏覽器的目錄後再執行";
    console.log(`\nUI E2E 略過：${reason}`);
    return { code: 0, skipped: true, reason };
  }
  console.log(`\nUI E2E 使用 ${playwright} 的 Playwright`);

  let code = run("pnpm", ["--filter", "web", "build"]);
  if (code !== 0) return { code, skipped: false };
  const testArgs = ["test", "-p", "baton-harness", "--test", "ui_e2e"];
  code = cargo([...testArgs, "--no-run"]);
  if (code !== 0) return { code, skipped: false };

  // 只有執行測試時換成專用的暫存目錄，編譯產生的暫存檔不算在內
  const temp = mkdtempSync(join(tmpdir(), "baton-e2e-"));
  const saved = { TEMP: process.env.TEMP, TMP: process.env.TMP };
  Object.assign(process.env, { TEMP: temp, TMP: temp });
  try {
    code = cargo([...testArgs, "--", "--ignored"]);
  } finally {
    for (const [name, value] of Object.entries(saved)) {
      if (value === undefined) delete process.env[name];
      else process.env[name] = value;
    }
  }
  const left = leftovers(temp);
  if (left.length > 0) {
    console.error(`\nUI E2E 結束後暫存目錄仍有遺留：\n${left.join("\n")}`);
    code = code || 1;
  }
  rmSync(temp, { recursive: true, force: true });
  return { code, skipped: false };
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  const { code, skipped } = runUiE2e();
  console.log(`\n=== UI E2E：${skipped ? "略過（沒有執行，不代表通過）" : code === 0 ? "通過" : `失敗（結束碼 ${code}）`} ===`);
  process.exit(code);
}
