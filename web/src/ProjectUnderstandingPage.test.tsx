import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { CHECKED_AT, projection, understandingItem, understandingView } from "./fixtures";
import { ProjectUnderstandingPage } from "./ProjectUnderstandingPage";
import type { ReconcileProjectionRequest, UnderstandingView, Workspace } from "./runtimeClient";

const workspace: Workspace = {
  id: "ws_1",
  name: "筆記",
  path: "C:\\work\\notes",
  status: "active",
  version: 1,
  created_at: CHECKED_AT,
  archived_at: null,
  model_overrides: { controller: null, worker: null, reviewer: null },
};

const SHA = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/** 三種文件各有項目，含假說、未知、需重新查驗與已移除的項目。 */
const items = [
  understandingItem(),
  understandingItem({
    id: "und_2",
    section: "brief",
    key: "audience",
    text: "可能只給作者自己使用",
    status: "unknown",
    sources: [],
  }),
  understandingItem({
    id: "und_3",
    section: "glossary",
    key: "Workspace",
    text: "Baton 管理的一個工作目錄",
    status: "confirmed",
    stale: true,
    sources: [
      { kind: "file", path: "docs/workspace.md", line: null, version: SHA, checked_at: CHECKED_AT, changed: true },
      {
        kind: "external",
        reference: "https://example.test/spec",
        version: "2026-10-01",
        checked_at: CHECKED_AT + 60_000,
        changed: false,
      },
    ],
  }),
  understandingItem({
    id: "und_4",
    section: "map",
    key: "runtime → web",
    text: "Runtime 透過 HTTP 提供 Web UI 資料",
    status: "hypothesis",
    sources: [],
  }),
  understandingItem({ id: "und_5", section: "map", key: "legacy", text: "舊的同步模組", status: "retired", sources: [] }),
];

const conflict = projection({
  path: "GLOSSARY.md",
  version: 3,
  status: "conflict",
  external_hash: "hash_external",
  detected_at: CHECKED_AT,
});

function fakeClient(view: UnderstandingView = understandingView({ items })) {
  return {
    understanding: vi.fn(async () => view),
    syncUnderstanding: vi.fn(async () => view),
    reconcileProjection: vi.fn(async (_workspaceId: string, _request: ReconcileProjectionRequest) => view),
  };
}

const renderPage = (client: ReturnType<typeof fakeClient>) =>
  render(<ProjectUnderstandingPage client={client} workspace={workspace} timeZone="Asia/Taipei" />);

const section = (name: string) => screen.getByRole("region", { name });
const item = (name: string) => screen.getByRole("listitem", { name });
const projectionRow = (path: string) => screen.getByRole("listitem", { name: path });

describe("專案理解", () => {
  it("開啟時重新比對投影（sync），依三種文件分組並標示狀態；假說與未知不會被標成已確認", async () => {
    const client = fakeClient();
    renderPage(client);

    expect(await screen.findByRole("heading", { name: "專案理解" })).toBeInTheDocument();
    await screen.findByRole("region", { name: "Project Brief" });
    expect(client.syncUnderstanding).toHaveBeenCalledWith("ws_1");
    expect(client.understanding).not.toHaveBeenCalled();

    expect(within(section("Project Brief")).getByRole("listitem", { name: "產品目標（purpose）" })).toBeInTheDocument();
    expect(within(section("Project Brief")).getByRole("listitem", { name: "使用者（audience）" })).toBeInTheDocument();
    expect(within(section("Glossary")).getByRole("listitem", { name: "Workspace" })).toBeInTheDocument();
    expect(within(section("Glossary")).queryByRole("listitem", { name: "產品目標（purpose）" })).not.toBeInTheDocument();
    expect(within(section("Project Map")).getByRole("listitem", { name: "runtime → web" })).toBeInTheDocument();

    expect(item("產品目標（purpose）")).toHaveTextContent("已確認");
    expect(item("產品目標（purpose）")).toHaveTextContent("整理個人筆記的工具");
    expect(item("使用者（audience）")).toHaveTextContent("未知，待確認");
    expect(item("使用者（audience）")).not.toHaveTextContent("已確認");
    expect(item("runtime → web")).toHaveTextContent("假說，未確認");
    expect(item("runtime → web")).not.toHaveTextContent("已確認");
    expect(item("Workspace")).toHaveTextContent("來源已變更，需重新查驗");
    expect(item("產品目標（purpose）")).not.toHaveTextContent("需重新查驗");
  });

  it("Project Brief 的段落以中文名稱當標題、原始鍵以等寬字附在旁邊；不認得的鍵與其他文件的名稱照原樣顯示", async () => {
    const hypothesis = { status: "hypothesis" as const, sources: [] };
    renderPage(
      fakeClient(
        understandingView({
          items: [
            understandingItem(),
            understandingItem({ id: "und_6", key: "Success-Criteria", text: "每天都會用", ...hypothesis }),
            understandingItem({ id: "und_7", key: "產品定位", text: "個人工具", ...hypothesis }),
            understandingItem({ id: "und_8", section: "glossary", key: "users", text: "使用筆記的人", ...hypothesis }),
          ],
        }),
      ),
    );
    await screen.findByRole("region", { name: "Project Brief" });

    const purpose = item("產品目標（purpose）");
    expect(purpose.querySelector(".understanding-key")).toHaveTextContent(/^產品目標purpose$/);
    expect(within(purpose).getByText("purpose")).toHaveClass("mono");
    // 大小寫與連字號不影響對照
    expect(item("成功條件（Success-Criteria）")).toBeInTheDocument();
    // 不認得的鍵照原樣顯示，不附原始鍵
    expect(item("產品定位").querySelector(".understanding-key")).toHaveTextContent(/^產品定位$/);
    // Glossary 的名詞不是段落名稱，不轉換
    expect(within(section("Glossary")).getByRole("listitem", { name: "users" })).toBeInTheDocument();
  });

  it("某份文件還沒有項目時告訴你下一步", async () => {
    renderPage(fakeClient(understandingView({ items: [understandingItem()] })));

    const map = await screen.findByRole("region", { name: "Project Map" });
    expect(within(map).getByText("還沒有項目。Controller 接入專案時會建立，也可以在 Controller 中要求它補上。")).toBeInTheDocument();
  });

  it("已移除的項目預設隱藏，勾選「顯示已移除」後才顯示", async () => {
    const user = userEvent.setup();
    renderPage(fakeClient());
    await screen.findByRole("region", { name: "Project Map" });

    expect(screen.queryByRole("listitem", { name: "legacy" })).not.toBeInTheDocument();

    await user.click(screen.getByRole("checkbox", { name: "顯示已移除" }));

    expect(item("legacy")).toHaveTextContent("已移除");
  });

  it("每個來源顯示位置、版本（檔案取前 12 碼）、查驗時間，並標示已變更；各自一段，不以「・」串接", async () => {
    renderPage(fakeClient());
    await screen.findByRole("region", { name: "Project Brief" });

    // 每一段是 .meta 中各自換行的單位；文字節點也會成為一段，所以只能有元素子節點
    const parts = (source: HTMLElement) => {
      expect(source.childNodes).toHaveLength(source.children.length);
      return [...source.children].map((part) => part.textContent);
    };

    const purpose = within(screen.getByRole("list", { name: "產品目標（purpose） 的來源" })).getAllByRole("listitem");
    expect(purpose).toHaveLength(1);
    expect(parts(purpose[0])).toEqual(["README.md:3", "版本 0123456789ab", "查驗於 2023/11/15 06:13"]);

    const glossary = within(screen.getByRole("list", { name: "Workspace 的來源" })).getAllByRole("listitem");
    expect(parts(glossary[0])).toEqual(["docs/workspace.md", "版本 0123456789ab", "查驗於 2023/11/15 06:13", "已變更"]);
    expect(parts(glossary[1])).toEqual(["https://example.test/spec", "版本 2026-10-01", "查驗於 2023/11/15 06:14"]);
    for (const source of [...purpose, ...glossary]) expect(source).not.toHaveTextContent("・");
  });

  it("縮短的長路徑來源可以複製完整路徑", async () => {
    const path = String.raw`C:\work\notes\docs\design\very\deep\workspace.md`;
    renderPage(
      fakeClient(
        understandingView({
          items: [understandingItem({ sources: [{ kind: "file", path, line: 12, version: SHA, checked_at: CHECKED_AT, changed: false }] })],
        }),
      ),
    );
    await screen.findByRole("region", { name: "Project Brief" });

    const source = within(screen.getByRole("list", { name: "產品目標（purpose） 的來源" })).getByRole("listitem");
    expect(within(source).getByTitle(`${path}:12`)).toHaveTextContent(String.raw`C:\…\deep\workspace.md:12`);
    expect(within(source).getByRole("button", { name: "複製路徑" })).toBeInTheDocument();
  });

  it("沒有任何項目時說明 Controller 尚未建立專案理解", async () => {
    renderPage(fakeClient(understandingView({ items: [] })));

    expect(await screen.findByText(/Controller 尚未建立專案理解/)).toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "Project Brief" })).not.toBeInTheDocument();
  });

  it("列出投影的路徑、版本與狀態；同步中的投影沒有協調按鈕", async () => {
    renderPage(fakeClient(understandingView({ projections: [projection({ path: "docs/adr/0001-a.md", kind: "adr", version: 2 })] })));

    const row = await screen.findByRole("listitem", { name: "docs/adr/0001-a.md" });
    // 區段用使用者的說法，不叫「投影」
    expect(screen.getByRole("region", { name: "寫入 repo 的檔案（GLOSSARY.md、ADR）" })).toContainElement(row);
    expect(row).toHaveTextContent("第 2 版");
    expect(row).toHaveTextContent("已同步");
    expect(within(row).queryByRole("button")).not.toBeInTheDocument();
    expect(screen.queryByText(/個寫入 repo 的檔案被外部修改/)).not.toBeInTheDocument();
  });

  it("接入前就已存在、Baton 還沒寫入過的投影檔不顯示版本", async () => {
    renderPage(fakeClient(understandingView({ projections: [projection({ path: "GLOSSARY.md", version: 0, status: "conflict" })] })));

    const row = await screen.findByRole("listitem", { name: "GLOSSARY.md" });
    expect(row).toHaveTextContent("Baton 尚未寫入");
    expect(row).not.toHaveTextContent("第 0 版");
  });

  it("投影被外部修改時顯示提示；「以 Baton 版本覆寫」送出 path、overwrite 與看到的外部版本雜湊", async () => {
    const resolved = understandingView({ projections: [projection({ path: "GLOSSARY.md", version: 4 })] });
    const client = fakeClient(understandingView({ projections: [conflict] }));
    client.reconcileProjection.mockResolvedValueOnce(resolved);
    const user = userEvent.setup();
    renderPage(client);

    expect(await screen.findByText("1 個寫入 repo 的檔案被外部修改，Baton 沒有覆寫，請在下方協調")).toBeInTheDocument();
    const row = projectionRow("GLOSSARY.md");
    expect(row).toHaveTextContent("第 3 版");
    expect(row).toHaveTextContent("待協調");
    expect(row).toHaveTextContent("檔案已在外部被修改，Baton 沒有覆寫");

    await user.click(within(row).getByRole("button", { name: "以 Baton 版本覆寫" }));

    expect(client.reconcileProjection).toHaveBeenCalledWith("ws_1", {
      path: "GLOSSARY.md",
      action: "overwrite",
      expected_external_hash: "hash_external",
    });
    await waitFor(() => expect(projectionRow("GLOSSARY.md")).toHaveTextContent("已同步"));
    expect(projectionRow("GLOSSARY.md")).toHaveTextContent("第 4 版");
    expect(screen.queryByText(/個寫入 repo 的檔案被外部修改/)).not.toBeInTheDocument();
  });

  it("「保留外部版本」送出 keep_external；之後投影顯示 Baton 停止寫入，只能再以 Baton 版本覆寫（送最後一次比對的外部版本）", async () => {
    // 保留之後使用者又改了檔：重新比對把目前的雜湊記在 external_hash，baseline_hash 仍是保留當時的版本
    const detached = projection({
      path: "GLOSSARY.md",
      version: 3,
      status: "detached",
      baseline_hash: "hash_external",
      external_hash: "hash_edited_after_keep",
    });
    const client = fakeClient(understandingView({ projections: [conflict] }));
    client.reconcileProjection.mockResolvedValueOnce(understandingView({ projections: [detached] }));
    const user = userEvent.setup();
    renderPage(client);

    await user.click(within(await screen.findByRole("listitem", { name: "GLOSSARY.md" })).getByRole("button", { name: "保留外部版本" }));

    expect(client.reconcileProjection).toHaveBeenCalledWith("ws_1", {
      path: "GLOSSARY.md",
      action: "keep_external",
      expected_external_hash: "hash_external",
    });
    await waitFor(() => expect(projectionRow("GLOSSARY.md")).toHaveTextContent("已保留外部版本"));
    expect(projectionRow("GLOSSARY.md")).toHaveTextContent("Baton 停止寫入這個檔案");
    const row = projectionRow("GLOSSARY.md");
    expect(within(row).queryByRole("button", { name: "保留外部版本" })).not.toBeInTheDocument();

    await user.click(within(row).getByRole("button", { name: "以 Baton 版本覆寫" }));

    expect(client.reconcileProjection).toHaveBeenLastCalledWith("ws_1", {
      path: "GLOSSARY.md",
      action: "overwrite",
      expected_external_hash: "hash_edited_after_keep",
    });
  });

  it("協調中檔案被外部刪除時說明已刪除，協調時送出 null", async () => {
    const client = fakeClient(understandingView({ projections: [{ ...conflict, external_hash: null }] }));
    const user = userEvent.setup();
    renderPage(client);

    const row = await screen.findByRole("listitem", { name: "GLOSSARY.md" });
    expect(row).toHaveTextContent("待協調");
    expect(row).toHaveTextContent("檔案已在外部被刪除");

    await user.click(within(row).getByRole("button", { name: "保留外部版本" }));

    expect(client.reconcileProjection).toHaveBeenCalledWith("ws_1", {
      path: "GLOSSARY.md",
      action: "keep_external",
      expected_external_hash: null,
    });
  });

  it("檢視後檔案又被修改（409 projection_changed）時顯示提示並重新比對", async () => {
    const changed = { ...conflict, external_hash: "hash_newer" };
    const client = fakeClient(understandingView({ projections: [conflict] }));
    client.reconcileProjection.mockRejectedValueOnce(
      Object.assign(new Error("投影檔在檢視後又被修改"), { code: "projection_changed" }),
    );
    client.syncUnderstanding.mockResolvedValueOnce(understandingView({ projections: [conflict] }));
    client.syncUnderstanding.mockResolvedValueOnce(understandingView({ projections: [changed] }));
    const user = userEvent.setup();
    renderPage(client);

    await user.click(within(await screen.findByRole("listitem", { name: "GLOSSARY.md" })).getByRole("button", { name: "以 Baton 版本覆寫" }));

    expect(await screen.findByText(/檔案在檢視後又被修改，已重新檢查/)).toBeInTheDocument();
    expect(client.syncUnderstanding).toHaveBeenCalledTimes(2);
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();

    // 再按一次時送出的是重新比對後看到的外部版本
    await user.click(within(projectionRow("GLOSSARY.md")).getByRole("button", { name: "以 Baton 版本覆寫" }));
    expect(client.reconcileProjection).toHaveBeenLastCalledWith("ws_1", {
      path: "GLOSSARY.md",
      action: "overwrite",
      expected_external_hash: "hash_newer",
    });
  });

  it("其他協調錯誤照 Runtime 的說明顯示，不重新比對", async () => {
    const client = fakeClient(understandingView({ projections: [conflict] }));
    client.reconcileProjection.mockRejectedValueOnce(
      Object.assign(new Error("投影目前不能這樣協調"), { code: "projection_transition_invalid" }),
    );
    const user = userEvent.setup();
    renderPage(client);

    await user.click(within(await screen.findByRole("listitem", { name: "GLOSSARY.md" })).getByRole("button", { name: "保留外部版本" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("投影目前不能這樣協調");
    expect(client.syncUnderstanding).toHaveBeenCalledTimes(1);
    // 保留目前內容，讓使用者再決定
    expect(projectionRow("GLOSSARY.md")).toHaveTextContent("檔案已在外部被修改");
  });

  it("「重新檢查」重新比對投影並顯示最新結果", async () => {
    const client = fakeClient(understandingView({ projections: [projection()] }));
    client.syncUnderstanding.mockResolvedValueOnce(understandingView({ projections: [projection()] }));
    client.syncUnderstanding.mockResolvedValueOnce(understandingView({ projections: [conflict] }));
    const user = userEvent.setup();
    renderPage(client);
    await screen.findByRole("listitem", { name: "GLOSSARY.md" });

    await user.click(screen.getByRole("button", { name: "重新檢查" }));

    expect(await screen.findByText("1 個寫入 repo 的檔案被外部修改，Baton 沒有覆寫，請在下方協調")).toBeInTheDocument();
    expect(client.syncUnderstanding).toHaveBeenCalledTimes(2);
  });

  it("不是 Git repo 時說明 Baton 不會寫入 GLOSSARY.md 與 ADR", async () => {
    renderPage(fakeClient(understandingView({ projection_enabled: false, projections: [] })));

    expect(await screen.findByText("這個 Workspace 不是 Git repo，Baton 不會寫入 GLOSSARY.md 與 ADR")).toBeInTheDocument();
    expect(screen.queryByText("Baton 還沒有寫入任何檔案")).not.toBeInTheDocument();
  });

  it("讀不到專案理解時顯示 Runtime 的說明", async () => {
    const client = fakeClient();
    client.syncUnderstanding.mockRejectedValueOnce(new Error("找不到 Workspace：ws_1"));
    renderPage(client);

    expect(await screen.findByRole("alert")).toHaveTextContent("無法取得專案理解：找不到 Workspace：ws_1");
  });
});
