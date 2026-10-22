import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { KnowledgePage } from "./KnowledgePage";
import type { KnowledgeClient, KnowledgeDetail, KnowledgeEntryView, SearchMode, Workspace } from "./runtimeClient";

const workspace: Workspace = {
  id: "ws_1",
  name: "筆記",
  path: "C:\\work\\notes",
  status: "active",
  version: 1,
  created_at: 1_700_000_000_000,
  archived_at: null,
};

/** 2023-11-14 22:13:20 UTC（台北是隔天 06:13）。 */
const T0 = 1_700_000_000_000;
const DAY = 86_400_000;

function entry(overrides: Partial<KnowledgeEntryView> = {}): KnowledgeEntryView {
  return {
    id: "kn_1",
    kind: "lesson",
    scope: { kind: "workspace", workspace_id: "ws_1", goal_id: null, task_id: null },
    status: "verified",
    source: { kind: "user" },
    valid_from: T0,
    valid_until: null,
    version: 1,
    revision: 3,
    supersedes: null,
    superseded_by: null,
    status_reason: null,
    content: {
      title: "部署前先備份資料庫",
      body: "上線前一定要備份",
      tags: ["部署", "資料庫"],
      evidence: ["事故報告 #12"],
      lesson: { trigger: "執行資料庫遷移", root_cause: "沒有備份可還原", resolution: "遷移前先執行備份" },
    },
    created_at: T0,
    updated_at: T0,
    effective: true,
    ...overrides,
  };
}

const candidate = entry({
  id: "kn_c",
  status: "candidate",
  effective: false,
  revision: 1,
  source: { kind: "agent", run_id: "run_7", generation: 2, task_id: "task_3" },
  content: {
    title: "測試前清空快取",
    body: "",
    tags: [],
    evidence: ["run_7 的日誌第 40 行"],
    lesson: { trigger: "測試偶發失敗", root_cause: "疑似快取殘留（待驗）", resolution: "測試前清空快取" },
  },
});

/** 以記憶體模擬 Runtime：操作會改變條目並遞增 revision，讓重新讀取看得到結果。 */
function fakeClient(initial: KnowledgeEntryView[], searchMode: SearchMode = "full_text") {
  let items = [...initial];
  const update = (id: string, patch: Partial<KnowledgeEntryView>) => {
    const current = items.find((item) => item.id === id)!;
    const next = { ...current, revision: current.revision + 1, ...patch };
    items = items.map((item) => (item.id === id ? next : item));
    return next;
  };
  return {
    listKnowledge: vi.fn(async ({ includeInactive }: { includeInactive: boolean }) => ({
      search_mode: searchMode,
      entries: includeInactive ? [...items] : items.filter((item) => item.effective),
    })),
    createKnowledge: vi.fn(async () => {
      const created = entry({ id: `kn_new_${items.length}` });
      items = [created, ...items];
      return created;
    }),
    knowledgeDetail: vi.fn(async (): Promise<KnowledgeDetail> => {
      throw new Error("未設定");
    }),
    confirmKnowledge: vi.fn(async (id: string) => update(id, { status: "verified", effective: true })),
    correctKnowledge: vi.fn(async (id: string) => update(id, {})),
    revokeKnowledge: vi.fn(async (id: string, { reason }: { reason: string }) =>
      update(id, { status: "revoked", effective: false, status_reason: reason }),
    ),
    retireKnowledge: vi.fn(async (id: string, { reason }: { reason: string }) =>
      update(id, { status: "retired", effective: false, status_reason: reason }),
    ),
  } satisfies KnowledgeClient;
}

const renderPage = (client: KnowledgeClient) => render(<KnowledgePage client={client} workspace={workspace} timeZone="Asia/Taipei" />);

const region = (name: string) => screen.findByRole("region", { name });

describe("知識", () => {
  it("以目前的 Workspace 讀取有效條目與含無效的條目", async () => {
    const client = fakeClient([]);

    renderPage(client);

    expect(await screen.findByRole("heading", { name: "知識：筆記" })).toBeInTheDocument();
    await waitFor(() => expect(client.listKnowledge).toHaveBeenCalledTimes(2));
    expect(client.listKnowledge).toHaveBeenCalledWith({ workspaceId: "ws_1", q: undefined, includeInactive: false });
    expect(client.listKnowledge).toHaveBeenCalledWith({ workspaceId: "ws_1", q: undefined, includeInactive: true });
    expect(await screen.findByText("還沒有有效的條目")).toBeInTheDocument();
    expect(screen.getByText("沒有待確認的候選")).toBeInTheDocument();
  });

  it("讀取失敗時顯示 Runtime 的說明", async () => {
    const client = fakeClient([]);
    client.listKnowledge.mockRejectedValue(new Error("需要桌面憑證"));

    renderPage(client);

    expect(await screen.findByRole("alert")).toHaveTextContent("無法取得知識：需要桌面憑證");
  });

  it("有效條目顯示種類、範圍、標題、內文、教訓欄位、標籤、證據、版本與有效期限", async () => {
    renderPage(fakeClient([entry({ version: 2, valid_until: T0 + 30 * DAY })]));

    const card = within(await region("有效條目")).getByRole("article", { name: "部署前先備份資料庫" });
    expect(card).toHaveTextContent("已驗證");
    expect(card).toHaveTextContent("教訓");
    expect(card).toHaveTextContent("範圍：Workspace");
    expect(card).toHaveTextContent("v2");
    expect(card).toHaveTextContent("有效期間：2023/11/15 06:13 起，至 2023/12/15 06:13");
    expect(card).toHaveTextContent("來源：使用者");
    expect(card).toHaveTextContent("上線前一定要備份");
    expect(card).toHaveTextContent("觸發條件執行資料庫遷移");
    expect(card).toHaveTextContent("根因或待驗假說沒有備份可還原");
    expect(card).toHaveTextContent("解法遷移前先執行備份");
    expect(within(card).getByRole("list", { name: "標籤" })).toHaveTextContent("部署資料庫");
    expect(card).toHaveTextContent("證據：事故報告 #12");
    // 已驗證的條目不能再確認
    expect(within(card).queryByRole("button", { name: "確認為已驗證" })).not.toBeInTheDocument();
  });

  it("沒有期限的條目註明無期限；全域條目標示全域", async () => {
    renderPage(fakeClient([entry({ scope: { kind: "global", workspace_id: null, goal_id: null, task_id: null } })]));

    const card = await screen.findByRole("article", { name: "部署前先備份資料庫" });
    expect(card).toHaveTextContent("範圍：全域");
    expect(card).toHaveTextContent("無期限");
  });

  describe("待確認的候選教訓", () => {
    it("清楚標示尚未驗證，顯示來源 Run、內容與證據；不出現在有效條目", async () => {
      renderPage(fakeClient([candidate, entry()]));

      const section = await region("待確認的候選教訓");
      const card = within(section).getByRole("article", { name: "測試前清空快取" });
      expect(card).toHaveTextContent("候選・尚未驗證");
      expect(card).toHaveTextContent("來源：Agent（Run run_7，第 2 次，Task task_3）");
      expect(card).toHaveTextContent("疑似快取殘留（待驗）");
      expect(card).toHaveTextContent("證據：run_7 的日誌第 40 行");
      expect(within(section).getByRole("heading", { name: "待確認的候選教訓（1）" })).toBeInTheDocument();
      expect(within(await region("有效條目")).queryByRole("article", { name: "測試前清空快取" })).not.toBeInTheDocument();
    });

    it("確認為已驗證：帶 revision 送出，重新讀取後移到有效條目", async () => {
      const user = userEvent.setup();
      const client = fakeClient([candidate]);
      renderPage(client);

      const card = within(await region("待確認的候選教訓")).getByRole("article", { name: "測試前清空快取" });
      await user.click(within(card).getByRole("button", { name: "確認為已驗證" }));

      expect(client.confirmKnowledge).toHaveBeenCalledWith("kn_c", 1);
      expect(await within(await region("有效條目")).findByRole("article", { name: "測試前清空快取" })).toHaveTextContent(
        "已驗證",
      );
      expect(within(await region("待確認的候選教訓")).getByText("沒有待確認的候選")).toBeInTheDocument();
    });

    it("撤銷必須填寫原因，沒填時不送出", async () => {
      const user = userEvent.setup();
      const client = fakeClient([candidate]);
      renderPage(client);

      const card = within(await region("待確認的候選教訓")).getByRole("article", { name: "測試前清空快取" });
      await user.click(within(card).getByRole("button", { name: "撤銷" }));
      const form = within(card).getByRole("form", { name: "撤銷" });
      await user.click(within(form).getByRole("button", { name: "確認撤銷" }));

      expect(within(form).getByRole("alert")).toHaveTextContent("請填寫原因");
      expect(client.revokeKnowledge).not.toHaveBeenCalled();

      await user.type(within(form).getByLabelText("撤銷原因"), "假說不成立");
      await user.click(within(form).getByRole("button", { name: "確認撤銷" }));

      expect(client.revokeKnowledge).toHaveBeenCalledWith("kn_c", { expected_revision: 1, reason: "假說不成立" });
      await waitFor(() => expect(screen.queryByRole("article", { name: "測試前清空快取" })).not.toBeInTheDocument());
    });
  });

  it("修正：表單預填目前內容，送出完整新內容、expected_revision、沿用的有效期限與修正說明", async () => {
    const user = userEvent.setup();
    const client = fakeClient([entry({ valid_until: T0 + DAY })]);
    renderPage(client);

    const card = await screen.findByRole("article", { name: "部署前先備份資料庫" });
    await user.click(within(card).getByRole("button", { name: "修正" }));
    const form = within(card).getByRole("form", { name: "修正" });
    expect(within(form).getByLabelText("標題")).toHaveValue("部署前先備份資料庫");
    expect(within(form).getByLabelText("標籤（以逗號或換行分隔）")).toHaveValue("部署, 資料庫");
    expect(within(form).getByLabelText("觸發條件")).toHaveValue("執行資料庫遷移");

    await user.clear(within(form).getByLabelText("標題"));
    await user.type(within(form).getByLabelText("標題"), "遷移前先備份資料庫");
    await user.clear(within(form).getByLabelText("標籤（以逗號或換行分隔）"));
    await user.type(within(form).getByLabelText("標籤（以逗號或換行分隔）"), "部署，遷移, 備份");
    await user.type(within(form).getByLabelText("修正說明（選填）"), "標題更精確");
    await user.click(within(form).getByRole("button", { name: "送出修正" }));

    expect(client.correctKnowledge).toHaveBeenCalledWith("kn_1", {
      expected_revision: 3,
      title: "遷移前先備份資料庫",
      body: "上線前一定要備份",
      tags: ["部署", "遷移", "備份"],
      evidence: ["事故報告 #12"],
      lesson: { trigger: "執行資料庫遷移", root_cause: "沒有備份可還原", resolution: "遷移前先執行備份" },
      valid_until: T0 + DAY,
      note: "標題更精確",
    });
    await waitFor(() => expect(within(card).queryByRole("form", { name: "修正" })).not.toBeInTheDocument());
  });

  it("修正非教訓的條目時沒有教訓欄位，也不送出 lesson", async () => {
    const user = userEvent.setup();
    const fact = entry({ kind: "fact", content: { title: "伺服器在台北", body: "", tags: [], evidence: [], lesson: null } });
    const client = fakeClient([fact]);
    renderPage(client);

    const card = await screen.findByRole("article", { name: "伺服器在台北" });
    await user.click(within(card).getByRole("button", { name: "修正" }));
    const form = within(card).getByRole("form", { name: "修正" });
    expect(within(form).queryByLabelText("觸發條件")).not.toBeInTheDocument();
    await user.click(within(form).getByRole("button", { name: "送出修正" }));

    expect(client.correctKnowledge).toHaveBeenCalledWith("kn_1", {
      expected_revision: 3,
      title: "伺服器在台北",
      body: "",
      tags: [],
      evidence: [],
      lesson: undefined,
      valid_until: undefined,
      note: undefined,
    });
  });

  it("淘汰需填原因，之後只在歷史中以無效顯示", async () => {
    const user = userEvent.setup();
    const client = fakeClient([entry()]);
    renderPage(client);

    const card = await screen.findByRole("article", { name: "部署前先備份資料庫" });
    await user.click(within(card).getByRole("button", { name: "淘汰" }));
    await user.type(within(card).getByLabelText("淘汰原因"), "已改用自動備份");
    await user.click(within(card).getByRole("button", { name: "確認淘汰" }));

    expect(client.retireKnowledge).toHaveBeenCalledWith("kn_1", { expected_revision: 3, reason: "已改用自動備份" });
    expect(await screen.findByText("還沒有有效的條目")).toBeInTheDocument();

    await user.click(screen.getByLabelText("顯示歷史"));
    const retired = within(await region("歷史條目")).getByRole("article", { name: "部署前先備份資料庫" });
    expect(retired).toHaveTextContent("無效：已淘汰");
    expect(retired).toHaveTextContent("原因：已改用自動備份");
    // 已淘汰的條目沒有操作，只能看詳情
    expect(within(retired).getAllByRole("button").map((b) => b.textContent)).toEqual(["詳情"]);
  });

  it("別人已改過（revision 衝突）時提示已重新整理並重新讀取", async () => {
    const user = userEvent.setup();
    const client = fakeClient([candidate]);
    client.confirmKnowledge.mockRejectedValueOnce(
      Object.assign(new Error("條目已被修改"), { code: "knowledge_revision_conflict" }),
    );
    renderPage(client);

    const card = within(await region("待確認的候選教訓")).getByRole("article", { name: "測試前清空快取" });
    await waitFor(() => expect(client.listKnowledge).toHaveBeenCalledTimes(2));
    await user.click(within(card).getByRole("button", { name: "確認為已驗證" }));

    expect(await screen.findByRole("status")).toHaveTextContent("已被修改，已重新整理");
    expect(client.listKnowledge).toHaveBeenCalledTimes(4);
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("其他操作失敗時在該條目顯示 Runtime 的說明，表單保留", async () => {
    const user = userEvent.setup();
    const client = fakeClient([entry()]);
    client.revokeKnowledge.mockRejectedValueOnce(new Error("狀態不允許撤銷"));
    renderPage(client);

    const card = await screen.findByRole("article", { name: "部署前先備份資料庫" });
    await user.click(within(card).getByRole("button", { name: "撤銷" }));
    await user.type(within(card).getByLabelText("撤銷原因"), "錯誤");
    await user.click(within(card).getByRole("button", { name: "確認撤銷" }));

    expect(await within(card).findByRole("alert")).toHaveTextContent("狀態不允許撤銷");
    expect(within(card).getByRole("form", { name: "撤銷" })).toBeInTheDocument();
  });

  describe("搜尋", () => {
    it("送出中文關鍵字，以空白分詞", async () => {
      const user = userEvent.setup();
      const client = fakeClient([]);
      renderPage(client);
      await screen.findByText("還沒有有效的條目");

      await user.type(screen.getByLabelText("搜尋關鍵字"), " 部署 備份 ");
      await user.click(screen.getByRole("button", { name: "搜尋" }));

      await waitFor(() =>
        expect(client.listKnowledge).toHaveBeenCalledWith({ workspaceId: "ws_1", q: "部署 備份", includeInactive: false }),
      );
      expect(client.listKnowledge).toHaveBeenCalledWith({ workspaceId: "ws_1", q: "部署 備份", includeInactive: true });
      expect(await screen.findByText("沒有符合搜尋的有效條目")).toBeInTheDocument();
    });

    it("任何關鍵字少於 2 個字元時提示且不送出", async () => {
      const user = userEvent.setup();
      const client = fakeClient([]);
      renderPage(client);
      await screen.findByText("還沒有有效的條目");
      client.listKnowledge.mockClear();

      await user.type(screen.getByLabelText("搜尋關鍵字"), "部署 庫");
      await user.click(screen.getByRole("button", { name: "搜尋" }));

      expect(screen.getByRole("alert")).toHaveTextContent("每個關鍵字至少 2 個字元（「庫」太短）");
      expect(client.listKnowledge).not.toHaveBeenCalled();
    });

    it("全文檢索降級時顯示明顯的提示", async () => {
      renderPage(fakeClient([], "like_fallback"));

      expect(await screen.findByRole("status")).toHaveTextContent(
        "全文檢索無法使用，已改用逐筆比對（降級）：結果相同但較慢",
      );
    });

    it("全文檢索正常時沒有降級提示", async () => {
      renderPage(fakeClient([]));

      await screen.findByText("還沒有有效的條目");
      expect(screen.queryByText(/降級/)).not.toBeInTheDocument();
    });
  });

  it("顯示歷史：預設隱藏；打開後列出撤銷、被取代與過期的條目與原因，標示為無效", async () => {
    const user = userEvent.setup();
    renderPage(
      fakeClient([
        entry(),
        entry({
          id: "kn_r",
          status: "revoked",
          effective: false,
          status_reason: "內容錯誤",
          content: { ...entry().content, title: "舊做法" },
        }),
        entry({
          id: "kn_s",
          status: "superseded",
          effective: false,
          superseded_by: "kn_1",
          content: { ...entry().content, title: "被取代的做法" },
        }),
        entry({ id: "kn_e", effective: false, valid_until: T0 + DAY, content: { ...entry().content, title: "過期的做法" } }),
      ]),
    );

    await screen.findByRole("article", { name: "部署前先備份資料庫" });
    expect(screen.queryByRole("region", { name: "歷史條目" })).not.toBeInTheDocument();

    await user.click(screen.getByLabelText("顯示歷史"));

    const history = await region("歷史條目");
    expect(within(history).getAllByRole("article").map((a) => a.getAttribute("aria-label"))).toEqual([
      "舊做法",
      "被取代的做法",
      "過期的做法",
    ]);
    const revoked = within(history).getByRole("article", { name: "舊做法" });
    expect(revoked).toHaveTextContent("無效：已撤銷");
    expect(revoked).toHaveTextContent("原因：內容錯誤");
    expect(within(history).getByRole("article", { name: "被取代的做法" })).toHaveTextContent("已被 kn_1 取代");
    expect(within(history).getByRole("article", { name: "過期的做法" })).toHaveTextContent("無效：已過期");
  });

  it("詳情：版本由舊到新，變更歷史含原因", async () => {
    const user = userEvent.setup();
    const client = fakeClient([entry({ version: 2 })]);
    const current = entry({ version: 2 });
    client.knowledgeDetail.mockResolvedValue({
      entry: current,
      versions: [
        { version: 2, content: current.content, note: "補上解法", created_at: T0 + 60_000 },
        { version: 1, content: { ...current.content, title: "先備份" }, note: null, created_at: T0 },
      ],
      history: [
        { revision: 1, event_type: "knowledge_created", occurred_at: T0, payload: {} },
        { revision: 2, event_type: "knowledge_corrected", occurred_at: T0 + 60_000, payload: {} },
        { revision: 3, event_type: "knowledge_revoked", occurred_at: T0 + 120_000, payload: { reason: "誤判" } },
      ],
    });
    renderPage(client);

    const card = await screen.findByRole("article", { name: "部署前先備份資料庫" });
    await user.click(within(card).getByRole("button", { name: "詳情" }));

    const detail = await region("條目詳情");
    expect(await within(detail).findByRole("heading", { name: "詳情：部署前先備份資料庫" })).toBeInTheDocument();
    expect(client.knowledgeDetail).toHaveBeenCalledWith("kn_1");
    const versions = within(detail).getAllByRole("listitem").filter((li) => li.getAttribute("aria-label")?.startsWith("v"));
    expect(versions.map((li) => li.getAttribute("aria-label"))).toEqual(["v1", "v2"]);
    expect(versions[0]).toHaveTextContent("先備份");
    expect(versions[1]).toHaveTextContent("修正說明：補上解法");
    expect(detail).toHaveTextContent("2023/11/15 06:13 建立");
    expect(detail).toHaveTextContent("2023/11/15 06:14 修正");
    expect(detail).toHaveTextContent("2023/11/15 06:15 撤銷：誤判");

    await user.click(within(detail).getByRole("button", { name: "關閉詳情" }));
    expect(screen.queryByRole("region", { name: "條目詳情" })).not.toBeInTheDocument();
  });

  describe("新增條目", () => {
    it("教訓欄位只在種類為教訓時顯示", async () => {
      const user = userEvent.setup();
      renderPage(fakeClient([]));

      const form = await screen.findByRole("form", { name: "新增條目" });
      expect(within(form).getByLabelText("觸發條件")).toBeInTheDocument();

      await user.selectOptions(within(form).getByLabelText("種類"), "決策");
      expect(within(form).queryByLabelText("觸發條件")).not.toBeInTheDocument();
    });

    it("預設建立在此 Workspace，標籤以逗號或換行、證據以換行分隔；成功後清空並重新讀取", async () => {
      const user = userEvent.setup();
      const client = fakeClient([]);
      renderPage(client);

      const form = await screen.findByRole("form", { name: "新增條目" });
      await user.type(within(form).getByLabelText("標題"), "不要在週五部署");
      await user.type(within(form).getByLabelText("內文"), "週末沒人值班");
      await user.type(within(form).getByLabelText("觸發條件"), "週五下午");
      await user.type(within(form).getByLabelText("標籤（以逗號或換行分隔）"), "部署, 流程，值班");
      await user.type(within(form).getByLabelText("證據（每行一項）"), "事故 #3{Enter}{Enter}事故 #5");
      await user.click(within(form).getByRole("button", { name: "新增" }));

      expect(client.createKnowledge).toHaveBeenCalledWith({
        kind: "lesson",
        scope: { kind: "workspace", workspace_id: "ws_1" },
        title: "不要在週五部署",
        body: "週末沒人值班",
        tags: ["部署", "流程", "值班"],
        evidence: ["事故 #3", "事故 #5"],
        lesson: { trigger: "週五下午", root_cause: "", resolution: "" },
      });
      await waitFor(() => expect(within(form).getByLabelText("標題")).toHaveValue(""));
      expect(await screen.findAllByRole("article")).toHaveLength(1);
    });

    it("可建立全域的決策，不帶教訓欄位", async () => {
      const user = userEvent.setup();
      const client = fakeClient([]);
      renderPage(client);

      const form = await screen.findByRole("form", { name: "新增條目" });
      await user.selectOptions(within(form).getByLabelText("種類"), "決策");
      await user.selectOptions(within(form).getByLabelText("範圍"), "全域");
      await user.type(within(form).getByLabelText("標題"), "統一使用 pnpm");
      await user.click(within(form).getByRole("button", { name: "新增" }));

      expect(client.createKnowledge).toHaveBeenCalledWith({
        kind: "decision",
        scope: { kind: "global" },
        title: "統一使用 pnpm",
        body: "",
        tags: [],
        evidence: [],
        lesson: undefined,
      });
    });

    it("標題空白時提示且不送出；Runtime 拒絕時顯示說明並保留輸入", async () => {
      const user = userEvent.setup();
      const client = fakeClient([]);
      client.createKnowledge.mockRejectedValueOnce(new Error("標題最多 200 個字元"));
      renderPage(client);

      const form = await screen.findByRole("form", { name: "新增條目" });
      await user.click(within(form).getByRole("button", { name: "新增" }));
      expect(within(form).getByRole("alert")).toHaveTextContent("請輸入標題");
      expect(client.createKnowledge).not.toHaveBeenCalled();

      await user.type(within(form).getByLabelText("標題"), "很長的標題");
      await user.click(within(form).getByRole("button", { name: "新增" }));

      expect(await within(form).findByRole("alert")).toHaveTextContent("標題最多 200 個字元");
      expect(within(form).getByLabelText("標題")).toHaveValue("很長的標題");
    });
  });
});
