import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import {
  CHECKED_AT,
  evidenceBundle,
  verificationOverview,
  verificationProfileVersion,
  verificationRequest,
  verificationStepRecord,
  webEvidenceBundle,
  webVerificationProfileDefinition,
} from "./fixtures";
import type {
  EvidenceArtifactContent,
  PublishProfileResponse,
  RunVerificationRequest,
  VerificationOverview,
  VerificationVerdict,
  Workspace,
} from "./runtimeClient";
import { VERDICT_LABELS, VerificationPage } from "./VerificationPage";

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

function fakeClient(overview: VerificationOverview = verificationOverview()) {
  return {
    verificationOverview: vi.fn(async (_workspaceId: string) => overview),
    publishVerificationProfile: vi.fn(
      async (_workspaceId: string, _definition: unknown): Promise<PublishProfileResponse> => ({
        profile: verificationProfileVersion({ id: "vprof_3", version: 3 }),
        created: true,
      }),
    ),
    runVerification: vi.fn(async (_workspaceId: string, _request: RunVerificationRequest) => verificationRequest()),
    evidenceBundle: vi.fn(async (_workspaceId: string, _bundleId: string) => evidenceBundle()),
    evidenceArtifact: vi.fn(
      async (_workspaceId: string, bundleId: string, index: number): Promise<EvidenceArtifactContent> => ({
        bundle_id: bundleId,
        index,
        kind: "screenshot",
        name: "open_orders.png",
        content_type: "image/png",
        size_bytes: 4,
        content_base64: "iVBORw==",
      }),
    ),
  };
}

const renderPage = (client: ReturnType<typeof fakeClient>) =>
  render(<VerificationPage client={client} workspace={workspace} timeZone="Asia/Taipei" />);

const region = (name: string) => screen.getByRole("region", { name });

describe("驗證", () => {
  it("顯示 Profile 版本、發布時間、已被證明可用、Feature Map 的旅程與未覆蓋範圍", async () => {
    const client = fakeClient();
    renderPage(client);

    expect(await screen.findByRole("heading", { name: "驗證：筆記" })).toBeInTheDocument();
    const profile = await screen.findByRole("region", { name: "Verification Profile" });
    expect(client.verificationOverview).toHaveBeenCalledWith("ws_1");
    expect(profile).toHaveTextContent("第 2 版");
    expect(profile).toHaveTextContent("2023/11/15 06:13");
    expect(profile).toHaveTextContent("由 Evidence Bundle vbun_1 證明");
    expect(profile).not.toHaveTextContent("還沒有實際跑過代表旅程");

    const features = within(profile).getByRole("list", { name: "Feature Map" });
    expect(within(features).getByRole("listitem", { name: "命令列" })).toHaveTextContent("驗收準則：可以顯示版本旅程：冒煙測試");
    expect(within(features).getByRole("listitem", { name: "匯出" })).toHaveTextContent("旅程：匯出報表");
    expect(within(features).getByRole("listitem", { name: "匯入" })).toHaveTextContent("沒有旅程（未覆蓋）");

    const uncovered = within(profile).getByRole("list", { name: "未覆蓋範圍" });
    expect(uncovered).toHaveTextContent("匯入：Feature Map 中沒有驗證這個功能的旅程");
    expect(uncovered).toHaveTextContent("圖形介面：CLI／API Adapter 不涵蓋");
  });

  it("未證明可用的版本明確標示不能當作驗證證據，要求執行時說明代表旅程一定會執行", async () => {
    renderPage(fakeClient(verificationOverview({ proven_by: null, bundles: [] })));

    const profile = await screen.findByRole("region", { name: "Verification Profile" });
    expect(profile).toHaveTextContent("這個版本還沒有實際跑過代表旅程，不能當作驗證證據");
    expect(region("要求執行驗證")).toHaveTextContent("代表旅程一定會一起執行");
    expect(region("最近的 Evidence Bundle")).toHaveTextContent("還沒有 Evidence Bundle");
  });

  it("還沒有 Profile 時說明要先發布，預填範例且不能要求執行", async () => {
    renderPage(fakeClient(verificationOverview({ profile: null, proven_by: null, uncovered: [], bundles: [] })));

    expect(await screen.findByRole("region", { name: "Verification Profile" })).toHaveTextContent(
      "還沒有 Verification Profile",
    );
    expect(String((screen.getByLabelText("Profile 內容（JSON）") as HTMLTextAreaElement).value)).toContain(
      '"representative": true',
    );
    expect(region("要求執行驗證")).toHaveTextContent("發布 Profile 後才能要求執行驗證");
    expect(screen.queryByRole("button", { name: "要求執行" })).not.toBeInTheDocument();
  });

  it("發布：預填目前的內容，送出解析後的 JSON，顯示新版本並重新載入", async () => {
    const client = fakeClient();
    const user = userEvent.setup();
    renderPage(client);

    const textarea = (await screen.findByLabelText("Profile 內容（JSON）")) as HTMLTextAreaElement;
    expect(JSON.parse(textarea.value)).toEqual(verificationProfileVersion().definition);

    await user.click(screen.getByRole("button", { name: "發布新版本" }));

    expect(client.publishVerificationProfile).toHaveBeenCalledWith("ws_1", verificationProfileVersion().definition);
    expect(await within(region("發布新版本")).findByRole("status")).toHaveTextContent(
      "已發布第 3 版；實際跑過並通過代表旅程後才算可用",
    );
    await waitFor(() => expect(client.verificationOverview).toHaveBeenCalledTimes(2));
  });

  it("發布：內容相同時說明沒有新增版本", async () => {
    const client = fakeClient();
    client.publishVerificationProfile.mockResolvedValueOnce({ profile: verificationProfileVersion(), created: false });
    const user = userEvent.setup();
    renderPage(client);

    await user.click(await screen.findByRole("button", { name: "發布新版本" }));

    expect(await within(region("發布新版本")).findByRole("status")).toHaveTextContent("內容與第 2 版相同，沒有新增版本");
  });

  it("發布：JSON 格式錯誤時不送出；Runtime 拒絕時顯示錯誤代碼與訊息", async () => {
    const client = fakeClient();
    client.publishVerificationProfile.mockRejectedValueOnce(
      Object.assign(new Error("Verification Profile 不正確：至少要有一條代表旅程"), { code: "verification_profile_invalid" }),
    );
    const user = userEvent.setup();
    renderPage(client);

    const textarea = await screen.findByLabelText("Profile 內容（JSON）");
    await user.clear(textarea);
    await user.type(textarea, "{{不是 JSON");
    await user.click(screen.getByRole("button", { name: "發布新版本" }));
    expect(within(region("發布新版本")).getByRole("alert")).toHaveTextContent("JSON 格式不正確");
    expect(client.publishVerificationProfile).not.toHaveBeenCalled();

    await user.clear(textarea);
    await user.type(textarea, '{{"interface": "cli_api"}');
    await user.click(screen.getByRole("button", { name: "發布新版本" }));
    expect(await within(region("發布新版本")).findByRole("alert")).toHaveTextContent(
      "發布失敗：Verification Profile 不正確：至少要有一條代表旅程（verification_profile_invalid）",
    );
    expect(client.publishVerificationProfile).toHaveBeenCalledWith("ws_1", { interface: "cli_api" });
  });

  it("要求執行：送出 Task 識別與勾選的旅程，顯示已排入", async () => {
    const client = fakeClient();
    const user = userEvent.setup();
    renderPage(client);

    const run = await screen.findByRole("region", { name: "要求執行驗證" });
    await user.type(within(run).getByLabelText("Task 識別（選填）"), " task_1 ");
    await user.click(within(run).getByRole("checkbox", { name: "匯出報表" }));
    expect(within(run).getByRole("checkbox", { name: "冒煙測試（代表旅程）" })).not.toBeChecked();
    await user.click(within(run).getByRole("button", { name: "要求執行" }));

    expect(client.runVerification).toHaveBeenCalledWith("ws_1", { task_id: "task_1", journeys: ["export_flow"] });
    expect(await within(run).findByRole("status")).toHaveTextContent(
      "已排入：要求 vreq_2（Profile 第 2 版，旅程：冒煙測試、匯出報表）",
    );
    await waitFor(() => expect(client.verificationOverview).toHaveBeenCalledTimes(2));
  });

  it("要求執行：不填 Task、不勾旅程時只送空的旅程清單；被拒時顯示錯誤代碼", async () => {
    const client = fakeClient();
    client.runVerification.mockRejectedValueOnce(
      Object.assign(new Error("找不到 Task task_9"), { code: "task_not_found" }),
    );
    const user = userEvent.setup();
    renderPage(client);

    await user.click(await screen.findByRole("button", { name: "要求執行" }));

    expect(client.runVerification).toHaveBeenCalledWith("ws_1", { journeys: [] });
    expect(await within(region("要求執行驗證")).findByRole("alert")).toHaveTextContent(
      "無法要求執行：找不到 Task task_9（task_not_found）",
    );
  });

  it("Evidence Bundle 顯示判定、綁定的 build、revision、需求與 Profile 版本、覆蓋範圍與清理結果", async () => {
    renderPage(fakeClient());

    const bundle = await screen.findByRole("region", { name: "Evidence Bundle vbun_1" });
    expect(bundle).toHaveTextContent("判定：通過 all_journeys_passed");
    expect(bundle).toHaveTextContent("tool 1.2.3");
    expect(bundle).toHaveTextContent("abc1234");
    expect(bundle).toHaveTextContent("Task task_1 契約第 3 版");
    expect(bundle).toHaveTextContent("Profile 版本第 2 版");
    expect(within(bundle).getByRole("list", { name: "旅程結果" })).toHaveTextContent("冒煙測試（代表旅程）：通過 all_steps_passed");
    expect(bundle).toHaveTextContent("覆蓋的功能：命令列");
    expect(within(bundle).getByRole("list", { name: "這次未覆蓋" })).toHaveTextContent("匯出：這次驗證沒有執行對應的旅程");
    expect(bundle).toHaveTextContent("清理：已移除本次驗證的狀態目錄；證據已保留");
  });

  it("沒有 Task、取不到 build 與 revision、清理失敗時如實顯示", async () => {
    const bundle = evidenceBundle({
      task_id: null,
      requirement_version: null,
      build: null,
      candidate_revision: null,
      cleanup: { state_dir: "C:\\s", state_dir_removed: false, service_stopped: false, problems: ["目錄被占用"] },
    });
    renderPage(fakeClient(verificationOverview({ bundles: [bundle] })));

    const card = await screen.findByRole("region", { name: "Evidence Bundle vbun_1" });
    expect(card).toHaveTextContent("沒有綁定 Task");
    expect(card).toHaveTextContent("build無法取得");
    expect(card).toHaveTextContent("無（不是 Git repo）");
    expect(card).toHaveTextContent("本次驗證的狀態目錄沒有移除；服務沒有停止；證據已保留");
    expect(within(card).getByRole("list", { name: "清理問題" })).toHaveTextContent("目錄被占用");
  });

  it("七種判定各有中文標籤，只有通過使用通過的樣式", async () => {
    const verdicts = Object.keys(VERDICT_LABELS) as VerificationVerdict[];
    const bundles = verdicts.map((verdict, index) => evidenceBundle({ id: `vbun_${index}`, verdict, reason_code: `r_${verdict}` }));
    renderPage(fakeClient(verificationOverview({ bundles })));

    await screen.findByRole("region", { name: "Evidence Bundle vbun_0" });
    expect(new Set(Object.values(VERDICT_LABELS)).size).toBe(7);
    verdicts.forEach((verdict, index) => {
      const card = region(`Evidence Bundle vbun_${index}`);
      const badge = within(card).getByText(VERDICT_LABELS[verdict], { selector: ".verification-bundle-verdict .verdict" });
      expect(badge).toHaveClass(`verdict-${verdict}`);
      expect(card).toHaveTextContent(`r_${verdict}`);
    });
    expect(document.querySelectorAll(".verification-bundle-verdict .verdict-passed")).toHaveLength(1);
  });

  it("展開步驟可看到 phase、判定、reason_code、engine_code、斷言結果與輸出", async () => {
    const steps = [
      verificationStepRecord({ phase: "doctor", journey: undefined, key: "tool", title: "", operation: "執行「tool --help」" }),
      verificationStepRecord({
        title: "匯出",
        journey: "export_flow",
        operation: "POST http://127.0.0.1:8080/export",
        verdict: "product_failure",
        reason_code: "assertion_failed",
        engine_code: "http:500",
        assertions: [
          { assertion: { kind: "status", equals: 200 }, passed: false, actual: "500" },
          { assertion: { kind: "json_equals", pointer: "/ok", value: true }, passed: false, actual: "false" },
        ],
        output: "internal error",
        attempts: 2,
      }),
    ];
    const user = userEvent.setup();
    renderPage(fakeClient(verificationOverview({ bundles: [evidenceBundle({ steps })] })));

    const card = await screen.findByRole("region", { name: "Evidence Bundle vbun_1" });
    await user.click(within(card).getByText("步驟（2）"));

    const doctor = within(card).getByRole("listitem", { name: "環境檢查：tool" });
    expect(doctor).toHaveTextContent("環境檢查・tool：通過 assertions_passed 引擎代碼 exit:0");
    expect(doctor).toHaveTextContent("結束碼為 0：符合（實際：0）");

    const failed = within(card).getByRole("listitem", { name: "旅程：匯出" });
    expect(failed).toHaveTextContent("旅程（匯出報表）・匯出：產品缺陷 assertion_failed 引擎代碼 http:500・嘗試 2 次");
    expect(failed).toHaveTextContent("POST http://127.0.0.1:8080/export");
    const assertions = within(failed).getByRole("list", { name: "斷言結果" });
    expect(assertions).toHaveTextContent("HTTP 狀態碼為 200：不符合（實際：500）");
    expect(assertions).toHaveTextContent("JSON /ok 等於 true：不符合（實際：false）");
    expect(failed).toHaveTextContent("internal error");
  });

  const webOverview = () =>
    verificationOverview({
      profile: verificationProfileVersion({ definition: webVerificationProfileDefinition() }),
      uncovered: [],
      bundles: [webEvidenceBundle()],
    });

  it("web Profile 列出秘密參照（只有 key 與參照）、persona 與登入身分；CLI／API Profile 不顯示", async () => {
    const { unmount } = renderPage(fakeClient(webOverview()));

    const profile = await screen.findByRole("region", { name: "Verification Profile" });
    expect(within(profile).getByRole("list", { name: "秘密參照" })).toHaveTextContent(
      "clerk_password：env:SHOP_CLERK_PASSWORD",
    );
    expect(profile).toHaveTextContent("秘密的值不會顯示");
    const personas = within(profile).getByRole("list", { name: "Persona" });
    expect(personas).toHaveTextContent("店家管理員（admin）：管理商品與訂單");
    expect(personas).toHaveTextContent("訪客（shopper）：沒有帳號的顧客");
    expect(within(profile).getByRole("list", { name: "登入身分" })).toHaveTextContent(
      "櫃台人員（clerk）：登入 2 個步驟，以「確認登入身分」核對身分",
    );
    unmount();

    renderPage(fakeClient());
    const cli = await screen.findByRole("region", { name: "Verification Profile" });
    expect(within(cli).queryByRole("list", { name: "秘密參照" })).not.toBeInTheDocument();
    expect(cli).not.toHaveTextContent("Persona");
  });

  it("web 旅程結果分開顯示 persona 與登入身分，沒有登入身分時顯示未登入；限流說明不是產品缺陷", async () => {
    renderPage(fakeClient(webOverview()));

    const card = await screen.findByRole("region", { name: "Evidence Bundle vbun_web" });
    expect(card).toHaveTextContent("判定：證據不足 judge_rate_limited（模型限流，無法判定，不是產品缺陷）");
    const journeys = within(card).getByRole("list", { name: "旅程結果" });
    const orders = within(journeys).getByRole("listitem", { name: "管理訂單" });
    expect(orders).toHaveTextContent("管理訂單（代表旅程）：證據不足 judge_rate_limited（模型限流，無法判定，不是產品缺陷）");
    expect(within(orders).getByText("persona：店家管理員（admin）")).toBeInTheDocument();
    expect(within(orders).getByText("登入身分：櫃台人員（clerk）")).toBeInTheDocument();
    const browse = within(journeys).getByRole("listitem", { name: "瀏覽商品" });
    expect(within(browse).getByText("persona：訪客（shopper）")).toBeInTheDocument();
    expect(within(browse).getByText("登入身分：未登入")).toBeInTheDocument();
  });

  it("步驟顯示登入與身分核對階段；語意判斷顯示預期與理由，限流時是無法判定而不是不符合", async () => {
    const user = userEvent.setup();
    renderPage(fakeClient(webOverview()));

    const card = await screen.findByRole("region", { name: "Evidence Bundle vbun_web" });
    await user.click(within(card).getByText("步驟（4）"));

    expect(within(card).getByRole("listitem", { name: "登入：輸入密碼" })).toHaveTextContent(
      "登入（管理訂單）・輸入密碼：通過 assertions_passed",
    );
    expect(within(card).getByRole("listitem", { name: "身分核對：確認登入身分" })).toHaveTextContent(
      "身分核對（管理訂單）・確認登入身分：通過",
    );

    const limited = within(card).getByRole("listitem", { name: "旅程：開啟訂單頁" });
    expect(limited).toHaveTextContent(
      "旅程（管理訂單）・開啟訂單頁：證據不足 judge_rate_limited（模型限流，無法判定，不是產品缺陷） 引擎代碼 model:rate_limited",
    );
    const limitedAssertions = within(limited).getByRole("list", { name: "斷言結果" });
    expect(limitedAssertions).toHaveTextContent("網址包含「/orders」：符合（實際：http://127.0.0.1:3000/orders）");
    expect(limitedAssertions).toHaveTextContent("語意判斷（預期：畫面列出今天的訂單）：無法判定（理由：模型限流，無法判定）");
    expect(limitedAssertions).not.toHaveTextContent("不符合");

    const browse = within(within(card).getByRole("listitem", { name: "旅程：商品列表" })).getByRole("list", {
      name: "斷言結果",
    });
    expect(browse).toHaveTextContent("「#products」可見：符合（實際：可見）");
    expect(browse).toHaveTextContent("語意判斷（預期：畫面列出商品）：符合（理由：畫面上有三項商品與價格）");
  });

  it("語意判斷做出判定但不符合時顯示不符合與理由", async () => {
    const steps = [
      verificationStepRecord({
        key: "open_orders",
        title: "開啟訂單頁",
        verdict: "product_failure",
        reason_code: "assertion_failed",
        assertions: [
          { assertion: { kind: "judge", expectation: "畫面列出今天的訂單" }, passed: false, actual: "畫面是空白的錯誤頁" },
        ],
      }),
    ];
    const user = userEvent.setup();
    renderPage(fakeClient(verificationOverview({ bundles: [webEvidenceBundle({ steps })] })));

    const card = await screen.findByRole("region", { name: "Evidence Bundle vbun_web" });
    await user.click(within(card).getByText("步驟（1）"));
    expect(within(card).getByRole("list", { name: "斷言結果" })).toHaveTextContent(
      "語意判斷（預期：畫面列出今天的訂單）：不符合（理由：畫面是空白的錯誤頁）",
    );
  });

  it("證據檔案：列出種類、旅程／步驟與大小；沒有保存的顯示原因且不能開啟；截圖可開啟、trace／錄影可下載", async () => {
    const client = fakeClient(webOverview());
    client.evidenceArtifact.mockResolvedValueOnce({
      bundle_id: "vbun_web",
      index: 2,
      kind: "video",
      name: "browse.webm",
      content_type: "video/webm",
      size_bytes: 4,
      content_base64: "GkXf",
    });
    const user = userEvent.setup();
    renderPage(client);

    const card = await screen.findByRole("region", { name: "Evidence Bundle vbun_web" });
    const files = within(card).getByRole("list", { name: "證據檔案" });

    const trace = within(files).getByRole("listitem", { name: "trace：orders-trace.zip" });
    expect(trace).toHaveTextContent("trace・orders-trace.zip・旅程：管理訂單");
    expect(trace).toHaveTextContent("沒有保存：這個 Session 輸入過秘密，無法確認 trace／錄影已遮蔽");
    expect(within(trace).queryByRole("button")).not.toBeInTheDocument();
    expect(within(trace).queryByRole("link")).not.toBeInTheDocument();

    const video = within(files).getByRole("listitem", { name: "錄影：browse.webm" });
    await user.click(within(video).getByRole("button", { name: "準備下載" }));
    expect(client.evidenceArtifact).toHaveBeenLastCalledWith("ws_1", "vbun_web", 2);
    const link = await within(video).findByRole("link", { name: "下載 browse.webm" });
    expect(link).toHaveAttribute("href", "data:video/webm;base64,GkXf");
    expect(link).toHaveAttribute("download", "browse.webm");

    const shot = within(files).getByRole("listitem", { name: "截圖：open_orders.png" });
    expect(shot).toHaveTextContent("截圖・open_orders.png・旅程：管理訂單／步驟：開啟訂單頁・2048 位元組");
    expect(within(shot).queryByRole("img")).not.toBeInTheDocument();
    await user.click(within(shot).getByRole("button", { name: "開啟截圖" }));
    expect(client.evidenceArtifact).toHaveBeenLastCalledWith("ws_1", "vbun_web", 0);
    expect(await within(shot).findByRole("img", { name: "截圖：open_orders.png" })).toHaveAttribute(
      "src",
      "data:image/png;base64,iVBORw==",
    );
    expect(client.evidenceArtifact).toHaveBeenCalledTimes(2);
  });

  it("證據檔案讀取失敗時顯示錯誤代碼；沒有 Artifact 的 Bundle 不顯示證據檔案", async () => {
    const client = fakeClient(verificationOverview({ bundles: [webEvidenceBundle(), evidenceBundle()] }));
    client.evidenceArtifact.mockRejectedValueOnce(
      Object.assign(new Error("找不到這個證據"), { code: "evidence_artifact_not_found" }),
    );
    const user = userEvent.setup();
    renderPage(client);

    const card = await screen.findByRole("region", { name: "Evidence Bundle vbun_web" });
    const shot = within(card).getByRole("listitem", { name: "截圖：open_orders.png" });
    await user.click(within(shot).getByRole("button", { name: "開啟截圖" }));
    expect(await within(shot).findByRole("alert")).toHaveTextContent(
      "無法讀取：找不到這個證據（evidence_artifact_not_found）",
    );
    expect(within(shot).queryByRole("img")).not.toBeInTheDocument();

    expect(within(region("Evidence Bundle vbun_1")).queryByRole("list", { name: "證據檔案" })).not.toBeInTheDocument();
  });

  it("讀取失敗時顯示原因", async () => {
    const client = fakeClient();
    client.verificationOverview.mockRejectedValueOnce(new Error("Runtime 沒有回應"));
    renderPage(client);

    expect(await screen.findByRole("alert")).toHaveTextContent("無法取得驗證資料：Runtime 沒有回應");
  });
});
