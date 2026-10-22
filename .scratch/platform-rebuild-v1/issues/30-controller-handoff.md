# 30: Controller 跨帳號交接

Status: ready-for-agent
Blocked by: 28, 16

**What to build:** Controller 帳號接近上限或限流時，以同一交接協議切換到其他帳號：建立新 Controller Session、從持久化對話、決策與檢查點重建，conversation_id 不變，未處理指示依序去重交付。

**Blocked by:** 28 檢查點與主動跨帳號交接；16 Controller 長期對話與事件驅動

**Spec:** [../spec.md](../spec.md)　**執行方式：** [../execution.md](../execution.md)

## Acceptance criteria

- [ ] Controller 換帳號後對話連續、待處理訊息不遺失也不重複
- [ ] Controller 無可用帳號時事件被保存，UI 顯示等待原因，Runtime 其餘功能正常
- [ ] 以主要 seam（Runtime harness，spec D-04）撰寫的情境測試涵蓋本票行為並通過；本票所做的 AI 決策已追加至 ai-decisions.md

## Comments
