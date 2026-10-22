import { describe, expect, it } from "vitest";

import { formatTimestamp } from "./time";

describe("時間顯示", () => {
  it("同一個 UTC 毫秒值依時區顯示不同的當地時間", () => {
    const utcMillis = Date.UTC(2023, 10, 14, 22, 13, 20);

    expect(formatTimestamp(utcMillis, "UTC")).toBe("2023/11/14 22:13");
    expect(formatTimestamp(utcMillis, "Asia/Taipei")).toBe("2023/11/15 06:13");
    expect(formatTimestamp(utcMillis, "America/New_York")).toBe("2023/11/14 17:13");
  });

  it("沒有指定時區時使用系統時區", () => {
    const utcMillis = Date.UTC(2024, 0, 15, 12, 0, 0);
    const system = Intl.DateTimeFormat().resolvedOptions().timeZone;

    expect(formatTimestamp(utcMillis)).toBe(formatTimestamp(utcMillis, system));
  });
});
