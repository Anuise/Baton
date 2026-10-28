import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ArtifactsPage } from "./ArtifactsPage";
import type {
  ArtifactClient,
  ArtifactList,
  ArtifactPreview,
  ArtifactVersionView,
  ArtifactView,
  Workspace,
} from "./runtimeClient";

const workspace: Workspace = {
  id: "ws_1",
  name: "筆記",
  path: "C:\\work\\notes",
  status: "active",
  version: 1,
  created_at: 1_700_000_000_000,
  archived_at: null,
  model_overrides: { controller: null, worker: null, reviewer: null },
};

/** 2023-11-14 22:13:20 UTC（台北是隔天 06:13）。 */
const T0 = 1_700_000_000_000;
const SHA = "0123456789abcdef".repeat(4);

function version(n: number, overrides: Partial<ArtifactVersionView> = {}): ArtifactVersionView {
  return {
    version: n,
    description: `第 ${n} 版說明`,
    size_bytes: 12 * n,
    sha256: SHA,
    storage_path: `ws/blob_${n}`,
    produced_by_run_id: `run_${n}`,
    produced_by_generation: n,
    created_at: T0 + n * 60_000,
    file_status: "ok",
    ...overrides,
  };
}

function artifact(overrides: Partial<ArtifactView> = {}): ArtifactView {
  return {
    id: "art_1",
    workspace_id: "ws_1",
    task_id: "task_1",
    name: "report.md",
    created_at: T0,
    versions: [version(1), version(2)],
    ...overrides,
  };
}

function fakeClient(list: ArtifactList | Error, preview?: ArtifactPreview | Error) {
  return {
    listArtifacts: vi.fn(async () => {
      if (list instanceof Error) throw list;
      return list;
    }),
    previewArtifact: vi.fn(async () => {
      if (preview instanceof Error) throw preview;
      return preview!;
    }),
    downloadArtifact: vi.fn(async () => ({ blob: new Blob(["內容"]), sha256: SHA })),
  } satisfies ArtifactClient;
}

const previewOf = (preview: ArtifactPreview["preview"]): ArtifactPreview => ({
  artifact_id: "art_1",
  version: 2,
  name: "report.md",
  size_bytes: 24,
  sha256: SHA,
  preview,
});

const renderPage = (client: ArtifactClient) => render(<ArtifactsPage client={client} workspace={workspace} timeZone="Asia/Taipei" />);

describe("成果", () => {
  it("載入中先顯示提示，讀取該 Workspace 的成果", async () => {
    const client = fakeClient({ artifacts: [], unregistered_files: [] });

    renderPage(client);

    expect(screen.getByText("載入中…")).toBeInTheDocument();
    expect(await screen.findByRole("heading", { name: "成果" })).toBeInTheDocument();
    expect(client.listArtifacts).toHaveBeenCalledWith("ws_1");
  });

  it("讀取失敗時顯示 Runtime 的說明", async () => {
    renderPage(fakeClient(new Error("需要桌面憑證")));

    expect(await screen.findByRole("alert")).toHaveTextContent("無法取得成果：需要桌面憑證");
  });

  it("沒有成果時顯示空狀態，沒有未登記的檔案時不顯示該區", async () => {
    renderPage(fakeClient({ artifacts: [], unregistered_files: [] }));

    expect(await screen.findByText("這個 Workspace 還沒有成果")).toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "未登記的檔案" })).not.toBeInTheDocument();
  });

  it("每個成果顯示名稱、所屬 Task 與最新版本；舊版本預設收合，展開後由新到舊列出", async () => {
    const user = userEvent.setup();
    renderPage(fakeClient({ artifacts: [artifact()], unregistered_files: [] }));

    const card = await screen.findByRole("article", { name: "report.md" });
    expect(card).toHaveTextContent("Task：task_1");
    const latest = within(card).getByRole("listitem", { name: "report.md v2" });
    expect(latest).toHaveTextContent("最新");
    expect(latest).toHaveTextContent("24 位元組");
    // 雜湊縮短為前 12 字，完整值在 title 與「複製雜湊」
    const sha = within(latest).getByText(`${SHA.slice(0, 12)}…`);
    expect(sha).toHaveClass("mono");
    expect(sha).toHaveAttribute("title", SHA);
    expect(within(latest).getByRole("button", { name: "複製雜湊" })).toBeInTheDocument();
    expect(latest).toHaveTextContent("產生者 Run（第 2 次）：run_2");
    expect(latest).toHaveTextContent("2023/11/15 06:15");
    expect(latest).toHaveTextContent("第 2 版說明");
    expect(within(card).queryByRole("listitem", { name: "report.md v1" })).not.toBeInTheDocument();

    await user.click(within(card).getByRole("button", { name: "全部版本（2）" }));

    const items = within(card).getAllByRole("listitem");
    expect(items.map((item) => item.getAttribute("aria-label"))).toEqual(["report.md v2", "report.md v1"]);
    expect(within(card).getByRole("button", { name: "全部版本（2）" })).toHaveAttribute("aria-expanded", "true");
  });

  it("大檔案換算單位並附千分位位元組；長的 Task 與 Run 識別碼縮短，完整值可複製", async () => {
    const taskId = "task_7b5deb026d9841b0cced7314aa0f9e8d";
    const runId = "run_0_e3b0c44298fc1c149afbf4c8996fb924";
    renderPage(
      fakeClient({
        artifacts: [artifact({ task_id: taskId, versions: [version(1, { size_bytes: 1_234_567_892, produced_by_run_id: runId })] })],
        unregistered_files: [],
      }),
    );

    const card = await screen.findByRole("article", { name: "report.md" });
    const latest = within(card).getByRole("listitem", { name: "report.md v1" });
    expect(latest).toHaveTextContent("1.15 GB（1,234,567,892 位元組）");
    expect(within(card).getByText("task_7b5deb…9e8d")).toHaveAttribute("title", taskId);
    expect(within(latest).getByText("run_0_e3b0…b924")).toHaveAttribute("title", runId);
    expect(within(card).getAllByRole("button", { name: "複製識別碼" })).toHaveLength(2);
  });

  it("只有一個版本時不顯示展開按鈕", async () => {
    renderPage(fakeClient({ artifacts: [artifact({ versions: [version(1)] })], unregistered_files: [] }));

    await screen.findByRole("article", { name: "report.md" });
    expect(screen.queryByRole("button", { name: /全部版本/ })).not.toBeInTheDocument();
  });

  it("檔案遺失或大小不符時以警示標示該版本不可信", async () => {
    const user = userEvent.setup();
    renderPage(
      fakeClient({
        artifacts: [
          artifact({
            versions: [version(1, { file_status: "missing" }), version(2, { file_status: "size_mismatch" })],
          }),
        ],
        unregistered_files: [],
      }),
    );

    const card = await screen.findByRole("article", { name: "report.md" });
    expect(within(within(card).getByRole("listitem", { name: "report.md v2" })).getByRole("alert")).toHaveTextContent(
      "檔案大小與登記不符，不可信",
    );
    await user.click(within(card).getByRole("button", { name: "全部版本（2）" }));
    expect(within(within(card).getByRole("listitem", { name: "report.md v1" })).getByRole("alert")).toHaveTextContent(
      "檔案遺失，不可信",
    );
  });

  it("狀態正常的版本沒有警示", async () => {
    renderPage(fakeClient({ artifacts: [artifact()], unregistered_files: [] }));

    await screen.findByRole("article", { name: "report.md" });
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("預覽文字內容；再按一次收合", async () => {
    const user = userEvent.setup();
    const client = fakeClient(
      { artifacts: [artifact()], unregistered_files: [] },
      previewOf({ kind: "text", text: "# 標題\n內文", truncated: false }),
    );
    renderPage(client);

    const button = await screen.findByRole("button", { name: "預覽 report.md v2" });
    await user.click(button);

    const preview = await screen.findByRole("group", { name: "預覽 report.md v2" });
    expect(preview).toHaveTextContent("# 標題 內文");
    expect(within(preview).queryByText(/截斷/)).not.toBeInTheDocument();
    expect(client.previewArtifact).toHaveBeenCalledWith("ws_1", "art_1", 2);

    await user.click(button);
    expect(screen.queryByRole("group", { name: "預覽 report.md v2" })).not.toBeInTheDocument();
  });

  it("預覽被截斷時註明", async () => {
    const user = userEvent.setup();
    renderPage(
      fakeClient({ artifacts: [artifact()], unregistered_files: [] }, previewOf({ kind: "text", text: "前段", truncated: true })),
    );

    await user.click(await screen.findByRole("button", { name: "預覽 report.md v2" }));

    expect(await screen.findByText("內容過長，只顯示前段；完整內容請下載")).toBeInTheDocument();
  });

  it("二進位內容不顯示預覽，提示下載", async () => {
    const user = userEvent.setup();
    renderPage(fakeClient({ artifacts: [artifact()], unregistered_files: [] }, previewOf({ kind: "binary" })));

    await user.click(await screen.findByRole("button", { name: "預覽 report.md v2" }));

    expect(await screen.findByText("非文字內容，請下載")).toBeInTheDocument();
  });

  it("預覽失敗時顯示 Runtime 的說明（例如完整性驗證失敗）", async () => {
    const user = userEvent.setup();
    renderPage(
      fakeClient({ artifacts: [artifact()], unregistered_files: [] }, new Error("成果檔案與登記的雜湊不符，拒絕提供")),
    );

    await user.click(await screen.findByRole("button", { name: "預覽 report.md v2" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("無法預覽：成果檔案與登記的雜湊不符，拒絕提供");
  });

  describe("下載", () => {
    const createObjectURL = vi.fn(() => "blob:fake-url");
    const revokeObjectURL = vi.fn();
    let clicked: { download: string; href: string }[];

    beforeEach(() => {
      clicked = [];
      createObjectURL.mockClear();
      revokeObjectURL.mockClear();
      URL.createObjectURL = createObjectURL;
      URL.revokeObjectURL = revokeObjectURL;
      vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(function (this: HTMLAnchorElement) {
        clicked.push({ download: this.download, href: this.href });
      });
    });

    afterEach(() => {
      vi.restoreAllMocks();
    });

    it("經 runtimeClient 取得內容後，以 object URL 與 download 連結儲存，檔名是成果名稱", async () => {
      const user = userEvent.setup();
      const client = fakeClient({ artifacts: [artifact()], unregistered_files: [] });
      renderPage(client);

      await user.click(await screen.findByRole("button", { name: "下載 report.md v2" }));

      await waitFor(() => expect(clicked).toEqual([{ download: "report.md", href: "blob:fake-url" }]));
      expect(client.downloadArtifact).toHaveBeenCalledWith("ws_1", "art_1", 2);
      expect(createObjectURL).toHaveBeenCalledOnce();
      expect(revokeObjectURL).toHaveBeenCalledWith("blob:fake-url");
      expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    });

    it("下載失敗時顯示 Runtime 的說明，不觸發儲存", async () => {
      const user = userEvent.setup();
      const client = fakeClient({ artifacts: [artifact()], unregistered_files: [] });
      client.downloadArtifact.mockRejectedValueOnce(new Error("成果檔案遺失"));
      renderPage(client);

      await user.click(await screen.findByRole("button", { name: "下載 report.md v2" }));

      expect(await screen.findByRole("alert")).toHaveTextContent("無法下載：成果檔案遺失");
      expect(clicked).toEqual([]);
      expect(createObjectURL).not.toHaveBeenCalled();
    });

    it("下載內容的雜湊與登記不符時拒絕儲存", async () => {
      const user = userEvent.setup();
      const client = fakeClient({ artifacts: [artifact()], unregistered_files: [] });
      client.downloadArtifact.mockResolvedValueOnce({ blob: new Blob(["被竄改"]), sha256: "f".repeat(64) });
      renderPage(client);

      await user.click(await screen.findByRole("button", { name: "下載 report.md v2" }));

      expect(await screen.findByRole("alert")).toHaveTextContent("無法下載：下載內容的雜湊與登記不符，已拒絕儲存");
      expect(clicked).toEqual([]);
    });
  });

  it("未登記的檔案獨立一區，說明不是成果、不可使用", async () => {
    renderPage(
      fakeClient({
        artifacts: [artifact()],
        unregistered_files: [
          { storage_path: "ws/blob_a.partial", size_bytes: 1, kind: "staging" },
          { storage_path: "ws/blob_b", size_bytes: 2048, kind: "unregistered" },
        ],
      }),
    );

    const section = await screen.findByRole("region", { name: "未登記的檔案" });
    expect(section).toHaveTextContent("未登記、不可使用");
    const items = within(section).getAllByRole("listitem");
    expect(items[0]).toHaveTextContent("ws/blob_a.partial");
    expect(items[0]).toHaveTextContent("暫存檔，內容不完整");
    expect(items[1]).toHaveTextContent("ws/blob_b");
    expect(items[1]).toHaveTextContent("2 KB（2,048 位元組）");
    expect(items[1]).toHaveTextContent("檔案完整但沒有登記");
    // 它們不是成果：沒有預覽、下載
    expect(within(section).queryByRole("button")).not.toBeInTheDocument();
    expect(screen.getAllByRole("article")).toHaveLength(1);
  });
});
