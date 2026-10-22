import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { CHECKED_AT, stageAttempt, stageView, workflowList, workflowView } from "./fixtures";
import type { SkipStageRequest, WorkflowList, Workspace } from "./runtimeClient";
import { WorkflowPage } from "./WorkflowPage";

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

function fakeClient(list: WorkflowList = workflowList()) {
  return {
    workflows: vi.fn(async () => list),
    skipStage: vi.fn(async (_workspaceId: string, _workflowId: string, _request: SkipStageRequest) => workflowView()),
  };
}

const renderPage = (client: ReturnType<typeof fakeClient>) =>
  render(<WorkflowPage client={client} workspace={workspace} timeZone="Asia/Taipei" />);

const stage = (name: string) => screen.getByRole("listitem", { name });

/** 目前 Stage 是需要裁定的「修正確認」。 */
const atDecisionGate = workflowView({
  current_stage: "fix_review",
  stages: workflowView().stages.map((s) =>
    s.key === "fix_review" ? { ...s, state: "current" as const } : s.key === "minimization" ? { ...s, state: "passed" as const } : s,
  ),
});

describe("流程", () => {
  it("列出 Workflow 的 Playbook 與版本、新版提示、綁定的 Task、狀態與 Router 的理由", async () => {
    const client = fakeClient();
    renderPage(client);

    expect(await screen.findByRole("heading", { name: "流程：筆記" })).toBeInTheDocument();
    const workflow = await screen.findByRole("region", { name: "缺陷處理 第 1 版" });
    expect(client.workflows).toHaveBeenCalledWith("ws_1");
    expect(workflow).toHaveTextContent("已有第 2 版；此 Workflow 維持第 1 版");
    expect(workflow).toHaveTextContent("修正匯出錯誤");
    expect(workflow).toHaveTextContent("進行中");
    expect(workflow).toHaveTextContent("Controller 的判斷：使用者回報匯出失敗");
    expect(workflow).toHaveTextContent("建立於2023/11/15 06:13");
  });

  it("Playbook 沒有新版時不顯示新版提示；沒有 Task 時說明尚未綁定", async () => {
    renderPage(fakeClient(workflowList({ workflows: [workflowView({ latest_playbook_version: 1, task_id: null, task_title: null })] })));

    const workflow = await screen.findByRole("region", { name: "缺陷處理 第 1 版" });
    expect(workflow).not.toHaveTextContent("已有第");
    expect(workflow).toHaveTextContent("尚未綁定 Task");
  });

  it("已阻擋的 Workflow 顯示原因；已完成的 Workflow 沒有跳過操作", async () => {
    const blocked = workflowView({ id: "wf_1", status: "blocked", blocked_reason: "失敗次數用盡" });
    const completed = workflowView({
      id: "wf_2",
      playbook_title: "新功能",
      status: "completed",
      current_stage: null,
      stages: workflowView().stages.map((s) => ({ ...s, state: "passed" as const })),
    });
    renderPage(fakeClient(workflowList({ workflows: [blocked, completed] })));

    expect(await screen.findByRole("region", { name: "缺陷處理 第 1 版" })).toHaveTextContent("已阻擋：失敗次數用盡");
    const done = screen.getByRole("region", { name: "新功能 第 1 版" });
    expect(done).toHaveTextContent("已完成");
    expect(within(done).queryByRole("button", { name: "跳過此 Stage" })).not.toBeInTheDocument();
    expect(within(done).queryByText("此 Gate 不能跳過")).not.toBeInTheDocument();
  });

  it("每個 Stage 顯示狀態、Gate、方法、條件、預算、離開條件與失敗去向", async () => {
    renderPage(fakeClient());
    await screen.findByRole("region", { name: "缺陷處理 第 1 版" });

    const reproduction = stage("建立重現");
    expect(reproduction).toHaveTextContent("已通過");
    expect(reproduction).toHaveTextContent("方法：reproduction");
    expect(within(reproduction).getByRole("list", { name: "建立重現 的進入條件" })).toHaveTextContent("已有症狀描述");
    expect(within(reproduction).getByRole("list", { name: "建立重現 的預期產出" })).toHaveTextContent("能捕捉原症狀的重現");
    expect(within(reproduction).getByRole("list", { name: "建立重現 的允許的動作" })).toHaveTextContent("讀取程式碼撰寫測試");
    expect(reproduction).toHaveTextContent("預算：最多失敗 3 次");
    expect(within(reproduction).getByRole("list", { name: "建立重現 的離開條件" })).toHaveTextContent(
      "能捕捉原症狀的重現（需要：產出物）",
    );
    expect(reproduction).toHaveTextContent("失敗去向：重試");

    const minimization = stage("縮小範圍");
    expect(minimization).toHaveTextContent("目前");
    expect(minimization).not.toHaveTextContent("方法：");
    expect(minimization).toHaveTextContent("失敗去向：回到「建立重現」");

    const review = stage("修正確認");
    expect(review).toHaveTextContent("待進行");
    expect(review).toHaveTextContent("裁定 Gate");
    expect(review).toHaveTextContent("（需要：裁定）");
    expect(review).toHaveTextContent("失敗去向：阻擋");
    expect(within(review).queryByRole("list", { name: "修正確認 的允許的動作" })).not.toBeInTheDocument();
  });

  it("安全 Gate 另外標示", async () => {
    const safety = workflowView({ stages: [stageView({ gate: "safety", title: "部署前確認" })] });
    renderPage(fakeClient(workflowList({ workflows: [safety] })));

    expect(await screen.findByRole("listitem", { name: "部署前確認" })).toHaveTextContent("安全 Gate");
  });

  it("attempt 歷史依次數顯示結果、缺少的證據、證據查驗狀態、說明與記錄者", async () => {
    const desktop = stageAttempt({
      id: "att_3",
      attempt: 1,
      outcome: "skipped",
      evidence: [{ check: "approved", kind: "decision", decision_request_id: "dec_1", status: "pending" }],
      note: "需求已在 issue 中寫明",
      recorded_by: "desktop",
      profile_id: null,
    });
    const workflow = workflowView();
    workflow.stages[2] = { ...workflow.stages[2], state: "skipped", attempts: [desktop] };
    renderPage(fakeClient(workflowList({ workflows: [workflow] })));
    await screen.findByRole("region", { name: "缺陷處理 第 1 版" });

    const attempts = within(screen.getByRole("list", { name: "建立重現 的嘗試紀錄" })).getAllByRole("listitem", {
      name: /^第 \d 次$/,
    });
    expect(attempts).toHaveLength(2);
    expect(attempts[0]).toHaveTextContent("第 1 次・缺少證據・2023/11/15 06:13");
    expect(within(attempts[0]).getByRole("list", { name: "缺少的證據" })).toHaveTextContent(
      "reproduction：缺少證據：能捕捉原症狀的重現",
    );
    expect(attempts[0]).toHaveTextContent("還沒有重現");
    expect(attempts[1]).toHaveTextContent("第 2 次・通過・2023/11/15 06:14");
    expect(within(attempts[1]).getByRole("list", { name: "證據" })).toHaveTextContent("reproduction：產出物 art_1・有效");
    expect(attempts[1]).toHaveTextContent("記錄者：ctl_1・帳號 prof_1");

    const skipped = within(screen.getByRole("list", { name: "修正確認 的嘗試紀錄" })).getByRole("listitem", { name: "第 1 次" });
    expect(skipped).toHaveTextContent("第 1 次・已跳過");
    expect(skipped).toHaveTextContent("approved：裁定 dec_1・尚未完成");
    expect(skipped).toHaveTextContent("記錄者：使用者");
    expect(skipped).not.toHaveTextContent("帳號");
    expect(skipped).not.toHaveTextContent("desktop");
  });

  it("跳過目前 Stage 必須輸入原因；送出時帶目前 revision，成功後以回應更新畫面", async () => {
    const skippedView = workflowView({
      revision: 4,
      current_stage: "fix_review",
      stages: atDecisionGate.stages.map((s) => (s.key === "minimization" ? { ...s, state: "skipped" as const } : s)),
    });
    const client = fakeClient();
    client.skipStage.mockResolvedValueOnce(skippedView);
    const user = userEvent.setup();
    renderPage(client);

    await screen.findByRole("region", { name: "缺陷處理 第 1 版" });
    const current = stage("縮小範圍");
    const submit = within(current).getByRole("button", { name: "跳過此 Stage" });
    expect(submit).toBeDisabled();
    // 其他 Stage 沒有跳過操作
    expect(within(stage("建立重現")).queryByRole("button", { name: "跳過此 Stage" })).not.toBeInTheDocument();

    await user.type(within(current).getByLabelText("跳過原因"), "   ");
    expect(submit).toBeDisabled();

    await user.clear(within(current).getByLabelText("跳過原因"));
    await user.type(within(current).getByLabelText("跳過原因"), "已有最小重現");
    await user.click(submit);

    expect(client.skipStage).toHaveBeenCalledWith("ws_1", "wf_1", {
      stage: "minimization",
      expected_revision: 3,
      reason: "已有最小重現",
    });
    await waitFor(() => expect(stage("縮小範圍")).toHaveTextContent("已跳過"));
    expect(within(stage("縮小範圍")).queryByRole("button", { name: "跳過此 Stage" })).not.toBeInTheDocument();
    expect(within(stage("修正確認")).getByText("此 Gate 不能跳過")).toBeInTheDocument();
  });

  it("目前 Stage 是裁定或安全 Gate 時不提供跳過，改說明不能跳過", async () => {
    const safety = workflowView({
      id: "wf_2",
      playbook_title: "遷移",
      current_stage: "cutover",
      stages: [stageView({ key: "cutover", title: "切換", gate: "safety", state: "current" })],
    });
    renderPage(fakeClient(workflowList({ workflows: [atDecisionGate, safety] })));

    await screen.findByRole("region", { name: "缺陷處理 第 1 版" });
    expect(within(stage("修正確認")).getByText("此 Gate 不能跳過")).toBeInTheDocument();
    expect(within(stage("切換")).getByText("此 Gate 不能跳過")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "跳過此 Stage" })).not.toBeInTheDocument();
  });

  it("跳過被拒時顯示 Runtime 的說明，保留目前內容與輸入", async () => {
    const client = fakeClient();
    client.skipStage.mockRejectedValueOnce(Object.assign(new Error("只能跳過目前的 Stage"), { code: "stage_not_current" }));
    const user = userEvent.setup();
    renderPage(client);

    await user.type(await screen.findByLabelText("跳過原因"), "不需要");
    await user.click(screen.getByRole("button", { name: "跳過此 Stage" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("只能跳過目前的 Stage");
    expect(stage("縮小範圍")).toHaveTextContent("目前");
    expect(screen.getByLabelText("跳過原因")).toHaveValue("不需要");
    expect(client.workflows).toHaveBeenCalledTimes(1);
  });

  it("revision 過時（409 workflow_revision_mismatch）時重新載入，再送出時帶最新的 revision", async () => {
    const client = fakeClient();
    client.workflows.mockResolvedValueOnce(workflowList());
    client.workflows.mockResolvedValueOnce(workflowList({ workflows: [workflowView({ revision: 5 })] }));
    client.skipStage.mockRejectedValueOnce(
      Object.assign(new Error("Workflow 已被更新"), { code: "workflow_revision_mismatch" }),
    );
    const user = userEvent.setup();
    renderPage(client);

    await user.type(await screen.findByLabelText("跳過原因"), "不需要");
    await user.click(screen.getByRole("button", { name: "跳過此 Stage" }));

    expect(await screen.findByText(/Workflow 在檢視後已更新，已重新載入/)).toBeInTheDocument();
    expect(client.workflows).toHaveBeenCalledTimes(2);
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "跳過此 Stage" }));
    expect(client.skipStage).toHaveBeenLastCalledWith("ws_1", "wf_1", {
      stage: "minimization",
      expected_revision: 5,
      reason: "不需要",
    });
  });

  it("沒有 Workflow 時說明 Controller 會依情境選擇 Playbook", async () => {
    renderPage(fakeClient(workflowList({ workflows: [] })));

    expect(await screen.findByText(/Controller 開始工作時會依情境選擇 Playbook/)).toBeInTheDocument();
  });

  it("讀不到流程時顯示 Runtime 的說明", async () => {
    const client = fakeClient();
    client.workflows.mockRejectedValueOnce(new Error("找不到 Workspace：ws_1"));
    renderPage(client);

    expect(await screen.findByRole("alert")).toHaveTextContent("無法取得流程：找不到 Workspace：ws_1");
  });
});
