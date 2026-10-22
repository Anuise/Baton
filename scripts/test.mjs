// 一次跑完 Rust、前端測試與 UI E2E：node scripts/test.mjs（或 pnpm test）
import { cargo, run } from "./lib.mjs";
import { runUiE2e } from "./test-e2e.mjs";

// 桌面殼需要先建置 web/dist，不在預設成員內；其餘 crate 全部測試（UI E2E 標為 ignored，由下面另外執行）
const results = [
  ["Rust", cargo(["test", "--workspace", "--exclude", "baton-desktop"])],
  ["前端", run("pnpm", ["--filter", "web", "test"])],
];
// UI E2E 需要 Playwright：找不到時標為略過（不算失敗，也不顯示為通過）
const e2e = runUiE2e();

console.log("\n=== 測試結果 ===");
for (const [name, code] of results) console.log(`${name}：${code === 0 ? "通過" : `失敗（結束碼 ${code}）`}`);
console.log(`UI E2E：${e2e.skipped ? `略過（${e2e.reason}）` : e2e.code === 0 ? "通過" : `失敗（結束碼 ${e2e.code}）`}`);
process.exit(results.every(([, code]) => code === 0) && e2e.code === 0 ? 0 : 1);
