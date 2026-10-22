# 22: 最小裁定中心與權限提示轉接

Status: in_progress（Work-8，run R029-22-impl，2026-10-22 16:10）
Blocked by: 11, 06

**What to build:** Agent 透過 request_decision 工具建立裁定請求；裁定分頁顯示待處理清單與單筆詳情（問題、原因、選項、建議、證據、阻擋範圍）；使用者經桌面憑證回答後，在同一交易保存 Decision、更新受影響 Task、追加喚醒 Outbox。依 spec D-08 實作 permission_prompt 工具：授權範圍內自動允許並記錄，範圍外建立裁定請求並回傳「等待裁定」。等待只阻擋受影響 Task。

**Blocked by:** 11 Agent API、範圍憑證與 MCP 工具 shim；06 Workspace 授權範本與確認（I2／P3）

**Spec:** [../spec.md](../spec.md)　**執行方式：** [../execution.md](../execution.md)

## Acceptance criteria

- [ ] Agent 憑證無法提交正式裁定
- [ ] 裁定後受影響 Task 自動恢復排程；無關 Task 從未被阻擋
- [ ] 逾時不視為同意
- [ ] 權限提示在授權內自動允許、範圍外產生裁定請求（以 fake Adapter 驗證）
- [ ] 以主要 seam（Runtime harness，spec D-04）撰寫的情境測試涵蓋本票行為並通過；本票所做的 AI 決策已追加至 ai-decisions.md

## Comments
