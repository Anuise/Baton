# 54: Controller 的共同工具

Status: in_progress（Work-8，run R163-54-impl，2026-10-23 05:53）
Blocked by: 17, 18, 37, 13

**What to build:** spec D-05 把 `search_knowledge`、`send_message`、`read_inbox`、`accept_artifact`（或對應的成果接受流程）與 `record_lesson_candidate` 列為 Controller 與 Worker 共同可用的 MCP 工具，但票 17 只讓 Controller 憑證呼叫 Controller 專用工具與 `permission_prompt`（AI-327）。最終 code-review 修正 run R155 判斷補齊這些工具需要新的 Domain／Store 模型（例如 Controller 沒有 Task 歸屬時的收件匣與成果接受範圍），留待另開票（AI-488）。本票補上 Controller 的共同工具：釐清每個工具在 Controller 身分下的範圍（Workspace 層級而非單一 Task），讓 Controller 憑證可以呼叫，並維持 Agent 不能取得超出範圍資料的保證。

**Blocked by:** 17 Controller 拆工與派工工具；18 Worker 間訊息；37 知識、教訓與全文檢索；13 成果接受與最小完成驗證（皆已完成）。

**Spec:** [../spec.md](../spec.md)　**執行方式：** [../execution.md](../execution.md)

## Acceptance criteria

- [ ] Controller 憑證可以呼叫 `search_knowledge`、`send_message`、`read_inbox` 與 Controller 版的 `record_lesson_candidate`，範圍限於其 Workspace
- [ ] Controller 可依 spec 接受成果（或明確記錄在 spec D-05 註記為何不提供，並更新 AI 決策）
- [ ] Controller 不能讀取或寫入其他 Workspace 的資料；Worker 的既有範圍限制不因此放寬
- [ ] 以 Runtime harness 撰寫情境測試涵蓋上述行為並通過；MCP shim 的工具清單與說明同步更新
- [ ] AI 決策已追加至 ai-decisions.md

## Comments

- 2026-10-23 主控：依最終 review 驗證 R160 的觀察與 AI-488 新增本票（AI-491）。
