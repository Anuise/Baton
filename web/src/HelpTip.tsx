import { useId, useState, type ReactNode } from "react";

interface Props {
  /** 說明的對象；按鈕的可及名稱為「{label}的說明」。 */
  label: string;
  /** 說明內容。 */
  children: ReactNode;
}

/**
 * 「?」懸停提示：取代畫面上的解釋性小字。滑鼠懸停或鍵盤聚焦時顯示，離開或按 Escape 收起；
 * 說明平時隱藏但仍以 aria-describedby 連到按鈕，螢幕報讀器隨時讀得到。
 */
export function HelpTip({ label, children }: Props) {
  const id = useId();
  const [open, setOpen] = useState(false);
  return (
    <span className="help-tip" onMouseEnter={() => setOpen(true)} onMouseLeave={() => setOpen(false)}>
      <button
        type="button"
        className="help-tip-trigger"
        aria-label={`${label}的說明`}
        aria-describedby={id}
        onFocus={() => setOpen(true)}
        onBlur={() => setOpen(false)}
        onKeyDown={(event) => {
          if (event.key === "Escape") setOpen(false);
        }}
      >
        ?
      </button>
      <span role="tooltip" id={id} className="help-tip-content" hidden={!open}>
        {children}
      </span>
    </span>
  );
}
