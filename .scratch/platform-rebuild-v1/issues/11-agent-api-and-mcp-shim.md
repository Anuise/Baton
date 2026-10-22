# 11: Agent API、範圍憑證與 MCP 工具 shim

Status: ready-for-agent
Blocked by: 10, 04

**What to build:** 依 spec D-05：Run 准入時簽發 Agent 範圍憑證（關聯 Workspace、Run、角色、世代），只經環境變數交給 MCP shim；shim 以 stdio 提供 MCP 工具並轉送到 Runtime 的 Agent API。本票實作共同工具：get_task_contract、report_progress、save_checkpoint、report_blocked。Runtime 每次請求以憑證核對身分與資源歸屬，不信任請求內填寫的 Workspace／Run。Run 結束或被接替後憑證失效。

**Blocked by:** 10 Supervisor、程序身分與 Worker Run 端到端（fake Adapter）；04 本機 API 身分驗證與網頁存取防護

**Spec:** [../spec.md](../spec.md)　**執行方式：** [../execution.md](../execution.md)

## Acceptance criteria

- [ ] Agent 存取其他 Workspace 或 Run 的資料被拒絕
- [ ] Agent 憑證不能呼叫桌面管理 API
- [ ] 過期世代的回報被保留為稽核但不改變目前狀態
- [ ] 憑證不出現在 Prompt、MCP 設定檔內容或日誌
- [ ] 以 fake Adapter 驗證 Worker 透過 shim 回報進度與 checkpoint 後可在 UI 看到
- [ ] 以主要 seam（Runtime harness，spec D-04）撰寫的情境測試涵蓋本票行為並通過；本票所做的 AI 決策已追加至 ai-decisions.md

## Comments
