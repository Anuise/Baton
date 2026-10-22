import { render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { BackupSettings } from "./BackupSettings";
import { backupStatus, backupSummary, CHECKED_AT, drillReport } from "./fixtures";
import type { BackupClient, BackupStatus } from "./runtimeClient";

function fakeClient(status: BackupStatus = backupStatus()) {
  const client: BackupClient = { backupStatus: vi.fn(async () => status) };
  return client;
}

// CHECKED_AT 是 2023-11-14 22:13:20 UTC，以台北時間顯示為 2023/11/15 06:13
const renderSettings = (client: BackupClient) => render(<BackupSettings client={client} timeZone="Asia/Taipei" />);

describe("備份與還原演練", () => {
  it("顯示備份政策說明", async () => {
    renderSettings(fakeClient());

    expect(
      await screen.findByText("每日自動備份，保留 7 份每日與 4 份每週備份；每週自動進行一次還原演練。備份不含登入秘密。"),
    ).toBeInTheDocument();
  });

  it("最新一次演練通過時，顯示演練時間、被演練的備份時間、各項核對與中斷的 Run 數", async () => {
    renderSettings(fakeClient(backupStatus({ latest_drill: drillReport({ interrupted_runs: 2 }) })));

    expect(await screen.findByText("還原演練通過")).toBeInTheDocument();
    expect(screen.getByText("演練時間：2023/11/15 07:13")).toBeInTheDocument();
    expect(screen.getByText("被演練的備份：2023/11/15 06:13")).toBeInTheDocument();
    const checks = within(screen.getByRole("list", { name: "核對項目" })).getAllByRole("listitem");
    expect(checks.map((item) => item.textContent)).toEqual([
      "還原：通過（已還原到暫存目錄）",
      "資料庫完整性：通過（integrity_check 回報 ok）",
      "外鍵關聯：通過（沒有違反外鍵的資料）",
      "成果檔案雜湊：通過（2 個成果檔案的 SHA-256 都相符）",
      "成果與資料庫的對應：通過（成果版本都對得上檔案）",
    ]);
    expect(screen.getByText("還原時標為中斷的 Run：2 個（還原後不會自動重做）")).toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("最新一次演練未通過時以警示標示，並指出未通過的核對", async () => {
    const drill = drillReport({
      passed: false,
      checks: [
        { kind: "restore", passed: true, detail: "已還原到暫存目錄" },
        { kind: "artifact_hashes", passed: false, detail: "1 個成果檔案的 SHA-256 不符" },
      ],
    });
    renderSettings(fakeClient(backupStatus({ latest_drill: drill })));

    expect(await screen.findByRole("alert")).toHaveTextContent("還原演練未通過");
    expect(screen.getByText("成果檔案雜湊：未通過（1 個成果檔案的 SHA-256 不符）")).toBeInTheDocument();
  });

  it("演練沒有可用的備份時，說明沒有被演練的備份", async () => {
    const drill = drillReport({
      passed: false,
      backup_id: null,
      backup_created_at: null,
      checks: [{ kind: "restore", passed: false, detail: "找不到可還原的備份" }],
    });
    renderSettings(fakeClient(backupStatus({ backups: [], latest_drill: drill })));

    expect(await screen.findByText("被演練的備份：沒有可演練的備份")).toBeInTheDocument();
  });

  it("尚未演練過時說明每週會自動進行", async () => {
    renderSettings(fakeClient(backupStatus({ latest_drill: null })));

    expect(await screen.findByText("尚未進行還原演練（每週自動一次）")).toBeInTheDocument();
    expect(screen.queryByRole("list", { name: "核對項目" })).not.toBeInTheDocument();
  });

  it("依 Runtime 的順序列出備份的時間、觸發原因、成果檔案數與保留原因", async () => {
    const backups = [
      backupSummary({
        id: "b3",
        trigger: "milestone",
        created_at: CHECKED_AT + 7_200_000,
        artifact_count: 5,
        retained_as: [],
      }),
      backupSummary({
        id: "b2",
        trigger: "pre_migration",
        created_at: CHECKED_AT + 3_600_000,
        artifact_count: 0,
        retained_as: ["latest_pre_migration"],
      }),
      backupSummary({ id: "b1" }),
    ];
    renderSettings(fakeClient(backupStatus({ backups })));

    const list = await screen.findByRole("list", { name: "備份清單" });
    expect(within(list).getAllByRole("listitem").map((item) => item.textContent)).toEqual([
      "2023/11/15 08:13・重要裁定／Stage 完成・成果檔案 5 個",
      "2023/11/15 07:13・migration 前・成果檔案 0 個・保留原因：最新的 migration 前備份",
      "2023/11/15 06:13・每日・成果檔案 2 個・保留原因：每日、每週",
    ]);
  });

  it("沒有備份時說明尚無備份", async () => {
    renderSettings(fakeClient(backupStatus({ backups: [], latest_drill: null })));

    expect(await screen.findByText("尚無備份")).toBeInTheDocument();
    expect(screen.queryByRole("list", { name: "備份清單" })).not.toBeInTheDocument();
  });

  it("只顯示狀態，沒有任何操作按鈕", async () => {
    renderSettings(fakeClient());

    await screen.findByText("還原演練通過");
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
  });

  it("讀不到備份狀態時顯示原因，不顯示清單", async () => {
    const client = fakeClient();
    vi.mocked(client.backupStatus).mockRejectedValueOnce(new Error("connection refused"));

    renderSettings(client);

    expect(await screen.findByRole("alert")).toHaveTextContent("無法取得備份狀態：connection refused");
    expect(screen.queryByRole("list", { name: "備份清單" })).not.toBeInTheDocument();
  });

  it("載入中先顯示提示", () => {
    const client: BackupClient = { backupStatus: () => new Promise(() => {}) };

    renderSettings(client);

    expect(screen.getByText("載入中…")).toBeInTheDocument();
  });
});
