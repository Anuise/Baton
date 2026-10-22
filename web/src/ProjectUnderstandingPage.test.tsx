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

    expect(await screen.findByRole("heading", { name: "專案理解：筆記" })).toBeInTheDocument();
    await screen.findByRole("region", { name: "Project Brief" });
    expect(client.syncUnderstanding).toHaveBeenCalledWith("ws_1");
    expect(client.understanding).not.toHaveBeenCalled();

    expect(within(section("Project Brief")).getByRole("listitem", { name: "purpose" })).toBeInTheDocument();
    expect(within(section("Project Brief")).getByRole("listitem", { name: "audience" })).toBeInTheDocument();
    expect(within(section("Glossary")).getByRole("listitem", { name: "Workspace" })).toBeInTheDocument();
    expect(within(section("Glossary")).queryByRole("listitem", { name: "purpose" })).not.toBeInTheDocument();
    expect(within(section("Project Map")).getByRole("listitem", { name: "runtime → web" })).toBeInTheDocument();

    expect(item("purpose")).toHaveTextContent("已確認");
    expect(item("purpose")).toHaveTextContent("整理個人筆記的工具");
    expect(item("audience")).toHaveTextContent("未知・待確認");
    expect(item("audience")).not.toHaveTextContent("已確認");
    expect(item("runtime → web")).toHaveTextContent("假說・未確認");
    expect(item("runtime → web")).not.toHaveTextContent("已確認");
    expect(item("Workspace")).toHaveTextContent("來源已變更・需重新查驗");
    expect(item("purpose")).not.toHaveTextContent("需重新查驗");
  });

  it("已移除的項目預設隱藏，勾選「顯示已移除」後才顯示", async () => {
    const user = userEvent.setup();
    renderPage(fakeClient());
    await screen.findByRole("region", { name: "Project Map" });

    expect(screen.queryByRole("listitem", { name: "legacy" })).not.toBeInTheDocument();

    await user.click(screen.getByRole("checkbox", { name: "顯示已移除" }));

    expect(item("legacy")).toHaveTextContent("已移除");
  });

  it("每個來源顯示位置、版本（檔案取前 12 碼）、查驗時間，並標示已變更", async () => {
    renderPage(fakeClient());
    await screen.findByRole("region", { name: "Project Brief" });

    const purpose = within(screen.getByRole("list", { name: "purpose 的來源" })).getAllByRole("listitem");
    expect(purpose).toHaveLength(1);
    expect(purpose[0]).toHaveTextContent("README.md:3・版本 0123456789ab・查驗於 2023/11/15 06:13");
    expect(purpose[0]).not.toHaveTextContent("0123456789abc");
    expect(purpose[0]).not.toHaveTextContent("已變更");

    const glossary = within(screen.getByRole("list", { name: "Workspace 的來源" })).getAllByRole("listitem");
    expect(glossary[0]).toHaveTextContent("docs/workspace.md・版本 0123456789ab・查驗於 2023/11/15 06:13・已變更");
    expect(glossary[0]).not.toHaveTextContent("docs/workspace.md:");
    expect(glossary[1]).toHaveTextContent("https://example.test/spec・版本 2026-10-01・查驗於 2023/11/15 06:14");
    expect(glossary[1]).not.toHaveTextContent("已變更");
  });

  it("沒有任何項目時說明 Controller 尚未建立專案理解", async () => {
    renderPage(fakeClient(understandingView({ items: [] })));

    expect(await screen.findByText(/Controller 尚未建立專案理解/)).toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "Project Brief" })).not.toBeInTheDocument();
  });

  it("列出投影的路徑、版本與狀態；同步中的投影沒有協調按鈕", async () => {
    renderPage(fakeClient(understandingView({ projections: [projection({ path: "docs/adr/0001-a.md", kind: "adr", version: 2 })] })));

    const row = await screen.findByRole("listitem", { name: "docs/adr/0001-a.md" });
    expect(row).toHaveTextContent("第 2 版");
    expect(row).toHaveTextContent("已同步");
    expect(within(row).queryByRole("button")).not.toBeInTheDocument();
    expect(screen.queryByText(/個投影檔被外部修改/)).not.toBeInTheDocument();
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

    expect(await screen.findByText("1 個投影檔被外部修改，Baton 沒有覆寫，請協調")).toBeInTheDocument();
    const row = projectionRow("GLOSSARY.md");
    expect(row).toHaveTextContent("第 3 版");
    expect(row).toHaveTextContent("外部已修改・待協調");

    await user.click(within(row).getByRole("button", { name: "以 Baton 版本覆寫" }));

    expect(client.reconcileProjection).toHaveBeenCalledWith("ws_1", {
      path: "GLOSSARY.md",
      action: "overwrite",
      expected_external_hash: "hash_external",
    });
    await waitFor(() => expect(projectionRow("GLOSSARY.md")).toHaveTextContent("已同步"));
    expect(projectionRow("GLOSSARY.md")).toHaveTextContent("第 4 版");
    expect(screen.queryByText(/個投影檔被外部修改/)).not.toBeInTheDocument();
  });

  it("「保留外部版本」送出 keep_external；之後投影顯示 Baton 停止寫入，只能再以 Baton 版本覆寫（送 baseline_hash）", async () => {
    const detached = projection({
      path: "GLOSSARY.md",
      version: 3,
      status: "detached",
      baseline_hash: "hash_external",
      external_hash: null,
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
    await waitFor(() => expect(projectionRow("GLOSSARY.md")).toHaveTextContent("保留外部版本・Baton 停止寫入"));
    const row = projectionRow("GLOSSARY.md");
    expect(within(row).queryByRole("button", { name: "保留外部版本" })).not.toBeInTheDocument();

    await user.click(within(row).getByRole("button", { name: "以 Baton 版本覆寫" }));

    expect(client.reconcileProjection).toHaveBeenLastCalledWith("ws_1", {
      path: "GLOSSARY.md",
      action: "overwrite",
      expected_external_hash: "hash_external",
    });
  });

  it("協調中檔案被外部刪除時說明已刪除，協調時送出 null", async () => {
    const client = fakeClient(understandingView({ projections: [{ ...conflict, external_hash: null }] }));
    const user = userEvent.setup();
    renderPage(client);

    const row = await screen.findByRole("listitem", { name: "GLOSSARY.md" });
    expect(row).toHaveTextContent("外部已刪除・待協調");

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

    expect(await screen.findByText(/投影檔在檢視後又被修改，已重新檢查/)).toBeInTheDocument();
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
    expect(projectionRow("GLOSSARY.md")).toHaveTextContent("外部已修改・待協調");
  });

  it("「重新檢查」重新比對投影並顯示最新結果", async () => {
    const client = fakeClient(understandingView({ projections: [projection()] }));
    client.syncUnderstanding.mockResolvedValueOnce(understandingView({ projections: [projection()] }));
    client.syncUnderstanding.mockResolvedValueOnce(understandingView({ projections: [conflict] }));
    const user = userEvent.setup();
    renderPage(client);
    await screen.findByRole("listitem", { name: "GLOSSARY.md" });

    await user.click(screen.getByRole("button", { name: "重新檢查" }));

    expect(await screen.findByText("1 個投影檔被外部修改，Baton 沒有覆寫，請協調")).toBeInTheDocument();
    expect(client.syncUnderstanding).toHaveBeenCalledTimes(2);
  });

  it("不是 Git repo 時說明不產生 GLOSSARY／ADR 投影", async () => {
    renderPage(fakeClient(understandingView({ projection_enabled: false, projections: [] })));

    expect(await screen.findByText("這個 Workspace 不是 Git repo，不產生 GLOSSARY／ADR 投影")).toBeInTheDocument();
    expect(screen.queryByText("還沒有投影檔")).not.toBeInTheDocument();
  });

  it("讀不到專案理解時顯示 Runtime 的說明", async () => {
    const client = fakeClient();
    client.syncUnderstanding.mockRejectedValueOnce(new Error("找不到 Workspace：ws_1"));
    renderPage(client);

    expect(await screen.findByRole("alert")).toHaveTextContent("無法取得專案理解：找不到 Workspace：ws_1");
  });
});
