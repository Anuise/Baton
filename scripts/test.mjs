// 一次跑完 Rust 與前端測試：node scripts/test.mjs（或 pnpm test）
import { cargo, run } from "./lib.mjs";

// 桌面殼需要先建置 web/dist，不在預設成員內；其餘 crate 全部測試
const results = [
  ["Rust", cargo(["test", "--workspace", "--exclude", "baton-desktop"])],
  ["前端", run("pnpm", ["--filter", "web", "test"])],
];

console.log("\n=== 測試結果 ===");
for (const [name, code] of results) console.log(`${name}：${code === 0 ? "通過" : `失敗（結束碼 ${code}）`}`);
process.exit(results.every(([, code]) => code === 0) ? 0 : 1);
