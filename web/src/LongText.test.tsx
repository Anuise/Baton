import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { CopyButton, LongText, ShortValue } from "./LongText";

const LONG = "第一段說明，內容很長。\n第二段說明，仍然很長。\n第三段。\n第四段。";

/** jsdom 不排版：以 scrollHeight／clientHeight 模擬「文字超過限制行數」。 */
function mockOverflow(overflowing: boolean) {
  vi.spyOn(HTMLElement.prototype, "scrollHeight", "get").mockImplementation(function (this: HTMLElement) {
    return overflowing && this.classList.contains("long-text-body") ? 200 : 60;
  });
  vi.spyOn(HTMLElement.prototype, "clientHeight", "get").mockReturnValue(60);
}

afterEach(() => {
  vi.restoreAllMocks();
});

describe("LongText", () => {
  it("預設限制 3 行；文字放得下時不顯示「展開」", () => {
    mockOverflow(false);
    render(<LongText text="額度已滿" />);

    expect(screen.getByText("額度已滿")).toHaveClass("long-text-body", "clamp-3");
    expect(screen.queryByRole("button", { name: "展開" })).not.toBeInTheDocument();
  });

  it("超過限制行數時顯示「展開」，展開後顯示全文、按鈕變成「收合」", async () => {
    mockOverflow(true);
    const user = userEvent.setup();
    render(<LongText text={LONG} lines={2} />);

    const body = screen.getByText(/第一段說明/);
    expect(body).toHaveClass("clamp-2");
    const toggle = screen.getByRole("button", { name: "展開" });
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    expect(toggle).toHaveAttribute("aria-controls", body.id);

    await user.click(toggle);
    expect(body).not.toHaveClass("clamp-2");
    expect(screen.getByRole("button", { name: "收合" })).toHaveAttribute("aria-expanded", "true");

    await user.click(screen.getByRole("button", { name: "收合" }));
    expect(body).toHaveClass("clamp-2");
    expect(screen.getByRole("button", { name: "展開" })).toHaveAttribute("aria-expanded", "false");
  });

  it("全文仍在 DOM 中（只是視覺上截斷），螢幕報讀器讀得到", () => {
    mockOverflow(true);
    render(<LongText text={LONG} />);

    expect(screen.getByText(/第一段說明/)).toHaveTextContent("第四段。");
  });

  it("指定 copyLabel 時附上複製按鈕", () => {
    mockOverflow(false);
    render(<LongText text="C:\\work\\notes" copyLabel="複製路徑" />);

    expect(screen.getByRole("button", { name: "複製路徑" })).toBeInTheDocument();
  });

  it("可以加上自己的 class", () => {
    mockOverflow(false);
    const { container } = render(<LongText text="說明" className="task-goal" />);

    expect(container.firstChild).toHaveClass("long-text", "task-goal");
  });
});

describe("CopyButton", () => {
  beforeEach(() => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  /** userEvent.setup() 會換上自己的剪貼簿，所以在它之後才放假的 writeText。 */
  function setup(writeText: (text: string) => Promise<void>) {
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    return user;
  }

  it("複製完整文字，暫時顯示「已複製」後恢復", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    const user = setup(writeText);
    render(<CopyButton text="task_7b5deb026d9841b0cced7314" label="複製識別碼" />);

    await user.click(screen.getByRole("button", { name: "複製識別碼" }));
    expect(writeText).toHaveBeenCalledWith("task_7b5deb026d9841b0cced7314");
    expect(await screen.findByRole("button", { name: "已複製" })).toBeInTheDocument();

    await act(async () => {
      vi.advanceTimersByTime(2000);
    });
    expect(screen.getByRole("button", { name: "複製識別碼" })).toBeInTheDocument();
  });

  it("剪貼簿拒絕時顯示「複製失敗」", async () => {
    const user = setup(vi.fn().mockRejectedValue(new Error("denied")));
    render(<CopyButton text="abc" />);

    await user.click(screen.getByRole("button", { name: "複製" }));
    expect(await screen.findByRole("button", { name: "複製失敗" })).toBeInTheDocument();
  });
});

describe("ShortValue", () => {
  it("縮短後等寬顯示，title 放完整值，旁邊附複製", () => {
    render(<ShortValue value="task_7b5deb026d9841b0cced7314" kind="id" />);

    expect(screen.getByText("task_7b5deb…7314")).toHaveAttribute("title", "task_7b5deb026d9841b0cced7314");
    expect(screen.getByRole("button", { name: "複製識別碼" })).toBeInTheDocument();
  });

  it("本來就夠短時原樣顯示、不附複製", () => {
    render(<ShortValue value="abc123" kind="hash" />);

    expect(screen.getByText("abc123")).not.toHaveAttribute("title");
    expect(screen.queryByRole("button")).toBeNull();
  });
});
