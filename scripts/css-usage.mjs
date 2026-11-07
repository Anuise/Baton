// 比對 Web UI 的 class 使用情形：元件（.tsx）用到卻沒有任何 CSS 定義的 class，以及 CSS 定義了卻沒有元件使用的 class。
// 只做靜態掃描：className 字串字面值與樣板字串中的固定片段；動態組出的 class（例如 `badge-${kind}`）以前綴比對。
//
// 用法：node scripts/css-usage.mjs
import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";

import { repoRoot } from "./lib.mjs";

const src = path.join(repoRoot, "web", "src");
const tsxFiles = readdirSync(src).filter((name) => name.endsWith(".tsx") && !name.endsWith(".test.tsx"));
const cssDir = path.join(src, "styles");
const cssFiles = readdirSync(cssDir).filter((name) => name.endsWith(".css"));

/** 元件中的 class：className 的字串、樣板字串的固定片段；`${` 前的片段視為前綴。 */
const used = new Map();
const prefixes = new Set();
for (const file of tsxFiles) {
  const text = readFileSync(path.join(src, file), "utf8");
  for (const match of text.matchAll(/className=(?:"([^"]*)"|\{`([^`]*)`\}|\{([^}]*)\})/g)) {
    const raw = match[1] ?? match[2] ?? match[3] ?? "";
    // 表達式中的字串字面值
    const pieces = match[3] !== undefined ? [...raw.matchAll(/["'`]([^"'`]*)["'`]/g)].map((m) => m[1]) : [raw];
    for (const piece of pieces) {
      for (const token of piece.split(/\s+/)) {
        if (token === "") continue;
        if (token.includes("${")) {
          const prefix = token.slice(0, token.indexOf("${"));
          if (prefix.length > 2) prefixes.add(prefix);
          continue;
        }
        if (!/^[a-z][a-z0-9-]*$/.test(token)) continue;
        if (!used.has(token)) used.set(token, new Set());
        used.get(token).add(file);
      }
    }
  }
}

const defined = new Map();
for (const file of cssFiles) {
  const text = readFileSync(path.join(cssDir, file), "utf8").replace(/\/\*[\s\S]*?\*\//g, "");
  for (const match of text.matchAll(/\.([a-z][a-z0-9-]*)/g)) {
    if (!defined.has(match[1])) defined.set(match[1], new Set());
    defined.get(match[1]).add(file);
  }
}

const missing = [...used].filter(([name]) => !defined.has(name)).sort(([a], [b]) => a.localeCompare(b));
const unused = [...defined]
  .filter(([name]) => !used.has(name) && ![...prefixes].some((prefix) => name.startsWith(prefix)))
  .sort(([a], [b]) => a.localeCompare(b));

console.log(`元件用到、CSS 沒有定義（${missing.length}）：`);
for (const [name, files] of missing) console.log(`  ${name}  ← ${[...files].join(", ")}`);
console.log(`\nCSS 定義了、元件沒有使用（${unused.length}；動態前綴 ${[...prefixes].join(" ")} 已排除）：`);
for (const [name, files] of unused) console.log(`  ${name}  ← ${[...files].join(", ")}`);
