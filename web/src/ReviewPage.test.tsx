import { render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { CHECKED_AT, deviationView, failingReviewView, reviewList } from "./fixtures";
import type { ReviewList, Workspace } from "./runtimeClient";
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

describe("審查與設計偏離", () => {
  it("列出審查的被審 Task、reviewer、版本、規格版本、規範來源與整體結果", async () => {
    const client = fakeClient();
    renderPage(client);

    expect(await screen.findByRole("heading", { name: "審查與設計偏離：筆記" })).toBeInTheDocument();
    const review = await screen.findByRole("region", { name: PASSED });
    expect(client.reviews).toHaveBeenCalledWith("ws_1");
    expect(review).toHaveTextContent("被審 Task修正匯出錯誤");
    expect(review).toHaveTextContent("Reviewer審查匯出修正");
    expect(review).toHaveTextContent("a1b2c3d → e4f5a6b");
    expect(review).toHaveTextContent("規格第 2 版");
    expect(review).toHaveTextContent("規範來源AGENTS.md");
    expect(review).toHaveTextContent("審查於2023/11/15 06:14");
    expect(within(review).getByText("通過")).toBeInTheDocument();
  });

  it("規範與需求兩軸分開顯示各自的結果、摘要、檔案與 finding", async () => {
    renderPage(fakeClient(reviewList({ reviews: [failingReviewView({ stale_reason: null })] })));

    const review = await screen.findByRole("region", { name: FAILING });
    const standards = within(review).getByRole("region", { name: "規範（Standards）" });
    const spec = within(review).getByRole("region", { name: "需求（Spec）" });

    expect(within(standards).getByRole("heading")).toHaveTextContent("規範（Standards）・未通過");
    expect(standards).toHaveTextContent("錯誤處理不符合慣例");
    expect(within(standards).getByRole("list", { name: "規範（Standards）的檔案" })).toHaveTextContent("src/export.rs");
    const open = within(standards).getByRole("listitem", { name: "匯出函式吞掉錯誤" });
    expect(open).toHaveTextContent("匯出函式吞掉錯誤・未分類・src/export.rs:42");
    expect(open).toHaveTextContent("寫檔失敗時沒有回報");
    expect(open).not.toHaveTextContent("理由：");
    expect(open).not.toHaveTextContent("裁定者");

    expect(within(spec).getByRole("heading")).toHaveTextContent("需求（Spec）・通過");
    expect(within(spec).getByRole("list", { name: "需求（Spec）的檔案" })).toHaveTextContent("src/export.rstests/export.rs");
    const dismissed = within(spec).getByRole("listitem", { name: "CSV 標頭順序不同" });
    expect(dismissed).toHaveTextContent("CSV 標頭順序不同・已駁回");
    expect(dismissed).toHaveTextContent("理由：契約沒有規定標頭順序");
    expect(dismissed).toHaveTextContent("裁定者：使用者");
    expect(dismissed).not.toHaveTextContent("desktop");
  });

  it("只有一軸還有未解決的 finding，整份審查就不通過", async () => {
    renderPage(fakeClient(reviewList({ reviews: [failingReviewView({ stale_reason: null })] })));

    const review = await screen.findByRole("region", { name: FAILING });
    expect(within(review).getByText("未通過")).toBeInTheDocument();
    expect(within(review).queryByText("通過")).not.toBeInTheDocument();
    expect(review).not.toHaveTextContent("已過期");
  });

  it("finding 的分類狀態各有標示", async () => {
    const base = failingReviewView({ stale_reason: null });
    const findings = (["fix", "awaiting_decision"] as const).map((state, index) => ({
      ...base.standards.findings[0],
      id: `fnd_${state}`,
      title: `finding ${index}`,
      state,
    }));
    renderPage(fakeClient(reviewList({ reviews: [{ ...base, standards: { ...base.standards, findings } }] })));

    expect(await screen.findByRole("listitem", { name: "finding 0" })).toHaveTextContent("finding 0・修正");
    expect(screen.getByRole("listitem", { name: "finding 1" })).toHaveTextContent("finding 1・等待裁定");
  });

  it("已過期的審查顯示原因，不顯示為通過或未通過", async () => {
    renderPage(fakeClient());

    const review = await screen.findByRole("region", { name: FAILING });
    expect(within(review).getByText("已過期：已有對較新 candidate 的審查")).toBeInTheDocument();
    expect(within(review).queryByText("未通過")).not.toBeInTheDocument();
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
    expect(accepted).toHaveTextContent("接受者：使用者・設計更新為第 2 版");
    expect(accepted).toHaveTextContent("處理理由：串流匯出較穩定");

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
    expect(screen.getByRole("region", { name: "偏離乙" })).toHaveTextContent("處理理由：必須依照目前的設計");
  });

  it("沒有審查與設計偏離時各自說明", async () => {
    renderPage(fakeClient(reviewList({ reviews: [], deviations: [] })));

    expect(await screen.findByText(/還沒有審查/)).toBeInTheDocument();
    expect(screen.getByText(/沒有設計偏離/)).toBeInTheDocument();
  });

  it("讀不到審查時顯示 Runtime 的說明", async () => {
    const client = fakeClient();
    client.reviews.mockRejectedValueOnce(new Error("找不到 Workspace ws_1"));
    renderPage(client);

    expect(await screen.findByRole("alert")).toHaveTextContent("無法取得審查：找不到 Workspace ws_1");
  });
});
