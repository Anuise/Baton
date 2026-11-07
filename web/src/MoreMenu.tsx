import { useEffect, useRef, useState, type ReactNode, type RefObject } from "react";

/**
 * `<details>` 浮層的收起行為：
 * - 開著時點浮層外面，或用 Tab 把焦點移出浮層，就收起（不搬動焦點；WCAG 2.4.11 焦點不被遮蔽）。
 *   焦點離開時延後一個 tick 才收起，不在焦點切換途中改 `open`（見 closeWhenFocusLeaves）。
 * - Esc 收起並把焦點還給觸發的 `<summary>`。只在焦點位於浮層內、或落在 body（點過浮層空白處）時處理，
 *   不會在頁面別處按 Esc 時把焦點搶回來。
 * Esc 監聽在 document：點浮層空白處後焦點落到 body，按鍵事件不會經過 details。
 * 回傳 `onToggle`，交給 `<details onToggle>`，讓外點與焦點監聽只在開著時存在；
 * `onClose`（可選）在收起時呼叫，例如重設浮層裡的暫時狀態。
 */
export function useDetailsDismiss(ref: RefObject<HTMLDetailsElement | null>, onClose?: () => void) {
  const [open, setOpen] = useState(false);

  useEffect(() => {
    if (!open) return;
    const details = ref.current;
    const closeOnOutsidePointer = (event: PointerEvent) => {
      if (details && !details.contains(event.target as Node)) details.open = false;
    };
    const closeWhenFocusLeaves = (event: FocusEvent) => {
      const next = event.relatedTarget as HTMLElement | null;
      // 焦點落到 body（null），或落到主區這類只供程式聚焦的容器（tabindex="-1"），都是點了浮層空白處：保持開著
      if (!details || next === null || details.contains(next) || next.getAttribute("tabindex") === "-1") return;
      // 不在焦點切換途中同步關閉：mousedown 的焦點切換途中改 details.open 會讓 Chromium／WebView2 的畫面程序當掉。
      // 切換完成後再確認焦點真的不在浮層裡才收起
      setTimeout(() => {
        if (details.open && !details.contains(document.activeElement)) details.open = false;
      }, 0);
    };
    document.addEventListener("pointerdown", closeOnOutsidePointer);
    details?.addEventListener("focusout", closeWhenFocusLeaves);
    return () => {
      document.removeEventListener("pointerdown", closeOnOutsidePointer);
      details?.removeEventListener("focusout", closeWhenFocusLeaves);
    };
  }, [open, ref]);

  useEffect(() => {
    const closeOnEscape = (event: KeyboardEvent) => {
      const details = ref.current;
      if (event.key !== "Escape" || !details?.open) return;
      const active = document.activeElement;
      // 焦點在浮層內、在 body，或在主區這類只供程式聚焦的容器（點過浮層空白處）時才處理
      const blank = active === null || active === document.body || active.getAttribute("tabindex") === "-1";
      if (!blank && !details.contains(active)) return;
      details.open = false;
      details.querySelector("summary")?.focus();
    };
    document.addEventListener("keydown", closeOnEscape);
    return () => document.removeEventListener("keydown", closeOnEscape);
  }, [ref]);

  return (event: React.SyntheticEvent<HTMLDetailsElement>) => {
    const isOpen = event.currentTarget.open;
    setOpen(isOpen);
    if (!isOpen) onClose?.();
  };
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
