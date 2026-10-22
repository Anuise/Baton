# 17: Controller 拆工與派工工具、漸進拆工

Status: ready-for-agent
Blocked by: 16, 09

**What to build:** 提供 Controller 專屬工具：propose_task、update_task、add_dependency、dispatch_task、cancel_task；Runtime 驗證契約、授權與循環後才接受。Controller 以 DR-20261022-02「邊釐清、邊調查」推進：不依賴未決答案的工作可派出，依賴答案者保持等待。Controller 不能直接寫資料庫或建立未受管理的 Worker。

**Blocked by:** 16 Controller 長期對話與事件驅動；09 Task、六項最小派工契約與工作面板

**Spec:** [../spec.md](../spec.md)　**執行方式：** [../execution.md](../execution.md)

## Acceptance criteria

- [ ] Controller 透過工具建立的 Task 與使用者手動建立的 Task 受相同准入檢查
- [ ] 超出 Workspace 授權的 Task 被拒絕並要求送裁定
- [ ] 依賴未回答問題的 Task 不會被派出，獨立的研究 Task 可派出（以 fake Adapter 的腳本化 Controller 驗證）
- [ ] 以主要 seam（Runtime harness，spec D-04）撰寫的情境測試涵蓋本票行為並通過；本票所做的 AI 決策已追加至 ai-decisions.md

## Comments
