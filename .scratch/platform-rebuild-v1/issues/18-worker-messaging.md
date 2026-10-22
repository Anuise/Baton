# 18: Worker 間訊息

Status: in_progress（Work-3，run R028-18-impl，2026-10-22 16:10）
Blocked by: 11

**What to build:** 實作 messages 與 message_deliveries：send_message 與 read_inbox 工具（spec D-27），訊息含 message_id、thread_id、correlation_id、類型、寄收者與交付／讀取狀態；每個收件人以 message_id 去重；Run 啟動時附上未讀摘要。訊息不能改變 Task 歸屬或範圍。

**Blocked by:** 11 Agent API、範圍憑證與 MCP 工具 shim

**Spec:** [../spec.md](../spec.md)　**執行方式：** [../execution.md](../execution.md)

## Acceptance criteria

- [ ] 重複送出的訊息只交付一次
- [ ] Worker 不能讀取不屬於自己範圍的訊息
- [ ] 嘗試以訊息變更範圍時被忽略並轉為給 Controller 的事件
- [ ] 以主要 seam（Runtime harness，spec D-04）撰寫的情境測試涵蓋本票行為並通過；本票所做的 AI 決策已追加至 ai-decisions.md

## Comments
