import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { endedPlatform, supportedPlatform, unofficialPlatform } from "./fixtures";
import { PlatformPage } from "./PlatformPage";
import type { PlatformResponse, RuntimeClient } from "./runtimeClient";

function clientWith(result: PlatformResponse | Error): RuntimeClient {
  return {
    getHealth: vi.fn(),
    getPlatform: vi.fn(async () => {
      if (result instanceof Error) throw result;
      return result;
    }),
  };
}

describe("平台狀態頁", () => {
  it("顯示診斷資訊：OS build、功能版本、版本別、原生架構與 App 版本", async () => {
    render(<PlatformPage client={clientWith(supportedPlatform)} />);

    expect(await screen.findByText("26300")).toBeInTheDocument();
    expect(screen.getByText("26H2")).toBeInTheDocument();
    expect(screen.getByText("Pro")).toBeInTheDocument();
    expect(screen.getByText("x64", { selector: "[data-field=native-arch]" })).toBeInTheDocument();
    expect(screen.getByText("0.1.0")).toBeInTheDocument();
    expect(screen.getByText("正式支援")).toBeInTheDocument();
  });

  it("標示支援期限資料的來源與查閱日期", async () => {
    render(<PlatformPage client={clientWith(supportedPlatform)} />);

    expect(await screen.findByText(/2026-10-22/)).toBeInTheDocument();
    expect(screen.getByText(/example\.test/)).toBeInTheDocument();
  });

  it("非正式支援時顯示警告原因，並說明不阻擋使用", async () => {
    render(<PlatformPage client={clientWith(unofficialPlatform)} />);

    expect(await screen.findByText("非正式支援")).toBeInTheDocument();
    expect(screen.getByRole("alert")).toHaveTextContent("x64 模擬執行");
    expect(screen.getByText(/不會阻擋使用/)).toBeInTheDocument();
  });

  it("已到期的版本顯示 L1 警告", async () => {
    render(<PlatformPage client={clientWith(endedPlatform)} />);

    expect(await screen.findByRole("alert")).toHaveTextContent("L1");
  });

  it("讀不到平台狀態時顯示原因，不假裝正式支援", async () => {
    render(<PlatformPage client={clientWith(new Error("connection refused"))} />);

    expect(await screen.findByText(/connection refused/)).toBeInTheDocument();
    expect(screen.queryByText("正式支援")).not.toBeInTheDocument();
  });
});
