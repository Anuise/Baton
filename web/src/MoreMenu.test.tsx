import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";

import { MoreMenu } from "./MoreMenu";

function renderMenu(align?: "start" | "end") {
  return render(
    <div>
      <button type="button">外面</button>
      <MoreMenu ariaLabel="更多（條目甲）" align={align}>
        <button type="button" className="btn-quiet">修正</button>
      </MoreMenu>
    </div>,
  );
}

describe("更多選單", () => {
  it("觸發鈕的可存取名稱包含看得到的「更多」；往左展開時加 more-menu-end", () => {
    const { container } = renderMenu("end");

    const summary = screen.getByLabelText("更多（條目甲）");
    expect(summary).toHaveTextContent("更多");
    expect(container.querySelector("details")).toHaveClass("more-menu", "more-menu-end");
  });

  it("點選單空白處（焦點落到 body）後按 Esc 仍收起，焦點回到觸發鈕", async () => {
    const user = userEvent.setup();
    const { container } = renderMenu();
    const details = container.querySelector("details")!;
    const summary = screen.getByLabelText("更多（條目甲）");

    await user.click(summary);
    expect(details).toHaveAttribute("open");
    await user.click(container.querySelector(".more-menu-list")!);
    (document.activeElement as HTMLElement | null)?.blur();
    expect(document.body).toHaveFocus();

    await user.keyboard("{Escape}");
    expect(details).not.toHaveAttribute("open");
    expect(summary).toHaveFocus();
  });

  it("點選單外面時收起", async () => {
    const user = userEvent.setup();
    const { container } = renderMenu();
    const details = container.querySelector("details")!;

    await user.click(screen.getByLabelText("更多（條目甲）"));
    expect(details).toHaveAttribute("open");
    await user.click(screen.getByRole("button", { name: "外面" }));

    expect(details).not.toHaveAttribute("open");
  });

  it("點選單空白處時焦點落到主區（tabindex=-1 的容器），選單保持開著", async () => {
    const user = userEvent.setup();
    const { container } = render(
      <main tabIndex={-1} id="main-content">
        <MoreMenu ariaLabel="更多（條目甲）">
          <button type="button" className="btn-quiet">修正</button>
        </MoreMenu>
      </main>,
    );
    const details = container.querySelector("details")!;
    await user.click(screen.getByLabelText("更多（條目甲）"));

    // 相當於在浮層內距按下滑鼠：焦點從觸發鈕移到可被點擊聚焦的主區
    container.querySelector("main")!.focus();
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(details).toHaveAttribute("open");
    // 這時按 Esc 仍收起並把焦點還給觸發鈕
    await user.keyboard("{Escape}");
    expect(details).not.toHaveAttribute("open");
    expect(screen.getByLabelText("更多（條目甲）")).toHaveFocus();
  });

  it("用 Tab 把焦點移出浮層時收起，不搬動焦點", async () => {
    const user = userEvent.setup();
    render(
      <div>
        <MoreMenu ariaLabel="更多（條目甲）">
          <button type="button" className="btn-quiet">修正</button>
        </MoreMenu>
        <button type="button">下一個</button>
      </div>,
    );
    const summary = screen.getByLabelText("更多（條目甲）");
    const details = summary.closest("details")!;

    await user.click(summary);
    await user.tab();
    expect(screen.getByRole("button", { name: "修正" })).toHaveFocus();
    await user.tab();

    expect(screen.getByRole("button", { name: "下一個" })).toHaveFocus();
    // 收起延後一個 tick（不在焦點切換途中改 open）
    await waitFor(() => expect(details).not.toHaveAttribute("open"));
  });

  it("焦點在頁面別處時按 Esc 不會把焦點搶回觸發鈕", async () => {
    const user = userEvent.setup();
    const { container } = renderMenu();
    const details = container.querySelector("details")!;

    await user.click(screen.getByLabelText("更多（條目甲）"));
    // 程式把焦點移到別處（不經過 focusout 的 relatedTarget 判斷時選單仍開著）
    details.open = true;
    screen.getByRole("button", { name: "外面" }).focus();
    await user.keyboard("{Escape}");

    expect(screen.getByRole("button", { name: "外面" })).toHaveFocus();
  });
});
