import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";

import { HelpTip } from "./HelpTip";

describe("HelpTip", () => {
  it("平時只顯示「?」按鈕，說明以 aria-describedby 連到按鈕", () => {
    render(<HelpTip label="自動續跑">只影響下次重新開啟</HelpTip>);

    const trigger = screen.getByRole("button", { name: "自動續跑的說明" });
    expect(trigger).toHaveTextContent("?");
    expect(trigger).toHaveAccessibleDescription("只影響下次重新開啟");
    expect(screen.queryByRole("tooltip")).not.toBeInTheDocument();
  });

  it("以鍵盤聚焦時顯示說明，離開焦點後收起", async () => {
    const user = userEvent.setup();
    render(
      <>
        <HelpTip label="自動續跑">只影響下次重新開啟</HelpTip>
        <button type="button">下一個</button>
      </>,
    );

    await user.tab();
    expect(screen.getByRole("button", { name: "自動續跑的說明" })).toHaveFocus();
    expect(screen.getByRole("tooltip")).toHaveTextContent("只影響下次重新開啟");

    await user.tab();
    expect(screen.queryByRole("tooltip")).not.toBeInTheDocument();
  });

  it("按 Escape 收起說明，焦點留在按鈕上", async () => {
    const user = userEvent.setup();
    render(<HelpTip label="自動續跑">只影響下次重新開啟</HelpTip>);

    await user.tab();
    await user.keyboard("{Escape}");

    expect(screen.queryByRole("tooltip")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "自動續跑的說明" })).toHaveFocus();
  });

  it("滑鼠懸停時顯示說明，移開後收起", async () => {
    const user = userEvent.setup();
    render(<HelpTip label="自動續跑">只影響下次重新開啟</HelpTip>);
    const trigger = screen.getByRole("button", { name: "自動續跑的說明" });

    await user.hover(trigger);
    expect(screen.getByRole("tooltip")).toHaveTextContent("只影響下次重新開啟");

    await user.unhover(trigger);
    expect(screen.queryByRole("tooltip")).not.toBeInTheDocument();
  });
});
