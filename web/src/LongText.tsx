import { useEffect, useId, useLayoutEffect, useRef, useState } from "react";

import { shortHash, shortId, shortPath } from "./textFormat";

/**
 * 長段落（說明、理由、Goal、契約內容、知識內文）：清單／卡片中限制行數，超過時附「展開」低調按鈕；
 * 全文一直在 DOM 中（只是視覺上截斷），螢幕報讀器讀得到。design-system/baton/MASTER.md 6.1。
 *
 * 用法：
 * ```tsx
 * <LongText text={task.goal} />                          // 3 行 + 展開（預設）
 * <LongText text={decision.question} lines={2} />        // 標題類 2 行
 * <LongText text={workflow.router_reason} copyLabel="複製理由" className="workflow-reason" />
 * ```
 * - `text` 是純文字；換行會保留（`white-space: pre-line`），連續空白合併。
 * - 外層是 `<div class="long-text">`（閱讀寬度上限 46em），內文是 `<p>`；不要放在 `<p>` 裡。
 * - 只有文字真的超過 `lines` 行時才出現「展開」（以 ResizeObserver 隨寬度重新判斷）。
 * - 詳情頁要完整顯示時不必用 LongText，直接輸出文字即可。
 */
export function LongText({
  text,
  lines = 3,
  copyLabel,
  className,
}: {
  /** 完整文字。 */
  text: string;
  /** 收合時最多顯示的行數。 */
  lines?: 1 | 2 | 3;
  /** 有值時附上複製全文的按鈕，值是按鈕文字（例：「複製理由」）。 */
  copyLabel?: string;
  /** 加在外層 `.long-text` 上的 class。 */
  className?: string;
}) {
  const id = useId();
  const bodyRef = useRef<HTMLParagraphElement>(null);
  const [expanded, setExpanded] = useState(false);
  const [overflowing, setOverflowing] = useState(false);

  useLayoutEffect(() => {
    const body = bodyRef.current;
    if (body === null || expanded) return;
    const measure = () => setOverflowing(body.scrollHeight > body.clientHeight + 1);
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(measure);
    observer.observe(body);
    return () => observer.disconnect();
  }, [text, lines, expanded]);

  const showToggle = expanded || overflowing;
  return (
    <div className={className === undefined ? "long-text" : `long-text ${className}`}>
      <p ref={bodyRef} id={id} className={expanded ? "long-text-body" : `long-text-body clamp-${lines}`}>
        {text}
      </p>
      {(showToggle || copyLabel !== undefined) && (
        <div className="long-text-actions">
          {showToggle && (
            <button
              type="button"
              className="btn-quiet btn-sm"
              aria-expanded={expanded}
              aria-controls={id}
              onClick={() => setExpanded((value) => !value)}
            >
              {expanded ? "收合" : "展開"}
            </button>
          )}
          {copyLabel !== undefined && <CopyButton text={text} label={copyLabel} />}
        </div>
      )}
    </div>
  );
}

/** 「已複製」「複製失敗」顯示多久後恢復原本的文字。 */
const FEEDBACK_MS = 1500;

/**
 * 複製完整文字的低調小按鈕，放在縮短的識別碼、雜湊、路徑旁邊（textFormat.ts 的 shortId／shortHash／shortPath）。
 *
 * 用法：
 * ```tsx
 * <span className="copy-text">
 *   <code className="mono" title={run.id}>{shortId(run.id)}</code>
 *   <CopyButton text={run.id} label="複製識別碼" />
 * </span>
 * ```
 * 按下後暫時顯示「已複製」或「複製失敗」（aria-live 讓螢幕報讀器唸出結果）。
 * 同一區塊有多個同名的複製按鈕時傳 `field`（例：「Task」「產生者 Run」），可存取名稱成為「複製識別碼（Task）」，
 * 仍包含看得到的按鈕文字。
 */
export function CopyButton({ text, label = "複製", field }: { text: string; label?: string; field?: string }) {
  const [status, setStatus] = useState<"idle" | "copied" | "failed">("idle");

  useEffect(() => {
    if (status === "idle") return;
    const timer = setTimeout(() => setStatus("idle"), FEEDBACK_MS);
    return () => clearTimeout(timer);
  }, [status]);

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(text);
      setStatus("copied");
    } catch {
      setStatus("failed");
    }
  };

  return (
    <button
      type="button"
      className="btn-quiet btn-sm"
      aria-live="polite"
      aria-label={field !== undefined && status === "idle" ? `${label}（${field}）` : undefined}
      onClick={() => void copy()}
    >
      {status === "copied" ? "已複製" : status === "failed" ? "複製失敗" : label}
    </button>
  );
}

const SHORTEN = { id: shortId, hash: shortHash, path: shortPath };
const COPY_LABELS = { id: "複製識別碼", hash: "複製雜湊", path: "複製路徑" };

/**
 * 識別碼、雜湊、路徑（MASTER.md 6.1）：等寬並縮短，完整值放在 title，旁邊附複製；
 * 本來就夠短時原樣顯示、不附複製。
 *
 * 用法：
 * ```tsx
 * <ShortValue value={artifact.task_id} kind="id" />
 * <ShortValue value={version.sha256} kind="hash" />
 * <ShortValue value={file.storage_path} kind="path" />
 * <ShortValue value={version.produced_by_run_id} kind="id" field="產生者 Run" />  // 同一區塊有多個時帶欄位名
 * ```
 */
export function ShortValue({ value, kind, field }: { value: string; kind: keyof typeof SHORTEN; field?: string }) {
  const short = SHORTEN[kind](value);
  if (short === value) return <code className="mono">{value}</code>;
  return (
    <span className="copy-text">
      <code className="mono" title={value}>
        {short}
      </code>
      <CopyButton text={value} label={COPY_LABELS[kind]} field={field} />
    </span>
  );
}
