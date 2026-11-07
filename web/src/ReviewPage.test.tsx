import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { CHECKED_AT, deviationView, failingReviewView, reviewList, reviewView } from "./fixtures";
import type { DeviationStatus, ReviewList, Workspace } from "./runtimeClient";
import { ReviewPage } from "./ReviewPage";

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

function fakeClient(list: ReviewList = reviewList()) {
  return { reviews: vi.fn(async () => list) };
}

const renderPage = (client: ReturnType<typeof fakeClient>) =>
  render(<ReviewPage client={client} workspace={workspace} timeZone="Asia/Taipei" />);

const PASSED = "修正匯出錯誤（candidate e4f5a6b）";
const FAILING = "修正匯出錯誤（candidate 9c8d7e6）";

/** 「已過期的審查 N」的收合摘要（計數只放數字，不加括號）。 */
async function staleSummary(count = 1) {
  const summary = await screen.findByText("已過期的審查", { selector: "summary" });
  expect(summary).toHaveTextContent(new RegExp(`^已過期的審查 ${count}$`));
  return summary;
}

/** 已過期的審查收在預設收合的「已過期的審查」中：展開後才能操作。 */
async function openStale(count = 1) {
  await userEvent.setup().click(await staleSummary(count));
}

describe("審查", () => {
  it("列出審查的被審 Task、reviewer、版本、規格版本、規範來源與整體結果", async () => {
    const client = fakeClient();
    renderPage(client);

    expect(await screen.findByRole("heading", { level: 2, name: "審查" })).toBeInTheDocument();
    const review = await screen.findByRole("region", { name: PASSED });
    expect(client.reviews).toHaveBeenCalledWith("ws_1");
    expect(review).toHaveTextContent("被審 Task修正匯出錯誤");
    expect(review).toHaveTextContent("Reviewer審查匯出修正");
    expect(review).toHaveTextContent("a1b2c3d → e4f5a6b");
    expect(review).toHaveTextContent("規格第 2 版");
    expect(review).toHaveTextContent("規範來源AGENTS.md");
    expect(review).toHaveTextContent("審查於2023/11/15 06:14");
    expect(within(review).getByText("通過", { selector: ".review-overall" })).toHaveClass("badge", "badge-success");
  });

  it("規範與需求兩軸分開顯示各自的結果、摘要、檔案與 finding", async () => {
    renderPage(fakeClient(reviewList({ reviews: [failingReviewView({ stale_reason: null })] })));

    const review = await screen.findByRole("region", { name: FAILING });
    const standards = within(review).getByRole("region", { name: "規範（Standards）" });
    const spec = within(review).getByRole("region", { name: "需求（Spec）" });

    // 每一軸的結論只用標題旁的徽章表達：審查卡已有色條，軸不再各加色條（框中框）
    expect(standards.className).not.toMatch(/stripe-/);
    expect(spec.className).not.toMatch(/stripe-/);
    const standardsHeading = within(standards).getByRole("heading");
    expect(standardsHeading).toHaveTextContent("規範（Standards）");
    expect(within(standardsHeading).getByText("未通過")).toHaveClass("badge", "badge-danger");
    expect(standards).toHaveTextContent("錯誤處理不符合慣例");
    expect(within(standards).getByRole("list", { name: "規範（Standards）的檔案" })).toHaveTextContent("src/export.rs");
    const open = within(standards).getByRole("listitem", { name: "匯出函式吞掉錯誤" });
    expect(open).toHaveTextContent("匯出函式吞掉錯誤");
    expect(within(open).getByText("未分類")).toHaveClass("badge");
    expect(within(open).getByText("src/export.rs:42")).toHaveClass("mono");
    expect(open).toHaveTextContent("寫檔失敗時沒有回報");
    expect(open).not.toHaveTextContent("理由：");
    expect(open).not.toHaveTextContent("裁定者");

    const specHeading = within(spec).getByRole("heading");
    expect(specHeading).toHaveTextContent("需求（Spec）");
    expect(within(specHeading).getByText("通過")).toHaveClass("badge", "badge-success");
    expect(within(spec).getByRole("list", { name: "需求（Spec）的檔案" })).toHaveTextContent("src/export.rstests/export.rs");
    const dismissed = within(spec).getByRole("listitem", { name: "CSV 標頭順序不同" });
    expect(dismissed).toHaveTextContent("CSV 標頭順序不同");
    expect(within(dismissed).getByText("已駁回")).toHaveClass("badge");
    expect(dismissed).toHaveTextContent("理由：契約沒有規定標頭順序");
    expect(dismissed).toHaveTextContent("裁定者：使用者");
    expect(dismissed).not.toHaveTextContent("desktop");
  });

  it("只有一軸還有未解決的 finding，整份審查就不通過", async () => {
    renderPage(fakeClient(reviewList({ reviews: [failingReviewView({ stale_reason: null })] })));

    const review = await screen.findByRole("region", { name: FAILING });
    expect(within(review).getByText("未通過", { selector: ".review-overall" })).toBeInTheDocument();
    expect(within(review).queryByText("通過", { selector: ".review-overall" })).not.toBeInTheDocument();
    expect(review).not.toHaveTextContent("已過期");
  });

  it("finding 的分類狀態各有標示，等待裁定的排在最前", async () => {
    const base = failingReviewView({ stale_reason: null });
    const findings = (["fix", "awaiting_decision"] as const).map((state, index) => ({
      ...base.standards.findings[0],
      id: `fnd_${state}`,
      title: `finding ${index}`,
      state,
    }));
    renderPage(fakeClient(reviewList({ reviews: [{ ...base, standards: { ...base.standards, findings } }] })));

    const fix = await screen.findByRole("listitem", { name: "finding 0" });
    expect(within(fix).getByText("修正")).toHaveClass("badge");
    const awaiting = screen.getByRole("listitem", { name: "finding 1" });
    expect(within(awaiting).getByText("等待裁定")).toHaveClass("badge", "badge-warning");
    const list = screen.getByRole("list", { name: "規範（Standards）的 finding" });
    expect(within(list).getAllByRole("listitem").map((item) => item.getAttribute("aria-label"))).toEqual(["finding 1", "finding 0"]);
  });

  it("已過期的審查收在預設收合的「已過期的審查」中，顯示原因，不顯示為通過或未通過", async () => {
    renderPage(fakeClient());

    const group = (await staleSummary(1)).closest("details")!;
    expect(group).not.toHaveAttribute("open");
    expect(group).not.toContainElement(screen.getByRole("region", { name: PASSED }));
    await openStale();

    const review = screen.getByRole("region", { name: FAILING });
    expect(group).toContainElement(review);
    // 狀態詞是徽章，原因是一般文字（不整段上色）
    expect(within(review).getByText("已過期")).toHaveClass("badge");
    expect(within(review).getByText("已有對較新 candidate 的審查")).not.toHaveClass("badge");
    expect(within(review).queryByText("未通過", { selector: ".review-overall" })).not.toBeInTheDocument();
    expect(within(review).queryByText("通過", { selector: ".review-overall" })).not.toBeInTheDocument();
  });

  it("過期原因只完整顯示第一句，其餘收進技術細節", async () => {
    const stale = String.raw`Runtime 拒絕連線。無法寫入 C:\data\baton.sqlite3-wal，SQLITE_IOERR_WRITE。`;
    renderPage(fakeClient(reviewList({ reviews: [failingReviewView({ stale_reason: stale })] })));
    await openStale();

    const review = await screen.findByRole("region", { name: FAILING });
    expect(within(review).getByText("Runtime 拒絕連線。")).toBeInTheDocument();
    const details = within(review).getByText("技術細節").closest("details");
    expect(details).toHaveTextContent(String.raw`無法寫入 C:\data\baton.sqlite3-wal，SQLITE_IOERR_WRITE。`);
  });

  it("版本雜湊與規範來源路徑以等寬縮短顯示，完整值在 title 並可複製", async () => {
    const base = "996fb92427ae41e4649b934ca495991b7852b855";
    const candidate = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4";
    const source = String.raw`C:\Users\Administrator\AppData\Local\Baton\fixtures\report.md`;
    renderPage(
      fakeClient(
        reviewList({ reviews: [reviewView({ base_revision: base, candidate_revision: candidate, standards_source: source })] }),
      ),
    );

    const review = await screen.findByRole("region", { name: "修正匯出錯誤（candidate e3b0c44298fc…）" });
    expect(within(review).getByText("996fb92427ae…", { selector: "dd code" })).toHaveAttribute("title", base);
    expect(within(review).getByText("e3b0c44298fc…", { selector: "dd code" })).toHaveAttribute("title", candidate);
    // 兩個「複製雜湊」以欄位名區分
    expect(within(review).getByRole("button", { name: "複製雜湊（基準版本）" })).toBeInTheDocument();
    expect(within(review).getByRole("button", { name: "複製雜湊（候選版本）" })).toBeInTheDocument();
    expect(within(review).getByText(String.raw`C:\…\fixtures\report.md`)).toHaveAttribute("title", source);
    expect(within(review).getByRole("button", { name: "複製路徑（規範來源）" })).toBeInTheDocument();
  });

  it("沿用先前審查的軸標示沿用來源", async () => {
    renderPage(fakeClient());

    const review = await screen.findByRole("region", { name: PASSED });
    expect(within(review).getByRole("region", { name: "需求（Spec）" })).toHaveTextContent("沿用自 rev_1");
    expect(within(review).getByRole("region", { name: "規範（Standards）" })).not.toHaveTextContent("沿用自");
  });

  it("已接受的設計偏離顯示接受者與設計的新版本；等待裁定的不顯示", async () => {
    renderPage(fakeClient());

    const accepted = await screen.findByRole("region", { name: "改用串流匯出" });
    expect(accepted).toHaveTextContent("已接受");
    expect(accepted).toHaveTextContent("Task修正匯出錯誤");
    expect(accepted).toHaveTextContent("匯出設計 第 1 版");
    expect(accepted).toHaveTextContent("原因大檔案會耗盡記憶體");
    expect(within(accepted).getByRole("list", { name: "改用串流匯出 受影響的契約" })).toHaveTextContent(
      "ExportService::export 的回傳型別",
    );
    expect(accepted).toHaveTextContent("接受者使用者");
    expect(accepted).toHaveTextContent("設計更新為第 2 版");
    expect(accepted).toHaveTextContent("處理理由串流匯出較穩定");

    const awaiting = screen.getByRole("region", { name: "匯出檔名加上時間戳" });
    expect(awaiting).toHaveTextContent("等待裁定");
    expect(awaiting).not.toHaveTextContent("接受者");
    expect(awaiting).not.toHaveTextContent("設計更新為");
    expect(within(awaiting).queryByRole("list")).not.toBeInTheDocument();
  });

  it("待處理與已拒絕的設計偏離各有標示", async () => {
    const deviations = [
      deviationView({ id: "dev_3", deviation: "偏離甲", status: "proposed", resolution_reason: null, accepted_by: null, accepted_design_version: null }),
      deviationView({ id: "dev_4", deviation: "偏離乙", status: "rejected", resolution_reason: "必須依照目前的設計", accepted_by: null, accepted_design_version: null }),
    ];
    renderPage(fakeClient(reviewList({ deviations })));

    expect(await screen.findByRole("region", { name: "偏離甲" })).toHaveTextContent("待處理");
    expect(screen.getByRole("region", { name: "偏離甲" })).not.toHaveTextContent("處理理由");
    expect(screen.getByRole("region", { name: "偏離乙" })).toHaveTextContent("已拒絕");
    expect(screen.getByRole("region", { name: "偏離乙" })).toHaveTextContent("處理理由必須依照目前的設計");
  });

  it("設計偏離待處理的排在前面：等待裁定、待處理、已接受、已拒絕", async () => {
    const of = (id: string, deviation: string, status: DeviationStatus) =>
      deviationView({ id, deviation, status, resolution_reason: null, accepted_by: null, accepted_design_version: null });
    const deviations = [of("dev_a", "甲", "accepted"), of("dev_r", "乙", "rejected"), of("dev_p", "丙", "proposed"), of("dev_w", "丁", "awaiting_decision")];
    renderPage(fakeClient(reviewList({ deviations })));

    const section = await screen.findByRole("region", { name: "設計偏離" });
    expect(within(section).getAllByRole("heading", { level: 4 }).map((heading) => heading.textContent)).toEqual(["丁", "丙", "甲", "乙"]);
  });

  it("沒有審查與設計偏離時各自說明", async () => {
    renderPage(fakeClient(reviewList({ reviews: [], deviations: [] })));

    expect(await screen.findByText(/還沒有審查/)).toBeInTheDocument();
    expect(screen.getByText(/沒有設計偏離/)).toBeInTheDocument();
    // 0 筆時計數一律顯示 0
    expect(within(screen.getByRole("region", { name: "雙軸審查" })).getByText("0", { selector: ".count" })).toBeInTheDocument();
    expect(within(screen.getByRole("region", { name: "設計偏離" })).getByText("0", { selector: ".count" })).toBeInTheDocument();
  });

  it("讀不到審查時顯示 Runtime 的說明", async () => {
    const client = fakeClient();
    client.reviews.mockRejectedValueOnce(new Error("找不到 Workspace ws_1"));
    renderPage(client);

    expect(await screen.findByRole("alert")).toHaveTextContent("無法取得審查：找不到 Workspace ws_1");
  });
});
