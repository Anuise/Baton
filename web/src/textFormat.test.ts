import { describe, expect, it } from "vitest";

import {
  docsHost,
  formatBytes,
  formatCount,
  formatNumber,
  optionLabel,
  shortHash,
  shortId,
  shortPath,
  shortPathParts,
  sizeLabel,
  splitExtension,
  splitFirstSentence,
  truncateText,
} from "./textFormat";

describe("shortId", () => {
  it("保留前綴與主體前 6 碼，中間省略，再接後 4 碼", () => {
    expect(shortId("task_7b5deb026d9841b0cced7314")).toBe("task_7b5deb…7314");
    expect(shortId("run_26fbcca2e69f40f439012ce9")).toBe("run_26fbcc…2ce9");
    expect(shortId("ws_75b131a47faed9ffc3cd741a")).toBe("ws_75b131…741a");
  });

  it("沒有前綴時取前 10 碼與後 4 碼", () => {
    expect(shortId("7b5deb026d9841b0cced7314aa0f9e8d")).toBe("7b5deb026d…9e8d");
  });

  it("已經夠短就原樣傳回", () => {
    expect(shortId("task_1")).toBe("task_1");
    expect(shortId("task_7b5deb02cd")).toBe("task_7b5deb02cd");
  });
});

describe("shortHash", () => {
  it("取前 12 字並以省略號標示", () => {
    const sha = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    expect(shortHash(sha)).toBe("e3b0c44298fc…");
  });

  it("不超過 12 字時原樣傳回", () => {
    expect(shortHash("2a0654b")).toBe("2a0654b");
  });
});

describe("shortPath", () => {
  it("Windows 路徑保留磁碟與最後 2 段", () => {
    expect(shortPath("C:\\Users\\User\\Documents\\au-program\\Baton\\fixtures\\report.md")).toBe(
      "C:\\…\\fixtures\\report.md",
    );
  });

  it("UNC 路徑保留伺服器與共用名稱", () => {
    expect(shortPath("\\\\server\\share\\team\\project\\docs\\a.md")).toBe("\\\\server\\share\\…\\docs\\a.md");
  });

  it("URL 保留網域", () => {
    expect(shortPath("https://docs.example.com/en/setup/windows/install.html")).toBe(
      "https://docs.example.com/…/windows/install.html",
    );
  });

  it("POSIX 路徑保留根目錄", () => {
    expect(shortPath("/home/user/work/project/src/main.rs")).toBe("/…/src/main.rs");
  });

  it("段數不多時原樣傳回", () => {
    expect(shortPath("C:\\work\\notes")).toBe("C:\\work\\notes");
    expect(shortPath("C:\\work\\docs\\a.md")).toBe("C:\\work\\docs\\a.md");
    expect(shortPath("report.md")).toBe("report.md");
  });

  it("可以指定保留的段數", () => {
    expect(shortPath("C:\\a\\b\\c\\d\\e.md", 1)).toBe("C:\\…\\e.md");
  });
});

describe("shortPathParts", () => {
  const LONG = "C:\\Users\\me\\fixtures\\deeply_nested\\report-final-revision-2026-10-28.md";

  it("分成開頭（磁碟＋省略號）、中段與一定顯示的結尾，三段相接等於 shortPath", () => {
    const parts = shortPathParts(LONG);
    // 結尾最多 10 字，從最靠前的分隔字元（\ / . _ - 空白）後面開始，不把「2026」切成兩半
    expect(parts).toEqual({ head: "C:\\…", start: "\\deeply_nested\\report-final-revision-2026-", end: "10-28.md" });
    expect(parts.head + parts.start + parts.end).toBe(shortPath(LONG));
  });

  it("可以指定保留的段數與結尾字數；結尾範圍內沒有分隔字元時直接取最後幾字", () => {
    expect(shortPathParts(LONG, 1, 5)).toEqual({ head: "C:\\…", start: "\\report-final-revision-2026-10-", end: "28.md" });
    expect(shortPathParts("C:\\a\\b\\c\\abcdefghijklmnop", 1, 4)).toEqual({ head: "C:\\…", start: "\\abcdefghijkl", end: "mnop" });
  });

  it("沒有省略時開頭為空；中段不比結尾長時全部放在結尾", () => {
    expect(shortPathParts("C:\\work\\notes")).toEqual({ head: "", start: "C:\\", end: "work\\notes" });
    expect(shortPathParts("C:\\a\\b\\c\\d.md")).toEqual({ head: "C:\\…", start: "", end: "\\c\\d.md" });
    expect(shortPathParts("report.md")).toEqual({ head: "", start: "", end: "report.md" });
  });

  it("UNC 與 URL 的開頭含伺服器或網域", () => {
    expect(shortPathParts("\\\\server\\share\\team\\project\\docs\\a.md").head).toBe("\\\\server\\share\\…");
    expect(shortPathParts("https://docs.example.com/en/setup/windows/install.html").head).toBe("https://docs.example.com/…");
  });
});

describe("splitExtension", () => {
  it("分出最後一個副檔名（含點）", () => {
    expect(splitExtension("fixtures/report-final.md")).toEqual({ base: "fixtures/report-final", ext: ".md" });
    expect(splitExtension("archive.tar.gz")).toEqual({ base: "archive.tar", ext: ".gz" });
  });

  it("沒有副檔名、點開頭的檔名或點在資料夾名稱中時 ext 為空", () => {
    expect(splitExtension("README")).toEqual({ base: "README", ext: "" });
    expect(splitExtension(".gitignore")).toEqual({ base: ".gitignore", ext: "" });
    expect(splitExtension("v1.2/notes")).toEqual({ base: "v1.2/notes", ext: "" });
    expect(splitExtension("C:\\a.b\\notes")).toEqual({ base: "C:\\a.b\\notes", ext: "" });
  });

  it("「副檔名」太長或含空白時不當成副檔名", () => {
    expect(splitExtension("報告 v2.最終版本與附錄說明")).toEqual({ base: "報告 v2.最終版本與附錄說明", ext: "" });
    expect(splitExtension("name.with spaces")).toEqual({ base: "name.with spaces", ext: "" });
  });
});

describe("formatCount", () => {
  it("超過 999 顯示 999+", () => {
    expect(formatCount(1284)).toBe("999+");
    expect(formatCount(99998)).toBe("999+");
  });

  it("999 以內顯示原數字", () => {
    expect(formatCount(0)).toBe("0");
    expect(formatCount(999)).toBe("999");
  });

  it("可以指定上限", () => {
    expect(formatCount(120, 99)).toBe("99+");
  });
});

describe("formatNumber", () => {
  it("加上千分位", () => {
    expect(formatNumber(1284)).toBe("1,284");
    expect(formatNumber(1234567)).toBe("1,234,567");
    expect(formatNumber(12)).toBe("12");
  });
});

describe("formatBytes", () => {
  it("以 1024 為級距換算並取 2 位小數", () => {
    expect(formatBytes(1234567892)).toBe("1.15 GB");
    expect(formatBytes(2048)).toBe("2 KB");
    expect(formatBytes(1536)).toBe("1.5 KB");
  });

  it("小於 1 KB 以位元組顯示", () => {
    expect(formatBytes(0)).toBe("0 位元組");
    expect(formatBytes(512)).toBe("512 位元組");
  });
});

describe("truncateText", () => {
  it("超過上限時截斷並加省略號，總長不超過上限", () => {
    expect(truncateText("一二三四五六七八九十", 5)).toBe("一二三四…");
    expect(Array.from(truncateText("一二三四五六七八九十", 5))).toHaveLength(5);
  });

  it("不超過上限時原樣傳回", () => {
    expect(truncateText("一二三", 5)).toBe("一二三");
  });

  it("以字元（code point）計算，不切開代理對", () => {
    expect(truncateText("😀😀😀😀😀😀", 3)).toBe("😀😀…");
  });
});

describe("optionLabel", () => {
  it("標題過長時截斷前段，並附上識別碼後 4 碼，總長不超過 60 字", () => {
    const title = "長文字壓力測試專用標記".repeat(10);
    const label = optionLabel(title, "task_7b5deb026d9841b0cced7314");
    expect(Array.from(label).length).toBeLessThanOrEqual(60);
    expect(label.startsWith("長文字壓力測試專用標記")).toBe(true);
    expect(label.endsWith("（7314）")).toBe(true);
  });

  it("標題夠短時完整保留", () => {
    expect(optionLabel("新增匯入功能", "task_7b5deb026d9841b0cced7314")).toBe("新增匯入功能（7314）");
  });

  it("沒有識別碼時只截斷標題", () => {
    expect(optionLabel("新增匯入功能")).toBe("新增匯入功能");
    expect(Array.from(optionLabel("長".repeat(100))).length).toBe(60);
  });
});

describe("splitFirstSentence", () => {
  it("以第一個句末標點分出摘要句與其餘細節", () => {
    expect(splitFirstSentence("無法寫入控制資料庫。SQLite 回報 SQLITE_IOERR_WRITE（code 778）。")).toEqual({
      summary: "無法寫入控制資料庫。",
      rest: "SQLite 回報 SQLITE_IOERR_WRITE（code 778）。",
    });
  });

  it("英文句點後接空白才算句末，路徑與版本號中的點不算", () => {
    expect(splitFirstSentence("Cannot open C:\\data\\baton.sqlite. Disk is full.")).toEqual({
      summary: "Cannot open C:\\data\\baton.sqlite.",
      rest: "Disk is full.",
    });
  });

  it("換行也算句末", () => {
    expect(splitFirstSentence("建置失敗\nerror[E0308]: mismatched types")).toEqual({
      summary: "建置失敗",
      rest: "error[E0308]: mismatched types",
    });
  });

  it("只有一句時 rest 為空字串", () => {
    expect(splitFirstSentence("額度已滿")).toEqual({ summary: "額度已滿", rest: "" });
  });
});

describe("sizeLabel", () => {
  it("小於 1 KB 只顯示位元組", () => {
    expect(sizeLabel(512)).toBe("512 位元組");
  });

  it("其餘換算單位並附千分位位元組", () => {
    expect(sizeLabel(1234567892)).toBe("1.15 GB（1,234,567,892 位元組）");
  });
});

describe("docsHost", () => {
  it("網址只留網域", () => {
    expect(docsHost("https://docs.example.com/guide/install?x=1")).toBe("docs.example.com");
  });

  it("不是網址時退回縮短路徑", () => {
    expect(docsHost("C:\\Users\\me\\baton\\docs\\platform.md")).toBe("C:\\…\\docs\\platform.md");
  });
});
