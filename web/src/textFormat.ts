/**
 * 長文字的縮短與數字格式（design-system/baton/MASTER.md 6.1、5.4）。
 *
 * 清單／卡片中顯示縮短形式，完整值一定另有取得路徑：`title` 屬性 + 複製按鈕（`CopyButton`），
 * 或點開詳情；詳情中顯示完整值（等寬、`.mono`，可在任何字元斷開）。
 *
 * 用法：
 * ```tsx
 * // 識別碼：前綴 + 主體前 6 碼 … 後 4 碼
 * <span className="copy-text">
 *   <code className="mono" title={task.id}>{shortId(task.id)}</code>
 *   <CopyButton text={task.id} label="複製識別碼" />
 * </span>
 * // 雜湊：前 12 字…
 * <code className="mono" title={sha}>{shortHash(sha)}</code>
 * // 路徑：保留磁碟（或網域）與最後 2 段
 * <span className="mono truncate" title={workspace.path}>{shortPath(workspace.path)}</span>
 * // 計數徽章：超過 999 顯示 999+，完整數字放在可存取名稱與 title
 * <span className="badge badge-warning" aria-label={`待裁定 ${formatNumber(n)} 項`} title={formatNumber(n)}>
 *   待裁定 {formatCount(n)}
 * </span>
 * // 選單選項：60 字以內（標題前段 + 識別碼後 4 碼）
 * <option value={task.id}>{optionLabel(task.title, task.id)}</option>
 * // 錯誤：第一句完整顯示，其餘收進「技術細節」
 * const { summary, rest } = splitFirstSentence(error);
 * ```
 */

const ELLIPSIS = "…";

/** 以字元（code point）計算長度，不切開代理對。 */
function chars(text: string): string[] {
  return Array.from(text);
}

/**
 * 識別碼的中間省略：`task_7b5deb026d9841b0cced7314` → `task_7b5deb…7314`。
 * 有英文前綴（`task_`、`run_`、`ws_`…）時保留前綴與主體前 6 碼，沒有前綴時保留前 10 碼；都再接後 4 碼。
 */
export function shortId(id: string): string {
  const prefix = /^[A-Za-z]+_/.exec(id)?.[0] ?? "";
  const body = id.slice(prefix.length);
  const head = prefix === "" ? 10 : 6;
  const tail = 4;
  if (body.length <= head + tail + 1) return id;
  return `${prefix}${body.slice(0, head)}${ELLIPSIS}${body.slice(-tail)}`;
}

/** 雜湊（SHA-256、commit）：前 12 字 + `…`；不超過 12 字時原樣傳回。 */
export function shortHash(hash: string): string {
  return hash.length <= 12 ? hash : `${hash.slice(0, 12)}${ELLIPSIS}`;
}

/**
 * 路徑與 URL：保留開頭（磁碟、UNC 的伺服器與共用名稱、URL 的網域、POSIX 的 /）與最後 `keep` 段，
 * 中間以 `…` 省略：`C:\Users\…\fixtures\report.md` → `C:\…\fixtures\report.md`。段數不多時原樣傳回。
 */
export function shortPath(path: string, keep = 2): string {
  const root = /^(?:[A-Za-z]:[\\/]|\\\\[^\\]+\\[^\\]+\\|[A-Za-z][\w+.-]*:\/\/[^/]+\/|\/)/.exec(path)?.[0] ?? "";
  const sep = path.includes("\\") ? "\\" : "/";
  const segments = path.slice(root.length).split(/[\\/]/).filter((segment) => segment !== "");
  if (segments.length <= keep + 1) return path;
  return `${root}${ELLIPSIS}${sep}${segments.slice(-keep).join(sep)}`;
}

/** 路徑與檔名中可以當作切點的字元：分隔符號、點、底線、連字號、空白。 */
const BOUNDARY = /[\\/._\- ]/;

/**
 * `shortPath` 拆成三段，給「中段省略、保留結尾」的顯示（LongText.tsx 的 `PathText`）：
 * `head` 是開頭與省略號（`C:\…`，沒有省略時為空）、`start` 是窄時先截斷的中段、`end` 是最後最多 `endChars` 字，
 * 從範圍內最靠前的分隔字元後面開始（`…revision-2026-10-28.md` → `10-28.md`），範圍內沒有分隔字元時直接取最後幾字。
 * 三段相接等於 `shortPath(path, keep)`。
 */
export function shortPathParts(path: string, keep = 2, endChars = 10): { head: string; start: string; end: string } {
  const short = shortPath(path, keep);
  const omitted = short.indexOf(ELLIPSIS);
  const head = short === path || omitted < 0 ? "" : short.slice(0, omitted + 1);
  const rest = chars(short.slice(head.length));
  if (rest.length <= endChars) return { head, start: "", end: rest.join("") };
  let cut = rest.length - endChars;
  while (cut < rest.length - 1 && !BOUNDARY.test(rest[cut - 1])) cut++;
  if (cut === rest.length - 1) cut = rest.length - endChars;
  return { head, start: rest.slice(0, cut).join(""), end: rest.slice(cut).join("") };
}

/**
 * 檔名的副檔名（最後一個 `.` 之後、1～10 個英數字）：`report.md` → `{ base: "report", ext: ".md" }`。
 * 點開頭的檔名（`.gitignore`）、點在資料夾名稱中、「副檔名」含空白或其他文字時 `ext` 為空。
 */
export function splitExtension(name: string): { base: string; ext: string } {
  const match = /(?<=[^\\/.])\.[A-Za-z0-9]{1,10}$/.exec(name);
  return match === null ? { base: name, ext: "" } : { base: name.slice(0, match.index), ext: match[0] };
}

/** 計數徽章：超過 `max`（預設 999）顯示 `999+`。完整數字以 `formatNumber` 放在可存取名稱與 title。 */
export function formatCount(count: number, max = 999): string {
  return count > max ? `${max}+` : String(count);
}

const numberFormat = new Intl.NumberFormat("zh-TW");

/** 千分位：`1284` → `1,284`。 */
export function formatNumber(value: number): string {
  return numberFormat.format(value);
}

const BYTE_UNITS = ["KB", "MB", "GB", "TB"];

/** 位元組換算（以 1024 為級距、最多 2 位小數）：`1234567892` → `1.15 GB`；小於 1 KB 時 `512 位元組`。 */
export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} 位元組`;
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < BYTE_UNITS.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${Number(value.toFixed(2))} ${BYTE_UNITS[unit]}`;
}

/** 截斷到 `max` 個字元以內（含結尾的 `…`）；不超過時原樣傳回。 */
export function truncateText(text: string, max: number): string {
  const list = chars(text);
  return list.length <= max ? text : `${list.slice(0, max - 1).join("")}${ELLIPSIS}`;
}

/**
 * 原生 `<select>` 的選項文字（原生選單以最長選項為最小寬度）：標題前段 + `（識別碼後 4 碼）`，總長不超過 `max`（預設 60）。
 */
export function optionLabel(title: string, id?: string, max = 60): string {
  if (id === undefined) return truncateText(title, max);
  const suffix = `（${id.slice(-4)}）`;
  return `${truncateText(title, max - suffix.length)}${suffix}`;
}

/**
 * 錯誤與警告：分出第一句（摘要，完整顯示）與其餘（技術細節，收進 `<details>`）。
 * 句末是全形 `。！？`、換行，或後面接空白的半形 `.!?`（路徑、版本號中的點不算）。
 */
export function splitFirstSentence(text: string): { summary: string; rest: string } {
  const match = /[。！？]|[.!?](?=\s)|\n/.exec(text);
  if (match === null) return { summary: text.trim(), rest: "" };
  const end = match.index + (match[0] === "\n" ? 0 : match[0].length);
  return { summary: text.slice(0, end).trim(), rest: text.slice(end).trim() };
}

/** 檔案大小：小於 1 KB 只顯示位元組，否則換算單位並附千分位位元組（`1.15 GB（1,234,567,892 位元組）`）。 */
export function sizeLabel(bytes: number): string {
  return bytes < 1024 ? formatBytes(bytes) : `${formatBytes(bytes)}（${formatNumber(bytes)} 位元組）`;
}

/** 網址只顯示網域（完整網址放在 title，另可複製）；不是網址時退回縮短路徑。首次精靈的說明連結、平台頁的資料來源用這個。 */
export function docsHost(url: string): string {
  try {
    return new URL(url).host || shortPath(url);
  } catch {
    return shortPath(url);
  }
}
