import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { CHECKED_AT, explorationFinding, explorationOverview, reproductionAttempt } from "./fixtures";
import type { ExplorationOverview, Workspace } from "./runtimeClient";
import { ExplorationPage } from "./ExplorationPage";

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

function fakeClient(overview: ExplorationOverview = explorationOverview()) {
  return { explorationOverview: vi.fn(async () => overview) };
}

const renderPage = (client: ReturnType<typeof fakeClient>) =>
  render(<ExplorationPage client={client} workspace={workspace} timeZone="Asia/Taipei" />);

const group = (title: string) => screen.getByRole("region", { name: title });

describe("探索", () => {
  it("顯示每個探索 Task 目前的章程", async () => {
    const client = fakeClient();
    renderPage(client);

    expect(await screen.findByRole("heading", { name: "探索" })).toBeInTheDocument();
    const charter = await screen.findByRole("region", { name: "探索結帳" });
    expect(client.explorationOverview).toHaveBeenCalledWith("ws_1");
    expect(charter).toHaveTextContent("區域結帳流程");
    expect(charter).toHaveTextContent("角色一般顧客");
    expect(charter).toHaveTextContent("目標找出金額計算錯誤");
    expect(charter).toHaveTextContent("步數上限40 步");
    expect(charter).toHaveTextContent("成本上限模型呼叫 5 次");
    expect(charter).toHaveTextContent("版本第 2 版");
    const effects = within(charter).getByRole("list", { name: "探索結帳 禁止的副作用" });
    expect(within(effects).getAllByRole("listitem").map((item) => item.textContent)).toEqual([
      "不得送出真實付款",
      "不得寄出通知信",
    ]);
  });

  it("疑點依狀態分在各自的區塊，疑點絕不出現在已確認缺陷區，也不顯示「已確認」", async () => {
    renderPage(fakeClient());

    // 標題不等資料讀回就會出現：等分區出現再斷言
    const defects = await screen.findByRole("region", { name: "已確認缺陷" });
    const suspected = group("疑點（未經重現）");
    const risks = group("未驗風險");
    const fixed = group("已修復・納入回歸");
    const excluded = group("已排除");

    const titles = (region: HTMLElement) =>
      within(region)
        .queryAllByRole("heading", { level: 4 })
        .map((heading) => heading.textContent);
    expect(titles(defects)).toEqual(["折扣後總額少算", "運費重複計算"]);
    expect(titles(suspected)).toEqual(["優惠碼欄位接受負數"]);
    expect(titles(risks)).toEqual(["大量商品時結帳變慢"]);
    expect(titles(fixed)).toEqual(["稅額四捨五入錯誤"]);
    expect(titles(excluded)).toEqual(["測試站台連線逾時", "折扣碼不能疊加"]);

    expect(within(defects).queryByRole("region", { name: "優惠碼欄位接受負數" })).not.toBeInTheDocument();
    expect(suspected).not.toHaveTextContent("已確認");
  });

  it("只有疑點時，已確認缺陷區是空的", async () => {
    renderPage(fakeClient(explorationOverview({ findings: [explorationFinding()] })));

    // 標題不等資料讀回就會出現：等分區出現再斷言
    await screen.findByRole("region", { name: "已確認缺陷" });
    expect(group("已確認缺陷")).toHaveTextContent("沒有已確認缺陷。");
    expect(within(group("已確認缺陷")).queryByRole("heading", { level: 4 })).not.toBeInTheDocument();
    const finding = within(group("疑點（未經重現）")).getByRole("region", { name: "優惠碼欄位接受負數" });
    expect(finding).not.toHaveTextContent("已確認");
  });

  it("每個疑點顯示 Task、功能、有序的重現步驟、預期、實際觀察、環境、證據與回報者", async () => {
    renderPage(fakeClient());

    const finding = await screen.findByRole("region", { name: "優惠碼欄位接受負數" });
    expect(finding).toHaveTextContent("Task探索結帳");
    expect(finding).toHaveTextContent("功能checkout");
    expect(finding).toHaveTextContent("預期拒絕負數優惠碼");
    expect(finding).toHaveTextContent("實際觀察總額增加 50 元");
    expect(finding).toHaveTextContent("環境本機 dev server");
    expect(finding).toHaveTextContent("證據art_1、art_2");
    expect(finding).toHaveTextContent("回報者run_1");
    const steps = within(finding).getByRole("list", { name: "優惠碼欄位接受負數 的重現步驟" });
    expect(steps.tagName).toBe("OL");
    expect(within(steps).getAllByRole("listitem").map((item) => item.textContent)).toEqual([
      "加入一件商品",
      "在優惠碼欄位輸入 -50",
      "按下套用",
    ]);
  });

  it("疑點卡只攤開摘要；重現步驟與回報資訊收在預設收合的詳細資訊中，可展開", async () => {
    const user = userEvent.setup();
    renderPage(fakeClient());

    const finding = await screen.findByRole("region", { name: "優惠碼欄位接受負數" });
    const toggle = within(finding).getByText("重現步驟與詳細資訊（3 步）");
    expect(toggle.tagName).toBe("SUMMARY");
    const details = toggle.closest("details")!;
    expect(details).not.toHaveAttribute("open");
    // 摘要：Task、功能、預期、實際觀察在收合區之外
    for (const label of ["Task", "功能", "預期", "實際觀察"]) {
      expect(details).not.toContainElement(within(finding).getByText(label, { selector: "dt" }));
    }
    // 收合區：重現步驟、章程、環境、證據、回報者、回報於
    expect(details).toContainElement(within(finding).getByRole("list", { name: "優惠碼欄位接受負數 的重現步驟", hidden: true }));
    for (const label of ["章程", "環境", "證據", "回報者", "回報於"]) {
      expect(details).toContainElement(within(finding).getByText(label, { selector: "dt" }));
    }
    // 摘要與收合區的說明清單同一種欄寬（值欄起點一致）
    expect(within(details).getByText("環境", { selector: "dt" }).closest("dl")).toHaveClass("exploration-summary");

    await user.click(toggle);
    expect(details).toHaveAttribute("open");
  });

  it("已確認缺陷的重現案例欄位留在摘要中", async () => {
    renderPage(fakeClient());

    const defect = await screen.findByRole("region", { name: "折扣後總額少算" });
    const details = within(defect).getByText(/^重現步驟與詳細資訊/).closest("details")!;
    expect(details).not.toContainElement(within(defect).getByText("確認的 Evidence Bundle", { selector: "dt" }));
    expect(details).not.toContainElement(within(defect).getByText("失敗的步驟", { selector: "dt" }));
  });

  it("已確認缺陷顯示確認它的 Evidence Bundle 與失敗的步驟", async () => {
    renderPage(fakeClient());

    const defect = await screen.findByRole("region", { name: "折扣後總額少算" });
    expect(defect).toHaveTextContent("確認的 Evidence Bundleevb_11");
    expect(defect).toHaveTextContent("失敗的步驟讀取折扣後總額（total）");
    expect(defect).not.toHaveTextContent("回歸失敗");

    const regressed = screen.getByRole("region", { name: "運費重複計算" });
    expect(within(regressed).getByText("回歸失敗", { selector: ".exploration-badge" })).toBeInTheDocument();
    expect(regressed).toHaveTextContent("確認的 Evidence Bundleevb_21");
    expect(regressed).not.toHaveTextContent("功能");
    expect(regressed).toHaveTextContent("證據沒有附證據");
    // 狀態徽章與標題同列；確認的 Bundle 與疑點的欄位在同一個清單中，標籤欄對齊
    expect(within(regressed).getByText("回歸失敗").parentElement).toBe(within(regressed).getByRole("heading", { level: 4 }).parentElement);
    expect(within(regressed).getByText("確認的 Evidence Bundle").closest("dl")).toBe(within(regressed).getByText("Task").closest("dl"));
  });

  it("「確認的 Evidence Bundle」在 8rem 標籤欄折行時「Evidence Bundle」不拆開（不讓「Bundle」單獨一行）", async () => {
    renderPage(fakeClient());

    const defect = await screen.findByRole("region", { name: "折扣後總額少算" });
    expect(within(defect).getByText("確認的 Evidence Bundle", { selector: "dt" }).textContent).toBe("確認的 Evidence Bundle");
  });

  it("長的 Bundle、驗證要求與證據識別碼以等寬縮短顯示，完整值在 title 並可複製", async () => {
    const bundle = "evb_7b5deb026d9841b0cced7314";
    const request = "vreq_e3b0c44298fc1c149afbf4c8";
    renderPage(
      fakeClient(
        explorationOverview({
          findings: [
            explorationFinding({
              evidence: ["art_0123456789abcdef0123"],
              attempts: [reproductionAttempt({ request_id: request, bundle_id: bundle })],
            }),
          ],
        }),
      ),
    );

    const finding = await screen.findByRole("region", { name: "優惠碼欄位接受負數" });
    expect(within(finding).getByText("art_012345…0123")).toHaveAttribute("title", "art_0123456789abcdef0123");
    const attempt = within(finding).getByRole("listitem", { name: `驗證要求 ${request}` });
    expect(within(attempt).getByText("vreq_e3b0c4…f4c8")).toHaveAttribute("title", request);
    expect(within(attempt).getByText("evb_7b5deb…7314")).toHaveAttribute("title", bundle);
    // 同一個疑點的多個「複製識別碼」以欄位名區分
    expect(within(finding).getByRole("button", { name: "複製識別碼（證據）" })).toBeInTheDocument();
    expect(within(attempt).getByRole("button", { name: "複製識別碼（驗證要求）" })).toBeInTheDocument();
    expect(within(attempt).getByRole("button", { name: "複製識別碼（Bundle）" })).toBeInTheDocument();
  });

  it("重現紀錄新的在前，顯示目的、驗證要求、Bundle、案例與對照判定和結論；沒有結論時是排隊中", async () => {
    renderPage(fakeClient());

    const suspected = await screen.findByRole("region", { name: "優惠碼欄位接受負數" });
    const attempts = within(suspected).getAllByRole("listitem", { name: /^驗證要求/ });
    expect(attempts.map((item) => item.getAttribute("aria-label"))).toEqual(["驗證要求 vreq_13", "驗證要求 vreq_12"]);
    expect(attempts[0]).toHaveTextContent("確認重現・排隊中");
    expect(attempts[0]).toHaveTextContent("Bundle 尚未產生");
    expect(attempts[0]).not.toHaveTextContent("案例判定");
    expect(attempts[1]).toHaveTextContent("確認重現・證據不足（不是產品缺陷）");
    expect(attempts[1]).toHaveTextContent("Bundle evb_12");
    expect(attempts[1]).toHaveTextContent("案例判定：環境阻礙（service_unreachable）");
    expect(attempts[1]).toHaveTextContent("對照判定：環境阻礙");

    const regressed = screen.getByRole("region", { name: "運費重複計算" });
    const history = within(within(regressed).getByRole("list", { name: "運費重複計算 的重現紀錄" })).getAllByRole("listitem", {
      name: /^驗證要求/,
    });
    expect(history.map((item) => item.querySelector(".exploration-attempt-outcome")?.textContent)).toEqual([
      "回歸・回歸失敗",
      "修復驗證・修復已驗證",
      "確認重現・已重現",
    ]);
    expect(history[2]).toHaveTextContent("案例判定：產品缺陷（assertion_failed）");
    expect(history[2]).toHaveTextContent("對照判定：通過");

    const fixed = screen.getByRole("region", { name: "稅額四捨五入錯誤" });
    expect(fixed).toHaveTextContent("回歸・回歸通過");
    expect(fixed).toHaveTextContent("修復驗證・仍然失敗");
    expect(screen.getByRole("region", { name: "大量商品時結帳變慢" })).toHaveTextContent("確認重現・無法重現");
  });

  it("未驗風險顯示原因，已排除顯示排除原因、理由與預期設計的裁定請求", async () => {
    renderPage(fakeClient());

    expect(await screen.findByRole("region", { name: "大量商品時結帳變慢" })).toHaveTextContent("未驗原因重現時沒有出現原症狀");
    const environment = screen.getByRole("region", { name: "測試站台連線逾時" });
    expect(environment).toHaveTextContent("排除原因環境故障");
    expect(environment).toHaveTextContent("排除理由測試站台當時正在重新部署");
    expect(environment).not.toHaveTextContent("裁定請求");
    const design = screen.getByRole("region", { name: "折扣碼不能疊加" });
    expect(design).toHaveTextContent("排除原因預期設計（裁定請求 dreq_1）");
    expect(design).toHaveTextContent("排除理由產品規則是一次只能用一個折扣碼");
  });

  it("seed data 與工具限制的排除原因各有標示", async () => {
    renderPage(
      fakeClient(
        explorationOverview({
          findings: [
            explorationFinding({ id: "xf_8", title: "庫存為零", status: "excluded", exclusion: { reason: "seed_data", rationale: "測試資料沒有庫存" } }),
            explorationFinding({ id: "xf_9", title: "拖曳失敗", status: "excluded", exclusion: { reason: "tool_limitation", rationale: "工具不支援拖曳" } }),
          ],
        }),
      ),
    );

    expect(await screen.findByRole("region", { name: "庫存為零" })).toHaveTextContent("排除原因seed data");
    expect(screen.getByRole("region", { name: "拖曳失敗" })).toHaveTextContent("排除原因工具限制");
  });

  it("沒有章程與疑點時顯示空狀態", async () => {
    renderPage(fakeClient({ workspace_id: "ws_1", charters: [], findings: [] }));

    expect(await screen.findByText("還沒有探索章程；Controller 為探索 Task 定下章程後會在這裡顯示。")).toBeInTheDocument();
    expect(group("已確認缺陷")).toHaveTextContent("沒有已確認缺陷。");
    expect(group("疑點（未經重現）")).toHaveTextContent("沒有待重現的疑點。");
    expect(group("未驗風險")).toHaveTextContent("沒有未驗風險。");
    expect(group("已修復・納入回歸")).toHaveTextContent("沒有已修復的缺陷。");
    expect(group("已排除")).toHaveTextContent("沒有已排除的疑點。");
  });

  it("讀取失敗時顯示錯誤，重新整理後顯示最新資料", async () => {
    const client = {
      explorationOverview: vi
        .fn<(workspaceId: string) => Promise<ExplorationOverview>>()
        .mockRejectedValueOnce(new Error("Runtime 沒有回應"))
        .mockResolvedValueOnce(explorationOverview()),
    };
    const user = userEvent.setup();
    render(<ExplorationPage client={client} workspace={workspace} timeZone="Asia/Taipei" />);

    expect(await screen.findByRole("alert")).toHaveTextContent("無法取得探索資料：Runtime 沒有回應");

    await user.click(screen.getByRole("button", { name: "重新整理" }));

    expect(await screen.findByRole("region", { name: "折扣後總額少算" })).toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(client.explorationOverview).toHaveBeenCalledTimes(2);
  });
});
