import { render, screen } from "@testing-library/react";
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
});
