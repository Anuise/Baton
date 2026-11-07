import { useEffect, useRef, useState, type ReactNode, type RefObject } from "react";

/**
 * `<details>` 浮層的收起行為：開著時點浮層外面收起；Esc 收起並把焦點還給觸發的 `<summary>`。
 * Esc 常駐監聽在 document：點浮層空白處後焦點落到 body，按鍵事件不會經過 details；
 * 也不等 toggle 事件更新狀態才掛上，開啟後立刻按 Esc 也收得起來。
 * 回傳 `onToggle`，交給 `<details onToggle>`，讓外點監聽只在開著時存在。
 */
export function useDetailsDismiss(ref: RefObject<HTMLDetailsElement | null>) {
  const [open, setOpen] = useState(false);

  useEffect(() => {
    if (!open) return;
    const close = (event: PointerEvent) => {
      if (ref.current && !ref.current.contains(event.target as Node)) ref.current.open = false;
    };
    document.addEventListener("pointerdown", close);
    return () => document.removeEventListener("pointerdown", close);
  }, [open, ref]);

  useEffect(() => {
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || !ref.current?.open) return;
      ref.current.open = false;
      ref.current.querySelector("summary")?.focus();
    };
    document.addEventListener("keydown", closeOnEscape);
    return () => document.removeEventListener("keydown", closeOnEscape);
  }, [ref]);

  return (event: React.SyntheticEvent<HTMLDetailsElement>) => setOpen(event.currentTarget.open);
}

/**
 * 「更多」選單：物件中第二順位以後的動作（MASTER 第 7 節）。外觀是 components.css 的 `.more-menu`。
 *
 * 用法：
 * ```tsx
 * <MoreMenu ariaLabel={`更多（${profile.name}）`} align="end">
 *   <button type="button" className="btn-quiet" onClick={…}>上移</button>
 *   <button type="button" className="btn-quiet more-menu-danger" onClick={…}>封存</button>
 * </MoreMenu>
 * ```
 * - 選項是 `.more-menu-list` 裡的按鈕；破壞性選項加 `.more-menu-danger`。
 * - `align="end"` 往左展開（放在列尾時用），不超出視窗。
 * - 選了選項後不會自動收起；需要時由呼叫端處理。
 */
export function MoreMenu({
  label = "更多",
  ariaLabel,
  align = "start",
  className,
  children,
}: {
  /** 觸發按鈕上看得到的文字。 */
  label?: string;
  /** 同一畫面有多個「更多」時的可存取名稱（應包含看得到的文字，例：「更多（帳號 1）」）。 */
  ariaLabel?: string;
  align?: "start" | "end";
  className?: string;
  children: ReactNode;
}) {
  const ref = useRef<HTMLDetailsElement>(null);
  const onToggle = useDetailsDismiss(ref);
  const classes = ["more-menu", align === "end" ? "more-menu-end" : null, className ?? null].filter(Boolean).join(" ");
  return (
    <details ref={ref} className={classes} onToggle={onToggle}>
      <summary aria-label={ariaLabel}>{label}</summary>
      <div className="more-menu-list">{children}</div>
    </details>
  );
}
